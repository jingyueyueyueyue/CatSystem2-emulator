//! Type-55 JavaScript tasks and typed Cs* host objects. The ordinary-table
//! 975 wrapper is verified at v4.01 VA 0051a060 (command 158, two DWORDs).
//! No temporary KCS VM, native pointer or replacement image is manufactured.
use std::collections::{BTreeMap, VecDeque};

use cs2_formats::HgFile;
use cs2_renderer::{Canvas, Color, FrameMetadata, ImageDrawOptions, Point, RgbaFrame};
use cs2_runtime::javascript::{
    JavaScriptCall, JavaScriptHost, JavaScriptInput, JavaScriptStep, JavaScriptTask,
    JavaScriptValue as Value,
};
use cs2_runtime::{
    RuntimeError, RuntimeResult, TaskCommand, TaskCommandBuffers, TaskCommandOutcome, TaskMessage,
};
use cs2_script::{NativeReturn, Vm};

use super::{
    DesktopMotionServices, LoggingHost, LogicEvent, MotionControllerState, MotionTaskState,
    ScriptTextEncoding, advance_motion_controller, create_motion_runner, expand_kcs_format,
    read_u32, trim_nul,
};

#[path = "javascript_save.rs"]
mod save;

type Result<T> = RuntimeResult<T>;
const JS_BUDGET: u32 = 4096;
const MAX_OBJECTS: usize = 4096;
const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Default)]
pub(super) struct JavaScriptStore {
    tasks: BTreeMap<u32, JsRecord>,
    // Native1006 can be called by a root handler while its real JS engine is
    // temporarily borrowed by Adapter. Keep this mutable object field live.
    save_services: BTreeMap<u32, u32>,
    surfaces: BTreeMap<u32, RgbaFrame>,
    input_owners: BTreeMap<u32, u32>,
}

#[derive(Default)]
struct JsRecord {
    engine: Option<JavaScriptTask>,
    loading: Option<Vec<u8>>,
    loading_sender: Option<u32>,
    initialization_complete: bool,
    queued: VecDeque<QueuedLoad>,
    objects: BTreeMap<u32, Object>,
    running: bool,
    exit: Option<u32>,
    update: Option<Update>,
    default_list: Option<u32>,
    context_owner: Option<u32>,
    save_dispatch: Option<save::SaveDispatch>,
    callback: Option<Callback>,
    inputs: VecDeque<JavaScriptInput>,
    active_input: Option<JavaScriptInput>,
    input_complete: bool,
    input_sender: Option<u32>,
}

struct Callback {
    sender: u32,
    name: Vec<u8>,
    first: u32,
    count: u32,
    arguments: Vec<Vec<u16>>,
    complete: bool,
}

#[derive(Clone)]
struct QueuedLoad {
    sender: u32,
    name: Vec<u8>,
}

struct Update {
    time: u32,
    objects: Vec<u32>,
    cursor: usize,
    script_complete: bool,
}

struct Geometry {
    position: [f32; 3],
    display: u32,
    controller: u32,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            position: [0.; 3],
            display: 0,
            controller: 0,
        }
    }
}

enum Object {
    Image {
        geometry: Geometry,
        plane: Option<u32>,
        scene_base: bool,
        source: Vec<u8>,
        layers: Vec<RgbaFrame>,
    },
    Plane {
        geometry: Geometry,
        task: u32,
        size: [u32; 2],
    },
    Sound {
        channel: Option<u32>,
        resource: Vec<u8>,
        group: Vec<u8>,
    },
}

impl Object {
    fn geometry(&self) -> Result<&Geometry> {
        match self {
            Self::Image { geometry, .. } | Self::Plane { geometry, .. } => Ok(geometry),
            _ => Err(native_state_error("JavaScript object has no geometry")),
        }
    }
    fn geometry_mut(&mut self) -> Result<&mut Geometry> {
        match self {
            Self::Image { geometry, .. } | Self::Plane { geometry, .. } => Ok(geometry),
            _ => Err(native_state_error("JavaScript object has no geometry")),
        }
    }
}

impl JavaScriptStore {
    pub(super) fn supports(index: u32) -> bool {
        matches!(index, 974..=976 | 1006)
    }

    pub(super) fn register(&mut self, owner: u32) {
        self.tasks.entry(owner).or_default();
        self.save_services.entry(owner).or_default();
    }

    /// The input platform supplies real message data1/kind and logical point.
    /// This method deliberately does not interpret FES event numbers as keys.
    pub(super) fn queue_region_input(
        &mut self,
        region: u32,
        input: JavaScriptInput,
    ) -> Result<bool> {
        let Some(&owner) = self.input_owners.get(&region) else {
            return Ok(false);
        };
        self.queue_task_input(owner, input)?;
        Ok(true)
    }

    pub(super) fn queue_task_input(&mut self, owner: u32, input: JavaScriptInput) -> Result<()> {
        let record = self
            .tasks
            .get_mut(&owner)
            .ok_or_else(|| native_state_error("JavaScript input region has no live owner"))?;
        if !record.running {
            return Ok(());
        }
        if record.inputs.len() >= 4096 {
            return Err(native_state_error(
                "JavaScript logical input queue exceeds 4096 packets",
            ));
        }
        record.inputs.push_back(input);
        Ok(())
    }

