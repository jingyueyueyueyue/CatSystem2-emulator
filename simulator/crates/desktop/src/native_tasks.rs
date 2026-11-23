//! Ordinary object execution bridge. The player calls update_ordinary_children
//! before resuming the root VM; merely attaching a VM does not schedule it.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cs2_runtime::language::{LanguageNotificationCache, LanguageState};
use cs2_runtime::{OrdinaryTask, TaskClock, TaskCommand, TaskMessage};
use cs2_script::{KcsProgram, NativeReturn, Result, ResumeStep, SharedExternalStorage, Vm};

use super::{
    LoggingHost, LogicEvent, SceneNode, cp932_to_utf8, expand_kcs_format, native_state_error,
    read_u32,
};

#[path = "task_commands.rs"]
mod task_commands;
pub(crate) use task_commands::SharedNumericBackupSnapshot;
#[path = "task_commands2.rs"]
mod task_commands2;
#[path = "task_controls.rs"]
mod task_controls;
pub(crate) use task_controls::checkpoint::PreparedLowerCommands;
#[path = "render_tasks.rs"]
mod render_tasks;
#[path = "export_tasks.rs"]
mod export_tasks;
#[path = "message_list_tasks.rs"]
mod message_list_tasks;
#[path = "scene_tool_tasks.rs"]
mod scene_tool_tasks;
pub(super) use task_commands2::{OwnedTaskBuffers, TaskBufferReceiver};
pub(crate) use task_commands2::PreparedBufferedCommands;
pub(super) use render_tasks::RenderTaskStore;
#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) use render_tasks::WipeStep;
pub(super) use export_tasks::ExportTaskStore;
pub(crate) use export_tasks::{ExportSoundPrompt, ExportSoundQuestion};
pub(crate) use render_tasks::RenderTaskOperation;
pub use message_list_tasks::MessageListView;
pub(super) use message_list_tasks::MessageListStore;
pub use scene_tool_tasks::SceneToolView;
pub(super) use scene_tool_tasks::SceneToolStore;
pub(crate) use scene_tool_tasks::PreparedSceneMessageMemory;
#[path = "native_object_state.rs"]
mod native_object_state;
pub(crate) use native_object_state::PreparedImageExportState;
#[path = "message_input_state.rs"]
mod message_input_state;
pub(crate) use message_input_state::PreparedInputSamplingState;
#[path = "object_graph.rs"]
mod object_graph;
pub(crate) use object_graph::ObjectGraphReceiver;
#[path = "hg_window_native.rs"]
mod hg_window_native;
pub(crate) use hg_window_native::HgWindowNative;
#[path = "state_tree_native.rs"]
mod state_tree_native;

#[derive(Default)]
pub(super) struct NativeTaskStore {
    graph: object_graph::GraphRequests,
    hg_window_abi: Option<hg_window_native::HgWindowAbi>,
    original_state_tree_abi: Option<state_tree_native::OriginalStateTreeAbi>,
    model_state_trees: BTreeMap<u32, (u32, cs2_formats::StateTree)>,
    pub(super) fes: super::fes_tasks::FesTaskStore,
    pub(super) javascript: super::javascript_native::JavaScriptStore,
    pub(super) render: RenderTaskStore,
    pub(super) exports: ExportTaskStore,
    pub(super) message_lists: MessageListStore,
    pub(super) scene_tools: SceneToolStore,
    pub(super) records: BTreeMap<u32, Record>,
    reserved: BTreeSet<u32>,
    traversal: Option<Traversal>,
    factory_external: Option<SharedExternalStorage>,
    released: VecDeque<u32>,
    external_tasks: BTreeSet<u32>,
    pub(super) root_dispatch: Option<RootTaskDispatch>,
    pub(super) executing_external: Option<u32>,
    direct_release: Option<Traversal>,
    shared_areas: task_commands::SharedValueAreas,
    message_traversals: BTreeMap<u32, MessageTraversal>,
    pub(super) synchronous_callers: BTreeMap<u32, Vm>,
    buffered_commands: task_commands2::BufferedTaskCommands,
    lower_commands: BTreeMap<u32, task_controls::LowerPending>,
}

#[derive(Clone, Copy, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct RootTaskDispatch {
    pub(super) target: u32,
    pub(super) message: TaskMessage,
    completed: bool,
    output: u32,
    reply: TaskMessage,
}

impl NativeTaskStore {
    pub(crate) fn checkpoint_task_kinds(&self) -> BTreeMap<u32, u32> {
        self.records.iter().map(|(&id, record)| (id, record.kind)).collect()
    }

    pub(crate) fn checkpoint_external_binding(&self, id: u32) -> Option<SharedExternalStorage> {
        self.records.get(&id)?.external.clone()
    }

    pub(crate) fn bind_fes_task_external(&mut self, id: u32, external: SharedExternalStorage) -> Result<()> {
        let record = self.records.get_mut(&id)
            .filter(|record| record.kind == 34)
            .ok_or_else(|| native_state_error("FES external binding needs its actual type34 record"))?;
        record.external = Some(external);
        Ok(())
    }

    pub(crate) fn checkpoint_message_owners(&self) -> BTreeMap<u32, u32> {
        self.records.iter().filter_map(|(&id, record)| record.message_owner.map(|owner| (id, owner))).collect()
    }

    pub(crate) fn validate_checkpoint_message_owners(&self, owners: &BTreeMap<u32, u32>) -> anyhow::Result<()> {
        for (&target, &caller) in owners {
            if !self.records.contains_key(&caller) || !self.records.get(&target)
                .and_then(|record| record.ordinary.as_ref()).is_some_and(OrdinaryTask::has_pending_command)
            { anyhow::bail!("saved synchronous task message lost its caller or pending command"); }
        }
        Ok(())
    }

    pub(crate) fn commit_checkpoint_message_owners(&mut self, owners: BTreeMap<u32, u32>) {
        for (id, owner) in owners { self.records.get_mut(&id).unwrap().message_owner = Some(owner); }
    }

    pub(super) fn register_direct_fes_task(
        &mut self,
        id: u32,
        now: u32,
        context: &[u8],
        external: SharedExternalStorage,
        languages: &LanguageState,
    ) -> Result<()> {
        if self.records.get(&id).is_some_and(|record| record.active) {
            return Err(native_state_error(
                "cannot replace an executing root with a direct FES entry",
            ));
        }
        if context.len() > 31 || context.contains(&0) {
            return Err(native_state_error("direct FES context exceeds 31 bytes"));
        }
        let mut record = Record::new(id, 34, languages);
        record.context = context.to_vec();
        record.external = Some(external);
        record.clock.current = now;
        record.clock.last_update = now;
        record.shared_area = self.shared_areas.host_binding();
        self.retire_scene_message_vm(id)?;
        self.records.insert(id, record);
        self.external_tasks.remove(&id);
        Ok(())
    }

    pub(super) fn pending_root_dispatch(&self) -> Option<(u32, TaskMessage)> {
        self.root_dispatch
            .filter(|request| !request.completed)
            .map(|request| (request.target, request.message))
    }

    pub(super) fn complete_root_dispatch(
        &mut self,
        target: u32,
        message: TaskMessage,
    ) -> Result<()> {
        let request = self
            .root_dispatch
            .as_mut()
            .ok_or_else(|| native_state_error("no pending FES root dispatch"))?;
        if request.target != target || request.message != message {
            return Err(native_state_error(
                "FES root dispatch acknowledgement changed",
            ));
        }
        request.completed = true;
        Ok(())
    }

    pub(super) fn complete_root_dispatch_step(
        &mut self, target: u32, message: TaskMessage, reply: TaskMessage,
        step: Option<ResumeStep>,
    ) -> Result<()> {
        self.complete_root_dispatch(target, message)?;
        self.root_dispatch.as_mut().unwrap().reply = reply;
        self.root_dispatch.as_mut().unwrap().output = match step {
            Some(ResumeStep::Completed { active, .. }) => u32::from(active),
            Some(ResumeStep::Paused | ResumeStep::Yield) => 1,
            None => 0,
        };
        Ok(())
    }

    pub(super) fn task_context(&self, id: u32) -> Option<&[u8]> {
        self.records
            .get(&id)
            .map(|record| record.context.as_slice())
    }

    /// Global number-index occupancy. Unregister/deletion do not free an id;
    /// a released record stays reserved until the enclosing root collects it.
    /// Zero is unavailable to the allocator even before an external VM loads.
    pub(super) fn task_number_reserved_or_live(&self, number: u32) -> bool {
        number == 0 || self.records.contains_key(&number)
            || self.reserved.contains(&number) || self.external_tasks.contains(&number)
    }

    /// Root command33 queries keys in the root's descendant-number index.
    /// It has no "root/self" shortcut. The retained reservation set is this
    /// bridge's index, including null-value keys until root collection;
    /// metadata-only nodes, external VMs and allocation exclusions are not.
    pub(super) fn root_number_index_contains(&self, number: u32) -> bool {
        self.reserved.contains(&number)
    }

