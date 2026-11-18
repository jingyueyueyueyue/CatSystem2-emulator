use thiserror::Error;

const MAX_CSTL_BYTES: usize = 256 * 1024 * 1024;
const MAX_CSTL_ITEMS: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstlMessage {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstlTable {
    pub languages: Vec<String>,
    pub messages: Vec<Vec<CstlMessage>>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CstlError {
    #[error("invalid CSTL signature")]
    InvalidSignature,
    #[error("CSTL header is truncated")]
    Truncated,
    #[error("unsupported CSTL header field")]
    UnsupportedHeader,
    #[error("CSTL FF integer is invalid")]
    InvalidInteger,
    #[error("CSTL string is invalid UTF-8")]
    InvalidUtf8,
    #[error("CSTL table exceeds limits")]
    TooLarge,
    #[error("CSTL data has trailing bytes")]
    TrailingBytes,
}

impl CstlTable {
    pub fn parse(data: &[u8]) -> Result<Self, CstlError> {
        let (table, consumed) = Self::parse_prefix(data)?;
        if consumed != data.len() {
            return Err(CstlError::TrailingBytes);
        }
        Ok(table)
    }

    /// Runtime scene loading accepts a complete CSTL followed by extra bytes.
    /// The strict standalone parser above continues to reject that suffix.
    pub fn parse_prefix(data: &[u8]) -> Result<(Self, usize), CstlError> {
        if data.len() > MAX_CSTL_BYTES {
            return Err(CstlError::TooLarge);
        }
        if data.len() < 4 {
            return Err(CstlError::Truncated);
        }
        if data.get(..4) != Some(b"CSTL") {
            return Err(CstlError::InvalidSignature);
        }
        let mut cursor = 4;
        if read_u32(data, &mut cursor)? != 0 {
            return Err(CstlError::UnsupportedHeader);
        }
        let language_count = read_ff_integer(data, &mut cursor)?;
        if language_count == 0 || language_count > MAX_CSTL_ITEMS {
            return Err(CstlError::TooLarge);
        }
        let mut languages = Vec::with_capacity(language_count);
        for _ in 0..language_count {
            let language = read_string(data, &mut cursor)?;
            if languages.contains(&language) {
                return Err(CstlError::InvalidInteger);
            }
            languages.push(language);
        }
        let message_count = read_ff_integer(data, &mut cursor)?;
        if message_count > MAX_CSTL_ITEMS
            || message_count
                .checked_mul(language_count)
                .is_none_or(|count| count > MAX_CSTL_ITEMS)
        {
            return Err(CstlError::TooLarge);
        }
        let mut messages = Vec::with_capacity(message_count);
        for _ in 0..message_count {
            let mut row = Vec::with_capacity(language_count);
            for _ in 0..language_count {
                row.push(CstlMessage {
                    name: read_string(data, &mut cursor)?,
                    content: read_string(data, &mut cursor)?,
                });
            }
            messages.push(row);
        }
        Ok((Self {
            languages,
            messages,
        }, cursor))
    }

    pub fn message(&self, language: &str, index: usize) -> Option<&CstlMessage> {
        let language = self.languages.iter().position(|value| value == language)?;
        self.messages.get(index)?.get(language)
    }

    pub fn segment(&self, language: &str, index: usize, fragment: usize) -> Option<&str> {
        let content = &self.message(language, index)?.content;
        content.split('\u{1f}').nth(fragment)
    }

    pub fn option(&self, language: &str, input_count: usize, choice: usize) -> Option<String> {
        let index = input_count.checked_sub(1)?.checked_add(choice)?;
        let content = &self.message(language, index)?.content;
        Some(
            content
                .split('\u{1f}')
                .take_while(|part| !part.is_empty())
                .collect::<String>(),
        )
    }
}

fn read_u32(data: &[u8], cursor: &mut usize) -> Result<u32, CstlError> {
    let end = cursor.checked_add(4).ok_or(CstlError::Truncated)?;
    let value = data
        .get(*cursor..end)
        .ok_or(CstlError::Truncated)?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| CstlError::Truncated)?;
    *cursor = end;
    Ok(value)
}

fn read_ff_integer(data: &[u8], cursor: &mut usize) -> Result<usize, CstlError> {
    let mut value = 0usize;
    loop {
        let byte = *data.get(*cursor).ok_or(CstlError::Truncated)?;
        *cursor += 1;
        value = value
            .checked_add(usize::from(byte))
            .filter(|value| *value <= MAX_CSTL_BYTES)
            .ok_or(CstlError::TooLarge)?;
        if byte != 0xff {
            return Ok(value);
        }
    }
}

fn read_string(data: &[u8], cursor: &mut usize) -> Result<String, CstlError> {
    let length = read_ff_integer(data, cursor)?;
    let end = cursor.checked_add(length).ok_or(CstlError::Truncated)?;
    let bytes = data.get(*cursor..end).ok_or(CstlError::Truncated)?;
    *cursor = end;
    String::from_utf8(bytes.to_vec()).map_err(|_| CstlError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ff(value: usize, output: &mut Vec<u8>) {
        let mut value = value;
        while value >= 255 {
            output.push(255);
            value -= 255;
        }
        output.push(value as u8);
    }

    fn string(value: &str, output: &mut Vec<u8>) {
        ff(value.len(), output);
        output.extend_from_slice(value.as_bytes());
    }

    #[test]
    fn parses_ff_lengths_and_message_segments() {
        let mut data = b"CSTL".to_vec();
        data.extend_from_slice(&0u32.to_le_bytes());
        ff(2, &mut data);
        string("ja", &mut data);
        string("zh", &mut data);
        ff(1, &mut data);
        string("名前", &mut data);
        string("first\u{1f}second", &mut data);
        string("姓名", &mut data);
        string("第一\u{1f}第二", &mut data);
        let table = CstlTable::parse(&data).unwrap();
        assert_eq!(table.languages, ["ja", "zh"]);
        assert_eq!(table.segment("zh", 0, 1), Some("第二"));
        assert_eq!(table.message("ja", 0).unwrap().name, "名前");
    }

    #[test]
    fn rejects_trailing_bytes_and_duplicate_languages() {
        let mut data = b"CSTL".to_vec();
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&[2, 2, b'e', b'n', 2, b'e', b'n', 0, 0]);
        assert!(matches!(
            CstlTable::parse(&data),
            Err(CstlError::TrailingBytes | CstlError::InvalidInteger)
        ));
    }
}
