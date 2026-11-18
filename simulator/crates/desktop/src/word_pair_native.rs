//! Ordinary874 packs two low WORDs; 875 sign-extends the packed halves.
//! Output addresses are resolved before either write, preserving aliases.
use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, read_u32};

impl LoggingHost<'_> {
    pub(super) fn pack_words_native(&self, args: &[u8]) -> Result<NativeReturn> {
        let low = read_u32(args, 0)? & 0xffff;
        let high = read_u32(args, 4)? & 0xffff;
        Ok(NativeReturn::Value(low | high << 16))
    }

    pub(super) fn unpack_words_native(&self, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let low_address = read_u32(args, 4)?;
        vm.read(low_address, 1)?;
        let high_address = read_u32(args, 8)?;
        vm.read(high_address, 1)?;
        // The first write can alias the native argument block itself. Its
        // upper WORD was already loaded into EDX by the recovered wrapper.
        let packed = read_u32(args, 0)?;
        let low = (packed as u16 as i16 as i32) as u32;
        let high = ((packed >> 16) as u16 as i16 as i32) as u32;
        vm.write(low_address, &low.to_le_bytes())?;
        vm.write(high_address, &high.to_le_bytes())?;
        Ok(NativeReturn::Continue)
    }
}
