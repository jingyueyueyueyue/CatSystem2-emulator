//! Prefix lookup over the same typed ordered dictionary used by 515/675.
use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, expand_kcs_format, native_state_error, read_u32, trim_nul};

impl LoggingHost<'_> {
    pub(super) fn dictionary_prefix_native(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 { return Ok(NativeReturn::Value(0)); }
        let query = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        let query = trim_nul(&query);
        if query.len() > 4095 { return Err(native_state_error("dictionary prefix query exceeds temporary capacity")); }
        let map = self.integer_maps.get(&handle)
            .ok_or_else(|| native_state_error("dictionary prefix query has an invalid/wrong-type handle"))?;
        let value = map.iter().find(|(key, _)| {
            let key = trim_nul(key);
            key.len() <= query.len() && query[..key.len()].eq_ignore_ascii_case(key)
        }).map_or(0, |(_, &value)| value);
        Ok(NativeReturn::Value(value))
    }
}
