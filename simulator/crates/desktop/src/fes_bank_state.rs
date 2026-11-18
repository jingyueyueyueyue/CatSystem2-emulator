//! Read-only bank/continuation identities shared by the two bank codecs.
//! These queries never evaluate a bank expression or replay a host effect.
use super::*;

impl FesTaskStore {
    pub(crate) fn validate_checkpoint_save_panels(&self,
        panels: &crate::fes_save_panel::FesSavePanelStore) -> Result<BTreeSet<(u32, Vec<u8>, i32)>> {
        let keys = panels.checkpoint_bank_keys().cloned().collect();
        self.validate_checkpoint_bank_consumers(b"SAVEPANEL", &keys)?;
        for (&owner, slot) in &self.slots {
            let object = slot.object.as_ref().context("SAVEPANEL checkpoint FES object is borrowed")?;
            let Some(runtime) = &object.runtime else { continue; };
            let driver = runtime.driver.as_ref().context("SAVEPANEL checkpoint FES driver is borrowed")?;
            panels.validate_checkpoint_owner(owner, runtime, driver)?;
        }
        for &owner in panels.checkpoint_pending_owners() {
            if !self.checkpoint_pending_effects(owner)?.any(|effect|
                panels.checkpoint_matches_pending(owner, effect)) {
                bail!("SAVEPANEL prepared request lost its exact retained command/context");
            }
        }
        Ok(keys)
    }

    /// Only the current, actually bound bank qualifies. An older duplicate
    /// declaration or the driver retained across reload is not its owner.
    pub(crate) fn checkpoint_bank_definition(&self, owner: u32, name: &[u8], bank: i32,
        kind: &[u8]) -> Result<&cs2_runtime::FesObjectDefinitionState> {
        let object = self.slots.get(&owner).and_then(|slot| slot.object.as_ref())
            .context("bank checkpoint has no owned FES object")?;
        let driver = object.runtime.as_ref().and_then(|runtime| runtime.driver.as_ref())
            .context("bank checkpoint FES driver is borrowed or absent")?;
        let definition = usize::try_from(bank).ok()
            .and_then(|bank| driver.adapter.object_bank_definition(name, bank))
            .context("bank checkpoint has no live declaration binding")?;
        if definition.kind_name != kind {
            bail!("bank checkpoint declaration belongs to another object kind");
        }
        Ok(definition)
    }

    /// Includes effects parked by nested BUTTON/lower notifications. It does
    /// not include merely queued driver notices: those have not entered the
    /// bank consumer yet and cannot own a partially applied request.
    pub(crate) fn checkpoint_pending_effects(&self, owner: u32)
        -> Result<impl Iterator<Item = &FesDriverEffect>> {
        let object = self.slots.get(&owner).and_then(|slot| slot.object.as_ref())
            .context("bank continuation checkpoint has no owned FES object")?;
        Ok(object.pending_effect.iter().chain(&object.boundary)
            .chain(object.button_refresh.iter().filter_map(|work| work.pending_effect.as_ref()))
            .chain(object.lower_notification.iter().filter_map(|work| work.pending_effect.as_ref())))
    }

    pub(crate) fn checkpoint_bank_creation_pending(&self, owner: u32, name: &[u8], kind: &[u8]) -> bool {
        self.checkpoint_pending_effects(owner).is_ok_and(|mut effects| effects.any(|effect|
            matches!(effect, FesDriverEffect::Initialize { event: FesEvent::CreateObject(definition), .. }
                if definition.name == name && definition.kind == kind)))
    }

    /// Match the innermost active media pass, not a later task/host tick.
    /// The retained pass is resumed after restore at this same logical time.
    pub(crate) fn validate_checkpoint_media_tick(&self, owner: u32, now: u32) -> Result<()> {
        let object = self.slots.get(&owner).and_then(|slot| slot.object.as_ref())
            .context("bank update checkpoint has no owned FES object")?;
        let retained = object.lower_notification.as_ref().map(|work| work.now)
            .or_else(|| object.button_refresh.as_ref().map(|work| work.now))
            .or_else(|| object.system.as_ref().map(|work| work.now))
            .or(object.load_now).or(object.invocation_now);
        if retained != Some(now) {
            bail!("bank update checkpoint lost its exact FES media pass tick");
        }
        Ok(())
    }

    /// Validate the other direction too: serializing a store with no entries
    /// cannot silently discard a driver binding. Keys are live bank consumers;
    /// the bank codec separately validates any released records. A retained
    /// constructor may own the not-yet-published suffix of its declaration.
    pub(crate) fn validate_checkpoint_bank_consumers(&self, kind: &[u8],
        keys: &BTreeSet<(u32, Vec<u8>, i32)>) -> Result<()> {
        for (owner, name, bank) in keys {
            self.checkpoint_bank_definition(*owner, name, *bank, kind)?;
        }
        for (&owner, slot) in &self.slots {
            let object = slot.object.as_ref().context("bank checkpoint FES owner is borrowed")?;
            let Some(driver) = object.runtime.as_ref().and_then(|runtime| runtime.driver.as_ref()) else { continue; };
            for record in &driver.adapter.objects {
                if record.definition.kind_name != kind { continue; }
                for bank in &record.banks {
                    if driver.adapter.object_bank_definition(&record.definition.name, bank.index)
                        .is_none_or(|definition| !std::ptr::eq(definition, &record.definition)) { continue; }
                    let index = i32::try_from(bank.index).context("bank checkpoint index exceeds signed storage")?;
                    if !keys.contains(&(owner, record.definition.name.clone(), index))
                        && !self.checkpoint_bank_creation_pending(owner, &record.definition.name, kind) {
                        bail!("FES declared bank has no actual consumer or retained constructor");
                    }
                }
            }
        }
        Ok(())
    }
}
