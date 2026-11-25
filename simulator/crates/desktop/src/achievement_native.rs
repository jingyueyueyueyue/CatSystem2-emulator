//! Independent achievement receiver for the identified Steam native family.
//!
//! This stores achievements in the simulator's private profile. It never loads
//! steam_api.dll and does not claim to publish achievements to a Steam account.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, bail};
use cs2_formats::ConfigXml;
use cs2_runtime::RuntimeError;
use cs2_script::{NativeReturn, Result, Vm};
use super::{HeadlessPlayer, LoggingHost, LogicEvent, expand_kcs_format, native_state_error, read_u32};

const MAGIC: &[u8; 8] = b"CS2ACH01";
const FILE: &str = "simulator-achievements.dat";
const MAX_FILE: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAchievement {
    /// Exact NAME bytes used by the script and the source achievement table.
    pub id: Vec<u8>,
    pub comment: String,
    pub unlocked: bool,
}

#[derive(Default)]
pub(super) struct AchievementState {
    initialized: bool,
    entries: BTreeMap<Vec<u8>, LocalAchievement>,
    // Preserve previously known entries if an update temporarily removes them.
    stored: BTreeSet<Vec<u8>>,
}

impl AchievementState {
    pub(super) fn supports(index: u32) -> bool { matches!(index, 1041..=1044) }

    fn initialize(&mut self, document: Option<ConfigXml>, root: &Path) -> anyhow::Result<()> {
        if self.initialized { return Ok(()); }
        let stored = load(root)?;
        let mut entries = BTreeMap::new();
        if let Some(document) = document {
            // The producer enumerates the XML root's direct children, reads
            // child/NAME and child/COMMENT, and keys the map by NAME (not child).
            for field in document.fields.iter().filter(|field| field.path.len() == 2) {
                let source = &field.path;
                let name_path = [source[0].as_slice(), source[1].as_slice(), b"NAME".as_slice()];
                let comment_path = [source[0].as_slice(), source[1].as_slice(), b"COMMENT".as_slice()];
                let id = document.text(&name_path).unwrap_or_default().to_vec();
                if id.len() > 8191 || entries.len() >= MAX_ENTRIES {
                    bail!("achievement declaration exceeds the receiver capacity");
                }
                let comment = std::str::from_utf8(document.text(&comment_path).unwrap_or_default())
                    .context("achievement COMMENT is not UTF-8")?.to_owned();
                // std::map insertion in the producer keeps the first NAME.
                let unlocked = stored.contains(&id);
                entries.entry(id.clone()).or_insert(LocalAchievement { id, comment, unlocked });
            }
        }
        self.entries = entries;
        self.stored = stored;
        self.initialized = true;
        Ok(())
    }

    fn unlock(&mut self, id: &[u8], root: &Path) -> anyhow::Result<bool> {
        let Some(entry) = self.entries.get(id) else { return Ok(false); };
        if entry.unlocked { return Ok(false); }
        let mut next = self.stored.clone();
        next.insert(id.to_vec());
        persist(root, &next)?;
        self.stored = next;
        self.entries.get_mut(id).expect("entry retained across persistence").unlocked = true;
        Ok(true)
    }

    fn reset(&mut self, root: &Path) -> anyhow::Result<()> {
        persist(root, &BTreeSet::new())?;
        self.stored.clear();
        for entry in self.entries.values_mut() { entry.unlocked = false; }
        Ok(())
    }
}

impl HeadlessPlayer {
    /// These are local profile records, independent of ordinary scene saves.
    pub fn local_achievements(&self) -> impl Iterator<Item = &LocalAchievement> {
        self.data_services.achievements.entries.values()
    }

    pub(super) fn initialize_local_achievements(&mut self) -> anyhow::Result<()> {
        if !self.data_services.ordinary_native_table.as_ref()
            .is_some_and(|table| table.has_achievement_family())
        {
            return Ok(());
        }
        let document = read_declarations(&self.scan.resources)?;
        self.data_services.achievements.initialize(document, &self.save_root)?;
        self.log.push(format!("local achievement profile: {} entries; Steam account synchronization unavailable",
            self.data_services.achievements.entries.len()));
        Ok(())
    }
}

