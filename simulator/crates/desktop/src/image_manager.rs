//! Ordinary 681/682 and the decoded-image manager shared with ordinary 111.
//! Cache records own independent pixels; neither task wrappers nor their
//! shared-pixel aliases can mutate a cached decode.
use std::collections::{BTreeMap, VecDeque};
use std::cell::RefCell;
use std::sync::Arc;

use cs2_formats::{ConfigXml, HgFile, HgFrame};
use cs2_renderer::{AnimationFrameSelection, FrameMetadata, HgFrameAttributes, HgFrameDecoder, RgbaFrame};
use cs2_runtime::{ResourceCandidate, RuntimeError};
use cs2_script::{NativeReturn, Result, Vm};

use super::{LoggingHost, cp932_to_utf8, expand_kcs_format, native_state_error, native_state_error_owned, read_u32, trim_nul};
use super::perf_detail::{ImageSourceStage, ImageSourceStep, SlowTask};

const MAX_FILE: usize = 64 * 1024 * 1024;
const MAX_PIXELS: usize = 128 * 1024 * 1024;
const MAX_CACHE: usize = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;

/// Native resource cache is independent from the current pixel surface.
#[derive(Debug, Clone, Copy, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ResourceGeometry {
    pub size: [u32; 2], pub canvas: [u32; 2], pub offset: [i32; 2], pub base: [i32; 2],
}
impl ResourceGeometry {
    pub(crate) fn from_frame(frame: &RgbaFrame) -> Self {
        let m = frame.metadata();
        let canvas = native_canvas_dimensions(frame);
        Self { size: if frame.is_empty() { [0; 2] } else { [frame.width(), frame.height()] },
            canvas: if canvas.contains(&0) { [0; 2] } else { canvas },
            offset: if frame.is_empty() { [0; 2] } else { m.offset }, base: m.base }
    }
}

/// Native 5e4770 resolves each zero axis independently. Keep this separate
/// from raw frame metadata: resize uses the stored axes, not this getter.
pub(crate) fn native_canvas_dimensions(frame: &RgbaFrame) -> [u32; 2] {
    let raw = frame.metadata().canvas_size;
    [if raw[0] == 0 { frame.width() } else { raw[0] },
        if raw[1] == 0 { frame.height() } else { raw[1] }]
}

#[path = "image_resource_transform.rs"]
mod transforms;
#[path = "image_resource_state.rs"]
mod state;
#[path = "image_resource_surface.rs"]
mod resource_surface;
#[path = "image_island_patch.rs"]
mod island_patch;
#[path = "image_material_async.rs"]
mod material_async;
pub(super) use state::{ImageCacheSnapshot, PreparedImageCache, PreparedResourceLayers, capture_frame, restore_frame};

pub(super) struct ImageManager {
    /// Interactive host policy, deliberately absent from saved image state.
    /// Offline/default players retain deterministic synchronous native710.
    async_enabled: bool,
    image_loads: material_async::Loads,
    prepared_material: Option<material_async::Ready>,
    initialized: bool,
    enabled: bool,
    capacity: usize,
    /// Constructor initializes +0x108 to one. Native 682 stores all bits.
    force_cache: u32,
    resident_byte_budget: bool,
    fast_forward: bool,
    bytes: usize,
    rgba_bytes: usize,
    frames: BTreeMap<String, (usize, RgbaFrame)>,
    /// Front is newest. A normal non-caching hit does not touch this deque.
    recency: VecDeque<String>,
    animation_aliases: cs2_formats::animation_alias::AnimationAliases,
    aliases_initialized: bool,
    /// Native resource layers own their frame tables and ANM interpreters.
    /// Cache eviction and releasing a task/list reference cannot detach them.
    resource_layers: BTreeMap<u32, ResourceLayers>,
    /// Derived from read-only game/DLL/pack identities, not decoded pixels or
    /// saved execution state. Restored cache/layer frames already contain the
    /// committed replacement; they must not run this decoder hook again.
    island_patch_enabled: Option<bool>,
    encoded_batch: RefCell<Option<EncodedImageBatch>>,
}

#[derive(Default)]
struct EncodedImageBatch {
    bytes: usize,
    assets: Vec<(ResourceCandidate, Arc<Vec<u8>>)>,
}

impl Default for ImageManager {
    fn default() -> Self {
        Self {
            async_enabled: false,
            image_loads: Default::default(), prepared_material: None,
            initialized: false, enabled: false, capacity: 0, force_cache: 1,
            resident_byte_budget: false, fast_forward: false,
            bytes: 0, rgba_bytes: 0, frames: BTreeMap::new(), recency: VecDeque::new(),
            animation_aliases: Default::default(), aliases_initialized: false,
            resource_layers: BTreeMap::new(),
            island_patch_enabled: None,
            encoded_batch: Default::default(),
        }
    }
}

impl super::HeadlessPlayer {
    /// Prepare native710 images off-thread and resume the same logical frame.
    /// Offline players keep the synchronous native contract unless opted in.
    pub fn enable_host_async_images(&mut self) {
        self.data_services.image_manager.async_enabled = true;
    }
}

impl ImageManager {
    pub(super) fn enable_resident_byte_budget(&mut self) {
        self.resident_byte_budget = true;
        self.trim_resident(0);
    }

    pub(super) fn set_fast_forward(&mut self, active: bool) {
        self.fast_forward = active;
        if active { self.trim_resident(0); }
    }

    fn resident_limit(&self) -> usize {
        if self.fast_forward { self.capacity.min(32 * 1024 * 1024) } else { self.capacity }
    }

    fn evict_oldest(&mut self) -> bool {
        let Some(oldest) = self.recency.pop_back() else { return false; };
        let (bytes, frame) = self.frames.remove(&oldest).expect("image cache recency entry");
        self.bytes -= bytes;
        self.rgba_bytes -= frame.pixels().len();
        true
    }

    fn trim_resident(&mut self, incoming: usize) {
        while self.rgba_bytes.saturating_add(incoming) > self.resident_limit() {
            if !self.evict_oldest() { break; }
        }
    }

    pub(super) fn pending_image_owners(&self) -> impl Iterator<Item = u32> + '_ { self.image_loads.owners() }
    pub(super) fn has_pending_image_loads(&self) -> bool { self.image_loads.has_pending() }
    pub(super) fn cancel_image_load(&mut self, owner: u32) { self.image_loads.cancel(owner); }
    /// 69aa68..69aa77 uses the same wrapper as both a sole static source and
    /// the current image. Current-only writes must reach that retained source
    /// too; a composite's separate list output must never overwrite its layers.
    pub(super) fn synchronize_single_static_source(&mut self, handle: u32, image: &RgbaFrame) {
        if let Some(layers) = self.resource_layers.get_mut(&handle)
            && let [layer] = layers.layers.as_mut_slice()
            && layer.animation.timeline().is_none() {
            layer.frames.insert(layer.selected, image.clone());
        }
    }

    pub(super) fn release_resource_layers(&mut self, handle: u32) {
        self.resource_layers.remove(&handle);
    }

    pub(super) fn resource_has_animation(&self, handle: u32) -> bool {
        self.resource_layers.get(&handle).is_some_and(|layers|
            layers.layers.iter().any(|layer| layer.animation.timeline().is_some()))
    }
    fn key(name: &str, id: u32) -> String {
        // PE uppercases a UTF-16 path before appending signed decimal :ID.
        // CP932 fullwidth/locale case folding remains a documented boundary.
        format!("{}:{}", name.to_ascii_uppercase(), id as i32)
    }

    fn get(&self, name: &str, id: u32) -> Option<RgbaFrame> {
        self.frames.get(&Self::key(name, id)).map(|(_, frame)| cache_copy(frame))
    }

    fn shared_frame(&self, name: &str, id: u32) -> Option<RgbaFrame> {
        self.frames.get(&Self::key(name, id)).map(|(_, frame)| frame.clone())
    }

    /// The layer planner uses the borrowed-cache lookup, which never
    /// refreshes recency. Its pixel getters supply fallback canvas dimensions.
    fn geometry(&self, name: &str, id: u32) -> Option<[u32; 9]> {
        self.frames.get(&Self::key(name, id)).map(|(_, frame)| {
            let meta = frame.metadata();
            let canvas = native_canvas_dimensions(frame);
            [
                meta.offset[0] as u32, meta.offset[1] as u32, frame.width(), frame.height(),
                canvas[0], canvas[1],
                u32::from(meta.bit_depth) / 8, meta.base[0] as u32, meta.base[1] as u32,
            ]
        })
    }

