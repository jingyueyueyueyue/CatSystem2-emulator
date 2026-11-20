//! Desktop FES control flow. The host applies one effect before requesting the
//! next at the same frame. Main Waiting/Frame ends a logical tick; Yield keeps
//! a budget continuation at the same frame. Handler-local Waiting continues.
//! Object pixels, input hit testing and child-task services remain host work.
//! All script text stays in original bytes and all commands retain parameters.
//!
//! Parent integration:
//! - declare `mod fes_driver`; store this driver in FesRuntime and construct it
//!   with `FesScriptDriver::new(&document)?` from a fallible FesRuntime::new;
//! - replace the old advance loop by pumping advance at one frame, applying
//!   each effect, and continuing only after successful host application;
//! - keep shared numeric/string tables in the supplied ExpressionState;
//! - route pixel command parameters directly, never through split_whitespace;
//! - use dispatch_system_block for notifications and on_input for input.
use anyhow::{Result, bail};
use cs2_runtime::{FesDispatch, FesRuntimeAdapter, FesRuntimeStatus};
use cs2_script::{
    ExpressionState, FesDocument, FesEvent, FesExecutionContext, FesInterpreter,
    FesProgram, FesStep, SharedExternalStorage,
    scene::FesControlPosition,
};
use std::collections::VecDeque;

#[path = "fes_files.rs"]
mod files;
#[path = "fes_statement.rs"]
mod statement;
#[path = "fes_main_flow.rs"]
pub(crate) mod main_flow;
pub(crate) use statement::FesStatementStep;

#[path = "fes_driver_state.rs"]
mod state;

#[cfg(test)]
#[path = "fes_input_tests.rs"]
mod input_tests;

const MAX_INPUTS: usize = 4096;
const MAX_NOTIFICATIONS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct FesDriverTarget {
    /// Unbanked object name as stored in the FES.
    pub name: Vec<u8>,
    pub bank: i32,
    /// Optional DEFAULT event group, supplied by the actual object binding.
    pub default_group: Option<Vec<u8>>,
    /// Object handle for input named values; use -1 if no handle is available.
    pub value: i32,
}

