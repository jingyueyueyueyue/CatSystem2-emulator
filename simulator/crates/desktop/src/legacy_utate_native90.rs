use cs2_runtime::TaskCommand;
use cs2_script::{NativeReturn, Result, Vm};

use super::css_model::{SHA as TSUNABAN_SHA, UTATE_SHA};
use super::particle_external::CAMPANELLA_SHA;
use super::plane_native::PlaneCommandBuffers;
use super::{
    LoggingHost, LogicEvent, OwnedTaskBuffers, motion_property_groups, native_state_error,
    read_motion_properties, read_u32,
};

const LEGACY_MOTION_GROUP_COUNT: usize = 8;
const CAMPANELLA_SSCRIPT_CODE_LEN: usize = 0x995b5;

fn campanella_motion_slot(index: u32, pc: usize, vm: &Vm) -> Option<(u32, u32, Option<u32>)> {
    let (local, first, stride, count): (u32, u32, u32, u32) = match (index, pc) {
        (311, 0x5c9c1 | 0x5ccdd) => (0x10, 0x74c, 0x314, 67),
        (311, 0x5d16a | 0x5d45e) => (0x10, 0xdd68, 0x568, 49),
        (311, 0x5dc64 | 0x5de65) => (0x10, 0x8000_0e44, 0x10c, 22),
        (311, 0x69810) => (0x0c, 0x8000_0e44, 0x10c, 22),
        (586, 0x6fbc4) => (0x0c, 0x8000_0e44, 0x10c, 4),
        (296, 0x7102e) => (0, 0x8000_0e44, 0x10c, 22),
        _ => return None,
    };
    let local_address = u32::try_from(vm.frame_base()).ok()?.checked_add(local)?;
    let slot = vm.read_u32(local_address).ok()?;
    if slot >= count {
        return None;
    }
    let address = first + slot * stride;
    Some((slot, address, vm.read_u32(address).ok()))
}

