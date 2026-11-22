//! Type34 control, CPU objects and shared motion in the CSS host transaction.
//! Resource state is captured, not reconstructed by rerunning FES declarations.
use super::*;
use anyhow::{Result, bail, Context};
use cs2_runtime::graph_serde::{Serialize, Deserialize};
use cs2_runtime::object_graph::{self as wire, ImageFrameSnapshot, ObjectGraphSnapshot};
use cs2_renderer::{Color, Point, Rect, RgbaFrame};
use crate::{FesMediaLayer, FesPlaneLayer, ScriptTextEncoding};
use crate::image_manager::{capture_frame, restore_frame};

#[path = "fes_extension_state.rs"]
mod extensions;
#[path = "fes_bank_state.rs"]
mod banks;

type Metadata = (Option<(Vec<u8>, u32)>, Option<(Vec<u8>, u32)>, bool);
fn metadata(value: &crate::fes_driver::main_flow::GeometryMetadata) -> Metadata {
    (value.position_source.as_ref().map(|v| (v.name.clone(), v.frame)),
        value.size_source.as_ref().map(|v| (v.name.clone(), v.frame)), value.size_dirty)
}
fn restore_metadata(value: Metadata) -> crate::fes_driver::main_flow::GeometryMetadata {
    use cs2_runtime::fes_controls::MetadataReference;
    crate::fes_driver::main_flow::GeometryMetadata {
        position_source: value.0.map(|(name, frame)| MetadataReference { name, frame }),
        size_source: value.1.map(|(name, frame)| MetadataReference { name, frame }), size_dirty: value.2,
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Media {
    navigation: ([i32; 2], u32, [i32; 2], Option<(Vec<u8>, i32)>), metadata: Metadata,
    motion: Option<Vec<u32>>, name: String, kind: String, file: Option<String>,
    frames: Vec<u32>, selected: usize, plane: Option<String>, position: [i32; 2],
    priority: i32, clip: Option<([i32; 2], [u32; 2])>, color: [u8; 4],
    visible: bool, enabled: bool, opacity: u8, image: Option<ImageFrameSnapshot>,
    offset: [i32; 2], base: [i32; 2], animation: Option<cs2_script::anm::snapshot::ImageAnimationSnapshot>,
    display: Option<[i32; 5]>, fade: Option<(i32, i32, i32, i32)>,
    source: crate::fes_media::ImageSourceState,
    animation_request: crate::fes_media::ImageAnimationRequest,
}
impl Media {
    fn capture(layer: &FesMediaLayer) -> Self {
        let n = &layer.navigation;
        Self { navigation: (n.size, n.mode, n.offset, n.control.clone()), metadata: metadata(&layer.metadata),
            motion: layer.motion.map(|v| v.to_vec()), name: layer.name.clone(), kind: layer.kind.clone(),
            file: layer.file.clone(), frames: layer.frame_ids.clone(), selected: layer.selected_id,
            plane: layer.plane.clone(), position: [layer.position.x, layer.position.y], priority: layer.priority,
            clip: layer.clip.map(|r| ([r.x, r.y], [r.width, r.height])),
            color: [layer.color.r, layer.color.g, layer.color.b, layer.color.a],
            visible: layer.visible, enabled: layer.enabled, opacity: layer.opacity,
            image: layer.image.as_ref().map(capture_frame), offset: layer.image_offset, base: layer.image_base,
            animation: layer.animation.as_ref().map(cs2_script::ImageAnimation::execution_snapshot),
            display: layer.animation_display.map(|v| [v.frame_id, v.blend, v.visible, v.x, v.y]), fade: layer.fade,
            source: layer.source.clone(), animation_request: layer.animation_request.clone() }
    }
    fn restore(self) -> Result<FesMediaLayer> {
        self.source.validate_checkpoint()?;
        let animation = self.animation.as_ref()
            .map(cs2_script::ImageAnimation::from_execution_snapshot).transpose()?;
        self.animation_request.validate_animation(animation.as_ref())?;
        if self.frames.len() > 1_000_000 || (!self.frames.is_empty() && self.selected >= self.frames.len()) {
            bail!("FES checkpoint media frame cursor is invalid");
        }
        Ok(FesMediaLayer {
            navigation: crate::fes_driver::main_flow::ImageNavigation { size: self.navigation.0,
                mode: self.navigation.1, offset: self.navigation.2, control: self.navigation.3 },
            metadata: restore_metadata(self.metadata), motion: self.motion.map(|v| v.try_into()
                .map_err(|_| anyhow::anyhow!("FES motion checkpoint must have36 words"))).transpose()?,
            name: self.name, kind: self.kind, file: self.file, frame_ids: self.frames, selected_id: self.selected,
            plane: self.plane, position: Point::new(self.position[0], self.position[1]), priority: self.priority,
            clip: self.clip.map(|(at, size)| Rect::new(at[0], at[1], size[0], size[1])),
            color: Color::rgba(self.color[0], self.color[1], self.color[2], self.color[3]),
            visible: self.visible, enabled: self.enabled, opacity: self.opacity,
            image: self.image.as_ref().map(restore_frame).transpose()?, image_offset: self.offset, image_base: self.base,
            animation,
            animation_display: self.display.map(|v| cs2_script::DisplayState {
                frame_id: v[0], blend: v[1], visible: v[2], x: v[3], y: v[4] }), fade: self.fade,
            source: self.source,
            animation_request: self.animation_request,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Plane {
    expose: u32, metadata: Metadata, motion: Option<Vec<u32>>, priority: i32,
    position: [i32; 2], size: [u32; 2], vram: [i32; 2], visible: bool, opacity: u8,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Runtime {
    driver: Option<FesScriptDriver>, document: FesDocument, cursor: (usize, usize, u32),
    exit_code: Option<i32>, values: BTreeMap<i32, i32>, named: BTreeMap<String, i32>,
    input: (bool, bool), media: BTreeMap<String, Media>, planes: BTreeMap<String, Plane>,
    random: u32, focus: Option<String>, event_return: Option<(usize, usize, u32)>,
    event_target: Option<String>, event_bank: Option<i32>,
}
impl Runtime {
    fn capture(value: &FesRuntime) -> Self {
        Self { driver: value.driver.clone(), document: value.document.clone(),
            cursor: (value.section, value.line, value.wait_frames), exit_code: value.exit_code,
            values: value.values.clone(), named: value.named.clone(), input: (value.input_mode, value.continuous_input),
            media: value.media.iter().map(|(key, layer)| (key.clone(), Media::capture(layer))).collect(),
            planes: value.planes.iter().map(|(key, layer)| (key.clone(), Plane {
                expose: layer.expose, metadata: metadata(&layer.metadata), motion: layer.motion.map(|v| v.to_vec()),
                priority: layer.priority, position: [layer.position.x, layer.position.y], size: layer.size,
                vram: layer.vram, visible: layer.visible, opacity: layer.opacity })).collect(),
            random: value.animation_random_state, focus: value.focused_object.clone(),
            event_return: value.event_return, event_target: value.event_target.clone(), event_bank: value.event_bank }
    }
    fn restore(self) -> Result<FesRuntime> {
        if self.media.len() > 65_536 || self.planes.len() > 65_536 || self.document.sections.len() > 1_000_000 {
            bail!("FES runtime checkpoint exceeds object bounds");
        }
        if let Some(driver) = &self.driver { driver.validate_checkpoint()?; }
        Ok(FesRuntime { driver: self.driver, document: self.document,
            section: self.cursor.0, line: self.cursor.1, wait_frames: self.cursor.2, exit_code: self.exit_code,
            values: self.values, named: self.named, input_mode: self.input.0, continuous_input: self.input.1,
            media: self.media.into_iter().map(|(key, layer)| Ok((key, layer.restore()?))).collect::<Result<_>>()?,
            planes: self.planes.into_iter().map(|(key, layer)| Ok((key, FesPlaneLayer {
                expose: layer.expose, metadata: restore_metadata(layer.metadata),
                motion: layer.motion.map(|v| v.try_into().map_err(|_| anyhow::anyhow!("FES plane motion checkpoint must have36 words"))).transpose()?,
                priority: layer.priority, position: Point::new(layer.position[0], layer.position[1]),
                size: layer.size, vram: layer.vram, visible: layer.visible, opacity: layer.opacity,
            }))).collect::<Result<_>>()?, animation_random_state: self.random, focused_object: self.focus,
            event_return: self.event_return, event_target: self.event_target, event_bank: self.event_bank })
    }
}

pub(super) mod runtime {
    use super::*;
    use cs2_runtime::graph_serde::{Serializer, Deserializer};
    pub(crate) fn serialize<S: Serializer>(value: &Option<FesRuntime>, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        value.as_ref().map(Runtime::capture).serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<FesRuntime>, D::Error> {
        Option::<Runtime>::deserialize(deserializer)?.map(Runtime::restore).transpose()
            .map_err(cs2_runtime::graph_serde::de::Error::custom)
    }
}

pub(crate) mod frame {
    use super::*;
    use cs2_runtime::graph_serde::{Serializer, Deserializer};
    pub(crate) fn serialize<S: Serializer>(value: &Option<RgbaFrame>, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        value.as_ref().map(capture_frame).serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<RgbaFrame>, D::Error> {
        Option::<ImageFrameSnapshot>::deserialize(deserializer)?.as_ref().map(restore_frame).transpose()
            .map_err(cs2_runtime::graph_serde::de::Error::custom)
    }
}

pub(crate) mod words48 {
    use cs2_runtime::graph_serde::{Serialize, Deserialize, Serializer, Deserializer};
    pub(crate) fn serialize<S: Serializer>(value: &[u32; 48], serializer: S) -> std::result::Result<S::Ok, S::Error> {
        value.as_slice().serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<[u32; 48], D::Error> {
        Vec::<u32>::deserialize(deserializer)?.try_into()
            .map_err(|_| cs2_runtime::graph_serde::de::Error::custom("FES system voice checkpoint needs48 words"))
    }
}

type Button = (cs2_runtime::message_object::InputButtonState, Option<u64>, Option<u64>, bool);
type Controller = (Option<FesDriverTarget>, Option<FesDriverTarget>, Option<FesDriverTarget>, u32, u32, u32);

impl FesTaskStore {
    fn validate_sound_bank_consumers(&self, banks: &BTreeSet<(u32, Vec<u8>, i32)>) -> Result<()> {
        for (&owner, slot) in &self.slots {
            let object = slot.object.as_ref().context("SOUND checkpoint FES owner is borrowed")?;
            let Some(driver) = object.runtime.as_ref().and_then(|runtime| runtime.driver.as_ref()) else { continue; };
            for record in &driver.adapter.objects {
                if !matches!(record.definition.kind_name.as_slice(), b"SOUND" | b"SNDTABLE") { continue; }
                for bank in &record.banks {
                    if driver.adapter.object_bank_definition(&record.definition.name, bank.index)
                        .is_none_or(|definition| definition != &record.definition) { continue; }
                    let index = i32::try_from(bank.index).context("SOUND checkpoint bank exceeds signed index")?;
                    if !banks.contains(&(owner, record.definition.name.clone(), index))
                        && !self.checkpoint_sound_creation_pending(owner, &record.definition.name) {
                        bail!("FES SOUND binding has no real bank consumer or retained constructor");
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn validate_sound_notice_checkpoint(&self, owner: u32, name: &[u8], bank: i32,
        event: &[u8], alias: Option<(&[u8], u32)>, callback_done: bool) -> Result<()> {
        let object = self.slots.get(&owner).and_then(|slot| slot.object.as_ref())
            .context("SOUND checkpoint notification has no owned FES task")?;
        let drivers = || object.runtime.iter().filter_map(|runtime| runtime.driver.as_ref())
            .chain(&object.previous_driver);
        let valid = if let Some((header, statement)) = alias {
            drivers().any(|driver| driver.checkpoint_media_alias(name, bank, header, statement))
        } else {
            callback_done || drivers().any(|driver| driver.checkpoint_object_notice(name, bank, event))
                || object.pending_effect.iter().chain(&object.boundary)
                    .chain(object.button_refresh.iter().filter_map(|work| work.pending_effect.as_ref()))
                    .chain(object.lower_notification.iter().filter_map(|work| work.pending_effect.as_ref()))
                    .any(|effect| effect.checkpoint_object_notice(name, bank, event))
                || object.button_refresh.iter().filter_map(|work| work.notifications.as_ref())
                    .chain(object.lower_notification.iter().filter_map(|work| work.notifications.as_ref()))
                    .any(|scope| scope.checkpoint_object_notice(name, bank, event))
        };
        if !valid { bail!("SOUND checkpoint notification lost its exact FES alias/handler/receipt target"); }
        Ok(())
    }

    pub(crate) fn validate_checkpoint_lower_notification(&self, owner: u32,
        command: cs2_runtime::TaskCommand, input: &FesDriverInput) -> Result<()> {
        let slot = self.slots.get(&owner).context("lower checkpoint FES owner slot is absent")?;
        let object = slot.object.as_ref().context("lower checkpoint FES object is borrowed")?;
        if slot.lower_notification_request.as_ref().is_some_and(|request| !request.matches(command, input))
            || object.lower_notification.as_ref().is_some_and(|notice| !notice.request.matches(command, input)) {
            bail!("lower command checkpoint and FES handler continuation packets differ");
        }
        Ok(())
    }

    /// Freeze only actual object bindings. Borrowed slots are rejected by the
    /// enclosing checkpoint and are not inferred from scene parentage here.
    pub(crate) fn checkpoint_list_bindings(&self) -> BTreeMap<u32, Option<FesListBinding>> {
        self.slots.iter().filter_map(|(&id, slot)|
            slot.object.as_ref().map(|object| (id, object.list.clone()))).collect()
    }

    /// The common property registry precedes the dedicated bank constructor.
    /// Preserve a partially constructed text declaration at that real effect.
    pub(crate) fn checkpoint_text_creation_pending(&self, owner: u32, name: &[u8]) -> bool {
        self.checkpoint_extension_creation_pending(owner, name, true)
    }

    pub(crate) fn checkpoint_sound_creation_pending(&self, owner: u32, name: &[u8]) -> bool {
        let Some(object) = self.slots.get(&owner).and_then(|slot| slot.object.as_ref()) else { return false; };
        object.pending_effect.iter().chain(&object.boundary)
            .chain(object.button_refresh.iter().filter_map(|r| r.pending_effect.as_ref()))
            .chain(object.lower_notification.iter().filter_map(|r| r.pending_effect.as_ref()))
            .any(|effect| matches!(effect,
                FesDriverEffect::Initialize { event: FesEvent::CreateObject(definition), .. }
                    if definition.name == name && matches!(definition.kind.as_slice(), b"SOUND" | b"SNDTABLE")))
    }

    pub(crate) fn checkpoint_extension_creation_pending(&self, owner: u32, name: &[u8], text_only: bool) -> bool {
        let Some(object) = self.slots.get(&owner).and_then(|slot| slot.object.as_ref()) else { return false; };
        let matches = |effect: &FesDriverEffect| matches!(effect,
            FesDriverEffect::Initialize { event: FesEvent::CreateObject(definition), .. }
                if definition.name == name && (crate::fes_text::is_text_kind(&definition.kind)
                    || !text_only && matches!(definition.kind.as_slice(), b"VIEWPORT" | b"IMGWND" | b"SLIDER" | b"SCROLL2" | b"SCROLL")));
        object.pending_effect.iter().chain(&object.boundary)
            .chain(object.button_refresh.iter().filter_map(|r| r.pending_effect.as_ref()))
            .chain(object.lower_notification.iter().filter_map(|r| r.pending_effect.as_ref()))
            .any(matches)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot {
    version: u32, slots: BTreeMap<u32, FesSlot>, platform: Vec<u8>, buffered: Vec<u8>,
    buttons: BTreeMap<u32, Button>, controllers: BTreeMap<u32, Controller>,
    message_owners: BTreeMap<u32, u32>, globals: Vec<u8>, settings: Vec<u8>, sound: Vec<u8>,
    text_options: (i32, bool, u32, i32), font_controls: Option<[u32; 5]>,
}
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct HostEnvelope { version: u32, host: Vec<u8>, tasks: Vec<u8> }
const HOST_TAG: &[u8] = b"CS2FESTASK1\0";
const COMPONENT_TAG: &[u8] = b"CS2FESCPU7\0";
struct Components { tasks: Vec<u8>, motion: Vec<u8>, extensions: Vec<u8> }
// Sequential component encoding has no optional-field wire evolution. CPU7
// adds real bank consumers to CPU6's retained raw sources/animation requests.
// Restoring these fields never replays a declaration or media instruction.
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct PhysicalComponents {
    version: u32, tasks: Vec<u8>, motion: Vec<u8>, extensions: Vec<u8>, physical_sound: Vec<u8>,
    particles: Vec<u8>, save_panels: Vec<u8>,
}
pub(crate) struct PreparedFesTaskState {
    store: FesTaskStore, buttons: BTreeMap<u32, crate::message_platform::InputButtonRuntime>,
    controllers: BTreeMap<u32, crate::message_platform::FesButtonController>,
    buffered: crate::native_tasks::PreparedBufferedCommands,
    message_owners: BTreeMap<u32, u32>, globals: crate::fes_globals::FesGlobalStore,
    settings: crate::fes_settings::FesSettingsStore, sound: crate::fes_sound::FesSoundStore,
    motion: crate::fes_motion::FesMotionStore,
    extensions: extensions::PreparedExtensions,
    physical_sound: Option<crate::sound_task::checkpoint::PreparedPhysicalSoundState>,
    particles: Option<crate::fes_particle::checkpoint::PreparedParticles>,
    text_options: (i32, bool, u32, i32), font_controls: Option<[u32; 5]>,
}

impl PreparedFesTaskState {
    /// Canonical type15 layers are prepared independently. Validate against
    /// those exact layers before any component is committed.
    pub(crate) fn validate_projection_links(&self,
        lists: &BTreeMap<u32, crate::fes_task_platform::ImageList>,
        kinds: &BTreeMap<u32, u32>) -> Result<()> {
        self.extensions.validate_projection_links(&self.store, lists, kinds)
    }

    /// Call against the canonical prepared list graph, before any live write.
    /// Old live entries and allocator-only plane identities cannot validate a
    /// restored source. Encoded bytes stay in Media.source; the ANM cursor
    /// stays in Media.animation and is restored without replay.
    pub(crate) fn validate_image_source_links(&self,
        lists: &crate::list_native::PreparedImageLists) -> Result<()> {
        self.store.validate_image_source_links(|handle| lists.has_retained_image_source(handle))
    }
}

impl FesTaskStore {
    fn validate_image_source_links(&self, mut retained: impl FnMut(u32) -> bool) -> Result<()> {
        for (&owner, slot) in &self.slots {
            let object = slot.object.as_ref().context("raw IMAGE checkpoint FES owner is borrowed")?;
            let Some(runtime) = &object.runtime else { continue; };
            for (name, layer) in &runtime.media {
                layer.source.validate_checkpoint()?;
                layer.animation_request.validate_animation(layer.animation.as_ref())?;
                if let Some(handle) = layer.source.raw_entry_handle() {
                    if !retained(handle) {
                        bail!("FES raw IMAGE {owner}/{name} checkpoint lost retained-entry object {handle}");
                    }
                }
            }
        }
        Ok(())
    }
}

impl FesTask {
    fn checkpoint_binding(&mut self, external: &SharedExternalStorage, bind: bool) -> Result<()> {
        if bind { self.external = external.clone(); }
        else if !self.external.same_storage(external) { bail!("FES task checkpoint external alias differs"); }
        for effect in self.pending_effect.iter_mut().chain(self.boundary.iter_mut()) {
            effect.checkpoint_external(external, bind)?;
        }
        if let Some(child) = &mut self.pending_child {
            if bind { child.external = external.clone(); }
            else if !child.external.same_storage(external) { bail!("FES child checkpoint external alias differs"); }
        }
        for driver in self.runtime.iter_mut().filter_map(|runtime| runtime.driver.as_mut()).chain(&mut self.previous_driver) {
            driver.checkpoint_external(external, bind)?;
        }
        if let Some(refresh) = &mut self.button_refresh {
            if let Some(effect) = &mut refresh.pending_effect { effect.checkpoint_external(external, bind)?; }
            if let Some(scope) = &mut refresh.notifications { scope.checkpoint_external(external, bind)?; }
        }
        if let Some(notification) = &mut self.lower_notification {
            if let Some(effect) = &mut notification.pending_effect { effect.checkpoint_external(external, bind)?; }
            if let Some(scope) = &mut notification.notifications { scope.checkpoint_external(external, bind)?; }
        }
        Ok(())
    }

    fn validate_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<()> {
        if kinds.get(&self.id) != Some(&34) || self.context.len() > 31 || self.context.contains(&0)
            || self.name.as_ref().is_some_and(|name| name.len() > 259)
            || self.requested_name.as_ref().is_some_and(|name| name.len() > 259)
            || self.system.as_ref().is_some_and(|system| system.configuration_phase > 3)
        { bail!("FES task checkpoint metadata is invalid"); }
        // Completed child numbers remain in the original task after collect.
        // Only a still-running creation continuation requires a live object.
        if let Some(child) = &self.pending_child {
            if child.name.len() > 259 || (child.phase != ChildPhase::Create && child.id.is_none())
                || child.id.is_some_and(|id| !kinds.contains_key(&id))
            { bail!("FES checkpoint pending child creation lost its real owner"); }
        }
        if self.direct_list.is_some_and(|id| kinds.get(&id) != Some(&15)) {
            bail!("FES checkpoint direct list is absent or not type15");
        }
        if let Some(list) = &self.list {
            if kinds.get(&list.task) != Some(&15) { bail!("FES checkpoint list binding is not type15"); }
        }
        if let Some(children) = &self.start_children { children.validate_fes_checkpoint(self.id, kinds)?; }
        for driver in self.runtime.iter().filter_map(|runtime| runtime.driver.as_ref()).chain(&self.previous_driver) {
            driver.validate_checkpoint_owner(self.id)?;
            if driver.adapter.objects.iter().any(|object|
                !crate::fes_task_platform::checkpoint::supported_object(&object.definition.kind_name)
                    && !crate::fes_text::is_text_kind(&object.definition.kind_name)
                    && !matches!(object.definition.kind_name.as_slice(),
                        b"VIEWPORT" | b"IMGWND" | b"SLIDER" | b"SCROLL2" | b"SCROLL" | b"DRAW" | b"SOUND" | b"SNDTABLE" | b"PARTICLE" | b"SAVEPANEL")) {
                bail!("FES checkpoint object kind needs its matching extension owner/physical-task adapter");
            }
            if let Some(scope) = self.button_refresh.as_ref().and_then(|refresh| refresh.notifications.as_ref()) {
                scope.validate_checkpoint(driver)?;
            }
            if let Some(scope) = self.lower_notification.as_ref().and_then(|notice| notice.notifications.as_ref()) {
                scope.validate_checkpoint(driver)?;
            }
            for effect in self.pending_effect.iter().chain(&self.boundary)
                .chain(self.button_refresh.iter().filter_map(|work| work.pending_effect.as_ref()))
                .chain(self.lower_notification.iter().filter_map(|work| work.pending_effect.as_ref())) {
                effect.validate_send_checkpoint(driver)?;
            }
        }
        Ok(())
    }
}

impl HeadlessPlayer {
    fn validate_control_sound_checkpoint(bytes: &[u8], kinds: &BTreeMap<u32, u32>) -> Result<()> {
        // The E component is committed beside the driver, never on its own.
        // A nonempty bank needs its physical kind13 channel adapter too. Keep
        // that boundary explicit until both sides can be prepared atomically.
        if bytes != crate::fes_sound::FesSoundStore::default().capture_checkpoint(kinds)?.as_slice() {
            bail!("FES SOUND checkpoint needs the matching physical sound-task graph adapter");
        }
        Ok(())
    }

    pub(crate) fn encode_fes_graph_host(&self, host: Vec<u8>) -> Result<Vec<u8>> {
        let mut bytes = HOST_TAG.to_vec();
        bytes.extend_from_slice(&wire::encode_component(&HostEnvelope {
            version: 1, host, tasks: self.capture_fes_task_state()?,
        })?);
        Ok(bytes)
    }

    pub(crate) fn decode_fes_graph_host(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        if !bytes.starts_with(HOST_TAG) { return Ok((bytes.to_vec(), Vec::new())); }
        let envelope: HostEnvelope = wire::decode_component(&bytes[HOST_TAG.len()..])?;
        if envelope.version != 1 { bail!("unsupported FES host-checkpoint envelope"); }
        Ok((envelope.host, envelope.tasks))
    }

    pub(crate) fn capture_fes_task_state(&self) -> Result<Vec<u8>> {
        let tasks = &self.data_services.tasks;
        let kinds = tasks.checkpoint_task_kinds();
        let save_panels = tasks.fes.platform.save_panels.capture_checkpoint(&kinds)?;
        let panel_keys = tasks.fes.validate_checkpoint_save_panels(&tasks.fes.platform.save_panels)?;
        tasks.fes.validate_image_source_links(|handle|
            self.data_services.list_native.checkpoint_has_retained_image_source(handle))?;
        let input = self.capture_input_sampling_for_graph()?;
        tasks.fes.platform.validate_checkpoint_with_save_panels(&tasks.fes, &kinds, &self.task_parents, &input)?;
        tasks.fes.platform.validate_cpu_checkpoint(&tasks.fes, &kinds, &self.task_parents,
            &self.image_handles.keys().copied().collect(), self.script_encoding)?;
        let mut slots = tasks.fes.slots.clone();
        for (&id, slot) in &mut slots {
            let object = slot.object.as_mut().context("cannot checkpoint a borrowed FES object")?;
            object.validate_checkpoint(&kinds)?;
            let external = tasks.checkpoint_external_binding(id)
                .context("FES checkpoint needs its actual record external binding")?;
            object.checkpoint_binding(&external, false)?;
        }
        let messages = &self.data_services.messages;
        let message_owners = tasks.checkpoint_message_owners();
        tasks.validate_checkpoint_message_owners(&message_owners)?;
        let sound = self.data_services.fes_sound.capture_checkpoint(&kinds)?;
        let sound_keys = self.data_services.fes_sound.validate_live_physical_checkpoint(
            self, &tasks.fes, self.script_encoding)?;
        tasks.fes.validate_sound_bank_consumers(&sound_keys)?;
        let physical_sound = self.capture_physical_sound_state(&kinds, &self.task_parents)?;
        let particles = self.capture_particle_checkpoint(&kinds)?;
        let particle_keys = self.data_services.fes_particles.checkpoint_bank_keys().cloned().collect();
        tasks.fes.validate_checkpoint_bank_consumers(b"PARTICLE", &particle_keys)?;
        let snapshot = wire::encode_component(&Snapshot { version: 2, slots,
            platform: tasks.fes.platform.capture_checkpoint_with_save_panels(&tasks.fes, &kinds, &self.task_parents, &input)?,
            buffered: tasks.capture_buffered_command_state()?,
            buttons: messages.input_buttons.iter().map(|(&id, value)| (id, value.execution_checkpoint())).collect(),
            controllers: messages.fes_button_controllers.iter().map(|(&id, value)| (id, (
                value.focus.clone(), value.preferred_focus.clone(), value.pressed.clone(), value.focus_lock,
                value.raw_state, value.controller_state))).collect(),
            message_owners, globals: self.data_services.fes_globals.capture_checkpoint(&kinds)?,
            settings: self.data_services.fes_settings.capture_checkpoint(&kinds)?,
            sound,
            text_options: self.data_services.fes_text.checkpoint_options(),
            font_controls: self.data_services.message_device.raw_font_controls(),
        })?;
        let extensions = self.capture_fes_extension_state(&kinds)?;
        let mut extension_keys = extensions::motion_extensions(&self.data_services.fes_text,
            &self.data_services.fes_controls, &self.data_services.fes_draw, &tasks.fes)?;
        extension_keys.sounds = sound_keys;
        extension_keys.inert.extend(particle_keys);
        extension_keys.inert.extend(panel_keys);
        let motion = self.data_services.fes_motion.capture_checkpoint_with_extensions(
            &tasks.fes, self.script_encoding, &extension_keys)?;
        let mut bytes = COMPONENT_TAG.to_vec();
        bytes.extend_from_slice(&wire::encode_component(&PhysicalComponents {
            version: 7, tasks: snapshot, motion, extensions, physical_sound, particles, save_panels,
        })?);
        Ok(bytes)
    }

    pub(crate) fn prepare_fes_task_state(&self, bytes: &[u8], saved: &ObjectGraphSnapshot,
        tasks: &super::super::native_tasks::NativeTaskStore,
        images: &BTreeMap<u32, RgbaFrame>) -> Result<PreparedFesTaskState> {
        let kinds = tasks.checkpoint_task_kinds();
        let (components, physical_bytes, particle_bytes, panel_bytes) = if bytes.starts_with(COMPONENT_TAG) {
            let parts: PhysicalComponents = wire::decode_component(&bytes[COMPONENT_TAG.len()..])?;
            if parts.version != 7 { bail!("unsupported FES real-bank component version"); }
            (Some(Components { tasks: parts.tasks, motion: parts.motion,
                extensions: parts.extensions }), Some(parts.physical_sound), Some(parts.particles), Some(parts.save_panels))
        } else if !bytes.is_empty() {
            bail!("obsolete FES checkpoint wire lacks real bank consumers; CPU7 is required");
        } else { (None, None, None, None) };
        let bytes = components.as_ref().map_or(bytes, |parts| parts.tasks.as_slice());
        let mut snapshot: Snapshot = if bytes.is_empty() {
            // Legacy host graphs predate type34 support. Keep their wire
            // readable without inventing FES objects or callback state.
            Snapshot { version: 2, slots: BTreeMap::new(),
                platform: wire::encode_component(&super::super::fes_task_platform::FesTaskPlatform::default())?,
                buffered: tasks.capture_buffered_command_state()?, buttons: BTreeMap::new(), controllers: BTreeMap::new(),
                message_owners: BTreeMap::new(),
                globals: crate::fes_globals::FesGlobalStore::default().capture_checkpoint(&kinds)?,
                settings: crate::fes_settings::FesSettingsStore::default().capture_checkpoint(&kinds)?,
                sound: crate::fes_sound::FesSoundStore::default().capture_checkpoint(&kinds)?,
                text_options: (-1, false, 0, 0), font_controls: None }
        } else { wire::decode_component(bytes)? };
        if snapshot.version != 2 || snapshot.slots.len() > 65_536 || snapshot.buttons.len() > 1_000_000 {
            bail!("FES task checkpoint version or count is invalid");
        }
        if snapshot.slots.keys().copied().collect::<BTreeSet<_>>() != kinds.iter()
            .filter_map(|(&id, &kind)| (kind == 34).then_some(id)).collect()
        { bail!("FES task checkpoint and actual type34 records differ"); }
        for (&id, slot) in &mut snapshot.slots {
            let object = slot.object.as_mut().context("saved FES object is borrowed")?;
            object.validate_checkpoint(&kinds)?;
            if object.id != id || slot.context_initialized != object.initialized
                || slot.inputs.len() > 4096 || slot.named_dwords.len() > 1_000_000
            { bail!("FES checkpoint slot and live object metadata differ"); }
            let external = tasks.checkpoint_external_binding(id).context("restored FES external binding is absent")?;
            object.checkpoint_binding(&external, true)?;
            if let (Some(name), Some(runtime)) = (object.name.as_ref(), object.runtime.as_mut())
                && let Some(driver) = runtime.driver.as_mut()
            {
                let resources = &self.scan.resources;
                let requested = String::from_utf8_lossy(&super::cp932_to_utf8(name)).into_owned();
                if let Ok(names) = resources.category_names(b"fes", &requested)
                    && let Some(candidate) = names.iter().find_map(|name| resources.find(name))
                {
                    let island_source = super::is_island_gbk_fes_source(resources, candidate);
                    if (island_source || candidate.size == super::REBELLIONS_MESSAGE_FES_SIZE as u64)
                        && let Ok(bytes) = resources.read_candidate(candidate)
                        && super::parse_document(&bytes).is_ok_and(|document| document == runtime.document)
                    {
                        driver.set_island_gbk_fes_source(island_source);
                        driver.set_rebellions_message_fes_source(
                            super::is_rebellions_message_fes_source(resources, &bytes),
                        );
                    }
                }
            }
            if let Some(driver) = object.runtime.as_ref().and_then(|runtime| runtime.driver.as_ref()) {
                let node = saved.nodes.get(&id).context("FES checkpoint scene node is absent")?;
                if driver.interpreter.local_values() != node.fes_parameters.as_slice() {
                    bail!("FES checkpoint locals differ from the task scene snapshot");
                }
            }
        }
        let mut store = FesTaskStore::default();
        store.slots = snapshot.slots;
        let save_panels = panel_bytes.as_deref().map(|bytes|
            crate::fes_save_panel::FesSavePanelStore::prepare_checkpoint(bytes, &kinds)).transpose()?;
        store.platform = if let Some(panels) = save_panels {
            // Install only in the independent prepared platform. Its wire
            // skips this bank; CPU7 carries the matching explicit codec.
            super::super::fes_task_platform::FesTaskPlatform::prepare_checkpoint_with_save_panels(
                &snapshot.platform, &self.data_services.tasks.fes.platform, &kinds,
                &saved.parents, &saved.input, panels)?
        } else {
            // Only the old empty, pre-FES host component can lack this codec.
            // Never replace an actual live bank/driver with guessed defaults.
            if !self.data_services.tasks.fes.slots.is_empty()
                || !self.data_services.tasks.fes.platform.save_panels.is_empty() {
                bail!("legacy FES component has no SAVEPANEL state; live banks/continuations retained");
            }
            super::super::fes_task_platform::FesTaskPlatform::prepare_checkpoint(
                &snapshot.platform, &self.data_services.tasks.fes.platform, &kinds, &saved.parents, &saved.input)?
        };
        store.hooks = self.data_services.tasks.fes.hooks;
        store.platform.validate_checkpoint_with_save_panels(&store, &kinds, &saved.parents, &saved.input)?;
        let panel_keys = store.validate_checkpoint_save_panels(&store.platform.save_panels)?;
        let encoding = match saved.encoding {
            0 => ScriptTextEncoding::Cp932, 1 => ScriptTextEncoding::Gbk, 2 => ScriptTextEncoding::Utf8,
            _ => bail!("FES checkpoint script encoding is invalid"),
        };
        store.platform.validate_cpu_checkpoint(&store, &kinds, &saved.parents,
            &saved.wrapper_pixels.keys().copied().collect(), encoding)?;
        let extensions = Self::prepare_fes_extension_state(
            components.as_ref().map_or(&[], |parts| parts.extensions.as_slice()),
            tasks, &store, saved, images, snapshot.text_options)?;
        // Prepare resources/VM/PPI before the physical SOUND registry lease.
        // Neither preparation may observe an already committed replacement.
        let particles = particle_bytes.as_deref().map(|bytes|
            crate::fes_particle::FesParticleStore::prepare_checkpoint(
                bytes, &self.scan.resources, &kinds, &saved.parents)).transpose()?;
        if let Some(particles) = &particles {
            particles.validate_fes_links(&store)?;
        } else if kinds.values().any(|kind| *kind == 16) || !self.particles.is_empty()
            || self.data_services.fes_particles.graph_object_count() != 0 {
            bail!("legacy FES component has no physical PARTICLE state; live or saved banks retained");
        }
        let sound = crate::fes_sound::FesSoundStore::prepare_checkpoint(&snapshot.sound, &kinds)?;
        let physical_sound = physical_bytes.as_deref().map(|bytes|
            self.prepare_physical_sound_state(bytes, &kinds, &saved.parents)).transpose()?;
        let mut extension_keys = extensions.motion_extensions(&store)?;
        let particle_keys = particles.as_ref().map(|state|
            state.store().checkpoint_bank_keys().cloned().collect()).unwrap_or_default();
        store.validate_checkpoint_bank_consumers(b"PARTICLE", &particle_keys)?;
        extension_keys.inert.extend(particle_keys);
        extension_keys.inert.extend(panel_keys);
        if let Some(physical) = &physical_sound {
            if wire::encode_component(physical.runtime_audio())? != wire::encode_component(&saved.state.audio)? {
                bail!("FES physical SOUND and host AudioState snapshots differ");
            }
            extension_keys.sounds = sound.validate_physical_checkpoint(physical, &store, encoding)?;
            store.validate_sound_bank_consumers(&extension_keys.sounds)?;
        } else {
            Self::validate_control_sound_checkpoint(&snapshot.sound, &kinds)?;
            if kinds.values().any(|kind| *kind == 13) || !self.sound_channels.is_empty()
                || self.state.audio.handles().next().is_some() || saved.state.audio.handles().next().is_some() {
                bail!("legacy FES component has no physical SOUND state; live or saved audio cannot be replaced");
            }
        }
        let motion_bytes = if let Some(parts) = &components { parts.motion.clone() } else {
            crate::fes_motion::FesMotionStore::default().capture_checkpoint(&store, encoding)?
        };
        let motion = crate::fes_motion::FesMotionStore::prepare_checkpoint_with_extensions(
            &motion_bytes, &store, encoding, &extension_keys)?;
        let buffered = tasks.prepare_buffered_command_state(&snapshot.buffered)?;
        let mut buttons = BTreeMap::new();
        for (id, value) in snapshot.buttons {
            if !matches!(kinds.get(&id), Some(2 | 6)) || value.0.id != id
                || value.0.receiver.is_some_and(|receiver| !kinds.contains_key(&receiver))
            { bail!("FES input-button checkpoint has an invalid task owner"); }
            buttons.insert(id, crate::message_platform::InputButtonRuntime::from_execution_checkpoint(value));
        }
        let mut controllers = BTreeMap::new();
        for (id, value) in snapshot.controllers {
            if kinds.get(&id) != Some(&34) { bail!("FES button controller owner is not type34"); }
            controllers.insert(id, crate::message_platform::FesButtonController { focus: value.0,
                preferred_focus: value.1, pressed: value.2, focus_lock: value.3,
                raw_state: value.4, controller_state: value.5 });
        }
        tasks.validate_checkpoint_message_owners(&snapshot.message_owners)?;
        let globals = crate::fes_globals::FesGlobalStore::prepare_checkpoint(&snapshot.globals, &kinds)?;
        let settings = crate::fes_settings::FesSettingsStore::prepare_checkpoint(&snapshot.settings, &kinds)?;
        if snapshot.text_options.0 >= 0 { bail!("FES text surface allocator must retain its negative namespace"); }
        Ok(PreparedFesTaskState { store, buttons, controllers, buffered, globals, settings, sound, motion, extensions, physical_sound, particles,
            message_owners: snapshot.message_owners, text_options: snapshot.text_options, font_controls: snapshot.font_controls })
    }

    /// Install logical consumers first. The enclosing graph starts physical
    /// playback only after all remaining host fields have been committed.
    pub(crate) fn commit_fes_task_state(&mut self, prepared: PreparedFesTaskState)
        -> PreparedFesPhysicalState {
        let PreparedFesTaskState { mut store, buttons, controllers, buffered, globals, settings, sound, motion, extensions, physical_sound, particles,
            message_owners, text_options, font_controls } = prepared;
        // Lists were already prepared/committed by the independent type15
        // component. Preserve its restored objects rather than its old state.
        store.platform.adopt_checkpoint_lists(&mut self.data_services.tasks.fes.platform);
        self.data_services.tasks.fes = store;
        self.data_services.messages.input_buttons = buttons;
        self.data_services.messages.fes_button_controllers = controllers;
        self.data_services.tasks.commit_buffered_command_state(buffered);
        self.data_services.tasks.commit_checkpoint_message_owners(message_owners);
        self.data_services.fes_globals = globals;
        self.data_services.fes_settings = settings;
        self.data_services.fes_sound = sound;
        self.data_services.fes_motion = motion;
        extensions.commit(&mut self.data_services);
        self.data_services.fes_text.restore_checkpoint_options(text_options);
        self.data_services.message_device.restore_raw_font_controls(font_controls);
        PreparedFesPhysicalState { sound: physical_sound, particles }
    }
}

/// Install after the outer graph has replaced its task tree, image aliases,
/// input, clocks and random streams. All fallible work finished in prepare.
pub(crate) struct PreparedFesPhysicalState {
    sound: Option<crate::sound_task::checkpoint::PreparedPhysicalSoundState>,
    particles: Option<crate::fes_particle::checkpoint::PreparedParticles>,
}
impl PreparedFesPhysicalState {
    pub(crate) fn commit(self, player: &mut HeadlessPlayer) {
        if let Some(particles) = self.particles { particles.commit(player); }
        if let Some(sound) = self.sound { player.commit_physical_sound_state(sound); }
    }
}
