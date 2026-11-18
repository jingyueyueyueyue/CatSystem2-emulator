//! CPU/input platform for type-34 FES tasks. Parsed parameters and live driver
//! expression state are retained throughout; no legacy line splitting is used.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, Color, ImageDrawOptions, Point, Rect, RgbaFrame};
use cs2_runtime::{ResourceIndex, RuntimeError};
use cs2_script::{ExpressionState, FesEvent, NativeReturn, Vm};

use super::fes_driver::{
    FesDriverCommand, FesDriverEffect, FesDriverInput, FesDriverTarget, FesScriptDriver,
};
use super::fes_tasks::{FesListBinding, FesTaskHooks, FesTaskView};
use super::{
    FesMediaLayer, FesPlaneLayer, FesRuntime, InputRegionState, LoggingHost, native_state_error,
    read_u32,
};

#[path = "fes_cpu_state.rs"]
pub(crate) mod checkpoint;
#[path = "fes_platform_state.rs"]
mod state;

#[derive(Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesTaskPlatform {
    #[serde(skip)]
    pub(super) lists: BTreeMap<u32, ImageList>,
    #[serde(skip)]
    pub(crate) save_panels: super::fes_save_panel::FesSavePanelStore,
    objects: BTreeMap<(u32, Vec<u8>, i32), ObjectBinding>,
    #[serde(skip)]
    image_masks: BTreeMap<(u32, String), u8>,
    button_records: BTreeMap<(u32, usize, i32), ButtonRecordState>,
    button_properties: BTreeMap<(u32, Vec<u8>, i32), ButtonProperties>,
    button_source_pending: BTreeSet<(u32, Vec<u8>, i32)>,
    button_regions: BTreeMap<u32, (u32, Vec<u8>, i32)>,
    initialization: BTreeMap<(u32, usize), usize>,
    low_buttons: BTreeMap<u32, u32>,
    released_extensions: BTreeSet<u32>,
    finished_extensions: BTreeSet<u32>,
    background_transfers: BTreeMap<u32, Option<u32>>,
    #[serde(skip)]
    pub(super) extra_effect: Option<super::fes_tasks::FesExtraEffectHandler>,
    #[serde(skip)]
    pub(super) finish_extension: Option<super::fes_tasks::FesFinishHandler>,
    #[serde(skip)]
    pub(super) release_extension: Option<super::fes_tasks::FesOwnerReleaseHandler>,
}

impl FesTaskPlatform {
    pub(super) fn auxiliary_button(&self, owner: u32) -> Option<u32> {
        self.low_buttons.get(&owner).copied()
    }

    pub(crate) fn preserve_checkpoint_callbacks(&mut self, old: &Self) {
        self.extra_effect = old.extra_effect;
        self.finish_extension = old.finish_extension;
        self.release_extension = old.release_extension;
    }

    pub(crate) fn validate_checkpoint(&self, kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>,
        input: &cs2_runtime::message_object::input_snapshot::InputSamplingStateSnapshot) -> Result<()> {
        if !self.save_panels.is_empty()
            || self.objects.values().any(|binding| binding.kind == b"SAVEPANEL") {
            bail!("SAVEPANEL checkpoint needs its dedicated source/CPU/list transaction component");
        }
        self.validate_checkpoint_core(kinds, parents, input)
    }

    /// CPU7 carries the dedicated codec with this exact platform. Independent
    /// platform wire must continue to reject the skipped bank storage.
    pub(crate) fn validate_checkpoint_with_save_panels(&self,
        tasks: &super::fes_tasks::FesTaskStore, kinds: &BTreeMap<u32, u32>,
        parents: &BTreeMap<u32, u32>,
        input: &cs2_runtime::message_object::input_snapshot::InputSamplingStateSnapshot,
    ) -> Result<()> {
        if !std::ptr::eq(self, &tasks.platform) {
            bail!("SAVEPANEL checkpoint proof belongs to another FES platform");
        }
        tasks.validate_checkpoint_save_panels(&self.save_panels)?;
        self.validate_checkpoint_core(kinds, parents, input)
    }

    fn validate_checkpoint_core(&self, kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>,
        input: &cs2_runtime::message_object::input_snapshot::InputSamplingStateSnapshot) -> Result<()> {
        self.save_panels.validate_checkpoint(kinds)?;
        let maskable: BTreeSet<_> = self.objects.iter().filter_map(|((owner, _, _), binding)| {
            matches!(binding.kind.as_slice(), b"IMAGE" | b"BUTTON" | b"CIMAGE")
                .then_some((*owner, binding.key.as_str()))
        }).collect();
        if self.image_masks.iter().any(|((owner, key), channel)| {
            !(1..=4).contains(channel) || !maskable.contains(&(*owner, key.as_str()))
        }) {
            bail!("FES MASK checkpoint has an invalid channel or image binding");
        }
        let owner = |id: &u32| kinds.get(id) == Some(&34);
        if self.objects.len() > 1_000_000 || self.button_records.len() > 1_000_000
            || self.initialization.len() > 1_000_000 || self.low_buttons.len() > 65_536
            || self.button_properties.len() > 1_000_000 || self.button_regions.len() > 1_000_000
            || self.button_source_pending.len() > 1_000_000
            || self.released_extensions.len() > 65_536 || self.finished_extensions.len() > 65_536
            || self.background_transfers.len() > 65_536
            || self.objects.iter().any(|((id, _, _), binding)| !owner(id)
                || binding.child.is_some_and(|child| !kinds.contains_key(&child)))
            || self.button_records.keys().any(|(id, _, _)| !owner(id))
            || self.button_properties.keys().any(|(id, _, _)| !owner(id))
            || self.button_source_pending.iter().any(|key| !self.button_properties.contains_key(key))
            || self.initialization.keys().any(|(id, _)| !owner(id))
            || self.released_extensions.iter().any(|id| !owner(id))
            || self.finished_extensions.iter().any(|id| !owner(id))
            || self.background_transfers.keys().any(|id| !owner(id))
        { bail!("FES platform checkpoint has invalid object owners or counts"); }
        for (&id, &button) in &self.low_buttons {
            if !owner(&id) || kinds.get(&button) != Some(&6) || parents.get(&button) != Some(&id) {
                bail!("FES low-button checkpoint differs from its actual task tree");
            }
        }
        for (region, key) in &self.button_regions {
            // CreateObject publishes the lower task before finish_task_media
            // configures its input region. That budget boundary is saveable.
            if self.objects.get(key).is_none_or(|binding| binding.kind != b"BUTTON")
                || kinds.get(region) != Some(&6)
                || parents.get(region) != Some(&key.0) {
                bail!("FES button checkpoint region has no real lower task/binding");
            }
        }
        // The input component owns registration order, geometry, selected
        // region and dirty epochs. Validate identities without sorting,
        // registering a missing region or refreshing an old hit image.
        for words in &input.task.regions {
            let id = words[0];
            if let Some((id_owner, _)) = self.region_target(id) {
                if !owner(&id_owner) || kinds.get(&id) != Some(&6)
                    || parents.get(&id) != Some(&id_owner) {
                    bail!("FES platform checkpoint input region differs from its actual button owner");
                }
            }
        }
        Ok(())
    }
}

pub(super) struct ImageList {
    pub(super) output: u32,
    pub(super) size: [u32; 2],
    pub(super) offset: [i32; 2],
    pub(super) base_plane: Option<u32>,
    pub(super) enabled: bool,
    pub(super) submit: bool,
    pub(super) force: bool,
    pub(super) layers: BTreeMap<(u32, String), Layer>,
}