impl FesDriverTarget {
    pub(crate) fn parameter(&self) -> Vec<u8> {
        let mut name = self.name.clone();
        name.extend_from_slice(format!("[{}]", self.bank).as_bytes());
        name
    }
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct FesDriverInput {
    /// Existing desktop logical input number (2..21).
    pub code: u32,
    pub target: Option<FesDriverTarget>,
    /// Explicit events such as FOCUS/UNFOCUS/STOP override code-derived events.
    pub event: Option<Vec<u8>>,
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct FesDriverCommand {
    pub offset: usize,
    /// Display/debug source; never split this again to reconstruct parameters.
    pub source: Vec<u8>,
    pub parameters: Vec<Vec<u8>>,
    pub context: FesExecutionContext,
    pub object_event: Option<FesDriverTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) enum FesChildKind {
    Fes,
    Kcs,
}

#[derive(Clone, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) enum FesDriverWait {
    Frames { until: u64 },
    Frame { after: u64 },
    Control { kind: Vec<u8>, arguments: Vec<Vec<u8>>, entered: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) enum FesObjectEventPhase {
    BeginDefault,
    BeginObject,
    Complete { handled: bool, transferred: bool },
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) enum FesDriverEffect {
    /// Adapter has already applied this metadata. Apply the same event to
    /// pixel/input objects; do not dispatch it to the adapter a second time.
    Initialize { event: FesEvent, dispatch: FesDispatch },
    Command(FesDriverCommand),
    /// Native 143/161/172 service sequence; initialization is one, followed
    /// by binding this exact shared object, including for nonempty storage.
    ExecKcs { name: Vec<u8>, #[serde(skip)] external: SharedExternalStorage },
    ExecFes { name: Vec<u8>, parameters: [i32; 16], #[serde(skip)] external: SharedExternalStorage },
    EndChild(FesChildKind),
    Send { message: [u32; 4], parent: bool },
    React { target: Option<FesDriverTarget> },
    /// Main exit continues at the next statement at the SAME logical frame.
    /// Handler exits are deferred until the handler has returned.
    Exit(i32),
    /// Ordered scene notifications; handlers queue these until the main pump.
    PreferredFocus(FesDriverTarget),
    FocusLock(u32),
    ObjectEvent { target: FesDriverTarget, event: Vec<u8>, phase: FesObjectEventPhase },
    SystemBlock { name: Vec<u8>, complete: bool },
    /// A queued single-statement dispatch returned. Its actual native bool
    /// and cursor are retained separately for the requesting media service.
    MediaStatementReturned,
    /// Apply the actual motion/text cancellation, then poll its completion.
    /// Text/root waits remain blocked until resume_control confirms completion.
    CancelWait { wait: FesDriverWait, finish_motion: bool, reveal_text: bool },
    /// A handler-local wait ends that handler; it does not install a new main
    /// sleep deadline. Main Waiting requires clock/control completion.
    Waiting { wait: FesDriverWait, handler_local: bool },
    Frame,
    /// Instruction budget exhausted; resume at the SAME logical frame.
    Yield,
    ControlFlow(main_flow::controls_flow::Request),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum HandlerPhase {
    Default,
    Object,
    System,
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Handler {
    target: FesDriverTarget,
    event: Vec<u8>,
    object_header: Vec<u8>,
    object_exists: bool,
    phase: HandlerPhase,
    depth: usize,
    saved_control: FesControlPosition,
    saved_status: FesRuntimeStatus,
    saved_wait: Option<FesDriverWait>,
    saved_current: i32,
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct SystemBlockRequest {
    name: Vec<u8>,
    header: Vec<u8>,
    object: Option<FesDriverTarget>,
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct FesScriptDriver {
    pub(crate) interpreter: FesInterpreter,
    pub(crate) adapter: FesRuntimeAdapter,
    #[serde(skip)]
    island_gbk_fes_source: bool,
    #[serde(skip)]
    rebellions_message_fes_source: bool,
    #[serde(skip)]
    pub(super) kanmi_message_fes_source: Option<bool>,
    expression: ExpressionState,
    #[serde(skip)]
    external: SharedExternalStorage,
    inputs: VecDeque<FesDriverInput>,
    systems: VecDeque<SystemBlockRequest>,
    deferred: VecDeque<FesDriverEffect>,
    notices: VecDeque<FesDriverEffect>,
    handler: Option<Handler>,
    waiting: Option<FesDriverWait>,
    last_frame: Option<u64>,
    focus: Option<FesDriverTarget>,
    task_id: u32,
    cancel_wait: bool,
    raw_input: bool,
    skip_input: bool,
    keyskip_on: bool,
    keyskip_ctrl: bool,
    keyskip_cont: bool,
    forced_skip: bool,
    wait_cancel_announced: bool,
    budget_frame: Option<u64>,
    pending_file: Option<files::FileCommand>,
    media_statement: Option<statement::MediaStatement>,
    media_statement_result: Option<statement::FesStatementReturn>,
    /// Synchronous callbacks cannot consume the main body's ordered notices.
    notification_only: bool,
}

/// In-memory checkpoint, including pending effects and control continuations.
/// It retains shared external identity; persist external bytes/groups separately.
#[derive(Clone, Debug)]
pub(crate) struct FesDriverSnapshot(Box<FesScriptDriver>);

/// Park the caller's notification queues while the SAME interpreter services
/// a synchronous media refresh. Locals, named values, calls, object bindings
/// and control position remain live; this is not a cloned script context.
#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct FesMediaNotificationScope {
    handler: Option<Handler>,
    systems: VecDeque<SystemBlockRequest>,
    inputs: VecDeque<FesDriverInput>,
    deferred: VecDeque<FesDriverEffect>,
    notices: VecDeque<FesDriverEffect>,
    pending_file: Option<files::FileCommand>,
    current: i32,
}

impl FesScriptDriver {
    pub(crate) fn new(document: &FesDocument) -> Result<Self> {
        let program = FesProgram::from_document(document)?;
        Ok(Self {
            interpreter: FesInterpreter::new(program)?,
            adapter: FesRuntimeAdapter::new(),
            island_gbk_fes_source: false,
            rebellions_message_fes_source: false,
            kanmi_message_fes_source: None,
            expression: ExpressionState::default(),
            external: SharedExternalStorage::default(),
            inputs: VecDeque::new(),
            systems: VecDeque::new(),
            deferred: VecDeque::new(),
            notices: VecDeque::new(),
            handler: None,
            waiting: None,
            last_frame: None,
            focus: None,
            task_id: 0,
            cancel_wait: false,
            raw_input: false,
            skip_input: false,
            keyskip_on: true,
            keyskip_ctrl: false,
            keyskip_cont: false,
            forced_skip: false,
            wait_cancel_announced: false,
            budget_frame: None,
            pending_file: None,
            media_statement: None,
            media_statement_result: None,
            notification_only: false,
        })
    }

    pub(crate) fn is_island_gbk_fes_source(&self) -> bool {
        self.island_gbk_fes_source
    }

    pub(super) fn set_island_gbk_fes_source(&mut self, enabled: bool) {
        self.island_gbk_fes_source = enabled;
    }

    pub(super) fn is_rebellions_message_fes_source(&self) -> bool {
        self.rebellions_message_fes_source
    }

    pub(super) fn set_rebellions_message_fes_source(&mut self, enabled: bool) {
        self.rebellions_message_fes_source = enabled;
    }

    pub(crate) fn set_task_id(&mut self, task_id: u32) {
        self.task_id = task_id;
    }

    /// Set at FES task construction, not from a copy of external byte data.
    pub(crate) fn bind_external(&mut self, external: SharedExternalStorage) {
        self.external = external;
    }

    pub(crate) fn external_binding(&self) -> SharedExternalStorage {
        self.external.clone()
    }

    pub(crate) fn set_named_value(&mut self, name: &str, index: i32, value: i32) {
        self.interpreter.named.entry(name.to_owned()).or_default().insert(index, value);
    }

    pub(crate) fn select_main_block(&mut self, name: &[u8]) -> bool {
        let previous = self.interpreter.control_position();
        if !self.interpreter.enter_block(name) { return false; }
        if let Some(handler) = self.handler.as_mut() {
            handler.saved_control = self.interpreter.control_position();
            self.interpreter.restore_control_position(previous)
                .expect("an existing live control position is valid");
        }
        true
    }

    pub(crate) fn media_statement_pending(&self) -> bool { self.media_statement.is_some() }

    pub(crate) fn set_child_exit_code(&mut self, value: i32) {
        self.set_named_value("_EXITCODE_", 0, value);
    }

    pub(crate) fn focus(&self) -> Option<&FesDriverTarget> {
        self.focus.as_ref()
    }

    pub(crate) fn snapshot(&self) -> FesDriverSnapshot {
        FesDriverSnapshot(Box::new(self.clone()))
    }

    pub(crate) fn restore_snapshot(&mut self, snapshot: &FesDriverSnapshot) {
        *self = snapshot.0.as_ref().clone();
    }

    pub(crate) fn begin_media_notification_scope(&mut self) -> FesMediaNotificationScope {
        FesMediaNotificationScope {
            handler: self.handler.take(),
            systems: std::mem::take(&mut self.systems),
            inputs: std::mem::take(&mut self.inputs),
            deferred: std::mem::take(&mut self.deferred),
            notices: std::mem::take(&mut self.notices),
            pending_file: self.pending_file.take(),
            current: self.expression.current,
        }
    }

    pub(crate) fn end_media_notification_scope(
        &mut self, mut scope: FesMediaNotificationScope,
    ) {
        // A statement dispatcher restores its own parked scope before its
        // caller publishes the statement receipt. Do not count that owning
        // statement/receipt as an unconsumed callback queue.
        debug_assert!(self.handler.is_none() && self.systems.is_empty()
            && self.inputs.is_empty() && self.notices.is_empty());
        // noact/lockactive/exit produced by a callback belong to the next
        // ordinary body, after notices already parked by its caller.
        scope.deferred.append(&mut self.deferred);
        self.handler = scope.handler;
        self.systems = scope.systems;
        self.inputs = scope.inputs;
        self.deferred = scope.deferred;
        self.notices = scope.notices;
        self.pending_file = scope.pending_file;
        self.expression.current = scope.current;
    }

    pub(crate) fn has_media_notification_work(&self) -> bool {
        self.media_statement.is_some() || self.handler.is_some()
            || !self.systems.is_empty() || !self.inputs.is_empty()
            || !self.notices.is_empty()
    }

    /// A receipt is consumed by finish_owner, never by the interpreter pump.
    pub(crate) fn media_statement_result_ready(&self) -> bool {
        self.media_statement_result.is_some()
    }

    /// One interpreter instruction per call is deliberate. A missing queued
    /// input handler consumes that iteration, so advance_inner cannot fall
    /// through to the parked main body in the same call.
    pub(crate) fn advance_media_notification(
        &mut self, frame: u64, shared: &mut ExpressionState,
    ) -> Result<Option<FesDriverEffect>> {
        if !self.has_media_notification_work() { return Ok(None); }
        if self.interpreter.initialization_pending() {
            bail!("FES media notification cannot execute unfinished declarations");
        }
        self.advance_notification(frame, shared, 1).map(Some)
    }

    /// Queue _SLEEP, _WAKE, MES_CHANGECONFIG or another exact block name.
    /// Returns false for an absent optional block. Pump advance until the
    /// matching SystemBlock { complete: true }; main position/wait is restored
    /// automatically while local and shared changes remain in effect.
    pub(crate) fn dispatch_system_block(&mut self, name: &[u8]) -> Result<bool> {
        self.dispatch_system_block_with_context(name, None)
    }

    /// Native 630's optional prefix/bank binds `@` while executing this exact
    /// block. It does not generate FOCUS/PUSH or a DEFAULT event.
    pub(crate) fn dispatch_system_block_with_context(
        &mut self, name: &[u8], object: Option<FesDriverTarget>,
    ) -> Result<bool> {
        let header = object.as_ref().map_or_else(
            || name.to_vec(), |target| event_header(&target.name, name),
        );
        if self.interpreter.program.block(&header).is_none() {
            return Ok(false);
        }
        if self.systems.len() >= MAX_INPUTS {
            bail!("FES system-block queue exceeds {MAX_INPUTS} entries");
        }
        self.systems.push_back(SystemBlockRequest { name: name.to_vec(), header, object });
        Ok(true)
    }

    pub(crate) fn has_pending_handler(&self) -> bool {
        self.handler.is_some() || !self.systems.is_empty() || !self.inputs.is_empty()
            || self.media_statement.is_some()
    }

    pub(crate) fn waiting(&self) -> Option<&FesDriverWait> {
        self.waiting.as_ref()
    }

    /// The pixel/motion/text host calls this only after its actual wait query
    /// completes. No object wait is silently assumed to have finished.
    pub(crate) fn resume_control(&mut self, kind: &[u8]) -> bool {
        let matches = matches!(&self.waiting,
            Some(FesDriverWait::Control { kind: waiting, .. })
                if kind.is_empty() || waiting == kind);
        if matches && self.handler.is_none() {
            self.waiting = None;
            self.adapter.status = FesRuntimeStatus::Running;
            self.interpreter.resume();
            true
        } else {
            false
        }
    }

    pub(crate) fn on_input(&mut self, input: FesDriverInput) -> Result<()> {
        if self.inputs.len() >= MAX_INPUTS {
            bail!("FES input queue exceeds {MAX_INPUTS} entries");
        }
        self.inputs.push_back(input);
        Ok(())
    }

    /// Media notifications enter synchronously at the saved scene boundary.
    /// Their alias protocol is independent of a button's DEFAULT declaration.
    pub(crate) fn begin_media_event(
        &mut self, target: FesDriverTarget, event: &[u8],
    ) -> Result<Option<FesDriverEffect>> {
        if self.handler.is_some() || self.pending_file.is_some() {
            bail!("FES media event cannot reenter an active handler or file command");
        }
        if self.notices.len() >= MAX_NOTIFICATIONS {
            bail!("FES media notification queue exceeds {MAX_NOTIFICATIONS} entries");
        }
        let effect = self.begin_input_mode(FesDriverInput {
            code: 0, target: Some(target), event: Some(event.to_vec()),
        }, false)?;
        if let Some(effect @ FesDriverEffect::ObjectEvent {
            phase: FesObjectEventPhase::BeginObject | FesObjectEventPhase::BeginDefault, ..
        }) = &effect {
            self.notices.push_back(effect.clone());
        }
        Ok(effect)
    }

    pub(crate) fn initialization_pending(&self) -> bool {
        self.interpreter.initialization_pending()
            || self.interpreter.pending().is_some_and(is_declaration)
    }

    /// Command 338 prepares declarations without entering START or changing
    /// the controller's logical frame, waits, input, or budget continuation.
    /// In particular, a later command 339 may use a paused parent's elapsed
    /// time, independently of the absolute time at which loading occurred.
    pub(crate) fn advance_initialization(
        &mut self, shared: &mut ExpressionState,
    ) -> Result<Option<FesDriverEffect>> {
        if !self.initialization_pending() {
            return Ok(None);
        }
        if self.handler.is_some() || self.pending_file.is_some() {
            bail!("FES declaration preparation cannot reenter an active handler/command");
        }
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        self.expression.current = shared.current;
        let result = (|| {
            if self.interpreter.run(&mut self.expression, 1)? == FesStep::Yield {
                return Ok(Some(FesDriverEffect::Yield));
            }
            let event = self.interpreter.pending().cloned()
                .ok_or_else(|| anyhow::anyhow!("FES declaration event is missing"))?;
            if !is_declaration(&event) {
                bail!("FES declaration preparation reached executable script control");
            }
            let dispatch = self.adapter.dispatch_pending(&self.interpreter)?;
            self.interpreter.resume();
            Ok(Some(FesDriverEffect::Initialize { event, dispatch }))
        })();
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        result
    }

    /// Task notifications use elapsed time, whereas the ordinary FES body
    /// receives the task's raw update time. Keep their clocks separate: a
    /// paused task's elapsed value may legally be below the main body time.
    /// The owning SystemCommand retains this invocation's frame across yield.
    pub(crate) fn advance_notification(
        &mut self, frame: u64, shared: &mut ExpressionState, budget: usize,
    ) -> Result<FesDriverEffect> {
        self.advance_system_notification(frame, shared, budget, None)
    }

    pub(super) fn advance_system_notification(
        &mut self, frame: u64, shared: &mut ExpressionState, budget: usize,
        bounded_block: Option<&[u8]>,
    ) -> Result<FesDriverEffect> {
        let main_last_frame = self.last_frame.replace(frame);
        let main_budget_frame = self.budget_frame.take();
        let notification_only = std::mem::replace(&mut self.notification_only, true);
        let result = self.advance_with_notification(frame, shared, budget, bounded_block);
        self.notification_only = notification_only;
        self.last_frame = main_last_frame;
        self.budget_frame = main_budget_frame;
        result
    }

    /// `shared.shared` and `shared.strings` are the authoritative game-wide
    /// tables. They are moved into the driver for this call and always moved
    /// back on both success and error. Locals and named values stay per FES.
    pub(crate) fn advance(
        &mut self,
        frame: u64,
        shared: &mut ExpressionState,
        budget: usize,
    ) -> Result<FesDriverEffect> {
        self.advance_with_notification(frame, shared, budget, None)
    }

    fn advance_with_notification(
        &mut self,
        frame: u64,
        shared: &mut ExpressionState,
        budget: usize,
        bounded_block: Option<&[u8]>,
    ) -> Result<FesDriverEffect> {
        if self.media_statement.is_some() {
            return match self.advance_media_statement(frame, shared, budget)? {
                statement::FesStatementStep::Effect(effect) => Ok(effect),
                statement::FesStatementStep::Yield => Ok(FesDriverEffect::Yield),
                statement::FesStatementStep::Returned { .. } => {
                    Ok(FesDriverEffect::MediaStatementReturned)
                }
            };
        }
        if self.media_statement_result.is_some() {
            // The media owner has not observed the actual return yet.
            // Preserve this boundary instead of entering another main line.
            return Ok(FesDriverEffect::MediaStatementReturned);
        }
        if self.pending_file.is_some() {
            bail!("FES file command must finish before advancing the script");
        }
        if self.budget_frame.is_some_and(|pending| pending != frame) {
            bail!("FES budget continuation must resume at its original logical frame");
        }
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        if self.handler.is_none() {
            self.expression.current = shared.current;
        }
        let result = self.advance_inner(frame, budget, bounded_block);
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        self.budget_frame = if matches!(&result, Ok(FesDriverEffect::Yield)) {
            Some(frame)
        } else {
            None
        };
        result
    }

    /// Host object numeric arguments must use this entry with their current
    /// property value. It retains bank/local scope and expression side effects.
    pub(crate) fn evaluate_parameter(
        &mut self,
        source: &[u8],
        current: i32,
        shared: &mut ExpressionState,
    ) -> Result<i32> {
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        let previous = self.expression.current;
        self.expression.current = current;
        let result = self.interpreter.evaluate_parameter(source, &mut self.expression);
        self.expression.current = previous;
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        Ok(result?)
    }

    /// Use after actual object/bank lookup fails, preserving the full expanded
    /// line and the already-observed target lookup side effects.
    pub(crate) fn fallback_command(
        &mut self,
        command: &FesDriverCommand,
        shared: &mut ExpressionState,
    ) -> Result<i32> {
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        let previous = self.expression.current;
        self.expression.current = command.context.current;
        let result = self.interpreter.eval_fallback(&command.source, &mut self.expression);
        self.expression.current = previous;
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        Ok(result?)
    }

    fn number(&mut self, source: &[u8]) -> Result<i32> {
        Ok(self.interpreter.evaluate_parameter(source, &mut self.expression)?)
    }

    fn advance_inner(
        &mut self, frame: u64, budget: usize, bounded_block: Option<&[u8]>,
    ) -> Result<FesDriverEffect> {
        if self.last_frame.is_some_and(|previous| frame < previous) {
            bail!("FES frame moved backwards; restore a driver snapshot before rewinding");
        }
        if self.last_frame.is_some_and(|previous| frame > previous) {
            self.clear_transient_input();
        }
        self.last_frame = Some(frame);
        self.set_named_value("_FRAME_", 0, frame as u32 as i32);
        if budget == 0 {
            return Ok(FesDriverEffect::Yield);
        }
        for _ in 0..budget {
            if let Some(effect) = self.notices.pop_front() {
                return Ok(effect);
            }
            if self.handler.is_none() && !self.interpreter.initialization_pending() {
                if let Some(request) = self.systems.pop_front() {
                    return self.begin_system(request);
                }
                if !self.notification_only && let Some(effect) = self.deferred.pop_front() {
                    if matches!(effect, FesDriverEffect::EndChild(_)) {
                        self.waiting = Some(FesDriverWait::Frame { after: frame });
                        self.adapter.status = FesRuntimeStatus::FrameBoundary;
                    }
                    return Ok(effect);
                }
                if let Some(input) = self.inputs.pop_front() {
                    if let Some(effect) = self.begin_input(input)? {
                        return Ok(effect);
                    }
                    continue;
                }
            }
            if let Some(handler) = self.handler.as_ref() {
                if handler.phase == HandlerPhase::System
                    && handler.target.name.is_empty()
                    && handler.target.bank == 0
                    && bounded_block == Some(handler.event.as_slice())
                    && matches!(self.interpreter.current_block(), b"SETBUTTON" | b"_SETBUTTON")
                    && self.interpreter.return_call_at_block_end(handler.depth)
                {
                    continue;
                }
                if let Some(effect) = self.check_handler_boundary()? {
                    return Ok(effect);
                }
            } else if let Some(wait) = self.waiting.clone() {
                let ready = self.complete_wait(frame, &wait)?;
                if let Some(effect) = self.notices.pop_front() {
                    return Ok(effect);
                }
                if !ready {
                    return Ok(match wait {
                        FesDriverWait::Frame { .. } => FesDriverEffect::Frame,
                        wait => FesDriverEffect::Waiting { wait, handler_local: false },
                    });
                }
            }
            match self.interpreter.run(&mut self.expression, 1)? {
                FesStep::Yield => continue,
                FesStep::Event => {}
            }
            let event = self.interpreter.pending().cloned()
                .ok_or_else(|| anyhow::anyhow!("FES interpreter reported an event without data"))?;
            let dispatch = self.adapter.dispatch_pending(&self.interpreter)?;
            match event {
                FesEvent::Command(line) => {
                    let (_, parameters) = self.adapter.take_command_with_parameters()
                        .ok_or_else(|| anyhow::anyhow!("FES adapter lost its pending command"))?;
                    let mut parameters = parameters
                        .ok_or_else(|| anyhow::anyhow!("FES command has no structured parameters"))?;
                    let object_event = self.handler.as_ref()
                        .filter(|handler| handler.phase != HandlerPhase::System
                            || !handler.target.name.is_empty())
                        .map(|handler| handler.target.clone());
                    if parameters.first().is_some_and(|word| word == b"@") {
                        if let Some(target) = object_event.as_ref() {
                            parameters[0] = target.parameter();
                        } else {
                            self.interpreter.resume();
                            self.interpreter.eval_fallback(&line.bytes, &mut self.expression)?;
                            continue;
                        }
                    }
                    let command = FesDriverCommand {
                        offset: line.offset,
                        source: line.bytes,
                        parameters,
                        context: self.interpreter.context(&self.expression),
                        object_event,
                    };
                    self.interpreter.resume();
                    if let Some(effect) = self.command(command, frame)? {
                        return Ok(effect);
                    }
                }
                FesEvent::Wait(count) => {
                    let until = frame.checked_add(count.max(1) as u64)
                        .ok_or_else(|| anyhow::anyhow!("FES wait deadline overflows"))?;
                    return self.install_wait(FesDriverWait::Frames { until });
                }
                FesEvent::WaitFrame => {
                    return self.install_wait(FesDriverWait::Control {
                        kind: Vec::new(), arguments: Vec::new(), entered: frame,
                    });
                }
                FesEvent::WaitControl { kind, arguments } => {
                    return self.install_wait(FesDriverWait::Control { kind, arguments, entered: frame });
                }
                FesEvent::Frame => {
                    if self.handler.is_some() {
                        return self.finish_handler(false);
                    }
                    self.waiting = Some(FesDriverWait::Frame { after: frame });
                    self.clear_transient_input();
                    self.finish_left_input_frame();
                    return Ok(FesDriverEffect::Frame);
                }
                FesEvent::Exit(code) => {
                    self.interpreter.resume();
                    self.adapter.status = FesRuntimeStatus::Running;
                    if self.handler.is_some() {
                        self.defer(FesDriverEffect::Exit(code))?;
                        return self.finish_handler(false);
                    }
                    return Ok(FesDriverEffect::Exit(code));
                }
                event => {
                    self.interpreter.resume();
                    return Ok(FesDriverEffect::Initialize { event, dispatch });
                }
            }
        }
        Ok(FesDriverEffect::Yield)
    }

    fn install_wait(&mut self, wait: FesDriverWait) -> Result<FesDriverEffect> {
        if self.handler.is_some() {
            // Control targets are shared even though the main deadline is
            // suspended. A handler wait ends this handler without sleeping.
            if let Some(handler) = self.handler.as_mut() {
                if matches!(handler.saved_wait, Some(FesDriverWait::Control { .. }))
                    && matches!(wait, FesDriverWait::Control { .. })
                {
                    handler.saved_wait = Some(wait.clone());
                    if let FesDriverWait::Control { kind, arguments, .. } = &wait {
                        handler.saved_status = FesRuntimeStatus::WaitingControl {
                            kind: kind.clone(), arguments: arguments.clone(),
                        };
                    }
                }
            }
            let boundary = self.finish_handler(false)?;
            self.notices.push_back(boundary);
            Ok(FesDriverEffect::Waiting { wait, handler_local: true })
        } else {
            self.wait_cancel_announced = false;
            self.adapter.status = match &wait {
                FesDriverWait::Frames { until } => FesRuntimeStatus::WaitingFrames {
                    wake_frame: until.saturating_sub(self.last_frame.unwrap_or(0)),
                },
                FesDriverWait::Frame { .. } => FesRuntimeStatus::FrameBoundary,
                FesDriverWait::Control { kind, arguments, .. } => FesRuntimeStatus::WaitingControl {
                    kind: kind.clone(), arguments: arguments.clone(),
                },
            };
            self.waiting = Some(wait.clone());
            Ok(FesDriverEffect::Waiting { wait, handler_local: false })
        }
    }

    fn complete_wait(&mut self, frame: u64, wait: &FesDriverWait) -> Result<bool> {
        let ready = match wait {
            FesDriverWait::Frames { until } => frame >= *until,
            FesDriverWait::Frame { after } => frame > *after,
            FesDriverWait::Control { kind, arguments, .. } if kind == b"frame" => {
                let value = arguments.first()
                    .map(|value| -> Result<i32> { Ok(std::str::from_utf8(value)?.parse()?) })
                    .transpose()?.unwrap_or(0) as u32;
                (frame as u32) >= value
            }
            FesDriverWait::Control { kind, arguments, entered } if kind == b"privateflag" => {
                let index = arguments.first()
                    .map(|value| -> Result<i32> { Ok(std::str::from_utf8(value)?.parse()?) })
                    .transpose()?.unwrap_or(0);
                frame > *entered && self.interpreter.local_values()
                    .get(index as usize).is_none_or(|value| *value != 0)
            }
            FesDriverWait::Control { .. } => false,
        };
        let (cancelled, finish_motion, reveal_text) = match wait {
            FesDriverWait::Frames { .. } => (self.keyskip_on && self.cancel_wait, true, false),
            FesDriverWait::Frame { .. } => (false, false, false),
            FesDriverWait::Control { kind, .. } if kind == b"privateflag" => (false, false, false),
            FesDriverWait::Control { kind, .. } if kind == b"frame" => {
                (self.keyskip_on && (self.cancel_wait || self.skip_input), true, false)
            }
            FesDriverWait::Control { kind, .. } if kind.is_empty() => {
                (self.keyskip_on && (self.cancel_wait || self.skip_input), true, false)
            }
            FesDriverWait::Control { kind, .. } => {
                (self.cancel_wait && (!self.skip_input || self.keyskip_ctrl || self.keyskip_on),
                    false, kind == b"string")
            }
        };
        if !ready && cancelled && !self.wait_cancel_announced {
            self.notices.push_back(FesDriverEffect::CancelWait {
                wait: wait.clone(), finish_motion, reveal_text,
            });
            self.wait_cancel_announced = true;
        }
        let requires_host = matches!(wait,
            FesDriverWait::Control { kind, .. } if kind.is_empty() || kind == b"string");
        if ready || (cancelled && !requires_host) {
            self.waiting = None;
            self.adapter.status = FesRuntimeStatus::Running;
            self.interpreter.resume();
            if !self.keyskip_cont && !self.skip_input {
                self.cancel_wait = false;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn command(&mut self, command: FesDriverCommand, frame: u64) -> Result<Option<FesDriverEffect>> {
        if let Some(effect) = self.main_flow_command(&command)? { return Ok(effect); }
        let arguments = &command.parameters;
        let name = arguments.first().map_or(&[][..], Vec::as_slice);
        match name {
            b"decode" | b"baseimg" | b"config_open" if self.handler.is_some() => {
                self.defer(FesDriverEffect::Command(command))?;
                self.finish_handler(false).map(Some)
            }
            b"execkcs" => {
                let Some(name) = arguments.get(1).filter(|name| !name.is_empty()) else {
                    return Ok(None);
                };
                Ok(Some(FesDriverEffect::ExecKcs {
                    name: name.clone(), external: self.external.clone(),
                }))
            }
            b"execfes" => {
                let Some(name) = arguments.get(1).filter(|name| !name.is_empty()) else {
                    return Ok(None);
                };
                let mut parameters = [0; 16];
                for (slot, argument) in parameters.iter_mut().zip(arguments.iter().skip(2)) {
                    *slot = self.number(argument)?;
                }
                Ok(Some(FesDriverEffect::ExecFes {
                    name: name.clone(), parameters, external: self.external.clone(),
                }))
            }
            b"endkcs" | b"endfes" => {
                self.set_child_exit_code(-1);
                let effect = FesDriverEffect::EndChild(if name == b"endkcs" {
                    FesChildKind::Kcs
                } else {
                    FesChildKind::Fes
                });
                if self.handler.is_some() {
                    self.defer(effect)?;
                    return Ok(Some(self.finish_handler(false)?));
                }
                self.adapter.status = FesRuntimeStatus::FrameBoundary;
                self.waiting = Some(FesDriverWait::Frame { after: frame });
                Ok(Some(effect))
            }
            b"send" => {
                if arguments.len() < 2 {
                    return Ok(None);
                }
                let mut values = [0, 0, 0, 1];
                for (slot, argument) in values.iter_mut().zip(arguments.iter().skip(1)) {
                    *slot = self.number(argument)? as u32;
                }
                let effect = FesDriverEffect::Send {
                    message: [self.task_id, values[0], values[1], values[2]],
                    parent: values[3] != 0,
                };
                if self.handler.is_some() {
                    self.defer(effect)?;
                    Ok(None)
                } else {
                    Ok(Some(effect))
                }
            }
            b"react" => {
                let effect = FesDriverEffect::React { target: self.focus.clone() };
                if self.handler.is_some() {
                    self.defer(effect)?;
                    Ok(None)
                } else {
                    Ok(Some(effect))
                }
            }
            b"keyskip" => {
                self.keyskip_on = true;
                self.keyskip_ctrl = false;
                self.keyskip_cont = false;
                let mut cursor = 1;
                while let Some(option) = arguments.get(cursor) {
                    if option.eq_ignore_ascii_case(b"on") {
                        self.keyskip_on = true;
                    } else if option.eq_ignore_ascii_case(b"off") {
                        self.keyskip_on = false;
                    } else if option.eq_ignore_ascii_case(b"ctrl") {
                        self.keyskip_ctrl = true;
                    } else if option.eq_ignore_ascii_case(b"cont") {
                        self.keyskip_cont = true;
                    } else if option.eq_ignore_ascii_case(b"force") {
                        cursor += 1;
                        self.forced_skip = match arguments.get(cursor) {
                            Some(value) => self.number(value)? != 0,
                            None => false,
                        };
                        if self.forced_skip {
                            self.cancel_wait = true;
                            self.skip_input = true;
                            self.set_named_value("_CLICK_L_", 0, -1);
                        }
                    } else {
                        self.keyskip_on = self.number(option)? != 0;
                    }
                    cursor += 1;
                }
                Ok(Some(FesDriverEffect::Command(command)))
            }
            _ => Ok(Some(FesDriverEffect::Command(command))),
        }
    }

    fn defer(&mut self, effect: FesDriverEffect) -> Result<()> {
        if self.deferred.len() >= MAX_NOTIFICATIONS {
            bail!("FES deferred notification queue exceeds {MAX_NOTIFICATIONS} entries");
        }
        self.deferred.push_back(effect);
        Ok(())
    }

    fn begin_input(&mut self, input: FesDriverInput) -> Result<Option<FesDriverEffect>> {
        self.begin_input_mode(input, true)
    }

    fn begin_input_mode(
        &mut self, input: FesDriverInput, button_defaults: bool,
    ) -> Result<Option<FesDriverEffect>> {
        self.wait_cancel_announced = false;
        if input.event.is_none() && input.code == 2
            && let Some(target) = input.target.as_ref()
            && !self.focus.as_ref()
                .is_some_and(|focus| focus.name == target.name && focus.bank == target.bank)
        {
            if self.inputs.len() + 2 > MAX_INPUTS {
                bail!("FES focus notification queue exceeds {MAX_INPUTS} entries");
            }
            let mut push = input.clone();
            push.event = Some(b"PUSH_L".to_vec());
            self.inputs.push_front(push);
            self.inputs.push_front(FesDriverInput {
                code: 0, target: Some(target.clone()), event: Some(b"FOCUS".to_vec()),
            });
            if let Some(previous) = self.focus.clone() {
                return self.begin_input(FesDriverInput {
                    code: 0, target: Some(previous), event: Some(b"UNFOCUS".to_vec()),
                });
            }
            let focus = self.inputs.pop_front().expect("focus was just queued");
            return self.begin_input(focus);
        }
        let value = input.target.as_ref().map_or(-1, |target| target.value);
        let named = match input.code {
            2 | 19 => Some(("_CLICK_L_", value)),
            3 => Some(("_CLICK_R_", value)),
            4 | 20 => Some(("_CLICK_L_", 0)),
            5 => Some(("_CLICK_R_", 0)),
            6 => Some(("_SCRL_U_", value)),
            7 => Some(("_SCRL_D_", value)),
            8 => Some(("_SCRL_P_", value)),
            9 => Some(("_PAGE_U_", value)),
            10 => Some(("_PAGE_D_", value)),
            11 => Some(("_PUSH_UP_", value)),
            12 => Some(("_PUSH_DOWN_", value)),
            13 => Some(("_PUSH_LEFT_", value)),
            14 => Some(("_PUSH_RIGHT_", value)),
            15 => Some(("_RELEASE_UP_", value)),
            16 => Some(("_RELEASE_DOWN_", value)),
            17 => Some(("_RELEASE_LEFT_", value)),
            18 => Some(("_RELEASE_RIGHT_", value)),
            21 => Some(("_TOUCH_", value)),
            _ => None,
        };
        if let Some((name, value)) = named {
            self.set_named_value(name, 0, value);
        }
        match input.code {
            2 => self.cancel_wait = true,
            19 => { self.raw_input = true; self.skip_input = true; self.cancel_wait = true; }
            20 => { self.raw_input = false; self.skip_input = false; }
            _ => {}
        }
        let Some(mut target) = input.target else {
            return Ok(None);
        };
        if button_defaults && target.default_group.is_none() {
            let default = self.adapter.object_bank(&target.name, target.bank as usize)
                .and_then(|bank| bank.initializers.iter().rev()
                    .find(|property| property.column.eq_ignore_ascii_case(b"DEFAULT")))
                .map(|property| property.value.clone());
            if let Some(default) = default {
                let mut source = b"default ".to_vec();
                source.extend_from_slice(&default);
                target.default_group = self.interpreter.program
                    .command_parameters(&source, &self.expression.strings)?
                    .get(1).filter(|name| !name.is_empty()).cloned();
            }
        }
        let event = input.event.or_else(|| match input.code {
            2 => Some(b"PUSH_L".to_vec()),
            3 => Some(b"PUSH_R".to_vec()),
            4 => Some(b"RELEASE_L".to_vec()),
            5 => Some(b"RELEASE_R".to_vec()),
            _ => None,
        });
        let Some(event) = event else {
            return Ok(None);
        };
        if event == b"FOCUS" {
            self.focus = Some(target.clone());
        } else if event == b"UNFOCUS" && self.focus.as_ref()
            .is_some_and(|focus| focus.name == target.name && focus.bank == target.bank)
        {
            self.focus = None;
        }
        let object_header = event_header(&target.name, &event);
        let object_exists = self.interpreter.program.block(&object_header).is_some();
        let default_header = target.default_group.as_ref().map(|group| event_header(group, &event))
            .filter(|header| self.interpreter.program.block(header).is_some());
        if !object_exists && default_header.is_none() {
            return Ok(Some(FesDriverEffect::ObjectEvent {
                target, event,
                phase: FesObjectEventPhase::Complete { handled: false, transferred: false },
            }));
        }
        let phase = if default_header.is_some() { HandlerPhase::Default } else { HandlerPhase::Object };
        let saved_current = self.expression.current;
        self.expression.current = target.bank;
        let handler = Handler {
            target: target.clone(), event: event.clone(), object_header: object_header.clone(),
            object_exists, phase, depth: self.interpreter.call_depth(),
            saved_control: self.interpreter.control_position(),
            saved_status: self.adapter.status.clone(),
            saved_wait: self.waiting.take(), saved_current,
        };
        let header = default_header.as_deref().unwrap_or(&object_header);
        if !self.interpreter.enter_block(header) {
            bail!("FES event block disappeared before dispatch");
        }
        self.adapter.status = FesRuntimeStatus::Running;
        self.handler = Some(handler);
        Ok(Some(FesDriverEffect::ObjectEvent {
            target, event,
            phase: if phase == HandlerPhase::Default {
                FesObjectEventPhase::BeginDefault
            } else {
                FesObjectEventPhase::BeginObject
            },
        }))
    }

    fn check_handler_boundary(&mut self) -> Result<Option<FesDriverEffect>> {
        let handler = self.handler.as_ref().expect("handler checked by caller");
        if handler.phase == HandlerPhase::Object
            && self.interpreter.call_depth() == handler.depth
            && self.interpreter.current_block() != handler.object_header.as_slice()
        {
            return Ok(Some(self.finish_handler(true)?));
        }
        let preserves_nested_returns = handler.phase == HandlerPhase::Object
            || (handler.phase == HandlerPhase::System
                && self.rebellions_message_fes_source
                && handler.target.name.is_empty()
                && matches!(
                    (handler.event.as_slice(), self.interpreter.current_block()),
                    (b"MES_SHOW" | b"MES_WAITINPUT", b"SETBUTTON" | b"_SETBUTTON")
                        | (b"MES_SHOW", b"PDA_ON")
                ));
        if self.interpreter.at_block_end()
            && (!preserves_nested_returns || self.interpreter.call_depth() <= handler.depth)
        {
            return Ok(Some(self.finish_handler(false)?));
        }
        Ok(None)
    }

    fn finish_handler(&mut self, transferred: bool) -> Result<FesDriverEffect> {
        let mut handler = self.handler.take()
            .ok_or_else(|| anyhow::anyhow!("FES handler completion without an active handler"))?;
        if handler.phase == HandlerPhase::Default && handler.object_exists {
            handler.phase = HandlerPhase::Object;
            if !self.interpreter.enter_block(&handler.object_header) {
                bail!("FES object event block disappeared");
            }
            self.adapter.status = FesRuntimeStatus::Running;
            let effect = FesDriverEffect::ObjectEvent {
                target: handler.target.clone(), event: handler.event.clone(),
                phase: FesObjectEventPhase::BeginObject,
            };
            self.handler = Some(handler);
            return Ok(effect);
        }
        if transferred {
            self.interpreter.resume();
            self.waiting = None;
            self.adapter.status = FesRuntimeStatus::Running;
        } else {
            self.interpreter.restore_control_position(handler.saved_control)?;
            self.waiting = handler.saved_wait;
            self.adapter.status = handler.saved_status;
        }
        self.expression.current = handler.saved_current;
        if handler.phase == HandlerPhase::System {
            return Ok(FesDriverEffect::SystemBlock { name: handler.event, complete: true });
        }
        Ok(FesDriverEffect::ObjectEvent {
            target: handler.target, event: handler.event,
            phase: FesObjectEventPhase::Complete {
                handled: handler.object_exists, transferred,
            },
        })
    }

    fn begin_system(&mut self, request: SystemBlockRequest) -> Result<FesDriverEffect> {
        let SystemBlockRequest { name, header, object } = request;
        let current = object.as_ref().map_or(0, |target| target.bank);
        let handler = Handler {
            target: object.unwrap_or_else(|| FesDriverTarget {
                name: Vec::new(), bank: 0, default_group: None, value: -1,
            }),
            event: name.clone(), object_header: header.clone(), object_exists: false,
            phase: HandlerPhase::System, depth: self.interpreter.call_depth(),
            saved_control: self.interpreter.control_position(),
            saved_status: self.adapter.status.clone(), saved_wait: self.waiting.take(),
            saved_current: self.expression.current,
        };
        if !self.interpreter.enter_block(&header) {
            bail!("FES system block disappeared before dispatch");
        }
        self.expression.current = current;
        self.adapter.status = FesRuntimeStatus::Running;
        self.handler = Some(handler);
        Ok(FesDriverEffect::SystemBlock { name, complete: false })
    }

    fn clear_transient_input(&mut self) {
        for name in [
            "_CLICK_R_", "_SCRL_U_", "_SCRL_D_", "_SCRL_P_", "_PAGE_U_", "_PAGE_D_",
            "_PUSH_UP_", "_PUSH_DOWN_", "_PUSH_LEFT_", "_PUSH_RIGHT_",
            "_RELEASE_UP_", "_RELEASE_DOWN_", "_RELEASE_LEFT_", "_RELEASE_RIGHT_", "_TOUCH_",
        ] {
            self.set_named_value(name, 0, 0);
        }
    }

    pub(super) fn finish_left_input_frame(&mut self) {
        if !self.raw_input && !self.skip_input && !self.keyskip_cont {
            self.set_named_value("_CLICK_L_", 0, 0);
            self.cancel_wait = false;
        }
    }
}

fn is_declaration(event: &FesEvent) -> bool {
    matches!(event, FesEvent::Declarations(_) | FesEvent::KeyBlocks(_)
        | FesEvent::CreateObject(_) | FesEvent::InitializeObject(_)
        | FesEvent::KeyBlockBinding(_) | FesEvent::Subcommands(_))
}

fn event_header(group: &[u8], event: &[u8]) -> Vec<u8> {
    let mut header = group.to_vec();
    header.push(b'.');
    header.extend_from_slice(event);
    header
}

impl super::FesRuntime {
    /// Build a driver input from the existing pixel hit test. Replace the old
    /// record_input/start_object_event path with driver.on_input(this value).
    pub(crate) fn input_for_driver(
        &self,
        code: u32,
        pointer: Option<cs2_renderer::Point>,
    ) -> FesDriverInput {
        let target = pointer.and_then(|point| self.hit_test_button(point))
            .or_else(|| self.focused_object.clone())
            .map(|target| {
                let (name, bank) = super::split_fes_object_bank(&target);
                FesDriverTarget {
                    name: name.as_bytes().to_vec(), bank,
                    default_group: None,
                    value: -1,
                }
            });
        FesDriverInput { code, target, event: None }
    }
}
