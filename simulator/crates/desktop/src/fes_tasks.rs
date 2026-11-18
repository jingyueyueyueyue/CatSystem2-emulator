//! Type-34 task ownership and ordinary 447/448/449. Script control is supplied
//! by fes_driver; rendering/input effects use explicit platform callbacks.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cs2_script::{
    ExpressionState, FesDocument, FesEvent, FesLine, FesSection, NativeReturn, Result,
    SharedExternalStorage, Vm,
};

use super::fes_driver::{
    FesChildKind, FesDriverEffect, FesDriverInput, FesDriverTarget, FesDriverWait,
    FesMediaNotificationScope, FesScriptDriver,
};
use super::native_tasks::NativeSubtreeContinuation;
use super::{
    FesRuntime, HeadlessPlayer, LoggingHost, LogicEvent, cp932_to_utf8, expand_kcs_format, native_state_error,
    native_state_error_owned, read_u32,
};

#[path = "fes_task_state.rs"]
pub(crate) mod state;

const MAX_FES_BYTES: usize = 64 * 1024 * 1024;
const SYNC_BUDGET: usize = 4096;
const LEGACY_SHARED_SLOTS: usize = 1024;

pub(super) fn startup_shared_slots(scan: &cs2_runtime::GameScan, entry: Option<&str>) -> usize {
    super::discover_shared_slots(&scan.resources)
        .or_else(|| verified_shared_slot_default(scan, entry))
        .unwrap_or(512)
}

fn verified_shared_slot_default(scan: &cs2_runtime::GameScan, entry: Option<&str>) -> Option<usize> {
    const PACKED: &str = "35ec21bcfcee0f324206e945eef1c082e35fd8af594ea0e738a46cd941f5b3f3";
    const COMPANION: &str = "d158c8915d4982c82badc631b4f92e1f140b246df5929ff2d9924d242c66765c";
    const PRODUCER: &str = "3294c9b35ec41e6ce157444a637d9529a0cd9876bdfbc126733e8ddc397b76cf";

    let sha256 = super::particle_plugin_digest::sha256;
    if !scan.engine.engine_binaries.iter().any(|binary| {
        binary.relative_path == std::path::Path::new("cs2.exe") && binary.sha256 == PACKED
    }) {
        return None;
    }
    let companion = scan.resources.files().iter().find(|file| {
        file.relative_path == std::path::Path::new("cs2_cn.exe") && file.size == 3_842_048
    })?;
    let companion = std::fs::read(scan.resources.root().join(&companion.relative_path)).ok()?;
    if sha256(&companion) != COMPANION {
        return None;
    }
    let producer = super::read_entry_with_fallback(&scan.resources, entry?).ok()?;
    (sha256(&producer) == PRODUCER).then_some(LEGACY_SHARED_SLOTS)
}

#[cfg(test)]
mod shared_variable_tests {
    use super::*;
    use cs2_script::expression::eval_expression;

    #[test]
    fn fes_start_uses_callers_high_shared_slot_without_widening_other_banks() -> anyhow::Result<()> {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-fes-shared-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root)?;
        let result = HeadlessPlayer::open_with_save_root(&root, root.join("profile"));
        std::fs::remove_dir(&root)?;
        let mut player = result?;
        assert_eq!(player.configured_shared_slots, 512);

        player.with_host(|host| {
            let caller = host.current_task_id;
            host.task_values_native(209, &(LEGACY_SHARED_SLOTS as u32).to_le_bytes())?;
            let parent = host.create_task_object(0, caller, b"parent", b"")?;
            host.current_task_id = parent;
            host.task_values_native(209, &512u32.to_le_bytes())?;
            let target = host.create_task_object(34, parent, b"flow", b"")?;
            host.current_task_id = caller;

            host.bind_fes_shared_values(target, caller)?;
            let scope = host.enter_task_values(target)?;
            let checked = with_expression(host, |_, state| {
                assert_eq!(eval_expression(b"$1021==0", state)?, 1);
                assert_eq!(eval_expression(b"$1021=1", state)?, 1);
                let error = eval_expression(b"$1024", state).unwrap_err();
                assert!(error.to_string().contains("domain=shared access=read index=1024 capacity=1024"));
                assert_eq!(state.local.len(), 512);
                Ok(())
            });
            host.leave_task_values(scope);
            checked?;
            assert_eq!(host.shared_values_for(caller).unwrap()[1021], 1);
            assert_eq!(host.shared_values_for(parent).unwrap().len(), 512);
            Ok(())
        })
    }
}

pub(super) fn fes_button_media_hook(
    host: &mut LoggingHost<'_>, owner: u32, budget: usize,
) -> Result<bool> {
    host.refresh_fes_button_media(owner, budget)
}

pub(super) fn fes_button_named_hook(
    host: &mut LoggingHost<'_>, owner: u32, name: &'static str, value: i32,
) -> Result<()> {
    host.write_fes_button_named_input(owner, name, value)
}

#[derive(Clone, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesListBinding {
    pub task: u32,
    pub clip: [i32; 4],
    pub offset: [i32; 2],
}

#[derive(Clone, Debug)]
pub(super) struct FesTaskView {
    pub id: u32,
    pub parent: u32,
    pub context: Vec<u8>,
    pub list: Option<FesListBinding>,
    pub now: u32,
    pub paused: u32,
}

pub(super) type FesEffectHandler = for<'a> fn(
    &mut LoggingHost<'a>,
    &FesTaskView,
    &mut FesRuntime,
    &mut FesScriptDriver,
    &FesDriverEffect,
    &mut ExpressionState,
) -> anyhow::Result<bool>;
/// None: unhandled, with no side effects. Some(false): handled but suspended;
/// Some(true): handled and complete. A retained effect is offered again only
/// to resume that handler's explicit continuation.
pub(super) type FesExtraEffectHandler = for<'a> fn(
    &mut LoggingHost<'a>,
    &FesTaskView,
    &mut FesRuntime,
    &mut FesScriptDriver,
    &FesDriverEffect,
    &mut ExpressionState,
) -> anyhow::Result<Option<bool>>;
pub(super) type FesPrepareHandler = for<'a> fn(
    &mut LoggingHost<'a>,
    &FesTaskView,
    &mut FesRuntime,
    &mut FesScriptDriver,
) -> anyhow::Result<bool>;
pub(super) type FesListHandler =
    for<'a> fn(&mut LoggingHost<'a>, u32) -> anyhow::Result<FesListBinding>;
pub(super) type FesSendHandler =
    for<'a> fn(&mut LoggingHost<'a>, u32, [u32; 4]) -> anyhow::Result<bool>;
pub(super) type FesReleaseHandler =
    for<'a> fn(&mut LoggingHost<'a>, &FesTaskView, &mut FesRuntime) -> anyhow::Result<bool>;
pub(super) type FesFinishHandler = for<'a> fn(
    &mut LoggingHost<'a>,
    &FesTaskView,
    &mut FesRuntime,
    &mut FesScriptDriver,
) -> anyhow::Result<bool>;
pub(super) type FesOwnerReleaseHandler =
    for<'a> fn(&mut LoggingHost<'a>, u32) -> anyhow::Result<()>;

#[derive(Clone, Copy, Default)]
pub(super) struct FesTaskHooks {
    pub effect: Option<FesEffectHandler>,
    /// Creates/enables the actual auxiliary input button. This is separate
    /// from metadata initialization and must not call the old advance loop.
    pub prepare: Option<FesPrepareHandler>,
    /// Queries a reachable type-15 list's real output size and offset.
    pub bind_list: Option<FesListHandler>,
    /// Required for synchronous delivery to a root VM held by the player.
    pub send: Option<FesSendHandler>,
    pub release: Option<FesReleaseHandler>,
}

