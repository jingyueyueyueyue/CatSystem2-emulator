//! Platform metadata and links to the canonical type-15/input components.
//! Lists and other FES stores keep their own codecs. No prepare/commit path
//! executes expressions, loads media, submits a list or registers a task.
use super::*;
use crate::fes_controls::FesControlStore;
use crate::fes_tasks::FesTaskStore;
use crate::fes_text::FesTextStore;
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_runtime::message_object::input_snapshot::InputSamplingStateSnapshot;
use cs2_runtime::object_graph::{decode_component, encode_component};

const TAG: &[u8] = b"CS2FESPLAT\x01";

#[derive(Serialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Capture<'a> { version: u32, platform: &'a FesTaskPlatform }

#[derive(Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Saved { version: u32, platform: FesTaskPlatform }

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct PlatformWithMasks<Platform, Masks> { version: u32, platform: Platform, image_masks: Masks }

impl FesTaskPlatform {
    pub(crate) fn capture_checkpoint(
        &self, kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>,
        input: &InputSamplingStateSnapshot,
    ) -> Result<Vec<u8>> {
        self.validate_checkpoint(kinds, parents, input)?;
        self.encode_checkpoint()
    }

    pub(crate) fn capture_checkpoint_with_save_panels(&self, tasks: &FesTaskStore,
        kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>,
        input: &InputSamplingStateSnapshot,
    ) -> Result<Vec<u8>> {
        self.validate_checkpoint_with_save_panels(tasks, kinds, parents, input)?;
        self.encode_checkpoint()
    }

