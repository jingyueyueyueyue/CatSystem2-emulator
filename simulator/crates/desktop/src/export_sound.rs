//! Type46 post-export sound-scheme installation, with retained decisions and
//! per-registry-write cursors. No platform operation occurs during construction.
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use cs2_runtime::ResourceIndex;
use cs2_runtime::export_sound::{ExportSoundEncoding, ExportSoundFile, ExportSoundScheme};

use crate::ScriptTextEncoding;

#[path = "export_sound_windows.rs"]
mod platform;
#[path = "export_sound_private.rs"]
mod private;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SoundQuestion {
    Register,
    Activate,
}

pub(crate) type SoundPrompt = fn(SoundQuestion) -> Result<bool>;

#[derive(Clone, Copy)]
pub(super) enum RegistryMode {
    Narrow,
    // This PE passes lstrlenW+1 directly as RegSetValueExW's byte count.
    // Keep that receiver behavior rather than silently repairing its data.
    WideCharacterCount,
}

pub(super) struct RegistryOperation {
    pub(super) key: String,
    pub(super) value: String,
    pub(super) create: bool,
}

#[derive(Clone)]
pub(super) struct PreparedScheme {
    pub(super) theme: String,
    events: Vec<(String, Option<String>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Definition,
    AskRegister,
    Register,
    AskActivate,
    Activate,
    Registered,
    Activated,
    Done,
}

pub(super) struct SoundInstallation {
    definition: Vec<u8>,
    phase: Phase,
    scheme: Option<PreparedScheme>,
    plan: Option<Vec<RegistryOperation>>,
    cursor: usize,
    mode: RegistryMode,
    registered: bool,
    activated: bool,
    destination: PathBuf,
    ui: Option<JoinHandle<Result<bool>>>,
    ui_error: Option<String>,
    discovery: Option<JoinHandle<Result<Vec<RegistryOperation>>>>,
    private_root: PathBuf,
}

impl SoundInstallation {
    pub(super) fn new(
        definition: &[u8],
        destination: &Path,
        mode: RegistryMode,
        private_root: &Path,
    ) -> Self {
        Self {
            definition: definition.to_vec(),
            phase: Phase::AskRegister,
            scheme: None,
            plan: None,
            cursor: 0,
            mode,
            registered: false,
            activated: false,
            destination: destination.to_owned(),
            ui: None,
            ui_error: None,
            discovery: None,
            private_root: private_root.to_owned(),
        }
    }

    pub(super) fn state(&self) -> (bool, bool) {
        (self.registered, self.activated)
    }

    /// Destruction waits for its already visible dialog without blocking the
    /// scheduler or applying an unanswered registration after task deletion.
    pub(super) fn can_release(&self) -> bool {
        self.ui.as_ref().is_none_or(JoinHandle::is_finished)
            && self.discovery.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn poll_ui(&mut self, action: UiAction, prompt: Option<SoundPrompt>) -> Result<Option<bool>> {
        // Android installs a private application theme and has no system-wide
        // sound-scheme consent prompt. An installed app UI adapter can opt out.
        #[cfg(target_os = "android")]
        if matches!(action, UiAction::Notice(_)) || prompt.is_none() {
            return Ok(Some(true));
        }
        if let Some(error) = &self.ui_error {
            bail!("{error}");
        }
        if self.ui.is_none() {
            self.ui = Some(
                std::thread::Builder::new()
                    .name("cs2-export-sound-ui".into())
                    .spawn(move || match action {
                        UiAction::Question(question) => {
                            prompt.unwrap_or(platform::confirm)(question)
                        }
                        UiAction::Notice(activated) => platform::notify(activated).map(|_| true),
                    })
                    .context("start sound-set dialog worker")?,
            );
            return Ok(None);
        }
        if !self.ui.as_ref().unwrap().is_finished() {
            return Ok(None);
        }
        let result = self
            .ui
            .take()
            .unwrap()
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("sound-set dialog worker panicked")));
        match result {
            Ok(answer) => Ok(Some(answer)),
            Err(error) => {
                self.ui_error = Some(format!("sound-set dialog: {error:#}"));
                bail!("{}", self.ui_error.as_deref().unwrap());
            }
        }
    }

