//! Validate serialized CPU bindings against their existing driver, task and
//! pixel consumers. The platform itself is serialized by the FES transaction.
use super::*;
use crate::fes_tasks::FesTaskStore;

pub(crate) fn supported_object(kind: &[u8]) -> bool {
    // SAVEPANEL also requires its dedicated source/pending codec in CPU7.
    matches!(kind, b"BASE" | b"IMAGE" | b"CIMAGE" | b"DIFIMAGE" | b"BUTTON" | b"PLANE" | b"CPLANE" | b"SAVEPANEL")
}

impl FesTaskPlatform {
    pub(crate) fn checkpoint_object_identities(&self) -> impl Iterator<Item = &(u32, Vec<u8>, i32)> {
        self.objects.keys()
    }

    pub(crate) fn checkpoint_object_key(&self, owner: u32, name: &[u8], bank: i32) -> Option<&str> {
        self.objects.get(&(owner, name.to_vec(), bank)).map(|binding| binding.key.as_str())
    }

    pub(crate) fn validate_cpu_checkpoint(&self, tasks: &FesTaskStore,
        kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>,
        wrappers: &BTreeSet<u32>, encoding: super::super::ScriptTextEncoding) -> Result<()> {
        if self.button_properties.len() > 1_000_000 || self.button_regions.len() > 1_000_000
            || self.button_source_pending.len() > 1_000_000 || self.background_transfers.len() > 65_536
        { bail!("FES CPU checkpoint exceeds object/input bounds"); }
        for ((owner, name, bank), binding) in &self.objects {
            let runtime = tasks.runtime(*owner).context("CPU checkpoint owner has no runtime")?;
            let driver = runtime.driver.as_ref().context("CPU checkpoint driver is borrowed")?;
            let definition = usize::try_from(*bank).ok()
                .and_then(|bank| driver.adapter.object_bank_definition(name, bank))
                .context("CPU checkpoint binding has no actual adapter bank")?;
            let decoded = encoding.decode(name);
            if !supported_object(&binding.kind) || binding.kind == b"BASE"
                || definition.kind_name != binding.kind
                || (binding.key != format!("{decoded}[{bank}]") && !(*bank == 0 && binding.key == decoded))
            { bail!("FES CPU checkpoint kind/name/bank differs from its actual declaration"); }
            let child_kind = match binding.kind.as_slice() {
                b"PLANE" | b"CPLANE" => Some(2), b"BUTTON" => Some(6), _ => None,
            };
            match (child_kind, binding.child) {
                (None, None) => {},
                (Some(kind), Some(child)) if kinds.get(&child) == Some(&kind) && parents.get(&child) == Some(owner) => {},
                _ => bail!("FES CPU checkpoint lost its real plane/button child"),
            }
            if matches!(binding.kind.as_slice(), b"PLANE" | b"CPLANE") && !runtime.planes.contains_key(&binding.key) {
                bail!("FES CPU checkpoint plane has no actual runtime layer");
            }
            let media_kind = match binding.kind.as_slice() { b"PLANE" => None, b"CPLANE" => Some(b"CIMAGE".as_slice()),
                kind => Some(kind) };
            if let Some(kind) = media_kind {
                let layer = runtime.media.get(&binding.key).context("FES CPU checkpoint pixel consumer is absent")?;
                if layer.kind.as_bytes() != kind || layer.name != binding.key
                { bail!("FES CPU checkpoint media kind/plane reference is invalid"); }
                // A released plane may remain named by an image. The real
                // draw consumer hides that image until the plane exists again;
                // preserve the reference instead of rejecting or clearing it.
            }
            if binding.kind == b"BUTTON" {
                let key = (*owner, name.clone(), *bank);
                if !self.button_properties.contains_key(&key)
                    || binding.child.is_none_or(|child| self.button_regions.get(&child) != Some(&key))
                { bail!("FES BUTTON checkpoint has no live properties/region identity"); }
            }
        }
        for ((owner, index, bank), _) in &self.button_records {
            let driver = tasks.runtime(*owner).and_then(|r| r.driver.as_ref())
                .context("FES BUTTON record owner driver is absent")?;
            let record = driver.adapter.objects.get(*index).context("FES BUTTON record declaration is absent")?;
            if record.definition.kind_name != b"BUTTON" || *bank < 0 || *bank as usize >= record.definition.bank_count
                || !self.objects.contains_key(&(*owner, record.definition.name.clone(), *bank))
            { bail!("FES BUTTON record checkpoint differs from its actual declaration identity"); }
        }
        if self.button_properties.keys().any(|key| self.objects.get(key).is_none_or(|b| b.kind != b"BUTTON"))
            || self.button_regions.values().any(|key| self.objects.get(key).is_none_or(|b| b.kind != b"BUTTON"))
        { bail!("FES BUTTON checkpoint properties/regions have no live BUTTON owner"); }
        for ((owner, offset), cursor) in &self.initialization {
            if tasks.runtime(*owner).is_none() || *offset > 64 * 1024 * 1024 || *cursor > 1_000_000 {
                bail!("FES CPU initializer checkpoint cursor is invalid");
            }
        }
        for (owner, wrapper) in &self.background_transfers {
            if tasks.runtime(*owner).is_none() || wrapper.is_some_and(|id| !wrappers.contains(&id)) {
                bail!("FES background transfer checkpoint lost its held image wrapper");
            }
        }
        Ok(())
    }
}