impl LoggingHost<'_> {
    pub(super) fn achievement_native(&mut self, index: u32, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        if !self.data_services.ordinary_native_table.as_ref()
            .is_some_and(|table| table.has_achievement_family())
        {
            return Err(native_state_error("achievement native requires a registered Steam receiver family"));
        }
        let expected = if matches!(index, 1042 | 1043) { 4 } else { 0 };
        if args.len() != expected {
            return Err(native_state_error("achievement native has an invalid argument byte count"));
        }
        // Formatting happens even when the original platform service/table is
        // absent. Keep that evaluation order and the raw NAME byte identity.
        let id = if matches!(index, 1042 | 1043) {
            Some(expand_kcs_format(vm, &vm.read_string(read_u32(args, 0)?)?, 0)?)
        } else { None };
        if !self.data_services.achievements.initialized {
            let document = read_declarations(self.resources).map_err(|error| self.achievement_error(error))?;
            self.data_services.achievements.initialize(document, self.save_root)
                .map_err(|error| self.achievement_error(error))?;
        }
        match index {
            1041 => Ok(NativeReturn::Continue),
            1042 => {
                let id = id.as_deref().expect("formatted unlock NAME");
                let changed = self.data_services.achievements.unlock(id, self.save_root)
                    .map_err(|error| self.achievement_error(error))?;
                if changed {
                    self.events.push(LogicEvent { frame: self.frame, kind: "local-achievement-unlocked".into(),
                        detail: String::from_utf8_lossy(id).into_owned() });
                }
                Ok(NativeReturn::Continue)
            }
            1043 => Ok(NativeReturn::Value(u32::from(self.data_services.achievements.entries
                .get(id.as_deref().expect("formatted query NAME")).is_some_and(|entry| entry.unlocked)))),
            1044 => {
                let returned = self.vm_context_native(vm)?;
                self.data_services.achievements.reset(self.save_root)
                    .map_err(|error| self.achievement_error(error))?;
                self.events.push(LogicEvent { frame: self.frame, kind: "local-achievements-reset".into(),
                    detail: "private simulator profile".into() });
                // The original status-1 wrapper does not write its output
                // DWORD. The dispatcher aliases it to the current VM pointer.
                Ok(returned)
            }
            _ => Err(native_state_error("not an achievement native")),
        }
    }

    fn achievement_error(&mut self, error: anyhow::Error) -> cs2_script::ScriptError {
        self.events.push(LogicEvent { frame: self.frame, kind: "local-achievement-error".into(),
            detail: format!("{error:#}") });
        native_state_error("local achievement operation failed; see local-achievement-error")
    }
}

fn read_declarations(resources: &cs2_runtime::ResourceIndex) -> anyhow::Result<Option<ConfigXml>> {
    match resources.read_category(b"config", "SteamAchievements.xml") {
        Ok(bytes) => Ok(Some(ConfigXml::parse_runtime(&bytes).context("parse SteamAchievements.xml")?)),
        Err(RuntimeError::ResourceNotFound(_)) | Err(RuntimeError::InvalidResourceName(_)) => Ok(None),
        Err(error) => Err(error).context("read SteamAchievements.xml through config category"),
    }
}

