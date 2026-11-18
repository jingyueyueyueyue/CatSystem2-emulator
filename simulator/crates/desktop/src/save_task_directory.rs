//! Explicit private save bindings and fixed Yukikoi directory production.
//!
//! Independently derived from PE 63de0e14: startup -> manager command37 ->
//! manager+84 -> script constructor argument8 -> task+104. This module does
//! not open files, change the process CWD, or select a guessed save folder.
//! The host explicitly relocates each directory binding to its private root.
//! The independent bootstrap uses a host-defined relative dot namespace; it
//! makes no claim about an unexamined PE's original directory producer.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use cs2_formats::ConfigXml;
use serde::{Deserialize, Serialize};

pub(crate) const YUKIKOI_SAVE_DIRECTORY_SHA256: &str =
    "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976";

const DIRECTORY_UNITS: usize = 260;

/// Resolved fields supplied by the platform/configuration owner. In this PE,
/// USERPROFILE means CSIDL_APPDATA (26), not the Windows profile directory;
/// ProgramData and ALLUSERSPROFILE both mean CSIDL_COMMON_APPDATA (35).
/// Inserted values are not expanded again. No environment-variable guesses.
#[derive(Default)]
pub(crate) struct SaveFolderBindings<'a> {
    pub user_application_data: Option<&'a [u16]>,
    pub common_application_data: Option<&'a [u16]>,
    pub brand: Option<&'a [u16]>,
    pub title: Option<&'a [u16]>,
    pub image_library: Option<&'a [u16]>,
}

/// Lazy host adapter for the two system markers. Call only for a field that
/// needs it; other platforms supply their explicit application-data binding.
/// This declaration does not query the host during module initialization.
pub(crate) fn windows_application_data(common: bool) -> Result<Vec<u16>> {
    #[cfg(windows)]
    {
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn SHGetFolderPathW(window: *mut std::ffi::c_void, folder: i32,
                token: *mut std::ffi::c_void, flags: u32, path: *mut u16) -> i32;
            fn SHGetSpecialFolderPathW(window: *mut std::ffi::c_void,
                path: *mut u16, folder: i32, create: i32) -> i32;
        }
        let folder = if common { 35 } else { 26 };
        let mut path = [0u16; DIRECTORY_UNITS];
        let status = unsafe { SHGetFolderPathW(std::ptr::null_mut(), folder,
            std::ptr::null_mut(), 0, path.as_mut_ptr()) };
        if status < 0 {
            path.fill(0);
            if unsafe { SHGetSpecialFolderPathW(std::ptr::null_mut(), path.as_mut_ptr(), folder, 0) } == 0 {
                bail!("task save-folder Windows application-data lookup failed");
            }
        }
        let end = path.iter().position(|&unit| unit == 0)
            .context("task save-folder Windows result is unterminated")?;
        if end == 0 { bail!("task save-folder Windows application-data result is empty"); }
        return Ok(path[..end].to_vec());
    }
    #[cfg(not(windows))]
    {
        let _ = common;
        bail!("task save folder requires an explicit platform application-data binding");
    }
}

/// A logical wide directory and its explicit independent-host relocation.
/// Keeping both avoids interpreting an absolute game path as permission to
/// write there. An empty original field stays empty; it is not a missing field.
/// Independent mode explicitly selects the host-relative dot namespace,
/// without claiming to recover an original PE field.
#[derive(Clone, Debug)]
pub(crate) struct SaveDirectoryBinding {
    original: Vec<u16>,
    private_root: PathBuf,
}

impl SaveDirectoryBinding {
    /// The caller supplies the actual directory value (command37 or a saved
    /// constructor field) and a deliberate private binding. Only the first NUL
    /// belongs to the field. No trimming, separator or Unicode normalization.
    pub(crate) fn relocated(original: &[u16], private_root: &Path) -> Result<Self> {
        let original = nul(original);
        check_directory(original)?;
        if private_root.as_os_str().is_empty() {
            bail!("task save directory has no explicit private-root binding");
        }
        Ok(Self { original: original.to_vec(), private_root: private_root.to_path_buf() })
    }

