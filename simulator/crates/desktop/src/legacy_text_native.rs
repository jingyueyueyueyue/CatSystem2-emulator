//! Old kcTaskStringPlane (factory kind 8), commands 102/104/105 and adjacent
//! proven controls. This owns distinct CP932 records, layout and pixel surfaces;
//! it is NOT an alias of the 455-series MessageObject protocol.
//!
//! Evidence: recovery_A_20261116_contracts plus static PE reads recorded in E's
//! report. No original machine code or third-party implementation is copied.

use std::collections::BTreeMap;

use cs2_renderer::{Canvas, Color, GlyphEffects, RgbaFrame};
use cs2_runtime::message_object::{
    MessageConfiguration, MessageDefaults, MessageEncoding, MessageUserGlyph,
};
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

use super::{
    SceneNode, expand_kcs_format, message_backend::DesktopMessageDevice,
    message_native::MessageNativeContext,
};

pub(super) const LEGACY_STRING_KIND: u32 = 8;
/// (ordinary slot, required DWORD prefix, receiver command). Each wrapper
/// shares the same argument validation and the independently typed receiver.
const LEGACY_NATIVES: &[(u32, usize, u32)] = &[
    (123, 3, 102),
    (124, 2, 104),
    (125, 1, 105),
    (126, 3, 107),
    (127, 2, 108),
    (128, 2, 106),
    (129, 1, 109),
    (130, 1, 110),
    (131, 2, 111),
    (132, 1, 113),
    (133, 2, 114),
    (136, 2, 117),
    (137, 6, 118),
    (138, 3, 119),
    (139, 2, 112),
    (418, 4, 102),
    (456, 2, 121),
    (557, 2, 122),
    (585, 2, 123),
    (599, 6, 124),
];
const MAX_RECORDS: usize = 1_000_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[derive(Clone, Debug)]
pub(super) struct LegacyTextConfiguration {
    /// The wrapper copies exactly 0x60 bytes; it has no size tag.
    pub(super) bytes: Vec<u8>,
    /// Command 102's first CP932 character set, from native 123's third argument.
    pub(super) no_start: Vec<u16>,
    pub(super) no_end: Vec<u16>,
    /// Exact command-102 payload, including count/table/zero fields.
    pub(super) command_record: Vec<u8>,
}