    fn encode_checkpoint(&self) -> Result<Vec<u8>> {
        let mut bytes = TAG.to_vec();
        let payload = if self.image_masks.is_empty() {
            encode_component(&Capture { version: 1, platform: self })?
        } else {
            encode_component(&PlatformWithMasks { version: 2, platform: self, image_masks: &self.image_masks })?
        };
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    pub(crate) fn prepare_checkpoint(
        bytes: &[u8], callbacks: &Self, kinds: &BTreeMap<u32, u32>,
        parents: &BTreeMap<u32, u32>, input: &InputSamplingStateSnapshot,
    ) -> Result<Self> {
        let mut prepared = Self::decode_checkpoint(bytes)?;
        prepared.validate_checkpoint(kinds, parents, input)?;
        prepared.preserve_checkpoint_callbacks(callbacks);
        Ok(prepared)
    }

    /// Consume the decoded dedicated bank component into independent storage.
    /// C then checks its exact driver/pending/list links on the prepared tree.
    pub(crate) fn prepare_checkpoint_with_save_panels(
        bytes: &[u8], callbacks: &Self, kinds: &BTreeMap<u32, u32>,
        parents: &BTreeMap<u32, u32>, input: &InputSamplingStateSnapshot,
        panels: crate::fes_save_panel::PreparedSavePanels,
    ) -> Result<Self> {
        let mut prepared = Self::decode_checkpoint(bytes)?;
        prepared.save_panels.commit_checkpoint(panels);
        prepared.validate_checkpoint_core(kinds, parents, input)?;
        prepared.preserve_checkpoint_callbacks(callbacks);
        Ok(prepared)
    }

    fn decode_checkpoint(bytes: &[u8]) -> Result<Self> {
        if let Some(bytes) = bytes.strip_prefix(TAG) {
            // The graph codec stores the struct field count before its version.
            let has_image_masks = bytes.get(..4) == Some(&3u32.to_le_bytes())
                && bytes.get(4..8) == Some(&2u32.to_le_bytes());
            if has_image_masks {
                let saved: PlatformWithMasks<Self, BTreeMap<(u32, String), u8>> = decode_component(bytes)?;
                let mut platform = saved.platform;
                platform.image_masks = saved.image_masks;
                return Ok(platform);
            }
            let saved: Saved = decode_component(bytes)?;
            if saved.version != 1 { bail!("unsupported FES platform checkpoint version"); }
            Ok(saved.platform)
        } else {
            // The previous task component encoded this same platform struct
            // directly. Its lists and host callbacks were also skipped.
            Ok(decode_component(bytes)?)
        }
    }

    /// Run after installing the already prepared type-15 component. Retain
    /// those exact lists/output wrappers/layers, not the session's old lists.
    /// The caller can then move this platform into its prepared FesTaskStore.
    pub(crate) fn commit_checkpoint(&mut self, mut prepared: Self) {
        prepared.adopt_checkpoint_lists(self);
        *self = prepared;
    }

    /// Same operation for C's whole-store commit: call on store.platform,
    /// then move that already prepared FesTaskStore into the live task store.
    pub(crate) fn adopt_checkpoint_lists(&mut self, installed: &mut Self) {
        self.lists = std::mem::take(&mut installed.lists);
        self.preserve_checkpoint_callbacks(installed);
    }

    /// Check submitted identities against the same prepared stores. Submitted
    /// pixels/options may predate a dirty bank: comparing or rebuilding them
    /// from its current projection would destroy a valid budget boundary.
    pub(crate) fn validate_projection_links(
        &self, tasks: &FesTaskStore, lists: &BTreeMap<u32, ImageList>,
        bindings: &BTreeMap<u32, Option<FesListBinding>>, kinds: &BTreeMap<u32, u32>,
        controls: &FesControlStore, text: &FesTextStore,
    ) -> Result<()> {
        if lists.len() > 1_000_000 || bindings.len() > 65_536 {
            bail!("FES platform checkpoint projection table exceeds graph bounds");
        }
        for (&owner, binding) in bindings {
            if kinds.get(&owner) != Some(&34)
                || binding.as_ref().is_some_and(|binding|
                    kinds.get(&binding.task) != Some(&15) || !lists.contains_key(&binding.task)) {
                bail!("FES platform checkpoint lost its original owner/list binding");
            }
        }
        for (owner, _, _) in self.objects.keys() {
            if !bindings.contains_key(owner) {
                bail!("FES platform checkpoint object owner has no prepared FES slot");
            }
        }
        for owner in self.background_transfers.keys() {
            if bindings.get(owner).and_then(Option::as_ref).is_none() {
                bail!("FES platform checkpoint background transfer lost its original list binding");
            }
        }

        // Exact keys are obtained from the real consumers. In particular text
        // surfaces use negative message IDs and hexadecimal raw-name layer
        // keys; neither is an ordinary task ID or an encoding to guess here.
        let mut extension_layers = BTreeSet::<(u32, String)>::new();
        for (owner, name, bank) in text.checkpoint_bank_keys() {
            let driver = tasks.runtime(*owner).and_then(|runtime| runtime.driver.as_ref())
                .context("text layer checkpoint driver is absent or borrowed")?;
            let definition = usize::try_from(*bank).ok()
                .and_then(|bank| driver.adapter.object_bank_definition(name, bank))
                .context("text layer checkpoint original bank is absent")?;
            for key in crate::fes_text::bank_release_layers(
                text, *owner, name, *bank, &definition.kind_name,
            )? {
                extension_layers.insert((*owner, key));
            }
        }
        {
            let panels = &self.save_panels;
            for (owner, name, bank) in panels.checkpoint_bank_keys() {
                tasks.checkpoint_bank_definition(*owner, name, *bank, b"SAVEPANEL")?;
                let key = panels.bank_layer(*owner, name, *bank)
                    .context("SAVEPANEL projection checkpoint lost its dedicated media layer")?;
                let runtime = tasks.runtime(*owner).context("SAVEPANEL projection owner runtime is absent")?;
                if runtime.media.get(key).is_none_or(|layer| layer.kind != "SAVEPANEL") {
                    bail!("SAVEPANEL projection checkpoint has no real pixel consumer");
                }
                extension_layers.insert((*owner, key.to_owned()));
            }
        }
        let mut submitted = BTreeSet::new();
        for (&list_id, list) in lists {
            if kinds.get(&list_id) != Some(&15) {
                bail!("FES platform checkpoint submission target is not type15");
            }
            for (owner, key) in list.layers.keys() {
                if *owner == u32::MAX {
                    if !key.starts_with("\0list-native:") {
                        bail!("saved native list layer uses an unknown reserved namespace");
                    }
                    continue;
                }
                let kind = kinds.get(owner).context("submitted list layer owner task is absent")?;
                if *kind != 34 { continue; } // Other task codecs own their layers.
                if !submitted.insert((*owner, key.clone())) {
                    bail!("FES checkpoint submits the same layer identity to multiple lists");
                }
                if extension_layers.contains(&(*owner, key.clone())) {
                    continue;
                }
                let fallback = bindings.get(owner).and_then(Option::as_ref).map(|b| b.task);
                if controls.checkpoint_layer_destination(*owner, key, fallback).is_some() {
                    continue;
                }
                let runtime = tasks.runtime(*owner).context("submitted FES layer has no live runtime")?;
                if !self.objects.iter().any(|((id, _, _), object)| *id == *owner && object.key == *key
                    && matches!(object.kind.as_slice(), b"IMAGE" | b"CIMAGE" | b"BUTTON"))
                    || !runtime.media.contains_key(key) || runtime.planes.contains_key(key)
                    || !bindings.contains_key(owner) {
                    bail!("submitted FES layer has no matching original bank/list consumer");
                }
            }
        }
        // A list-binding or override command can run before finish_task_media.
        // The old submitted destination is then still authoritative in the
        // canonical list component. Do not move that layer to its new desired
        // destination, require fresh pixels, or reject this saved dirty stage.
        Ok(())
    }

    /// A narrow borrowed view of DRAW's existing 44-byte transition record.
    /// C supplies these fields from the prepared DRAW store; no second DRAW
    /// state or transition pixels are serialized by the platform component.
    pub(crate) fn validate_draw_projection_link(
        &self, lists: &BTreeMap<u32, ImageList>, owner: u32, parent: &str,
        list: Option<u32>, parameters: Option<&[u8]>, prepared_hidden: bool,
    ) -> Result<()> {
        if !self.objects.iter().any(|((id, _, _), object)|
            *id == owner && object.key == parent && object.kind == b"PLANE") {
            bail!("FES DRAW projection lost its original platform PLANE binding");
        }
        let target = list.map(|id| lists.get(&id).context("FES DRAW shared list is absent")).transpose()?;
        if prepared_hidden && (target.is_none() || parameters.is_none()) {
            bail!("FES DRAW hidden submission has no prepared source record");
        }
        if let Some(bytes) = parameters {
            let target = target.context("FES DRAW source record has no shared image list")?;
            if bytes.len() != 44 || u32::from_le_bytes(bytes[16..20].try_into()?) != target.output {
                bail!("FES DRAW new-image source differs from the canonical list output wrapper");
            }
        }
        // submit/enabled are independently mutable native list switches.
        // Preserve the saved values even while DRAW's prepared_hidden is set.
        Ok(())
    }
}
