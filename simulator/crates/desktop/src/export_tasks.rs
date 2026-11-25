//! Typed runtime export workers. Finished-thread queries are independent of
//! filesystem success; errors retain the current entry and its write cursor.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use cs2_runtime::export_tree::{ExportEntry, ExportWork};
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{NativeReturn, Vm};

use crate::{LoggingHost, expand_kcs_format, native_state_error, read_u32};

const V401: &str = "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f";
const YUKIKOI: &str = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976";
const CHUNK: usize = 64 * 1024;

#[path = "export_sound.rs"]
mod sound;
pub(crate) use sound::{SoundPrompt as ExportSoundPrompt, SoundQuestion as ExportSoundQuestion};

#[derive(Default)]
pub(crate) struct ExportTaskStore {
    tasks: BTreeMap<u32, ExportTask>,
    sound_prompt: Option<ExportSoundPrompt>,
}

struct ExportTask {
    kind: u32,
    source: Vec<u8>,
    definition: Vec<u8>,
    destination: PathBuf,
    private_root: PathBuf,
    started: bool,
    complete: bool,
    succeeded: bool,
    work: Option<ExportWork>,
    file: Option<File>,
    written: usize,
    sound: Option<sound::SoundInstallation>,
}

impl ExportTaskStore {
    pub(crate) fn supports(index: u32) -> bool {
        matches!(index, 690..=692)
    }
    pub(super) fn register(&mut self, id: u32, kind: u32, private_root: &Path) {
        self.tasks.insert(
            id,
            ExportTask {
                kind,
                source: Vec::new(),
                definition: Vec::new(),
                destination: private_root.join("export"),
                private_root: private_root.to_owned(),
                started: false,
                complete: true,
                succeeded: false,
                work: None,
                file: None,
                written: 0,
                sound: None,
            },
        );
    }
    pub(super) fn release(&mut self, id: u32) {
        self.tasks.remove(&id);
    }

    pub(super) fn can_release(&self, id: u32) -> bool {
        self.tasks
            .get(&id)
            .and_then(|task| task.sound.as_ref())
            .is_none_or(sound::SoundInstallation::can_release)
    }

    /// A platform directory picker/settings adapter may choose another private
    /// export destination before starting the worker.
    pub(crate) fn set_destination(&mut self, id: u32, directory: PathBuf) -> Result<()> {
        let task = self.tasks.get_mut(&id).context("export task is absent")?;
        if task.started {
            bail!("export destination cannot change after its worker starts");
        }
        task.destination = directory;
        Ok(())
    }
    pub(crate) fn result(&self, id: u32) -> Option<(bool, bool)> {
        self.tasks
            .get(&id)
            .map(|task| (task.complete, task.succeeded))
    }

    pub(crate) fn set_sound_prompt(&mut self, prompt: ExportSoundPrompt) {
        self.sound_prompt = Some(prompt);
    }

    pub(crate) fn sound_result(&self, id: u32) -> Option<(bool, bool)> {
        self.tasks
            .get(&id)?
            .sound
            .as_ref()
            .map(sound::SoundInstallation::state)
    }
}

