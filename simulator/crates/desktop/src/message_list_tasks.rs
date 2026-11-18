//! Type-47 message inspector: CST source ownership, bounded refresh, actual
//! desktop viewer, and typed portable UI requests. No original window code.
use std::collections::BTreeMap;

use cs2_runtime::message_list::{MessageListRow, MessageListScan};
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{CstDocument, NativeReturn, Result, Vm};

use crate::{HeadlessPlayer, LoggingHost, ScriptTextEncoding, native_state_error, read_u32};

#[path = "message_list_windows.rs"]
pub(super) mod platform;

#[derive(Clone, Debug)]
pub struct MessageListView {
    pub task: u32,
    pub visible: bool,
    pub rows: Vec<MessageListRow>,
    pub encoding: ScriptTextEncoding,
}

#[derive(Default)]
pub(crate) struct MessageListStore {
    tasks: BTreeMap<u32, MessageListTask>,
    revision: u64,
    base: Option<u32>,
}

struct MessageListTask {
    source: u32,
    source_snapshot: Option<(CstDocument, ScriptTextEncoding)>,
    open: bool,
    visible: bool,
    rows: Vec<MessageListRow>,
    scan: Option<MessageListScan>,
    refresh_requested: bool,
    selection: Option<u32>,
    next_selection: Option<u32>,
    viewer: Option<platform::Viewer>,
    encoding: ScriptTextEncoding,
}

impl MessageListStore {
    pub(crate) fn supports(index: u32) -> bool { matches!(index, 570..=572) }
    pub(crate) fn identify(&mut self, sha256: &str) {
        self.base = message_base_for_pe(sha256);
    }

    pub(super) fn register(&mut self, id: u32) {
        self.tasks.insert(id, MessageListTask {
            source: 0, source_snapshot: None, open: false, visible: false, rows: Vec::new(), scan: None,
            refresh_requested: false, selection: None, next_selection: None, viewer: None,
            encoding: ScriptTextEncoding::Cp932,
        });
        self.changed();
    }

    fn changed(&mut self) { self.revision = self.revision.wrapping_add(1); }

    /// Portable shells display these actual rows in their own list widget.
    /// Reading a view neither starts the task nor acknowledges an input.
    pub(crate) fn view(&self, id: u32) -> Option<MessageListView> {
        let task = self.tasks.get(&id)?;
        Some(MessageListView {
            task: id, visible: task.visible, rows: task.rows.clone(), encoding: task.encoding,
        })
    }
}

impl HeadlessPlayer {
    pub fn message_list_revision(&self) -> u64 {
        self.data_services.tasks.message_lists.revision
    }

    pub fn message_list_views(&self) -> Vec<MessageListView> {
        self.data_services.tasks.message_lists.tasks.keys().copied()
            .filter_map(|id| self.data_services.tasks.message_lists.view(id))
            .collect()
    }

    pub fn message_list_view(&self, id: u32) -> Option<MessageListView> {
        self.data_services.tasks.message_lists.view(id)
    }

    pub fn select_message_list_input(&mut self, id: u32, input: u32) -> anyhow::Result<()> {
        let task = self.data_services.tasks.message_lists.tasks.get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("message-list task is absent"))?;
        if !task.visible || !task.rows.iter().any(|row| row.input == input) {
            anyhow::bail!("message-list selection is not a displayed source input");
        }
        if task.selection.is_some() { anyhow::bail!("finish the pending message-list selection first"); }
        task.selection = Some(input);
        Ok(())
    }

    pub fn set_message_list_visible(&mut self, id: u32, visible: bool) -> anyhow::Result<()> {
        let task = self.data_services.tasks.message_lists.tasks.get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("message-list task is absent"))?;
        let changed = task.visible != visible;
        task.visible = visible;
        if let Some(viewer) = task.viewer.as_mut() { viewer.visible(visible)?; }
        if changed { self.data_services.tasks.message_lists.changed(); }
        Ok(())
    }
}

