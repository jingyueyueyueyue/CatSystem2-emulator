//! V401 JS save consumers, recovered from the registration pairs and PE.
//! cfSaveData(135900) writes the bound CSS; cfIsExistSave(135430) loads a
//! separate CSS and checks its unsigned tag. Load/update notify the actual
//! registered parent KCS through its synchronous receiver, retaining budget state.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use cs2_formats::CssFile;
use cs2_runtime::{TaskCommand, RuntimeResult};
use crate::native_tasks::OwnedTaskBuffers;
use super::{Adapter, JavaScriptCall, LogicEvent, Value, native_state_error, uint};

const V401: &str = "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f";
const MAX_SAVE_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct SaveDispatch {
    packet: [u32; 4],
    command: TaskCommand,
    buffers: OwnedTaskBuffers,
    output: u32,
}

pub(super) fn supports(operation: &str) -> bool {
    matches!(operation, "cfIsExistSave" | "cfSaveData" | "cfUpdateSaveData"
        | "cfLoadSaveData" | "cfLoadSaveDataOnlyFlag")
}

impl Adapter<'_, '_> {
    pub(super) fn save_call(&mut self, call: &JavaScriptCall, budget: usize) -> RuntimeResult<Option<Value>> {
        let sha = self.host.data_services.native_profile.map(|profile| profile.pe_sha256)
            .or(self.host.data_services.css_task_pe_sha256);
        if sha != Some(V401) {
            return Err(native_state_error("JavaScript save callback needs its exact PE descriptor"));
        }
        if budget == 0 { return Ok(None); }
        if self.record.save_dispatch.is_some()
            && !matches!(call.operation.as_str(),
                "cfLoadSaveData" | "cfLoadSaveDataOnlyFlag" | "cfUpdateSaveData")
        {
            return Err(native_state_error("finish the retained JavaScript save notification before another save call"));
        }
        let args = &call.arguments;
        match call.operation.as_str() {
            "cfSaveData" => {
                // 53592e/535933: absent slot or null CSS returns false.
                let service = self.host.data_services.tasks.javascript.save_services.get(&self.owner)
                    .copied().ok_or_else(|| native_state_error("JavaScript save binding owner is absent"))?;
                if args.is_empty() || service == 0 {
                    return Ok(Some(Value::Boolean(false)));
                }
                let slot = uint(args, 0)? as i32;
                let container = self.host.css_handles.get(&service)
                    .ok_or_else(|| native_state_error(
                        "JavaScript bound save container is released or has the wrong object type"))?;
                let bytes = container.to_css().map_err(|_| native_state_error(
                    "JavaScript CSS save serialization failed"))?;
                if bytes.len() > MAX_SAVE_BYTES {
                    return Err(native_state_error("JavaScript CSS save exceeds 64 MiB"));
                }
                let name = format!("save{slot:04}.dat");
                let path = crate::save_path(self.host.save_root, &name)
                    .map_err(|_| native_state_error("JavaScript CSS save path is invalid"))?;
                // Factory ba96c passes its real save directory to task+15c.
                // The portable factory's same directory is host.save_root;
                // ordinary creation-context text is unrelated to this field.
                let mut partial = false;
                let written = (|| -> std::io::Result<()> {
                    fs::create_dir_all(self.host.save_root)?;
                    let mut file = OpenOptions::new().create(true).truncate(true).write(true).open(&path)?;
                    partial = true;
                    file.write_all(&bytes)?;
                    file.flush()?;
                    partial = false;
                    Ok(())
                })();
                if let Err(error) = written {
                    if partial {
                        fs::remove_file(&path).map_err(|_| native_state_error(
                            "JavaScript CSS save could not remove its incomplete output"))?;
                    }
                    self.io_error(&name, &error);
                    return Ok(Some(Value::Boolean(false)));
                }
                // Helper624360's third argument is1 here: unlike native1031,
                // it does not restore the previous file modification time.
                Ok(Some(Value::Boolean(true)))
            }
            "cfIsExistSave" => {
                if args.is_empty() { return Ok(Some(Value::Number(0.))); }
                let slot = uint(args, 0)? as i32;
                let name = format!("save{slot:04}.dat");
                let path = crate::save_path(self.host.save_root, &name)
                    .map_err(|_| native_state_error("JavaScript CSS query path is invalid"))?;
                let bytes = match read_bounded(&path) {
                    Ok(bytes) => bytes,
                    Err(ReadFailure::Io(error)) => {
                        if error.kind() != std::io::ErrorKind::NotFound { self.io_error(&name, &error); }
                        return Ok(Some(Value::Number(0.)));
                    }
                    Err(ReadFailure::Limit) => return Err(native_state_error(
                        "JavaScript CSS query exceeds 64 MiB")),
                };
                let Some(container) = CssFile::from_css(&bytes).map_err(|_| native_state_error(
                    "JavaScript CSS query has malformed container structure"))? else {
                    return Ok(Some(Value::Number(0.)));
                };
                // 224610 invokes inflate but ignores its result before the
                // unsigned tag test. Keep that result, while exposing a bad
                // stream/declared-length mismatch instead of hiding damage.
                if let Some(status) = container.inflation_status() {
                    if !status.stream_end || status.output_truncated || status.limit_reached
                        || status.error.is_some() || status.output_written != container.used_len()
                    {
                        self.host.events.push(LogicEvent {
                            frame: self.host.frame, kind: "javascript-save-damaged".into(),
                            detail: format!(
                                "task {} {name}: CSS declared={} inflated={} input={} end={} truncated={} limit={} error={:?}; original tag query retained",
                                self.owner, container.used_len(), status.output_written,
                                status.input_consumed, status.stream_end, status.output_truncated,
                                status.limit_reached, status.error,
                            ),
                        });
                    }
                }
                let minimum = if args.len() > 1 { uint(args, 1)? } else { u32::MAX };
                let maximum = if args.len() > 2 { uint(args, 2)? } else { u32::MAX };
                // 535540..535556 skips both comparisons if min==-1. Its
                // jb/jbe compare the CSS tag as an unsigned DWORD.
                let result = if minimum != u32::MAX
                    && (container.tag() < minimum || container.tag() > maximum) { -1. } else { 1. };
                Ok(Some(Value::Number(result)))
            }
            "cfLoadSaveData" | "cfLoadSaveDataOnlyFlag" | "cfUpdateSaveData" => {
                if call.operation != "cfUpdateSaveData" && args.is_empty() {
                    if self.record.save_dispatch.is_some() {
                        return Err(native_state_error("retained JavaScript load notification lost its slot"));
                    }
                    return Ok(Some(Value::Undefined));
                }
                let code = match call.operation.as_str() {
                    "cfLoadSaveData" => 0x409,
                    "cfLoadSaveDataOnlyFlag" => 0x1001,
                    _ => 0x1000,
                };
                let slot = if code == 0x1000 { 0 } else { uint(args, 0)? };
                let packet = [self.owner, code, 0, slot];
                if let Some(dispatch) = &self.record.save_dispatch {
                    if dispatch.packet != packet {
                        return Err(native_state_error("JavaScript save notification changed across its budget continuation"));
                    }
                } else {
                    let target = self.save_notification_target()?;
                    if self.host.data_services.tasks.message_task_state(target)
                        .is_none_or(|(kind, _, _)| !matches!(kind, 0 | 12))
                    {
                        return Err(native_state_error(
                            "JavaScript save notification needs its registered parent KCS receiver"));
                    }
                    let bytes = packet.into_iter().flat_map(u32::to_le_bytes).collect();
                    self.record.save_dispatch = Some(SaveDispatch {
                        packet,
                        // Typed command173 is the shared synchronous packet
                        // operation. The V401 raw number is170; no VM pointer
                        // is encoded into either data DWORD.
                        command: TaskCommand { target: target as i32, sender: self.owner as i32,
                            code: 173, data: [0, 1, 0] },
                        buffers: OwnedTaskBuffers::new(None, Some(bytes))
                            .map_err(|cause| cs2_runtime::RuntimeError::Host(cause.to_string()))?,
                        output: 0,
                    });
                }
                Ok(self.finish_save_dispatch(budget)?.then_some(Value::Undefined))
            }
            _ => unreachable!(),
        }
    }