    /// Called in the existing scene plane order, before the primitive fallback.
    /// The surface contains the actual HG layers composited during this update.
    pub(super) fn render_plane(
        &self,
        canvas: &mut Canvas,
        node: &super::SceneNode,
    ) -> anyhow::Result<bool> {
        let Some(surface) = self.surfaces.get(&node.id) else {
            return Ok(false);
        };
        if !node.visible || !node.initialized {
            return Ok(true);
        }
        let mut options =
            ImageDrawOptions::new(Point::new(node.position[0] as i32, node.position[1] as i32));
        options.scale_x = node.scale[0];
        options.scale_y = node.scale[1];
        canvas.draw_frame(surface, &options)?;
        Ok(true)
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn owns_plane(&self, id: u32) -> bool {
        self.surfaces.contains_key(&id)
    }
}

impl LoggingHost<'_> {
    pub(super) fn javascript_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &Vm,
    ) -> Result<NativeReturn> {
        let required = if index == 976 { 16 } else { 8 };
        if args.len() != required {
            return Err(native_state_error(
                "JavaScript native has an invalid argument byte count",
            ));
        }
        let mut target = read_u32(args, 0)?;
        if target == 0 && matches!(index, 976 | 1006) {
            target = self
                .task_parents
                .get(&self.current_task_id)
                .copied()
                .ok_or_else(|| {
                    native_state_error("JavaScript default parent task is unavailable")
                })?;
        }
        if index == 974 {
            let list = read_u32(args, 4)?;
            if self
                .data_services
                .tasks
                .message_task_state(list)
                .is_none_or(|(kind, deleted, _)| kind != 15 || deleted != 0)
                || !self
                    .data_services
                    .tasks
                    .fes
                    .platform
                    .lists
                    .contains_key(&list)
            {
                return Err(native_state_error(
                    "JavaScript522 needs an initialized live image-list task",
                ));
            }
            let record = self
                .data_services
                .tasks
                .javascript
                .tasks
                .get_mut(&target)
                .ok_or_else(|| native_state_error("JavaScript522 target is not registered"))?;
            if record.update.is_some()
                || record.loading.is_some()
                || record.callback.is_some()
                || record.active_input.is_some()
            {
                return Err(native_state_error(
                    "cannot replace an executing JavaScript context",
                ));
            }
            record.default_list = Some(list);
            record.context_owner = Some(self.current_task_id);
            self.data_services.tasks.bind_javascript_context(
                target,
                self.current_task_id,
                vm.external_binding(),
            )?;
            self.prepare_javascript_input(target, list)?;
            return Ok(NativeReturn::Continue);
        }
        if index == 1006 {
            self.data_services
                .tasks
                .javascript
                .save_services
                .get_mut(&target)
                .ok_or_else(|| native_state_error("JavaScript524 target is not registered"))?
                .clone_from(&read_u32(args, 4)?);
            return Ok(NativeReturn::Continue);
        }
        let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        if index == 976 {
            return if self.call_javascript_function(
                target,
                self.current_task_id,
                trim_nul(&name),
                read_u32(args, 8)?,
                read_u32(args, 12)?,
            )? {
                Ok(NativeReturn::Continue)
            } else {
                Ok(NativeReturn::Retry)
            };
        }
        if self.load_javascript_task(target, self.current_task_id, trim_nul(&name), JS_BUDGET as usize)? {
            Ok(NativeReturn::Continue)
        } else {
            Ok(NativeReturn::Retry)
        }
    }

    fn prepare_javascript_input(&mut self, owner: u32, list: u32) -> Result<()> {
        let size = self
            .data_services
            .tasks
            .fes
            .platform
            .lists
            .get(&list)
            .ok_or_else(|| native_state_error("JavaScript input list disappeared"))?
            .size;
        let mut priority = 1i32;
        let mut current = self.task_parents.get(&owner).copied();
        while let Some(id) = current {
            if priority >= 256 {
                return Err(native_state_error(
                    "JavaScript input ancestor chain exceeds 256 levels",
                ));
            }
            if id == 0 || self.scene_nodes.get(&id).is_some_and(|node| node.kind == 1) {
                break;
            }
            priority += 1;
            current = self.task_parents.get(&id).copied();
        }
        // 005104f8 creates a true type6 child; 00510599 registers and
        // enables its logical rectangle. This is independent of CsPlane.
        let button = self.create_task_object(6, owner, b"js base button", b"")?;
        self.data_services
            .tasks
            .javascript
            .input_owners
            .insert(button, owner);
        super::fes_task_platform::register_region(
            self,
            button,
            priority,
            [0, 0, size[0] as i32, size[1] as i32],
            true,
        )
        .map_err(|cause| script_error(cause.to_string()))?;
        if let Some(node) = self.scene_nodes.get_mut(&button) {
            node.initialized = true;
            node.size = size.map(|value| value as f32);
        }
        Ok(())
    }

    pub(super) fn dispatch_javascript_input(
        &mut self,
        owner: u32,
        sender: u32,
        input: JavaScriptInput,
        budget: usize,
    ) -> Result<bool> {
        let mut record = self
            .data_services
            .tasks
            .javascript
            .tasks
            .remove(&owner)
            .ok_or_else(|| native_state_error("JavaScript input target is not registered"))?;
        let scope = match self.enter_task_values(owner) {
            Ok(scope) => scope,
            Err(cause) => {
                self.data_services
                    .tasks
                    .javascript
                    .tasks
                    .insert(owner, record);
                return Err(cause.into());
            }
        };
        let saved = (self.current_task_id, self.task_time, self.task_origin);
        self.current_task_id = owner;
        if let Some(clock) = self.data_services.tasks.task_clock(owner) {
            self.task_time = clock.current;
            self.task_origin = clock.origin;
        }
        let result = (|| {
            if !record.running && record.active_input.is_none() {
                return Ok(true);
            }
            if record.loading.is_some() || record.callback.is_some() || record.update.is_some() {
                return Err(native_state_error(
                    "synchronous JavaScript input cannot reenter its active program",
                ));
            }
            if record.input_sender.is_some_and(|saved| saved != sender)
                || (record.active_input.is_some() && record.input_sender.is_none())
            {
                return Err(native_state_error(
                    "JavaScript input continuation belongs to another sender",
                ));
            }
            let input = match record.active_input {
                Some(saved) if saved.value == input.value && saved.kind == input.kind => saved,
                Some(_) => {
                    return Err(native_state_error(
                        "JavaScript input continuation changed its packet",
                    ));
                }
                None => input,
            };
            record.input_sender = Some(sender);
            let done = self.execute_javascript_input(owner, &mut record, input, budget)?;
            if done {
                record.input_sender = None;
            }
            Ok(done)
        })();
        (self.current_task_id, self.task_time, self.task_origin) = saved;
        self.leave_task_values(scope);
        self.data_services
            .tasks
            .javascript
            .tasks
            .insert(owner, record);
        result
    }

    fn execute_javascript_input(
        &mut self,
        owner: u32,
        record: &mut JsRecord,
        input: JavaScriptInput,
        budget: usize,
    ) -> Result<bool> {
        if record.active_input.is_some_and(|saved| saved != input) {
            return Err(native_state_error(
                "JavaScript input continuation changed its logical event",
            ));
        }
        if record.active_input.is_none() {
            record.input_complete = false;
        }
        record.active_input = Some(input);
        if !record.input_complete {
            let mut engine = record.engine.take().ok_or_else(|| {
                native_state_error("running JavaScript input target has no interpreter")
            })?;
            let result = engine.dispatch_input(
                input,
                &mut Adapter {
                    host: self,
                    owner,
                    record,
                },
                budget,
            );
            record.engine = Some(engine);
            if result? == JavaScriptStep::Pending {
                return Ok(false);
            }
            record.input_complete = true;
        }
        self.publish_javascript(owner, record)?;
        record.active_input = None;
        Ok(true)
    }

    fn call_javascript_function(
        &mut self,
        owner: u32,
        sender: u32,
        name: &[u8],
        first: u32,
        count: u32,
    ) -> Result<bool> {
        if count > 64 {
            return Err(native_state_error(
                "JavaScript523 argument count exceeds 64",
            ));
        }
        let mut record = self
            .data_services
            .tasks
            .javascript
            .tasks
            .remove(&owner)
            .ok_or_else(|| native_state_error("JavaScript523 target is not registered"))?;
        let values = match self.enter_task_values(owner) {
            Ok(values) => values,
            Err(cause) => {
                self.data_services
                    .tasks
                    .javascript
                    .tasks
                    .insert(owner, record);
                return Err(cause.into());
            }
        };
        let saved = (self.current_task_id, self.task_time, self.task_origin);
        self.current_task_id = owner;
        if let Some(clock) = self.data_services.tasks.task_clock(owner) {
            self.task_time = clock.current;
            self.task_origin = clock.origin;
        }
        let result = (|| {
            if record.loading.is_some() || record.update.is_some() || record.active_input.is_some()
            {
                return Err(native_state_error(
                    "JavaScript523 cannot reenter an active program",
                ));
            }
            let Some(mut engine) = record.engine.take() else {
                return Ok(true);
            };
            let result = (|| {
                if let Some(callback) = &record.callback {
                    if callback.sender != sender
                        || callback.name != name
                        || callback.first != first
                        || callback.count != count
                    {
                        return Err(native_state_error(
                            "JavaScript523 continuation changed its argument range",
                        ));
                    }
                } else {
                    let mut arguments = Vec::with_capacity(count as usize);
                    for offset in 0..count {
                        let value = self
                            .string_values
                            .get(&first.wrapping_add(offset))
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        arguments.push(decode(self.text_encoding, value)?.encode_utf16().collect());
                    }
                    record.callback = Some(Callback {
                        sender,
                        name: name.to_vec(),
                        first,
                        count,
                        arguments,
                        complete: false,
                    });
                }
                let callback = record.callback.as_ref().unwrap();
                if !callback.complete {
                    let callback_name = decode(self.text_encoding, &callback.name)?;
                    let arguments = callback.arguments.clone();
                    let step = engine.call_function(
                        callback_name,
                        arguments,
                        &mut Adapter {
                            host: self,
                            owner,
                            record: &mut record,
                        },
                        JS_BUDGET as usize,
                    )?;
                    if step == JavaScriptStep::Pending {
                        return Ok(false);
                    }
                    record.callback.as_mut().unwrap().complete = true;
                }
                self.publish_javascript(owner, &record)?;
                record.callback = None;
                Ok(true)
            })();
            record.engine = Some(engine);
            result
        })();
        (self.current_task_id, self.task_time, self.task_origin) = saved;
        self.leave_task_values(values);
        self.data_services
            .tasks
            .javascript
            .tasks
            .insert(owner, record);
        result
    }

    /// Raw 69/70 command 158 copies the source name before retaining a request.
    /// Queued packets contain bytes, never the sender VM's address.
    pub(super) fn dispatch_javascript_message(
        &mut self,
        target: u32,
        packet: TaskMessage,
        queued: bool,
        output: &mut u32,
        vm: Option<&mut Vm>,
    ) -> Result<bool> {
        if packet[1] != 158 {
            return Ok(false);
        }
        if packet[2] != 0 {
            return Err(native_state_error(
                "JavaScript command158 with inline script source has no recovered ABI",
            ));
        }
        let vm =
            vm.ok_or_else(|| native_state_error("JavaScript158 needs the sender name buffer"))?;
        let name = vm.read_string(packet[3])?;
        let name = trim_nul(&name).to_vec();
        if queued {
            let record = self
                .data_services
                .tasks
                .javascript
                .tasks
                .get_mut(&target)
                .ok_or_else(|| {
                    native_state_error("type55 JavaScript receiver was not registered")
                })?;
            if record.queued.len() >= 4096 {
                return Err(native_state_error(
                    "JavaScript load queue exceeds 4096 commands",
                ));
            }
            record.queued.push_back(QueuedLoad {
                sender: packet[0],
                name,
            });
            *output = 1;
            return Ok(true);
        }
        if !self.load_javascript_task(target, packet[0], &name, JS_BUDGET as usize)? {
            return Ok(false);
        }
        *output = 1;
        Ok(true)
    }

    /// Explicit buffers for FES/platform producers. KCS pointers are copied by
    /// the raw message adapter; typed callers supply the filename bytes here.
    pub(crate) fn receive_javascript_command(
        &mut self,
        command: TaskCommand,
        buffers: &mut TaskCommandBuffers<'_>,
        _output: &mut u32,
        _budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let owner = command.target as u32;
        let done = match command.code {
            158 => {
                if command.data[0] != 0 {
                    return Err(native_state_error(
                        "JavaScript158 inline program ABI is not recovered",
                    ));
                }
                let bytes = buffers.data1.as_deref().ok_or_else(|| {
                    native_state_error("JavaScript158 requires owned filename bytes")
                })?;
                let end = bytes.iter().position(|&byte| byte == 0).ok_or_else(|| {
                    native_state_error("JavaScript158 filename lacks a terminator")
                })?;
                self.load_javascript_task(owner, command.sender as u32, &bytes[..end], _budget)?
            }
            524 => {
                self.data_services
                    .tasks
                    .javascript
                    .save_services
                    .get_mut(&owner)
                    .ok_or_else(|| native_state_error("JavaScript524 target is absent"))?
                    .clone_from(&command.data[1]);
                true
            }
            522 | 523 => {
                return Err(native_state_error(
                    "raw JavaScript context/function records require a typed native974/976 producer",
                ));
            }
            // v4.01 message table at 005106c0/005106d4 maps
            // 6/19 to 00510411; 64/65 fail the bounded table check and
            // reach the same return-one branch. No JS handler is called.
            6 | 19 | 64 | 65 => {
                return Ok(Some(TaskCommandOutcome {
                    done: true,
                    propagate: true,
                }));
            }
            9 | 10 => self.dispatch_javascript_input(
                owner,
                command.sender as u32,
                JavaScriptInput {
                    value: command.data[1],
                    kind: u32::from(command.code == 9),
                    pointer: self.input_task.pointer_position,
                },
                _budget,
            )?,
            _ => return Ok(None),
        };
        Ok(Some(TaskCommandOutcome {
            done,
            propagate: false,
        }))
    }

    pub(super) fn load_javascript_task(
        &mut self,
        owner: u32,
        sender: u32,
        name: &[u8],
        budget: usize,
    ) -> Result<bool> {
        if budget == 0 { return Ok(false); }
        if self
            .data_services
            .tasks
            .message_task_state(owner)
            .is_none_or(|(kind, deletion, _)| kind != 55 || deletion != 0)
        {
            return Err(native_state_error(
                "JavaScript load target is not a live type55 task",
            ));
        }
        let mut record = self
            .data_services
            .tasks
            .javascript
            .tasks
            .remove(&owner)
            .ok_or_else(|| native_state_error("JavaScript task state is absent"))?;
        let values = match self.enter_task_values(owner) {
            Ok(values) => values,
            Err(cause) => {
                self.data_services
                    .tasks
                    .javascript
                    .tasks
                    .insert(owner, record);
                return Err(cause.into());
            }
        };
        let previous_task = self.current_task_id;
        let previous_time = self.task_time;
        let previous_origin = self.task_origin;
        self.current_task_id = owner;
        if let Some(clock) = self.data_services.tasks.task_clock(owner) {
            self.task_time = clock.current;
            self.task_origin = clock.origin;
        }
        let result = (|| {
            if let Some(loading) = &record.loading {
                if loading != name || record.loading_sender != Some(sender) {
                    return Err(native_state_error(
                        "JavaScript load continuation changed its resource name",
                    ));
                }
            } else {
                if record.update.is_some()
                    || record.callback.is_some()
                    || record.active_input.is_some()
                    || record.save_dispatch.is_some()
                {
                    return Err(native_state_error(
                        "cannot reload a JavaScript task during its active frame",
                    ));
                }
                let decoded = decode(self.text_encoding, name)?;
                let bytes = self
                    .resources
                    .read_category(b"javascript", &decoded)
                    .map_err(|cause| script_error(format!("cannot load {name:?}: {cause}")))?;
                let source = if let Some(source) = bytes.strip_prefix(b"\xef\xbb\xbf") {
                    std::str::from_utf8(source)
                        .map(str::to_owned)
                        .map_err(|_| {
                            native_state_error("JavaScript UTF-8 BOM source is malformed")
                        })?
                } else {
                    decode(self.text_encoding, &bytes)?
                };
                let engine = JavaScriptTask::new(name.to_vec(), source, JS_BUDGET)
                    .map_err(|cause| script_error(cause.to_string()))?;
                self.release_javascript_objects(owner, &mut record)?;
                record.engine = Some(engine);
                record.loading = Some(name.to_vec());
                record.loading_sender = Some(sender);
                record.initialization_complete = false;
                record.running = true;
                record.exit = None;
            }
            let time = self
                .data_services
                .tasks
                .task_clock(owner)
                .ok_or_else(|| native_state_error("JavaScript task has no clock"))?
                .elapsed();
            if !record.initialization_complete {
                let mut engine = record
                    .engine
                    .take()
                    .ok_or_else(|| native_state_error("JavaScript interpreter is absent"))?;
                let result = engine.advance(
                    time,
                    &mut Adapter {
                        host: self,
                        owner,
                        record: &mut record,
                    },
                    budget,
                );
                record.engine = Some(engine);
                if result? == JavaScriptStep::Pending {
                    return Ok(false);
                }
                record.initialization_complete = true;
            }
            self.publish_javascript(owner, &record)?;
            record.loading = None;
            record.loading_sender = None;
            Ok(true)
        })();
        self.current_task_id = previous_task;
        self.task_time = previous_time;
        self.task_origin = previous_origin;
        self.leave_task_values(values);
        self.data_services
            .tasks
            .javascript
            .tasks
            .insert(owner, record);
        result
    }

    pub(super) fn update_javascript_task(&mut self, owner: u32, budget: usize) -> Result<bool> {
        if budget == 0 {
            return Ok(false);
        }
        // A queued load is completed before motion/GameLoop and is removed
        // only after its one initialization has really finished.
        if let Some(load) = self
            .data_services
            .tasks
            .javascript
            .tasks
            .get(&owner)
            .and_then(|record| record.queued.front())
            .cloned()
        {
            if !self.load_javascript_task(owner, load.sender, &load.name, budget)? {
                return Ok(false);
            }
            self.data_services
                .tasks
                .javascript
                .tasks
                .get_mut(&owner)
                .unwrap()
                .queued
                .pop_front();
        }
        let mut record = self
            .data_services
            .tasks
            .javascript
            .tasks
            .remove(&owner)
            .ok_or_else(|| native_state_error("type55 update has no JavaScript state"))?;
        let values = match self.enter_task_values(owner) {
            Ok(values) => values,
            Err(cause) => {
                self.data_services
                    .tasks
                    .javascript
                    .tasks
                    .insert(owner, record);
                return Err(cause.into());
            }
        };
        let previous_task = self.current_task_id;
        let previous_time = self.task_time;
        let previous_origin = self.task_origin;
        self.current_task_id = owner;
        if let Some(clock) = self.data_services.tasks.task_clock(owner) {
            self.task_time = clock.current;
            self.task_origin = clock.origin;
        }
        let result = (|| {
            if record.loading.is_some() || record.callback.is_some() {
                return Err(native_state_error(
                    "JavaScript task update overlaps an unfinished synchronous load",
                ));
            }
            if !record.running && record.update.is_none() && record.active_input.is_none() {
                record.inputs.clear();
                return Ok(true);
            }
            for _ in 0..budget {
                if !record.running && record.active_input.is_none() {
                    break;
                }
                let Some(input) = record.inputs.front().copied() else {
                    break;
                };
                if record.update.is_some() {
                    break; // Input arriving during a frame yield waits for its next update.
                }
                if !self.execute_javascript_input(owner, &mut record, input, budget)? {
                    return Ok(false);
                }
                record.inputs.pop_front();
            }
            if record.active_input.is_some() {
                // A synchronous 69 owns this continuation, and must finish
                // it before the task's normal body can begin.
                return Err(native_state_error(
                    "JavaScript body overlaps synchronous input continuation",
                ));
            }
            if !record.running && record.update.is_none() {
                record.inputs.clear();
                return Ok(true);
            }
            if !record.inputs.is_empty() && record.update.is_none() {
                return Ok(false);
            }
            let time = self
                .data_services
                .tasks
                .task_clock(owner)
                .ok_or_else(|| native_state_error("JavaScript task clock is absent"))?
                .elapsed();
            let update = record.update.get_or_insert_with(|| Update {
                time,
                objects: record.objects.keys().copied().collect(),
                cursor: 0,
                script_complete: false,
            });
            if update.time != time {
                return Err(native_state_error(
                    "JavaScript continuation advanced its frame time",
                ));
            }
            while let Some(&id) = record
                .update
                .as_ref()
                .unwrap()
                .objects
                .get(record.update.as_ref().unwrap().cursor)
            {
                if let Some(object) = record.objects.get_mut(&id)
                    && let Ok(geometry) = object.geometry_mut()
                    && geometry.controller != 0
                {
                    let controller = self
                        .motion_controllers
                        .get_mut(&geometry.controller)
                        .ok_or_else(|| {
                            native_state_error("JavaScript motion controller disappeared")
                        })?;
                    let mut services = DesktopMotionServices {
                        random: self.script_random,
                        events: self.events,
                        frame: self.frame,
                    };
                    if matches!(
                        advance_motion_controller(
                            controller,
                            time,
                            false,
                            false,
                            &mut services,
                            budget
                        )?,
                        NativeReturn::Retry
                    ) {
                        return Ok(false);
                    }
                    geometry.position =
                        std::array::from_fn(|axis| f32::from_bits(controller.properties[axis]));
                    if let Object::Plane { geometry, task, .. } = object {
                        let node = self.scene_nodes.get_mut(task).ok_or_else(|| {
                            native_state_error("JavaScript plane task was released")
                        })?;
                        node.position = geometry.position;
                    }
                }
                record.update.as_mut().unwrap().cursor += 1;
            }
            if !record.update.as_ref().unwrap().script_complete {
                let mut engine = record.engine.take().ok_or_else(|| {
                    native_state_error("running JavaScript task has no loaded program")
                })?;
                let result = engine.advance(
                    time,
                    &mut Adapter {
                        host: self,
                        owner,
                        record: &mut record,
                    },
                    budget,
                );
                record.engine = Some(engine);
                if result? == JavaScriptStep::Pending {
                    return Ok(false);
                }
                record.update.as_mut().unwrap().script_complete = true;
            }
            self.publish_javascript(owner, &record)?;
            record.update = None;
            Ok(true)
        })();
        self.current_task_id = previous_task;
        self.task_time = previous_time;
        self.task_origin = previous_origin;
        self.leave_task_values(values);
        self.data_services
            .tasks
            .javascript
            .tasks
            .insert(owner, record);
        result
    }

    fn publish_javascript(&mut self, owner: u32, record: &JsRecord) -> Result<()> {
        if let Some(code) = record.exit {
            self.data_services.tasks.write_task_property(owner, code)?;
        }
        if let Some(list_id) = record.default_list {
            let list = self
                .data_services
                .tasks
                .fes
                .platform
                .lists
                .get_mut(&list_id)
                .ok_or_else(|| native_state_error("JavaScript default image list was released"))?;
            list.layers.retain(|(layer_owner, name), _| {
                *layer_owner != owner || !name.starts_with("javascript:")
            });
            for (&id, object) in &record.objects {
                let Object::Image {
                    geometry,
                    plane: None,
                    layers,
                    ..
                } = object
                else {
                    continue;
                };
                if geometry.display == 0 {
                    continue;
                }
                for (part, frame) in layers.iter().enumerate() {
                    let metadata = frame.metadata();
                    list.layers.insert(
                        (owner, format!("javascript:{id}:{part}")),
                        super::fes_task_platform::Layer {
                            frame: frame.clone(),
                            options: ImageDrawOptions::new(Point::new(
                                (geometry.position[0] as i32)
                                    .wrapping_add(metadata.offset[0])
                                    .wrapping_sub(metadata.base[0]),
                                (geometry.position[1] as i32)
                                    .wrapping_add(metadata.offset[1])
                                    .wrapping_sub(metadata.base[1]),
                            )),
                            priority: id as i32,
                            affine: None,
                        },
                    );
                }
            }
            super::fes_task_platform::compose_list(self, list_id, false)
                .map_err(|cause| script_error(cause.to_string()))?;
        }
        for object in record.objects.values() {
            let Object::Plane {
                geometry,
                task,
                size,
            } = object
            else {
                continue;
            };
            let mut canvas =
                Canvas::new(size[0], size[1]).map_err(|cause| script_error(cause.to_string()))?;
            canvas.clear(Color::TRANSPARENT);
            for image in record.objects.values() {
                let Object::Image {
                    geometry: image_geometry,
                    plane: Some(plane),
                    layers,
                    ..
                } = image
                else {
                    continue;
                };
                if plane != task || image_geometry.display == 0 {
                    continue;
                }
                for layer in layers {
                    let metadata = layer.metadata();
                    let at = Point::new(
                        (image_geometry.position[0] as i32)
                            .wrapping_add(metadata.offset[0])
                            .wrapping_sub(metadata.base[0]),
                        (image_geometry.position[1] as i32)
                            .wrapping_add(metadata.offset[1])
                            .wrapping_sub(metadata.base[1]),
                    );
                    canvas
                        .draw_frame(layer, &ImageDrawOptions::new(at))
                        .map_err(|cause| script_error(cause.to_string()))?;
                }
            }
            let snapshot = canvas.snapshot();
            let surface = RgbaFrame::from_rgba8(
                snapshot.width(),
                snapshot.height(),
                snapshot.rgba().to_vec(),
            )
            .map_err(|cause| script_error(cause.to_string()))?;
            self.data_services
                .tasks
                .javascript
                .surfaces
                .insert(*task, surface);
            let node = self
                .scene_nodes
                .get_mut(task)
                .ok_or_else(|| native_state_error("JavaScript plane task was released"))?;
            node.position = geometry.position;
            node.visible = geometry.display != 0;
        }
        Ok(())
    }

    pub(super) fn release_javascript_task(&mut self, owner: u32, budget: usize) -> Result<bool> {
        if budget == 0 {
            return Ok(!self.data_services.tasks.javascript.tasks.contains_key(&owner));
        }
        let Some(mut record) = self.data_services.tasks.javascript.tasks.remove(&owner) else {
            return Ok(true);
        };
        // The registered parent KCS can delete this JS sender while its save/load
        // notification is suspended. Ack that same packet before dropping
        // the engine, owned buffers or retained synchronous dispatch.
        if record.save_dispatch.is_some() {
            let previous = self.current_task_id;
            self.current_task_id = owner;
            let result = Adapter { host: self, owner, record: &mut record }
                .finish_save_dispatch(budget);
            self.current_task_id = previous;
            if !matches!(result, Ok(true)) {
                self.data_services.tasks.javascript.tasks.insert(owner, record);
                return result;
            }
        }
        // No further bridge call may race the removal of an owned input
        // region or object. Drop cancels the actual retained worker/future.
        record.engine.take();
        let regions = self
            .data_services
            .tasks
            .javascript
            .input_owners
            .iter()
            .filter_map(|(&region, &actual_owner)| (actual_owner == owner).then_some(region))
            .collect::<Vec<_>>();
        for region in regions {
            self.input_task.regions.retain(|input| input.id() != region);
            self.data_services
                .tasks
                .javascript
                .input_owners
                .remove(&region);
        }
        // Retain the object cursor and CSS binding on host release errors.
        if let Err(cause) = self.release_javascript_objects(owner, &mut record) {
            self.data_services
                .tasks
                .javascript
                .tasks
                .insert(owner, record);
            return Err(cause);
        }
        self.data_services.tasks.javascript.save_services.remove(&owner);
        Ok(true)
    }

    fn release_javascript_objects(&mut self, owner: u32, record: &mut JsRecord) -> Result<()> {
        if let Some(list) = record
            .default_list
            .and_then(|id| self.data_services.tasks.fes.platform.lists.get_mut(&id))
        {
            list.layers.retain(|(layer_owner, name), _| {
                *layer_owner != owner || !name.starts_with("javascript:")
            });
        }
        while let Some((&id, object)) = record.objects.first_key_value() {
            match object {
                Object::Sound {
                    channel: Some(channel),
                    ..
                } => {
                    self.control_sound_values(156, *channel, 0, 0)?;
                }
                Object::Plane { task, .. } => {
                    self.data_services.tasks.javascript.surfaces.remove(task);
                    if self.task_is_reachable(*task) {
                        self.request_ordinary_task_delete(*task)?;
                    }
                }
                _ => {}
            }
            if let Ok(geometry) = object.geometry() {
                self.motion_controllers.remove(&geometry.controller);
            }
            record.objects.remove(&id);
        }
        Ok(())
    }
}

