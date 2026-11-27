//! FES text planes over the real CPU MessageObject backend.
//! Storage is independent of the image platform's private bindings. Mutating
//! text commands queue work; update_owner commits it and captures query caches.
//! Shift, break-character edits and user-glyph mappings use the same real
//! backend before text/playback updates. LOGSTRING has its own history state.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, ImageDrawOptions, Point, Rect, RgbaFrame};
use cs2_runtime::message_object::{
    MessageCopy, MessageDefaults, MessageEncoding, MessageObject,
    MessagePlaneDisplay,
};
use cs2_script::{Expression, ExpressionState, FesEvent, FesObjectDefinition, FesObjectInitialization, FesProgram};

use super::cst_context_native::{CstBodySource, CstSourceProbe, CstTextOrigins, body_control_only, join_body_sources};
use super::fes_driver::{
    FesDriverCommand, FesDriverEffect, FesDriverWait, FesScriptDriver,
};
use super::fes_tasks::FesTaskView;
use super::perf_detail::{SlowTask, TextHostTimer, TextProjection, TextStage, TextTimer, TextWork};
use super::story_trace::{story_trace_10 as djibril10, story_trace_12 as hatsumira, story_trace_15 as labyrinth15};
use super::{FesRuntime, HeadlessPlayer, LoggingHost, LogicEvent, ScriptTextEncoding, message_backend::DesktopMessageDevice};

const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;
const MAX_BANKS: usize = 65536;

#[path = "fes_log.rs"]
mod log;
pub(super) use log::is_island_gbk_scene;
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
    AppendLegacyGbk(Vec<u8>),
    AppendDecoded { bytes: Vec<u8>, text: String },
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
    #[serde(skip)]
    story_body: StoryBodyProgress,
}

#[derive(Clone, Debug, Default)]
struct StoryBodyProgress {
    source: Option<(u64, u64)>,
    provenance: Option<Vec<CstBodySource>>,
    natural_complete: Option<(u64, u64)>,
    eden_layout_skip_pending: bool,
    yukikoi_layout_skip_pending: bool,
    eden_layout_surface: Option<u64>,
    eden_layout_complete: Option<(u64, u64)>,
    committed_epoch: Option<u64>,
    novel_composed: Option<(u64, u64)>,
    forced: bool,
}

impl StoryBodyProgress {
    fn note_source(&mut self, frame: u64) {
        let epoch = self.source.map_or(1, |(epoch, _)| epoch.wrapping_add(1));
        *self = Self { source: Some((epoch, frame)), ..Self::default() };
    }

    fn note_text(&mut self, frame: u64, keep_prefix: bool, suffix: Option<&[CstBodySource]>) {
        let prefix = self.provenance.take();
        self.note_source(frame);
        self.provenance = suffix.and_then(|suffix| {
            let prefix = if keep_prefix { prefix.as_deref()? } else { &[] };
            join_body_sources(prefix, suffix)
        });
    }

    fn force(&mut self) {
        self.forced = true;
        self.natural_complete = None;
        self.eden_layout_surface = None;
        self.eden_layout_complete = None;
        self.committed_epoch = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StoryBodyGeneration {
    pub(super) source_epoch: u64,
    pub(super) source_frame: u64,
    pub(super) natural_complete: Option<(u64, u64)>,
    pub(super) committed_epoch: Option<u64>,
    pub(super) remaining: u32,
    pub(super) remaining_segment: u32,
    pub(super) pending_text: bool,
    pub(super) drawing: bool,
    pub(super) skip_pending: bool,
    pub(super) forced: bool,
    pub(super) projection: Option<(bool, u8)>,
}

#[derive(Clone, Debug, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct NovelText {
    division: i32,
    insert_pending: bool,
    speaker: Vec<u8>,
    #[serde(default)]
    preceding_markers: u32,
}

impl NovelText {
    fn set_decoded_speaker(&mut self, bytes: &[u8], text: &str, encoding: MessageEncoding) {
        if encoding == MessageEncoding::Unicode {
            self.speaker = text.as_bytes().to_vec();
        } else {
            self.speaker.clear();
            self.speaker.push(0);
            self.speaker.extend_from_slice(bytes);
            self.speaker.push(0);
            self.speaker.extend_from_slice(text.as_bytes());
        }
    }

