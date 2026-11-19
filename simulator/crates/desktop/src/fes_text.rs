//! FES text planes over the real CPU MessageObject backend.
//! Storage is independent of the image platform's private bindings. Mutating
//! text commands queue work; update_owner commits it and captures query caches.
//! Shift, break-character edits and user-glyph mappings use the same real
//! backend before text/playback updates. LOGSTRING has its own history state.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Context, Result, bail};
use cs2_renderer::{ImageDrawOptions, Point, Rect, RgbaFrame};
use cs2_runtime::message_object::{
    MessageCopy, MessageDefaults, MessageEncoding, MessageObject,
    MessagePlaneDisplay,
};
use cs2_script::{Expression, ExpressionState, FesEvent, FesObjectDefinition, FesObjectInitialization};

use super::fes_driver::{
    FesDriverCommand, FesDriverEffect, FesDriverWait, FesScriptDriver,
};
use super::fes_tasks::FesTaskView;
use super::{FesRuntime, LoggingHost, ScriptTextEncoding, message_backend::DesktopMessageDevice};

const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;
const MAX_BANKS: usize = 65536;

#[path = "fes_log.rs"]
mod log;
#[path = "fes_text_state.rs"]
mod state;
pub(crate) use state::PreparedFesTextState;
#[path = "fes_text_motion.rs"]
mod motion;
pub(super) use motion::{motion_properties, motion_context, apply_motion_properties,
    synchronize_motion_properties, prepare_motion_projection};
#[path = "fes_log_image.rs"]
mod log_image;
pub(super) use log_image::apply_name_image_table;
type Key = (u32, Vec<u8>, i32);

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum TextRequest {
    Clear,
    Append(Vec<u8>),
    Separator,
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct TextBank {
    log: Option<log::LogString>,
    #[serde(default)]
    novel: Option<NovelText>,
    #[serde(with = "state::metadata")]
    metadata: super::fes_driver::main_flow::GeometryMetadata,
    message: MessageObject,
    configuration: Vec<u8>,
    configured: bool,
    dirty: bool,
    /// glyph parameter, face, weight, shadow inheritance.
    inherited: [bool; 4],
    restore_colors: [u32; 3],
    source: Vec<u8>,
    source_initialized: bool,
    pending: VecDeque<TextRequest>,
    shift: u32,
    shift_dirty: bool,
    clear_collections: [bool; 4],
    remove_characters: Option<(u32, Vec<u8>)>,
    add_characters: Option<(u32, Vec<u8>)>,
    user_glyph_source: Option<Vec<u8>>,
    user_source_dirty: bool,
    user_mappings: Vec<(u16, u16)>,
    user_mapping_cursor: usize,
    needs_prepare: bool,
    draw: Option<u32>,
    skip: bool,
    remaining: u32,
    remaining_segment: u32,
    line_count: u32,
    cursor: [i32; 2],
    draw_area: [i32; 2],
    position: [i32; 2],
    origin: [i32; 2],
    vram: [i32; 2],
    parent_plane: Option<String>,
    priority: i32,
    visible: bool,
    tint: u32,
    alpha: u8,
    scale: [f32; 2],
    rotation: f32,
    #[serde(with = "state::clip")]
    clip: Option<Rect>,
    last_frame: Option<u32>,
    #[serde(with = "state::projection")]
    projection: Option<FesTextProjection>,
}

#[derive(Clone, Debug, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct NovelText {
    division: i32,
    insert_pending: bool,
    speaker: Vec<u8>,
}

/// Real submitted RGBA and its plane geometry, ready to merge with image-list
/// layers in signed priority order. No unsubmitted reveal surface is exposed.
#[derive(Clone, Debug)]
pub(super) struct FesTextProjection {
    pub(super) owner: u32,
    pub(super) name: Vec<u8>,
    pub(super) bank: i32,
    pub(super) frame: RgbaFrame,
    pub(super) options: ImageDrawOptions,
    pub(super) priority: i32,
}

impl FesTextProjection {
    /// Lossless image-list identity; CP932 names must not be collapsed by
    /// String::from_utf8_lossy when combined with ordinary image layers.
    pub(super) fn layer_key(&self) -> String {
        layer_key(&self.name, self.bank)
    }
}

fn layer_key(name: &[u8], bank: i32) -> String {
    use std::fmt::Write;
    let mut key = String::from("\0text:");
    for byte in name { let _ = write!(key, "{byte:02x}"); }
    let _ = write!(key, "[{bank}]");
    key
}

pub(super) fn is_text_kind(kind: &[u8]) -> bool {
    [b"STRING".as_slice(), b"STRING8", b"LOGSTRING", b"LOGSTRING8", b"NVSTRING", b"NVSTRING8"]
        .iter().any(|name| kind.eq_ignore_ascii_case(name))
}

fn is_novel(kind: &[u8]) -> bool {
    kind.eq_ignore_ascii_case(b"NVSTRING") || kind.eq_ignore_ascii_case(b"NVSTRING8")
}

fn is_log(kind: &[u8]) -> bool {
    kind.eq_ignore_ascii_case(b"LOGSTRING") || kind.eq_ignore_ascii_case(b"LOGSTRING8")
}