struct Adapter<'a, 'host> {
    host: &'a mut LoggingHost<'host>,
    owner: u32,
    record: &'a mut JsRecord,
}

impl JavaScriptHost for Adapter<'_, '_> {
    fn javascript_call(
        &mut self,
        call: &JavaScriptCall,
        budget: usize,
    ) -> RuntimeResult<Option<Value>> {
        if save::supports(&call.operation) {
            return self.save_call(call, budget);
        }
        self.call(call).map(Some)
    }
}

impl Adapter<'_, '_> {
    fn call(&mut self, call: &JavaScriptCall) -> Result<Value> {
        let args = &call.arguments;
        match call.operation.as_str() {
            "cvMouseX" | "cvMouseY" => {
                let axis = usize::from(call.operation == "cvMouseY");
                return Ok(Value::Number(f64::from(
                    self.record
                        .active_input
                        .map_or(self.host.pointer_position[axis], |input| {
                            input.pointer[axis]
                        }),
                )));
            }
            "cfCalc" => {
                if self.record.context_owner.is_none() {
                    // 0053383a..0053391a: a string without the real
                    // interpreter binding follows the integer-zero branch.
                    return Ok(Value::Number(0.));
                }
                let source = encoded(self.host.text_encoding, args, 0)?;
                return Ok(Value::Number(f64::from(
                    self.host.eval_script_expression(&source, false)?,
                )));
            }
            "cfGetVar" | "cfSetVar" | "cfGetStrVar" | "cfSetStrVar" => {
                if self.record.context_owner.is_none() {
                    return Err(native_state_error(
                        "JavaScript variable access requires native974 context binding",
                    ));
                }
                let index = uint(args, 0)?;
                match call.operation.as_str() {
                    // 005d90d0 checks the unsigned bound and returns an
                    // actual zero for absent/out-of-range numeric cells.
                    "cfGetVar" => {
                        return Ok(Value::Number(f64::from(
                            self.host
                                .shared_values
                                .get(index as usize)
                                .copied()
                                .unwrap_or(0) as i32,
                        )));
                    }
                    "cfSetVar" => {
                        let value = uint(args, 1)?;
                        // 005d90f0 never extends the interpreter's value area.
                        if let Some(slot) = self.host.shared_values.get_mut(index as usize) {
                            *slot = value;
                        }
                    }
                    "cfGetStrVar" => {
                        let raw = self
                            .host
                            .string_values
                            .get(&index)
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        // 00533ffc passes the fixed 4096-byte conversion
                        // buffer. Never fabricate a malformed multibyte tail.
                        if raw.len() >= 4096 {
                            return Err(native_state_error(
                                "JavaScript string-variable bounded conversion is not recovered above 4095 bytes",
                            ));
                        }
                        let value = decode(self.host.text_encoding, trim_nul(raw))?;
                        return Ok(Value::String(value.encode_utf16().collect()));
                    }
                    "cfSetStrVar" => {
                        let value = string(args, 1)?;
                        let mut raw =
                            super::sound_native::encode_name(self.host.text_encoding, &value)?;
                        raw.truncate(raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len()));
                        if raw.len() > 16 * 1024 * 1024 {
                            return Err(native_state_error(
                                "JavaScript string variable exceeds 16 MiB",
                            ));
                        }
                        self.host.string_values.insert(index, raw);
                    }
                    _ => unreachable!(),
                }
                return Ok(Value::Undefined);
            }
            "cfLog" => {
                let text = string(args, 0)?;
                self.host.events.push(LogicEvent {
                    frame: self.host.frame,
                    kind: "javascript-log".into(),
                    detail: text,
                });
                return Ok(Value::Undefined);
            }
            "cfExit" => {
                self.record.exit = Some(uint(args, 0)?);
                self.record.running = false;
                return Ok(Value::Undefined);
            }
            "new.CsImage" | "new.CsPlane" | "new.CsSound" => {
                if self.record.objects.len() >= MAX_OBJECTS {
                    return Err(native_state_error(
                        "JavaScript task exceeds 4096 host objects",
                    ));
                }
                let object = match call.operation.as_str() {
                    "new.CsImage" => Object::Image {
                        geometry: Geometry::default(),
                        plane: None,
                        scene_base: false,
                        source: Vec::new(),
                        layers: Vec::new(),
                    },
                    "new.CsPlane" => {
                        let default = self
                            .host
                            .canvas
                            .as_ref()
                            .map(|canvas| [canvas.width(), canvas.height()])
                            .or_else(|| {
                                self.record.default_list.and_then(|id| {
                                    self.host
                                        .data_services
                                        .tasks
                                        .fes
                                        .platform
                                        .lists
                                        .get(&id)
                                        .map(|list| list.size)
                                })
                            })
                            .ok_or_else(|| {
                                native_state_error("CsPlane default size has no logical surface")
                            })?;
                        let size = [
                            if matches!(args.first(), Some(Value::Undefined)) {
                                default[0]
                            } else {
                                uint(args, 0)?
                            },
                            if matches!(args.get(1), Some(Value::Undefined)) {
                                default[1]
                            } else {
                                uint(args, 1)?
                            },
                        ];
                        if size.contains(&0)
                            || u64::from(size[0]) * u64::from(size[1]) * 4 > MAX_IMAGE_BYTES as u64
                        {
                            return Err(native_state_error(
                                "JavaScript plane has invalid/oversized dimensions",
                            ));
                        }
                        let task = self
                            .host
                            .create_task_object(2, self.owner, b"JS Plane", b"")?;
                        // V8 Int32Value, then cvtdq2ps in 005363d8..00536424.
                        let position = [
                            uint(args, 2)? as i32 as f32,
                            uint(args, 3)? as i32 as f32,
                            if matches!(args.get(4), Some(Value::Undefined)) {
                                5000.
                            } else {
                                uint(args, 4)? as i32 as f32
                            },
                        ];
                        let node = self.host.scene_nodes.get_mut(&task).unwrap();
                        node.size = size.map(|value| value as f32);
                        node.position = position;
                        node.initialized = true;
                        self.host.data_services.planes.observe_existing(
                            79,
                            &words(&[task, (size[0] as f32).to_bits(), (size[1] as f32).to_bits()]),
                            self.host.scene_nodes,
                        )?;
                        self.host.scene_nodes.get_mut(&task).unwrap().position = position;
                        Object::Plane {
                            geometry: Geometry {
                                position,
                                ..Geometry::default()
                            },
                            task,
                            size,
                        }
                    }
                    _ => Object::Sound {
                        channel: None,
                        resource: Vec::new(),
                        group: Vec::new(),
                    },
                };
                let id = self.host.allocate_handle();
                self.record.objects.insert(id, object);
                return Ok(Value::Number(f64::from(id)));
            }
            _ => {}
        }
        let id = uint(args, 0)?;
        // Identities are private WeakMap entries in the JS realm and checked
        // against this task's registry, including the concrete object type.
        if !self.record.objects.contains_key(&id) {
            return Err(native_state_error(
                "JavaScript host object is released or belongs to another task",
            ));
        }
        match call.operation.as_str() {
            "object.setPos" => {
                let object = self.record.objects.get_mut(&id).unwrap();
                let geometry = object.geometry_mut()?;
                for axis in 0..3 {
                    if !matches!(args.get(axis + 1), Some(Value::Undefined) | None) {
                        geometry.position[axis] = float(args, axis + 1)?;
                    }
                }
                if geometry.controller != 0 {
                    let controller = self
                        .host
                        .motion_controllers
                        .get_mut(&geometry.controller)
                        .unwrap();
                    for axis in 0..3 {
                        controller.properties[axis] = geometry.position[axis].to_bits();
                        controller.base_properties[axis] = controller.properties[axis];
                    }
                }
            }
            "object.setDisp" => {
                self.record
                    .objects
                    .get_mut(&id)
                    .unwrap()
                    .geometry_mut()?
                    .display = uint(args, 1)?
            }
            "object.getDisp" => {
                return Ok(Value::Number(f64::from(
                    self.record.objects[&id].geometry()?.display,
                )));
            }
            "image.setPlane" => {
                let plane_id = uint(args, 1)?;
                let task = match self.record.objects.get(&plane_id) {
                    Some(Object::Plane { task, .. }) => *task,
                    _ => {
                        return Err(native_state_error(
                            "CsImage.setPlane expects this task's CsPlane",
                        ));
                    }
                };
                let Object::Image { plane, .. } = self.record.objects.get_mut(&id).unwrap() else {
                    return Err(native_state_error(
                        "CsImage.setPlane receiver is not an image",
                    ));
                };
                *plane = Some(task);
            }
            "image.setBaseType" => {
                let mode = string(args, 1)?;
                let Object::Image { scene_base, .. } = self.record.objects.get_mut(&id).unwrap()
                else {
                    return Err(native_state_error(
                        "CsImage.setBaseType receiver is not an image",
                    ));
                };
                // 0053d740: string 'scene' sets zero; every other string sets one.
                *scene_base = mode.eq_ignore_ascii_case("scene");
            }
            "image.load" => {
                let source = encoded(self.host.text_encoding, args, 1)?;
                let Object::Image {
                    scene_base,
                    source: saved,
                    layers,
                    ..
                } = self.record.objects.get_mut(&id).unwrap()
                else {
                    return Err(native_state_error("CsImage.load receiver is not an image"));
                };
                layers.clear();
                *saved = source.clone();
                *layers = load_image(self.host, &source, *scene_base)?;
                return Ok(Value::Boolean(!layers.is_empty()));
            }
            "object.motMove" => {
                let geometry = self.record.objects.get_mut(&id).unwrap().geometry_mut()?;
                let mut parameters = [0u32; 256];
                parameters[0] = (uint(args, 1)? as i32 as f32).to_bits();
                for axis in 0..3 {
                    let target = if matches!(args.get(axis + 2), Some(Value::Undefined) | None) {
                        geometry.position[axis]
                    } else {
                        float(args, axis + 2)?
                    };
                    parameters[axis + 4] = (target - geometry.position[axis]).to_bits();
                }
                let bytes = self
                    .host
                    .resources
                    .read_category(b"mot", "motMove")
                    .map_err(|cause| script_error(format!("motMove resource: {cause}")))?;
                let program = cs2_script::KcsProgram::parse(&bytes)?;
                if geometry.controller == 0 {
                    geometry.controller = self.host.allocate_handle();
                    let mut controller = MotionControllerState::default();
                    for axis in 0..3 {
                        controller.properties[axis] = geometry.position[axis].to_bits();
                        controller.base_properties[axis] = controller.properties[axis];
                    }
                    self.host
                        .motion_controllers
                        .insert(geometry.controller, controller);
                }
                let controller = self
                    .host
                    .motion_controllers
                    .get_mut(&geometry.controller)
                    .unwrap();
                let runner = create_motion_runner(
                    program,
                    b"motMove.kcs",
                    &parameters,
                    &[],
                    0,
                    controller.properties,
                    0,
                )?;
                let task_id = controller.next_task_id;
                controller.next_task_id = task_id
                    .checked_add(1)
                    .ok_or_else(|| native_state_error("JavaScript motion identity exhausted"))?;
                controller.tasks.insert(
                    task_id,
                    MotionTaskState {
                        name: b"motMove.kcs".to_vec(),
                        group: 0,
                        parameters,
                        saved_data: Vec::new(),
                        snapshot_flags: 0,
                        external_value: 0,
                        origin: 0,
                        elapsed: 0,
                        running: 1,
                        finishing: 0,
                        runner,
                    },
                );
            }
            "cfIsExecMotion" => {
                let controller = self.record.objects[&id].geometry()?.controller;
                let running =
                    self.host
                        .motion_controllers
                        .get(&controller)
                        .is_some_and(|controller| {
                            controller.tasks.values().any(|task| task.running != 0)
                        });
                return Ok(Value::Boolean(running));
            }
            "sound.load" => {
                let source = encoded(self.host.text_encoding, args, 1)?;
                let group = if matches!(args.get(2), Some(Value::Undefined) | None) {
                    Vec::new()
                } else {
                    encoded(self.host.text_encoding, args, 2)?
                };
                let Object::Sound {
                    channel,
                    resource,
                    group: saved_group,
                } = self.record.objects.get_mut(&id).unwrap()
                else {
                    return Err(native_state_error("CsSound.load receiver is not a sound"));
                };
                if let Some(channel) = channel.take() {
                    self.host.control_sound_values(156, channel, 0, 0)?;
                }
                *resource = source.clone();
                *saved_group = group.clone();
                let loaded = if group.is_empty() {
                    let target = self.host.select_free_sound_channel()?;
                    match self.host.load_sound_name(target, &source, 1, 0)? {
                        NativeReturn::Value(0) => None,
                        NativeReturn::Value(_) => Some(target),
                        _ => {
                            return Err(native_state_error(
                                "CsSound.load channel returned no load result",
                            ));
                        }
                    }
                } else {
                    let key = self
                        .host
                        .sound_groups
                        .iter()
                        .find(|entry| entry.label.eq_ignore_ascii_case(&group))
                        .map(|entry| entry.key);
                    match key {
                        Some(key) => {
                            match self.host.load_sound_group_name(key, true, &source, 1, 0)? {
                                NativeReturn::Value(u32::MAX) => None,
                                NativeReturn::Value(channel) => Some(channel),
                                _ => {
                                    return Err(native_state_error(
                                        "CsSound.load group returned no channel",
                                    ));
                                }
                            }
                        }
                        None => {
                            return Err(native_state_error(
                                "CsSound.load named sound group is unavailable",
                            ));
                        }
                    }
                };
                *channel = loaded;
                // 00545e17 stores the selected channel; cleanup returns
                // without writing the V8 return slot.
                return Ok(Value::Undefined);
            }
            "sound.play" | "sound.stop" => {
                let Object::Sound {
                    channel: Some(channel),
                    ..
                } = &self.record.objects[&id]
                else {
                    return Ok(Value::Undefined); // Original receiver accepts an unloaded sound object.
                };
                if call.operation == "sound.play" {
                    self.host
                        .control_sound_values(157, *channel, uint(args, 1)?, 0)?;
                } else {
                    self.host.control_sound_values(159, *channel, 0, 0)?;
                }
            }
            _ => {
                return Err(script_error(format!(
                    "JavaScript host method is not recovered: {}",
                    call.operation
                )));
            }
        }
        Ok(Value::Undefined)
    }
}

