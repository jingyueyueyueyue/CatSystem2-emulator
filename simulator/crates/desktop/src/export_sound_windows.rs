//! Native Windows adapter for the type46 AppEvents protocol.
//! All calls use this process's platform APIs; no game binary is loaded.
use anyhow::{Result, bail};

use super::{PreparedScheme, RegistryMode, RegistryOperation, SoundQuestion};

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    type KeyHandle = *mut c_void;
    const KEY_READ: u32 = 0x20019;
    const KEY_WRITE: u32 = 0x20006;
    const NOT_FOUND: i32 = 2;
    const NO_MORE_ITEMS: i32 = 259;
    const MAX_KEYS: u32 = 4096;
    const MAX_OPERATIONS: usize = 65_536;
    const HKCU: KeyHandle = (0x8000_0001u32 as i32 as isize) as KeyHandle;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            key: KeyHandle,
            name: *const u16,
            options: u32,
            access: u32,
            output: *mut KeyHandle,
        ) -> i32;
        fn RegCreateKeyExW(
            key: KeyHandle,
            name: *const u16,
            reserved: u32,
            class: *mut u16,
            options: u32,
            access: u32,
            security: *const c_void,
            output: *mut KeyHandle,
            disposition: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: KeyHandle) -> i32;
        fn RegEnumKeyExW(
            key: KeyHandle,
            index: u32,
            name: *mut u16,
            length: *mut u32,
            reserved: *mut u32,
            class: *mut u16,
            class_length: *mut u32,
            time: *mut c_void,
        ) -> i32;
        fn RegQueryValueExW(
            key: KeyHandle,
            name: *const u16,
            reserved: *mut u32,
            kind: *mut u32,
            bytes: *mut u8,
            length: *mut u32,
        ) -> i32;
        fn RegQueryValueExA(
            key: KeyHandle,
            name: *const u8,
            reserved: *mut u32,
            kind: *mut u32,
            bytes: *mut u8,
            length: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            key: KeyHandle,
            name: *const u16,
            reserved: u32,
            kind: u32,
            bytes: *const u8,
            length: u32,
        ) -> i32;
        fn RegSetValueExA(
            key: KeyHandle,
            name: *const u8,
            reserved: u32,
            kind: u32,
            bytes: *const u8,
            length: u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetACP() -> u32;
        fn WideCharToMultiByte(
            code_page: u32,
            flags: u32,
            input: *const u16,
            input_length: i32,
            output: *mut u8,
            output_length: i32,
            replacement: *const u8,
            replaced: *mut i32,
        ) -> i32;
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            input: *const u8,
            input_length: i32,
            output: *mut u16,
            output_length: i32,
        ) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            owner: *mut c_void,
            text: *const u16,
            caption: *const u16,
            flags: u32,
        ) -> i32;
    }

    struct Key(KeyHandle);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn open(path: &str, write: bool, create: bool) -> Result<Option<Key>> {
        let path_wide = wide(path);
        let mut output = null_mut();
        let code = unsafe {
            if create {
                RegCreateKeyExW(
                    HKCU,
                    path_wide.as_ptr(),
                    0,
                    null_mut(),
                    0,
                    KEY_WRITE,
                    null(),
                    &mut output,
                    null_mut(),
                )
            } else {
                RegOpenKeyExW(
                    HKCU,
                    path_wide.as_ptr(),
                    0,
                    if write { KEY_WRITE } else { KEY_READ },
                    &mut output,
                )
            }
        };
        if code == NOT_FOUND && !create {
            return Ok(None);
        }
        if code != 0 {
            bail!("sound-set registry open {path}: Windows error {code}");
        }
        Ok(Some(Key(output)))
    }

    fn children(path: &str) -> Result<Vec<String>> {
        let Some(key) = open(path, false, false)? else {
            bail!("sound-set registry branch is absent: {path}");
        };
        let mut result = Vec::new();
        for index in 0..MAX_KEYS {
            let mut name = [0u16; 260];
            let mut length = name.len() as u32;
            let code = unsafe {
                RegEnumKeyExW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            };
            if code == NO_MORE_ITEMS {
                return Ok(result);
            }
            if code != 0 {
                bail!("sound-set registry enumeration {path}: Windows error {code}");
            }
            result.push(String::from_utf16(&name[..length as usize])?);
        }
        bail!("sound-set registry enumeration exceeds 4096 keys")
    }

    fn read(path: &str, mode: RegistryMode) -> Result<Option<String>> {
        let Some(key) = open(path, false, false)? else {
            return Ok(None);
        };
        // Both original readers set the query byte count to 260, including
        // the W variant whose stack array has capacity for 260 wide units.
        let mut buffer = [0u8; 260];
        let mut length = buffer.len() as u32;
        let mut kind = 1u32;
        let code = unsafe {
            match mode {
                RegistryMode::Narrow => RegQueryValueExA(
                    key.0,
                    null(),
                    null_mut(),
                    &mut kind,
                    buffer.as_mut_ptr(),
                    &mut length,
                ),
                RegistryMode::WideCharacterCount => RegQueryValueExW(
                    key.0,
                    null(),
                    null_mut(),
                    &mut kind,
                    buffer.as_mut_ptr(),
                    &mut length,
                ),
            }
        };
        if code == NOT_FOUND {
            return Ok(Some(String::new()));
        }
        if code != 0 {
            bail!("sound-set registry read {path}: Windows error {code}");
        }
        if kind != 1 && kind != 2 {
            bail!("sound-set registry value is not a string: {path}");
        }
        let bytes = &buffer[..length as usize];
        Ok(Some(match mode {
            RegistryMode::Narrow => from_ansi(bytes)?,
            RegistryMode::WideCharacterCount => {
                let words: Vec<_> = bytes
                    .chunks_exact(2)
                    .map(|bytes| u16::from_le_bytes(bytes.try_into().unwrap()))
                    .take_while(|&v| v != 0)
                    .collect();
                String::from_utf16(&words)?
            }
        }))
    }

    fn from_ansi(bytes: &[u8]) -> Result<String> {
        let bytes = &bytes[..bytes.iter().position(|&v| v == 0).unwrap_or(bytes.len())];
        if bytes.is_empty() {
            return Ok(String::new());
        }
        let length =
            unsafe { MultiByteToWideChar(0, 0, bytes.as_ptr(), bytes.len() as i32, null_mut(), 0) };
        if length <= 0 {
            bail!("sound-set system ANSI decoding failed");
        }
        let mut words = vec![0; length as usize];
        if unsafe {
            MultiByteToWideChar(
                0,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                words.as_mut_ptr(),
                length,
            )
        } != length
        {
            bail!("sound-set system ANSI decoding was incomplete");
        }
        Ok(String::from_utf16(&words)?)
    }

    fn ansi(text: &str) -> Result<Vec<u8>> {
        let words = wide(text);
        let mut replaced = 0;
        let replaced_pointer = if unsafe { GetACP() } == 65001 {
            null_mut()
        } else {
            &mut replaced
        };
        let length = unsafe {
            WideCharToMultiByte(
                0,
                0,
                words.as_ptr(),
                words.len() as i32,
                null_mut(),
                0,
                null(),
                replaced_pointer,
            )
        };
        if length <= 0 || replaced != 0 {
            bail!("sound-set filename is not representable in system ANSI");
        }
        let mut bytes = vec![0; length as usize];
        let written = unsafe {
            WideCharToMultiByte(
                0,
                0,
                words.as_ptr(),
                words.len() as i32,
                bytes.as_mut_ptr(),
                length,
                null(),
                replaced_pointer,
            )
        };
        if written != length || replaced != 0 {
            bail!("sound-set system ANSI encoding was incomplete");
        }
        Ok(bytes)
    }

    pub(super) fn registration_plan(
        scheme: &PreparedScheme,
        mode: RegistryMode,
    ) -> Result<Vec<RegistryOperation>> {
        let mut plan = vec![RegistryOperation {
            key: format!("AppEvents\\Schemes\\Names\\{}", scheme.theme),
            value: scheme.theme.clone(),
            create: true,
        }];
        let apps = "AppEvents\\Schemes\\Apps";
        let mut scanned = 0usize;
        for app in children(apps)? {
            let app_path = format!("{apps}\\{app}");
            for event in children(&app_path)? {
                scanned += 1;
                if scanned >= MAX_OPERATIONS {
                    bail!("sound-set discovery exceeds 65536 events");
                }
                if let Some(value) = read(&format!("{app_path}\\{event}\\.Current"), mode)? {
                    if plan.len() >= MAX_OPERATIONS {
                        bail!("sound-set registration plan exceeds 65536 writes");
                    }
                    plan.push(RegistryOperation {
                        key: format!("{app_path}\\{event}\\{}", scheme.theme),
                        value,
                        create: true,
                    });
                }
            }
        }
        super::super::append_overrides(&mut plan, scheme);
        Ok(plan)
    }

    pub(super) fn activation_plan(
        scheme: &PreparedScheme,
        mode: RegistryMode,
    ) -> Result<Vec<RegistryOperation>> {
        let mut plan = vec![RegistryOperation {
            key: "AppEvents\\Schemes".into(),
            value: scheme.theme.clone(),
            create: false,
        }];
        // The original current-selection receiver copies only .Default app
        // events, even though registration inherits all applications.
        let root = "AppEvents\\Schemes\\Apps\\.Default";
        for event in children(root)? {
            if let Some(value) = read(&format!("{root}\\{event}\\{}", scheme.theme), mode)? {
                plan.push(RegistryOperation {
                    key: format!("{root}\\{event}\\.Current"),
                    value,
                    create: false,
                });
            }
        }
        Ok(plan)
    }

    pub(super) fn write(operation: &RegistryOperation, mode: RegistryMode) -> Result<()> {
        let Some(key) = open(&operation.key, true, operation.create)? else {
            // XML events absent from Windows' own event list are skipped by
            // the original non-creating open of the override receiver.
            return Ok(());
        };
        let code = unsafe {
            match mode {
                RegistryMode::Narrow => {
                    let value = ansi(&operation.value)?;
                    RegSetValueExA(key.0, null(), 0, 1, value.as_ptr(), value.len() as u32)
                }
                RegistryMode::WideCharacterCount => {
                    let words = wide(&operation.value);
                    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
                    RegSetValueExW(key.0, null(), 0, 1, bytes.as_ptr(), words.len() as u32)
                }
            }
        };
        if code != 0 {
            bail!(
                "sound-set registry write {}: Windows error {code}",
                operation.key
            );
        }
        Ok(())
    }

    pub(super) fn confirm(question: SoundQuestion) -> Result<bool> {
        let (caption, text) = match question {
            SoundQuestion::Register => (
                "Register sound scheme",
                "The sound files have been exported. Register them as a Windows sound scheme?",
            ),
            SoundQuestion::Activate => (
                "Select sound scheme",
                "The sound scheme has been registered. Select it as the current Windows sound scheme?",
            ),
        };
        let result =
            unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide(caption).as_ptr(), 4) };
        match result {
            6 => Ok(true),
            7 => Ok(false),
            _ => bail!("sound-set confirmation failed"),
        }
    }
    pub(super) fn notify(activated: bool) -> Result<()> {
        let text = if activated {
            "The Windows sound scheme has been selected."
        } else {
            "The Windows sound scheme has been registered."
        };
        if unsafe {
            MessageBoxW(
                null_mut(),
                wide(text).as_ptr(),
                wide("Sound scheme").as_ptr(),
                0,
            )
        } == 0
        {
            bail!("sound-set completion dialog failed");
        }
        Ok(())
    }
}

