//! Per-script gallery queries over the game's CGL table and CSS registration tree.
use anyhow::{Context, Result};
use cs2_runtime::{GalleryEntry, NativeSequence, resource_router::ResourceMode};
use cs2_script::{ExpressionState, data_snapshot::SequenceSnapshot};

use super::LoggingHost;
use super::fes_driver::{FesDriverCommand, FesDriverEffect, FesScriptDriver};

#[derive(Clone, Debug, Default)]
pub(super) struct Gallery {
    sequence: NativeSequence,
    entries: Vec<GalleryEntry>,
}

impl Gallery {
    fn new(table: Vec<u8>, registrations: &[u8]) -> Result<Self> {
        let mut sequence = NativeSequence::default();
        sequence.load_file(table)?;
        sequence.restore_tree(registrations)?;
        Self::from_sequence(sequence)
    }

    fn from_sequence(sequence: NativeSequence) -> Result<Self> {
        let entries = sequence.gallery_entries()?;
        Ok(Self { sequence, entries })
    }

    fn named(&self, category: u32, name: &[u8]) -> Option<&GalleryEntry> {
        self.entries.iter().find(|entry| entry.category == category && entry.name.eq_ignore_ascii_case(name))
    }
}

impl cs2_runtime::graph_serde::Serialize for Gallery {
    fn serialize<S: cs2_runtime::graph_serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use cs2_runtime::graph_serde::ser::Error;
        self.sequence.snapshot().map_err(S::Error::custom)?.serialize(serializer)
    }
}

impl<'de> cs2_runtime::graph_serde::Deserialize<'de> for Gallery {
    fn deserialize<D: cs2_runtime::graph_serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use cs2_runtime::graph_serde::de::Error;
        let snapshot = SequenceSnapshot::deserialize(deserializer)?;
        let sequence = NativeSequence::from_snapshot(&snapshot).map_err(D::Error::custom)?;
        Self::from_sequence(sequence).map_err(D::Error::custom)
    }
}

pub(super) fn apply_effect(
    host: &LoggingHost<'_>, driver: &mut FesScriptDriver,
    effect: &FesDriverEffect, shared: &mut ExpressionState,
) -> Result<bool> {
    let FesDriverEffect::Command(command) = effect else { return Ok(false); };
    let [verb, operands @ ..] = command.parameters.as_slice() else { return Ok(false); };
    if !verb.get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"cgreg")) {
        return Ok(false);
    }
    let current = command.context.current;
    if verb.eq_ignore_ascii_case(b"cgreg_init") {
        if let [save, record, table, ..] = operands {
            let save = host.text_encoding.decode(save);
            let table = host.text_encoding.decode(table);
            let record = driver.evaluate_parameter(record, current, shared)? as u32;
            let save = host.read_fes_css_save(&save)?;
            let registrations = save.as_ref().and_then(|file| file.record(record)).unwrap_or_default();
            let data = host.resources.read_mode(&table, ResourceMode::LooseFirst)?.unwrap_or_default();
            driver.gallery = Gallery::new(data, registrations)
                .with_context(|| format!("FES gallery table {table:?}"))?;
        } else {
            driver.gallery = Gallery::default();
        }
        return Ok(true);
    }

    let verb = verb.to_ascii_lowercase();
    let (key, output, category) = match (verb.as_slice(), operands) {
        (b"cgreg" | b"cgreg_t", [output, key, tail @ ..])
        | (b"cgregnum" | b"cgregmax" | b"cgregtbl", [key, output, tail @ ..]) => (key, output, tail.first()),
        (b"cgreg" | b"cgreg_t" | b"cgregnum" | b"cgregmax" | b"cgregtbl", _) => return Ok(true),
        _ => return Ok(false),
    };
    let output = driver.evaluate_parameter(output, current, shared)?;
    let category = category.map(|value| driver.evaluate_parameter(value, current, shared))
        .transpose()?.unwrap_or(0) as u32;
    let value = match verb.as_slice() {
        b"cgreg" => i32::from(driver.gallery.sequence.contains(key)?),
        b"cgregtbl" => {
            if driver.gallery.entries.is_empty() { 0 } else {
                driver.gallery.entries.iter().position(|entry| entry.name.eq_ignore_ascii_case(key))
                    .map_or(-1, |index| index as i32)
            }
        }
        _ => {
            let entry = find_entry(driver, key, category, current, shared)?;
            entry.map_or(0, |entry| match verb.as_slice() {
                b"cgreg_t" => i32::from(entry.variants.iter().any(|variant| variant.registered)),
                b"cgregnum" => entry.variants.iter().filter(|variant| variant.registered).count() as i32,
                _ => entry.variants.len() as i32,
            })
        }
    };
    if let Some(slot) = shared.shared.get_mut(output as usize) {
        *slot = value;
    }
    Ok(true)
}

