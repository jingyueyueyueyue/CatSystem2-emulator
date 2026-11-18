//! Ordinary 29..32 file objects. Game sources are immutable; writable objects
//! are rooted in the emulator's save directory. Handles are opaque typed IDs.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use cs2_runtime::MediaResourceReader;
use cs2_script::{NativeReturn, Result, Vm};

use super::{LoggingHost, expand_kcs_format, native_state_error, read_u32, trim_nul};

const MAX_IO: usize = 64 * 1024 * 1024;
const MAX_OPEN_FILES: usize = 4096;

#[derive(Default)]
pub(super) struct FileNativeStore {
    objects: BTreeMap<u32, ScriptFile>,
}

enum ScriptFile {
    Resource(MediaResourceReader),
    Read(File),
    Write(File),
}

impl FileNativeStore {
    pub(crate) fn graph_open_count(&self) -> usize { self.objects.len() }
}

impl LoggingHost<'_> {
    pub(super) fn write_text_file_native567(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let address = read_u32(args, 0)?;
        vm.read(address, 1)?;
        let text = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        let text = trim_nul(&text);
        let name = vm.read_string(address)?;
        if name.len() >= 1024 {
            return Err(native_state_error("native 567 raw filename exceeds its conversion buffer"));
        }
        let name = super::ScriptTextEncoding::Cp932.decode(&name).replace('\\', "/");
        let path = save_file(self.save_root, &name)?;
        let mut options = OpenOptions::new();
        options.create(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let mut file = options.open(path)
            .map_err(|_| native_state_error("native 567 private output file could not be opened"))?;
        let length = file.metadata()
            .map_err(|_| native_state_error("native 567 output size query failed"))?.len();
        if length > u64::from(u32::MAX) {
            return Err(native_state_error("native 567 output exceeds its original DWORD size"));
        }
        // 52e7a0 obtains size, then 52ea90(size) clamps equality to size-1.
        // Preserve that last-byte overwrite; ordinary append would differ.
        // On an empty file the original negative seek fails at position zero.
        file.seek(SeekFrom::Start(length.saturating_sub(1)))
            .map_err(|_| native_state_error("native 567 output seek failed"))?;
        file.write_all(text).and_then(|_| file.write_all(b"\r\n")).and_then(|_| file.flush())
            .map_err(|_| native_state_error("native 567 output write/flush failed"))?;
        Ok(NativeReturn::Continue)
    }

    pub(super) fn file_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        match index {
            29 => {
                let raw = vm.read_string(read_u32(args, 0)?)?;
                let name = expand_kcs_format(vm, &raw, 0)?;
                let raw_mode = vm.read_string(read_u32(args, 4)?)?;
                let mode = expand_kcs_format(vm, &raw_mode, 0)?;
                let selector = trim_nul(&mode).iter().find(|&&byte| matches!(byte, b'r' | b'w')).copied()
                    .ok_or_else(|| native_state_error("native 29 mode has no recovered r/w selector"))?;
                let name = self.text_encoding.decode(trim_nul(&name)).replace('\\', "/");
                if name.encode_utf16().count() >= 1024 {
                    return Err(native_state_error("native 29 formatted filename exceeds capacity"));
                }
                if self.data_services.files.objects.len() >= MAX_OPEN_FILES {
                    return Err(native_state_error("native 29 file object limit exceeded"));
                }
                let save_path = save_file(self.save_root, &name)?;
                let file = if selector == b'w' {
                    if let Some(parent) = save_path.parent() {
                        fs::create_dir_all(parent).map_err(|_| native_state_error("native 29 save directory creation failed"))?;
                    }
                    match OpenOptions::new().create(true).write(true).truncate(true).open(save_path) {
                        Ok(file) => ScriptFile::Write(file),
                        Err(_) => return Ok(NativeReturn::Value(0)),
                    }
                } else if save_path.is_file() {
                    match File::open(save_path) {
                        Ok(file) => ScriptFile::Read(file),
                        Err(_) => return Ok(NativeReturn::Value(0)),
                    }
                } else if let Some(candidate) = self.resources.resolve(&name)
                    .map_err(|_| native_state_error("native 29 resource routing failed"))? {
                    ScriptFile::Resource(self.resources.open_media_candidate(candidate, 4 * 1024 * 1024 * 1024)
                        .map_err(|_| native_state_error("native 29 resource could not be opened"))?)
                } else {
                    return Ok(NativeReturn::Value(0));
                };
                let handle = self.allocate_handle();
                self.data_services.files.objects.insert(handle, file);
                Ok(NativeReturn::Value(handle))
            }
            30 | 31 => {
                let address = read_u32(args, 0)?;
                let length = read_u32(args, 4)?.wrapping_mul(read_u32(args, 8)?) as usize;
                let handle = read_u32(args, 12)?;
                if handle == 0 { return Ok(NativeReturn::Value(0)); }
                if length > MAX_IO { return Err(native_state_error("native file transfer exceeds 64 MiB")); }
                if length == 0 { return Ok(NativeReturn::Value(0)); }
                let object = self.data_services.files.objects.get_mut(&handle)
                    .ok_or_else(|| native_state_error("native file object is invalid or already closed"))?;
                if index == 30 {
                    // Validate the complete destination before advancing the
                    // resource cursor. Short reads leave the suffix unchanged.
                    vm.read(address, length)?;
                    let mut bytes = vec![0; length];
                    let mut written = 0;
                    while written < length {
                        let count = match object {
                            ScriptFile::Resource(file) => file.read(&mut bytes[written..]),
                            ScriptFile::Read(file) => file.read(&mut bytes[written..]),
                            ScriptFile::Write(_) => return Err(native_state_error("native 30 cannot read a write-only file")),
                        };
                        match count {
                            Ok(0) => break,
                            Ok(count) => written += count,
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => return Ok(NativeReturn::Value(0)),
                        }
                    }
                    vm.write(address, &bytes[..written])?;
                    Ok(NativeReturn::Value(written as u32))
                } else {
                    let bytes = vm.read(address, length)?;
                    let ScriptFile::Write(file) = object else {
                        return Err(native_state_error("native 31 cannot modify a game/read-only resource"));
                    };
                    let mut written = 0;
                    while written < bytes.len() {
                        match file.write(&bytes[written..]) {
                            Ok(0) => break,
                            Ok(count) => written += count,
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => return Ok(NativeReturn::Value(0)),
                        }
                    }
                    Ok(NativeReturn::Value(written as u32))
                }
            }
            32 => {
                let handle = read_u32(args, 0)?;
                if handle == 0 { return Ok(NativeReturn::Continue); }
                let file = self.data_services.files.objects.remove(&handle)
                    .ok_or_else(|| native_state_error("native 32 file object is invalid or already closed"))?;
                if let ScriptFile::Write(mut file) = file {
                    file.flush().map_err(|_| native_state_error("native 32 file flush failed"))?;
                }
                Ok(NativeReturn::Continue)
            }
            _ => Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }
    }
}

fn save_file(root: &Path, name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    if name.is_empty() || path.components().any(|component| !matches!(component, Component::Normal(_))) {
        return Err(native_state_error("native file name must be relative to the game/save root"));
    }
    Ok(root.join(path))
}
