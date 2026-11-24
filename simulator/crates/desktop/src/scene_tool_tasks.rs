//! Type32/33 debugger and scene-list receivers over owned records and real
//! task services. Windows UI is shared with the existing inspector backend.
use std::collections::{BTreeMap, VecDeque};

use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_runtime::message_list::MessageListRow;
use cs2_runtime::scene_tools::{SceneDebugRecord, SceneListEntry, SceneSourceRecord,
    SceneSourceTable, PendingSceneString, SceneStringLease,
    SceneSelectionContinuation as SceneSelection, parse_scene_list};
use cs2_script::{NativeReturn, Result, Vm};

use super::message_list_tasks::platform;
use super::{LoggingHost, TaskMessage};
use crate::{HeadlessPlayer, ScriptTextEncoding, expand_kcs_format, native_state_error, read_u32};

const MAX_BYTES: usize = 64 * 1024 * 1024;

#[path = "scene_message_memory.rs"]
mod scene_message_memory;
pub(crate) use scene_message_memory::PreparedSceneMessageMemory;

#[derive(Clone, Debug)]
pub struct SceneToolView {
    pub task: u32,
    pub kind: u32,
    pub visible: bool,
    pub records: Vec<SceneDebugRecord>,
    pub log: Vec<Vec<u8>>,
    pub variables: Vec<u32>,
    pub stack_offset: u32,
    pub scenes: Vec<SceneListEntry>,
    pub encoding: ScriptTextEncoding,
}

#[derive(Default)]
pub(crate) struct SceneToolStore {
    tasks: BTreeMap<u32, Tool>,
    strings: BTreeMap<u32, VecDeque<PendingSceneString>>,
    selections: BTreeMap<u32, SceneSelection>,
    vm_sources: BTreeMap<u32, SceneSourceTable>,
    vm_generations: BTreeMap<u32, u64>,
    next_vm_generation: u64,
    string_leases: BTreeMap<u32, BTreeMap<u32, SceneStringLease>>,
    leased_bytes: usize,
    revision: u64,
}

struct Tool {
    kind: u32,
    source: u32,
    open: bool,
    visible: bool,
    records: Vec<SceneDebugRecord>,
    log: Vec<Vec<u8>>,
    log_bytes: usize,
    variables: Vec<u32>,
    stack_offset: u32,
    scenes: Vec<SceneListEntry>,
    loaded: bool,
    selected: Option<u32>,
    viewer: Option<platform::Viewer>,
    dirty: bool,
    encoding: ScriptTextEncoding,
}

impl SceneToolStore {
    pub(super) fn register(&mut self, id: u32, kind: u32, source: u32, encoding: ScriptTextEncoding) {
        self.tasks.insert(id, Tool {
            kind, source, open: false, visible: false, records: Vec::new(), log: Vec::new(),
            log_bytes: 0, variables: Vec::new(), stack_offset: 0, scenes: Vec::new(),
            loaded: false, selected: None, viewer: None, dirty: true, encoding,
        });
        self.changed();
    }
    fn changed(&mut self) { self.revision = self.revision.wrapping_add(1); }
    pub(super) fn has_strings(&self, id: u32) -> bool {
        self.strings.get(&id).is_some_and(|queue| !queue.is_empty())
    }
    pub(super) fn release_vm(&mut self, id: u32) {
        if self.vm_sources.remove(&id).is_some() { self.changed(); }
        self.vm_generations.remove(&id);
        if let Some(leases) = self.string_leases.remove(&id) {
            self.leased_bytes -= leases.values().map(|lease| lease.length as usize).sum::<usize>();
        }
        if let Some(strings) = self.strings.get_mut(&id) {
            for string in strings { string.address = None; }
        }
    }

