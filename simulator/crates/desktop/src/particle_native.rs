//! Type16 particle messages, independently recovered from the fixed PE pair.
//! The 0x290-byte record owns its parameters and strings; no caller VM address
//! survives dispatch. External draw is a separate sink, never a window overlay.
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_runtime::resource_router::ResourceMode;
use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, ParticleObjectState, expand_kcs_format, native_state_error,
    particle_vm, read_u32, trim_nul};

#[path = "particle_resource.rs"]
mod resource;

// Each tuple binds independently decoded registered wrappers, not a version
// number range. Evidence: 20261116_D_particle669_profiles.json.
const PROFILES: &[(&str, u32, bool)] = &[
    ("0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f", 309, false),
    ("1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5", 320, true),
    ("3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18", 311, false),
    ("474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1", 304, false),
    ("4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94", 314, false),
    ("63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976", 320, true),
    // Happiness2 plain companion: table5e40e8,27119e940/69319eb50,
    // 1068193c00/1069194150. No identity is inferred for the packed EXEs.
    ("5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c", 320, true),
    ("6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de", 311, false),
    // Tunaban native 271 pushes 0x109; the control wrappers use 0x10a..0x110.
    (super::css_model::SHA, 265, false),
    (super::css_model::UTATE_SHA, 277, false),
    ("f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e", 285, false),
    ("9f3a5ad0ed22074420685f1e2a3bc92436d68289c7bac854d5f7d7331dc3ef67", 285, false),
    ("012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f", 287, false),
    ("985096b6064e35c7f48c080aa9008d67442fc0df7e33fec7210fe13fefc1dc4f", 265, false),
    ("b70a802a74c49a5d6f429114f81d878f83e25a4acbcb2778c6f4d5f68528de7b", 292, false),
    ("f36f63798eb27b1eb6742c2fe7c3a13f172a249ee6623a12fc79fd8c3dee3cfe", 297, false),
    ("a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3", 302, false),
    ("7bd9f6dcc7d62868d4ad9cc68d5580452e71bd907540ec4f993d960d051d2c45", 304, false),
    ("a540b1dec1185a63cc68c4d283b6bc99bcce8de219e1bead3b8d113577de4027", 306, false),
    ("cfb7f74ac3674962a8d982fe7e0ee53b3306fb47362653db794ad6dcb0fcb35d", 314, false),
    ("ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5", 292, false),
    ("c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c", 297, false),
    ("c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3", 306, false),
    ("c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78", 304, false),
    ("c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc", 314, false),
];
const RECORD_BYTES: usize = 0x290;
const PARAMETERS: usize = 0x10c;
const KANMI_PACKED: &str = "c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26";
const KANMI_STARTUP: &str = "7e11ce11146b4eb4fa27679f6c2547055f08eeff24c8ab122d2d0dfc959128a4";
const KANMI_SSCRIPT: &str = "17de548390b8c2b15a4f413418bbf1b408d6dee6ab830879f09cf305f5c128fd";

fn kanmi_particle_producer_matches(runtime: &str, startup: &str, sscript: &str) -> bool {
    runtime == KANMI_PACKED && startup == KANMI_STARTUP && sscript == KANMI_SSCRIPT
}

fn kanmi_runtime_pe_sha256(producer: Option<&str>, startup: Option<&str>) -> Option<&'static str> {
    (producer == Some(KANMI_PACKED) && startup == Some("kcs.int/main.kcs"))
        .then_some(KANMI_PACKED)
}

fn particle_control_offset(index: u32) -> Option<u32> {
    match index {
        272 => Some(1),
        273 => Some(2),
        274 => Some(4),
        275 => Some(5),
        426 => Some(6),
        427 => Some(7),
        _ => None,
    }
}

fn kanmi_particle_operation(
    producer: Option<&str>, index: u32, target: Option<(u32, u32, u32)>,
) -> Option<u32> {
    if producer != Some(KANMI_PACKED) || !matches!(target, Some((16, 0, _))) {
        return None;
    }
    match index {
        271 | 693 => Some(0),
        _ => particle_control_offset(index),
    }
}

fn command_base(host: &LoggingHost<'_>) -> Result<u32> {
    if host.data_services.particle_external.legacy_kc_view() { return Ok(271); }
    let digest = host.data_services.particle_message_pe_sha256
        .or_else(|| host.data_services.native_profile.map(|profile| profile.pe_sha256));
    PROFILES.iter().find(|profile| Some(profile.0) == digest).map(|profile| profile.1)
        .ok_or_else(|| native_state_error("particle message numbering requires an identified PE profile"))
}

fn camera_commands(host: &LoggingHost<'_>) -> bool {
    let digest = host.data_services.particle_message_pe_sha256
        .or_else(|| host.data_services.native_profile.map(|profile| profile.pe_sha256));
    PROFILES.iter().any(|profile| profile.2
        && digest == Some(profile.0))
}

pub(super) fn identified_pe_sha256(sha: &str) -> Option<&'static str> {
    PROFILES.iter().find(|profile| profile.0 == sha).map(|profile| profile.0)
}

pub(super) fn projection(group: u32, host: &super::MpegMovieHost) -> particle_vm::ParticleProjection {
    projection_in(group, &host.planes.projection, &host.planes.cameras)
}

