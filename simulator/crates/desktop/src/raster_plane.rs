//! Raster18's single drawable vertex callback.
//! It changes the selected drawable's real vertices before its local matrix;
//! it owns neither a second drawable nor an invented tessellation or depth.
use std::collections::BTreeMap;

use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result, Vm};

use super::{LoggingHost, PlaneShape, error, word};

const V401: &str = "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f";
const YUKI: &str = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976";
const KANMI_PACKED: &str = "c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26";
const KANMI_SSCRIPT_CODE: &str =
    "7a068090a8d37c7a446a74e98448458e3d6ed75d94bb1a00022eaeb07d7a3921";

#[derive(Clone, Copy)]
enum Operation {
    Start,
    Stop,
    Query,
    Configure,
    Parameters,
}

// Ordinary wrappers and object messages are different namespaces.
const DESCRIPTORS: &[(Option<u32>, u32, u32, Operation)] = &[
    (Some(197), 252, 263, Operation::Start),
    (Some(198), 253, 264, Operation::Stop),
    (Some(200), 254, 265, Operation::Query),
    (Some(196), 258, 269, Operation::Configure),
    (Some(201), 259, 270, Operation::Parameters),
];

fn hydro_code(current: bool, code: u32) -> bool {
    if current {
        matches!(code, 263..=265 | 278 | 279)
    } else {
        matches!(code, 252..=254 | 267 | 268)
    }
}

