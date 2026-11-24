//! Bounded, data-only reader for the original executable's numeric UI resources.
//! No module is loaded; these are templates, not executable resource providers.
use anyhow::{Context, Result, bail};

pub(super) struct UiResources { bytes: Vec<u8>, base: usize, sections: Vec<[u32; 3]> }
pub(super) struct Reader<'a> { pub bytes: &'a [u8], pub at: usize }
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self { Self { bytes, at: 0 } }
    pub fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(length).context("UI resource range overflow")?;
        let bytes = self.bytes.get(self.at..end).context("truncated UI resource")?;
        self.at = end; Ok(bytes)
    }
    pub fn word(&mut self) -> Result<u16> { Ok(u16::from_le_bytes(self.take(2)?.try_into()?)) }
    pub fn dword(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into()?)) }
    pub fn align(&mut self) { self.at = (self.at + 3) & !3; }
    pub fn text(&mut self) -> Result<Vec<u16>> {
        let mut text = Vec::new();
        loop {
            let unit = self.word()?;
            if unit == 0 { return Ok(text); }
            if text.len() >= 32768 { bail!("UI resource string exceeds 32768 units"); }
            text.push(unit);
        }
    }
    pub fn ordinal(&mut self) -> Result<Vec<u16>> {
        let start = self.at;
        if self.word()? == 0xffff { self.word()?; Ok(Vec::new()) }
        else { self.at = start; self.text() }
    }
}
fn dword(bytes: &[u8], at: usize) -> Result<u32> {
    let end = at.checked_add(4).context("PE UI header offset overflow")?;
    Ok(u32::from_le_bytes(bytes.get(at..end).context("truncated PE UI header")?.try_into()?))
}
fn word(bytes: &[u8], at: usize) -> Result<u16> {
    let end = at.checked_add(2).context("PE UI header offset overflow")?;
    Ok(u16::from_le_bytes(bytes.get(at..end).context("truncated PE UI header")?.try_into()?))
}
impl UiResources {
    pub(super) fn file_version(&self) -> Result<String> {
        let mut reader = Reader::new(self.resource(16, 1)?);
        reader.word()?;
        let value_size = reader.word()?;
        reader.word()?;
        if reader.text()? != "VS_VERSION_INFO".encode_utf16().collect::<Vec<_>>() || value_size < 52 {
            bail!("invalid PE version resource");
        }
        reader.align();
        if reader.dword()? != 0xFEEF04BD { bail!("invalid PE fixed version signature"); }
        reader.dword()?;
        let high = reader.dword()?;
        let low = reader.dword()?;
        Ok(format!("{}.{}.{}.{}", high >> 16, high & 0xffff, low >> 16, low & 0xffff))
    }