fn is_unicode(kind: &[u8]) -> bool {
    [b"STRING8".as_slice(), b"LOGSTRING8", b"NVSTRING8"]
        .iter().any(|name| kind.eq_ignore_ascii_case(name))
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesTextStore {
    banks: BTreeMap<Key, TextBank>,
    declarations: BTreeSet<(u32, usize)>,
    initializers: BTreeMap<(u32, usize), usize>,
    waits: BTreeMap<u32, (u64, Vec<u8>, Option<i32>)>,
    next_surface: i32,
    pub(super) unicode_strings: bool,
    pub(super) measurement_parameter: u32,
    pub(super) frame_delay: i32,
}

impl Default for FesTextStore {
    fn default() -> Self {
        Self {
            banks: BTreeMap::new(),
            declarations: BTreeSet::new(),
            initializers: BTreeMap::new(),
            waits: BTreeMap::new(),
            // Ordinary message task ids are nonnegative; FES-owned surfaces
            // occupy their own descending namespace and are never native type8.
            next_surface: -1,
            unicode_strings: false,
            measurement_parameter: 0,
            frame_delay: 0,
        }
    }
}

impl FesTextStore {
    pub(crate) fn checkpoint_options(&self) -> (i32, bool, u32, i32) {
        (self.next_surface, self.unicode_strings, self.measurement_parameter, self.frame_delay)
    }
    pub(crate) fn restore_checkpoint_options(&mut self, value: (i32, bool, u32, i32)) {
        self.next_surface = value.0;
        self.unicode_strings = value.1;
        self.measurement_parameter = value.2;
        self.frame_delay = value.3;
    }

    pub(crate) fn check_graph_boundary(&self) -> Result<()> {
        if !self.banks.is_empty() || !self.declarations.is_empty() || !self.initializers.is_empty()
            || !self.waits.is_empty()
        { bail!("FES STRING/LOGSTRING owner checkpoints are not yet supported"); }
        Ok(())
    }

    fn refresh_font_defaults(&mut self, owner: u32, defaults: &MessageDefaults) -> Result<()> {
        for ((id, _, _), bank) in &mut self.banks {
            if *id != owner { continue; }
            let tail = tail(bank.message.encoding());
            if bank.inherited[0] { set(&mut bank.configuration, tail + 12, 0); }
            if bank.inherited[1] { put_font(&mut bank.configuration, bank.message.encoding(), &defaults.font_name)?; }
            if bank.inherited[2] { set(&mut bank.configuration, tail, if defaults.bold { 700 } else { 400 }); }
            if bank.inherited[3] {
                set(&mut bank.configuration, tail + 4, defaults.shadow);
                set(&mut bank.configuration, tail + 8, defaults.outline);
            }
            bank.dirty = true;
        }
        Ok(())
    }

    pub(super) fn text(&self, owner: u32, name: &[u8], bank: i32) -> Option<&[u8]> {
        self.banks.get(&(owner, name.to_vec(), bank)).map(|record| record.source.as_slice())
    }

    pub(super) fn release_owner(&mut self, owner: u32, device: &mut DesktopMessageDevice) {
        // An owner spans arbitrary names; use the same per-bank destructor
        // as mixed subtree release and declaration replacement.
        let keys: Vec<_> = self.banks.keys()
            .filter(|(id, _, _)| *id == owner).cloned().collect();
        for key in keys { self.release_bank(&key, device); }
        self.declarations.retain(|(id, _)| *id != owner);
        self.initializers.retain(|(id, _), _| *id != owner);
        self.waits.remove(&owner);
    }

    fn release_bank(&mut self, key: &Key, device: &mut DesktopMessageDevice) {
        if let Some(bank) = self.banks.remove(key) { bank.release(device); }
        // Keep the evaluated wait target until its normal polling boundary:
        // remaining banks still own their wait, and no expression is replayed.
        // An exact deleted bank now resolves to no remaining text work.
    }

    fn keys(&self, owner: u32, name: &[u8], bank: Option<i32>) -> Vec<Key> {
        self.banks.keys().filter(|(id, object, index)|
            *id == owner && object == name && bank.is_none_or(|bank| bank == -1 || bank == *index))
            .cloned().collect()
    }

    fn create(
        &mut self,
        owner: u32,
        definition: &FesObjectDefinition,
        defaults: &MessageDefaults,
        device: &mut DesktopMessageDevice,
    ) -> Result<()> {
        if self.declarations.contains(&(owner, definition.offset)) {
            return Ok(());
        }
        let count = if definition.banks < 1 { 1 } else { definition.banks as usize };
        if count > MAX_BANKS || self.banks.len().saturating_add(count) > MAX_BANKS {
            bail!("FES text bank count exceeds {MAX_BANKS}");
        }
        let encoding = if is_unicode(&definition.kind) || self.unicode_strings {
            MessageEncoding::Unicode
        } else {
            MessageEncoding::Cp932
        };
        // Build all configuration bytes before replacing an old binding.
        let configuration = initial_configuration(encoding, defaults, self.measurement_parameter)?;
        for index in 0..count {
            let key = (owner, definition.name.clone(), index as i32);
            // A duplicate declaration has its own lower message object. The
            // latest text binding is retained without borrowing image bindings.
            if let Some(old) = self.banks.remove(&key) {
                old.release(device);
            }
            let id = self.next_surface;
            self.next_surface = id.checked_sub(1).context("FES text surface id space exhausted")?;
            self.banks.insert(key, TextBank {
                log: is_log(&definition.kind).then(log::LogString::default),
                novel: is_novel(&definition.kind).then(NovelText::default),
                metadata: Default::default(),
                message: MessageObject::new(id, encoding),
                configuration: configuration.clone(),
                configured: false, dirty: true,
                inherited: [encoding == MessageEncoding::Cp932, true, true, true],
                restore_colors: [0xffff_ffff, 0xff00_0000, 0xffff_ffff],
                source: Vec::new(), source_initialized: false, pending: VecDeque::new(),
                shift: 0, shift_dirty: false, clear_collections: [false; 4],
                remove_characters: None, add_characters: None,
                user_glyph_source: None, user_source_dirty: false,
                user_mappings: Vec::new(), user_mapping_cursor: 0,
                needs_prepare: false, draw: None, skip: false,
                remaining: 0, remaining_segment: 0, line_count: 0,
                cursor: [0; 2], draw_area: [0; 2],
                position: [0; 2], origin: [0; 2], vram: [0; 2],
                parent_plane: None, priority: 0, visible: false,
                tint: u32::MAX, alpha: 255, scale: [1.0; 2], rotation: 0.0,
                clip: None, last_frame: None, projection: None,
            });
        }
        self.declarations.insert((owner, definition.offset));
        Ok(())
    }

    fn initialize(
        &mut self,
        host: &mut LoggingHost<'_>,
        view: &FesTaskView,
        runtime: &mut FesRuntime,
        initializer: &FesObjectInitialization,
        driver: &mut FesScriptDriver,
        shared: &mut ExpressionState,
    ) -> Result<bool> {
        let owner = view.id;
        let encoding = host.text_encoding;
        let (name, bank_source) = split_target(&initializer.target)?;
        if self.keys(owner, name, None).is_empty() {
            return Ok(false);
        }
        // Initialization targets were already numerically resolved by the
        // FesInterpreter. Read that decimal bank without repeating expressions.
        let bank = bank_source.map(|source| -> Result<i32> {
            Ok(std::str::from_utf8(source)?.parse()?)
        }).transpose()?;
        let keys = self.keys(owner, name, bank);
        if keys.is_empty() { return Ok(true); }
        let count = initializer.columns.len().min(initializer.values.len());
        let mut cursor = *self.initializers.get(&(owner, initializer.offset)).unwrap_or(&0);
        while cursor < count {
            let column = initializer.columns[cursor].as_slice();
            let raw = initializer.values[cursor].as_slice();
            if column == b"PARENT" {
                let canvas_parent = if keys.iter().any(|key| uses_canvas_atlas(&self.banks[key])) {
                    resolve_canvas_parent(raw, encoding, driver, shared)?
                } else {
                    None
                };
                let motion_parent = canvas_parent.as_ref().map_or(raw, |(_, parent)| parent.as_slice());
                for key in &keys {
                    super::fes_motion::initialize_field(
                        host, view, runtime, driver, shared, name, key.2,
                        initializer.offset, cursor, column, motion_parent,
                    )?;
                    let bank = self.banks.get_mut(key).unwrap();
                    if uses_canvas_atlas(bank) && let Some((plane, _)) = &canvas_parent {
                        bank.parent_plane = plane.clone();
                    }
                }
            } else if column == b"PL" {
                let plane = encoding.decode(raw);
                for key in &keys {
                    self.banks.get_mut(key).unwrap().parent_plane =
                        (!plane.is_empty()).then_some(plane.clone());
                }
            } else if matches!(column, b"STR" | b"TEXT") {
                for key in &keys {
                    let bank = self.banks.get_mut(key).unwrap();
                    let text = normalize_text(raw, bank.message.encoding(), false, encoding)?;
                    bank.replace(&text)?;
                }
            } else if matches!(column, b"POS2" | b"SIZE2") {
                let reference = super::fes_controls::metadata_reference(raw, encoding)?;
                for key in &keys {
                    let metadata = &mut self.banks.get_mut(key).unwrap().metadata;
                    if column == b"POS2" { metadata.position_source = Some(reference.clone()); }
                    else { metadata.size_source = Some(reference.clone()); metadata.size_dirty = true; }
                    if column == b"POS2" {
                        if let Some(log) = &mut self.banks.get_mut(key).unwrap().log { log.geometry_dirty = true; }
                    }
                }
            } else {
                let values = raw.split(|&byte| byte == b',')
                    .map(|value| driver.evaluate_parameter(value, 0, shared))
                    .collect::<Result<Vec<_>>>()?;
                for key in &keys {
                    let bank = self.banks.get_mut(key).unwrap();
                    match column {
                        b"POS" => bank.position = pair(&values, [0; 2]),
                        b"VRAM" => { bank.vram = pair(&values, [0; 2]); bank.dirty = true; }
                        b"ORG" => bank.origin = pair(&values, [0; 2]),
                        b"PRI" => bank.priority = values.first().copied().unwrap_or(0),
                        b"DISP" => bank.visible = values.first().copied().unwrap_or(0) != 0,
                        b"COUNT" if bank.log.is_some() => bank.log.as_mut().unwrap()
                            .set_count(values.first().copied().unwrap_or(3))?,
                        b"DIV" => bank.initialize_division(values.first().copied().unwrap_or(0))?,
                        b"SIZE" => bank.set_size(pair(&values, [0; 2]))?,
                        b"COLOR" => bank.tint = values.first().copied().unwrap_or(-1) as u32,
                        b"ALPHA" | b"BLEND" => bank.alpha = values.first().copied().unwrap_or(255)
                            .clamp(0, 255) as u8,
                        b"SCALE" => {
                            let values = pair(&values, [100; 2]);
                            bank.scale = [values[0] as f32 / 100.0, values[1] as f32 / 100.0];
                        }
                        b"ARC" => bank.rotation = values.first().copied().unwrap_or(0) as f32
                            * std::f32::consts::PI / 180.0,
                        _ => bail!("unsupported STRING initializer column {:?}", String::from_utf8_lossy(column)),
                    }
                    if let Some(log) = &mut bank.log {
                        log.geometry_dirty |= column == b"POS";
                        if column == b"DISP" { log.initialize_display(bank.visible); }
                    }
                }
            }
            for key in &keys {
                host.data_services.fes_motion.synchronize_text(
                    self, owner, &key.1, key.2, column,
                )?;
            }
            cursor += 1;
            self.initializers.insert((owner, initializer.offset), cursor);
        }
        Ok(true)
    }

    fn command(
        &mut self,
        owner: u32,
        command: &FesDriverCommand,
        driver: &mut FesScriptDriver,
        shared: &mut ExpressionState,
        defaults: &MessageDefaults,
        resource_encoding: ScriptTextEncoding,
        runtime: &mut FesRuntime,
        host: &mut LoggingHost<'_>,
    ) -> Result<bool> {
        let Some(target) = command.parameters.first() else { return Ok(false) };
        let (name, bank_source) = split_target(target)?;
        if self.keys(owner, name, None).is_empty() {
            return Ok(false);
        }
        let Some(verb) = command.parameters.get(1).map(Vec::as_slice) else { return Ok(true) };
        if matches!(verb, b"stop" | b"oneshot" | b"continue") {
            return Ok(false); // Delegate before evaluating any bank or arguments.
        }
        if verb == b"getsize" && bank_source.is_none()
            && driver.adapter.object(name).is_some_and(|object| object.definition.bank_count > 1)
        {
            return Ok(true); // Array query skips even destination expressions.
        }
        let bank = bank_source.map(|value|
            driver.evaluate_parameter(value, command.context.current, shared)).transpose()?;
        let keys = self.keys(owner, name, bank);
        if keys.is_empty() {
            driver.fallback_command(command, shared)?;
            return Ok(true);
        }
        let args = &command.parameters[2..];
        if log::command(self, &keys, command, driver, shared, host.backlogs, bank == Some(-1))? {
            return Ok(true);
        }
        if self.banks[&keys[0]].log.is_some()
            && !matches!(verb, b"layout" | b"reset" | b"userfontobj" | b"userfont"
                | b"pri" | b"alpha" | b"color" | b"pos2" | b"org" | b"vram"
                | b"size" | b"size2" | b"scale" | b"arc" | b"clip")
        {
            bail!("LOGSTRING command {:?} has no connected history receiver", String::from_utf8_lossy(verb));
        }
        match verb {
            b"layout" => self.layout(&keys, command, driver, shared, defaults)?,
            b"insert" if self.banks[&keys[0]].novel.is_some() => {
                for key in &keys {
                    self.banks.get_mut(key).unwrap().novel.as_mut().unwrap().insert_pending = true;
                }
            }
            b"name" => {
                let raw = raw_after_spaces(&command.source, 2).unwrap_or_default();
                for key in &keys {
                    self.banks.get_mut(key).unwrap().set_speaker(raw, resource_encoding)?;
                }
            }
            b"str" | b"strsj" | b"apend" => {
                let raw = raw_after_spaces(&command.source, 2).unwrap_or_default();
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    let text = normalize_text(raw, record.message.encoding(), verb == b"strsj", resource_encoding)?;
                    if verb == b"apend" { record.append_body(&text)?; } else { record.replace(&text)?; }
                }
            }
            b"apendmark" => for key in &keys {
                let record = self.banks.get_mut(key).unwrap();
                if !record.source_initialized { bail!("STRING apendmark requires initialized text"); }
                let marker: &[u8] = if record.message.encoding() == MessageEncoding::Unicode {
                    b"\xef\xbf\xbf"
                } else { b"\xff" };
                record.check_append(marker.len())?;
                record.source.extend_from_slice(marker);
                record.pending.push_back(TextRequest::Separator);
                record.needs_prepare = true;
            },
            b"clear" => for key in &keys { self.banks.get_mut(key).unwrap().replace(&[])?; },
            b"reset" => for key in &keys {
                let record = self.banks.get_mut(key).unwrap();
                let size = [word(&record.configuration, 8), word(&record.configuration, 12)];
                record.configuration = initial_configuration(record.message.encoding(), defaults, self.measurement_parameter)?;
                set(&mut record.configuration, 8, size[0]);
                set(&mut record.configuration, 12, size[1]);
                for (offset, color) in [0x10, 0x14, 0x18].into_iter().zip(record.restore_colors) {
                    set(&mut record.configuration, offset, color);
                }
                record.inherited = [record.message.encoding() == MessageEncoding::Cp932, true, true, true];
                record.dirty = true;
            },
            b"draw" => {
                let segment = optional_number(args, 0, 0, driver, shared)?;
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    record.draw = Some(segment as u32);
                    record.remaining = 1;
                    record.remaining_segment = 1;
                }
            }
            b"skip" => for key in &keys { self.banks.get_mut(key).unwrap().skip = true; },
            b"userfontobj" => {
                let Some(reference) = args.first() else { return Ok(true) };
                let (source_name, source_bank) = split_target(reference)?;
                let source_bank = source_bank
                    .map(|source| driver.evaluate_parameter(source, command.context.current, shared))
                    .transpose()?.unwrap_or(0);
                let decoded = resource_encoding.decode(source_name);
                let banked = format!("{decoded}[{source_bank}]");
                let source_key = if runtime.media.contains_key(&banked) { Some(banked) }
                    else if source_bank == 0 { Some(decoded) } else { None };
                let source = source_key.as_ref().and_then(|key| runtime.media.get_mut(key));
                let bytes = if let Some(source) = source {
                    if source.kind != "IMAGE" {
                        bail!("STRING userfontobj requires an IMAGE raw-data source");
                    }
                    let bytes = super::fes_media::prepare_raw_image_source(host, source)?;
                    if bytes.is_some_and(|bytes| bytes.len() > MAX_TEXT_BYTES) {
                        bail!("STRING user glyph source exceeds 64 MiB");
                    }
                    bytes.map(<[u8]>::to_vec)
                } else { None };
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    record.user_glyph_source = bytes.clone();
                    record.user_source_dirty = true;
                    record.user_mapping_cursor = 0;
                }
            }
            b"userfont" => {
                let Some(character) = args.first() else { return Ok(true) };
                let character = driver.evaluate_parameter(character, command.context.current, shared)? as u16;
                let Some(frame) = args.get(1) else { return Ok(true) };
                let frame = driver.evaluate_parameter(frame, command.context.current, shared)? as u16;
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    if record.user_mappings.len() >= MAX_BANKS {
                        bail!("STRING user glyph mapping count exceeds limit");
                    }
                    record.user_mappings.push((character, frame));
                    record.user_mapping_cursor = 0;
                }
            }
            b"isdraw" | b"isdrawmark" => {
                for key in &keys {
                    let record = &self.banks[key];
                    if let Some(slot) = driver.interpreter.local_values_mut().first_mut() {
                        *slot = if verb == b"isdraw" { record.remaining } else { record.remaining_segment } as i32;
                    }
                }
            }
            b"linecnt" => {
                let output = optional_number(args, 0, -1, driver, shared)?;
                for key in &keys { write_shared(shared, output, self.banks[key].line_count as i32); }
            }
            b"getsize" => {
                let mut outputs = [-1; 3];
                for (axis, output) in outputs.iter_mut().enumerate() {
                    *output = optional_number(args, axis, -1, driver, shared)?;
                }
                for key in &keys {
                    let record = &self.banks[key];
                    let size = [word(&record.configuration, 8) as i32,
                        word(&record.configuration, 12) as i32, 0];
                    for axis in 0..3 { write_shared(shared, outputs[axis], size[axis]); }
                }
            }
            b"endpos" | b"drawarea" | b"getpos2" => {
                let x = optional_number(args, 0, -1, driver, shared)?;
                let y = optional_number(args, 1, -1, driver, shared)?;
                for key in &keys {
                    let record = &self.banks[key];
                    let point = if verb == b"endpos" { record.cursor }
                        else if verb == b"drawarea" { record.draw_area } else { record.position };
                    write_shared(shared, x, point[0]);
                    write_shared(shared, y, point[1]);
                }
            }
            b"strid" => {
                let mut id = -1;
                let mut output = -1;
                for key in &keys {
                    if id == -1 { id = optional_number(args, 0, -1, driver, shared)?; }
                    if output == -1 { output = optional_number(args, 1, -1, driver, shared)?; }
                    let record = &self.banks[key];
                    let value = if id == -1 || !record.configured { 0 } else { record.message.identifier(id) };
                    write_shared(shared, output, value as i32);
                }
            }
            b"lineinfo" => {
                for arg in args.iter().take(3) {
                    driver.evaluate_parameter(arg, command.context.current, shared)?;
                }
            }
            b"disp" | b"pri" | b"alpha" | b"color" => {
                let fallback = if matches!(verb, b"alpha") { 255 } else if verb == b"color" { -1 } else { 0 };
                let value = optional_number(args, 0, fallback, driver, shared)?;
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    match verb {
                        b"disp" => record.visible = value != 0,
                        b"pri" => record.priority = value,
                        b"alpha" => record.alpha = value.clamp(0, 255) as u8,
                        _ => record.tint = value as u32,
                    }
                }
            }
            b"pos2" if args.first().is_some_and(|source| !numeric_position_parameter(source)) => {
                let Some(resource) = args.first() else { return Ok(true) };
                if resource.len() > 127 || resource.contains(&0) {
                    bail!("STRING POS2 resource exceeds its bounded field");
                }
                let frame = optional_number(args, 1, 0, driver, shared)? as u32;
                let reference = cs2_runtime::fes_controls::MetadataReference {
                    name: host.data_services.language.languages.expand_image(resource)?,
                    frame,
                };
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    record.metadata.position_source = Some(reference.clone());
                    if let Some(log) = &mut record.log { log.geometry_dirty = true; }
                }
            }
            b"pos" | b"pos2" | b"org" | b"vram" | b"size" | b"size2" | b"scale" => {
                let old = &self.banks[&keys[0]];
                let current = match verb {
                    b"pos" | b"pos2" => old.position, b"org" => old.origin, b"vram" => old.vram,
                    b"size" | b"size2" => [word(&old.configuration, 8) as i32, word(&old.configuration, 12) as i32],
                    _ => [(old.scale[0] * 100.0) as i32, (old.scale[1] * 100.0) as i32],
                };
                let values = [property_number(args, 0, current[0], driver, shared)?,
                    property_number(args, 1, current[1], driver, shared)?];
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    match verb {
                        b"pos" | b"pos2" => {
                            record.position = values;
                            record.metadata.position_source = None;
                        }
                        b"org" => record.origin = values,
                        b"vram" => { record.vram = values; record.dirty = true; }
                        b"size" | b"size2" => record.set_size(values)?,
                        _ => record.scale = [values[0] as f32 / 100.0, values[1] as f32 / 100.0],
                    }
                    if let Some(log) = &mut record.log { log.geometry_dirty = true; }
                }
            }
            b"arc" => {
                let degrees = optional_number(args, 0, 0, driver, shared)?;
                for key in &keys { self.banks.get_mut(key).unwrap().rotation = (degrees as f32).to_radians(); }
            }
            b"clip" => {
                let mut values = [0; 4];
                for (index, value) in values.iter_mut().enumerate() {
                    *value = optional_number(args, index, 0, driver, shared)?;
                }
                if values[2] < 0 || values[3] < 0 { bail!("STRING clip has negative size"); }
                for key in &keys { self.banks.get_mut(key).unwrap().clip =
                    Some(Rect::new(values[0], values[1], values[2] as u32, values[3] as u32)); }
            }
            _ => bail!("unsupported ordinary STRING command {:?}", String::from_utf8_lossy(verb)),
        }
        for key in &keys {
            host.data_services.fes_motion.synchronize_text(self, owner, &key.1, key.2, verb)?;
        }
        Ok(true)
    }

    fn layout(
        &mut self, keys: &[Key], command: &FesDriverCommand, driver: &mut FesScriptDriver,
        shared: &mut ExpressionState, defaults: &MessageDefaults,
    ) -> Result<()> {
        let Some(verb) = command.parameters.get(2).map(Vec::as_slice) else { return Ok(()) };
        let args = &command.parameters[3..];
        let field_count = match verb {
            b"view" | b"move" | b"shadow" => 2,
            b"color" | b"vcolor" | b"acolor" | b"vacolor" | b"pcolor" | b"vpcolor" => 9,
            b"margin" => 4, b"size" | b"rsize" => 5,
            b"ruby" | b"frame" | b"indent" | b"weight" | b"escape" | b"fontfix"
                | b"prof_enable" | b"profmode" | b"scrolltype" | b"wipewidth"
                | b"fontind_sub" | b"profmode_sub" | b"prof_enable_sub" | b"c_prof_sub"
                | b"prof_sub" | b"shift" | b"prof" | b"d_prof" | b"c_prof" => 1,
            b"shift_sub" => 2,
            b"font" | b"fontind" => 0,
            _ => return Ok(()), // Documented unknown layouts do not evaluate arguments.
        };
        let mut values = Vec::new();
        for value in args.iter().take(field_count) {
            values.push(driver.evaluate_parameter(value, command.context.current, shared)?);
        }
        if matches!(verb, b"fontind_sub" | b"profmode_sub" | b"prof_enable_sub" | b"c_prof_sub"
            | b"prof_sub" | b"shift_sub" | b"vcolor" | b"acolor" | b"vacolor" | b"pcolor" | b"vpcolor") {
            return Ok(());
        }
        if matches!(verb, b"shift" | b"prof" | b"d_prof" | b"c_prof") {
            let index = values.first().copied().unwrap_or(0) as u32;
            let characters = if matches!(verb, b"prof" | b"d_prof") {
                Some(raw_after_spaces(&command.source, 4)
                    .context("STRING character collection lacks its raw character body")?)
            } else { None };
            // Parameters were evaluated once before broadcasting to banks.
            // The two raw accumulators have independent last-index wins rules.
            for key in keys {
                let record = self.banks.get_mut(key).unwrap();
                match verb {
                    b"shift" => { record.shift = index; record.shift_dirty = true; }
                    b"c_prof" => {
                        if let Some(clear) = record.clear_collections.get_mut(index as usize) {
                            *clear = true;
                        }
                    }
                    b"prof" => accumulate_characters(&mut record.add_characters, index, characters.unwrap())?,
                    b"d_prof" => accumulate_characters(&mut record.remove_characters, index, characters.unwrap())?,
                    _ => unreachable!(),
                }
            }
            return Ok(());
        }
        if matches!(verb, b"font" | b"fontind") {
            let backend = if verb == b"fontind" {
                optional_number(args, 0, 0, driver, shared)? as u32
            } else { 0 };
            let count = if verb == b"fontind" { 4 } else { 3 };
            let name = raw_after_spaces(&command.source, count).unwrap_or_default();
            for key in keys {
                let record = self.banks.get_mut(key).unwrap();
                let font = if name.is_empty() { defaults.font_name.clone() }
                    else if verb == b"font" && record.message.encoding() == MessageEncoding::Unicode {
                        unicode_layout_font(name)?
                    } else {
                        std::str::from_utf8(name).context("STRING layout font must be UTF-8")?
                            .encode_utf16().collect()
                    };
                put_font(&mut record.configuration, record.message.encoding(), &font)?;
                let tail = tail(record.message.encoding());
                set(&mut record.configuration, tail + 20, backend);
                record.inherited[1] = false; record.dirty = true;
            }
            return Ok(());
        }
        let value = |index: usize, default: i32| values.get(index).copied().unwrap_or(default);
        for key in keys {
            let record = self.banks.get_mut(key).unwrap();
            let tail = tail(record.message.encoding());
            let was_dirty = record.dirty;
            let mut configure = true;
            match verb {
                b"view" => {
                    record.set_size([value(0, 800), value(1, 600)])?;
                    if record.message.encoding() == MessageEncoding::Unicode {
                        // Observed Unicode path writes this font-buffer DWORD
                        // rather than setting its independent config-dirty bit.
                        set(&mut record.configuration, 0xe8, 1);
                        configure = false;
                    }
                }
                b"color" => {
                    for (color, offset) in [0x10, 0x14, 0x18].into_iter().enumerate() {
                        let rgb = 0xff00_0000 | ((value(color * 3, 0) as u32) & 255)
                            | (((value(color * 3 + 1, 0) as u32) & 255) << 8)
                            | (((value(color * 3 + 2, 0) as u32) & 255) << 16);
                        set(&mut record.configuration, offset, rgb);
                        record.restore_colors[color] = rgb;
                    }
                }
                b"margin" => for (index, offset) in [0x44, 0x48, 0x4c, 0x50].into_iter().enumerate() {
                    set(&mut record.configuration, offset, value(index, 0) as u32);
                },
                b"move" => { set(&mut record.configuration, 0x54, value(0, 0) as u32);
                    set(&mut record.configuration, 0x58, value(1, 0) as u32); }
                b"ruby" => set(&mut record.configuration, 0x5c, value(0, 0) as u32),
                b"frame" => {
                    let delay = match value(0, -1) { -1 => self.frame_delay, v => v };
                    set(&mut record.configuration, 0x74, delay as u32);
                    set(&mut record.configuration, 0x78, delay as u32);
                }
                b"shadow" => {
                    set(&mut record.configuration, tail + 4, value(0, 0) as u32);
                    set(&mut record.configuration, tail + 8, value(1, 0) as u32);
                    record.inherited[3] = false;
                }
                b"size" | b"rsize" => {
                    let first = value(0, if verb == b"size" { 20 } else { 10 });
                    let offset = if verb == b"size" { 0x1c } else { 0x30 };
                    for index in 0..5 { set(&mut record.configuration, offset + index * 4, value(index, first) as u32); }
                }
                b"indent" => set(&mut record.configuration, 0x70, value(0, 0) as u32),
                b"escape" => set(&mut record.configuration, 0x64, value(0, 0) as u32),
                b"fontfix" => set(&mut record.configuration, tail + 12, value(0, 0) as u32),
                b"prof_enable" => set(&mut record.configuration, 0x6c, value(0, 0) as u32),
                b"profmode" if record.message.encoding() == MessageEncoding::Unicode =>
                    set(&mut record.configuration, 0x11c, value(0, 0) as u32),
                b"profmode" => configure = false,
                b"scrolltype" => set(&mut record.configuration, 4, u32::from(value(0, 0) != 0)),
                b"wipewidth" => set(&mut record.configuration, tail + 16, value(0, 100) as u32),
                b"weight" => {
                    let weight = value(0, -1);
                    record.inherited[2] = weight == -1;
                    set(&mut record.configuration, tail, if weight == -1 {
                        if defaults.bold { 700 } else { 400 }
                    } else { weight as u32 });
                    configure = false; // Baseline keeps the dirty flag unchanged.
                }
                _ => configure = false,
            }
            record.dirty = was_dirty || configure;
        }
        Ok(())
    }
}