    pub(super) fn finish_save_dispatch(&mut self, budget: usize) -> RuntimeResult<bool> {
        let Some(dispatch) = self.record.save_dispatch.as_mut() else { return Ok(true); };
        if budget == 0 { return Ok(false); }
        let outcome = self.host.route_task_command_buffers(dispatch.command,
            &mut dispatch.buffers, &mut dispatch.output, budget)
            .map_err(|cause| cs2_runtime::RuntimeError::Host(cause.to_string()))?
            .ok_or_else(|| native_state_error("JavaScript save notification parent is unavailable"))?;
        if !outcome.done { return Ok(false); }
        if outcome.propagate {
            return Err(native_state_error("JavaScript save notification has no concrete parent receiver"));
        }
        self.record.save_dispatch = None;
        Ok(true)
    }

    fn save_notification_target(&self) -> RuntimeResult<u32> {
        // 90c570 reads task+c, not an ancestry search. Base command2 at
        // 4b9406..4b940b registers a child by invoking its vtable+4 with the
        // receiving parent. The actual type55 slot is8a7df0, which stores
        // exactly that argument into child+c. A nested sscript VM must get
        // its409/1000/1001 packet; the top-level main VM is another receiver.
        self.host.task_parents.get(&self.owner).copied()
            .filter(|&parent| parent != self.owner)
            .filter(|&parent| self.host.data_services.tasks.task_clock(parent).is_some())
            .ok_or_else(|| native_state_error("JavaScript registered parent object is absent"))
    }

    fn io_error(&mut self, name: &str, error: &std::io::Error) {
        self.host.events.push(LogicEvent { frame: self.host.frame, kind: "javascript-save-error".into(),
            detail: format!("task {} {name}: {error}", self.owner) });
    }
}

enum ReadFailure { Io(std::io::Error), Limit }

fn read_bounded(path: &Path) -> std::result::Result<Vec<u8>, ReadFailure> {
    let file = File::open(path).map_err(ReadFailure::Io)?;
    if file.metadata().map_err(ReadFailure::Io)?.len() > MAX_SAVE_BYTES as u64 {
        return Err(ReadFailure::Limit);
    }
    let mut bytes = Vec::new();
    file.take(MAX_SAVE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(ReadFailure::Io)?;
    if bytes.len() > MAX_SAVE_BYTES { return Err(ReadFailure::Limit); }
    Ok(bytes)
}