/// Both the final window and568's retained view consume the same group
/// mapping. Capture must use its own saved view, without advancing a VM or
/// substituting the window's subsequently changed camera.
pub(super) fn projection_in(
    group: u32, projection: &super::MoviePlaneProjection,
    cameras: &std::collections::BTreeMap<u32, super::movie_plane::PlaneCamera>,
) -> particle_vm::ParticleProjection {
    let mut view = particle_vm::ParticleProjection {
        canvas_matrix: projection.matrix,
        translation: projection.translation,
        ..Default::default()
    };
    if let Some(camera) = cameras.get(&group).filter(|_| group != 0) {
        view.coordinate_scale = camera.scale;
        view.coordinate_offset = camera.offset;
        view.coordinate_matrix = camera.matrix;
    }
    view
}

/// Submit one ordinary kind16 at its place in the existing Canvas owner
/// order. Window and568 use this same committed snapshot; neither path runs
/// an update or borrows another owner's texture. External draws keep their
/// existing explicit sink.
pub(super) fn draw_ordinary_task(
    target: u32, canvas: &mut cs2_renderer::Canvas,
    particles: &std::collections::BTreeMap<u32, ParticleObjectState>,
    projection: &super::MoviePlaneProjection,
    cameras: &std::collections::BTreeMap<u32, super::movie_plane::PlaneCamera>,
    lower: f32, upper: f32,
) -> anyhow::Result<bool> {
    let Some(particle) = particles.get(&target).filter(|particle| particle.external_draw == 0)
        else { return Ok(false); };
    if particle.runner.as_ref().is_some_and(|runner| !runner.ordinary_draw_registered()) {
        return Ok(true);
    }
    if let Some(snapshot) = &particle.rendered {
        let view = projection_in(particle.coordinate_group, projection, cameras);
        super::particle_render::draw_range_with_projection(
            canvas, snapshot, particle.texture.as_ref(), &view, lower, upper)?;
    }
    Ok(true)
}

fn record_string(bytes: &[u8]) -> Result<&[u8]> {
    let end = bytes.iter().position(|&byte| byte == 0)
        .ok_or_else(|| native_state_error("particle record string lacks its bounded terminator"))?;
    Ok(&bytes[..end])
}

fn put_string(target: &mut [u8], source: &[u8]) -> Result<()> {
    if source.len() >= target.len() || source.contains(&0) {
        return Err(native_state_error("particle record string exceeds its original capacity"));
    }
    target[..source.len()].copy_from_slice(source);
    target[source.len()] = 0;
    Ok(())
}