impl LegacyTextConfiguration {
    fn value(&self, offset: usize) -> u32 {
        u32::from_le_bytes(self.bytes[offset..offset + 4].try_into().unwrap())
    }
    fn size(&self) -> [u32; 2] {
        [self.value(8), self.value(12)]
    }
    fn origin(&self) -> [i32; 2] {
        [self.value(0x2c) as i32, self.value(0x30) as i32]
    }
    pub(super) fn submit(&self) -> bool {
        self.value(0x14) != 0
    }
    fn position(&self) -> [i32; 2] {
        [self.value(0) as i32, self.value(4) as i32]
    }
    fn set_value(&mut self, offset: usize, value: u32) {
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        self.command_record[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}

#[derive(Clone, Debug)]
pub(super) struct LegacyTextRecord {
    /// Original CP932 number, 0 newline, 0xffff separator.
    pub(super) code: u16,
    pub(super) character: Option<char>,
    pub(super) color: u32,
    pub(super) size: u32,
    pub(super) position: [i32; 2],
    pub(super) advance: i32,
    pub(super) ruby: bool,
    pub(super) anchor: Option<usize>,
    pub(super) complete: bool,
    pub(super) deadline: u32,
}

#[derive(Clone, Debug)]
pub(super) struct LegacyTextObject {
    pub(super) id: u32,
    pub(super) configuration: LegacyTextConfiguration,
    pub(super) records: Vec<LegacyTextRecord>,
    pub(super) raw_appends: Vec<Vec<u8>>,
    pub(super) cursor: [i32; 2],
    pub(super) current_size: u32,
    pub(super) current_color: u32,
    pub(super) shadow_color: u32,
    pub(super) hanging_indent: u32,
    pub(super) presets: [u32; 5],
    pub(super) markup: u32,
    pub(super) mode: u32,
    pub(super) character_delay: u32,
    pub(super) segment_limit: u32,
    pub(super) vertical_alignment: u32,
    pub(super) line_spacing: i32,
    pub(super) complete: bool,
    pub(super) playback_active: bool,
    pub(super) playback_cursor: usize,
    pub(super) scheduled: usize,
    pub(super) last_elapsed: u32,
    pub(super) time_base: u32,
    immediate_paint_pending: bool,
    force_paint_pending: bool,
    was_visible: bool,
    indent_active: bool,
    glyph_source: Option<RgbaFrame>,
    glyph_rectangles: BTreeMap<u16, [u32; 4]>,
    font_request: MessageConfiguration,
    ruby_font_request: MessageConfiguration,
    surface: Canvas,
    submitted: Canvas,
}

#[derive(Clone, Debug, Default)]
pub(super) struct LegacyTextNativeServices {
    objects: BTreeMap<u32, LegacyTextObject>,
    constructor_controls: BTreeMap<u32, LegacyConstructorControls>,
}

/// These commands also write real constructor fields before command102
/// allocates records. Configuration replaces color/presets, while shadow
/// color and the hanging-indent switch survive.
#[derive(Clone, Debug)]
struct LegacyConstructorControls {
    shadow_color: u32,
    hanging_indent: u32,
    color: u32,
    presets: [u32; 5],
}

impl Default for LegacyConstructorControls {
    fn default() -> Self {
        Self {
            shadow_color: 0xff00_0000,
            hanging_indent: 0,
            color: 0,
            presets: [20; 5],
        }
    }
}

impl LegacyTextNativeServices {
    pub(super) fn supports(index: u32) -> bool {
        LEGACY_NATIVES.iter().any(|spec| spec.0 == index)
    }
    pub(super) fn paints_after_call(index: u32) -> bool {
        LEGACY_NATIVES
            .iter()
            .any(|spec| spec.0 == index && matches!(spec.2, 104 | 110 | 114))
    }
    pub(super) fn object(&self, id: u32) -> Option<&LegacyTextObject> {
        self.objects.get(&id)
    }
    pub(super) fn objects(&self) -> &BTreeMap<u32, LegacyTextObject> {
        &self.objects
    }
    pub(super) fn release(&mut self, id: u32) {
        self.objects.remove(&id);
        self.constructor_controls.remove(&id);
    }
    /// Called for every successful plane47/48 reception, including repeated
    /// values. This runs before returning to the script, rather than inferring
    /// command events from a changed boolean during the next frame.
    pub(super) fn display_command(
        &mut self,
        id: u32,
        value: u32,
        inherited: bool,
        self_show: u32,
        visible: bool,
        elapsed: u32,
        device: &DesktopMessageDevice,
    ) -> Result<()> {
        let Some(object) = self.objects.get_mut(&id) else {
            return Ok(());
        };
        object.was_visible = visible;
        if value != 0 && (!inherited || self_show != 0) {
            if visible {
                object.schedule(elapsed, Some(60))?;
                // Mode1 prepares the already reached prefix immediately;
                // subsequent records use the saved task-clock deadlines.
                object.flush_pending(device)?;
            } else {
                object.playback_active = false;
            }
        }
        Ok(())
    }

    pub(super) fn call(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
        nodes: &BTreeMap<u32, SceneNode>,
        defaults: Option<&MessageDefaults>,
        context: &mut impl MessageNativeContext,
    ) -> Result<NativeReturn> {
        let &(_, words, command) = LEGACY_NATIVES
            .iter()
            .find(|spec| spec.0 == index)
            .ok_or(ScriptError::UnimplementedNativeCall { index })?;
        if arguments.len() < words * 4 || arguments.len() % 4 != 0 {
            return Err(invalid(
                "legacy StringPlane native is missing its documented DWORD prefix",
            ));
        }
        // These old wrappers read fixed words, not an argument-count field.
        // MesWnd.kcs@0x16a8 in the 2.5.2 sample declares native126 with sixteen
        // bytes; rejecting a valid extra stack word would interrupt its normal
        // mode/interval update. The VM owns/pops the entire supplied argument
        // region; the receiver interprets only its known command prefix.
        let word = |index: usize| {
            u32::from_le_bytes(arguments[index * 4..index * 4 + 4].try_into().unwrap())
        };
        if command == 117 && word(1) == 0 {
            return Ok(NativeReturn::Continue);
        }
        // The old wrapper resolves its image before sending the command.
        let source = if command == 117 {
            Some(context.message_image(word(1))?)
        } else {
            None
        };
        let text = if index == 124 {
            let input = vm.read_string(word(1))?;
            let formatted = expand_kcs_format(vm, &input, 0)?;
            let length = formatted
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(formatted.len());
            if length > MAX_BYTES {
                return Err(invalid("legacy StringPlane formatted text exceeds 64 MiB"));
            }
            Some(formatted[..length].to_vec())
        } else {
            None
        };
        // Both old configuration wrappers package their buffers before
        // routing. Native418 also carries the second prohibited-character set.
        let configuration = if command == 102 {
            let bytes = vm.read(word(1), 0x60)?;
            let no_start = cp932_codes(&vm.read_string(word(2))?)?;
            let no_end = if index == 418 {
                cp932_codes(&vm.read_string(word(3))?)?
            } else {
                Vec::new()
            };
            if no_start.len() > 256 || no_end.len() > 256 {
                return Err(invalid(
                    "legacy StringPlane prohibited-character collection exceeds 256 entries",
                ));
            }
            let mut command_record = vec![0; 0x468];
            command_record[..0x60].copy_from_slice(&bytes);
            command_record[0x60..0x64].copy_from_slice(&(no_start.len() as u32).to_le_bytes());
            for (index, code) in no_start.iter().enumerate() {
                command_record[0x64 + index * 2..0x66 + index * 2]
                    .copy_from_slice(&code.to_le_bytes());
            }
            // For 256 entries the terminal shares this documented zero field.
            command_record[0x264..0x268].copy_from_slice(&(no_end.len() as u32).to_le_bytes());
            for (index, code) in no_end.iter().enumerate() {
                command_record[0x268 + index * 2..0x26a + index * 2]
                    .copy_from_slice(&code.to_le_bytes());
            }
            Some(LegacyTextConfiguration {
                bytes,
                no_start,
                no_end,
                command_record,
            })
        } else {
            None
        };
        let route = context
            .route_message(word(0))?
            .filter(|route| {
                route.deletion == 0
                    && nodes
                        .get(&route.id)
                        .is_some_and(|node| node.kind == LEGACY_STRING_KIND)
            })
            .ok_or_else(|| {
                invalid("legacy StringPlane native requires a reachable live kind-8 receiver")
            })?;
        if let Some(configuration) = configuration {
            if configuration.value(0x3c) == 0 {
                return Ok(NativeReturn::Continue);
            }
            let defaults = defaults
                .ok_or_else(|| invalid("legacy StringPlane global font settings are missing"))?;
            let mut object = LegacyTextObject::new(
                route.id,
                configuration,
                defaults,
                &context.message_ruby_font_name(),
            )?;
            if let Some(old) = self.objects.get(&route.id) {
                object.shadow_color = old.shadow_color;
                object.hanging_indent = old.hanging_indent;
                object.glyph_source = old.glyph_source.clone();
                object.glyph_rectangles = old.glyph_rectangles.clone();
            } else if let Some(controls) = self.constructor_controls.remove(&route.id) {
                object.shadow_color = controls.shadow_color;
                object.hanging_indent = controls.hanging_indent;
            }
            self.objects.insert(route.id, object);
            return Ok(NativeReturn::Continue);
        }
        if !self.objects.contains_key(&route.id) && matches!(command, 121..=124) {
            let controls = self.constructor_controls.entry(route.id).or_default();
            match command {
                121 => controls.hanging_indent = word(1),
                122 => controls.shadow_color = word(1),
                123 => controls.color = word(1),
                124 => controls.presets = std::array::from_fn(|n| word(n + 1)),
                _ => unreachable!(),
            }
            return Ok(NativeReturn::Continue);
        }
        // The identified receiver's parser/clear branches explicitly skip an
        // unallocated record array. This typed no-write is evidenced, unlike
        // pretending a missing or wrong-type task accepted the command.
        if !self.objects.contains_key(&route.id) && matches!(index, 124 | 125) {
            return Ok(NativeReturn::Continue);
        }
        if !self.objects.contains_key(&route.id) && index == 129 {
            // ctor explicitly initializes object+0x740 to zero; command109
            // returns its zero comparison even before command102 allocates.
            return Ok(NativeReturn::Value(1));
        }
        let object = self
            .objects
            .get_mut(&route.id)
            .ok_or_else(|| invalid("legacy StringPlane has no command-102 record allocation"))?;
        match command {
            104 => {
                let (ordinary_record_added, bad_at) = object.append(text.as_ref().unwrap())?;
                if let Some(offset) = bad_at {
                    context.note_legacy_text_decode(offset);
                }
                object.immediate_paint_pending |= ordinary_record_added && object.mode != 1;
            }
            105 => object.clear(),
            107 => {
                // Mode 1 has its own task-clock schedule; retaining it is
                // preferable to aliasing the new 467 speed/deadline machinery.
                object.mode = word(1);
                object.character_delay = word(2);
                object.scheduled = 0;
                // Command107 resets +0x738 but does not write +0x740.
                object.playback_cursor = 0;
            }
            108 => object.line_spacing = word(1) as i32,
            106 => object.markup = word(1),
            109 => return Ok(NativeReturn::Value(u32::from(!object.playback_active))),
            110 => {
                if route.display != cs2_runtime::message_object::MessagePlaneDisplay::Unreachable {
                    object.force_finish()?;
                }
            }
            111 => {
                object.current_size = word(1);
                object.configuration.set_value(0x18, word(1));
            }
            113 => object.separator()?,
            114 => {
                if object.segment_limit != u32::MAX {
                    object.segment_limit = object.segment_limit.wrapping_add(word(1));
                }
                if route.display == cs2_runtime::message_object::MessagePlaneDisplay::Visible {
                    object.schedule(route.elapsed, Some(context.legacy_frame_rate()))?;
                    object.was_visible = true;
                }
            }
            117 => object.glyph_source = source,
            118 => {
                // The original ordered-map insertion keeps the first value
                // for a repeated code; it does not replace an existing region.
                object
                    .glyph_rectangles
                    .entry(word(1) as u16)
                    .or_insert_with(|| std::array::from_fn(|n| word(n + 2)));
            }
            119 => object.cursor = [word(1) as i32, word(2) as i32],
            // Old command112 is vertical post-parse alignment, not new speed.
            112 => {
                object.vertical_alignment = word(1);
                object.align_records();
            }
            121 => object.hanging_indent = word(1),
            122 => object.shadow_color = word(1),
            123 => {
                object.current_color = word(1);
                object.configuration.set_value(0x40, word(1));
            }
            124 => object.presets = std::array::from_fn(|n| word(n + 1)),
            _ => unreachable!(),
        }
        Ok(NativeReturn::Continue)
    }

    /// Real CPU preparation/render follows successful append unless mode==1.
    /// The platform calls this before the native returns and again per frame.
    pub(super) fn paint_pending(&mut self, id: u32, device: &DesktopMessageDevice) -> Result<()> {
        if let Some(object) = self.objects.get_mut(&id) {
            object.flush_pending(device)?;
        }
        Ok(())
    }

    pub(super) fn frame_update(
        &mut self,
        id: u32,
        elapsed: u32,
        visible: bool,
        time_base: Option<u32>,
        device: &DesktopMessageDevice,
    ) -> Result<()> {
        if let Some(object) = self.objects.get_mut(&id) {
            object.last_elapsed = elapsed;
            if visible && !object.was_visible {
                object.schedule(elapsed, time_base)?;
            }
            object.flush_pending(device)?;
            if object.mode == 1 && object.playback_active {
                object.paint(device, elapsed, false)?;
            }
            object.was_visible = visible;
        }
        Ok(())
    }
}

impl LegacyTextObject {
    fn new(
        id: u32,
        configuration: LegacyTextConfiguration,
        defaults: &MessageDefaults,
        ruby_font_name: &[u16],
    ) -> Result<Self> {
        let [width, height] = configuration.size();
        if width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(invalid(
                "legacy StringPlane signed canvas dimensions are negative",
            ));
        }
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|&n| n <= MAX_BYTES)
            .ok_or_else(|| invalid("legacy StringPlane surface exceeds 64 MiB"))?;
        let _ = bytes;
        if configuration.value(0x3c) as usize > MAX_RECORDS {
            return Err(invalid(
                "legacy StringPlane character capacity exceeds storage limit",
            ));
        }
        let surface = Canvas::new(width, height)
            .map_err(|_| invalid("legacy StringPlane pixel surface allocation failed"))?;
        let current_size = configuration.value(0x18);
        let current_color = configuration.value(0x40);
        let cursor = configuration.origin();
        let mode = configuration.value(0x44);
        let character_delay = configuration.value(0x48);
        let vertical_alignment = configuration.value(0x4c);
        let markup = configuration.value(0x50);
        let line_spacing = configuration.value(0x24) as i32;
        // Reuse only the device font request, with 64 UTF-16 name capacity.
        // No message page, native slot, source or playback state is shared.
        let mut font_defaults = defaults.clone();
        // Original 0x13c721 jump table: style0/unknown clears both flags,
        // style1 enables outline, style2/3 enables the alternate shadow path.
        // They are object-local flags, not the current global shadow setting.
        let style_mode = configuration.value(0x54);
        font_defaults.outline = u32::from(style_mode == 1);
        font_defaults.shadow = u32::from(matches!(style_mode, 2 | 3));
        let font_request = font_defaults.configuration(MessageEncoding::Unicode)?;
        // Old command102 constructs a separate ruby glyph service when the
        // global ruby name is nonempty; an empty name shares the body service.
        let ruby_font_request = if ruby_font_name.is_empty() {
            font_request.clone()
        } else {
            font_defaults.font_name = ruby_font_name.to_vec();
            font_defaults.configuration(MessageEncoding::Unicode)?
        };
        Ok(Self {
            id,
            configuration,
            records: Vec::new(),
            raw_appends: Vec::new(),
            cursor,
            current_size,
            current_color,
            shadow_color: 0xff00_0000,
            hanging_indent: 0,
            presets: [current_size; 5],
            markup,
            mode,
            character_delay,
            segment_limit: 0,
            vertical_alignment,
            line_spacing,
            complete: true,
            playback_active: false,
            playback_cursor: 0,
            scheduled: 0,
            last_elapsed: 0,
            time_base: 0,
            immediate_paint_pending: false,
            force_paint_pending: false,
            was_visible: false,
            indent_active: false,
            glyph_source: None,
            glyph_rectangles: BTreeMap::new(),
            font_request,
            ruby_font_request,
            submitted: surface.clone(),
            surface,
        })
    }

