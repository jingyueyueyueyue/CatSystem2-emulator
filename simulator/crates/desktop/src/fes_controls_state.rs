//! Codec of the existing controls store for the enclosing graph transaction.
//! Preparing owns all fallible work; committing never refreshes UI or pixels.
use super::*;
use cs2_runtime::graph_serde::{Serializer, Deserializer};
use cs2_runtime::object_graph::{encode_component, decode_component, ImageFrameSnapshot};

#[derive(Serialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Capture<'a> { version: u32, store: &'a FesControlStore, sources: BTreeSet<Key> }
#[derive(Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Saved { version: u32, store: FesControlStore, sources: BTreeSet<Key> }

impl FesControlStore {
    pub(crate) fn capture_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<Vec<u8>> {
        self.validate_checkpoint(kinds)?;
        Ok(encode_component(&Capture { version: 1, store: self,
            sources: self.banks.iter().filter(|(_, bank)| bank.source.is_some())
                .map(|(key, _)| key.clone()).collect() })?)
    }

    pub(crate) fn prepare_checkpoint(bytes: &[u8], kinds: &BTreeMap<u32, u32>) -> Result<Self> {
        let mut saved: Saved = decode_component(bytes)?;
        if saved.version != 1 { bail!("unsupported FES controls checkpoint version"); }
        saved.store.validate_checkpoint(kinds)?;
        if saved.sources.len() > MAX_BANKS || saved.sources.iter().any(|key|
            saved.store.banks.get(key).is_none_or(|bank| bank.source_bytes.is_empty())) {
            bail!("control checkpoint HG metadata identity has no retained bytes");
        }
        for (key, bank) in &mut saved.store.banks {
            if saved.sources.contains(key) {
                let source = HgFile::parse(&bank.source_bytes)?;
                if source.signature != *b"HG-3" {
                    bail!("control checkpoint retained source is not HG-3");
                }
                bank.source = Some(source);
            } else if !bank.source_bytes.is_empty() {
                bail!("control checkpoint source bytes lack their captured HG identity");
            }
        }
        Ok(saved.store)
    }

    pub(crate) fn commit_checkpoint(&mut self, prepared: Self) { *self = prepared; }

    pub(crate) fn checkpoint_bank_identities(&self) -> BTreeSet<(u32, Vec<u8>, i32)> {
        self.banks.keys().cloned().collect()
    }

    pub(crate) fn checkpoint_lower_owner(&self, task: u32) -> Option<u32> {
        self.banks.iter().find_map(|((owner, _, _), bank)|
            (bank.lower == Some(task)).then_some(*owner))
    }

    pub(crate) fn validate_checkpoint_owner(&self, owner: u32, driver: &FesScriptDriver) -> Result<()> {
        for ((id, name, bank), object) in &self.banks {
            if *id != owner { continue; }
            let definition = driver.adapter.object_bank_definition(name, *bank as usize)
                .context("control checkpoint bank lost its original driver declaration")?;
            if control_kind(&definition.kind_name) != Some(object.kind) {
                bail!("control checkpoint kind differs from its original driver declaration");
            }
        }
        Ok(())
    }

