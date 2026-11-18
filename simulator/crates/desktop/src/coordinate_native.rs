//! Query the coordinate map explicitly selected by the common logical-input
//! host. Each output is re-read after preceding writes, preserving VM aliases.
use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, native_state_error, read_u32};

impl LoggingHost<'_> {
    pub(super) fn display_rectangle_native(
        &self, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let mut addresses = [0; 4];
        for (index, address) in addresses.iter_mut().enumerate() {
            *address = read_u32(args, index * 4)?;
            vm.read(*address, 1)?;
        }
        // The independent software device presents the completed logical
        // surface. Window/Android letterboxing is applied by the UI host
        // after this device rectangle, just as for its input coordinates.
        let device = self.canvas.as_ref()
            .ok_or_else(|| native_state_error("native 787 has no bound rendering device"))?;
        let rectangle = [0, 0, device.width(), device.height()];
        for (address, value) in addresses.into_iter().zip(rectangle) {
            vm.write(address, &value.to_le_bytes())?;
        }
        Ok(NativeReturn::Continue)
    }

    pub(super) fn doubled_device_offset_native(
        &mut self, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let x = read_u32(args, 0)?;
        let y = read_u32(args, 4)?;
        vm.read(x, 1)?;
        vm.read(y, 1)?;
        let map = self.data_services.host_input_coordinates
            .ok_or_else(|| native_state_error("input coordinates are not bound by this host"))?;
        self.ensure_canvas_state()
            .map_err(|_| native_state_error("coordinate query drawing configuration failed"))?;
        vm.write(y, &0_u32.to_le_bytes())?;
        vm.write(x, &0_u32.to_le_bytes())?;
        if self.data_services.canvas_native.mode == 0 && self.data_services.canvas_native.enabled {
            for (address, value) in [x, y].into_iter().zip(map.device_offset) {
                let raw = cs2_runtime::input_coordinates::x86_truncate(value) as u32;
                vm.write(address, &raw.to_le_bytes())?;
            }
        }
        for address in [x, y] {
            let raw = read_u32(&vm.read(address, 4)?, 0)?.wrapping_mul(2);
            vm.write(address, &raw.to_le_bytes())?;
        }
        for (axis, address) in [x, y].into_iter().enumerate() {
            let current = read_u32(&vm.read(address, 4)?, 0)? as i32;
            vm.write(address, &(map.logical_axis(axis, current) as u32).to_le_bytes())?;
        }
        Ok(NativeReturn::Continue)
    }
}
