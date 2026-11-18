//! DRAW's own cursor and borrowed canonical identities, not transition pixels.
use super::*;
use std::collections::BTreeSet;
use cs2_runtime::object_graph::{encode_component, decode_component};

#[derive(Serialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Capture<'a> { version: u32, store: &'a FesDrawStore }
#[derive(Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Saved { version: u32, store: FesDrawStore }

impl FesDrawStore {
    pub(crate) fn capture_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<Vec<u8>> {
        self.validate_checkpoint(kinds)?;
        Ok(encode_component(&Capture { version: 1, store: self })?)
    }
    pub(crate) fn prepare_checkpoint(bytes: &[u8], kinds: &BTreeMap<u32, u32>) -> Result<Self> {
        let saved: Saved = decode_component(bytes)?;
        if saved.version != 1 { bail!("unsupported FES DRAW checkpoint version"); }
        saved.store.validate_checkpoint(kinds)?;
        Ok(saved.store)
    }
    pub(crate) fn commit_checkpoint(&mut self, prepared: Self) { *self = prepared; }

    pub(crate) fn checkpoint_motion_identities(
        &self,
    ) -> BTreeMap<(u32, Vec<u8>, i32), (Vec<u8>, i32)> {
        self.owners.iter().map(|(&owner, draw)|
            ((owner, DRAW_NAME.to_vec(), 0), draw.parent_motion.clone())).collect()
    }

    /// The dynamic adapter name is published before attach_draw_motion can
    /// finish. Only this retained creation phase may lack its motion node.
    pub(crate) fn checkpoint_motion_creation_pending(&self, owner: u32) -> bool {
        self.owners.get(&owner).is_some_and(|draw| draw.creating)
    }

    /// Call after validate_checkpoint_links: retained parameters are either
    /// absent or the original 44-byte record, never truncated/reconstructed.
    pub(crate) fn checkpoint_transition_links(
        &self,
    ) -> Vec<(u32, u32, Option<i32>, Option<Vec<u8>>)> {
        self.owners.iter().filter_map(|(&owner, draw)| draw.native_task.map(|task|
            (owner, task, draw.phase, draw.parameters.clone()))).collect()
    }

    pub(crate) fn validate_checkpoint_owner(&self, owner: u32, driver: &FesScriptDriver) -> Result<()> {
        if let Some(draw) = self.owners.get(&owner) {
            let temporary = driver.adapter.object_bank_definition(DRAW_NAME, 0)
                .context("DRAW checkpoint lost its dynamic driver identity")?;
            let parent = driver.adapter.object_bank_definition(&draw.parent_motion.0, draw.parent_motion.1 as usize)
                .context("DRAW checkpoint lost its actual parent PLANE bank")?;
            if temporary.kind_name != b"DRAW" || parent.kind_name != b"PLANE" {
                bail!("DRAW checkpoint driver identity/parent kind differs");
            }
        }
        Ok(())
    }

    fn validate_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<()> {
        if self.owners.len() > 65_536 || self.restore_lists.len() > 65_536 {
            bail!("FES DRAW checkpoint exceeds owner bounds");
        }
        let mut tasks = BTreeSet::new();
        let mut snapshots = BTreeSet::new();
        for (owner, draw) in &self.owners {
            if draw.wipe.is_some() {
                bail!("portable checkpoint does not support an active FES PLANE wipe");
            }
            if kinds.get(owner) != Some(&34) || draw.duration == 0 || draw.parent.len() > 4096
                || draw.parent_motion.0.len() > 31 || draw.parent_motion.0.contains(&0) || draw.parent_motion.1 < 0
                || draw.native_task.is_some_and(|id| kinds.get(&id) != Some(&27) || !tasks.insert(id))
                || draw.list.is_some_and(|id| kinds.get(&id) != Some(&15))
                || draw.snapshot.is_some_and(|id| id == 0 || !snapshots.insert(id))
                || draw.prepared_hidden && (draw.list.is_none() || draw.parameters.is_none()) {
                bail!("FES DRAW checkpoint lost an actual owner or preparation stage");
            }
            if let Some(bytes) = &draw.parameters {
                if bytes.len() != 44 || draw.snapshot.is_none() || draw.list.is_none()
                    || word(bytes, 0)? != draw.duration || Some(word(bytes, 1)?) != draw.snapshot
                    || word(bytes, 2)? != 0 || word(bytes, 3)? != 0 {
                    bail!("FES DRAW checkpoint parameter record differs from its captured sources");
                }
            }
        }
        if self.restore_lists.iter().any(|(owner, list)| kinds.get(owner) != Some(&34) || kinds.get(list) != Some(&15)) {
            bail!("FES DRAW submission-restore checkpoint lost its actual list owner");
        }
        Ok(())
    }

    /// Call on the same prepared canonical task/wrapper maps. Snapshot and
    /// new-source handles retain their exact pixel aliases and ownership;
    /// prepare/commit never call register_image_wrapper or take a screenshot.
    pub(crate) fn validate_checkpoint_links(
        &self, kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>, wrappers: &BTreeSet<u32>,
    ) -> Result<()> {
        self.validate_checkpoint(kinds)?;
        for (owner, draw) in &self.owners {
            if draw.native_task.is_some_and(|id| parents.get(&id) != Some(owner))
                || draw.snapshot.is_some_and(|id| !wrappers.contains(&id)) {
                bail!("FES DRAW checkpoint task parent/snapshot wrapper differs");
            }
            if let Some(bytes) = &draw.parameters {
                if !wrappers.contains(&word(bytes, 4)?) {
                    bail!("FES DRAW checkpoint new-image wrapper is absent");
                }
            }
        }
        Ok(())
    }

    /// Join the original DRAW cursor to the canonical type15 component.
    /// Neither the list nor the transition is rebuilt here.
    pub(crate) fn validate_platform_links(
        &self, platform: &crate::fes_task_platform::FesTaskPlatform,
        lists: &BTreeMap<u32, crate::fes_task_platform::ImageList>,
        bindings: &BTreeMap<u32, Option<crate::fes_tasks::FesListBinding>>,
    ) -> Result<()> {
        for (&owner, draw) in &self.owners {
            platform.validate_draw_projection_link(lists, owner, &draw.parent,
                draw.list, draw.parameters.as_deref(), draw.prepared_hidden)?;
            if !bindings.contains_key(&owner) {
                bail!("FES DRAW checkpoint has no original FES owner slot");
            }
        }
        for (owner, list) in &self.restore_lists {
            if !lists.contains_key(list) || !bindings.contains_key(owner) {
                bail!("FES DRAW submission-restore cursor lost its canonical list binding");
            }
        }
        Ok(())
    }
}

fn word(bytes: &[u8], index: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes.get(index * 4..index * 4 + 4)
        .context("DRAW checkpoint word exceeds retained record")?.try_into()?))
}