impl LoggingHost<'_> {
    pub(crate) fn message_list_native(
        &mut self, index: u32, args: &[u8],
    ) -> Result<NativeReturn> {
        let target = read_u32(args, 0)?;
        let source = if index == 571 { read_u32(args, 4)? } else { 0 };
        // The original wrapper unwraps the opaque object before routing,
        // including when the target is missing. This CST object is not a VM
        // address, an image wrapper, or a copied struct from caller memory.
        if source != 0 && !self.cst_contexts.contains_key(&source) {
            return Err(native_state_error("native 571 source is not a CST context"));
        }
        let base = message_base(self).ok_or_else(||
            native_state_error("message-list command ABI needs an identified PE"))?;
        let code = base + match index { 570 => 0, 571 => 3, 572 => 2, _ => unreachable!() };
        let outcome = self.receive_task_command(
            TaskCommand { target: target as i32, sender: self.current_task_id as i32,
                code, data: [0, source, 0] },
            &mut TaskCommandBuffers::default(), &mut 0, 4096,
        )?;
        Ok(if outcome.is_some_and(|outcome| !outcome.done) {
            NativeReturn::Retry
        } else { NativeReturn::Continue })
    }

    pub(crate) fn receive_message_list_command(
        &mut self, command: TaskCommand, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let Some(base) = message_base(self) else { return Ok(None); };
        let Some(operation) = command.code.checked_sub(base).filter(|code| *code <= 3) else {
            return Ok(None);
        };
        let id = command.target as u32;
        let mut task = self.data_services.tasks.message_lists.tasks.remove(&id)
            .ok_or_else(|| native_state_error("type47 message-list service is not registered"))?;
        let mut changed = false;
        let result = (|| -> Result<bool> {
            match operation {
                0 => {
                    if budget == 0 { return Ok(false); }
                    if !task.open {
                        #[cfg(windows)]
                        { task.viewer = Some(platform::Viewer::open()
                            .map_err(|cause| self.message_list_error(id, cause))?); }
                        // Android and other shells use the public typed view
                        // and selection API; no HWND or host pointer escapes.
                        task.open = true;
                        task.visible = true;
                        changed = true;
                    }
                    if let Some(viewer) = task.viewer.as_mut() {
                        return viewer.ready().map_err(|cause| self.message_list_error(id, cause));
                    }
                }
                1 => {
                    changed |= task.visible != (command.data[1] != 0);
                    task.visible = command.data[1] != 0;
                    if let Some(viewer) = task.viewer.as_mut() {
                        viewer.visible(task.visible).map_err(|cause| self.message_list_error(id, cause))?;
                    }
                }
                2 => {
                    if task.open {
                        task.scan = None;
                        task.refresh_requested = true;
                    }
                }
                3 => {
                    let source = command.data[1];
                    let snapshot = if source == 0 { None } else {
                        let context = self.cst_contexts.get(&source)
                            .ok_or_else(|| native_state_error("message-list binding is not a CST context"))?;
                        context.document.clone().map(|document| (document, context.encoding))
                    };
                    task.source = source;
                    // The UI worker owns the bound source bytes, so releasing
                    // a caller's CST wrapper cannot leave a dangling pointer.
                    task.source_snapshot = snapshot;
                }
                _ => unreachable!(),
            }
            Ok(true)
        })();
        self.data_services.tasks.message_lists.tasks.insert(id, task);
        if changed { self.data_services.tasks.message_lists.changed(); }
        result.map(|done| Some(TaskCommandOutcome { done, propagate: false }))
    }

    pub(super) fn message_list_parent_selection(
        &mut self, command: TaskCommand, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let Some(base) = message_base(self) else { return Ok(None); };
        if command.code != base + 4
            || self.data_services.tasks.message_task_state(command.sender as u32)
                .is_none_or(|(kind, deleted, _)| kind != 47 || deleted != 0)
        {
            return Ok(None);
        }
        let done = self.send_fes_task_message(command.target as u32,
            [command.sender as u32, 0xffff_0015, 0, command.data[1]])?;
        let _ = budget; // message dispatch owns its explicit internal slice.
        Ok(Some(TaskCommandOutcome { done, propagate: false }))
    }

    pub(super) fn update_message_list_task(&mut self, id: u32, budget: usize) -> Result<bool> {
        let Some(mut task) = self.data_services.tasks.message_lists.tasks.remove(&id) else {
            return Ok(true);
        };
        let mut changed = false;
        let result = (|| -> Result<bool> {
            if let Some(viewer) = task.viewer.as_mut() {
                for event in viewer.events().map_err(|cause| self.message_list_error(id, cause))? {
                    match event {
                        platform::Event::Selected(input) => {
                            if task.selection.is_some() { task.next_selection = Some(input); }
                            else { task.selection = Some(input); }
                        }
                        platform::Event::Hidden => {
                            changed |= task.visible;
                            task.visible = false;
                        }
                        platform::Event::Failure(cause) => {
                            return Err(self.message_list_error(id, anyhow::anyhow!(cause)));
                        }
                    }
                }
            }
            if task.refresh_requested && task.scan.is_none() {
                let document = if task.source == 0 { CstDocument { inputs: Vec::new(), records: Vec::new() } }
                else {
                    if let Some(source) = self.cst_contexts.get(&task.source) {
                        task.source_snapshot = source.document.clone().map(|document| (document, source.encoding));
                    }
                    if let Some((document, encoding)) = &task.source_snapshot {
                        changed |= task.encoding != *encoding;
                        task.encoding = *encoding;
                        document.clone()
                    } else { CstDocument { inputs: Vec::new(), records: Vec::new() } }
                };
                changed |= !task.rows.is_empty();
                task.rows.clear();
                task.scan = Some(MessageListScan::new(document)
                    .map_err(|cause| self.task_error(id, cause))?);
            }
            if let Some(scan) = task.scan.as_mut() {
                if scan.advance(budget.min(64)).map_err(|cause| self.task_error(id, cause))? {
                    changed |= task.rows.as_slice() != scan.rows();
                    task.rows = scan.rows().to_vec();
                    if let Some(viewer) = task.viewer.as_mut() {
                        viewer.replace(&task.rows, task.encoding)
                            .map_err(|cause| self.message_list_error(id, cause))?;
                    }
                    task.scan = None;
                    task.refresh_requested = false;
                }
            }
            if let Some(input) = task.selection {
                let parent = self.task_parents.get(&id).copied()
                    .ok_or_else(|| native_state_error("message-list selection has no parent receiver"))?;
                let base = message_base(self).ok_or_else(||
                    native_state_error("message-list parent protocol needs an identified PE"))?;
                let packet = [id, 0xffff_0015, 0, input];
                // A receiver can mark itself deleted before returning from
                // this notification. Finish its original root request before
                // a fresh-entry deletion check consumes the selection and
                // strands the receipt ahead of the next notification.
                let retained_root = self.data_services.tasks.root_dispatch.is_some_and(|request|
                    request.target == parent && request.message == packet);
                if retained_root || self.data_services.tasks.message_task_state(parent)
                    .is_some_and(|(kind, deleted, _)| matches!(kind, 0 | 12) && deleted == 0)
                {
                    // V401 command441 / Yukikoi452 become the ordinary VM's
                    // ffff0015 notification. This is unrelated to native441.
                    if !self.send_fes_task_message(parent, packet)? {
                        return Ok(false);
                    }
                    task.selection = task.next_selection.take();
                    return Ok(true);
                }
                let outcome = self.receive_task_command(
                    TaskCommand { target: parent as i32, sender: id as i32,
                        code: base + 4, data: [0, input, 0] },
                    &mut TaskCommandBuffers::default(), &mut 0, budget,
                )?;
                match outcome {
                    Some(outcome) if !outcome.done => return Ok(false),
                    Some(outcome) if !outcome.propagate => task.selection = task.next_selection.take(),
                    _ => return Err(native_state_error("message-list parent selection receiver is not recovered")),
                }
            }
            Ok(true)
        })();
        self.data_services.tasks.message_lists.tasks.insert(id, task);
        if changed { self.data_services.tasks.message_lists.changed(); }
        result
    }

    pub(super) fn release_message_list_task(&mut self, id: u32) -> Result<bool> {
        let Some(task) = self.data_services.tasks.message_lists.tasks.get_mut(&id) else { return Ok(true); };
        if let Some(viewer) = task.viewer.as_mut() {
            if !viewer.close().map_err(|_| native_state_error("message-list viewer release failed"))? {
                return Ok(false);
            }
        }
        self.data_services.tasks.message_lists.tasks.remove(&id);
        self.data_services.tasks.message_lists.changed();
        Ok(true)
    }

    pub(super) fn dispatch_message_list_packet(
        &mut self, id: u32, packet: [u32; 4], queued: bool, output: &mut u32, _vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        let Some(base) = message_base(self) else { return Ok(None); };
        if !(base..=base+3).contains(&packet[1]) { return Ok(None); }
        let command = TaskCommand {
            target: id as i32, sender: packet[0] as i32, code: packet[1],
            data: [packet[2], packet[3], 0],
        };
        if queued {
            let found = self.queue_task_command_buffers(command, &Default::default(), output)?;
            return Ok(found.then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        self.receive_message_list_command(command, 4096)
    }

    fn message_list_error(&mut self, id: u32, cause: anyhow::Error) -> cs2_script::ScriptError {
        self.task_error(id, cs2_runtime::RuntimeError::Host(format!("message list: {cause:#}")))
    }
}

fn message_base(host: &LoggingHost<'_>) -> Option<u32> {
    host.data_services.tasks.message_lists.base
}

fn message_base_for_pe(sha256: &str) -> Option<u32> {
    let profile = cs2_runtime::native_task_profile::exact_task_native_profile(sha256)?;
    let [create, bind, refresh] = profile.message_list?;
    // Refuse a future incompatible layout; wrapper identities alone do not
    // license assigning arbitrary operations to intervening message numbers.
    (bind == create + 3 && refresh == create + 2).then_some(create)
}
