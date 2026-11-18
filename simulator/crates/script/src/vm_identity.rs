//! Portable identity of the VM object, distinct from script storage offsets.
use super::{Result, ScriptError, Vm};

impl Vm {
    pub fn native_context_identity(&self) -> Option<u32> { self.native_context_identity }

    /// The host's object allocator supplies this identity. Moving the Rust VM
    /// or loading another program in the same object does not change it.
    pub fn bind_native_context_identity(&mut self, identity: u32) -> Result<()> {
        if identity == 0 || identity == u32::MAX
            || self.native_context_identity.is_some_and(|old| old != identity)
        {
            return Err(ScriptError::InvalidStructuredText {
                offset: 0, message: "invalid or conflicting native VM object identity",
            });
        }
        self.native_context_identity = Some(identity);
        Ok(())
    }
}
