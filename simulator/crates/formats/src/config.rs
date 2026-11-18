use thiserror::Error;

const MAX_XML_SIZE: usize = 64 * 1024 * 1024;
const MAX_XML_DEPTH: usize = 256;
const MAX_XML_FIELDS: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlField {
    pub path: Vec<Vec<u8>>,
    pub text: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigXml {
    pub root: Vec<u8>,
    pub fields: Vec<XmlField>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("configuration XML is too large")]
    TooLarge,
    #[error("configuration XML is malformed at byte {0}")]
    Malformed(usize),
    #[error("configuration XML has an unclosed element")]
    UnclosedElement,
}

impl ConfigXml {
    /// Runtime XML: a leading 1.0 declaration, direct
    /// text normalization, and last-text-wins path merging. The first NUL or
    /// completed root ends input. Fields keep this API's rooted paths; only
    /// `document` is an implicit root for script-facing path queries.
    /// `parse` remains the independent raw/legacy format inspector.
    pub fn parse_runtime(data: &[u8]) -> Result<Self, ConfigError> {
        use std::collections::BTreeMap;
        let end = data.iter().position(|&byte| byte == 0).unwrap_or(data.len());
        let limited = end > MAX_XML_SIZE;
        let data = &data[..end.min(MAX_XML_SIZE)];
        let (mut position, encoding) = runtime_declaration(data)?;
        let legacy_encoding = if encoding.eq_ignore_ascii_case(b"Shift_JIS") {
            Some(encoding_rs::SHIFT_JIS)
        } else if encoding.eq_ignore_ascii_case(b"GB2312") {
            Some(encoding_rs::GBK)
        } else {
            None
        };
        if let Some(legacy_encoding) = legacy_encoding {
            // V401 uses CP932; Newton's localized config/startup.xml declares
            // GB2312. Strictly normalize both before shared UTF-8 consumers,
            // preserving the parser's merge, whitespace, and error behavior.
            let body = &data[position..];
            let text = legacy_encoding
                .decode_without_bom_handling_and_without_replacement(body)
                .or_else(|| {
                    // The shipped Chinese startup document retains its original
                    // Shift_JIS declaration. Match its exact GBK title value.
                    const LOCALIZED_TITLE: &[u8] = b"<title>\xcd\xbb\xc8\xbb\xd6\xae\xbc\xe4\xb7\xa2\xcf\xd6\xce\xd2\xd2\xd1\xc1\xb5\xc9\xcf\xc4\xe3</title>";
                    (legacy_encoding == encoding_rs::SHIFT_JIS
                        && body.windows(LOCALIZED_TITLE.len()).any(|value| value == LOCALIZED_TITLE))
                        .then(|| encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(body))
                        .flatten()
                })
                .ok_or(ConfigError::Malformed(position))?;
            let mut normalized = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_vec();
            normalized.extend_from_slice(text.as_bytes());
            return Self::parse_runtime(&normalized);
        }
        if !encoding.eq_ignore_ascii_case(b"UTF-8") { return Err(ConfigError::Malformed(0)); }
        let mut stack: Vec<(Vec<u8>, usize)> = Vec::new();
        let mut root = Vec::new();
        let mut fields: Vec<XmlField> = Vec::new();
        let mut indices: BTreeMap<Vec<Vec<u8>>, usize> = BTreeMap::new();
        let mut stored_paths = 0usize;
        while position < data.len() {
            if data[position] != b'<' {
                let next = data[position..].iter().position(|&byte| byte == b'<')
                    .map_or(data.len(), |offset| position + offset);
                let text = &data[position..next];
                if let Some(&(_, field)) = stack.last() {
                    append_runtime_text(&mut fields[field].text, text);
                } else if !text_is_whitespace(text) {
                    return Err(ConfigError::Malformed(position));
                }
                position = next;
                continue;
            }
            if data[position..].starts_with(b"<!--") {
                position = find_after(data, position + 4, b"-->")?;
                continue;
            }
            // No CDATA, DOCTYPE, or second processing declaration.
            if matches!(data.get(position + 1), Some(b'!' | b'?')) {
                return Err(ConfigError::Malformed(position));
            }
            let close = find_tag_end(data, position)?;
            let body = &data[position + 1..close];
            if let Some(body) = body.strip_prefix(b"/") {
                let body = body.trim_ascii_end();
                if body.is_empty() || body.iter().any(u8::is_ascii_whitespace)
                    || stack.last().is_none_or(|(name, _)| name.as_slice() != body)
                {
                    return Err(ConfigError::Malformed(position));
                }
                stack.pop();
                position = close + 1;
                if stack.is_empty() {
                    validate_runtime_utf8(&data[..position])?;
                    return Ok(Self { root, fields });
                }
                continue;
            }
            let (name, self_closing) = parse_start_tag(body, position)?;
            if name.len() > 511 { return Err(ConfigError::TooLarge); }
            if self_closing && !stack.is_empty() {
                return Err(ConfigError::Malformed(position));
            }
            if stack.len() == MAX_XML_DEPTH { return Err(ConfigError::TooLarge); }
            if stack.is_empty() { root.clone_from(&name); }
            let mut path: Vec<_> = stack.iter().map(|(name, _)| name.clone()).collect();
            path.push(name.clone());
            let field = if let Some(&field) = indices.get(&path) {
                // Descendant fields are separate map entries and survive.
                fields[field].text.clear();
                field
            } else {
                if fields.len() == MAX_XML_FIELDS { return Err(ConfigError::TooLarge); }
                let size = path.iter().map(Vec::len).sum::<usize>();
                stored_paths = stored_paths.checked_add(size).filter(|&size| size <= MAX_XML_SIZE)
                    .ok_or(ConfigError::TooLarge)?;
                let field = fields.len();
                indices.insert(path.clone(), field);
                fields.push(XmlField { path, text: Vec::new() });
                field
            };
            position = close + 1;
            if self_closing {
                validate_runtime_utf8(&data[..position])?;
                return Ok(Self { root, fields });
            }
            stack.push((name, field));
        }
        if limited { return Err(ConfigError::TooLarge); }
        if !stack.is_empty() { return Err(ConfigError::UnclosedElement); }
        Err(ConfigError::Malformed(position))
    }

