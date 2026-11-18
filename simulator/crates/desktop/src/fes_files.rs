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
            Some(b"newsave" | b"saveexist" | b"datasave")
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
        }
    }
}

fn write_shared(state: &mut ExpressionState, destination: i32, value: i32) {
    if let Some(slot) = state.shared.get_mut(destination as usize) {
        *slot = value;
    }
}

fn slot_filename(slot: i32) -> String {
    format!("save{slot:04}.dat")
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
