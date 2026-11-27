//! Prepare real extension consumers with the driver, lower tasks and pixels.
use super::*;
use crate::fes_controls::FesControlStore;
use crate::fes_draw::FesDrawStore;
use crate::fes_motion::checkpoint::CheckpointExtensions;
use crate::fes_text::{FesTextStore, PreparedFesTextState};
use crate::native_tasks::{NativeTaskStore, PreparedLowerCommands};
use crate::transition_native::TransitionStore;

const TAG: &[u8] = b"CS2FESEXT2\0";
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Legacy { version: u32, text: Vec<u8>, controls: Vec<u8>, draw: Vec<u8> }
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot {
    version: u32, text: Vec<u8>, controls: Vec<u8>, draw: Vec<u8>,
    transitions: Vec<u8>, lower: Vec<u8>,
}
pub(super) struct PreparedExtensions {
    text: PreparedFesTextState, controls: FesControlStore, draw: FesDrawStore,
    transitions: TransitionStore, lower: PreparedLowerCommands,
}
impl PreparedExtensions {
    pub(super) fn motion_extensions(&self, tasks: &FesTaskStore) -> Result<CheckpointExtensions> {
        motion_extensions(self.text.store(), &self.controls, &self.draw, tasks)
    }
    pub(super) fn validate_projection_links(&self, tasks: &FesTaskStore,
        lists: &BTreeMap<u32, crate::fes_task_platform::ImageList>, kinds: &BTreeMap<u32, u32>) -> Result<()> {
        let bindings = tasks.checkpoint_list_bindings();
        tasks.platform.validate_projection_links(tasks, lists, &bindings, kinds, &self.controls, self.text.store())?;
        self.draw.validate_platform_links(&tasks.platform, lists, &bindings)
    }
    pub(super) fn commit(self, data: &mut crate::data_services::DataServices) {
        data.fes_controls.commit_checkpoint(self.controls);
        data.fes_draw.commit_checkpoint(self.draw);
        data.transitions = self.transitions;
        data.tasks.commit_lower_command_state(self.lower);
        data.fes_text.commit_checkpoint(self.text, &mut data.message_device);
    }
}

pub(super) fn motion_extensions(text: &FesTextStore, controls: &FesControlStore,
    draw: &FesDrawStore, tasks: &FesTaskStore) -> Result<CheckpointExtensions> {
    let mut proof = CheckpointExtensions::default();
    proof.texts.extend(text.checkpoint_bank_keys().cloned());
    for identity in controls.checkpoint_bank_identities() {
        let driver = tasks.runtime(identity.0).and_then(|r| r.driver.as_ref())
            .context("control checkpoint motion owner driver is absent")?;
        let definition = usize::try_from(identity.2).ok()
            .and_then(|bank| driver.adapter.object_bank_definition(&identity.1, bank))
            .context("control checkpoint motion bank declaration is absent")?;
        match definition.kind_name.as_slice() {
            b"SLIDER" | b"SCROLL2" | b"SCROLL" => { proof.sliders.insert(identity); },
            b"VIEWPORT" | b"IMGWND" => { proof.inert.insert(identity); },
            _ => bail!("control checkpoint motion identity belongs to a different object kind"),
        }
    }
    proof.draws = draw.checkpoint_motion_identities();
    for (owner, _, _) in proof.draws.keys() {
        if draw.checkpoint_motion_creation_pending(*owner) { proof.creating_draws.insert(*owner); }
    }
    Ok(proof)
}

fn validate_owners(text: &FesTextStore, controls: &FesControlStore, draw: &FesDrawStore,
    tasks: &FesTaskStore, kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>) -> Result<()> {
    for (owner, name, bank) in text.checkpoint_bank_keys() {
        let driver = tasks.runtime(*owner).and_then(|r| r.driver.as_ref())
            .context("FES text checkpoint actual owner driver is absent")?;
        let definition = usize::try_from(*bank).ok()
            .and_then(|index| driver.adapter.object_bank_definition(name, index))
            .context("FES text checkpoint original bank is absent")?;
        crate::fes_text::bank_release_layers(text, *owner, name, *bank, &definition.kind_name)?;
    }
    for (&id, &kind) in kinds {
        if matches!(kind, 41 | 42) {
            let owner = controls.checkpoint_lower_owner(id)
                .context("task41/42 checkpoint needs its actual FES lower receiver; legacy model42 is separate")?;
            if kinds.get(&owner) != Some(&34) || parents.get(&id) != Some(&owner) {
                bail!("lower task checkpoint parent differs from its real control bank");
            }
        }
        if kind == 34 {
            if let Some(driver) = tasks.runtime(id).and_then(|r| r.driver.as_ref()) {
                controls.validate_checkpoint_owner(id, driver)?;
                draw.validate_checkpoint_owner(id, driver)?;
            }
        }
    }
    Ok(())
}