impl TextBank {
    fn set_speaker(&mut self, raw: &[u8], resource_encoding: ScriptTextEncoding) -> Result<()> {
        if let Some(novel) = &mut self.novel {
            let label = raw.strip_prefix(b"\\pr").unwrap_or(raw);
            novel.speaker = normalize_text(label, self.message.encoding(), false, resource_encoding)?;
        }
        Ok(())
    }

    fn initialize_division(&mut self, division: i32) -> Result<()> {
        if let Some(novel) = &mut self.novel {
            novel.division = division;
        } else if self.log.is_some() || division != 0 {
            bail!("unsupported STRING initializer column DIV value {division}");
        }
        Ok(())
    }

    fn release(self, device: &mut DesktopMessageDevice) {
        device.release(self.message.id());
        if let Some(log) = self.log {
            for row in log.rows { row.release(device); }
        }
    }

    fn check_append(&self, length: usize) -> Result<()> {
        if self.source.len().checked_add(length).is_none_or(|n| n > MAX_TEXT_BYTES) {
            bail!("STRING accumulated text exceeds 64 MiB");
        }
        if self.pending.len() >= MAX_BANKS { bail!("STRING pending request count exceeds limit"); }
        Ok(())
    }
    fn append(&mut self, bytes: &[u8]) -> Result<()> {
        self.check_append(bytes.len())?;
        self.source.extend_from_slice(bytes);
        self.source_initialized = true;
        self.pending.push_back(TextRequest::Append(bytes.to_vec()));
        self.needs_prepare = true; self.remaining = u32::MAX; self.remaining_segment = u32::MAX;
        Ok(())
    }
    fn append_body(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() || self.novel.as_ref().is_none_or(|novel| {
            !novel.insert_pending && novel.speaker.is_empty()
        }) {
            return self.append(bytes);
        }
        let novel = self.novel.as_ref().unwrap();
        let mut paragraph = Vec::new();
        if novel.insert_pending && !self.source.is_empty() {
            paragraph.push(b'\n');
        }
        if !novel.speaker.is_empty() {
            paragraph.extend_from_slice(&novel.speaker);
            paragraph.push(b'\n');
        }
        paragraph.extend_from_slice(bytes);
        self.append(&paragraph)?;
        let novel = self.novel.as_mut().unwrap();
        novel.insert_pending = false;
        novel.speaker.clear();
        Ok(())
    }
    fn replace(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_TEXT_BYTES { bail!("STRING replacement text exceeds 64 MiB"); }
        if let Some(novel) = &mut self.novel {
            novel.insert_pending = false;
            novel.speaker.clear();
        }
        self.source.clear(); self.pending.clear();
        self.pending.push_back(TextRequest::Clear);
        self.pending.push_back(TextRequest::Clear);
        self.append(bytes)?;
        Ok(())
    }
    fn set_size(&mut self, size: [i32; 2]) -> Result<()> {
        if size.iter().any(|value| *value < 0) { bail!("STRING view has negative dimensions"); }
        let bytes = (size[0] as usize).checked_mul(size[1] as usize).and_then(|n| n.checked_mul(4));
        if bytes.is_none_or(|n| n > MAX_TEXT_BYTES) { bail!("STRING view exceeds 64 MiB"); }
        set(&mut self.configuration, 8, size[0] as u32); set(&mut self.configuration, 12, size[1] as u32);
        self.dirty = true;
        self.metadata.size_dirty = true;
        Ok(())
    }
}