    fn remember(&mut self, name: &str, id: u32, frame: &RgbaFrame) -> Result<()> {
        if !self.enabled { return Ok(()); }
        let key = Self::key(name, id);
        if self.frames.contains_key(&key) {
            self.recency.retain(|candidate| candidate != &key);
            self.recency.push_front(key);
            return Ok(());
        }
        let cost = cache_cost(frame)?;
        if self.resident_byte_budget {
            let rgba = frame.pixels().len();
            if rgba > self.resident_limit() || cost > MAX_CACHE { return Ok(()); }
            self.trim_resident(rgba);
        } else {
            // The original manager allows a sole entry larger than its native budget.
            while self.bytes.checked_add(cost).is_none_or(|sum| sum >= self.capacity) {
                if !self.evict_oldest() { break; }
            }
        }
        let total = self.bytes.checked_add(cost).filter(|&bytes| bytes <= MAX_CACHE)
            .ok_or_else(|| native_state_error("decoded image cache exceeds 256 MiB"))?;
        if self.frames.len() >= MAX_ENTRIES {
            return Err(native_state_error("decoded image cache exceeds one million entries"));
        }
        let rgba_total = self.rgba_bytes.checked_add(frame.pixels().len())
            .filter(|&bytes| bytes <= MAX_CACHE)
            .ok_or_else(|| native_state_error("decoded image cache RGBA storage exceeds 256 MiB"))?;
        self.frames.insert(key.clone(), (cost, cache_copy(frame)));
        self.recency.push_front(key);
        self.bytes = total;
        self.rgba_bytes = rgba_total;
        Ok(())
    }
}

