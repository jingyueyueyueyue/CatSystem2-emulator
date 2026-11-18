//! NEKOPARA type54 player placement, separate from its final full-screen quad.
//! PE c71c5b85: 2385ff/238695/2386e9 -> 1743e0/174420/174450.
use super::EmoteNativeServices;
use crate::{LoggingHost, native_state_error, read_u32};
use crate::native_tasks::OwnedTaskBuffers;
use cs2_runtime::{TaskCommand, TaskCommandOutcome};
use cs2_runtime::emote::{CpuDriverTransform, EmoteError};
use cs2_script::{NativeReturn, Result, Vm};

#[derive(Clone, Copy)]
pub(super) struct Placement {
    pub profile_position: [f32; 2],
    pub profile_scale: f32,
    pub scale_offset: [f32; 2],
    position: [f32; 2],
    position_linked: bool,
    scale: [f32; 2],
    rotation: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self { profile_position: [0.0; 2], profile_scale: 1.0,
            scale_offset: [0.0; 2], position: [0.0; 2], position_linked: false, scale: [1.0; 2], rotation: 0.0 }
    }
}

impl Placement {
    pub(super) fn driver_transform(&self) -> std::result::Result<CpuDriverTransform, EmoteError> {
        // SSE arithmetic is single precision before crossing into the CPU
        // evaluator. Driver kind1 uses only c4/b4; c8/b8 are retained by the
        // original setters but do not enter these driver calls.
        let compensation = 1.0_f32 - self.scale[0];
        let x = (self.position[0] + self.profile_position[0])
            + self.scale_offset[0] * compensation;
        let y = (self.position[1] + self.profile_position[1])
            + self.scale_offset[1] * compensation;
        let scale = self.profile_scale * self.scale[0];
        CpuDriverTransform::new([x, y], scale, self.rotation)
    }
}

impl EmoteNativeServices {
    pub(super) fn apply_placement(&mut self, task: u32) -> std::result::Result<(), EmoteError> {
        let placement = self.placements.get(&task).ok_or(EmoteError::UnknownTask(task))?;
        self.state.set_m2_force_position(task, placement.position, placement.position_linked)?;
        self.state.set_driver_transform(task, placement.driver_transform()?)
    }
}

impl LoggingHost<'_> {
    pub(crate) fn emote_geometry_native(
        &mut self, index: u32, arguments: &[u8],
    ) -> Result<Option<NativeReturn>> {
        if !matches!(index, 84 | 93 | 101) || arguments.len() < 4 { return Ok(None); }
        let target = read_u32(arguments, 0)?;
        if !self.scene_nodes.get(&target).is_some_and(|node| node.kind == 54) {
            return Ok(None);
        }
        if arguments.len() < 20 {
            return Err(native_state_error("M2 geometry wrapper needs five DWORDs"));
        }
        let command = TaskCommand { target: target as i32, sender: self.current_task_id as i32,
            code: match index { 84 => 51, 93 => 55, _ => 66 }, data: [read_u32(arguments, 16)?, 0, 0] };
        let mut buffers = OwnedTaskBuffers::new(None, Some(arguments[4..16].to_vec()))?;
        let mut output = 0;
        if self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?
            .is_some_and(|outcome| !outcome.done) {
            return Ok(Some(NativeReturn::Retry));
        }
        Ok(Some(NativeReturn::Continue))
    }

    /// These commands place the actual driver player, never its final quad or
    /// task depth. Position/scale use XY; rotation uses only the third float.
    pub(crate) fn receive_emote_geometry(
        &mut self, command: TaskCommand, payload: Option<&[u8]>,
    ) -> Result<Option<TaskCommandOutcome>> {
        if !matches!(command.code, 51 | 55 | 66) { return Ok(None); }
        let id = command.target as u32;
        if !self.data_services.emote.state.state(id)
            .is_some_and(|state| state.successful_resource_imports != 0) {
            // 2393a0 consumes without reading the caller's data1.
            return Ok(Some(TaskCommandOutcome { done: true, propagate: false }));
        }
        let store = &mut self.data_services.emote;
        let placement = store.placements.get_mut(&id)
            .ok_or_else(|| native_state_error("M2 player has no owned placement state"))?;
        if command.code == 55 {
            let bytes = payload.and_then(|bytes| bytes.get(8..12))
                .ok_or_else(|| native_state_error("M2 rotation requires its third owned float word"))?;
            let angle = f32::from_le_bytes(bytes.try_into().unwrap());
            if !angle.is_finite() { return Err(native_state_error("M2 rotation is nonfinite")); }
            placement.rotation = angle;
        } else {
            let bytes = payload.and_then(|bytes| bytes.get(..8))
                .ok_or_else(|| native_state_error("M2 position/scale requires two owned float words"))?;
            let pair = std::array::from_fn::<_, 2, _>(|index|
                f32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap()));
            if !pair[0].is_finite() || (command.code == 51 && !pair[1].is_finite()) {
                return Err(native_state_error("M2 position/scale contains a nonfinite used value"));
            }
            if command.code == 51 { placement.position = pair; placement.position_linked = true; } else { placement.scale = pair; }
        }
        store.apply_placement(id).map_err(super::error)?;
        Ok(Some(TaskCommandOutcome { done: true, propagate: false }))
    }

    pub(crate) fn dispatch_emote_geometry(
        &mut self, target: u32, packet: [u32; 4], queued: bool,
        output: &mut u32, vm: Option<&Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        if !matches!(packet[1], 51 | 55 | 66) { return Ok(None); }
        let command = TaskCommand { target: target as i32, sender: packet[0] as i32,
            code: packet[1], data: [packet[2], packet[3], 0] };
        let mut owned = if !queued {
            self.pending_owned_task_buffers(command)
        } else { None };
        if owned.is_none() {
            let loaded = self.data_services.emote.state.state(target)
                .is_some_and(|state| state.successful_resource_imports != 0);
            let payload = if loaded || queued {
                let vm = vm.ok_or_else(|| native_state_error("M2 raw geometry requires its caller VM"))?;
                Some(vm.read(packet[3], if packet[1] == 55 {12} else {8})?)
            } else { None };
            owned = Some(OwnedTaskBuffers::new(None, payload)?);
        }
        let mut owned = owned.unwrap();
        if queued {
            return self.queue_task_command_buffers(command, &owned, output)
                .map(|found| found.then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        self.route_task_command_buffers(command, &mut owned, output, 4096)
    }
}