impl LoggingHost<'_> {
    pub(crate) fn export_task_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let expected = match index {
            690 => 8,
            691 => 4,
            692 => 12,
            _ => return Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        };
        if args.len() != expected {
            return Err(native_state_error(
                "export native has an invalid argument layout",
            ));
        }
        let id = read_u32(args, 0)?;
        if !self.data_services.tasks.exports.tasks.contains_key(&id) {
            return Err(native_state_error(
                "export native requires a live type45/type46 receiver",
            ));
        }
        if index == 691 {
            return Ok(NativeReturn::Value(u32::from(
                self.data_services.tasks.exports.tasks[&id].complete,
            )));
        }
        let source = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        if source.is_empty() {
            return Ok(NativeReturn::Continue);
        }
        let definition = if index == 692 {
            Some(expand_kcs_format(
                vm,
                &vm.read_string(read_u32(args, 8)?)?,
                0,
            )?)
        } else {
            None
        };
        self.begin_export_task_with_definition(id, Some(&source), definition.as_deref())
            .map_err(|cause| {
                self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string()))
            })?;
        Ok(NativeReturn::Continue)
    }

    /// Semantic producer shared by ordinary690 and a real FES owned-buffer
    /// sender. The source name remains original script-encoding bytes.
    pub(crate) fn begin_export_task(&mut self, id: u32, source: &[u8]) -> Result<()> {
        self.begin_export_task_with_definition(id, Some(source), None)
    }

    pub(crate) fn begin_export_task_with_definition(
        &mut self,
        id: u32,
        source: Option<&[u8]>,
        definition: Option<&[u8]>,
    ) -> Result<()> {
        let source = source.map(export_name).transpose()?;
        let definition = definition.map(export_name).transpose()?;
        let task = self
            .data_services
            .tasks
            .exports
            .tasks
            .get_mut(&id)
            .context("type45/type46 export worker is not registered")?;
        if let Some(source) = source {
            task.source = source.to_vec();
        }
        if task.kind == 46 {
            task.definition = definition.unwrap_or_default().to_vec();
        } else {
            task.succeeded = false;
        }
        if !task.started {
            task.started = true;
            task.complete = false;
            task.succeeded = false;
        }
        Ok(())
    }

    pub(super) fn update_export_task(&mut self, id: u32, budget: usize) -> cs2_script::Result<()> {
        let prompt = self.data_services.tasks.exports.sound_prompt;
        let Some(mut task) = self.data_services.tasks.exports.tasks.remove(&id) else {
            return Ok(());
        };
        let result = (|| -> Result<()> {
            if !task.started || task.complete {
                return Ok(());
            }
            if task.work.is_none() {
                let name = self.text_encoding.decode(&task.source).replace('\\', "/");
                if name.is_empty() {
                    bail!("export worker has no source resource");
                }
                let bytes = self
                    .resources
                    .read_category(b"export", &name)
                    .context("resolve and read export source resource")?;
                task.work = Some(ExportWork::decode(&bytes)?);
            }
            for _ in 0..budget.min(64) {
                let work = task.work.as_mut().unwrap();
                let Some(entry) = work.next_entry()? else {
                    task.succeeded = true;
                    if task.kind == 46 && !task.definition.is_empty() {
                        if task.sound.is_none() {
                            let mode = sound_mode(self.data_services.native_profile)?;
                            task.sound = Some(sound::SoundInstallation::new(
                                &task.definition,
                                &task.destination,
                                mode,
                                &task.private_root,
                            ));
                        }
                        task.complete = task.sound.as_mut().unwrap().advance(
                            self.resources,
                            self.text_encoding,
                            prompt,
                            budget,
                        )?;
                    } else {
                        task.complete = true;
                    }
                    return Ok(());
                };
                match entry {
                    ExportEntry::Directory { path } => {
                        let path =
                            export_path(&task.destination, &self.text_encoding.decode(path))?;
                        std::fs::create_dir_all(path).context("create exported directory")?;
                        work.acknowledge()?;
                    }
                    ExportEntry::File { path, bytes } => {
                        if task.file.is_none() {
                            let path =
                                export_path(&task.destination, &self.text_encoding.decode(path))?;
                            if let Some(parent) = path.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            task.file = Some(
                                OpenOptions::new()
                                    .create(true)
                                    .write(true)
                                    .truncate(true)
                                    .open(path)
                                    .context("open exported member")?,
                            );
                            task.written = 0;
                        }
                        let remaining = bytes.len() - task.written;
                        let end = task.written + remaining.min(CHUNK);
                        if remaining != 0 {
                            let count = task
                                .file
                                .as_mut()
                                .unwrap()
                                .write(&bytes[task.written..end])
                                .context("write exported member")?;
                            if count == 0 {
                                bail!("export destination accepted zero bytes");
                            }
                            task.written += count;
                        }
                        if task.written == bytes.len() {
                            task.file
                                .as_mut()
                                .unwrap()
                                .flush()
                                .context("flush exported member")?;
                            task.file.take();
                            task.written = 0;
                            work.acknowledge()?;
                        }
                    }
                }
            }
            Ok(())
        })();
        self.data_services.tasks.exports.tasks.insert(id, task);
        result.map_err(|cause| {
            self.task_error(
                id,
                cs2_runtime::RuntimeError::Host(format!("runtime export worker: {cause:#}")),
            )
        })
    }

    pub(crate) fn receive_export_task_command(
        &mut self,
        command: TaskCommand,
        buffers: &mut TaskCommandBuffers<'_>,
        output: &mut u32,
    ) -> cs2_script::Result<Option<TaskCommandOutcome>> {
        let id = command.target as u32;
        if !self.data_services.tasks.exports.tasks.contains_key(&id) {
            return Ok(None);
        }
        let Some(begin) = self.data_services.scene_tool_commands().export_base else {
            if command.code < 450 {
                return Ok(None);
            }
            return Err(native_state_error(
                "export receiver needs a verified registered command layout",
            ));
        };
        if command.code == begin {
            self.begin_export_task_with_definition(id, buffers.data1.as_deref(), buffers.data0)
                .map_err(|cause| {
                    self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string()))
                })?;
        } else if command.code == begin + 1 {
            *output = u32::from(self.data_services.tasks.exports.tasks[&id].complete);
        } else {
            return Ok(None);
        }
        Ok(Some(TaskCommandOutcome {
            done: true,
            propagate: false,
        }))
    }

    pub(super) fn dispatch_export_task_message(
        &mut self,
        id: u32,
        packet: [u32; 4],
        queued: bool,
        output: &mut u32,
        vm: Option<&mut Vm>,
    ) -> cs2_script::Result<Option<TaskCommandOutcome>> {
        let Some(begin) = self.data_services.scene_tool_commands().export_base else {
            return Ok(None);
        };
        if !matches!(packet[1], code if code == begin || code == begin + 1) {
            return Ok(None);
        }
        let command = TaskCommand {
            target: id as i32,
            sender: packet[0] as i32,
            code: packet[1],
            data: [packet[2], packet[3], 0],
        };
        if !queued && let Some(mut buffers) = self.pending_owned_task_buffers(command) {
            return self.route_task_command_buffers(command, &mut buffers, output, 4096);
        }
        let memory = vm.as_deref();
        let bytes = if packet[1] == begin && packet[3] != 0 {
            let mut bytes = memory
                .ok_or_else(|| {
                    native_state_error("export source needs its sender VM or owned bytes")
                })?
                .read_string(packet[3])?;
            bytes.push(0);
            Some(bytes)
        } else {
            None
        };
        let definition = if packet[1] == begin
            && packet[2] != 0
            && self.data_services.tasks.exports.tasks[&id].kind == 46
        {
            let mut bytes = memory
                .ok_or_else(|| {
                    native_state_error("sound export definition needs its sender VM or owned bytes")
                })?
                .read_string(packet[2])?;
            bytes.push(0);
            Some(bytes)
        } else {
            None
        };
        let mut buffers = super::OwnedTaskBuffers::new(definition, bytes)?;
        if queued {
            let found = self.queue_task_command_buffers(command, &buffers, output)?;
            Ok(found.then_some(TaskCommandOutcome {
                done: true,
                propagate: false,
            }))
        } else {
            self.route_task_command_buffers(command, &mut buffers, output, 4096)
        }
    }
}

fn export_name(bytes: &[u8]) -> Result<&[u8]> {
    let bytes = &bytes[..bytes.iter().position(|&v| v == 0).unwrap_or(bytes.len())];
    if bytes.len() >= 260 {
        bail!("export filename exceeds the original buffer");
    }
    Ok(bytes)
}

fn sound_mode(
    profile: Option<&cs2_runtime::native_profile::FeatureProfile>,
) -> Result<sound::RegistryMode> {
    #[cfg(target_os = "android")]
    {
        // The private application library has no Windows A/W byte-count ABI.
        let _ = profile;
        return Ok(sound::RegistryMode::Narrow);
    }
    #[cfg(not(target_os = "android"))]
    match profile.map(|profile| profile.pe_sha256) {
        Some(V401) => Ok(sound::RegistryMode::Narrow),
        Some(YUKIKOI) => Ok(sound::RegistryMode::WideCharacterCount),
        _ => bail!("sound-export registry ABI needs an identified PE"),
    }
}

fn export_path(root: &Path, name: &str) -> Result<PathBuf> {
    let name = name.replace('\\', "/");
    let path = Path::new(&name);
    if name.contains(':')
        || path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        bail!("export member is outside its destination directory");
    }
    Ok(root.join(path))
}