    pub(super) fn flush_strings(
        &mut self, id: u32, words: &mut VecDeque<TaskMessage>, vm: &mut Vm,
    ) -> Result<()> {
        if !self.has_strings(id) { return Ok(()); }
        let generation = match self.vm_generations.get(&id).copied() {
            Some(generation) => generation,
            None => {
                self.next_vm_generation = self.next_vm_generation.checked_add(1)
                    .ok_or_else(|| native_state_error("scene-message VM generation exhausted"))?;
                self.vm_generations.insert(id, self.next_vm_generation);
                self.next_vm_generation
            }
        };
        let strings = self.strings.get_mut(&id).unwrap();
        if strings.iter().any(|string| string.before > words.len()) {
            return Err(native_state_error("owned scene-message queue lost its original ordering"));
        }
        for string in strings.iter_mut() {
            if let Some(address) = string.address {
                let lease = self.string_leases.get(&id).and_then(|leases| leases.get(&address))
                    .filter(|lease| lease.generation == generation
                        && lease.length as usize == string.bytes.len()+2)
                    .ok_or_else(|| native_state_error("scene string retry lost its owning VM allocation lease"))?;
                // A script may modify delivered bytes. Content equality
                // cannot establish VM identity and must never trigger a
                // second allocation or silently bind another VM's address.
                vm.read(address, lease.length as usize)?;
            } else {
                let length = string.bytes.len()+2;
                let total = self.leased_bytes.checked_add(length)
                    .filter(|&total| total <= MAX_BYTES)
                    .ok_or_else(|| native_state_error("scene-string leases exceed64 MiB"))?;
                if self.string_leases.get(&id).is_some_and(|leases| leases.len() >= 1_000_000) {
                    return Err(native_state_error("scene-string leases exceed one million allocations"));
                }
                let mut bytes = string.bytes.clone();
                bytes.extend_from_slice(&[0; 2]);
                let address = vm.allocate_region(&bytes)?;
                self.string_leases.entry(id).or_default().insert(address, SceneStringLease {
                    owner: id, generation, base: address, length: length as u32,
                });
                self.leased_bytes = total;
                string.address = Some(address);
            }
        }
        // Positions count only the ordinary words which preceded the owning
        // string at enqueue time. No drain is allowed before this conversion.
        let mut result = VecDeque::with_capacity(words.len()+strings.len());
        let mut index = 0;
        loop {
            while strings.front().is_some_and(|string| string.before <= index) {
                let string = strings.pop_front().unwrap();
                let mut packet = string.packet;
                packet[3] = string.address.unwrap();
                result.push_back(packet);
            }
            let Some(packet) = words.pop_front() else { break; };
            result.push_back(packet);
            index += 1;
        }
        debug_assert!(strings.is_empty());
        *words = result;
        self.strings.remove(&id);
        Ok(())
    }

    fn view(&self, id: u32) -> Option<SceneToolView> {
        let tool = self.tasks.get(&id)?;
        Some(SceneToolView {
            task: id, kind: tool.kind, visible: tool.visible, records: tool.records.clone(),
            log: tool.log.clone(), variables: tool.variables.clone(), stack_offset: tool.stack_offset,
            scenes: tool.scenes.clone(), encoding: tool.encoding,
        })
    }
}

impl HeadlessPlayer {
    pub fn scene_tool_revision(&self) -> u64 { self.data_services.tasks.scene_tools.revision }
    pub fn scene_tool_views(&self) -> Vec<SceneToolView> {
        let store = &self.data_services.tasks.scene_tools;
        store.tasks.keys().filter_map(|&id| store.view(id)).collect()
    }
    pub fn scene_tool_source_records(&self, task: u32) -> &[SceneSourceRecord] {
        self.data_services.tasks.scene_tools.vm_sources.get(&task)
            .map_or(&[], SceneSourceTable::records)
    }
    pub fn set_scene_tool_visible(&mut self, id: u32, visible: bool) -> anyhow::Result<()> {
        let store = &mut self.data_services.tasks.scene_tools;
        let tool = store.tasks.get_mut(&id).ok_or_else(|| anyhow::anyhow!("scene tool is absent"))?;
        if let Some(viewer) = tool.viewer.as_mut() { viewer.visible(visible)?; }
        if tool.visible != visible { tool.visible = visible; store.changed(); }
        Ok(())
    }
    pub fn select_scene_list_entry(&mut self, id: u32, line: u32) -> anyhow::Result<()> {
        let tool = self.data_services.tasks.scene_tools.tasks.get_mut(&id)
            .filter(|tool| tool.kind == 33 && tool.visible)
            .ok_or_else(|| anyhow::anyhow!("scene list is absent or hidden"))?;
        if !tool.scenes.iter().any(|entry| entry.line == line) {
            anyhow::bail!("scene selection is not an original CSV record");
        }
        if tool.selected.is_some() { anyhow::bail!("finish the pending scene selection first"); }
        tool.selected = Some(line);
        Ok(())
    }