fn load_image(host: &LoggingHost<'_>, source: &[u8], scene_base: bool) -> Result<Vec<RgbaFrame>> {
    let source = trim_nul(source);
    let fields = source.split(|&byte| byte == b',').collect::<Vec<_>>();
    if fields.is_empty() || fields[0].is_empty() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    if fields.len() > 1 {
        if fields.len() > 64 {
            return Err(native_state_error(
                "CsImage scene descriptor exceeds 63 parts",
            ));
        }
        for (index, field) in fields[1..].iter().enumerate() {
            let Some(&part) = field.first() else {
                continue;
            };
            if part == b'0' {
                continue;
            }
            // VA0054b8e0..0054b9cc: each successive field gets one
            // additional leading ASCII zero after the shared underscore.
            if !part.is_ascii() {
                return Err(native_state_error(
                    "CsImage signed non-ASCII difference selector is not recovered",
                ));
            }
            let mut name = fields[0].to_vec();
            name.push(b'_');
            name.extend(std::iter::repeat_n(b'0', index));
            // 00629710 reads the first signed byte of each comma token.
            name.push(part);
            names.push(name);
        }
    } else if fields.len() == 1 {
        names.push(source.to_vec());
    }
    let mut layers = Vec::new();
    let mut bytes_used = 0usize;
    for name in names {
        let decoded = decode(host.text_encoding, &name)?;
        let names = host
            .resources
            .category_names(b"image", &decoded)
            .map_err(|cause| script_error(cause.to_string()))?;
        let Some(resource) = names.iter().find_map(|name| host.resources.find(name)) else {
            return Ok(Vec::new());
        };
        let bytes = host
            .resources
            .read_candidate(resource)
            .map_err(|cause| script_error(cause.to_string()))?;
        let file = HgFile::parse(&bytes).map_err(|cause| script_error(cause.to_string()))?;
        let Some(frame) = file.frames.first() else {
            return Ok(Vec::new());
        };
        let pixels = frame
            .decode_pixels_rgba8(&bytes)
            .map_err(|cause| script_error(cause.to_string()))?;
        bytes_used = bytes_used
            .checked_add(pixels.len())
            .filter(|&n| n <= MAX_IMAGE_BYTES)
            .ok_or_else(|| native_state_error("CsImage pixels exceed 256 MiB"))?;
        let (offset, base, canvas_size) = if scene_base {
            (frame.offset, frame.base, frame.canvas_size)
        } else {
            // 0053c860 normal mode saves offset-base as its geometric
            // anchor, clears source offsets and uses physical dimensions.
            (
                [0; 2],
                [
                    frame.offset[0].wrapping_sub(frame.base[0]),
                    frame.offset[1].wrapping_sub(frame.base[1]),
                ],
                [frame.width, frame.height],
            )
        };
        let image = RgbaFrame::from_rgba8(frame.width, frame.height, pixels)
            .map_err(|cause| script_error(cause.to_string()))?
            .with_raw_metadata(FrameMetadata {
                offset,
                base,
                canvas_size,
                ..FrameMetadata::raster([frame.width, frame.height])
            });
        layers.push(image);
    }
    Ok(layers)
}

