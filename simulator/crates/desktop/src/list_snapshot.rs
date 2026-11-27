//! Exact shared-entry and image-list continuation in portable graph v2.
use super::*;
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_runtime::object_graph::{self as wire, ImageFrameSnapshot, ObjectGraphSnapshot};
use crate::image_manager::{capture_frame, restore_frame};
use crate::fes_task_platform::ImageList;

#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct Rectangle { at: [i32; 2], size: [u32; 2] }
pub(super) fn rect(r: Rect) -> Rectangle { Rectangle { at: [r.x, r.y], size: [r.width, r.height] } }
pub(super) fn unrect(r: &Rectangle) -> Rect { Rect::new(r.at[0], r.at[1], r.size[0], r.size[1]) }

#[derive(Clone, Serialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct Options {
    at: [i32; 2], scale: [u32; 2], opacity: u8, visible: bool,
    clip: Option<Rectangle>, mask: Option<([u32; 2], Vec<u8>)>,
    pivot: [u32; 2], rotation: u32, flip: [bool; 2], blend: u32, linear: bool,
    pub(super) affine: Option<AffinePlacement>,
}

impl<'de> Deserialize<'de> for Options {
    fn deserialize<D: cs2_runtime::graph_serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use cs2_runtime::graph_serde::de::{Error, SeqAccess, Visitor};