impl LoggingHost<'_> {
    pub(super) fn trace_campanella_motion_controller(&mut self, index: u32, handle: u32, vm: &Vm) {
        if self.css_task_sha() != Some(CAMPANELLA_SHA)
            || index == 311 && (handle == 0 || self.motion_controllers.contains_key(&handle))
        {
            return;
        }
        let pc = vm.instruction_pointer.checked_sub(11);
        let source = pc.filter(|&pc| vm.program().is_some_and(|program| {
            program.code.len() == CAMPANELLA_SSCRIPT_CODE_LEN
                && program.instruction_at(pc).is_some_and(|instruction| instruction.opcode == 0x2b)
        })).and_then(|pc| campanella_motion_slot(index, pc, vm));
        if index != 311 && source.is_none() {
            return;
        }
        let field = source.map_or_else(|| "unresolved".to_owned(), |(slot, address, value)| {
            format!("{slot}@{address:#010x}={value:#x?}")
        });
        let caller_pc = vm.frames().last().map(|frame| frame.return_pc);
        self.events.push(LogicEvent {
            frame: self.frame,
            kind: "campanella-motion-controller".into(),
            detail: format!("task={} native={index} pc={pc:#x?} caller_pc={caller_pc:#x?} \
                handle={handle:#x} field={field}", self.current_task_id),
        });
    }

    pub(super) fn tsunaban_legacy_string_plane(&self, kind: u32) -> bool {
        kind == 45 && self.css_task_sha() == Some(TSUNABAN_SHA)
    }

    pub(super) fn legacy_utate_string_binding(&self, kind: u32, command: u32) -> bool {
        matches!(command, 50 | 52 | 53)
            && (self.tsunaban_legacy_string_plane(kind)
                || matches!(
                    (self.css_task_sha(), kind),
                    (Some(UTATE_SHA), 47) | (Some(CAMPANELLA_SHA), 45)
                ))
    }

    pub(super) fn legacy_utate_native90(
        &mut self,
        index: u32,
        args: &[u8],
    ) -> Result<Option<NativeReturn>> {
        if index != 90 {
            return Ok(None);
        }
        let target = read_u32(args, 0)?;
        let Some(kind) = self.scene_nodes.get(&target).map(|node| node.kind) else {
            return Ok(None);
        };
        let tsunaban_button = kind == 6 && self.css_task_sha() == Some(TSUNABAN_SHA);
        let accepts = match kind {
            6 if tsunaban_button || self.css_task_sha() == Some(CAMPANELLA_SHA) => true,
            6 | 47 => self.bind_utate_plane_input_profile()?,
            45 => self.legacy_utate_string_binding(kind, 53),
            _ => false,
        };
        if !accepts {
            return Ok(None);
        }
        if args.len() != 16 {
            return Err(native_state_error(
                "legacy native 90 requires target and three float offsets",
            ));
        }
        if tsunaban_button {
            return Ok(Some(NativeReturn::Continue));
        }
        let command = TaskCommand {
            target: target as i32,
            sender: self.current_task_id as i32,
            code: 53,
            data: [0; 3],
        };
        let mut output = 0;
        if kind == 6 {
            self.install_device_input_receiver();
            let mut buffers = OwnedTaskBuffers::new(None, Some(args[4..].to_vec()))?;
            let routed =
                self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?;
            return Ok(Some(if routed.is_some_and(|outcome| !outcome.done) {
                NativeReturn::Retry
            } else {
                NativeReturn::Continue
            }));
        }
        let mut offsets = [0; 12];
        offsets.copy_from_slice(&args[4..]);
        let mut buffers = PlaneCommandBuffers {
            data0: None,
            data1: Some(&mut offsets),
        };
        if !self.dispatch_plane_command(command, &mut buffers, &mut output)? {
            return Err(native_state_error(
                "legacy string plane declined its parent binding",
            ));
        }
        Ok(Some(NativeReturn::Continue))
    }

    pub(super) fn legacy_utate_motion_properties_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<Option<NativeReturn>> {
        if !matches!(index, 299..=302) || args.len() != (1 + LEGACY_MOTION_GROUP_COUNT) * 4 {
            return Ok(None);
        }
        let verified_short_layout = matches!(index, 299 | 301 | 302)
            && matches!(self.css_task_sha(), Some(CAMPANELLA_SHA) | Some(TSUNABAN_SHA));
        if !verified_short_layout && !self.bind_utate_plane_input_profile()? {
            return Ok(None);
        }

        let handle = read_u32(args, 0)?;
        if handle == 0 {
            return Ok(Some(NativeReturn::Continue));
        }
        let controller = self
            .motion_controllers
            .get_mut(&handle)
            .ok_or_else(|| native_state_error("motion controller handle is invalid"))?;
        let properties = if matches!(index, 299 | 300) {
            &mut controller.base_properties
        } else {
            &mut controller.properties
        };
        let mut group_args = [0; LEGACY_MOTION_GROUP_COUNT + 1];
        for (target, bytes) in group_args.iter_mut().zip(args[4..].chunks_exact(4)) {
            *target = u32::from_le_bytes(bytes.try_into().unwrap());
        }

        if matches!(index, 299 | 301) {
            let (extra_property, _) = motion_property_groups()[LEGACY_MOTION_GROUP_COUNT];
            group_args[LEGACY_MOTION_GROUP_COUNT] = properties[extra_property];
            *properties = read_motion_properties(vm, &group_args)?;
        } else {
            for ((start, length), address) in motion_property_groups()
                .into_iter()
                .zip(group_args[..LEGACY_MOTION_GROUP_COUNT].iter().copied())
            {
                let bytes = properties[start..start + length]
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect::<Vec<_>>();
                vm.write(address, &bytes)?;
            }
        }
        Ok(Some(NativeReturn::Continue))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeadlessPlayer;
    use std::path::Path;

    #[test]
    fn campanella_native311_keeps_handle_validation_strict() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let save_root = std::env::temp_dir().join(format!("cs2-campanella-311-{}", std::process::id()));
        let mut player = HeadlessPlayer::open_with_save_root(root, save_root).unwrap();
        player.with_host(|host| {
            let mut vm = Vm::new();
            vm.instruction_pointer = 0x5dc6f;
            let invalid = 0x1234_5678_u32.to_le_bytes();
            let event_count = host.events.len();
            assert!(host.motion_native(311, &invalid, &mut vm).is_err());
            assert_eq!(host.events.len(), event_count);

            host.data_services.css_task_pe_sha256 = Some(CAMPANELLA_SHA);
            assert_eq!(host.motion_native(311, &[0; 4], &mut vm)?, NativeReturn::Value(0));
            let NativeReturn::Value(handle) = host.motion_native(294, &[], &mut vm)? else {
                panic!("native 294 did not allocate a motion controller");
            };
            assert_eq!(host.motion_native(311, &handle.to_le_bytes(), &mut vm)?, NativeReturn::Value(0));
            assert_eq!(host.events.len(), event_count);

            assert!(host.motion_native(311, &invalid, &mut vm).is_err());
            assert_eq!(host.events.len(), event_count + 1);
            let event = host.events.last().unwrap();
            assert_eq!(event.kind, "campanella-motion-controller");
            assert!(event.detail.contains("native=311 pc=Some(0x5dc64)"));
            assert!(event.detail.contains("handle=0x12345678 field=unresolved"));
            Ok(())
        }).unwrap();
    }

    #[test]
    fn legacy_native301_short_layout_preserves_last_dword_for_verified_pes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let save_root = std::env::temp_dir().join(format!("cs2-legacy-301-{}", std::process::id()));
        let mut player = HeadlessPlayer::open_with_save_root(root, save_root).unwrap();
        player.with_host(|host| {
            let mut vm = Vm::new();
            let mut expected: [u32; 36] = std::array::from_fn(|index| 0x1000 + index as u32);
            expected[28] = u32::MAX;
            vm.bind_external(expected.map(u32::to_le_bytes).concat())?;
            let NativeReturn::Value(handle) = host.motion_native(294, &[], &mut vm)? else {
                panic!("native 294 did not allocate a motion controller");
            };
            let controller = host.motion_controllers.get_mut(&handle).unwrap();
            let base_before = controller.base_properties;

            let pointer = |index: u32| 0x8000_0000 | (index * 4);
            let short = [
                handle, pointer(0), pointer(3), pointer(6), pointer(9),
                pointer(25), u32::MAX, pointer(29), pointer(32),
            ].map(u32::to_le_bytes).concat();
            for (sha, tail) in [(CAMPANELLA_SHA, 0xdead_beef), (TSUNABAN_SHA, 0xface_cafe)] {
                host.data_services.css_task_pe_sha256 = Some(sha);
                let controller = host.motion_controllers.get_mut(&handle).unwrap();
                controller.properties = [0; 36];
                controller.properties[35] = tail;
                assert!(host.motion_native(300, &short, &mut vm).is_err());
                assert_eq!(host.motion_native(301, &short, &mut vm)?, NativeReturn::Continue);
                expected[35] = tail;
                assert_eq!(host.motion_controllers[&handle].properties, expected);
                assert_eq!(host.motion_controllers[&handle].base_properties, base_before);
            }

            host.data_services.css_task_pe_sha256 = Some("unverified");
            assert!(host.motion_native(301, &short, &mut vm).is_err());
            assert_eq!(host.motion_controllers[&handle].properties, expected);

            let mut full = short;
            expected[35] = 0x1357_9bdf;
            full.extend_from_slice(&expected[35].to_le_bytes());
            assert_eq!(host.motion_native(301, &full, &mut vm)?, NativeReturn::Continue);
            assert_eq!(host.motion_controllers[&handle].properties, expected);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn legacy_native299_and_302_use_only_eight_groups_for_verified_pes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let save_root = std::env::temp_dir().join(format!("cs2-legacy-299-302-{}", std::process::id()));
        let mut player = HeadlessPlayer::open_with_save_root(root, save_root).unwrap();
        player.with_host(|host| {
            let mut vm = Vm::new();
            let mut expected_base: [u32; 36] = std::array::from_fn(|index| 0x1000 + index as u32);
            expected_base[28] = u32::MAX;
            expected_base[35] = 0x1111_2222;
            let current: [u32; 36] = std::array::from_fn(|index| 0x2000 + index as u32);
            let sentinel = 0xcccc_cccc_u32;
            let mut memory = expected_base.map(u32::to_le_bytes).concat();
            memory.extend([sentinel; 36].map(u32::to_le_bytes).concat());
            vm.bind_external(memory)?;
            let NativeReturn::Value(handle) = host.motion_native(294, &[], &mut vm)? else {
                panic!("native 294 did not allocate a motion controller");
            };
            let address = |index: u32| 0x8000_0000 | (index * 4);
            let set = [
                handle, address(0), address(3), address(6), address(9),
                address(25), u32::MAX, address(29), address(32),
            ].map(u32::to_le_bytes).concat();
            let get = [
                handle, address(36), address(39), address(42), address(45),
                address(61), address(64), address(65), address(68),
            ].map(u32::to_le_bytes).concat();

            host.data_services.css_task_pe_sha256 = Some("unverified");
            assert!(host.motion_native(299, &set, &mut vm).is_err());
            assert!(host.motion_native(302, &get, &mut vm).is_err());

            for sha in [CAMPANELLA_SHA, TSUNABAN_SHA] {
                host.data_services.css_task_pe_sha256 = Some(sha);
                let controller = host.motion_controllers.get_mut(&handle).unwrap();
                controller.base_properties = [0; 36];
                controller.base_properties[35] = expected_base[35];
                controller.properties = current;
                vm.write(address(36), &[sentinel; 36].map(u32::to_le_bytes).concat())?;

                assert_eq!(host.motion_native(299, &set, &mut vm)?, NativeReturn::Continue);
                assert_eq!(host.motion_controllers[&handle].base_properties, expected_base);
                assert_eq!(host.motion_controllers[&handle].properties, current);
                assert_eq!(host.motion_native(302, &get, &mut vm)?, NativeReturn::Continue);
                for (index, value) in current[..35].iter().enumerate() {
                    assert_eq!(vm.read_u32(address(36 + index as u32))?, *value);
                }
                assert_eq!(vm.read_u32(address(71))?, sentinel);
                assert_eq!(host.motion_controllers[&handle].properties, current);
            }
            Ok(())
        }).unwrap();
    }
}
