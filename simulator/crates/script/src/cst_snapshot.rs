//! Portable, versioned full independent-CST-context state for a host save
//! graph. This is an application save schema, not ordinary385's table format.
use serde::{Deserialize, Serialize};

use crate::{CstDocument, CstRecord, Result, ScriptError};
use cs2_formats::{CstlMessage, CstlTable};

const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CstContextStoreSnapshot {
    pub version: u32,
    pub contexts: Vec<CstContextSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CstContextSnapshot {
    pub handle: u32,
    pub position: [i32; 2],
    /// 0 = CP932, 1 = GBK, 2 = UTF-8. Captured at successful scene load.
    pub encoding: u32,
    pub document: Option<CstDocumentSnapshot>,
    pub registered_languages: Vec<Vec<u8>>,
    pub localization: Option<CstLocalizationSnapshot>,
    /// Current table wire format, including attachments and opaque data.
    /// A host restore must not apply ordinary386's absent-second-block rule.
    pub registrations: Vec<u8>,
    pub history: Vec<u8>,
    #[serde(default)]
    pub source_name: Vec<u8>,
    #[serde(default)]
    pub source_line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CstDocumentSnapshot {
    pub records: Vec<(u8, Vec<u8>)>,
    pub inputs: Vec<[u32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CstLocalizationSnapshot {
    pub languages: Vec<String>,
    pub messages: Vec<Vec<(String, String)>>,
}

impl CstDocumentSnapshot {
    pub fn storage_bytes(&self) -> Result<usize> {
        self.validate()?;
        self.records.iter().try_fold(self.inputs.len().saturating_mul(8), |total, (_, text)| {
            total.checked_add(text.len()).and_then(|total| total.checked_add(32))
                .filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("CST document snapshot storage exceeds 256 MiB"))
        })
    }

    pub fn capture(document: &CstDocument) -> Result<Self> {
        let snapshot = Self {
            records: document.records.iter().map(|record| (record.kind, record.text.clone())).collect(),
            inputs: document.inputs.iter().map(|range| {
                Ok([u32::try_from(range.start).map_err(|_| invalid("CST input start exceeds DWORD"))?,
                    u32::try_from(range.end).map_err(|_| invalid("CST input end exceeds DWORD"))?])
            }).collect::<Result<_>>()?,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn validate(&self) -> Result<()> {
        if self.records.len() > MAX_ITEMS || self.inputs.len() > MAX_ITEMS {
            return Err(invalid("CST document snapshot exceeds its item limit"));
        }
        let mut end = 0usize;
        for &[begin, next] in &self.inputs {
            if begin as usize != end || next < begin || next as usize > self.records.len() {
                return Err(invalid("CST document snapshot has invalid input ranges"));
            }
            end = next as usize;
        }
        if end != self.records.len() || self.records.iter().any(|(_, text)| text.contains(&0)) {
            return Err(invalid("CST document snapshot has uncovered or unterminated record structure"));
        }
        self.records.iter().try_fold(0usize, |total, (_, bytes)| {
            total.checked_add(bytes.len()).filter(|&total| total <= 64 * 1024 * 1024)
                .ok_or_else(|| invalid("CST document snapshot exceeds 64 MiB"))
        })?;
        Ok(())
    }

    pub fn restore(&self) -> Result<CstDocument> {
        self.validate()?;
        Ok(CstDocument {
            records: self.records.iter().map(|(kind, text)| CstRecord { kind: *kind, text: text.clone() }).collect(),
            inputs: self.inputs.iter().map(|range| range[0] as usize..range[1] as usize).collect(),
        })
    }
}

impl CstLocalizationSnapshot {
    pub fn capture(table: &CstlTable) -> Self {
        Self { languages: table.languages.clone(), messages: table.messages.iter().map(|row| {
            row.iter().map(|message| (message.name.clone(), message.content.clone())).collect()
        }).collect() }
    }

    fn validate(&self) -> Result<()> {
        if self.languages.is_empty() || self.languages.len() > MAX_ITEMS
            || self.messages.len().checked_mul(self.languages.len()).is_none_or(|count| count > MAX_ITEMS)
            || self.messages.iter().any(|row| row.len() != self.languages.len())
        {
            return Err(invalid("CST localization snapshot dimensions are invalid"));
        }
        let mut languages = self.languages.iter().map(String::as_str).collect::<Vec<_>>();
        languages.sort_unstable();
        if languages.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid("CST localization snapshot repeats a language"));
        }
        self.languages.iter().map(String::as_str)
            .chain(self.messages.iter().flatten().flat_map(|(name, content)|
                [name.as_str(), content.as_str()]))
            .try_fold(0usize, |total, text| {
                total.checked_add(text.len()).filter(|&total| total <= MAX_BYTES)
                    .ok_or_else(|| invalid("CST localization snapshot exceeds 256 MiB"))
            })?;
        Ok(())
    }

    pub fn restore(&self) -> Result<CstlTable> {
        self.validate()?;
        Ok(CstlTable { languages: self.languages.clone(), messages: self.messages.iter().map(|row| {
            row.iter().map(|(name, content)| CstlMessage { name: name.clone(), content: content.clone() }).collect()
        }).collect() })
    }
}

impl CstContextStoreSnapshot {
    /// Host still validates cross-type handle identities as part of its full
    /// object graph. This validates the complete CST group before installation.
    pub fn validate(&self) -> Result<()> {
        self.storage_bytes().map(|_| ())
    }

    pub fn storage_bytes(&self) -> Result<usize> {
        if self.version != 1 || self.contexts.len() > MAX_ITEMS {
            return Err(invalid("CST context snapshot version or count is invalid"));
        }
        let mut handles = std::collections::BTreeSet::new();
        let mut total = 0usize;
        for context in &self.contexts {
            if context.handle == 0 || !handles.insert(context.handle) || context.encoding > 2
                || context.registered_languages.len() > MAX_ITEMS
                || context.registered_languages.windows(2).any(|pair| pair[0] >= pair[1])
                || context.registered_languages.iter().any(|name| name.contains(&0))
                || context.source_name.len() >= 260 || context.source_name.contains(&0)
            {
                return Err(invalid("CST context identity, encoding or registered languages are invalid"));
            }
            for bytes in [&context.registrations, &context.history] {
                total = total.checked_add(bytes.len()).filter(|&total| total <= MAX_BYTES)
                    .ok_or_else(|| invalid("CST context snapshots exceed 256 MiB"))?;
            }
            total = total.checked_add(context.source_name.len()).filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("CST context snapshots exceed 256 MiB"))?;
            for language in &context.registered_languages {
                total = total.checked_add(language.len()).filter(|&total| total <= MAX_BYTES)
                    .ok_or_else(|| invalid("CST context snapshots exceed 256 MiB"))?;
            }
            if let Some(document) = &context.document {
                document.validate()?;
                for (_, text) in &document.records {
                    total = total.checked_add(text.len()).filter(|&total| total <= MAX_BYTES)
                        .ok_or_else(|| invalid("CST context snapshots exceed 256 MiB"))?;
                }
            }
            if let Some(table) = &context.localization {
                // Check lengths before making an owned localization table.
                for text in table.languages.iter().map(String::as_str)
                    .chain(table.messages.iter().flatten().flat_map(|(name, content)|
                        [name.as_str(), content.as_str()]))
                {
                    total = total.checked_add(text.len()).filter(|&total| total <= MAX_BYTES)
                        .ok_or_else(|| invalid("CST context snapshots exceed 256 MiB"))?;
                }
                table.validate()?;
            }
        }
        Ok(total)
    }
}
