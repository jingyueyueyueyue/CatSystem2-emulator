//! Panel-owned sources and command stages. Final submitted pixels remain in
//! the existing CPU media/list component; restoration never repaints them.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use anyhow::ensure;
use cs2_runtime::graph_serde::{Serialize, Deserialize, Serializer, Deserializer};
use cs2_runtime::object_graph::{self as wire, ImageFrameSnapshot};
use crate::{FesRuntime, fes_driver::FesDriverEffect};

#[derive(Serialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Capture<'a> { version: u32, store: &'a FesSavePanelStore }
#[derive(Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Saved { version: u32, store: FesSavePanelStore }

pub(crate) struct PreparedSavePanels { store: FesSavePanelStore }
impl PreparedSavePanels {
    pub(crate) fn store(&self) -> &FesSavePanelStore { &self.store }
    pub(crate) fn into_store(self) -> FesSavePanelStore { self.store }
}

impl FesSavePanelStore {
    pub(crate) fn capture_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<Vec<u8>> {
        self.validate_checkpoint(kinds)?;
        Ok(wire::encode_component(&Capture { version: 1, store: self })?)
    }
    pub(crate) fn prepare_checkpoint(bytes: &[u8], kinds: &BTreeMap<u32, u32>) -> Result<PreparedSavePanels> {
        let saved: Saved = wire::decode_component(bytes)?;
        ensure!(saved.version == 1, "unsupported SAVEPANEL checkpoint version");
        saved.store.validate_checkpoint(kinds)?;
        Ok(PreparedSavePanels { store: saved.store })
    }
    pub(crate) fn commit_checkpoint(&mut self, prepared: PreparedSavePanels) {
        *self = prepared.into_store();
    }
    pub(crate) fn checkpoint_bank_keys(&self) -> impl Iterator<Item = &bank::Key> { self.banks.keys() }
    pub(crate) fn checkpoint_pending_owners(&self) -> impl Iterator<Item = &u32> { self.pending.keys() }
    /// The enclosing transaction must find this same command among its
    /// retained effects, including notification/handler-local continuations.
    pub(crate) fn checkpoint_matches_pending(&self, owner: u32, effect: &FesDriverEffect) -> bool {
        let Some(pending) = self.pending.get(&owner) else { return false };
        matches!(effect, FesDriverEffect::Command(command) if same_command(&pending.command, command))
    }
    pub(crate) fn validate_checkpoint_owner(&self, owner: u32, runtime: &FesRuntime,
        driver: &FesScriptDriver) -> Result<()> {
        let mut layers = BTreeSet::new();
        for ((id, name, index), panel) in &self.banks {
            if *id != owner { continue; }
            let declaration = usize::try_from(*index).ok()
                .and_then(|index| driver.adapter.object_bank_definition(name, index))
                .context("SAVEPANEL checkpoint has no original adapter bank")?;
            ensure!(declaration.kind_name == b"SAVEPANEL", "SAVEPANEL checkpoint declaration kind differs");
            let layer = runtime.media.get(&panel.layer).context("SAVEPANEL checkpoint has no CPU media consumer")?;
            ensure!(layers.insert(panel.layer.clone()) && layer.name == panel.layer && layer.kind == "SAVEPANEL"
                && !runtime.planes.contains_key(&panel.layer) && layer.animation.is_none()
                && !layer.source.has_request(), "SAVEPANEL checkpoint CPU layer identity differs");
        }
        for (key, layer) in &runtime.media {
            if layer.kind == "SAVEPANEL" {
                ensure!(layers.contains(key), "SAVEPANEL CPU pixels have no dedicated source owner");
            }
        }
        Ok(())
    }
    pub(crate) fn validate_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<()> {
        let owner = |id: &u32| kinds.get(id) == Some(&34);
        ensure!(self.banks.len() <= bank::MAX_BANKS && self.declarations.len() <= 1_000_000
            && self.initializers.len() <= 1_000_000 && self.pending.len() <= bank::MAX_BANKS
            && self.sources.len() <= bank::MAX_CELLS, "SAVEPANEL checkpoint exceeds live item bounds");
        let mut storage = 0usize;
        for ((id, name, index), panel) in &self.banks {
            ensure!(owner(id) && !name.is_empty() && name.len() <= 31 && !name.contains(&0)
                && (0..bank::MAX_BANKS as i32).contains(index) && panel.layer.len() <= 4096
                && !panel.layer.contains('\0') && panel.file.len() <= 255 && !panel.file.contains(&0)
                && panel.expanded_file.as_ref().is_none_or(|name| name.len() <= 2047 && !name.contains(&0))
                && panel.data.flags.0.len() == 1024 && panel.data.save_name.len() <= 259
                && !panel.data.save_name.contains(&0), "SAVEPANEL checkpoint bank/data identity is invalid");
            ensure!(panel.cells.len() <= bank::MAX_CELLS && panel.contents.len() <= bank::MAX_CELLS
                && panel.enabled.len() <= bank::MAX_CELLS && panel.user_fonts.len() <= bank::MAX_CELLS
                && panel.reference_sources.len() <= panel.contents.len(), "SAVEPANEL checkpoint cell storage exceeds bounds");
            charge(&mut storage, panel.data.flags.0.len() * 4)?;
            if let Some(data) = &panel.data.data { charge(&mut storage, data.storage().len())?; }
            for definition in panel.cells.values() { validate_record(definition)?; charge(&mut storage, 188)?; }
            for (id, content) in &panel.contents {
                ensure!(panel.cells.contains_key(id), "SAVEPANEL applied cell has no original definition");
                validate_record(&content.record)?;
                ensure!(content.text.len() < 256 && !content.text.contains(&0), "SAVEPANEL saved text exceeds its native field");
                charge(&mut storage, 188 + content.text.len())?;
                if let Some(image) = &content.image { charge(&mut storage, image.pixels().len())?; }
            }
            for (id, fonts) in &panel.user_fonts {
                ensure!(panel.cells.contains_key(id) && fonts.len() <= bank::MAX_CELLS,
                    "SAVEPANEL userfont has no owned cell or exceeds mapping bounds");
                for font in fonts {
                    ensure!(font.file.len() < 64 && !font.file.contains(&0), "SAVEPANEL userfont source is invalid");
                    charge(&mut storage, 88)?;
                }
            }
            for (name, frames) in &panel.reference_sources {
                ensure!(name.as_ref().is_none_or(|name| name.len() <= 4096 && !name.contains('\0'))
                    && frames.len() <= 1_000_000, "SAVEPANEL retained digit-source identity is invalid");
                charge(&mut storage, frames.len() * 4)?;
            }
        }
        for (name, bytes) in &self.sources {
            ensure!(!name.is_empty() && name.len() <= 4096 && !name.contains(&0),
                "SAVEPANEL encoded resource name is invalid");
            charge(&mut storage, bytes.len())?;
        }
        ensure!(!self.banks.is_empty() || self.sources.is_empty(), "SAVEPANEL resource cache outlived every bank");
        ensure!(self.declarations.iter().all(|(id, offset)| owner(id) && *offset <= 64 * 1024 * 1024)
            && self.initializers.iter().all(|((id, offset), cursor)| owner(id)
                && *offset <= 64 * 1024 * 1024 && *cursor <= bank::MAX_BANKS * 128),
            "SAVEPANEL checkpoint declaration/initializer owner is invalid");
        for (id, pending) in &self.pending {
            ensure!(owner(id) && pending.key.0 == *id && self.banks.contains_key(&pending.key),
                "SAVEPANEL pending source lost its actual owner/bank");
            let command = &pending.command;
            ensure!(command.offset <= 64 * 1024 * 1024 && command.source.len() <= 1024 * 1024
                && command.parameters.len() <= 64 && command.parameters.iter().all(|p| p.len() <= 65_536)
                && matches!(command.parameters.get(1).map(Vec::as_slice), Some(b"setsave" | b"getflag")),
                "SAVEPANEL pending command is not a retained source request");
            if command.parameters.first().is_some_and(|p| p == b"@") {
                ensure!(command.object_event.as_ref().is_some_and(|event|
                    event.name == pending.key.1 && event.bank == pending.key.2),
                    "SAVEPANEL pending @ command changed input identity");
            }
            if matches!(pending.request, PanelDataRequest::ApplyCellSources | PanelDataRequest::SetSave { .. }) {
                ensure!(command.parameters.get(1).is_some_and(|v| v == b"setsave"),
                    "SAVEPANEL source-application stage belongs to a different command");
            }
            if matches!(pending.request, PanelDataRequest::GetFlag { .. }) {
                ensure!(command.parameters.get(1).is_some_and(|v| v == b"getflag"),
                    "SAVEPANEL prepared flag query belongs to a different command");
            }
        }
        Ok(())
    }
}