    pub(super) fn write_fes_context(&mut self, id: u32, context: &[u8]) -> Result<()> {
        if context.len() > 31 || context.contains(&0) {
            return Err(native_state_error(
                "FES creation context exceeds its 31-byte field",
            ));
        }
        let record = self
            .records
            .get_mut(&id)
            .ok_or_else(|| native_state_error("FES creation context target is absent"))?;
        if record.kind != 34 {
            return Err(native_state_error(
                "FES creation context target has a different type",
            ));
        }
        record.context = context.to_vec();
        Ok(())
    }

    pub(super) fn task_external(&self, id: u32) -> Option<SharedExternalStorage> {
        self.records
            .get(&id)
            .and_then(|record| record.external.clone())
    }

    /// JavaScript command522 aliases the caller's existing numeric/external
    /// bindings. No pointer-sized value or copied numeric vector is exposed.
    pub(super) fn bind_javascript_context(
        &mut self, target: u32, sender: u32, external: SharedExternalStorage,
    ) -> Result<()> {
        let source = self.records.get(&sender)
            .ok_or_else(|| native_state_error("JavaScript context source task is absent"))?;
        let area = source.shared_area;
        let target = self.records.get_mut(&target)
            .filter(|record| record.kind == 55 && record.deletion == 0)
            .ok_or_else(|| native_state_error("JavaScript context target is unavailable"))?;
        target.shared_area = area;
        target.external = Some(external);
        Ok(())
    }

    pub(super) fn task_clock(&self, id: u32) -> Option<TaskClock> {
        self.records.get(&id).map(|record| record.clock.clone())
    }

    pub(super) fn task_is_registered(&self, id: u32) -> bool {
        self.records.get(&id).is_none_or(|record| record.registered)
    }

    pub(super) fn task_property(&self, id: u32) -> Option<u32> {
        self.records
            .get(&id)
            .filter(|record| record.deletion == 0)
            .map(|record| record.property)
    }

    pub(super) fn write_task_property(&mut self, id: u32, property: u32) -> Result<()> {
        let record = self
            .records
            .get_mut(&id)
            .ok_or_else(|| native_state_error("property target has no registered task state"))?;
        record.property = property;
        if let Some(task) = record.ordinary.as_mut() {
            task.property = property;
        }
        Ok(())
    }

    pub(super) fn set_task_clock(&mut self, id: u32, now: u32) -> Result<()> {
        self.records
            .get_mut(&id)
            .ok_or_else(|| native_state_error("clock target has no registered task state"))?
            .clock
            .update(now);
        Ok(())
    }

    /// Platform services owned outside this module release their objects
    /// using these ids. Each completed task release is emitted exactly once.
    pub(super) fn take_released_task_ids(&mut self) -> Vec<u32> {
        self.released.drain(..).collect()
    }

    pub(super) fn set_factory_external(&mut self, external: SharedExternalStorage) {
        self.factory_external = Some(external);
    }

    /// Registers metadata for a VM (such as the root) owned by the player.
    /// Existing lifetime caches and pending messages are retained.
    pub(super) fn register_external_task(
        &mut self,
        id: u32,
        kind: u32,
        languages: &LanguageState,
    ) -> Result<()> {
        if let Some(record) = self.records.get(&id) {
            if record.kind != kind {
                return Err(native_state_error(
                    "external task kind conflicts with registered object",
                ));
            }
            self.scene_tools.check_vm_replacement(id, &record.queued)?;
            self.scene_tools.release_vm(id);
            self.external_tasks.insert(id);
            return Ok(());
        }
        self.records.insert(id, Record::new(id, kind, languages));
        self.external_tasks.insert(id);
        Ok(())
    }

    pub(super) fn message_task_state(&self, id: u32) -> Option<(u32, u32, u32)> {
        self.records
            .get(&id)
            .map(|record| (record.kind, record.deletion, record.clock.elapsed()))
    }

    /// Frame-end configuration notification never runs ordinary script code.
    /// The caller supplies has_program for a root VM held outside this store.
    pub(super) fn notify_configuration_command(
        &mut self,
        target: u32,
        command: TaskCommand,
        languages: &LanguageState,
        has_program: bool,
    ) -> Result<()> {
        if command.code != 19 || command.target as u32 != target {
            return Err(native_state_error(
                "configuration adapter accepts only the addressed command 19",
            ));
        }
        let Some(record) = self.records.get_mut(&target) else {
            return Err(native_state_error(
                "configuration target has no registered task state",
            ));
        };
        if record.deletion != 0 {
            return Ok(());
        }
        if !ordinary_kind(record.kind) {
            return Err(native_state_error(
                "configuration command 19 requires this object's specific refresh service",
            ));
        }
        record
            .queued
            .extend(record.language_notifications.configuration_messages(
                command.sender as u32,
                languages,
                has_program || record.had_vm,
            ));
        Ok(())
    }
}

pub(super) struct Record {
    pub(super) kind: u32,
    clock: TaskClock,
    deletion: u32,
    ordinary: Option<OrdinaryTask>,
    active: bool,
    queued: VecDeque<TaskMessage>,
    context: Vec<u8>,
    had_vm: bool,
    property: u32,
    external: Option<SharedExternalStorage>,
    language_notifications: LanguageNotificationCache,
    loading: Option<OrdinaryLoad>,
    pub(super) shared_area: Option<u64>,
    message_owner: Option<u32>,
    registered: bool,
}

struct OrdinaryLoad {
    name: Vec<u8>,
    initialize: u32,
    external: SharedExternalStorage,
    installed: bool,
    initialized: bool,
    system_time_ms: u32,
}

impl Record {
    fn new(id: u32, kind: u32, languages: &LanguageState) -> Self {
        Self {
            kind,
            clock: TaskClock::default(),
            deletion: 0,
            // Ordinary type zero is created unloaded. Its display name is
            // not a program filename; command 161/native 143 loads the VM.
            ordinary: ordinary_kind(kind).then(|| OrdinaryTask::new(id as i32, None)),
            active: false,
            queued: VecDeque::new(),
            context: Vec::new(),
            had_vm: false,
            property: u32::MAX,
            external: None,
            language_notifications: LanguageNotificationCache::new(languages),
            loading: None,
            shared_area: None,
            message_owner: None,
            registered: true,
        }
    }
}

