//! Original 188-byte definitions and applied content. Definitions do not
//! replace previously applied text/images until a source actually updates.
use super::*;
use cs2_renderer::{FrameMetadata, RgbaFrame};
use cs2_runtime::graph_serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct Cell { pub(super) words: Vec<u8> }
impl Default for Cell { fn default() -> Self { Self { words: vec![0; 188] } } }
impl Cell {
    pub(super) fn word(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.words[at..at + 4].try_into().unwrap())
    }
    pub(super) fn put(&mut self, at: usize, value: u32) {
        self.words[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    pub(super) fn name(&self, at: usize, end: usize) -> &[u8] {
        self.words[at..end].split(|&b| b == 0).next().unwrap()
    }
    fn copy_name(&mut self, at: usize, name: &[u8]) {
        self.words[at..at + name.len()].copy_from_slice(name);
        self.words[at + name.len()] = 0;
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct Content {
    pub(super) record: Cell,
    pub(super) text: Vec<u8>,
    #[serde(with = "state::image")]
    pub(super) image: Option<RgbaFrame>,
}

struct Arguments<'a> {
    args: &'a [Vec<u8>], current: i32, driver: &'a mut FesScriptDriver,
    shared: &'a mut ExpressionState, cell: Cell,
}
impl Arguments<'_> {
    fn eval(&mut self, text: &[u8]) -> Result<u32> {
        if text.is_empty() { return Ok(0); }
        Ok(self.driver.evaluate_parameter(text, self.current, self.shared)? as u32)
    }
    fn value(&mut self, index: usize) -> Result<Option<u32>> {
        let Some(text) = self.args.get(index) else { return Ok(None) };
        self.eval(text).map(Some)
    }
    fn number(&mut self, index: usize, at: usize) -> Result<bool> {
        let Some(value) = self.value(index)? else { return Ok(false) };
        self.cell.put(at, value); Ok(true)
    }
    fn reference(&mut self, index: usize) -> Result<bool> {
        let Some(text) = self.args.get(index).filter(|text| !text.is_empty()) else { return Ok(false) };
        let (name, bank) = bank::split_target(text)?;
        anyhow::ensure!(name.len() < 32, "SAVEPANEL reference exceeds its 32-byte field");
        let mut index = bank.map(|text| self.eval(text)).transpose()?.unwrap_or(u32::MAX);
        if index == u32::MAX && self.driver.adapter.object(name)
            .map_or(0, |object| object.definition.bank_count) <= 1 {
            index = 0;
        }
        self.cell.copy_name(0x20, name);
        self.cell.put(0x40, index); Ok(true)
    }
    fn selector(&mut self, index: usize) -> Result<bool> {
        let Some(text) = self.args.get(index) else { return Ok(false) };
        let text = text.split(|&b| b == 0).next().unwrap();
        let end = text.iter().position(|&b| b == b'.' || b == b':').unwrap_or(text.len());
        let name = &text[..end];
        anyhow::ensure!(name.len() < 32, "SAVEPANEL selector exceeds its original field");
        let mut cursor = end;
        let first = if text.get(cursor) == Some(&b'.') {
            let start = cursor + 1;
            cursor = text[start..].iter().position(|&b| b == b':').map_or(text.len(), |i| start + i);
            anyhow::ensure!(cursor - start < 32, "SAVEPANEL selector expression exceeds capacity");
            self.eval(&text[start..cursor])?
        } else { 0 };
        let second = if text.get(cursor) == Some(&b':') {
            let tail = &text[cursor + 1..];
            anyhow::ensure!(tail.len() < 32, "SAVEPANEL selector suffix exceeds capacity");
            Some(self.eval(tail)?)
        } else { None };
        let kind = match name { b"FLAG" => 0, b"DATE_Y" => 3, b"DATE_M" => 4,
            b"DATE_D" => 5, b"DATE_H" => 6, b"DATE_N" => 7, b"DATE_S" => 8, _ => 9 };
        self.cell.put(4, kind);
        match kind {
            0 => { self.cell.put(8, first); self.cell.put(12, second.unwrap_or(0)); }
            9 => { let value = self.eval(name)?; self.cell.put(0x60, value); }
            _ => self.cell.put(12, second.unwrap_or(u32::MAX)),
        }
        Ok(true)
    }
}

pub(super) fn parse(command: &FesDriverCommand, driver: &mut FesScriptDriver,
    shared: &mut ExpressionState) -> Result<Option<(i32, Cell)>> {
    let args = &command.parameters;
    let Some(name) = args.get(2).map(Vec::as_slice) else { return Ok(None) };
    let kind = match name { b"NORM" => 0, b"IMAGE" | b"SIMAGE" => 1,
        b"IMAGEOBJ" => 2, b"STR" | b"SSTR" => 3, b"NUM_S" => 4,
        b"NUM_O" | b"NUM_O2" => 5, b"NUM_I" => 6, _ => return Ok(None) };
    let mut p = Arguments { args, current: command.context.current, driver, shared, cell: Cell::default() };
    let Some(id) = p.value(3)? else { return Ok(None) };
    p.cell.put(0, kind);
    if name == b"NUM_O2" { p.cell.put(0x54, 1); }
    match kind {
        1 if name == b"SIMAGE" => {
            if !p.number(4, 0x10)? { return Ok(None); }
            if p.number(5, 0xa4)? {
                for (index, at) in [(6, 0xa8), (7, 0xac), (8, 0xb0), (9, 0xb4), (10, 0xb8)] {
                    p.number(index, at)?;
                }
                if p.cell.word(0xb4) == 0 || p.cell.word(0xb8) == 0 {
                    p.cell.put(0xac, 0); p.cell.put(0xb0, 0);
                    p.cell.put(0xb4, p.cell.word(0xa4).wrapping_sub(1));
                    p.cell.put(0xb8, p.cell.word(0xa8).wrapping_sub(1));
                }
                p.cell.put(0, 7); p.cell.put(4, 10);
            } else { p.cell.put(4, 1); }
        }
        2 if !p.reference(4)? => return Ok(None),
        3 => {
            p.cell.put(0x58, 1);
            if !p.selector(4)? { return Ok(None); }
            if p.cell.word(4) == 9 {
                p.cell.put(0x10, p.cell.word(0x60)); p.cell.put(4, 2);
            } else { p.cell.put(0x10, u32::MAX); }
            // The width/color argument positions are deliberately not adjacent
            // in the record: size, line width, color -> 14,1c,18.
            for (index, at) in [(5, 0x14), (6, 0x1c), (7, 0x18)] {
                if !p.number(index, at)? { return Ok(None); }
            }
            if let Some(value) = p.value(8)? {
                p.cell.put(0x48, value & 1); p.cell.put(0x4c, value >> 1 & 1);
            }
            for (index, at) in [(9, 0x50), (10, 0x58), (11, 0x5c)] { p.number(index, at)?; }
            if let Some(name) = args.get(12) {
                let name = name.split(|&b| b == 0).next().unwrap();
                anyhow::ensure!(name.len() < 88, "SAVEPANEL font exceeds its 88-byte field");
                p.cell.copy_name(0x64, name);
            }
        }
        5 if !p.reference(4)? || !p.selector(5)? => return Ok(None),
        _ => {}
    }
    Ok(Some((id as i32, p.cell)))
}

fn value(data: &SavePanelData, definition: &Cell) -> Result<Option<(i32, Vec<u8>)>> {
    let value = match definition.word(4) {
        0 => data.flag(definition.word(8) as i32),
        2 => {
            let Some(record) = data.record(definition.word(0x10))? else { return Ok(None) };
            if record.len() >= 256 { return Ok(None); }
            if record.is_empty() { return Ok(Some((0, Vec::new()))); }
            // Bad source does not destroy a previously applied cell.
            let Some(end) = record.iter().position(|&b| b == 0) else { return Ok(None) };
            return Ok(Some((0, record[..end].to_vec())));
        }
        source @ 3..=8 => data.date()[(source - 3) as usize] as i32,
        9 => definition.word(0x60) as i32,
        _ => return Ok(None),
    };
    Ok(Some((value, value.to_string().into_bytes())))
}

fn decode_thumbnail(host: &mut LoggingHost<'_>, bytes: &[u8]) -> Result<Option<RgbaFrame>> {
    if bytes.starts_with(b"\x89PNG") {
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let decoder = reader.into_decoder()?;
        let (width, height) = image::ImageDecoder::dimensions(&decoder);
        let size = [width, height];
        pixels::checked_size(size)?;
        let pixels = image::DynamicImage::from_decoder(decoder)?.into_rgba8();
        return Ok(Some(RgbaFrame::from_rgba8(size[0], size[1], pixels.into_raw())?
            .with_raw_metadata(FrameMetadata::raster(size))));
    }
    if [b"HG-3".as_slice(), b"HG-2", b"BM", b"\xff\xd8"].iter().any(|magic| bytes.starts_with(magic)) {
        return Ok(host.decode_managed_image_frame(bytes, 0)?);
    }
    Ok(None)
}

pub(super) fn apply_sources(panel: &mut bank::Bank, host: &mut LoggingHost<'_>) -> Result<()> {
    // Ordered cells make source retries deterministic. Applying a cell has no
    // script/external effects; old content survives a failed source/decode.
    for (&id, definition) in &panel.cells {
        match definition.word(0) {
            0 => panel.contents.entry(id).or_default().record.put(0, 0),
            kind @ (1 | 7) => {
                if kind == 1 && definition.word(4) != 1 {
                    let content = panel.contents.entry(id).or_default();
                    content.record.put(0, 1); content.record.put(0x28, 0);
                    content.record.put(0x2c, 0); content.image = None; continue;
                }
                if kind == 7 && definition.word(4) != 10 { continue; }
                let Some(bytes) = panel.data.record(definition.word(0x10))? else { continue };
                let image = match decode_thumbnail(host, bytes) {
                    Ok(image) => image,
                    Err(error) => {
                        host.events.push(crate::LogicEvent { frame: host.frame, kind: "save-panel-image-error".into(),
                            detail: format!("file={:?} record={:#x} cell={id}: {error}",
                                String::from_utf8_lossy(panel.data.save_name()), definition.word(0x10)),
                        });
                        continue;
                    }
                };
                if let Some(image) = image {
                    let content = panel.contents.entry(id).or_default();
                    content.record.put(0, kind); content.record.put(0x28, 0);
                    content.record.put(0x2c, 1); content.image = Some(image);
                }
                if kind == 7 && let Some(content) = panel.contents.get_mut(&id) {
                    content.record.words[0xa4..].copy_from_slice(&definition.words[0xa4..]);
                }
            }
            2 => {
                let content = panel.contents.entry(id).or_default();
                content.record.put(0, 2);
                content.record.copy_name(4, definition.name(0x20, 0x40));
                content.record.put(0x24, definition.word(0x40));
            }
            kind @ 3..=6 => {
                let Some((number, text)) = value(&panel.data, definition)? else { continue };
                let content = panel.contents.entry(id).or_default();
                if kind == 3 || kind == 4 {
                    content.record.put(0, 3); content.text = text;
                    for (to, from) in [(0x34, 0x14), (0x38, 0x18), (0x3c, 0x1c),
                        (0x4c, 0x48), (0x50, 0x4c), (0x54, 0x50), (0x5c, 0x58), (0x60, 0x5c)] {
                        content.record.put(to, definition.word(from));
                    }
                    content.record.copy_name(0x64, definition.name(0x64, 188));
                } else {
                    content.record.put(0, kind); content.record.put(0x40, number as u32);
                    if kind == 5 {
                        content.record.copy_name(4, definition.name(0x20, 0x40));
                        content.record.put(0x24, definition.word(0x40));
                    } else { content.record.put(0x28, definition.word(0x44)); content.image = None; }
                    content.record.put(0x44, definition.word(12));
                    content.record.put(0x48, definition.word(0x54));
                    content.record.put(0x5c, 1); content.record.put(0x60, 0);
                }
            }
            _ => anyhow::bail!("SAVEPANEL definition kind is invalid"),
        }
    }
    panel.dirty = true;
    Ok(())
}
