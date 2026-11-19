//! Utate's ordinary699/702 address BUTTON input, not backlog/movie queries.
//! Independently recovered from the fixed PE and its sole main.kcs caller.

use cs2_runtime::{TaskCommand, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result};

use crate::{LoggingHost, native_state_error, read_u32};

const UTATE_SHA256: &str =
    "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52";

impl LoggingHost<'_> {
    pub(crate) fn bind_utate_plane_input_profile(&mut self) -> Result<bool> {
        if let Some(sha) = self.data_services.css_task_pe_sha256 {
            return Ok(sha == UTATE_SHA256);
        }
        // The scanner selects cs2conf.exe in this installed sample. The
        // existing identity check pins both the real runtime and effective
        // startup KCS; the configuration tool itself is never an ABI alias.
        if !self.verified_utate_sound_loop_source()? {
            return Ok(false);
        }
        self.data_services.css_task_pe_sha256 = Some(UTATE_SHA256);
        Ok(true)
    }

    /// Raw 69/70 adapter. The owned queue copies only these scalar DWORDs;
    /// its BUTTON callback must call receive_legacy_plane_input91 before the
    /// modern command91 receiver. Queuing does not mutate the region early.
    pub(crate) fn dispatch_legacy_plane_input91(
        &mut self,
        target: u32,
        packet: [u32; 4],
        queued: bool,
        output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        if packet[1] != 91 || !self.bind_utate_plane_input_profile()? {
            return Ok(None);
        }
        let kind = self.data_services.tasks.message_task_state(target).map(|state| state.0)
            .or_else(|| self.scene_nodes.get(&target).map(|node| node.kind));
        if kind != Some(6) {
            return Ok(None);
        }
        if !self.task_is_reachable(target) {
            return Ok(Some(TaskCommandOutcome { done: true, propagate: false }));
        }
        self.ensure_task_record(target)?;
        let command = TaskCommand {
            target: target as i32,
            sender: packet[0] as i32,
            code: 91,
            data: [packet[2], packet[3], 0],
        };
        if queued {
            self.install_device_input_receiver();
            self.queue_task_command_buffers(command, &Default::default(), output)?;
            return Ok(Some(TaskCommandOutcome { done: true, propagate: false }));
        }
        self.receive_legacy_plane_input91(command)
    }

    /// Call before modern backlog/movie dispatch: both old calls have two DWORDs.
    /// Other PEs remain with their existing ABI; malformed Utate calls fail.
    pub(crate) fn legacy_plane_input_native(
        &mut self,
        index: u32,
        args: &[u8],
    ) -> Result<Option<NativeReturn>> {
        if !matches!(index, 699 | 702) || !self.bind_utate_plane_input_profile()? {
            return Ok(None);
        }
        if args.len() != 8 {
            return Err(native_state_error("Utate native699/702 requires target and an input flag"));
        }
        let target = read_u32(args, 0)?;
        let command = TaskCommand {
            target: target as i32,
            sender: self.current_task_id as i32,
            code: if index == 699 { 90 } else { 91 },
            data: [0, read_u32(args, 4)?, 0],
        };
        // An absent address has no receiver. Zero is still addressed to the
        // root, rather than treated as an unconditional null-target early-out.
        if !self.task_is_reachable(target) {
            return Ok(Some(NativeReturn::Continue));
        }
        self.ensure_task_record(target)?;
        let normalized = if index == 699 { 136 } else { 154 };
        self.receive_utate_button_region_flag(command, normalized)?.ok_or_else(|| {
            native_state_error("Utate native699/702 target is outside the recovered BUTTON receiver")
        })?;
        // 45db16/45db46 return native state2 regardless of the message's reply.
        Ok(Some(NativeReturn::Continue))
    }

    /// Exact old BUTTON91 -> root input143 receiver. No caller output is
    /// written, and data[2] is forwarded as payload, never child traversal.
    /// This must precede the modern BUTTON91 unregister receiver if reused
    /// for raw messages. None leaves other object types explicitly unhandled.
    pub(crate) fn receive_legacy_plane_input91(
        &mut self,
        command: TaskCommand,
    ) -> Result<Option<TaskCommandOutcome>> {
        if command.code != 91 || !self.bind_utate_plane_input_profile()? {
            return Ok(None);
        }
        self.receive_utate_button_region_flag(command, 154)
    }

    fn receive_utate_button_region_flag(
        &mut self,
        command: TaskCommand,
        normalized_code: u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        let id = command.target as u32;
        let Some((kind, deletion, _)) = self.data_services.tasks.message_task_state(id) else {
            return Ok(None);
        };
        if kind != 6 {
            return Ok(None);
        }
        let done = TaskCommandOutcome { done: true, propagate: false };
        if deletion != 0 {
            return Ok(Some(done));
        }
        if !self.ensure_input_button(id)? {
            return Err(native_state_error("Utate BUTTON input state has no matching live task"));
        }
        // Old button+6c == -1 gates both operations (42e2e1/42e49c).
        if self.data_services.messages.input_buttons[&id].state.receiver.is_none()
            || !self.input_task.alive
        {
            return Ok(Some(done));
        }
        // Old root31 resolves the shared input service. BUTTON90 -> INPUT126
        // writes region+24 and always marks geometry dirty (43ab07/43ab0a),
        // equivalent to the real current command136/word9 setter. BUTTON91
        // -> INPUT143 writes +34 (43aef9), current command154/word14. The
        // latter is a movement flag, not the current word13 mask height.
        let normalized = TaskCommand {
            target: self.input_task.owner_id.unwrap_or(0) as i32,
            sender: id as i32,
            code: normalized_code,
            data: [id, command.data[1], command.data[2]],
        };
        let mut ignored_output = 0;
        if self.input_task.owner_id.is_none() {
            // The existing implicit-root input service owns these real regions
            // until an explicit type11 task is installed; use the same setter.
            self.receive_input_region_command(normalized, &mut Default::default(), &mut ignored_output)?;
        } else if self.data_services.tasks.message_task_state(normalized.target as u32)
            .is_some_and(|(kind, deleted, _)| kind == 11 && deleted == 0)
        {
            self.receive_device_input_buffers(normalized, &mut Default::default(), &mut ignored_output)?
                .ok_or_else(|| native_state_error("Utate button flag has no concrete input receiver"))?;
        }
        // A missing/deleted input service or region is a no-op in the old
        // synchronous message chain. No region/selection/mask/binding is freed.
        Ok(Some(done))
    }
}