impl LoggingHost<'_> {
    pub(super) fn with_encoded_image_batch<T>(&mut self, operation: impl FnOnce(&mut Self) -> T) -> T {
        let batch = self.data_services.image_manager.encoded_batch.get_mut();
        if batch.is_some() {
            return operation(self);
        }
        *batch = Some(EncodedImageBatch::default());
        let result = operation(self);
        *self.data_services.image_manager.encoded_batch.get_mut() = None;
        result
    }

    pub(super) fn read_encoded_image_candidate(
        &self, candidate: &ResourceCandidate,
    ) -> cs2_runtime::RuntimeResult<Arc<Vec<u8>>> {
        let mut batch = self.data_services.image_manager.encoded_batch.borrow_mut();
        if let Some(batch) = batch.as_ref()
            && let Some((_, bytes)) = batch.assets.iter().find(|(source, _)| source == candidate)
        {
            return Ok(bytes.clone());
        }
        let bytes = Arc::new(self.resources.read_candidate(candidate)?);
        if let Some(batch) = batch.as_mut()
            && batch.assets.len() < 256
            && batch.bytes + bytes.len() <= 24 * 1024 * 1024
        {
            batch.bytes += bytes.len();
            batch.assets.push((candidate.clone(), bytes.clone()));
        }
        Ok(bytes)
    }

    /// Ordinary710 shares the category, HG decoder and layer compositor with
    /// the image manager, rather than the legacy direct CST filename guesses.
    pub(super) fn load_image_resource_native710(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let entered = std::time::Instant::now();
        let owner = self.current_task_id;
        if self.data_services.image_manager.image_loads.saved(owner) {
            return self.poll_image_resource710();
        }
        if args.len() != 4 {
            return Err(native_state_error("native 710 requires exactly one filename address"));
        }
        let address = read_u32(args, 0)?;
        match vm.read(address, 1) {
            Err(cs2_script::ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Value(0)),
            other => { other?; }
        }
        let expanded = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
        let raw = trim_nul(&expanded);
        if raw.is_empty() { return Ok(NativeReturn::Value(0)); }
        let decoded = super::sound_native::decode_name(self.text_encoding, raw)?;
        // The original Unicode-mode wrapper converts to its CP932 scratch
        // buffer before category completion. Reject replacement characters.
        let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(&decoded);
        if invalid || encoded.len() >= 260 {
            return Err(native_state_error("native 710 name exceeds its CP932 buffer or is unencodable"));
        }
        if !self.data_services.image_manager.async_enabled {
            let names = self.resources.category_names(b"image", &decoded)
                .map_err(|_| native_state_error("native 710 image category resolution failed"))?;
            let Some(name) = names.first() else { return Ok(NativeReturn::Value(0)); };
            let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(name);
            if invalid { return Err(native_state_error("native 710 completed name is not CP932")); }
            let (name, id) = super::image_pixels::image_reference(&encoded)?;
            let name = super::sound_native::decode_name(super::ScriptTextEncoding::Cp932, &name)?;
            return self.commit_image_resource710(&name, id);
        }
        let warm = self.data_services.ordinary.image_predecode_snapshot();
        let manager = &mut self.data_services.image_manager;
        let aliases = manager.aliases_initialized.then(|| manager.animation_aliases.clone());
        let cached = manager.frames.keys().cloned().collect();
        manager.image_loads.begin(owner, decoded, self.save_root.to_owned(), aliases, manager.initialized, cached, warm)?;
        // Even a very fast worker cannot complete in the initial invocation.
        // Retain the KCS native PC and evaluated filename before any I/O.
        let resources = self.data_services.sound_async.resources.get_or_insert_with(|| Arc::new(self.resources.clone())).clone();
        self.data_services.image_manager.image_loads.poll(owner, resources)?;
        super::perf_detail::visual_note("image_material", format!("event=queued owner={owner} enqueue_us={}", entered.elapsed().as_micros()));
        Ok(NativeReturn::Retry)
    }

    fn poll_image_resource710(&mut self) -> Result<NativeReturn> {
        let started = std::time::Instant::now();
        let owner = self.current_task_id;
        let resources = self.data_services.sound_async.resources.get_or_insert_with(|| Arc::new(self.resources.clone())).clone();
        let Some(mut ready) = self.data_services.image_manager.image_loads.poll(owner, resources)? else { return Ok(NativeReturn::Retry); };
        let Some((name, id)) = ready.name.take() else { return Ok(NativeReturn::Value(0)); };
        let manager = &mut self.data_services.image_manager;
        if !manager.aliases_initialized && let Some(aliases) = ready.aliases.take() {
            manager.animation_aliases = aliases; manager.aliases_initialized = true;
        }
        if !manager.initialized && let Some((enabled, capacity)) = ready.configuration {
            manager.enabled = enabled; manager.capacity = capacity; manager.initialized = true;
        }
        let detail = format!("owner={owner} resource={name:?} read_us={} decode_us={} background_us={} source_bytes={} decoded_bytes={} prewarm_hits={}",
            ready.read_us, ready.decode_us, ready.wall_us, ready.source_bytes, ready.decoded_bytes, ready.warm_hits);
        manager.prepared_material = Some(ready);
        let result = self.commit_image_resource710(&name, id);
        self.data_services.image_manager.prepared_material = None;
        let detail = format!("{detail} commit_us={}", started.elapsed().as_micros());
        super::perf_detail::visual_note("image_material", format!("event=completed {detail}"));
        self.events.push(super::LogicEvent { frame: self.frame, kind: "image-material".into(),
            detail });
        result
    }

    fn commit_image_resource710(&mut self, name: &str, id: u32) -> Result<NativeReturn> {
        let (image, mut layers) = self.load_resource_layers710(name, id)?;
        if let Some(layers) = &mut layers && layers.surface.is_some() {
            self.ensure_canvas_state()
                .map_err(|_| native_state_error("resource list tile configuration failed"))?;
            layers.surface = resource_surface::create(&layers.layers, &layers.geometry,
                self.data_services.canvas_native.tile_size())?;
        }
        let Some(image) = image else {
            return Ok(NativeReturn::Value(0));
        };
        // A single-file resource keeps its file Alpha classification; a
        // composite scans the actual result, whose file Alpha flag is zero.
        let uses_alpha = if layers.as_ref().is_some_and(|state|
            state.layers.iter().any(|layer| layer.animation.timeline().is_some()))
            || image_plan_components(name).0.len() > 1 {
            image.has_alpha()
        } else { image.metadata().alpha_flag != 0 };
        let attributes = image.hg_attributes().iter()
            .map(|(&id, &values)| (u32::from(id), values)).collect();
        let handle = self.allocate_handle();
        self.image_resources.insert(handle, super::DesktopImageResource {
            geometry: ResourceGeometry::from_frame(&image),
            image, owner: Some(self.current_task_id), references: 1,
            has_source_layer: true, updated: true, uses_alpha, auxiliary: 0, attributes,
        });
        if let Some(layers) = layers {
            self.data_services.image_manager.resource_layers.insert(handle, layers);
        }
        Ok(NativeReturn::Value(handle))
    }

    fn load_resource_layers710(&mut self, name: &str, requested: u32)
        -> Result<(Option<RgbaFrame>, Option<ResourceLayers>)> {
        let (parts, extension) = image_plan_components(name);
        if parts.len() <= 1 {
            let Some(anm) = self.resolve_image_animation_resource(name)? else {
                let image = self.load_managed_image(name, requested, false)?;
                let layers = image.as_ref().map(|frame| ResourceLayer::new(
                    BTreeMap::from([(requested, frame.clone())]), Default::default()))
                    .transpose()?.map(|layer| ResourceLayers::new(vec![layer])).transpose()?;
                return Ok((image, layers));
            };
            let Some(layer) = self.load_resource_animation_layer(name, &anm)? else {
                return Ok((None, None));
            };
            let layers = ResourceLayers::new(vec![layer])?;
            return Ok((layers.compose()?, Some(layers)));
        }
        if parts.len() > 65 {
            return Err(native_state_error("native 710 exceeds 64 layer selectors"));
        }
        let selector_text = parts.join(",");
        let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(&selector_text);
        if invalid || encoded.len() >= 256 {
            return Err(native_state_error("native 710 selector table exceeds its CP932 buffer"));
        }
        // 69b7c9..69bf5b flushes each static run at an ANM transition and
        // flushes the final run. Keep that intermediate image: flattening
        // every static selector separately changes integer Alpha rounding.
        let mut layers = Vec::new();
        let mut selectors = vec!["0".to_owned(); parts.len() - 1];
        let mut static_end = None;
        for (index, selector) in parts[1..].iter().enumerate() {
            let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(selector);
            let first = encoded[0];
            if invalid || !first.is_ascii() {
                return Err(native_state_error("native 710 layer selector is not an ASCII first byte"));
            }
            if first == b'0' { continue; }
            let physical = format!("{}_{}{}.{}", parts[0], "0".repeat(index), char::from(first), extension);
            if let Some(anm) = self.resolve_image_animation_resource(&physical)? {
                self.flush_resource_static_layers(parts[0], extension, &mut selectors,
                    static_end.take(), &mut layers)?;
                if let Some(layer) = self.load_resource_animation_layer(&physical, &anm)? {
                    layers.push(layer);
                }
            } else {
                selectors[index] = char::from(first).to_string();
                static_end = Some(index + 1);
            }
        }
        self.flush_resource_static_layers(parts[0], extension, &mut selectors,
            static_end, &mut layers)?;
        if layers.is_empty() { return Ok((None, None)); }
        let layers = ResourceLayers::new(layers)?;
        Ok((layers.compose()?, Some(layers)))
    }

    fn flush_resource_static_layers(&mut self, base: &str, extension: &str,
        selectors: &mut [String], end: Option<usize>, layers: &mut Vec<ResourceLayer>) -> Result<()> {
        if let Some(end) = end {
            let name = format!("{base},{}.{}", selectors[..end].join(","), extension);
            if let Some(frame) = self.load_managed_image_plan(&name, false)? {
                layers.push(ResourceLayer::new(BTreeMap::from([(0, frame)]), Default::default())?);
            }
            selectors.fill("0".to_owned());
        }
        Ok(())
    }

    fn load_resource_animation_layer(&mut self, name: &str, anm_name: &str) -> Result<Option<ResourceLayer>> {
        let Some((path, bytes)) = self.read_shared_image_asset(name, false)? else { return Ok(None); };
        let mut frames = BTreeMap::new();
        if bytes.starts_with(b"HG-3") {
            let file = {
                let _timing = super::measure_host_phase(super::HostPhase::Decode);
                HgFile::parse(&bytes)
                    .map_err(|_| native_state_error("native 710 animated HG directory is malformed"))?
            };
            let mut indices = BTreeMap::new();
            for (index, source) in file.frames.iter().enumerate() {
                // Native lookup uses the first directory entry for a duplicate ID.
                indices.entry(source.id).or_insert(index);
            }
            if indices.is_empty() {
                return Ok(None);
            }
            let decoder = {
                let _timing = super::measure_host_phase(super::HostPhase::Decode);
                HgFrameDecoder::new(&bytes, &file)
                    .map_err(|_| native_state_error("image manager HG frame decode failed"))?
            };
            let mut total = 0usize;
            for (id, index) in indices {
                let frame = {
                    let _timing = super::measure_host_phase(super::HostPhase::Decode);
                    match self.data_services.image_manager.prepared_material.as_ref().and_then(|ready| ready.frame(&bytes, id)) {
                        Some(result) => result?.ok_or_else(|| native_state_error("native 710 prepared animation frame is missing"))?,
                        None => checked_hg_frame(decoder.decode(index))?,
                    }
                };
                let frame = self.apply_managed_hg3_patch(&bytes, id, Some(&file.frames[index]), frame)?;
                total = total.checked_add(frame.pixels().len()).filter(|&n| n <= MAX_CACHE)
                    .ok_or_else(|| native_state_error("native 710 retained frames exceed 256 MiB"))?;
                frames.insert(id, name_image_frame(frame, &path, id));
            }
        } else if let Some(frame) = self.decode_named_image_frame(&bytes, 0, &path)? {
            if frame.pixels().len() > MAX_CACHE {
                return Err(native_state_error("native 710 retained frames exceed 256 MiB"));
            }
            frames.insert(0, frame);
        }
        if frames.is_empty() { return Ok(None); }
        let mut animation = cs2_script::ImageAnimation::default();
        if let Some(bytes) = self.read_managed_image_bytes(anm_name)? {
            // The original keeps its initial frame when companion loading
            // fails. A malformed nonempty body is an explicit diagnostic here.
            animation.load(&bytes, *frames.last_key_value().unwrap().0)
                .map_err(|_| native_state_error("native 710 companion ANM is malformed"))?;
            animation.reset_time_on_update = true;
        }
        ResourceLayer::new(frames, animation).map(Some)
    }

    pub(super) fn release_image_resource_layers(&mut self, handle: u32) {
        self.data_services.image_manager.resource_layers.remove(&handle);
    }

    pub(super) fn set_image_resource_animation(&mut self, handle: u32, playing: i32, _now: u32) {
        if let Some(layers) = self.data_services.image_manager.resource_layers.get_mut(&handle) {
            for layer in &mut layers.layers {
                layer.animation.playing = playing;
                // 6a0b20 writes only +50; resume preserves ANM phase. The
                // attach marker resets origin at its first actual update.
            }
        }
    }

    pub(super) fn set_image_resource_animation_variable(&mut self, handle: u32, index: usize, value: i32) {
        if let Some(layers) = self.data_services.image_manager.resource_layers.get_mut(&handle) {
            for layer in &mut layers.layers {
                if let Some(slot) = layer.animation.timeline_mut()
                    .and_then(|timeline| timeline.interpreter.variables.get_mut(index)) { *slot = value; }
            }
        }
    }

    pub(super) fn restart_image_resource_animation(&mut self, handle: u32) {
        if let Some(layers) = self.data_services.image_manager.resource_layers.get_mut(&handle) {
            layers.pending = Some(ResourceAnimationRound {
                now: u32::MAX, cursor: 0, restart: true, started: false, changed: false,
            });
        }
    }

    pub(super) fn take_image_resource_animation_change(&mut self, handle: u32) -> bool {
        self.data_services.image_manager.resource_layers.get_mut(&handle)
            .is_some_and(|layers| std::mem::take(&mut layers.changed))
    }

    /// 6a1050 reaches 69d640 from drawing as well as from ANM updates. This
    /// flush only consumes retained dirty tiles; it never advances a timeline.
    pub(super) fn redraw_image_resource_surface(&mut self, handle: u32) -> Result<()> {
        let Some(layers) = self.data_services.image_manager.resource_layers.get_mut(&handle) else {
            return Ok(());
        };
        let Some(surface) = &layers.surface else { return Ok(()); };
        if !surface.dirty.iter().any(Option::is_some) { return Ok(()); }
        if layers.pending.is_some() {
            return Err(native_state_error("finish the pending resource ANM round before drawing"));
        }
        let resource = self.image_resources.get_mut(&handle)
            .ok_or_else(|| native_state_error("resource list output was released before drawing"))?;
        let image = resource_surface::redraw(&layers.layers, surface, &resource.image)?;
        resource.attributes = image.hg_attributes().iter()
            .map(|(&id, &values)| (u32::from(id), values)).collect();
        resource.uses_alpha = image.has_alpha();
        resource.image = image;
        resource.updated = true;
        layers.surface.as_mut().unwrap().dirty.fill(None);
        layers.changed = true;
        self.synchronize_list_resource(handle)
            .map_err(|_| native_state_error("resource dirty-tile wrapper synchronization failed"))
    }

    /// Called through the existing list source update, never a second host
    /// clock. A yielded round resumes at its exact layer and instruction.
    pub(super) fn update_image_resource_animation(&mut self, handle: u32, now: u32,
        budget: usize) -> Result<bool> {
        let Some(layers) = self.data_services.image_manager.resource_layers.get_mut(&handle) else {
            return Ok(true);
        };
        let round = layers.pending.get_or_insert(ResourceAnimationRound {
            now, cursor: 0, restart: false, started: false, changed: false,
        });
        while let Some(layer) = layers.layers.get_mut(round.cursor) {
            let reset = layer.frames.contains_key(&10000);
            let done = if round.restart && !round.started {
                round.started = true;
                layer.animation.restart_with_random(budget, reset, &mut || self.script_random.next_integer())
            } else {
                layer.animation.update_with_random(round.now, budget, reset, &mut || self.script_random.next_integer())
            }.map_err(|_| native_state_error("native 710 internal ANM instruction is invalid"))?;
            if !done { return Ok(false); }
            if let Some(frame) = layer.animation.frame_change() {
                // Missing frame IDs retain the previous image, but display
                // position/visibility/opacity changes still invalidate it.
                if layer.frames.contains_key(&(frame as u32)) { layer.selected = frame as u32; }
                round.changed = true;
                if let Some(surface) = &mut layers.surface {
                    resource_surface::invalidate(surface, layer, round.cursor, true)?;
                }
            }
            round.cursor += 1;
            round.started = false;
        }
        let changed = round.changed;
        layers.pending = None;
        if changed || layers.surface.as_ref().is_some_and(|s| s.dirty.iter().any(Option::is_some)) {
            let image = if let Some(surface) = &layers.surface {
                let resource = self.image_resources.get(&handle)
                    .ok_or_else(|| native_state_error("animated image resource was released mid-update"))?;
                Some(resource_surface::redraw(&layers.layers, surface, &resource.image)?)
            } else { layers.compose()? };
            if let Some(image) = image {
                layers.changed = true;
                let resource = self.image_resources.get_mut(&handle)
                    .ok_or_else(|| native_state_error("animated image resource was released mid-update"))?;
                resource.uses_alpha = image.has_alpha();
                resource.attributes = image.hg_attributes().iter()
                    .map(|(&id, &values)| (u32::from(id), values)).collect();
                resource.image = image;
                resource.updated = true;
            }
            if let Some(surface) = &mut layers.surface { surface.dirty.fill(None); }
            self.synchronize_list_resource(handle)
                .map_err(|_| native_state_error("internal ANM list pixel synchronization failed"))?;
        }
        Ok(true)
    }

    pub(super) fn image_manager_native(
        &mut self, index: u32, args: &[u8], vm: &Vm,
    ) -> Result<NativeReturn> {
        match index {
            430 => {
                let expanded = expand_kcs_format(vm, &vm.read_string(read_u32(args, 0)?)?, 0)?;
                let name = trim_nul(&expanded);
                if name.is_empty() { return Ok(NativeReturn::Value(0)); }
                if name.len() >= 260 {
                    return Err(native_state_error("native 430 image name exceeds its 259-byte buffer"));
                }
                let name = super::ScriptTextEncoding::Cp932.decode(name);
                let names = self.resources.category_names(b"image", &name)
                    .map_err(|_| native_state_error("native 430 image category resolution failed"))?;
                for name in &names {
                    if self.image_is_animation_resource(name)? {
                        return Ok(NativeReturn::Value(1));
                    }
                }
                Ok(NativeReturn::Value(0))
            }
            682 => {
                self.data_services.image_manager.force_cache = read_u32(args, 0)?;
                Ok(NativeReturn::Continue)
            }
            460 | 681 => {
                let expanded = expand_kcs_format(vm, &vm.read_string(read_u32(args, 0)?)?, 0)?;
                let name = trim_nul(&expanded);
                if name.len() >= 260 {
                    return Err(native_state_error("native 681 image name exceeds 259 CP932 bytes"));
                }
                // The wrapper formats and resolves one complete name. It
                // does not use native 111's ASCII-space list parser.
                let name = String::from_utf8(cp932_to_utf8(name)).expect("CP932 conversion emits UTF-8");
                let names = self.resources.category_names(b"image", &name)
                    .map_err(|_| native_state_error("native 681 image category resolution failed"))?;
                let resolved = names.first()
                    .ok_or_else(|| native_state_error("native 681 category resolved no image names"))?;
                // A real decode is required even if image caching is disabled;
                // successful 681 drops its temporary wrapper after caching.
                let image = self.load_managed_image_plan(resolved, index == 681)?;
                if index == 681 {
                    Ok(NativeReturn::Continue)
                } else {
                    // 460 returns its wrapper without the task +0xdc owner
                    // registration performed by ordinary 111/364.
                    Ok(NativeReturn::Value(match image {
                        Some(frame) => self.register_image_wrapper(frame, false)?,
                        None => 0,
                    }))
                }
            }
            _ => Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }
    }

    fn initialize_animation_aliases(&mut self) -> Result<()> {
        if self.data_services.image_manager.aliases_initialized { return Ok(()); }
        let aliases = match self.cst_resource(b"image", b"anmtable.csv")? {
            Some((_, bytes)) => cs2_formats::animation_alias::AnimationAliases::parse(&bytes)
                .map_err(|_| native_state_error("image manager anmtable.csv is malformed or oversized"))?,
            None => Default::default(),
        };
        self.data_services.image_manager.animation_aliases = aliases;
        self.data_services.image_manager.aliases_initialized = true;
        Ok(())
    }

    fn ensure_image_manager(&mut self) -> Result<()> {
        if self.data_services.image_manager.initialized { return Ok(()); }
        self.initialize_animation_aliases()?;
        let mut megabytes = 32u32;
        if let Some(startup) = self.read_legacy_sound_xml("startup.xml")
            .map_err(|_| native_state_error("image cache startup XML read/parse failed"))? {
            megabytes = startup.integer(&[b"document", b"APP", b"imgcache"], 32) as u32;
        }
        let path = self.save_root.join("setup.xml");
        match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > MAX_FILE {
                    return Err(native_state_error("image cache private setup.xml exceeds 64 MiB"));
                }
                let user = ConfigXml::parse_runtime(&bytes)
                    .map_err(|_| native_state_error("image cache private setup.xml is malformed"))?;
                megabytes = user.integer(&[b"document", b"ETC", b"imgcache"], megabytes as i32) as u32;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(native_state_error("image cache private setup.xml read failed")),
        }
        let capacity = megabytes.wrapping_shl(20) as usize;
        if capacity > MAX_CACHE {
            return Err(native_state_error("configured image cache exceeds portable 256 MiB limit"));
        }
        let manager = &mut self.data_services.image_manager;
        manager.enabled = megabytes != 0;
        manager.capacity = capacity;
        manager.initialized = true;
        Ok(())
    }

    /// Independent pixels on every hit, matching wrapper::copy (0x5e1200).
    /// Cache lookup remains active when force_cache/request_cache are zero;
    /// those flags govern insertion and recency refresh, not lookup.
    pub(super) fn load_managed_image(
        &mut self, name: &str, id: u32, request_cache: bool,
    ) -> Result<Option<RgbaFrame>> {
        self.load_managed_frame(name, id, request_cache, false)
    }

    /// FES projections only replace whole frames. Sharing decoded bytes
    /// preserves their texture identity without exposing native wrappers.
    pub(super) fn load_managed_image_projection(&mut self, name: &str, id: u32) -> Result<Option<RgbaFrame>> {
        self.load_managed_frame(name, id, false, true)
    }

    fn load_managed_frame(
        &mut self, name: &str, id: u32, request_cache: bool, share_pixels: bool,
    ) -> Result<Option<RgbaFrame>> {
        let task = self.current_task_id;
        let kind = self.scene_nodes.get(&task).map_or(-1, |node| node.kind as i32);
        let scope = |operation| SlowTask::new(task, || kind, operation).named(name.as_bytes(), None).code(id);
        if !self.data_services.image_manager.initialized {
            let _detail = scope("image_manager_init");
            self.ensure_image_manager()?;
        }
        let manager = &self.data_services.image_manager;
        let cache = request_cache || manager.force_cache != 0 || manager.fast_forward;
        let cached = {
            let _detail = scope("image_frame_cache_lookup");
            let _stage = ImageSourceStep::new(ImageSourceStage::CacheLookup);
            if share_pixels { self.data_services.image_manager.shared_frame(name, id) }
            else { self.data_services.image_manager.get(name, id) }
        };
        if let Some(frame) = cached {
            ImageSourceStep::cache_hit();
            if cache {
                let _detail = scope("image_frame_cache_touch");
                let _stage = ImageSourceStep::new(ImageSourceStage::CacheTouch);
                self.data_services.image_manager.remember(name, id, &frame)?;
            }
            return Ok(Some(frame));
        }
        let asset = {
            let _detail = scope("image_encoded_ingress");
            let _stage = ImageSourceStep::new(ImageSourceStage::Ingress);
            self.read_shared_image_asset(name, false)?
        };
        let Some((path, bytes)) = asset else { return Ok(None); };
        let frame = {
            let _detail = scope("image_frame_decode");
            let _stage = ImageSourceStep::new(ImageSourceStage::Decode);
            self.decode_named_image_frame(&bytes, id, &path)?
        };
        let Some(frame) = frame else { return Ok(None); };
        if cache {
            let _detail = scope("image_frame_cache_insert");
            self.data_services.image_manager.remember(name, id, &frame)?;
            if share_pixels && let Some(shared) = self.data_services.image_manager.shared_frame(name, id) {
                return Ok(Some(shared));
            }
        }
        Ok(Some(frame))
    }

    pub(super) fn decode_named_image_frame(&mut self, bytes: &[u8], id: u32, path: &str)
        -> Result<Option<RgbaFrame>> {
        let frame = self.decode_managed_image_frame(bytes, id).map_err(|error| {
            native_state_error_owned(format!("image manager resource={path:?} frame_id={id}: {error}"))
        })?;
        if self.data_services.story_trace && id == 0
            && path.rsplit(['/', '\\']).next().is_some_and(|name| name.eq_ignore_ascii_case("TyukDra01.hg3"))
            && let Some(frame) = &frame
        {
            self.events.push(super::LogicEvent { frame: self.frame, kind: "hatsumira-large-hg".into(),
                detail: format!("resource={path:?} frame_id={id} sha256={} size={}x{} rgba_bytes={}",
                    super::particle_plugin_digest::sha256(bytes), frame.width(), frame.height(), frame.pixels().len()),
            });
        }
        Ok(frame.map(|frame| name_image_frame(frame, path, id)))
    }

    /// The original translation hook runs inside native HG-3 decoding, before
    /// image cache insertion, wrapper registration, or retained-list binding.
    /// Keep one host-aware dispatcher for all three producers.
    pub(super) fn decode_managed_image_frame(&mut self, bytes: &[u8], id: u32)
        -> Result<Option<RgbaFrame>> {
        let prepared = self.data_services.image_manager.prepared_material.as_ref().and_then(|ready| ready.frame(bytes, id));
        let frame = if let Some(prepared) = prepared {
            let Some(frame) = prepared? else { return Ok(None); }; frame
        } else { match self.data_services.ordinary.take_predecoded_image(bytes, id) {
            Some(frame) => frame,
            None => {
                let Some(frame) = decode_image_frame(bytes, id)? else { return Ok(None); };
                frame
            }
        }};
        if !bytes.starts_with(b"HG-3") { return Ok(Some(frame)); }
        self.apply_managed_hg3_patch(bytes, id, None, frame).map(Some)
    }

    fn apply_managed_hg3_patch(&mut self, bytes: &[u8], id: u32, source: Option<&HgFrame>,
        frame: RgbaFrame) -> Result<RgbaFrame> {
        let _timing = super::measure_host_phase(super::HostPhase::Patch);
        let enabled = match self.data_services.image_manager.island_patch_enabled {
            Some(enabled) => enabled,
            None => {
                // Keep the precise read/directory failure. Only successful
                // identity decisions are derived cached state.
                let enabled = self.resources.game_language() != cs2_runtime::resource_router::GameLanguage::Japanese
                    && island_patch::identify(self.resources);
                self.data_services.image_manager.island_patch_enabled = Some(enabled);
                enabled
            }
        };
        if !enabled { return Ok(frame); }
        let mut warnings = Vec::new();
        let replacement = island_patch::apply(self.resources, bytes, id, source, frame,
            &mut |warning| warnings.push(warning));
        for warning in warnings {
            self.events.push(super::LogicEvent { frame: self.frame,
                kind: "island-image-patch".into(), detail: warning });
        }
        replacement
    }

    pub(super) fn image_is_animation_resource(&mut self, name: &str) -> Result<bool> {
        Ok(self.resolve_image_animation_resource(name)?.is_some())
    }

    pub(super) fn list_animation_filename624(&mut self, name: &str) -> Result<String> {
        match self.data_services.css_task_pe_sha256 {
            // These actual old624 wrappers complete the image category and
            // replace the extension, then send the filename without probing
            // existence or consulting the later anmtable alias service.
            // UTATE453600, TSUNABAN44b490, CAMPANELLA44fe10.
            Some("a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52"
                | "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8"
                | "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad") => {
                let names = self.resources.category_names(b"image", name)
                    .map_err(|_| native_state_error("legacy native 624 image category resolution failed"))?;
                let name = names.first()
                    .ok_or_else(|| native_state_error("legacy native 624 category resolved no name"))?;
                let name = with_extension(name, Some("anm"));
                let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(&name);
                if invalid || encoded.len() >= 260 {
                    return Err(native_state_error("legacy native 624 ANM name exceeds its CP932 buffer"));
                }
                Ok(name)
            }
            _ => Ok(self.resolve_image_animation_resource(name)?.unwrap_or_default()),
        }
    }

    /// The same image-manager lookup also supplies native624's ANM filename.
    /// Preserve the resolved alias path rather than returning a mere hit bit.
    pub(super) fn resolve_image_animation_resource(&mut self, name: &str) -> Result<Option<String>> {
        use cs2_runtime::resource_router::ResourceMode;
        let exists = |name: &str| -> Result<bool> {
            let anm = with_extension(name, Some("anm"));
            if let Some(exists) = self.data_services.image_manager.prepared_material.as_ref().and_then(|ready| ready.exists(&anm)) { return Ok(exists); }
            Ok(self.resources.resolve_mode_utf16(&with_extension(name, Some("anm")).encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst)
                .map_err(|_| native_state_error("image animation resource routing failed"))?.is_some())
        };
        if exists(name)? { return Ok(Some(with_extension(name, Some("anm")))); }
        self.initialize_animation_aliases()?;
        let start = name.rfind(['/', '\\']).filter(|&at| at > 0).map_or(0, |at| at + 1);
        let key = with_extension(&name[start..], None);
        let (key, _, invalid) = encoding_rs::SHIFT_JIS.encode(&key);
        if invalid || key.len() >= 256 { return Err(native_state_error("image animation alias key exceeds CP932 capacity")); }
        let Some(alias) = self.data_services.image_manager.animation_aliases.resolve(&key) else { return Ok(None); };
        let alias = super::ScriptTextEncoding::Cp932.decode(alias);
        let resolved = self.resources.category_names(b"image", &alias)
            .map_err(|_| native_state_error("image animation alias category resolution failed"))?;
        for name in resolved {
            if self.resources.resolve_mode_utf16(&with_extension(&name, Some("anm")).encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst)
                .map_err(|_| native_state_error("image animation alias routing failed"))?.is_some() {
                    return Ok(Some(with_extension(&name, Some("anm"))));
                }
        }
        Ok(None)
    }

    pub(super) fn load_managed_image_plan(&mut self, name: &str, request_cache: bool) -> Result<Option<RgbaFrame>> {
        self.load_managed_image_plan_selected(name, 0, request_cache, false)
    }

    fn load_managed_image_plan_selected(&mut self, name: &str, id: u32,
        request_cache: bool, animation_initial: bool) -> Result<Option<RgbaFrame>> {
        // 0x5c9850 removes the extension before splitting comma selectors.
        // Physical part i is stem + "_" + i zeroes + first selector byte.
        let (components, extension) = image_plan_components(name);
        let Some(base) = components.first() else { return Ok(None); };
        if components.len() == 1 {
            let id = if animation_initial { self.initial_resource_frame_id(name, id)? } else { id };
            return self.load_managed_image(name, id, request_cache);
        }
        if components.len() > 65 {
            return Err(native_state_error("native 681 image plan exceeds 64 layer selectors"));
        }
        let dot = name.rfind('.').filter(|&dot|
            name[..dot].rfind(['/', '\\']).is_none_or(|slash| slash < dot)).unwrap_or(name.len());
        let (selectors, _, invalid) = encoding_rs::SHIFT_JIS.encode(&name[..dot]);
        if invalid || selectors.len() >= 256 {
            return Err(native_state_error("image layer selector table exceeds its CP932 buffer"));
        }
        let mut names = Vec::new();
        for (index, selector) in components[1..].iter().enumerate() {
            let first = selector.as_bytes()[0];
            if first == b'0' { continue; }
            if !first.is_ascii() {
                return Err(native_state_error("native 681 non-ASCII first-byte layer selector is unsupported"));
            }
            let part = format!("{base}_{}{first_char}.{extension}",
                "0".repeat(index), first_char = char::from(first));
            let id = if animation_initial { self.initial_resource_frame_id(&part, 0)? } else { 0 };
            names.push((part, id));
        }
        self.ensure_image_manager()?;
        let mut geometry = PlanGeometry::default();
        // 0x5c9980..0x5c9ba8 is a metadata-only pass. In particular a later
        // layer must not evict cached geometry needed by an earlier pass.
        for (part, id) in &names {
            let fields = match self.data_services.image_manager.geometry(part, *id) {
                Some(fields) => Some(fields),
                None => match self.read_shared_image_asset(part, false)? {
                    Some((_, bytes)) => cs2_runtime::native_image_metadata(&bytes, *id)?,
                    None => None,
                },
            };
            if let Some(fields) = fields { geometry.include(fields)?; }
        }
        let Some(mut output) = LayerComposition::new(geometry)? else { return Ok(None); };
        // 0x5c9cf1 borrows an existing cached layer without touching it.
        // Only a miss follows the load/insert path, then draws immediately.
        for (part, id) in &names {
            let source = match self.data_services.image_manager.get(part, *id) {
                Some(frame) => Some(frame),
                None => self.load_managed_image(part, *id, request_cache)?,
            };
            if let Some(source) = source { output.draw(&source)?; }
        }
        output.finish().map(Some)
    }

    fn initial_resource_frame_id(&mut self, name: &str, requested: u32) -> Result<u32> {
        if !self.image_is_animation_resource(name)? { return Ok(requested); }
        let Some(bytes) = self.read_managed_image_bytes(name)? else { return Ok(0); };
        // The retained image receiver enumerates HG-3 IDs as unsigned map
        // keys. Other dispatchers supply only zero. ANM existence selects
        // this initial image; it does not execute or start the time line.
        if !bytes.starts_with(b"HG-3") { return Ok(0); }
        let file = cs2_formats::HgFile::parse(&bytes)
            .map_err(|_| native_state_error("native 710 animated HG frame directory is malformed"))?;
        Ok(file.frames.iter().map(|frame| frame.id).min().unwrap_or(0))
    }

    pub(super) fn read_managed_image_bytes(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.read_managed_image_asset(name, false)?.map(|(_, bytes)| bytes))
    }

    /// A raw-data object retains successfully opened empty files. Its later
    /// consumer, rather than the resource router, diagnoses the empty format.
    pub(crate) fn read_managed_raw_image_bytes(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.read_managed_image_asset(name, true)?.map(|(_, bytes)| bytes))
    }

    pub(super) fn read_managed_image_asset(&self, name: &str, raw: bool) -> Result<Option<(String, Vec<u8>)>> {
        Ok(self.read_shared_image_asset(name, raw)?
            .map(|(path, bytes)| (path, Arc::unwrap_or_clone(bytes))))
    }

    fn read_shared_image_asset(&self, name: &str, raw: bool) -> Result<Option<(String, Arc<Vec<u8>>)>> {
        let _timing = super::measure_host_phase(super::HostPhase::Read);
        if let Some(asset) = self.data_services.image_manager.prepared_material.as_ref().and_then(|ready| ready.asset(name, raw)) { return Ok(asset); }
        if name.encode_utf16().count() >= 260 {
            return Err(native_state_error("image manager name exceeds 259 UTF-16 units"));
        }
        if let Some(bytes) = self.data_services.ordinary.cached_resource(name) {
            return Ok((raw || !bytes.is_empty()).then(|| (name.to_owned(), Arc::new(bytes.to_vec()))));
        }
        let Some(candidate) = self.resources.resolve_mode_utf16(
            &name.encode_utf16().collect::<Vec<_>>(), cs2_runtime::resource_router::ResourceMode::LooseFirst,
        )
            .map_err(|_| native_state_error("image resource routing failed"))? else { return Ok(None); };
        let candidate = candidate.value;
        if candidate.size > MAX_FILE as u64 {
            return Err(native_state_error("image manager resource exceeds 64 MiB"));
        }
        match self.read_encoded_image_candidate(candidate) {
            Ok(bytes) if !raw && bytes.is_empty() => Ok(None),
            Ok(bytes) if bytes.len() <= MAX_FILE => Ok(Some((candidate.name.clone(), bytes))),
            Ok(_) => Err(native_state_error("image manager resource exceeds 64 MiB")),
            Err(RuntimeError::ResourceNotFound(_)) => Ok(None),
            Err(RuntimeError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound
                || (!raw && source.kind() == std::io::ErrorKind::PermissionDenied) => Ok(None),
            Err(_) => Err(native_state_error("image manager resource read failed")),
        }
    }
}