#[test]
fn fes_text_zero_div_preserves_normal_text_crop() {
    let mut store = FesTextStore::default();
    let mut device = DesktopMessageDevice::default();
    let defaults = defaults();
    for (offset, kind) in [b"STRING".as_slice(), b"STRING8", b"NVSTRING", b"LOGSTRING"]
        .into_iter().enumerate()
    {
        store.create(12, &FesObjectDefinition {
            offset,
            name: kind.to_vec(),
            kind: kind.to_vec(),
            bank_expression: None,
            banks: 1,
        }, &defaults, &mut device).unwrap();
        let bank = store.banks.get_mut(&(12, kind.to_vec(), 0)).unwrap();
        match kind {
            b"NVSTRING" => {
                bank.initialize_division(170).unwrap();
                assert_eq!(bank.novel.as_ref().unwrap().division, 170);
            }
            b"LOGSTRING" => assert!(bank.initialize_division(0).is_err()),
            _ => {
                bank.set_size([200, 64]).unwrap();
                bank.clip = Some(Rect::new(10, 5, 50, 30));
                let configuration = bank.configuration.clone();
                bank.initialize_division(0).unwrap();
                assert_eq!(bank.configuration, configuration);
                assert_eq!(bank.clip, Some(Rect::new(10, 5, 50, 30)));
                assert!(bank.initialize_division(170).is_err());
            }
        }
    }
}

