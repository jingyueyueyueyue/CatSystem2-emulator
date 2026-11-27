//! Owned temporary-buffer routing for platform producers and raw 69/70.
//! Receivers never retain VM addresses. Read/write flags distinguish an
//! actual query output from a missing-entry query that must not touch memory.
use std::collections::{BTreeMap, VecDeque};

use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{Result, Vm};

use super::{LoggingHost, NativeTaskStore};
use crate::native_state_error;

#[path = "fes_command_profile.rs"]
mod fes_command_profile;

const MAX_BUFFER: usize = 64 * 1024 * 1024;
const MAX_QUEUE_BYTES: usize = 256 * 1024 * 1024;
const MAX_PENDING: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct OwnedTaskBuffers {
    first: Option<Vec<u8>>,
    second: Option<Vec<u8>>,
    first_written: usize,
    second_written: usize,
}

impl OwnedTaskBuffers {
    pub(crate) fn new(data0: Option<Vec<u8>>, data1: Option<Vec<u8>>) -> Result<Self> {
        if data0.as_ref().is_some_and(|bytes| bytes.len() > MAX_BUFFER)
            || data1.as_ref().is_some_and(|bytes| bytes.len() > MAX_BUFFER)
        {
            return Err(native_state_error("task command temporary buffer exceeds 64 MiB"));
        }
        Ok(Self { first: data0, second: data1, first_written: 0, second_written: 0 })
    }

    pub(crate) fn data0(&self) -> Option<&[u8]> { self.first.as_deref() }
    pub(crate) fn data1(&self) -> Option<&[u8]> { self.second.as_deref() }
    pub(crate) fn data1_write_length(&self) -> usize { self.second_written }

    pub(crate) fn write_data0(&mut self, bytes: &[u8]) -> Result<()> {
        Self::write(&mut self.first, &mut self.first_written, bytes)
    }

    pub(crate) fn write_data1(&mut self, bytes: &[u8]) -> Result<()> {
        Self::write(&mut self.second, &mut self.second_written, bytes)
    }

    fn write(slot: &mut Option<Vec<u8>>, written: &mut usize, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() { return Ok(()); }
        let target = slot.as_mut().and_then(|target| target.get_mut(..bytes.len()))
            .ok_or_else(|| native_state_error("task command output buffer is absent or truncated"))?;
        target.copy_from_slice(bytes);
        *written = (*written).max(bytes.len());
        Ok(())
    }

    fn len(&self) -> usize {
        self.first.as_ref().map_or(0, Vec::len) + self.second.as_ref().map_or(0, Vec::len)
    }
}

pub(crate) type TaskBufferReceiver = for<'host> fn(
    &mut LoggingHost<'host>, TaskCommand, &mut OwnedTaskBuffers, &mut u32, usize,
) -> Result<Option<TaskCommandOutcome>>;

#[derive(Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct BufferedTaskCommands {
    #[serde(skip)]
    receivers: BTreeMap<u32, TaskBufferReceiver>,
    routes: BTreeMap<u32, BufferedRoute>,
    queued: BTreeMap<u32, VecDeque<BufferedPacket>>,
    queued_bytes: usize,
}

#[derive(cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct BufferedPacket {
    command: TaskCommand,
    buffers: OwnedTaskBuffers,
    output: u32,
}

#[derive(Clone, Copy, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum Phase { Receiver, Children(Option<i32>) }

#[derive(cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct BufferedRoute {
    command: TaskCommand,
    original: OwnedTaskBuffers,
    buffers: OwnedTaskBuffers,
    output: u32,
    found: bool,
    propagate: bool,
    stack: Vec<(u32, Phase)>,
}

pub(crate) struct PreparedBufferedCommands(BufferedTaskCommands);

impl NativeTaskStore {
    pub(crate) fn capture_buffered_command_state(&self) -> anyhow::Result<Vec<u8>> {
        Ok(cs2_runtime::object_graph::encode_component(&self.buffered_commands)?)
    }