fn name_image_frame(frame: RgbaFrame, path: &str, id: u32) -> RgbaFrame {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    frame.with_asset_source(path.trim_start_matches("./").trim_start_matches('/').into(), id)
}

fn image_plan_components(name: &str) -> (Vec<&str>, &str) {
    let dot = name.rfind('.').filter(|&dot|
        name[..dot].rfind(['/', '\\']).is_none_or(|slash| slash < dot));
    let (stem, extension) = dot.map_or((name, ""), |dot| (&name[..dot], &name[dot + 1..]));
    (stem.split(',').filter(|part| !part.is_empty()).collect(), extension)
}

fn with_extension(name: &str, extension: Option<&str>) -> String {
    let stem = name.rfind('.').filter(|&dot|
        name[..dot].rfind(['/', '\\']).is_none_or(|slash| slash < dot))
        .map_or(name, |dot| &name[..dot]);
    match extension {
        Some(extension) => format!("{stem}.{extension}"),
        None => stem.to_owned(),
    }
}

fn cache_copy(frame: &RgbaFrame) -> RgbaFrame {
    // 0x5e3530 deep-copies through the canvas getter, which resolves zero
    // canvas axes to current pixel dimensions. Cold loads keep their raw
    // metadata; the independent cache and subsequent copies get these values.
    let mut meta = frame.metadata();
    meta.canvas_size = native_canvas_dimensions(frame);
    frame.clone().with_raw_metadata(meta)
}