    fn validate_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<()> {
        let owner = |id: &u32| kinds.get(id) == Some(&34);
        let key_ok = |key: &Key| owner(&key.0) && key.2 >= 0 && key.1.len() <= 31
            && !key.1.contains(&0);
        if self.banks.len() > MAX_BANKS || self.declarations.len() > MAX_BANKS
            || self.creations.len() > MAX_BANKS || self.initializers.len() > MAX_BANKS
            || self.commands.len() > MAX_BANKS || self.consumers.len() > MAX_BANKS
            || self.text_viewport_initializers.len() > MAX_BANKS || self.pending_inputs.len() > 4096 {
            bail!("FES controls checkpoint exceeds live storage bounds");
        }
        let mut tasks = BTreeSet::new();
        let mut bytes = 0usize;
        for (key, bank) in &self.banks {
            if !key_ok(key) || bank.direction > 1 || bank.state > 3
                || bank.file.len() > 259 || bank.expanded_file.as_ref().is_some_and(|v| v.len() > 259)
                || bank.source_bytes.len() as u64 > MAX_IMAGE
                || bank.kind == Kind::Viewport && !bank.source_bytes.is_empty()
                || (!bank.is_slider() && (bank.lower.is_some()
                    || bank.button_regions().next().is_some() || bank.drag_origin.is_some()))
                || !bank.buttons.valid_for(bank.kind) {
                bail!("FES controls checkpoint bank or source identity is invalid");
            }
            for reference in bank.plane.iter().chain(&bank.parent) {
                validate_reference(reference)?;
            }
            for reference in bank.size_source.iter().chain(&bank.position_source)
                .chain(bank.viewport.size_source.iter()).chain(bank.viewport.position_source.iter()) {
                if reference.name.len() > 259 || reference.name.contains(&0) {
                    bail!("FES controls checkpoint metadata resource is invalid");
                }
            }
            if bank.is_slider() {
                let lower = bank.lower.context("control checkpoint slider has no actual lower task")?;
                let lower_kind = if bank.kind == Kind::Slider { 42 } else { 41 };
                if kinds.get(&lower) != Some(&lower_kind)
                    || !tasks.insert(lower) {
                    bail!("control checkpoint lower task kind/identity differs");
                }
                for button in bank.button_regions() {
                    if kinds.get(&button) != Some(&6) || !tasks.insert(button) {
                        bail!("control checkpoint button task kind/identity differs");
                    }
                }
            }
            if bank.list_override.is_some_and(|id| kinds.get(&id) != Some(&15)) {
                bail!("control checkpoint list override is not a live type15 owner");
            }
            bytes = bytes.checked_add(bank.source_bytes.len())
                .context("control checkpoint byte accounting overflow")?;
            for frame in bank.control_image.iter().chain(bank.projection.iter().map(|p| &p.frame)) {
                bytes = bytes.checked_add(frame.pixels().len())
                    .context("control checkpoint pixel accounting overflow")?;
            }
            if bytes > cs2_runtime::object_graph::MAX_GRAPH_BYTES {
                bail!("control checkpoint source/pixel storage exceeds graph limit");
            }
            if let Some(projection) = &bank.projection {
                if (projection.owner, &projection.name, projection.bank) != (key.0, &key.1, key.2)
                    || projection.list.is_some_and(|id| kinds.get(&id) != Some(&15)) {
                    bail!("control projection differs from its actual bank/list owner");
                }
            }
            if bank.source.is_some() && bank.source_bytes.is_empty() {
                bail!("control checkpoint has HG metadata without its retained source bytes");
            }
            if bank.source.as_ref().is_some_and(|source| source.signature != *b"HG-3") {
                bail!("control checkpoint metadata is not the actual HG-3 source");
            }
        }
        if self.declarations.iter().any(|(id, _)| !owner(id))
            || self.creations.iter().any(|((id, _), cursor)| !owner(id) || *cursor > MAX_BANKS)
            || self.text_viewport_initializers.keys().any(|(id, _)| !owner(id)) {
            bail!("control checkpoint declaration/initializer owner is absent");
        }
        for ((id, _), progress) in &self.initializers {
            if !owner(id) || progress.cursor > progress.fields.len() || progress.fields.len() > MAX_BANKS
                || progress.keys.len() > MAX_BANKS || progress.keys.iter().any(|key|
                    key.0 != *id || !self.banks.contains_key(key))
                || progress.fields.iter().any(|field| *field >= progress.fields.len())
                || progress.fields.iter().copied().collect::<BTreeSet<_>>().len() != progress.fields.len() {
                bail!("control checkpoint initializer cursor or bank owner is invalid");
            }
        }
        for ((id, _), progress) in &self.commands {
            let argument_count = match progress.verb.as_slice() {
                b"disp" | b"enable" | b"alpha" | b"pri" | b"setid" | b"getid" | b"lock" => 1,
                b"size" | b"settbl" => 2,
                b"pos" | b"page" | b"getpos" | b"getpos2" | b"getsize" | b"keypos" => 3,
                b"load" => 0,
                b"pos2" => usize::from(progress.raw.is_some()),
                _ => bail!("control checkpoint contains an unknown prepared command"),
            };
            if !owner(id) || progress.cursor > progress.keys.len() || progress.keys.len() > MAX_BANKS
                || progress.arguments.len() != argument_count || progress.bank_values.len() > progress.keys.len()
                || progress.verb == b"load" && progress.bank_values.len() != progress.keys.len()
                || progress.raw.as_ref().is_some_and(|raw| raw.len() > MAX_IMAGE as usize)
                || progress.keys.iter().any(|key| key.0 != *id || !self.banks.contains_key(key)) {
                bail!("control checkpoint command lost its exact cursor/bank owner");
            }
        }
        for (key, consumer) in &self.consumers {
            if !key_ok(key) || consumer.layer_key.len() > 4096
                || consumer.task.is_some_and(|task| kinds.get(&task) != Some(&2)) {
                bail!("control checkpoint viewport consumer is invalid");
            }
            // A released/missing viewport deliberately retains last bounds.
            validate_reference(&consumer.reference)?;
        }
        for (id, input) in &self.pending_inputs {
            if !owner(id) || input.event.as_ref().is_some_and(|event| event.len() > 259)
                || input.target.as_ref().is_some_and(|target|
                    !self.banks.contains_key(&(*id, target.name.clone(), target.bank))) {
                bail!("control checkpoint input lost its actual FES bank");
            }
        }
        Ok(())
    }