#[test]
fn fes_text_name_only_updates_novel_speaker() {
    let mut store = FesTextStore::default();
    let mut device = DesktopMessageDevice::default();
    let defaults = defaults();
    for (offset, kind) in [b"STRING".as_slice(), b"STRING8", b"NVSTRING"].into_iter().enumerate() {
        store.create(12, &FesObjectDefinition {
            offset,
            name: kind.to_vec(),
            kind: kind.to_vec(),
            bank_expression: None,
            banks: 1,
        }, &defaults, &mut device).unwrap();
        let bank = store.banks.get_mut(&(12, kind.to_vec(), 0)).unwrap();
        bank.replace(b"existing text").unwrap();
        bank.clip = Some(Rect::new(10, 5, 50, 30));
        let request_count = bank.pending.len();
        bank.set_speaker(b"\\pr$fixture", ScriptTextEncoding::Cp932).unwrap();
        assert_eq!(bank.source, b"existing text");
        assert_eq!(bank.pending.len(), request_count);
        assert_eq!(bank.clip, Some(Rect::new(10, 5, 50, 30)));
        if let Some(novel) = &bank.novel {
            assert_eq!(novel.speaker, b"$fixture");
        }
    }
}

/// None means this service has made no change. Install in the extra_effect
/// dispatcher ahead of the generic CPU object initializer.
pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, effect: &FesDriverEffect, shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    let mut store = std::mem::take(&mut host.data_services.fes_text);
    let defaults = host.data_services.messages.defaults.clone().unwrap_or_else(defaults);
    let result = (|| -> Result<Option<bool>> {
        match effect {
            FesDriverEffect::Initialize { event: FesEvent::CreateObject(definition), .. }
                if is_text_kind(&definition.kind) =>
            {
                store.unicode_strings = host.text_encoding == ScriptTextEncoding::Utf8;
                store.create(view.id, definition, &defaults, &mut host.data_services.message_device)?;
                for key in store.keys(view.id, &definition.name, None) {
                    host.data_services.fes_motion.synchronize_text(
                        &store, view.id, &key.1, key.2, b"ALL",
                    )?;
                }
                Ok(Some(true))
            }
            FesDriverEffect::Initialize { event: FesEvent::InitializeObject(initializer), .. } => {
                Ok(store.initialize(host, view, runtime, initializer, driver, shared)?.then_some(true))
            }
            FesDriverEffect::Command(command) => {
                if log::remake_command(host, &mut store, view.id, command, driver, shared)? {
                    return Ok(Some(true));
                }
                Ok(store.command(view.id, command, driver, shared, &defaults, host.text_encoding,
                    runtime, host)?.then_some(true))
            },
            FesDriverEffect::Waiting { wait: FesDriverWait::Control { kind, arguments, entered }, handler_local: false }
                if kind == b"string" =>
            {
                let Some(target) = arguments.first() else {
                    if view.now as u64 > *entered { driver.resume_control(kind); }
                    return Ok(Some(true));
                };
                if store.waits.get(&view.id).is_none_or(|(at, _, _)| *at != *entered) {
                    let (name, bank) = split_target(target)?;
                    let bank = bank.map(|value| driver.evaluate_parameter(value, 0, shared)).transpose()?;
                    store.waits.insert(view.id, (*entered, name.to_vec(), bank));
                }
                let (_, name, bank) = &store.waits[&view.id];
                let keys = store.keys(view.id, name, *bank);
                if view.now as u64 > *entered
                    && keys.iter().all(|key| store.banks[key].remaining == 0)
                {
                    driver.resume_control(kind);
                    store.waits.remove(&view.id);
                }
                Ok(Some(true))
            }
            FesDriverEffect::CancelWait { wait: FesDriverWait::Control { kind, arguments, entered }, reveal_text: true, .. }
                if kind == b"string" =>
            {
                if let Some(target) = arguments.first() {
                    if store.waits.get(&view.id).is_none_or(|(at, _, _)| *at != *entered) {
                        let (name, bank) = split_target(target)?;
                        let bank = bank.map(|value| driver.evaluate_parameter(value, 0, shared)).transpose()?;
                        store.waits.insert(view.id, (*entered, name.to_vec(), bank));
                    }
                    let (_, name, bank) = &store.waits[&view.id];
                    if let Some(key) = store.keys(view.id, name, *bank).into_iter()
                        .find(|key| store.banks[key].remaining != 0)
                    { store.banks.get_mut(&key).unwrap().skip = true; }
                }
                Ok(Some(true))
            }
            FesDriverEffect::SystemBlock { name, complete: true } if name == b"MES_CHANGECONFIG" => {
                store.refresh_font_defaults(view.id, &defaults)?;
                Ok(Some(true))
            }
            _ => Ok(None),
        }
    })();
    host.data_services.fes_text = store;
    result
}