fn version(host: &LoggingHost<'_>) -> Result<bool> {
    match host
        .data_services
        .native_profile
        .map(|profile| profile.pe_sha256)
    {
        Some(V401) => Ok(false),
        Some(YUKI) => Ok(true),
        _ => Err(error(
            "Raster vertex callback needs an identified PE descriptor",
        )),
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RasterVertexTransform {
    axis: usize,
    amplitude: f32,
    wavelength: f32,
    phase_period: f32,
    relative: bool,
    counter: u32,
}

impl RasterVertexTransform {
    pub(crate) fn apply(self, point: &mut [f32; 4]) -> anyhow::Result<()> {
        let other = point[1 - self.axis];
        let tau = std::f32::consts::TAU;
        let angle = if self.relative {
            // +4c is initialized to unsigned1; neither identified task base
            // writes it afterwards. Do not substitute task property or pause.
            let phase = ((self.counter as f32) / self.phase_period).fract() * tau;
            (other * self.wavelength).fract() * tau + phase
        } else {
            // Retain each observed single-precision writeback boundary.
            let remainder = ((self.counter as f32 + other) % self.wavelength) as f32;
            ((remainder * std::f32::consts::PI) + (remainder * std::f32::consts::PI))
                / self.wavelength
        };
        let displacement = (f64::from(angle).sin() as f32) * self.amplitude;
        point[self.axis] += displacement;
        if point.iter().any(|value| !value.is_finite()) {
            anyhow::bail!("Raster callback produced non-finite drawable coordinates");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Raster {
    target: Option<u32>,
    axis: u32,
    active: u32,
    counter: u32,
    last: u32,
    entry_active: Option<u32>,
    parameters: Option<crate::media_wrapper_native::RasterParameters>,
}

#[derive(Clone, Copy, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Binding {
    owner: u32,
    source: bool,
    movie_handle: Option<u32>,
}

#[derive(Clone, Debug, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct RasterStore {
    tasks: BTreeMap<u32, Raster>,
    callbacks: BTreeMap<u32, Binding>,
    constructor_fps: Option<u32>,
}

impl RasterStore {
    pub(super) fn validate_graph(
        &self,
        saved: &cs2_runtime::object_graph::ObjectGraphSnapshot,
        proxies: &BTreeMap<u32, cs2_runtime::object_graph::SceneNodeSnapshot>,
    ) -> anyhow::Result<()> {
        if self.tasks.len() > 65_536 || self.callbacks.len() > 65_536
            || (!self.tasks.is_empty() && self.constructor_fps.is_none())
            || saved.records.iter().any(|(id, record)| record.kind == 18 && !self.tasks.contains_key(id))
            || self.tasks.iter().any(|(id, task)|
                saved.records.get(id).is_none_or(|record| record.kind != 18)
                || task.active > 1 || task.entry_active.is_some_and(|active| active > 1)
                || (task.active != 0 && (task.target.is_none() || task.parameters.is_none())))
        {
            anyhow::bail!("saved Raster owner, parameters or update continuation is invalid");
        }
        if (!self.tasks.is_empty() || !self.callbacks.is_empty())
            && !matches!(saved.pe_sha256.as_str(), V401 | YUKI) {
            anyhow::bail!("saved Raster state needs its identified PE descriptor");
        }
        for (target, binding) in &self.callbacks {
            if binding.movie_handle.is_some() {
                anyhow::bail!("saved movie Raster callback needs the media graph adapter");
            }
            if !self.tasks.contains_key(&binding.owner)
                || saved.nodes.get(target).or_else(|| proxies.get(target))
                    .is_none_or(|node| node.kind != 2)
            {
                anyhow::bail!("saved Raster drawable binding lost its owner or target");
            }
        }
        Ok(())
    }

    pub(super) fn check_graph_boundary(&self) -> anyhow::Result<()> {
        if self.callbacks.values().any(|binding| binding.movie_handle.is_some()) {
            anyhow::bail!("movie Raster callback needs the media graph adapter");
        }
        Ok(())
    }

    pub(super) fn graph_is_empty(&self) -> bool {
        self.tasks.is_empty() && self.callbacks.is_empty() && self.constructor_fps.is_none()
    }

    pub(super) fn owns(&self, id: u32) -> bool {
        self.tasks.contains_key(&id)
    }

    pub(super) fn release(&mut self, id: u32) {
        self.tasks.remove(&id);
        self.callbacks.remove(&id);
        self.callbacks.retain(|_, binding| binding.owner != id);
    }

    pub(super) fn callback(&self, id: u32, source: bool) -> Option<RasterVertexTransform> {
        let binding = self
            .callbacks
            .get(&id)
            .filter(|binding| binding.source == source)?;
        let task = self.tasks.get(&binding.owner)?;
        let parameters = task.parameters?;
        Some(RasterVertexTransform {
            axis: usize::from(task.axis != 1),
            amplitude: f32::from_bits(parameters.mode),
            wavelength: f32::from_bits(parameters.period_bits),
            phase_period: f32::from_bits(parameters.amplitude_bits),
            relative: parameters.relative,
            counter: task.counter,
        })
    }
}

impl super::PlaneServices {
    /// Freeze the real graphics-thread constructor value after startup/setup
    /// initialization, before scripts can edit the mutable platform setting.
    pub(crate) fn bind_raster_constructor_frame_rate(&mut self, fps: u32) -> Result<()> {
        if self
            .raster
            .constructor_fps
            .is_some_and(|previous| previous != fps)
        {
            return Err(error(
                "cannot change an existing Raster constructor frame rate",
            ));
        }
        self.raster.constructor_fps = Some(fps);
        Ok(())
    }
}

pub(super) fn synchronize_movie_callbacks(
    planes: &super::PlaneServices,
    movie: &mut crate::MpegMovieHost,
) {
    let targets: Vec<_> = movie.planes.properties.keys().copied().collect();
    for target in targets {
        let handle = movie.plane_target_handle(target);
        let callback = planes
            .raster
            .callbacks
            .get(&target)
            .filter(|binding| binding.movie_handle.is_some() && binding.movie_handle == handle)
            .and_then(|_| planes.raster.callback(target, false));
        movie.planes.properties.get_mut(&target).unwrap().raster = callback;
    }
}

impl LoggingHost<'_> {
    pub(crate) fn register_raster_task(&mut self, id: u32) -> Result<()> {
        // This packed identity is issued only after matching both producer KCS files.
        if self.data_services.particle_message_pe_sha256 != Some(KANMI_PACKED) {
            version(self)?;
        }
        // The thread ctor's second argument is reported as '%d fps'; manager
        // passes it to root+48 and each child factory inherits parent+48.
        if self.data_services.planes.raster.constructor_fps.is_none() {
            self.ensure_configuration_state().map_err(|cause| {
                self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string()))
            })?;
            let fps = self
                .data_services
                .configuration
                .platform_value("WINDOW/fps")
                .ok_or_else(|| error("Raster constructor frame rate is unavailable"))?;
            self.data_services.planes.raster.constructor_fps = Some(fps);
        }
        let store = &mut self.data_services.planes.raster;
        if store.tasks.len() >= 65_536 && !store.tasks.contains_key(&id) {
            return Err(error("Raster owner limit exceeds65536"));
        }
        store.tasks.entry(id).or_default();
        Ok(())
    }

    fn synchronize_raster_parameters(&mut self, id: u32) -> Result<()> {
        let parameters = self
            .raster_media_parameters(id)
            .ok_or_else(|| error("Raster configured parameters are unavailable"))?;
        self.data_services
            .planes
            .raster
            .tasks
            .get_mut(&id)
            .ok_or_else(|| error("Raster callback owner is unavailable"))?
            .parameters = Some(parameters);
        Ok(())
    }

    pub(crate) fn receive_raster_command(
        &mut self,
        command: TaskCommand,
        buffers: &mut TaskCommandBuffers<'_>,
        output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        let id = command.target as u32;
        let Some((kind, deletion, _)) = self.data_services.tasks.message_task_state(id) else {
            return Ok(None);
        };
        if !matches!(kind, 18 | 25) {
            return Ok(None);
        }
        // Kanmi's Raster reset uses native 67 and the base task delete command.
        if kind == 18
            && command.code == 3
            && self.data_services.particle_message_pe_sha256 == Some(KANMI_PACKED)
            && self.data_services.native_profile.is_none()
        {
            return Ok(None);
        }
        let current = version(self)?;
        if kind == 25 {
            // Both identified Hydro vtables delegate these messages without
            // reading any record or scalar, writing output or making pixels.
            return Ok(
                hydro_code(current, command.code).then_some(TaskCommandOutcome {
                    done: true,
                    propagate: deletion == 0,
                }),
            );
        }
        let Some((_, _, _, operation)) = DESCRIPTORS
            .iter()
            .find(|(_, old, new, _)| command.code == if current { *new } else { *old })
        else {
            return Ok(None);
        };
        let consumed = Some(TaskCommandOutcome {
            done: true,
            propagate: false,
        });
        if deletion != 0 {
            return Ok(consumed);
        }
        self.apply_raster_operation(id, *operation, buffers.data1.as_deref(), output)?;
        Ok(consumed)
    }

    fn apply_raster_operation(
        &mut self,
        id: u32,
        operation: Operation,
        bytes: Option<&[u8]>,
        output: &mut u32,
    ) -> Result<()> {
        self.register_raster_task(id)?;
        match operation {
            Operation::Configure => {
                let bytes = bytes
                    .filter(|bytes| bytes.len() >= 20)
                    .ok_or_else(|| error("Raster primary configuration requires20 bytes"))?;
                let target = word(bytes, 0)?;
                let mode = word(bytes, 4)?;
                let fps = self
                    .data_services
                    .planes
                    .raster
                    .constructor_fps
                    .ok_or_else(|| error("Raster constructor frame rate is unavailable"))?;
                self.bind_raster_media_context(id, mode & 0x8000_0000 != 0, fps)?;
                self.update_raster_media_parameters(id, Some(bytes))?;
                let task = self.data_services.planes.raster.tasks.get_mut(&id).unwrap();
                task.target = Some(target);
                task.axis = mode & 0x7fff_ffff;
                self.synchronize_raster_parameters(id)?;
            }
            Operation::Parameters => {
                // D owns the exact raw float words and relative conversion.
                // Refresh here, rather than only during task body: root script
                // parameter changes must affect this frame's real geometry.
                self.update_raster_media_parameters(id, bytes)?;
                self.synchronize_raster_parameters(id)?;
            }
            Operation::Start => {
                let target = self.data_services.planes.raster.tasks[&id]
                    .target
                    .ok_or_else(|| error("Raster start precedes its primary configuration"))?;
                let now = self
                    .data_services
                    .tasks
                    .task_clock(id)
                    .ok_or_else(|| error("Raster owner clock is unavailable"))?
                    .elapsed();
                if let Some(node) = self
                    .scene_nodes
                    .get(&target)
                    .filter(|node| node.initialized)
                {
                    if node.kind != 2 {
                        return Err(error(
                            "Raster target drawable needs its concrete vertex callback bridge",
                        ));
                    }
                    let movie_handle = self
                        .movie_host
                        .as_ref()
                        .and_then(|movie| movie.plane_target_handle(target));
                    let source = movie_handle.is_none()
                        && self
                            .data_services
                            .planes
                            .planes
                            .get(&target)
                            .is_some_and(|plane| plane.shape == PlaneShape::Canvas);
                    if self.data_services.planes.raster.callbacks.len() >= 65_536
                        && !self
                            .data_services
                            .planes
                            .raster
                            .callbacks
                            .contains_key(&target)
                    {
                        return Err(error("Raster drawable callback limit exceeds65536"));
                    }
                    self.data_services.planes.raster.callbacks.insert(
                        target,
                        Binding {
                            owner: id,
                            source,
                            movie_handle,
                        },
                    );
                }
                // The original sets active even when drawable lookup returns
                // absent. A later drawable creation does not install retroactively.
                let task = self.data_services.planes.raster.tasks.get_mut(&id).unwrap();
                task.active = 1;
                task.counter = 0;
                task.last = now;
            }
            Operation::Stop => {
                let task = self.data_services.planes.raster.tasks.get_mut(&id).unwrap();
                task.active = 0;
                if let Some(target) = task.target {
                    self.data_services.planes.raster.callbacks.remove(&target);
                }
            }
            Operation::Query => {
                *output = self.data_services.planes.raster.tasks[&id].active;
            }
        }
        self.synchronize_raster_movie_vertices();
        Ok(())
    }

    pub(crate) fn advance_raster_task(&mut self, id: u32) -> Result<()> {
        let Some(task) = self.data_services.planes.raster.tasks.get(&id) else {
            return Ok(());
        };
        // Both vfuncs snapshot active before calling their child-first base.
        // A child/queued start or stop must not replace that saved gate.
        if task.entry_active.unwrap_or(task.active) == 0 {
            self.data_services
                .planes
                .raster
                .tasks
                .get_mut(&id)
                .unwrap()
                .entry_active = None;
            return Ok(());
        }
        let parameters = task
            .parameters
            .ok_or_else(|| error("active Raster has no configured float parameters"))?;
        let now = self
            .data_services
            .tasks
            .task_clock(id)
            .ok_or_else(|| error("active Raster has no task clock"))?
            .elapsed();
        let mode = self.system_config_blocks[2]
            .get(..4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .unwrap_or(0);
        let speed = match mode {
            1 => 1.5,
            2 => 2.,
            3 => 3.,
            4 => 4.,
            _ => 1.,
        };
        let increment = float_word(now.wrapping_sub(task.last) as f32 * speed)?;
        let modulus = float_word(f32::from_bits(parameters.period_bits))?;
        let task = self.data_services.planes.raster.tasks.get_mut(&id).unwrap();
        task.counter = task.counter.wrapping_add(increment);
        if task.counter >= modulus {
            task.counter = task.counter.wrapping_sub(modulus);
        }
        task.last = now;
        task.entry_active = None;
        self.synchronize_raster_movie_vertices();
        Ok(())
    }

    fn synchronize_raster_movie_vertices(&mut self) {
        if let Some(movie) = self.movie_host.as_deref_mut() {
            synchronize_movie_callbacks(&self.data_services.planes, movie);
        }
    }

    pub(crate) fn begin_raster_task_update(&mut self, id: u32) -> Result<()> {
        self.register_raster_task(id)?;
        let task = self.data_services.planes.raster.tasks.get_mut(&id).unwrap();
        task.entry_active = Some(task.active);
        Ok(())
    }

    pub(super) fn raster_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<Option<NativeReturn>> {
        if !matches!(index, 196..=198 | 200 | 201) {
            return Ok(None);
        }
        let id = word(args, 0)?;
        let Some(kind @ (18 | 25)) = self.scene_nodes.get(&id).map(|node| node.kind) else {
            return Ok(None);
        };
        let required = match index {
            196 => 8,
            201 => 12,
            _ => 4,
        };
        if args.len() < required {
            return Err(error("Raster/Hydro ordinary argument record is truncated"));
        }
        // The producer proves these operations through their post-call PCs.
        // Keep the packed PE out of the raw object-message descriptor table.
        if kind == 18
            && self.data_services.particle_message_pe_sha256 == Some(KANMI_PACKED)
            && vm.program_code_sha256() == Some(KANMI_SSCRIPT_CODE)
            && let Some(operation) = match (index, vm.instruction_pointer) {
                (196, 0x96c1b) => Some(Operation::Configure),
                (197, 0x96d2b) => Some(Operation::Start),
                (200, 0x96db3) => Some(Operation::Query),
                _ => None,
            }
        {
            return self.kanmi_raster_native(id, operation, args, vm).map(Some);
        }
        let current = version(self)?;
        if kind == 25 {
            if index == 200 {
                return Err(error(
                    "identified Hydro base receiver does not define a scalar query result",
                ));
            }
            if index == 196 {
                word(args, 4)?;
            }
            if index == 201 {
                word(args, 8)?;
                if word(args, 4)? != 0 {
                    return Ok(Some(NativeReturn::Continue));
                }
            }
            let code = match (index, current) {
                (196, false) => 267,
                (196, true) => 278,
                (197, false) => 252,
                (197, true) => 263,
                (198, false) => 253,
                (198, true) => 264,
                (201, false) => 268,
                (201, true) => 279,
                _ => unreachable!(),
            };
            let command = TaskCommand {
                target: id as i32,
                sender: self.current_task_id as i32,
                code,
                data: [0; 3],
            };
            let outcome = self.route_task_command_buffers(
                command,
                &mut crate::native_tasks::OwnedTaskBuffers::default(),
                &mut 0,
                4096,
            )?;
            return Ok(Some(if outcome.is_some_and(|outcome| !outcome.done) {
                NativeReturn::Retry
            } else {
                NativeReturn::Continue
            }));
        }
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_some_and(|(_, deletion, _)| deletion != 0)
        {
            return if index == 200 {
                Err(error(
                    "Raster query recipient is deleted and does not write a result",
                ))
            } else {
                Ok(Some(NativeReturn::Continue))
            };
        }
        let (_, old, new, _) = DESCRIPTORS
            .iter()
            .find(|(slot, _, _, _)| *slot == Some(index))
            .unwrap();
        if index == 201 && word(args, 4)? != 0 {
            return Ok(Some(NativeReturn::Continue));
        }
        let mut bytes = if matches!(index, 196 | 201) {
            Some(vm.read(word(args, if index == 196 { 4 } else { 8 })?, 20)?)
        } else {
            None
        };
        let mut output = 0;
        self.receive_raster_command(
            TaskCommand {
                target: id as i32,
                sender: self.current_task_id as i32,
                code: if current { *new } else { *old },
                data: [0, 1, 0],
            },
            &mut TaskCommandBuffers {
                data0: None,
                data1: bytes.as_deref_mut(),
            },
            &mut output,
        )?
        .ok_or_else(|| error("Raster ordinary wrapper has no concrete recipient"))?;
        Ok(Some(if index == 200 {
            NativeReturn::Value(output)
        } else {
            NativeReturn::Continue
        }))
    }

    fn kanmi_raster_native(
        &mut self, id: u32, operation: Operation, args: &[u8], vm: &Vm,
    ) -> Result<NativeReturn> {
        if !self.data_services.tasks.message_task_state(id)
            .is_some_and(|(kind, deletion, _)| kind == 18 && deletion == 0)
        {
            return Err(error("Kanmi Raster requires its live kind18 recipient"));
        }
        let mut output = 0;
        let record = if matches!(operation, Operation::Configure) {
            Some(vm.read(word(args, 4)?, 20)?)
        } else {
            None
        };
        self.apply_raster_operation(id, operation, record.as_deref(), &mut output)?;
        let raster = &self.data_services.planes.raster;
        let detail = match operation {
            Operation::Configure => {
                let bytes = record.as_deref().unwrap();
                format!(
                    "configure owner={id} target={} mode={:#010x} amplitude={:#010x} wavelength={:#010x} phase_period={:#010x}",
                    word(bytes, 0)?, word(bytes, 4)?, word(bytes, 8)?,
                    word(bytes, 12)?, word(bytes, 16)?,
                )
            }
            Operation::Start => {
                let target = raster.tasks[&id].target.unwrap();
                let bound = raster.callbacks.get(&target).is_some_and(|binding| binding.owner == id);
                format!("start owner={id} target={target} bound={bound}")
            }
            Operation::Query => return Ok(NativeReturn::Value(output)),
            _ => unreachable!("Kanmi Raster callsite operation"),
        };
        self.events.push(crate::LogicEvent {
            frame: self.frame,
            kind: "kanmi-raster".into(),
            detail,
        });
        Ok(NativeReturn::Continue)
    }

    pub(crate) fn dispatch_raster_message(
        &mut self,
        target: u32,
        packet: [u32; 4],
        queued: bool,
        output: &mut u32,
        vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        let current = version(self)?;
        if self
            .data_services
            .tasks
            .message_task_state(target)
            .is_some_and(|state| state.0 == 25)
        {
            if !hydro_code(current, packet[1]) {
                return Ok(None);
            }
            let command = TaskCommand {
                target: target as i32,
                sender: packet[0] as i32,
                code: packet[1],
                data: [packet[2], packet[3], 0],
            };
            let mut buffers = crate::native_tasks::OwnedTaskBuffers::default();
            if queued {
                return self
                    .queue_task_command_buffers(command, &buffers, output)
                    .map(|found| {
                        found.then_some(TaskCommandOutcome {
                            done: true,
                            propagate: false,
                        })
                    });
            }
            return self.route_task_command_buffers(command, &mut buffers, output, 4096);
        }
        let Some((_, _, _, operation)) = DESCRIPTORS
            .iter()
            .find(|(_, old, new, _)| packet[1] == if current { *new } else { *old })
        else {
            return Ok(None);
        };
        let command = TaskCommand {
            target: target as i32,
            sender: packet[0] as i32,
            code: packet[1],
            data: [packet[2], packet[3], 0],
        };
        let mut buffers = if let Some(buffers) = self.pending_owned_task_buffers(command) {
            buffers
        } else {
            let bytes = if matches!(operation, Operation::Configure | Operation::Parameters) {
                Some(
                    vm.ok_or_else(|| {
                        error("raw Raster configuration needs its VM or owned record")
                    })?
                    .read(packet[3], 20)?,
                )
            } else {
                None
            };
            crate::native_tasks::OwnedTaskBuffers::new(None, bytes)?
        };
        if queued {
            return self
                .queue_task_command_buffers(command, &buffers, output)
                .map(|found| {
                    found.then_some(TaskCommandOutcome {
                        done: true,
                        propagate: false,
                    })
                });
        }
        self.route_task_command_buffers(command, &mut buffers, output, 4096)
    }
}

fn float_word(value: f32) -> Result<u32> {
    let value = f64::from(value).trunc();
    // Both PE helpers return signed64; Raster retains only EAX. Preserve the
    // low DWORD rather than saturating a float directly to u32.
    if !value.is_finite() || !(-9223372036854775808. ..9223372036854775808.).contains(&value) {
        return Err(error(
            "Raster counter conversion is outside the recovered DWORD range",
        ));
    }
    Ok(value as i64 as u32)
}