fn same_command(a: &FesDriverCommand, b: &FesDriverCommand) -> bool {
    a.offset == b.offset && a.source == b.source && a.parameters == b.parameters
        && a.context == b.context && a.object_event == b.object_event
}
fn charge(total: &mut usize, bytes: usize) -> Result<()> {
    *total = total.checked_add(bytes).filter(|n| *n <= wire::MAX_GRAPH_BYTES)
        .context("SAVEPANEL checkpoint source/pixel storage exceeds graph limit")?;
    Ok(())
}
fn validate_record(record: &cell::Cell) -> Result<()> {
    ensure!(record.words.len() == 188, "SAVEPANEL cell record is not 188 bytes");
    ensure!(record.word(0) <= 7, "SAVEPANEL cell record kind is invalid");
    Ok(())
}

pub(super) mod css {
    use super::*;
    type Value = (u32, u32, u32, Vec<u8>);
    pub(crate) fn serialize<S: Serializer>(v: &Option<CssFile>, s: S) -> std::result::Result<S::Ok, S::Error> {
        v.as_ref().map(|file| (file.tag(), file.capacity(), file.used_len(), file.storage().to_vec())).serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<CssFile>, D::Error> {
        Option::<Value>::deserialize(d)?.map(|(tag, capacity, used, storage)|
            CssFile::from_storage_state(tag, capacity, used, storage)
                .map_err(cs2_runtime::graph_serde::de::Error::custom)).transpose()
    }
}
pub(super) mod image {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(v: &Option<cs2_renderer::RgbaFrame>, s: S) -> std::result::Result<S::Ok, S::Error> {
        v.as_ref().map(crate::image_manager::capture_frame).serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<cs2_renderer::RgbaFrame>, D::Error> {
        Option::<ImageFrameSnapshot>::deserialize(d)?.map(|frame|
            crate::image_manager::restore_frame(&frame).map_err(cs2_runtime::graph_serde::de::Error::custom)).transpose()
    }
}