/// Run once per owner logical frame (including hidden text). Successful
/// requests are removed individually before later queries, so a backend error
/// does not append the same suffix twice when update is retried.
pub(super) fn update_owner(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
    _driver: &mut FesScriptDriver,
) -> Result<()> {
    // Resolve the resource references before configuring MessageObject. This
    // changes its actual page size and the existing projection geometry.
    let keys: Vec<_> = host.data_services.fes_text.banks.keys()
        .filter(|(owner, _, _)| *owner == view.id).cloned().collect();
    for key in keys {
        if host.data_services.fes_text.banks[&key].log.is_some() {
            log::geometry(host, &key)?;
            continue;
        }
        let metadata = host.data_services.fes_text.banks[&key].metadata.clone();
        if metadata.size_dirty {
            if let Some(reference) = &metadata.size_source {
                let size = super::fes_controls::read_metadata(host, reference)?.map_or([0; 2], |(_, size)| size);
                host.data_services.fes_text.banks.get_mut(&key).unwrap().set_size(size)?;
            }
            host.data_services.fes_text.banks.get_mut(&key).unwrap().metadata.size_dirty = false;
        }
        if let Some(reference) = &metadata.position_source {
            let position = super::fes_controls::read_metadata(host, reference)?.map_or([0; 2], |(position, _)| position);
            let bank = host.data_services.fes_text.banks.get_mut(&key).unwrap();
            bank.position = position;
            bank.metadata.position_source = None;
            let data = &mut *host.data_services;
            data.fes_motion.synchronize_text(&data.fes_text, view.id, &key.1, key.2, b"POS2")?;
        }
    }
    host.data_services.message_device.load_fonts(host.resources);
    let data = &mut *host.data_services;
    let next_surface = &mut data.fes_text.next_surface;
    for ((owner, name, index), bank) in &mut data.fes_text.banks {
        if *owner != view.id { continue; }
        if bank.log.is_some() {
            log::update(bank, next_surface, &mut data.message_device, view, runtime, name, *index, host.backlogs)?;
        } else {
            render_bank(&mut data.message_device, view, runtime, bank, name, *index)?;
        }
    }
    if view.list.is_some() {
        let novel: Vec<_> = host.data_services.fes_text.banks.iter()
            .filter(|((owner, _, _), bank)| *owner == view.id && bank.novel.is_some())
            .filter_map(|(key, bank)| Some((key.clone(), bank.projection.clone()?)))
            .collect();
        for (key, projection) in novel {
            let mut projection = prepare_motion_projection(host, view, runtime, projection)?;
            let layer = projection.layer_key();
            super::fes_controls::apply_layer_clip(host.data_services, view.id, &layer, &mut projection.options);
            host.data_services.fes_text.banks.get_mut(&key).unwrap().projection = Some(projection);
        }
    }
    Ok(())
}