#[derive(Default)]
pub(super) struct FesTaskStore {
    slots: BTreeMap<u32, FesSlot>,
    hooks: FesTaskHooks,
    pub(super) platform: super::fes_task_platform::FesTaskPlatform,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct FesSlot {
    object: Option<FesTask>,
    inputs: VecDeque<FesDriverInput>,
    context_initialized: bool,
    named_dwords: BTreeMap<Vec<u8>, u32>,
    save_object: Option<u32>,
    restore_entry: bool,
    pending_input_named: BTreeMap<String, i32>,
    button_media_request: ButtonMediaRequest,
    lower_notification_request: Option<LowerNotificationRequest>,
    program_headers: BTreeSet<Vec<u8>>,
    pending_entry: Option<Vec<u8>>,
    headers_dirty: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum ButtonMediaRequest { Idle, Requested, Complete }

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct LowerNotificationRequest {
    command: cs2_runtime::TaskCommand,
    input: FesDriverInput,
    complete: bool,
}

impl LowerNotificationRequest {
    fn matches(&self, command: cs2_runtime::TaskCommand, input: &FesDriverInput) -> bool {
        self.command == command && self.input.code == input.code
            && self.input.target == input.target && self.input.event == input.event
    }
}

/// The object protocol is distinct from ordinary-native slot numbering.
/// Locals remain accessible during synchronous child execution; the driver's
/// control change from command 350 is applied at its next safe host boundary.
enum FesDataCommand<'a> {
    SaveObject(u32),                         // 342
    SetParameter { index: u32, value: u32 }, // 346
    GetParameter(u32),                      // 347
    SaveParameters,                        // 349
    RestoreParameters(&'a [u8]),            // 350
    GetNamed(&'a [u8]),                     // 357
    SetNamed { name: &'a [u8], value: u32 }, // 358
}

enum FesDataReply {
    Done,
    Word(u32),
    Bytes(Vec<u8>),
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct FesTask {
    id: u32,
    context: Vec<u8>,
    #[serde(skip)]
    external: SharedExternalStorage,
    #[serde(with = "state::runtime")]
    runtime: Option<FesRuntime>,
    initialized: bool,
    requested_name: Option<Vec<u8>>,
    name: Option<Vec<u8>>,
    loaded: bool,
    prepared: bool,
    started: bool,
    paused: u32,
    list: Option<FesListBinding>,
    pending_effect: Option<FesDriverEffect>,
    pending_child: Option<ChildCreation>,
    child_fes: Option<u32>,
    child_kcs: Option<u32>,
    start_children: Option<NativeSubtreeContinuation>,
    start_now: Option<u32>,
    invocation_now: Option<u32>,
    auxiliary_prepared: bool,
    media_frame: Option<u32>,
    system: Option<SystemCommand>,
    load_now: Option<u32>,
    start_complete: bool,
    previous_driver: Option<FesScriptDriver>,
    direct_entry: bool,
    boundary: Option<FesDriverEffect>,
    boundary_notice: bool,
    direct_list: Option<u32>,
    button_refresh: Option<ButtonMediaRefresh>,
    lower_notification: Option<LowerNotification>,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct LowerNotification {
    request: LowerNotificationRequest,
    now: u32,
    entered: bool,
    notifications: Option<FesMediaNotificationScope>,
    pending_effect: Option<FesDriverEffect>,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ButtonMediaRefresh {
    now: u32,
    motion_advanced: bool,
    media_advanced: bool,
    pending_effect: Option<FesDriverEffect>,
    notifications: Option<FesMediaNotificationScope>,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct SystemCommand {
    key: u32,
    value: u32,
    entered: bool,
    block_done: bool,
    media_advanced: bool,
    media_done: bool,
    configuration_phase: u8,
    handler_notice_pending: bool,
    now: u32,
    block: Vec<u8>,
    object: Option<FesDriverTarget>,
    input: Option<FesDriverInput>,
    refresh_media: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum ChildPhase {
    Create,
    Initialize,
    Load,
    Bind,
    Parameters,
    Start,
    Complete,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ChildCreation {
    kind: FesChildKind,
    id: Option<u32>,
    phase: ChildPhase,
    name: Vec<u8>,
    parameters: [i32; 16],
    #[serde(skip)]
    external: SharedExternalStorage,
}

impl FesTaskStore {
    pub(super) fn set_hooks(&mut self, hooks: FesTaskHooks) {
        self.hooks = hooks;
    }

    pub(super) fn register(
        &mut self,
        id: u32,
        context: Vec<u8>,
        external: SharedExternalStorage,
    ) -> Result<()> {
        if self.slots.contains_key(&id) {
            return Err(native_state_error("type-34 task is already registered"));
        }
        if context.len() > 31 || context.contains(&0) {
            return Err(native_state_error(
                "FES creation context exceeds its 31-byte field",
            ));
        }
        self.slots.insert(
            id,
            FesSlot {
                object: Some(FesTask {
                    id,
                    context,
                    external,
                    runtime: None,
                    initialized: false,
                    requested_name: None,
                    name: None,
                    loaded: false,
                    prepared: false,
                    started: false,
                    paused: 0,
                    list: None,
                    pending_effect: None,
                    pending_child: None,
                    child_fes: None,
                    child_kcs: None,
                    start_children: None,
                    start_now: None,
                    invocation_now: None,
                    auxiliary_prepared: false,
                    media_frame: None,
                    system: None,
                    load_now: None,
                    start_complete: false,
                    previous_driver: None,
                    direct_entry: false,
                    boundary: None,
                    boundary_notice: false,
                    direct_list: None,
                    button_refresh: None,
                    lower_notification: None,
                }),
                inputs: VecDeque::new(),
                context_initialized: false,
                named_dwords: BTreeMap::new(),
                save_object: None,
                restore_entry: false,
                pending_input_named: BTreeMap::new(),
                button_media_request: ButtonMediaRequest::Idle,
                lower_notification_request: None,
                program_headers: BTreeSet::from([b"START".to_vec()]),
                pending_entry: None,
                headers_dirty: true,
            },
        );
        Ok(())
    }

    pub(super) fn contains(&self, id: u32) -> bool {
        self.slots.contains_key(&id)
    }

    pub(super) fn context_initialized(&self, id: u32) -> bool {
        self.slots.get(&id).is_some_and(|slot| slot.context_initialized)
    }

    pub(super) fn is_direct_entry(&self, id: u32) -> bool {
        self.slots.get(&id).and_then(|slot| slot.object.as_ref())
            .is_some_and(|object| object.direct_entry)
    }

    pub(super) fn has_direct_entry(&self) -> bool {
        self.slots.values().filter_map(|slot| slot.object.as_ref()).any(|object| object.direct_entry)
    }

    pub(super) fn direct_exit_code(&self, id: u32) -> Option<i32> {
        self.slots.get(&id)?.object.as_ref()?.runtime.as_ref()?.exit_code
    }

    /// Install the explicit diagnostic/root FES entry without a legacy
    /// FesRuntime::advance. The host registers type-34 metadata separately and
    /// advances preparation/start through advance_direct_fes_entry.
    pub(super) fn install_direct_document(
        &mut self,
        id: u32,
        document: FesDocument,
        context: Vec<u8>,
        external: SharedExternalStorage,
    ) -> Result<()> {
        if self.slots.contains_key(&id) {
            return Err(native_state_error("direct FES entry is already installed"));
        }
        self.register(id, context, external)?;
        let runtime = self.slots[&id]
            .object
            .as_ref()
            .unwrap()
            .new_runtime(document);
        let runtime = match runtime {
            Ok(runtime) => runtime,
            Err(error) => {
                self.slots.remove(&id);
                return Err(error);
            }
        };
        self.slots.get_mut(&id).unwrap().program_headers = runtime.driver.as_ref()
            .map(|driver| driver.interpreter.program.blocks.iter()
                .map(|block| block.name.clone()).collect()).unwrap_or_default();
        self.slots.get_mut(&id).unwrap().headers_dirty = false;
        let object = self.slots.get_mut(&id).unwrap().object.as_mut().unwrap();
        object.runtime = Some(runtime);
        object.initialized = true;
        object.loaded = true;
        object.direct_entry = true;
        self.slots.get_mut(&id).unwrap().context_initialized = true;
        Ok(())
    }

    pub(super) fn queue_input(&mut self, id: u32, input: FesDriverInput) -> Result<()> {
        let slot = self
            .slots
            .get_mut(&id)
            .ok_or_else(|| native_state_error("FES input target is not registered"))?;
        if slot.inputs.len() >= 4096 {
            return Err(native_state_error(
                "FES task input queue exceeds 4096 entries",
            ));
        }
        slot.inputs.push_back(input);
        Ok(())
    }

    pub(super) fn runtime(&self, id: u32) -> Option<&FesRuntime> {
        self.slots.get(&id)?.object.as_ref()?.runtime.as_ref()
    }

    pub(super) fn runtimes(&self) -> impl Iterator<Item = (u32, &FesRuntime)> {
        self.slots.iter().filter_map(|(&id, slot)| {
            slot.object
                .as_ref()
                .filter(|object| object.started && object.paused == 0)
                .and_then(|object| object.runtime.as_ref())
                .map(|runtime| (id, runtime))
        })
    }

    pub(super) fn external_binding(&self, id: u32) -> Option<SharedExternalStorage> {
        self.slots
            .get(&id)?
            .object
            .as_ref()
            .map(|object| object.external.clone())
    }

    /// Called only after prepare_fes_task_release completed.
    pub(super) fn release(&mut self, id: u32) {
        self.platform.remove_task(id);
        self.slots.remove(&id);
    }
}

impl HeadlessPlayer {
    /// An entry switch is accepted only between completed logical frames.
    /// Large delete-notification trees retain their release cursor; retrying
    /// load_entry continues that same release without replaying prior calls.
    pub(super) fn retire_direct_fes_for_entry(&mut self) -> anyhow::Result<()> {
        let root = self.current_task_id;
        self.check_scene_message_vm_replacement(root)?;
        if !self.data_services.tasks.fes.is_direct_entry(root) { return Ok(()); }
        if self.frame_phase != super::PlayerFramePhase::Start {
            anyhow::bail!("finish the current logical frame before replacing its direct FES entry");
        }
        let done = self.with_host(|host| Ok(host.release_direct_fes_entry(root, SYNC_BUDGET)?))?;
        if !done {
            anyhow::bail!("direct FES entry release is pending; repeat load_entry to continue the retained cursor");
        }
        for released in self.data_services.tasks.take_released_task_ids() {
            self.data_services.messages.release(released);
            self.data_services.message_device.release(released as i32);
            self.release_plane_state(released);
            let wrappers: Vec<_> = self.data_services.image_wrapper_owners.iter()
                .filter_map(|(&handle, &owner)| (owner == released).then_some(handle)).collect();
            for handle in wrappers {
                self.image_handles.remove(&handle);
                self.data_services.image_wrapper_owners.remove(&handle);
                self.data_services.release_image_pixel_identity(handle);
            }
        }
        self.message_wait_address = None;
        self.pending_messages.clear();
        self.pending_fes_events.clear();
        Ok(())
    }
}

impl LoggingHost<'_> {
    pub(super) fn fes_control_native(
        &mut self, index: u32, args: &[u8], vm: &Vm,
    ) -> Result<NativeReturn> {
        let target = read_u32(args, 0)?;
        cs2_runtime::native_task_profile::exact_task_native_profile(
            self.data_services.native_profile.map_or("", |profile| profile.pe_sha256))
            .and_then(|profile| profile.fes_control)
            .ok_or_else(|| native_state_error("FES control wrapper needs its fixed PE descriptor"))?;
        if index == 450 {
            let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
            return self.select_fes_entry(target, nul(&name))
                .map(|found| NativeReturn::Value(u32::from(found)));
        }
        if index != 666 {
            return Err(cs2_script::ScriptError::UnimplementedNativeCall { index });
        }
        if self.fes_target(target)?
            && self.data_services.tasks.fes.context_initialized(target)
        {
            let values = self.scene_nodes[&target].fes_parameters;
            self.copy_fes_locals_to_task(target, self.current_task_id, &values);
        }
        Ok(NativeReturn::Continue)
    }

    fn select_fes_entry(&mut self, target: u32, name: &[u8]) -> Result<bool> {
        if name.len() > 31 || name.contains(&0) {
            return Err(native_state_error("FES entry name exceeds its 31-byte controller field"));
        }
        if !self.fes_target(target)? { return Ok(false); }
        let slot = self.data_services.tasks.fes.slots.get_mut(&target).unwrap();
        if !slot.context_initialized || !slot.program_headers.contains(name) { return Ok(false); }
        let driver = slot.object.as_mut()
            .filter(|object| object.button_refresh.is_none() && object.lower_notification.is_none())
            .and_then(|object| object.runtime.as_mut())
            .and_then(|runtime| runtime.driver.as_mut())
            .filter(|driver| !driver.media_statement_pending());
        if let Some(driver) = driver {
            if !driver.select_main_block(name) {
                return Err(native_state_error("FES live entry table differs from its registered headers"));
            }
        } else {
            slot.pending_entry = Some(name.to_vec());
        }
        Ok(true)
    }

    pub(super) fn refresh_fes_button_media(&mut self, owner: u32, budget: usize) -> Result<bool> {
        if budget == 0 { return Ok(false); }
        let slot = self.data_services.tasks.fes.slots.get_mut(&owner)
            .ok_or_else(|| native_state_error("FES BUTTON media owner is absent"))?;
        if slot.button_media_request == ButtonMediaRequest::Complete {
            slot.button_media_request = ButtonMediaRequest::Idle;
            return Ok(true);
        }
        if slot.object.is_none() {
            // The outer effect must first return its real interpreter. The
            // ownership boundary below performs this request before retrying
            // the sender; acknowledgement never fabricates a media refresh.
            slot.button_media_request = ButtonMediaRequest::Requested;
            return Ok(false);
        }
        let done = self.with_fes_task(owner, |host, object| object.refresh_button_media(host, budget))?;
        if done {
            self.data_services.tasks.fes.slots.get_mut(&owner).unwrap().button_media_request =
                ButtonMediaRequest::Idle;
        }
        Ok(done)
    }

    pub(super) fn write_fes_button_named_input(
        &mut self, owner: u32, name: &'static str, value: i32,
    ) -> Result<()> {
        if name.is_empty() || name.len() > 127 || name.as_bytes().contains(&0) {
            return Err(native_state_error("FES BUTTON named input is invalid"));
        }
        let slot = self.data_services.tasks.fes.slots.get_mut(&owner)
            .ok_or_else(|| native_state_error("FES BUTTON named input owner is absent"))?;
        if let Some(object) = slot.object.as_mut() {
            let driver = object.runtime.as_mut().and_then(|runtime| runtime.driver.as_mut())
                .ok_or_else(|| native_state_error("FES BUTTON named input has no live driver"))?;
            driver.set_named_value(&name, 0, value);
        } else {
            if slot.pending_input_named.len() >= 64 && !slot.pending_input_named.contains_key(name) {
                return Err(native_state_error("FES BUTTON named input queue exceeds 64 names"));
            }
            slot.pending_input_named.insert(name.to_owned(), value);
        }
        Ok(())
    }

    pub(super) fn refresh_fes_lower_control(&mut self, owner: u32, lower: u32) -> Result<bool> {
        self.with_fes_task(owner, |host, object| {
            let now = host.data_services.tasks.task_clock(owner)
                .map_or(0, |clock| clock.elapsed());
            let view = object.view(host, now);
            let runtime = object.runtime.as_mut()
                .ok_or_else(|| native_state_error("lower control owner has no FES runtime"))?;
            let driver = runtime.driver.take()
                .ok_or_else(|| native_state_error("lower control owner driver is executing"))?;
            let result = super::fes_controls::refresh_lower(host, &view, runtime, &driver, lower)
                .map_err(|error| host.fes_task_error(owner, error));
            runtime.driver = Some(driver);
            result
        })
    }

    pub(super) fn notify_fes_lower_control(
        &mut self, notification: &super::fes_controls::ControlNotification, budget: usize,
    ) -> Result<bool> {
        let owner = notification.owner;
        let event = &notification.event;
        event.event.as_deref()
            .ok_or_else(|| native_state_error("lower control notification has no explicit event"))?;
        let slot = self.data_services.tasks.fes.slots.get_mut(&owner)
            .ok_or_else(|| native_state_error("lower control notification owner is absent"))?;
        if let Some(request) = &slot.lower_notification_request {
            if !request.matches(notification.command, event) {
                return Err(native_state_error("nested FES input notification changed its packet"));
            }
            if request.complete {
                slot.lower_notification_request = None;
                return Ok(true);
            }
        }
        if slot.object.is_none() {
            slot.lower_notification_request.get_or_insert_with(|| LowerNotificationRequest {
                command: notification.command, input: event.clone(), complete: false,
            });
            return Ok(false);
        }
        let done = self.with_fes_task(owner, |host, object|
            object.notify_lower_input(host, notification.command, event, budget))?;
        if done {
            self.data_services.tasks.fes.slots.get_mut(&owner).unwrap()
                .lower_notification_request = None;
        }
        Ok(done)
    }

    /// Entry installation does not execute a script or create media children.
    /// A nonempty external area retains its shared identity across the root,
    /// child FES objects, and execkcs-created ordinary tasks.
    pub(super) fn begin_direct_fes_entry(
        &mut self, id: u32, document: FesDocument, external: SharedExternalStorage,
    ) -> Result<()> {
        let context = self.data_services.tasks.task_context(id).unwrap_or_default().to_vec();
        self.begin_direct_fes_entry_with_context(id, document, &context, external)
    }

    fn begin_direct_fes_entry_with_context(
        &mut self, id: u32, document: FesDocument, context: &[u8],
        external: SharedExternalStorage,
    ) -> Result<()> {
        if self.data_services.tasks.fes.contains(id) {
            return Err(native_state_error("release the current FES entry before installing another root"));
        }
        if self.task_parents.iter().any(|(&child, &parent)| child != id && parent == id) {
            return Err(native_state_error("release the current root's child tree before installing a direct FES entry"));
        }
        self.data_services.tasks.fes.install_direct_document(
            id, document, context.to_vec(), external.clone(),
        )?;
        if let Err(error) = self.data_services.tasks.register_direct_fes_task(
            id, self.task_time, context, external.clone(), &self.data_services.language.languages,
        ) {
            self.data_services.tasks.fes.release(id);
            return Err(error);
        }
        self.data_services.tasks.set_factory_external(external);
        self.scene_nodes.insert(id, super::SceneNode::new(id, 34, id, "direct FES".into()));
        self.scene_nodes.get_mut(&id).unwrap().initialized = true;
        if id == self.current_task_id {
            self.initialize_task_values(u32::try_from(self.configured_shared_slots)
                .map_err(|error| native_state_error_owned(format!("configured FES shared slot count exceeds DWORD: {error}")))?)?;
        }
        Ok(())
    }

    /// Called once per same-frame budget slice after the ordinary child-tree
    /// stage has completed. The object's stage flags prevent repeated load,
    /// child creation, or immediate-start work.
    pub(super) fn advance_direct_fes_entry(
        &mut self, id: u32, now: u32, budget: usize,
    ) -> Result<bool> {
        if !self.data_services.tasks.fes.is_direct_entry(id) {
            return Err(native_state_error("root FES driver entry is not installed"));
        }
        if budget == 0 { return Ok(false); }
        if !self.update_task_command_buffers(id, budget)? { return Ok(false); }
        self.with_fes_task(id, |host, object| {
            if object.list.is_none() {
                let list = match object.direct_list {
                    Some(list) => list,
                    None => {
                        let list = host.create_task_object(15, id, b"direct FES list", b"")?;
                        object.direct_list = Some(list);
                        list
                    }
                };
                let size = host.canvas.as_ref().map(|canvas| [canvas.width(), canvas.height()])
                    .ok_or_else(|| native_state_error("direct FES list requires a render context"))?;
                host.initialize_fes_image_list(list, size[0], size[1])
                    .map_err(|error| host.fes_task_error(id, error))?;
                object.bind_list(host, list)?;
            }
            if !object.prepared {
                object.load_now.get_or_insert(now);
                if !object.prepare_script(host, budget)? { return Ok(false); }
                object.prepared = true;
                object.load_now = None;
            }
            if !object.started || object.start_now.is_some() {
                return object.start(host, budget);
            }
            object.update(host, now, budget)
        })
    }

    /// Explicit FES diagnostics use the same task state and CPU services as
    /// native-created FES objects. The player must first stop/drop its root
    /// VM and reset its frame-stage cursor before changing entry kind.
    pub(super) fn install_direct_fes_document(
        &mut self,
        id: u32,
        document: FesDocument,
        context: &[u8],
        external: SharedExternalStorage,
        list: Option<u32>,
        budget: usize,
    ) -> Result<bool> {
        if !self.data_services.tasks.fes.contains(id) {
            self.begin_direct_fes_entry_with_context(id, document, context, external)?;
        }
        if let Some(list) = list {
            self.with_fes_task(id, |host, object| {
                if object
                    .list
                    .as_ref()
                    .is_none_or(|binding| binding.task != list)
                {
                    object.bind_list(host, list)?;
                }
                Ok(())
            })?;
        }
        self.advance_direct_fes_entry(id, self.task_time, budget)
    }

    /// One typed receiver backs all seven wrappers. Void wrappers ignore a
    /// missing/uninitialized receiver; word queries require an actual reply.
    pub(super) fn fes_data_native(
        &mut self, index: u32, args: &[u8], vm: &Vm,
    ) -> Result<NativeReturn> {
        let first = read_u32(args, 0)?;
        if matches!(index, 649 | 650) {
            let container = first;
            if container == 0 {
                return Ok(NativeReturn::Value(0));
            }
            let record = read_u32(args, 4)?;
            let target = read_u32(args, 8)?;
            if index == 649 {
                let Some(FesDataReply::Bytes(bytes)) =
                    self.fes_data_command(target, FesDataCommand::SaveParameters)?
                else {
                    return Ok(NativeReturn::Value(0));
                };
                self.css_handles.get_mut(&container)
                    .ok_or_else(|| native_state_error("CSS container handle is invalid"))?
                    .append(record, &bytes)
                    .map_err(|error| native_state_error_owned(format!("FES local parameter append failed: {error:#}")))?;
            } else {
                let Some(bytes) = self.css_handles.get(&container)
                    .ok_or_else(|| native_state_error("CSS container handle is invalid"))?
                    .try_record(record)
                    .map_err(|error| native_state_error_owned(format!("FES parameter CSS record storage is malformed: {error:#}")))?
                    .map(ToOwned::to_owned)
                else {
                    return Ok(NativeReturn::Value(0));
                };
                // The wrapper returns one after finding the CSS record,
                // independently of whether command 350 found a receiver.
                self.fes_data_command(target, FesDataCommand::RestoreParameters(&bytes))?;
            }
            return Ok(NativeReturn::Value(1));
        }
        let request = match index {
            451 => FesDataCommand::SaveObject(read_u32(args, 4)?),
            645 => FesDataCommand::SetParameter {
                index: read_u32(args, 4)?, value: read_u32(args, 8)?,
            },
            646 => FesDataCommand::GetParameter(read_u32(args, 4)?),
            882 | 883 => {
                // Full formatting precedes dispatch even for target zero.
                let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
                let reply = if index == 882 {
                    self.fes_data_command(first, FesDataCommand::SetNamed {
                        name: nul(&name), value: read_u32(args, 8)?,
                    })?
                } else {
                    self.fes_data_command(first, FesDataCommand::GetNamed(nul(&name)))?
                };
                return if index == 882 {
                    Ok(NativeReturn::Continue)
                } else {
                    match reply {
                        Some(FesDataReply::Word(value)) => Ok(NativeReturn::Value(value)),
                        _ => Err(native_state_error("FES named query has no initialized receiver")),
                    }
                };
            }
            _ => return Err(native_state_error("native has no FES data receiver contract")),
        };
        let reply = self.fes_data_command(first, request)?;
        if index == 646 {
            match reply {
                Some(FesDataReply::Word(value)) => Ok(NativeReturn::Value(value)),
                _ => Err(native_state_error("FES parameter query has no initialized receiver")),
            }
        } else {
            Ok(NativeReturn::Continue)
        }
    }

    fn fes_data_command(
        &mut self, target: u32, command: FesDataCommand<'_>,
    ) -> Result<Option<FesDataReply>> {
        if !self.task_is_reachable(target)
            || self.scene_nodes.get(&target).is_none_or(|node| node.kind != 34)
            || self.data_services.tasks.message_task_state(target)
                .is_none_or(|(_, deleted, _)| deleted != 0)
        {
            return Ok(None);
        }
        let Some(slot) = self.data_services.tasks.fes.slots.get_mut(&target)
            .filter(|slot| slot.context_initialized)
        else {
            return Ok(None);
        };
        let parameters = &mut self.scene_nodes.get_mut(&target).unwrap().fes_parameters;
        let reply = match command {
            FesDataCommand::SaveObject(value) => {
                // This DWORD is an opaque value, not a pointer to dereference.
                slot.save_object = Some(value);
                FesDataReply::Done
            }
            FesDataCommand::SetParameter { index, value } => {
                if let Some(parameter) = parameters.get_mut(index as usize) {
                    *parameter = value as i32;
                }
                FesDataReply::Done
            }
            FesDataCommand::GetParameter(index) => FesDataReply::Word(
                parameters.get(index as usize).copied().unwrap_or(0) as u32,
            ),
            FesDataCommand::SaveParameters => FesDataReply::Bytes(
                parameters.iter().flat_map(|word| word.to_le_bytes()).collect(),
            ),
            FesDataCommand::RestoreParameters(bytes) => {
                for (parameter, chunk) in parameters.iter_mut().zip(bytes.chunks(4)) {
                    let mut word = parameter.to_le_bytes();
                    word[..chunk.len()].copy_from_slice(chunk);
                    *parameter = i32::from_le_bytes(word);
                }
                slot.restore_entry |= !bytes.is_empty();
                FesDataReply::Done
            }
            FesDataCommand::GetNamed(name) => {
                // This task-owned358 table stays in the slot while its driver
                // is borrowed by a synchronous KCS child or media callback.
                FesDataReply::Word(slot.named_dwords.get(nul(name)).copied().unwrap_or(0))
            }
            FesDataCommand::SetNamed { name, value } => {
                slot.named_dwords.insert(nul(name).to_vec(), value);
                FesDataReply::Done
            }
        };
        // Idle objects also expose these changes to reload/snapshot callers.
        // Executing objects absorb them after the nested host call returns.
        let entry_ready = slot.object.as_ref().is_some_and(|object|
            object.button_refresh.is_none() && object.lower_notification.is_none());
        if let Some(driver) = slot.object.as_mut()
            .and_then(|object| object.runtime.as_mut())
            .and_then(|runtime| runtime.driver.as_mut())
        {
            driver.interpreter.local_values_mut().copy_from_slice(parameters);
            if let Some(value) = slot.save_object.take() {
                driver.set_named_value("_SAVEOBJ_", 0, value as i32);
            }
            if entry_ready && !driver.media_statement_pending()
                && std::mem::take(&mut slot.restore_entry)
            {
                // 350 changes the parked main position. A synchronous load
                // inside a handler must finish that same handler first.
                driver.select_main_block(b"_LOAD_");
            }
        }
        let values = *parameters;
        self.sync_fes_local_region(target, &values);
        Ok(Some(reply))
    }

    /// Receiver for platform-originated FES commands. Pointer-shaped DWORDs
    /// are only presence flags: their data is supplied in explicit buffers.
    /// Some(false) retains the existing load/start/system continuation.
    pub(super) fn receive_fes_command(
        &mut self, command: cs2_runtime::TaskCommand,
        buffers: &mut cs2_runtime::TaskCommandBuffers<'_>, output: &mut u32,
        budget: usize,
    ) -> Result<Option<bool>> {
        if !matches!(command.code, 337..=350 | 353 | 357 | 358)
            || matches!(command.code, 344 | 345)
        {
            return Ok(None);
        }
        let id = command.target as u32;
        if !self.fes_target(id)? { return Ok(Some(true)); }
        if command.code == 337 {
            if command.data[1] != 0 {
                return Err(native_state_error("FES 337 uses implicit shared state, not a host pointer"));
            }
            self.bind_fes_shared_values(id, command.sender as u32)?;
            self.with_fes_task(id, |host, object| object.initialize(host))?;
            return Ok(Some(true));
        }
        // 449 may bind a newly created type-34 object before 447 sends
        // 337/338. The list belongs to the task, not its execution context.
        if command.code == 340 {
            self.with_fes_task(id, |host, object| object.bind_list(host, command.data[1]))?;
            return Ok(Some(true));
        }
        if !self.data_services.tasks.fes.slots[&id].context_initialized {
            if command.code == 349 { *output = 0; }
            return Ok(Some(true));
        }
        let done = match command.code {
            341 => {
                let name = nul(buffers.data1.as_deref()
                    .ok_or_else(|| native_state_error("FES entry selection requires its name buffer"))?);
                *output = u32::from(self.select_fes_entry(id, name)?);
                true
            }
            353 => {
                if command.data[1] != command.sender as u32 {
                    return Err(native_state_error("FES local copy requires the actual sender task binding"));
                }
                let values = self.scene_nodes[&id].fes_parameters;
                self.copy_fes_locals_to_task(id, command.sender as u32, &values);
                true
            }
            338 => {
                let name = nul(buffers.data1.as_deref()
                    .ok_or_else(|| native_state_error("FES 338 requires a filename buffer"))?);
                if name.len() > 259 {
                    return Err(native_state_error("FES 338 filename exceeds 259 bytes"));
                }
                let resolved = String::from_utf8(cp932_to_utf8(name)).expect("CP932 emits UTF-8");
                self.with_fes_task(id, |host, object|
                    object.load(host, name, &[resolved], budget))?
            }
            339 => self.with_fes_task(id, |host, object| object.start(host, budget))?,
            343 => {
                let name = nul(buffers.data1.as_deref()
                    .ok_or_else(|| native_state_error("FES 343 requires a block name buffer"))?);
                if name.len() > 63 {
                    return Err(native_state_error("FES 343 block name exceeds 63 bytes"));
                }
                let object = if command.data[0] != 0 {
                    let bytes = buffers.data0.and_then(|bytes| bytes.get(..36))
                        .ok_or_else(|| native_state_error("FES 343 requires 36 object-context bytes"))?;
                    let field = &bytes[..32];
                    let prefix = field.iter().position(|&byte| byte == 0)
                        .ok_or_else(|| native_state_error("FES object-context name is not terminated"))?;
                    (prefix != 0).then(|| FesDriverTarget {
                        name: field[..prefix].to_vec(),
                        bank: u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as i32,
                        default_group: None, value: -1,
                    })
                } else { None };
                let bank = object.as_ref().map_or(0, |object| object.bank as u32);
                self.with_fes_task(id, |host, task|
                    task.system_command_with_context(host, 343, bank, name, object, budget))?
            }
            348 => self.with_fes_task(id, |host, object|
                object.set_paused(host, command.data[1], budget))?,
            342 | 346 | 347 | 349 | 350 | 357 | 358 => {
                let request = match command.code {
                    342 => FesDataCommand::SaveObject(command.data[1]),
                    346 => FesDataCommand::SetParameter {
                        index: command.data[0], value: command.data[1],
                    },
                    347 => FesDataCommand::GetParameter(command.data[0]),
                    349 => FesDataCommand::SaveParameters,
                    350 => {
                        let count = (command.data[0] as usize).min(2048);
                        let bytes = if count == 0 { &[][..] } else {
                            buffers.data1.as_deref().and_then(|bytes| bytes.get(..count))
                                .ok_or_else(|| native_state_error("FES 350 source buffer is truncated"))?
                        };
                        FesDataCommand::RestoreParameters(bytes)
                    }
                    357 => FesDataCommand::GetNamed(buffers.data0
                        .ok_or_else(|| native_state_error("FES 357 requires a name buffer"))?),
                    358 => FesDataCommand::SetNamed {
                        name: buffers.data0
                            .ok_or_else(|| native_state_error("FES 358 requires a name buffer"))?,
                        value: command.data[1],
                    },
                    _ => unreachable!("FES data receiver code was filtered"),
                };
                let reply = self.fes_data_command(id, request)?;
                match (command.code, reply) {
                    (347, Some(FesDataReply::Word(value))) => *output = value,
                    (357, Some(FesDataReply::Word(value))) => {
                        buffers.data1.as_deref_mut().and_then(|bytes| bytes.get_mut(..4))
                            .ok_or_else(|| native_state_error("FES 357 output requires four bytes"))?
                            .copy_from_slice(&value.to_le_bytes());
                    }
                    (349, Some(FesDataReply::Bytes(bytes))) => {
                        if let Some(buffer) = buffers.data1.as_deref_mut() {
                            if command.data[0] as usize >= bytes.len() {
                                let target = buffer.get_mut(..bytes.len())
                                    .ok_or_else(|| native_state_error("FES 349 output buffer is truncated"))?;
                                target.copy_from_slice(&bytes);
                                *output = bytes.len() as u32;
                            } else {
                                *output = 0;
                            }
                        } else {
                            *output = bytes.len() as u32;
                        }
                    }
                    _ => {}
                }
                true
            }
            _ => return Ok(None),
        };
        Ok(Some(done))
    }

    pub(super) fn fes_named_block_native(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let id = read_u32(args, 0)?;
        let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        let prefix = expand_kcs_format(vm, &vm.read_string(read_u32(args, 8)?)?, 0)?;
        let bank = read_u32(args, 12)?;
        if nul(&name).len() > 63 || nul(&prefix).len() > 31 {
            return Err(native_state_error(
                "FES named block/context exceeds its ABI field",
            ));
        }
        if !self.fes_target(id)? {
            return Ok(NativeReturn::Continue);
        }
        let object_context = (!nul(&prefix).is_empty()).then(|| FesDriverTarget {
            name: nul(&prefix).to_vec(), bank: bank as i32,
            default_group: None, value: -1,
        });
        let done = self.with_fes_task(id, |host, object| {
            object.system_command_with_context(
                host, 343, bank, nul(&name), object_context, SYNC_BUDGET,
            )
        })?;
        Ok(if done {
            NativeReturn::Continue
        } else {
            NativeReturn::Retry
        })
    }

    /// All three ordinary wrappers return status two without replacing a
    /// native result slot. Retry retains the exact VM call and object stages.
    pub(super) fn fes_native(&mut self, index: u32, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let previous = self.data_services.tasks.executing_external;
        if previous.is_none() {
            self.data_services.tasks.executing_external = Some(self.current_task_id);
        }
        let result = self.fes_native_inner(index, args, vm);
        self.data_services.tasks.executing_external = previous;
        result
    }

    fn fes_native_inner(&mut self, index: u32, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let target = read_u32(args, 0)?;
        let done = match index {
            447 => {
                let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
                let name = nul(&name);
                // Expansion and category resolution precede target dispatch,
                // including an empty name and target zero.
                let names = self.fes_resource_names(name)?;
                if !self.fes_target(target)? {
                    return Ok(NativeReturn::Continue);
                }
                self.bind_fes_shared_values(target, self.current_task_id)?;
                self.with_fes_task(target, |host, object| {
                    object.initialize(host)?;
                    object.load(host, name, &names, SYNC_BUDGET)
                })?
            }
            448 => {
                if !self.fes_target(target)? {
                    return Ok(NativeReturn::Continue);
                }
                self.with_fes_task(target, |host, object| object.start(host, SYNC_BUDGET))?
            }
            449 => {
                let list = read_u32(args, 4)?;
                if !self.fes_target(target)? {
                    return Ok(NativeReturn::Continue);
                }
                self.with_fes_task(target, |host, object| object.bind_list(host, list))?;
                true
            }
            _ => return Err(native_state_error("unsupported FES task native")),
        };
        Ok(if done {
            NativeReturn::Continue
        } else {
            NativeReturn::Retry
        })
    }

    pub(super) fn update_fes_task(&mut self, id: u32, now: u32, budget: usize) -> Result<bool> {
        if !self.fes_target(id)? {
            return Ok(true);
        }
        if self.data_services.tasks.fes.is_direct_entry(id) {
            return self.advance_direct_fes_entry(id, now, budget);
        }
        self.with_fes_task(id, |host, object| {
            if object.button_refresh.is_some() || object.lower_notification.is_some() {
                // These may have been requested by this body's own pending
                // effect. Keep the logical frame until the parked caller is
                // restored; a true result here could skip that caller's tail.
                return Ok(false);
            }
            if object.load_now.is_some() || object.start_now.is_some() || object.system.is_some()
            {
                // A retained native command owns the continuation; ordinary
                // tree bodies must not enter its driver at a different time.
                return Ok(true);
            }
            object.update(host, now, budget)
        })
    }

    /// Configuration notification is synchronous, while ordinary messages
    /// produced by child scripts still keep their own continuation state.
    pub(super) fn notify_fes_configuration(&mut self, id: u32, budget: usize) -> Result<bool> {
        if !self.fes_target(id)? {
            return Ok(true);
        }
        self.with_fes_task(id, |host, object| {
            object.system_command(host, 19, 0, b"MES_CHANGECONFIG", budget)
        })
    }

    pub(super) fn fes_pause_native(&mut self, args: &[u8]) -> Result<NativeReturn> {
        let id = read_u32(args, 0)?;
        let value = read_u32(args, 4)?;
        if !self.fes_target(id)? {
            return Ok(NativeReturn::Continue);
        }
        let done = self.with_fes_task(id, |host, object| {
            object.set_paused(host, value, SYNC_BUDGET)
        })?;
        Ok(if done {
            NativeReturn::Continue
        } else {
            NativeReturn::Retry
        })
    }

    /// Type-34 prepare_release restores shared-list submission and removes
    /// input/graphics registrations through the real platform service.
    pub(super) fn prepare_fes_task_release(&mut self, id: u32) -> Result<bool> {
        if !self.data_services.tasks.fes.contains(id) {
            return Ok(true);
        }
        self.with_fes_task(id, |host, object| {
            let Some(runtime) = object.runtime.as_mut() else {
                return Ok(true);
            };
            let hook = host
                .data_services
                .tasks
                .fes
                .hooks
                .release
                .ok_or_else(|| native_state_error("FES resource release platform is absent"))?;
            let view = object_view(
                host,
                object.id,
                &object.context,
                &object.list,
                object.invocation_now.unwrap_or(host.task_time),
                object.paused,
            );
            hook(host, &view, runtime).map_err(|error| host.fes_task_error(id, error))
        })
    }

    fn fes_target(&mut self, id: u32) -> Result<bool> {
        if !self.task_is_reachable(id) { return Ok(false); }
        let Some(node) = self.scene_nodes.get(&id) else {
            return Ok(false);
        };
        if node.kind != 34 {
            return Ok(false);
        }
        self.ensure_task_record(id)?;
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_some_and(|(_, deletion, _)| deletion != 0)
        {
            return Ok(false);
        }
        if !self.data_services.tasks.fes.contains(id) {
            let context = self
                .data_services
                .tasks
                .task_context(id)
                .unwrap_or_default()
                .to_vec();
            let external = self
                .data_services
                .tasks
                .task_external(id)
                .unwrap_or_default();
            self.data_services
                .tasks
                .fes
                .register(id, context, external)?;
        }
        Ok(true)
    }

    fn bind_fes_shared_values(&mut self, id: u32, sender: u32) -> Result<()> {
        if self.data_services.tasks.fes.context_initialized(id) {
            return Ok(());
        }
        // 447 passes the caller's numeric context to 337. A FES object's
        // tree parent may be an unbound container, independent of that caller.
        let area = self.data_services.tasks.records.get(&sender)
            .ok_or_else(|| native_state_error("FES initialization sender is absent"))?
            .shared_area;
        let record = self.data_services.tasks.records.get_mut(&id)
            .ok_or_else(|| native_state_error("FES initialization target is absent"))?;
        let previous = record.shared_area;
        record.shared_area = area;
        if previous != area {
            self.release_task_values(previous);
        }
        Ok(())
    }

    fn with_fes_task<T>(
        &mut self,
        id: u32,
        call: impl FnOnce(&mut Self, &mut FesTask) -> Result<T>,
    ) -> Result<T> {
        if self.data_services.tasks.fes.slots.get(&id)
            .is_none_or(|slot| slot.object.is_none())
        {
            return Err(native_state_error("FES task is absent or already executing"));
        }
        let values = self.enter_task_values(id)?;
        let slot = self
            .data_services
            .tasks
            .fes
            .slots
            .get_mut(&id)
            .ok_or_else(|| native_state_error("FES task storage is unavailable"))?;
        let mut object = slot
            .object
            .take()
            .ok_or_else(|| native_state_error("FES task is already executing"))?;
        if let Some(context) = self.data_services.tasks.task_context(id) {
            object.context = context.to_vec();
        }
        let clock = self.data_services.tasks.task_clock(id).unwrap_or_default();
        let saved = (self.current_task_id, self.task_time, self.task_origin);
        self.current_task_id = id;
        self.task_time = clock.current;
        self.task_origin = clock.origin;
        let mut result = call(self, &mut object);
        let lower = self.data_services.tasks.fes.slots[&id].lower_notification_request.as_ref()
            .filter(|request| !request.complete)
            .map(|request| (request.command, request.input.clone()));
        if result.is_ok() && let Some((command, input)) = lower {
            match object.notify_lower_input(self, command, &input, SYNC_BUDGET) {
                Ok(true) => {
                    self.data_services.tasks.fes.slots.get_mut(&id).unwrap()
                        .lower_notification_request.as_mut().unwrap().complete = true;
                }
                Ok(false) => {}
                Err(error) => result = Err(error),
            }
        }
        if result.is_ok() && self.data_services.tasks.fes.slots[&id].button_media_request
            == ButtonMediaRequest::Requested
        {
            match object.refresh_button_media(self, SYNC_BUDGET) {
                Ok(true) => {
                    self.data_services.tasks.fes.slots.get_mut(&id).unwrap().button_media_request =
                        ButtonMediaRequest::Complete;
                }
                Ok(false) => {}
                Err(error) if result.is_ok() => result = Err(error),
                Err(_) => {}
            }
        }
        (self.current_task_id, self.task_time, self.task_origin) = saved;
        self.leave_task_values(values);
        let slot = self
            .data_services
            .tasks
            .fes
            .slots
            .get_mut(&id)
            .expect("FES release is deferred while its object executes");
        slot.object = Some(object);
        result
    }

    fn fes_resource_names(&self, name: &[u8]) -> Result<Vec<String>> {
        let name = String::from_utf8(cp932_to_utf8(nul(name))).expect("CP932 emits UTF-8");
        let names = self
            .resources
            .category_names(b"fes", &name)
            .map_err(|error| native_state_error_owned(format!("FES resource category resolution failed: {error:#}")))?;
        for name in &names {
            let (bytes, _, invalid) = encoding_rs::SHIFT_JIS.encode(name);
            if invalid || bytes.len() > 259 {
                return Err(native_state_error(
                    "FES resolved filename exceeds its CP932 259-byte field",
                ));
            }
        }
        Ok(names)
    }

    fn fes_task_error(
        &mut self,
        id: u32,
        error: impl std::fmt::Display,
    ) -> cs2_script::ScriptError {
        let detail = format!("FES task={id}: {error:#}");
        self.events.push(LogicEvent {
            frame: self.frame,
            kind: "task-error".into(),
            detail: detail.clone(),
        });
        native_state_error_owned(detail)
    }
}

impl FesTask {
    /// The sender waits for this exact input handler, while an already
    /// suspended scene/system handler keeps its own queues and pending host
    /// effect. Both operate on the same interpreter, locals and named bank.
    fn notify_lower_input(
        &mut self, host: &mut LoggingHost<'_>, command: cs2_runtime::TaskCommand,
        input: &FesDriverInput, budget: usize,
    ) -> Result<bool> {
        if self.button_refresh.is_some() || budget == 0 { return Ok(false); }
        if !self.initialized {
            return Err(native_state_error("FES input notification has no initialized owner"));
        }
        if self.lower_notification.is_none() && self.runtime.as_ref()
            .and_then(|runtime| runtime.driver.as_ref())
            .is_some_and(FesScriptDriver::initialization_pending)
        {
            // Leave the declaration loader free to continue. Installing an
            // input cursor here would block that loader while waiting for it.
            return Ok(false);
        }
        if let Some(pending) = &self.lower_notification {
            if !pending.request.matches(command, input) {
                return Err(native_state_error("FES input handler continuation changed its packet"));
            }
        } else {
            self.lower_notification = Some(LowerNotification {
                request: LowerNotificationRequest { command, input: input.clone(), complete: false },
                now: self.system.as_ref().map(|system| system.now)
                    .unwrap_or_else(|| host.data_services.tasks.task_clock(self.id)
                        .map_or(0, |clock| clock.elapsed())),
                entered: false, notifications: None, pending_effect: None,
            });
        }
        let mut runtime = self.runtime.take()
            .ok_or_else(|| native_state_error("FES input notification runtime is executing"))?;
        let Some(mut driver) = runtime.driver.take() else {
            self.runtime = Some(runtime);
            return Err(native_state_error("FES input notification driver is executing"));
        };
        let now = self.lower_notification.as_ref().unwrap().now;
        let result = (|| {
            self.apply_data_commands(host, &mut driver);
            if driver.initialization_pending() { return Ok(false); }
            if self.lower_notification.as_ref().unwrap().notifications.is_none() {
                self.lower_notification.as_mut().unwrap().notifications =
                    Some(driver.begin_media_notification_scope());
            }
            if !self.lower_notification.as_ref().unwrap().entered {
                driver.on_input(input.clone()).map_err(|error| host.fes_task_error(self.id, error))?;
                self.lower_notification.as_mut().unwrap().entered = true;
            }
            for iteration in 0..budget {
                let pending = self.lower_notification.as_mut().unwrap().pending_effect.take();
                if pending.is_none() && driver.media_statement_result_ready() {
                    if !super::fes_task_platform::finish_task_media(
                        host, &self.view(host, now), &mut runtime, &mut driver,
                    ).map_err(|error| host.fes_task_error(self.id, error))? {
                        if driver.media_statement_pending() { continue; }
                        return Ok(false);
                    }
                    continue;
                }
                if pending.is_none() && !driver.has_media_notification_work() {
                    let scope = self.lower_notification.as_mut().unwrap().notifications.take()
                        .expect("input notification owns its caller's queues");
                    driver.end_media_notification_scope(scope);
                    self.lower_notification = None;
                    return Ok(true);
                }
                let effect = match pending {
                    Some(effect) => effect,
                    None => with_expression(host, |_, shared|
                        driver.advance_media_notification(now as u64, shared))
                        .map_err(|error| host.fes_task_error(self.id, error))?
                        .ok_or_else(|| native_state_error("FES input notification lost its cursor"))?,
                };
                sync_locals(host, self.id, &driver);
                let result = self.effect(host, &mut runtime, &mut driver, &effect, now, budget - iteration);
                self.apply_data_commands(host, &mut driver);
                match result {
                    Ok(EffectResult::Continue | EffectResult::Boundary | EffectResult::Yield) => {}
                    Ok(EffectResult::Pending) => {
                        self.lower_notification.as_mut().unwrap().pending_effect = Some(effect);
                        return Ok(false);
                    }
                    Err(error) => {
                        self.lower_notification.as_mut().unwrap().pending_effect = Some(effect);
                        return Err(error);
                    }
                }
            }
            Ok(false)
        })();
        sync_locals(host, self.id, &driver);
        runtime.driver = Some(driver);
        self.runtime = Some(runtime);
        result
    }

    fn refresh_button_media(&mut self, host: &mut LoggingHost<'_>, budget: usize) -> Result<bool> {
        if !self.initialized || self.runtime.is_none() {
            return Err(native_state_error("FES BUTTON media owner has no initialized runtime"));
        }
        if self.button_refresh.is_none() {
            if self.runtime.as_ref().and_then(|runtime| runtime.driver.as_ref())
                .is_some_and(FesScriptDriver::initialization_pending)
            {
                return Ok(false);
            }
            let now = self.system.as_ref().map(|system| system.now)
                .unwrap_or_else(|| host.data_services.tasks.task_clock(self.id)
                .map_or(0, |clock| clock.elapsed()));
            self.button_refresh = Some(ButtonMediaRefresh {
                now, motion_advanced: false, media_advanced: false,
                pending_effect: None, notifications: None,
            });
        }
        let mut runtime = self.runtime.take()
            .ok_or_else(|| native_state_error("FES BUTTON media runtime is executing"))?;
        let Some(mut driver) = runtime.driver.take() else {
            self.runtime = Some(runtime);
            return Err(native_state_error("FES BUTTON media driver is executing"));
        };
        let now = self.button_refresh.as_ref().unwrap().now;
        let result = (|| {
            self.apply_data_commands(host, &mut driver);
            if self.button_refresh.as_ref().unwrap().notifications.is_none() {
                self.button_refresh.as_mut().unwrap().notifications =
                    Some(driver.begin_media_notification_scope());
            }
            for iteration in 0..budget {
                let remaining = budget - iteration;
                let pending = self.button_refresh.as_mut().unwrap().pending_effect.take();
                if pending.is_some() || driver.media_statement_pending()
                    || (driver.has_media_notification_work()
                        && !driver.media_statement_result_ready())
                {
                    let effect = match pending {
                        Some(effect) => effect,
                        None => with_expression(host, |_, shared|
                            driver.advance_media_notification(now as u64, shared))
                            .map_err(|error| host.fes_task_error(self.id, error))?
                            .ok_or_else(|| native_state_error("FES media notification lost its cursor"))?,
                    };
                    sync_locals(host, self.id, &driver);
                    let outcome = self.effect(host, &mut runtime, &mut driver, &effect, now, remaining);
                    self.apply_data_commands(host, &mut driver);
                    match outcome {
                        Ok(EffectResult::Continue | EffectResult::Boundary) => {}
                        // One instruction is intentional; spend the rest of
                        // this host slice only on the notification queues.
                        Ok(EffectResult::Yield) => continue,
                        Ok(EffectResult::Pending) => {
                            self.button_refresh.as_mut().unwrap().pending_effect = Some(effect);
                            return Ok(false);
                        }
                        Err(error) => {
                            self.button_refresh.as_mut().unwrap().pending_effect = Some(effect);
                            return Err(error);
                        }
                    }
                    continue;
                }
                if !self.button_refresh.as_ref().unwrap().motion_advanced {
                    if !super::fes_motion::advance_owner(
                        host, &self.view(host, now), &mut runtime, &mut driver, remaining,
                    ).map_err(|error| host.fes_task_error(self.id, error))? {
                        return Ok(false);
                    }
                    self.button_refresh.as_mut().unwrap().motion_advanced = true;
                }
                if !self.button_refresh.as_ref().unwrap().media_advanced {
                    if self.media_frame != Some(now) {
                        runtime.update_driver_media(now as u64, remaining, host.resources, host.events)
                            .map_err(|error| host.fes_task_error(self.id, error))?;
                        self.media_frame = Some(now);
                    }
                    self.button_refresh.as_mut().unwrap().media_advanced = true;
                }
                if !super::fes_task_platform::finish_task_media(
                    host, &self.view(host, now), &mut runtime, &mut driver,
                ).map_err(|error| host.fes_task_error(self.id, error))? {
                    if driver.has_media_notification_work() { continue; }
                    return Ok(false);
                }
                let scope = self.button_refresh.as_mut().unwrap().notifications.take()
                    .expect("media refresh owns the parked notification queues");
                driver.end_media_notification_scope(scope);
                self.button_refresh = None;
                return Ok(true);
            }
            Ok(false)
        })();
        sync_locals(host, self.id, &driver);
        runtime.driver = Some(driver);
        self.runtime = Some(runtime);
        result
    }

    fn set_paused(&mut self, host: &mut LoggingHost<'_>, value: u32, budget: usize) -> Result<bool> {
        if self.invocation_now.is_some() && self.system.is_none() {
            return Ok(false);
        }
        if self.system.is_none() && self.paused == value {
            return Ok(true);
        }
        self.paused = value;
        self.system_command(
            host, 348, value,
            if value == 0 { b"_FES_WAKE_" } else { b"_FES_SLEEP_" },
            budget,
        )
    }

    fn system_command(
        &mut self,
        host: &mut LoggingHost<'_>,
        key: u32,
        value: u32,
        block: &[u8],
        budget: usize,
    ) -> Result<bool> {
        self.system_command_with_context(host, key, value, block, None, budget)
    }

    fn system_command_with_context(
        &mut self,
        host: &mut LoggingHost<'_>,
        key: u32,
        value: u32,
        block: &[u8],
        object: Option<FesDriverTarget>,
        budget: usize,
    ) -> Result<bool> {
        self.system_command_with_input(host, key, value, block, object, None, true, budget)
    }

    fn system_command_with_input(
        &mut self,
        host: &mut LoggingHost<'_>,
        key: u32,
        value: u32,
        block: &[u8],
        object: Option<FesDriverTarget>,
        input: Option<FesDriverInput>,
        refresh_media: bool,
        budget: usize,
    ) -> Result<bool> {
        if self.button_refresh.is_some() || self.lower_notification.is_some() { return Ok(false); }
        if !self.initialized {
            return Ok(true);
        }
        if self.invocation_now.is_some() && self.system.is_none() {
            return Ok(false);
        }
        if let Some(system) = &self.system {
            if system.key != key || system.value != value || system.block != block
                || system.object != object
                || system.refresh_media != refresh_media
                || system.input.as_ref().map(|input| (&input.target, input.code, &input.event))
                    != input.as_ref().map(|input| (&input.target, input.code, &input.event))
            {
                return Err(native_state_error(
                    "FES system continuation changed its command",
                ));
            }
        } else {
            self.system = Some(SystemCommand {
                key,
                value,
                entered: false,
                block_done: false,
                media_advanced: false,
                media_done: false,
                configuration_phase: 0,
                handler_notice_pending: false,
                now: host
                    .data_services
                    .tasks
                    .task_clock(self.id)
                    .map(|clock| clock.elapsed())
                    .unwrap_or(0),
                block: block.to_vec(),
                object,
                input,
                refresh_media,
            });
        }
        let mut runtime = self
            .runtime
            .take()
            .ok_or_else(|| native_state_error("FES system context is missing"))?;
        let mut driver = runtime
            .driver
            .take()
            .ok_or_else(|| native_state_error("FES system driver is missing"))?;
        let now = self.system.as_ref().unwrap().now;
        let result = (|| {
            self.apply_data_commands(host, &mut driver);
            if !self.system.as_ref().unwrap().entered {
                let exists = if let Some(input) = self.system.as_ref().unwrap().input.clone() {
                    if driver.has_pending_handler() {
                        return Err(native_state_error(
                            "finish the pending FES handler before a lower control notification",
                        ));
                    }
                    driver.on_input(input)
                        .map_err(|error| host.fes_task_error(self.id, error))?;
                    true
                } else {
                    driver.dispatch_system_block_with_context(
                        block, self.system.as_ref().unwrap().object.clone(),
                    ).map_err(|error| host.fes_task_error(self.id, error))?
                };
                let system = self.system.as_mut().unwrap();
                system.entered = true;
                system.block_done = !exists;
            }
            for iteration in 0..budget {
                if self.pending_effect.is_none()
                    && !driver.media_statement_pending()
                    && self.system.as_ref().unwrap().block_done
                    && (driver.media_statement_result_ready()
                        || (!driver.has_pending_handler()
                            && !self.system.as_ref().unwrap().handler_notice_pending))
                {
                    if !self.system.as_ref().unwrap().refresh_media {
                        self.system = None;
                        return Ok(true);
                    }
                    if !self.system.as_ref().unwrap().media_advanced {
                        // A synchronous BUTTON refresh may already have
                        // advanced this same scene time inside the handler.
                        if self.media_frame != Some(now) {
                            runtime.update_driver_media(
                                now as u64, budget - iteration, host.resources, host.events,
                            ).map_err(|error| host.fes_task_error(self.id, error))?;
                            self.media_frame = Some(now);
                        }
                        self.system.as_mut().unwrap().media_advanced = true;
                    }
                    if !self.system.as_ref().unwrap().media_done {
                        if !super::fes_task_platform::finish_task_media(
                            host,
                            &self.view(host, now),
                            &mut runtime,
                            &mut driver,
                        )
                        .map_err(|error| host.fes_task_error(self.id, error))?
                        {
                            return Ok(false);
                        }
                        self.system.as_mut().unwrap().media_done = true;
                    }
                    if key == 19 {
                        self.finish_configuration(host)?;
                    }
                    self.system = None;
                    return Ok(true);
                }
                let effect = match self.pending_effect.take() {
                    Some(effect) => effect,
                    None => with_expression(host, |_, shared| {
                        driver.advance_notification(now as u64, shared, budget - iteration)
                    })
                    .map_err(|error| host.fes_task_error(self.id, error))?,
                };
                sync_locals(host, self.id, &driver);
                if matches!(&effect, FesDriverEffect::SystemBlock { name, complete: true } if name == block)
                {
                    let system = self.system.as_mut().unwrap();
                    system.block_done = true;
                    system.handler_notice_pending = false;
                    continue;
                }
                let effect_result =
                    self.effect(host, &mut runtime, &mut driver, &effect, now, budget - iteration);
                self.apply_data_commands(host, &mut driver);
                if self.system.as_ref().unwrap().input.is_some()
                    && matches!(&effect, FesDriverEffect::ObjectEvent {
                        event, phase: super::fes_driver::FesObjectEventPhase::Complete { .. }, ..
                    } if event.as_slice() == block)
                    && matches!(&effect_result, Ok(EffectResult::Continue))
                {
                    self.system.as_mut().unwrap().block_done = true;
                }
                match effect_result {
                    Ok(EffectResult::Continue | EffectResult::Boundary) => {
                        match &effect {
                            FesDriverEffect::Waiting { handler_local: true, .. } => {
                                self.system.as_mut().unwrap().handler_notice_pending = true;
                            }
                            FesDriverEffect::ObjectEvent {
                                phase: super::fes_driver::FesObjectEventPhase::Complete { .. }, ..
                            }
                            | FesDriverEffect::SystemBlock { complete: true, .. } => {
                                self.system.as_mut().unwrap().handler_notice_pending = false;
                            }
                            _ => {}
                        }
                    }
                    Ok(EffectResult::Yield) => return Ok(false),
                    Ok(EffectResult::Pending) => {
                        self.pending_effect = Some(effect);
                        return Ok(false);
                    }
                    Err(error) => {
                        self.pending_effect = Some(effect);
                        return Err(error);
                    }
                }
            }
            Ok(false)
        })();
        sync_locals(host, self.id, &driver);
        runtime.driver = Some(driver);
        self.runtime = Some(runtime);
        result
    }

    /// Command 19 first updates the scene, including synchronous PLAY blocks.
    /// Its font and sound refreshes then schedule work for the next update.
    /// Keeping separate completion stages also avoids replaying the scene or
    /// the font refresh if a later service fails and the command is retried.
    fn finish_configuration(&mut self, host: &mut LoggingHost<'_>) -> Result<()> {
        if self.system.as_ref().unwrap().configuration_phase == 0 {
            let defaults = host.data_services.messages.defaults.clone().unwrap_or(
                cs2_runtime::message_object::MessageDefaults {
                    font_name: Vec::new(),
                    bold: false,
                    shadow: 1,
                    outline: 1,
                    glyph_parameter: Some(0),
                    gradual: false,
                },
            );
            super::fes_text::refresh_defaults(host.data_services, self.id, &defaults)
                .map_err(|error| host.fes_task_error(self.id, error))?;
            self.system.as_mut().unwrap().configuration_phase = 1;
        }
        if self.system.as_ref().unwrap().configuration_phase == 1 {
            super::fes_sound::refresh_configuration(host, self.id)
                .map_err(|error| host.fes_task_error(self.id, error))?;
            self.system.as_mut().unwrap().configuration_phase = 2;
        }
        Ok(())
    }

    fn initialize(&mut self, host: &mut LoggingHost<'_>) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        let parent = host.task_parents.get(&self.id).copied().unwrap_or(0);
        self.context = host
            .data_services
            .tasks
            .task_context(parent)
            .unwrap_or(&self.context)
            .to_vec();
        host.data_services
            .tasks
            .write_fes_context(self.id, &self.context)?;
        let document = FesDocument {
            sections: Vec::new(),
        };
        if self.runtime.is_none() {
            self.runtime = Some(self.new_runtime(document)?);
        }
        self.initialized = true;
        host.data_services.tasks.fes.slots.get_mut(&self.id).unwrap()
            .context_initialized = true;
        if let Some(node) = host.scene_nodes.get_mut(&self.id) {
            node.initialized = true;
        }
        Ok(())
    }

    fn new_runtime(&self, document: FesDocument) -> Result<FesRuntime> {
        let mut driver = FesScriptDriver::new(&document)
            .map_err(|error| native_state_error_owned(format!("FES program initialization failed: {error:#}")))?;
        driver.set_task_id(self.id);
        driver.bind_external(self.external.clone());
        let mut runtime = FesRuntime::new(document);
        // Objects are initialized only by the driver's ordered declaration
        // events, never by the old whitespace-based document shortcut.
        runtime.media.clear();
        runtime.planes.clear();
        runtime.driver = Some(driver);
        Ok(runtime)
    }

    fn load(
        &mut self,
        host: &mut LoggingHost<'_>,
        requested: &[u8],
        resolved: &[String],
        budget: usize,
    ) -> Result<bool> {
        if self.button_refresh.is_some() || self.lower_notification.is_some() { return Ok(false); }
        if self.loaded && self.prepared {
            if self.invocation_now.is_some()
                || self.system.is_some()
                || self.pending_effect.is_some()
                || self
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.driver.as_ref())
                    .is_some_and(FesScriptDriver::has_pending_handler)
            {
                return Ok(false);
            }
            // Parse/allocate before touching the existing object. A missing
            // or unreadable new resource preserves its current controller.
            let Some(candidate) = resolved.iter().find_map(|name| host.resources.find(name)) else {
                host.data_services
                    .tasks
                    .write_task_property(self.id, (-2i32) as u32)?;
                return Ok(true);
            };
            if candidate.size > MAX_FES_BYTES as u64 {
                return Err(native_state_error("FES resource exceeds 64 MiB"));
            }
            let bytes = host
                .resources
                .read_candidate(candidate)
                .map_err(|error| host.fes_task_error(self.id, error))?;
            let document = parse_document(&bytes)?;
            let mut replacement = self.new_runtime(document)?;
            let mut previous = self
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.driver.as_ref())
                .ok_or_else(|| native_state_error("FES reload has no previous driver"))?
                .clone();
            replacement.driver.as_mut().unwrap().interpreter.named =
                previous.interpreter.named.clone();
            replacement
                .driver
                .as_mut()
                .unwrap()
                .interpreter
                .local_values_mut()
                .copy_from_slice(previous.interpreter.local_values());
            super::fes_task_platform::release_reload_objects(host, self.id)
                .map_err(|error| host.fes_task_error(self.id, error))?;
            if let Some(runtime) = self.runtime.as_mut() {
                super::fes_media::release_image_sources(host, runtime)
                    .map_err(|error| host.fes_task_error(self.id, error))?;
            }
            previous.bind_external(self.external.clone());
            self.previous_driver = Some(previous);
            self.runtime = Some(replacement);
            let slot = host.data_services.tasks.fes.slots.get_mut(&self.id).unwrap();
            slot.headers_dirty = true;
            slot.pending_entry = None;
            self.prepared = false;
            self.requested_name = Some(requested.to_vec());
            self.name = Some(requested.to_vec());
            self.load_now = Some(
                host.data_services
                    .tasks
                    .task_clock(self.id)
                    .map(|clock| clock.last_update)
                    .unwrap_or(0),
            );
            self.media_frame = None;
        }
        if let Some(previous) = &self.requested_name {
            if self.loaded && previous != requested {
                return Err(native_state_error(
                    "finish pending FES load before changing its name",
                ));
            }
        }
        if !self.loaded {
            let candidate = resolved.iter().find_map(|name| host.resources.find(name));
            let Some(candidate) = candidate else {
                host.data_services
                    .tasks
                    .write_task_property(self.id, (-2i32) as u32)?;
                self.requested_name = None;
                return Ok(true);
            };
            if candidate.size > MAX_FES_BYTES as u64 {
                return Err(native_state_error("FES resource exceeds 64 MiB"));
            }
            let bytes = host
                .resources
                .read_candidate(candidate)
                .map_err(|error| host.fes_task_error(self.id, error))?;
            let document = parse_document(&bytes)?;
            let mut replacement = self.new_runtime(document)?;
            if let Some(previous) = self
                .runtime
                .as_mut()
                .and_then(|runtime| runtime.driver.as_mut())
            {
                replacement.driver.as_mut().unwrap().interpreter.named =
                    previous.interpreter.named.clone();
                replacement
                    .driver
                    .as_mut()
                    .unwrap()
                    .interpreter
                    .local_values_mut()
                    .copy_from_slice(previous.interpreter.local_values());
                self.previous_driver = Some(previous.clone());
            }
            self.runtime = Some(replacement);
            let slot = host.data_services.tasks.fes.slots.get_mut(&self.id).unwrap();
            slot.headers_dirty = true;
            slot.pending_entry = None;
            self.load_now = Some(
                host.data_services
                    .tasks
                    .task_clock(self.id)
                    .map(|clock| clock.last_update)
                    .unwrap_or(0),
            );
            self.requested_name = Some(requested.to_vec());
            self.name = Some(requested.to_vec());
            self.loaded = true;
        }
        let done = self.prepare_script(host, budget)?;
        if done {
            self.prepared = true;
            self.load_now = None;
            if let Some(mut previous) = self.previous_driver.take() {
                let runtime = self.runtime.as_mut().unwrap();
                let staged = runtime.driver.take().unwrap();
                previous.interpreter = staged.interpreter;
                previous.adapter = staged.adapter;
                previous.set_task_id(self.id);
                previous.bind_external(self.external.clone());
                runtime.driver = Some(previous);
            }
        }
        Ok(done)
    }

    fn bind_list(&mut self, host: &mut LoggingHost<'_>, list: u32) -> Result<()> {
        // Original receiver 57f857 stores the list and queries its actual
        // image/offset before a Scene exists. Keep that binding through 337
        // and the later load; neither initialize a runtime nor start it here.
        let hook = host
            .data_services
            .tasks
            .fes
            .hooks
            .bind_list
            .ok_or_else(|| native_state_error("FES shared image-list query platform is absent"))?;
        self.list = Some(hook(host, list).map_err(|error| host.fes_task_error(self.id, error))?);
        Ok(())
    }

    fn start(&mut self, host: &mut LoggingHost<'_>, budget: usize) -> Result<bool> {
        if self.button_refresh.is_some() || self.lower_notification.is_some() { return Ok(false); }
        if !self.initialized {
            return Err(native_state_error("FES start requires command 337"));
        }
        let now = match self.start_now {
            Some(now) => now,
            None => {
                let parent = host.task_parents.get(&self.id).copied().unwrap_or(0);
                let now = host
                    .data_services
                    .tasks
                    .task_clock(parent)
                    .map(|clock| clock.elapsed())
                    .unwrap_or_else(|| host.task_time.wrapping_sub(host.task_origin));
                host.data_services.tasks.set_task_clock(self.id, now)?;
                self.start_now = Some(now);
                self.start_complete = false;
                self.started = true;
                now
            }
        };
        if !self.start_complete {
            if !host.update_detached_task_children(
                self.id,
                now,
                0,
                &mut self.start_children,
                budget,
            )? {
                return Ok(false);
            }
            self.start_complete = true;
        }
        let done = self.update(host, now, budget)?;
        if done {
            self.start_now = None;
        }
        Ok(done)
    }

    fn update(&mut self, host: &mut LoggingHost<'_>, now: u32, budget: usize) -> Result<bool> {
        if self.button_refresh.is_some() || self.lower_notification.is_some() { return Ok(false); }
        if self.system.is_some() {
            return Err(native_state_error(
                "finish FES system notification before ordinary body update",
            ));
        }
        if self.paused != 0 {
            return Ok(true);
        }
        if budget == 0 {
            return Ok(false);
        }
        let now = *self.invocation_now.get_or_insert(now);
        let mut runtime = match self.runtime.take() {
            Some(runtime) => runtime,
            None => {
                // Uninitialized tasks still need a real first-body input
                // preparation; initialization itself is command 337's job.
                self.new_runtime(FesDocument {
                    sections: Vec::new(),
                })?
            }
        };
        let mut driver = runtime
            .driver
            .take()
            .ok_or_else(|| native_state_error("FES runtime has no complete script driver"))?;
        let result = (|| {
            if !self.auxiliary_prepared {
                let hook = host.data_services.tasks.fes.hooks.prepare.ok_or_else(|| {
                    native_state_error("FES auxiliary input preparation platform is absent")
                })?;
                let view = self.view(host, now);
                if !hook(host, &view, &mut runtime, &mut driver)
                    .map_err(|error| host.fes_task_error(self.id, error))?
                {
                    return Ok(false);
                }
                self.auxiliary_prepared = true;
            }
            if !self.started {
                return Ok(true);
            }
            self.drive(host, &mut runtime, &mut driver, now, budget, false)
        })();
        sync_locals(host, self.id, &driver);
        runtime.driver = Some(driver);
        self.runtime = Some(runtime);
        if matches!(result, Ok(true)) {
            self.invocation_now = None;
        }
        result
    }

    fn prepare_script(&mut self, host: &mut LoggingHost<'_>, budget: usize) -> Result<bool> {
        let mut runtime = self
            .runtime
            .take()
            .ok_or_else(|| native_state_error("FES load requires an execution context"))?;
        let mut driver = runtime
            .driver
            .take()
            .ok_or_else(|| native_state_error("FES load has no complete script driver"))?;
        let now = self.load_now.unwrap_or_else(|| {
            host.data_services
                .tasks
                .task_clock(self.id)
                .map(|clock| clock.last_update)
                .unwrap_or(0)
        });
        let result = self.drive(host, &mut runtime, &mut driver, now, budget, true);
        sync_locals(host, self.id, &driver);
        runtime.driver = Some(driver);
        self.runtime = Some(runtime);
        result
    }

    fn drive(
        &mut self,
        host: &mut LoggingHost<'_>,
        runtime: &mut FesRuntime,
        driver: &mut FesScriptDriver,
        now: u32,
        budget: usize,
        preparing: bool,
    ) -> Result<bool> {
        self.apply_data_commands(host, driver);
        if !preparing && self.boundary.is_none() {
            // A retained boundary still owns the unfinished media update.
            // Leave newly arrived input queued until it completes; otherwise
            // a later host call could enter a new handler before that update.
            while let Some(input) = host
                .data_services
                .tasks
                .fes
                .slots
                .get(&self.id)
                .and_then(|slot| slot.inputs.front().cloned())
            {
                driver
                    .on_input(input)
                    .map_err(|error| host.fes_task_error(self.id, error))?;
                host.data_services
                    .tasks
                    .fes
                    .slots
                    .get_mut(&self.id)
                    .unwrap()
                    .inputs
                    .pop_front();
            }
            driver.set_named_value("_MOUSE_X_", 0, host.pointer_position[0]);
            driver.set_named_value("_MOUSE_Y_", 0, host.pointer_position[1]);
        }
        for iteration in 0..budget {
            if !preparing
                && host.data_services.tasks.message_task_state(self.id)
                    .is_some_and(|(_, deletion, _)| deletion != 0)
            {
                // A synchronously initialized child can mark its owning FES
                // for deletion. The tree owns destruction; do not execute the
                // rest of this owner's script after that child returns.
                return Ok(true);
            }
            if preparing
                && self.pending_effect.is_none()
                && !driver.initialization_pending()
            {
                return Ok(true);
            }
            let resumed_boundary = if self.pending_effect.is_none()
                && !driver.media_statement_pending()
                && (driver.media_statement_result_ready()
                    || (!driver.has_pending_handler() && !self.boundary_notice))
            {
                self.boundary.take()
            } else {
                None
            };
            let finishing_boundary = resumed_boundary.is_some();
            let effect = match resumed_boundary {
                Some(effect) => effect,
                None => match self.pending_effect.take() {
                    Some(effect) => effect,
                    None => with_expression(host, |_, shared| {
                        if preparing {
                            driver.advance_initialization(shared)?
                                .ok_or_else(|| anyhow::anyhow!("FES preparation lost its declaration"))
                        } else {
                            driver.advance(now as u64, shared, budget - iteration)
                        }
                    })
                    .map_err(|error| host.fes_task_error(self.id, error))?,
                },
            };
            sync_locals(host, self.id, driver);
            // The boundary effect was already applied/acknowledged before
            // media yielded. Resume only its finish stage, not the dispatcher.
            let result = if finishing_boundary {
                Ok(EffectResult::Boundary)
            } else {
                self.effect(host, runtime, driver, &effect, now, budget - iteration)
            };
            self.apply_data_commands(host, driver);
            match result {
                Ok(EffectResult::Continue) => {
                    if self.boundary.is_some() {
                        match &effect {
                            FesDriverEffect::Waiting {
                                handler_local: true,
                                ..
                            } => self.boundary_notice = true,
                            FesDriverEffect::ObjectEvent {
                                phase: super::fes_driver::FesObjectEventPhase::Complete { .. },
                                ..
                            }
                            | FesDriverEffect::SystemBlock { complete: true, .. } => {
                                self.boundary_notice = false
                            }
                            _ => {}
                        }
                    }
                }
                Ok(EffectResult::Boundary) => {
                    if !finishing_boundary && matches!(effect, FesDriverEffect::Waiting { .. }) {
                        driver.finish_left_input_frame();
                    }
                    self.refresh_child_status(host, driver);
                    let finish = (|| {
                        if !super::fes_motion::advance_owner(
                            host, &self.view(host, now), runtime, driver, budget - iteration,
                        ).map_err(|error| host.fes_task_error(self.id, error))? {
                            return Ok(false);
                        }
                        if self.media_frame != Some(now) {
                            runtime
                                .update_driver_media(
                                    now as u64,
                                    budget - iteration,
                                    host.resources,
                                    host.events,
                                )
                                .map_err(|error| host.fes_task_error(self.id, error))?;
                            self.media_frame = Some(now);
                        }
                        super::fes_task_platform::finish_task_media(
                            host,
                            &self.view(host, now),
                            runtime,
                            driver,
                        )
                        .map_err(|error| host.fes_task_error(self.id, error))
                    })();
                    sync_locals(host, self.id, driver);
                    match finish {
                        Err(error) => {
                            // The script effect succeeded; only its media
                            // stage failed. Keep the same ownership as yield.
                            self.boundary = Some(effect);
                            return Err(error);
                        }
                        Ok(false) => {
                            self.boundary = Some(effect);
                            return Ok(false);
                        }
                        Ok(true) => {}
                    }
                    return Ok(true);
                }
                Ok(EffectResult::Yield) => return Ok(false),
                Ok(EffectResult::Pending) => {
                    self.pending_effect = Some(effect);
                    return Ok(false);
                }
                Err(error) => {
                    self.pending_effect = Some(effect);
                    return Err(error);
                }
            }
            if let Some(node) = host.scene_nodes.get(&self.id) {
                driver
                    .interpreter
                    .local_values_mut()
                    .copy_from_slice(&node.fes_parameters);
            }
        }
        Ok(false)
    }

    fn apply_data_commands(&mut self, host: &mut LoggingHost<'_>, driver: &mut FesScriptDriver) {
        if let Some(context) = host.data_services.tasks.task_context(self.id) {
            self.context = context.to_vec();
        }
        if let Some(node) = host.scene_nodes.get(&self.id) {
            driver.interpreter.local_values_mut().copy_from_slice(&node.fes_parameters);
        }
        let entry_ready = self.button_refresh.is_none() && self.lower_notification.is_none()
            && !driver.media_statement_pending();
        let slot = host.data_services.tasks.fes.slots.get_mut(&self.id)
            .expect("FES receiver state is retained during execution");
        if entry_ready && let Some(name) = slot.pending_entry.take() {
            if !driver.select_main_block(&name) {
                // A successful lookup was already returned to the sender.
                // Preserve it if a staged program replacement is not ready.
                slot.pending_entry = Some(name);
            }
        }
        for (name, value) in std::mem::take(&mut slot.pending_input_named) {
            driver.set_named_value(&name, 0, value);
        }
        if let Some(value) = slot.save_object.take() {
            driver.set_named_value("_SAVEOBJ_", 0, value as i32);
        }
        if entry_ready && std::mem::take(&mut slot.restore_entry) {
            // Keep the live handler and its parked main wait/call context.
            // A media statement owns another parked scope until its receipt
            // is consumed, so it uses the same safe boundary as entry341.
            driver.select_main_block(b"_LOAD_");
        }
    }

    fn effect(
        &mut self,
        host: &mut LoggingHost<'_>,
        runtime: &mut FesRuntime,
        driver: &mut FesScriptDriver,
        effect: &FesDriverEffect,
        now: u32,
        budget: usize,
    ) -> Result<EffectResult> {
        let acknowledge_statement = driver.media_statement_effect_pending();
        let result = self.effect_inner(host, runtime, driver, effect, now, budget);
        if acknowledge_statement
            && matches!(&result, Ok(EffectResult::Continue | EffectResult::Boundary))
        {
            driver.complete_media_statement_effect()
                .map_err(|error| host.fes_task_error(self.id, error))?;
        }
        result
    }

    fn effect_inner(
        &mut self,
        host: &mut LoggingHost<'_>,
        runtime: &mut FesRuntime,
        driver: &mut FesScriptDriver,
        effect: &FesDriverEffect,
        now: u32,
        budget: usize,
    ) -> Result<EffectResult> {
        match effect {
            FesDriverEffect::MediaStatementReturned => Ok(EffectResult::Continue),
            FesDriverEffect::ExecKcs { name, external } => {
                self.begin_child(FesChildKind::Kcs, name, [0; 16], external)?;
                if self.advance_child(host, budget)? {
                    Ok(EffectResult::Continue)
                } else {
                    Ok(EffectResult::Pending)
                }
            }
            FesDriverEffect::ExecFes {
                name,
                parameters,
                external,
            } => {
                self.begin_child(FesChildKind::Fes, name, *parameters, external)?;
                if self.advance_child(host, budget)? {
                    Ok(EffectResult::Continue)
                } else {
                    Ok(EffectResult::Pending)
                }
            }
            FesDriverEffect::EndChild(kind) => {
                let child = if *kind == FesChildKind::Kcs {
                    &mut self.child_kcs
                } else {
                    &mut self.child_fes
                };
                if let Some(id) = *child {
                    let result = host.receive_task_command(
                        cs2_runtime::TaskCommand {
                            target: id as i32, sender: self.id as i32, code: 3, data: [0; 3],
                        },
                        &mut cs2_runtime::TaskCommandBuffers::default(), &mut 0, budget,
                    )?;
                    if result.is_some_and(|outcome| !outcome.done) {
                        return Ok(EffectResult::Pending);
                    }
                    *child = None;
                }
                driver.set_child_exit_code(-1);
                Ok(EffectResult::Continue)
            }
            FesDriverEffect::Send { message, parent } => {
                let target = if *parent {
                    host.task_parents.get(&self.id).copied()
                } else {
                    self.child_kcs
                };
                let Some(target) = target else {
                    return Ok(EffectResult::Continue);
                };
                let done = if let Some(send) = host.data_services.tasks.fes.hooks.send {
                    send(host, target, *message)
                        .map_err(|error| host.fes_task_error(self.id, error))?
                } else {
                    host.send_fes_task_message(target, *message)?
                };
                Ok(if done {
                    EffectResult::Continue
                } else {
                    EffectResult::Pending
                })
            }
            FesDriverEffect::Exit(value) => {
                host.data_services
                    .tasks
                    .write_task_property(self.id, *value as u32)?;
                runtime.exit_code = Some(*value);
                Ok(EffectResult::Continue)
            }
            FesDriverEffect::Frame => Ok(EffectResult::Boundary),
            FesDriverEffect::Yield => Ok(EffectResult::Yield),
            FesDriverEffect::Waiting {
                handler_local: true,
                ..
            } => Ok(EffectResult::Continue),
            FesDriverEffect::Waiting {
                wait: FesDriverWait::Frames { .. } | FesDriverWait::Frame { .. },
                ..
            } => Ok(EffectResult::Boundary),
            FesDriverEffect::Waiting {
                wait: FesDriverWait::Control { kind, .. },
                ..
            } if kind == b"frame" || kind == b"privateflag" => Ok(EffectResult::Boundary),
            FesDriverEffect::Waiting { .. } => {
                if !self.platform_effect(host, runtime, driver, effect, now)? {
                    Ok(EffectResult::Pending)
                } else if driver.waiting().is_some() {
                    Ok(EffectResult::Boundary)
                } else {
                    Ok(EffectResult::Continue)
                }
            }
            _ => {
                if self.platform_effect(host, runtime, driver, effect, now)? {
                    Ok(EffectResult::Continue)
                } else {
                    Ok(EffectResult::Pending)
                }
            }
        }
    }

    fn platform_effect(
        &self,
        host: &mut LoggingHost<'_>,
        runtime: &mut FesRuntime,
        driver: &mut FesScriptDriver,
        effect: &FesDriverEffect,
        now: u32,
    ) -> Result<bool> {
        let handler = host.data_services.tasks.fes.hooks.effect.ok_or_else(|| {
            native_state_error("FES parsed object/media effect platform is absent")
        })?;
        let view = self.view(host, now);
        let result = with_expression(host, |host, shared| {
            handler(host, &view, runtime, driver, effect, shared)
        });
        sync_locals(host, self.id, driver);
        result.map_err(|error| host.fes_task_error(self.id, error))
    }

    fn begin_child(
        &mut self,
        kind: FesChildKind,
        name: &[u8],
        parameters: [i32; 16],
        external: &SharedExternalStorage,
    ) -> Result<()> {
        if let Some(pending) = &self.pending_child {
            if pending.kind != kind || pending.name != name {
                return Err(native_state_error(
                    "FES child creation continuation changed",
                ));
            }
            return Ok(());
        }
        if self.child_fes.is_some() || self.child_kcs.is_some() {
            return Ok(());
        }
        self.pending_child = Some(ChildCreation {
            kind,
            name: name.to_vec(),
            parameters,
            external: external.clone(),
            id: None,
            phase: ChildPhase::Create,
        });
        Ok(())
    }

    fn advance_child(&mut self, host: &mut LoggingHost<'_>, budget: usize) -> Result<bool> {
        let Some(pending) = self.pending_child.as_mut() else {
            return Ok(true);
        };
        for _ in 0..budget {
            let id = pending.id.unwrap_or(0);
            match pending.phase {
                ChildPhase::Create => {
                    let id = if pending.kind == FesChildKind::Kcs {
                        host.create_unloaded_ordinary_task_shared(
                            self.id,
                            &self.context,
                            pending.external.clone(),
                        )?
                    } else {
                        host.create_task_object(34, self.id, b"", &self.context)?
                    };
                    pending.id = Some(id);
                    pending.phase = if pending.kind == FesChildKind::Kcs {
                        ChildPhase::Load
                    } else {
                        ChildPhase::Initialize
                    };
                }
                ChildPhase::Initialize => {
                    host.with_fes_task(id, |host, object| {
                        object.external = pending.external.clone();
                        host.data_services.tasks.bind_fes_task_external(id, pending.external.clone())?;
                        object.initialize(host)
                    })?;
                    pending.phase = ChildPhase::Bind;
                }
                ChildPhase::Bind => {
                    if let Some(list) = self.list.as_ref() {
                        host.with_fes_task(id, |host, object| object.bind_list(host, list.task))?;
                    }
                    pending.phase = ChildPhase::Parameters;
                }
                ChildPhase::Parameters => {
                    let node = host.scene_nodes.get_mut(&id).ok_or_else(|| {
                        native_state_error("FES child disappeared before parameters")
                    })?;
                    node.fes_parameters[..16].copy_from_slice(&pending.parameters);
                    pending.phase = ChildPhase::Load;
                }
                ChildPhase::Load => {
                    if pending.kind == FesChildKind::Kcs {
                        if !host.load_ordinary_task_shared_step(
                            id,
                            &pending.name,
                            1,
                            pending.external.clone(),
                            budget,
                        )? {
                            return Ok(false);
                        }
                        pending.phase = ChildPhase::Complete;
                    } else {
                        let names = host.fes_resource_names(&pending.name)?;
                        if !host.with_fes_task(id, |host, object| {
                            object.load(host, &pending.name, &names, budget)
                        })? {
                            return Ok(false);
                        }
                        pending.phase = ChildPhase::Start;
                    }
                }
                ChildPhase::Start => {
                    if !host.with_fes_task(id, |host, object| object.start(host, budget))? {
                        return Ok(false);
                    }
                    pending.phase = ChildPhase::Complete;
                }
                ChildPhase::Complete => {
                    if pending.kind == FesChildKind::Kcs {
                        self.child_kcs = pending.id;
                    } else {
                        self.child_fes = pending.id;
                    }
                    self.pending_child = None;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn refresh_child_status(&self, host: &LoggingHost<'_>, driver: &mut FesScriptDriver) {
        for id in [self.child_fes, self.child_kcs].into_iter().flatten() {
            if let Some(value) = host
                .data_services
                .tasks
                .task_property(id)
                .filter(|value| *value != u32::MAX)
            {
                driver.set_child_exit_code(value as i32);
            }
        }
    }

    fn view(&self, host: &LoggingHost<'_>, now: u32) -> FesTaskView {
        object_view(host, self.id, &self.context, &self.list, now, self.paused)
    }
}

enum EffectResult {
    Continue,
    Boundary,
    Yield,
    Pending,
}

fn object_view(
    host: &LoggingHost<'_>,
    id: u32,
    context: &[u8],
    list: &Option<FesListBinding>,
    now: u32,
    paused: u32,
) -> FesTaskView {
    FesTaskView {
        id,
        parent: host.task_parents.get(&id).copied().unwrap_or(0),
        context: context.to_vec(),
        list: list.clone(),
        now,
        paused,
    }
}

fn sync_locals(host: &mut LoggingHost<'_>, id: u32, driver: &FesScriptDriver) {
    if let Some(slot) = host.data_services.tasks.fes.slots.get_mut(&id)
        .filter(|slot| slot.headers_dirty)
    {
        slot.program_headers = driver.interpreter.program.blocks.iter()
            .map(|block| block.name.clone()).collect();
        slot.headers_dirty = false;
    }
    if let Some(node) = host.scene_nodes.get_mut(&id) {
        node.fes_parameters
            .copy_from_slice(driver.interpreter.local_values());
    }
    host.sync_fes_local_region(id, driver.interpreter.local_values());
}

fn with_expression<T>(
    host: &mut LoggingHost<'_>,
    call: impl FnOnce(&mut LoggingHost<'_>, &mut ExpressionState) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let mut shared = ExpressionState {
        shared: std::mem::take(host.shared_values)
            .into_iter()
            .map(|word| word as i32)
            .collect(),
        strings: std::mem::take(host.string_values),
        current: *host.current_parameter as i32,
        ..ExpressionState::default()
    };
    let result = call(host, &mut shared);
    *host.shared_values = shared.shared.into_iter().map(|word| word as u32).collect();
    *host.string_values = shared.strings;
    result
}

fn nul(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())]
}

fn parse_document(bytes: &[u8]) -> Result<FesDocument> {
    if bytes.len() > MAX_FES_BYTES {
        return Err(native_state_error("FES data exceeds 64 MiB"));
    }
    if bytes.starts_with(b"FES\0") {
        return FesDocument::parse(bytes);
    }
    let bytes = nul(bytes);
    let mut sections: Vec<FesSection> = Vec::new();
    let mut offset = 0usize;
    for raw in bytes.split_inclusive(|&byte| byte == b'\n') {
        let line = raw.strip_suffix(b"\n").unwrap_or(raw);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let trimmed = line.trim_ascii();
        if let Some(header) = trimmed.strip_prefix(b"#") {
            sections.push(FesSection {
                header: header.to_vec(),
                lines: Vec::new(),
            });
        } else if let Some(section) = sections.last_mut() {
            if !trimmed.is_empty() {
                section.lines.push(FesLine {
                    offset,
                    bytes: line.to_vec(),
                });
            }
        }
        offset += raw.len();
    }
    Ok(FesDocument { sections })
}