    pub(crate) fn prepare_buffered_command_state(&self, bytes: &[u8]) -> anyhow::Result<PreparedBufferedCommands> {
        let mut state: BufferedTaskCommands = cs2_runtime::object_graph::decode_component(bytes)?;
        let mut queued_bytes = 0usize;
        if state.queued.len() > 1_000_000 || state.routes.len() > 1_000_000 {
            anyhow::bail!("owned command checkpoint has too many owners");
        }
        for (&id, queue) in &state.queued {
            if !self.records.contains_key(&id) || queue.len() > MAX_PENDING {
                anyhow::bail!("owned command checkpoint queue has an invalid owner or length");
            }
            for packet in queue {
                if packet.command.target as u32 != id || packet.buffers.len() > MAX_BUFFER * 2 {
                    anyhow::bail!("owned command checkpoint packet differs from its queue owner");
                }
                queued_bytes = queued_bytes.checked_add(packet.buffers.len())
                    .filter(|&size| size <= MAX_QUEUE_BYTES)
                    .ok_or_else(|| anyhow::anyhow!("owned command checkpoint exceeds queue byte bounds"))?;
                validate_checkpoint_buffers(&packet.buffers)?;
            }
        }
        if queued_bytes != state.queued_bytes { anyhow::bail!("owned command checkpoint byte accounting differs"); }
        for (&origin, route) in &state.routes {
            if !self.records.contains_key(&origin) || !self.records.contains_key(&(route.command.target as u32))
                || route.stack.len() > 256 || route.stack.iter().any(|(id, _)| !self.records.contains_key(id))
            { anyhow::bail!("owned command checkpoint traversal has an invalid task identity"); }
            validate_checkpoint_buffers(&route.original)?;
            validate_checkpoint_buffers(&route.buffers)?;
        }
        state.receivers = self.buffered_commands.receivers.clone();
        Ok(PreparedBufferedCommands(state))
    }

    pub(crate) fn commit_buffered_command_state(&mut self, prepared: PreparedBufferedCommands) {
        self.buffered_commands = prepared.0;
    }

    pub(super) fn graph_buffer_idle(&self) -> bool {
        self.buffered_commands.routes.is_empty() && self.buffered_commands.queued.is_empty()
            && self.buffered_commands.queued_bytes == 0
    }

    pub(super) fn preserve_graph_buffer_adapters(&mut self, old: &Self) {
        self.buffered_commands.receivers = old.buffered_commands.receivers.clone();
    }
    /// Install the actual object implementation before accepting its commands.
    /// None means unhandled without side effects; Some(done=false) resumes
    /// that exact receiver on the next route call.
    pub(crate) fn install_buffer_receiver(
        &mut self, kind: u32, receiver: TaskBufferReceiver,
    ) -> Option<TaskBufferReceiver> {
        self.buffered_commands.receivers.insert(kind, receiver)
    }
}

fn validate_checkpoint_buffers(value: &OwnedTaskBuffers) -> anyhow::Result<()> {
    if value.first.as_ref().is_some_and(|bytes| bytes.len() > MAX_BUFFER)
        || value.second.as_ref().is_some_and(|bytes| bytes.len() > MAX_BUFFER)
        || value.first_written > value.first.as_ref().map_or(0, Vec::len)
        || value.second_written > value.second.as_ref().map_or(0, Vec::len)
    { anyhow::bail!("owned command checkpoint contains an invalid read/write buffer"); }
    Ok(())
}