fn cache_cost(frame: &RgbaFrame) -> Result<usize> {
    let depth = if frame.metadata().bit_depth == 24 { 3 } else { 4 };
    let row = (frame.width() as usize).checked_mul(depth)
        .ok_or_else(|| native_state_error("image cache native allocation size overflow"))?;
    // JPEG BGR rows align to four bytes; HG rows retain packed allocation.
    let row = if frame.metadata().bit_depth == 24
        && frame.metadata().container == cs2_renderer::HgContainer::None {
        row.checked_add(3).map(|row| row & !3)
            .ok_or_else(|| native_state_error("image cache JPEG stride overflow"))?
    } else { row };
    row.checked_mul(frame.height() as usize)
        .ok_or_else(|| native_state_error("image cache native allocation size overflow"))
}

#[cfg(test)]
mod fast_forward_tests {
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> anyhow::Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "cs2-image-manager-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn pending_image_load_retains_the_frame_until_script_resumes() -> anyhow::Result<()> {
        let scratch = Scratch::new()?;
        let mut player = super::super::HeadlessPlayer::open_with_save_root(&scratch.0, scratch.0.join("profile"))?;
        player.enable_host_async_images();
        let (completion, gate) = std::sync::mpsc::channel();
        let manager = &mut player.data_services.image_manager;
        manager.aliases_initialized = true;
        manager.image_loads.gate = Some(Arc::new(std::sync::Mutex::new(gate)));
        let mut code = Vec::new();
        let mut data = vec![0; 64];
        for (address, name) in [(0, b"image/first.jpg\0".as_slice()), (32, b"image/second.jpg\0")] {
            data[address..address + name.len()].copy_from_slice(name);
            for (opcode, operands) in [(0x25, &[address as u32][..]), (0x2b, &[710, 4][..])] {
                code.push(opcode);
                for &value in operands {
                    let operand = cs2_script::Operand::immediate(value);
                    code.push(operand.flags.bits());
                    code.extend_from_slice(&operand.payload.to_le_bytes());
                }
            }
        }
        code.push(0x3b);
        let vm = Vm::from_program(cs2_script::KcsProgram::from_code(&code, data, 64)?)?;
        let owner = player.current_task_id;
        player.data_services.tasks.set_factory_external(vm.external_binding());
        player.data_services.tasks.register_external_task(owner, 0, &player.data_services.language.languages)?;
        player.vm = Some(vm);