    /// Parses XML structure while preserving text and names as their original bytes.
    ///
    /// This supports the ASCII XML markup and raw CP932/GB2312/UTF-8 text used
    /// in startup and sound configuration files; entities remain literal.
    pub fn parse(data: &[u8]) -> Result<Self, ConfigError> {
        if data.len() > MAX_XML_SIZE {
            return Err(ConfigError::TooLarge);
        }
        let mut position = 0usize;
        let mut stack: Vec<(Vec<u8>, usize)> = Vec::new();
        let mut root = Vec::new();
        let mut fields: Vec<XmlField> = Vec::new();

        while position < data.len() {
            if data[position] != b'<' {
                let end = data[position..]
                    .iter()
                    .position(|&byte| byte == b'<')
                    .map_or(data.len(), |relative| position + relative);
                if !stack.is_empty() {
                    let text = &data[position..end];
                    if !text.is_empty() {
                        fields[stack.last().unwrap().1].text.extend_from_slice(text);
                    }
                } else if !text_is_whitespace(&data[position..end]) {
                    return Err(ConfigError::Malformed(position));
                }
                position = end;
                continue;
            }

            if data[position..].starts_with(b"<!--") {
                position = find_after(data, position + 4, b"-->")?;
                continue;
            }
            if data[position..].starts_with(b"<?") {
                position = find_after(data, position + 2, b"?>")?;
                continue;
            }
            if data[position..].starts_with(b"<![CDATA[") {
                let end = find_after(data, position + 9, b"]]>")?;
                if stack.is_empty() {
                    return Err(ConfigError::Malformed(position));
                }
                let text = &data[position + 9..end - 3];
                fields[stack.last().unwrap().1].text.extend_from_slice(text);
                position = end;
                continue;
            }
            if data[position..].starts_with(b"<!") {
                return Err(ConfigError::Malformed(position));
            }

            let close = find_tag_end(data, position)?;
            let mut body = &data[position + 1..close];
            if body.first() == Some(&b'/') {
                body = &body[1..];
                body = body.trim_ascii();
                let name_end = body
                    .iter()
                    .position(u8::is_ascii_whitespace)
                    .unwrap_or(body.len());
                if name_end == 0 || !body[name_end..].trim_ascii().is_empty() {
                    return Err(ConfigError::Malformed(position));
                }
                let name = &body[..name_end];
                if stack.last().is_none_or(|(current, _)| current != name) {
                    return Err(ConfigError::Malformed(position));
                }
                stack.pop();
                position = close + 1;
                continue;
            }

            let (name, self_closing) = parse_start_tag(body, position)?;
            if stack.is_empty() {
                if !root.is_empty() {
                    return Err(ConfigError::Malformed(position));
                }
                root.clone_from(&name);
            }
            if stack.len() == MAX_XML_DEPTH || fields.len() == MAX_XML_FIELDS {
                return Err(ConfigError::TooLarge);
            }
            let mut path = stack
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            path.push(name.clone());
            let field = if let Some(index) = fields.iter().position(|field| field.path == path) {
                fields[index].text.clear();
                index
            } else {
                let index = fields.len();
                fields.push(XmlField {
                    path,
                    text: Vec::new(),
                });
                index
            };
            stack.push((name, field));
            if self_closing {
                stack.pop();
            }
            position = close + 1;
        }

        if !stack.is_empty() {
            return Err(ConfigError::UnclosedElement);
        }
        if root.is_empty() {
            return Err(ConfigError::Malformed(0));
        }
        Ok(Self { root, fields })
    }

