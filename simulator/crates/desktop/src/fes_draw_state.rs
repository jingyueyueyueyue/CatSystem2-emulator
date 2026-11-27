//! DRAW's own cursor and borrowed canonical identities, not transition pixels.
use super::*;
use std::collections::BTreeSet;
use cs2_runtime::object_graph::{encode_component, decode_component};

const WIPE_TAG: &[u8] = b"CS2FESDRAW\x02";

#[derive(Serialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Capture<'a> { version: u32, store: &'a FesDrawStore }
#[derive(Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Saved { version: u32, store: FesDrawStore }

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct WithWipes<Store, Mask> {
    draw: Store,
    wipes: BTreeMap<u32, Mask>,
}

impl FesDrawStore {
    pub(crate) fn capture_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<Vec<u8>> {
        self.validate_checkpoint(kinds)?;
        let draw = Capture { version: 1, store: self };
        let wipes: BTreeMap<_, _> = self.owners.iter().filter_map(|(&owner, draw)|
            draw.wipe.as_ref().map(|wipe| (owner, wipe))).collect();
        if wipes.is_empty() {
            return Ok(encode_component(&draw)?);
        }
        let mut bytes = WIPE_TAG.to_vec();
        bytes.extend(encode_component(&WithWipes { draw, wipes })?);
        Ok(bytes)
    }
    pub(crate) fn prepare_checkpoint(bytes: &[u8], kinds: &BTreeMap<u32, u32>) -> Result<Self> {
        let saved: Saved = if let Some(bytes) = bytes.strip_prefix(WIPE_TAG) {
            let WithWipes { mut draw, wipes }: WithWipes<Saved, Wipe> = decode_component(bytes)?;
            for (owner, wipe) in wipes {
                draw.store.owners.get_mut(&owner).context("FES wipe checkpoint has no DRAW owner")?.wipe = Some(wipe);
            }
            draw
        } else {
            decode_component(bytes)?
        };
        if saved.version != 1 { bail!("unsupported FES DRAW checkpoint version"); }
        saved.store.validate_checkpoint(kinds)?;
        Ok(saved.store)
    }
    pub(crate) fn commit_checkpoint(&mut self, prepared: Self) { *self = prepared; }

    pub(crate) fn checkpoint_motion_identities(
        &self,
    ) -> BTreeMap<(u32, Vec<u8>, i32), (Vec<u8>, i32)> {
        self.owners.iter().map(|(&owner, draw)|
            ((owner, draw.temporary_name().to_vec(), 0), draw.parent_motion.clone())).collect()
    }

    /// The dynamic adapter name is published before attach_draw_motion can
    /// finish. Only this retained creation phase may lack its motion node.
    pub(crate) fn checkpoint_motion_creation_pending(&self, owner: u32) -> bool {
        self.owners.get(&owner).is_some_and(|draw| draw.creating)
    }

    pub(crate) fn checkpoint_transition_links(&self) -> impl Iterator<Item = (u32, u32, u32)> + '_ {
        self.owners.iter().filter_map(|(&owner, draw)| draw.native_task.map(|task|
            (owner, task, draw.task_kind())))
    }

    pub(crate) fn validate_checkpoint_owner(&self, owner: u32, driver: &FesScriptDriver) -> Result<()> {
        if let Some(draw) = self.owners.get(&owner) {
            let temporary = driver.adapter.object_bank_definition(draw.temporary_name(), 0)
                .context("DRAW checkpoint lost its dynamic driver identity")?;
            let parent = driver.adapter.object_bank_definition(&draw.parent_motion.0, draw.parent_motion.1 as usize)
                .context("DRAW checkpoint lost its actual parent PLANE bank")?;
            if temporary.kind_name != draw.temporary_kind() || parent.kind_name != b"PLANE" {
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
            if kinds.get(owner) != Some(&34) || draw.duration == 0 || draw.parent.len() > 4096
                || draw.parent_motion.0.len() > 31 || draw.parent_motion.0.contains(&0) || draw.parent_motion.1 < 0
                || draw.native_task.is_some_and(|id| kinds.get(&id) != Some(&draw.task_kind()) || !tasks.insert(id))
                || draw.list.is_some_and(|id| kinds.get(&id) != Some(&15))
                || [draw.snapshot, draw.wipe.as_ref().and_then(|wipe| wipe.mask)].into_iter().flatten()
                    .any(|id| id == 0 || !snapshots.insert(id))
                || draw.wipe.as_ref().is_some_and(|wipe| wipe.resource.is_empty()
                    || wipe.resource.len() > 4096 || wipe.resource.contains('\0'))
                || draw.prepared_hidden && (draw.list.is_none() || draw.parameters.is_none()) {
                bail!("FES DRAW checkpoint lost an actual owner or preparation stage");
            }
            if let Some(bytes) = &draw.parameters {
                let size = if draw.wipe.is_some() { 56 } else { 44 };
                if bytes.len() != size
                    || draw.snapshot.is_none() || draw.list.is_none()
                    || word(bytes, 0)? != draw.duration || Some(word(bytes, 1)?) != draw.snapshot
                    || word(bytes, 2)? != 0 || word(bytes, 3)? != 0
                    || draw.wipe.as_ref().is_some_and(|wipe| wipe.mask.is_none())
                    || (draw.wipe.is_some() && Some(word(bytes, 7)?) != draw.wipe.as_ref().and_then(|wipe| wipe.mask)) {
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
                || [draw.snapshot, draw.wipe.as_ref().and_then(|wipe| wipe.mask)].into_iter().flatten()
                    .any(|id| !wrappers.contains(&id)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wipe_checkpoint_preserves_mask_and_motion_identity() -> Result<()> {
        let draw = Draw {
            command: 0,
            parent: "plane".into(),
            parent_motion: (b"plane".to_vec(), 0),
            duration: 30,
            creating: false,
            native_task: Some(4),
            snapshot: Some(10),
            list: Some(2),
            parameters: Some([30u32, 10, 0, 0, 11, 0, 0, 12, 0, 0, 0, 0, 0, 0]
                .into_iter().flat_map(u32::to_le_bytes).collect()),
            phase: Some(3),
            last_frame: Some(19),
            prepared_hidden: true,
            wipe: Some(Wipe { resource: "image/mask.hg3".into(), mask: Some(12) }),
        };
        let store = FesDrawStore { owners: BTreeMap::from([(1, draw)]), ..Default::default() };
        let kinds = BTreeMap::from([(1, 34), (2, 15), (4, 26)]);
        let bytes = store.capture_checkpoint(&kinds)?;
        let restored = FesDrawStore::prepare_checkpoint(&bytes, &kinds)?;
        let parents = BTreeMap::from([(4, 1)]);
        restored.validate_checkpoint_links(&kinds, &parents, &BTreeSet::from([10, 11, 12]))?;
        assert!(restored.validate_checkpoint_links(&kinds, &parents, &BTreeSet::from([10, 11])).is_err());
        assert!(restored.checkpoint_motion_identities().contains_key(&(1, WIPE_NAME.to_vec(), 0)));
        assert_eq!(restored.checkpoint_transition_links().collect::<Vec<_>>(), [(1, 4, 26)]);
        assert_eq!(restored.capture_checkpoint(&kinds)?, bytes);
        Ok(())
    }
}
