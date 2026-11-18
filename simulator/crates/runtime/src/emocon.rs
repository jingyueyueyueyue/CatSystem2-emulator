//! Task-owned graphical mark definitions. This service owns configuration
//! data, not the animated-model renderer or a reference engine's code.
use cs2_formats::ConfigXml;
use crate::{RuntimeError, RuntimeResult};
use serde::{Deserialize, Serialize};

const MAX_ROWS: usize = 65_536;
const MAX_TEXT_UNITS: usize = 4095;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Size { id: u32, scale: i32 }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Offset { id: u32, position: [i32; 2] }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pose { id: u32, offsets: Vec<Offset> }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Character { id: u32, poses: Vec<Pose> }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Mark {
    id: u32,
    name: Vec<u16>,
    label: Vec<u16>,
    position: [i32; 2],
    scale: i32,
    half: u32,
    textures: [Vec<u16>; 2],
}

#[derive(Clone, Debug)]
pub struct MarkPlacement {
    pub position: [i32; 2],
    pub scale: i32,
    pub texture: Vec<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmoconConfiguration {
    sizes: Vec<Size>,
    characters: Vec<Character>,
    marks: Vec<Mark>,
    texture_scale: i32,
    stop_label: Vec<u16>,
}

impl Default for EmoconConfiguration {
    fn default() -> Self {
        Self { sizes: Vec::new(), characters: Vec::new(), marks: Vec::new(),
            texture_scale: 100, stop_label: Vec::new() }
    }
}

impl EmoconConfiguration {
    /// Clear all three tables before trying the primary XML. Invalid/missing
    /// primary data preserves the previous two scalar/text settings.
    pub fn clear_tables(&mut self) {
        self.sizes.clear(); self.characters.clear(); self.marks.clear();
    }

    pub fn validate(&self) -> RuntimeResult<()> {
        if self.sizes.len() > MAX_ROWS || self.characters.len() > MAX_ROWS
            || self.marks.len() > MAX_ROWS || self.stop_label.len() > MAX_TEXT_UNITS
            || self.stop_label.contains(&0)
        { return Err(error("emocon snapshot exceeds its table/text bounds")); }
        let mut total = 0usize;
        for character in &self.characters {
            for pose in &character.poses {
                total = total.checked_add(1).and_then(|value| value.checked_add(pose.offsets.len()))
                    .filter(|&value| value <= MAX_ROWS)
                    .ok_or_else(|| error("emocon snapshot exceeds its pose/offset bound"))?;
            }
        }
        for mark in &self.marks {
            for text in [&mark.name, &mark.label, &mark.textures[0], &mark.textures[1]] {
                if text.len() > MAX_TEXT_UNITS || text.contains(&0) {
                    return Err(error("emocon snapshot has invalid encoded text storage"));
                }
            }
        }
        Ok(())
    }

    pub fn load_document(
        &mut self, document: &ConfigXml,
        mut load_pose: impl FnMut(&[u16]) -> RuntimeResult<Option<ConfigXml>>,
    ) -> RuntimeResult<()> {
        self.clear_tables();
        self.texture_scale = integer(document, "size/texscale", 100);
        self.stop_label = wide(document, "emocon/stop/label")?.unwrap_or_default();
        for index in 1..=MAX_ROWS {
            let base = format!("size/s{index:02}");
            if text(document, &format!("{base}/prefix")).is_none() { break; }
            self.sizes.push(Size {
                id: integer(document, &format!("{base}/id"), 100) as u32,
                scale: integer(document, &format!("{base}/scale"), 100),
            });
            limit(index)?;
        }
        let mut total = 0usize;
        for index in 1..=MAX_ROWS {
            let base = format!("ch/c{index:02}");
            let id = integer(document, &format!("{base}/id"), -1) as u32;
            if id == u32::MAX { break; }
            let mut character = Character { id, poses: Vec::new() };
            for pose_index in 1..=MAX_ROWS {
                let pose_base = format!("{base}/p{pose_index:02}");
                let id = integer(document, &format!("{pose_base}/id"), -1) as u32;
                if id == u32::MAX { break; }
                total = total.checked_add(1).filter(|&value| value <= MAX_ROWS)
                    .ok_or_else(|| error("emocon pose count exceeds 65536"))?;
                let mut pose = Pose { id, offsets: Vec::new() };
                if let Some(name) = wide(document, &format!("{pose_base}/table"))? {
                    if let Some(table) = load_pose(&name)? {
                        for mark in 1..=MAX_ROWS {
                            let entry = format!("em{mark:02}");
                            let id = integer(&table, &format!("{entry}/id"), -1) as u32;
                            if id == u32::MAX { break; }
                            total = total.checked_add(1).filter(|&value| value <= MAX_ROWS)
                                .ok_or_else(|| error("emocon offset count exceeds 65536"))?;
                            pose.offsets.push(Offset { id, position: [
                                integer(&table, &format!("{entry}/offset/x"), 0),
                                integer(&table, &format!("{entry}/offset/y"), 0),
                            ] });
                            limit(mark)?;
                        }
                    }
                }
                character.poses.push(pose);
                limit(pose_index)?;
            }
            self.characters.push(character);
            limit(index)?;
        }
        for index in 1..=MAX_ROWS {
            let base = format!("emocon/em{index:02}");
            let id = integer(document, &format!("{base}/id"), -1) as u32;
            if id == u32::MAX { break; }
            self.marks.push(Mark {
                id, name: wide(document, &format!("{base}/name"))?.unwrap_or_default(),
                label: wide(document, &format!("{base}/label"))?.unwrap_or_default(),
                position: [integer(document, &format!("{base}/pos/x"), 100),
                    integer(document, &format!("{base}/pos/y"), 100)],
                scale: integer(document, &format!("{base}/scale"), -1),
                half: integer(document, &format!("{base}/tex/half"), 0) as u32,
                textures: [
                    wide(document, &format!("{base}/tex/l"))?.unwrap_or_default(),
                    wide(document, &format!("{base}/tex/s"))?.unwrap_or_default(),
                ],
            });
            limit(index)?;
        }
        Ok(())
    }

