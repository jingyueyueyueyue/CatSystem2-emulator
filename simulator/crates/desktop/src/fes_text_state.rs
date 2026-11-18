//! Exact FES text state and device pixels in the existing graph component wire.
use super::*;
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_runtime::message_object::{MessageConfiguration, MessagePixelSnapshot};
use cs2_runtime::object_graph::{self as wire, ImageFrameSnapshot};

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot {
    version: u32,
    store: FesTextStore,
    pixels: BTreeMap<i32, MessagePixelSnapshot>,
}

/// Only the original store and already validated negative-id device surfaces.
/// Fonts/settings stay on the live device; no text is parsed or painted here.
pub(crate) struct PreparedFesTextState {
    store: FesTextStore,
    device: DesktopMessageDevice,
}

impl PreparedFesTextState {
    pub(crate) fn into_parts(self) -> (FesTextStore, DesktopMessageDevice) {
        (self.store, self.device)
    }

    pub(crate) fn store(&self) -> &FesTextStore { &self.store }
}

impl FesTextStore {
    /// Commit only after the enclosing graph's stores/receipts/lists/Backlogs
    /// have all prepared successfully. No device work can fail at this point.
    pub(crate) fn commit_checkpoint(
        &mut self, prepared: PreparedFesTextState, device: &mut DesktopMessageDevice,
    ) {
        let (store, pixels) = prepared.into_parts();
        device.commit_fes_surfaces(pixels);
        *self = store;
    }

    pub(crate) fn checkpoint_bank_keys(&self) -> impl Iterator<Item = &Key> { self.banks.keys() }

    pub(crate) fn capture_checkpoint(
        &self, device: &DesktopMessageDevice, kinds: &BTreeMap<u32, u32>,
        backlogs: &BTreeSet<u32>,
    ) -> Result<Vec<u8>> {
        let messages = self.checkpoint_messages(kinds, backlogs)?;
        let mut pixels = BTreeMap::new();
        for (id, message) in messages {
            // Validate the portable message state without changing its live
            // generation or replaying any layout, draw, or pending request.
            MessageObject::from_snapshot(message.snapshot())?;
            if let Some(surface) = device.snapshot_pixels(id) {
                validate_pixels(message, &surface)?;
                pixels.insert(id, surface);
            } else if message.configured() {
                bail!("FES configured text checkpoint needs its actual device pixels");
            }
        }
        Ok(wire::encode_component(&Snapshot { version: 1, store: self.clone(), pixels })?)
    }

    /// Prepares every MessageObject and real surface before any live writes.
    /// Backlog handles refer to the enclosing transaction's retained objects.
    pub(crate) fn prepare_checkpoint(
        bytes: &[u8], kinds: &BTreeMap<u32, u32>, backlogs: &BTreeSet<u32>,
    ) -> Result<PreparedFesTextState> {
        let mut saved: Snapshot = wire::decode_component(bytes)?;
        if saved.version != 1 { bail!("FES text checkpoint version is unsupported"); }
        let messages = saved.store.checkpoint_messages(kinds, backlogs)?;
        if saved.pixels.len() > messages.len()
            || saved.pixels.keys().any(|id| !messages.contains_key(id))
        { bail!("FES text pixels have no message owner"); }
        for (&id, message) in &messages {
            if let Some(surface) = saved.pixels.get(&id) {
                validate_pixels(message, surface)?;
            } else if message.configured() {
                bail!("FES text checkpoint omitted configured surface pixels");
            }
        }
        drop(messages);
        let mut device = DesktopMessageDevice::default();
        for bank in saved.store.banks.values_mut() {
            restore_bank(bank, &mut saved.pixels, &mut device)?;
        }
        Ok(PreparedFesTextState { store: saved.store, device })
    }

    fn checkpoint_messages<'a>(
        &'a self, kinds: &BTreeMap<u32, u32>, backlogs: &BTreeSet<u32>,
    ) -> Result<BTreeMap<i32, &'a MessageObject>> {
        let owner = |id: &u32| kinds.get(id) == Some(&34);
        if self.next_surface >= 0 || self.banks.len() > MAX_BANKS
            || self.declarations.len() > 1_000_000 || self.initializers.len() > 1_000_000
            || self.waits.len() > MAX_BANKS
            || self.declarations.iter().any(|(id, _)| !owner(id))
            || self.initializers.keys().any(|(id, _)| !owner(id))
            || self.waits.iter().any(|(id, (_, name, _))| !owner(id) || !valid_name(name))
        { bail!("FES text checkpoint allocator/owner/wait state is invalid"); }
        let mut messages = BTreeMap::new();
        for ((id, name, index), bank) in &self.banks {
            if !owner(id) || !valid_name(name) || *index < 0 {
                bail!("FES text checkpoint bank has an invalid owner or identity");
            }
            collect_bank(bank, *id, name, *index, self.next_surface, backlogs, &mut messages)?;
            if let Some(log) = &bank.log {
                log.validate_checkpoint()?;
                for (row, bank) in log.rows.iter().enumerate() {
                    collect_bank(bank, *id, &log::row_name(name, row), *index,
                        self.next_surface, backlogs, &mut messages)?;
                }
            }
        }
        Ok(messages)
    }
}