    fn speaker_parts(&self) -> Result<(&[u8], Option<&str>)> {
        let Some(tagged) = self.speaker.strip_prefix(&[0]) else { return Ok((&self.speaker, None)); };
        let end = tagged.iter().position(|&byte| byte == 0).context("missing decoded speaker delimiter")?;
        Ok((&tagged[..end], Some(std::str::from_utf8(&tagged[end + 1..])?)))
    }
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
    pub(super) fn draw(&self, canvas: &mut Canvas) -> cs2_renderer::Result<()> {
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        if super::gpu_raster::draw_image(canvas, &self.frame, &self.options) {
            return Ok(());
        }
        canvas.try_pixels()?;
        canvas.draw_frame(&self.frame, &self.options)
    }

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

fn uses_paragraph_local_draw(program: &FesProgram, command: &FesDriverCommand) -> bool {
    if command.context.block != b"MES_INSERT" || command.source != b"str insert" {
        return false;
    }
    [
        (b"MES_SETMARK".as_slice(), b"str apendmark".as_slice()),
        (b"MES_DRAW".as_slice(), br"str draw \0".as_slice()),
        (b"MES_ISDRAW".as_slice(), b"str isdraw".as_slice()),
    ].iter().all(|(block, statement)| {
        program.block(block).is_some_and(|block|
            block.lines.iter().any(|line| line.bytes.trim_ascii() == *statement))
    })
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
    pub(super) fn trace_visual_progress(&self) -> (usize, usize) {
        use cs2_runtime::message_object::{MessageAtomKind, MessageGlyphState};
        let mut seen = 0;
        let mut kept = 0;
        // Prefer story/revealing banks over unrelated visible menu labels.
        for tier in 0..3 {
        for ((owner, name, index), bank) in &self.banks {
            let visible = bank.projection.as_ref().is_some_and(|projection|
                projection.options.visible && projection.options.opacity != 0);
            if bank.log.is_some() || !(visible || bank.message.active() || bank.story_body.source.is_some()) { continue; }
            let live = visible || bank.message.active();
            let priority = if live && (bank.story_body.source.is_some() || bank.message.active()) { 0 }
                else if live { 1 } else { 2 };
            if priority != tier { continue; }
            seen += 1;
            if kept == 2 { continue; }
            let mut counts = [0usize; 3];
            if let Some(layout) = bank.message.layout() {
                for (entry, state) in layout.entries.iter().zip(bank.message.glyph_states()) {
                    if !entry.atom.and_then(|atom| bank.message.atoms().get(atom))
                        .is_some_and(|atom| matches!(&atom.kind, MessageAtomKind::Glyph { .. })) { continue; }
                    counts[match state { MessageGlyphState::Hidden => 0, MessageGlyphState::Revealing => 1,
                        MessageGlyphState::Complete => 2 }] += 1;
                }
            }
            let state = bank.message.render_state();
            super::perf_detail::visual_note("visual_text", format!(
                "task={owner} bank={index} name={} text_now={} playback_now={} source_epoch={} glyph_total={} hidden={} revealing={} complete={} remaining={} segment_remaining={} active={} visible={} prepared={} pending={} surface_generation={}",
                super::visual_progress::token(name), super::visual_progress::optional(bank.last_frame), state.playback_time,
                bank.story_body.source.map_or(0, |source| source.0), counts.iter().sum::<usize>(), counts[0], counts[1], counts[2],
                bank.remaining, bank.remaining_segment, u8::from(bank.message.active()), u8::from(visible),
                u8::from(bank.message.prepared()), bank.pending.len(), bank.message.surface_generation()));
            kept += 1;
        }
        }
        (seen, kept)
    }

    pub(super) fn story_body_generation(&self, owner: u32) -> Option<StoryBodyGeneration> {
        let bank = self.banks.get(&(owner, b"str".to_vec(), 0))?;
        let (source_epoch, source_frame) = bank.story_body.source?;
        Some(StoryBodyGeneration {
            source_epoch,
            source_frame,
            natural_complete: bank.story_body.natural_complete,
            committed_epoch: bank.story_body.committed_epoch,
            remaining: bank.remaining,
            remaining_segment: bank.remaining_segment,
            pending_text: !bank.pending.is_empty() || bank.needs_prepare,
            drawing: bank.draw.is_some() || bank.message.active(),
            skip_pending: bank.skip,
            forced: bank.story_body.forced,
            projection: bank.projection.as_ref().map(|projection|
                (projection.options.visible, projection.options.opacity)),
        })
    }

    pub(super) fn eden_layout_complete(&self, owner: u32) -> Option<(u64, u64)> {
        let bank = self.banks.get(&(owner, b"str".to_vec(), 0))?;
        if bank.story_body.eden_layout_surface != Some(bank.message.surface_generation()) {
            return None;
        }
        bank.story_body.eden_layout_complete
    }

    pub(super) fn story_body_sources(&self, owner: u32, source_epoch: u64, origins: &CstTextOrigins)
        -> Option<&[CstBodySource]>
    {
        let bank = self.banks.get(&(owner, b"str".to_vec(), 0))?;
        let (epoch, _) = bank.story_body.source?;
        let sources = bank.story_body.provenance.as_deref()?;
        (epoch == source_epoch && origins.body_is_current(sources)).then_some(sources)
    }

    pub(super) fn story_body_appends_match_gbk(&self, owner: u32, epoch: u64, raw: &[u8]) -> bool {
        let Some(bank) = self.banks.get(&(owner, b"str".to_vec(), 0)) else { return false; };
        if bank.story_body.source.is_none_or(|(current, _)| current != epoch) {
            return false;
        }
        let decoded;
        let expected = if bank.message.encoding() == MessageEncoding::Unicode {
            let Ok(text) = decode_text(raw, ScriptTextEncoding::Gbk) else { return false; };
            decoded = text;
            decoded.as_bytes()
        } else {
            raw
        };
        // OP's sscript sends the default color as a separate MES_SETSTRING.
        // FES apendmark uses append_separator and stays out of raw_appends.
        let mut appends = bank.message.raw_appends().iter().filter(|append| !append.is_empty()).peekable();
        if appends.peek().is_some_and(|append| append.as_slice() == b"\\cd") {
            appends.next();
        }
        appends.flatten().copied().eq(expected.iter().copied())
    }

    pub(super) fn story_body_append_diagnostics(&self, owner: u32) -> String {
        let Some(bank) = self.banks.get(&(owner, b"str".to_vec(), 0)) else {
            return "bank missing".into();
        };
        let appends = bank.message.raw_appends();
        let sizes: Vec<_> = appends.iter().take(12).map(Vec::len).collect();
        let color_prefix = appends.iter().find(|append| !append.is_empty())
            .is_some_and(|append| append.as_slice() == b"\\cd");
        format!("encoding={:?} queued_bytes={} append_count={} append_sizes={sizes:?} color_prefix={color_prefix}",
            bank.message.encoding(), bank.source.len(), appends.len())
    }

    pub(super) fn story_novel_segment_ready(&self, owner: u32) -> bool {
        let Some(bank) = self.banks.get(&(owner, b"str".to_vec(), 0)) else {
            return false;
        };
        bank.novel.is_some()
            && !bank.source.is_empty()
            && bank.pending.is_empty()
            && bank.draw.is_none()
            && !bank.needs_prepare
            && bank.remaining_segment == 0
            && bank.projection.as_ref().is_some_and(|projection|
                projection.options.visible && projection.options.opacity > 0)
    }

    pub(super) fn note_rebellions_novel_composed(&mut self, layer: &FesTextProjection, frame: u64) {
        if layer.name.as_slice() != b"str" || layer.bank != 0 || layer.options.opacity == 0 {
            return;
        }
        let Some(bank) = self.banks.get_mut(&(layer.owner, b"str".to_vec(), 0)) else { return; };
        let progress = &mut bank.story_body;
        if let Some(epoch) = progress.committed_epoch
            && bank.novel.is_some() && !progress.forced
        {
            progress.novel_composed = Some((epoch, frame));
        }
    }

    pub(super) fn rebellions_novel_composed(&self, owner: u32, epoch: u64, frame: u64) -> bool {
        self.banks.get(&(owner, b"str".to_vec(), 0)).is_some_and(|bank| {
            bank.novel.is_some() && bank.story_body.novel_composed == Some((epoch, frame))
        })
    }

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
            let previous = bank.configuration.clone();
            let tail = tail(bank.message.encoding());
            if bank.inherited[0] { set(&mut bank.configuration, tail + 12, 0); }
            if bank.inherited[1] { put_font(&mut bank.configuration, bank.message.encoding(), &defaults.font_name)?; }
            if bank.inherited[2] { set(&mut bank.configuration, tail, if defaults.bold { 700 } else { 400 }); }
            if bank.inherited[3] {
                set(&mut bank.configuration, tail + 4, defaults.shadow);
                set(&mut bank.configuration, tail + 8, defaults.outline);
            }
            bank.dirty |= bank.configuration != previous;
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
                story_body: StoryBodyProgress::default(),
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
                    let unicode = bank.message.encoding() == MessageEncoding::Unicode;
                    let codec = if !unicode && host.data_services.has_loose_gbk_text() {
                        ScriptTextEncoding::Gbk
                    } else if unicode && encoding == ScriptTextEncoding::Cp932 && std::str::from_utf8(raw).is_ok() {
                        ScriptTextEncoding::Utf8
                    } else { encoding };
                    let (text, bad_at) = codec.decode_message(raw);
                    if let Some(bad_at) = bad_at {
                        host.data_services.note_text_decode(host.events, host.frame, format!(
                            "FES initializer offset={:#x} bank={:?} codec={codec:?} bad_at={bad_at}",
                            initializer.offset, String::from_utf8_lossy(name),
                        ));
                    }
                    bank.replace_text(if unicode { text.as_bytes() } else { raw }, false, Some(&text))?;
                    bank.observe_story_source(key, host);
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
            if verb == b"lineinfo" && host.data_services.story_trace {
                let slot = optional_number(args, 1, 0, driver, shared)? as u32;
                let source = shared.strings.get(&slot).map(|value| String::from_utf8_lossy(value));
                host.events.push(crate::LogicEvent {
                    frame: host.frame,
                    kind: "backlog-lineinfo".into(),
                    detail: format!("slot={slot} source={source:?}"),
                });
            }
            return Ok(true);
        }
        if self.banks[&keys[0]].log.is_some()
            && !matches!(verb, b"layout" | b"reset" | b"userfontobj" | b"userfont"
                | b"pri" | b"alpha" | b"color" | b"pos2" | b"org" | b"vram"
                | b"size" | b"size2" | b"scale" | b"arc" | b"clip")
        {
            bail!("LOGSTRING command {:?} has no connected history receiver", String::from_utf8_lossy(verb));
        }
        let yukikoi_before = (host.data_services.story_trace
            && matches!(verb, b"str" | b"strsj" | b"apend" | b"apendmark" | b"clear"
                | b"reset" | b"draw" | b"skip")
            && host.css_task_sha() == Some("63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
            && host.cst_contexts.values().any(|cst| cst.story.is_scene("__tar01")))
            .then(|| keys.iter().find_map(|key| {
                let record = &self.banks[key];
                record.is_story_body(key).then(|| record.story_body.clone())
            }))
            .flatten();
        let mut yukikoi_input = None;
        match verb {
            b"layout" => {
                let hatsumira_font_layout = command.context.block == b"STRLAYOUT"
                    && matches!(command.parameters.get(2).map(Vec::as_slice), Some(b"font" | b"fontind"))
                    && host.css_task_sha() == Some(hatsumira::PE);
                // The window can rerun STRLAYOUT while $5 still selects
                // file fonts. In this script, "def" restores the default.
                let reset_file_font = hatsumira_font_layout
                    && command.offset == 0x4dd8
                    && command.source == b"str layout fontind 1 def";
                if hatsumira_font_layout && host.data_services.story_trace {
                    host.events.push(LogicEvent {
                        frame: host.frame,
                        kind: "hatsumira-font-layout".into(),
                        detail: format!(
                            "owner={owner} offset={:#x} comparison={:?} command={:?} reset_file_font={reset_file_font}",
                            command.offset, shared.shared.get(5), String::from_utf8_lossy(&command.source),
                        ),
                    });
                }
                self.layout(&keys, command, driver, shared, defaults, resource_encoding, host, reset_file_font)?;
            }
            b"insert" if self.banks[&keys[0]].novel.is_some() => {
                let paragraph_local_draw = driver.is_rebellions_message_fes_source()
                    || uses_paragraph_local_draw(&driver.interpreter.program, command);
                for key in &keys {
                    self.banks.get_mut(key).unwrap()
                        .insert_novel_paragraph(paragraph_local_draw);
                }
            }
            b"name" => {
                if keys.iter().all(|key| self.banks[key].novel.is_none()) { return Ok(true); }
                let raw = raw_after_spaces(&command.source, 2).unwrap_or_default();
                let decoded = sourced_text(owner, command, verb, raw, driver, shared, host)?;
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    record.set_speaker(raw, resource_encoding, decoded.as_ref().map(|value| value.text.as_str()))?;
                }
            }
            b"str" | b"strsj" | b"apend" => {
                let raw = raw_after_spaces(&command.source, 2).unwrap_or_default();
                let decoded = sourced_text(owner, command, verb, raw, driver, shared, host)?;
                let hatsumira_setstring = host.data_services.story_trace
                    && host.css_task_sha() == Some(super::story_trace::story_trace_12::PE)
                    && verb == b"apend"
                    && command.object_event.is_none()
                    && command.context.block.as_slice() == b"MES_SETSTRING"
                    && fes_text_statement(driver, command.offset).is_some_and(|(block, template)|
                        block == b"MES_SETSTRING" && template == b"$str1000");
                // sscript publishes the standalone default-color control before
                // publishing the body. It contributes no CST record to the bank.
                let hatsumira_color_reset = hatsumira_setstring && raw == b"\\cd"
                    && shared.strings.get(&1000).is_some_and(|value| value.as_slice() == raw);
                // Eden also publishes the color-default control before the scene body.
                let eden_color_reset = host.data_services.story_trace && verb == b"apend"
                    && raw == b"\\cd" && command.offset == 0x640f
                    && command.object_event.is_none()
                    && command.context.block.as_slice() == b"MES_SETSTRING"
                    && shared.strings.get(&1000).is_some_and(|value| value.as_slice() == raw)
                    && eden_diagnostic_fes(host, driver) == Some(true);
                let labyrinth_color_reset = host.data_services.story_trace && verb == b"apend"
                    && raw == b"\\cd" && command.offset == 0x55c3
                    && command.object_event.is_none()
                    && command.context.block.as_slice() == b"MES_SETSTRING"
                    && shared.strings.get(&1000).is_some_and(|value| value.as_slice() == raw)
                    && host.css_task_sha() == Some(labyrinth15::PE)
                    && host.cst_contexts.values().any(labyrinth15::opening_scene)
                    && labyrinth15::verified_meswnd(host.resources, &driver.interpreter.program).is_some();
                let yukikoi_color_reset = yukikoi_before.is_some() && verb == b"apend"
                    && command.offset == 0x73cf && raw == b"\\cd" && command.object_event.is_none()
                    && fes_text_statement(driver, command.offset)
                        .is_some_and(|(block, template)|
                        block == b"MES_SETSTRING" && template == b"$str1000")
                    && shared.strings.get(&1000).is_some_and(|value| value.as_slice() == raw);
                let djibril_color_reset = host.data_services.story_trace && verb == b"apend"
                    && raw == b"\\cd" && command.offset == 0x5b1f && command.object_event.is_none()
                    && fes_text_statement(driver, command.offset).is_some_and(|(block, template)|
                        block == b"MES_SETSTRING" && template == b"$str1000")
                    && shared.strings.get(&1000).is_some_and(|value| value.as_slice() == raw)
                    && host.css_task_sha() == Some(djibril10::PE)
                    && host.cst_contexts.values().any(djibril10::opening_scene)
                    && djibril10::verified_meswnd(host.resources, &driver.interpreter.program);
                // Track the live segment without changing the accumulated NVSTRING.
                let rebellions_novel = host.data_services.story_trace && verb == b"apend"
                    && command.object_event.is_none()
                    && command.context.block.as_slice() == b"MES_SETSTRING"
                    && fes_text_statement(driver, command.offset)
                        .is_some_and(|(block, template)| block == b"MES_SETSTRING" && template == b"$str1000")
                    && host.data_services.cst_text_origins.shared_body(1000, raw).is_some_and(|sources| {
                        let [source] = sources else { return false; };
                        host.data_services.cst_text_origins.source_record_observations(source.context_handle)
                            .iter().any(|read| read.probe == CstSourceProbe::Rebellions && read.source == *source
                                && read.native == 327 && read.kind == 0x20 && read.body_tagged)
                    })
                    && HeadlessPlayer::verified_rebellions_novel_fes(host.resources, &driver.interpreter.program).is_some();
                let provenance = (host.data_services.story_trace && verb != b"strsj"
                    && (rebellions_novel || keys.iter().any(|key| self.banks[key].is_story_body(key))))
                    .then(|| {
                        if hatsumira_color_reset || eden_color_reset || labyrinth_color_reset || yukikoi_color_reset
                            || djibril_color_reset
                        {
                            Some(Vec::new())
                        } else {
                            story_command_sources(raw, command, driver, shared, &host.data_services.cst_text_origins)
                        }
                    })
                    .flatten();
                if yukikoi_before.is_some() {
                    let statement = fes_text_statement(driver, command.offset);
                    yukikoi_input = Some((raw.len(), body_control_only(raw), decoded.is_some(),
                        statement.is_some(), provenance.clone()));
                }
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    let keep_prefix = verb == b"apend" && !record.source.is_empty();
                    if host.data_services.story_trace && record.novel.is_some() && key.1.as_slice() == b"str" && key.2 == 0
                        && (rebellions_novel || record.story_body.source.is_some())
                    {
                        let current = provenance.as_deref().filter(|sources| rebellions_novel && sources.len() == 1);
                        record.story_body.note_text(host.frame, false, current);
                    }
                    if djibril_color_reset && record.is_story_body(key) {
                        host.events.push(LogicEvent {
                            frame: host.frame, kind: "djibril10-color-reset".into(),
                            detail: format!("owner={owner} offset=0x5b1f prefix_bytes={}", record.source.len()),
                        });
                    }
                    if eden_color_reset && record.is_story_body(key) {
                        host.events.push(LogicEvent {
                            frame: host.frame,
                            kind: "fes-text-eden-color-reset".into(),
                            detail: format!("owner={owner} offset=0x640f prefix_bytes={}", record.source.len()),
                        });
                    }
                    if labyrinth_color_reset && record.is_story_body(key) {
                        host.events.push(LogicEvent {
                            frame: host.frame,
                            kind: "labyrinth15-color-reset".into(),
                            detail: format!("owner={owner} offset=0x55c3 prefix_bytes={}", record.source.len()),
                        });
                    }
                    if hatsumira_setstring && record.is_story_body(key) {
                        let shared_body = host.data_services.cst_text_origins.shared_body(1000, raw);
                        let prefix = record.story_body.provenance.as_deref();
                        host.events.push(LogicEvent {
                            frame: host.frame,
                            kind: "hatsumira-body-setstring".into(),
                            detail: format!("owner={owner} bytes={} prefix_bytes={} color_reset={hatsumira_color_reset} prefix={prefix:?} shared={shared_body:?} suffix={provenance:?}",
                                raw.len(), record.source.len()),
                        });
                    }
                    let legacy_gbk = host.data_services.has_loose_gbk_text()
                        && record.message.encoding() == MessageEncoding::Cp932;
                    if let Some(SourcedText { text, .. }) = &decoded {
                        let bytes = if record.message.encoding() == MessageEncoding::Unicode {
                            text.as_bytes()
                        } else { raw };
                        if verb == b"apend" {
                            record.append_body_text(bytes, legacy_gbk, Some(text))?;
                        } else {
                            record.replace_text(bytes, false, Some(text))?;
                        }
                        record.observe_story_text(key, host, keep_prefix, provenance.as_deref());
                        continue;
                    }
                    let explicit_cp932 = verb == b"strsj";
                    let unicode = record.message.encoding() == MessageEncoding::Unicode;
                    let codec = if explicit_cp932 { ScriptTextEncoding::Cp932 }
                        else if legacy_gbk { ScriptTextEncoding::Gbk }
                        else if unicode && resource_encoding == ScriptTextEncoding::Cp932
                            && std::str::from_utf8(raw).is_ok() { ScriptTextEncoding::Utf8 }
                        else { resource_encoding };
                    let (text, bad_at) = codec.decode_message(raw);
                    if let Some(bad_at) = bad_at {
                        host.data_services.note_text_decode(host.events, host.frame, format!(
                            "FES block={:?} offset={:#x} bank={:?} codec={codec:?} bad_at={bad_at}",
                            String::from_utf8_lossy(&command.context.block), command.offset,
                            String::from_utf8_lossy(&key.1),
                        ));
                    }
                    let bytes = if unicode { text.as_bytes() } else { raw };
                    if verb == b"apend" {
                        record.append_body_text(bytes, legacy_gbk, Some(&text))?;
                    } else {
                        record.replace_text(bytes, false, Some(&text))?;
                    }
                    record.observe_story_text(key, host, keep_prefix, provenance.as_deref());
                }
                if host.data_services.story_trace && let Some(decoded) = &decoded {
                    host.events.push(LogicEvent {
                        frame: host.frame,
                        kind: "fes-text-sourced-queued".into(),
                        detail: decoded.trace_detail(raw.len()),
                    });
                }
            }
            b"apendmark" => for key in &keys {
                let record = self.banks.get_mut(key).unwrap();
                if !record.source_initialized { bail!("STRING apendmark requires initialized text"); }
                let keep_prefix = !record.source.is_empty();
                let marker: &[u8] = if record.message.encoding() == MessageEncoding::Unicode {
                    b"\xef\xbf\xbf"
                } else { b"\xff" };
                record.check_append(marker.len())?;
                record.source.extend_from_slice(marker);
                record.pending.push_back(TextRequest::Separator);
                record.needs_prepare = true;
                if record.novel.is_some() && record.story_body.source.is_some() {
                    // A separator changes reveal boundaries while retaining the appended body.
                    record.story_body.note_text(host.frame, true, Some(&[]));
                }
                record.observe_story_text(key, host, keep_prefix, Some(&[]));
            },
            b"clear" => for key in &keys {
                let record = self.banks.get_mut(key).unwrap();
                record.replace(&[])?;
                if record.novel.is_some() && record.story_body.source.is_some() {
                    record.story_body.note_source(host.frame);
                }
                record.observe_story_source(key, host);
            },
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
                if host.data_services.story_trace && record.is_story_body(key)
                    && host.css_task_sha() == Some(labyrinth15::PE)
                    && command.context.block.as_slice() == b"STRLAYOUT"
                    && host.cst_contexts.values().any(|cst| cst.story.is_scene("yum_h01"))
                    && let Some((epoch, source_frame)) = record.story_body.source
                    && host.frame <= source_frame.saturating_add(80)
                {
                    host.events.push(LogicEvent {
                        frame: host.frame,
                        kind: "labyrinth15-text".into(),
                        detail: format!("owner={owner} block=STRLAYOUT offset={:#x} op=reset epoch={epoch} source_frame={source_frame} provenance_before={:?}",
                            command.offset, record.story_body.provenance),
                    });
                }
                let labyrinth_layout = host.data_services.story_trace && command.offset == 0x53a0
                    && command.context.block.as_slice() == b"STRLAYOUT"
                    && command.object_event.is_none() && record.is_story_body(key)
                    && host.css_task_sha() == Some(labyrinth15::PE) && record.labyrinth_first_body(host)
                    && labyrinth15::verified_meswnd(host.resources, &driver.interpreter.program).is_some();
                // STRLAYOUT rebuilds formatting while retaining the queued line.
                let yukikoi_relayout = yukikoi_before.is_some() && record.is_story_body(key)
                    && command.offset == 0x6613 && command.context.block.as_slice() == b"STRLAYOUT"
                    && command.object_event.is_none() && record.story_body.eden_layout_skip_pending;
                let djibril_layout = host.data_services.story_trace && command.offset == 0x57fb
                    && command.context.block.as_slice() == b"STRLAYOUT"
                    && command.object_event.is_none() && record.is_story_body(key)
                    && host.css_task_sha() == Some(djibril10::PE)
                    && record.story_body.provenance.as_deref().is_some_and(|sources| {
                        let origins = &host.data_services.cst_text_origins;
                        origins.body_is_current(sources) && sources.iter().all(|&source| {
                            host.cst_contexts.get(&source.context_handle).is_some_and(djibril10::opening_scene)
                                && djibril10::source_read(origins, source, 0, host.frame)
                        })
                    })
                    && djibril10::verified_meswnd(host.resources, &driver.interpreter.program);
                if !labyrinth_layout && !yukikoi_relayout && !djibril_layout
                    && record.eden_body_source(&host.data_services.cst_text_origins).is_none()
                {
                    record.story_body.provenance = None;
                }
            },
            b"draw" => {
                let segment = optional_number(args, 0, 0, driver, shared)?;
                for key in &keys {
                    let record = self.banks.get_mut(key).unwrap();
                    record.draw = Some(record.draw_range(segment));
                    record.remaining = 1;
                    record.remaining_segment = 1;
                }
            }
            b"skip" => {
                // MES_SCROLLSPD can rebuild the hidden layout outside the system handler.
                let flags = driver.story_skip_flags();
                let yukikoi_automag = yukikoi_before.is_some() && command.offset == 0x7afc
                    && command.context.block.as_slice() == b"MES_AUTOMAG"
                    && command.object_event.is_none() && flags == (false, false, false, false)
                    && driver.interpreter.local_values().get(150) == Some(&-1);
                let scroll_layout = host.data_services.story_trace && command.offset == 0x6809
                    && command.context.block.as_slice() == b"MES_SCROLLSPD"
                    && command.object_event.is_none()
                    && flags == (false, false, false, false)
                    && keys.iter().any(|key| {
                        let bank = &self.banks[key];
                        bank.is_story_body(key)
                            && bank.eden_body_source(&host.data_services.cst_text_origins).is_some()
                    })
                    && eden_diagnostic_fes(host, driver) == Some(true);
                let labyrinth_layout = host.data_services.story_trace && command.offset == 0x5baa
                    && command.context.block.as_slice() == b"MES_SCROLLSPD"
                    && command.object_event.is_none()
                    && flags == (false, false, false, false)
                    && host.css_task_sha() == Some(labyrinth15::PE)
                    && keys.iter().any(|key| self.banks[key].is_story_body(key) && self.banks[key].labyrinth_first_body(host))
                    && labyrinth15::verified_meswnd(host.resources, &driver.interpreter.program).is_some();
                for key in &keys {
                    let bank = self.banks.get_mut(key).unwrap();
                    let body = &bank.story_body;
                    let sourced_layout = (scroll_layout
                        && bank.eden_body_source(&host.data_services.cst_text_origins).is_some())
                        || (labyrinth_layout && bank.labyrinth_first_body(host));
                    let layout = sourced_layout && bank.is_story_body(key)
                        && (!bank.visible || body.eden_layout_surface.is_some())
                        && (!bank.skip || body.eden_layout_skip_pending) && !body.forced
                        && body.natural_complete.is_none();
                    if host.data_services.story_trace && command.offset == 0x6809
                        && command.context.block.as_slice() == b"MES_SCROLLSPD"
                        && bank.is_story_body(key)
                        && let Some(source) = bank.eden_body_source(&host.data_services.cst_text_origins)
                        && (source.group, source.absolute_record) == (4, 102)
                        && let Some((epoch, source_frame)) = body.source
                        && host.frame <= source_frame.saturating_add(80)
                    {
                        let unskipped = driver.in_unskipped_system_block(b"MES_SCROLLSPD");
                        let object = command.object_event.is_some();
                        let live_fes = eden_diagnostic_fes(host, driver);
                        host.events.push(LogicEvent {
                            frame: host.frame,
                            kind: "eden-scroll-gate".into(),
                            detail: format!("owner={owner} epoch={epoch} source={source:?} object={object} \
                                unskipped={unskipped} flags(force,input,cont,cancel)={flags:?} live_fes={live_fes:?} \
                                visible={} already_skip={} forced={} natural={:?} layout_surface={:?} layout={layout}",
                                bank.visible, bank.skip, body.forced, body.natural_complete, body.eden_layout_surface),
                        });
                    }
                    let yukikoi_layout = yukikoi_automag && bank.is_story_body(key)
                        && (!bank.skip || body.yukikoi_layout_skip_pending) && !body.forced
                        && bank.yukikoi_body_source(&host.data_services.cst_text_origins);
                    if yukikoi_layout {
                        // The new layout must submit again even if storage is reused.
                        bank.story_body.committed_epoch = None;
                        bank.story_body.eden_layout_complete = None;
                    }
                    bank.story_body.eden_layout_skip_pending = layout || yukikoi_layout;
                    bank.story_body.yukikoi_layout_skip_pending = yukikoi_layout;
                    bank.skip = true;
                }
            },
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
            b"mag" => {
                // CS2 stores reveal wait scaling separately from pixel geometry;
                // 0 and -1 disable artificial pacing rather than zeroing scale.
                let _ = optional_number(args, 0, 100, driver, shared)?;
            }
            _ => bail!("unsupported ordinary STRING command {:?}", String::from_utf8_lossy(verb)),
        }
        for key in &keys {
            host.data_services.fes_motion.synchronize_text(self, owner, &key.1, key.2, verb)?;
            let record = &self.banks[key];
            if let Some(before) = &yukikoi_before && record.is_story_body(key) {
                let block = String::from_utf8_lossy(&command.context.block);
                let op = String::from_utf8_lossy(verb);
                let object_event = command.object_event.is_some();
                let flags = driver.story_skip_flags();
                let after = &record.story_body;
                host.events.push(LogicEvent {
                    frame: host.frame,
                    kind: "yukikoi-body-op".into(),
                    detail: format!("owner={owner} block={block} offset={:#x} op={op} object_event={object_event} \
                        input(bytes,control_only,decoded,statement,provenance)={yukikoi_input:?} \
                        before={before:?} after={after:?} visible={} skip={} flags(force,input,cont,cancel)={flags:?}",
                        command.offset, record.visible, record.skip),
                });
            }
            if host.data_services.story_trace && matches!(verb, b"apend" | b"apendmark" | b"skip" | b"draw")
                && host.css_task_sha() == Some(labyrinth15::PE)
                && host.cst_contexts.values().any(|cst| cst.story.is_scene("yum_h01"))
                && let Some(record) = self.banks.get(key).filter(|bank| bank.is_story_body(key))
                && let Some((epoch, source_frame)) = record.story_body.source
                && host.frame <= source_frame.saturating_add(80)
            {
                let op = String::from_utf8_lossy(verb);
                let block = String::from_utf8_lossy(&command.context.block);
                let flags = driver.story_skip_flags();
                host.events.push(LogicEvent {
                    frame: host.frame,
                    kind: "labyrinth15-text".into(),
                    detail: format!("owner={owner} block={block} offset={:#x} op={op} epoch={epoch} source_frame={source_frame} provenance={:?} visible={} skip={} layout_skip={} object={} flags(force,input,cont,cancel)={flags:?}",
                        command.offset, record.story_body.provenance, record.visible, record.skip,
                        record.story_body.eden_layout_skip_pending, command.object_event.is_some()),
                });
            }
            if host.data_services.story_trace && matches!(verb, b"apend" | b"reset" | b"skip" | b"draw") {
                let record = &self.banks[key];
                if let Some((epoch, source_frame)) = record.story_body.source
                    && let Some(source) = record.eden_body_source(&host.data_services.cst_text_origins)
                {
                    let block = String::from_utf8_lossy(driver.interpreter.current_block());
                    let op = String::from_utf8_lossy(verb);
                    let payload = host.cst_contexts.get(&source.context_handle)
                        .and_then(|context| context.document.as_ref())
                        .and_then(|document| document.records.get(source.absolute_record))
                        .map(|record| record.text.as_slice());
                    if let Some(payload) = payload {
                        let digest = super::particle_plugin_digest::sha256(payload);
                        host.events.push(LogicEvent {
                            frame: host.frame,
                            kind: "fes-text-eden-body-op".into(),
                            detail: format!("owner={owner} block={block} offset={:#x} op={op} epoch={epoch} source_frame={source_frame} source={source:?} body_bytes={} body_sha256={digest} visible={} skip={} draw={:?} layout_skip={}",
                                command.offset, payload.len(), record.visible, record.skip, record.draw,
                                record.story_body.eden_layout_skip_pending),
                        });
                    }
                }
            }
        }
        Ok(true)
    }

    fn layout(
        &mut self, keys: &[Key], command: &FesDriverCommand, driver: &mut FesScriptDriver,
        shared: &mut ExpressionState, defaults: &MessageDefaults,
        resource_encoding: ScriptTextEncoding, host: &mut LoggingHost<'_>, reset_file_font: bool,
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
            let backend = if verb == b"fontind" && !reset_file_font {
                optional_number(args, 0, 0, driver, shared)? as u32
            } else { 0 };
            let name = if reset_file_font {
                b"".as_slice()
            } else {
                let count = if verb == b"fontind" { 4 } else { 3 };
                raw_after_spaces(&command.source, count).unwrap_or_default()
            };
            for key in keys {
                let record = self.banks.get_mut(key).unwrap();
                let font = if name.is_empty() { defaults.font_name.clone() }
                    else if verb == b"font" && record.message.encoding() == MessageEncoding::Unicode {
                        unicode_layout_font(name)?
                    } else {
                        decode_layout_font(name, resource_encoding, command.offset, host)
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
    fn yukikoi_body_source(&self, origins: &CstTextOrigins) -> bool {
        let Some([source]) = self.story_body.provenance.as_deref() else { return false; };
        origins.source_record_observations(source.context_handle).iter().any(|entry| {
            entry.source == *source && entry.probe == super::cst_context_native::CstSourceProbe::Yukikoi
                && entry.body_tagged && entry.kind == 0x20 && entry.raw_bytes > 0 && entry.output_bytes > 0
        })
    }

    fn eden_body_source(&self, origins: &CstTextOrigins) -> Option<CstBodySource> {
        let [source] = self.story_body.provenance.as_deref()? else { return None; };
        origins.source_record_observations(source.context_handle).iter().any(|entry| {
            entry.source == *source
                && entry.probe == super::cst_context_native::CstSourceProbe::Eden
                && entry.native == 327 && entry.kind == 0x20 && entry.body_tagged && !entry.localized
                && entry.stored_codec == ScriptTextEncoding::Gbk && entry.output_codec == ScriptTextEncoding::Gbk
        }).then_some(*source)
    }

    fn labyrinth_first_body(&self, host: &LoggingHost<'_>) -> bool {
        let Some(sources @ [source]) = self.story_body.provenance.as_deref() else { return false; };
        host.data_services.cst_text_origins.body_is_current(sources)
            && host.cst_contexts.get(&source.context_handle)
                .is_some_and(|context| labyrinth15::first_body(context, *source))
    }

    fn is_story_body(&self, key: &Key) -> bool {
        key.1.as_slice() == b"str" && key.2 == 0 && self.novel.is_none() && self.log.is_none()
    }

    fn observe_story_source(&mut self, key: &Key, host: &LoggingHost<'_>) {
        if host.data_services.story_trace && self.is_story_body(key) {
            self.story_body.note_source(host.frame);
        }
    }

    fn observe_story_text(&mut self, key: &Key, host: &LoggingHost<'_>, keep_prefix: bool,
        suffix: Option<&[CstBodySource]>,
    ) {
        if host.data_services.story_trace && self.is_story_body(key) {
            self.story_body.note_text(host.frame, keep_prefix, suffix);
        }
    }

    fn observe_story_reveal(&mut self, frame: u64, naturally_updated: bool, start_requested: bool,
        device: &DesktopMessageDevice,
    ) {
        let Some((epoch, _)) = self.story_body.source else { return; };
        if self.story_body.forced || self.source.is_empty() {
            return;
        }
        let fully_visible = !self.message.active() && self.remaining == 0 && self.remaining_segment == 0
            && self.projection.as_ref().is_some_and(|projection| projection.options.visible);
        let layout_rendered = self.story_body.eden_layout_surface == Some(self.message.surface_generation());
        let layout_ready = layout_rendered && fully_visible && self.pending.is_empty()
            && !self.needs_prepare && self.draw.is_none() && !self.skip;
        if !layout_rendered && naturally_updated && fully_visible && self.message.last_complete() {
            self.story_body.natural_complete.get_or_insert((epoch, frame));
        }
        if (self.story_body.natural_complete.is_some() || layout_ready)
            && self.story_body.committed_epoch != Some(epoch)
            && (layout_rendered || start_requested
                || self.message.configuration().is_some_and(|config| config.ordinary_submit())
                || self.message.complete_snapshot())
            && device.surface(self.message.id()).is_some_and(|surface| {
                surface.generation == self.message.surface_generation() && surface.dirty.is_none()
                    && surface.submitted.pixels().chunks_exact(4).any(|pixel| pixel[3] != 0)
            })
        {
            self.story_body.committed_epoch = Some(epoch);
            if layout_rendered {
                // The hidden force operation can paint without submitting.
                // Count this only after the current surface is visible and clean.
                self.story_body.eden_layout_complete = Some((epoch, frame));
            }
        }
    }

    fn set_speaker(&mut self, raw: &[u8], resource_encoding: ScriptTextEncoding, decoded: Option<&str>) -> Result<()> {
        if let Some(novel) = &mut self.novel {
            let label = raw.strip_prefix(b"\\pr").unwrap_or(raw);
            if let Some(decoded) = decoded {
                let label_text = decoded.strip_prefix("\\pr").unwrap_or(decoded);
                novel.set_decoded_speaker(label, label_text, self.message.encoding());
            } else {
                novel.speaker = normalize_text(label, self.message.encoding(), false, resource_encoding)?;
            }
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
    fn append(&mut self, bytes: &[u8], legacy_gbk: bool) -> Result<()> {
        self.append_text(bytes, legacy_gbk, None)
    }

    fn insert_novel_paragraph(&mut self, paragraph_local_draw: bool) {
        let novel = self.novel.as_mut().unwrap();
        novel.insert_pending = true;
        if paragraph_local_draw {
            // The legacy NOVEL adapter starts a new local draw range on
            // insert. The shared layout keeps earlier paragraphs in one surface.
            novel.preceding_markers = self.pending.iter().fold(
                self.message.separator_offsets().len() as u32,
                |count, request| match request {
                    TextRequest::Clear => 0,
                    TextRequest::Separator => count + 1,
                    _ => count,
                },
            );
        }
    }

    fn draw_range(&self, segment: i32) -> u32 {
        if segment < 0 { return segment as u32; }
        let preceding = self.novel.as_ref().map_or(0, |novel| novel.preceding_markers);
        segment.saturating_add(preceding as i32) as u32
    }

    fn append_text(&mut self, bytes: &[u8], legacy_gbk: bool, decoded: Option<&str>) -> Result<()> {
        self.check_append(bytes.len())?;
        self.source.extend_from_slice(bytes);
        self.source_initialized = true;
        let request = if let Some(text) = decoded {
            TextRequest::AppendDecoded { bytes: bytes.to_vec(), text: text.to_owned() }
        } else if legacy_gbk && self.message.encoding() == MessageEncoding::Cp932 {
            TextRequest::AppendLegacyGbk(bytes.to_vec())
        } else {
            TextRequest::Append(bytes.to_vec())
        };
        self.pending.push_back(request);
        self.needs_prepare = true; self.remaining = u32::MAX; self.remaining_segment = u32::MAX;
        Ok(())
    }
    fn append_body(&mut self, bytes: &[u8], legacy_gbk: bool) -> Result<()> {
        self.append_body_text(bytes, legacy_gbk, None)
    }
    fn append_body_text(&mut self, bytes: &[u8], legacy_gbk: bool, decoded: Option<&str>) -> Result<()> {
        if bytes.is_empty() || self.novel.as_ref().is_none_or(|novel| {
            !novel.insert_pending && novel.speaker.is_empty()
        }) {
            return self.append_text(bytes, legacy_gbk, decoded);
        }
        let novel = self.novel.as_ref().unwrap();
        let (speaker_raw, speaker_decoded) = novel.speaker_parts()?;
        let mut paragraph = Vec::new();
        let newline = novel.insert_pending && !self.source.is_empty();
        if newline {
            paragraph.push(b'\n');
        }
        if !speaker_raw.is_empty() {
            paragraph.extend_from_slice(speaker_raw);
            paragraph.push(b'\n');
        }
        paragraph.extend_from_slice(bytes);
        let text = if decoded.is_some() || speaker_decoded.is_some() {
            let codec = match self.message.encoding() {
                MessageEncoding::Unicode => ScriptTextEncoding::Utf8,
                MessageEncoding::Cp932 if legacy_gbk => ScriptTextEncoding::Gbk,
                MessageEncoding::Cp932 => ScriptTextEncoding::Cp932,
            };
            let mut text = String::new();
            if newline { text.push('\n'); }
            if !speaker_raw.is_empty() {
                if let Some(speaker) = speaker_decoded { text.push_str(speaker); }
                else { text.push_str(&codec.decode_message(speaker_raw).0); }
                text.push('\n');
            }
            if let Some(body) = decoded { text.push_str(body); }
            else { text.push_str(&codec.decode_message(bytes).0); }
            Some(text)
        } else { None };
        self.append_text(&paragraph, legacy_gbk, text.as_deref())?;
        let novel = self.novel.as_mut().unwrap();
        novel.insert_pending = false;
        novel.speaker.clear();
        Ok(())
    }
    fn replace(&mut self, bytes: &[u8]) -> Result<()> {
        self.replace_with_codec(bytes, false)
    }
    fn replace_with_codec(&mut self, bytes: &[u8], legacy_gbk: bool) -> Result<()> {
        self.replace_text(bytes, legacy_gbk, None)
    }
    fn replace_text(&mut self, bytes: &[u8], legacy_gbk: bool, decoded: Option<&str>) -> Result<()> {
        if bytes.len() > MAX_TEXT_BYTES { bail!("STRING replacement text exceeds 64 MiB"); }
        if let Some(novel) = &mut self.novel {
            novel.insert_pending = false;
            novel.speaker.clear();
            novel.preceding_markers = 0;
        }
        self.source.clear(); self.pending.clear();
        self.pending.push_back(TextRequest::Clear);
        self.pending.push_back(TextRequest::Clear);
        self.append_text(bytes, legacy_gbk, decoded)?;
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
fn fes_story_body_generation_drops_stale_reveal_and_commit() {
    let mut store = FesTextStore::default();
    store.create(65, &FesObjectDefinition {
        offset: 0, name: b"str".to_vec(), kind: b"STRING".to_vec(),
        bank_expression: None, banks: 1,
    }, &defaults(), &mut DesktopMessageDevice::default()).unwrap();
    let key = (65, b"str".to_vec(), 0);
    assert!(store.story_body_generation(65).is_none());
    let bank = store.banks.get_mut(&key).unwrap();
    bank.story_body.note_source(1105);
    bank.story_body.natural_complete = Some((1, 1170));
    bank.story_body.committed_epoch = Some(1);
    assert!(store.story_body_generation(66).is_none());
    assert_eq!(store.story_body_generation(65).unwrap().natural_complete, Some((1, 1170)));

    let bank = store.banks.get_mut(&key).unwrap();
    bank.append(b"next", false).unwrap();
    bank.story_body.note_source(1501);
    let next = store.story_body_generation(65).unwrap();
    assert_eq!((next.source_epoch, next.source_frame), (2, 1501));
    assert_eq!((next.natural_complete, next.committed_epoch), (None, None));
    assert!(next.pending_text && next.remaining > 0);

    let bank = store.banks.get_mut(&key).unwrap();
    bank.story_body.force();
    bank.observe_story_reveal(1502, true, true, &DesktopMessageDevice::default());
    let forced = store.story_body_generation(65).unwrap();
    assert!(forced.forced);
    assert_eq!((forced.natural_complete, forced.committed_epoch), (None, None));
}

#[test]
fn fes_story_body_provenance_requires_every_fragment_of_the_current_page() {
    let mut progress = StoryBodyProgress::default();
    let first = CstBodySource { context_handle: 208, load_id: 7, group: 1, absolute_record: 20 };
    let second = CstBodySource { absolute_record: 21, ..first };
    let other_page = CstBodySource { group: 2, absolute_record: 24, ..first };
    progress.note_text(1, false, Some(&[first]));
    progress.note_text(2, true, Some(&[second]));
    assert_eq!(progress.source, Some((2, 2)));
    assert_eq!(progress.provenance.as_deref(), Some([first, second].as_slice()));
    progress.natural_complete = Some((2, 2));
    progress.committed_epoch = Some(2);
    progress.novel_composed = Some((2, 2));
    progress.note_text(3, true, Some(&[]));
    assert_eq!(progress.source, Some((3, 3)));
    assert_eq!(progress.provenance.as_deref(), Some([first, second].as_slice()));
    assert_eq!((progress.natural_complete, progress.committed_epoch, progress.novel_composed),
        (None, None, None));
    progress.note_text(4, true, Some(&[other_page]));
    assert!(progress.provenance.is_none());
    progress.note_text(5, true, Some(&[other_page]));
    assert!(progress.provenance.is_none());
    progress.note_text(6, false, Some(&[other_page]));
    assert_eq!(progress.provenance.as_deref(), Some([other_page].as_slice()));
    progress.note_text(7, true, None);
    assert!(progress.provenance.is_none());
    progress.note_text(8, false, Some(&[other_page]));
    progress.note_source(9);
    assert!(progress.provenance.is_none());
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
        bank.set_speaker(b"\\pr$fixture", ScriptTextEncoding::Cp932, None).unwrap();
        assert_eq!(bank.source, b"existing text");
        assert_eq!(bank.pending.len(), request_count);
        assert_eq!(bank.clip, Some(Rect::new(10, 5, 50, 30)));
        if let Some(novel) = &bank.novel {
            assert_eq!(novel.speaker, b"$fixture");
        }
    }
}

#[test]
fn hatsumira_font_reset_uses_configured_face_after_temporary_file_font() -> Result<()> {
    use cs2_runtime::message_object::MessageConfiguration;
    use cs2_script::{FesDocument, FesExecutionContext};

    let root = std::env::temp_dir().join(format!("cs2-hatsumira-font-{}", std::process::id()));
    std::fs::create_dir(&root)?;
    let player = HeadlessPlayer::open_with_save_root(&root, root.join("profile"));
    std::fs::remove_dir(&root)?;
    let mut player = player?;
    let document = FesDocument { sections: Vec::new() };
    let mut runtime = FesRuntime::new(document.clone());
    let mut driver = FesScriptDriver::new(&document)?;
    let mut store = FesTextStore::default();
    let mut defaults = defaults();
    defaults.font_name = "Fixture default".encode_utf16().collect();
    let key = (13, b"str".to_vec(), 0);
    store.create(13, &FesObjectDefinition {
        offset: 0, name: key.1.clone(), kind: b"STRING".to_vec(), bank_expression: None, banks: 1,
    }, &defaults, &mut DesktopMessageDevice::default())?;
    let mut shared = ExpressionState::new(6);
    shared.shared[5] = -10;

    player.with_host(|host| {
        for (profile, file, backend, face) in [
            (hatsumira::PE, "nemukemedium.ttf", 1, "nemukemedium.ttf"),
            (hatsumira::PE, "def", 0, "Fixture default"),
            ("different-game", "def", 1, "def"),
        ] {
            host.data_services.css_task_pe_sha256 = Some(profile);
            let source = format!("str layout fontind 1 {file}").into_bytes();
            let command = FesDriverCommand {
                offset: 0x4dd8,
                parameters: source.split(|&byte| byte == b' ').map(<[u8]>::to_vec).collect(),
                source,
                context: FesExecutionContext {
                    block: b"STRLAYOUT".to_vec(), position: 0, local: Vec::new(), current: 0,
                },
                object_event: None,
            };
            assert!(store.command(13, &command, &mut driver, &mut shared, &defaults,
                ScriptTextEncoding::Cp932, &mut runtime, host)?);
            let configuration = MessageConfiguration::from_bytes(
                &store.banks[&key].configuration, MessageEncoding::Cp932,
            )?;
            assert_eq!(configuration.font_backend(), backend);
            assert_eq!(String::from_utf16(&configuration.font_name())?, face);
            assert_eq!(shared.shared[5], -10);
        }
        Ok(())
    })
}

#[test]
fn legacy_fes_text_uses_selected_gbk_and_keeps_separator() {
    let defaults = defaults();
    let mut device = DesktopMessageDevice::default();
    let mut message = MessageObject::new(-1, MessageEncoding::Cp932);
    let configuration = initial_configuration(MessageEncoding::Cp932, &defaults, 0).unwrap();
    message.reconfigure(&configuration, None, &mut device).unwrap();

    let ambiguous = b"\xb7\xc7";
    message.append(ambiguous).unwrap();
    assert_ne!(message.source(), "\u{975e}");
    message.clear();

    let raw = b"\xb7\xc7\xff\xd5\xe2";
    message.append(raw).unwrap();
    assert!(message.source().contains('\u{fffd}'));
    message.clear();
    message.append_legacy_gbk(raw).unwrap();
    assert_eq!(message.source(), "\u{975e}\u{ffff}\u{8fd9}");
    assert_eq!(message.raw_appends(), &[raw.to_vec()]);
    message.append_legacy_gbk(b"\x81").unwrap();
    assert_eq!(message.source(), "\u{975e}\u{ffff}\u{8fd9}\u{fffd}");
    assert!(message.append_legacy_gbk(b"embedded\0zero").is_err());
    assert_eq!(message.raw_appends().last().unwrap(), b"\x81");
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
            let _stage = TextHostTimer::new(TextStage::Metadata);
            log::geometry(host, &key)?;
            continue;
        }
        let metadata = &host.data_services.fes_text.banks[&key].metadata;
        if !metadata.size_dirty && metadata.position_source.is_none() {
            continue;
        }
        let _stage = TextHostTimer::new(TextStage::Metadata);
        let metadata = metadata.clone();
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
    {
        let _stage = TextHostTimer::new(TextStage::FontCatalog);
        let _detail = SlowTask::new(view.id, || 34, "fes_text_load_fonts");
        host.data_services.message_device.load_fonts(host.resources);
    }
    let data = &mut *host.data_services;
    let loose_gbk = data.has_loose_gbk_text();
    let next_surface = &mut data.fes_text.next_surface;
    let mut warnings = Vec::new();
    for ((owner, name, index), bank) in &mut data.fes_text.banks {
        if *owner != view.id { continue; }
        if bank.log.is_some() {
            let _stage = TextHostTimer::new(TextStage::History);
            let _detail = SlowTask::new(view.id, || 34, "fes_text_log").named(name, None).code(*index);
            log::update(bank, next_surface, &mut data.message_device, view, runtime, name, *index,
                host.backlogs, host.resources, loose_gbk, |detail| warnings.push(detail))?;
        } else {
            let observed = bank.story_body.source.is_some();
            let forced = observed && bank.skip && !bank.story_body.eden_layout_skip_pending;
            let start_requested = bank.draw.is_some();
            let naturally_updated = observed && !forced && bank.last_frame != Some(view.now)
                && view.list.is_some() && (bank.message.active() || start_requested);
            if forced { bank.story_body.force(); }
            render_bank(&mut data.message_device, view, runtime, bank, name, *index)
                .with_context(|| format!("FES text {}[{index}] source={:?}",
                    String::from_utf8_lossy(name), bank.metadata.size_source))?;
            if observed {
                bank.observe_story_reveal(host.frame, naturally_updated, start_requested, &data.message_device);
            }
        }
    }
    for detail in warnings {
        data.note_text_decode(host.events, host.frame, detail);
    }
    if view.list.is_some() {
        let scene_text: Vec<_> = host.data_services.fes_text.banks.iter()
            .filter(|((owner, _, _), bank)| *owner == view.id && has_scene_projection(bank))
            .filter_map(|(key, bank)| Some((key.clone(), bank.projection.clone()?)))
            .collect();
        for (key, projection) in scene_text {
            let _stage = TextHostTimer::new(TextStage::NovelProjection);
            let _detail = SlowTask::new(view.id, || 34, "fes_text_scene_projection").named(&key.1, None).code(key.2);
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
    let pixel_less = uses_canvas_atlas(bank)
        && word(&bank.configuration, 8) == 0 && word(&bank.configuration, 12) == 0
        && bank.metadata.size_source.as_ref().is_some_and(|reference| {
            (matches!(reference.frame, 10 | 11)
                && reference.name.eq_ignore_ascii_case(b"sys_mdummy.hg3"))
                // This configuration script creates six banks for five image slots.
                || (name == b"font_list" && index == 5 && reference.frame == 735
                    && (reference.name.eq_ignore_ascii_case(b"sys_config")
                        || reference.name.eq_ignore_ascii_case(b"sys_config.hg3")))
        });
    if pixel_less {
        device.without_message_pixels(bank.message.id(), |device| {
            render_bank_with_surface(device, view, runtime, bank, (name, index), false)
        })
    } else {
        render_bank_with_surface(device, view, runtime, bank, (name, index), true)
    }
}

fn render_bank_with_surface(
    device: &mut DesktopMessageDevice, view: &FesTaskView, runtime: &FesRuntime,
    bank: &mut TextBank, (name, index): (&[u8], i32), allocate_surface: bool,
) -> Result<()> {
    let text = TextWork::new(view.id, index, name);
    let layout = SlowTask::new(view.id, || 34, "fes_text_layout").named(name, None).code(index);
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
        let _stage = TextTimer::new(TextStage::Configure);
        bank.message.reconfigure(&bank.configuration, scale, device)?;
        bank.configured = true; bank.dirty = false;
    }
    if bank.user_source_dirty || bank.user_mapping_cursor < bank.user_mappings.len() {
        let _stage = TextTimer::new(TextStage::UserMaps);
        if bank.user_source_dirty {
            bank.message.set_user_glyph_source(bank.user_glyph_source.clone())?;
            bank.user_source_dirty = false;
        }
        while let Some(&(character, frame)) = bank.user_mappings.get(bank.user_mapping_cursor) {
            bank.message.map_user_glyph(u32::from(character), u32::from(frame), device)?;
            bank.user_mapping_cursor += 1;
        }
    }
    let mut restore_completed_plane = false;
    if bank.dirty {
        let _stage = TextTimer::new(TextStage::Configure);
        let completed_plane = allocate_surface
            && bank.novel.is_none()
            && has_scene_projection(bank)
            && bank.visible
            && !bank.message.active()
            && bank.remaining == 0
            && bank.draw.is_none()
            && bank.pending.is_empty()
            && !bank.message.source().is_empty();
        if !completed_plane || bank.message.configuration_bytes() != Some(bank.configuration.as_slice()) {
            bank.message.reconfigure(&bank.configuration, scale, device)?;
            restore_completed_plane = completed_plane;
        }
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
                let _stage = TextTimer::new(TextStage::Clear);
                bank.message.clear();
                // Submit the genuinely empty page before any new suffix
                // is appended; str alone must not reveal its first glyph.
                bank.message.prepare(scale, device)?;
                bank.message.advance_display(0, MessageCopy::Full, device)?;
            }
            TextRequest::Append(bytes) => {
                let codec = match bank.message.encoding() {
                    MessageEncoding::Cp932 => ScriptTextEncoding::Cp932,
                    MessageEncoding::Unicode => ScriptTextEncoding::Utf8,
                };
                bank.message.append_decoded(&bytes, &codec.decode_message(&bytes).0)?;
            }
            TextRequest::AppendLegacyGbk(bytes) => {
                let codec = ScriptTextEncoding::Gbk;
                bank.message.append_decoded(&bytes, &codec.decode_message(&bytes).0)?;
            }
            TextRequest::AppendDecoded { bytes, text } => bank.message.append_decoded(&bytes, &text)?,
            TextRequest::Separator => bank.message.append_separator()?,
        }
        bank.pending.pop_front();
    }
    if bank.needs_prepare {
        let _stage = TextTimer::new(TextStage::Prepare);
        bank.message.prepare(scale, device)?;
        bank.line_count = bank.message.layout().map_or(0, |layout| layout.line_count);
        bank.needs_prepare = false;
    }
    drop(layout);
    let raster = SlowTask::new(view.id, || 34, "fes_text_raster").named(name, None).code(index);
    let visible = bank.visible && display_parent.is_none_or(|plane| plane.visible);
    let display = if view.list.is_none() { MessagePlaneDisplay::Unreachable }
        else if visible { MessagePlaneDisplay::Visible } else { MessagePlaneDisplay::Hidden };
    let playback = TextTimer::new(TextStage::Playback);
    if restore_completed_plane {
        bank.message.force_complete(display, scale, device)?;
    }
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
        let update = bank.message.force_complete(display, scale, device)?;
        if std::mem::take(&mut bank.story_body.eden_layout_skip_pending) {
            let previous_surface = bank.story_body.eden_layout_surface;
            let yukikoi_layout = std::mem::take(&mut bank.story_body.yukikoi_layout_skip_pending);
            // Yukikoi's verified MES_AUTOMAG path can rebuild an already visible window.
            if (matches!(display, MessagePlaneDisplay::Hidden)
                || (matches!(display, MessagePlaneDisplay::Visible) && (previous_surface.is_some() || yukikoi_layout)))
                && update.is_some_and(|update| update.complete)
                && bank.message.remaining_all()? == 0
            {
                let surface = Some(bank.message.surface_generation());
                if previous_surface != surface {
                    bank.story_body.eden_layout_complete = None;
                    bank.story_body.committed_epoch = None;
                }
                bank.story_body.eden_layout_surface = surface;
            } else {
                bank.story_body.force();
            }
        }
        bank.skip = false;
    }
    if bank.last_frame != Some(view.now) {
        bank.message.frame_update(view.now, scale, device)?;
        bank.last_frame = Some(view.now);
    }
    drop(playback);
    if allocate_surface {
        let _stage = TextTimer::new(TextStage::SurfaceSync);
        device.sync_object(&bank.message)?;
    }
    bank.remaining = bank.message.remaining_all()?;
    bank.remaining_segment = bank.message.remaining_segment()?;
    if let Some(layout) = bank.message.layout() {
        bank.cursor = layout.cursor; bank.draw_area = layout.cursor;
    }
    drop(raster);
    let _stage = TextTimer::new(TextStage::Projection);
    let _detail = SlowTask::new(view.id, || 34, "fes_text_projection").named(name, None).code(index);
    let Some(frame) = device.projection_frame(bank.message.id(), bank.tint)? else {
        bank.projection = None; return Ok(());
    };
    let texture_size = [frame.width(), frame.height()];
    let (registered_size, _) = bank.message.registered_geometry()
        .context("configured STRING has no registered geometry")?;
    // The backend surface is local to this text plane. FES samples that
    // plane at its VRAM origin; the cache coordinates must not also move
    // the resulting screen projection (history uses an offscreen atlas).
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
    if text.enabled() && bank.novel.is_none() {
        // Novel motion can replace the stored frame after this comparison point.
        let previous = bank.projection.as_ref().map(|projection| &projection.frame);
        let change = if previous.is_some_and(|previous| previous.pixel_identity().matches(&frame)) {
            TextProjection::Reused
        } else if let Some((base, region)) = frame.pixel_patch()
            && previous.is_some_and(|previous| base.matches(previous))
        {
            TextProjection::Patched(u64::from(region.width) * u64::from(region.height))
        } else {
            TextProjection::Replaced
        };
        text.projection(change, view.list.is_some() && options.visible && options.opacity != 0);
    }
    bank.projection = Some(FesTextProjection {
        owner: view.id, name: name.to_vec(), bank: index, frame, options, priority: bank.priority,
    });

    Ok(())
}

#[test]
#[cfg(windows)]
fn visual_progress_counts_real_fes_glyphs_without_newline_controls() {
    let mut store = FesTextStore::default();
    let mut device = DesktopMessageDevice::default();
    let windows = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:/Windows".into()));
    let bytes = std::fs::read(windows.join("Fonts/arial.ttf")).unwrap();
    device.bind_font("Arial", &bytes, 0).unwrap();
    let mut defaults = defaults();
    defaults.font_name = "Arial".encode_utf16().collect();
    store.create(39, &FesObjectDefinition {
        offset: 0, name: b"str".to_vec(), kind: b"STRING".to_vec(), bank_expression: None, banks: 1,
    }, &defaults, &mut device).unwrap();
    let bank = store.banks.get_mut(&(39, b"str".to_vec(), 0)).unwrap();
    bank.set_size([180, 80]).unwrap();
    bank.visible = true;
    bank.story_body.note_source(1);
    bank.append(b"A\nB", false).unwrap();
    bank.draw = Some(u32::MAX);
    let runtime = FesRuntime::new(cs2_script::FesDocument { sections: Vec::new() });
    let view = FesTaskView { id: 39, parent: 0, context: Vec::new(),
        list: Some(super::fes_tasks::FesListBinding { task: 1, clip: [0, 0, 180, 80], offset: [0, 0] }),
        now: 19, paused: 0 };
    render_bank(&mut device, &view, &runtime, bank, b"str", 0).unwrap();
    let before = bank.message.clone();
    let (_, details) = super::perf_detail::HostPerfDetails::capture(false, || store.trace_visual_progress());
    let rows = details.rows(91, 1, 1);
    assert!(rows.contains("visual_text session=91"), "{rows}");
    assert!(rows.contains("glyph_total=2"), "{rows}");
    assert!(rows.contains("text_now=19"), "{rows}");
    assert_eq!(store.banks[&(39, b"str".to_vec(), 0)].message, before, "tracing cannot advance reveal state");
}

#[test]
fn fes_dummy_text_defers_pixels_without_skipping_layout() {
    let mut store = FesTextStore::default();
    let mut device = DesktopMessageDevice::default();
    let runtime = FesRuntime::new(cs2_script::FesDocument { sections: Vec::new() });
    let view = FesTaskView {
        id: 39, parent: 8, context: Vec::new(), list: None, now: 269, paused: 0,
    };
    for (offset, name, index, resource, frame, size) in [
        (0, b"name".as_slice(), 0, "sys_mdummy.hg3", 11, [471, 28]),
        (1, b"str".as_slice(), 0, "sys_mdummy.hg3", 10, [819, 125]),
        (2, b"font_list".as_slice(), 5, "sys_config", 735, [171, 20]),
    ] {
        store.create(view.id, &FesObjectDefinition {
            offset, name: name.to_vec(), kind: b"STRING".to_vec(), bank_expression: None,
            banks: index + 1,
        }, &defaults(), &mut device).unwrap();
        let bank = store.banks.get_mut(&(view.id, name.to_vec(), index)).unwrap();
        bank.metadata.size_source = Some(super::fes_controls::metadata_reference(
            format!("{resource}:{frame}").as_bytes(), ScriptTextEncoding::Cp932,
        ).unwrap());
        bank.set_size([0, 0]).unwrap();
        bank.replace(b"\n").unwrap();
        bank.draw = Some(0);
        render_bank(&mut device, &view, &runtime, bank, name, index).unwrap();
        assert_eq!(bank.message.source(), "\n");
        assert!(bank.pending.is_empty() && bank.draw.is_none());
        assert!(bank.message.layout().is_some());
        assert!(bank.projection.is_none());
        assert!(device.surface(bank.message.id()).is_none());

        let mut wrong_resource = bank.clone();
        wrong_resource.metadata.size_source.as_mut().unwrap().name = b"required.hg3".to_vec();
        let mut wrong_frame = bank.clone();
        wrong_frame.metadata.size_source.as_mut().unwrap().frame = 9;
        let mut partial_size = bank.clone();
        partial_size.set_size([0, 28]).unwrap();
        for mut invalid in [wrong_resource, wrong_frame, partial_size] {
            let error = render_bank(&mut device, &view, &runtime, &mut invalid, name, index).unwrap_err();
            assert!(format!("{error:#}").contains("message pixel surface"));
        }

        if name == b"font_list" {
            let error = render_bank(&mut device, &view, &runtime, &mut bank.clone(), name, index - 1)
                .unwrap_err();
            assert!(format!("{error:#}").contains("message pixel surface"));
        }

        bank.set_size(size).unwrap();
        let next_view = FesTaskView { now: view.now + 1, ..view.clone() };
        render_bank(&mut device, &next_view, &runtime, bank, name, index).unwrap();
        assert_eq!(bank.message.source(), "\n");
        let surface = device.surface(bank.message.id()).unwrap();
        assert_eq!([surface.submitted.width(), surface.submitted.height()], size.map(|dimension| dimension as u32));
    }
}

fn uses_canvas_atlas(bank: &TextBank) -> bool {
    bank.novel.is_none() && bank.log.is_none()
}

fn has_scene_projection(bank: &TextBank) -> bool {
    // An unparented STRING with its own VRAM source samples its submitted
    // glyphs at its screen pose and PRI, independently of any FES list plane.
    bank.novel.is_some()
        || (bank.log.is_none() && bank.parent_plane.is_none() && bank.vram != [0; 2])
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
        if *id != owner || has_scene_projection(bank) { continue; }
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

pub(super) fn scene_projections(
    data: &super::data_services::DataServices, lower: f32, upper: f32,
) -> Vec<FesTextProjection> {
    let mut projections: Vec<_> = data.fes_text.banks.values()
        .filter(|bank| has_scene_projection(bank))
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

fn decode_text(bytes: &[u8], codec: ScriptTextEncoding) -> Result<String> {
    if codec == ScriptTextEncoding::Utf8 {
        return Ok(std::str::from_utf8(bytes)?.to_owned());
    }
    let mut result = String::new();
    for (index, part) in bytes.split(|&byte| byte == 0xff).enumerate() {
        if index != 0 { result.push('\u{ffff}'); }
        let decoded = if codec == ScriptTextEncoding::Gbk {
            encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(part)
        } else {
            cs2_runtime::message_object::decode_cp932_display(part)
        }.with_context(|| format!("invalid {codec:?} FES text from declared source"))?;
        result.push_str(&decoded);
    }
    Ok(result)
}

struct SourcedText {
    text: String,
    literal: ScriptTextEncoding,
    variables: Vec<ScriptTextEncoding>,
}

impl SourcedText {
    fn trace_detail(&self, length: usize) -> String {
        let first = first_text_codepoint(&self.text)
            .map_or_else(|| "none".into(), |character| format!("U+{:04X}", u32::from(character)));
        format!("bytes={length} first={first} fes={:?} cst={:?}", self.literal, self.variables)
    }
}

fn first_text_codepoint(mut text: &str) -> Option<char> {
    loop {
        text = text.trim_start_matches(|character: char|
            character.is_control() || character == '\u{ffff}' || character == '[');
        let Some(after_escape) = text.strip_prefix('\\') else { return text.chars().next() };
        let marker = after_escape.as_bytes().first()?.to_ascii_lowercase();
        match marker {
            b'n' | b'r' | b'@' => text = &after_escape[1..],
            b'p' => {
                text = &after_escape[1..];
                if text.as_bytes().first().is_some_and(|byte| matches!(byte.to_ascii_lowercase(), b'c' | b'l' | b'r')) {
                    text = &text[1..];
                }
            }
            _ => return None,
        }
    }
}

fn fes_text_statement(driver: &FesScriptDriver, offset: usize) -> Option<(&[u8], &[u8])> {
    driver.interpreter.program.blocks.iter().find_map(|block| {
        let index = block.lines.binary_search_by_key(&offset, |line| line.offset).ok()?;
        let template = raw_after_spaces(&block.lines[index].bytes, 2)?;
        Some((block.name.as_slice(), template))
    })
}

fn story_command_sources(raw: &[u8], command: &FesDriverCommand, driver: &FesScriptDriver,
    shared: &ExpressionState, origins: &CstTextOrigins,
) -> Option<Vec<CstBodySource>> {
    if body_control_only(raw) { return Some(Vec::new()); }
    let (_, template) = fes_text_statement(driver, command.offset)?;
    story_template_sources(template, raw, shared, origins)
}

fn story_template_sources(mut template: &[u8], mut expanded: &[u8], shared: &ExpressionState,
    origins: &CstTextOrigins,
) -> Option<Vec<CstBodySource>> {
    let mut sources = Vec::new();
    while let Some(start) = template.windows(4).position(|part| part == b"$str") {
        let literal = &template[..start];
        if !body_control_only(literal) { return None; }
        expanded = expanded.strip_prefix(literal)?;
        let digits = &template[start + 4..];
        let count = digits.iter().take_while(|byte| byte.is_ascii_digit()).count();
        let id = std::str::from_utf8(&digits[..count]).ok()?.parse::<u32>().ok()?;
        let bytes = shared.strings.get(&id).map_or(&[][..], Vec::as_slice);
        expanded = expanded.strip_prefix(bytes)?;
        if !body_control_only(bytes) {
            sources = join_body_sources(&sources, origins.shared_body(id, bytes)?)?;
        }
        template = &digits[count + usize::from(digits.get(count) == Some(&b';'))..];
    }
    (expanded == template && body_control_only(template)).then_some(sources)
}

fn sourced_text(
    owner: u32, command: &FesDriverCommand, verb: &[u8], raw: &[u8], driver: &FesScriptDriver,
    shared: &ExpressionState, host: &mut LoggingHost<'_>,
) -> Result<Option<SourcedText>> {
    if raw.is_ascii() || verb == b"strsj" { return Ok(None); }
    let literal = if driver.is_island_gbk_fes_source() { ScriptTextEncoding::Gbk }
        else { host.text_encoding };
    let Some((block, template)) = fes_text_statement(driver, command.offset) else { return Ok(None); };
    let origins = &host.data_services.cst_text_origins;
    let trace = host.data_services.story_trace;
    let mut variables = Vec::new();
    let mut failures = Vec::new();
    let decoded = decode_sourced_text_with_failure(
        template, raw, literal, host.text_encoding, driver.is_island_gbk_fes_source(),
        |id| {
            let bytes = shared.strings.get(&id).map_or(&[][..], Vec::as_slice);
            let codec = origins.shared(id, bytes);
            if trace && let Some(codec) = codec && !variables.contains(&codec) {
                variables.push(codec);
            }
            (bytes, codec, origins.shared_display(id, bytes))
        },
        |part| failures.push(part.diagnostic_detail()),
        true,
    );
    for part in &failures {
        host.data_services.note_text_decode(host.events, host.frame, format!(
            "FES block={:?} offset={:#x} {part}",
            String::from_utf8_lossy(block), command.offset,
        ));
    }
    if let Some(part) = failures.first() && eden_diagnostic_fes(host, driver) == Some(true) {
        let resolved_cst = match host.cst_resource_with_source(b"scene", b"BLAOP1_END.cst") {
            Ok(Some((_, resource))) if resource.bytes.len() == 26298
                && super::particle_plugin_digest::sha256(&resource.bytes)
                    == "c09bcb90152977cd9d21b68ec019d627ccc80136036503a5ce376a2ae6020561" => "v1-patch",
            Ok(Some(_)) => "other",
            Ok(None) => "missing",
            Err(_) => "lookup-error",
        };
        host.events.push(LogicEvent {
            frame: host.frame,
            kind: "fes-text-eden-decode-error".into(),
            detail: format!("task={owner} resource=meswnd.fes block={} offset={:#x} resolved_cst={resolved_cst} {part}",
                String::from_utf8_lossy(block), command.offset),
        });
    }
    Ok(decoded?.map(|text| SourcedText { text, literal, variables }))
}

fn eden_diagnostic_fes(host: &LoggingHost<'_>, driver: &FesScriptDriver) -> Option<bool> {
    const MESWND_SIZE: usize = 4971;
    if host.css_task_sha() != Some("c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c") {
        return None;
    }
    let names = host.resources.category_names(b"fes", "meswnd.fes").ok()?;
    let candidate = names.iter().find_map(|name| host.resources.find(name))?;
    if candidate.size != MESWND_SIZE as u64 { return None; }
    let bytes = host.resources.read_candidate_bounded(candidate, MESWND_SIZE).ok()??;
    if super::particle_plugin_digest::sha256(&bytes)
        != "24d7b9d39471a48939c6ad5329e5f21d2431d8cc4b2168992c1e9cf0ee9ee205" {
        return None;
    }
    let document = cs2_script::FesDocument::parse(&bytes).ok()?;
    let program = cs2_script::FesProgram::from_document(&document).ok()?;
    Some(program == driver.interpreter.program)
}

struct SourcedPart<'a> {
    bytes: &'a [u8],
    codec: ScriptTextEncoding,
    variable: Option<(u32, bool)>,
    display: Option<&'a str>,
}

impl SourcedPart<'_> {
    fn diagnostic_detail(&self) -> String {
        let source = match self.variable {
            None => "literal".into(),
            Some((id, true)) => format!("shared-tagged variable={id}"),
            Some((id, false)) => format!("shared-default variable={id}"),
        };
        let bad_at = malformed_text_offset(self.bytes, self.codec)
            .map_or_else(|| "unknown".into(), |offset| offset.to_string());
        format!("source={source} codec={:?} bytes={} bad_at={bad_at}", self.codec, self.bytes.len())
    }
}

fn malformed_text_offset(bytes: &[u8], codec: ScriptTextEncoding) -> Option<usize> {
    let encoding = match codec {
        ScriptTextEncoding::Utf8 => return std::str::from_utf8(bytes).err().map(|err| err.valid_up_to()),
        ScriptTextEncoding::Gbk => encoding_rs::GBK,
        ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS,
    };
    let mut base = 0;
    let mut output = [0; 256];
    for part in bytes.split(|&byte| byte == 0xff) {
        let mut decoder = encoding.new_decoder_without_bom_handling();
        let mut consumed = 0;
        loop {
            let (result, read, _) = decoder.decode_to_utf8_without_replacement(&part[consumed..], &mut output, true);
            consumed += read;
            match result {
                encoding_rs::DecoderResult::InputEmpty => break,
                encoding_rs::DecoderResult::OutputFull => continue,
                encoding_rs::DecoderResult::Malformed(length, after) => {
                    return consumed.checked_sub(usize::from(length) + usize::from(after))
                        .map(|offset| base + offset);
                }
            }
        }
        base += part.len() + 1;
    }
    None
}

#[cfg(test)]
pub(super) fn decode_sourced_text<'a>(
    template: &'a [u8], expected: &[u8], literal: ScriptTextEncoding,
    default_variable_codec: ScriptTextEncoding, sourced: bool,
    variable: impl FnMut(u32) -> (&'a [u8], Option<ScriptTextEncoding>),
) -> Result<Option<String>> {
    let mut variable = variable;
    decode_sourced_text_with_failure(template, expected, literal, default_variable_codec, sourced, |id| {
        let (bytes, codec) = variable(id);
        (bytes, codec, None)
    }, |_| {}, false)
}

fn decode_sourced_text_with_failure<'a>(
    mut template: &'a [u8], expected: &[u8], literal: ScriptTextEncoding,
    default_variable_codec: ScriptTextEncoding, mut sourced: bool,
    mut variable: impl FnMut(u32) -> (&'a [u8], Option<ScriptTextEncoding>, Option<&'a str>),
    mut on_failure: impl FnMut(SourcedPart<'a>), recover: bool,
) -> Result<Option<String>> {
    let literal_part = |bytes: &'a [u8]| SourcedPart { bytes, codec: literal, variable: None, display: None };
    let mut parts = Vec::new();
    while let Some(start) = template.windows(4).position(|part| part == b"$str") {
        let digits = &template[start + 4..];
        let count = digits.iter().take_while(|byte| byte.is_ascii_digit()).count();
        if count == 0 {
            parts.push(literal_part(&template[..start + 4]));
            template = digits;
            continue;
        }
        parts.push(literal_part(&template[..start]));
        let id = digits[..count].iter().fold(0u32, |id, byte| {
            id.wrapping_mul(10).wrapping_add(u32::from(*byte - b'0'))
        });
        let (bytes, codec, display) = variable(id);
        sourced |= codec.is_some() || display.is_some();
        parts.push(SourcedPart {
            bytes, codec: codec.unwrap_or(default_variable_codec), variable: Some((id, codec.is_some())), display,
        });
        template = &digits[count + usize::from(digits.get(count) == Some(&b';'))..];
    }
    parts.push(literal_part(template));
    if !sourced || !parts.iter().flat_map(|part| part.bytes.iter()).eq(expected) {
        return Ok(None);
    }
    let mut text = String::new();
    for part in parts {
        if let Some(display) = part.display {
            text.push_str(display);
            continue;
        }
        match decode_text(part.bytes, part.codec) {
            Ok(decoded) => text.push_str(&decoded),
            Err(error) => {
                if !recover {
                    on_failure(part);
                    return Err(error);
                }
                text.push_str(&part.codec.decode_message(part.bytes).0);
                on_failure(part);
            }
        }
    }
    Ok(Some(text))
}

#[test]
fn fes_sourced_failure_reports_only_the_rejected_segment() {
    let cp932 = ScriptTextEncoding::Cp932;
    let mut details = Vec::new();
    let result = decode_sourced_text_with_failure(
        b"\x81\x79$str1000;$str1001;\x81\x7a", b"\x81\x79A\xff\xec\xcdB\x81\x7a",
        cp932, cp932, false,
        |id| match id {
            1000 => (b"A\xff\xec\xcd", None, None),
            1001 => (b"B", Some(cp932), None),
            _ => unreachable!(),
        },
        |part| details.push(part.diagnostic_detail()), false,
    );
    assert_eq!(result.unwrap_err().to_string(), "invalid Cp932 FES text from declared source");
    assert_eq!(details, ["source=shared-default variable=1000 codec=Cp932 bytes=4 bad_at=2"]);
    assert_eq!(malformed_text_offset(&[vec![b'A'; 512], vec![0x81]].concat(), cp932), Some(512));
}

#[test]
fn fes_sourced_cp932_keeps_legacy_placeholder_and_raw_append() -> Result<()> {
    let cp932 = ScriptTextEncoding::Cp932;
    let raw = b"\\cd\x81\x75\x81\x01\x81\x01\x81\x76";
    let decoded = decode_sourced_text(b"$str1000", raw, cp932, cp932, false,
        |_| (raw, Some(cp932)))?.unwrap();
    assert_eq!(decoded, "\\cd\u{300c}\u{30fb}\u{30fb}\u{300d}");
    let mut device = DesktopMessageDevice::default();
    let mut message = MessageObject::new(-1, MessageEncoding::Cp932);
    let configuration = initial_configuration(MessageEncoding::Cp932, &defaults(), 0)?;
    message.reconfigure(&configuration, None, &mut device)?;
    message.append_decoded(raw, &decoded)?;
    assert_eq!(message.source(), decoded);
    assert_eq!(message.raw_appends(), &[raw.to_vec()]);
    assert!(decode_sourced_text(b"$str1000", raw, cp932, cp932, false,
        |_| (raw, Some(ScriptTextEncoding::Gbk))).is_err());
    Ok(())
}

#[test]
fn fes_sourced_text_keeps_cp932_literals_and_gbk_and_utf8_variables() {
    let raw = b"\x81\x69\xd6\xd0\x81\x6a\xe6\x97\xa5\xff";
    let decode = |expected: &[u8]| decode_sourced_text(
        b"\x81\x69$str1000;\x81\x6a$str1001;\xff", expected,
        ScriptTextEncoding::Cp932, ScriptTextEncoding::Cp932, false,
        |id| match id {
            1000 => (b"\xd6\xd0", Some(ScriptTextEncoding::Gbk)),
            1001 => (b"\xe6\x97\xa5", Some(ScriptTextEncoding::Utf8)),
            _ => unreachable!(),
        },
    );
    let text = decode(raw).unwrap().unwrap();
    assert_eq!(text, "\u{ff08}\u{4e2d}\u{ff09}\u{65e5}\u{ffff}");
    let defaults = defaults();
    let mut device = DesktopMessageDevice::default();
    let mut message = MessageObject::new(-1, MessageEncoding::Cp932);
    let configuration = initial_configuration(MessageEncoding::Cp932, &defaults, 0).unwrap();
    message.reconfigure(&configuration, None, &mut device).unwrap();
    message.append_decoded(raw, &text).unwrap();
    assert_eq!(message.source(), text);
    assert_eq!(message.raw_appends(), &[raw.to_vec()]);
    assert!(decode(&raw[..raw.len() - 1]).unwrap().is_none());
    assert!(decode_sourced_text(b"$str1000", b"\x81", ScriptTextEncoding::Cp932,
        ScriptTextEncoding::Cp932, false,
        |_| (b"\x81", Some(ScriptTextEncoding::Gbk))).is_err());
    assert_eq!(decode_sourced_text(b"\xd6\xd0$str1000", b"\xd6\xd0\x81\x69",
        ScriptTextEncoding::Gbk, ScriptTextEncoding::Cp932, true,
        |_| (b"\x81\x69", None)).unwrap().as_deref(), Some("\u{4e2d}\u{ff08}"));
}

#[test]
fn fes_text_apend_keeps_speaker_and_body_byte_sources() {
    let mut store = FesTextStore::default();
    let mut device = DesktopMessageDevice::default();
    let kind = b"NVSTRING".to_vec();
    store.create(12, &FesObjectDefinition {
        offset: 0,
        name: kind.clone(),
        kind: kind.clone(),
        bank_expression: None,
        banks: 1,
    }, &defaults(), &mut device).unwrap();
    let bank = store.banks.get_mut(&(12, kind, 0)).unwrap();
    let ambiguous = b"\xc2\xa9";
    let body = decode_sourced_text(b"$str1000", ambiguous, ScriptTextEncoding::Cp932,
        ScriptTextEncoding::Cp932, false, |_| (ambiguous, Some(ScriptTextEncoding::Utf8)))
        .unwrap().unwrap();

    bank.set_speaker(b"\\pr\xc2\xa9", ScriptTextEncoding::Cp932, None).unwrap();
    bank.append_body_text(ambiguous, true, Some(&body)).unwrap();
    let Some(TextRequest::AppendDecoded { bytes, text }) = bank.pending.pop_back() else {
        panic!("missing sourced paragraph");
    };
    assert_eq!(bytes, b"\xc2\xa9\n\xc2\xa9");
    assert_eq!(text, "\u{6f0f}\n\u{a9}");

    bank.set_speaker(b"\\pr\xc2\xa9", ScriptTextEncoding::Cp932, Some("\\pr\u{a9}")).unwrap();
    bank.append_body(ambiguous, true).unwrap();
    let Some(TextRequest::AppendDecoded { bytes, text }) = bank.pending.pop_back() else {
        panic!("missing paragraph with sourced speaker");
    };
    assert_eq!(bytes, b"\xc2\xa9\n\xc2\xa9");
    assert_eq!(text, "\u{a9}\n\u{6f0f}");
    assert!(bank.novel.as_ref().unwrap().speaker.is_empty());
}

#[test]
fn fes_sourced_trace_omits_text_and_skips_leading_message_controls() {
    let decoded = SourcedText {
        text: "\u{ffff}\\n\\pc\u{8d77}private text".into(),
        literal: ScriptTextEncoding::Cp932,
        variables: vec![ScriptTextEncoding::Gbk, ScriptTextEncoding::Utf8],
    };
    assert_eq!(decoded.trace_detail(24), "bytes=24 first=U+8D77 fes=Cp932 cst=[Gbk, Utf8]");
    assert_eq!(first_text_codepoint("\n\\r"), None);
    assert_eq!(first_text_codepoint("\\nA\u{8d77}"), Some('A'));
}

#[test]
fn fes_unicode_input_honors_declared_gbk_even_when_also_valid_utf8() {
    let bytes = b"\xc2\xa9";
    assert_eq!(normalize_text(bytes, MessageEncoding::Unicode, false, ScriptTextEncoding::Gbk).unwrap(),
        "\u{6f0f}".as_bytes());
    assert_eq!(normalize_text(bytes, MessageEncoding::Unicode, false, ScriptTextEncoding::Utf8).unwrap(), bytes);
    assert!(normalize_text(b"\x81", MessageEncoding::Unicode, false, ScriptTextEncoding::Gbk).is_err());
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
fn decode_layout_font(
    name: &[u8],
    resource_encoding: ScriptTextEncoding,
    offset: usize,
    host: &mut LoggingHost<'_>,
) -> String {
    if let Ok(text) = std::str::from_utf8(name) {
        return text.to_owned();
    }
    if let Some(text) = resource_encoding.decode_strict(name) {
        return text.into_owned();
    }
    if resource_encoding != ScriptTextEncoding::Cp932
        && let Some(text) = ScriptTextEncoding::Cp932.decode_strict(name)
    {
        return text.into_owned();
    }
    let (text, bad_at) = resource_encoding.decode_message(name);
    if let Some(bad_at) = bad_at {
        host.data_services.note_text_decode(
            host.events,
            host.frame,
            format!("STRING layout font offset={offset:#x} encoding={resource_encoding:?} bad_at={bad_at}"),
        );
    }
    text
}
fn normalize_text(raw: &[u8], encoding: MessageEncoding, sjis: bool, resource: ScriptTextEncoding) -> Result<Vec<u8>> {
    if sjis {
        if encoding == MessageEncoding::Cp932 {
            return Ok(raw.to_vec());
        }
        let text = cs2_runtime::message_object::decode_cp932_display(raw)
            .context("STRING strsj input is invalid CP932")?;
        return Ok(text.as_bytes().to_vec());
    }
    if encoding == MessageEncoding::Unicode {
        if resource == ScriptTextEncoding::Gbk {
            let text = encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(raw)
                .context("STRING8 text has invalid GBK resource bytes")?;
            return Ok(text.as_bytes().to_vec());
        }
        if let Err(error) = std::str::from_utf8(raw) {
            if resource == ScriptTextEncoding::Cp932
                && let Some(text) = cs2_runtime::message_object::decode_cp932_display(raw)
            {
                return Ok(text.into_owned().into_bytes());
            }
            return Err(error).context("STRING8 source is not valid UTF-8");
        }
    }
    Ok(raw.to_vec())
}

#[test]
fn fes_unicode_input_accepts_legacy_text_from_cp932_resources() -> Result<()> {
    let message = MessageEncoding::Unicode;
    let cp932 = ScriptTextEncoding::Cp932;
    let legacy = b"\x81\x75\x82\xa0\x81\x76";
    assert_eq!(normalize_text(legacy, message, false, cp932)?, "\u{300c}\u{3042}\u{300d}".as_bytes());
    assert_eq!(normalize_text(b"\xc2\xa9", message, false, cp932)?, b"\xc2\xa9");
    assert!(normalize_text(legacy, message, false, ScriptTextEncoding::Utf8).is_err());
    assert!(normalize_text(b"\x81", message, false, cp932).is_err());
    Ok(())
}

#[test]
fn fes_strsj_uses_the_target_message_encoding() {
    let sample = b"\x83\x81\x83b\x83Z\x81[\x83W";
    let legacy = normalize_text(sample, MessageEncoding::Cp932, true, ScriptTextEncoding::Gbk).unwrap();
    let unicode = normalize_text(sample, MessageEncoding::Unicode, true, ScriptTextEncoding::Gbk).unwrap();
    assert_eq!(legacy, sample);
    assert_eq!(str::from_utf8(&unicode).unwrap(), "\u{30e1}\u{30c3}\u{30bb}\u{30fc}\u{30b8}");
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
