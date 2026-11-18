//! Independent byte-oriented configuration writer. Values are literal script
//! XML bytes: entity references are not escaped a second time. This is the
//! documented engine format, not a general-purpose XML serializer.
use cs2_formats::ConfigXml;
use crate::{RuntimeError, RuntimeResult};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_NODES: usize = 1_000_000;
const MAX_DEPTH: usize = 256;

#[derive(Clone, Debug)]
struct Element {
    name: Vec<u8>,
    value: Vec<u8>,
    children: Vec<Element>,
}

#[derive(Clone, Debug)]
pub struct ConfigurationDocument {
    root: Element,
    nodes: usize,
}

impl Default for ConfigurationDocument {
    fn default() -> Self {
        Self {
            root: Element { name: b"document".to_vec(), value: Vec::new(), children: Vec::new() },
            nodes: 1,
        }
    }
}

impl ConfigurationDocument {
    /// Import a parsed document without changing its sibling order. Unmodeled
    /// platform fields survive a later save. Existing paths are overwritten in
    /// place, and newly written paths are inserted at the front of their level.
    pub fn from_config(document: &ConfigXml) -> RuntimeResult<Self> {
        if document.root != b"document" {
            return Err(error("configuration document requires the document root"));
        }
        let mut result = Self::default();
        for field in &document.fields {
            if field.path.first().map(Vec::as_slice) != Some(b"document".as_slice()) {
                return Err(error("configuration field is outside its document root"));
            }
            let path: Vec<_> = field.path.iter().skip(1).map(Vec::as_slice).collect();
            result.write_ordered(&path, &field.text, false)?;
        }
        Ok(result)
    }

    pub fn write(&mut self, path: &[&[u8]], value: &[u8]) -> RuntimeResult<()> {
        self.write_ordered(path, value, true)
    }

    pub fn integer(&mut self, path: &[&[u8]], value: u32) -> RuntimeResult<()> {
        self.write(path, (value as i32).to_string().as_bytes())
    }

    pub fn wide(&mut self, path: &[&[u8]], units: &[u16]) -> RuntimeResult<()> {
        let mut bytes = Vec::new();
        for &unit in units.iter().take_while(|&&unit| unit != 0) {
            if unit == 0xffff {
                bytes.extend_from_slice(&[0xff, 0xff]);
            } else {
                let mut buffer = [0; 4];
                bytes.extend_from_slice(
                    char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)
                        .encode_utf8(&mut buffer).as_bytes(),
                );
            }
            if bytes.len() > 4095 {
                return Err(error("wide configuration field exceeds 4095 bytes"));
            }
        }
        self.write(path, &bytes)
    }

    pub fn float(&mut self, path: &[&[u8]], value: f32) -> RuntimeResult<()> {
        let text = if value.is_nan() {
            if value.is_sign_negative() { "-nan(ind)".into() } else { "nan".into() }
        } else if value.is_infinite() {
            if value.is_sign_negative() { "-inf".into() } else { "inf".into() }
        } else {
            format!("{:.6}", f64::from(value))
        };
        self.write(path, text.as_bytes())
    }

    pub fn to_bytes(&self) -> RuntimeResult<Vec<u8>> {
        let mut bytes = Vec::new();
        append(&mut bytes, b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n")?;
        emit(&self.root, 0, &mut bytes)?;
        Ok(bytes)
    }

    fn write_ordered(&mut self, path: &[&[u8]], value: &[u8], front: bool) -> RuntimeResult<()> {
        if path.len() >= MAX_DEPTH {
            return Err(error("configuration nesting exceeds 256 levels"));
        }
        let value = nul(value);
        if value.len() > MAX_BYTES {
            return Err(error("configuration field exceeds 64 MiB"));
        }
        let mut element = &mut self.root;
        for &segment in path {
            let name = nul(segment);
            if name.is_empty() || name.len() > 511
                || name.iter().any(|byte| byte.is_ascii_whitespace() || matches!(*byte, b'<' | b'>' | b'/' | b'=' | b'\'' | b'"'))
            {
                return Err(error("invalid configuration element name"));
            }
            let index = match element.children.iter().position(|child| child.name == name) {
                Some(index) => index,
                None => {
                    if self.nodes >= MAX_NODES {
                        return Err(error("configuration exceeds one million nodes"));
                    }
                    let node = Element { name: name.to_vec(), value: Vec::new(), children: Vec::new() };
                    self.nodes += 1;
                    if front { element.children.insert(0, node); 0 }
                    else { element.children.push(node); element.children.len() - 1 }
                }
            };
            element = &mut element.children[index];
        }
        element.value = value.to_vec();
        Ok(())
    }
}

fn emit(element: &Element, depth: usize, bytes: &mut Vec<u8>) -> RuntimeResult<()> {
    for _ in 0..depth { append(bytes, b"  ")?; }
    append(bytes, b"<")?;
    append(bytes, &element.name)?;
    append(bytes, b">")?;
    if element.children.is_empty() {
        append(bytes, &element.value)?;
    } else {
        append(bytes, b"\r\n")?;
        for child in &element.children { emit(child, depth + 1, bytes)?; }
        for _ in 0..depth { append(bytes, b"  ")?; }
    }
    append(bytes, b"</")?;
    append(bytes, &element.name)?;
    append(bytes, b">\r\n")
}

fn append(bytes: &mut Vec<u8>, data: &[u8]) -> RuntimeResult<()> {
    if data.len() > MAX_BYTES.saturating_sub(bytes.len()) {
        return Err(error("configuration output exceeds 64 MiB"));
    }
    bytes.try_reserve(data.len()).map_err(|_| error("configuration output allocation failed"))?;
    bytes.extend_from_slice(data);
    Ok(())
}

fn nul(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len())]
}

fn error(message: &str) -> RuntimeError { RuntimeError::Host(message.into()) }
