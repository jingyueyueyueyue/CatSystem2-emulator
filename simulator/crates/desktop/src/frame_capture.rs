//! Ordinary441 configures continuous capture, 565/566 read that same state.
//! The present boundary owns capture; depth-slice render callbacks never do.
use std::fs;
use anyhow::{Context, Result};
use cs2_script::{NativeReturn, Vm};
use super::{HeadlessPlayer, LoggingHost, LogicEvent, expand_kcs_format, native_state_error, read_u32};

#[derive(Default)]
pub(super) struct FrameCaptureState {
    pub(super) enabled: u32,
    pub(super) next_index: u32,
    name: String,
}

impl LoggingHost<'_> {
    pub(super) fn frame_capture_native(
        &mut self, arguments: &[u8], vm: &Vm,
    ) -> cs2_script::Result<NativeReturn> {
        // The format expansion happens before the other arguments are read.
        let source = expand_kcs_format(vm, &vm.read_string(read_u32(arguments, 4)?)?, 0)?;
        let name = self.text_encoding.decode(super::trim_nul(&source));
        if name.encode_utf16().count() >= 1024 {
            return Err(native_state_error("native 441 name exceeds the formatted wide buffer"));
        }
        let next_index = read_u32(arguments, 8)?;
        let enabled = read_u32(arguments, 0)?;
        self.data_services.frame_capture = FrameCaptureState { enabled, next_index, name };
        Ok(NativeReturn::Continue)
    }
}

impl HeadlessPlayer {
    pub(super) fn capture_completed_frame(&mut self) {
        if self.data_services.frame_capture.enabled == 0 { return; }
        let capture = &mut self.data_services.frame_capture;
        let index = capture.next_index;
        // The original increments before calling the writer and ignores its
        // bool result. A failed write must not keep retrying the same index.
        capture.next_index = index.wrapping_add(1);
        let name = format!("{0}\\{0}_{1:010}.bmp", capture.name, index as i32);
        let result = self.write_capture_frame(&name);
        self.events.push(LogicEvent {
            frame: self.state.clock.frame,
            kind: if result.is_ok() { "frame-capture-written" } else { "frame-capture-failed" }.into(),
            detail: match result {
                Ok(()) => format!("index={index} name={name}"),
                Err(error) => format!("index={index} name={name} error={error:#}"),
            },
        });
    }

    fn write_capture_frame(&self, name: &str) -> Result<()> {
        // Both %s arguments are the same stored string. Keep that behavior,
        // including nested names, inside the private writable game profile.
        if name.encode_utf16().count() >= 260 {
            anyhow::bail!("frame capture name exceeds the original output buffer");
        }
        let path = super::save_path(&self.save_root, name)?;
        let bytes = cs2_renderer::encode_capture_bmp(&self.canvas)?;
        fs::create_dir_all(&self.save_root).context("create private capture profile")?;
        // The original writer does not create the capture subdirectory.
        // File/directory operations and the host own its existence.
        fs::write(&path, bytes).with_context(|| format!("write capture {}", path.display()))
    }
}