    pub fn set_scene_debug_variable(&mut self, id: u32, index: u32, value: u32) -> anyhow::Result<()> {
        let source = self.data_services.tasks.scene_tools.tasks.get(&id)
            .filter(|tool| tool.kind == 32 && tool.open)
            .ok_or_else(|| anyhow::anyhow!("scene debugger is not open"))?.source;
        self.with_host(|host| {
            let values = host.shared_values_for_mut(source)
                .ok_or_else(|| native_state_error("scene debugger has no bound shared numeric service"))?;
            let slot = values.get_mut(index as usize)
                .ok_or_else(|| native_state_error("scene debugger variable is outside its actual numeric area"))?;
            *slot = value;
            Ok(())
        })
    }
}

impl LoggingHost<'_> {
    /// Root integration: allocations happen against the actual root VM before
    /// exposing its four-DWORD messages. No temporary/fake VM address is used.
    pub(crate) fn drain_root_task_messages_with_vm(
        &mut self, root: u32, vm: &mut Vm,
    ) -> Result<Vec<TaskMessage>> {
        let store = &mut self.data_services.tasks;
        if let Some(record) = store.records.get_mut(&root) {
            store.scene_tools.flush_strings(root, &mut record.queued, vm)?;
        }
        Ok(self.drain_root_task_messages(root))
    }

    pub(super) fn prepare_scene_tool_strings(&mut self, id: u32) -> Result<()> {
        let store = &mut self.data_services.tasks;
        if !store.scene_tools.has_strings(id) { return Ok(()); }
        let record = store.records.get_mut(&id)
            .ok_or_else(|| native_state_error("scene message target record disappeared"))?;
        let vm = self.task_vms.get_mut(&id)
            .or_else(|| record.ordinary.as_mut().and_then(|task| task.vm.as_mut()))
            .ok_or_else(|| native_state_error("scene message needs its actual suspended VM"))?;
        store.scene_tools.flush_strings(id, &mut record.queued, vm)
    }

    pub(crate) fn scene_tool_native(
        &mut self, index: u32, args: &[u8], vm: &Vm,
    ) -> Result<NativeReturn> {
        let target = read_u32(args, 0)?;
        let profile = task_profile(self)?;
        let (code, second) = match index {
            279 => (profile.scene_debug.ok_or_else(no_profile)?[0], None),
            280 => (profile.scene_debug.ok_or_else(no_profile)?[1], None),
            281 => (profile.scene_debug.ok_or_else(no_profile)?[2],
                Some(vm.read(read_u32(args, 4)?, 0x160)?)),
            282 => {
                let bytes = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
                if bytes.len() >= 128 { return Err(native_state_error("debug log exceeds127 encoded bytes")); }
                let mut bytes = bytes; bytes.push(0);
                (profile.scene_debug.ok_or_else(no_profile)?[3], Some(bytes))
            }
            283 => (profile.scene_debug.ok_or_else(no_profile)?[4], None),
            287 => (profile.scene_list.ok_or_else(no_profile)?[0], None),
            288 => (profile.scene_list.ok_or_else(no_profile)?[1], None),
            _ => return Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        };
        let value = if matches!(index, 280 | 288) { read_u32(args, 4)? } else { 0 };
        let mut second = second;
        let outcome = self.receive_task_command(
            TaskCommand { target: target as i32, sender: self.current_task_id as i32, code,
                data: [0, value, 0] },
            &mut TaskCommandBuffers { data0: None, data1: second.as_deref_mut() },
            &mut 0, 4096,
        )?;
        if outcome.is_some_and(|outcome| !outcome.done) { return Ok(NativeReturn::Retry); }
        if index == 281 {
            let offset = vm.program().map(|program|
                (vm.stack_pointer() as u32).wrapping_sub(program.data.len() as u32))
                .ok_or(cs2_script::ScriptError::NoProgram)?;
            let code = profile.scene_debug_refresh.ok_or_else(no_profile)?;
            self.receive_task_command(
                TaskCommand { target: target as i32, sender: self.current_task_id as i32,
                    code, data: [0, offset, 0] },
                &mut TaskCommandBuffers::default(), &mut 0, 4096,
            )?;
        }
        Ok(NativeReturn::Continue)
    }

    pub(crate) fn receive_scene_tool_command(
        &mut self, command: TaskCommand, buffers: &mut TaskCommandBuffers<'_>, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let profile = task_profile(self)?;
        let id = command.target as u32;
        let kind = self.data_services.tasks.message_task_state(id).map(|state| state.0);
        let operation = match kind {
            Some(32) => profile.scene_debug.and_then(|codes| codes.iter()
                .position(|&code| code == command.code).map(|index| index as u8))
                .or_else(|| (profile.scene_debug_refresh == Some(command.code)).then_some(5)),
            Some(33) => profile.scene_list.and_then(|codes| codes.iter()
                .position(|&code| code == command.code).map(|index| index as u8)),
            _ => return Ok(None),
        };
        let Some(operation) = operation else { return Ok(None); };
        let mut tool = self.data_services.tasks.scene_tools.tasks.remove(&id)
            .ok_or_else(|| native_state_error("scene-tool receiver was not registered"))?;
        let mut changed = false;
        let result = (|| -> Result<bool> {
            match operation {
                0 => {
                    if budget == 0 { return Ok(false); }
                    if !tool.open {
                        #[cfg(windows)]
                        { tool.viewer = Some(platform::Viewer::open_with_title(
                            if tool.kind == 32 { "Scene debugger" } else { "Scene list" },
                        ).map_err(|cause| self.tool_error(id, cause))?); }
                        tool.open = true; tool.visible = true; tool.dirty = true; changed = true;
                    }
                    if let Some(viewer) = tool.viewer.as_mut() {
                        return viewer.ready().map_err(|cause| self.tool_error(id, cause));
                    }
                }
                1 => {
                    let visible = command.data[1] != 0;
                    if let Some(viewer) = tool.viewer.as_mut() {
                        viewer.visible(visible).map_err(|cause| self.tool_error(id, cause))?;
                    }
                    changed |= tool.visible != visible; tool.visible = visible;
                    tool.dirty |= visible;
                }
                2 if tool.open => {
                    let record = SceneDebugRecord::parse(buffers.data1.as_deref()
                        .ok_or_else(|| native_state_error("scene-debug object record buffer is absent"))?)
                        .map_err(|cause| self.task_error(id, cause))?;
                    if let Some(old) = tool.records.iter_mut().find(|old| old.label == record.label) {
                        changed |= *old != record; *old = record;
                    } else {
                        if tool.records.len() >= MAX_BYTES/0x160 {
                            return Err(native_state_error("scene-debug object records exceed64 MiB"));
                        }
                        tool.records.push(record); changed = true;
                    }
                    tool.dirty |= changed;
                }
                3 if tool.open => {
                    let bytes = buffers.data1.as_deref()
                        .ok_or_else(|| native_state_error("scene-debug log buffer is absent"))?;
                    let end = bytes.iter().position(|&byte| byte == 0)
                        .ok_or_else(|| native_state_error("scene-debug log is not terminated"))?;
                    if tool.log.len() >= 1_000_000 || tool.log_bytes.saturating_add(end+1) > MAX_BYTES {
                        return Err(native_state_error("scene-debug log exceeds64 MiB"));
                    }
                    tool.log.push(bytes[..end].to_vec()); tool.log_bytes += end+1;
                    changed = true; tool.dirty = true;
                }
                4 if tool.open => {
                    changed |= !tool.log.is_empty(); tool.log.clear(); tool.log_bytes = 0;
                    tool.dirty |= changed;
                }
                5 if tool.open => {
                    changed |= tool.stack_offset != command.data[1];
                    tool.stack_offset = command.data[1]; tool.dirty |= changed;
                }
                _ => {}
            }
            Ok(true)
        })();
        self.data_services.tasks.scene_tools.tasks.insert(id, tool);
        if changed { self.data_services.tasks.scene_tools.changed(); }
        result.map(|done| Some(TaskCommandOutcome { done, propagate: false }))
    }

    pub(super) fn dispatch_scene_tool_packet(
        &mut self, target: u32, packet: TaskMessage, queued: bool,
        output: &mut u32, vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        let profile = task_profile(self)?;
        let kind = self.data_services.tasks.message_task_state(target).map(|state| state.0);
        let operation = match kind {
            Some(32) => profile.scene_debug.and_then(|codes| codes.iter()
                .position(|&code| code == packet[1]).map(|index| index as u8))
                .or_else(|| (profile.scene_debug_refresh == Some(packet[1])).then_some(5)),
            Some(33) => profile.scene_list.and_then(|codes| codes.iter()
                .position(|&code| code == packet[1]).map(|index| index as u8)),
            _ => return Ok(None),
        };
        let Some(operation) = operation else { return Ok(None); };
        let command = TaskCommand { target: target as i32, sender: packet[0] as i32,
            code: packet[1], data: [packet[2], packet[3], 0] };
        let mut buffers = match self.pending_owned_task_buffers(command) {
            Some(buffers) => buffers,
            None => {
                let open = self.data_services.tasks.scene_tools.tasks.get(&target)
                    .is_some_and(|tool| tool.open);
                let second = match (kind, operation, open) {
                    (Some(32), 2, true) => Some(vm.as_deref()
                        .ok_or_else(|| native_state_error("scene-debug record needs sender VM or owned bytes"))?
                        .read(packet[3], 0x160)?),
                    (Some(32), 3, true) => {
                        let mut bytes = vm.as_deref()
                            .ok_or_else(|| native_state_error("scene-debug log needs sender VM or owned bytes"))?
                            .read_string(packet[3])?;
                        bytes.push(0); Some(bytes)
                    }
                    _ => None,
                };
                super::OwnedTaskBuffers::new(None, second)?
            }
        };
        if queued {
            let found = self.queue_task_command_buffers(command, &buffers, output)?;
            return Ok(found.then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        self.route_task_command_buffers(command, &mut buffers, output, 4096)
    }

    pub(crate) fn receive_scene_selection_command(
        &mut self, command: TaskCommand, buffers: &mut TaskCommandBuffers<'_>,
        output: &mut u32, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let profile = match self.data_services.task_native_profile()
        { Some(profile) => profile, None => return Ok(None) };
        let Some(codes) = profile.scene_list else { return Ok(None); };
        if command.code != codes[0]+2 { return Ok(None); }
        let target = command.target as u32;
        let has_program = self.task_vms.contains_key(&target)
            || self.data_services.tasks.records.get(&target).is_some_and(|record| record.had_vm
                || record.ordinary.as_ref().is_some_and(|task| task.vm.is_some()));
        if !has_program {
            *output = 1;
            return Ok(Some(TaskCommandOutcome { done: true, propagate: false }));
        }
        let bytes = buffers.data1.as_deref()
            .ok_or_else(|| native_state_error("script scene-selection requires owned name bytes"))?;
        let end = bytes.iter().position(|&byte| byte == 0)
            .ok_or_else(|| native_state_error("script scene-selection name is not terminated"))?;
        let done = self.queue_scene_selection(
            target, command.sender as u32, command.data[0], &bytes[..end], budget,
        )?;
        *output = 1;
        Ok(Some(TaskCommandOutcome { done, propagate: false }))
    }

    pub(super) fn dispatch_scene_selection_packet(
        &mut self, target: u32, packet: TaskMessage, queued: bool,
        output: &mut u32, vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        let profile = match self.data_services.task_native_profile()
        { Some(profile) => profile, None => return Ok(None) };
        if profile.scene_list.is_none_or(|codes| packet[1] != codes[0]+2) { return Ok(None); }
        let command = TaskCommand { target: target as i32, sender: packet[0] as i32,
            code: packet[1], data: [packet[2], packet[3], 0] };
        let mut buffers = match self.pending_owned_task_buffers(command) {
            Some(buffers) => buffers,
            None => {
                let mut bytes = vm.as_deref()
                    .ok_or_else(|| native_state_error("raw scene-selection requires its actual sender VM"))?
                    .read_string(packet[3])?;
                bytes.push(0);
                super::OwnedTaskBuffers::new(None, Some(bytes))?
            }
        };
        if queued {
            let found = self.queue_task_command_buffers(command, &buffers, output)?;
            return Ok(found.then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        self.route_task_command_buffers(command, &mut buffers, output, 4096)
    }

    pub(super) fn update_scene_tool_task(&mut self, id: u32, budget: usize) -> Result<bool> {
        let Some(mut tool) = self.data_services.tasks.scene_tools.tasks.remove(&id) else { return Ok(true); };
        let mut changed = false;
        let result = (|| -> Result<bool> {
            if !tool.open { return Ok(true); }
            if tool.kind == 33 && !tool.loaded {
                let entries = match self.resources.find("scenelist.csv") {
                    Some(resource) => {
                        let bytes = self.resources.read_candidate(resource)
                            .map_err(|cause| self.task_error(id, cause))?;
                        parse_scene_list(&bytes).map_err(|cause| self.task_error(id, cause))?
                    }
                    None => Vec::new(),
                };
                tool.scenes = entries; tool.loaded = true; tool.dirty = true; changed = true;
            }
            if tool.kind == 32 {
                if let Some(values) = self.shared_values_for(tool.source)
                    .filter(|values| tool.variables.as_slice() != *values)
                {
                    tool.variables = values.to_vec(); tool.dirty = true; changed = true;
                }
            }
            if let Some(viewer) = tool.viewer.as_mut() {
                for event in viewer.events().map_err(|cause| self.tool_error(id, cause))? {
                    match event {
                        platform::Event::Hidden => { changed |= tool.visible; tool.visible = false; }
                        platform::Event::Selected(line) if tool.kind == 33 => tool.selected = Some(line),
                        platform::Event::Selected(_) => {}
                        platform::Event::Failure(cause) => return Err(self.tool_error(id, anyhow::anyhow!(cause))),
                    }
                }
            }
            if tool.dirty {
                let rows = tool_rows(&tool);
                if let Some(viewer) = tool.viewer.as_mut() {
                    viewer.replace(&rows, tool.encoding).map_err(|cause| self.tool_error(id, cause))?;
                }
                tool.dirty = false;
            }
            if let Some(line) = tool.selected {
                let entry = tool.scenes.iter().find(|entry| entry.line == line)
                    .ok_or_else(|| native_state_error("scene-list source record disappeared"))?;
                let name = if entry.scene.is_empty() { &entry.group } else { &entry.scene };
                let parent = self.task_parents.get(&id).copied()
                    .ok_or_else(|| native_state_error("scene list has no parent receiver"))?;
                if !entry.scene.is_empty() {
                    self.register_scene_list_source(parent, entry.value, &entry.group)?;
                }
                if !self.queue_scene_selection(parent, id, 0, name, budget)? { return Ok(false); }
                tool.selected = None;
            }
            Ok(true)
        })();
        self.data_services.tasks.scene_tools.tasks.insert(id, tool);
        if changed { self.data_services.tasks.scene_tools.changed(); }
        result
    }

    pub(super) fn release_scene_tool_task(&mut self, id: u32) -> Result<bool> {
        let store = &mut self.data_services.tasks.scene_tools;
        if let Some(tool) = store.tasks.get_mut(&id) {
            if let Some(viewer) = tool.viewer.as_mut() {
                if !viewer.close().map_err(|_| native_state_error("scene-tool UI release failed"))? {
                    return Ok(false);
                }
            }
        }
        if store.tasks.remove(&id).is_some() { store.changed(); }
        Ok(true)
    }

    /// The release cursor may suspend after closing the tool UI. Keep VM
    /// allocations and their ledger until the task's actual ownership drop.
    pub(super) fn release_scene_tool_memory(&mut self, id: u32) {
        let store = &mut self.data_services.tasks.scene_tools;
        store.strings.remove(&id);
        store.selections.remove(&id);
        store.release_vm(id);
    }

    fn register_scene_list_source(&mut self, target: u32, key: u32, name: &[u8]) -> Result<()> {
        // The original queries175, and skips this one side effect when that
        // query returned no interpreter. A real suspended/external VM owns
        // this table for its complete lifetime, independently of the UI.
        let has_vm = self.task_vms.contains_key(&target)
            || self.data_services.tasks.records.get(&target).is_some_and(|record| record.had_vm
                || record.ordinary.as_ref().is_some_and(|task| task.vm.is_some()));
        if !has_vm { return Ok(()); }
        let grows = self.data_services.tasks.scene_tools.vm_sources.get(&target)
            .is_none_or(|table| !table.records().iter().any(|record| record.key == key));
        if grows { self.data_services.tasks.scene_tools.check_memory_growth(68, 1)?; }
        let table = self.data_services.tasks.scene_tools.vm_sources.entry(target).or_default();
        if table.insert(key, name).map_err(|cause| self.task_error(target, cause))? {
            self.data_services.tasks.scene_tools.changed();
        }
        Ok(())
    }

    pub(super) fn queue_scene_selection(
        &mut self, target: u32, sender: u32, data0: u32, name: &[u8], budget: usize,
    ) -> Result<bool> {
        if name.len() > 31 || name.contains(&0) {
            return Err(native_state_error("scene selection exceeds31 encoded context bytes"));
        }
        if !self.task_is_reachable(target) { return Ok(true); }
        self.ensure_task_record(target)?;
        let record = &self.data_services.tasks.records[&target];
        if record.deletion != 0 { return Ok(true); }
        if !super::ordinary_kind(record.kind) {
            return Err(native_state_error("scene-list parent requires its actual script receiver"));
        }
        if !record.had_vm && !self.task_vms.contains_key(&target)
            && record.ordinary.as_ref().is_none_or(|task| task.vm.is_none())
        {
            return Ok(true);
        }
        if record.active && !self.data_services.tasks.external_tasks.contains(&target) {
            return Err(native_state_error("scene selection needs the active ordinary VM's owned-string adapter"));
        }
        let mut pending = match self.data_services.tasks.scene_tools.selections.remove(&target) {
            Some(pending) => {
                if pending.sender != sender || pending.data0 != data0 || pending.name != name {
                    self.data_services.tasks.scene_tools.selections.insert(target, pending);
                    return Err(native_state_error("scene-selection continuation changed its packet"));
                }
                pending
            }
            None => {
                let mut nodes = Vec::new();
                let mut stack = vec![(target, 0usize)];
                while let Some((id, depth)) = stack.pop() {
                    if depth >= 256 || nodes.len() >= 1_000_000 {
                        return Err(native_state_error("scene-context broadcast exceeds task-tree bounds"));
                    }
                    nodes.push(id);
                    let mut children = Vec::new();
                    let mut cursor = None;
                    while let Some(child) = self.next_task_message_child(id, cursor) {
                        children.push(child); cursor = Some(child as i32);
                    }
                    stack.extend(children.into_iter().rev().map(|child| (child, depth+1)));
                }
                SceneSelection { sender, data0, name: name.to_vec(), nodes, cursor: 0, enqueued: false }
            }
        };
        let result = (|| -> Result<bool> {
            let new_string = usize::from(!pending.enqueued);
            self.data_services.tasks.scene_tools.check_memory_growth(
                pending.nodes.len().saturating_mul(4).saturating_add(pending.name.len())
                    .saturating_add(new_string * (pending.name.len() + 2)),
                pending.nodes.len().saturating_add(new_string),
            )?;
            if !pending.enqueued {
                let store = &mut self.data_services.tasks;
                let before = store.records[&target].queued.len();
                let strings = store.scene_tools.strings.entry(target).or_default();
                if strings.len() >= 4096 {
                    return Err(native_state_error("owned scene-message queue exceeds4096 entries"));
                }
                strings.push_back(PendingSceneString {
                    before, packet: [sender, 0xffff_0010, data0, 0],
                    bytes: name.to_vec(), address: None,
                });
                pending.enqueued = true;
            }
            // Command11 forces traversal after receiver consumption. Sender
            // is the script object which accepted336, not the UI child.
            for _ in 0..budget {
                let Some(&id) = pending.nodes.get(pending.cursor) else { return Ok(true); };
                let mut bytes = pending.name.clone(); bytes.push(0);
                self.receive_task_command(
                    TaskCommand { target: id as i32, sender: target as i32, code: 26, data: [0, 1, 0] },
                    &mut TaskCommandBuffers { data0: None, data1: Some(&mut bytes) },
                    &mut 0, budget,
                )?;
                pending.cursor += 1;
            }
            Ok(pending.cursor == pending.nodes.len())
        })();
        if !matches!(result, Ok(true)) {
            self.data_services.tasks.scene_tools.selections.insert(target, pending);
        }
        result
    }

    fn tool_error(&mut self, id: u32, cause: anyhow::Error) -> cs2_script::ScriptError {
        self.task_error(id, cs2_runtime::RuntimeError::Host(format!("scene tool: {cause:#}")))
    }
}

fn task_profile(host: &LoggingHost<'_>) -> Result<cs2_runtime::native_task_profile::TaskNativeProfile> {
    host.data_services.task_native_profile().ok_or_else(no_profile)
}
fn no_profile() -> cs2_script::ScriptError {
    native_state_error("scene-tool protocol needs its exact registered PE descriptor")
}

fn tool_rows(tool: &Tool) -> Vec<MessageListRow> {
    if tool.kind == 33 {
        return tool.scenes.iter().map(|entry| {
            let mut message = entry.title.clone(); message.extend_from_slice(b" | ");
            message.extend_from_slice(&entry.scene);
            MessageListRow { input: entry.line, name: entry.group.clone(), message }
        }).collect();
    }
    let mut rows = Vec::new();
    for (index, record) in tool.records.iter().enumerate() {
        let mut message = record.name.clone();
        for offset in (0x108..0x160).step_by(4) {
            let word = record.word(offset).unwrap();
            message.extend_from_slice(format!(" [{offset:03x}]={word:08x}").as_bytes());
        }
        rows.push(MessageListRow { input: index as u32, name: record.label.clone(), message });
    }
    for (index, value) in tool.variables.iter().enumerate() {
        rows.push(MessageListRow { input: 1_000_000+index as u32, name: b"variable".to_vec(),
            message: format!("{index}: {} (0x{value:08x})", *value as i32).into_bytes() });
    }
    for (index, message) in tool.log.iter().enumerate() {
        rows.push(MessageListRow { input: 2_000_000+index as u32, name: b"log".to_vec(),
            message: message.clone() });
    }
    rows
}
