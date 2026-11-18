//! Native549 returns the unchanged VM input/return parameter in proven PEs.
use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, native_state_error};

impl LoggingHost<'_> {
    pub(super) fn vm_context_native(&mut self, vm: &mut Vm) -> Result<NativeReturn> {
        let digest = self.data_services.native_profile.map_or("", |profile| profile.pe_sha256);
        if !cs2_runtime::native_context_profile::returns_vm_identity(digest) {
            return Err(native_state_error("native 549 needs its exact VM return-slot alias profile"));
        }
        let identity = if let Some(identity) = vm.native_context_identity() {
            // A restored context still reserves its original object identity.
            *self.next_xml_handle = (*self.next_xml_handle).max(identity + 1);
            identity
        } else {
            let identity = *self.next_xml_handle;
            let next = identity.checked_add(1).filter(|_| identity != 0)
                .ok_or_else(|| native_state_error("native VM object identities exhausted"))?;
            vm.bind_native_context_identity(identity)?;
            *self.next_xml_handle = next;
            identity
        };
        Ok(NativeReturn::Value(identity))
    }
}