fn words(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
fn uint(args: &[Value], index: usize) -> Result<u32> {
    args.get(index)
        .ok_or_else(|| native_state_error("JavaScript host numeric argument is absent"))?
        .uint32()
        .map_err(|cause| script_error(cause.to_string()))
}
fn float(args: &[Value], index: usize) -> Result<f32> {
    let value = args
        .get(index)
        .ok_or_else(|| native_state_error("JavaScript float argument is absent"))?
        .number()
        .map_err(|cause| script_error(cause.to_string()))? as f32;
    if !value.is_finite() {
        return Err(native_state_error(
            "JavaScript geometry value is non-finite",
        ));
    }
    Ok(value)
}
fn string(args: &[Value], index: usize) -> Result<String> {
    let Some(Value::String(value)) = args.get(index) else {
        return Err(native_state_error(
            "JavaScript host string argument is absent",
        ));
    };
    String::from_utf16(value)
        .map_err(|_| native_state_error("JavaScript host string has an unpaired surrogate"))
}
fn encoded(encoding: ScriptTextEncoding, args: &[Value], index: usize) -> Result<Vec<u8>> {
    let value = string(args, index)?;
    let bytes = super::sound_native::encode_name(encoding, &value)?;
    if bytes.contains(&0) || bytes.len() > 4095 {
        return Err(native_state_error(
            "JavaScript resource name contains NUL or exceeds 4095 bytes",
        ));
    }
    Ok(bytes)
}
fn decode(encoding: ScriptTextEncoding, bytes: &[u8]) -> Result<String> {
    Ok(super::sound_native::decode_name(encoding, bytes)?)
}
fn script_error(message: String) -> RuntimeError {
    RuntimeError::Host(format!("JavaScript: {message}"))
}
fn native_state_error(message: &'static str) -> RuntimeError {
    script_error(message.to_owned())
}