    fn clear(&mut self) {
        self.records.clear();
        self.raw_appends.clear();
        self.cursor = self.configuration.origin();
        self.current_color = self.configuration.value(0x40);
        self.segment_limit = 0;
        self.complete = true;
        self.playback_cursor = 0;
        self.scheduled = 0;
        self.immediate_paint_pending = false;
        self.force_paint_pending = false;
        self.indent_active = false;
        self.surface.clear(Color::rgba(0, 0, 0, 0));
        if self.configuration.submit() {
            self.submitted = self.surface.clone();
        }
    }

    pub(super) fn remaining_records(&self) -> usize {
        self.records
            .iter()
            .filter(|record| record.character.is_some() && !record.complete)
            .count()
    }
    pub(super) fn surface(&self) -> &Canvas {
        &self.surface
    }
    pub(super) fn submitted(&self) -> &Canvas {
        &self.submitted
    }

    fn push_record(&mut self, code: u16, character: Option<char>, ruby: bool) -> Result<bool> {
        if self.records.len().saturating_add(1) >= self.configuration.value(0x3c) as usize {
            return Ok(false);
        }
        let size = i32::try_from(self.current_size)
            .map_err(|_| invalid("legacy StringPlane font size exceeds i32"))?;
        let advance = if code & 0xff00 != 0 {
            size
        } else {
            size.wrapping_sub(3) / 2 + 4
        };
        if self.hanging_indent != 0 && self.records.is_empty() && code == 0x8175 {
            self.indent_active = true;
        }
        let width = self.configuration.size()[0] as i32;
        if character.is_some() && !ruby && self.configuration.value(0x58) != 0 {
            let near_edge = self.cursor[0]
                .wrapping_add((self.configuration.value(0x5c) as i32).wrapping_mul(size))
                .wrapping_add(advance)
                >= width;
            if near_edge {
                let prohibited = self.configuration.no_start.contains(&code)
                    || self.configuration.no_end.contains(&code);
                if !prohibited {
                    if !self.push_record(0, None, false)? {
                        return Ok(false);
                    }
                } else if self.cursor[0].wrapping_add(advance) > width {
                    let mut split = self.records.len();
                    // Move a contiguous prohibited line-ending suffix onto
                    // the next line, preserving raw record order and state.
                    while split > 0
                        && self
                            .configuration
                            .no_end
                            .contains(&self.records[split - 1].code)
                    {
                        split -= 1;
                    }
                    if split == 0 {
                        split = self.records.len();
                    }
                    if !self.insert_line_break(split, size)? {
                        return Ok(false);
                    }
                }
            }
        }
        if character.is_some() && !ruby && self.cursor[0].wrapping_add(advance) > width {
            if !self.push_record(0, None, false)? {
                return Ok(false);
            }
        }
        if character.is_some()
            && !ruby
            && self.cursor[1]
                .wrapping_add(size)
                .wrapping_add(self.line_spacing)
                > self.configuration.size()[1] as i32
        {
            return Ok(false);
        }
        let record = LegacyTextRecord {
            code,
            character,
            color: if code == 0 { 0 } else { self.current_color },
            size: if code == 0 { 0 } else { self.current_size },
            position: if code == 0 { [0; 2] } else { self.cursor },
            advance: if character.is_some() { advance } else { 0 },
            ruby,
            anchor: None,
            complete: character.is_none(),
            deadline: 0,
        };
        self.records.push(record);
        if code == 0 {
            self.cursor = [
                if self.indent_active {
                    size.wrapping_add(self.configuration.value(0x20) as i32)
                } else {
                    0
                },
                self.cursor[1]
                    .wrapping_add(size)
                    .wrapping_add(self.line_spacing),
            ];
        } else if code != 0xffff && !ruby {
            self.cursor[0] = self.cursor[0]
                .wrapping_add(advance)
                .wrapping_add(self.configuration.value(0x20) as i32);
        }
        if character.is_some() {
            self.complete = false;
        }
        if code == 0 && self.records.len() + 1 >= self.configuration.value(0x3c) as usize {
            // The newline itself remains registered when the reserved final
            // slot prevents the following ordinary record.
            return Ok(false);
        }
        Ok(true)
    }

