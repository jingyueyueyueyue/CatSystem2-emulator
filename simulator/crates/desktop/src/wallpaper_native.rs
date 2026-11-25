//! Ordinary600/601 and the type44 wallpaper worker. The old two-DWORD
//! producer opens one file; the newer three-DWORD producer selects a member
//! from the export package. Neither is a sequence/string-table operation.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use cs2_runtime::export_tree::{ExportEntry, ExportWork};
use cs2_script::{NativeReturn, Vm};
use super::{HeadlessPlayer, LoggingHost, ScriptTextEncoding, expand_kcs_format,
    native_state_error, read_u32, trim_nul};

#[path = "wallpaper_windows.rs"]
mod platform;

#[derive(Clone, Debug)]
pub struct WallpaperExportRequest {
    pub task_id: u32,
    pub suggested_path: PathBuf,
    pub byte_length: usize,
}

#[derive(Clone, Debug)]
pub struct WallpaperExportSelection {
    pub path: PathBuf,
    /// None only exports. -1 keeps current placement; 1 centers, 2 tiles,
    /// 3 stretches, following the original wallpaper helper's registry data.
    pub placement: Option<i32>,
}

/// Return None for a user cancellation, never for an unavailable adapter.
pub type WallpaperExportPrompt =
    fn(&WallpaperExportRequest) -> Result<Option<WallpaperExportSelection>>;
pub type WallpaperApplyHandler = fn(&std::path::Path, i32) -> Result<()>;

#[derive(Default)]
pub(super) struct WallpaperStore {
    tasks: BTreeMap<u32, WallpaperTask>,
    prompt: Option<WallpaperExportPrompt>,
    apply: Option<WallpaperApplyHandler>,
}

struct WallpaperTask {
    source: Vec<u8>,
    member: Option<Vec<u8>>,
    encoding: ScriptTextEncoding,
    started: bool,
    complete: bool,
    bytes: Option<Vec<u8>>,
    prompt: Option<JoinHandle<Result<Option<WallpaperExportSelection>>>>,
    prompt_failure: Option<String>,
    selection: Option<WallpaperExportSelection>,
    file: Option<File>,
    written: usize,
}

impl WallpaperStore {
    pub(super) fn register(&mut self, id: u32) {
        self.tasks.insert(id, WallpaperTask {
            source: Vec::new(), member: None, encoding: ScriptTextEncoding::Cp932,
            started: false, complete: true, bytes: None, prompt: None, prompt_failure: None,
            selection: None, file: None, written: 0,
        });
    }
    pub(super) fn can_release(&self, id: u32) -> bool {
        self.tasks.get(&id).and_then(|task| task.prompt.as_ref())
            .is_none_or(JoinHandle::is_finished)
    }
    pub(super) fn release(&mut self, id: u32) { self.tasks.remove(&id); }
}

impl HeadlessPlayer {
    pub fn set_wallpaper_export_prompt(&mut self, prompt: WallpaperExportPrompt) {
        self.data_services.wallpapers.prompt = Some(prompt);
        for task in self.data_services.wallpapers.tasks.values_mut() { task.prompt_failure = None; }
    }
    pub fn set_wallpaper_apply_handler(&mut self, handler: WallpaperApplyHandler) {
        self.data_services.wallpapers.apply = Some(handler);
    }
}