impl LoggingHost<'_> {
    /// FES owns already-expanded names, not addresses in an ordinary VM.
    /// Its 0x8c bank record becomes the same owned 0x290 load record as271/
    /// 693 (Yuki1b6d20..1b6de3). Parameters are submitted separately by325.
    pub(super) fn prepare_fes_particle_load(
        &self, plugin: u32, program: &[u8], image: &[u8], count: u32, depth: i32,
    ) -> Result<Vec<u8>> {
        let mut record = vec![0; RECORD_BYTES];
        record[..4].copy_from_slice(&plugin.to_le_bytes());
        let program = if plugin != 0 {
            self.complete_particle_plugin_name(program)?
                .ok_or_else(|| native_state_error("FES particle plugin file category is unavailable"))?
        } else { self.complete_particle_resource_name(b"ptcl", program)? };
        let image = self.complete_particle_resource_name(b"image", image)?;
        put_string(&mut record[4..0x84], &program)?;
        put_string(&mut record[0x84..0x104], &image)?;
        record[0x104..0x108].copy_from_slice(&count.to_le_bytes());
        record[0x108..0x10c].copy_from_slice(&(depth as f32).to_bits().to_le_bytes());
        Ok(record)
    }

    fn complete_particle_resource_name(&self, category: &[u8], bytes: &[u8]) -> Result<Vec<u8>> {
        let name = self.text_encoding.decode(trim_nul(bytes));
        if name.is_empty() { return Ok(Vec::new()); }
        let names = self.resources.category_names(category, &name)
            .map_err(|_| native_state_error("particle startup resource category resolution failed"))?;
        let name = names.first().ok_or_else(|| native_state_error("particle category resolved no name"))?;
        let encoding = match self.text_encoding {
            super::ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS,
            super::ScriptTextEncoding::Gbk => encoding_rs::GBK,
            super::ScriptTextEncoding::Utf8 => return Ok(name.as_bytes().to_vec()),
        };
        let (bytes, _, errors) = encoding.encode(name);
        if errors { return Err(native_state_error("particle completed name is not representable in script encoding")); }
        Ok(bytes.into_owned())
    }

    /// Semantic FES-to-kind16 boundary. These are normalized320..327
    /// operations, NOT guessed ordinary message numbers for an older PE.
    /// Both producers enter the single receiver body below, with owned bytes;
    /// no native pointer or second VM/renderer is introduced.
    pub(super) fn submit_fes_particle_command(
        &mut self, target: u32, command: u32, value: u32, bytes: Option<&[u8]>,
    ) -> Result<u32> {
        if !matches!(self.data_services.tasks.message_task_state(target), Some((16, 0, _))) {
            return Err(native_state_error("FES particle submission has no live kind16 lower task"));
        }
        let offset = command.checked_sub(320).filter(|offset| *offset < 8)
            .ok_or_else(|| native_state_error("FES particle operation is outside320..327"))?;
        self.submit_particle_operation(target, offset, value, bytes)
    }

    fn submit_particle_operation(
        &mut self, target: u32, offset: u32, value: u32, bytes: Option<&[u8]>,
    ) -> Result<u32> {
        self.retain_particle_controls(target);
        let mut output = 0;
        self.receive_particle_body(target, offset, value, bytes, &mut output)?;
        Ok(output)
    }

    fn kanmi_particle_native_operation(&self, index: u32, target: u32) -> Option<u32> {
        if self.data_services.particle_message_pe_sha256 != Some(KANMI_PACKED)
            || self.data_services.particle_external.legacy_kc_view()
            || self.data_services.native_profile.is_some()
            || !self.task_is_reachable(target)
        {
            return None;
        }
        kanmi_particle_operation(self.data_services.particle_message_pe_sha256, index,
            self.data_services.tasks.message_task_state(target))
    }

    fn retain_particle_controls(&mut self, target: u32) {
        self.particles.entry(target).or_insert_with(||
            ParticleObjectState::new(String::new(), String::new(), 0, [0; 128]));
    }

    pub(super) fn release_particle_program(&mut self, target: u32) {
        self.data_services.particle_external.release(target);
        if let Some(state) = self.particles.get_mut(&target) {
            state.runner = None;
            state.plugin = None;
            state.texture = None;
            state.rendered = None;
            state.points.clear();
            state.enabled = false;
            state.elapsed = 0;
            state.external_extent_bound = false;
        }
    }

    /// Runs at type16's real post-children task body. A yielded script retains
    /// its grid, time and instruction cursor; other tasks cannot advance it by
    /// invoking the final scene compositor a second time.
    pub(crate) fn advance_particle_task(&mut self, target: u32, now: u32, budget: usize) -> Result<bool> {
        let Some(particle) = self.particles.get_mut(&target) else { return Ok(true); };
        if let Some(plugin) = particle.plugin.as_mut() {
            if particle.external_draw != 0 && !particle.external_extent_bound {
                return Err(native_state_error("external particle plugin has no real extent binding"));
            }
            plugin.refresh_source(particle.transform_source as i32,
                self.scene_nodes.get(&particle.transform_source).map(|node|
                    particle_vm::ParticleSource { color: node.color,
                        position_2d: [node.position[0], node.position[1]], position_3d: node.position }));
            plugin.update(now, &particle.parameters, self.script_random);
            let snapshot = plugin.snapshot(particle.opacity);
            particle.enabled = plugin.active();
            particle.elapsed = snapshot.elapsed;
            particle.points = if particle.external_draw == 0 { snapshot_points(&snapshot)? } else { Vec::new() };
            if particle.external_draw != 0 && self.data_services.particle_external.legacy_kc_view() {
                self.data_services.particle_external.submit(target, &snapshot)?;
            }
            particle.rendered = Some(snapshot);
            return Ok(true);
        }
        // The task exists before its first load and after command323. Retained
        // controls in that interval are not an executable or a successful load.
        let Some(runner) = particle.runner.as_mut() else { return Ok(true); };
        if particle.external_draw != 0 && !particle.external_extent_bound {
            return Err(native_state_error("external particle has no real texture extent binding; guessed geometry is forbidden"));
        }
        runner.bind_source(particle.transform_source as i32);
        runner.refresh_source(self.scene_nodes.get(&particle.transform_source).map(|node|
            particle_vm::ParticleSource { color: node.color,
                position_2d: [node.position[0], node.position[1]], position_3d: node.position }));
        if !runner.has_continuation() { runner.set_parameters(particle.parameters)?; }
        // Yuki14e424..14e45b uses the original inherited update argument and
        // integer multiplier (+4c, constructor default1). Base134890 adjusts
        // the task-local clock but forwards the original tick to children;
        // this subclass likewise retains its input in edi across that call.
        // Do not substitute clock.elapsed() or a wall-clock conversion here.
        let progress = runner.update(now, 1, budget, self.script_random)?;
        particle.enabled = runner.active();
        particle.elapsed = runner.elapsed();
        if progress != particle_vm::ParticleUpdate::Yield {
            particle.parameters = runner.parameters()?;
            let snapshot = runner.render_snapshot();
            particle.points = if particle.external_draw == 0 {
                snapshot.projected_quads(&particle_vm::ParticleProjection::default())?
                    .iter().map(|quad| {
                        let vertex = quad.vertices[0];
                        super::ParticlePoint::new(super::Point::new(vertex.position[0] as i32, vertex.position[1] as i32),
                            super::Color::rgba((vertex.color >> 16) as u8, (vertex.color >> 8) as u8,
                                vertex.color as u8, (vertex.color >> 24) as u8))
                    }).collect()
            } else { Vec::new() };
            if progress == particle_vm::ParticleUpdate::Complete && particle.external_draw != 0
                && self.data_services.particle_external.legacy_kc_view() {
                self.data_services.particle_external.submit(target, &snapshot)?;
            }
            particle.rendered = Some(snapshot);
        }
        for bytes in runner.drain_debug_messages() {
            self.events.push(super::LogicEvent { frame: self.frame, kind: "particle-debug".into(),
                detail: self.text_encoding.decode(&bytes) });
        }
        Ok(progress != particle_vm::ParticleUpdate::Yield)
    }

    pub(super) fn load_particle_native(
        &mut self, index: u32, arguments: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        if arguments.len() != 24 {
            return Err(native_state_error("particle load requires exactly24 argument bytes"));
        }
        let target = read_u32(arguments, 0)?;
        let operation = self.kanmi_particle_native_operation(index, target);
        let command = if operation.is_none() {
            Some(TaskCommand { target: target as i32,
                sender: self.current_task_id as i32, code: command_base(self)?, data: [0, 1, 0] })
        } else { None };
        if let Some(command) = command && let Some(mut buffers) = self.pending_owned_task_buffers(command) {
            let result = self.route_task_command_buffers(command, &mut buffers, &mut 1, 4096)?;
            return Ok(if result.is_some_and(|result| !result.done) { NativeReturn::Retry }
                else { NativeReturn::Value(1) });
        }
        let expand_name = |address| -> Result<Vec<u8>> {
            expand_kcs_format(vm, &vm.read_string(address)?, 0)
        };
        let Ok(program) = expand_name(read_u32(arguments, 4)?) else { return Ok(NativeReturn::Value(0)); };
        let Ok(image) = expand_name(read_u32(arguments, 8)?) else { return Ok(NativeReturn::Value(0)); };
        // 669: target,program,image,count,parameters,configuration.
        // 271/693: target,program,image,count,depth,parameters.
        let mut record = vec![0; RECORD_BYTES];
        record[..4].copy_from_slice(&u32::from(index == 693).to_le_bytes());
        let program = if index == 693 {
            let Some(path) = self.complete_particle_plugin_name(&program)? else {
                return Ok(NativeReturn::Value(0));
            };
            path
        } else { self.complete_particle_resource_name(b"ptcl", &program)? };
        put_string(&mut record[4..0x84], &program)?;
        put_string(&mut record[0x84..0x104], &self.complete_particle_resource_name(b"image", &image)?)?;
        record[0x104..0x108].copy_from_slice(&arguments[12..16]);
        let parameter_address = read_u32(arguments, if index == 669 { 16 } else { 20 })?;
        record[PARAMETERS..PARAMETERS + 128].copy_from_slice(&vm.read(parameter_address, 128)?);
        if index == 669 {
            record[0x18c..0x190].copy_from_slice(&1u32.to_le_bytes());
            // The final format is optional in both wrappers. A failed optional
            // expansion leaves the zeroed name; it does not undo the load.
            if let Ok(format) = vm.read_string(read_u32(arguments, 20)?) {
                if let Ok(config) = expand_kcs_format(vm, &format, 0) {
                    put_string(&mut record[0x190..], trim_nul(&config))?;
                }
            }
        } else {
            record[0x108..0x10c].copy_from_slice(&arguments[16..20]);
        }
        // target0 remains the real root, never substituted with the sender.
        if let Some(command) = command {
            let mut buffers = super::native_tasks::OwnedTaskBuffers::new(None, Some(record))?;
            if self.route_task_command_buffers(command, &mut buffers, &mut 1, 4096)?
                .is_some_and(|outcome| !outcome.done)
            { return Ok(NativeReturn::Retry); }
        } else {
            self.submit_particle_operation(target, operation.unwrap(), 1, Some(&record))?;
        }
        // The wrapper reports its own successful argument preparation; it does
        // not substitute the receiver's untouched output as a load result.
        Ok(NativeReturn::Value(1))
    }

    pub(super) fn control_particle_native(
        &mut self, index: u32, arguments: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let required = if matches!(index, 272..=274) { 4 } else { 8 };
        if arguments.len() != required {
            return Err(native_state_error("particle control argument byte count is invalid"));
        }
        let offset = particle_control_offset(index).expect("particle control native index");
        let value = if required == 8 { read_u32(arguments, 4)? } else { 0 };
        let target = read_u32(arguments, 0)?;
        if let Some(operation) = self.kanmi_particle_native_operation(index, target) {
            let parameters = if index == 275 { Some(vm.read(value, 128)?) } else { None };
            let output = self.submit_particle_operation(target, operation, value, parameters.as_deref())?;
            return Ok(if index == 274 { NativeReturn::Value(output) } else { NativeReturn::Continue });
        }
        let command = TaskCommand { target: target as i32,
            sender: self.current_task_id as i32, code: command_base(self)? + offset,
            data: [0, value, 0] };
        if let Some(mut buffers) = self.pending_owned_task_buffers(command) {
            let mut output = 0;
            let result = self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?;
            return Ok(if result.is_some_and(|result| !result.done) { NativeReturn::Retry }
                else if index == 274 { NativeReturn::Value(output) } else { NativeReturn::Continue });
        }
        let parameters = if index == 275 { Some(vm.read(value, 128)?) } else { None };
        let mut output = 0;
        let mut buffers = super::native_tasks::OwnedTaskBuffers::new(None, parameters)?;
        let outcome = self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?;
        if outcome.is_some_and(|outcome| !outcome.done) { return Ok(NativeReturn::Retry); }
        Ok(if index == 274 { NativeReturn::Value(output) } else { NativeReturn::Continue })
    }

    pub(crate) fn receive_particle_command(
        &mut self, command: TaskCommand, buffers: &mut TaskCommandBuffers<'_>, output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        let target = command.target as u32;
        let Some((16, deletion, _)) = self.data_services.tasks.message_task_state(target) else {
            return Ok(None);
        };
        if camera_commands(self) && matches!(command.code, 591 | 592) {
            if deletion == 0 {
                let group = command.data[1];
                let valid = command.code == 591 && self.scene_nodes.get(&group)
                    .is_some_and(|node| node.kind == 65)
                    && !self.data_services.tasks.message_task_state(group)
                        .is_some_and(|(_, deleted, _)| deleted != 0);
                if valid {
                    self.movie_host.as_deref_mut()
                        .ok_or_else(|| native_state_error("particle coordinate group has no graphics service"))?
                        .planes.cameras.entry(group).or_default();
                }
                self.retain_particle_controls(target);
                self.apply_particle_command(target, command.code, Some(if valid { group } else { 0 }))?;
            }
            return Ok(Some(TaskCommandOutcome { done: true, propagate: false }));
        }
        if !(271..=327).contains(&command.code) { return Ok(None); }
        let base = command_base(self)?;
        let Some(offset) = command.code.checked_sub(base).filter(|offset| *offset < 8) else {
            return Ok(None);
        };
        let consumed = TaskCommandOutcome { done: true, propagate: false };
        if deletion != 0 { return Ok(Some(consumed)); }
        self.retain_particle_controls(target);
        self.receive_particle_body(target, offset, command.data[1], buffers.data1.as_deref(), output)?;
        Ok(Some(consumed))
    }

    fn receive_particle_body(
        &mut self, target: u32, offset: u32, value: u32, bytes: Option<&[u8]>, output: &mut u32,
    ) -> Result<()> {
        match offset {
            0 => if let Some(bytes) = bytes {
                if self.data_services.particle_external.legacy_kc_view() && bytes.len() == RECORD_BYTES - 4 {
                    // Typed senders may also own the original old record.
                    let mut record = vec![0; 4]; record.extend_from_slice(bytes);
                    self.load_particle_record(target, &record)?;
                } else { self.load_particle_record(target, bytes)?; }
            },
            1 => { self.apply_particle_command(target, 321, None)?; }
            2 => { self.apply_particle_command(target, 322, None)?; }
            3 => { self.apply_particle_command(target, 323, None)?; }
            4 => { *output = self.apply_particle_command(target, 324, None)?.unwrap_or(0); }
            5 => {
                let bytes = bytes
                    .and_then(|bytes| bytes.get(..128))
                    .ok_or_else(|| native_state_error("particle parameter command needs128 owned bytes"))?;
                self.apply_particle_parameters(target, bytes.try_into().unwrap())?;
            }
            6 => { self.apply_particle_command(target, 326, Some(value))?; }
            7 => { self.apply_particle_command(target, 327, Some(value))?; }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn load_particle_record(&mut self, target: u32, bytes: &[u8]) -> Result<()> {
        let record = bytes.get(..RECORD_BYTES)
            .ok_or_else(|| native_state_error("particle load record needs0x290 bytes"))?;
        // The real helper destroys the prior VM, texture and mesh BEFORE I/O.
        let (opacity, source, group, was_active) = self.particles.get(&target)
            .map(|state| (state.opacity, state.transform_source, state.coordinate_group,
                state.enabled))
            .unwrap_or((256, u32::MAX, 0, false));
        self.release_particle_program(target);
        let program_name = self.text_encoding.decode(record_string(&record[4..0x84])?);
        let image_name = self.text_encoding.decode(record_string(&record[0x84..0x104])?);
        // Raw271-era records need not initialize the unused configuration tail.
        // Bound the copied field without scanning beyond the original record.
        let config = trim_nul(&record[0x190..]).to_vec();
        let external_draw = read_u32(record, 0x18c)?;
        let plugin = read_u32(record, 0)? != 0;
        // The PPI loader is a plain LoadLibraryA filename, not the resource
        // service's mode1 (which still searches update archives first). Read
        // bytes for the independent SHA model; never load or execute the DLL.
        let program_bytes = if plugin {
            self.resources.resolve_plain_file(&program_name)
                .and_then(|candidate| candidate.map(|candidate|
                    self.resources.read_candidate(candidate)).transpose())
        } else { self.resources.read_mode(&program_name, ResourceMode::LooseFirst) }
            .map_err(|_| native_state_error("particle program resource read failed"))?
            .ok_or_else(|| native_state_error("particle program resource is unavailable; old state released"))?;
        let program = if plugin { None } else { Some(particle_vm::ParticleProgram::parse(&program_bytes)?) };
        let legacy_external = external_draw != 0 && self.data_services.particle_external.legacy_kc_view();
        if legacy_external { self.data_services.particle_external.bound_canvas()?; }
        let texture = if image_name.is_empty() || (external_draw != 0 && !legacy_external) { None }
            else if legacy_external { self.load_managed_image(&image_name, 0, true)? }
            else {
                // Yuki14e8e7 calls the raw image wrapper5e2700, then14e928
                // reads its decoded width/height via5e1f30 ->5e48a0. Reuse
                // the real image dispatcher, including HG2/BMP/JPEG and HG
                // metadata/translation, without introducing cache insertion
                // or expanding a cropped HG frame to its canvas dimensions.
                let bytes = self.read_managed_image_bytes(&image_name)?
                    .ok_or_else(|| native_state_error("particle texture resource is unavailable; old state released"))?;
                Some(self.decode_managed_image_frame(&bytes, 0)?
                    .ok_or_else(|| native_state_error("particle texture frame0 is unavailable; old state released"))?)
            };
        // Campanella35e68 supplies an actual zero extent on missing texture.
        // Later PEs omit the creator entirely: retain an explicitly UNBOUND
        // extent, not a previous load's dimensions or a synthetic256 square.
        let image_size = texture.as_ref().map_or([0.; 2],
            |image| [image.width() as f32, image.height() as f32]);
        let viewport = match self.canvas.as_ref() {
            Some(canvas) => [canvas.width(), canvas.height()],
            None if plugin => return Err(native_state_error("particle plugin needs a real canvas extent")),
            None => [0;2],
        };
        let parameters = record[PARAMETERS..PARAMETERS + 128].try_into().unwrap();
        let count = read_u32(record, 0x104)?;
        let depth = read_u32(record, 0x108)?;
        let mut state = ParticleObjectState::new(program_name, image_name, count, parameters);
        state.depth_bits = depth;
        let canvas = particle_vm::ParticleCanvas::at_depth(viewport, f32::from_bits(depth));
        if plugin {
            let mut plugin = super::particle_plugin::ParticlePlugin::load(&program_bytes, count, image_size, canvas)?;
            plugin.retain_loaded_active(was_active);
            state.plugin = Some(plugin);
        } else {
            let mut runner = particle_vm::ParticleVm::new(program.unwrap(), count, image_size, parameters, canvas)?;
            runner.set_opacity(opacity);
            runner.bind_source(source as i32);
            runner.retain_loaded_active(was_active);
            state.runner = Some(runner);
        }
        state.texture = texture;
        state.external_draw = external_draw;
        state.creation_config = config;
        state.external_extent_bound = external_draw == 0 || legacy_external;
        state.opacity = opacity;
        state.transform_source = source;
        state.coordinate_group = group;
        state.enabled = was_active;
        if legacy_external {
            self.data_services.particle_external.create(target, &state.creation_config, state.texture.as_ref())?;
        }
        if external_draw == 0 {
            //14ec1a creates a Canvas object keyed by the task, then14ec3c
            //registers (0,0,record.depth). Use that same scene owner instead
            //of a tail overlay whose order was the numeric task-map order.
            if let Some(node) = self.scene_nodes.get_mut(&target) {
                node.position[2] = f32::from_bits(depth);
                node.initialized = true;
                node.visible = true;
            }
        }
        self.particles.insert(target, state);
        Ok(())
    }

    pub(crate) fn dispatch_particle_message(
        &mut self, target: u32, packet: [u32; 4], queued: bool, output: &mut u32,
        vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        let camera = camera_commands(self) && matches!(packet[1], 591 | 592);
        if !(271..=327).contains(&packet[1]) && !camera { return Ok(None); }
        let base = command_base(self)?;
        let Some(offset) = packet[1].checked_sub(base).filter(|offset| *offset < 8)
            .or_else(|| camera.then_some(8)) else {
            return Ok(None);
        };
        let command = TaskCommand { target: target as i32, sender: packet[0] as i32,
            code: packet[1], data: [packet[2], packet[3], 0] };
        if queued && offset == 4 {
            return Err(native_state_error("queued particle status cannot retain a caller output slot"));
        }
        let mut buffers = match self.pending_owned_task_buffers(command) {
            Some(buffers) => buffers,
            None => {
                let legacy = self.data_services.particle_external.legacy_kc_view();
                let length = match offset { 0 if legacy => RECORD_BYTES - 4, 0 => RECORD_BYTES, 5 => 128, _ => 0 };
                let data = if length != 0 && packet[3] != 0 {
                    let data = vm.as_deref().ok_or_else(|| native_state_error("raw particle record requires a caller VM"))?
                        .read(packet[3], length)?;
                    Some(if legacy && offset == 0 { let mut record = vec![0; 4]; record.extend(data); record }
                        else { data })
                } else { None };
                super::native_tasks::OwnedTaskBuffers::new(None, data)?
            }
        };
        if queued {
            let found = self.queue_task_command_buffers(command, &buffers, output)?;
            Ok(found.then_some(TaskCommandOutcome { done: true, propagate: false }))
        } else { self.route_task_command_buffers(command, &mut buffers, output, 4096) }
    }

    pub(super) fn particle_camera_native(&mut self, index: u32, args: &[u8]) -> Result<Option<NativeReturn>> {
        if !matches!(index, 1068 | 1069) { return Ok(None); }
        let target = read_u32(args, 0)?;
        if !self.scene_nodes.get(&target).is_some_and(|node| node.kind == 16) { return Ok(None); }
        if !camera_commands(self) {
            return Err(native_state_error("particle coordinate native is not identified for this PE"));
        }
        if args.len() != if index == 1068 { 8 } else { 4 } {
            return Err(native_state_error("particle coordinate native argument count is invalid"));
        }
        let code = if index == 1068 { 591 } else { 592 };
        let value = if index == 1068 { read_u32(args, 4)? } else { 0 };
        let mut buffers = super::native_tasks::OwnedTaskBuffers::default();
        let result = self.route_task_command_buffers(TaskCommand {
            target: target as i32, sender: self.current_task_id as i32,
            code, data: [0, value, 0],
        }, &mut buffers, &mut 0, 4096)?;
        Ok(Some(if result.is_some_and(|result| !result.done) { NativeReturn::Retry }
            else { NativeReturn::Continue }))
    }
}

impl super::HeadlessPlayer {
    pub(super) fn verified_kanmi_runtime_pe_sha256(&self) -> Option<&'static str> {
        kanmi_runtime_pe_sha256(self.data_services.particle_message_pe_sha256, self.default_entry())
    }

    /// The portable particle protocol can be recovered independently of the
    /// general ordinary-native table. Never use a file-version string to pick
    /// it. The one packed-package binding additionally requires the exact
    /// shipped plain companion AND the effective (patch-first) producer KCS.
    pub(super) fn initialize_particle_message_profile(&mut self) -> anyhow::Result<()> {
        const HAPPINESS_PLAIN: &str =
            "5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c";
        const HAPPINESS_PRODUCER: &str =
            "a853f3e0a1e1a1476cedd4bbc293a9ef93e996cec2db9986b1dd793ca71a3d15";
        let Some(binary) = self.scan.engine.primary_executable.as_ref() else { return Ok(()); };
        self.data_services.particle_message_pe_sha256 = identified_pe_sha256(&binary.sha256);
        if self.data_services.particle_message_pe_sha256.is_some() { return Ok(()); }
        let resources = &self.scan.resources;
        let kanmi_executable = std::path::Path::new(
            "\u{5c1a}\u{6709}\u{4f73}\u{5bc6}\u{4f34}\u{604b}\u{5fc3}.exe",
        );
        if resources.files().iter().any(|file|
            file.relative_path.as_path() == kanmi_executable && file.size == 4_223_965)
            && let Ok(runtime) = std::fs::read(resources.root().join(kanmi_executable))
            && let Ok(Some(startup)) = resources.read_mode("kcs.int/main.kcs", ResourceMode::LooseFirst)
            && let Ok(sscript) = resources.read_category(b"kcs", "sscript")
        {
            let sha256 = super::particle_plugin_digest::sha256;
            if kanmi_particle_producer_matches(&sha256(&runtime), &sha256(&startup), &sha256(&sscript)) {
                self.data_services.particle_message_pe_sha256 = Some(KANMI_PACKED);
                self.log.push("particle ABI: exact Kanmi KCS producer; packed raw numbering unverified".into());
                return Ok(());
            }
        }
        // These identify the installed package only. They are deliberately
        // NOT entries in PROFILES: their packed native tables are not proven.
        if !matches!(binary.sha256.as_str(),
            "21dfeb88edafc4bf19226d158e040687738f592282c578740e9d972127938df7" |
            "42cdb649a805eb489ccb5964b44b21981918ade062d7900e8386d6e146b20908")
        { return Ok(()); }
        let Some(companion) = resources.resolve_plain_file("cs2_crack.exe")? else {
            self.log.push("particle ABI unresolved: fixed Happiness2 plain companion is absent".into());
            return Ok(());
        };
        let bytes = resources.read_candidate(companion)?;
        if super::particle_plugin_digest::sha256(&bytes) != HAPPINESS_PLAIN {
            self.log.push("particle ABI unresolved: Happiness2 companion identity differs".into());
            return Ok(());
        }
        let producer = resources.read_category(b"kcs", "sscript")?;
        if super::particle_plugin_digest::sha256(&producer) != HAPPINESS_PRODUCER {
            self.log.push("particle ABI unresolved: effective Happiness2 sscript identity differs".into());
            return Ok(());
        }
        self.data_services.particle_message_pe_sha256 = Some(HAPPINESS_PLAIN);
        self.log.push("particle ABI: exact Happiness2 producer KCS + verified plain companion table; packed executable table unverified".into());
        Ok(())
    }

    /// Bind real external geometry before starting a 669 owner. Unlike the
    /// process-pointer API this takes explicit owned texture pixels and extent.
    pub fn configure_external_particle_surface(
        &mut self, target: u32, image_size: [f32; 2], texture: Option<cs2_renderer::RgbaFrame>,
    ) -> anyhow::Result<()> {
        if image_size.iter().any(|size| !size.is_finite() || *size < 0.) {
            anyhow::bail!("external particle image extent must be finite and nonnegative");
        }
        let state = self.particles.get_mut(&target)
            .ok_or_else(|| anyhow::anyhow!("external particle owner is not loaded"))?;
        if state.external_draw == 0 { anyhow::bail!("particle is not configured for external drawing"); }
        if self.data_services.particle_external.legacy_kc_view() {
            anyhow::bail!("legacy kcView extent belongs to its real image manager and cannot be replaced with an ordinary particle surface");
        }
        if let Some(plugin) = state.plugin.as_mut() {
            plugin.reset_extent(image_size)?;
        } else {
            let runner = state.runner.as_mut()
                .ok_or_else(|| anyhow::anyhow!("external particle program has been released"))?;
            if runner.active() || runner.has_continuation() {
                anyhow::bail!("bind external particle geometry before starting its program");
            }
            runner.reset_quads(state.grid_count, image_size)?;
        }
        state.texture = texture;
        state.external_extent_bound = true;
        state.rendered = None;
        Ok(())
    }
    /// Exact owned bytes. Campanella consumes them as a shared kcView parent
    /// path; later fixed receivers only copy them.
    pub fn particle_creation_configuration(&self, target: u32) -> Option<&[u8]> {
        self.particles.get(&target).map(|state| state.creation_config.as_slice())
    }

    /// Submit through the SHA-identified shared3D view's real camera graph.
    /// The formal scene/offscreen callbacks use the same retained sink.
    pub fn draw_external_particle(
        &self, target: u32, _destination: &mut cs2_renderer::Canvas,
        _texture: Option<&cs2_renderer::RgbaFrame>,
    ) -> anyhow::Result<()> {
        let state = self.particles.get(&target)
            .ok_or_else(|| anyhow::anyhow!("external particle owner is not loaded"))?;
        if state.external_draw == 0 { anyhow::bail!("particle belongs to the ordinary scene callback"); }
        let owner = self.data_services.particle_external.get(target)
            .ok_or_else(|| anyhow::anyhow!("external particle has no live shared graph object"))?.canvas;
        self.data_services.particle_external.draw_particle(owner, target, _destination, &self.particles)?;
        Ok(())
    }
}