    fn insert_line_break(&mut self, at: usize, size: i32) -> Result<bool> {
        if at > self.records.len() {
            return Err(invalid("legacy line-break insertion has an invalid index"));
        }
        if self.records.len() + 1 >= self.configuration.value(0x3c) as usize {
            return Ok(false);
        }
        self.records.insert(
            at,
            LegacyTextRecord {
                code: 0,
                character: None,
                color: 0,
                size: 0,
                position: [0; 2],
                advance: 0,
                ruby: false,
                anchor: None,
                complete: true,
                deadline: 0,
            },
        );
        for record in &mut self.records {
            if let Some(anchor) = &mut record.anchor {
                if *anchor >= at {
                    *anchor += 1;
                }
            }
        }
        if self.playback_cursor > at {
            self.playback_cursor += 1;
        }
        self.cursor[1] = self.cursor[1]
            .wrapping_add(size)
            .wrapping_add(self.line_spacing);
        self.cursor[0] = if self.indent_active {
            size.wrapping_add(self.configuration.value(0x20) as i32)
        } else {
            0
        };
        for record in &mut self.records[at + 1..] {
            record.position[0] = self.cursor[0];
            self.cursor[0] = self.cursor[0]
                .wrapping_add(record.advance)
                .wrapping_add(self.configuration.value(0x20) as i32);
        }
        Ok(true)
    }