    /// Original physical-code menus are byte-code +32-byte CP932 label rows.
    /// Tokens identify each table across address rebasing; FF terminates it.
    pub fn input_code_labels(&self, token: &str) -> Result<Vec<(u32, Vec<u16>)>> {
        let needle = token.as_bytes();
        if needle.is_empty() || needle.len() > 32 { bail!("invalid original input code table token"); }
        let starts = self.bytes.windows(needle.len()).enumerate()
            .filter_map(|(at, bytes)| if bytes == needle && at > 0 && self.bytes[at - 1] == 0 { Some(at - 1) } else { None })
            .collect::<Vec<_>>();
        if starts.len() != 1 { bail!("original input code table token is absent or ambiguous"); }
        let mut out = Vec::new();
        for index in 0..=256 {
            let at = starts[0] + index * 33;
            let code = *self.bytes.get(at).context("input code table is truncated")?;
            if code == 255 { return Ok(out); }
            let bytes = self.bytes.get(at + 1..at + 33).context("input code table row is truncated")?;
            let length = bytes.iter().position(|&byte| byte == 0).context("input code label has no terminator")?;
            if length == 0 || !bytes.starts_with(b"$0010-") || out.iter().any(|(previous, _)| *previous == u32::from(code)) {
                bail!("original input code table row does not match inspected layout");
            }
            let label = super::ScriptTextEncoding::Cp932.decode(&bytes[..length]).encode_utf16().collect();
            out.push((u32::from(code), label));
        }
        bail!("original input code table exceeds 256 rows")
    }
    /// Engine literals carry the same language tokens as the dialog resources.
    /// Locate the token in read-only file data; never call its PE resolver.
    pub fn wide_label(&self, token: &str) -> Result<Vec<u16>> {
        let needle = token.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>();
        if needle.is_empty() { bail!("original UI label token is empty"); }
        let mut found = None;
        for at in (0..self.bytes.len().saturating_sub(needle.len()) + 1).step_by(2) {
            if self.bytes.get(at..at + needle.len()) != Some(needle.as_slice()) { continue; }
            let end = (at + 8192).min(self.bytes.len());
            let mut reader = Reader::new(&self.bytes[at..end]);
            let text = reader.text()?;
            if found.as_ref().is_some_and(|previous| previous != &text) {
                bail!("original UI language token has conflicting literals");
            }
            found = Some(text);
        }
        found.context("original UI language token literal is absent")
    }
    pub fn read(path: &std::path::Path) -> Result<Self> {
        use std::io::Read;
        let file = std::fs::File::open(path).context("open original UI resource image read-only")?;
        if file.metadata()?.len() > 128 * 1024 * 1024 { bail!("UI PE exceeds 128 MiB"); }
        let mut bytes = Vec::new();
        file.take(128 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 128 * 1024 * 1024 { bail!("UI PE grew beyond limit"); }
        if bytes.get(..2) != Some(b"MZ") { bail!("UI image has no DOS signature"); }
        let pe = dword(&bytes, 0x3c)? as usize;
        if pe > bytes.len().saturating_sub(256) { bail!("UI PE header exceeds original file"); }
        if bytes.get(pe..pe + 4) != Some(b"PE\0\0") { bail!("UI image has no PE signature"); }
        let optional = pe + 24;
        let directories = match word(&bytes, optional)? { 0x10b => 96, 0x20b => 112, _ => bail!("unsupported UI PE") };
        let rva = dword(&bytes, optional + directories + 16)?;
        let table = optional + usize::from(word(&bytes, pe + 20)?);
        let mut sections = Vec::new();
        let count = word(&bytes, pe + 6)?;
        if count > 96 { bail!("UI PE has too many sections"); }
        for index in 0..usize::from(count) {
            let at = table + index * 40;
            sections.push([dword(&bytes, at + 12)?, dword(&bytes, at + 16)?, dword(&bytes, at + 20)?]);
        }
        let mut image = Self { bytes, base: 0, sections };
        image.base = image.offset(rva)?;
        Ok(image)
    }
    fn offset(&self, rva: u32) -> Result<usize> {
        for &[start, size, raw] in &self.sections {
            if let Some(delta) = rva.checked_sub(start).filter(|&delta| delta < size) {
                let offset = raw.checked_add(delta).context("UI section range overflow")? as usize;
                if offset >= self.bytes.len() { bail!("UI section offset exceeds original file"); }
                return Ok(offset);
            }
        }
        bail!("UI resource RVA is outside original file data")
    }
    fn entries(&self, directory: usize) -> Result<Vec<[u32; 2]>> {
        let count = usize::from(word(&self.bytes, directory + 12)?) + usize::from(word(&self.bytes, directory + 14)?);
        if count > 32768 { bail!("UI directory count exceeds limit"); }
        (0..count).map(|index| {
            let at = directory + 16 + index * 8;
            Ok([dword(&self.bytes, at)?, dword(&self.bytes, at + 4)?])
        }).collect()
    }
    pub fn resource(&self, kind: u32, id: u32) -> Result<&[u8]> {
        let mut directory = self.base;
        for key in [kind, id] {
            let entry = self.entries(directory)?.into_iter().find(|entry| entry[0] == key)
                .context("original numeric UI resource is absent")?;
            if entry[1] & 0x8000_0000 == 0 { bail!("UI directory points to premature payload"); }
            directory = self.base + (entry[1] & 0x7fff_ffff) as usize;
        }
        let languages = self.entries(directory)?;
        let entry = languages.iter().find(|entry| entry[0] == 0)
            .or_else(|| languages.iter().find(|entry| entry[0] == 1041))
            .or_else(|| languages.first()).context("UI resource has no language")?;
        if entry[1] & 0x8000_0000 != 0 { bail!("UI language entry is not data"); }
        let data = self.base + entry[1] as usize;
        let at = self.offset(dword(&self.bytes, data)?)?;
        let length = dword(&self.bytes, data + 4)? as usize;
        if length > 4 * 1024 * 1024 { bail!("UI resource exceeds 4 MiB"); }
        self.bytes.get(at..at.checked_add(length).context("UI payload overflow")?)
            .context("UI payload exceeds original image")
    }
    pub fn dialog(&self, id: u32) -> Result<(Vec<u16>, Vec<(u32, Vec<u16>)>)> {
        let mut r = Reader::new(self.resource(5, id)?);
        let extended = r.bytes.get(..4) == Some(&[1, 0, 255, 255]);
        let (style, count) = if extended {
            r.take(12)?; let style = r.dword()?; (style, r.word()?)
        } else { let style = r.dword()?; r.dword()?; (style, r.word()?) };
        r.take(8)?; r.ordinal()?; r.ordinal()?;
        let title = r.text()?;
        if style & 0x40 != 0 { r.word()?; if extended { r.take(4)?; } r.text()?; }
        if count > 4096 { bail!("UI dialog has too many controls"); }
        let mut controls = Vec::new();
        for _ in 0..count {
            r.align(); r.take(if extended { 20 } else { 16 })?;
            let id = if extended { r.dword()? } else { u32::from(r.word()?) };
            r.ordinal()?; let text = r.ordinal()?;
            let length = r.word()?; r.take(usize::from(length))?;
            controls.push((id, text));
        }
        Ok((title, controls))
    }
}