    /// `startup` must be the resource router's selected, decoded runtime XML,
    /// not a second loose-file search. Other PE versions require their own
    /// constructor/default evidence before selecting this profile.
    pub(crate) fn from_yukikoi_startup(
        pe_sha256: &str,
        startup: Option<&ConfigXml>,
        fields: &SaveFolderBindings<'_>,
        private_root: &Path,
    ) -> Result<Self> {
        if pe_sha256 != YUKIKOI_SAVE_DIRECTORY_SHA256 {
            bail!("task save-directory startup producer needs its exact PE profile");
        }
        // 67880b..678836: CSIDL_APPDATA\\cs2\\ういんどみる. This is the
        // fixed PE's initial value, NOT APP/brand or APP/title substitution.
        // 679802 uses that value only when FOLDER/save is missing.
        // Runtime queries skip only the special document root. An absent
        // document uses the recovered built-in value, not a fabricated XML.
        let configured = startup.filter(|xml| xml.root == b"document")
            .and_then(|xml| xml.text(&[b"document", b"FOLDER", b"save"]));
        let original = match configured {
            Some(bytes) => {
                let bytes = &bytes[..bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len())];
                let text = std::str::from_utf8(bytes)
                    .context("task save FOLDER/save is not decoded runtime XML")?;
                // The original XML wide-text helper retains the first UTF16
                // unit of each scalar, including an isolated high surrogate.
                let input: Vec<u16> = text.chars().take(4096).map(|character| {
                    let mut units = [0; 2];
                    character.encode_utf16(&mut units)[0]
                }).collect();
                if input.len() > 4095 { bail!("task save-folder XML wide field exceeds 4095 units"); }
                expand_folder(&input, fields)?
            }
            None => yukikoi_default_directory(fields)?,
        };
        Self::relocated(&original, private_root)
    }

    pub(crate) fn original(&self) -> &[u16] { &self.original }
    pub(crate) fn private_root(&self) -> &Path { &self.private_root }

    /// Input is the *already KCS-formatted* filename, before text decoding.
    /// 245/247/990/1031 all use 5ffc90 -> 5ffc30: code page932, flags1. They
    /// do not use the active CST encoding or the resource category resolver.
    pub(crate) fn resolve_expanded_filename(&self, expanded: &[u8]) -> Result<ResolvedSaveTaskPath> {
        let raw = &expanded[..expanded.iter().position(|&byte| byte == 0).unwrap_or(expanded.len())];
        let filename = encoding_rs::SHIFT_JIS
            .decode_without_bom_handling_and_without_replacement(raw)
            .context("task CSS filename has an unsupported malformed CP932 sequence")?;
        let wide: Vec<u16> = filename.encode_utf16().collect();
        // The wrappers have 1024-unit conversion scratch, but the shared
        // helpers concatenate into a 260-unit local buffer. Diagnose overflow
        // at the smallest real buffer instead of reproducing memory damage.
        let length = self.original.len().checked_add(1).and_then(|n| n.checked_add(wide.len()))
            .context("task CSS path length overflow")?;
        if length >= DIRECTORY_UNITS {
            bail!("task CSS directory plus separator and filename exceeds 259 UTF16 units");
        }
        let mut original_file = self.original.clone();
        original_file.push(u16::from(b'\\'));
        original_file.extend_from_slice(&wide);
        // Preserve the same private save boundary used by existing native
        // consumers, including component and symlink checks. Absolute names
        // cannot replace the directory, and there is no INT/game-file fallback.
        let private_path = super::save_path(&self.private_root, &filename)?;
        Ok(ResolvedSaveTaskPath {
            original_file,
            directory: self.private_root.clone(),
            path: private_path,
        })
    }
}

pub(crate) struct ResolvedSaveTaskPath {
    /// Logical directory + literal backslash + converted filename, no NUL.
    /// Independent mode uses the explicit host-relative dot namespace here.
    pub original_file: Vec<u16>,
    /// Use this for mkdir/1031 volume query, not a global or process CWD.
    pub directory: PathBuf,
    /// Use this exact path for CSS read/write, mtime and990 deletion.
    pub path: PathBuf,
}