impl LoggingHost<'_> {
    pub(crate) fn pending_owned_task_buffers(&self, command: TaskCommand) -> Option<OwnedTaskBuffers> {
        self.data_services.tasks.buffered_commands.routes.get(&self.current_task_id)
            .filter(|route| route.command == command)
            .map(|route| route.original.clone())
    }

    /// Route explicit owned buffers with signed-id child order. target/sender
    /// remain real task identities. Missing targets return None; budget yield
    /// returns Some(done=false) and retains buffers/output/child cursors.
    /// Without data[2] traversal, preserve the addressed receiver's actual
    /// propagation reply for the caller's raw-message traversal.
    pub(crate) fn route_task_command_buffers(
        &mut self, command: TaskCommand, buffers: &mut OwnedTaskBuffers,
        output: &mut u32, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let origin = self.current_task_id;
        let mut route = match self.data_services.tasks.buffered_commands.routes.remove(&origin) {
            Some(route) => {
                if route.command != command || (route.original != *buffers && route.buffers != *buffers) {
                    self.data_services.tasks.buffered_commands.routes.insert(origin, route);
                    return Err(native_state_error("buffered task route continuation changed its input"));
                }
                route
            }
            None => {
                if !self.task_is_reachable(command.target as u32) { return Ok(None); }
                BufferedRoute {
                    command, original: buffers.clone(), buffers: buffers.clone(),
                    output: *output, found: true, propagate: false,
                    stack: vec![(command.target as u32, Phase::Receiver)],
                }
            }
        };
        let result = (|| {
            for _ in 0..budget {
                let Some((id, phase)) = route.stack.last().copied() else { return Ok(true); };
                if route.stack.len() > 256 {
                    return Err(native_state_error("buffered task hierarchy exceeds 256 levels"));
                }
                match phase {
                    Phase::Receiver => {
                        let addressed = TaskCommand { target: id as i32, ..command };
                        let outcome = self.receive_owned_task_command(
                            addressed, &mut route.buffers, &mut route.output, budget,
                        )?;
                        if command.data[2] == 0 && id == command.target as u32 {
                            route.propagate = outcome.is_some_and(|outcome| outcome.done && outcome.propagate);
                        }
                        match outcome {
                            Some(outcome) if !outcome.done => return Ok(false),
                            Some(outcome) if outcome.propagate && command.data[2] != 0 => {
                                route.stack.last_mut().unwrap().1 = Phase::Children(None);
                            }
                            _ => { route.stack.pop(); }
                        }
                    }
                    Phase::Children(cursor) => {
                        if let Some(child) = self.next_task_message_child(id, cursor) {
                            route.stack.last_mut().unwrap().1 = Phase::Children(Some(child as i32));
                            route.stack.push((child, Phase::Receiver));
                        } else {
                            route.stack.pop();
                        }
                    }
                }
            }
            Ok(route.stack.is_empty())
        })();
        *output = route.output;
        *buffers = route.buffers.clone();
        let found = route.found;
        let propagate = route.propagate;
        if !matches!(result, Ok(true)) {
            self.data_services.tasks.buffered_commands.routes.insert(origin, route);
        }
        result.map(|done| found.then_some(TaskCommandOutcome { done, propagate: done && propagate }))
    }

    fn receive_owned_task_command(
        &mut self, command: TaskCommand, buffers: &mut OwnedTaskBuffers,
        output: &mut u32, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let id = command.target as u32;
        if !self.task_is_reachable(id) { return Ok(None); }
        self.ensure_task_record(id)?;
        // Retained synchronous root packets must be acknowledged even after
        // their own handler marks the root/sender deleted. The owned route
        // otherwise stops before reaching the typed receiver's ack branch.
        if command.code == 173 && super::ordinary_kind(self.data_services.tasks.records[&id].kind) {
            if let Some(request) = self.data_services.tasks.root_dispatch.filter(|request| request.target == id) {
                let bytes = buffers.data1().and_then(|bytes| bytes.get(..16))
                    .ok_or_else(|| native_state_error("retained root packet requires 16 owned data1 bytes"))?;
                let mut packet = [0; 4];
                for (word, bytes) in packet.iter_mut().zip(bytes.chunks_exact(4)) {
                    *word = u32::from_le_bytes(bytes.try_into().unwrap());
                }
                if packet == request.message {
                    let done = self.dispatch_root_message_request(id, &mut packet, output)?;
                    if done { buffers.write_data1(&packet.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>())?; }
                    return Ok(Some(TaskCommandOutcome { done, propagate: false }));
                }
            }
            if self.data_services.tasks.records[&id].message_owner == Some(self.current_task_id) {
                let bytes = buffers.data1().and_then(|bytes| bytes.get(..16))
                    .ok_or_else(|| native_state_error("retained KCS packet requires 16 owned data1 bytes"))?;
                let mut packet = [0; 4];
                for (word, bytes) in packet.iter_mut().zip(bytes.chunks_exact(4)) {
                    *word = u32::from_le_bytes(bytes.try_into().unwrap());
                }
                let done = self.dispatch_ordinary_message(id, &mut packet, output, budget)?;
                if done { buffers.write_data1(&packet.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>())?; }
                return Ok(Some(TaskCommandOutcome { done, propagate: false }));
            }
        }
        if self.data_services.tasks.records[&id].deletion != 0 {
            return Ok(Some(TaskCommandOutcome { done: true, propagate: false }));
        }
        let kind = self.data_services.tasks.records[&id].kind;
        if let Some(outcome) = self.receive_owned_movie_plane_query(command, buffers, output)? {
            return Ok(Some(outcome));
        }
        if kind == 2 && crate::plane_native::plane_command_supported(command.code)
            && !matches!(command.code, 61 | 62 | 75 | 76)
            && !self.movie_host.as_ref().is_some_and(|host| host.plane_target_handle(id).is_some())
        {
            let consumed = self.dispatch_plane_command(command,
                &mut crate::plane_native::PlaneCommandBuffers {
                    data0: None, data1: buffers.second.as_deref_mut(),
                }, output)?;
            if consumed {
                return Ok(Some(TaskCommandOutcome { done: true, propagate: false }));
            }
        }
        if let Some(outcome) = self.receive_owned_css_model_query(command, buffers, output)? {
            return Ok(Some(outcome));
        }
        if let Some(outcome) = self.receive_owned_model_query(command, buffers)? {
            return Ok(Some(outcome));
        }
        if command.code == 354 && self.legacy_particle_canvas(kind)
            && self.data_services.particle_external.canvases.get(&id).is_some_and(|canvas| canvas.configured)
        {
            // This receiver writes its four-byte sentinel before resolving
            // the path. Preserve that write even for equal bytes or errors.
            let mut color = u32::MAX.to_le_bytes();
            buffers.write_data1(&color)?;
            let result = self.receive_particle_canvas_command(command,
                &mut TaskCommandBuffers { data0: buffers.first.as_deref(), data1: Some(&mut color) },
                output);
            buffers.write_data1(&color)?;
            return result;
        }
        if let Some(receiver) = self.data_services.tasks.buffered_commands.receivers.get(&kind).copied() {
            if let Some(outcome) = receiver(self, command, buffers, output, budget)? {
                return Ok(Some(outcome));
            }
        } else if kind == 15 && list_command(command.code) {
            return Err(native_state_error("type-15 owned command receiver is not installed"));
        }
        // Existing typed receivers use a readonly first and mutable second
        // buffer. Keep successful mutations even when a later service fails.
        let before = buffers.second.clone();
        let result = self.receive_task_command(
            command,
            &mut TaskCommandBuffers {
                data0: buffers.first.as_deref(), data1: buffers.second.as_deref_mut(),
            },
            output, budget,
        );
        if buffers.second != before {
            buffers.second_written = buffers.second.as_ref().map_or(0, Vec::len);
        }
        // Equal-byte outputs still count as writes. In particular a missing
        // named value must really zero its destination, even though the
        // scratch output started as zero and no caller memory was read.
        if result.is_ok() {
            if kind == 34 && self.data_services.tasks.fes.context_initialized(id) {
                if command.code == 357 {
                    buffers.second_written = buffers.second_written.max(4);
                } else if command.code == 349 && command.data[0] >= 2048
                    && buffers.second.is_some() && *output == 2048
                {
                    buffers.second_written = buffers.second_written.max(2048);
                }
            }
            if command.code == 25 && command.data[1] != 0
                && (super::ordinary_kind(kind) || kind == 34)
            {
                if let Some(context) = self.data_services.tasks.task_context(id) {
                    buffers.second_written = buffers.second_written.max(context.len() + 1);
                }
            }
        }
        result
    }

    /// An action such as BUTTON 96 or input 156 needs its concrete receiver.
    /// A propagating unknown-command base response is never a query result.
    pub(crate) fn dispatch_registered_task_buffers(
        &mut self, command: TaskCommand, buffers: &mut OwnedTaskBuffers,
        output: &mut u32, budget: usize,
    ) -> Result<TaskCommandOutcome> {
        let id = command.target as u32;
        if !self.task_is_reachable(id) {
            return Err(native_state_error("required task action target is unreachable"));
        }
        self.ensure_task_record(id)?;
        let kind = self.data_services.tasks.records[&id].kind;
        let receiver = self.data_services.tasks.buffered_commands.receivers.get(&kind).copied()
            .ok_or_else(|| native_state_error("required task action has no concrete buffer receiver"))?;
        receiver(self, command, buffers, output, budget)?
            .filter(|outcome| !outcome.propagate)
            .ok_or_else(|| native_state_error("required task action was not consumed by its receiver"))
    }

    /// Copy a command now. Body update consumes its owned buffers later;
    /// output buffers cannot refer to a VM whose lifetime ends before then.
    pub(crate) fn queue_task_command_buffers(
        &mut self, command: TaskCommand, buffers: &OwnedTaskBuffers, output: &mut u32,
    ) -> Result<bool> {
        let id = command.target as u32;
        if !self.task_is_reachable(id) { return Ok(false); }
        self.ensure_task_record(id)?;
        if self.data_services.tasks.records[&id].deletion != 0 { return Ok(true); }
        let kind = self.data_services.tasks.records[&id].kind;
        if kind == 15 && list_command(command.code)
            && !self.data_services.tasks.buffered_commands.receivers.contains_key(&15)
        {
            return Err(native_state_error("type-15 owned queue receiver is not installed"));
        }
        let store = &mut self.data_services.tasks.buffered_commands;
        if store.queued_bytes.checked_add(buffers.len()).is_none_or(|size| size > MAX_QUEUE_BYTES) {
            return Err(native_state_error("task command owned queue exceeds 256 MiB"));
        }
        let queue = store.queued.entry(id).or_default();
        if queue.len() >= MAX_PENDING {
            return Err(native_state_error("task command owned queue exceeds 4096 packets"));
        }
        queue.push_back(BufferedPacket { command, buffers: buffers.clone(), output: 0 });
        store.queued_bytes += buffers.len();
        *output = 1;
        Ok(true)
    }

    pub(crate) fn update_task_command_buffers(&mut self, id: u32, budget: usize) -> Result<bool> {
        let clock = self.data_services.tasks.task_clock(id).unwrap_or_default();
        let saved = (self.current_task_id, self.task_time, self.task_origin);
        self.current_task_id = id;
        self.task_time = clock.current;
        self.task_origin = clock.origin;
        let result = (|| {
            for _ in 0..budget {
                let packet = self.data_services.tasks.buffered_commands.queued.get_mut(&id)
                    .and_then(VecDeque::pop_front);
                let Some(mut packet) = packet else { return Ok(true); };
                let result = self.route_task_command_buffers(
                    packet.command, &mut packet.buffers, &mut packet.output, budget,
                );
                if matches!(&result, Ok(None) | Ok(Some(TaskCommandOutcome { done: true, .. }))) {
                    self.data_services.tasks.buffered_commands.queued_bytes -= packet.buffers.len();
                } else {
                    self.data_services.tasks.buffered_commands.queued.entry(id).or_default()
                        .push_front(packet);
                }
                if result?.is_some_and(|outcome| !outcome.done) { return Ok(false); }
            }
            Ok(self.data_services.tasks.buffered_commands.queued.get(&id)
                .is_none_or(VecDeque::is_empty))
        })();
        (self.current_task_id, self.task_time, self.task_origin) = saved;
        result
    }

    pub(crate) fn release_task_command_buffers(&mut self, id: u32) {
        let store = &mut self.data_services.tasks.buffered_commands;
        if let Some(queue) = store.queued.remove(&id) {
            store.queued_bytes -= queue.iter().map(|packet| packet.buffers.len()).sum::<usize>();
        }
        store.routes.remove(&id);
    }

    pub(crate) fn dispatch_buffered_list_message(
        &mut self, target: u32, packet: [u32; 4], queued: bool,
        output: &mut u32, vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        let command = TaskCommand {
            target: target as i32, sender: packet[0] as i32, code: packet[1],
            data: [packet[2], packet[3], 0],
        };
        if !list_command(command.code) { return Ok(None); }
        let mut raw = self.snapshot_list_command(command, vm.as_deref(), queued)?;
        if queued {
            let found = self.queue_task_command_buffers(command, &raw.buffers, output)?;
            return Ok(found.then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        let result = self.route_task_command_buffers(command, &mut raw.buffers, output, 4096);
        if raw.buffers.first_written != 0 || raw.buffers.second_written != 0 {
            let vm = vm.ok_or_else(|| native_state_error("list query output requires its caller VM"))?;
            if let Some(address) = raw.write0 {
                if raw.buffers.first_written != 0 {
                    vm.write(address, &raw.buffers.first.as_ref().unwrap()[..raw.buffers.first_written])?;
                }
            }
            if let Some(address) = raw.write1 {
                if raw.buffers.second_written != 0 {
                    vm.write(address, &raw.buffers.second.as_ref().unwrap()[..raw.buffers.second_written])?;
                }
            }
        }
        result
    }

    pub(crate) fn dispatch_buffered_fes_message(
        &mut self, target: u32, packet: [u32; 4], queued: bool,
        output: &mut u32, mut vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        let code = if matches!(packet[1], 19 | 25 | 26) {
            packet[1]
        } else {
            // Leave unrelated base messages to their actual receiver. Raw
            // object commands must never fall back to canonical numbering:
            // V401 338/339 mean save/restore, not Yukikoi load/start.
            if packet[1] < 256 { return Ok(None); }
            let digest = self.data_services.native_profile.map(|profile| profile.pe_sha256)
                .or(self.data_services.css_task_pe_sha256).unwrap_or("");
            let controls = self.data_services.ordinary_native_table.as_ref()
                .map(|table| table.fes_control_wrappers())
                .or_else(|| self.data_services.startup_companion_natives.as_ref()
                    .map(|table| table.fes_control_wrappers))
                .unwrap_or_default();
            let Some(code) = fes_command_profile::canonical_command(packet[1], controls, digest) else {
                return Ok(None);
            };
            code
        };
        let command = TaskCommand {
            target: target as i32, sender: packet[0] as i32, code,
            data: [packet[2], packet[3], 0],
        };
        let initialized = self.data_services.tasks.fes.context_initialized(target);
        let output_len = if code == 25 && packet[3] != 0 {
            self.data_services.tasks.task_context(target).map(|context| context.len() + 1)
        } else if initialized {
            match code {
                349 if packet[3] != 0 => Some((packet[2] as usize).min(2048)),
                357 => Some(4),
                _ => None,
            }
        } else { None };
        if queued && output_len.is_some() {
            return Err(native_state_error("queued FES query cannot retain a VM output address"));
        }
        let mut buffers = match self.pending_owned_task_buffers(command) {
            Some(buffers) => buffers,
            None => {
                let first = if initialized {
                    match code {
                        343 if packet[2] != 0 => Some(vm.as_deref()
                            .ok_or_else(|| native_state_error("raw FES object context requires a VM"))?
                            .read(packet[2], 36)?),
                        357 | 358 => Some(vm.as_deref()
                            .ok_or_else(|| native_state_error("raw FES name requires a VM"))?
                            .read_string(packet[2])?),
                        _ => None,
                    }
                } else { None };
                let second = if let Some(length) = output_len {
                    Some(vec![0; length])
                } else if code == 26 && packet[3] != 0 {
                    // Base task context exists before FES command337.
                    let mut bytes = vm.as_deref()
                        .ok_or_else(|| native_state_error("raw task context requires a VM"))?
                        .read_string(packet[3])?;
                    bytes.push(0);
                    Some(bytes)
                } else if initialized {
                    match code {
                        338 | 341 | 343 => {
                            let mut bytes = vm.as_deref()
                                .ok_or_else(|| native_state_error("raw FES text buffer requires a VM"))?
                                .read_string(packet[3])?;
                            bytes.push(0);
                            Some(bytes)
                        }
                        350 if packet[2] != 0 => Some(vm.as_deref()
                            .ok_or_else(|| native_state_error("raw FES restore buffer requires a VM"))?
                            .read(packet[3], (packet[2] as usize).min(2048))?),
                        _ => None,
                    }
                } else { None };
                OwnedTaskBuffers::new(first, second)?
            }
        };
        if queued {
            let found = self.queue_task_command_buffers(command, &buffers, output)?;
            return Ok(found.then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        let result = self.route_task_command_buffers(command, &mut buffers, output, 4096);
        if buffers.second_written != 0 {
            vm.as_deref_mut()
                .ok_or_else(|| native_state_error("raw FES query requires its caller VM"))?
                .write(packet[3], &buffers.second.as_ref().unwrap()[..buffers.second_written])?;
        }
        result
    }

    fn snapshot_list_command(
        &self, command: TaskCommand, vm: Option<&Vm>, queued: bool,
    ) -> Result<RawListCommand> {
        let code = command.code;
        let mut raw = RawListCommand { buffers: OwnedTaskBuffers::default(), write0: None, write1: None };
        let output0 = code == 290;
        let output1 = matches!(code, 290 | 291 | 304 | 307 | 308 | 313 | 315);
        if queued && (output0 || output1) {
            return Err(native_state_error("queued list query cannot retain a caller VM output address"));
        }
        if output0 {
            raw.buffers.first = Some(vec![0; 4]);
            raw.write0 = Some(command.data[0]);
        }
        if output1 {
            raw.buffers.second = Some(vec![0; 4]);
            raw.write1 = Some(command.data[1]);
        }
        if let Some(original) = self.pending_owned_task_buffers(command) {
            raw.buffers = original;
            return Ok(raw);
        }
        // A missing list does not dereference registration-record pointers.
        let initialized = self.data_services.tasks.fes.platform.lists
            .contains_key(&(command.target as u32));
        let input0 = (code == 302 && command.data[1] != 0 && command.data[0] != 0).then_some(16);
        let input1 = match code {
            286 | 288 if initialized => Some(60),
            295 if command.data[1] != 0 && crate::list_native::has_entry(self, command) => Some(16),
            316 if initialized && command.data[1] != 0 => Some(16),
            318 if crate::list_native::has_animation(self, command) => Some(8),
            _ => None,
        };
        if code == 299 {
            let mut bytes = vm.ok_or_else(|| native_state_error("list ANM filename needs a VM or owned bytes"))?
                .read_string(command.data[1])?;
            bytes.push(0);
            raw.buffers.second = Some(bytes);
        }
        if let Some(size) = input0 {
            raw.buffers.first = Some(vm.ok_or_else(||
                native_state_error("raw list command requires a VM; use owned buffers from FES"))?
                .read(command.data[0], size)?);
        }
        if let Some(size) = input1 {
            raw.buffers.second = Some(vm.ok_or_else(||
                native_state_error("raw list command requires a VM; use owned buffers from FES"))?
                .read(command.data[1], size)?);
        }
        Ok(raw)
    }
}

struct RawListCommand {
    buffers: OwnedTaskBuffers,
    write0: Option<u32>,
    write1: Option<u32>,
}

fn list_command(code: u32) -> bool { code == 19 || (285..=319).contains(&code) }