    fn separator(&mut self) -> Result<()> {
        if self.records.len() < self.configuration.value(0x3c) as usize {
            self.records.push(LegacyTextRecord {
                code: 0xffff,
                character: None,
                color: 0,
                size: 0,
                position: [0; 2],
                advance: 0,
                ruby: false,
                anchor: None,
                complete: true,
                deadline: 0,
            });
        }
        Ok(())
    }

    fn append(&mut self, text: &[u8]) -> Result<(bool, Option<usize>)> {
        let total: usize = self.raw_appends.iter().map(Vec::len).sum();
        if total.saturating_add(text.len()) > MAX_BYTES {
            return Err(invalid("legacy StringPlane retained source exceeds 64 MiB"));
        }
        self.raw_appends.push(text.to_vec());
        let mut cursor = 0usize;
        let mut group_start = None;
        let mut ruby_start = None;
        let mut ruby_codes = Vec::new();
        let mut ordinary_record_added = false;
        let mut bad_at = None;
        while cursor < text.len() {
            let start = cursor;
            let (code, character, width) = decode_cp932_unit(text, cursor);
            if character == char::REPLACEMENT_CHARACTER {
                bad_at.get_or_insert(start);
            }
            cursor += width;
            if code == 0xffff {
                self.separator()?;
                continue;
            }
            if width == 1 && self.markup != 0 {
                match text[start] {
                    b'[' => {
                        group_start = Some(self.records.len());
                        ruby_start = None;
                        ruby_codes.clear();
                        continue;
                    }
                    b'/' if group_start.is_some() => {
                        ruby_start = Some(self.records.len());
                        continue;
                    }
                    b']' if ruby_start.is_some() => {
                        self.append_ruby(group_start.unwrap(), ruby_start.unwrap(), &ruby_codes)?;
                        group_start = None;
                        ruby_start = None;
                        ruby_codes.clear();
                        continue;
                    }
                    b'\\' => {
                        if cursor == text.len() {
                            break;
                        }
                        let marker = text[cursor].to_ascii_lowercase();
                        cursor += 1;
                        match marker {
                            b'c' => {
                                if let Some(value) = legacy_number(text, &mut cursor) {
                                    self.current_color = value;
                                    if text.get(cursor) == Some(&b';') {
                                        cursor += 1;
                                    }
                                }
                            }
                            b'f' => {
                                let sign = text.get(cursor).copied();
                                let preset =
                                    match sign.map(|byte| byte.to_ascii_lowercase()) {
                                        Some(b's') => {
                                            cursor += 1;
                                            if text.get(cursor).is_some_and(|byte| {
                                                byte.eq_ignore_ascii_case(&b's')
                                            }) {
                                                cursor += 1;
                                                Some(0)
                                            } else {
                                                Some(1)
                                            }
                                        }
                                        Some(b'n') => {
                                            cursor += 1;
                                            Some(2)
                                        }
                                        Some(b'l') => {
                                            cursor += 1;
                                            if text.get(cursor).is_some_and(|byte| {
                                                byte.eq_ignore_ascii_case(&b'l')
                                            }) {
                                                cursor += 1;
                                                Some(4)
                                            } else {
                                                Some(3)
                                            }
                                        }
                                        _ => None,
                                    };
                                self.current_size = if let Some(index) = preset {
                                    self.presets[index]
                                } else if let Some(value) = legacy_number(text, &mut cursor) {
                                    if matches!(sign, Some(b'+' | b'-')) {
                                        self.current_size.wrapping_add(value)
                                    } else {
                                        value
                                    }
                                } else {
                                    // The old failed-size branch restores the
                                    // normal preset and consumes one suffix.
                                    if cursor < text.len() {
                                        cursor += 1;
                                    }
                                    self.presets[2]
                                };
                                if text.get(cursor) == Some(&b';') {
                                    cursor += 1;
                                }
                            }
                            b'n' => {
                                self.push_record(0, None, false)?;
                            }
                            b'\\' => {
                                self.push_record(u16::from(b'\\'), Some('\\'), false)?;
                            }
                            // The old parser has no new-message wait/id/style
                            // entry: its default consumes the escape letter.
                            _ => {}
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            if ruby_start.is_some() {
                if ruby_codes.len() >= 32 {
                    return Err(invalid(
                        "legacy StringPlane ruby exceeds its 32-unit parser buffer",
                    ));
                }
                ruby_codes.push((code, character));
            } else {
                if !self.push_record(code, Some(character), false)? {
                    break;
                }
                ordinary_record_added = true;
            }
        }
        // The original parser finalizes layout state at every append; unlike
        // the new Page parser, a pending bracket does not join the next call.
        self.align_records();
        Ok((ordinary_record_added, bad_at))
    }

    fn align_records(&mut self) {
        if !matches!(self.vertical_alignment, 1 | 2) {
            return;
        }
        let mut first = 0;
        let mut top = if self.vertical_alignment == 1 {
            self.configuration.value(0x1c) as i32
        } else {
            self.configuration.origin()[1]
        };
        for end in 0..=self.records.len() {
            if end != self.records.len() && self.records[end].code != 0 {
                continue;
            }
            let height = self.records[first..end]
                .iter()
                .filter(|record| !record.ruby)
                .map(|record| record.size as i32)
                .max()
                .unwrap_or(0);
            for index in first..end {
                if let Some(anchor) = self.records[index].anchor {
                    self.records[index].position[1] = self.records[anchor].position[1]
                        .wrapping_add(self.configuration.value(0x28) as i32);
                } else {
                    self.records[index].position[1] = if self.vertical_alignment == 1 {
                        top
                    } else {
                        top.wrapping_add(height)
                            .wrapping_sub(self.records[index].size as i32)
                    };
                }
            }
            top = top.wrapping_add(height).wrapping_add(self.line_spacing);
            first = end.saturating_add(1);
        }
    }

    fn append_ruby(&mut self, begin: usize, end: usize, reading: &[(u16, char)]) -> Result<()> {
        if reading.is_empty() {
            return Ok(());
        }
        if begin > end || end > self.records.len() {
            return Err(invalid("legacy ruby body range is invalid"));
        }
        // The original receiver skips leading zero records and returns zero
        // when it reaches the registered count. Empty/newline-only bases
        // leave their ordinary prefix intact and do not register ruby.
        let Some(begin) = (begin..end).find(|&index| self.records[index].code != 0) else {
            return Ok(());
        };
        let final_count = self
            .records
            .len()
            .checked_add(reading.len())
            .filter(|&count| {
                count <= self.configuration.value(0x3c) as usize && count <= MAX_RECORDS
            })
            .ok_or_else(|| invalid("legacy ruby records exceed their configured capacity"))?;
        self.records.reserve(final_count - self.records.len());
        let nominal = self.configuration.value(0x1c);
        let nominal =
            i32::try_from(nominal).map_err(|_| invalid("legacy ruby font size exceeds i32"))?;
        let ink_width = nominal.wrapping_sub(2);
        let spacing = self.configuration.value(0x20) as i32;
        let color = self.records[begin].color;
        let body_width = self.records[begin..end]
            .iter()
            .fold(0i32, |sum, record| {
                sum.wrapping_add(record.advance).wrapping_add(spacing)
            })
            .wrapping_sub(spacing);
        let ruby_width = reading.iter().fold(0i32, |sum, (code, _)| {
            sum.wrapping_add(if code & 0xff00 != 0 {
                ink_width
            } else {
                ink_width / 2
            })
        });
        let step = if body_width > ruby_width {
            ((body_width as u32) / (reading.len() as u32 + 1)) as i32
        } else {
            ink_width.wrapping_sub(2)
        };
        let mut offset = body_width
            .wrapping_sub((reading.len() as i32 - 1).wrapping_mul(step))
            .wrapping_sub(ink_width)
            / 2;
        if self.records[begin].position[0].wrapping_add(offset) < 0 {
            offset = 0;
        }
        let mut anchor = begin;
        let mut local = offset;
        let mut edge = self.records[begin].advance;
        for &(code, character) in reading {
            while offset >= edge && anchor < end - 1 {
                let previous = anchor;
                anchor += 1;
                while anchor < end - 1 && self.records[anchor].code == 0 {
                    anchor += 1;
                }
                edge = edge
                    .wrapping_add(self.records[anchor].advance)
                    .wrapping_add(spacing);
                local = local
                    .wrapping_sub(self.records[previous].advance)
                    .wrapping_sub(spacing);
            }
            let body = &self.records[anchor];
            self.records.push(LegacyTextRecord {
                code,
                character: Some(character),
                color,
                size: nominal as u32,
                position: [
                    body.position[0].wrapping_add(local).max(0),
                    body.position[1],
                ],
                advance: if code & 0xff00 != 0 {
                    nominal
                } else {
                    nominal / 2 + 1
                },
                ruby: true,
                anchor: Some(anchor),
                complete: false,
                deadline: 0,
            });
            offset = offset.wrapping_add(step);
            local = local.wrapping_add(step);
        }
        self.complete = false;
        Ok(())
    }

    fn force_finish(&mut self) -> Result<()> {
        // Command 110 ends the currently reachable old record range. Do not
        // confuse it with the new protocol's 461 deadline override.
        // Original command110 prepares the current range without rewriting
        // any record deadline; later ranges retain their ordinary schedule.
        self.force_paint_pending = true;
        self.immediate_paint_pending = true;
        Ok(())
    }

    fn flush_pending(&mut self, device: &DesktopMessageDevice) -> Result<()> {
        if self.immediate_paint_pending {
            self.paint(device, self.last_elapsed, self.force_paint_pending)?;
            self.immediate_paint_pending = false;
            self.force_paint_pending = false;
        }
        Ok(())
    }

    fn user_glyph(&self, code: u16) -> Result<Option<Option<MessageUserGlyph>>> {
        let Some(&[x, y, width, height]) = self.glyph_rectangles.get(&code) else {
            return Ok(None);
        };
        // A registered rectangle without a source takes the original empty
        // full-width-space path, rather than requesting this character's font.
        let Some(source) = &self.glyph_source else {
            return Ok(Some(None));
        };
        if width == 0 || height == 0 {
            return Ok(Some(None));
        }
        if width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(invalid(
                "legacy user glyph rectangle has negative dimensions",
            ));
        }
        let length = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|&n| n <= MAX_BYTES)
            .ok_or_else(|| invalid("legacy user glyph region exceeds 64 MiB"))?;
        let mut bgra = vec![0; length];
        for row in 0..height {
            let sy = i64::from(y as i32) + i64::from(row);
            if sy < 0 || sy >= i64::from(source.height()) {
                continue;
            }
            for column in 0..width {
                let sx = i64::from(x as i32) + i64::from(column);
                if sx < 0 || sx >= i64::from(source.width()) {
                    continue;
                }
                let from = (sy as usize * source.width() as usize + sx as usize) * 4;
                let to = (row as usize * width as usize + column as usize) * 4;
                let rgba = &source.pixels()[from..from + 4];
                bgra[to..to + 4].copy_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
            }
        }
        Ok(Some(Some(MessageUserGlyph {
            size: [width, height],
            bgra,
        })))
    }

    fn schedule(&mut self, elapsed: u32, time_base: Option<u32>) -> Result<()> {
        self.last_elapsed = elapsed;
        self.time_base = time_base.unwrap_or(0);
        if !matches!(self.mode, 0 | 1) {
            // Original 0x13dfbf..0x13e12c dispatches ONLY zero and one.
            // Other DWORDs preserve deadlines/active state and return success;
            // command104 still prepares all records because mode != 1.
            self.scheduled = self.records.len();
            return Ok(());
        }
        let mut delay_units = 0u32;
        for index in self.playback_cursor..self.records.len() {
            let deadline = if let Some(anchor) = self.records[index].anchor {
                self.records[anchor].deadline
            } else if self.mode == 1 {
                let product = self.time_base.wrapping_mul(delay_units);
                let converted = (product as f32 / 60.0) as i32;
                elapsed.wrapping_add(converted.max(1) as u32)
            } else {
                elapsed
            };
            self.records[index].deadline = deadline;
            if self.records[index].anchor.is_none() {
                delay_units = delay_units.wrapping_add(self.character_delay);
            }
        }
        self.scheduled = self.records.len();
        if self.mode == 1 {
            self.playback_active = true;
        } else {
            self.immediate_paint_pending = true;
        }
        self.complete = self.remaining_records() == 0;
        Ok(())
    }

    fn paint(&mut self, device: &DesktopMessageDevice, now: u32, force: bool) -> Result<()> {
        if self.mode == 1 && !self.playback_active && !force {
            return Ok(());
        }
        let mut stopped = false;
        let start = if self.mode == 1 || force {
            self.playback_cursor
        } else {
            0
        };
        for index in start..self.records.len() {
            let record = &self.records[index];
            if record.code == 0xffff && (self.mode == 1 || force) {
                if self.segment_limit != 0 && self.segment_limit != u32::MAX {
                    self.segment_limit = self.segment_limit.wrapping_sub(1);
                }
                self.playback_cursor = index + 1;
                if self.segment_limit == 0 {
                    stopped = true;
                    break;
                }
            }
            if record.complete || record.character.is_none() {
                if self.mode == 1 || force {
                    self.playback_cursor = index + 1;
                }
                continue;
            }
            if self.mode == 1 && !force && record.deadline > now {
                break;
            }
            // Old prepare 0x13c1af subtracts TWO from the record's nominal
            // height before requesting glyph ink. Record advance/line height
            // remain the nominal values rather than shrinking the layout.
            let raster_size = record.size.saturating_sub(2);
            let tail = 0x104;
            let effects = GlyphEffects {
                shadow: self.font_request.word(tail + 4)? != 0,
                outline: self.font_request.word(tail + 8)? != 0,
                shadow_offset: [1, 1],
            };
            match self.user_glyph(record.code)? {
                Some(None) => {}
                Some(Some(glyph)) => {
                    super::message_backend::paint_message_user_glyph(
                        &mut self.surface,
                        &glyph,
                        record.position,
                        raster_size,
                        self.shadow_color,
                        effects,
                    )?;
                }
                None => {
                    let font_request = if record.ruby {
                        &self.ruby_font_request
                    } else {
                        &self.font_request
                    };
                    let glyph =
                        device.legacy_mask(font_request, record.character.unwrap(), raster_size)?;
                    super::message_backend::paint_message_mask(
                        &mut self.surface,
                        &glyph,
                        record.position,
                        raster_size,
                        record.color,
                        self.shadow_color,
                        effects,
                    )?;
                }
            }
            self.records[index].complete = true;
            if self.mode == 1 || force {
                self.playback_cursor = index + 1;
            }
        }
        self.complete = self.records.iter().all(|record| record.complete);
        if self.complete && self.mode == 1 {
            self.playback_active = false;
        }
        if stopped {
            self.playback_active = false;
        }
        if force {
            self.playback_active = false;
        }
        if self.configuration.submit() {
            self.submitted = self.surface.clone();
        }
        Ok(())
    }

    pub(super) fn draw(&self, target: &mut Canvas, node: &SceneNode) -> Result<()> {
        if !node.visible {
            return Ok(());
        }
        let offset = self.configuration.position();
        let frame = RgbaFrame::from_rgba8(
            self.submitted.width(),
            self.submitted.height(),
            self.submitted.pixels().to_vec(),
        )
        .map_err(|_| invalid("legacy StringPlane frame creation failed"))?;
        super::message_backend::draw_message_plane(
            target,
            frame,
            node,
            offset,
            self.configuration.size(),
        )
    }
}

fn cp932_codes(bytes: &[u8]) -> Result<Vec<u16>> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let width = legacy_cp932_width(bytes, cursor);
        let code = if width == 2 {
            u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]])
        } else {
            // The native123 configuration wrapper sign-extends a single
            // byte, while the text decoder below explicitly zero-extends it.
            bytes[cursor] as i8 as i16 as u16
        };
        result.push(code);
        cursor += width;
    }
    Ok(result)
}
fn decode_cp932_unit(bytes: &[u8], cursor: usize) -> (u16, char, usize) {
    let first = bytes[cursor];
    let width = legacy_cp932_width(bytes, cursor);
    let unit = &bytes[cursor..cursor + width];
    let (code, character) = if width == 2 {
        let decoded = super::ScriptTextEncoding::Cp932.decode_strict(unit);
        (u16::from_be_bytes([unit[0], unit[1]]),
            decoded.and_then(|text| text.chars().next()))
    } else if matches!(first, 0x81..=0x9f | 0xe0..=0xfc) {
        (u16::from(first), None)
    } else {
        let decoded = super::cp932_to_utf8(unit);
        (u16::from(first), String::from_utf8_lossy(&decoded).chars().next())
    };
    (code, character.unwrap_or(char::REPLACEMENT_CHARACTER), width)
}

