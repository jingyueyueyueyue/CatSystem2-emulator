use crate::{RuntimeError, RuntimeResult};
use cs2_script::{
    CstEvent, CstTextToken, CstlLocalization, CstlTable, FesEvent, FesKeyBlockBinding,
    FesKeyBlockTable, FesObjectDefinition, FesObjectInitialization, FesSection, FesSubcommand,
    FesSubcommandArgument, FesSubcommandTable, MessageLayout, MessageLayoutSettings, MessageReveal,
    MessageRevealUpdate, MessageSchedule, MessageTimingSettings, ScriptError,
};

const MAX_OBJECT_BANKS: usize = 1_000_000;
const MAX_KEY_BLOCKS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub enum FesObjectKind {
    Base,
    Draw,
    CImage,
    Image,
    DifImage,
    CPlane,
    Plane,
    Button,
    Slider,
    Scroll2,
    String,
    String8,
    LogString,
    LogString8,
    Sound,
    SoundTable,
    SavePanel,
    BackgroundTable2,
    CharacterTable3,
    Particle,
    ImageWindow,
    Viewport,
    Wipe,
    NovelString,
    NovelString8,
}

impl FesObjectKind {
    fn parse(name: &[u8]) -> Option<Self> {
        if name.eq_ignore_ascii_case(b"BASE") {
            Some(Self::Base)
        } else if name.eq_ignore_ascii_case(b"DRAW") {
            Some(Self::Draw)
        } else if name.eq_ignore_ascii_case(b"WIPE") {
            Some(Self::Wipe)
        } else if name.eq_ignore_ascii_case(b"CIMAGE") {
            Some(Self::CImage)
        } else if name.eq_ignore_ascii_case(b"IMAGE") {
            Some(Self::Image)
        } else if name.eq_ignore_ascii_case(b"DIFIMAGE") {
            Some(Self::DifImage)
        } else if name.eq_ignore_ascii_case(b"CPLANE") {
            Some(Self::CPlane)
        } else if name.eq_ignore_ascii_case(b"PLANE") {
            Some(Self::Plane)
        } else if name.eq_ignore_ascii_case(b"BUTTON") {
            Some(Self::Button)
        } else if name.eq_ignore_ascii_case(b"SLIDER") {
            Some(Self::Slider)
        } else if name.eq_ignore_ascii_case(b"SCROLL2") {
            Some(Self::Scroll2)
        } else if name.eq_ignore_ascii_case(b"STRING") {
            Some(Self::String)
        } else if name.eq_ignore_ascii_case(b"STRING8") {
            Some(Self::String8)
        } else if name.eq_ignore_ascii_case(b"LOGSTRING") {
            Some(Self::LogString)
        } else if name.eq_ignore_ascii_case(b"LOGSTRING8") {
            Some(Self::LogString8)
        } else if name.eq_ignore_ascii_case(b"NVSTRING") {
            Some(Self::NovelString)
        } else if name.eq_ignore_ascii_case(b"NVSTRING8") {
            Some(Self::NovelString8)
        } else if name.eq_ignore_ascii_case(b"SOUND") {
            Some(Self::Sound)
        } else if name.eq_ignore_ascii_case(b"SNDTABLE") {
            Some(Self::SoundTable)
        } else if name.eq_ignore_ascii_case(b"SAVEPANEL") {
            Some(Self::SavePanel)
        } else if name.eq_ignore_ascii_case(b"BGTABLE2") {
            Some(Self::BackgroundTable2)
        } else if name.eq_ignore_ascii_case(b"CHTABLE3") {
            Some(Self::CharacterTable3)
        } else if name.eq_ignore_ascii_case(b"PARTICLE") {
            Some(Self::Particle)
        } else if name.eq_ignore_ascii_case(b"IMGWND") {
            Some(Self::ImageWindow)
        } else if name.eq_ignore_ascii_case(b"VIEWPORT") {
            Some(Self::Viewport)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesObjectDefinitionState {
    pub kind: FesObjectKind,
    pub kind_name: Vec<u8>,
    pub name: Vec<u8>,
    pub bank_expression: Option<Vec<u8>>,
    pub requested_banks: i32,
    pub bank_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesObjectProperty {
    pub column: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesObjectBankState {
    pub index: usize,
    pub initializers: Vec<FesObjectProperty>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesObjectState {
    pub definition: FesObjectDefinitionState,
    pub banks: Vec<FesObjectBankState>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesSubcommandInvocation {
    pub mode: FesSubcommandMode,
    pub name: Vec<u8>,
    pub target: Vec<u8>,
    pub parameters: Vec<Vec<u8>>,
    pub arguments: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub enum FesSubcommandMode {
    OneShot,
    Continue,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub enum FesRuntimeStatus {
    Ready,
    Running,
    WaitingFrames {
        wake_frame: u64,
    },
    WaitingControl {
        kind: Vec<u8>,
        arguments: Vec<Vec<u8>>,
    },
    FrameBoundary,
    CommandPending,
    Exited(i32),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub enum FesDispatch {
    Declarations {
        include_count: usize,
        preload_count: usize,
    },
    KeyBlocks {
        count: usize,
    },
    ObjectCreated {
        name: Vec<u8>,
        banks: usize,
    },
    ObjectInitialized {
        name: Vec<u8>,
        banks: usize,
        properties: usize,
    },
    KeyBlockBound {
        target: Vec<u8>,
        block: i32,
    },
    Subcommands {
        count: usize,
    },
    ControlEvent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageRevealProgress {
    pub newly_visible: Vec<usize>,
    pub complete: bool,
}

impl From<MessageRevealUpdate> for MessageRevealProgress {
    fn from(update: MessageRevealUpdate) -> Self {
        Self {
            newly_visible: update.newly_visible,
            complete: update.complete,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessagePresentation {
    pub speaker: bool,
    pub source_text: Vec<u8>,
    pub language: Option<String>,
    pub tokens: Vec<CstTextToken>,
    pub layout: MessageLayout,
    pub schedule: MessageSchedule,
    reveal: MessageReveal,
}

impl MessagePresentation {
    pub fn visible_glyphs(&self) -> Vec<bool> {
        (0..self.layout.entries.len())
            .map(|index| self.reveal.is_visible(index).unwrap_or(false))
            .collect()
    }

    pub fn remaining_glyphs(&self) -> usize {
        self.reveal.remaining()
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesRuntimeAdapter {
    pub status: FesRuntimeStatus,
    pub declarations: Vec<FesSection>,
    pub include_requests: Vec<Vec<u8>>,
    pub preload_requests: Vec<Vec<u8>>,
    pub definitions: Vec<FesObjectDefinitionState>,
    pub initializers: Vec<FesObjectInitialization>,
    pub objects: Vec<FesObjectState>,
    pub keyblocks: Option<FesKeyBlockTable>,
    pub keyblock_bindings: Vec<FesKeyBlockBinding>,
    pub subcommands: Option<FesSubcommandTable>,
    pub last_command: Option<Vec<u8>>,
    pub last_command_parameters: Option<Vec<Vec<u8>>>,
    #[serde(skip)]
    pub message: Option<MessagePresentation>,
    #[serde(skip)]
    localization: Option<CstlLocalization>,
    // Repeated declarations still allocate objects, but each name/bank keeps
    // its first binding. Later declarations can add previously absent banks.
    object_bindings: std::collections::BTreeMap<(Vec<u8>, usize), (usize, usize)>,
}

impl Default for FesRuntimeStatus {
    fn default() -> Self {
        Self::Ready
    }
}

impl FesRuntimeAdapter {
    /// FES checkpoints do not silently discard a CST presentation or locale.
    pub fn validate_fes_checkpoint(&self) -> RuntimeResult<()> {
        if self.message.is_some() || self.localization.is_some()
            || self.objects.len() > MAX_OBJECT_BANKS
            || self.subcommands.as_ref().is_some_and(|table| !table.diagnostics.is_empty())
        {
            return Err(RuntimeError::InvalidSave("unsupported or malformed FES adapter checkpoint".into()));
        }
        for ((name, bank), &(object, slot)) in &self.object_bindings {
            let state = self.objects.get(object)
                .ok_or_else(|| RuntimeError::InvalidSave("FES checkpoint binding object is absent".into()))?;
            if state.definition.name != *name || state.banks.get(slot).is_none_or(|state| state.index != *bank) {
                return Err(RuntimeError::InvalidSave("FES checkpoint binding bank differs".into()));
            }
        }
        Ok(())
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn dispatch(&mut self, event: FesEvent) -> RuntimeResult<FesDispatch> {
        if !matches!(
            self.status,
            FesRuntimeStatus::Ready | FesRuntimeStatus::Running
        ) {
            return Err(scene_error(
                "FES dispatch",
                None,
                format!(
                    "cannot dispatch a new event while adapter status is {:?}",
                    self.status
                ),
            ));
        }
        match event {
            FesEvent::Declarations(sections) => {
                let result = self.dispatch_declarations(sections)?;
                self.status = FesRuntimeStatus::Running;
                Ok(result)
            }
            FesEvent::KeyBlocks(table) => {
                let result = self.dispatch_keyblocks(table)?;
                self.status = FesRuntimeStatus::Running;
                Ok(result)
            }
            FesEvent::CreateObject(definition) => self.dispatch_create_object(definition),
            FesEvent::InitializeObject(initializer) => self.dispatch_initialize_object(initializer),
            FesEvent::KeyBlockBinding(binding) => {
                let result = self.dispatch_keyblock_binding(binding)?;
                self.status = FesRuntimeStatus::Running;
                Ok(result)
            }
            FesEvent::Subcommands(table) => self.dispatch_subcommands(table),
            FesEvent::Command(line) => {
                self.last_command = Some(line.bytes);
                self.last_command_parameters = None;
                self.status = FesRuntimeStatus::CommandPending;
                Ok(FesDispatch::ControlEvent)
            }
            FesEvent::Wait(frames) => {
                let frames = frames.max(1) as u64;
                self.status = FesRuntimeStatus::WaitingFrames { wake_frame: frames };
                Ok(FesDispatch::ControlEvent)
            }
            FesEvent::WaitControl { kind, arguments } => {
                self.status = FesRuntimeStatus::WaitingControl { kind, arguments };
                Ok(FesDispatch::ControlEvent)
            }
            FesEvent::WaitFrame => {
                self.status = FesRuntimeStatus::WaitingFrames { wake_frame: 1 };
                Ok(FesDispatch::ControlEvent)
            }
            FesEvent::Frame => {
                self.status = FesRuntimeStatus::FrameBoundary;
                Ok(FesDispatch::ControlEvent)
            }
            FesEvent::Exit(code) => {
                self.status = FesRuntimeStatus::Exited(code);
                Ok(FesDispatch::ControlEvent)
            }
        }
    }

    /// Preserve parameter boundaries instead of splitting an expanded string
    /// for a second time in the host.
    pub fn dispatch_pending(
        &mut self,
        interpreter: &cs2_script::FesInterpreter,
    ) -> RuntimeResult<FesDispatch> {
        let event = interpreter.pending().cloned().ok_or_else(|| {
            scene_error("FES dispatch", None, "interpreter has no pending event".to_string())
        })?;
        let parameters = interpreter.pending_command_parameters().map(|parameters| parameters.to_vec());
        let result = self.dispatch(event)?;
        self.last_command_parameters = parameters;
        Ok(result)
    }

    /// Consume one host frame while waiting.
    ///
    /// FES wait events carry durations; the host owns the frame clock and
    /// calls this once for each elapsed frame. Returns true when runnable.
    pub fn advance_wait_frame(&mut self) -> bool {
        let remaining = match &mut self.status {
            FesRuntimeStatus::WaitingFrames { wake_frame } => wake_frame,
            _ => {
                return matches!(
                    &self.status,
                    FesRuntimeStatus::Ready | FesRuntimeStatus::Running
                );
            }
        };
        if *remaining <= 1 {
            self.status = FesRuntimeStatus::Running;
            true
        } else {
            *remaining -= 1;
            false
        }
    }

    /// Resume a named control wait. An empty kind resumes any control wait.
    pub fn resume_control(&mut self, kind: &[u8]) -> bool {
        let matches = match &self.status {
            FesRuntimeStatus::WaitingControl {
                kind: waiting_kind, ..
            } => kind.is_empty() || waiting_kind.as_slice() == kind,
            _ => false,
        };
        if matches {
            self.status = FesRuntimeStatus::Running;
        }
        matches
    }

    /// Resume after the host has completed work at the current frame boundary.
    pub fn resume_frame_boundary(&mut self) -> bool {
        if self.status == FesRuntimeStatus::FrameBoundary {
            self.status = FesRuntimeStatus::Running;
            true
        } else {
            false
        }
    }

    /// Take the most recent command event and make the adapter runnable.
    pub fn take_command(&mut self) -> Option<Vec<u8>> {
        self.last_command_parameters = None;
        let command = self.last_command.take();
        if command.is_some() && self.status == FesRuntimeStatus::CommandPending {
            self.status = FesRuntimeStatus::Running;
        }
        command
    }

    pub fn take_command_with_parameters(&mut self) -> Option<(Vec<u8>, Option<Vec<Vec<u8>>>)> {
        let parameters = self.last_command_parameters.take();
        self.take_command().map(|command| (command, parameters))
    }

    fn dispatch_declarations(&mut self, sections: Vec<FesSection>) -> RuntimeResult<FesDispatch> {
        let mut includes = Vec::new();
        let mut preloads = Vec::new();
        for section in &sections {
            let kind = section
                .header
                .split(u8::is_ascii_whitespace)
                .next()
                .unwrap_or_default();
            match kind {
                b"INCLUDE" => includes.extend(section.lines.iter().map(|line| line.bytes.clone())),
                b"PRELOAD" => preloads.extend(section.lines.iter().map(|line| line.bytes.clone())),
                _ => {
                    return Err(scene_error(
                        "FES declarations",
                        None,
                        format!(
                            "unexpected declaration section {:?}; only INCLUDE and PRELOAD are dispatched here",
                            String::from_utf8_lossy(kind)
                        ),
                    ));
                }
            }
        }
        self.include_requests.extend(includes);
        self.preload_requests.extend(preloads);
        self.declarations.extend(sections);
        Ok(FesDispatch::Declarations {
            include_count: self.include_requests.len(),
            preload_count: self.preload_requests.len(),
        })
    }

    fn dispatch_keyblocks(&mut self, table: FesKeyBlockTable) -> RuntimeResult<FesDispatch> {
        if table.declared_count > MAX_KEY_BLOCKS {
            return Err(scene_error(
                "FES KEYBLOCK",
                Some(table.offset),
                format!(
                    "declared {} blocks exceeds runtime limit {}",
                    table.declared_count, MAX_KEY_BLOCKS
                ),
            ));
        }
        if table.blocks.len() != table.declared_count {
            return Err(scene_error(
                "FES KEYBLOCK",
                Some(table.offset),
                format!(
                    "declared {} blocks but received {}",
                    table.declared_count,
                    table.blocks.len()
                ),
            ));
        }
        let mut seen = vec![false; table.declared_count];
        for block in &table.blocks {
            if block.index >= table.declared_count || seen[block.index] {
                return Err(scene_error(
                    "FES KEYBLOCK",
                    Some(table.offset),
                    format!("invalid or duplicate keyblock index {}", block.index),
                ));
            }
            seen[block.index] = true;
        }
        let count = table.blocks.len();
        self.keyblocks = Some(table);
        Ok(FesDispatch::KeyBlocks { count })
    }

    fn dispatch_create_object(
        &mut self,
        definition: FesObjectDefinition,
    ) -> RuntimeResult<FesDispatch> {
        let kind = FesObjectKind::parse(&definition.kind).ok_or_else(|| {
            scene_error(
                "FES object creation",
                Some(definition.offset),
                format!(
                    "unrecognized FES object type {:?}",
                    String::from_utf8_lossy(&definition.kind)
                ),
            )
        })?;
        if definition.name.is_empty() {
            return Err(scene_error(
                "FES object creation",
                Some(definition.offset),
                "object name is empty".to_string(),
            ));
        }
        if definition.banks < -1 {
            return Err(scene_error(
                "FES object creation",
                Some(definition.offset),
                format!("invalid bank count {}", definition.banks),
            ));
        }
        let prior_count = self
            .objects
            .iter()
            .find(|object| object.definition.name == definition.name)
            .map_or(1, |object| object.banks.len());
        let bank_count = match definition.banks {
            -1 if prior_count > 1 => {
                return Err(scene_error(
                    "FES object creation",
                    Some(definition.offset),
                    "redeclaring an array requires an explicit bank count".to_string(),
                ));
            }
            -1 => 1,
            0 => 1,
            count => count as usize,
        };
        if bank_count > MAX_OBJECT_BANKS {
            return Err(scene_error(
                "FES object creation",
                Some(definition.offset),
                format!("bank count {bank_count} exceeds runtime limit"),
            ));
        }
        let state = FesObjectDefinitionState {
            kind,
            kind_name: definition.kind,
            name: definition.name.clone(),
            bank_expression: definition.bank_expression,
            requested_banks: definition.banks,
            bank_count,
        };
        let banks = (0..bank_count)
            .map(|index| FesObjectBankState {
                index,
                initializers: Vec::new(),
            })
            .collect();
        self.definitions.push(state.clone());
        let object_index = self.objects.len();
        for bank in 0..bank_count {
            self.object_bindings
                .entry((definition.name.clone(), bank))
                .or_insert((object_index, bank));
        }
        self.objects.push(FesObjectState {
            definition: state,
            banks,
        });
        self.status = FesRuntimeStatus::Running;
        Ok(FesDispatch::ObjectCreated {
            name: definition.name,
            banks: bank_count,
        })
    }

    fn dispatch_initialize_object(
        &mut self,
        initializer: FesObjectInitialization,
    ) -> RuntimeResult<FesDispatch> {
        let (name, selected_bank) = parse_object_target(&initializer.target, initializer.offset)?;
        let count = self
            .objects
            .iter()
            .find(|object| object.definition.name == name)
            .map_or(0, |object| object.definition.bank_count);
        let bank_indices = match selected_bank {
            Some(index) => vec![index],
            None => (0..count).collect(),
        };
        let properties = initializer
            .columns
            .iter()
            .cloned()
            .zip(initializer.values.iter().cloned())
            .map(|(column, value)| FesObjectProperty { column, value })
            .collect::<Vec<_>>();
        let mut applied = 0;
        for index in bank_indices {
            if let Some(&(object_index, bank_index)) =
                self.object_bindings.get(&(name.clone(), index))
            {
                self.objects[object_index].banks[bank_index]
                    .initializers
                    .extend(properties.iter().cloned());
                applied += 1;
            }
        }
        self.initializers.push(initializer);
        self.status = FesRuntimeStatus::Running;
        Ok(FesDispatch::ObjectInitialized {
            name,
            banks: applied,
            properties: properties.len(),
        })
    }

    fn dispatch_keyblock_binding(
        &mut self,
        binding: FesKeyBlockBinding,
    ) -> RuntimeResult<FesDispatch> {
        let (name, bank) = parse_object_target(&binding.target, binding.offset)?;
        // KEYBLOCK is an object initializer and skips absent target banks.
        if !self.object_bindings.contains_key(&(name, bank.unwrap_or(0))) {
            return Ok(FesDispatch::ControlEvent);
        }
        if binding.block < -1 {
            return Err(scene_error(
                "FES KEYBLOCK binding",
                Some(binding.offset),
                format!("invalid keyblock index {}", binding.block),
            ));
        }
        if binding.block >= 0 {
            let table = self.keyblocks.as_ref().ok_or_else(|| {
                scene_error(
                    "FES KEYBLOCK binding",
                    Some(binding.offset),
                    "binding arrived before the KEYBLOCK table".to_string(),
                )
            })?;
            if !table
                .blocks
                .iter()
                .any(|block| block.index == binding.block as usize)
            {
                return Err(scene_error(
                    "FES KEYBLOCK binding",
                    Some(binding.offset),
                    format!("keyblock {} does not exist", binding.block),
                ));
            }
        }
        let target = binding.target.clone();
        let block = binding.block;
        self.keyblock_bindings.push(binding);
        Ok(FesDispatch::KeyBlockBound { target, block })
    }

    fn dispatch_subcommands(&mut self, table: FesSubcommandTable) -> RuntimeResult<FesDispatch> {
        // The parser has already omitted malformed lines. Retain their
        // diagnostics alongside valid bindings instead of rejecting all
        // motion declarations because one independent line was invalid.
        let count = table.bindings.len();
        self.subcommands = Some(table);
        self.status = FesRuntimeStatus::Running;
        Ok(FesDispatch::Subcommands { count })
    }

    pub fn invoke_subcommand(
        &self,
        name: &[u8],
        supplied_parameters: &[Vec<u8>],
        context: &[(Vec<u8>, Vec<u8>)],
    ) -> RuntimeResult<FesSubcommandInvocation> {
        let table = self.subcommands.as_ref().ok_or_else(|| {
            scene_error(
                "FES SUBCOMMAND invocation",
                None,
                "SUBCOMMAND table has not been dispatched".to_string(),
            )
        })?;
        let binding = table
            .bindings
            .iter()
            .find(|binding| binding.name == name)
            .ok_or_else(|| {
                scene_error(
                    "FES SUBCOMMAND invocation",
                    None,
                    format!(
                        "subcommand {:?} is not declared",
                        String::from_utf8_lossy(name)
                    ),
                )
            })?;
        if supplied_parameters.len() > binding.parameters.len() {
            return Err(scene_error(
                "FES SUBCOMMAND invocation",
                Some(binding.offset),
                format!(
                    "received {} parameters for a {}-parameter declaration",
                    supplied_parameters.len(),
                    binding.parameters.len()
                ),
            ));
        }
        let parameters = binding
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                supplied_parameters
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| parameter.default.clone())
            })
            .collect::<Vec<_>>();
        let mut arguments = Vec::with_capacity(binding.arguments.len());
        for argument in &binding.arguments {
            let value = match argument {
                FesSubcommandArgument::Parameter(index) => {
                    parameters.get(*index).cloned().ok_or_else(|| {
                        scene_error(
                            "FES SUBCOMMAND invocation",
                            Some(binding.offset),
                            format!("mapping references missing parameter {index}"),
                        )
                    })?
                }
                FesSubcommandArgument::Integer(value) => value.to_string().into_bytes(),
                FesSubcommandArgument::Context(key) => context
                    .iter()
                    .find(|(candidate, _)| candidate == key)
                    .map(|(_, value)| value.clone())
                    .ok_or_else(|| {
                        scene_error(
                            "FES SUBCOMMAND invocation",
                            Some(binding.offset),
                            format!(
                                "context value {:?} was not supplied",
                                String::from_utf8_lossy(key)
                            ),
                        )
                    })?,
            };
            arguments.push(value);
        }
        let mode = match binding.mode.as_slice() {
            b"oneshot" => FesSubcommandMode::OneShot,
            b"continue" => FesSubcommandMode::Continue,
            other => {
                return Err(scene_error(
                    "FES SUBCOMMAND invocation",
                    Some(binding.offset),
                    format!(
                        "unsupported subcommand mode {:?}",
                        String::from_utf8_lossy(other)
                    ),
                ));
            }
        };
        Ok(FesSubcommandInvocation {
            mode,
            name: binding.name.clone(),
            target: binding.target.clone(),
            parameters,
            arguments,
        })
    }

    pub fn set_cstl_localization(&mut self, localization: Option<CstlLocalization>) {
        self.localization = localization;
    }

    pub fn set_cstl_table(
        &mut self,
        table: Option<CstlTable>,
        requested_language: Option<&str>,
        fallback_language: Option<&str>,
    ) {
        self.localization =
            table.map(|table| CstlLocalization::new(table, requested_language, fallback_language));
    }

    pub fn set_cstl_language(&mut self, language: Option<&str>) {
        if let Some(localization) = &mut self.localization {
            localization.set_language(language);
        }
    }

    pub fn cstl_language(&self) -> Option<&str> {
        self.localization
            .as_ref()
            .and_then(CstlLocalization::language)
    }

    pub fn localized_name(&self, index: usize, fallback: &[u8]) -> Vec<u8> {
        self.localization.as_ref().map_or_else(
            || fallback.to_vec(),
            |localization| localization.name_bytes(index, fallback),
        )
    }

    pub fn localized_content(&self, index: usize, fragment: i32, fallback: &[u8]) -> Vec<u8> {
        self.localization.as_ref().map_or_else(
            || fallback.to_vec(),
            |localization| localization.content_bytes(index, fragment, fallback),
        )
    }

    pub fn localized_option(&self, input_count: usize, choice: usize, fallback: &[u8]) -> Vec<u8> {
        self.localization.as_ref().map_or_else(
            || fallback.to_vec(),
            |localization| localization.option_bytes(input_count, choice, fallback),
        )
    }

    pub fn dispatch_message(
        &mut self,
        event: &CstEvent,
        layout_settings: &MessageLayoutSettings,
        timing_settings: &MessageTimingSettings,
        initial_units: u32,
    ) -> RuntimeResult<Option<MessageRevealProgress>> {
        let (speaker, source_text, language) = match event {
            CstEvent::Speaker(text) => (true, text.clone(), None),
            CstEvent::Message(text) => (false, text.clone(), None),
            CstEvent::LocalizedSpeaker { text, language } => {
                (true, text.clone(), Some(language.clone()))
            }
            CstEvent::LocalizedMessage { text, language } => {
                (false, text.clone(), Some(language.clone()))
            }
            _ => return Ok(None),
        };
        let tokens = event
            .message_tokens()
            .map_err(|error| {
                scene_error(
                    "CST message parsing",
                    script_error_offset(&error),
                    error.to_string(),
                )
            })?
            .ok_or_else(|| {
                scene_error(
                    "CST message parsing",
                    None,
                    "message event did not produce text tokens".to_string(),
                )
            })?;
        let layout = MessageLayout::from_tokens(&tokens, layout_settings).map_err(|error| {
            scene_error(
                "message layout",
                script_error_offset(&error),
                error.to_string(),
            )
        })?;
        let schedule =
            MessageSchedule::from_tokens(&tokens, &layout, timing_settings, initial_units)
                .map_err(|error| {
                    scene_error(
                        "message schedule",
                        script_error_offset(&error),
                        error.to_string(),
                    )
                })?;
        let mut reveal = MessageReveal::new(&schedule);
        let initial_progress = reveal.advance(initial_units).into();
        self.message = Some(MessagePresentation {
            speaker,
            source_text,
            language,
            tokens,
            layout,
            schedule,
            reveal,
        });
        Ok(Some(initial_progress))
    }

    pub fn advance_message(&mut self, now: u32) -> Option<MessageRevealProgress> {
        self.message
            .as_mut()
            .map(|message| message.reveal.advance(now).into())
    }

    pub fn reveal_message_all(&mut self) -> Option<MessageRevealProgress> {
        self.message
            .as_mut()
            .map(|message| message.reveal.reveal_all().into())
    }

    pub fn object(&self, name: &[u8]) -> Option<&FesObjectState> {
        self.objects
            .iter()
            .find(|object| object.definition.name == name)
    }

    pub fn object_bank(&self, name: &[u8], bank: usize) -> Option<&FesObjectBankState> {
        let &(object, bank) = self.object_bindings.get(&(name.to_vec(), bank))?;
        self.objects.get(object)?.banks.get(bank)
    }

    /// Deleting a binding never promotes an unbound duplicate object.
    pub fn remove_object_bank(&mut self, name: &[u8], bank: usize) -> bool {
        self.object_bindings.remove(&(name.to_vec(), bank)).is_some()
    }

    pub fn object_bank_definition(
        &self,
        name: &[u8],
        bank: usize,
    ) -> Option<&FesObjectDefinitionState> {
        let &(object, _) = self.object_bindings.get(&(name.to_vec(), bank))?;
        Some(&self.objects.get(object)?.definition)
    }

    pub fn subcommand(&self, name: &[u8]) -> Option<&FesSubcommand> {
        self.subcommands
            .as_ref()?
            .bindings
            .iter()
            .find(|binding| binding.name == name)
    }
}

fn parse_object_target(target: &[u8], offset: usize) -> RuntimeResult<(Vec<u8>, Option<usize>)> {
    let Some(open) = target.iter().position(|byte| *byte == b'[') else {
        if target.is_empty() {
            return Err(scene_error(
                "FES object reference",
                Some(offset),
                "object target is empty".to_string(),
            ));
        }
        return Ok((target.to_vec(), None));
    };
    if open == 0 || target.last() != Some(&b']') || open + 1 >= target.len() - 1 {
        return Err(scene_error(
            "FES object reference",
            Some(offset),
            format!(
                "malformed object bank target {:?}",
                String::from_utf8_lossy(target)
            ),
        ));
    }
    let bank = std::str::from_utf8(&target[open + 1..target.len() - 1])
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .map(|value| value as usize)
        .ok_or_else(|| {
            scene_error(
                "FES object reference",
                Some(offset + open),
                "bank selector must be a resolved signed decimal index".to_string(),
            )
        })?;
    Ok((target[..open].to_vec(), Some(bank)))
}

fn script_error_offset(error: &ScriptError) -> Option<usize> {
    match error {
        ScriptError::Truncated { offset, .. }
        | ScriptError::InvalidStructuredText { offset, .. }
        | ScriptError::InvalidExpression { offset, .. }
        | ScriptError::InvalidSceneControl { offset, .. }
        | ScriptError::InvalidInstruction { offset }
        | ScriptError::InvalidOpcode { offset, .. }
        | ScriptError::UnimplementedOpcode { offset, .. }
        | ScriptError::UnsupportedOpcode { offset, .. } => Some(*offset),
        _ => None,
    }
}

fn scene_error(phase: &'static str, offset: Option<usize>, message: String) -> RuntimeError {
    RuntimeError::Scene {
        phase,
        offset,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_script::{FesInterpreter, FesLine, FesProgram, expression::ExpressionState};

    #[test]
    fn keyblock_initialization_skips_missing_title_button_banks() {
        for (button, bank_count, block) in [("btn_omk", 5, 1), ("btn_ij", 4, 2)] {
            let last_bank = bank_count - 1;
            let source = format!(
                "#DEFINE\nBUTTON {button}[{bank_count}]\n\
                 #KEYBLOCK 3\n{block} 1,{bank_count} -1,-1,-1,-1\n\
                 #OBJECT KEYBLOCK\n{button}[{last_bank}] {block},0,{last_bank}\n\
                 {button}[{bank_count}] {block},0,{bank_count}\n\
                 {button}[0] {block},0,0\n#START\nexit\n"
            );
            let mut interpreter =
                FesInterpreter::new(FesProgram::from_text(source.as_bytes()).unwrap()).unwrap();
            let mut state = ExpressionState::default();
            let mut runtime = FesRuntimeAdapter::new();

            for _ in 0..16 {
                interpreter.run(&mut state, 1).unwrap();
                runtime.dispatch(interpreter.pending().unwrap().clone()).unwrap();
                if runtime.status == FesRuntimeStatus::Exited(0) {
                    break;
                }
                interpreter.resume();
            }

            assert_eq!(runtime.status, FesRuntimeStatus::Exited(0));
            assert!(runtime.object_bank(button.as_bytes(), bank_count as usize).is_none());
            let bindings = runtime.keyblock_bindings.as_slice();
            assert_eq!(bindings.len(), 2);
            for (binding, bank) in bindings.iter().zip([last_bank, 0]) {
                assert_eq!(binding.target, format!("{button}[{bank}]").into_bytes());
                assert_eq!(binding.block, block);
                assert_eq!(binding.position, [0, bank]);
            }
        }
    }

    #[test]
    fn temporary_wipe_object_can_be_registered_and_released() {
        let mut adapter = FesRuntimeAdapter::new();
        let name = b"__wipeobj__";
        adapter.dispatch(FesEvent::CreateObject(FesObjectDefinition {
            offset: 0, kind: b"WIPE".to_vec(), name: name.to_vec(),
            bank_expression: None, banks: 1,
        })).unwrap();
        assert_eq!(adapter.object_bank_definition(name, 0).unwrap().kind, FesObjectKind::Wipe);
        assert!(adapter.remove_object_bank(name, 0));
        assert!(adapter.object_bank(name, 0).is_none());
    }

    #[test]
    fn duplicate_arrays_keep_first_bindings_and_allow_new_explicit_banks() {
        let mut runtime = FesRuntimeAdapter::new();
        for (kind, banks) in [(b"IMAGE".as_slice(), 2i32), (b"PLANE".as_slice(), 3i32)] {
            runtime.dispatch(FesEvent::CreateObject(FesObjectDefinition {
                offset: 0, kind: kind.to_vec(), name: b"item".to_vec(),
                bank_expression: Some(banks.to_string().into_bytes()), banks,
            })).unwrap();
        }
        assert_eq!(runtime.objects.len(), 2);
        assert_eq!(runtime.object_bank_definition(b"item", 0).unwrap().kind, FesObjectKind::Image);
        assert_eq!(runtime.object_bank_definition(b"item", 2).unwrap().kind, FesObjectKind::Plane);
        assert_eq!(runtime.dispatch(FesEvent::InitializeObject(FesObjectInitialization {
            offset: 0, columns: vec![b"DISP".to_vec()], target: b"item".to_vec(),
            values: vec![b"1".to_vec()],
        })).unwrap(), FesDispatch::ObjectInitialized { name: b"item".to_vec(), banks: 2, properties: 1 });
        assert!(runtime.object_bank(b"item", 2).unwrap().initializers.is_empty());
        assert!(runtime.remove_object_bank(b"item", 0));
        assert!(runtime.object_bank(b"item", 0).is_none());
        assert!(runtime.objects[1].banks[0].initializers.is_empty());
        assert_eq!(runtime.dispatch(FesEvent::InitializeObject(FesObjectInitialization {
            offset: 0, columns: vec![b"DISP".to_vec()], target: b"item[-1]".to_vec(),
            values: vec![b"1".to_vec()],
        })).unwrap(), FesDispatch::ObjectInitialized { name: b"item".to_vec(), banks: 0, properties: 1 });
    }

    #[test]
    fn waits_cannot_be_overwritten_by_new_script_events() {
        let mut runtime = FesRuntimeAdapter::new();
        runtime.dispatch(FesEvent::Wait(3)).unwrap();

        assert!(
            runtime
                .dispatch(FesEvent::Command(FesLine {
                    offset: 0,
                    bytes: b"draw".to_vec(),
                }))
                .is_err()
        );
        assert!(matches!(
            runtime.status,
            FesRuntimeStatus::WaitingFrames { wake_frame: 3 }
        ));
        assert!(!runtime.advance_wait_frame());
        assert!(!runtime.advance_wait_frame());
        assert!(runtime.advance_wait_frame());
        assert_eq!(runtime.status, FesRuntimeStatus::Running);
    }

    #[test]
    fn control_wait_frame_boundary_and_command_resume_require_their_own_resumers() {
        let mut runtime = FesRuntimeAdapter::new();
        runtime
            .dispatch(FesEvent::WaitControl {
                kind: b"key".to_vec(),
                arguments: vec![b"enter".to_vec()],
            })
            .unwrap();
        assert!(!runtime.resume_control(b"mouse"));
        assert!(runtime.resume_control(b"key"));
        assert_eq!(runtime.status, FesRuntimeStatus::Running);

        runtime.dispatch(FesEvent::Frame).unwrap();
        assert!(!runtime.advance_wait_frame());
        assert!(runtime.resume_frame_boundary());

        runtime
            .dispatch(FesEvent::Command(FesLine {
                offset: 4,
                bytes: b"draw".to_vec(),
            }))
            .unwrap();
        assert!(runtime.dispatch(FesEvent::Exit(0)).is_err());
        assert_eq!(runtime.take_command().as_deref(), Some(b"draw".as_slice()));
        assert_eq!(runtime.status, FesRuntimeStatus::Running);
    }

    #[test]
    fn exited_adapter_is_terminal() {
        let mut runtime = FesRuntimeAdapter::new();
        runtime.dispatch(FesEvent::Exit(7)).unwrap();
        assert!(!runtime.advance_wait_frame());
        assert!(runtime.dispatch(FesEvent::Frame).is_err());
        assert_eq!(runtime.status, FesRuntimeStatus::Exited(7));
    }
}