        let frame = player.state.clock.frame + 1;
        let completed_materials = |player: &super::super::HeadlessPlayer| {
            player.events().iter().filter(|event| event.kind == "image-material" && event.frame == frame).count()
        };
        for loaded in 0..2 {
            for _ in 0..3 {
                assert!(!player.advance_host_frame_with_retries(1000)?);
                assert_eq!(player.state.clock.frame, frame);
                assert_eq!(completed_materials(&player), loaded);
            }
            completion.send(())?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !player.data_services.image_manager.image_loads.result_ready(owner) {
                assert!(std::time::Instant::now() < deadline, "image worker did not complete");
                std::thread::yield_now();
            }
        }
        assert!(player.advance_host_frame_with_retries(1000)?);
        assert_eq!(player.state.clock.frame, frame);
        assert_eq!(completed_materials(&player), 2);
        Ok(())
    }

    #[test]
    fn fast_forward_twenty_image_cache_cycles() -> anyhow::Result<()> {
        let scratch = Scratch::new()?;
        let backgrounds = scratch.0.join("bg");
        std::fs::create_dir(&backgrounds)?;
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&vec![0; 512 * 512 * 3], 512, 512, image::ExtendedColorType::Rgb8)?;
        for round in 1..=20 {
            for index in 0..8 {
                std::fs::write(backgrounds.join(format!("{round}-{index}.jpg")), &jpeg)?;
            }
        }
        let mut player = super::super::HeadlessPlayer::open_with_save_root(&scratch.0, &scratch.0)?;
        let mut router = cs2_runtime::resource_router::ResourceRouter::default();
        router.slots[0] = cs2_runtime::resource_router::ResourceSlot {
            prefix: "bg".encode_utf16().collect(),
            loose_directory: "bg".encode_utf16().collect(),
            loose_enabled: true,
            archive: None,
        };
        player.scan.resources.set_resource_router(router)?;
        assert_eq!(player.enable_host_fast_forward(6 * 1024_u64.pow(3)), 768 * 1024 * 1024);
        let manager = &mut player.data_services.image_manager;
        manager.initialized = true;
        manager.enabled = true;
        manager.force_cache = 0;
        manager.capacity = 4 * 1024 * 1024;
        let ctrl = cs2_runtime::InputKey::Keyboard(17);
        let mut peak = 0;
        let mut evictions = 0;
        for round in 1..=20 {
            player.apply_host_input(cs2_runtime::InputEvent::KeyDown(ctrl))?;
            assert!(player.advance_host_frame(1000)?);
            assert!(player.data_services.fast_forward);
            for index in 0..8 {
                let before = player.data_services.image_manager.frames.len();
                if index == 4 {
                    let name = format!("bg/{round}-0.jpg");
                    player.with_host(|host| Ok(host.load_managed_image_projection(&name, 0)?))?;
                }
                let name = format!("bg/{round}-{index}.jpg");
                let frame = player.with_host(|host| Ok(host.load_managed_image_projection(&name, 0)?))?
                    .expect("mock background should decode");
                assert_eq!(frame.metadata().bit_depth, 24);
                let manager = &player.data_services.image_manager;
                if index == 4 {
                    assert!(manager.shared_frame(&format!("bg/{round}-0.jpg"), 0).is_some());
                    assert!(manager.shared_frame(&format!("bg/{round}-1.jpg"), 0).is_none());
                }
                peak = peak.max(manager.rgba_bytes);
                evictions += before + 1 - manager.frames.len();
                player.advance_host_frame(1000)?;
            }
            player.apply_host_input(cs2_runtime::InputEvent::KeyUp(ctrl))?;
            assert!(player.advance_host_frame(1000)?);
            assert!(!player.data_services.fast_forward);
        }
        println!("fast_forward cycles=20 decoded_backgrounds=160 lru_hits_checked=20 peak_decoded_bytes={peak} evictions={evictions}");
        assert_eq!(peak, 4 * 1024 * 1024);
        assert_eq!(evictions, 156);
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct ResourceLayer {
    frames: BTreeMap<u32, RgbaFrame>,
    animation: cs2_script::ImageAnimation,
    selected: u32,
}