impl LoggingHost<'_> {
    pub(super) fn wallpaper_native(
        &mut self, index: u32, args: &[u8], vm: &Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let id = read_u32(args, 0)?;
        let invocation = if index == 600 {
            if !matches!(args.len(), 8 | 12) {
                return Err(native_state_error("native 600 requires the legacy two- or package three-DWORD ABI"));
            }
            if args.len() == 8 && self.data_services.ordinary_native_table.as_ref()
                .is_some_and(cs2_runtime::native_table::OrdinaryNativeTable::has_wallpaper_package_wrapper)
            {
                return Err(native_state_error("identified native 600 wrapper requires the package ABI"));
            }
            let source = formatted(vm, read_u32(args, 4)?)?;
            let member = if args.len() == 12 { Some(formatted(vm, read_u32(args, 8)?)?) } else { None };
            Some((source, member))
        } else {
            if args.len() != 4 { return Err(native_state_error("native 601 requires one task DWORD")); }
            None
        };
        if !self.task_is_reachable(id) {
            return Ok(if index == 601 { NativeReturn::Value(0) } else { NativeReturn::Continue });
        }
        let Some(kind) = self.data_services.tasks.message_task_state(id).map(|state| state.0)
            .or_else(|| self.scene_nodes.get(&id).map(|node| node.kind))
        else { return Err(native_state_error("wallpaper receiver has no task identity")); };
        if kind != 44 {
            return Err(native_state_error("native 600/601 requires its type44 wallpaper receiver"));
        }
        if index == 601 {
            let task = self.data_services.wallpapers.tasks.get(&id)
                .ok_or_else(|| native_state_error("wallpaper receiver state is unavailable"))?;
            return Ok(NativeReturn::Value(u32::from(task.complete)));
        }
        let (source, member) = invocation.unwrap();
        let task = self.data_services.wallpapers.tasks.get_mut(&id)
            .ok_or_else(|| native_state_error("wallpaper receiver state is unavailable"))?;
        task.source = source;
        task.member = member;
        task.encoding = self.text_encoding;
        // The original keeps its thread object after completion. Later starts
        // update the supplied fields but cannot start a second worker.
        if !task.started { task.started = true; task.complete = false; }
        Ok(NativeReturn::Continue)
    }

    pub(super) fn update_wallpaper_task(&mut self, id: u32) -> cs2_script::Result<()> {
        let Some(mut task) = self.data_services.wallpapers.tasks.remove(&id) else { return Ok(()); };
        let result = self.update_wallpaper_inner(id, &mut task);
        self.data_services.wallpapers.tasks.insert(id, task);
        result.map_err(|cause| self.task_error(id,
            cs2_runtime::RuntimeError::Host(format!("wallpaper worker: {cause:#}"))))
    }

    fn update_wallpaper_inner(&mut self, id: u32, task: &mut WallpaperTask) -> Result<()> {
        if !task.started || task.complete { return Ok(()); }
        if let Some(failure) = &task.prompt_failure { bail!("{failure}"); }
        if task.bytes.is_none() {
            let source = task.encoding.decode(&task.source).replace('\\', "/");
            let names = if task.member.is_some() {
                self.resources.category_names(b"export", &source)?
            } else { vec![source] };
            let mut input = None;
            for name in names {
                if let Some(bytes) = self.data_services.ordinary.cached_resource(&name) {
                    input = Some(bytes.to_vec()); break;
                }
                let mut candidates = self.resources.candidates(&name).iter()
                    .filter(|candidate| matches!(&candidate.location, cs2_runtime::ResourceLocation::File { .. }))
                    .collect::<Vec<_>>();
                if let Some(preferred) = self.resources.find(&name) {
                    if !candidates.iter().any(|item| std::ptr::eq(*item, preferred)) { candidates.push(preferred); }
                }
                for candidate in self.resources.candidates(&name) {
                    if !candidates.iter().any(|item| std::ptr::eq(*item, candidate)) { candidates.push(candidate); }
                }
                for candidate in candidates {
                    if candidate.size > 64 * 1024 * 1024 { bail!("wallpaper source exceeds 64 MiB"); }
                    match self.resources.read_candidate(candidate) {
                        Ok(bytes) => { input = Some(bytes); break; }
                        Err(cs2_runtime::RuntimeError::ResourceNotFound(_)) => continue,
                        Err(cs2_runtime::RuntimeError::Io { source, .. })
                            if source.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(cause) => return Err(cause.into()),
                    }
                }
                if input.is_some() { break; }
            }
            let bytes = input.context("wallpaper source resource is missing")?;
            if bytes.len() > 64 * 1024 * 1024 { bail!("wallpaper source exceeds 64 MiB"); }
            task.bytes = Some(if let Some(member) = &task.member {
                let wanted = normalize_member(member);
                let mut work = ExportWork::decode(&bytes)?;
                let mut selected = None;
                while let Some(entry) = work.next_entry()? {
                    if let ExportEntry::File { path, bytes } = entry {
                        if normalize_member(path).eq_ignore_ascii_case(&wanted) {
                            selected = Some(bytes.clone()); break;
                        }
                    }
                    work.acknowledge()?;
                }
                selected.context("wallpaper package does not contain its requested member")?
            } else { bytes });
        }
        if task.selection.is_none() {
            if task.prompt.is_none() {
                let name = task.member.as_deref().unwrap_or(&task.source);
                let name = task.encoding.decode(name).replace('\\', "/");
                let name = name.rsplit('/').next().filter(|name| !name.is_empty()).unwrap_or("wallpaper.bmp");
                let request = WallpaperExportRequest { task_id: id,
                    suggested_path: self.save_root.join("export").join(name),
                    byte_length: task.bytes.as_ref().unwrap().len() };
                let prompt = self.data_services.wallpapers.prompt.unwrap_or(platform::select);
                task.prompt = Some(std::thread::spawn(move || prompt(&request)));
                return Ok(());
            }
            if !task.prompt.as_ref().unwrap().is_finished() { return Ok(()); }
            let selected = match task.prompt.take().unwrap().join() {
                Ok(Ok(selected)) => selected,
                result => {
                    let reason = match result {
                        Err(_) => "wallpaper prompt thread panicked".to_owned(),
                        Ok(Err(cause)) => cause.to_string(),
                        Ok(Ok(_)) => unreachable!(),
                    };
                    task.prompt_failure = Some(reason.clone());
                    bail!("{reason}");
                }
            };
            let Some(selection) = selected else { task.complete = true; return Ok(()); };
            if selection.placement.is_some_and(|mode| !matches!(mode, -1 | 1 | 2 | 3)) {
                bail!("wallpaper placement is outside the original supported values");
            }
            task.selection = Some(selection);
        }
        let selection = task.selection.as_ref().unwrap();
        if task.file.is_none() {
            if let Some(parent) = selection.path.parent() { std::fs::create_dir_all(parent)?; }
            task.file = Some(OpenOptions::new().write(true).create(true).truncate(true)
                .open(&selection.path).context("open selected wallpaper export")?);
            task.written = 0;
        }
        let bytes = task.bytes.as_ref().unwrap();
        let end = task.written.saturating_add(64 * 1024).min(bytes.len());
        if end != task.written {
            let written = task.file.as_mut().unwrap().write(&bytes[task.written..end])?;
            if written == 0 { bail!("wallpaper export accepted zero output bytes"); }
            task.written += written;
        }
        if task.written == bytes.len() {
            task.file.as_mut().unwrap().flush()?;
            if let Some(mode) = selection.placement {
                self.data_services.wallpapers.apply.unwrap_or(platform::apply)(&selection.path, mode)?;
            }
            task.file.take();
            task.complete = true;
        }
        Ok(())
    }
}

fn normalize_member(name: &[u8]) -> Vec<u8> {
    // Preserve CP932 trail backslashes; only standalone ASCII separators fold.
    let mut result = Vec::with_capacity(name.len());
    let mut cursor = 0;
    while cursor < name.len() {
        let byte = name[cursor];
        result.push(if byte == b'\\' { b'/' } else { byte });
        cursor += 1;
        if matches!(byte, 0x81..=0x9f | 0xe0..=0xfc) && cursor < name.len() {
            result.push(name[cursor]); cursor += 1;
        }
    }
    result
}

fn formatted(vm: &Vm, address: u32) -> cs2_script::Result<Vec<u8>> {
    let value = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    let value = trim_nul(&value);
    if value.len() >= 260 { return Err(native_state_error("native 600 name exceeds 259 bytes")); }
    Ok(value.to_vec())
}