#[cfg(windows)]
pub(super) fn confirm(question: SoundQuestion) -> Result<bool> {
    windows::confirm(question)
}
#[cfg(windows)]
pub(super) fn notify(activated: bool) -> Result<()> {
    windows::notify(activated)
}
#[cfg(windows)]
pub(super) fn registration_plan(
    scheme: &PreparedScheme,
    mode: RegistryMode,
) -> Result<Vec<RegistryOperation>> {
    windows::registration_plan(scheme, mode)
}
#[cfg(windows)]
pub(super) fn activation_plan(
    scheme: &PreparedScheme,
    mode: RegistryMode,
) -> Result<Vec<RegistryOperation>> {
    windows::activation_plan(scheme, mode)
}
#[cfg(windows)]
pub(super) fn write(operation: &RegistryOperation, mode: RegistryMode) -> Result<()> {
    windows::write(operation, mode)
}

#[cfg(not(windows))]
pub(super) fn confirm(_: SoundQuestion) -> Result<bool> {
    bail!("Windows sound-set consent needs a Windows platform or an explicit prompt adapter")
}
#[cfg(not(windows))]
pub(super) fn notify(_: bool) -> Result<()> {
    bail!("Windows sound-set UI is unavailable on this platform")
}
#[cfg(not(windows))]
pub(super) fn registration_plan(
    _: &PreparedScheme,
    _: RegistryMode,
) -> Result<Vec<RegistryOperation>> {
    bail!("Windows AppEvents registration is unavailable on this platform")
}
#[cfg(not(windows))]
pub(super) fn activation_plan(
    _: &PreparedScheme,
    _: RegistryMode,
) -> Result<Vec<RegistryOperation>> {
    bail!("Windows AppEvents selection is unavailable on this platform")
}
#[cfg(not(windows))]
pub(super) fn write(_: &RegistryOperation, _: RegistryMode) -> Result<()> {
    bail!("Windows AppEvents write is unavailable on this platform")
}