    pub fn mark_count(&self) -> u32 { self.marks.len() as u32 }
    pub fn mark_name(&self, index: u32) -> &[u16] {
        self.marks.get(index as usize).map_or(&[], |mark| mark.name.as_slice())
    }
    pub fn mark_label(&self, index: u32) -> &[u16] {
        self.marks.get(index as usize).map_or(&[], |mark| mark.label.as_slice())
    }
    pub fn stop_label(&self) -> &[u16] { &self.stop_label }
    pub fn mark_id(&self, index: u32) -> Option<u32> {
        self.marks.get(index as usize).map(|mark| mark.id)
    }

    pub fn placement(&self, name: &[u16], size: u32, base: [i32; 2]) -> Option<MarkPlacement> {
        let size = self.sizes.iter().find(|entry| entry.id == size)?;
        let mark = self.marks.iter().find(|entry| entry.name.as_slice() == name)?;
        let position = std::array::from_fn(|axis|
            mark.position[axis].wrapping_add(base[axis]).wrapping_mul(size.scale) / 100);
        let mut scale = (self.texture_scale.wrapping_mul(size.scale) / 100)
            .wrapping_mul(mark.scale) / 100;
        let small = scale < 50 && mark.half != 0;
        if small { scale = scale.wrapping_mul(2); }
        Some(MarkPlacement { position, scale, texture: mark.textures[usize::from(small)].clone() })
    }

    pub fn pose_offset(&self, character: u32, pose: u32, mark: u32) -> Option<[i32; 2]> {
        self.characters.iter().find(|entry| entry.id == character)?.poses.iter()
            .find(|entry| entry.id == pose)?.offsets.iter().find(|entry| entry.id == mark)
            .map(|entry| entry.position)
    }
}

fn parts<'a>(document: &ConfigXml, path: &'a str) -> Vec<&'a [u8]> {
    let mut parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).map(str::as_bytes).collect();
    if document.root == b"document" { parts.insert(0, b"document"); }
    parts
}
fn text<'a>(document: &'a ConfigXml, path: &str) -> Option<&'a [u8]> {
    document.text(&parts(document, path))
}
fn integer(document: &ConfigXml, path: &str, default: i32) -> i32 {
    document.integer(&parts(document, path), default)
}
fn wide(document: &ConfigXml, path: &str) -> RuntimeResult<Option<Vec<u16>>> {
    let Some(bytes) = text(document, path) else { return Ok(None); };
    let bytes = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
    let text = std::str::from_utf8(bytes).map_err(|_| error("emocon XML text is not UTF-8"))?;
    let mut units = Vec::new();
    // The recovered configuration wide reader keeps the FIRST unit of each
    // character block. Do not silently expand supplementary characters.
    for character in text.chars() {
        if units.len() >= MAX_TEXT_UNITS { return Err(error("emocon text exceeds 4095 wide units")); }
        units.push(character.encode_utf16(&mut [0; 2])[0]);
    }
    Ok(Some(units))
}
fn limit(index: usize) -> RuntimeResult<()> {
    if index == MAX_ROWS { Err(error("emocon table has no bounded terminator")) } else { Ok(()) }
}
fn error(message: &str) -> RuntimeError { RuntimeError::Host(message.into()) }
