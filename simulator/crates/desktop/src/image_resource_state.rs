//! Canonical resource-owned sources for the existing portable object graph.
//! Preparing is fallible; installing never decodes pixels, reads resources or
//! advances ANM/random. Current composites are restored separately verbatim.
use super::*;
use cs2_runtime::object_graph::{ImageFrameSnapshot, ImageResourceLayersSnapshot,
    ImageResourceRoundSnapshot, ImageResourceSnapshot, ImageSourceLayerSnapshot};
use cs2_runtime::graph_serde::{Deserialize, Serialize};

pub(crate) struct PreparedResourceLayers(BTreeMap<u32, ResourceLayers>);

/// Part of the existing image-list component, not a second resource graph.
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ImageCacheSnapshot {
    initialized: bool, enabled: bool, capacity: usize, force_cache: u32,
    bytes: usize, rgba_bytes: usize,
    frames: BTreeMap<String, (usize, ImageFrameSnapshot)>, recency: VecDeque<String>,
    aliases_initialized: bool, aliases: BTreeMap<Vec<u8>, Vec<u8>>,
}

pub(crate) struct PreparedImageCache {
    initialized: bool, enabled: bool, capacity: usize, force_cache: u32,
    bytes: usize, rgba_bytes: usize, frames: BTreeMap<String, (usize, RgbaFrame)>,
    recency: VecDeque<String>, aliases_initialized: bool,
    aliases: cs2_formats::animation_alias::AnimationAliases,
}

impl ImageManager {
    pub(crate) fn cache_snapshot(&self) -> ImageCacheSnapshot {
        ImageCacheSnapshot {
            initialized: self.initialized, enabled: self.enabled, capacity: self.capacity,
            force_cache: self.force_cache, bytes: self.bytes, rgba_bytes: self.rgba_bytes,
            frames: self.frames.iter().map(|(key, (cost, frame))|
                (key.clone(), (*cost, capture_frame(frame)))).collect(), recency: self.recency.clone(),
            aliases_initialized: self.aliases_initialized, aliases: self.animation_aliases.entries().clone(),
        }
    }

    pub(crate) fn prepare_cache_restore(s: ImageCacheSnapshot) -> anyhow::Result<PreparedImageCache> {
        if s.capacity > MAX_CACHE || s.bytes > MAX_CACHE || s.rgba_bytes > MAX_CACHE
            || s.frames.len() > MAX_ENTRIES || s.recency.len() != s.frames.len()
            // A nonzero MB setting can wrap its DWORD <<20 capacity to zero.
            // enabled records the setting, not whether that wrapped capacity
            // is nonzero. Keep both values rather than re-deriving the flag.
            || (!s.enabled && (s.capacity != 0 || !s.frames.is_empty()))
            || (!s.initialized && s.enabled)
            || (!s.initialized && (s.capacity != 0 || !s.frames.is_empty()))
            || (s.initialized && !s.aliases_initialized)
            || (!s.aliases_initialized && !s.aliases.is_empty()) {
            anyhow::bail!("saved image cache control/capacity is inconsistent");
        }
        let recency = s.recency.iter().collect::<std::collections::BTreeSet<_>>();
        if recency.len() != s.recency.len() || recency.iter().any(|key| !s.frames.contains_key(*key)) {
            anyhow::bail!("saved image cache recency is not a permutation of its records");
        }
        let mut bytes = 0usize;
        let mut rgba = 0usize;
        let mut frames = BTreeMap::new();
        for (key, (cost, frame)) in s.frames {
            if key.len() > 4096 || key.contains('\0')
                || key.rsplit_once(':').is_none_or(|(_, id)| id.parse::<i32>().is_err()) {
                anyhow::bail!("saved image cache key is invalid");
            }
            let image = restore_frame(&frame)?;
            if cost != cache_cost(&image)? { anyhow::bail!("saved image cache native cost differs from its pixels"); }
            bytes = bytes.checked_add(cost).filter(|&n| n <= MAX_CACHE)
                .ok_or_else(|| anyhow::anyhow!("saved image cache native storage exceeds limit"))?;
            rgba = rgba.checked_add(image.pixels().len()).filter(|&n| n <= MAX_CACHE)
                .ok_or_else(|| anyhow::anyhow!("saved image cache RGBA storage exceeds limit"))?;
            frames.insert(key, (cost, image));
        }
        if bytes != s.bytes || rgba != s.rgba_bytes {
            anyhow::bail!("saved image cache accounting differs from its records");
        }
        // A sole image can exceed configured capacity; native eviction allows
        // it after removing every prior record. Do not reject that valid state.
        let aliases = cs2_formats::animation_alias::AnimationAliases::from_entries(s.aliases)?;
        Ok(PreparedImageCache { initialized: s.initialized, enabled: s.enabled,
            capacity: s.capacity, force_cache: s.force_cache, bytes, rgba_bytes: rgba,
            frames, recency: s.recency, aliases_initialized: s.aliases_initialized, aliases })
    }