impl ResourceLayer {
    fn new(frames: BTreeMap<u32, RgbaFrame>, animation: cs2_script::ImageAnimation) -> Result<Self> {
        let selected = *frames.first_key_value()
            .ok_or_else(|| native_state_error("retained image layer has no frames"))?.0;
        Ok(Self { frames, animation, selected })
    }
}

#[derive(Debug, Clone)]
struct ResourceAnimationRound {
    now: u32,
    cursor: usize,
    restart: bool,
    started: bool,
    changed: bool,
}

#[derive(Debug, Clone)]
struct ResourceLayers {
    layers: Vec<ResourceLayer>,
    geometry: PlanGeometry,
    pending: Option<ResourceAnimationRound>,
    changed: bool,
    surface: Option<cs2_runtime::object_graph::ImageResourceSurfaceSnapshot>,
}

impl ResourceLayers {
    fn new(layers: Vec<ResourceLayer>) -> Result<Self> {
        let geometry = Self::directory_geometry(&layers)?;
        let surface = resource_surface::create(&layers, &geometry, None)?;
        Ok(Self { layers, geometry, pending: None, changed: false, surface })
    }

    fn directory_geometry(layers: &[ResourceLayer]) -> Result<PlanGeometry> {
        let mut geometry = PlanGeometry::default();
        let mut bytes = 0usize;
        for layer in layers {
            for frame in layer.frames.values() {
                bytes = bytes.checked_add(frame.pixels().len()).filter(|&n| n <= MAX_CACHE)
                    .ok_or_else(|| native_state_error("retained image layers exceed 256 MiB"))?;
                let meta = frame.metadata();
                let canvas = native_canvas_dimensions(frame);
                geometry.include([meta.offset[0] as u32, meta.offset[1] as u32,
                    frame.width(), frame.height(), canvas[0], canvas[1],
                    u32::from(meta.bit_depth) / 8, meta.base[0] as u32, meta.base[1] as u32])?;
            }
        }
        Ok(geometry)
    }

    fn compose(&self) -> Result<Option<RgbaFrame>> {
        if let [layer] = self.layers.as_slice()
            && layer.animation.timeline().is_none() {
            return Ok(Some(layer.frames[&layer.selected].clone()));
        }
        let Some(mut target) = LayerComposition::new(self.geometry.clone())? else { return Ok(None); };
        for layer in &self.layers {
            let frame = &layer.frames[&layer.selected];
            let display = layer.animation.display();
            let animated = layer.animation.timeline().is_some();
            if !animated || display.visible != 0 {
                target.draw_animation(frame, if animated { [display.x, display.y] } else { [0; 2] },
                    if animated { display.compose((0., 0.), 255, 1).blend as u32 } else { 255 })?;
            }
        }
        target.finish().map(Some)
    }
}

#[derive(Default, Debug, Clone)]
struct PlanGeometry {
    bounds: Option<[i32; 4]>,
    canvas: [u32; 2],
    base: Option<[i32; 2]>,
}

impl PlanGeometry {
    fn include(&mut self, fields: [u32; 9]) -> Result<()> {
        let x = fields[0] as i32;
        let y = fields[1] as i32;
        let width = i32::try_from(fields[2])
            .map_err(|_| native_state_error("image plan metadata width exceeds signed coordinates"))?;
        let height = i32::try_from(fields[3])
            .map_err(|_| native_state_error("image plan metadata height exceeds signed coordinates"))?;
        let right = x.checked_add(width)
            .ok_or_else(|| native_state_error("image plan crop X overflows signed coordinates"))?;
        let top = y.checked_add(height)
            .ok_or_else(|| native_state_error("image plan crop Y overflows signed coordinates"))?;
        if width > 0 && height > 0 {
            self.bounds = Some(match self.bounds {
                Some(bounds) => [bounds[0].min(x), bounds[1].min(y),
                    bounds[2].max(right), bounds[3].max(top)],
                None => [x, y, right, top],
            });
        }
        for axis in 0..2 {
            // Raw metadata retains zero canvas axes. Cache hits above use
            // the native pixel getter and therefore already resolve zero.
            if fields[4 + axis] as i32 > self.canvas[axis] as i32 {
                self.canvas[axis] = fields[4 + axis];
            }
        }
        self.base.get_or_insert([fields[7] as i32, fields[8] as i32]);
        Ok(())
    }
}

struct LayerComposition {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    metadata: FrameMetadata,
    attributes: BTreeMap<u16, [u32; 5]>,
}

impl LayerComposition {
    fn new(geometry: PlanGeometry) -> Result<Option<Self>> {
        // A retained directory of zero-sized frames still owns sources and
        // ANM state. Only an absent directory has neither bounds nor base.
        if geometry.bounds.is_none() && geometry.base.is_none() { return Ok(None); }
        let bounds = geometry.bounds.unwrap_or([0; 4]);
        let width = u32::try_from(i64::from(bounds[2]) - i64::from(bounds[0]))
            .map_err(|_| native_state_error("native 681 composed width overflow"))?;
        let height = u32::try_from(i64::from(bounds[3]) - i64::from(bounds[1]))
            .map_err(|_| native_state_error("native 681 composed height overflow"))?;
        let length = (width as usize).checked_mul(height as usize).and_then(|size| size.checked_mul(4))
            .filter(|&size| size <= MAX_PIXELS)
            .ok_or_else(|| native_state_error("native 681 composed pixels exceed 128 MiB"))?;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(length)
            .map_err(|_| native_state_error("native 681 composed pixel allocation failed"))?;
        pixels.resize(length, 0);
        Ok(Some(Self {
            width, height, pixels, attributes: BTreeMap::new(),
            metadata: FrameMetadata {
                offset: [bounds[0], bounds[1]], canvas_size: geometry.canvas,
                base: geometry.base.unwrap_or([0; 2]), bit_depth: 32,
                ..FrameMetadata::default()
            },
        }))
    }