impl HeadlessPlayer {
    pub(super) fn capture_fes_extension_state(&self, kinds: &BTreeMap<u32, u32>) -> Result<Vec<u8>> {
        let data = &self.data_services;
        data.fes_controls.validate_checkpoint_links(kinds, &self.task_parents)?;
        data.fes_draw.validate_checkpoint_links(kinds, &self.task_parents,
            &self.image_handles.keys().copied().collect())?;
        validate_owners(&data.fes_text, &data.fes_controls, &data.fes_draw,
            &data.tasks.fes, kinds, &self.task_parents)?;
        let bindings = data.tasks.fes.checkpoint_list_bindings();
        data.tasks.fes.platform.validate_projection_links(&data.tasks.fes,
            &data.tasks.fes.platform.lists, &bindings, kinds, &data.fes_controls, &data.fes_text)?;
        data.fes_draw.validate_platform_links(&data.tasks.fes.platform, &data.tasks.fes.platform.lists, &bindings)?;
        data.transitions.validate_draw_checkpoint(data.fes_draw.checkpoint_transition_links(), kinds, &self.task_parents)?;
        let snapshot = Snapshot { version: 2,
            text: data.fes_text.capture_checkpoint(&data.message_device, kinds, &self.backlogs.keys().copied().collect())?,
            controls: data.fes_controls.capture_checkpoint(kinds)?, draw: data.fes_draw.capture_checkpoint(kinds)?,
            transitions: data.transitions.capture_checkpoint(kinds, &self.image_handles)?,
            lower: data.tasks.capture_lower_command_state(&data.fes_controls, &data.tasks.fes)?,
        };
        let mut bytes = TAG.to_vec();
        bytes.extend_from_slice(&wire::encode_component(&snapshot)?);
        Ok(bytes)
    }

    pub(super) fn prepare_fes_extension_state(bytes: &[u8], records: &NativeTaskStore,
        tasks: &FesTaskStore, saved: &ObjectGraphSnapshot, images: &BTreeMap<u32, RgbaFrame>,
        options: (i32, bool, u32, i32)) -> Result<PreparedExtensions> {
        let kinds = records.checkpoint_task_kinds();
        let backlogs = saved.backlogs.keys().copied().collect();
        let snapshot: Snapshot = if let Some(bytes) = bytes.strip_prefix(TAG) {
            wire::decode_component(bytes)?
        } else {
            // Older components never admitted lower/transition task records.
            // They cannot provide missing consumers for a newer graph.
            if kinds.values().any(|kind| matches!(kind, 26 | 27 | 41 | 42)) {
                bail!("legacy FES extension checkpoint lacks lower/transition task state");
            }
            let old: Legacy = if bytes.is_empty() {
                let mut text = FesTextStore::default();
                text.restore_checkpoint_options(options);
                Legacy { version: 1, text: text.capture_checkpoint(
                    &crate::message_backend::DesktopMessageDevice::default(), &kinds, &backlogs)?,
                    controls: FesControlStore::default().capture_checkpoint(&kinds)?,
                    draw: FesDrawStore::default().capture_checkpoint(&kinds)? }
            } else { wire::decode_component(bytes)? };
            if old.version != 1 { bail!("unsupported legacy FES extension version"); }
            Snapshot { version: 2, text: old.text, controls: old.controls, draw: old.draw,
                transitions: TransitionStore::default().capture_checkpoint(&kinds, images)?,
                lower: NativeTaskStore::default().capture_lower_command_state(&FesControlStore::default(), tasks)?, }
        };
        if snapshot.version != 2 { bail!("unsupported FES extension component version"); }
        let text = FesTextStore::prepare_checkpoint(&snapshot.text, &kinds, &backlogs)?;
        let controls = FesControlStore::prepare_checkpoint(&snapshot.controls, &kinds)?;
        let draw = FesDrawStore::prepare_checkpoint(&snapshot.draw, &kinds)?;
        controls.validate_checkpoint_links(&kinds, &saved.parents)?;
        draw.validate_checkpoint_links(&kinds, &saved.parents, &saved.wrapper_pixels.keys().copied().collect())?;
        validate_owners(text.store(), &controls, &draw, tasks, &kinds, &saved.parents)?;
        if text.store().checkpoint_options() != options { bail!("FES text component allocator/options copies differ"); }
        let transitions = TransitionStore::prepare_checkpoint(&snapshot.transitions, &kinds, images)?;
        transitions.validate_draw_checkpoint(draw.checkpoint_transition_links(), &kinds, &saved.parents)?;
        let lower = records.prepare_lower_command_state(&snapshot.lower, &controls, tasks)?;
        Ok(PreparedExtensions { text, controls, draw, transitions, lower })
    }
}
