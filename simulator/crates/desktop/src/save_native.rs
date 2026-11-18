//! Container operations using the same image bytes and motion snapshots as
//! their producers. Copies never retain a pointer into another object's data.
use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, native_state_error, read_u32};

#[path = "save_storage_space.rs"]
mod storage_space;

impl LoggingHost<'_> {
    pub(super) fn save_file_native(&mut self, index: u32, arguments: &[u8], vm: &Vm) -> Result<NativeReturn> {
        use std::fs::{self, FileTimes, OpenOptions};
        use std::io::Write;
        let address = if index == 990 { read_u32(arguments, 0)? } else {
            let handle = read_u32(arguments, 0)?;
            if handle == 0 { return Ok(NativeReturn::Value(0)); }
            if !self.css_handles.contains_key(&handle) {
                return Err(native_state_error("native 1031 invalid/wrong-type CSS container"));
            }
            read_u32(arguments, 4)?
        };
        let name = super::expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
        let name = super::trim_nul(&name);
        if name.len() >= 1024 { return Err(native_state_error("save filename exceeds its original conversion buffer")); }
        let name = super::ScriptTextEncoding::Cp932.decode(name);
        let path = super::save_path(self.save_root, &name)
            .map_err(|_| native_state_error("save filename is outside the private save directory"))?;
        if index == 990 {
            match fs::metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(NativeReturn::Continue),
                Err(_) => return Err(native_state_error("native 990 private save metadata query failed")),
                Ok(metadata) if metadata.is_dir() => return Ok(NativeReturn::Continue),
                Ok(metadata) => {
                    #[cfg(windows)]
                    if metadata.permissions().readonly() {
                        let mut permissions = metadata.permissions();
                        permissions.set_readonly(false);
                        fs::set_permissions(&path, permissions)
                            .map_err(|_| native_state_error("native 990 could not clear read-only save attributes"))?;
                    }
                    #[cfg(not(windows))]
                    let _ = metadata;
                }
            }
            fs::remove_file(&path).map_err(|_| native_state_error("native 990 private save removal failed"))?;
            return Ok(NativeReturn::Continue);
        }
        let bytes = self.css_handles[&read_u32(arguments, 0)?].to_css()
            .map_err(|_| native_state_error("native 1031 CSS serialization failed"))?;
        let mut partial_output = false;
        let result = (|| -> std::io::Result<()> {
            storage_space::require_available_space(self.save_root, bytes.len())?;
            let previous = match fs::metadata(&path) {
                Ok(metadata) => Some(metadata.modified()?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            fs::create_dir_all(self.save_root)?;
            let mut file = OpenOptions::new().create(true).truncate(true).write(true).open(&path)?;
            partial_output = true;
            file.write_all(&bytes)?;
            file.flush()?;
            partial_output = false;
            // The wrapper's third helper argument is zero. GetFileTime and
            // SetFileTime receive only lpLastWriteTime, preserving old mtime.
            if let Some(time) = previous { file.set_times(FileTimes::new().set_modified(time))?; }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(NativeReturn::Value(1)),
            Err(error) => {
                if partial_output {
                    fs::remove_file(&path)
                        .map_err(|_| native_state_error("native 1031 failed to remove its incomplete CSS output"))?;
                }
                self.events.push(super::LogicEvent { frame: self.frame, kind: "save-file-error".into(),
                    detail: format!("native1031 {}: {error}", path.display()) });
                Ok(NativeReturn::Value(0))
            }
        }
    }

    pub(super) fn append_data_object_native(&mut self, arguments: &[u8]) -> Result<NativeReturn> {
        let container = read_u32(arguments, 0)?;
        if container == 0 { return Ok(NativeReturn::Value(0)); }
        let record = read_u32(arguments, 4)?;
        let source = read_u32(arguments, 8)?;
        if source == 0 { return Ok(NativeReturn::Value(0)); }
        let bytes = if let Some(snapshot) = self.motion_snapshots.get(&source) {
            snapshot.storage.get(..snapshot.size)
                .ok_or_else(|| native_state_error("motion snapshot size exceeds its storage"))?.to_vec()
        } else {
            self.data_services.ordinary.raw_image(source)?
        };
        self.css_handles.get_mut(&container)
            .ok_or_else(|| native_state_error("native 387 has an invalid/wrong-type data container"))?
            .append(record, &bytes)
            .map_err(|_| native_state_error("native 387 data-container append failed"))?;
        Ok(NativeReturn::Value(1))
    }

    pub(super) fn clone_data_container_native(&mut self, arguments: &[u8]) -> Result<NativeReturn> {
        let source = read_u32(arguments, 0)?;
        if source == 0 { return Ok(NativeReturn::Value(0)); }
        let container = self.css_handles.get(&source)
            .ok_or_else(|| native_state_error("native 390 has an invalid/wrong-type data container"))?
            .try_clone_container()
            .map_err(|_| native_state_error("native 390 container clone failed"))?;
        let handle = self.allocate_handle();
        self.css_handles.insert(handle, container);
        Ok(NativeReturn::Value(handle))
    }
}