    fn draw(&mut self, source: &RgbaFrame) -> Result<()> {
        let width = self.width;
        let height = self.height;
        let offset = self.metadata.offset;
        let base = self.metadata.base;
        let bounds = [
            i64::from(offset[0]), i64::from(offset[1]),
            i64::from(offset[0]) + i64::from(width),
            i64::from(offset[1]) + i64::from(height),
        ];
        let meta = source.metadata();
        // Same global rectangle gate and independent local clip as the PE.
        // It deliberately uses destination width/height in the first gate.
        let left = i64::from(meta.offset[0]).max(i64::from(offset[0]));
        let top = i64::from(meta.offset[1]).max(i64::from(offset[1]));
        let right = (i64::from(meta.offset[0]) + i64::from(width)).min(bounds[2]);
        let bottom = (i64::from(meta.offset[1]) + i64::from(height)).min(bounds[3]);
        if left >= right || top >= bottom { return Ok(()); }
        let at = [
            left - i64::from(offset[0]) + i64::from(base[0]) - i64::from(meta.base[0]),
            top - i64::from(offset[1]) + i64::from(base[1]) - i64::from(meta.base[1]),
        ];
        let target_x = at[0].max(0).min(i64::from(width));
        let target_y = at[1].max(0).min(i64::from(height));
        let source_width = source.width().min(width);
        let source_height = source.height().min(height);
        let end_x = (at[0] + i64::from(source_width)).max(0).min(i64::from(width));
        let end_y = (at[1] + i64::from(source_height)).max(0).min(i64::from(height));
        let draw_width = (end_x - target_x).min(i64::from(source_width));
        let draw_height = (end_y - target_y).min(i64::from(source_height));
        if draw_width <= 0 || draw_height <= 0 { return Ok(()); }
        let pixels = &mut self.pixels;
        for row in 0..draw_height as usize {
            let source_start = row * source.width() as usize * 4;
            let target_start = ((target_y as usize + row) * width as usize
                + target_x as usize) * 4;
            if target_x > 0 {
                super::image_blit::seed_rgb(&mut pixels[target_start - 4..target_start],
                    source.pixels()[source_start..source_start + 4].try_into().expect("source pixel"));
            }
            for column in 0..draw_width as usize {
                let input = source_start + column * 4;
                let output = target_start + column * 4;
                super::image_blit::blend_pixel_integer(&mut pixels[output..output + 4],
                    source.pixels()[input..input + 4].try_into().expect("source pixel"));
            }
            if target_x + draw_width < i64::from(width) {
                let input = source_start + (draw_width as usize - 1) * 4;
                let output = target_start + draw_width as usize * 4;
                super::image_blit::seed_rgb(&mut pixels[output..output + 4],
                    source.pixels()[input..input + 4].try_into().expect("source pixel"));
            }
        }
        self.attributes.extend(source.hg_attributes().iter().map(|(&id, &values)| (id, values)));
        Ok(())
    }

    /// Internal resource list layers use the normal list Alpha rule and top-origin
    /// coordinates. The output rectangle stays the retained directory union;
    /// moving frames outside it are clipped, rather than reallocating it.
    fn draw_animation(&mut self, source: &RgbaFrame, position: [i32; 2], opacity: u32) -> Result<()> {
        let meta = source.metadata();
        let at = [meta.offset[0].wrapping_add(position[0]).wrapping_sub(self.metadata.offset[0]),
            meta.offset[1].wrapping_add(position[1]).wrapping_sub(self.metadata.offset[1])];
        self.draw_animation_at(source, at, opacity, None)
    }

    fn draw_animation_at(&mut self, source: &RgbaFrame, at: [i32; 2], opacity: u32,
        clip: Option<[i32; 4]>) -> Result<()> {
        self.attributes.extend(source.hg_attributes().iter().map(|(&id, &values)| (id, values)));
        if opacity == 0 { return Ok(()); }
        let meta = source.metadata();
        let opaque = match meta.container {
            cs2_renderer::HgContainer::Hg3V300 => meta.alpha_flag == 0,
            _ => !source.has_alpha(),
        };
        for sy in 0..source.height() {
            let y = i64::from(at[1]) + i64::from(sy);
            if y < 0 || y >= i64::from(self.height) { continue; }
            if clip.is_some_and(|r| y < i64::from(r[1]) || y >= i64::from(r[3])) { continue; }
            for sx in 0..source.width() {
                let x = i64::from(at[0]) + i64::from(sx);
                if x < 0 || x >= i64::from(self.width) { continue; }
                if clip.is_some_and(|r| x < i64::from(r[0]) || x >= i64::from(r[2])) { continue; }
                let input = (sy as usize * source.width() as usize + sx as usize) * 4;
                let output = (y as usize * self.width as usize + x as usize) * 4;
                let pixel: [u8; 4] = source.pixels()[input..input + 4].try_into().unwrap();
                if opaque && opacity == 255 {
                    self.pixels[output..output + 4].copy_from_slice(&pixel);
                } else {
                    super::list_native::blend_list_pixel(&mut self.pixels[output..output + 4], pixel, opacity);
                    for neighbor in [x - 1, x + 1] {
                        if clip.is_some_and(|r| neighbor < i64::from(r[0]) || neighbor >= i64::from(r[2])) {
                            continue;
                        }
                        if neighbor >= 0 && neighbor < i64::from(self.width) {
                            let at = (y as usize * self.width as usize + neighbor as usize) * 4;
                            super::image_blit::seed_rgb(&mut self.pixels[at..at + 4], pixel);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<RgbaFrame> {
        RgbaFrame::from_native_rgba8(self.width, self.height, self.pixels, self.metadata)
            .map(|frame| frame.with_raw_metadata(self.metadata)
                .with_hg_attributes(HgFrameAttributes::new(self.attributes)))
            .map_err(|_| native_state_error("native 681 composed frame construction failed"))
    }
}

pub(super) fn decode_image_frame(bytes: &[u8], id: u32) -> Result<Option<RgbaFrame>> {
    let _timing = super::measure_host_phase(super::HostPhase::Decode);
    if bytes.get(..2) == Some(b"BM") {
        let bitmap = cs2_formats::BitmapImage::decode(bytes)
            .map_err(|_| native_state_error("image manager malformed/unsupported BMP layout"))?;
        return RgbaFrame::from_rgba8(bitmap.width, bitmap.height, bitmap.pixels)
            .map(|frame| Some(frame.with_raw_metadata(FrameMetadata::raster([bitmap.width, bitmap.height]))))
            .map_err(|_| native_state_error("image manager BMP frame construction failed"));
    }
    if bytes.get(..2) == Some(b"\xff\xd8") {
        // 0x5fbae0 does not read its frame argument. Keep native 24-bit
        // metadata until callers such as ordinary 111/364 convert to 32 bits.
        let jpeg = cs2_formats::JpegImage::decode(bytes)
            .map_err(|_| native_state_error("image dispatcher malformed/unsupported JPEG pixels"))?;
        let mut metadata = FrameMetadata::raster([jpeg.metadata.width, jpeg.metadata.height]);
        metadata.bit_depth = 24;
        return RgbaFrame::from_rgba8(jpeg.metadata.width, jpeg.metadata.height, jpeg.pixels)
            .map(|frame| Some(frame.with_raw_metadata(metadata)))
            .map_err(|_| native_state_error("image dispatcher JPEG frame construction failed"));
    }
    let file = cs2_formats::HgFile::parse(bytes)
        .map_err(|error| native_state_error_owned(format!("image manager HG header parse failed: {error}")))?;
    let selection = if file.signature == *b"HG-2" && file.version as u16 == 0x10 {
        AnimationFrameSelection::Index(0)
    } else {
        if !file.frames.iter().any(|frame| frame.id == id) { return Ok(None); }
        AnimationFrameSelection::Id(id)
    };
    checked_hg_frame(RgbaFrame::decode_parsed_hg(bytes, &file, selection)).map(Some)
}

fn checked_hg_frame(frame: cs2_renderer::Result<RgbaFrame>) -> Result<RgbaFrame> {
    let frame = frame.map_err(|error| native_state_error_owned(format!("image manager HG frame decode failed: {error}")))?;
    if frame.pixels().len() > MAX_PIXELS {
        return Err(native_state_error("image manager decoded pixels exceed 128 MiB"));
    }
    Ok(frame)
}