#[derive(Clone, Copy, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum Phase {
    Begin,
    Children(Option<i32>),
    Body,
    Unregister,
    NotifyChildren(Option<i32>),
    NotifySelf,
    DestroyChildren(Option<i32>),
    Release,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Traversal {
    root: u32,
    now: u32,
    increment: u32,
    stack: Vec<(u32, Phase)>,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct NativeSubtreeContinuation(Traversal);

impl NativeSubtreeContinuation {
    pub(crate) fn validate_fes_checkpoint(&self, owner: u32, records: &BTreeMap<u32, u32>) -> Result<()> {
        if self.0.root != owner || self.0.stack.len() > 256
            || self.0.stack.iter().any(|(id, _)| !records.contains_key(id))
        { return Err(native_state_error("FES child traversal checkpoint lost its task owners")); }
        Ok(())
    }
}

#[derive(Clone, Copy, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum MessagePhase {
    Receiver,
    Children(Option<i32>),
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct MessageTraversal {
    target: u32,
    packet: TaskMessage,
    queued: bool,
    output: u32,
    stack: Vec<(u32, MessagePhase)>,
}

pub(super) struct TaskValueScope {
    previous_area: u64,
    swapped: bool,
}

fn ordinary_kind(kind: u32) -> bool {
    // v4.01 main.kcs creates type 12 at 0x1b21, saves its id in G0x58,
    // then loads that same object through native 143 at 0x1b4a.
    matches!(kind, 0 | 12)
}

impl LoggingHost<'_> {
    /// Keep this aligned with create_task_object's allocation exclusions.
    /// External integrations can publish a VM/node before its record exists.
    pub(super) fn task_number_reserved_or_live(&self, number: u32) -> bool {
        number == self.current_task_id
            || self.data_services.tasks.task_number_reserved_or_live(number)
            || self.task_parents.contains_key(&number)
            || self.scene_nodes.contains_key(&number)
            || self.task_vms.contains_key(&number)
    }

    pub(super) fn release_direct_fes_entry(&mut self, root: u32, budget: usize) -> Result<bool> {
        if self.data_services.tasks.traversal.is_some() {
            return Err(native_state_error("complete the current task-tree frame before retiring its root"));
        }
        let mut traversal = match self.data_services.tasks.direct_release.take() {
            Some(traversal) if traversal.root == root => traversal,
            Some(traversal) => {
                self.data_services.tasks.direct_release = Some(traversal);
                return Err(native_state_error("another direct FES root is being released"));
            }
            None => {
                if !self.data_services.tasks.fes.is_direct_entry(root) { return Ok(true); }
                Traversal { root, now: self.task_time, increment: 0,
                    stack: vec![(root, Phase::Unregister)] }
            }
        };
        let result = self.drive_ordinary_children(&mut traversal, budget);
        if !matches!(result, Ok(true)) {
            self.data_services.tasks.direct_release = Some(traversal);
        } else {
            self.data_services.tasks.reserved.retain(|id| self.task_parents.contains_key(id));
        }
        result
    }

    /// External root execution stays owned by the player. These wrappers
    /// expose its real active flag during nested native/FES notifications and
    /// restore that flag on every success, budget yield, or error.
    pub(super) fn dispatch_external_task_message(
        &mut self,
        vm: &mut Vm,
        message: &mut TaskMessage,
        budget: usize,
    ) -> Result<Option<ResumeStep>> {
        self.with_external_execution(vm, |host, vm| vm.dispatch_message(message, host, budget))
    }

    pub(super) fn resume_external_task(
        &mut self,
        vm: &mut Vm,
        budget: usize,
    ) -> Result<ResumeStep> {
        self.with_external_execution(vm, |host, vm| vm.resume(host, budget))
    }

    fn with_external_execution<T>(
        &mut self,
        vm: &mut Vm,
        call: impl FnOnce(&mut Self, &mut Vm) -> Result<T>,
    ) -> Result<T> {
        let id = self.current_task_id;
        self.ensure_task_record(id)?;
        let values = self.enter_task_values(id)?;
        let record = self.data_services.tasks.records.get_mut(&id).unwrap();
        let previous_active = record.active;
        record.active = true;
        record.had_vm = vm.program().is_some();
        record.external = Some(vm.external_binding());
        let previous = self.data_services.tasks.executing_external.replace(id);
        let saved_clock = (self.task_time, self.task_origin);
        let clock = self.data_services.tasks.records[&id].clock.clone();
        self.task_time = clock.current;
        self.task_origin = clock.origin;
        let result = call(self, vm);
        (self.task_time, self.task_origin) = saved_clock;
        self.leave_task_values(values);
        self.data_services.tasks.executing_external = previous;
        if let Some(record) = self.data_services.tasks.records.get_mut(&id) {
            record.active = previous_active;
            record.external = Some(vm.external_binding());
            record.had_vm = vm.program().is_some();
        }
        result
    }

    /// A synchronous FES start owns a separate child traversal. It must not
    /// replace the outer frame's pending root traversal when it yields.
    pub(super) fn update_detached_task_children(
        &mut self,
        root: u32,
        now: u32,
        increment: u32,
        pending: &mut Option<NativeSubtreeContinuation>,
        budget: usize,
    ) -> Result<bool> {
        let mut traversal = pending
            .take()
            .map(|pending| pending.0)
            .unwrap_or(Traversal {
                root,
                now,
                increment,
                stack: vec![(root, Phase::Children(None))],
            });
        let result = self.drive_ordinary_children(&mut traversal, budget);
        if !matches!(result, Ok(true)) {
            *pending = Some(NativeSubtreeContinuation(traversal));
        }
        result
    }

    /// FES send ignores the activity result, but delivery must actually enter
    /// the target VM. A root VM outside this store requires the platform hook.
    pub(super) fn send_fes_task_message(
        &mut self,
        target: u32,
        packet: TaskMessage,
    ) -> Result<bool> {
        if self.data_services.tasks.external_tasks.contains(&target)
            && !self.task_vms.contains_key(&target)
        {
            if self.data_services.tasks.executing_external == Some(target) {
                return Ok(true);
            }
            let mut packet = packet;
            return self.dispatch_root_message_request(target, &mut packet, &mut 0);
        }
        // Passive child updates can retain the root's host task number.
        // Only external execution above proves that its VM is reentrant;
        // an idle root must receive/acknowledge the child's actual packet.
        if target == self.current_task_id {
            return Ok(true);
        }
        let mut output = 0;
        self.message_branch(target, packet, false, &mut output)
    }

    pub(super) fn create_task_native(&mut self, arguments: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let kind = read_u32(arguments, 0)?;
        let parent = read_u32(arguments, 4)?;
        let name = expand_kcs_format(vm, &vm.read_string(read_u32(arguments, 8)?)?, 0)?;
        if !self.task_is_reachable(parent) {
            return Err(native_state_error("native 66 parent task is unavailable"));
        }
        if let Some(value) = self.data_services.native_profile.map(|profile| profile.pe_sha256)
            .or(self.data_services.css_task_pe_sha256)
            .and_then(|sha| cs2_runtime::native_factory::exact_factory_result(sha, kind))
        {
            return Ok(NativeReturn::Value(value));
        }
        let context = self
            .data_services
            .tasks
            .records
            .get(&self.current_task_id)
            .map(|record| record.context.clone())
            .unwrap_or_default();
        if self.data_services.tasks.factory_external.is_none() {
            self.data_services.tasks.factory_external = Some(vm.external_binding());
        }
        let id = self.create_task_object(kind, parent, &name, &context)?;
        Ok(NativeReturn::Value(id))
    }

    /// Direct FES factory entry. Context bytes are not a filename or format.
    pub(super) fn create_unloaded_ordinary_task(
        &mut self,
        parent: u32,
        context: &[u8],
    ) -> Result<u32> {
        self.create_task_object(0, parent, b"", context)
    }

    pub(super) fn create_unloaded_ordinary_task_shared(
        &mut self,
        parent: u32,
        context: &[u8],
        external: SharedExternalStorage,
    ) -> Result<u32> {
        let id = self.create_task_object(0, parent, b"", context)?;
        self.data_services
            .tasks
            .records
            .get_mut(&id)
            .unwrap()
            .external = Some(external);
        Ok(id)
    }

    pub(super) fn create_task_object(
        &mut self,
        kind: u32,
        parent: u32,
        name: &[u8],
        context: &[u8],
    ) -> Result<u32> {
        if !self.task_is_reachable(parent)
        {
            return Err(native_state_error("native 66 parent task is unavailable"));
        }
        let mut ancestors = BTreeSet::new();
        let mut ancestor = Some(parent);
        while let Some(id) = ancestor {
            if !ancestors.insert(id) || ancestors.len() >= 256 {
                return Err(native_state_error(
                    "native 66 invalid/cyclic task hierarchy",
                ));
            }
            ancestor = if id == 0 {
                None
            } else {
                self.task_parents.get(&id).copied()
            };
        }
        let name = &name[..name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(name.len())];
        let name = String::from_utf8(cp932_to_utf8(name)).expect("CP932 produces UTF-8");
        let mut id = 0u32;
        while id == 0
            || id == self.current_task_id
            || self.task_parents.contains_key(&id)
            || self.scene_nodes.contains_key(&id)
            || self.task_vms.contains_key(&id)
            || self.data_services.tasks.reserved.contains(&id)
        {
            id = id
                .checked_add(1)
                .filter(|&id| id <= i32::MAX as u32)
                .ok_or_else(|| native_state_error("native 66 task numbers exhausted"))?;
        }
        let mut record = Record::new(id, kind, &self.data_services.language.languages);
        record.external = self.data_services.tasks.factory_external.clone();
        // Creation context is raw bytes, independent of the display name.
        let context = &context[..context
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(context.len())];
        if context.len() > 31 {
            return Err(native_state_error(
                "native 66 creation context exceeds 31 bytes",
            ));
        }
        record.context = context.to_vec();
        if matches!(kind, 34 | 55) {
            record.shared_area = self.data_services.tasks.records.get(&parent)
                .and_then(|parent| parent.shared_area);
        }
        if kind == 13 {
            self.register_sound_task(id, context)?;
        }
        if kind == 54 {
            self.emote_register_task_hook(id, kind)?;
        }
        if kind == 61 {
            self.effekseer_register_task_hook(id)?;
        }
        if matches!(kind, 45 | 46) {
            self.data_services.tasks.exports.register(id, kind, self.save_root);
        }
        if kind == 47 {
            self.data_services.tasks.message_lists.register(id);
        }
        if matches!(kind, 32 | 33) {
            self.data_services.tasks.scene_tools.register(id, kind, self.current_task_id, self.text_encoding);
        }
        if kind == 44 { self.data_services.wallpapers.register(id); }
        if kind == 62 { self.register_tweet_task(id)?; }
        if matches!(kind, 50..=52) {
            self.register_retained_frame_effect(id, kind)?;
        } else if self.legacy_css_model(kind) {
            if self.css_task_sha()==Some(super::css_model::UTATE_SHA) {
                self.data_services.css_models.register_utate(id);
            } else { self.data_services.css_models.register(id); }
        } else if self.legacy_particle_canvas(kind) {
            self.data_services.particle_external.canvases.entry(id).or_default();
        } else if matches!(kind, 35..=39) {
            self.register_retained_render_task(id, kind)?;
        } else if RenderTaskStore::supports_kind(kind) {
            // Page/cell flips allocate their mesh/auxiliary surfaces from
            // the live graphics window extent. They do not copy a completed
            // back buffer in their constructors. The logical surface already
            // exists when startup creates them before the first presentation.
            let surface = if matches!(kind, 57 | 58) {
                self.back_buffer.or(self.canvas.as_deref())
            } else {
                self.back_buffer
            };
            self.data_services.tasks.render.register(id, kind,
                surface, self.data_services.native_profile)?;
        }
        self.data_services.planes.register_non_draw_media(
            id, kind, self.data_services.native_profile);
        if let Some(task) = record.ordinary.as_mut() {
            task.set_creation_context(context)
                .map_err(|_| native_state_error("native 66 invalid creation context"))?;
            record.context = task.creation_context().to_vec();
        }
        let fes_context = record.context.clone();
        let fes_external = record.external.clone().unwrap_or_default();
        self.data_services.tasks.records.insert(id, record);
        if kind == 18 { self.register_raster_task(id)?; }
        if kind == 34 {
            self.data_services
                .tasks
                .fes
                .register(id, fes_context, fes_external)?;
        }
        if kind == 55 {
            self.data_services.tasks.javascript.register(id);
        }
        self.data_services.tasks.reserved.insert(id);
        *self.next_task_id = (*self.next_task_id).max(id + 1);
        self.task_parents.insert(id, parent);
        self.scene_nodes
            .insert(id, SceneNode::new(id, kind, parent, name.clone()));
        self.events.push(LogicEvent {
            frame: self.frame,
            kind: "task-create".into(),
            detail: format!(
                "id={id} kind={kind} parent={parent} name={name:?} ordinary_unloaded={}",
                ordinary_kind(kind)
            ),
        });
        Ok(id)
    }

    /// Native 143 performs load/initialization against the object's previous
    /// binding, then binds the caller's shared storage without copying bytes.
    pub(super) fn load_ordinary_native(
        &mut self,
        arguments: &[u8],
        caller: &Vm,
    ) -> Result<NativeReturn> {
        let previous = self.data_services.tasks.executing_external;
        if previous.is_none() {
            self.data_services.tasks.executing_external = Some(self.current_task_id);
        }
        let result = self.load_ordinary_native_inner(arguments, caller);
        self.data_services.tasks.executing_external = previous;
        result
    }

    fn load_ordinary_native_inner(
        &mut self,
        arguments: &[u8],
        caller: &Vm,
    ) -> Result<NativeReturn> {
        let target = read_u32(arguments, 0)?;
        let name = expand_kcs_format(caller, &caller.read_string(read_u32(arguments, 4)?)?, 0)?;
        let initialize = read_u32(arguments, 8)?;
        if !self.load_ordinary_task_shared_step(
            target,
            &name,
            initialize,
            caller.external_binding(),
            4096,
        )? {
            return Ok(NativeReturn::Retry);
        }
        Ok(if initialize == 0 {
            NativeReturn::Continue
        } else {
            NativeReturn::YieldVoid
        })
    }

    /// Direct execkcs adapter; name is already expanded CP932 resource text.
    /// Byte-only compatibility entry for callers that request an empty area.
    pub(super) fn load_ordinary_task_bytes(
        &mut self,
        target: u32,
        name: &[u8],
        initialize: u32,
        external: &[u8],
    ) -> Result<()> {
        if !external.is_empty() {
            return Err(native_state_error(
                "native 143 nonempty shared external binding requires shared VM storage",
            ));
        }
        self.load_ordinary_task_shared(target, name, initialize, SharedExternalStorage::default())
    }

    /// FES must retain the factory binding supplied at construction and pass
    /// that binding here. Child rebindings do not change the FES's binding.
    pub(super) fn load_ordinary_task_shared(
        &mut self,
        target: u32,
        name: &[u8],
        initialize: u32,
        external: SharedExternalStorage,
    ) -> Result<()> {
        for _ in 0..256 {
            if self.load_ordinary_task_shared_step(
                target,
                name,
                initialize,
                external.clone(),
                4096,
            )? {
                return Ok(());
            }
        }
        Err(native_state_error(
            "ordinary initialization exceeded synchronous limit; continuation retained",
        ))
    }

    pub(super) fn load_ordinary_task_shared_step(
        &mut self,
        target: u32,
        name: &[u8],
        initialize: u32,
        external: SharedExternalStorage,
        budget: usize,
    ) -> Result<bool> {
        if target == self.current_task_id {
            return Err(native_state_error(
                "native 143 cannot replace the currently executing VM",
            ));
        }
        let name = &name[..name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(name.len())];
        if !self.task_is_reachable(target) {
            return Ok(true);
        }
        self.ensure_task_record(target)?;
        let record = &self.data_services.tasks.records[&target];
        if record.deletion != 0 || !ordinary_kind(record.kind) {
            return Ok(true);
        }
        if record.active || self.data_services.tasks.external_tasks.contains(&target) {
            return Err(native_state_error(
                "command 161 cannot replace an executing or externally owned root VM",
            ));
        }
        self.data_services.tasks.scene_tools.check_vm_replacement(
            target, record.queued.iter().chain(
                record.ordinary.iter().flat_map(OrdinaryTask::queued_messages),
            ),
        )?;
        let pending = self
            .data_services
            .tasks
            .records
            .get_mut(&target)
            .unwrap()
            .loading
            .take();
        let mut pending = match pending {
            Some(pending) => {
                if pending.name != name
                    || pending.initialize != initialize
                    || !pending.external.same_storage(&external)
                {
                    self.data_services
                        .tasks
                        .records
                        .get_mut(&target)
                        .unwrap()
                        .loading = Some(pending);
                    return Err(native_state_error("ordinary load continuation changed"));
                }
                pending
            }
            None => OrdinaryLoad {
                name: name.to_vec(),
                initialize,
                external,
                installed: false,
                initialized: false,
                system_time_ms: self.host_millis(),
            },
        };
        let result = self.drive_ordinary_load(target, &mut pending, budget);
        if !matches!(result, Ok(true)) {
            self.data_services
                .tasks
                .records
                .get_mut(&target)
                .unwrap()
                .loading = Some(pending);
        }
        result
    }

    fn drive_ordinary_load(
        &mut self,
        target: u32,
        pending: &mut OrdinaryLoad,
        budget: usize,
    ) -> Result<bool> {
        let program = if !pending.installed {
            let name =
                String::from_utf8(cp932_to_utf8(&pending.name)).expect("CP932 produces UTF-8");
            let names = self
                .resources
                .category_names(b"kcs", &name)
                .map_err(|_| native_state_error("native 143 kcs category is unavailable"))?;
            names
                .iter()
                .find_map(|name| self.resources.find(name))
                .map(|candidate| {
                    let bytes = self.resources.read_candidate(candidate).map_err(|_| {
                        native_state_error("native 143 program resource read failed")
                    })?;
                    KcsProgram::parse(&bytes)
                })
                .transpose()?
        } else {
            None
        };
        let parent = self.task_parents.get(&target).copied().unwrap_or(0);
        let context = self
            .data_services
            .tasks
            .records
            .get(&parent)
            .map(|record| record.context.clone())
            .unwrap_or_default();
        let previous_binding = self.data_services.tasks.records[&target].external.clone();
        let binding_after_load = pending.external.clone();
        if !pending.installed {
            let shared_area = self.data_services.tasks.records.get(&parent)
                .and_then(|record| record.shared_area);
            let previous = self.data_services.tasks.records[&target].shared_area;
            self.data_services.tasks.records.get_mut(&target).unwrap().shared_area = shared_area;
            self.release_task_values(previous);
        }
        let done = self.with_ordinary_task(target, |host, task| {
            if task.has_pending_update() || task.has_pending_command() {
                return Err(native_state_error(
                    "native 143 target has a suspended execution continuation",
                ));
            }
            if !pending.installed {
                let previous_external = previous_binding
                    .or_else(|| task.vm.as_ref().map(Vm::external_binding))
                    .unwrap_or_default();
                host.data_services.tasks.scene_tools.release_vm(target);
                task.vm = None;
                host.release_task_image_resources(target);
                if let Some(program) = program {
                    let mut vm = Vm::new();
                    vm.load_program_with_shared_external(program, previous_external)?;
                    task.vm = Some(vm);
                    task.running = 1;
                }
                task.set_creation_context(&context)
                    .map_err(|error| host.task_error(target, error))?;
                pending.installed = true;
            }
            if !pending.initialized && pending.initialize != 0 && task.vm.is_some() {
                task.system_time_ms = pending.system_time_ms;
                if !task
                    .initialize_execution(host, budget)
                    .map_err(|error| host.task_error(target, error))?
                {
                    return Ok(false);
                }
            }
            pending.initialized = true;
            if let Some(vm) = task.vm.as_mut() {
                vm.bind_shared_external(pending.external.clone())?;
            }
            Ok(true)
        })?;
        if done && self.task_vms.contains_key(&target) {
            self.data_services
                .tasks
                .records
                .get_mut(&target)
                .unwrap()
                .external = Some(binding_after_load);
        }
        Ok(done)
    }

    pub(super) fn task_message_native(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let target = read_u32(arguments, 0)?;
        let packet = [
            self.current_task_id,
            read_u32(arguments, 4)?,
            read_u32(arguments, 8)?,
            read_u32(arguments, 12)?,
        ];
        let mut output = 0;
        let done =
            self.message_branch_with_vm(target, packet, index == 70, &mut output, Some(vm))?;
        Ok(if done { NativeReturn::Value(output) } else { NativeReturn::Retry })
    }

    pub(super) fn broadcast_title_return(&mut self) -> Result<()> {
        // The window receiver uses command 11's forced traversal with inner
        // command 21. Ordinary receivers queue message 0xffff_0013 for update.
        self.broadcast_title_return_branch(self.current_task_id, 0)
    }

    fn broadcast_title_return_branch(&mut self, target: u32, depth: usize) -> Result<()> {
        if depth >= 256 {
            return Err(native_state_error("title-return task hierarchy exceeds 256 levels"));
        }
        self.ensure_task_record(target)?;
        if ordinary_kind(self.data_services.tasks.records[&target].kind) {
            self.receive_task_command(
                TaskCommand { target: target as i32, sender: 0, code: 21, data: [0; 3] },
                &mut cs2_runtime::TaskCommandBuffers::default(), &mut 0, 4096,
            )?;
        }
        let mut cursor = None;
        while let Some(child) = self.next_task_message_child(target, cursor) {
            self.broadcast_title_return_branch(child, depth + 1)?;
            cursor = Some(child as i32);
        }
        Ok(())
    }

    pub(super) fn message_branch(
        &mut self,
        target: u32,
        packet: TaskMessage,
        queued: bool,
        output: &mut u32,
    ) -> Result<bool> {
        self.message_branch_with_vm(target, packet, queued, output, None)
    }

    fn message_branch_with_vm(
        &mut self,
        target: u32,
        packet: TaskMessage,
        queued: bool,
        output: &mut u32,
        mut vm: Option<&mut Vm>,
    ) -> Result<bool> {
        let caller = self.current_task_id;
        let mut traversal = match self.data_services.tasks.message_traversals.remove(&caller) {
            Some(traversal) => {
                if traversal.target != target || traversal.packet != packet || traversal.queued != queued {
                    self.data_services.tasks.message_traversals.insert(caller, traversal);
                    return Err(native_state_error("task message continuation changed its input"));
                }
                traversal
            }
            None => MessageTraversal {
                target, packet, queued, output: *output,
                stack: vec![(target, MessagePhase::Receiver)],
            },
        };
        let result = (|| {
            for _ in 0..4096 {
                let Some((id, phase)) = traversal.stack.last().copied() else {
                    return Ok(true);
                };
                if traversal.stack.len() > 256 {
                    return Err(native_state_error("task message hierarchy exceeds 256 levels"));
                }
                match phase {
                    MessagePhase::Receiver => {
                        let Some(outcome) = self.receive_task_message(
                            id, packet, queued, &mut traversal.output, vm.as_deref_mut(),
                        )? else {
                            return Ok(false);
                        };
                        if outcome {
                            traversal.stack.last_mut().unwrap().1 = MessagePhase::Children(None);
                        } else {
                            traversal.stack.pop();
                        }
                    }
                    MessagePhase::Children(cursor) => {
                        if let Some(child) = self.next_task_message_child(id, cursor) {
                            traversal.stack.last_mut().unwrap().1 =
                                MessagePhase::Children(Some(child as i32));
                            traversal.stack.push((child, MessagePhase::Receiver));
                        } else {
                            traversal.stack.pop();
                        }
                    }
                }
            }
            Ok(traversal.stack.is_empty())
        })();
        *output = traversal.output;
        if !matches!(result, Ok(true)) {
            self.data_services.tasks.message_traversals.insert(caller, traversal);
        }
        result
    }

    /// None suspends this receiver. Some(false) consumes its branch;
    /// Some(true) permits signed-id traversal of its current children.
    fn receive_task_message(
        &mut self, target: u32, mut packet: TaskMessage, queued: bool,
        output: &mut u32, mut vm: Option<&mut Vm>,
    ) -> Result<Option<bool>> {
        if !self.task_is_reachable(target) {
            return Ok(Some(false));
        }
        self.ensure_task_record(target)?;
        let record = &self.data_services.tasks.records[&target];
        let (kind, deletion, message_owner) = (record.kind, record.deletion, record.message_owner);
        // The native call has already entered this receiver. A synthetic
        // budget boundary must not turn a later deletion flag into a fresh
        // dispatch and abandon the retained VM message or root receipt.
        if !queued && ordinary_kind(kind) {
            if self.data_services.tasks.root_dispatch.is_some_and(|request|
                request.target == target && request.message == packet)
            {
                return self.dispatch_root_message_request(target, &mut packet, output)
                    .map(|done| done.then_some(false));
            }
            if message_owner == Some(self.current_task_id) {
                let done = self.dispatch_ordinary_message_with_caller(target, &mut packet, output, 4096, vm)?;
                return Ok(done.then_some(false));
            }
        }
        if deletion != 0 {
            return Ok(Some(false));
        }
        if self.receive_root_sound_message_query(target, packet, output)? {
            return Ok(Some(false));
        }
        if let Some(outcome) = self.dispatch_legacy_plane_input91(target, packet, queued, output)? {
            return Ok(outcome.done.then_some(outcome.propagate));
        }
        if matches!(kind, 2 | 65) {
            if let Some(outcome) = self.dispatch_movie_plane_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if kind == 2 && queued {
            if let Some(outcome) = self.queue_plane_raw_message(target, packet, output, vm.as_deref())? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if let Some(outcome) = self.dispatch_css_model_message(
            target, packet, queued, output, vm.as_deref_mut(),
        )? {
            return Ok(outcome.done.then_some(outcome.propagate));
        }
        if let Some(outcome) = self.dispatch_css_task_message(target, packet, queued, output)? {
            return Ok(outcome.done.then_some(outcome.propagate));
        }
        if kind == 54 {
            if let Some(outcome) = self.dispatch_emote_geometry(
                target, packet, queued, output, vm.as_deref(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if kind == 61 {
            if let Some(outcome) = self.dispatch_effekseer_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if kind == 16 {
            if let Some(outcome) = self.dispatch_particle_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if matches!(kind, 18 | 25) {
            if let Some(outcome) = self.dispatch_raster_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if matches!(kind, 32 | 33) {
            if let Some(outcome) = self.dispatch_scene_tool_packet(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if ordinary_kind(kind) {
            if let Some(outcome) = self.dispatch_scene_selection_packet(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if kind == 47 {
            if let Some(outcome) = self.dispatch_message_list_packet(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if matches!(kind, 45 | 46) {
            if let Some(outcome) = self.dispatch_export_task_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if self.legacy_particle_canvas(kind) {
            if let Some(outcome) = self.dispatch_model_state_message(target, packet, queued, output, vm.as_deref_mut())? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
            if let Some(outcome) = self.dispatch_particle_canvas_message(target, packet, queued, output, vm.as_deref_mut())? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        } else if RenderTaskStore::supports_kind(kind) && !self.legacy_css_model(kind) {
            if let Some(outcome) = self.dispatch_render_task_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if matches!(kind, 8 | 48 | 63) {
            if let Some(propagate) = self.dispatch_buffered_text_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(Some(propagate));
            }
        }
        if kind == 55 && packet[1] == 158 {
            let done = self.dispatch_javascript_message(
                target, packet, queued, output, vm.as_deref_mut(),
            ).map_err(|cause| self.task_error(target, cause))?;
            return Ok(done.then_some(false));
        }
        if kind == 55 && matches!(packet[1], 6 | 9 | 10 | 19 | 64 | 65 | 522..=524) {
            if queued && matches!(packet[1], 9 | 10) {
                self.data_services.tasks.javascript.queue_task_input(target,
                    cs2_runtime::javascript::JavaScriptInput {
                        value: packet[3], kind: u32::from(packet[1] == 9),
                        pointer: self.input_task.pointer_position,
                    },
                ).map_err(|cause| self.task_error(target, cause))?;
                *output = 1;
                return Ok(Some(false));
            }
            let outcome = self.receive_javascript_command(
                TaskCommand {
                    target: target as i32, sender: packet[0] as i32,
                    code: packet[1], data: [packet[2], packet[3], 0],
                },
                &mut cs2_runtime::TaskCommandBuffers::default(), output, 4096,
            ).map_err(|cause| self.task_error(target, cause))?
                .ok_or_else(|| native_state_error("JavaScript command receiver is unavailable"))?;
            return Ok(outcome.done.then_some(outcome.propagate));
        }
        if matches!(kind, 41 | 42)
            && self.data_services.fes_controls.owns_lower(target)
        {
            if let Some(outcome) = self.dispatch_lower_control_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
            if matches!(packet[1], 9 | 10 | 64 | 65) {
                return Err(native_state_error(
                    "raw FES input event requires its real button-state and object-target receiver",
                ));
            }
        }
        if kind == 15 {
            if let Some(outcome) = self.dispatch_buffered_list_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if kind == 62 {
            if let Some(outcome) = self.dispatch_tweet_message(target, packet, queued, output, vm.as_deref())? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if kind == 34 {
            if let Some(outcome) = self.dispatch_buffered_fes_message(
                target, packet, queued, output, vm.as_deref_mut(),
            )? {
                return Ok(outcome.done.then_some(outcome.propagate));
            }
        }
        if kind == 13 && self.dispatch_sound_task_message(
            target, packet, queued, output, vm.as_deref_mut(),
        )? {
            return Ok(Some(false));
        }
        if matches!(kind, 26..=28) && self.dispatch_transition_message(
            target, packet, queued, output, vm.as_deref_mut(),
        )? {
            return Ok(Some(false));
        }
        if ordinary_kind(kind) {
            if queued {
                self.data_services.tasks.records.get_mut(&target).unwrap().queued.push_back(packet);
                *output = 1;
            } else if self.data_services.tasks.external_tasks.contains(&target)
                && !self.task_vms.contains_key(&target)
            {
                if self.data_services.tasks.executing_external == Some(target)
                    || self.data_services.tasks.records[&target].active
                {
                    *output = 0;
                    return Ok(Some(false));
                }
                return self.dispatch_root_message_request(target, &mut packet, output)
                    .map(|done| done.then_some(false));
            } else if !self.data_services.tasks.records[&target].active
                && target != self.current_task_id
            {
                let done = self.dispatch_ordinary_message_with_caller(target, &mut packet, output, 4096, vm)?;
                return Ok(done.then_some(false));
            } else {
                *output = 0;
            }
            return Ok(Some(false));
        }
        Ok(Some(true))
    }

    fn dispatch_ordinary_message(
        &mut self, target: u32, packet: &mut TaskMessage, output: &mut u32, budget: usize,
    ) -> Result<bool> {
        self.dispatch_ordinary_message_with_caller(target, packet, output, budget, None)
    }

    fn dispatch_ordinary_message_with_caller(
        &mut self, target: u32, packet: &mut TaskMessage, output: &mut u32,
        budget: usize, mut caller_vm: Option<&mut Vm>,
    ) -> Result<bool> {
        let caller = self.current_task_id;
        let record = &self.data_services.tasks.records[&target];
        let busy = record.loading.is_some() || record.active
            || record.ordinary.as_ref().is_some_and(|task| task.has_pending_update()
                || (task.has_pending_command() && record.message_owner != Some(caller)));
        if busy {
            *output = 0;
            return Ok(true);
        }
        if let Some(vm) = caller_vm.as_deref_mut() {
            let std::collections::btree_map::Entry::Vacant(slot) =
                self.data_services.tasks.synchronous_callers.entry(caller)
            else {
                return Err(native_state_error("synchronous caller VM is already exposed"));
            };
            slot.insert(std::mem::take(vm));
        }
        self.data_services.tasks.records.get_mut(&target).unwrap().message_owner = Some(caller);
        let result = self.with_ordinary_task(target, |host, task| {
            task.dispatch_message(packet, output, host, budget)
                .map_err(|error| host.task_error(target, error))
        });
        if let Some(vm) = caller_vm {
            *vm = self.data_services.tasks.synchronous_callers.remove(&caller)
                .expect("synchronous dispatch owns its caller VM until it returns");
        }
        if matches!(result, Ok(true)) {
            self.data_services.tasks.records.get_mut(&target).unwrap().message_owner = None;
        }
        result
    }

    fn dispatch_root_message_request(
        &mut self, target: u32, packet: &mut TaskMessage, output: &mut u32,
    ) -> Result<bool> {
        match self.data_services.tasks.root_dispatch {
            Some(request) if request.target == target && request.message == *packet => {
                if request.completed {
                    *output = request.output;
                    *packet = request.reply;
                    self.data_services.tasks.root_dispatch = None;
                    return Ok(true);
                }
            }
            Some(_) => return Err(native_state_error(
                "finish the pending synchronous root message before another request",
            )),
            None => self.data_services.tasks.root_dispatch = Some(RootTaskDispatch {
                target, message: *packet, completed: false, output: 0, reply: *packet,
            }),
        }
        Ok(false)
    }

    pub(super) fn delete_task_native(&mut self, arguments: &[u8]) -> Result<NativeReturn> {
        let id = read_u32(arguments, 0)?;
        let result = self.receive_task_command(
            TaskCommand { target: id as i32, sender: self.current_task_id as i32,
                code: 3, data: [0; 3] },
            &mut cs2_runtime::TaskCommandBuffers::default(), &mut 0, 4096,
        )?;
        Ok(if result.is_some_and(|outcome| !outcome.done) {
            NativeReturn::Retry
        } else {
            NativeReturn::Continue
        })
    }

    pub(super) fn request_ordinary_task_delete(&mut self, id: u32) -> Result<()> {
        if id == 0 {
            return Err(native_state_error(
                "native 67 root deletion requires the player's root lifecycle",
            ));
        }
        if self.task_parents.contains_key(&id) || self.scene_nodes.contains_key(&id) {
            self.ensure_task_record(id)?;
            // The render receiver unregisters its callback on command 3.
            // A replacement may register before deferred task destruction.
            self.data_services.tasks.render.release(id);
            if self.data_services.tasks.records[&id].kind == 2 {
                self.release_plane_history(id);
                self.clear_plane_mesh(id);
                if let Some(node) = self.scene_nodes.get_mut(&id) {
                    node.initialized = false;
                    node.visible = false;
                }
            }
            self.data_services
                .tasks
                .records
                .get_mut(&id)
                .unwrap()
                .deletion = 1;
        }
        Ok(())
    }

    pub(super) fn task_property_native(
        &mut self,
        index: u32,
        arguments: &[u8],
    ) -> Result<NativeReturn> {
        let argument = read_u32(arguments, 0)?;
        let mut output = u32::MAX;
        let (target, code, value) = if index == 453 {
            (self.current_task_id, 24, argument)
        } else {
            (argument, 23, 0)
        };
        self.receive_task_command(
            TaskCommand { target: target as i32, sender: self.current_task_id as i32,
                code, data: [0, value, 0] },
            &mut cs2_runtime::TaskCommandBuffers::default(), &mut output, 4096,
        )?;
        Ok(if index == 453 { NativeReturn::Continue } else { NativeReturn::Value(output) })
    }

    /// Ordinary 454 copies the same 32-byte task cache used by commands
    /// 25/26 and native 66. V401 stores it at +1c4, Yukikoi at +30c;
    /// neither wrapper formats it or treats it as a resource filename.
    pub(super) fn task_context_native(
        &mut self, arguments: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let destination = read_u32(arguments, 0)?;
        self.ensure_task_record(self.current_task_id)?;
        let mut bytes = self.data_services.tasks.task_context(self.current_task_id)
            .ok_or_else(|| native_state_error("current task context is unavailable"))?
            .to_vec();
        bytes.push(0);
        vm.write(destination, &bytes)?;
        Ok(NativeReturn::Continue)
    }

    /// Integration point for run_frames. Call before the root VM's body.
    /// Re-enter with the same root until true; now/increment are captured on
    /// first entry, so a budget slice must not advance the player's frame.
    pub(super) fn update_ordinary_children(
        &mut self,
        root: u32,
        now: u32,
        increment: u32,
        instruction_budget: usize,
    ) -> Result<bool> {
        let _timing = super::measure_script_detail(super::HostPhase::ScriptChildren);
        if self.data_services.tasks.direct_release.is_some() {
            return Err(native_state_error("finish direct FES root release before updating tasks"));
        }
        if self.data_services.tasks.traversal.is_none() {
            self.ensure_task_record(root)?;
            self.data_services.tasks.set_task_clock(root, now)?;
        }
        let mut traversal = self
            .data_services
            .tasks
            .traversal
            .take()
            .unwrap_or(Traversal {
                root,
                now,
                increment,
                stack: vec![(root, Phase::Children(None))],
            });
        if traversal.root != root {
            self.data_services.tasks.traversal = Some(traversal);
            return Err(native_state_error(
                "finish the active ordinary subtree update first",
            ));
        }
        let result = self.drive_ordinary_children(&mut traversal, instruction_budget);
        if !matches!(result, Ok(true)) {
            self.data_services.tasks.traversal = Some(traversal);
        }
        result
    }

    /// Until the root VM is moved into this store, the player feeds these
    /// packets into its existing pending_messages queue before root resume.
    pub(super) fn drain_root_task_messages(&mut self, root: u32) -> Vec<TaskMessage> {
        if self.data_services.tasks.scene_tools.has_strings(root) {
            // The root-VM integration must use the fallible with_vm variant
            // while an owned string is queued. Hold all words in FIFO order.
            return Vec::new();
        }
        self.data_services
            .tasks
            .records
            .get_mut(&root)
            .map(|record| record.queued.drain(..).collect())
            .unwrap_or_default()
    }

    fn drive_ordinary_children(
        &mut self,
        traversal: &mut Traversal,
        budget: usize,
    ) -> Result<bool> {
        for _ in 0..budget {
            if traversal.stack.len() > 256 {
                return Err(native_state_error(
                    "ordinary task hierarchy exceeds 256 levels",
                ));
            }
            let Some((id, phase)) = traversal.stack.last().copied() else {
                return Ok(true);
            };
            if id != traversal.root && !self.task_parents.contains_key(&id) {
                traversal.stack.pop();
                continue;
            }
            let next = match phase {
                Phase::Begin => {
                    self.ensure_task_record(id)?;
                    let _detail = super::perf_detail::SlowTask::new(id,
                        || self.data_services.tasks.records[&id].kind as i32, "child_begin");
                    self.data_services
                        .tasks
                        .records
                        .get_mut(&id)
                        .unwrap()
                        .clock
                        .update(traversal.now);
                    if matches!(self.data_services.tasks.records[&id].kind, 2 | 6) {
                        let _timing = super::measure_script_detail(super::HostPhase::ScriptButton);
                        self.begin_input_button_update(id)?;
                    }
                    if self.data_services.tasks.records[&id].kind == 18 {
                        self.begin_raster_task_update(id)?;
                    }
                    Phase::Children(None)
                }
                Phase::Children(cursor) => {
                    if let Some(child) = self.next_task_child(id, cursor) {
                        traversal.stack.last_mut().unwrap().1 = Phase::Children(Some(child as i32));
                        traversal.stack.push((child, Phase::Begin));
                        continue;
                    }
                    Phase::Body
                }
                Phase::Body => {
                    if id == traversal.root {
                        self.data_services
                            .tasks
                            .reserved
                            .retain(|id| self.task_parents.contains_key(id));
                        traversal.stack.pop();
                        continue;
                    }
                    let kind = self.data_services.tasks.records[&id].kind;
                    let _detail = super::perf_detail::SlowTask::new(id, || kind as i32, "child_body");
                    // Deletion is checked before entering the original body,
                    // not between artificial instruction-budget slices. A
                    // nested native67 can mark the detached record while its
                    // VM still owns a message/resume continuation.
                    let resuming_ordinary = ordinary_kind(kind)
                        && self.data_services.tasks.records[&id].ordinary.as_ref()
                            .is_some_and(OrdinaryTask::has_pending_update);
                    if self.data_services.tasks.records[&id].deletion != 0 && !resuming_ordinary {
                        Phase::Unregister
                    } else {
                        // Queued owned commands precede a new body, but must
                        // not reenter a VM whose current body has only yielded
                        // to the portable scheduler's instruction budget.
                        if !resuming_ordinary && !self.update_task_command_buffers(id, budget)? {
                            return Ok(false);
                        }
                        if self.data_services.tasks.records[&id].deletion != 0 && !resuming_ordinary {
                            traversal.stack.last_mut().unwrap().1 = Phase::Unregister;
                            continue;
                        }
                        if kind == 34 {
                            let _timing = super::measure_script_detail(super::HostPhase::ScriptFes);
                            if !self.update_fes_task(id, traversal.now, budget)? {
                                return Ok(false);
                            }
                        }
                        if kind == 55 && !self.update_javascript_task(id, budget)
                            .map_err(|cause| self.task_error(id, cause))?
                        {
                            return Ok(false);
                        }
                        if kind == 2 {
                            self.advance_plane_history(id, traversal.now)?;
                        }
                        if self.data_services.tasks.records[&id].kind == 15
                            && self.data_services.tasks.fes.platform.lists.get(&id)
                                .is_some_and(|list| list.enabled)
                        {
                            let _timing = super::measure_script_detail(super::HostPhase::ScriptList);
                            if !super::list_native::update_animations(self, id, traversal.increment, budget)
                                .map_err(|error| self.task_error(id,
                                    cs2_runtime::RuntimeError::Host(format!("list animation update: {error}"))))?
                            {
                                return Ok(false);
                            }
                            super::fes_task_platform::compose_list(self, id, false)
                                .map_err(|error| self.task_error(id,
                                    cs2_runtime::RuntimeError::Host(format!(
                                        "image-list automatic update: {error}",
                                    ))))?;
                        }
                        if self.data_services.tasks.records[&id].kind == 13 {
                            self.update_sound_task(id)?;
                        }
                        if kind == 16 && !self.advance_particle_task(id, traversal.now, budget)? {
                            return Ok(false);
                        }
                        if matches!(kind, 45 | 46) {
                            self.update_export_task(id, budget)?;
                        }
                        if kind == 47 && !self.update_message_list_task(id, budget)? {
                            return Ok(false);
                        }
                        if matches!(kind, 32 | 33) && !self.update_scene_tool_task(id, budget)? {
                            return Ok(false);
                        }
                        if kind == 44 { self.update_wallpaper_task(id)?; }
                        if kind == 62 { self.update_tweet_task(id)?; }
                        if kind == 18 { self.advance_raster_task(id)?; }
                        if kind == 54 {
                            let elapsed = self.data_services.tasks.records[&id].clock.elapsed();
                            self.emote_update_task_hook(id, elapsed)?;
                        }
                        if self.legacy_particle_canvas(kind) { self.advance_legacy_particle_canvas(id)?; }
                        if kind == 61 {
                            self.effekseer_update_task_hook(id)?;
                        }
                        if RenderTaskStore::supports_kind(kind) && !self.legacy_particle_canvas(kind)
                            && !self.legacy_css_model(kind) {
                            let elapsed = self.data_services.tasks.records[&id].clock.elapsed();
                            self.data_services.tasks.render.update(id, elapsed).map_err(|cause|
                                self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?;
                        }
                        if matches!(self.data_services.tasks.records[&id].kind, 26..=28) {
                            self.advance_transition_task(id)?;
                        }
                        let record = &self.data_services.tasks.records[&id];
                        if ordinary_kind(record.kind)
                            && record.loading.is_none()
                            && !self.with_ordinary_task(id, |host, task| {
                                let _detail = super::perf_detail::SlowTask::new(id,
                                    || kind as i32, "kcs_body");
                                let profile = super::perf_detail::KcsBody::new(id,
                                    task.vm.as_ref().map(|vm| vm.instruction_pointer));
                                task.system_time_ms = host.host_millis();
                                let result = task.update_body_with_messages(
                                    traversal.increment,
                                    host,
                                    budget,
                                    &mut |host| {
                                        host.data_services
                                            .tasks
                                            .records
                                            .get_mut(&id)
                                            .map(|record| record.queued.drain(..).collect())
                                            .unwrap_or_default()
                                    },
                                );
                                profile.end_pc(task.vm.as_ref().map(|vm| vm.instruction_pointer));
                                result.map_err(|error| host.task_error(id, error))
                            })?
                        {
                            return Ok(false);
                        }
                        traversal.stack.pop();
                        continue;
                    }
                }
                Phase::Unregister => {
                    self.ensure_task_record(id)?;
                    self.data_services.tasks.records.get_mut(&id).unwrap().registered = false;
                    Phase::NotifyChildren(None)
                }
                Phase::NotifyChildren(cursor) => {
                    if let Some(child) = self.next_task_child(id, cursor) {
                        if !self.notify_task_delete(child, budget)? {
                            return Ok(false);
                        }
                        Phase::NotifyChildren(Some(child as i32))
                    } else {
                        Phase::NotifySelf
                    }
                }
                Phase::NotifySelf => {
                    if !self.notify_task_delete(id, budget)? {
                        return Ok(false);
                    }
                    Phase::DestroyChildren(None)
                }
                Phase::DestroyChildren(cursor) => {
                    if let Some(child) = self.next_task_child(id, cursor) {
                        traversal.stack.last_mut().unwrap().1 =
                            Phase::DestroyChildren(Some(child as i32));
                        traversal.stack.push((child, Phase::Unregister));
                        continue;
                    }
                    Phase::Release
                }
                Phase::Release => {
                    let _detail = super::perf_detail::SlowTask::new(id,
                        || self.data_services.tasks.records.get(&id).map_or(-1, |record| record.kind as i32),
                        "child_release");
                    if !self.release_scene_tool_task(id)? {
                        return Ok(false);
                    }
                    if !self.release_message_list_task(id)? {
                        return Ok(false);
                    }
                    if !self.data_services.wallpapers.can_release(id) { return Ok(false); }
                    if !self.data_services.tweets.can_release(id) { return Ok(false); }
                    self.update_tweet_task(id)?;
                    if !self.data_services.tasks.exports.can_release(id) {
                        return Ok(false);
                    }
                    if self.data_services.tasks.fes.contains(id)
                        && !self.prepare_fes_task_release(id)?
                    {
                        return Ok(false);
                    }
                    if !self.release_javascript_task(id, budget)
                        .map_err(|cause| self.task_error(id, cause))? { return Ok(false); }
                    self.emote_release_task_hook(id)?;
                    self.data_services.effekseer.manager.release(id);
                    self.data_services.emocon.release(id);
                    self.data_services.tasks.exports.release(id);
                    self.data_services.wallpapers.release(id);
                    self.data_services.tweets.release(id)?;
                    if matches!(self.data_services.tasks.records[&id].kind, 2 | 6) {
                        self.data_services.messages.release(id);
                    }
                    self.data_services.tasks.render.release(id);
                    self.release_media_effect_parameters(id);
                    self.release_plane_history(id);
                    super::list_native::release_list(self, id)
                        .map_err(|error| self.task_error(id,
                            cs2_runtime::RuntimeError::Host(format!(
                                "image-list resource release: {error}",
                            ))))?;
                    self.release_fes_list_output(id)?;
                    self.release_sound_task(id)?;
                    self.release_movie_task_resources(id)?;
                    super::transition_native::release(&mut self.data_services.transitions, id);
                    let shared_area = self.data_services.tasks.records.get(&id)
                        .and_then(|record| record.shared_area);
                    self.task_parents.remove(&id);
                    self.scene_nodes.remove(&id);
                    self.release_scene_tool_memory(id);
                    self.task_vms.remove(&id);
                    self.particles.remove(&id);
                    self.data_services.particle_external.release(id);
                    self.data_services.particle_external.release_canvas(id);
                    self.data_services.css_models.release(id);
                    self.data_services.tasks.model_state_trees.retain(|_, (owner, _)| *owner != id);
                    self.data_services.tasks.records.remove(&id);
                    self.data_services.tasks.external_tasks.remove(&id);
                    self.data_services.tasks.lower_commands.remove(&id);
                    self.release_task_command_buffers(id);
                    self.data_services.tasks.message_traversals.remove(&id);
                    self.release_task_values(shared_area);
                    self.data_services.tasks.fes.release(id);
                    if self.data_services.tasks.root_dispatch
                        .is_some_and(|request| request.message[0] == id)
                    {
                        // A root callback may have deleted the sending FES.
                        // Its retained send effect will never acknowledge
                        // again once destruction has reached this owner.
                        self.data_services.tasks.root_dispatch = None;
                    }
                    self.data_services.tasks.released.push_back(id);
                    self.input_task.regions.retain(|region| region.id() != id);
                    self.release_task_image_resources(id);
                    traversal.stack.pop();
                    continue;
                }
            };
            traversal.stack.last_mut().unwrap().1 = next;
        }
        Ok(traversal.stack.is_empty())
    }

    fn notify_task_delete(&mut self, id: u32, budget: usize) -> Result<bool> {
        let _detail = super::perf_detail::SlowTask::new(id,
            || self.data_services.tasks.records.get(&id).map_or(-1, |record| record.kind as i32),
            "child_notify_delete");
        self.ensure_task_record(id)?;
        if self.data_services.tasks.records[&id].deletion != 0 {
            return Ok(true);
        }
        if ordinary_kind(self.data_services.tasks.records[&id].kind) {
            if !self.with_ordinary_task(id, |host, task| {
                task.request_delete(host, budget)
                    .map_err(|error| host.task_error(id, error))
            })? {
                return Ok(false);
            }
        }
        self.data_services
            .tasks
            .records
            .get_mut(&id)
            .unwrap()
            .deletion = 1;
        Ok(true)
    }

    fn release_task_image_resources(&mut self, id: u32) {
        let handles: Vec<_> = self
            .image_resources
            .iter()
            .filter_map(|(&handle, resource)| (resource.owner == Some(id)).then_some(handle))
            .collect();
        for handle in handles {
            let resource = self.image_resources.get_mut(&handle).unwrap();
            resource.owner = None;
            resource.references = resource.references.saturating_sub(1);
            if resource.references == 0 {
                self.image_resources.remove(&handle);
                self.release_image_resource_layers(handle);
            }
        }
    }

    pub(super) fn ensure_task_record(&mut self, id: u32) -> Result<()> {
        if self.data_services.tasks.records.contains_key(&id) {
            return Ok(());
        }
        let kind = self.scene_nodes.get(&id).map(|node| node.kind).unwrap_or(0);
        self.data_services.tasks.records.insert(
            id,
            Record::new(id, kind, &self.data_services.language.languages),
        );
        Ok(())
    }

    fn next_task_child(&self, parent: u32, cursor: Option<i32>) -> Option<u32> {
        self.task_parents
            .iter()
            .filter_map(|(&id, &actual_parent)| {
                (actual_parent == parent
                    && id != parent
                    && cursor.is_none_or(|cursor| (id as i32) > cursor))
                .then_some(id)
            })
            .min_by_key(|&id| id as i32)
    }

    fn next_task_message_child(&self, parent: u32, mut cursor: Option<i32>) -> Option<u32> {
        while let Some(id) = self.next_task_child(parent, cursor) {
            if self.data_services.tasks.task_is_registered(id) { return Some(id); }
            cursor = Some(id as i32);
        }
        None
    }

    /// Unregistering an owner removes its number from ancestor indexes while
    /// its actual object and children remain alive. Descendants can still
    /// address it by following their own parent chain during delete callbacks.
    pub(super) fn task_is_reachable(&self, target: u32) -> bool {
        if target != 0 && target != self.current_task_id
            && !self.task_parents.contains_key(&target)
            && !self.scene_nodes.contains_key(&target)
        {
            return false;
        }
        if self.data_services.tasks.task_is_registered(target) { return true; }
        let mut current = self.current_task_id;
        for _ in 0..256 {
            if current == target { return true; }
            let Some(parent) = self.task_parents.get(&current).copied() else { break };
            if parent == current { break; }
            current = parent;
        }
        false
    }

    fn with_ordinary_task<T>(
        &mut self,
        id: u32,
        call: impl FnOnce(&mut Self, &mut OrdinaryTask) -> Result<T>,
    ) -> Result<T> {
        self.ensure_task_record(id)?;
        let record = &self.data_services.tasks.records[&id];
        if record.active || !ordinary_kind(record.kind) || record.ordinary.is_none() {
            return Err(native_state_error(
                "ordinary task is active or has a different object type",
            ));
        }
        self.prepare_scene_tool_strings(id)?;
        let values = self.enter_task_values(id)?;
        let record = self.data_services.tasks.records.get_mut(&id).unwrap();
        let mut task = record
            .ordinary
            .take()
            .ok_or_else(|| native_state_error("ordinary object storage is unavailable"))?;
        record.active = true;
        task.clock = record.clock.clone();
        task.property = record.property;
        task.deletion = record.deletion;
        if task.vm.is_none() {
            task.vm = self.task_vms.remove(&id);
            if task.vm.is_some() && !record.had_vm {
                task.running = 1;
            }
        }
        while let Some(message) = record.queued.pop_front() {
            task.enqueue_message(message);
        }
        let initial_context = record.context.clone();
        task.set_creation_context(&initial_context)
            .expect("cached task contexts are bounded at their write entry");
        let saved = (self.current_task_id, self.task_time, self.task_origin);
        self.current_task_id = id;
        self.task_time = task.clock.current;
        self.task_origin = task.clock.origin;
        let result = call(self, &mut task);
        (self.current_task_id, self.task_time, self.task_origin) = saved;
        self.leave_task_values(values);
        // Always restore ownership on error as well as success.
        let had_vm = task.vm.is_some();
        if let Some(vm) = task.vm.take() {
            self.task_vms.insert(id, vm);
        }
        let record = self
            .data_services
            .tasks
            .records
            .get_mut(&id)
            .expect("active task removal is deferred until tree release");
        // Nested command 29 modifies the detached object's exposed record.
        // Preserve it rather than restoring the stale pre-call pause value.
        task.clock.paused = record.clock.paused;
        task.clock.single_step = record.clock.single_step;
        record.clock = task.clock.clone();
        if record.deletion == 0 {
            record.deletion = task.deletion;
        } else {
            task.deletion = record.deletion;
        }
        // Native 453 writes the exposed record while the object is detached;
        // preserve that write even if subsequent script execution failed.
        task.property = record.property;
        if record.context != initial_context {
            task.set_creation_context(&record.context)
                .expect("task context command validates its bounded bytes");
        } else {
            record.context = task.creation_context().to_vec();
        }
        record.had_vm = had_vm;
        record.ordinary = Some(task);
        record.active = false;
        result
    }

    pub(super) fn task_error(&mut self, id: u32, error: cs2_runtime::RuntimeError) -> cs2_script::ScriptError {
        self.events.push(LogicEvent {
            frame: self.frame,
            kind: "task-error".into(),
            detail: format!("task={id}: {error}"),
        });
        native_state_error("ordinary task operation failed; see task-error event")
    }
}