fn valid_name(name: &[u8]) -> bool { !name.is_empty() && name.len() <= 31 && !name.contains(&0) }

fn collect_bank<'a>(
    bank: &'a TextBank, owner: u32, name: &[u8], index: i32, next: i32,
    backlogs: &BTreeSet<u32>, messages: &mut BTreeMap<i32, &'a MessageObject>,
) -> Result<()> {
    let id = bank.message.id();
    if id >= 0 || id <= next || messages.insert(id, &bank.message).is_some()
        || messages.len() > 1_000_000 || bank.source.len() > MAX_TEXT_BYTES
        || bank.pending.len() > MAX_BANKS || bank.user_mappings.len() > MAX_BANKS
        || bank.user_mapping_cursor > bank.user_mappings.len()
        || bank.user_glyph_source.as_ref().is_some_and(|source| source.len() > MAX_TEXT_BYTES)
        || [bank.remove_characters.as_ref(), bank.add_characters.as_ref()].into_iter()
            .flatten().any(|(index, bytes)| *index > 4 || bytes.len() > MAX_TEXT_BYTES)
        || bank.pending.iter().any(|request| matches!(request,
            TextRequest::Append(bytes) if bytes.len() > MAX_TEXT_BYTES))
        || bank.parent_plane.as_ref().is_some_and(|name| name.len() > 259 || name.contains('\0'))
        || bank.configured && !bank.message.configured()
    { bail!("FES text checkpoint message storage or surface identity is invalid"); }
    MessageConfiguration::from_bytes(&bank.configuration, bank.message.encoding())?;
    if let Some(log) = &bank.log {
        let handle = log.checkpoint_handle();
        if handle != 0 && !backlogs.contains(&handle) {
            bail!("LOGSTRING checkpoint lost its retained Backlog handle");
        }
    }
    for reference in [&bank.metadata.position_source, &bank.metadata.size_source].into_iter().flatten() {
        if reference.name.len() > 127 || reference.name.contains(&0) {
            bail!("FES text checkpoint metadata resource name is invalid");
        }
    }
    if let Some(projection) = &bank.projection {
        if projection.owner != owner || projection.name != name || projection.bank != index
            || projection.frame.pixels().len() > MAX_TEXT_BYTES
        { bail!("FES text checkpoint projection differs from its original bank/row"); }
    }
    Ok(())
}

fn validate_pixels(message: &MessageObject, pixels: &MessagePixelSnapshot) -> Result<()> {
    let configuration = message.configuration().context("unconfigured FES text has device pixels")?;
    let length = (pixels.size[0] as usize).checked_mul(pixels.size[1] as usize)
        .and_then(|n| n.checked_mul(4));
    if pixels.version != 1 || pixels.object_id != message.id()
        || pixels.surface_generation != message.surface_generation()
        || pixels.size != configuration.size()
        || length.is_none_or(|n| n > MAX_TEXT_BYTES || n != pixels.visible_rgba.len()
            || n != pixels.submitted_rgba.len() || pixels.source_rgba.as_ref().is_some_and(|p| p.len() != n))
        || pixels.source_rgba.is_some() != (configuration.reveal_width() != 0)
    { bail!("FES text checkpoint pixels are stale or differ from the real message surface"); }
    Ok(())
}

fn restore_bank(
    bank: &mut TextBank, pixels: &mut BTreeMap<i32, MessagePixelSnapshot>,
    device: &mut DesktopMessageDevice,
) -> Result<()> {
    bank.message = MessageObject::from_snapshot(bank.message.snapshot())?;
    if let Some(surface) = pixels.remove(&bank.message.id()) {
        device.restore_pixels(&bank.message, surface)?;
    }
    if let Some(log) = &mut bank.log {
        for row in &mut log.rows { restore_bank(row, pixels, device)?; }
    }
    Ok(())
}

