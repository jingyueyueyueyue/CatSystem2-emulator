use thiserror::Error;

const MAX_SOURCE_SIZE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WssSource {
    pub source: Vec<u8>,
    pub lines: Vec<std::ops::Range<usize>>,
    pub includes: Vec<Vec<u8>>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WssError {
    #[error("WSS source exceeds the format reader limit")]
    TooLarge,
    #[error("WSS source contains an embedded NUL byte")]
    EmbeddedNul,
}

impl WssSource {
    /// Indexes source lines without decoding CP932 or attempting compilation.
    pub fn parse(data: &[u8]) -> Result<Self, WssError> {
        if data.len() > MAX_SOURCE_SIZE {
            return Err(WssError::TooLarge);
        }
        if data.contains(&0) {
            return Err(WssError::EmbeddedNul);
        }

        let mut lines = Vec::new();
        let mut includes = Vec::new();
        let mut in_block_comment = false;
        let mut start = 0;
        for (offset, &byte) in data.iter().enumerate() {
            if byte == b'\n' {
                lines.push(start..offset);
                let line = strip_comments(&data[start..offset], &mut in_block_comment);
                if let Some(name) = include_name(&line) {
                    includes.push(name.to_vec());
                }
                start = offset + 1;
            }
        }
        if start < data.len() {
            lines.push(start..data.len());
            let line = strip_comments(&data[start..], &mut in_block_comment);
            if let Some(name) = include_name(&line) {
                includes.push(name.to_vec());
            }
        }
        Ok(Self {
            source: data.to_vec(),
            lines,
            includes,
        })
    }

    pub fn line(&self, index: usize) -> Option<&[u8]> {
        let range = self.lines.get(index)?;
        let line = &self.source[range.clone()];
        Some(line.strip_suffix(b"\r").unwrap_or(line))
    }
}

fn include_name(line: &[u8]) -> Option<&[u8]> {
    let line = line.strip_suffix(b"\r").unwrap_or(line).trim_ascii();
    let argument = line.strip_prefix(b"#include")?.trim_ascii_start();
    let quote = *argument.first()?;
    let closing = match quote {
        b'"' => b'"',
        b'<' => b'>',
        _ => return None,
    };
    let end = argument[1..].iter().position(|&byte| byte == closing)? + 1;
    let trailing = argument.get(end + 1..).unwrap_or_default();
    if end <= 1 || !trailing.trim_ascii().is_empty() {
        return None;
    }
    Some(&argument[1..end])
}

fn strip_comments(line: &[u8], in_block_comment: &mut bool) -> Vec<u8> {
    let mut output = Vec::with_capacity(line.len());
    let mut index = 0usize;
    let mut quote = None;
    while index < line.len() {
        if is_multibyte_pair(line, index) {
            if !*in_block_comment {
                output.extend_from_slice(&line[index..index + 2]);
            }
            index += 2;
            continue;
        }
        if *in_block_comment {
            if line.get(index..index + 2) == Some(b"*/") {
                *in_block_comment = false;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            let byte = line[index];
            output.push(byte);
            if byte == b'\\' {
                if let Some(&escaped) = line.get(index + 1) {
                    output.push(escaped);
                    index += 2;
                    continue;
                }
            } else if byte == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }
        if line.get(index..index + 2) == Some(b"/*") {
            *in_block_comment = true;
            index += 2;
        } else if line.get(index..index + 2) == Some(b"//") {
            break;
        } else {
            let byte = line[index];
            output.push(byte);
            if byte == b'"' || byte == b'\'' {
                quote = Some(byte);
            }
            index += 1;
        }
    }
    output
}

fn is_multibyte_pair(line: &[u8], index: usize) -> bool {
    matches!(line.get(index), Some(0x81..=0x9f | 0xe0..=0xef))
        && line
            .get(index + 1)
            .is_some_and(|byte| matches!(*byte, 0x40..=0x7e | 0x80..=0xfc))
}