    /// C calls this against the same prepared task/input graph, before any
    /// component is installed. Missing registered regions are legal: the
    /// normal producer can recreate them, retaining its initial_region bit.
    pub(crate) fn validate_checkpoint_links(
        &self, kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>,
    ) -> Result<()> {
        self.validate_checkpoint(kinds)?;
        for ((owner, _, _), bank) in &self.banks {
            if let Some(lower) = bank.lower {
                if parents.get(&lower) != Some(owner) {
                    bail!("control checkpoint lower task has a different parent");
                }
                for button in bank.button_regions() {
                    if parents.get(&button) != Some(&lower) {
                        bail!("control checkpoint button is not its actual lower-task child");
                    }
                }
            }
        }
        Ok(())
    }

    /// Read an existing bank's destination without cloning its projection or
    /// manufacturing another control store. None means the layer is not one
    /// of this store's live banks; Some(None) is a real, currently unbound bank.
    pub(crate) fn checkpoint_layer_destination(
        &self, owner: u32, layer: &str, fallback: Option<u32>,
    ) -> Option<Option<u32>> {
        self.banks.iter().find_map(|((id, name, bank), object)| {
            (*id == owner && object.kind != Kind::Viewport && control_layer_key(name, *bank) == layer)
                .then(|| object.projection.as_ref().and_then(|p| p.list)
                    .or(object.list_override).or(fallback))
        })
    }
}

fn validate_reference(reference: &Reference) -> Result<()> {
    if reference.name.len() > 31 || reference.name.contains(&0) {
        bail!("control checkpoint object reference exceeds the raw name contract");
    }
    Ok(())
}

pub(super) mod metadata {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(value: &Option<MetadataReference>, s: S) -> std::result::Result<S::Ok, S::Error> {
        value.as_ref().map(|r| (&r.name, r.frame)).serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<MetadataReference>, D::Error> {
        Ok(Option::<(Vec<u8>, u32)>::deserialize(d)?.map(|(name, frame)| MetadataReference { name, frame }))
    }
}

pub(super) mod viewport {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(crate = "cs2_runtime::graph_serde")]
    struct Value { position: [u32; 2], size: [u32; 2],
        position_source: Option<(Vec<u8>, u32)>, size_source: Option<(Vec<u8>, u32)>, rebuild: bool, dirty: bool }
    pub(crate) fn serialize<S: Serializer>(value: &ViewportState, s: S) -> std::result::Result<S::Ok, S::Error> {
        Value { position: value.position.map(f32::to_bits), size: value.size.map(f32::to_bits),
            position_source: value.position_source.as_ref().map(|r| (r.name.clone(), r.frame)),
            size_source: value.size_source.as_ref().map(|r| (r.name.clone(), r.frame)),
            rebuild: value.rebuild, dirty: value.dirty }.serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<ViewportState, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(ViewportState { position: v.position.map(f32::from_bits), size: v.size.map(f32::from_bits),
            position_source: v.position_source.map(|(name, frame)| MetadataReference { name, frame }),
            size_source: v.size_source.map(|(name, frame)| MetadataReference { name, frame }), rebuild: v.rebuild, dirty: v.dirty })
    }
}