pub(super) mod metadata {
    use super::*;
    use cs2_runtime::fes_controls::MetadataReference;
    type Saved = (Option<(Vec<u8>, u32)>, Option<(Vec<u8>, u32)>, bool);
    pub(crate) fn serialize<S: cs2_runtime::graph_serde::Serializer>(
        value: &crate::fes_driver::main_flow::GeometryMetadata, serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let reference = |v: &Option<MetadataReference>| v.as_ref().map(|v| (v.name.clone(), v.frame));
        (reference(&value.position_source), reference(&value.size_source), value.size_dirty).serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: cs2_runtime::graph_serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<crate::fes_driver::main_flow::GeometryMetadata, D::Error> {
        let (position, size, dirty) = Saved::deserialize(deserializer)?;
        let reference = |v: (Vec<u8>, u32)| MetadataReference { name: v.0, frame: v.1 };
        Ok(crate::fes_driver::main_flow::GeometryMetadata {
            position_source: position.map(reference), size_source: size.map(reference), size_dirty: dirty,
        })
    }
}

pub(super) mod clip {
    use super::*;
    type Saved = Option<([i32; 2], [u32; 2])>;
    pub(crate) fn serialize<S: cs2_runtime::graph_serde::Serializer>(
        value: &Option<Rect>, serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        value.map(|v| ([v.x, v.y], [v.width, v.height])).serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: cs2_runtime::graph_serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<Rect>, D::Error> {
        Ok(Saved::deserialize(deserializer)?.map(|(at, size)| Rect::new(at[0], at[1], size[0], size[1])))
    }
}

pub(super) mod projection {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(crate = "cs2_runtime::graph_serde")]
    struct Saved {
        owner: u32, name: Vec<u8>, bank: i32, frame: ImageFrameSnapshot,
        at: [i32; 2], scale: [u32; 2], opacity: u8, visible: bool,
        #[serde(with = "super::clip")]
        clip: Option<Rect>,
        pivot: [u32; 2], rotation: u32, flip: [bool; 2], blend: u32, linear: bool,
        priority: i32,
    }
    pub(crate) fn serialize<S: cs2_runtime::graph_serde::Serializer>(
        value: &Option<FesTextProjection>, serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        if value.as_ref().is_some_and(|v| v.options.mask.is_some()) {
            return Err(cs2_runtime::graph_serde::ser::Error::custom("FES text projection has an unsupported mask owner"));
        }
        value.as_ref().map(|v| {
            let o = &v.options;
            Saved { owner: v.owner, name: v.name.clone(), bank: v.bank,
                frame: crate::image_manager::capture_frame(&v.frame),
                at: [o.position.x, o.position.y], scale: [o.scale_x.to_bits(), o.scale_y.to_bits()],
                opacity: o.opacity, visible: o.visible, clip: o.clip, pivot: o.pivot.map(f32::to_bits),
                rotation: o.rotation_radians.to_bits(), flip: [o.flip_x, o.flip_y], blend: o.blend_mode as u32,
                linear: o.filter == cs2_renderer::ScaleFilter::Linear, priority: v.priority }
        }).serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: cs2_runtime::graph_serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<FesTextProjection>, D::Error> {
        Option::<Saved>::deserialize(deserializer)?.map(|s| {
            if s.blend > 5 {
                return Err(cs2_runtime::graph_serde::de::Error::custom("FES text projection blend is invalid"));
            }
            let frame = crate::image_manager::restore_frame(&s.frame)
                .map_err(cs2_runtime::graph_serde::de::Error::custom)?;
            let mut options = ImageDrawOptions::new(Point::new(s.at[0], s.at[1]));
            options.scale_x = f32::from_bits(s.scale[0]); options.scale_y = f32::from_bits(s.scale[1]);
            options.opacity = s.opacity; options.visible = s.visible; options.clip = s.clip;
            options.pivot = s.pivot.map(f32::from_bits); options.rotation_radians = f32::from_bits(s.rotation);
            options.flip_x = s.flip[0]; options.flip_y = s.flip[1];
            options.blend_mode = cs2_renderer::BlendMode::from_raw(s.blend);
            options.filter = if s.linear { cs2_renderer::ScaleFilter::Linear } else { cs2_renderer::ScaleFilter::Nearest };
            Ok(FesTextProjection { owner: s.owner, name: s.name, bank: s.bank,
                frame, options, priority: s.priority })
        }).transpose()
    }
}