/// Task identities remain the scheduler's identities; no second allocator.
/// Directory values are constructor snapshots, not a parent walk at I/O time.
#[derive(Default)]
pub(crate) struct SaveTaskDirectories {
    managers: BTreeMap<u32, SaveDirectoryBinding>,
    tasks: BTreeMap<u32, SaveDirectoryBinding>,
}

/// Only logical constructor fields belong in persistent game state. Host
/// absolute paths are intentionally supplied again by a validated relocation
/// policy; loading a save must never grant write access to a serialized path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SaveTaskDirectorySnapshot {
    pub version: u32,
    pub managers: BTreeMap<u32, Vec<u16>>,
    pub tasks: BTreeMap<u32, Vec<u16>>,
}

pub(crate) struct PreparedSaveTaskDirectories {
    managers: BTreeMap<u32, SaveDirectoryBinding>,
    tasks: BTreeMap<u32, SaveDirectoryBinding>,
}

impl SaveTaskDirectories {
    /// Bootstrap the independent player's existing private save root. The dot
    /// is an explicit host-relative namespace, not a startup/XML default or a
    /// reconstructed task+104 field from an unexamined PE. This path neither
    /// reads native configuration nor invokes system-folder APIs. The host
    /// supplies actual manager/root IDs (which may both be zero because these
    /// are separate stores). Both bindings are prepared before either insert.
    /// Do not call this on every I/O or overwrite a live task's binding.
    pub(crate) fn initialize_private_root(
        &mut self,
        private_root: &Path,
        manager: u32,
        root_task: u32,
    ) -> Result<()> {
        let directory = SaveDirectoryBinding::relocated(&[u16::from(b'.')], private_root)?;
        self.install_root_pair(manager, root_task, directory)
    }

    /// Bind a newly constructed independent-host script (including its kind0
    /// unloaded-task entry) when the task domain has no proven creating-manager
    /// mapping. The supplied root is deliberate host policy, not an I/O-time
    /// fallback for a missing task. Proven manager copies use construct_task.
    pub(crate) fn construct_private_task(&mut self, task: u32, private_root: &Path) -> Result<()> {
        let directory = SaveDirectoryBinding::relocated(&[u16::from(b'.')], private_root)?;
        self.install_task(task, directory)
    }

    /// Fixed manager constructor535580 starts with a literal dot. Startup
    /// command37 replaces that value before creating the root script. Nested
    /// managers do not implicitly inherit their parent's directory.
    pub(crate) fn construct_manager(&mut self, manager: u32, private_root: &Path) -> Result<()> {
        self.install_manager(manager, SaveDirectoryBinding::relocated(&[u16::from(b'.')], private_root)?)
    }

    /// Complete root bootstrap in this module; the host supplies only the
    /// existing router's configuration, platform fields and real task IDs.
    /// Validate before installing either object so failure leaves both maps unchanged.
    pub(crate) fn initialize_yukikoi(
        &mut self,
        pe_sha256: &str,
        startup: Option<&ConfigXml>,
        fields: &SaveFolderBindings<'_>,
        private_root: &Path,
        manager: u32,
        root_task: u32,
    ) -> Result<()> {
        let directory = SaveDirectoryBinding::from_yukikoi_startup(
            pe_sha256, startup, fields, private_root,
        )?;
        self.install_root_pair(manager, root_task, directory)
    }

    fn install_root_pair(
        &mut self, manager: u32, root_task: u32, directory: SaveDirectoryBinding,
    ) -> Result<()> {
        if self.managers.contains_key(&manager) || self.tasks.contains_key(&root_task) {
            bail!("task save-directory root is already initialized");
        }
        self.require_consistent_relocation(&directory)?;
        let task_directory = directory.clone();
        self.managers.insert(manager, directory);
        self.tasks.insert(root_task, task_directory);
        Ok(())
    }

    /// Install the startup command37 result before constructing its scripts.
    pub(crate) fn install_manager(&mut self, manager: u32, directory: SaveDirectoryBinding) -> Result<()> {
        if self.managers.contains_key(&manager) { bail!("save-directory manager is already installed"); }
        self.require_consistent_relocation(&directory)?;
        self.managers.insert(manager, directory);
        Ok(())
    }