pub(super) struct Layer {
    pub(super) frame: RgbaFrame,
    pub(super) options: ImageDrawOptions,
    pub(super) priority: i32,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ObjectBinding {
    key: String,
    kind: Vec<u8>,
    child: Option<u32>,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ButtonProperties {
    enabled: u32,
    syskey: i32,
    raw_type: u32,
    locked_position: Option<[f32; 2]>,
}

impl Default for ButtonProperties {
    fn default() -> Self { Self { enabled: 1, syskey: -1, raw_type: 0, locked_position: None } }
}

impl<'de> cs2_runtime::graph_serde::Deserialize<'de> for ButtonProperties {
    fn deserialize<D: cs2_runtime::graph_serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use cs2_runtime::graph_serde::de::{Error, SeqAccess, Visitor};

        struct ButtonPropertiesVisitor;
        impl<'de> Visitor<'de> for ButtonPropertiesVisitor {
            type Value = ButtonProperties;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("BUTTON properties with an optional locked position")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error> {
                Ok(ButtonProperties {
                    enabled: sequence.next_element()?.ok_or_else(|| A::Error::invalid_length(0, &self))?,
                    syskey: sequence.next_element()?.ok_or_else(|| A::Error::invalid_length(1, &self))?,
                    raw_type: sequence.next_element()?.ok_or_else(|| A::Error::invalid_length(2, &self))?,
                    locked_position: sequence.next_element()?.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_seq(ButtonPropertiesVisitor)
    }
}

#[derive(Clone, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ButtonRecordState {
    #[serde(with = "crate::fes_tasks::state::frame")]
    image: Option<RgbaFrame>,
    source_updated: bool,
}

pub(super) struct FesButtonRecordImage {
    pub(super) region: u32,
    pub(super) raw_type: u32,
    pub(super) source_updated: bool,
    pub(super) image: Option<RgbaFrame>,
}

pub(super) fn button_target_lookup(
    host: &LoggingHost<'_>, region: u32,
) -> Option<(u32, Option<FesDriverTarget>)> {
    host.data_services.tasks.fes.platform.region_target(region)
}

pub(super) fn button_binding_lookup(host: &LoggingHost<'_>, region: u32) -> Option<u32> {
    button_target_lookup(host, region).map(|(owner, _)| owner)
}

pub(super) fn mark_button_source_changed(
    host: &mut LoggingHost<'_>, owner: u32, name: &[u8], bank: i32,
) -> cs2_script::Result<()> {
    let identity = (owner, name.to_vec(), bank);
    if !host.data_services.tasks.fes.platform.button_properties.contains_key(&identity) {
        return Err(native_state_error("BUTTON source request has no live object binding"));
    }
    host.data_services.tasks.fes.platform.button_source_pending.insert(identity);
    Ok(())
}

fn ordered_button_record(
    driver: &FesScriptDriver, record_index: usize,
) -> Option<(usize, Vec<u8>, i32)> {
    driver.adapter.objects.iter().enumerate()
        .filter(|(_, object)| object.definition.kind_name == b"BUTTON")
        .flat_map(|(index, object)| (0..object.definition.bank_count)
            .map(move |bank| (index, object.definition.name.clone(), bank as i32)))
        .nth(record_index)
}

pub(super) fn button_record_lookup(
    host: &LoggingHost<'_>, owner: u32, record_index: usize,
) -> cs2_script::Result<Option<super::message_platform::FesButtonRecord>> {
    let runtime = host.data_services.tasks.fes.runtime(owner)
        .ok_or_else(|| native_state_error("BUTTON records need a loaded FES runtime"))?;
    let driver = runtime.driver.as_ref()
        .ok_or_else(|| native_state_error("BUTTON records need the live FES driver"))?;
    let Some((_, name, bank)) = ordered_button_record(driver, record_index) else { return Ok(None); };
    let identity = (owner, name.clone(), bank);
    let platform = &host.data_services.tasks.fes.platform;
    let binding = platform.objects.get(&identity);
    let properties = platform.button_properties.get(&identity);
    let live = usize::try_from(bank).ok()
        .is_some_and(|bank| driver.adapter.object_bank(&name, bank).is_some());
    Ok(Some(super::message_platform::FesButtonRecord {
        target: FesDriverTarget {
            name, bank, value: binding.and_then(|binding| binding.child).map_or(-1, |id| id as i32),
            default_group: None,
        },
        enabled: if live { properties.map_or(0, |properties| properties.enabled) } else { 0 },
        syskey: properties.map_or(-1, |properties| properties.syskey),
    }))
}

pub(super) fn button_record_image_lookup(
    host: &LoggingHost<'_>, owner: u32, record_index: usize,
) -> cs2_script::Result<Option<FesButtonRecordImage>> {
    let runtime = host.data_services.tasks.fes.runtime(owner)
        .ok_or_else(|| native_state_error("BUTTON image records need a loaded FES runtime"))?;
    let driver = runtime.driver.as_ref()
        .ok_or_else(|| native_state_error("BUTTON image records need the live FES driver"))?;
    let Some((index, name, bank)) = ordered_button_record(driver, record_index) else { return Ok(None); };
    button_record_image(&host.data_services.tasks.fes.platform, owner, index, &name, bank)
        .map(Some)
}

pub(crate) fn bind_save_panel_layers(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    let bindings: Vec<_> = host.data_services.tasks.fes.platform.save_panels.owner_bindings(owner)
        .map(|(name, bank, key)| (name.to_vec(), bank, key.to_owned())).collect();
    let platform = &mut host.data_services.tasks.fes.platform;
    for (name, bank, key) in bindings {
        let identity = (owner, name, bank);
        if let Some(binding) = platform.objects.get(&identity) {
            if binding.kind != b"SAVEPANEL" || binding.key != key || binding.child.is_some() {
                bail!("SAVEPANEL platform identity is owned by another consumer");
            }
        } else {
            platform.objects.insert(identity, ObjectBinding { key, kind: b"SAVEPANEL".to_vec(), child: None });
        }
    }
    Ok(())
}

/// Read the existing object identities; navigation owns no parallel registry.
pub(crate) fn navigation_bindings(
    host: &LoggingHost<'_>, owner: u32,
) -> Vec<(FesDriverTarget, String, u32)> {
    let platform = &host.data_services.tasks.fes.platform;
    platform.objects.iter().filter(|((id, _, _), binding)| *id == owner && binding.kind == b"BUTTON")
        .map(|((_, name, bank), binding)| (FesDriverTarget {
            name: name.clone(), bank: *bank, default_group: None,
            value: binding.child.map_or(-1, |child| child as i32),
        }, binding.key.clone(), platform.button_properties.get(&(owner, name.clone(), *bank))
            .map_or(0, |properties| properties.raw_type)))
        .collect()
}

pub(crate) fn navigation_binding(
    host: &LoggingHost<'_>, owner: u32, name: &[u8], bank: i32,
) -> Option<(FesDriverTarget, String, u32)> {
    navigation_bindings(host, owner).into_iter()
        .find(|(target, _, _)| target.name == name && target.bank == bank)
}

pub(super) fn button_locked_position(
    host: &LoggingHost<'_>, owner: u32, key: &str,
) -> Option<[f32; 2]> {
    let platform = &host.data_services.tasks.fes.platform;
    platform.button_properties.iter().find_map(|(identity, properties)| {
        let position = properties.locked_position?;
        (identity.0 == owner && platform.objects.get(identity).is_some_and(|binding| binding.key == key))
            .then_some(position)
    })
}

fn button_record_image(
    platform: &FesTaskPlatform, owner: u32, index: usize, name: &[u8], bank: i32,
) -> cs2_script::Result<FesButtonRecordImage> {
    let identity = (owner, name.to_vec(), bank);
    let region = platform.objects.get(&identity).and_then(|binding| binding.child)
        .ok_or_else(|| native_state_error("BUTTON image record has no live lower button"))?;
    let record = platform.button_records.get(&(owner, index, bank))
        .ok_or_else(|| native_state_error("BUTTON declaration record has not been created"))?;
    Ok(FesButtonRecordImage {
        region, raw_type: platform.button_properties.get(&identity).map_or(0, |properties| properties.raw_type),
        source_updated: record.source_updated, image: record.image.clone(),
    })
}

impl FesTaskPlatform {
    pub(super) fn remove_task(&mut self, id: u32) {
        self.save_panels.forget_owner(id);
        self.objects.retain(|(owner, _, _), _| *owner != id);
        self.image_masks.retain(|(owner, _), _| *owner != id);
        self.button_records.retain(|(owner, _, _), _| *owner != id);
        self.button_properties.retain(|(owner, _, _), _| *owner != id);
        self.button_source_pending.retain(|(owner, _, _)| *owner != id);
        self.button_regions.retain(|_, (owner, _, _)| *owner != id);
        self.button_regions.remove(&id);
        self.initialization.retain(|(owner, _), _| *owner != id);
        self.low_buttons.remove(&id);
        self.released_extensions.remove(&id);
        self.finished_extensions.remove(&id);
        self.lists.remove(&id);
        for list in self.lists.values_mut() {
            list.layers.retain(|(owner, _), _| *owner != id);
        }
    }

    pub(super) fn list_output(&self, id: u32) -> Option<(u32, [u32; 2], [i32; 2], bool)> {
        self.lists
            .get(&id)
            .map(|list| (list.output, list.size, list.offset, list.submit))
    }

    fn region_target(&self, region: u32) -> Option<(u32, Option<FesDriverTarget>)> {
        if let Some((owner, name, bank)) = self.button_regions.get(&region) {
            return Some((*owner, Some(FesDriverTarget {
                name: name.clone(), bank: *bank, value: region as i32, default_group: None,
            })));
        }
        for ((owner, name, bank), binding) in &self.objects {
            if binding.child == Some(region) && binding.kind == b"BUTTON" {
                return Some((
                    *owner,
                    Some(FesDriverTarget {
                        name: name.clone(),
                        bank: *bank,
                        value: region as i32,
                        default_group: None,
                    }),
                ));
            }
        }
        self.low_buttons
            .iter()
            .find_map(|(&owner, &button)| (button == region).then_some((owner, None)))
    }
}

impl super::fes_tasks::FesTaskStore {
    pub(super) fn install_cpu_hooks(&mut self) {
        self.set_hooks(FesTaskHooks {
            effect: Some(apply_effect),
            prepare: Some(prepare_input),
            bind_list: Some(query_list),
            send: Some(send_message),
            release: Some(release_task),
        });
    }

    /// Call after render_scene clears/draws its background. Each output is the
    /// actual type-15 pixel buffer produced by compose_list at task boundaries.
    pub(super) fn render_cpu_lists(
        &self,
        canvas: &mut Canvas,
        images: &BTreeMap<u32, RgbaFrame>,
    ) -> Result<()> {
        for list in self.platform.lists.values().filter(|list| list.submit) {
            let image = images
                .get(&list.output)
                .context("FES image-list output wrapper was released")?;
            canvas.draw_frame(
                image,
                &ImageDrawOptions::new(Point::new(list.offset[0], list.offset[1])),
            )?;
        }
        Ok(())
    }

    /// `code` is the driver's logical input code (2..21), not a native slot.
    /// The player's real input hit-test provides the region number. Auxiliary
    /// buttons resolve their current object hit from the same runtime pixels.
    pub(super) fn queue_region_input(
        &mut self,
        region: u32,
        code: u32,
        pointer: Option<Point>,
    ) -> cs2_script::Result<bool> {
        let Some((owner, target)) = self.platform.region_target(region) else {
            return Ok(false);
        };
        let input = match target {
            Some(target) => FesDriverInput {
                code,
                target: Some(target),
                event: None,
            },
            None => self
                .runtime(owner)
                .ok_or_else(|| native_state_error("auxiliary FES input has no loaded runtime"))?
                .input_for_driver(code, pointer),
        };
        self.queue_input(owner, input)?;
        Ok(true)
    }

    /// Selection notifications use the explicit event names rather than
    /// pretending they are ordinary mouse button presses.
    pub(super) fn queue_region_focus(
        &mut self,
        region: u32,
        focused: bool,
    ) -> cs2_script::Result<bool> {
        let Some((owner, target)) = self.platform.region_target(region) else {
            return Ok(false);
        };
        let Some(target) = target else {
            return Ok(false);
        };
        self.queue_input(
            owner,
            FesDriverInput {
                code: 0,
                target: Some(target),
                event: Some(if focused {
                    b"FOCUS".to_vec()
                } else {
                    b"UNFOCUS".to_vec()
                }),
            },
        )?;
        Ok(true)
    }
}

impl LoggingHost<'_> {
    pub(super) fn initialize_fes_image_list(&mut self, id: u32, width: u32, height: u32) -> Result<()> {
        if self.scene_nodes.get(&id).is_none_or(|node| node.kind != 15) {
            bail!("image-list initialization requires a live type-15 task");
        }
        let frame = blank(width, height)?;
        super::list_native::clear_entries(self, id)?;
        let size = self.canvas.as_ref().map(|canvas| [canvas.width(), canvas.height()])
            .context("type-15 base plane needs a render context")?;
        let old = self.data_services.tasks.fes.platform.lists.remove(&id);
        let [enabled, submit, force] = self.data_services.list_native.initial_switches(id);
        let (enabled, submit, force, base_plane) = old.as_ref()
            .map(|old| (old.enabled, old.submit, old.force, old.base_plane))
            .unwrap_or((enabled, submit, force, None));
        if let Some(old) = old { self.release_image_wrapper(old.output)?; }
        let output = self.register_image_wrapper(frame, false)?;
        self.data_services.tasks.fes.platform.lists.insert(id, ImageList {
            output, size: [width, height], offset: [0; 2], base_plane,
            enabled, submit, force, layers: BTreeMap::new(),
        });
        if base_plane.is_none() {
            let child = self.create_task_object(2, id, b"base plane", b"")?;
            // Persist ownership before configuring the child, so a later
            // service error cannot allocate another base plane on retry.
            self.data_services.tasks.fes.platform.lists.get_mut(&id).unwrap().base_plane = Some(child);
            let node = self.scene_nodes.get_mut(&child).context("new list base plane disappeared")?;
            node.size = [size[0] as f32, size[1] as f32];
            node.position[2] = 0.0;
            node.visible = false;
            node.initialized = true;
            self.data_services.planes.initialize_list_plane(node, size);
            let base = [child.to_le_bytes(), 1000f32.to_bits().to_le_bytes()].concat();
            self.data_services.planes.observe_existing(88, &base, self.scene_nodes)?;
        }
        Ok(())
    }

    pub(super) fn release_fes_list_output(&mut self, id: u32) -> cs2_script::Result<()> {
        if let Some(list) = self.data_services.tasks.fes.platform.lists.remove(&id) {
            self.release_image_wrapper(list.output)?;
        }
        Ok(())
    }

    /// The native wrappers for the actual type-15 output object used by FES
    /// command 340. These also make the output available to native 261.
    pub(super) fn fes_list_native(
        &mut self,
        index: u32,
        args: &[u8],
    ) -> cs2_script::Result<NativeReturn> {
        self.list_contract_native(index, args)
    }
}

fn query_list(host: &mut LoggingHost<'_>, id: u32) -> Result<FesListBinding> {
    if host.scene_nodes.get(&id).is_none_or(|node| node.kind != 15) {
        bail!("FES command 340 requires a live type-15 image list");
    }
    let list = host
        .data_services
        .tasks
        .fes
        .platform
        .lists
        .get(&id)
        .context("FES command 340 image list has not received native 256/command 285")?;
    let output = host
        .image_handles
        .get(&list.output)
        .context("image-list output wrapper was released")?;
    Ok(FesListBinding {
        task: id,
        clip: [0, 0, output.width() as i32, output.height() as i32],
        offset: list.offset,
    })
}

fn prepare_input(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    _: &mut FesRuntime,
    _: &mut FesScriptDriver,
) -> Result<bool> {
    let size = host
        .canvas
        .as_ref()
        .map(|canvas| [canvas.width(), canvas.height()])
        .context("FES auxiliary button needs a real render context")?;
    let mut priority = 0i32;
    let mut current = Some(view.parent);
    while let Some(id) = current {
        if priority >= 256 {
            bail!("FES button ancestor depth exceeds 256");
        }
        if id == 0 || host.scene_nodes.get(&id).is_some_and(|node| node.kind == 1) {
            break;
        }
        priority += 1;
        current = host.task_parents.get(&id).copied();
    }
    let id = match host
        .data_services
        .tasks
        .fes
        .platform
        .low_buttons
        .get(&view.id)
        .copied()
    {
        Some(id) => id,
        None => {
            let id = host.create_task_object(6, view.id, b"low button", b"")?;
            // Save before registration; retry the same child after a service
            // error instead of reporting an unregistered button as prepared.
            host.data_services
                .tasks
                .fes
                .platform
                .low_buttons
                .insert(view.id, id);
            id
        }
    };
    register_region(
        host,
        id,
        priority,
        [0, 0, size[0] as i32, size[1] as i32],
        true,
    )?;
    if let Some(node) = host.scene_nodes.get_mut(&id) {
        node.initialized = true;
        node.size = [size[0] as f32, size[1] as f32];
    }
    Ok(true)
}

fn send_message(host: &mut LoggingHost<'_>, target: u32, message: [u32; 4]) -> Result<bool> {
    Ok(host.send_fes_task_message(target, message)?)
}

fn release_task(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
) -> Result<bool> {
    release_extensions(host, view.id)?;
    super::fes_save_panel::release_owner(host, view.id, runtime)?;
    super::fes_media::release_image_sources(host, runtime)?;
    let ids: Vec<_> = host
        .data_services
        .tasks
        .fes
        .platform
        .objects
        .iter()
        .filter_map(|((owner, _, _), binding)| {
            (*owner == view.id).then_some(binding.child).flatten()
        })
        .chain(
            host.data_services.tasks.fes.platform.button_regions.iter()
                .filter_map(|(&region, (owner, _, _))| (*owner == view.id).then_some(region)),
        )
        .chain(
            host.data_services
                .tasks
                .fes
                .platform
                .low_buttons
                .get(&view.id)
                .copied(),
        )
        .collect();
    let old_regions = host.input_task.regions.len();
    host.input_task
        .regions
        .retain(|region| !ids.contains(&region.id()));
    if old_regions != host.input_task.regions.len() { host.mark_input_geometry_dirty(); }
    for list in host.data_services.tasks.fes.platform.lists.values_mut() {
        list.layers.retain(|(owner, _), _| *owner != view.id);
    }
    if let Some(binding) = &view.list {
        if let Some(list) = host
            .data_services
            .tasks
            .fes
            .platform
            .lists
            .get_mut(&binding.task)
        {
            list.submit = true;
            list.enabled = true;
            compose_list(host, binding.task, true)?;
        }
    }
    Ok(true)
}

pub(super) fn release_reload_objects(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    release_extensions(host, owner)?;
    let children: Vec<_> = host
        .data_services
        .tasks
        .fes
        .platform
        .objects
        .iter()
        .filter(|((id, _, _), _)| *id == owner)
        .filter_map(|(_, binding)| binding.child)
        .chain(host.data_services.tasks.fes.platform.button_regions.iter()
            .filter_map(|(&region, (id, _, _))| (*id == owner).then_some(region)))
        .collect();
    for &child in &children {
        host.request_ordinary_task_delete(child)?;
    }
    let old_regions = host.input_task.regions.len();
    host.input_task
        .regions
        .retain(|region| !children.contains(&region.id()));
    if old_regions != host.input_task.regions.len() { host.mark_input_geometry_dirty(); }
    host.data_services
        .tasks
        .fes
        .platform
        .objects
        .retain(|(id, _, _), _| *id != owner);
    host.data_services.tasks.fes.platform.button_records.retain(|(id, _, _), _| *id != owner);
    host.data_services.tasks.fes.platform.image_masks.retain(|(id, _), _| *id != owner);
    host.data_services.tasks.fes.platform.button_properties.retain(|(id, _, _), _| *id != owner);
    host.data_services.tasks.fes.platform.button_source_pending.retain(|(id, _, _)| *id != owner);
    host.data_services.tasks.fes.platform.button_regions.retain(|_, (id, _, _)| *id != owner);
    host.data_services
        .tasks
        .fes
        .platform
        .initialization
        .retain(|(id, _), _| *id != owner);
    for list in host.data_services.tasks.fes.platform.lists.values_mut() {
        list.layers.retain(|(id, _), _| *id != owner);
    }
    host.data_services.tasks.fes.platform.save_panels.forget_owner(owner);
    // The next loaded program may create a fresh set of extension objects.
    host.data_services.tasks.fes.platform.released_extensions.remove(&owner);
    host.data_services.tasks.fes.platform.finished_extensions.remove(&owner);
    Ok(())
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesBankRelease {
    pub(super) name: Vec<u8>,
    pub(super) bank: i32,
    pub(super) kind: Vec<u8>,
    motion: super::fes_motion::MotionReleaseIdentity,
    key: Option<String>,
    tasks: Vec<u32>,
    #[serde(default)]
    models: Vec<super::css_model::ModelTaskRelease>,
    layers: Vec<String>,
    lists: Vec<u32>,
    button_records: Vec<(usize, i32)>,
}

impl FesBankRelease {
    pub(super) fn is_sound(&self) -> bool {
        matches!(self.kind.as_slice(), b"SOUND" | b"SNDTABLE")
    }
}

/// Validate every dedicated consumer before starting a mixed-kind deletion.
/// Sound storage stays with the caller, including when it is moved out of host.
pub(super) fn plan_bank_release(
    host: &mut LoggingHost<'_>, runtime: &FesRuntime, driver: &FesScriptDriver,
    owner: u32, name: &[u8], bank: i32,
) -> Result<Vec<FesBankRelease>> {
    let identities = super::fes_motion::plan_release_subtree(host, owner, name, bank)?;
    let mut plan = Vec::with_capacity(identities.len());
    for motion in identities {
        let kind = driver.adapter.object_bank_definition(&motion.name, motion.bank as usize)
            .context("FES delete has a motion identity without a live adapter binding")?
            .kind_name.clone();
        let identity = (owner, motion.name.clone(), motion.bank);
        let mut item = FesBankRelease {
            name: motion.name.clone(), bank: motion.bank, kind,
            motion, key: None, tasks: Vec::new(), layers: Vec::new(),
            lists: Vec::new(), button_records: Vec::new(), models: Vec::new(),
        };
        match item.kind.as_slice() {
            b"SOUND" | b"SNDTABLE" | b"BASE" => {}
            b"PARTICLE" => {
                item.tasks = super::fes_particle::bank_release_targets(
                    &host.data_services.fes_particles, owner, &item.name, item.bank,
                ).context("FES delete particle bank is unavailable")?;
            }
            kind if super::fes_text::is_text_kind(kind) => {
                item.layers = super::fes_text::bank_release_layers(
                    &host.data_services.fes_text, owner, &item.name, item.bank, &item.kind,
                )?;
            }
            b"SAVEPANEL" => {
                item.layers = host.data_services.tasks.fes.platform.save_panels
                    .bank_release_layers(owner, &item.name, item.bank)?;
                let key = item.layers.first().context("SAVEPANEL release has no actual layer")?;
                if !runtime.media.get(key).is_some_and(|layer| layer.kind == "SAVEPANEL") {
                    bail!("SAVEPANEL release CPU layer identity differs");
                }
            }
            b"DRAW" | b"WIPE" if matches!(item.name.as_slice(), b"__drawobj__" | b"__wipeobj__") => {
                if !super::fes_draw::has_owner(&host.data_services.fes_draw, owner) {
                    bail!("FES DRAW delete has no live snapshot/transition owner");
                }
            }
            b"IMAGE" | b"CIMAGE" | b"DIFIMAGE" | b"BUTTON" | b"CPLANE" | b"PLANE" => {
                let platform = &host.data_services.tasks.fes.platform;
                let binding = platform.objects.get(&identity)
                    .context("FES delete has no live graphics binding")?;
                if binding.kind != item.kind {
                    bail!("FES delete graphics binding differs from its original declaration");
                }
                let plane = matches!(item.kind.as_slice(), b"CPLANE" | b"PLANE");
                if plane && !runtime.planes.contains_key(&binding.key)
                    || item.kind != b"PLANE" && !runtime.media.contains_key(&binding.key) {
                    bail!("FES delete graphics consumer state is absent");
                }
                item.key = Some(binding.key.clone());
                item.layers.push(binding.key.clone());
                item.tasks.extend(binding.child);
                item.tasks.extend(platform.button_regions.iter().filter_map(|(&region, key)|
                    (key == &identity).then_some(region)));
                if item.kind == b"BUTTON" {
                    item.button_records = driver.adapter.objects.iter().enumerate()
                        .filter(|(_, object)| object.definition.kind_name == b"BUTTON"
                            && object.definition.name == item.name)
                        .map(|(record, _)| (record, item.bank)).collect();
                }
            }
            b"SLIDER" | b"SCROLL2" | b"IMGWND" | b"VIEWPORT" => {
                let (tasks, key) = super::fes_controls::bank_release_targets(
                    &host.data_services.fes_controls, owner, &item.name, item.bank,
                ).context("FES delete control bank is unavailable or executing in another owner")?;
                item.tasks = tasks;
                item.layers.push(key);
            }
            _ => bail!("FES mixed delete needs the actual {:?} bank release adapter",
                String::from_utf8_lossy(&item.kind)),
        }
        item.tasks.sort_unstable();
        item.tasks.dedup();
        item.models = super::css_model::plan_model_descendants(host, &item.tasks)?;
        item.lists = host.data_services.tasks.fes.platform.lists.iter()
            .filter_map(|(&id, list)| item.layers.iter().any(|key|
                list.layers.contains_key(&(owner, key.clone()))).then_some(id)).collect();
        plan.push(item);
    }
    Ok(plan)
}

/// False is an already committed identity; a reused name is an error.
pub(super) fn validate_bank_release(
    host: &LoggingHost<'_>, item: &FesBankRelease,
) -> Result<bool> {
    super::fes_motion::validate_release_identity(&host.data_services.fes_motion, &item.motion)
}

/// Call only after the dedicated consumer has accepted its real destruction.
/// This does not inspect or mutate FesSoundStore.
pub(super) fn finish_bank_release(
    host: &mut LoggingHost<'_>, driver: &mut FesScriptDriver, item: &FesBankRelease,
) -> Result<()> {
    super::fes_motion::commit_release_identity(&mut host.data_services.fes_motion, &item.motion)?;
    driver.adapter.remove_object_bank(&item.name, item.bank as usize);
    if item.kind == b"BUTTON"
        && let Ok(name) = std::str::from_utf8(&item.name)
        && let std::collections::btree_map::Entry::Occupied(mut binding) =
            driver.interpreter.named.entry(name.to_owned())
    {
        binding.get_mut().remove(&item.bank);
        if binding.get().is_empty() {
            binding.remove();
        }
    }
    Ok(())
}

/// Release F-owned consumers, then their motion and original adapter binding.
/// Keep the same planned item on error; completed requests are idempotent.
pub(super) fn release_graphics_bank(
    host: &mut LoggingHost<'_>, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, item: &FesBankRelease,
) -> Result<()> {
    if item.is_sound() { bail!("SOUND deletion must use the caller's live sound store"); }
    if !validate_bank_release(host, item)? { return Ok(()); }
    let owner = item.motion.owner;
    if item.kind == b"DRAW" {
        super::fes_draw::release_owner(host, owner)?;
    }
    if item.kind == b"PARTICLE" {
        super::fes_particle::release_bank(host, owner, &item.name, item.bank)?;
    }
    super::css_model::request_model_descendants(host, &item.models)?;
    for &task in &item.tasks {
        host.request_ordinary_task_delete(task)?;
    }
    for &list_id in &item.lists {
        let Some(list) = host.data_services.tasks.fes.platform.lists.get_mut(&list_id) else {
            continue;
        };
        for key in &item.layers { list.layers.remove(&(owner, key.clone())); }
        if list.enabled { compose_list(host, list_id, false)?; }
    }
    let old_regions = host.input_task.regions.len();
    host.input_task.regions.retain(|region| !item.tasks.contains(&region.id()));
    if old_regions != host.input_task.regions.len() { host.mark_input_geometry_dirty(); }
    if host.input_task.selected_region.is_some_and(|id| item.tasks.contains(&id)) {
        host.input_task.selected_region = None;
    }
    for task in &item.tasks {
        if let Some(node) = host.scene_nodes.get_mut(task) {
            node.visible = false;
            node.initialized = false;
        }
    }
    let identity = (owner, item.name.clone(), item.bank);
    let platform = &mut host.data_services.tasks.fes.platform;
    platform.objects.remove(&identity);
    if let Some(key) = &item.key {
        platform.image_masks.remove(&(owner, key.clone()));
    }
    platform.button_properties.remove(&identity);
    platform.button_source_pending.remove(&identity);
    platform.button_regions.retain(|_, key| key != &identity);
    for &(record, bank) in &item.button_records {
        platform.button_records.remove(&(owner, record, bank));
    }
    if let Some(key) = &item.key {
        if let Some(layer) = runtime.media.get_mut(key) {
            super::fes_media::release_image_source(host, layer)?;
        }
        runtime.release_driver_media_bank(key);
    }
    super::fes_controls::forget_released_bank(
        &mut host.data_services.fes_controls, owner, &item.name, item.bank,
    );
    if item.kind == b"PARTICLE" {
        super::fes_particle::forget_released_bank(
            &mut host.data_services.fes_particles, owner, &item.name, item.bank,
        );
    }
    if super::fes_text::is_text_kind(&item.kind) {
        super::fes_text::release_bank(host, owner, &item.name, item.bank)?;
    }
    if item.kind == b"SAVEPANEL" {
        super::fes_save_panel::release_bank(host, owner, &item.name, item.bank, runtime)?;
    }
    finish_bank_release(host, driver, item)
}

fn graphics_delete_command(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, command: &FesDriverCommand, shared: &mut ExpressionState,
) -> Result<bool> {
    if !command.parameters.get(1).is_some_and(|verb| verb.eq_ignore_ascii_case(b"delete")) {
        return Ok(false);
    }
    let source = object_command_target(command)?;
    let (name, index) = super::fes_media::split_target(&source, host.text_encoding)?;
    let Some(object) = driver.adapter.object(name).filter(|object| matches!(
        object.definition.kind_name.as_slice(), b"PLANE" | b"CPLANE" | b"IMAGE" | b"CIMAGE" | b"BUTTON",
    )) else { return Ok(false); };
    let count = object.definition.bank_count as i32;
    let bank = index.map(|source| driver.evaluate_parameter(source, command.context.current, shared)).transpose()?;
    let banks = match bank {
        None | Some(-1) => 0..count,
        Some(bank) if (0..count).contains(&bank) => bank..bank + 1,
        _ => return Ok(true),
    };
    let mut plan = Vec::new();
    for bank in banks {
        if driver.adapter.object_bank(name, bank as usize).is_none() { break; }
        let items = plan_bank_release(host, runtime, driver, view.id, name, bank)?;
        if items.is_empty() { bail!("FES graphics delete has no shared motion identity"); }
        if items.iter().any(FesBankRelease::is_sound) {
            bail!("FES graphics delete with SOUND descendants requires the shared sound release dispatcher");
        }
        plan.extend(items);
    }
    for item in &plan {
        release_graphics_bank(host, runtime, driver, item)?;
    }
    Ok(true)
}

fn release_extensions(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    if let Some(wrapper) = host.data_services.tasks.fes.platform.background_transfers.get(&owner).copied() {
        if let Some(wrapper) = wrapper { host.release_image_wrapper(wrapper)?; }
        host.data_services.tasks.fes.platform.background_transfers.remove(&owner);
    }
    if !host.data_services.tasks.fes.platform.released_extensions.contains(&owner) {
        if let Some(release) = host.data_services.tasks.fes.platform.release_extension {
            release(host, owner)?;
        }
        // Commit only after success. A later CPU/list failure resumes after
        // this completed callback; an extension error retries its own cursor.
        host.data_services.tasks.fes.platform.released_extensions.insert(owner);
    }
    Ok(())
}

fn apply_effect(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    effect: &FesDriverEffect,
    shared: &mut ExpressionState,
) -> Result<bool> {
    if let FesDriverEffect::Command(command) = effect
        && button_property_command(host, view, runtime, driver, command, shared)? {
        return Ok(true);
    }
    if let FesDriverEffect::Command(command) = effect
        && super::fes_driver::main_flow::object_command(host, view, runtime, driver, command, shared)? {
        return Ok(true);
    }
    if matches!(effect,FesDriverEffect::Command(command)
        if command.parameters.first().is_some_and(|verb| verb.eq_ignore_ascii_case(b"baseimg")))
        && (host.data_services.tasks.fes.platform.background_transfers.contains_key(&view.id)
            || runtime.media.contains_key("\0driver.baseimg")) {
        return commit_background(host,view,runtime);
    }
    if let Some(extra) = host.data_services.tasks.fes.platform.extra_effect {
        if let Some(done) = extra(host, view, runtime, driver, effect, shared)? {
            return Ok(done);
        }
    }
    match effect {
        FesDriverEffect::Command(command) => {
            if graphics_delete_command(host, view, runtime, driver, command, shared)? {
                return Ok(true);
            }
            let verb = command
                .parameters
                .first()
                .map(Vec::as_slice)
                .unwrap_or_default();
            if verb == b"keyskip" {
                return Ok(true);
            }
            if super::fes_text::apply_name_image_table(
                runtime, driver, command, shared, host.text_encoding,
            )? {
                return Ok(true);
            }
            if super::fes_media::apply_image_source_command(host, runtime, driver, command, shared)? {
                return Ok(true);
            }
            if super::fes_media::apply_difimage_reference(host, runtime, driver, command, shared)? {
                return Ok(true);
            }
            if super::fes_media::apply_difimage_attribute(host, runtime, driver, command, shared)? {
                return Ok(true);
            }
            let previous_sources: Vec<_> = host.data_services.tasks.fes.platform.objects.iter()
                .filter(|((owner, _, _), binding)| *owner == view.id && binding.kind == b"BUTTON")
                .filter_map(|(identity, binding)| runtime.media.get(&binding.key)
                    .map(|layer| (identity.clone(), binding.key.clone(),
                        (layer.file.clone(), layer.selected_id, layer.clip, layer.image.clone()))))
                .collect();
            if runtime.apply_driver_media_encoded(
                command,
                driver,
                shared,
                host.resources,
                view.now as u64,
                host.events,
                host.text_encoding,
            )? {
                for (identity, key, previous) in previous_sources {
                    if runtime.media.get(&key).is_some_and(|layer|
                        previous != (layer.file.clone(), layer.selected_id, layer.clip, layer.image.clone())) {
                        host.data_services.tasks.fes.platform.button_source_pending.insert(identity);
                    }
                }
                if verb.eq_ignore_ascii_case(b"baseimg") && !driver.has_pending_handler() {
                    commit_background(host,view,runtime)?;
                }
                return Ok(true);
            }
            if let Some(name) = command.parameters.first() {
                let (name, _) = target(name)?;
                if driver.adapter.object(name).is_none()
                    && !matches!(
                        verb,
                        b"draw"
                            | b"decode"
                            | b"save"
                            | b"load"
                            | b"reset"
                            | b"play"
                            | b"stop"
                            | b"caption"
                            | b"preload"
                    )
                {
                    driver.fallback_command(command, shared)?;
                    return Ok(true);
                }
            }
        }
        FesDriverEffect::Initialize {
            event: FesEvent::CreateObject(definition),
            ..
        } => {
            create_objects(host, view, runtime, driver, definition)?;
            return Ok(true);
        }
        FesDriverEffect::Initialize {
            event: FesEvent::InitializeObject(initializer),
            ..
        } => {
            initialize_objects(host, view, runtime, driver, initializer, shared)?;
            return Ok(true);
        }
        FesDriverEffect::Initialize {
            event: FesEvent::Declarations(sections),
            ..
        } => {
            for section in sections {
                let kind = section.header.split(u8::is_ascii_whitespace).next().unwrap_or_default();
                if kind.eq_ignore_ascii_case(b"INCLUDE") && !section.lines.is_empty() {
                    // Compiled includes must already be expanded. Reading raw
                    // sources here would change macro scope/control offsets.
                    bail!("runtime FES INCLUDE needs the source inclusion service");
                }
                if kind.eq_ignore_ascii_case(b"PRELOAD") {
                    for line in &section.lines {
                        let name = host.text_encoding.decode(&line.bytes);
                        read_fes_preload_image(host.resources, &name)?;
                    }
                }
            }
            return Ok(true);
        }
        FesDriverEffect::Initialize {
            event: FesEvent::Subcommands(table),
            ..
        } => {
            for diagnostic in &table.diagnostics {
                host.events.push(super::LogicEvent {
                    frame: host.frame,
                    kind: "fes-subcommand-diagnostic".into(),
                    detail: format!("task={} {diagnostic}", view.id),
                });
            }
            return Ok(true);
        }
        FesDriverEffect::Initialize {
            event: FesEvent::KeyBlocks(_) | FesEvent::KeyBlockBinding(_),
            ..
        } => return Ok(true),
        FesDriverEffect::ObjectEvent { .. } | FesDriverEffect::SystemBlock { .. } => {
            // The driver already changed interpreter control and named input.
            // Actual display/input properties are synchronized at the boundary.
            return Ok(true);
        }
        FesDriverEffect::Waiting {
            wait: super::fes_driver::FesDriverWait::Control { kind, .. },
            ..
        } if kind.is_empty() => {
            let active = runtime.media.values().any(|layer| {
                layer.fade.is_some()
                    || layer
                        .animation
                        .as_ref()
                        .and_then(|animation| animation.timeline())
                        .is_some_and(|timeline| !timeline.is_finished())
            });
            if !active {
                driver.resume_control(kind);
            }
            return Ok(true);
        }
        FesDriverEffect::React { target } => {
            if let Some(target) = target {
                let binding = host
                    .data_services
                    .tasks
                    .fes
                    .platform
                    .objects
                    .get(&(view.id, target.name.clone(), target.bank))
                    .context("FES react target is not registered")?;
                if let Some(id) = binding.child {
                    if let Some(region) = host
                        .input_task
                        .regions
                        .iter_mut()
                        .find(|region| region.id() == id)
                    {
                        region.words[7] = 1;
                        host.input_task.selected_region = Some(id);
                    }
                }
            }
            return Ok(true);
        }
        _ => {}
    }
    bail!(
        "FES task {} has no implemented platform service for {effect:?}",
        view.id
    )
}

fn read_fes_preload_image(resources: &ResourceIndex, name: &str) -> Result<Option<Vec<u8>>> {
    match resources.read_category(b"image", name) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(RuntimeError::ResourceNotFound(_)) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("FES PRELOAD image {name:?}")),
    }
}

#[cfg(test)]
#[test]
fn difimage_refex_projects_and_releases_independent_resources() -> Result<()> {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!("cs2-fes-difimage-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&root)?;
    let player = super::HeadlessPlayer::open_with_save_root(&root, root.join("profile"));
    std::fs::remove_dir(&root)?;
    let mut player = player?;
    player.with_host(|host| {
        let parent = host.current_task_id;
        let id = host.create_task_object(34, parent, b"meswnd", b"")?;
        let list = host.create_task_object(15, parent, b"test_list", b"")?;
        host.initialize_fes_image_list(list, 16, 16)?;
        let view = FesTaskView { id, parent, context: Vec::new(), now: 0, paused: 0,
            list: Some(FesListBinding { task: list, clip: [0, 0, 16, 16], offset: [0; 2] }) };
        let sections: [(&[u8], &[u8]); 3] = [
            (b"DEFINE", b"DIFIMAGE face[3]"),
            (b"OBJECT POS DISP PRI", b"face 6,7 1 150"),
            (b"START", b"exit 0"),
        ];
        let document = cs2_script::FesDocument {
            sections: sections.into_iter().enumerate().map(|(index, (header, line))| cs2_script::FesSection {
                header: header.to_vec(),
                lines: vec![cs2_script::FesLine { offset: index * 100 + 50, bytes: line.to_vec() }],
            }).collect(),
        };
        let mut driver = FesScriptDriver::new(&document)?;
        let mut runtime = FesRuntime::new(document);
        runtime.media.clear();
        let mut shared = ExpressionState::default();
        while let Some(effect) = driver.advance_initialization(&mut shared)? {
            super::fes_motion::observe(host, &view, &driver, &effect)?;
            assert!(apply_effect(host, &view, &mut runtime, &mut driver, &effect, &mut shared)?);
        }
        assert_eq!(runtime.media.len(), 3);
        assert!(host.data_services.tasks.fes.platform.objects.values().all(|object| object.child.is_none()));
        assert!(super::fes_media::source_image(host, &runtime.media["face[1]"])?.is_none());

        let first = [255, 44, 23, 255];
        let second = [11, 90, 242, 255];
        for (handle, pixel, base) in [(901, first, [2, 3]), (902, second, [-2, 1])] {
            let image = RgbaFrame::from_rgba8(1, 1, pixel.to_vec())?;
            let mut geometry = super::image_manager::ResourceGeometry::from_frame(&image);
            geometry.base = base;
            host.image_resources.insert(handle, super::DesktopImageResource {
                image, geometry, owner: Some(parent), references: 1, has_source_layer: false,
                updated: false, uses_alpha: true, auxiliary: 0,
                attributes: BTreeMap::from([
                    (99, [base[0] as u32, base[1] as u32, 0, 0, 0]),
                    (101, [1, 2, 3, 4, 0xff80_1020]),
                ]),
            });
        }
        let mut apply = |host: &mut LoggingHost<'_>, runtime: &mut FesRuntime,
                         driver: &mut FesScriptDriver, parameters: &[&[u8]]| -> Result<[i32; 6]> {
            let command = FesDriverCommand { offset: 0, source: Vec::new(),
                parameters: parameters.iter().map(|parameter| parameter.to_vec()).collect(),
                context: driver.interpreter.context(&shared), object_event: None };
            assert!(apply_effect(host, &view, runtime, driver, &FesDriverEffect::Command(command), &mut shared)?);
            Ok(shared.shared[850..856].try_into()?)
        };
        let key = (id, "face[1]".to_owned());
        apply(host, &mut runtime, &mut driver, &[b"face[0+1]", b"refex", b"901"])?;
        apply(host, &mut runtime, &mut driver, &[b"face[1]", b"refex", b"901"])?;
        assert_eq!(apply(host, &mut runtime, &mut driver,
            &[b"face[0+1]", b"attr", b"98+1", b"850", b"851"])?, [2, 3, 0, 0, 0, 0]);
        let expected = [1, 2, 3, 4, 0xff80_1020u32 as i32, 0];
        assert_eq!(apply(host, &mut runtime, &mut driver,
            &[b"face[1]", b"attr", b"65637", b"850", b"851", b"852", b"853", b"854"])?, expected);
        assert_eq!(apply(host, &mut runtime, &mut driver,
            &[b"face[1]", b"attr", b"100", b"850", b"851"])?, expected);
        assert_eq!(apply(host, &mut runtime, &mut driver,
            &[b"face[0]", b"attr", b"99", b"850", b"851"])?, expected);
        assert_eq!(host.image_resources[&901].references, 2);
        assert!(runtime.media[&key.1].source.validate_checkpoint().is_err());
        assert!(finish_task_media(host, &view, &mut runtime, &mut driver)?);
        let layers = &host.data_services.tasks.fes.platform.lists[&list].layers;
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[&key].frame.pixels(), &first);
        assert_eq!(layers[&key].options.position, Point::new(4, 4));

        assert!(apply(host, &mut runtime, &mut driver, &[b"face[1]", b"refex", b"999"]).is_err());
        assert_eq!(host.image_resources[&901].references, 2);
        apply(host, &mut runtime, &mut driver, &[b"face[1]", b"refex", b"902"])?;
        assert_eq!(apply(host, &mut runtime, &mut driver,
            &[b"face[-1]", b"attr", b"99", b"850", b"$850+853"])?,
            [-2, 2, 3, 4, 1, 0]);
        assert_eq!(host.image_resources[&901].references, 1);
        assert!(finish_task_media(host, &view, &mut runtime, &mut driver)?);
        let layers = &host.data_services.tasks.fes.platform.lists[&list].layers;
        assert_eq!(layers[&key].frame.pixels(), &second);
        assert_eq!(layers[&key].options.position, Point::new(8, 6));

        apply(host, &mut runtime, &mut driver, &[b"face[1]", b"refex"])?;
        assert_eq!(host.image_resources[&902].references, 1);
        assert_eq!(runtime.media[&key.1].image_base, [-2, 1]);
        assert!(finish_task_media(host, &view, &mut runtime, &mut driver)?);
        assert!(host.data_services.tasks.fes.platform.lists[&list].layers.is_empty());

        apply(host, &mut runtime, &mut driver, &[b"face[-1]", b"refex", b"902"])?;
        assert_eq!(host.image_resources[&902].references, 4);
        let items = plan_bank_release(host, &runtime, &driver, id, b"face", 1)?;
        for item in &items {
            release_graphics_bank(host, &mut runtime, &mut driver, item)?;
        }
        assert_eq!(host.image_resources[&902].references, 3);
        host.image_resources.get_mut(&902).unwrap().references -= 1;
        assert!(release_task(host, &view, &mut runtime)?);
        assert!(!host.image_resources.contains_key(&902));
        Ok(())
    })
}

#[cfg(test)]
#[test]
fn fes_preload_routes_literal_names_and_allows_misses() {
    struct Fixture(std::path::PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            for file in ["startup.xml", "image/window art.hg3", "image/empty.hg3"] {
                let _ = std::fs::remove_file(self.0.join(file));
            }
            let _ = std::fs::remove_dir(self.0.join("image"));
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("cs2-fes-preload-{}-{unique}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let fixture = Fixture(root);
    std::fs::create_dir(fixture.0.join("image")).unwrap();
    std::fs::write(fixture.0.join("startup.xml"), b"<document><INT><int00><name>image</name><int>image.int</int><ext>hg3</ext><dir>image</dir></int00></INT></document>").unwrap();
    std::fs::write(fixture.0.join("image/window art.hg3"), b"raw bytes").unwrap();
    std::fs::write(fixture.0.join("image/empty.hg3"), []).unwrap();
    let resources = ResourceIndex::scan(&fixture.0).unwrap();

    assert_eq!(read_fes_preload_image(&resources, "$str900").unwrap(), None);
    assert_eq!(read_fes_preload_image(&resources, "window art").unwrap(), Some(b"raw bytes".to_vec()));
    assert_eq!(read_fes_preload_image(&resources, "image.int/window art.hg3").unwrap(), Some(b"raw bytes".to_vec()));
    assert_eq!(read_fes_preload_image(&resources, "empty").unwrap(), Some(Vec::new()));
    assert!(read_fes_preload_image(&resources, &"x".repeat(260)).is_err());
}

fn object_command_target(command: &FesDriverCommand) -> Result<Vec<u8>> {
    if command.parameters.first().is_some_and(|target| target == b"@") {
        Ok(command.object_event.as_ref().context("FES @ has no event object")?.parameter())
    } else {
        Ok(command.parameters.first().cloned().unwrap_or_default())
    }
}

fn button_property_command(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, command: &FesDriverCommand, shared: &mut ExpressionState,
) -> Result<bool> {
    let Some(verb) = command.parameters.get(1) else { return Ok(false); };
    let lock = verb.eq_ignore_ascii_case(b"lock");
    if !lock && !verb.eq_ignore_ascii_case(b"enable") {
        return Ok(false);
    }
    let source = object_command_target(command)?;
    let (name, bank) = super::fes_media::split_target(&source, host.text_encoding)?;
    let Some(definition) = driver.adapter.object(name)
        .filter(|object| object.definition.kind_name == b"BUTTON") else { return Ok(false); };
    let count = definition.definition.bank_count;
    let bank = bank.map(|bank| driver.evaluate_parameter(bank, command.context.current, shared))
        .transpose()?;
    let banks: Vec<_> = match bank {
        None | Some(-1) => (0..count as i32).collect(),
        Some(bank) if bank >= 0 => vec![bank],
        _ => Vec::new(),
    };
    if lock {
        let value = command.parameters.get(2).map(|source| driver.evaluate_parameter(source, 0, shared))
            .transpose()?.unwrap_or(0);
        for bank in banks {
            if driver.adapter.object_bank(name, bank as usize).is_none() { continue; }
            let identity = (view.id, name.to_vec(), bank);
            let platform = &host.data_services.tasks.fes.platform;
            let key = &platform.objects.get(&identity).context("BUTTON lock binding is absent")?.key;
            let old = platform.button_properties.get(&identity)
                .context("BUTTON lock properties are absent")?.locked_position;
            let position = match (value, old) {
                (0, _) => None,
                (_, Some(position)) => Some(position),
                (_, None) => Some(super::fes_motion::button_unlocked_coordinates(host, view.id, runtime, key)?),
            };
            host.data_services.tasks.fes.platform.button_properties.get_mut(&identity).unwrap()
                .locked_position = position;
        }
        return Ok(true);
    }
    let first = banks.first().and_then(|&bank|host.data_services.tasks.fes.platform.button_properties
        .get(&(view.id, name.to_vec(), bank)));
    let Some(first) = first else {
        driver.fallback_command(command, shared)?;
        return Ok(true);
    };
    let current = first.enabled as i32;
    let value = command.parameters.get(2).map(|source| driver.evaluate_parameter(source, current, shared))
        .transpose()?.unwrap_or(0);
    let value = u32::from(value != 0);
    for bank in banks {
        if driver.adapter.object_bank(name, bank as usize).is_none() { break; }
        let identity = (view.id, name.to_vec(), bank);
        let Some(binding) = host.data_services.tasks.fes.platform.objects.get(&identity) else { break; };
        let key = binding.key.clone();
        host.data_services.tasks.fes.platform.button_properties.get_mut(&identity)
            .context("BUTTON live properties are absent")?.enabled = value;
        runtime.media.get_mut(&key).context("BUTTON enable image is absent")?.enabled = value != 0;
    }
    Ok(true)
}

pub(super) fn commit_background(host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime) -> Result<bool> {
    const PENDING: &str = "\0driver.baseimg.pending";
    const BACKGROUND: &str = "\0driver.baseimg";
    if !host.data_services.tasks.fes.platform.background_transfers.contains_key(&view.id) {
        let pending = runtime.media.get(PENDING);
        if pending.is_none() && !runtime.media.contains_key(BACKGROUND) {return Ok(true);}
        let list = view.list.as_ref().context("FES baseimg needs a real bound image list")?.task;
        let plane = pending.is_some_and(|pending|pending.kind == "DRIVER_PLANE_BACKGROUND");
        let snapshot = if plane {
            compose_list(host,list,false)?;
            let output = host.data_services.tasks.fes.platform.lists[&list].output;
            Some(host.image_handles.get(&output).cloned().context("baseimg list output disappeared")?)
        } else {None};
        if pending.is_some() { runtime.commit_driver_background(snapshot.as_ref())?; }
        let image = runtime.media.get(BACKGROUND).and_then(|layer|layer.image.as_ref()).cloned();
        let wrapper = image.map(|image|host.register_image_wrapper(image,false)).transpose()?;
        host.data_services.tasks.fes.platform.background_transfers.insert(view.id,wrapper);
        runtime.media.remove(BACKGROUND);
    }
    let list = view.list.as_ref().context("FES baseimg lost its image-list binding")?.task;
    let wrapper = host.data_services.tasks.fes.platform.background_transfers[&view.id];
    let mut output = 0;
    let result = super::list_native::receive(host, cs2_runtime::TaskCommand {
        target:list as i32,sender:view.id as i32,code:302,data:[0,wrapper.unwrap_or(0),0],
    }, &mut super::OwnedTaskBuffers::default(), &mut output,4096)?;
    if result.is_none() {bail!("FES baseimg list receiver is unavailable");}
    if let Some(wrapper) = wrapper {host.release_image_wrapper(wrapper)?;}
    host.data_services.tasks.fes.platform.background_transfers.remove(&view.id);
    Ok(true)
}

fn create_objects(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    definition: &cs2_script::FesObjectDefinition,
) -> Result<()> {
    let declaration = driver.adapter.objects.iter().rposition(|object|
        object.definition.name == definition.name
            && object.definition.kind_name == definition.kind)
        .context("FES creation has no original declaration record")?;
    let count = if definition.kind == b"BUTTON" {
        driver.adapter.objects[declaration].definition.bank_count
    } else { driver
        .adapter
        .object(&definition.name)
        .context("FES definition lacks adapter metadata")?
        .definition
        .bank_count };
    let name = host.text_encoding.decode(&definition.name);
    for bank in 0..count {
        let identity = (view.id, definition.name.clone(), bank as i32);
        if host
            .data_services
            .tasks
            .fes
            .platform
            .objects
            .contains_key(&identity)
        {
            if definition.kind == b"BUTTON"
                && !host.data_services.tasks.fes.platform.button_records
                    .contains_key(&(view.id, declaration, bank as i32)) {
                let child = host.create_task_object(6, view.id, definition.name.as_slice(), b"")?;
                host.data_services.tasks.fes.platform.button_regions.insert(child, identity.clone());
                host.data_services.tasks.fes.platform.objects.get_mut(&identity).unwrap().child = Some(child);
                host.data_services.tasks.fes.platform.button_records
                    .insert((view.id, declaration, bank as i32), ButtonRecordState::default());
                if let Ok(name) = std::str::from_utf8(&definition.name) {
                    driver.set_named_value(name, bank as i32, child as i32);
                }
            }
            continue;
        }
        let key = if count == 1 {
            name.clone()
        } else {
            format!("{name}[{bank}]")
        };
        let kind = definition.kind.as_slice();
        let child = match kind {
            b"PLANE" | b"CPLANE" => {
                runtime.planes.insert(key.clone(), FesPlaneLayer::default());
                if kind == b"CPLANE" {
                    runtime
                        .media
                        .insert(key.clone(), blank_layer(key.clone(), "CIMAGE".into()));
                }
                Some(host.create_task_object(2, view.id, definition.name.as_slice(), b"")?)
            }
            b"IMAGE" | b"CIMAGE" | b"DIFIMAGE" | b"BUTTON" => {
                runtime.media.insert(
                    key.clone(),
                    blank_layer(key.clone(), String::from_utf8_lossy(kind).into_owned()),
                );
                if kind == b"BUTTON" {
                    Some(host.create_task_object(6, view.id, definition.name.as_slice(), b"")?)
                } else {
                    None
                }
            }
            _ => {
                bail!(
                    "FES {} construction requires its dedicated object platform",
                    String::from_utf8_lossy(kind)
                );
            }
        };
        host.data_services.tasks.fes.platform.objects.insert(
            identity.clone(),
            ObjectBinding {
                key,
                kind: kind.to_vec(),
                child,
            },
        );
        if kind == b"BUTTON" {
            if let Some(child) = child {
                host.data_services.tasks.fes.platform.button_regions.insert(child, identity.clone());
                if let Ok(name) = std::str::from_utf8(&definition.name) {
                    driver.set_named_value(name, bank as i32, child as i32);
                }
            }
            host.data_services.tasks.fes.platform.button_properties.entry(identity.clone()).or_default();
            host.data_services.tasks.fes.platform.button_source_pending.insert(identity);
            host.data_services.tasks.fes.platform.button_records
                .insert((view.id, declaration, bank as i32), ButtonRecordState::default());
        }
    }
    Ok(())
}

fn initialize_objects(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    init: &cs2_script::FesObjectInitialization,
    shared: &mut ExpressionState,
) -> Result<()> {
    let (raw_name, bank) = target(&init.target)?;
    let count = driver
        .adapter
        .object(raw_name)
        .context("FES initializer target has no definition")?
        .definition
        .bank_count;
    let banks: Vec<_> = if let Some(bank) = bank {
        let bank = driver.evaluate_parameter(bank, 0, shared)?;
        if bank < 0 || bank as usize >= count {
            return Ok(());
        }
        vec![bank]
    } else {
        (0..count as i32).collect()
    };
    let fields = init.columns.len().min(init.values.len());
    let identity = (view.id, init.offset);
    let mut cursor = *host
        .data_services
        .tasks
        .fes
        .platform
        .initialization
        .get(&identity)
        .unwrap_or(&0);
    let total = banks
        .len()
        .checked_mul(fields)
        .context("FES initializer field count overflows")?;
    while cursor < total {
        let bank = banks[cursor / fields];
        let field = cursor % fields;
        let binding = host
            .data_services
            .tasks
            .fes
            .platform
            .objects
            .get(&(view.id, raw_name.to_vec(), bank))
            .context("FES initializer references an uncreated bank")?;
        let key = binding.key.clone();
        let kind = binding.kind.clone();
        let child = binding.child;
        if !matches!(
            kind.as_slice(),
            b"PLANE" | b"CPLANE" | b"IMAGE" | b"CIMAGE" | b"DIFIMAGE" | b"BUTTON"
        ) {
            bail!(
                "FES initialization of {} needs its dedicated object platform",
                String::from_utf8_lossy(&kind)
            );
        }
        let column = init.columns[field].as_slice();
        let value = init.values[field].as_slice();
        let button_field = kind == b"BUTTON" && matches!(column, b"ENABLE" | b"SYSKEY" | b"TYPE");
        if button_field {
            let value = driver.evaluate_parameter(value, 0, shared)?;
            let properties = host.data_services.tasks.fes.platform.button_properties
                .entry((view.id, raw_name.to_vec(), bank)).or_default();
            match column {
                b"ENABLE" => {
                    properties.enabled = value as u32;
                    runtime.media.get_mut(&key).context("BUTTON ENABLE image is absent")?.enabled = value != 0;
                }
                b"SYSKEY" => properties.syskey = value,
                b"TYPE" => properties.raw_type = value as u32,
                _ => unreachable!(),
            }
        }
        let navigation_field = super::fes_driver::main_flow::initialize_field(
            runtime, driver, shared, host.text_encoding, &key, &kind, column, value,
        )?;
        let motion_field = button_field || navigation_field || super::fes_motion::initialize_field(
            host, view, runtime, driver, shared, raw_name, bank, init.offset, field, column, value,
        )?;
        if !motion_field { match column {
            b"VIEWPORT" if matches!(kind.as_slice(), b"PLANE" | b"CPLANE") => {
                super::fes_controls::bind_viewport_consumer(
                    host, view.id, raw_name.to_vec(), bank,
                    child.context("FES VIEWPORT consumer has no plane task")?,
                    value, driver, shared,
                )?;
            }
            b"DEFAULT" => {
                // The driver's already-applied adapter keeps the event group
                // used by begin_input. No pixel or input command is implied.
            }
            b"FILE" => {
                let layer = runtime
                    .media
                    .get_mut(&key)
                    .context("FILE used on nonimage FES object")?;
                if matches!(kind.as_slice(), b"IMAGE" | b"BUTTON") {
                    super::fes_media::request_image_file(host, layer, value)?;
                } else {
                    layer.file = Some(host.text_encoding.decode(value));
                }
            }
            b"PL" => {
                runtime
                    .media
                    .get_mut(&key)
                    .context("PL used on nonimage FES object")?
                    .plane = Some(host.text_encoding.decode(value))
            }
            b"MASK" if matches!(kind.as_slice(), b"IMAGE" | b"BUTTON" | b"CIMAGE") => {
                let channel = driver.evaluate_parameter(value, 0, shared)?;
                let masks = &mut host.data_services.tasks.fes.platform.image_masks;
                match channel {
                    0 => { masks.remove(&(view.id, key.clone())); }
                    1..=4 => { masks.insert((view.id, key.clone()), channel as u8); }
                    _ => bail!("FES MASK channel {channel} is outside 0..=4"),
                }
            }
            b"IDN" if matches!(kind.as_slice(), b"IMAGE" | b"BUTTON") => {
                let id = driver.evaluate_parameter(value, 0, shared)?;
                let layer = runtime.media.get_mut(&key)
                    .context("IDN used on an absent IMAGE bank")?;
                super::fes_media::initialize_image_idn(layer, id);
            }
            b"VRAM" => {
                let xy = pair(value, driver, shared)?;
                runtime
                    .planes
                    .get_mut(&key)
                    .context("VRAM used on nonplane FES object")?
                    .vram = xy;
            }
            b"PRI" => {
                let value = driver.evaluate_parameter(value, 0, shared)?;
                if let Some(layer) = runtime.media.get_mut(&key) {
                    layer.priority = value;
                }
                if let Some(child) = child {
                    if let Some(node) = host.scene_nodes.get_mut(&child) {
                        node.position[2] = value as f32;
                    }
                }
            }
            _ if column.starts_with(b"ID.") => {
                let index: usize = std::str::from_utf8(&column[3..])?.parse()?;
                if index > 4095 {
                    bail!("FES image ID table exceeds 4096 entries");
                }
                let value = driver.evaluate_parameter(value, 0, shared)? as u32;
                let layer = runtime
                    .media
                    .get_mut(&key)
                    .context("ID used on nonimage FES object")?;
                layer
                    .frame_ids
                    .resize(layer.frame_ids.len().max(index + 1), 0);
                layer.frame_ids[index] = value;
                if matches!(kind.as_slice(), b"IMAGE" | b"BUTTON") {
                    layer.source.request_rebuild();
                }
            }
            _ => {
                let verb = match column {
                    b"POS" => b"pos".as_slice(),
                    b"SIZE" => b"size".as_slice(),
                    b"COLOR" => b"color".as_slice(),
                    b"DISP" => b"disp".as_slice(),
                    b"ENABLE" => b"enable".as_slice(),
                    b"ALPHA" | b"BLEND" => b"alpha".as_slice(),
                    _ => bail!(
                        "FES initializer column {:?} has no recovered media binding",
                        String::from_utf8_lossy(column)
                    ),
                };
                let args: Vec<_> = if matches!(column, b"POS" | b"SIZE") {
                    value
                        .split(|&byte| byte == b',')
                        .map(<[u8]>::to_vec)
                        .collect()
                } else {
                    vec![value.to_vec()]
                };
                let mut parameters = vec![key.as_bytes().to_vec(), verb.to_vec()];
                parameters.extend(args);
                let command = FesDriverCommand {
                    offset: init.offset,
                    source: Vec::new(),
                    parameters,
                    context: driver.interpreter.context(shared),
                    object_event: None,
                };
                if !runtime.apply_driver_media_encoded(
                    &command,
                    driver,
                    shared,
                    host.resources,
                    view.now as u64,
                    host.events,
                    host.text_encoding,
                )? {
                    bail!("FES initializer media target was rejected");
                }
            }
        }
        }
        if kind == b"BUTTON" && (matches!(column, b"FILE" | b"SIZE" | b"IDN") || column.starts_with(b"ID.")) {
            host.data_services.tasks.fes.platform.button_source_pending
                .insert((view.id, raw_name.to_vec(), bank));
        }
        cursor += 1;
        host.data_services
            .tasks
            .fes
            .platform
            .initialization
            .insert(identity, cursor);
    }
    Ok(())
}

pub(super) fn finish_task_media(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
) -> Result<bool> {
    super::fes_driver::main_flow::update_geometry(host, view, runtime)?;
    super::fes_media::update_image_sources(host, view, runtime)?;
    if !runtime.prepare_driver_animations(host, view)? {
        return Ok(false);
    }
    let color_planes: Vec<_> = host
        .data_services
        .tasks
        .fes
        .platform
        .objects
        .iter()
        .filter(|((owner, _, _), binding)| *owner == view.id && binding.kind == b"CPLANE")
        .map(|(_, binding)| binding.key.clone())
        .collect();
    for key in color_planes {
        if let (Some(layer), Some(plane)) = (runtime.media.get(&key), runtime.planes.get_mut(&key))
        {
            if plane.motion.is_none() {
                plane.position = layer.position;
                plane.visible = layer.visible;
                plane.opacity = layer.opacity;
            }
            if let Some(clip) = layer.clip {
                plane.size = [clip.width, clip.height];
            }
        }
    }
    if !host.data_services.tasks.fes.platform.finished_extensions.contains(&view.id) {
        if let Some(finish) = host.data_services.tasks.fes.platform.finish_extension {
            if !finish(host, view, runtime, driver)? {
                return Ok(false);
            }
        }
        // CPU projection/region/list work below may still fail. Retain this
        // completed stage so retry cannot play a sample or DRAW action twice.
        host.data_services.tasks.fes.platform.finished_extensions.insert(view.id);
    }
    let drawable_planes: Vec<_>=host.data_services.tasks.fes.platform.objects.iter()
        .filter(|((owner,_,_),binding)|*owner==view.id && matches!(binding.kind.as_slice(),b"PLANE"|b"CPLANE"))
        .filter_map(|(_,binding)|binding.child.map(|child|(child,binding.key.clone(),binding.kind==b"PLANE"))).collect();
    for (child,key,canvas) in drawable_planes {
        let plane=runtime.planes.get(&key).context("FES drawable plane state disappeared")?;
        let mut words=super::fes_motion::plane_render_properties(host,view,runtime,&key)?;
        if !canvas && plane.motion.is_none()
            && let Some(layer)=runtime.media.get(&key) {
            words[28]=u32::from(plane.opacity)<<24|u32::from(layer.color.r)<<16
                |u32::from(layer.color.g)<<8|u32::from(layer.color.b);
        }
        let mut options=ImageDrawOptions::new(Point::new(0,0));
        options.clip=runtime.driver_plane_clip(&key);
        super::fes_controls::apply_layer_clip(host.data_services,view.id,&key,&mut options);
        let node=host.scene_nodes.get_mut(&child).context("FES lower plane task disappeared")?;
        let offset=view.list.as_ref().map_or([0;2],|binding|binding.offset);
        let vram=[plane.vram[0].wrapping_add(offset[0]),plane.vram[1].wrapping_add(offset[1])];
        host.data_services.planes.sync_fes_plane(node,canvas,plane.size,vram,
            &words,plane.priority,view.paused==0,options.clip);
    }
    let mut button_images = BTreeMap::new();
    let mut record_index = 0;
    while let Some((declaration, name, bank)) = ordered_button_record(driver, record_index) {
        record_index += 1;
        if driver.adapter.object_bank(&name, bank as usize).is_none() { continue; }
        let identity = (view.id, name.clone(), bank);
        let Some(binding) = host.data_services.tasks.fes.platform.objects.get(&identity) else { continue; };
        let key = binding.key.clone();
        if host.data_services.tasks.fes.platform.button_source_pending.contains(&identity) {
            let image = if let Some(layer) = runtime.media.get(&key) {
                super::fes_media::source_image(host, layer)?
            } else { None };
            let record = host.data_services.tasks.fes.platform.button_records
                .get_mut(&(view.id, declaration, bank))
                .context("BUTTON source update has no original declaration record")?;
            record.image = image;
            record.source_updated = true;
            host.data_services.tasks.fes.platform.button_source_pending.remove(&identity);
        }
        let record = host.data_services.tasks.fes.platform.button_records
            .get(&(view.id, declaration, bank))
            .context("BUTTON projection has no original declaration record")?;
        button_images.insert(key, record.image.clone());
    }
    let mut replacement = Vec::new();
    for (name, layer) in &runtime.media {
        if runtime.planes.contains_key(name) {continue;}
        let image = if layer.kind == "BUTTON" {
            button_images.get(name).cloned().flatten()
        } else { super::fes_media::source_image(host, layer)? };
        if view.paused == 0 && let Some(image) = image.as_ref() {
            if let Some(mut options) = runtime.driver_media_draw_options(name)? {
                super::fes_media::source_draw_options(layer, image, &mut options);
                if let Some(&channel) = host.data_services.tasks.fes.platform.image_masks
                    .get(&(view.id, name.clone())) {
                    options.blend_mode = cs2_renderer::BlendMode::list_alpha_mask(u32::from(channel))
                        .context("FES projection has an invalid MASK channel")?;
                }
                let (image, projected_options) = super::fes_motion::prepare_projection(
                    host, view, runtime, name, image, options,
                )?;
                options = projected_options;
                super::fes_controls::apply_layer_clip(host.data_services, view.id, name, &mut options);
                if let Some(plane) = &layer.plane {
                    super::fes_controls::apply_layer_clip(host.data_services, view.id, plane, &mut options);
                }
                replacement.push(((view.id, name.clone()), Layer {
                    frame: image, options, priority: layer.priority,
                }));
            }
        }
    }
    for projection in super::fes_text::projections(host.data_services, view.id) {
        let projection = super::fes_text::prepare_motion_projection(host, view, runtime, projection)?;
        let key = projection.layer_key();
        let mut options = projection.options;
        super::fes_controls::apply_layer_clip(host.data_services, view.id, &key, &mut options);
        replacement.push((
            (view.id, key),
            Layer { frame: projection.frame, options,
                    priority: projection.priority },
        ));
    }
    let mut by_list: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    if let Some(binding) = &view.list {
        by_list.entry(binding.task).or_default().extend(replacement);
    }
    for projection in super::fes_controls::projections(host.data_services, view.id) {
        if let Some(list) = projection.list.or_else(|| view.list.as_ref().map(|binding| binding.task)) {
            let key = projection.layer_key();
            by_list.entry(list).or_default().push(((view.id, key), Layer {
                frame: projection.frame, options: projection.options, priority: projection.priority,
            }));
        }
    }
    let mut record_index = 0;
    while let Some((declaration, name, bank)) = ordered_button_record(driver, record_index) {
        record_index += 1;
        if driver.adapter.object_bank(&name, bank as usize).is_none() { continue; }
        let identity = (view.id, name.clone(), bank);
        let Some(binding) = host.data_services.tasks.fes.platform.objects.get(&identity) else { continue; };
        let key = binding.key.clone();
        let record = button_record_image(&host.data_services.tasks.fes.platform,
            view.id, declaration, &name, bank)?;
        let child = record.region;
        let layer = runtime
            .media
            .get(&key)
            .context("FES button media disappeared")?;
        let image = record.image.as_ref();
        let point = super::fes_motion::button_input_position(host, view, runtime, &key)?;
        // DISP never gates button input. Its raw ENABLE is independent.
        let enabled = view.paused == 0 && layer.enabled;
        let size = layer.clip.map(|clip| [clip.width as i32, clip.height as i32])
            .unwrap_or_else(|| image.map_or(layer.navigation.size,
                |image| [image.width() as i32, image.height() as i32]));
        register_region(
            host,
            child,
            layer.priority.wrapping_add(100),
            [
                point.x,
                point.y,
                point.x.wrapping_add(size[0]),
                point.y.wrapping_add(size[1]),
            ],
            enabled,
        )?;
        host.sync_fes_button_hit_image(child, record.raw_type, record.source_updated, image)?;
        host.data_services.tasks.fes.platform.button_records
            .get_mut(&(view.id, declaration, bank)).unwrap().source_updated = false;
    }
    if let Some(button) = host
        .data_services
        .tasks
        .fes
        .platform
        .low_buttons
        .get(&view.id)
        .copied()
    {
        host.mark_input_geometry_dirty();
        if let Some(region) = host
            .input_task
            .regions
            .iter_mut()
            .find(|region| region.id() == button)
        {
            region.words[8] = u32::from(view.paused == 0);
        }
    }
    for id in by_list.keys() {
        if !host.data_services.tasks.fes.platform.lists.contains_key(id) {
            bail!("FES bound/control image list {id} was released");
        }
    }
    let mut changed = BTreeSet::new();
    for (&id, list) in &mut host.data_services.tasks.fes.platform.lists {
        let old_count = list.layers.len();
        list.layers.retain(|(owner, _), _| *owner != view.id);
        if old_count != list.layers.len() {
            changed.insert(id);
        }
    }
    for (id, layers) in by_list {
        host.data_services.tasks.fes.platform.lists.get_mut(&id)
            .context("FES image list disappeared during submission")?.layers.extend(layers);
        changed.insert(id);
    }
    for id in changed {
        if host.data_services.tasks.fes.platform.lists[&id].enabled {
            compose_list(host, id, false)?;
        }
    }
    host.data_services.tasks.fes.platform.finished_extensions.remove(&view.id);
    Ok(true)
}

pub(super) fn compose_list(host: &mut LoggingHost<'_>, id: u32, force: bool) -> Result<()> {
    super::list_native::compose(host, id, force, None)
}

pub(super) fn register_region(
    host: &mut LoggingHost<'_>,
    id: u32,
    priority: i32,
    bounds: [i32; 4],
    enabled: bool,
) -> Result<()> {
    if let Some(region) = host
        .input_task
        .regions
        .iter_mut()
        .find(|region| region.id() == id)
    {
        region.words[1] = priority as u32;
        region.words[3..7].copy_from_slice(&bounds.map(|value| value as u32));
        region.words[8] = u32::from(enabled);
        host.mark_input_geometry_dirty();
        host.input_task
            .regions
            .sort_by_key(|region| std::cmp::Reverse(region.words[1] as i32));
        return Ok(());
    }
    if host.input_task.regions.len() >= 1_000_000 {
        bail!("FES input region limit exceeded");
    }
    let mut words = [0; 15];
    words[0] = id;
    words[1] = priority as u32;
    words[2] = 7;
    words[3..7].copy_from_slice(&bounds.map(|value| value as u32));
    words[8] = u32::from(enabled);
    words[9] = 1;
    words[14] = 1;
    host.input_task.regions.push(InputRegionState { words });
    host.mark_input_geometry_dirty();
    host.input_task
        .regions
        .sort_by_key(|region| std::cmp::Reverse(region.words[1] as i32));
    Ok(())
}

fn blank(width: u32, height: u32) -> Result<RgbaFrame> {
    let size = (width as usize)
        .checked_mul(height as usize)
        .and_then(|size| size.checked_mul(4))
        .filter(|&size| size <= 64 * 1024 * 1024)
        .context("FES image-list size exceeds 64 MiB")?;
    Ok(RgbaFrame::from_native_rgba8(
        width, height, vec![0; size], cs2_renderer::FrameMetadata::raster([width, height]),
    )?)
}

fn blank_layer(name: String, kind: String) -> FesMediaLayer {
    let clip = (kind == "CIMAGE").then(|| Rect::new(0, 0, 0, 0));
    FesMediaLayer {
        navigation: Default::default(),
        metadata: Default::default(),
        motion: None,
        name,
        kind,
        file: None,
        source: Default::default(),
        frame_ids: Vec::new(),
        selected_id: 0,
        plane: None,
        position: Point::default(),
        priority: 0,
        clip,
        color: Color::TRANSPARENT,
        visible: false,
        enabled: true,
        opacity: 255,
        image: None,
        image_offset: [0; 2],
        image_base: [0; 2],
        animation: None,
        animation_request: Default::default(),
        animation_display: None,
        fade: None,
    }
}

fn target(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    if let Some(open) = bytes.iter().position(|&byte| byte == b'[') {
        if !bytes.ends_with(b"]") {
            bail!("FES target bank is not closed");
        }
        Ok((&bytes[..open], Some(&bytes[open + 1..bytes.len() - 1])))
    } else {
        Ok((bytes, None))
    }
}

fn pair(
    bytes: &[u8],
    driver: &mut FesScriptDriver,
    shared: &mut ExpressionState,
) -> Result<[i32; 2]> {
    let mut values = bytes.split(|&byte| byte == b',');
    let x = values.next().context("FES pair is missing X")?;
    let y = values.next().context("FES pair is missing Y")?;
    if values.next().is_some() {
        bail!("FES pair has extra values");
    }
    Ok([
        driver.evaluate_parameter(x, 0, shared)?,
        driver.evaluate_parameter(y, 0, shared)?,
    ])
}

#[cfg(test)]
mod media_lifecycle_tests {
    use super::*;
    use crate::HeadlessPlayer;
    use cs2_script::{FesDocument, FesLine, FesSection};

    fn with_script(
        definition: &[u8], commands: &[&[u8]],
        test: impl FnOnce(&mut LoggingHost<'_>, &FesTaskView, &mut FesRuntime,
            &mut FesScriptDriver, &mut ExpressionState) -> Result<()>,
    ) -> Result<()> {
        let directory = std::env::temp_dir().join(format!("cs2-fes-media-{}-{}",
            std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()));
        std::fs::create_dir(&directory)?;
        let result = HeadlessPlayer::open_with_save_root(&directory, directory.join("profile"))
            .and_then(|mut player| player.with_host(|host| {
                let mut offset = 0;
                let mut section = |header: &[u8], lines: &[&[u8]]| FesSection {
                    header: header.to_vec(),
                    lines: lines.iter().map(|line| {
                        let line = FesLine { offset, bytes: line.to_vec() };
                        offset += line.bytes.len() + 1;
                        line
                    }).collect(),
                };
                let document = FesDocument { sections: vec![
                    section(b"DEFINE", &[definition]), section(b"START", commands),
                ] };
                let mut driver = FesScriptDriver::new(&document)?;
                let mut runtime = FesRuntime::new(document);
                let mut shared = ExpressionState::default();
                let id = host.create_task_object(34, 0, b"media test", b"")?;
                let view = FesTaskView { id, parent: 0, context: Vec::new(), list: None, now: 1, paused: 0 };
                while let Some(effect) = driver.advance_initialization(&mut shared)? {
                    if !matches!(effect, FesDriverEffect::Yield) {
                        assert!(apply_effect(host, &view, &mut runtime, &mut driver, &effect, &mut shared)?);
                    }
                }
                test(host, &view, &mut runtime, &mut driver, &mut shared)
            }));
        std::fs::remove_dir(&directory)?;
        result
    }

    fn command(
        host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
        driver: &mut FesScriptDriver, shared: &mut ExpressionState,
    ) -> Result<()> {
        let effect = driver.advance(u64::from(view.now), shared, 64)?;
        assert!(matches!(effect, FesDriverEffect::Command(_)), "{effect:?}");
        assert!(apply_effect(host, view, runtime, driver, &effect, shared)?);
        Ok(())
    }

    #[test]
    fn fes_message_window_reads_adjusted_images_and_local_planes() -> Result<()> {
        with_script(b"IMAGE face[2]", &[
            b"face getpos2 ($7+=1) 1",
            b"face[1] getpos2 0 1",
            b"face[1] getpos2 2 2 -1",
        ], |host, view, runtime, driver, shared| {
            let image = runtime.media.get_mut("face[1]").unwrap();
            image.position = cs2_renderer::Point::new(30, 20);
            image.image_offset = [12, -4];
            image.image_base = [5, 2];
            image.plane = Some("attached".into());
            runtime.planes.insert("attached".into(), super::super::FesPlaneLayer {
                position: cs2_renderer::Point::new(100, 200),
                vram: [11, 19],
                ..Default::default()
            });
            command(host, view, runtime, driver, shared)?;
            assert_eq!(shared.shared[7], 0);
            command(host, view, runtime, driver, shared)?;
            command(host, view, runtime, driver, shared)?;
            assert_eq!(&shared.shared[..3], &[37, 14, 14]);
            Ok(())
        })?;
        with_script(b"PLANE pl_face", &[b"pl_face getpos 0 1 2"],
            |host, view, runtime, driver, shared| {
                let plane = runtime.planes.get_mut("pl_face").unwrap();
                let mut words = [0; 36];
                words[6..9].copy_from_slice(&[-8.75f32, 12.5, 23.25].map(f32::to_bits));
                plane.motion = Some(words);
                plane.vram = [100, 200];
                command(host, view, runtime, driver, shared)?;
                assert_eq!(&shared.shared[..3], &[-8, 12, 23]);
                Ok(())
            })
    }

    #[test]
    fn fes_string8_getpos_reads_local_position_into_shared_slots() -> Result<()> {
        with_script(
            b"STRING8 strsub",
            &[
                b"strsub pos2 -83 124",
                b"strsub org 35 -19",
                b"strsub scale 170 130",
                b"strsub getpos 850 851",
                b"strsub getpos ($7+=1) ($7+=1) 3",
                b"strsub getpos 4 4 -1",
            ],
            |host, view, runtime, driver, shared| {
                shared.shared[3] = 500;
                for _ in 0..4 {
                    command(host, view, runtime, driver, shared)?;
                }
                assert_eq!(&shared.shared[850..852], &[-83, 124]);
                assert_eq!(shared.shared[3], 500);
                command(host, view, runtime, driver, shared)?;
                assert_eq!(&shared.shared[1..4], &[-83, 124, 0]);
                assert_eq!(shared.shared[7], 2);
                command(host, view, runtime, driver, shared)?;
                assert_eq!(shared.shared[4], 124);
                Ok(())
            },
        )
    }

    #[test]
    fn fes_logo_delete_releases_only_the_selected_plane_banks() -> Result<()> {
        with_script(b"PLANE pl[4]", &[b"pl[1] delete", b"pl[2] delete", b"pl[3] disp 1"],
            |host, view, runtime, driver, shared| {
                for bank in [1, 2] {
                    let identity = (view.id, b"pl".to_vec(), bank);
                    let child = host.data_services.tasks.fes.platform.objects[&identity].child.unwrap();
                    command(host, view, runtime, driver, shared)?;
                    assert!(!runtime.planes.contains_key(&format!("pl[{bank}]")));
                    assert!(driver.adapter.object_bank(b"pl", bank as usize).is_none());
                    assert!(!host.data_services.tasks.fes.platform.objects.contains_key(&identity));
                    assert_eq!(host.data_services.tasks.message_task_state(child).unwrap().1, 1);
                    assert!(!host.scene_nodes[&child].visible);
                    assert!(plan_bank_release(host, runtime, driver, view.id, b"pl", bank)?.is_empty());
                }
                command(host, view, runtime, driver, shared)?;
                assert!(runtime.planes["pl[3]"].visible);
                assert!(driver.adapter.object_bank(b"pl", 0).is_some());
                assert!(driver.adapter.object_bank(b"pl", 3).is_some());
                Ok(())
            })
    }

    #[test]
    fn fes_button_lock_holds_hit_geometry_without_disabling_the_button() -> Result<()> {
        with_script(b"BUTTON btn[2]", &[b"btn lock 1", b"btn[0] lock 2", b"btn[0] lock 0"],
            |host, view, runtime, driver, shared| {
                runtime.media.get_mut("btn[0]").unwrap().position = Point::new(30, 40);
                runtime.media.get_mut("btn[1]").unwrap().position = Point::new(60, 70);
                command(host, view, runtime, driver, shared)?;
                let layer = runtime.media.get_mut("btn[0]").unwrap();
                layer.position = Point::new(100, 120);
                layer.navigation.size = [20, 10];
                command(host, view, runtime, driver, shared)?;
                assert_eq!(super::super::fes_motion::button_image_position(host, view.id, runtime, "btn[0]")?,
                    Point::new(30, 40));
                assert!(finish_task_media(host, view, runtime, driver)?);
                let region = host.data_services.tasks.fes.platform.objects[&(view.id, b"btn".to_vec(), 0)].child.unwrap();
                let region = host.input_task.regions.iter().find(|candidate| candidate.id() == region).unwrap();
                assert_eq!(&region.words[3..7], &[30, 40, 50, 50]);
                assert_eq!(region.words[8], 1);
                command(host, view, runtime, driver, shared)?;
                assert_eq!(super::super::fes_motion::button_image_position(host, view.id, runtime, "btn[0]")?,
                    Point::new(100, 120));
                assert_eq!(button_locked_position(host, view.id, "btn[1]"), Some([60.0, 70.0]));
                Ok(())
            })
    }

    #[test]
    fn fes_button_lock_checkpoint_preserves_position_and_accepts_legacy_records() -> Result<()> {
        use cs2_runtime::object_graph::{decode_component, encode_component};

        let legacy = encode_component(&(1u32, -1i32, 0u32))?;
        let mut properties: ButtonProperties = decode_component(&legacy)?;
        assert!(properties.locked_position.is_none());
        properties.locked_position = Some([40.5, 65.0]);
        let restored: ButtonProperties = decode_component(&encode_component(&properties)?)?;
        assert_eq!(restored.locked_position, properties.locked_position);
        Ok(())
    }
}