fn render_bank(
    device: &mut DesktopMessageDevice, view: &FesTaskView, runtime: &FesRuntime,
    bank: &mut TextBank, name: &[u8], index: i32,
) -> Result<()> {
    let scale = device.time_scale;
    let parent = bank.parent_plane.as_ref().map(|name|
        runtime.planes.get(name).context("STRING parent PLANE does not exist")).transpose()?;
    let canvas_parent = is_canvas_parent(bank, runtime);
    let (parent_position, parent_clip) = parent_projection(parent, canvas_parent);
    let display_parent = parent.filter(|_| !canvas_parent);
    let vram = [
        bank.vram[0].wrapping_add(display_parent.map_or(0, |plane| plane.vram[0])),
        bank.vram[1].wrapping_add(display_parent.map_or(0, |plane| plane.vram[1])),
    ];
    let canvas = canvas_fields(bank.message.encoding());
    if word(&bank.configuration, canvas) != vram[0] as u32
        || word(&bank.configuration, canvas + 4) != vram[1] as u32
    {
        set(&mut bank.configuration, canvas, vram[0] as u32);
        set(&mut bank.configuration, canvas + 4, vram[1] as u32);
        bank.dirty = true;
    }
    // A new message needs its first configuration before user glyphs.
    // Existing messages receive source/mapping changes before reconfigure.
    if !bank.configured {
        bank.message.reconfigure(&bank.configuration, scale, device)?;
        bank.configured = true; bank.dirty = false;
    }
    if bank.user_source_dirty {
        bank.message.set_user_glyph_source(bank.user_glyph_source.clone())?;
        bank.user_source_dirty = false;
    }
    while let Some(&(character, frame)) = bank.user_mappings.get(bank.user_mapping_cursor) {
        bank.message.map_user_glyph(u32::from(character), u32::from(frame), device)?;
        bank.user_mapping_cursor += 1;
    }
    if bank.dirty {
        bank.message.reconfigure(&bank.configuration, scale, device)?;
        bank.dirty = false;
    }
    if bank.shift_dirty {
        bank.message.set_shift(bank.shift, device)?;
        bank.shift_dirty = false;
    }
    for index in 0..bank.clear_collections.len() {
        if bank.clear_collections[index] {
            bank.message.clear_break_characters(index as u32);
            bank.clear_collections[index] = false;
        }
    }
    if let Some((index, bytes)) = &bank.remove_characters {
        bank.message.remove_break_characters(*index, bytes)?;
        bank.remove_characters = None;
    }
    if let Some((index, bytes)) = &bank.add_characters {
        bank.message.add_break_characters(*index, bytes)?;
        bank.add_characters = None;
    }
    while let Some(request) = bank.pending.front().cloned() {
        match request {
            TextRequest::Clear => {
                bank.message.clear();
                // Submit the genuinely empty page before any new suffix
                // is appended; str alone must not reveal its first glyph.
                bank.message.prepare(scale, device)?;
                bank.message.advance_display(0, MessageCopy::Full, device)?;
            }
            TextRequest::Append(bytes) => bank.message.append(&bytes)?,
            TextRequest::Separator => bank.message.append_separator()?,
        }
        bank.pending.pop_front();
    }
    if bank.needs_prepare {
        bank.message.prepare(scale, device)?;
        bank.line_count = bank.message.layout().map_or(0, |layout| layout.line_count);
        bank.needs_prepare = false;
    }
    let visible = bank.visible && display_parent.is_none_or(|plane| plane.visible);
    let display = if view.list.is_none() { MessagePlaneDisplay::Unreachable }
        else if visible { MessagePlaneDisplay::Visible } else { MessagePlaneDisplay::Hidden };
    if let Some(segment) = bank.draw {
        let continuing_novel_segment = bank.novel.is_some()
            && matches!(display, MessagePlaneDisplay::Visible)
            && bank.message.active() && bank.message.segment_limit() == segment;
        if !continuing_novel_segment {
            bank.message.start(segment, display, view.now, scale, device)?;
        }
        bank.draw = None;
    }
    if bank.skip {
        bank.message.force_complete(display, scale, device)?;
        bank.skip = false;
    }
    if bank.last_frame != Some(view.now) {
        bank.message.frame_update(view.now, scale, device)?;
        bank.last_frame = Some(view.now);
    }
    device.sync_object(&bank.message)?;
    bank.remaining = bank.message.remaining_all()?;
    bank.remaining_segment = bank.message.remaining_segment()?;
    if let Some(layout) = bank.message.layout() {
        bank.cursor = layout.cursor; bank.draw_area = layout.cursor;
    }
    let Some(surface) = device.surface(bank.message.id()) else {
        bank.projection = None; return Ok(());
    };
    let texture_size = [surface.submitted.width(), surface.submitted.height()];
    let (registered_size, _) = bank.message.registered_geometry()
        .context("configured STRING has no registered geometry")?;
    // The backend surface is local to this text plane. FES samples that
    // plane at its VRAM origin; the cache coordinates must not also move
    // the resulting screen projection (history uses an offscreen atlas).
    let mut pixels = surface.submitted.pixels().to_vec();
    if bank.tint != u32::MAX {
        let channels = [(bank.tint >> 16) as u8, (bank.tint >> 8) as u8, bank.tint as u8, (bank.tint >> 24) as u8];
        for pixel in pixels.chunks_exact_mut(4) {
            for index in 0..4 { pixel[index] = ((u16::from(pixel[index]) * u16::from(channels[index]) + 127) / 255) as u8; }
        }
    }
    let frame = RgbaFrame::from_rgba8(surface.submitted.width(), surface.submitted.height(), pixels)?;
    let mut options = ImageDrawOptions::new(Point::new(
        bank.position[0].wrapping_sub(bank.origin[0]).wrapping_add(parent_position.x),
        bank.position[1].wrapping_sub(bank.origin[1]).wrapping_add(parent_position.y),
    ));
    options.visible = visible && view.paused == 0 && !texture_size.contains(&0);
    if bank.novel.is_some() && view.list.is_none() {
        options.visible = false;
    }
    options.opacity = ((u16::from(bank.alpha)
        * u16::from(display_parent.map_or(255, |plane| plane.opacity))) / 255) as u8;
    options.scale_x = bank.scale[0] * registered_size[0] as f32 / texture_size[0].max(1) as f32;
    options.scale_y = bank.scale[1] * registered_size[1] as f32 / texture_size[1].max(1) as f32;
    options.rotation_radians = bank.rotation;
    options.clip = if let Some(parent_clip) = parent_clip {
        Some(match bank.clip {
            Some(clip) => intersect_clip(clip, parent_clip),
            None => parent_clip,
        })
    } else { bank.clip };
    bank.projection = Some(FesTextProjection {
        owner: view.id, name: name.to_vec(), bank: index, frame, options, priority: bank.priority,
    });

    Ok(())
}

fn uses_canvas_atlas(bank: &TextBank) -> bool {
    bank.novel.is_none() && bank.log.is_none()
}

fn is_canvas_parent(bank: &TextBank, runtime: &FesRuntime) -> bool {
    uses_canvas_atlas(bank)
        && bank.parent_plane.as_ref().is_some_and(|name| !runtime.media.contains_key(name))
}

fn resolve_canvas_parent(
    raw: &[u8], encoding: ScriptTextEncoding, driver: &mut FesScriptDriver,
    shared: &mut ExpressionState,
) -> Result<Option<(Option<String>, Vec<u8>)>> {
    let (name, index) = split_target(raw)?;
    let Some(plane) = driver.adapter.object(name)
        .filter(|object| object.definition.kind_name == b"PLANE") else { return Ok(None); };
    let count = plane.definition.bank_count;
    let bank = index.map(|value| driver.evaluate_parameter(value, 0, shared))
        .transpose()?.unwrap_or(0);
    let mut normalized = name.to_vec();
    if index.is_some() {
        normalized.extend_from_slice(format!("[{bank}]").as_bytes());
    }
    let key = (0..count as i32).contains(&bank).then(|| {
        if count == 1 {
            encoding.decode(name)
        } else {
            format!("{}[{bank}]", encoding.decode(name))
        }
    });
    Ok(Some((key, normalized)))
}

fn parent_projection(
    parent: Option<&super::FesPlaneLayer>, canvas_parent: bool,
) -> (Point, Option<Rect>) {
    let Some(plane) = parent else { return (Point::new(0, 0), None); };
    let position = if canvas_parent {
        Point::new(plane.vram[0], plane.vram[1])
    } else {
        plane.position
    };
    (position, Some(Rect::new(position.x, position.y, plane.size[0], plane.size[1])))
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) {
    let data = &mut *host.data_services;
    data.fes_text.release_owner(owner, &mut data.message_device);
}

/// Validate the dedicated owner before any node in a mixed plan is removed.
/// Enumerate allocated history rows, including rows awaiting a COUNT change.
pub(super) fn bank_release_layers(
    store: &FesTextStore, owner: u32, name: &[u8], index: i32, kind: &[u8],
) -> Result<Vec<String>> {
    if !is_text_kind(kind) { bail!("FES text release needs a STRING/LOGSTRING declaration"); }
    let bank = store.banks.get(&(owner, name.to_vec(), index))
        .context("FES text delete has no live text bank owner")?;
    if bank.log.is_some() != is_log(kind) || bank.novel.is_some() != is_novel(kind)
        || is_unicode(kind) && bank.message.encoding() != MessageEncoding::Unicode
    { bail!("FES text delete owner differs from its original declaration"); }
    let mut layers = vec![layer_key(name, index)];
    if let Some(log) = &bank.log {
        for row in 0..log.rows.len() {
            layers.push(layer_key(&log::row_name(name, row), index));
        }
    }
    Ok(layers)
}

/// The shared postorder consumer removes planned list layers before calling
/// this destructor, and commits motion/adapter identity only after it returns.
pub(super) fn release_bank(
    host: &mut LoggingHost<'_>, owner: u32, name: &[u8], index: i32,
) -> Result<()> {
    let data = &mut *host.data_services;
    data.fes_text.release_bank(&(owner, name.to_vec(), index), &mut data.message_device);
    Ok(())
}