fn find_entry<'a>(
    driver: &'a mut FesScriptDriver, key: &[u8], category: u32,
    current: i32, shared: &mut ExpressionState,
) -> Result<Option<&'a GalleryEntry>> {
    if key.first().is_some_and(|byte| byte.is_ascii_digit() || matches!(byte, b'$' | b'\\' | b'(' | b'-' | b'+')) {
        let index = driver.evaluate_parameter(key, current, shared)?;
        Ok(driver.gallery.entries.get(index as usize))
    } else {
        Ok(driver.gallery.named(category, key))
    }
}

pub(super) fn image_source(
    driver: &mut FesScriptDriver, command: &FesDriverCommand,
    shared: &mut ExpressionState, unlocked_only: bool,
) -> Result<Option<(Vec<u8>, i32)>> {
    let [_, _, category, key, optional @ ..] = command.parameters.as_slice() else { return Ok(None); };
    let current = command.context.current;
    let category = driver.evaluate_parameter(category, current, shared)? as u32;
    let variant = optional.first().map(|value| driver.evaluate_parameter(value, current, shared))
        .transpose()?.unwrap_or(0);
    let frame = optional.get(1).map(|value| driver.evaluate_parameter(value, current, shared))
        .transpose()?.unwrap_or(0);
    if let Some(entry) = find_entry(driver, key, category, current, shared)? {
        return Ok(entry.variants.iter().filter(|variant| variant.registered || !unlocked_only)
            .nth(variant as usize).map(|variant| (variant.name.clone(), frame)));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_runtime::object_graph::{decode_component, encode_component};
    use cs2_script::{FesDocument, FesLine, FesSection};

    #[test]
    fn gallery_driver_checkpoint_preserves_unlocks_and_reads_older_layout() -> Result<()> {
        let mut driver = FesScriptDriver::new(&FesDocument {
            sections: vec![FesSection {
                header: b"START".to_vec(),
                lines: vec![FesLine { offset: 0, bytes: b"exit 0".to_vec() }],
            }],
        })?;
        let mut table = b"CGL\0".to_vec();
        table.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        let mut item = [0; 72];
        item[4..9].copy_from_slice(b"event");
        item[68] = 2;
        table.extend_from_slice(&item);
        for name in [b"event:0", b"event:1"] {
            let mut variant = [0; 256];
            variant[..name.len()].copy_from_slice(name);
            table.extend_from_slice(&variant);
        }
        let mut registrations = NativeSequence::default();
        registrations.register(b"event:1", 0)?;
        driver.gallery = Gallery::new(table, &registrations.save_tree()?)?;

        let mut bytes = encode_component(&driver)?;
        let restored: FesScriptDriver = decode_component(&bytes).context("decode current gallery driver checkpoint")?;
        let entry = restored.gallery.named(0, b"event").context("missing gallery entry")?;
        assert!(!entry.variants[0].registered);
        assert!(entry.variants[1].registered);

        let fields = u32::from_le_bytes(bytes[..4].try_into()?);
        let appended_fields = encode_component(&driver.gallery)?.len()
            + encode_component(&None::<u32>)?.len()
            + encode_component(&false)?.len()
            + encode_component(&0u64)?.len()
            + encode_component(&None::<u64>)?.len()
            + encode_component(&None::<[i32; 2]>)?.len();
        bytes.truncate(bytes.len() - appended_fields);
        bytes[..4].copy_from_slice(&(fields - 6).to_le_bytes());
        let older: FesScriptDriver = decode_component(&bytes).context("decode older gallery checkpoint")?;
        assert!(older.gallery.entries.is_empty());
        assert_eq!(older.send_button_selection(), 1);
        assert!(!older.force_redraw_pending);
        Ok(())
    }
}