    pub fn text(&self, path: &[&[u8]]) -> Option<&[u8]> {
        self.fields
            .iter()
            .find(|field| {
                field
                    .path
                    .iter()
                    .map(Vec::as_slice)
                    .eq(path.iter().copied())
            })
            .map(|field| field.text.as_slice())
    }

    /// Read an untrimmed, wrapping integer prefix. A point immediately after
    /// decimal digits invalidates the value; hexadecimal prefixes stop there.
    /// Only a missing field uses `default`, not empty or invalid text.
    pub fn integer(&self, path: &[&[u8]], default: i32) -> i32 {
        self.text(path).map_or(default, parse_integer_prefix)
    }
}

fn validate_runtime_utf8(data: &[u8]) -> Result<(), ConfigError> {
    std::str::from_utf8(data).map(|_| ()).map_err(|error| ConfigError::Malformed(error.valid_up_to()))
}

fn runtime_declaration(data: &[u8]) -> Result<(usize, &[u8]), ConfigError> {
    if !data.starts_with(b"<?xml") || !data.get(5).is_some_and(u8::is_ascii_whitespace) {
        return Err(ConfigError::Malformed(0));
    }
    let end = find_after(data, 5, b"?>")?;
    let body = &data[5..end - 2];
    let mut cursor = 0;
    let mut version = None;
    let mut encoding = None;
    while cursor < body.len() {
        if !body[cursor].is_ascii_whitespace() { return Err(ConfigError::Malformed(5 + cursor)); }
        while body.get(cursor).is_some_and(u8::is_ascii_whitespace) { cursor += 1; }
        if cursor == body.len() { break; }
        let start = cursor;
        while body.get(cursor).is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'=') {
            cursor += 1;
        }
        let name = &body[start..cursor];
        if name.is_empty() { return Err(ConfigError::Malformed(5 + cursor)); }
        while body.get(cursor).is_some_and(u8::is_ascii_whitespace) { cursor += 1; }
        if body.get(cursor) != Some(&b'=') { return Err(ConfigError::Malformed(5 + cursor)); }
        cursor += 1;
        while body.get(cursor).is_some_and(u8::is_ascii_whitespace) { cursor += 1; }
        let quote = *body.get(cursor).ok_or(ConfigError::Malformed(5 + cursor))?;
        if !matches!(quote, b'\'' | b'"') { return Err(ConfigError::Malformed(5 + cursor)); }
        cursor += 1;
        let start = cursor;
        while body.get(cursor).is_some_and(|&byte| byte != quote) { cursor += 1; }
        if cursor == body.len() { return Err(ConfigError::Malformed(5 + cursor)); }
        let value = &body[start..cursor];
        match name {
            b"version" if version.is_none() => version = Some(value),
            b"encoding" if encoding.is_none() => encoding = Some(value),
            b"version" | b"encoding" => return Err(ConfigError::Malformed(5 + start)),
            _ => {}
        }
        cursor += 1;
    }
    if version != Some(b"1.0".as_slice()) || encoding.is_none() {
        return Err(ConfigError::Malformed(0));
    }
    Ok((end, encoding.unwrap()))
}

fn append_runtime_text(target: &mut Vec<u8>, text: &[u8]) {
    let mut cursor = 0;
    while cursor < text.len() {
        match text[cursor] {
            b'\r' if text.get(cursor + 1) == Some(&b'\n') => { target.push(b' '); cursor += 2; }
            b'\n' | b'\t' => { target.push(b' '); cursor += 1; }
            byte => { target.push(byte); cursor += 1; }
        }
    }
}