pub(super) fn projections(data: &super::data_services::DataServices, owner: u32) -> Vec<FesTextProjection> {
    let mut result = Vec::new();
    for ((id, name, index), bank) in &data.fes_text.banks {
        if *id != owner || bank.novel.is_some() { continue; }
        // The reference belongs to the original FES bank. History row names
        // identify independent submitted surfaces, not new viewport owners.
        // Start from the saved, unclipped projection each time so a moved or
        // cleared viewport can expand again without repainting the message.
        let key = layer_key(name, *index);
        let mut submit = |projection: &Option<FesTextProjection>| {
            if let Some(mut projection) = projection.clone() {
                super::fes_controls::apply_layer_clip(data, owner, &key, &mut projection.options);
                result.push(projection);
            }
        };
        if let Some(log) = &bank.log {
            for row in &log.rows { submit(&row.projection); }
        } else { submit(&bank.projection); }
    }
    result.sort_by_key(|projection| (projection.priority, projection.name.clone(), projection.bank));
    result
}

pub(super) fn novel_projections(
    data: &super::data_services::DataServices, lower: f32, upper: f32,
) -> Vec<FesTextProjection> {
    let mut projections: Vec<_> = data.fes_text.banks.values()
        .filter(|bank| bank.novel.is_some())
        .filter_map(|bank| bank.projection.as_ref())
        .filter(|projection| {
            let depth = projection.priority as f32;
            projection.options.visible && lower <= depth && depth < upper
        })
        .cloned()
        .collect();
    projections.sort_by_key(|projection| projection.priority);
    projections
}

/// Refreshes only inherited fields after MES_CHANGECONFIG. Existing messages,
/// accumulated text and wait caches remain intact until the next media update.
pub(super) fn refresh_defaults(
    data: &mut super::data_services::DataServices, owner: u32, defaults: &MessageDefaults,
) -> Result<()> {
    data.fes_text.refresh_font_defaults(owner, defaults)
}

fn defaults() -> MessageDefaults {
    MessageDefaults { font_name: Vec::new(), bold: false, shadow: 1, outline: 1,
        glyph_parameter: Some(0), gradual: false }
}
fn initial_configuration(encoding: MessageEncoding, defaults: &MessageDefaults, measure: u32) -> Result<Vec<u8>> {
    let mut bytes = defaults.configuration(encoding)?.bytes().to_vec();
    for offset in [0x10, 0x14, 0x18] { set(&mut bytes, offset, 0); }
    set(&mut bytes, 0x80, measure);
    if encoding == MessageEncoding::Unicode { set(&mut bytes, tail(encoding) + 12, 0); }
    let canvas = canvas_fields(encoding);
    set(&mut bytes, canvas + 8, 1);
    Ok(bytes)
}
fn tail(encoding: MessageEncoding) -> usize {
    if encoding == MessageEncoding::Cp932 { 0xc4 } else { 0x104 }
}
fn canvas_fields(encoding: MessageEncoding) -> usize {
    if encoding == MessageEncoding::Cp932 { 0xdc } else { 0x120 }
}
fn put_font(bytes: &mut [u8], encoding: MessageEncoding, font: &[u16]) -> Result<()> {
    let font = &font[..font.iter().position(|&unit| unit == 0).unwrap_or(font.len())];
    let end = tail(encoding);
    if font.len() >= (end - 0x84) / 2 { bail!("STRING font name exceeds UTF-16 capacity"); }
    bytes[0x84..end].fill(0);
    for (index, unit) in font.iter().enumerate() { bytes[0x84 + index * 2..0x86 + index * 2].copy_from_slice(&unit.to_le_bytes()); }
    Ok(())
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn set(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn pair(values: &[i32], defaults: [i32; 2]) -> [i32; 2] {
    [values.first().copied().unwrap_or(defaults[0]), values.get(1).copied().unwrap_or(defaults[1])]
}
fn write_shared(shared: &mut ExpressionState, index: i32, value: i32) {
    if index >= 0 { if let Some(slot) = shared.shared.get_mut(index as usize) { *slot = value; } }
}
fn optional_number(args: &[Vec<u8>], index: usize, default: i32, driver: &mut FesScriptDriver, shared: &mut ExpressionState) -> Result<i32> {
    match args.get(index) { Some(value) => driver.evaluate_parameter(value, default, shared), None => Ok(default) }
}
fn numeric_position_parameter(source: &[u8]) -> bool {
    matches!(source.first(), Some(b'0'..=b'9' | b'+' | b'-' | b'#' | b'$' | b'(' | b'@'))
        && Expression::parse(source).is_ok()
}
fn property_number(args: &[Vec<u8>], index: usize, current: i32, driver: &mut FesScriptDriver, shared: &mut ExpressionState) -> Result<i32> {
    optional_number(args, index, current, driver, shared)
}
fn split_target(target: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    let Some(open) = target.iter().position(|&byte| byte == b'[') else { return Ok((target, None)) };
    if target.last() != Some(&b']') { bail!("STRING target bank is not closed"); }
    Ok((&target[..open], Some(&target[open + 1..target.len() - 1])))
}
fn raw_after_spaces(source: &[u8], count: usize) -> Option<&[u8]> {
    let mut cursor = 0;
    for _ in 0..count {
        cursor += source.get(cursor..)?.iter().position(|&byte| byte == b' ')? + 1;
    }
    source.get(cursor..).map(|bytes| &bytes[..bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len())])
}
fn unicode_layout_font(mut raw: &[u8]) -> Result<Vec<u16>> {
    let mut font = Vec::new();
    while !raw.is_empty() {
        if raw.starts_with(&[0xff, 0xff]) {
            font.push(0xffff);
            raw = &raw[2..];
            continue;
        }
        // The original wide-font entry stops on a continuation byte in lead
        // position. A nonempty source can therefore explicitly clear the name.
        let length = super::utf8_byte_width(raw, 0);
        if length <= 0 { break; }
        let length = length as usize;
        let character = raw.get(..length).context("STRING layout font has a truncated UTF-8 unit")?;
        // The flags=0 converter retains only the first UTF-16 unit.
        let (text, _) = encoding_rs::UTF_8.decode_without_bom_handling(character);
        if let Some(unit) = text.encode_utf16().next() { font.push(unit); }
        raw = &raw[length..];
    }
    Ok(font)
}
fn normalize_text(raw: &[u8], encoding: MessageEncoding, sjis: bool, resource: ScriptTextEncoding) -> Result<Vec<u8>> {
    if sjis {
        let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(raw);
        if errors { bail!("STRING strsj input is invalid CP932"); }
        return Ok(text.as_bytes().to_vec());
    }
    if encoding == MessageEncoding::Unicode {
        if std::str::from_utf8(raw).is_ok() { return Ok(raw.to_vec()); }
        if matches!(resource, ScriptTextEncoding::Cp932) {
            bail!("STRING8 source is not valid UTF-8");
        }
        return match resource {
            ScriptTextEncoding::Gbk => {
                let (text, _, errors) = encoding_rs::GBK.decode(raw);
                if errors { bail!("STRING8 text has invalid GBK resource bytes"); }
                Ok(text.as_bytes().to_vec())
            }
            _ => bail!("STRING8 text is not valid UTF-8"),
        };
    }
    Ok(raw.to_vec())
}

fn intersect_clip(left: Rect, right: Rect) -> Rect {
    let x = i64::from(left.x).max(i64::from(right.x));
    let y = i64::from(left.y).max(i64::from(right.y));
    let end_x = (i64::from(left.x) + i64::from(left.width))
        .min(i64::from(right.x) + i64::from(right.width));
    let end_y = (i64::from(left.y) + i64::from(left.height))
        .min(i64::from(right.y) + i64::from(right.height));
    Rect::new(x as i32, y as i32, (end_x - x).max(0) as u32, (end_y - y).max(0) as u32)
}

fn accumulate_characters(
    pending: &mut Option<(u32, Vec<u8>)>, index: u32, characters: &[u8],
) -> Result<()> {
    let old_length = pending.as_ref().map_or(0, |(_, bytes)| bytes.len());
    if old_length.checked_add(characters.len()).is_none_or(|length| length > MAX_TEXT_BYTES) {
        bail!("STRING pending character collection exceeds 64 MiB");
    }
    let (old_index, bytes) = pending.get_or_insert_with(|| (index, Vec::new()));
    *old_index = index;
    bytes.extend_from_slice(characters);
    Ok(())
}
