//! Independent history receiver; Backlog owns the retained rows and sources.
//! Only visible row surfaces belong to the FES owner and are released with it.
use super::*;
use cs2_runtime::backlog::{Backlog, Segment};

#[path = "fes_log_remake.rs"]
mod remake;
pub(super) use remake::apply as remake_command;

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct LogString {
    handle: u32,
    count: usize,
    position: i32,
    drawing: bool,
    redraw: bool,
    pub(super) geometry_dirty: bool,
    pub(super) positions: Vec<[i32; 2]>,
    pub(super) rows: Vec<TextBank>,
    colors: [[u32; 3]; 6],
    row_colors: Vec<usize>,
    line_colors: BTreeMap<usize, [u32; 3]>,
}

impl Default for LogString {
    fn default() -> Self {
        Self {
            handle: 0, count: 3, position: 0, drawing: false, redraw: true,
            geometry_dirty: true, positions: Vec::new(), rows: Vec::new(),
            colors: [[u32::MAX, 0xff00_0000, u32::MAX]; 6],
            row_colors: vec![0; 3], line_colors: BTreeMap::new(),
        }
    }
}

impl LogString {
    pub(super) fn checkpoint_handle(&self) -> u32 { self.handle }

    pub(super) fn validate_checkpoint(&self) -> Result<()> {
        if self.count > MAX_BANKS || self.rows.len() > MAX_BANKS
            || self.positions.len() > MAX_BANKS || self.row_colors.len() != self.count
            || self.row_colors.iter().any(|&color| color > 2)
            || self.line_colors.len() > MAX_BANKS
            || self.position < 0 || self.rows.iter().any(|row| row.log.is_some())
        { bail!("LOGSTRING checkpoint row state is invalid"); }
        Ok(())
    }

    pub(super) fn initialize_display(&mut self, visible: bool) {
        self.drawing = visible;
        self.redraw = true;
    }
    pub(super) fn set_count(&mut self, count: i32) -> Result<()> {
        if count < 0 || count as usize > MAX_BANKS { bail!("LOGSTRING COUNT exceeds storage limit"); }
        self.count = count as usize;
        self.row_colors.resize(self.count, 0);
        self.geometry_dirty = true;
        self.redraw = true;
        Ok(())
    }

    fn scroll(&mut self, position: i32, log: Option<&Backlog>, relative: bool) {
        let Some(log) = log else { return };
        let end = log.lines().len() as i32 - self.count as i32;
        if relative && end < 0 { return; }
        let next = position.max(0).min(end.max(0));
        if !relative || self.position != next {
            self.position = next;
            self.row_colors.fill(0);
            self.line_colors.clear();
            self.redraw = true;
        }
    }
}

pub(super) fn row_name(name: &[u8], row: usize) -> Vec<u8> {
    let mut result = name.to_vec();
    result.extend_from_slice(format!("\0logrow:{row}").as_bytes());
    result
}

fn row_info(log: &Backlog, index: u32) -> Option<(u32, &Segment)> {
    let line = log.lines().get(index as usize)?;
    let group = line.segment.wrapping_sub(log.segment_start());
    Some((group, log.segments().get(group as usize)?))
}