    /// Actual manager command37 receives data1=wide-directory, not data0.
    /// Supply a complete explicit relocation; a changed original path does
    /// not silently share another path's private files. Existing tasks retain
    /// their copies. No script VM pointer is retained across dispatch.
    pub(crate) fn replace_manager(&mut self, manager: u32, directory: SaveDirectoryBinding) -> Result<()> {
        self.require_consistent_relocation(&directory)?;
        let current = self.managers.get_mut(&manager)
            .context("save-directory manager is absent")?;
        *current = directory;
        Ok(())
    }

    /// Command38 copies the wide string to data1, including a trailing NUL;
    /// the caller owns validation/writing of the actual output buffer.
    pub(crate) fn manager_directory_terminated(&self, manager: u32) -> Result<Vec<u16>> {
        let mut value = self.managers.get(&manager)
            .context("save-directory manager is absent")?.original.clone();
        value.push(0);
        Ok(value)
    }

    /// Fixed factory kind12. Parent ID and current active directory do not
    /// substitute for the creating manager's buffer. Kind34 has its own copy
    /// field and can use the same binding when its consumer is integrated.
    pub(crate) fn construct_task(&mut self, task: u32, manager: u32) -> Result<()> {
        if self.tasks.contains_key(&task) { bail!("task save-directory binding already exists"); }
        let directory = self.managers.get(&manager)
            .context("creating manager has no save-directory producer")?.clone();
        self.tasks.insert(task, directory);
        Ok(())
    }

    /// Attach a previously validated constructor field during an enclosing
    /// task transaction. The transaction must validate matching task identity;
    /// this does not regenerate a directory from today's manager/configuration.
    pub(crate) fn install_task(&mut self, task: u32, directory: SaveDirectoryBinding) -> Result<()> {
        if self.tasks.contains_key(&task) { bail!("task save-directory binding already exists"); }
        self.require_consistent_relocation(&directory)?;
        self.tasks.insert(task, directory);
        Ok(())
    }

    pub(crate) fn task(&self, task: u32) -> Result<&SaveDirectoryBinding> {
        self.tasks.get(&task).context("current task has no constructed save-directory binding")
    }

    pub(crate) fn resolve(&self, task: u32, expanded_filename: &[u8]) -> Result<ResolvedSaveTaskPath> {
        self.task(task)?.resolve_expanded_filename(expanded_filename)
    }

    /// Call from the real task/manager release consumers. Dropping a manager
    /// does not invalidate directory copies still owned by live scripts.
    pub(crate) fn release_task(&mut self, task: u32) { self.tasks.remove(&task); }
    pub(crate) fn release_manager(&mut self, manager: u32) { self.managers.remove(&manager); }

    pub(crate) fn capture(&self) -> SaveTaskDirectorySnapshot {
        SaveTaskDirectorySnapshot {
            version: 1,
            managers: self.managers.iter().map(|(&id, value)| (id, value.original.clone())).collect(),
            tasks: self.tasks.iter().map(|(&id, value)| (id, value.original.clone())).collect(),
        }
    }

    /// The enclosing task transaction passes its already validated *save-
    /// capable* task/manager IDs. Other graph stores are not serialized here.
    /// Resolve each distinct original directory once through the host's
    /// explicit private mapping. No manager is consulted to reconstruct an
    /// old task, and missing fields do not fall back to current configuration.
    pub(crate) fn prepare(
        saved: &SaveTaskDirectorySnapshot,
        expected_managers: &BTreeSet<u32>,
        expected_tasks: &BTreeSet<u32>,
        mut relocate: impl FnMut(&[u16]) -> Result<PathBuf>,
    ) -> Result<PreparedSaveTaskDirectories> {
        if saved.version != 1 { bail!("unsupported task save-directory snapshot version"); }
        if saved.managers.len() != expected_managers.len()
            || !saved.managers.keys().all(|id| expected_managers.contains(id))
            || saved.tasks.len() != expected_tasks.len()
            || !saved.tasks.keys().all(|id| expected_tasks.contains(id))
        {
            bail!("task save-directory snapshot does not match the prepared task graph");
        }
        for directory in saved.managers.values().chain(saved.tasks.values()) {
            check_directory(directory)?;
            if directory.contains(&0) { bail!("persisted task save directory contains an embedded terminator"); }
        }
        let mut bindings: BTreeMap<Vec<u16>, SaveDirectoryBinding> = BTreeMap::new();
        for directory in saved.managers.values().chain(saved.tasks.values()) {
            if !bindings.contains_key(directory) {
                let root = relocate(directory)?;
                bindings.insert(directory.clone(), SaveDirectoryBinding::relocated(directory, &root)?);
            }
        }
        let bind = |source: &BTreeMap<u32, Vec<u16>>| -> BTreeMap<u32, SaveDirectoryBinding> {
            source.iter().map(|(&id, directory)| (id, bindings[directory].clone())).collect()
        };
        Ok(PreparedSaveTaskDirectories { managers: bind(&saved.managers), tasks: bind(&saved.tasks) })
    }