pub(super) mod slider {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(v: &SliderControl, s: S) -> std::result::Result<S::Ok, S::Error> {
        (v.range, v.travel, v.position, v.pixel, v.dragging).serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<SliderControl, D::Error> {
        let (range, travel, position, pixel, dragging) = Deserialize::deserialize(d)?;
        Ok(SliderControl { range, travel, position, pixel, dragging })
    }
}

pub(super) mod scroll {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(v: &Scroll2Control, s: S) -> std::result::Result<S::Ok, S::Error> {
        (v.range, v.page, v.track_extent, v.minimum_extent, v.knob_extent, v.travel, v.position, v.pixel, v.dragging).serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Scroll2Control, D::Error> {
        let (range, page, track_extent, minimum_extent, knob_extent, travel, position, pixel, dragging) = Deserialize::deserialize(d)?;
        Ok(Scroll2Control { range, page, track_extent, minimum_extent, knob_extent, travel, position, pixel, dragging })
    }
}

pub(super) mod frame {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(v: &RgbaFrame, s: S) -> std::result::Result<S::Ok, S::Error> {
        crate::image_manager::capture_frame(v).serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<RgbaFrame, D::Error> {
        crate::image_manager::restore_frame(&ImageFrameSnapshot::deserialize(d)?)
            .map_err(cs2_runtime::graph_serde::de::Error::custom)
    }
}

pub(super) mod options {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(crate = "cs2_runtime::graph_serde")]
    struct Value { position: [i32; 2], scale: [u32; 2], opacity: u8, visible: bool,
        clip: Option<([i32; 2], [u32; 2])>, mask: Option<([u32; 2], Vec<u8>)>,
        pivot: [u32; 2], rotation: u32, flip: [bool; 2], blend: u32, filter: u32 }
    pub(crate) fn serialize<S: Serializer>(v: &ImageDrawOptions, s: S) -> std::result::Result<S::Ok, S::Error> {
        Value { position: [v.position.x, v.position.y], scale: [v.scale_x.to_bits(), v.scale_y.to_bits()],
            opacity: v.opacity, visible: v.visible, clip: v.clip.map(|r| ([r.x, r.y], [r.width, r.height])),
            mask: v.mask.as_ref().map(|m| ([m.width(), m.height()], m.alpha().to_vec())), pivot: v.pivot.map(f32::to_bits),
            rotation: v.rotation_radians.to_bits(), flip: [v.flip_x, v.flip_y], blend: v.blend_mode.raw(),
            filter: match v.filter { cs2_renderer::ScaleFilter::Nearest => 0, cs2_renderer::ScaleFilter::Linear => 1 } }.serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<ImageDrawOptions, D::Error> {
        let v = Value::deserialize(d)?;
        if v.blend > 5 || v.filter > 1 { return Err(cs2_runtime::graph_serde::de::Error::custom("invalid control projection blend/filter")); }
        let mask = v.mask.map(|(size, bytes)| cs2_renderer::AlphaMask::new(size[0], size[1], bytes))
            .transpose().map_err(cs2_runtime::graph_serde::de::Error::custom)?;
        Ok(ImageDrawOptions { position: Point::new(v.position[0], v.position[1]), scale_x: f32::from_bits(v.scale[0]),
            scale_y: f32::from_bits(v.scale[1]), opacity: v.opacity, visible: v.visible,
            clip: v.clip.map(|(p, s)| Rect::new(p[0], p[1], s[0], s[1])), mask, pivot: v.pivot.map(f32::from_bits),
            rotation_radians: f32::from_bits(v.rotation), flip_x: v.flip[0], flip_y: v.flip[1],
            blend_mode: cs2_renderer::BlendMode::from_raw(v.blend), filter: if v.filter == 0 { cs2_renderer::ScaleFilter::Nearest } else { cs2_renderer::ScaleFilter::Linear } })
    }
}