    pub(crate) fn install_cache_restore(&mut self, p: PreparedImageCache) {
        self.initialized = p.initialized; self.enabled = p.enabled; self.capacity = p.capacity;
        self.force_cache = p.force_cache; self.bytes = p.bytes; self.rgba_bytes = p.rgba_bytes;
        self.frames = p.frames; self.recency = p.recency;
        self.aliases_initialized = p.aliases_initialized; self.animation_aliases = p.aliases;
        // resource_layers was prepared from canonical resources separately.
    }

    pub(crate) fn validate_resource_layer_owners(
        &self, resources: &BTreeMap<u32, crate::DesktopImageResource>,
    ) -> anyhow::Result<()> {
        if self.resource_layers.keys().any(|id| !resources.contains_key(id))
            || resources.iter().any(|(id, r)| r.references == 0
                || r.has_source_layer != self.resource_layers.contains_key(id)) {
            anyhow::bail!("image resource source storage differs from its canonical owner table");
        }
        for (&id, sources) in &self.resource_layers {
            if let [layer] = sources.layers.as_slice()
                && layer.animation.timeline().is_none()
                && layer.frames.get(&layer.selected) != Some(&resources[&id].image) {
                anyhow::bail!("single static resource source differs from its aliased current image");
            }
        }
        Ok(())
    }

    pub(crate) fn resource_layers_snapshot(&self, handle: u32) -> Option<ImageResourceLayersSnapshot> {
        self.resource_layers.get(&handle).map(|s| ImageResourceLayersSnapshot {
            layers: s.layers.iter().map(|layer| ImageSourceLayerSnapshot {
                frames: layer.frames.iter().map(|(&id, frame)| (id, capture_frame(frame))).collect(),
                selected: layer.selected, animation: layer.animation.execution_snapshot(),
            }).collect(),
            bounds: s.geometry.bounds, canvas: s.geometry.canvas, base: s.geometry.base,
            pending: s.pending.as_ref().map(|p| ImageResourceRoundSnapshot {
                now: p.now, cursor: p.cursor, restart: p.restart, started: p.started, changed: p.changed,
            }), changed: s.changed, surface: s.surface.clone(),
        })
    }

