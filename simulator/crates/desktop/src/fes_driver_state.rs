//! Driver checkpoint validation and shared-external rebinding. The wire holds
//! no process pointer and never recreates an external group by copying bytes.
use super::*;

impl FesDriverEffect {
    pub(crate) fn checkpoint_object_notice(&self, name: &[u8], bank: i32, event: &[u8]) -> bool {
        matches!(self, Self::ObjectEvent { target, event: actual, .. }
            if target.name == name && target.bank == bank && actual == event)
    }
    pub(crate) fn checkpoint_external(&mut self, external: &SharedExternalStorage, bind: bool) -> Result<()> {
        if let Self::Initialize { event, .. } = self { event.validate_execution_checkpoint()?; }
        if let Self::ExecKcs { external: field, .. } | Self::ExecFes { external: field, .. } = self {
            if bind { *field = external.clone(); }
            else if !field.same_storage(external) { bail!("FES effect has a different external owner"); }
        }
        Ok(())
    }
}

impl FesScriptDriver {
    pub(crate) fn checkpoint_object_notice(&self, name: &[u8], bank: i32, event: &[u8]) -> bool {
        self.handler.as_ref().is_some_and(|handler| handler.target.name == name
            && handler.target.bank == bank && handler.event == event)
            || self.notices.iter().chain(&self.deferred).any(|effect| effect.checkpoint_object_notice(name, bank, event))
            || self.inputs.iter().any(|input| input.event.as_deref() == Some(event)
                && input.target.as_ref().is_some_and(|target| target.name == name && target.bank == bank))
            || self.checkpoint_statement_object_notice(name, bank, event)
    }
    pub(crate) fn validate_checkpoint_owner(&self, owner: u32) -> Result<()> {
        if self.task_id != owner { bail!("FES driver checkpoint task identity differs from its owner"); }
        self.validate_checkpoint()
    }

    pub(crate) fn validate_checkpoint(&self) -> Result<()> {
        self.interpreter.validate_execution_checkpoint()?;
        self.adapter.validate_fes_checkpoint()?;
        if self.inputs.len() > MAX_INPUTS || self.systems.len() > MAX_INPUTS
            || self.notices.len() > MAX_NOTIFICATIONS || self.deferred.len() > MAX_NOTIFICATIONS
            || self.expression.local.len() != 512 || self.expression.shared.len() > 1_000_000
        { bail!("FES driver checkpoint exceeds its live-state bounds"); }
        if let Some(handler) = &self.handler {
            handler.saved_control.validate_execution_checkpoint(&self.interpreter)?;
            if handler.depth > 128 { bail!("FES handler checkpoint call depth is invalid"); }
        }
        if let Some(statement) = &self.media_statement { statement.validate_checkpoint(self)?; }
        if let Some(receipt) = &self.media_statement_result { receipt.validate_checkpoint()?; }
        if self.media_statement.is_some() && self.media_statement_result.is_some() {
            bail!("FES checkpoint has both a running statement and an unconsumed receipt");
        }
        Ok(())
    }

    pub(crate) fn checkpoint_external(&mut self, external: &SharedExternalStorage, bind: bool) -> Result<()> {
        if bind { self.external = external.clone(); }
        else if !self.external.same_storage(external) { bail!("FES driver lost its task external binding"); }
        for effect in self.notices.iter_mut().chain(&mut self.deferred) {
            effect.checkpoint_external(external, bind)?;
        }
        if let Some(statement) = &mut self.media_statement { statement.checkpoint_external(external, bind)?; }
        Ok(())
    }
}

impl FesMediaNotificationScope {
    pub(crate) fn checkpoint_object_notice(&self, name: &[u8], bank: i32, event: &[u8]) -> bool {
        self.handler.as_ref().is_some_and(|handler| handler.target.name == name
            && handler.target.bank == bank && handler.event == event)
            || self.notices.iter().chain(&self.deferred).any(|effect| effect.checkpoint_object_notice(name, bank, event))
            || self.inputs.iter().any(|input| input.event.as_deref() == Some(event)
                && input.target.as_ref().is_some_and(|target| target.name == name && target.bank == bank))
    }
    pub(crate) fn checkpoint_external(&mut self, external: &SharedExternalStorage, bind: bool) -> Result<()> {
        for effect in self.notices.iter_mut().chain(&mut self.deferred) {
            effect.checkpoint_external(external, bind)?;
        }
        Ok(())
    }

    pub(crate) fn validate_checkpoint(&self, driver: &FesScriptDriver) -> Result<()> {
        if self.inputs.len() > MAX_INPUTS || self.systems.len() > MAX_INPUTS
            || self.notices.len() > MAX_NOTIFICATIONS || self.deferred.len() > MAX_NOTIFICATIONS
        { bail!("FES parked notification checkpoint exceeds queue bounds"); }
        if let Some(handler) = &self.handler {
            handler.saved_control.validate_execution_checkpoint(&driver.interpreter)?;
        }
        Ok(())
    }
}