/// Called before ordinary text verbs, whose pos/getpos/draw have a different ABI.
pub(super) fn command(
    store: &mut FesTextStore, keys: &[Key], command: &FesDriverCommand,
    driver: &mut FesScriptDriver, shared: &mut ExpressionState,
    backlogs: &BTreeMap<u32, Backlog>, broadcast: bool,
) -> Result<bool> {
    if store.banks[&keys[0]].log.is_none() { return Ok(false); }
    let verb = command.parameters[1].as_slice();
    let layout = (verb == b"layout").then(|| command.parameters.get(2).map(Vec::as_slice)).flatten();
    let palette = match layout {
        Some(b"color") => Some(0), Some(b"vcolor") => Some(1),
        Some(b"acolor") => Some(2), Some(b"vacolor") => Some(3),
        Some(b"pcolor") => Some(4), Some(b"vpcolor") => Some(5), _ => None,
    };
    let defaults: &[i32] = match verb {
        b"setlog" | b"pos" | b"page" => &[0], b"max" => &[-1],
        b"getpos" => &[-1, -1, -1], b"next" | b"disp" => &[1], b"rev" => &[-1],
        b"blockcolor" => &[0, 0], b"linecolor" => &[0, -1, -1, -1],
        b"lineinfo" | b"pathinfo" => &[0, 0, 0], b"blockinfo" => &[0, -1, -1, -1, -1],
        b"draw" => &[], b"layout" if palette.is_some() => &[0; 9],
        // The host-facing path handles remake before borrowing the stores.
        b"remake" => bail!("LOGSTRING remake bypassed its resource receiver"),
        _ => return Ok(false),
    };
    let args = &command.parameters[if layout.is_some() { 3 } else { 2 }..];
    // History queries/palette commands retain the event's @ bank. Their
    // missing-argument defaults are not a property-current expression context.
    let values = defaults.iter().enumerate().map(|(i, default)| match args.get(i) {
        Some(source) => driver.evaluate_parameter(source, command.context.current, shared),
        None => Ok(*default),
    }).collect::<Result<Vec<_>>>()?;
    for key in keys {
        let bank = store.banks.get_mut(key).unwrap();
        let state = bank.log.as_mut().unwrap();
        let log = if state.handle == 0 { None } else {
            Some(backlogs.get(&state.handle).context("LOGSTRING backlog handle was released or has another type")?)
        };
        match verb {
            b"setlog" => {
                let handle = values[0] as u32;
                let log = if handle == 0 { None } else {
                    Some(backlogs.get(&handle).context("LOGSTRING setlog requires a live Backlog handle")?)
                };
                state.handle = handle;
                if let Some(log) = log { state.position = log.lines().len().saturating_sub(state.count) as i32; }
                state.redraw = true;
            }
            b"max" => write_shared(shared, values[0], log.map_or(0, |log| log.lines().len() as i32)),
            b"getpos" if !broadcast => for (slot, value) in values.iter().zip([state.position, 0, 0]) {
                write_shared(shared, *slot, value);
            },
            b"getpos" => {},
            b"pos" => state.scroll(values[0], log, false),
            b"page" => state.scroll(state.position.wrapping_add(values[0].wrapping_mul(state.count as i32)), log, false),
            b"next" | b"rev" => state.scroll(state.position.wrapping_add(values[0]), log, true),
            b"draw" => { state.row_colors.fill(0); state.line_colors.clear(); state.redraw = true; state.drawing = true; },
            b"disp" => { state.drawing = values[0] != 0; state.redraw = true; },
            b"blockcolor" => {
                if values[0] >= 0 && (values[0] as usize) < state.count {
                    if let Some((_, info)) = log.and_then(|log| row_info(log, state.position.wrapping_add(values[0]) as u32)) {
                        let log = log.unwrap();
                        let first = info.first_line.wrapping_sub(log.line_start());
                        let last = info.last_line.wrapping_sub(log.line_start());
                        for (i, color) in state.row_colors.iter_mut().enumerate() {
                            let index = state.position.wrapping_add(i as i32) as u32;
                            if index >= first && index <= last { *color = if matches!(values[1], 1 | 2) { values[1] as usize } else { 0 }; }
                        }
                        state.redraw = true;
                    }
                }
            }
            b"linecolor" => {
                if let Some(row) = state.rows.get(values[0] as usize) {
                    let mut colors = row.restore_colors;
                    for (color, value) in colors.iter_mut().zip(&values[1..]) { if *value != -1 { *color = *value as u32; } }
                    state.line_colors.insert(values[0] as usize, colors);
                    state.redraw = true;
                }
            }
            b"lineinfo" => {
                let info = log.and_then(|log| row_info(log, values[0] as u32));
                let source = info.and_then(|(_, info)| log.unwrap().sources().get(info.source as usize)).cloned().unwrap_or_default();
                shared.strings.insert(values[1] as u32, source);
                write_shared(shared, values[2], info.map_or(-1, |(_, info)| info.input as i32));
            }
            b"pathinfo" => if let Some((_, info)) = log.and_then(|log| row_info(log, values[0] as u32)) {
                write_shared(shared, values[1], info.extra as i32);
                write_shared(shared, values[2], info.input as i32);
            },
            b"blockinfo" => {
                let result = log.and_then(|log| row_info(log, values[0] as u32)).map_or([-1, -1, -1, 0], |(group, info)| {
                    let start = log.unwrap().line_start();
                    [group as i32, info.first_line.wrapping_sub(start) as i32, info.last_line.wrapping_sub(start) as i32, (info.flags & 4) as i32]
                });
                for (slot, value) in values[1..].iter().zip(result) { write_shared(shared, *slot, value); }
            }
            b"layout" => {
                let mut colors = [0xff00_0000; 3];
                for (i, value) in values.iter().enumerate() { colors[i / 3] |= (*value as u8 as u32) << (i % 3 * 8); }
                state.colors[palette.unwrap()] = colors;
                state.redraw = true;
            }
            _ => unreachable!(),
        }
    }
    Ok(true)
}

pub(super) fn geometry(host: &mut LoggingHost<'_>, key: &Key) -> Result<()> {
    let bank = &host.data_services.fes_text.banks[key];
    let state = bank.log.as_ref().unwrap();
    let metadata = bank.metadata.clone();
    let count = state.count;
    let positions = if state.geometry_dirty || state.positions.len() != count {
        let mut positions = Vec::with_capacity(count);
        for i in 0..count {
            positions.push(if let Some(reference) = &metadata.position_source {
                let mut reference = reference.clone();
                reference.frame = reference.frame.wrapping_add(i as u32);
                super::super::fes_controls::read_metadata(host, &reference)?.map_or([0; 2], |(position, _)| position)
            } else { host.data_services.fes_text.banks[key].position });
        }
        Some(positions)
    } else { None };
    if metadata.size_dirty {
        if let Some(reference) = &metadata.size_source {
            let size = super::super::fes_controls::read_metadata(host, reference)?.map_or([0; 2], |(_, size)| size);
            host.data_services.fes_text.banks.get_mut(key).unwrap().set_size(size)?;
        }
        host.data_services.fes_text.banks.get_mut(key).unwrap().metadata.size_dirty = false;
    }
    let state = host.data_services.fes_text.banks.get_mut(key).unwrap().log.as_mut().unwrap();
    if let Some(positions) = positions { state.positions = positions; }
    state.geometry_dirty = false;
    Ok(())
}

