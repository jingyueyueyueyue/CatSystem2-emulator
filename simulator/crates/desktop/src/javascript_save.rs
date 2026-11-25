//! JS save consumers, selected by the registered CSS and notification ABI.
//! cfSaveData writes the bound CSS; cfIsExistSave loads a
//! separate CSS and checks its unsigned tag. Load/update notify the actual
//! registered parent KCS through its synchronous receiver, retaining budget state.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use cs2_formats::CssFile;
use cs2_runtime::{TaskCommand, RuntimeResult};
use crate::native_tasks::OwnedTaskBuffers;
use super::{Adapter, JavaScriptCall, LogicEvent, Value, native_state_error, uint};

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
        if !self.host.data_services.ordinary_native_table.as_ref()
            .is_some_and(|table| table.javascript_save_callbacks_registered())
        {
            return Err(native_state_error("JavaScript save callback needs a recognized CSS and notification registration"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeadlessPlayer;
    use crate::javascript_native::JsRecord;
    use cs2_runtime::javascript::JavaScriptHost;
    use cs2_script::{KcsProgram, NativeReturn, Vm};
    use std::{path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

    #[test]
    #[ignore = "requires the installed V401 sample and its archived ordinary save"]
    fn javascript_save_queries_and_writes_through_the_opened_game() -> anyhow::Result<()> {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let game = workspace.join("games/cs2_full_v401");
        let original = workspace.join(concat!(
            "archive/2026-11/20261118_194934_952041_v401_visibility/",
            "20261119_v401_save_quit_confirm.profile/save0000.dat",
        ));
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let scratch = Scratch(std::env::temp_dir().join(format!("cs2-js-save-{}-{nonce}", std::process::id())));
        fs::create_dir(&scratch.0)?;
        fs::copy(&original, scratch.0.join("save0000.dat"))?;
        let original_file = CssFile::parse(&fs::read(&original)?)?;
        assert_eq!(original_file.tag(), 1000);

        let mut player = HeadlessPlayer::open_with_save_root(&game, &scratch.0)?;
        player.data_services.native_profile = None;
        player.data_services.css_task_pe_sha256 = None;
        let request = |operation: &str, args: &[u32]| JavaScriptCall {
            operation: operation.into(),
            arguments: args.iter().map(|&word| Value::Number(f64::from(word))).collect(),
        };
        let original_query = request("cfIsExistSave", &[0, 1000, 1000]);
        let written_query = request("cfIsExistSave", &[1, 1000, 1000]);
        player.with_host(|host| {
            let owner = host.create_task_object(55, 0, b"save regression", b"")?;
            host.css_handles.insert(1, original_file.clone());
            let vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 8], 0)?)?;
            let binding = [owner, 1].map(u32::to_le_bytes).concat();
            assert_eq!(host.javascript_native(1006, &binding, &vm)?, NativeReturn::Continue);
            let mut record = JsRecord::default();
            let mut adapter = Adapter { host, owner, record: &mut record };
            assert_eq!(adapter.javascript_call(&original_query, 1)?, Some(Value::Number(1.)));
            let above_tag = request("cfIsExistSave", &[0, 0x8000_0000, u32::MAX]);
            assert_eq!(adapter.javascript_call(&above_tag, 1)?, Some(Value::Number(-1.)));
            assert_eq!(adapter.javascript_call(&written_query, 1)?, Some(Value::Number(0.)));
            let write = request("cfSaveData", &[1]);
            assert_eq!(adapter.javascript_call(&write, 0)?, None);
            assert_eq!(adapter.javascript_call(&written_query, 1)?, Some(Value::Number(0.)));
            assert_eq!(adapter.javascript_call(&write, 1)?, Some(Value::Boolean(true)));
            Ok(())
        })?;
        drop(player);

        let mut reopened = HeadlessPlayer::open_with_save_root(&game, &scratch.0)?;
        reopened.with_host(|host| {
            let owner = host.create_task_object(55, 0, b"save regression", b"")?;
            let mut record = JsRecord::default();
            let mut adapter = Adapter { host, owner, record: &mut record };
            assert_eq!(adapter.javascript_call(&written_query, 1)?, Some(Value::Number(1.)));
            Ok(())
        })?;
        let written = CssFile::parse(&fs::read(scratch.0.join("save0001.dat"))?)?;
        assert_eq!(written.used_bytes()?, original_file.used_bytes()?);
        assert_eq!(fs::read(scratch.0.join("save0000.dat"))?, fs::read(&original)?);

        reopened.data_services.ordinary_native_table = None;
        reopened.with_host(|host| {
            let mut record = JsRecord::default();
            let mut adapter = Adapter { host, owner: 0, record: &mut record };
            let error = adapter.javascript_call(&original_query, 1).unwrap_err();
            assert!(error.to_string().contains("recognized CSS and notification registration"));
            Ok(())
        })
    }
}