fn load(root: &Path) -> anyhow::Result<BTreeSet<Vec<u8>>> {
    let file = match std::fs::File::open(root.join(FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error).context("open local achievement profile"),
    };
    let mut bytes = Vec::new();
    file.take((MAX_FILE + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE || !bytes.starts_with(MAGIC) || bytes.len() < 12 {
        bail!("invalid local achievement profile header or length");
    }
    let count = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
    if count > MAX_ENTRIES { bail!("local achievement entry limit exceeded"); }
    let mut offset = 12usize;
    let mut keys = BTreeSet::new();
    for _ in 0..count {
        let length_end = offset.checked_add(4).filter(|&end| end <= bytes.len())
            .context("truncated local achievement length")?;
        let size = u32::from_le_bytes(bytes[offset..length_end].try_into()?) as usize;
        offset = length_end;
        let end = offset.checked_add(size).filter(|&end| end <= bytes.len())
            .context("truncated local achievement NAME")?;
        if size > 8191 || bytes[offset..end].contains(&0) || !keys.insert(bytes[offset..end].to_vec()) {
            bail!("invalid or duplicated local achievement NAME");
        }
        offset = end;
    }
    if offset != bytes.len() { bail!("trailing local achievement profile bytes"); }
    Ok(keys)
}

fn persist(root: &Path, keys: &BTreeSet<Vec<u8>>) -> anyhow::Result<()> {
    let mut bytes = MAGIC.to_vec();
    if keys.len() > MAX_ENTRIES { bail!("local achievement entry limit exceeded"); }
    bytes.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    for key in keys {
        bytes.extend_from_slice(&(key.len() as u32).to_le_bytes());
        bytes.extend_from_slice(key);
    }
    if bytes.len() > MAX_FILE { bail!("local achievement profile is too large"); }
    std::fs::create_dir_all(root).context("create private achievement directory")?;
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = root.join(format!("{FILE}.pending-{}-{sequence}", std::process::id()));
    let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&temporary)
        .context("write local achievement profile")?;
    let written = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| replace(&temporary, &root.join(FILE)));
    if result.is_err() { let _ = std::fs::remove_file(&temporary); }
    result.context("commit local achievement profile")
}

#[cfg(not(windows))]
fn replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<_> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 1 | 8) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TempDir;
    use cs2_script::{Host, KcsProgram};

    #[test]
    #[ignore = "requires the installed NEKOPARA Vol.3 game"]
    fn achievement_family_dispatches_local_save_through_opened_game() -> anyhow::Result<()> {
        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join(concat!(
            "../../../games/[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 ",
            "steam\u{7248} (files)/nekopara_vol3",
        ));
        let profile = TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(&game, &profile.0)?;
        let id = b"ACHIEVEMENT_2_15";
        assert!(player.local_achievements().any(|entry| entry.id == id && !entry.unlocked));
        player.data_services.native_profile = None;
        let mut vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 64], 0)?)?;
        vm.write(4, b"ACHIEVEMENT_2_15\0")?;
        let args = 4u32.to_le_bytes();
        player.with_host(|host| {
            assert_eq!(host.call(1041, &[], &mut vm)?, NativeReturn::Continue);
            assert_eq!(host.call(1043, &args, &mut vm)?, NativeReturn::Value(0));
            assert_eq!(host.call(1042, &args, &mut vm)?, NativeReturn::Continue);
            assert_eq!(host.call(1043, &args, &mut vm)?, NativeReturn::Value(1));
            Ok(())
        })?;
        assert_eq!(load(&profile.0)?, BTreeSet::from([id.to_vec()]));
        drop(player);

        let mut reopened = HeadlessPlayer::open_with_save_root(&game, &profile.0)?;
        assert!(reopened.local_achievements().any(|entry| entry.id == id && entry.unlocked));
        reopened.with_host(|host| {
            assert_eq!(host.call(1043, &args, &mut vm)?, NativeReturn::Value(1));
            let identity = host.call(549, &[], &mut vm)?;
            assert!(matches!(identity, NativeReturn::Value(value) if value != 0));
            assert_ne!(host.call(549, &[], &mut Vm::new())?, identity);
            assert_eq!(host.call(1044, &[], &mut vm)?, identity);
            assert_eq!(host.call(1043, &args, &mut vm)?, NativeReturn::Value(0));
            Ok(())
        })?;
        assert!(load(&profile.0)?.is_empty());

        reopened.data_services.ordinary_native_table = None;
        reopened.with_host(|host| {
            let error = host.call(1042, &args, &mut vm).unwrap_err();
            assert!(error.to_string().contains("registered Steam receiver family"));
            let error = host.call(549, &[], &mut vm).unwrap_err();
            assert!(error.to_string().contains("registered VM return-slot alias"));
            Ok(())
        })
    }
}
