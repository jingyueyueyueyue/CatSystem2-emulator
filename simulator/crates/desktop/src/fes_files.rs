//! FES file globals, independently implemented from docs/save.md.
//! This module is a child of fes_driver so its continuations participate in
//! driver snapshots without exposing host file handles or process pointers.
use std::fs::{self, File, Metadata};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use cs2_formats::CssFile;
use cs2_script::ExpressionState;

use super::{FesDriverCommand, FesDriverEffect, FesScriptDriver};
use crate::{LoggingHost, LogicEvent};

const MAX_SAVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_QUERY_SLOTS: i64 = 1_000_000;
// One retained effect performs bounded host work, including when the task's
// script budget is large. Returning false retains its original logical tick.
const QUERY_BATCH: usize = 32;

impl LoggingHost<'_> {
    /// SAVEPANEL shares the original FES CSS reader, save-path boundary and
    /// allocation limit with saveexist. It never reads a second save format.
    pub(crate) fn read_fes_css_save(&self, filename: &str) -> Result<Option<CssFile>> {
        read_css(self.save_root, filename)
    }
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FileCommand {
    command: FesDriverCommand,
    numbers: Vec<i32>,
    preparation_error: Option<String>,
    operation: Option<Operation>,
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum Operation {
    Skipped,
    NewSave {
        destination: i32,
        next: Option<i32>,
        last: i32,
        selected: i32,
        latest: u64,
    },
    SaveExist {
        destination: i32,
        filename: Vec<u8>,
        lower: u32,
        upper: u32,
    },
    DataSave {
        slot: i32,
        container: u32,
    },
    SaveAppendString {
        slot: i32,
        record: u32,
        text: Vec<u8>,
    },
    SaveGetString {
        slot: i32,
        record: u32,
        destination: u32,
    },
    DeleteSave {
        slot: i32,
    },
    CopySave {
        from: i32,
        to: i32,
        exchange: bool,
    },
    SortSave {
        destination: i32,
        first: i32,
        last: i32,
        slots: Vec<(i32, u64)>,
    },
}

impl FesScriptDriver {
    /// None makes no change. Some(false) keeps this same command pending;
    /// the task must offer the retained effect again before calling advance.
    pub(crate) fn apply_file_effect(
        &mut self,
        host: &mut LoggingHost<'_>,
        effect: &FesDriverEffect,
        shared: &mut ExpressionState,
    ) -> Result<Option<bool>> {
        let FesDriverEffect::Command(command) = effect else {
            return Ok(None);
        };
        if !matches!(
            command.parameters.first().map(Vec::as_slice),
            Some(b"newsave" | b"saveexist" | b"datasave" | b"saveapend_str" | b"saveget_str"
                | b"savedelete" | b"savecopy" | b"saveexchg" | b"sortsave")
        ) {
            return Ok(None);
        }
        if let Some(pending) = &self.pending_file {
            if pending.command.offset != command.offset
                || pending.command.source != command.source
                || pending.command.parameters != command.parameters
                || pending.command.context != command.context
                || pending.command.object_event != command.object_event
            {
                bail!("FES file-command continuation changed its command or scope");
            }
        }
        let mut pending = self.pending_file.take().unwrap_or_else(|| FileCommand {
            command: command.clone(),
            numbers: Vec::new(),
            preparation_error: None,
            operation: None,
        });
        let result = (|| {
            pending.prepare(self, shared)?;
            pending.step(host, shared)
        })();
        if !matches!(&result, Ok(true)) {
            self.pending_file = Some(pending);
        }
        Ok(Some(result?))
    }
}

impl FileCommand {
    // Cache each successful numeric argument before moving to the next one.
    // A preparation failure is terminal for this command: retrying it must
    // not repeat partial side effects inside the failing expression either.
    fn prepare(&mut self, driver: &mut FesScriptDriver, shared: &mut ExpressionState) -> Result<()> {
        if let Some(error) = &self.preparation_error {
            bail!("{error}");
        }
        if self.operation.is_some() {
            return Ok(());
        }
        let result = self.prepare_inner(driver, shared);
        if let Err(error) = &result {
            self.preparation_error = Some(format!("{error:#}"));
        }
        result
    }

    fn number(
        &mut self,
        ordinal: usize,
        parameter: usize,
        driver: &mut FesScriptDriver,
        shared: &mut ExpressionState,
    ) -> Result<Option<i32>> {
        if let Some(number) = self.numbers.get(ordinal) {
            return Ok(Some(*number));
        }
        if self.numbers.len() != ordinal {
            bail!("FES file-command numeric argument order is invalid");
        }
        let Some(source) = self.command.parameters.get(parameter) else {
            return Ok(None);
        };
        let number = driver.evaluate_parameter(source, self.command.context.current, shared)?;
        self.numbers.push(number);
        Ok(Some(number))
    }

    fn prepare_inner(
        &mut self, driver: &mut FesScriptDriver, shared: &mut ExpressionState,
    ) -> Result<()> {
        // Required arguments are visited separately. A missing later argument
        // preserves earlier side effects, and does not evaluate any suffix.
        match self.command.parameters[0].as_slice() {
            b"newsave" => {
                let Some(destination) = self.number(0, 1, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                let Some(first) = self.number(1, 2, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                let Some(last) = self.number(2, 3, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                if last == i32::MAX {
                    bail!("FES newsave ending slot would overflow the original scan");
                }
                if i64::from(last) - i64::from(first) + 1 > MAX_QUERY_SLOTS {
                    bail!("FES newsave range exceeds {MAX_QUERY_SLOTS} slots");
                }
                self.operation = Some(Operation::NewSave {
                    destination, next: (first <= last).then_some(first),
                    last, selected: -1, latest: 0,
                });
            }
            b"sortsave" => {
                let mut bounds = [0; 3];
                for (ordinal, bound) in bounds.iter_mut().enumerate() {
                    let Some(number) = self.number(ordinal, ordinal + 1, driver, shared)? else {
                        self.operation = Some(Operation::Skipped);
                        return Ok(());
                    };
                    *bound = number;
                }
                let [destination, first, last] = bounds;
                if i64::from(last) - i64::from(first) + 1 > MAX_QUERY_SLOTS {
                    bail!("FES sortsave range exceeds {MAX_QUERY_SLOTS} slots");
                }
                self.operation = Some(if first < last {
                    Operation::SortSave { destination, first, last, slots: Vec::new() }
                } else {
                    Operation::Skipped
                });
            }
            b"saveexist" => {
                let Some(slot) = self.number(0, 1, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                let (filename, destination_parameter) = if slot < 0 {
                    let Some(filename) = self.command.parameters.get(2) else {
                        self.operation = Some(Operation::Skipped);
                        return Ok(());
                    };
                    (nul(filename).to_vec(), 3)
                } else {
                    (slot_filename(slot).into_bytes(), 2)
                };
                let Some(destination) = self.number(1, destination_parameter, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                let lower = self.number(2, destination_parameter + 1, driver, shared)?
                    .map(|value| value as u32).unwrap_or(u32::MAX);
                // No upper parameter can be visible without a lower one.
                let upper = if self.command.parameters.get(destination_parameter + 1).is_some() {
                    self.number(3, destination_parameter + 2, driver, shared)?
                        .map(|value| value as u32).unwrap_or(lower)
                } else {
                    lower
                };
                self.operation = Some(Operation::SaveExist {
                    destination, filename, lower, upper,
                });
            }
            b"datasave" => {
                let Some(slot) = self.number(0, 1, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                let Some(container) = self.number(1, 2, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                self.operation = Some(Operation::DataSave { slot, container: container as u32 });
            }
            b"saveapend_str" | b"saveget_str" => {
                let Some(slot) = self.number(0, 1, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                let Some(record) = self.number(1, 2, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                let record = record as u32;
                self.operation = Some(if self.command.parameters[0] == b"saveapend_str" {
                    match self.command.parameters.get(3) {
                        Some(text) => Operation::SaveAppendString {
                            slot, record, text: nul(text).to_vec(),
                        },
                        None => Operation::Skipped,
                    }
                } else {
                    match self.number(2, 3, driver, shared)? {
                        Some(destination) => Operation::SaveGetString {
                            slot, record, destination: destination as u32,
                        },
                        None => Operation::Skipped,
                    }
                });
            }
            b"savedelete" => {
                self.operation = Some(match self.number(0, 1, driver, shared)? {
                    Some(slot) => Operation::DeleteSave { slot },
                    None => Operation::Skipped,
                });
            }
            b"savecopy" | b"saveexchg" => {
                let Some(from) = self.number(0, 1, driver, shared)? else {
                    self.operation = Some(Operation::Skipped);
                    return Ok(());
                };
                self.operation = Some(match self.number(1, 2, driver, shared)? {
                    Some(to) => Operation::CopySave {
                        from, to, exchange: self.command.parameters[0] == b"saveexchg",
                    },
                    None => Operation::Skipped,
                });
            }
            _ => bail!("FES file-command kind is invalid"),
        }
        Ok(())
    }

    fn step(&mut self, host: &mut LoggingHost<'_>, shared: &mut ExpressionState) -> Result<bool> {
        match self.operation.as_mut().context("FES file command is not prepared")? {
            Operation::Skipped => Ok(true),
            Operation::NewSave { destination, next, last, selected, latest } => {
                for _ in 0..QUERY_BATCH {
                    let Some(slot) = *next else { break };
                    // Advance only after a successful service response.
                    // An I/O failure retries this file and no earlier file.
                    if let Some(time) = modified_time(host.save_root, slot)? {
                        // -1 doubles as a real slot and the unselected marker.
                        if *selected == -1 || time > *latest {
                            *selected = slot;
                            *latest = time;
                        }
                    }
                    *next = if slot == *last { None } else {
                        Some(slot.checked_add(1).context("FES newsave slot overflow")?)
                    };
                }
                if next.is_some() {
                    return Ok(false);
                }
                write_shared(shared, *destination, *selected);
                Ok(true)
            }
            Operation::SortSave { destination, first, last, slots } => {
                let count = (i64::from(*last) - i64::from(*first) + 1) as usize;
                for index in slots.len()..count.min(slots.len() + QUERY_BATCH) {
                    let slot = *first + index as i32;
                    slots.push((slot, modified_time(host.save_root, slot)?.unwrap_or(0)));
                }
                if slots.len() < count { return Ok(false); }
                sort_save_slots(slots);
                for (offset, &(slot, _)) in slots.iter().enumerate() {
                    write_shared(shared, destination.wrapping_add(offset as i32), slot);
                }
                Ok(true)
            }
            Operation::SaveExist { destination, filename, lower, upper } => {
                let filename = host.text_encoding.decode(filename);
                let found = match read_css(host.save_root, &filename)? {
                    None => 0,
                    Some(file) if *lower == u32::MAX
                        || (*lower <= file.tag && file.tag <= *upper) => 1,
                    Some(_) => -1,
                };
                write_shared(shared, *destination, found);
                Ok(true)
            }
            Operation::DataSave { slot, container } => {
                if *container == 0 {
                    return Ok(true);
                }
                let file = host.css_handles.get(container)
                    .context("FES datasave has an invalid or wrong-type CSS container")?;
                let bytes = file.to_css().context("FES datasave CSS serialization failed")?;
                let filename = slot_filename(*slot);
                let path = crate::save_path(host.save_root, &filename)?;
                // The file-service contract distinguishes ordinary write
                // failure from protocol/serialization errors. Record the
                // former while completing the command as documented.
                let write = fs::create_dir_all(host.save_root)
                    .and_then(|()| File::create(&path))
                    .and_then(|mut file| file.write_all(&bytes));
                host.events.push(LogicEvent {
                    frame: host.frame,
                    kind: "fes-datasave".into(),
                    detail: match write {
                        Ok(()) => format!("slot={slot} container={container} written=true"),
                        Err(error) => format!(
                            "slot={slot} container={container} written=false error={error}"
                        ),
                    },
                });
                Ok(true)
            }
            Operation::SaveAppendString { slot, record, text } => {
                append_save_string(host.save_root, *slot, *record, text)?;
                Ok(true)
            }
            Operation::SaveGetString { slot, record, destination } => {
                if let Some(text) = get_save_string(host.save_root, *slot, *record)? {
                    shared.strings.insert(*destination, text);
                }
                Ok(true)
            }
            Operation::DeleteSave { slot } => {
                let path = crate::save_path(host.save_root, &slot_filename(*slot))?;
                let result = remove_slot(&path);
                record_file_result(host, "savedelete", format!("slot={slot}"), result);
                Ok(true)
            }
            Operation::CopySave { from, to, exchange } => {
                let source = crate::save_path(host.save_root, &slot_filename(*from))?;
                let destination = crate::save_path(host.save_root, &slot_filename(*to))?;
                let verb = if *exchange { "saveexchg" } else { "savecopy" };
                let result = copy_slots(&source, &destination, *exchange);
                record_file_result(host, verb, format!("from={from} to={to}"), result);
                Ok(true)
            }
        }
    }
}

fn record_file_result(host: &mut LoggingHost<'_>, verb: &str, slots: String, result: Result<bool>) {
    let outcome = match result {
        Ok(changed) => format!("changed={changed}"),
        Err(error) => format!("error={error:#}"),
    };
    host.events.push(LogicEvent {
        frame: host.frame,
        kind: "fes-save-file".into(),
        detail: format!("command={verb} {slots} {outcome}"),
    });
}

fn remove_slot(path: &Path) -> Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    #[cfg(windows)]
    if metadata.permissions().readonly() {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(windows))]
    let _ = metadata;
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

struct SlotContents {
    bytes: Vec<u8>,
    modified: std::time::SystemTime,
}

impl SlotContents {
    fn read(path: &Path) -> Result<Option<Self>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() { return Ok(None); }
        if metadata.len() > MAX_SAVE_BYTES { bail!("FES save file exceeds 64 MiB"); }
        let mut bytes = Vec::new();
        file.take(MAX_SAVE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SAVE_BYTES { bail!("FES save file exceeds 64 MiB"); }
        Ok(Some(Self { bytes, modified: metadata.modified()? }))
    }

    fn write(&self, path: &Path) -> Result<()> {
        // Windows cannot replace a read-only file. The original save-copy
        // helper also permits overwriting read-only slots.
        #[cfg(windows)]
        let previous_permissions = match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() && metadata.permissions().readonly() => {
                let previous = metadata.permissions();
                let mut writable = previous.clone();
                writable.set_readonly(false);
                fs::set_permissions(path, writable)?;
                Some(previous)
            }
            Ok(_) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let written = cs2_runtime::write_private_file_with_modified_time(path, &self.bytes, self.modified);
        #[cfg(windows)]
        if written.is_err() {
            if let Some(previous) = previous_permissions {
                fs::set_permissions(path, previous).context("restore save slot permissions")?;
            }
        }
        written?;
        Ok(())
    }
}

fn copy_slots(source: &Path, destination: &Path, exchange: bool) -> Result<bool> {
    if source == destination { return Ok(false); }
    let first = SlotContents::read(source)?;
    if !exchange {
        let Some(first) = first else { return Ok(false); };
        first.write(destination)?;
        return Ok(true);
    }

    let second = SlotContents::read(destination)?;
    match (first, second) {
        (Some(first), Some(second)) => {
            // Keep both originals until the replacement writes succeed. A
            // second-write failure can then restore the first slot in place.
            second.write(source)?;
            if let Err(error) = first.write(destination) {
                first.write(source).with_context(|| format!(
                    "FES save exchange rollback failed after destination write: {error:#}",
                ))?;
                return Err(error);
            }
        }
        (Some(_), None) => fs::rename(source, destination)?,
        (None, Some(_)) => fs::rename(destination, source)?,
        (None, None) => return Ok(false),
    }
    Ok(true)
}

fn append_save_string(root: &Path, slot: i32, record: u32, text: &[u8]) -> Result<()> {
    let name = slot_filename(slot);
    let Some(mut file) = read_css(root, &name)? else { return Ok(()); };
    let path = crate::save_path(root, &name)?;
    let modified = fs::metadata(&path)?.modified()?;
    file.try_remove(record)?;
    // The original stores four bytes after the text, including the terminator.
    let mut bytes = Vec::with_capacity(text.len() + 4);
    bytes.extend_from_slice(text);
    bytes.extend_from_slice(&[0; 4]);
    file.append(record, &bytes)?;
    cs2_runtime::write_private_file_with_modified_time(&path, &file.to_css()?, modified)?;
    Ok(())
}

fn get_save_string(root: &Path, slot: i32, record: u32) -> Result<Option<Vec<u8>>> {
    let Some(file) = read_css(root, &slot_filename(slot))? else { return Ok(None); };
    Ok(file.try_record(record)?.map(|bytes| nul(bytes).to_vec()))
}

fn write_shared(state: &mut ExpressionState, destination: i32, value: i32) {
    if let Some(slot) = state.shared.get_mut(destination as usize) {
        *slot = value;
    }
}

fn slot_filename(slot: i32) -> String {
    format!("save{slot:04}.dat")
}

fn sort_save_slots(mut slots: &mut [(i32, u64)]) {
    // The original partitions around the lower midpoint and moves only
    // strictly newer entries to its left. Equal timestamps keep its swaps.
    while slots.len() > 1 {
        slots.swap(0, (slots.len() - 1) / 2);
        let pivot = slots[0].1;
        let mut split = 0;
        let mut all_equal = true;
        for index in 1..slots.len() {
            all_equal &= slots[index].1 == pivot;
            if slots[index].1 > pivot {
                split += 1;
                slots.swap(index, split);
            }
        }
        if all_equal {
            // The remaining partitions only swap their first and middle entries.
            for first in 1..slots.len() - 1 {
                slots.swap(first, (first + slots.len() - 1) / 2);
            }
            return;
        }
        slots.swap(0, split);
        let (newer, rest) = slots.split_at_mut(split);
        let older = &mut rest[1..];
        if newer.len() < older.len() {
            sort_save_slots(newer);
            slots = older;
        } else {
            sort_save_slots(older);
            slots = newer;
        }
    }
}

fn modified_time(root: &Path, slot: i32) -> Result<Option<u64>> {
    let path = crate::save_path(root, &slot_filename(slot))?;
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!(
            "FES newsave could not query {}", path.display(),
        )),
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    file_time(&metadata).with_context(|| format!(
        "FES newsave could not read modified time for {}", path.display(),
    )).map(Some)
}

#[cfg(windows)]
fn file_time(metadata: &Metadata) -> Result<u64> {
    use std::os::windows::fs::MetadataExt;
    Ok(metadata.last_write_time())
}

#[cfg(not(windows))]
fn file_time(metadata: &Metadata) -> Result<u64> {
    use std::time::UNIX_EPOCH;
    const UNIX_FILETIME: u64 = 116_444_736_000_000_000;
    let time = metadata.modified()?;
    let (duration, after_epoch) = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => (duration, true),
        Err(error) => (error.duration(), false),
    };
    let ticks = u64::try_from(duration.as_nanos() / 100)
        .context("file timestamp exceeds the FILETIME range")?;
    if after_epoch { UNIX_FILETIME.checked_add(ticks) } else { UNIX_FILETIME.checked_sub(ticks) }
        .context("file timestamp is outside the unsigned FILETIME range")
}

fn read_css(root: &Path, name: &str) -> Result<Option<CssFile>> {
    let path = crate::save_path(root, name)?;
    // Missing/unopenable files and wrong signatures are ordinary "not a
    // save" results. Once a CSS signature is present, corruption diagnoses.
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut signature = [0; 4];
    let mut count = 0;
    while count < signature.len() {
        let read = file.read(&mut signature[count..])
            .with_context(|| format!("read FES save signature {}", path.display()))?;
        if read == 0 { return Ok(None); }
        count += read;
    }
    if signature != *b"CSS\0" {
        return Ok(None);
    }
    let mut bytes = signature.to_vec();
    file.take(MAX_SAVE_BYTES - 3).read_to_end(&mut bytes)
        .with_context(|| format!("read FES save file {}", path.display()))?;
    if bytes.len() as u64 > MAX_SAVE_BYTES {
        bail!("FES save file exceeds 64 MiB");
    }
    Ok(Some(CssFile::parse(&bytes)
        .with_context(|| format!("invalid FES CSS save {}", path.display()))?))
}

fn nul(bytes: &[u8]) -> &[u8] {
    bytes.split(|&byte| byte == 0).next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_script::{FesInterpreter, FesProgram, FesStep};

    #[test]
    fn fes_save_slot_operations_preserve_bytes_and_timestamps() -> Result<()> {
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-fes-save-slot-{}-{unique}", std::process::id()));
        fs::create_dir(&root)?;
        let first = root.join(slot_filename(1));
        let second = root.join(slot_filename(2));
        let third = root.join(slot_filename(3));
        let bytes_a = b"CSS\0first\xff";
        let bytes_b = b"CSS\0second";
        let time_a = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let time_b = UNIX_EPOCH + Duration::from_secs(1_700_000_100);
        let result = (|| -> Result<()> {
            cs2_runtime::write_private_file_with_modified_time(&first, bytes_a, time_a)?;
            cs2_runtime::write_private_file_with_modified_time(&second, bytes_b, time_b)?;
            #[cfg(windows)]
            {
                let mut permissions = fs::metadata(&second)?.permissions();
                permissions.set_readonly(true);
                fs::set_permissions(&second, permissions)?;
            }
            assert!(copy_slots(&first, &second, false)?);
            assert_eq!(fs::read(&first)?, bytes_a);
            assert_eq!(fs::read(&second)?, bytes_a);
            assert_eq!(fs::metadata(&second)?.modified()?, time_a);

            cs2_runtime::write_private_file_with_modified_time(&second, bytes_b, time_b)?;
            assert!(copy_slots(&first, &second, true)?);
            assert_eq!(fs::read(&first)?, bytes_b);
            assert_eq!(fs::read(&second)?, bytes_a);
            assert_eq!(fs::metadata(&first)?.modified()?, time_b);
            assert_eq!(fs::metadata(&second)?.modified()?, time_a);
            assert!(copy_slots(&third, &second, true)?);
            assert!(!second.exists());
            assert_eq!(fs::read(&third)?, bytes_a);
            assert_eq!(fs::metadata(&third)?.modified()?, time_a);

            assert!(!copy_slots(&second, &first, false)?);
            assert!(!copy_slots(&first, &first, true)?);
            assert_eq!(fs::read(&first)?, bytes_b);
            assert!(remove_slot(&first)?);
            assert!(!remove_slot(&first)?);
            Ok(())
        })();
        for path in [&first, &second, &third] { remove_slot(path)?; }
        fs::remove_dir(&root)?;
        result
    }

    #[test]
    fn fes_save_string_roundtrip_keeps_text_and_existing_save_records() -> Result<()> {
        let program = FesProgram::from_text(
            b"#MACRO\nVALNO_S_NEW 42\n#START\n\
              if (1) saveapend_str $VALNO_S_NEW 0x80000000 $str155\n\
              saveapend_str $VALNO_S_NEW 0x200 $str751\n",
        )?;
        let mut interpreter = FesInterpreter::new(program)?;
        let mut state = ExpressionState::default();
        let title = b"Chapter \"2 (one $str3";
        let comment = b"First save\t[story]";
        state.strings.insert(155, title.to_vec());
        state.strings.insert(751, comment.to_vec());

        let root = std::env::temp_dir().join(format!(
            "cs2-fes-save-string-{}-{:?}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos(),
        ));
        fs::create_dir(&root)?;
        let result = (|| -> Result<()> {
            let path = root.join(slot_filename(0));
            let mut file = CssFile::new(0x91);
            file.append(0x10, b"game state")?;
            file.append(0x80000000, b"old title\0")?;
            fs::write(&path, file.to_css()?)?;
            File::options().write(true).open(&path)?.set_times(
                fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
            )?;
            let modified = fs::metadata(&path)?.modified()?;

            for (id, record, text) in [
                (b"0x80000000".as_slice(), 0x80000000, title.as_slice()),
                (b"0x200".as_slice(), 0x200, comment.as_slice()),
            ] {
                assert!(matches!(interpreter.run(&mut state, 8)?, FesStep::Event));
                let parameters = interpreter.pending_command_parameters().unwrap();
                assert_eq!(parameters.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                    [b"saveapend_str".as_slice(), b"$42", id, text]);
                append_save_string(&root, 0, record, &parameters[3])?;
                interpreter.resume();
            }

            let file = read_css(&root, &slot_filename(0))?.unwrap();
            assert_eq!(file.tag(), 0x91);
            assert_eq!(file.record(0x10), Some(b"game state".as_slice()));
            assert_eq!(file.record(0x80000000), Some([title.as_slice(), &[0; 4]].concat().as_slice()));
            assert_eq!(file.records.iter().filter(|record| record.id == 0x80000000).count(), 1);
            assert_eq!(get_save_string(&root, 0, 0x200)?.as_deref(), Some(comment.as_slice()));
            assert_eq!(get_save_string(&root, 0, 0x201)?, None);
            assert_eq!(fs::metadata(&path)?.modified()?, modified);
            Ok(())
        })();
        fs::remove_file(root.join(slot_filename(0)))?;
        fs::remove_dir(&root)?;
        result
    }
}