    /// All validation and allocation precede the enclosing transaction commit.
    pub(crate) fn commit(&mut self, prepared: PreparedSaveTaskDirectories) {
        self.managers = prepared.managers;
        self.tasks = prepared.tasks;
    }

    fn require_consistent_relocation(&self, directory: &SaveDirectoryBinding) -> Result<()> {
        if self.managers.values().chain(self.tasks.values()).any(|current| {
            current.original == directory.original && current.private_root != directory.private_root
        }) {
            bail!("one original save directory has conflicting private-root bindings");
        }
        Ok(())
    }
}

fn nul(input: &[u16]) -> &[u16] {
    &input[..input.iter().position(|&unit| unit == 0).unwrap_or(input.len())]
}

fn check_directory(input: &[u16]) -> Result<()> {
    if input.len() >= DIRECTORY_UNITS { bail!("task save directory exceeds its 259-unit field"); }
    Ok(())
}

fn yukikoi_default_directory(fields: &SaveFolderBindings<'_>) -> Result<Vec<u16>> {
    let mut value = nul(fields.user_application_data
        .context("Yukikoi default save directory requires the platform's APPDATA binding")?).to_vec();
    // Original600b40 removes the first component's trailing backslashes, then
    // inserts exactly one separator between components. No guessed title.
    while value.last() == Some(&u16::from(b'\\')) { value.pop(); }
    value.extend("\\cs2\\".encode_utf16());
    value.extend_from_slice(&[0x3046, 0x3044, 0x3093, 0x3069, 0x307f, 0x308b]);
    check_directory(&value)?;
    Ok(value)
}

fn expand_folder(input: &[u16], fields: &SaveFolderBindings<'_>) -> Result<Vec<u16>> {
    let input = nul(input);
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut seed = None;
    while cursor < input.len() {
        if input[cursor] != u16::from(b'%') {
            output.push(input[cursor]);
            cursor += 1;
        } else {
            let end = input[cursor + 1..].iter().position(|&unit| unit == u16::from(b'%'))
                .map_or(input.len(), |offset| cursor + offset + 2);
            let marker = &input[cursor..end];
            // Original marker scratch is260 UTF16 units; preserved unknown
            // markers must respect that buffer too.
            check_directory(marker)?;
            let value = match String::from_utf16(marker).as_deref() {
                Ok("%USERPROFILE%") => fields.user_application_data
                    .context("save-folder USERPROFILE requires APPDATA, not a guessed home directory")?,
                Ok("%ProgramData%" | "%ALLUSERSPROFILE%") => fields.common_application_data
                    .context("save-folder common APPDATA binding is absent")?,
                Ok("%brand%") => fields.brand.context("save-folder resolved brand field is absent")?,
                Ok("%title%") => fields.title.context("save-folder resolved title field is absent")?,
                Ok("%save%") => {
                    if seed.is_none() { seed = Some(yukikoi_default_directory(fields)?); }
                    seed.as_deref().context("save-folder seed was not produced")?
                }
                Ok("%imglib%") => fields.image_library.context("save-folder initialized image-library field is absent")?,
                _ => marker,
            };
            output.extend_from_slice(nul(value));
            cursor = end;
        }
        check_directory(&output)?;
    }
    Ok(output)
}