#[test]
fn legacy_string_plane_recovers_malformed_units_at_character_boundaries() {
    assert_eq!(decode_cp932_unit(b"\x82\xa0", 0), (0x82a0, '\u{3042}', 2));
    assert_eq!(decode_cp932_unit(b"\x82", 0), (0x82, char::REPLACEMENT_CHARACTER, 1));
    assert_eq!(decode_cp932_unit(b"\x82/", 0), (0x82, char::REPLACEMENT_CHARACTER, 1));
    assert_eq!(decode_cp932_unit(b"\x82/", 1), (0x2f, '/', 1));
    assert_eq!(decode_cp932_unit(b"\xff", 0), (0xff, '\u{f8f3}', 1));
}
fn legacy_cp932_width(bytes: &[u8], cursor: usize) -> usize {
    if matches!(bytes[cursor], 0x81..=0x9f | 0xe0..=0xef)
        && bytes
            .get(cursor + 1)
            .is_some_and(|&byte| matches!(byte, 0x40..=0x7e | 0x80..=0xfc))
    {
        2
    } else {
        1
    }
}
fn legacy_number(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let start = *cursor;
    let negative = bytes.get(*cursor) == Some(&b'-');
    if matches!(bytes.get(*cursor), Some(b'+' | b'-')) {
        *cursor += 1;
    }
    if !bytes.get(*cursor).is_some_and(u8::is_ascii_digit) {
        *cursor = start;
        return None;
    }
    let radix = if bytes.get(*cursor) == Some(&b'0')
        && matches!(bytes.get(*cursor + 1), Some(b'x' | b'X'))
    {
        *cursor += 2;
        16
    } else {
        10
    };
    let mut value = 0u32;
    while let Some(digit) = bytes
        .get(*cursor)
        .and_then(|&byte| (byte as char).to_digit(radix))
    {
        value = value.wrapping_mul(radix).wrapping_add(digit);
        *cursor += 1;
    }
    Some(if negative {
        value.wrapping_neg()
    } else {
        value
    })
}