    pub(super) fn advance(
        &mut self,
        resources: &ResourceIndex,
        encoding: ScriptTextEncoding,
        prompt: Option<SoundPrompt>,
        budget: usize,
    ) -> Result<bool> {
        for _ in 0..budget.min(64) {
            match self.phase {
                Phase::Definition => {
                    let name = encoding.decode(&self.definition).replace('\\', "/");
                    let bytes = resources
                        .read_category(b"export", &name)
                        .context("resolve and read sound-set definition resource")?;
                    let definition = ExportSoundScheme::decode(&bytes)?;
                    self.scheme = Some(prepare(&definition, encoding, &self.destination)?);
                    self.phase = Phase::Register;
                }
                Phase::AskRegister => {
                    let Some(yes) =
                        self.poll_ui(UiAction::Question(SoundQuestion::Register), prompt)?
                    else {
                        return Ok(false);
                    };
                    self.phase = if yes { Phase::Definition } else { Phase::Done };
                }
                Phase::Register | Phase::Activate => {
                    let scheme = self
                        .scheme
                        .as_ref()
                        .context("sound scheme was not prepared")?;
                    #[cfg(target_os = "android")]
                    {
                        if self.phase == Phase::Register {
                            private::register(&self.private_root, scheme)?;
                            self.registered = true;
                            self.phase = Phase::AskActivate;
                        } else {
                            private::select(&self.private_root, &scheme.theme)?;
                            self.activated = true;
                            self.phase = Phase::Activated;
                        }
                        continue;
                    }
                    #[cfg(not(target_os = "android"))]
                    {
                        if self.plan.is_none() {
                            if self.discovery.is_none() {
                                let scheme = scheme.clone();
                                let mode = self.mode;
                                let register = self.phase == Phase::Register;
                                self.discovery = Some(
                                    std::thread::Builder::new()
                                        .name("cs2-export-sound-registry".into())
                                        .spawn(move || {
                                            if register {
                                                platform::registration_plan(&scheme, mode)
                                            } else {
                                                platform::activation_plan(&scheme, mode)
                                            }
                                        })
                                        .context("start sound-set registry reader")?,
                                );
                                return Ok(false);
                            }
                            if !self.discovery.as_ref().unwrap().is_finished() {
                                return Ok(false);
                            }
                            self.plan =
                                Some(self.discovery.take().unwrap().join().unwrap_or_else(
                                    |_| Err(anyhow::anyhow!("sound-set registry reader panicked")),
                                )?);
                            self.cursor = 0;
                        }
                        let plan = self.plan.as_ref().unwrap();
                        if let Some(operation) = plan.get(self.cursor) {
                            platform::write(operation, self.mode)?;
                            self.cursor += 1;
                            continue;
                        }
                        self.plan = None;
                        self.cursor = 0;
                        if self.phase == Phase::Register {
                            self.registered = true;
                            self.phase = Phase::AskActivate;
                        } else {
                            self.activated = true;
                            self.phase = Phase::Activated;
                        }
                    }
                }
                Phase::AskActivate => {
                    let Some(yes) =
                        self.poll_ui(UiAction::Question(SoundQuestion::Activate), prompt)?
                    else {
                        return Ok(false);
                    };
                    self.phase = if yes {
                        Phase::Activate
                    } else {
                        Phase::Registered
                    };
                }
                Phase::Registered | Phase::Activated => {
                    if self
                        .poll_ui(UiAction::Notice(self.phase == Phase::Activated), prompt)?
                        .is_none()
                    {
                        return Ok(false);
                    }
                    self.phase = Phase::Done;
                }
                Phase::Done => return Ok(true),
            }
        }
        Ok(self.phase == Phase::Done)
    }
}

impl crate::HeadlessPlayer {
    /// Android shell lifecycle/UI events use explicit event names from the
    /// imported definition. None means absent/muted, not a device success.
    pub fn play_application_sound_event(&mut self, event: &str) -> Result<Option<u64>> {
        private::play(&self.save_root, event, &mut self.audio_host)
    }

    pub fn application_sound_schemes(&self) -> Result<(Option<String>, Vec<String>)> {
        let library = private::load(&self.save_root)?;
        Ok((
            library.selected().map(str::to_owned),
            library.themes().map(str::to_owned).collect(),
        ))
    }

    pub fn select_application_sound_scheme(&mut self, theme: &str) -> Result<()> {
        private::select(&self.save_root, theme)
    }
}

#[derive(Clone, Copy)]
enum UiAction {
    Question(SoundQuestion),
    Notice(bool),
}

fn prepare(
    definition: &ExportSoundScheme,
    encoding: ScriptTextEncoding,
    destination: &Path,
) -> Result<PreparedScheme> {
    let encoding = match definition.encoding {
        ExportSoundEncoding::Script => encoding,
        ExportSoundEncoding::Cp932 => ScriptTextEncoding::Cp932,
        ExportSoundEncoding::Gbk => ScriptTextEncoding::Gbk,
        ExportSoundEncoding::Utf8 => ScriptTextEncoding::Utf8,
    };
    let theme = encoding.decode(&definition.theme);
    component(&theme)?;
    let mut events = Vec::new();
    for event in &definition.events {
        let name = encoding.decode(&event.name);
        component(&name)?;
        let file = match &event.file {
            ExportSoundFile::KeepCurrent => None,
            ExportSoundFile::Mute => Some(String::new()),
            ExportSoundFile::File(file) => {
                let file = encoding.decode(file);
                let path = super::export_path(destination, &file)?;
                // Registry sound filenames need an absolute path independent
                // of whichever working directory the user later launches.
                let path = std::path::absolute(path)?;
                Some(path.to_string_lossy().into_owned())
            }
        };
        events.push((name, file));
    }
    Ok(PreparedScheme { theme, events })
}

fn component(value: &str) -> Result<()> {
    if value.is_empty() || value.contains(['\\', '/', '\0']) || value.encode_utf16().count() >= 256
    {
        bail!("sound-set registry name is empty, too long or contains a separator");
    }
    Ok(())
}

pub(super) fn append_overrides(plan: &mut Vec<RegistryOperation>, scheme: &PreparedScheme) {
    for (event, file) in &scheme.events {
        if let Some(file) = file {
            plan.push(RegistryOperation {
                key: format!(
                    "AppEvents\\Schemes\\Apps\\.Default\\{event}\\{}",
                    scheme.theme
                ),
                value: file.clone(),
                create: false,
            });
        }
    }
}
