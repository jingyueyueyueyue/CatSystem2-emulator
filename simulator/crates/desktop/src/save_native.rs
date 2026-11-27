//! Container operations using the same image bytes and motion snapshots as
//! their producers. Copies never retain a pointer into another object's data.
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, native_state_error, read_u32};

#[path = "save_storage_space.rs"]
mod storage_space;

pub(super) fn remove_slot(path: &Path) -> anyhow::Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    #[cfg(windows)]
    let original_permissions = if metadata.permissions().readonly() {
        let original = metadata.permissions();
        let mut permissions = original.clone();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
        Some(original)
    } else {
        None
    };
    #[cfg(not(windows))]
    let _ = metadata;
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => {
            #[cfg(windows)]
            if let Some(original) = original_permissions {
                use anyhow::Context;
                fs::set_permissions(path, original)
                    .with_context(|| format!("restore save slot attributes after deletion failed: {error}"))?;
            }
            Err(error.into())
        }
    }
}

impl LoggingHost<'_> {
    pub(super) fn save_file_native(&mut self, index: u32, arguments: &[u8], vm: &Vm) -> Result<NativeReturn> {
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
            if let Err(error) = remove_slot(&path) {
                self.events.push(super::LogicEvent { frame: self.frame, kind: "save-file-error".into(),
                    detail: format!("native990 {}: {error:#}", path.display()) });
            }
            return Ok(NativeReturn::Continue);
        }
        let bytes = self.css_handles[&read_u32(arguments, 0)?].to_css()
            .map_err(|_| native_state_error("native 1031 CSS serialization failed"))?;
        let result = (|| -> anyhow::Result<()> {
            storage_space::require_available_space(self.save_root, bytes.len())?;
            let previous = match fs::metadata(&path) {
                Ok(metadata) => Some(metadata.modified()?),
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            fs::create_dir_all(self.save_root)?;
            // The wrapper preserves the previous slot's last-write time.
            if let Some(time) = previous {
                cs2_runtime::write_private_file_with_modified_time(&path, &bytes, time)?;
            } else {
                cs2_runtime::write_private_file(&path, &bytes)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(NativeReturn::Value(1)),
            Err(error) => {
                self.events.push(super::LogicEvent { frame: self.frame, kind: "save-file-error".into(),
                    detail: format!("native1031 {}: {error:#}", path.display()) });
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::HeadlessPlayer;
    use cs2_formats::CssFile;
    use std::fs::{self, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::PathBuf;

    struct TestRoot(PathBuf);

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn save_native_keeps_locked_slot_and_retries_after_release() -> anyhow::Result<()> {
        let root = TestRoot(std::env::temp_dir().join(format!(
            "cs2-native-save-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos(),
        )));
        fs::create_dir(&root.0)?;
        let mut player = HeadlessPlayer::open_with_save_root(&root.0, &root.0)?;
        let path = root.0.join("save0000.dat");
        let original = CssFile::new(1).to_css()?;
        fs::write(&path, &original)?;
        let modified = fs::metadata(&path)?.modified()?;

        let program = cs2_script::KcsProgram::from_code(&[0x3b], Vec::new(), 0)?;
        let mut vm = Vm::from_program(program)?;
        let address = vm.allocate_region(b"save0000.dat\0")?;
        let filename = address.to_le_bytes();
        let handle = 1u32;
        let arguments = [handle.to_le_bytes(), filename].concat();
        let call = |player: &mut HeadlessPlayer, index, args: &[u8]| {
            player.with_host(|host| Ok(host.save_file_native(index, args, &vm)?))
        };
        player.css_handles.insert(handle, CssFile::new(2));

        // Permit reads and writes through the open handle, but prevent file
        // replacement and deletion until the other owner releases it.
        let lock = OpenOptions::new().read(true).share_mode(0x1 | 0x2).open(&path)?;
        assert_eq!(call(&mut player, 1031, &arguments)?, NativeReturn::Value(0));
        assert_eq!(call(&mut player, 990, &filename)?, NativeReturn::Continue);
        assert_eq!(fs::read(&path)?, original);
        assert_eq!(fs::metadata(&path)?.modified()?, modified);
        assert_eq!(fs::read_dir(&root.0)?.count(), 1);
        assert_eq!(player.events().iter().filter(|event| event.kind == "save-file-error").count(), 2);
        drop(lock);

        assert_eq!(call(&mut player, 1031, &arguments)?, NativeReturn::Value(1));
        assert_eq!(CssFile::parse(&fs::read(&path)?)?.tag, 2);
        assert_eq!(fs::metadata(&path)?.modified()?, modified);
        assert_eq!(call(&mut player, 990, &filename)?, NativeReturn::Continue);
        assert_eq!(call(&mut player, 990, &filename)?, NativeReturn::Continue);
        assert!(!path.exists());
        Ok(())
    }
}
