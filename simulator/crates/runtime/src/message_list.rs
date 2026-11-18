//! Type-47 CST inspection rows. This is independent of the scrolling backlog:
//! the original viewer concatenates raw 20/21 records and keeps input ids.
use cs2_script::CstDocument;

use crate::{RuntimeError, RuntimeResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageListRow {
    pub input: u32,
    pub name: Vec<u8>,
    pub message: Vec<u8>,
}

impl MessageListRow {
    pub fn matches(&self, name: Option<&[u8]>, monologue: bool, text: &[u8]) -> bool {
        (!monologue || self.name.is_empty())
            && name.is_none_or(|name| contains(&self.name, name))
            && contains(&self.message, text)
    }
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || bytes.windows(needle.len()).any(|part| part == needle)
}

/// One bounded input scan per step. A first-record query of -1 ends the
/// original enumeration, including when an input's record range is empty.
pub struct MessageListScan {
    document: CstDocument,
    next: usize,
    rows: Vec<MessageListRow>,
    complete: bool,
    row_bytes: usize,
}

impl MessageListScan {
    pub fn new(document: CstDocument) -> RuntimeResult<Self> {
        if document.inputs.len() > 1_000_000 || document.records.len() > 1_000_000 {
            return Err(RuntimeError::Host("message list source exceeds one million records".into()));
        }
        let bytes = document.records.iter().try_fold(0usize, |sum, record|
            sum.checked_add(record.text.len()).filter(|&bytes| bytes <= 64*1024*1024))
            .ok_or_else(|| RuntimeError::Host("message-list source exceeds 64 MiB".into()))?;
        let _ = bytes;
        Ok(Self { document, next: 0, rows: Vec::new(), complete: false, row_bytes: 0 })
    }

    pub fn advance(&mut self, budget: usize) -> RuntimeResult<bool> {
        for _ in 0..budget {
            if self.complete { return Ok(true); }
            let Some(records) = self.document.input_records(self.next) else {
                self.complete = true;
                return Ok(true);
            };
            if records.is_empty() {
                self.complete = true;
                return Ok(true);
            }
            let mut name = Vec::new();
            let mut message = Vec::new();
            for record in records {
                let target = match record.kind {
                    0x20 => &mut message,
                    0x21 => &mut name,
                    _ => continue,
                };
                let text = record.text.split(|byte| *byte == 0).next().unwrap_or_default();
                if target.len().saturating_add(text.len()) >= 2048 {
                    return Err(RuntimeError::Host("message list raw text exceeds its 2047-byte field".into()));
                }
                target.extend_from_slice(text);
            }
            // The receiver writes NUL at byte 62 after concatenating names,
            // even if this cuts a multibyte character. Keep those exact bytes.
            name.truncate(62);
            if !name.is_empty() || !message.is_empty() {
                self.row_bytes = self.row_bytes.checked_add(16 + name.len() + message.len())
                    .filter(|&bytes| bytes <= 64*1024*1024)
                    .ok_or_else(|| RuntimeError::Host("message-list rows exceed 64 MiB".into()))?;
                self.rows.push(MessageListRow { input: self.next as u32, name, message });
            }
            self.next += 1;
        }
        Ok(self.complete)
    }

    pub fn rows(&self) -> &[MessageListRow] { &self.rows }
}