pub(super) fn update(
    bank: &mut TextBank, next_surface: &mut i32, device: &mut DesktopMessageDevice,
    view: &FesTaskView, runtime: &FesRuntime, name: &[u8], index: i32,
    backlogs: &BTreeMap<u32, Backlog>,
) -> Result<()> {
    let mut state = bank.log.take().unwrap();
    let result = (|| -> Result<()> {
        let log = if state.handle == 0 { None } else { Some(backlogs.get(&state.handle).context("LOGSTRING backlog handle was released")?) };
        if state.rows.len() != state.count {
            for row in state.rows.drain(..) { row.release(device); }
            for _ in 0..state.count {
                let id = *next_surface;
                *next_surface = id.checked_sub(1).context("FES text surface id space exhausted")?;
                let mut row = bank.clone();
                row.message = MessageObject::new(id, bank.message.encoding());
                row.configured = false; row.dirty = true; row.projection = None;
                row.pending.clear(); row.source.clear(); row.source_initialized = false;
                row.user_source_dirty = true; row.user_mapping_cursor = 0; row.last_frame = None;
                state.rows.push(row);
            }
            state.redraw = true;
        }
        let redraw = state.redraw || bank.dirty || bank.shift_dirty;
        let height = word(&bank.configuration, 12) as i32;
        for (i, row) in state.rows.iter_mut().enumerate() {
            let initial = !row.configured;
            if bank.dirty || initial {
                row.configuration.clone_from(&bank.configuration);
                row.inherited = bank.inherited;
                row.dirty = true;
            }
            row.position = state.positions.get(i).copied().unwrap_or(bank.position);
            row.vram = [bank.vram[0], bank.vram[1].wrapping_add(height.wrapping_add(2).wrapping_mul(i as i32))];
            row.origin = bank.origin; row.parent_plane.clone_from(&bank.parent_plane);
            row.priority = bank.priority; row.visible = state.drawing;
            row.tint = bank.tint; row.alpha = bank.alpha; row.scale = bank.scale;
            row.rotation = bank.rotation; row.clip = bank.clip;
            if bank.user_source_dirty || initial {
                row.user_glyph_source.clone_from(&bank.user_glyph_source); row.user_source_dirty = true;
                row.user_mapping_cursor = 0;
            }
            if row.user_mappings != bank.user_mappings {
                row.user_mappings.clone_from(&bank.user_mappings); row.user_mapping_cursor = 0;
            }
            if bank.shift_dirty { row.shift = bank.shift; row.shift_dirty = true; }
            // A previous render may have stopped before consuming these row
            // requests. An empty template must not discard that continuation.
            for (pending, incoming) in row.clear_collections.iter_mut().zip(bank.clear_collections) {
                *pending |= incoming;
            }
            if bank.remove_characters.is_some() {
                row.remove_characters.clone_from(&bank.remove_characters);
            }
            if bank.add_characters.is_some() {
                row.add_characters.clone_from(&bank.add_characters);
            }
            if (redraw || initial) && state.drawing && log.is_some() {
                let line_index = state.position.wrapping_add(i as i32) as u32;
                let voice = log.and_then(|log| row_info(log, line_index)).is_some_and(|(_, info)| info.flags & 4 != 0);
                let colors = state.line_colors.get(&i).copied().unwrap_or(state.colors[state.row_colors[i] * 2 + usize::from(voice)]);
                for (offset, color) in [0x10, 0x14, 0x18].into_iter().zip(colors) { set(&mut row.configuration, offset, color); }
                row.restore_colors = colors; row.dirty = true;
                let text = log.and_then(|log| log.lines().get(line_index as usize)).map_or(&[][..], |line| line.text.as_slice());
                row.replace(text)?;
                row.draw = Some(u32::MAX);
            }
        }
        // All rows now own the actual configuration, text and glyph requests.
        // Retire the template batch before any fallible device consumption:
        // retry must keep completed row pixels and each remaining row cursor,
        // rather than replacing/replaying the whole visible history page.
        bank.dirty = false; bank.user_source_dirty = false; bank.shift_dirty = false;
        bank.clear_collections.fill(false); bank.remove_characters = None; bank.add_characters = None;
        state.redraw = false;
        for (i, row) in state.rows.iter_mut().enumerate() {
            // Row identity is separate from the FES bank and never exposed as
            // another selectable object. The existing BUTTON grid owns input.
            render_bank(device, view, runtime, row, &row_name(name, i), index)?;
        }
        Ok(())
    })();
    bank.log = Some(state);
    result
}