fn snapshot_points(snapshot: &particle_vm::ParticleRenderSnapshot) -> Result<Vec<super::ParticlePoint>> {
    Ok(snapshot.projected_quads(&particle_vm::ParticleProjection::default())?.iter().map(|quad| {
        let vertex = quad.vertices[0];
        super::ParticlePoint::new(super::Point::new(vertex.position[0] as i32, vertex.position[1] as i32),
            super::Color::rgba((vertex.color >>16) as u8, (vertex.color >>8) as u8,
                vertex.color as u8, (vertex.color >>24) as u8))
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kanmi_particle_producer_keeps_raw_numbering_unidentified() {
        assert!(kanmi_particle_producer_matches(KANMI_PACKED, KANMI_STARTUP, KANMI_SSCRIPT));
        assert!(!kanmi_particle_producer_matches("other", KANMI_STARTUP, KANMI_SSCRIPT));
        assert!(!kanmi_particle_producer_matches(KANMI_PACKED, "other", KANMI_SSCRIPT));
        assert!(!kanmi_particle_producer_matches(KANMI_PACKED, KANMI_STARTUP, "other"));
        assert_eq!(identified_pe_sha256(KANMI_PACKED), None);
    }

    #[test]
    fn kanmi_runtime_identity_requires_verified_producer_and_exact_entry() {
        let startup = Some("kcs.int/main.kcs");
        let box_loader = "2044cd9c096c509da8609c0c2174038e0e03255aeaffd57fea6e817406e802ab";
        assert_eq!(kanmi_runtime_pe_sha256(Some(KANMI_PACKED), startup), Some(KANMI_PACKED));
        assert_eq!(kanmi_runtime_pe_sha256(Some(box_loader), startup), None);
        assert_eq!(kanmi_runtime_pe_sha256(None, startup), None);
        assert_eq!(kanmi_runtime_pe_sha256(Some(KANMI_PACKED), Some("main.kcs")), None);
        assert_eq!(kanmi_runtime_pe_sha256(Some(KANMI_PACKED), None), None);
    }

    #[test]
    fn kanmi_particle_producer_maps_only_verified_slots_on_live_kind16() {
        for (index, operation) in [(271, 0), (693, 0), (272, 1), (273, 2), (274, 4),
            (275, 5), (426, 6), (427, 7)] {
            assert_eq!(kanmi_particle_operation(Some(KANMI_PACKED), index, Some((16, 0, 0))), Some(operation));
            assert_eq!(kanmi_particle_operation(Some("other"), index, Some((16, 0, 0))), None);
            assert_eq!(kanmi_particle_operation(Some(KANMI_PACKED), index, Some((16, 1, 0))), None);
            assert_eq!(kanmi_particle_operation(Some(KANMI_PACKED), index, Some((34, 0, 0))), None);
            assert_eq!(kanmi_particle_operation(Some(KANMI_PACKED), index, None), None);
        }
        for index in [669, 320, 1068, 1069] {
            assert_eq!(kanmi_particle_operation(Some(KANMI_PACKED), index, Some((16, 0, 0))), None);
        }
    }
}