fn find_after(data: &[u8], start: usize, delimiter: &[u8]) -> Result<usize, ConfigError> {
    data[start..]
        .windows(delimiter.len())
        .position(|window| window == delimiter)
        .map(|relative| start + relative + delimiter.len())
        .ok_or(ConfigError::Malformed(start))
}

fn text_is_whitespace(data: &[u8]) -> bool {
    data.iter().all(u8::is_ascii_whitespace)
}

fn find_tag_end(data: &[u8], start: usize) -> Result<usize, ConfigError> {
    let mut quote = None;
    for (offset, &byte) in data.iter().enumerate().skip(start + 1) {
        match quote {
            Some(delimiter) if byte == delimiter => quote = None,
            Some(_) => {}
            None if byte == b'\'' || byte == b'"' => quote = Some(byte),
            None if byte == b'<' => return Err(ConfigError::Malformed(offset)),
            None if byte == b'>' => return Ok(offset),
            None => {}
        }
    }
    Err(ConfigError::Malformed(start))
}

fn parse_start_tag(body: &[u8], offset: usize) -> Result<(Vec<u8>, bool), ConfigError> {
    let body = body.trim_ascii_end();
    let self_closing = body.last() == Some(&b'/');
    let body = if self_closing {
        &body[..body.len() - 1]
    } else {
        body
    };
    let name_end = body
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(body.len());
    if name_end == 0
        || body[..name_end]
            .iter()
            .any(|byte| matches!(*byte, b'/' | b'=' | b'<' | b'>'))
    {
        return Err(ConfigError::Malformed(offset));
    }

    let name = body[..name_end].to_vec();
    let mut cursor = name_end;
    while cursor < body.len() {
        while body.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if cursor == body.len() {
            break;
        }
        let attribute_start = cursor;
        while body.get(cursor).is_some_and(|byte| {
            !byte.is_ascii_whitespace() && !matches!(*byte, b'=' | b'/' | b'<' | b'>')
        }) {
            cursor += 1;
        }
        if cursor == attribute_start {
            return Err(ConfigError::Malformed(offset));
        }
        while body.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if body.get(cursor) != Some(&b'=') {
            return Err(ConfigError::Malformed(offset));
        }
        cursor += 1;
        while body.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let delimiter = *body.get(cursor).ok_or(ConfigError::Malformed(offset))?;
        if delimiter != b'\'' && delimiter != b'"' {
            return Err(ConfigError::Malformed(offset));
        }
        cursor += 1;
        let value_start = cursor;
        while body
            .get(cursor)
            .is_some_and(|&byte| byte != delimiter && byte != b'<')
        {
            cursor += 1;
        }
        if cursor == body.len() || body.get(cursor) != Some(&delimiter) {
            return Err(ConfigError::Malformed(offset + value_start));
        }
        cursor += 1;
    }
    Ok((name, self_closing))
}

/// CatSystem's shared integer parser (NEKO6a4db0): optional sign, decimal
/// or hexadecimal prefix, DWORD wrapping and decimal-point rejection.
pub fn parse_integer_prefix(bytes: &[u8]) -> i32 {
    let mut input = bytes;
    let negative = match input.first() {
        Some(b'-') => {
            input = &input[1..];
            true
        }
        Some(b'+') => {
            input = &input[1..];
            false
        }
        _ => false,
    };
    let (digits, radix) = if input.starts_with(b"0x") || input.starts_with(b"0X") {
        (&input[2..], 16)
    } else {
        (input, 10)
    };
    let digit_count = digits
        .iter()
        .position(|byte| match radix {
            16 => !byte.is_ascii_hexdigit(),
            _ => !byte.is_ascii_digit(),
        })
        .unwrap_or(digits.len());
    // Original XML integer conversion rejects a decimal point directly
    // after its digit run, even when the rest would be an ordinary suffix.
    // Hexadecimal conversion keeps the already consumed integer instead.
    if radix == 10 && digits.get(digit_count) == Some(&b'.') {
        return 0;
    }
    let value = digits[..digit_count]
        .iter()
        .fold(0u32, |value, &byte| {
            let digit = if byte.is_ascii_digit() {
                (byte - b'0') as u32
            } else {
                (byte.to_ascii_lowercase() - b'a' + 10) as u32
            };
            value.wrapping_mul(radix).wrapping_add(digit)
        });
    (if negative {
        value.wrapping_neg()
    } else {
        value
    }) as i32
}