    pub(crate) fn prepare_resource_layers_restore(
        saved: &BTreeMap<u32, ImageResourceSnapshot>,
    ) -> anyhow::Result<PreparedResourceLayers> {
        let mut resources = BTreeMap::new();
        let mut bytes = 0usize;
        let mut frames = 0usize;
        for (&handle, resource) in saved {
            if resource.has_source_layer != resource.source_layers.is_some() {
                anyhow::bail!("saved resource source marker differs from its retained sources");
            }
            let Some(s) = &resource.source_layers else { continue; };
            if handle == 0 || resource.references == 0 || !resource.has_source_layer
                || s.layers.is_empty() || s.layers.len() > 65 {
                anyhow::bail!("invalid resource layer identity/owner or layer count");
            }
            if let [layer] = s.layers.as_slice()
                && layer.animation.timeline.is_none()
                && layer.frames.get(&layer.selected) != Some(&resource.image) {
                anyhow::bail!("saved single static source differs from its aliased current image");
            }
            let mut layers = Vec::new();
            for layer in &s.layers {
                if layer.frames.is_empty() || !layer.frames.contains_key(&layer.selected) {
                    anyhow::bail!("saved resource source has no selected frame");
                }
                let mut images = BTreeMap::new();
                for (&id, frame) in &layer.frames {
                    frames = frames.checked_add(1).filter(|&n| n <= MAX_ENTRIES)
                        .ok_or_else(|| anyhow::anyhow!("saved source frame count exceeds limit"))?;
                    bytes = bytes.checked_add(frame.rgba.len()).filter(|&n| n <= MAX_CACHE)
                        .ok_or_else(|| anyhow::anyhow!("saved source frames exceed 256 MiB"))?;
                    images.insert(id, restore_frame(frame)?);
                }
                if layer.animation.timeline.as_ref().is_some_and(|t|
                    t.max_image_id as u32 != *layer.frames.last_key_value().unwrap().0) {
                    anyhow::bail!("saved ANM maximum ID differs from its retained frame directory");
                }
                let animation = cs2_script::ImageAnimation::from_execution_snapshot(&layer.animation)?;
                layers.push(ResourceLayer { frames: images, animation, selected: layer.selected });
            }
            let mut restored = ResourceLayers::new(layers)?;
            // 726 changes source frames without rebuilding the original
            // internal list rectangle. Its retained composition bounds may
            // intentionally differ from the post-blur directory union.
            if let Some(b) = s.bounds {
                let width = i64::from(b[2]) - i64::from(b[0]);
                let height = i64::from(b[3]) - i64::from(b[1]);
                if width <= 0 || height <= 0 || width.checked_mul(height).and_then(|n| n.checked_mul(4))
                    .is_none_or(|n| n > MAX_PIXELS as i64) {
                    anyhow::bail!("saved source composition bounds are invalid");
                }
            }
            restored.geometry = PlanGeometry { bounds: s.bounds, canvas: s.canvas, base: s.base };
            let composite = restored.layers.len() != 1 || restored.layers[0].animation.timeline().is_some();
            if composite != s.surface.is_some() {
                anyhow::bail!("saved resource list surface differs from its source directory");
            }
            if let Some(surface) = &s.surface {
                resource_surface::validate(surface, &restored.layers, &restored.geometry)?;
            }
            restored.surface = s.surface.clone();
            if let Some(p) = &s.pending {
                if p.cursor >= restored.layers.len() || (!p.restart && p.started)
                    || restored.layers.iter().enumerate().any(|(i, l)|
                        l.animation.timeline().is_some_and(|t| t.has_pending_update()) && i != p.cursor) {
                    anyhow::bail!("saved source continuation has an invalid layer cursor");
                }
                restored.pending = Some(ResourceAnimationRound {
                    now: p.now, cursor: p.cursor, restart: p.restart, started: p.started, changed: p.changed,
                });
            } else if restored.layers.iter().any(|l|
                l.animation.timeline().is_some_and(|t| t.has_pending_update())) {
                anyhow::bail!("saved source ANM continuation has no resource round");
            }
            restored.changed = s.changed;
            resources.insert(handle, restored);
        }
        Ok(PreparedResourceLayers(resources))
    }

    pub(crate) fn install_resource_layers_restore(&mut self, prepared: PreparedResourceLayers) {
        self.resource_layers = prepared.0;
    }
}

pub(crate) fn capture_frame(frame: &RgbaFrame) -> ImageFrameSnapshot {
    use cs2_renderer::HgContainer;
    let m = frame.metadata();
    ImageFrameSnapshot { size: [frame.width(), frame.height()], rgba: frame.pixels().to_vec(),
        container: match m.container { HgContainer::None => 0, HgContainer::Hg2V10 => 1,
            HgContainer::Hg2V20 => 2, HgContainer::Hg2V25 => 3, HgContainer::Hg3V300 => 4 },
        frame_id: m.frame_id, bit_depth: m.bit_depth, offset: m.offset,
        canvas_size: m.canvas_size, base: m.base, alpha_flag: m.alpha_flag,
        attributes: frame.hg_attributes().iter().map(|(&id, &value)| (id, value)).collect() }
}

pub(crate) fn restore_frame(s: &ImageFrameSnapshot) -> anyhow::Result<RgbaFrame> {
    use cs2_renderer::HgContainer;
    let size = (s.size[0] as usize).checked_mul(s.size[1] as usize).and_then(|n| n.checked_mul(4));
    if size != Some(s.rgba.len()) || s.rgba.len() > MAX_PIXELS || !matches!(s.bit_depth, 24 | 32) {
        anyhow::bail!("saved image has invalid dimensions, storage size or pixel depth");
    }
    let m = FrameMetadata {
        container: match s.container { 0 => HgContainer::None, 1 => HgContainer::Hg2V10,
            2 => HgContainer::Hg2V20, 3 => HgContainer::Hg2V25, 4 => HgContainer::Hg3V300,
            _ => anyhow::bail!("saved image container is invalid") },
        frame_id: s.frame_id, bit_depth: s.bit_depth, offset: s.offset,
        canvas_size: s.canvas_size, base: s.base, alpha_flag: s.alpha_flag,
    };
    Ok(RgbaFrame::from_native_rgba8(s.size[0], s.size[1], s.rgba.clone(), m)?
        .with_hg_attributes(HgFrameAttributes::new(s.attributes.clone())))
}