        struct OptionsVisitor;
        impl<'de> Visitor<'de> for OptionsVisitor {
            type Value = Options;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("list draw options with an optional affine placement")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
                let missing = || A::Error::custom("truncated list draw options");
                Ok(Options {
                    at: seq.next_element()?.ok_or_else(missing)?,
                    scale: seq.next_element()?.ok_or_else(missing)?,
                    opacity: seq.next_element()?.ok_or_else(missing)?,
                    visible: seq.next_element()?.ok_or_else(missing)?,
                    clip: seq.next_element()?.ok_or_else(missing)?,
                    mask: seq.next_element()?.ok_or_else(missing)?,
                    pivot: seq.next_element()?.ok_or_else(missing)?,
                    rotation: seq.next_element()?.ok_or_else(missing)?,
                    flip: seq.next_element()?.ok_or_else(missing)?,
                    blend: seq.next_element()?.ok_or_else(missing)?,
                    linear: seq.next_element()?.ok_or_else(missing)?,
                    affine: seq.next_element()?.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_seq(OptionsVisitor)
    }
}

pub(super) fn options(o: &ImageDrawOptions, affine: Option<AffinePlacement>) -> Options {
    Options { at: [o.position.x, o.position.y], scale: [o.scale_x.to_bits(), o.scale_y.to_bits()],
        opacity: o.opacity, visible: o.visible, clip: o.clip.map(rect),
        mask: o.mask.as_ref().map(|m| ([m.width(), m.height()], m.alpha().to_vec())),
        pivot: o.pivot.map(f32::to_bits), rotation: o.rotation_radians.to_bits(),
        flip: [o.flip_x, o.flip_y], blend: o.blend_mode as u32,
        linear: o.filter == cs2_renderer::ScaleFilter::Linear, affine }
}
pub(super) fn unoptions(o: &Options) -> Result<ImageDrawOptions> {
    if let Some(affine) = o.affine { affine.validate()?; }
    let mut result = ImageDrawOptions::new(Point::new(o.at[0], o.at[1]));
    result.scale_x = f32::from_bits(o.scale[0]); result.scale_y = f32::from_bits(o.scale[1]);
    result.opacity = o.opacity; result.visible = o.visible; result.clip = o.clip.as_ref().map(unrect);
    result.mask = o.mask.as_ref().map(|(size, bytes)|
        cs2_renderer::AlphaMask::new(size[0], size[1], bytes.clone())).transpose()?;
    result.pivot = o.pivot.map(f32::from_bits); result.rotation_radians = f32::from_bits(o.rotation);
    result.flip_x = o.flip[0]; result.flip_y = o.flip[1];
    result.blend_mode = if o.blend <= 5 { cs2_renderer::BlendMode::from_raw(o.blend) } else {
        cs2_renderer::BlendMode::list_alpha_mask(o.blend - 5).context("saved list blend mode is invalid")?
    };
    result.filter = if o.linear { cs2_renderer::ScaleFilter::Linear } else { cs2_renderer::ScaleFilter::Nearest };
    Ok(result)
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct EntryState {
    key: u32, source: [u32; 3], position: [u32; 2], base: [u32; 2], rotation: u32,
    scale: [u32; 2], opacity: u8, raw_opacity: u32, visible: bool,
    transform: u32, priority: i32, clip: Option<Rectangle>,
    animation: cs2_script::anm::snapshot::ImageAnimationSnapshot,
    frames: BTreeMap<u32, ImageFrameSnapshot>, frame_handles: BTreeMap<u32, u32>,
    selected: Option<u32>, identity: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct UpdateState {
    command: Option<cs2_runtime::TaskCommand>, now: u32, keys: Vec<u32>, cursor: usize,
    compose: bool, signal_ready: bool, entry_ready: bool, resource_changes: Vec<u32>,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct LayerState { frame: ImageFrameSnapshot, options: Options, priority: i32 }

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ListState {
    output: u32, size: [u32; 2], offset: [i32; 2], base_plane: Option<u32>,
    enabled: bool, submit: bool, force: bool, layers: BTreeMap<(u32, String), LayerState>,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot {
    version: u32, shared: Vec<EntryState>, entries: BTreeMap<(u32, u32), u32>,
    objects: BTreeMap<u32, u32>, owners: BTreeMap<u32, u32>,
    surfaces: BTreeMap<u32, tiles::SurfaceSnapshot>,
    invalidated: Vec<(u32, u32)>, backgrounds: BTreeMap<u32, (u32, bool)>,
    updates: BTreeMap<u32, UpdateState>, sounds: BTreeMap<(u32, u32), (u32, u32)>,
    offsets: BTreeMap<u32, [i32; 2]>, switches: BTreeMap<u32, [bool; 3]>,
    lists: BTreeMap<u32, ListState>,
    resource_geometry: BTreeMap<u32, crate::image_manager::ResourceGeometry>,
    planes: Vec<u8>, canvas: Vec<u8>,
    cache: crate::image_manager::ImageCacheSnapshot,
}

pub(crate) struct PreparedImageLists {
    native: ListNativeStore, lists: BTreeMap<u32, ImageList>,
    planes: crate::plane_native::PreparedPlaneVisuals,
    canvas: crate::canvas_native::CanvasNativeState,
    vram_overflow: Option<crate::canvas_native::VramOverflow>,
    cache: crate::image_manager::PreparedImageCache,
}
impl PreparedImageLists {
    /// Borrow the already prepared canonical layers for FES link validation.
    /// No second list/pixel schema or copy is introduced.
    pub(crate) fn checkpoint_lists(&self) -> &BTreeMap<u32, ImageList> {
        &self.lists
    }

    /// FES raw IMAGE holds an actual retained-entry object. Plane proxies
    /// share the allocator domain but cannot satisfy this binding.
    pub(crate) fn has_retained_image_source(&self, handle: u32) -> bool {
        self.native.checkpoint_has_retained_image_source(handle)
    }

    pub(crate) fn object_handles(&self) -> impl Iterator<Item = u32> + '_ {
        self.native.objects.keys().copied().chain(self.planes.object_handles())
    }

    pub(crate) fn prepare_scene_nodes(&mut self, nodes: &mut BTreeMap<u32, crate::SceneNode>) -> Result<()> {
        self.planes.prepare_scene_nodes(nodes)
    }
}

impl ListNativeStore {
    pub(crate) fn checkpoint_has_retained_image_source(&self, handle: u32) -> bool {
        self.objects.contains_key(&handle)
    }
}

impl crate::HeadlessPlayer {
    pub(crate) fn image_list_state_snapshot(&self) -> Result<Vec<u8>> {
        let s = &self.data_services.list_native;
        let mut identities = BTreeMap::new();
        let mut shared = Vec::new();
        let mut reference = |entry: &SharedEntry| -> Result<u32> {
            let pointer = Rc::as_ptr(entry) as usize;
            if let Some(&id) = identities.get(&pointer) { return Ok(id); }
            let id = u32::try_from(shared.len())?;
            let e = entry.try_borrow().context("cannot snapshot an actively borrowed list entry")?;
            shared.push(EntryState {
                key: e.key, source: match e.source { Source::Resource { handle, wrapper } => [0, handle, wrapper],
                    Source::Wrapper(wrapper) => [1, wrapper, 0], Source::Frame => [2, 0, 0],
                    Source::Empty => [3, 0, 0] },
                position: e.position.map(f32::to_bits), base: e.base.map(f32::to_bits),
                rotation: e.rotation.to_bits(), scale: e.scale.map(f32::to_bits),
                opacity: e.opacity, raw_opacity: e.raw_opacity, visible: e.visible,
                transform: e.transform, priority: e.priority, clip: e.clip.map(rect),
                animation: e.animation.execution_snapshot(),
                frames: e.frames.iter().map(|(&id, frame)| (id, capture_frame(frame))).collect(),
                frame_handles: e.frame_handles.clone(),
                selected: e.selected_frame, identity: e.identity,
            });
            identities.insert(pointer, id);
            Ok(id)
        };
        let entries = s.entries.0.iter().map(|(&key, e)| Ok((key, reference(e)?))).collect::<Result<_>>()?;
        let objects = s.objects.iter().map(|(&key, e)| Ok((key, reference(e)?))).collect::<Result<_>>()?;
        let mut holds = BTreeMap::<u32, usize>::new();
        for entry in &shared {
            if let [0, handle, wrapper] = entry.source {
                if !self.image_resources.contains_key(&handle) || !self.image_handles.contains_key(&wrapper) {
                    bail!("image-list source lost its canonical resource or held wrapper");
                }
                *holds.entry(handle).or_default() += 1;
            }
        }
        for (&id, resource) in &self.image_resources {
            if resource.references < holds.get(&id).copied().unwrap_or(0) + usize::from(resource.owner.is_some())
                || resource.owner.is_some_and(|owner| !self.data_services.tasks.records.contains_key(&owner)) {
                bail!("image resource owner/reference count differs from its live holders");
            }
        }
        let lists = self.data_services.tasks.fes.platform.lists.iter().map(|(&id, l)| (id, ListState {
            output: l.output, size: l.size, offset: l.offset, base_plane: l.base_plane,
            enabled: l.enabled, submit: l.submit, force: l.force,
            layers: l.layers.iter().map(|(key, l)| (key.clone(), LayerState {
                frame: capture_frame(&l.frame), options: options(&l.options, l.affine), priority: l.priority,
            })).collect(),
        })).collect();
        let snapshot = Snapshot {
            version: 4, shared, entries, objects, owners: s.object_owners.clone(),
            surfaces: s.surfaces.iter().map(|(&id, s)| (id, s.snapshot())).collect(),
            invalidated: s.invalidated.iter().copied().collect(),
            backgrounds: s.backgrounds.iter().map(|(&id, b)| (id, (b.wrapper, b.active))).collect(),
            updates: s.animation_updates.iter().map(|(&id, u)| (id, UpdateState {
                command: u.command, now: u.now, keys: u.keys.clone(), cursor: u.cursor,
                compose: u.compose, signal_ready: u.signal_ready, entry_ready: u.entry_ready,
                resource_changes: u.resource_changes.clone(),
            })).collect(), sounds: s.sound_bindings.clone(), offsets: s.task_offsets.clone(),
            switches: s.switches.clone(), lists,
            resource_geometry: self.image_resources.iter().map(|(&id, r)| (id, r.geometry)).collect(),
            planes: self.plane_visuals_snapshot()?, canvas: self.data_services.canvas_native
                .graph_snapshot(self.data_services.planes.vram_overflow.as_ref())?,
            cache: self.data_services.image_manager.cache_snapshot(),
        };
        Ok(wire::encode_component(&snapshot)?)
    }

    pub(crate) fn prepare_image_list_state_restore(&self, saved: &ObjectGraphSnapshot,
        images: &BTreeMap<u32, RgbaFrame>, resources: &mut BTreeMap<u32, crate::DesktopImageResource>,
    ) -> Result<PreparedImageLists> {
        let s: Snapshot = wire::decode_component(&saved.image_lists)?;
        if s.version != 4 {
            bail!("unsupported saved image component revision {}; expected 4", s.version);
        }
        if s.shared.len() > 1_000_000 || s.entries.len() > 1_000_000
            || s.objects.len() > 65_536 || s.lists.len() > 1_000_000 {
            bail!("saved image list table exceeds its limits");
        }
        if s.resource_geometry.keys().ne(resources.keys()) {
            bail!("saved resource geometry identities differ from their canonical resources");
        }
        for (&id, geometry) in &s.resource_geometry {
            if geometry.size.iter().any(|&n| n > i32::MAX as u32)
                || u64::from(geometry.size[0]) * u64::from(geometry.size[1]) > 128 * 1024 * 1024 / 4 {
                bail!("saved resource cached geometry exceeds supported image limits");
            }
            resources.get_mut(&id).unwrap().geometry = *geometry;
        }
        let planes = Self::prepare_plane_visuals_restore(&s.planes, saved)?;
        let (canvas, vram_overflow) = crate::canvas_native::CanvasNativeState::prepare_graph_restore(&s.canvas,
            saved.back_buffer.size, self.data_services.image_list_memory)?;
        let cache = crate::image_manager::ImageManager::prepare_cache_restore(s.cache)?;
        let list = |id: &u32| s.lists.contains_key(id)
            && saved.records.get(id).is_some_and(|r| r.kind == 15);
        if s.lists.keys().any(|id| !list(id))
            || saved.nodes.iter().any(|(id, n)| n.kind == 15 && n.initialized && !list(id))
            || s.entries.keys().any(|(id, _)| !list(id))
            || s.surfaces.keys().chain(s.backgrounds.keys()).chain(s.updates.keys()).any(|id| !list(id))
            || s.offsets.keys().chain(s.switches.keys()).any(|id| !saved.records.contains_key(id))
            || s.invalidated.iter().chain(s.sounds.keys()).any(|(id, _)| !list(id))
            || s.owners.iter().any(|(id, owner)| !s.objects.contains_key(id) || !saved.records.contains_key(owner))
            || s.objects.keys().any(|id| *id == 0 || images.contains_key(id) || resources.contains_key(id)) {
            bail!("saved image-list owner, output or entry-object identity is invalid");
        }
        let referenced = s.entries.values().chain(s.objects.values()).copied()
            .collect::<std::collections::BTreeSet<_>>();
        if referenced.iter().any(|&id| id as usize >= s.shared.len())
            || referenced.len() != s.shared.len() {
            bail!("saved shared-entry table has absent or unreachable identities");
        }
        let mut shared = Vec::new();
        let mut holds = BTreeMap::<u32, usize>::new();
        let frame_cache = ListFrameCache::default();
        for e in &s.shared {
            let source = match e.source {
                [0, handle, wrapper] if resources.contains_key(&handle) && images.contains_key(&wrapper) => {
                    *holds.entry(handle).or_default() += 1;
                    Source::Resource { handle, wrapper }
                }
                [1, wrapper, 0] if images.contains_key(&wrapper) => Source::Wrapper(wrapper),
                [2, 0, 0] => Source::Frame,
                [3, 0, 0] if e.frames.is_empty() => Source::Empty,
                _ => bail!("saved list source has no retained resource or wrapper"),
            };
            if e.frames.keys().ne(e.frame_handles.keys())
                || e.frame_handles.values().any(|id| !images.contains_key(id))
                || e.selected.is_some_and(|id| !e.frames.contains_key(&id))
                || (matches!(source, Source::Frame) && e.selected.is_none()) {
                bail!("saved list frame table, wrapper identity or selected frame is invalid");
            }
            let mut frames = BTreeMap::new();
            let mut frame_charges = BTreeMap::new();
            for (&id, frame) in &e.frames {
                let image = &images[&e.frame_handles[&id]];
                let bytes = frame.rgba.len().checked_add(image.pixels().len())
                    .context("saved list frame cache allocation size overflow")?;
                frame_charges.insert(id, frame_cache.reserve(bytes)?);
                frames.insert(id, restore_frame(frame)?);
            }
            shared.push(Rc::new(RefCell::new(Entry {
                key: e.key, source, position: e.position.map(f32::from_bits), base: e.base.map(f32::from_bits),
                rotation: f32::from_bits(e.rotation), scale: e.scale.map(f32::from_bits),
                opacity: e.opacity, raw_opacity: e.raw_opacity, visible: e.visible, transform: e.transform,
                priority: e.priority, clip: e.clip.as_ref().map(unrect),
                animation: ImageAnimation::from_execution_snapshot(&e.animation)?, frames,
                frame_handles: e.frame_handles.clone(), frame_charges,
                selected_frame: e.selected, identity: e.identity,
            })));
        }
        for (id, count) in holds {
            if resources[&id].references < count + usize::from(resources[&id].owner.is_some()) {
                bail!("saved list resource references are smaller than actual holders");
            }
        }
        let reference = |id: &u32| shared.get(*id as usize).cloned().context("saved shared-entry identity is absent");
        let entries = s.entries.iter().map(|(&key, id)| Ok((key, reference(id)?))).collect::<Result<_>>()?;
        let objects = s.objects.iter().map(|(&key, id)| Ok((key, reference(id)?))).collect::<Result<_>>()?;
        let mut lists = BTreeMap::new();
        for (&id, l) in &s.lists {
            if images.get(&l.output).is_none_or(|f| [f.width(), f.height()] != l.size)
                || l.base_plane.is_some_and(|p| saved.nodes.get(&p).is_none_or(|n| n.kind != 2 || !n.initialized)
                    || saved.parents.get(&p) != Some(&id)) {
                bail!("saved image-list output or base plane is absent");
            }
            let layers = l.layers.iter().map(|(key, layer)| Ok((key.clone(), Layer {
                frame: restore_frame(&layer.frame)?, options: unoptions(&layer.options)?, priority: layer.priority,
                affine: layer.options.affine,
            }))).collect::<Result<_>>()?;
            lists.insert(id, ImageList { output: l.output, size: l.size, offset: l.offset,
                base_plane: l.base_plane, enabled: l.enabled, submit: l.submit, force: l.force, layers });
        }
        let mut updates = BTreeMap::new();
        for (&id, u) in &s.updates {
            if u.cursor > u.keys.len() || u.keys.len() > 1_000_000
                || u.command.is_some_and(|c| c.target as u32 != id)
                || u.keys.iter().any(|key| !s.entries.contains_key(&(id, *key)))
                || u.resource_changes.iter().any(|key| !s.entries.contains_key(&(id, *key))) {
                bail!("saved list continuation has invalid keys or cursor");
            }
            if u.cursor == u.keys.len() && (u.signal_ready || u.entry_ready) {
                bail!("saved list continuation has phase flags past its final entry");
            }
            updates.insert(id, AnimationUpdate { command: u.command, now: u.now, keys: u.keys.clone(),
                cursor: u.cursor, compose: u.compose, signal_ready: u.signal_ready,
                entry_ready: u.entry_ready, resource_changes: u.resource_changes.clone() });
        }
        let surfaces = s.surfaces.iter().map(|(&id, surface)| {
            if surface.size != lists[&id].size { bail!("saved list tile surface has the wrong size"); }
            Ok((id, tiles::ListSurface::from_snapshot(surface)?))
        }).collect::<Result<_>>()?;
        if s.backgrounds.values().any(|(wrapper, _)| !images.contains_key(wrapper)) {
            bail!("saved list background wrapper is absent");
        }
        Ok(PreparedImageLists { native: ListNativeStore {
            entries: EntryBindings(entries), objects, object_owners: s.owners, frame_cache, surfaces,
            invalidated: s.invalidated.into_iter().collect(), backgrounds: s.backgrounds.into_iter()
                .map(|(id, (wrapper, active))| (id, Background { wrapper, active })).collect(),
            animation_updates: updates, sound_bindings: s.sounds, task_offsets: s.offsets, switches: s.switches,
        }, lists, planes, canvas, vram_overflow, cache })
    }

    pub(crate) fn install_image_list_state_restore(&mut self, prepared: PreparedImageLists) {
        self.install_plane_visuals_restore(prepared.planes);
        self.data_services.planes.vram_overflow = prepared.vram_overflow;
        self.data_services.canvas_native = prepared.canvas;
        self.data_services.image_manager.install_cache_restore(prepared.cache);
        self.data_services.list_native = prepared.native;
        self.data_services.tasks.fes.platform.lists = prepared.lists;
    }
}
