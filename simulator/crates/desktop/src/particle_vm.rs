//! Independent particle KCS host. Native indices belong to the particle table.
//! Geometry stays in script coordinates until the renderer projects it.

use cs2_script::{Host, KcsProgram, NativeReturn, Result, ScriptError, Step, Vm};
use std::fmt;
use std::sync::{Arc, Mutex};
use cs2_runtime::graph_serde::{Deserialize, Serialize};

#[path = "particle_vm_state.rs"]
pub(crate) mod checkpoint;

const EXTERNAL_BASE: u32 = 0x8000_0000;
const MAX_STORAGE: usize = 64 * 1024 * 1024;
const VERTEX_BYTES: usize = 28;
const QUAD_BYTES: usize = VERTEX_BYTES * 4;
const PARAMETER_BYTES: usize = 128;
const FAR_DEPTH: f32 = 65_000.0;
const FOCAL_LENGTH: f32 = 1_000.0;
const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

pub(super) type ParticleExternal = Arc<Mutex<Vec<u8>>>;
pub(super) type ParticleParameters = Arc<Mutex<[u8; PARAMETER_BYTES]>>;

#[derive(Clone, Debug)]
pub(super) struct ParticleProgram {
    program: Arc<KcsProgram>,
}

impl ParticleProgram {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        Self::from_program(KcsProgram::parse(bytes)?)
    }

    pub(super) fn from_program(program: KcsProgram) -> Result<Self> {
        let storage = program.data.len().checked_add(program.stack_size);
        if storage.is_none_or(|length| length > MAX_STORAGE) {
            return Err(invalid(0, "particle program storage exceeds 64 MiB"));
        }
        Ok(Self { program: Arc::new(program) })
    }

    pub(super) fn program(&self) -> &KcsProgram {
        &self.program
    }

    pub(super) fn version(&self) -> u32 {
        self.program.header.program_version
    }

    pub(super) fn native_count(&self) -> u32 {
        if self.version() == 2 { 51 } else { 44 }
    }
}

/// The byte layout is XYZ, RHW, ARGB, UV (seven little-endian DWORDs).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct ParticleVertex {
    pub position: [f32; 3],
    pub rhw: f32,
    pub color: u32,
    pub uv: [f32; 2],
}

impl Default for ParticleVertex {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rhw: 1.0,
            color: u32::MAX,
            uv: [0.0; 2],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct ParticleSource {
    pub color: u32,
    pub position_2d: [f32; 2],
    pub position_3d: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct ParticleCanvas {
    pub viewport: [u32; 2],
    pub frame_rate: u32,
    pub depth_offset: f32,
    pub camera_depth: f32,
}

impl Default for ParticleCanvas {
    fn default() -> Self {
        Self {
            viewport: [0; 2],
            frame_rate: 60,
            depth_offset: 0.0,
            camera_depth: 0.0,
        }
    }
}

impl ParticleCanvas {
    pub(super) fn new(viewport: [u32; 2]) -> Self {
        Self { viewport, ..Self::default() }
    }

    /// Choose a camera whose unit scale lies at the load-time effect depth.
    pub(super) fn at_depth(viewport: [u32; 2], depth_offset: f32) -> Self {
        Self {
            viewport,
            depth_offset,
            camera_depth: sub(sub(FAR_DEPTH, depth_offset), FOCAL_LENGTH),
            ..Self::default()
        }
    }

    /// Native 38: unproject a screen rectangle without a homogeneous W divide.
    pub(super) fn projection_bounds(&self, input_depth: f32) -> [f32; 4] {
        let depth = sub(FAR_DEPTH, add(self.depth_offset, input_depth));
        let distance = if depth == self.camera_depth {
            1.0
        } else {
            sub(depth, self.camera_depth)
        };
        let scale = max_zero(div(FOCAL_LENGTH, distance));
        let mut lens = [0.0; 16];
        lens[0] = scale;
        lens[5] = scale;
        lens[10] = 1.0 / FAR_DEPTH;
        lens[15] = div(1.0, distance);
        let lens = matrix_multiply(IDENTITY, lens);
        let inverse = matrix_inverse(lens).unwrap_or(lens);
        let [half_x, half_y] = self.viewport.map(|size| (size >> 1) as f32);
        let point = transform([half_x, half_y, depth, 1.0], inverse);
        [
            sub(half_x, point[0]),
            sub(half_y, point[1]),
            add(half_x, point[0]),
            add(half_y, point[1]),
        ]
    }
}

/// Row-major matrices; four-dimensional row vectors multiply from the left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ParticleProjection {
    pub coordinate_scale: [f32; 2],
    pub coordinate_offset: [f32; 2],
    pub coordinate_matrix: [f32; 16],
    pub canvas_matrix: [f32; 16],
    pub translation: [f32; 2],
}

impl Default for ParticleProjection {
    fn default() -> Self {
        Self {
            coordinate_scale: [1.0; 2],
            coordinate_offset: [0.0; 2],
            coordinate_matrix: IDENTITY,
            canvas_matrix: IDENTITY,
            translation: [0.0; 2],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct ParticleQuad {
    pub index: u32,
    pub vertices: [ParticleVertex; 4],
    pub offset: [f32; 4],
    /// SDK+7c: separate post-projection depth adjustment, not XYZ movement.
    pub depth_bias: f32,
    pub flag: u32,
}

impl ParticleQuad {
    pub(super) fn depth(&self) -> f32 {
        sub(add(self.vertices[0].position[2], self.offset[2]),mul(self.depth_bias,FAR_DEPTH))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct ParticleRenderSnapshot {
    pub active: bool,
    pub frame_complete: bool,
    pub elapsed: u32,
    pub previous_elapsed: u32,
    pub current_quad: u32,
    pub render_mode: u32,
    pub opacity: u32,
    pub canvas: ParticleCanvas,
    pub quads: Vec<ParticleQuad>,
}

impl ParticleRenderSnapshot {
    /// Complete triangle-strip quads in stable back-to-front submission order.
    /// The renderer binds a bottom-up particle texture and triangulates
    /// [0, 1, 2] and [2, 1, 3]. Returned RHW remains a separate interpolant.
    pub(super) fn projected_quads(
        &self,
        view: &ParticleProjection,
    ) -> Result<Vec<ParticleQuad>> {
        if !self.frame_complete {
            return Err(invalid(0, "particle frame still has a budget continuation"));
        }
        if !self.active || self.opacity == 0 {
            return Ok(Vec::new());
        }
        let mut quads: Vec<_> = self.quads.iter().filter(|quad| quad.flag != 0).cloned().collect();
        quads.sort_by(|left, right| {
            let delta = sub(left.depth(), right.depth());
            if delta < 0.0 {
                std::cmp::Ordering::Less
            } else if delta > 0.0 {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        for quad in &mut quads {
            quad.vertices = quad.vertices.map(|vertex| {
                let mut projected=project_vertex(vertex, quad.offset, self.canvas, view, self.opacity);
                projected.position[2]=sub(projected.position[2],quad.depth_bias);
                projected
            });
            quad.offset = [0.0; 4];
            quad.depth_bias = 0.0;
        }
        Ok(quads)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ParticleUpdate {
    Idle,
    Yield,
    Complete,
}

/// Implement this on the ordinary-task random service to share both streams.
pub(super) trait ParticleRandomSource {
    fn next_integer(&mut self) -> u32;
    fn next_float_bits(&mut self) -> u32;
}

#[derive(Clone, Debug)]
pub(super) struct ParticleRandom {
    integer: Mt19937,
    floating: Mt19937,
}

impl Default for ParticleRandom {
    fn default() -> Self {
        Self::seeded(4357)
    }
}

impl ParticleRandom {
    pub(super) fn seeded(seed: u32) -> Self {
        Self {
            integer: Mt19937::seeded(seed),
            floating: Mt19937::seeded(seed),
        }
    }

    pub(super) fn next_integer(&mut self) -> u32 {
        self.integer.next()
    }

    pub(super) fn next_float_bits(&mut self) -> u32 {
        let unit = f64::from(self.floating.next()) * (1.0 / 4_294_967_295.0);
        (unit as f32).to_bits()
    }
}

impl ParticleRandomSource for ParticleRandom {
    fn next_integer(&mut self) -> u32 {
        ParticleRandom::next_integer(self)
    }

    fn next_float_bits(&mut self) -> u32 {
        ParticleRandom::next_float_bits(self)
    }
}

#[derive(Clone, Debug)]
struct Mt19937 {
    words: [u32; 624],
    cursor: usize,
}

impl Mt19937 {
    fn seeded(mut seed: u32) -> Self {
        let words = std::array::from_fn(|_| {
            let upper = seed & 0xffff_0000;
            seed = seed.wrapping_mul(69_069).wrapping_add(1);
            let word = upper | (seed >> 16);
            seed = seed.wrapping_mul(69_069).wrapping_add(1);
            word
        });
        Self { words, cursor: 624 }
    }

    fn next(&mut self) -> u32 {
        if self.cursor == self.words.len() {
            for index in 0..624 {
                let adjacent = if index == 623 { 0 } else { index + 1 };
                let remote = if index < 227 { index + 397 } else { index - 227 };
                let joined = (self.words[index] & 0x8000_0000)
                    | (self.words[adjacent] & 0x7fff_ffff);
                let odd = 0_u32.wrapping_sub(joined & 1);
                self.words[index] = self.words[remote] ^ (joined >> 1) ^ (odd & 0x9908_b0df);
            }
            self.cursor = 0;
        }
        let mut word = self.words[self.cursor];
        self.cursor += 1;
        word ^= word >> 11;
        word ^= (word << 7) & 0x9d2c_5680;
        word ^= (word << 15) & 0xefc6_0000;
        word ^ (word >> 18)
    }
}

struct ParticleState {
    canvas: ParticleCanvas,
    quads: Vec<[ParticleVertex; 4]>,
    saved_quads: Vec<[ParticleVertex; 4]>,
    offsets: Vec<[f32; 4]>,
    flags: Vec<u32>,
    parameters: ParticleParameters,
    parameter_external_offset: Option<u32>,
    elapsed: u32,
    previous_elapsed: u32,
    active: bool,
    current_quad: u32,
    object_id: i32,
    source: Option<ParticleSource>,
    render_mode: u32,
    opacity: u32,
    debug_messages: Vec<Vec<u8>>,
}

pub(super) struct ParticleVm {
    program: ParticleProgram,
    vm: Vm,
    state: ParticleState,
    external: ParticleExternal,
    image_size: [f32; 2],
    origin: u32,
    previous_time: u32,
    round_open: bool,
    invocation_open: bool,
    ordinary_draw_registered: bool,
    fault: Option<ScriptError>,
}

impl fmt::Debug for ParticleVm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ParticleVm")
            .field("version", &self.program.version())
            .field("quad_count", &self.state.quads.len())
            .field("current_quad", &self.state.current_quad)
            .field("elapsed", &self.state.elapsed)
            .field("active", &self.state.active)
            .field("round_open", &self.round_open)
            .field("pc", &self.vm.instruction_pointer)
            .field("fault", &self.fault)
            .finish()
    }
}

impl ParticleVm {
    pub(super) fn new(
        program: ParticleProgram,
        count: u32,
        image_size: [f32; 2],
        parameters: [u8; PARAMETER_BYTES],
        canvas: ParticleCanvas,
    ) -> Result<Self> {
        let vm = Vm::from_program(program.program().clone())?;
        let mut particle = Self {
            program,
            vm,
            state: ParticleState {
                canvas,
                quads: Vec::new(),
                saved_quads: Vec::new(),
                offsets: Vec::new(),
                flags: Vec::new(),
                parameters: Arc::new(Mutex::new(parameters)),
                parameter_external_offset: None,
                elapsed: 0,
                previous_elapsed: u32::MAX,
                active: false,
                current_quad: 0,
                object_id: -1,
                source: None,
                render_mode: 0,
                opacity: 256,
                debug_messages: Vec::new(),
            },
            external: Arc::new(Mutex::new(Vec::new())),
            image_size,
            origin: 0,
            previous_time: 0,
            round_open: false,
            invocation_open: false,
            ordinary_draw_registered: false,
            fault: None,
        };
        particle.reset_quads(count, image_size)?;
        Ok(particle)
    }

    pub(super) fn from_kcs(
        bytes: &[u8],
        count: u32,
        image_size: [f32; 2],
        parameters: [u8; PARAMETER_BYTES],
        canvas: ParticleCanvas,
    ) -> Result<Self> {
        Self::new(ParticleProgram::parse(bytes)?, count, image_size, parameters, canvas)
    }

    /// Start and restart reconstruct geometry and the entry frame, preserving
    /// the script's mutable data, parameters, external storage and random state.
    pub(super) fn start(&mut self) -> Result<()> {
        self.round_open = false;
        self.invocation_open = false;
        self.reset_quads(self.state.quads.len() as u32, self.image_size)?;
        self.reset_execution_preserving_memory()?;
        self.origin = 0;
        self.previous_time = 0;
        self.state.elapsed = 0;
        self.state.previous_elapsed = u32::MAX;
        self.state.current_quad = 0;
        self.state.active = true;
        self.round_open = false;
        self.invocation_open = false;
        self.fault = None;
        self.ordinary_draw_registered = true;
        Ok(())
    }

    pub(super) fn restart(&mut self) -> Result<()> {
        self.start()
    }

    pub(super) fn stop(&mut self) {
        self.state.active = false;
        self.ordinary_draw_registered = false;
        // An external322 prevents the next body from executing at all
        // (Yuki14ef8b and14e3e3). A host budget continuation must not bypass
        // that active check and keep consuming instructions/random samples.
        // Keep VM memory; a later321 resets its execution frame. Native32
        // inside ParticleHost deliberately keeps completing its current round.
        self.round_open = false;
        self.invocation_open = false;
    }

    /// Load320 preserves the task's active DWORD, but its release helper
    /// removes the previous ordinary Canvas callback (Yuki14de90..14debd).
    /// Only start321 registers it again at14ee19..14ee30. Do not turn a
    /// reload into start: the new VM may update while no draw is registered.
    pub(super) fn retain_loaded_active(&mut self, active: bool) {
        self.state.active = active;
        self.ordinary_draw_registered = false;
    }

    pub(super) fn ordinary_draw_registered(&self) -> bool {
        self.ordinary_draw_registered
    }

    pub(super) fn active(&self) -> bool {
        self.state.active
    }

    pub(super) fn elapsed(&self) -> u32 {
        self.state.elapsed
    }

    pub(super) fn previous_elapsed(&self) -> u32 {
        self.state.previous_elapsed
    }

    pub(super) fn has_continuation(&self) -> bool {
        self.round_open
    }

    pub(super) fn program(&self) -> &ParticleProgram {
        &self.program
    }

    pub(super) fn interpreter(&self) -> &Vm {
        &self.vm
    }

    pub(super) fn canvas(&self) -> ParticleCanvas {
        self.state.canvas
    }

    pub(super) fn image_size(&self) -> [f32; 2] {
        self.image_size
    }

    pub(super) fn set_canvas(&mut self, canvas: ParticleCanvas) -> Result<()> {
        if self.round_open {
            return Err(invalid(0, "cannot replace particle canvas during a frame continuation"));
        }
        self.state.canvas = canvas;
        Ok(())
    }

    pub(super) fn set_opacity(&mut self, raw: u32) {
        self.state.opacity = if (raw as i32) < 255 { raw } else { 256 };
    }

    pub(super) fn bind_source(&mut self, object_id: i32) {
        self.state.object_id = object_id;
        if object_id == -1 {
            self.state.source = None;
        }
    }

    /// A missing lookup retains the last cached source until explicit detach.
    pub(super) fn refresh_source(&mut self, source: Option<ParticleSource>) {
        if self.state.object_id == -1 {
            self.state.source = None;
        } else if let Some(source) = source {
            self.state.source = Some(source);
        }
    }

    pub(super) fn set_source(&mut self, object_id: i32, source: Option<ParticleSource>) {
        self.bind_source(object_id);
        self.state.source = if object_id == -1 { None } else { source };
    }

    pub(super) fn parameter_block(&self) -> ParticleParameters {
        Arc::clone(&self.state.parameters)
    }

    pub(super) fn bind_parameters(&mut self, parameters: ParticleParameters) {
        self.state.parameters = parameters;
        self.state.parameter_external_offset = None;
    }

    pub(super) fn parameters(&self) -> Result<[u8; PARAMETER_BYTES]> {
        if let Some(offset) = self.state.parameter_external_offset {
            let bytes = self.external.lock()
                .map_err(|_| invalid(offset as usize, "particle external memory lock is poisoned"))?;
            let range = byte_range(offset, PARAMETER_BYTES, bytes.len())?;
            let mut output = [0; PARAMETER_BYTES];
            output.copy_from_slice(&bytes[range]);
            Ok(output)
        } else {
            let parameters = self.state.parameters.lock()
                .map_err(|_| invalid(0, "particle parameter lock is poisoned"))?;
            Ok(*parameters)
        }
    }

    pub(super) fn set_parameters(&mut self, parameters: [u8; PARAMETER_BYTES]) -> Result<()> {
        if let Some(offset) = self.state.parameter_external_offset {
            let mut external = self.external.lock()
                .map_err(|_| invalid(offset as usize, "particle external memory lock is poisoned"))?;
            let range = byte_range(offset, PARAMETER_BYTES, external.len())?;
            external[range].copy_from_slice(&parameters);
        } else {
            *self.state.parameters.lock()
                .map_err(|_| invalid(0, "particle parameter lock is poisoned"))? = parameters;
        }
        Ok(())
    }

    pub(super) fn set_parameter_words(&mut self, words: [u32; 32]) -> Result<()> {
        let mut parameters = [0; PARAMETER_BYTES];
        for (target, word) in parameters.chunks_exact_mut(4).zip(words) {
            target.copy_from_slice(&word.to_le_bytes());
        }
        self.set_parameters(parameters)
    }

    /// Explicitly alias native 35's parameter source into shared external RAM.
    pub(super) fn parameters_from_external(&mut self, offset: u32) -> Result<()> {
        if offset & EXTERNAL_BASE != 0 {
            return Err(invalid(offset as usize, "parameter external offset must be relative"));
        }
        let external = self.external.lock()
            .map_err(|_| invalid(offset as usize, "particle external memory lock is poisoned"))?;
        byte_range(offset, PARAMETER_BYTES, external.len())?;
        self.state.parameter_external_offset = Some(offset);
        Ok(())
    }

    pub(super) fn external_storage(&self) -> ParticleExternal {
        Arc::clone(&self.external)
    }

    pub(super) fn bind_external(&mut self, external: ParticleExternal) -> Result<()> {
        {
            let bytes = external.lock()
                .map_err(|_| invalid(0, "particle external memory lock is poisoned"))?;
            if bytes.len() > MAX_STORAGE {
                return Err(invalid(0, "particle external memory exceeds 64 MiB"));
            }
            if let Some(offset) = self.state.parameter_external_offset {
                byte_range(offset, PARAMETER_BYTES, bytes.len())?;
            }
        }
        self.external = external;
        Ok(())
    }

    pub(super) fn quads(&self) -> &[[ParticleVertex; 4]] {
        &self.state.quads
    }

    pub(super) fn saved_quads(&self) -> &[[ParticleVertex; 4]] {
        &self.state.saved_quads
    }

    pub(super) fn offsets(&self) -> &[[f32; 4]] {
        &self.state.offsets
    }

    pub(super) fn flags(&self) -> &[u32] {
        &self.state.flags
    }

    pub(super) fn drain_debug_messages(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.state.debug_messages)
    }

    pub(super) fn render_snapshot(&self) -> ParticleRenderSnapshot {
        ParticleRenderSnapshot {
            active: self.state.active,
            frame_complete: !self.round_open,
            elapsed: self.state.elapsed,
            previous_elapsed: self.state.previous_elapsed,
            current_quad: self.state.current_quad,
            render_mode: self.state.render_mode,
            opacity: self.state.opacity,
            canvas: self.state.canvas,
            quads: self.state.quads.iter().enumerate().map(|(index, vertices)| ParticleQuad {
                index: index as u32,
                vertices: *vertices,
                offset: self.state.offsets[index],
                depth_bias: 0.0,
                flag: self.state.flags[index],
            }).collect(),
        }
    }

    pub(super) fn reset_quads(&mut self, count: u32, image_size: [f32; 2]) -> Result<()> {
        if self.round_open {
            return Err(invalid(0, "cannot resize particle quads during a frame continuation"));
        }
        // 112 bytes each for current/saved geometry, 16 for offset, 4 for flag.
        let bytes = (count as usize).checked_mul(QUAD_BYTES * 2 + 20);
        if bytes.is_none_or(|bytes| bytes > MAX_STORAGE) {
            return Err(invalid(count as usize, "particle quad arrays exceed 64 MiB"));
        }
        let [width, height] = image_size;
        let positions = [[0.0, 0.0], [width, 0.0], [0.0, height], [width, height]];
        let uv = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        let quad = std::array::from_fn(|index| ParticleVertex {
            position: [positions[index][0], positions[index][1], 0.0],
            uv: uv[index],
            ..ParticleVertex::default()
        });
        self.state.quads = vec![quad; count as usize];
        self.state.saved_quads = self.state.quads.clone();
        self.state.offsets = vec![[0.0; 4]; count as usize];
        self.state.flags = vec![1; count as usize];
        self.image_size = image_size;
        Ok(())
    }

    fn reset_execution_preserving_memory(&mut self) -> Result<()> {
        self.vm.reset_execution()
    }

    /// Budget is a total instruction budget for this call, across all quads.
    /// A continuation retains this round's time and current interpreter frame.
    pub(super) fn update(
        &mut self,
        now: u32,
        speed: u32,
        budget: usize,
        random: &mut impl ParticleRandomSource,
    ) -> Result<ParticleUpdate> {
        if let Some(error) = &self.fault {
            return Err(error.clone());
        }
        if !self.round_open {
            if !self.state.active {
                return Ok(ParticleUpdate::Idle);
            }
            if self.origin == 0 {
                self.origin = now;
                self.state.elapsed = 0;
                self.state.previous_elapsed = u32::MAX;
            } else {
                self.state.elapsed = self.state.elapsed.wrapping_add(
                    now.wrapping_sub(self.previous_time).wrapping_mul(speed),
                );
            }
            self.previous_time = now;
            self.state.current_quad = 0;
            self.round_open = true;
            self.invocation_open = false;
        }

        // Hold one backing store through this scheduler slice. VM external
        // reads and writes are committed before another bound task can run.
        let external = Arc::clone(&self.external);
        let mut bytes = external.lock()
            .map_err(|_| invalid(0, "particle external memory lock is poisoned"))?;
        if bytes.len() > MAX_STORAGE {
            return Err(invalid(0, "particle external memory exceeds 64 MiB"));
        }
        if let Some(offset) = self.state.parameter_external_offset {
            byte_range(offset, PARAMETER_BYTES, bytes.len())?;
        }
        self.vm.bind_external(bytes.clone())?;
        let result = self.update_slice(budget, random);
        bytes.copy_from_slice(&self.vm.external_bytes()?);
        if let Err(error) = &result {
            self.fault = Some(error.clone());
        }
        result
    }

    fn update_slice(
        &mut self,
        mut budget: usize,
        random: &mut impl ParticleRandomSource,
    ) -> Result<ParticleUpdate> {
        loop {
            if self.state.current_quad as usize >= self.state.quads.len() {
                self.state.previous_elapsed = self.state.elapsed;
                self.round_open = false;
                self.invocation_open = false;
                return Ok(ParticleUpdate::Complete);
            }
            if budget == 0 {
                return Ok(ParticleUpdate::Yield);
            }
            if !self.invocation_open {
                self.reset_execution_preserving_memory()?;
                self.invocation_open = true;
            }
            let mut host = ParticleHost {
                state: &mut self.state,
                version: self.program.version(),
                random: &mut *random,
            };
            let step = self.vm.run(&mut host, 1)?;
            budget -= 1;
            match step {
                Step::Yield => {}
                Step::Halted | Step::Event(_) => {
                    self.state.current_quad = self.state.current_quad.wrapping_add(1);
                    self.invocation_open = false;
                }
            }
        }
    }
}

struct ParticleHost<'a, R> {
    state: &'a mut ParticleState,
    version: u32,
    random: &'a mut R,
}

impl<R: ParticleRandomSource> Host for ParticleHost<'_, R> {
    fn call(&mut self, index: u32, arguments: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let limit = if self.version == 2 { 51 } else { 44 };
        if index >= limit {
            return Err(invalid(index as usize, "native index is outside the particle version table"));
        }
        let args = Arguments { bytes: arguments };
        particle_native(index, args, self.state, self.random, vm)
    }
}

#[derive(Clone, Copy)]
struct Arguments<'a> {
    bytes: &'a [u8],
}

impl Arguments<'_> {
    fn word(self, index: usize) -> Result<u32> {
        let offset = index.checked_mul(4).ok_or_else(|| invalid(index, "native argument offset overflows"))?;
        let raw = self.bytes.get(offset..offset.saturating_add(4))
            .ok_or(ScriptError::Truncated { offset, needed: 4 })?;
        let mut bytes = [0; 4];
        bytes.copy_from_slice(raw);
        Ok(u32::from_le_bytes(bytes))
    }

    fn float(self, index: usize) -> Result<f32> {
        Ok(f32::from_bits(self.word(index)?))
    }

    fn exact(self, words: usize) -> Result<()> {
        if self.bytes.len() != words * 4 {
            return Err(invalid(self.bytes.len(), "particle native argument byte count is invalid"));
        }
        Ok(())
    }
}

fn invalid(offset: usize, message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset, message }
}

fn byte_range(offset: u32, length: usize, storage: usize) -> Result<std::ops::Range<usize>> {
    let start = offset as usize;
    let end = start.checked_add(length).filter(|end| *end <= storage)
        .ok_or(ScriptError::MemoryOutOfBounds { address: EXTERNAL_BASE | offset, length })?;
    Ok(start..end)
}

fn particle_native(
    index: u32,
    args: Arguments<'_>,
    state: &mut ParticleState,
    random: &mut impl ParticleRandomSource,
    vm: &mut Vm,
) -> Result<NativeReturn> {
    let words = match index {
        0 | 3 | 6..=11 | 13..=15 | 17 | 19..=21 | 30 | 31 | 35 | 42 | 50 => 1,
        5 | 12 | 16 | 18 | 22..=24 | 39 | 43 | 48 => 2,
        27..=29 | 49 => 3,
        25 | 26 => 4,
        38 | 41 => 5,
        37 => 6,
        1 | 2 | 4 | 32..=34 | 36 | 40 | 44..=47 => 0,
        _ => return Err(invalid(index as usize, "invalid particle native index")),
    };
    args.exact(words)?;
    let value = match index {
        0 => {
            let format = vm.read_string(args.word(0)?)?;
            let text = super::expand_kcs_format(vm, &format, 0)?;
            let used = state.debug_messages.iter().try_fold(text.len(), |sum, message| {
                sum.checked_add(message.len())
            });
            if used.is_none_or(|used| used > MAX_STORAGE) {
                return Err(invalid(0, "particle debug message storage exceeds 64 MiB"));
            }
            state.debug_messages.push(text);
            return Ok(NativeReturn::Continue);
        }
        1 => state.elapsed,
        2 => state.previous_elapsed,
        3 => {
            let input = args.word(0)?;
            if (input as i32) <= 0 {
                input
            } else {
                let ratio = div(state.canvas.frame_rate as f32, 60.0);
                float_to_i32(mul(ratio, (input as i32) as f32)).max(1) as u32
            }
        }
        4 => state.canvas.frame_rate,
        5 => {
            write_words(vm, &[args.word(0)?, args.word(1)?], &state.canvas.viewport)?;
            return Ok(NativeReturn::Continue);
        }
        6..=20 => {
            let first = args.word(0)?;
            let second = if matches!(index, 12 | 16 | 18) { args.word(1)? } else { 0 };
            particle_math(index, first, second)
        }
        21 => (args.word(0)? as i32).wrapping_abs() as u32,
        22..=24 => {
            let address = args.word(0)?;
            let angle = args.float(1)?;
            let mut quad = read_quad(vm, address)?;
            let sine = f32::from_bits(math_unary(angle.to_bits(), f64::sin));
            let cosine = f32::from_bits(math_unary(angle.to_bits(), f64::cos));
            for vertex in &mut quad {
                let [x, y, z] = vertex.position;
                vertex.position = match index {
                    22 => [x, sub(mul(y, cosine), mul(z, sine)), add(mul(y, sine), mul(z, cosine))],
                    23 => [add(mul(x, cosine), mul(z, sine)), y, sub(mul(z, cosine), mul(x, sine))],
                    24 => [sub(mul(x, cosine), mul(y, sine)), add(mul(x, sine), mul(y, cosine)), z],
                    _ => unreachable!(),
                };
            }
            write_quad(vm, address, quad)?;
            return Ok(NativeReturn::Continue);
        }
        25 | 26 => {
            let address = args.word(0)?;
            let components = [args.float(1)?, args.float(2)?, args.float(3)?];
            let mut quad = read_quad(vm, address)?;
            for vertex in &mut quad {
                for (target, component) in vertex.position.iter_mut().zip(components) {
                    *target = if index == 25 { add(*target, component) } else { mul(*target, component) };
                }
                if index == 25 {
                    vertex.rhw = add(vertex.rhw, 0.0);
                }
            }
            write_quad(vm, address, quad)?;
            return Ok(NativeReturn::Continue);
        }
        27 => {
            let index = current_quad(state)?;
            state.offsets[index] = [args.float(0)?, args.float(1)?, args.float(2)?, 0.0];
            return Ok(NativeReturn::Continue);
        }
        28 => {
            let index = current_quad(state)?;
            let offsets = state.offsets[index];
            write_floats(vm, &[args.word(0)?, args.word(1)?, args.word(2)?], &offsets[..3])?;
            return Ok(NativeReturn::Continue);
        }
        29 => {
            let index = current_quad(state)?;
            // The verified table consumes three float DWORDs, despite the
            // four-integer declaration present in older SDK headers.
            let u = div(sub(args.float(0)?, 0.5), state.canvas.viewport[0] as f32);
            let v = div(sub(args.float(2)?, 0.5), state.canvas.viewport[1] as f32);
            let uv = [[0.0, 0.0], [u, 0.0], [0.0, v], [u, v]];
            for (vertex, uv) in state.quads[index].iter_mut().zip(uv) {
                vertex.uv = uv;
            }
            return Ok(NativeReturn::Continue);
        }
        30 => {
            let index = current_quad(state)?;
            write_quad(vm, args.word(0)?, state.quads[index])?;
            return Ok(NativeReturn::Continue);
        }
        31 => {
            let index = current_quad(state)?;
            state.quads[index] = read_quad(vm, args.word(0)?)?;
            return Ok(NativeReturn::Continue);
        }
        32 => {
            state.active = false;
            return Ok(NativeReturn::YieldVoid);
        }
        33 => random.next_integer(),
        34 => random.next_float_bits(),
        35 => {
            let parameters = if let Some(offset) = state.parameter_external_offset {
                vm.read(EXTERNAL_BASE | offset, PARAMETER_BYTES)?
            } else {
                state.parameters.lock()
                    .map_err(|_| invalid(0, "particle parameter lock is poisoned"))?
                    .to_vec()
            };
            vm.write(args.word(0)?, &parameters)?;
            return Ok(NativeReturn::Continue);
        }
        36 => state.current_quad,
        37 => {
            let delta = [
                sub(args.float(0)?, args.float(3)?),
                sub(args.float(1)?, args.float(4)?),
                sub(args.float(2)?, args.float(5)?),
            ];
            let square = add(add(mul(delta[0], delta[0]), mul(delta[1], delta[1])), mul(delta[2], delta[2]));
            math_unary(square.to_bits(), f64::sqrt)
        }
        38 => {
            let bounds = state.canvas.projection_bounds(args.float(0)?);
            let targets = [args.word(1)?, args.word(2)?, args.word(3)?, args.word(4)?];
            write_floats(vm, &targets, &bounds)?;
            return Ok(NativeReturn::Continue);
        }
        39 => {
            if let Some(flag) = state.flags.get_mut(args.word(0)? as usize) {
                *flag = args.word(1)?;
            }
            return Ok(NativeReturn::Continue);
        }
        40 => (state.quads.len() as u32).wrapping_sub(1),
        41 => linear_interpolation([
            args.float(0)?, args.float(1)?, args.float(2)?, args.float(3)?, args.float(4)?,
        ]).to_bits(),
        42 => {
            let shifted = add(args.float(0)?, 0.5);
            let rounded = f32::from_bits(math_unary(shifted.to_bits(), f64::floor));
            float_to_i32(rounded) as u32
        }
        43 => triangle_wave(args.float(0)?, args.float(1)?).to_bits(),
        44 => {
            if state.saved_quads.len() != state.quads.len() {
                return Err(invalid(0, "particle saved/current quad counts differ"));
            }
            state.quads.clone_from(&state.saved_quads);
            return Ok(NativeReturn::Continue);
        }
        45 => {
            if state.saved_quads.len() != state.quads.len() {
                return Err(invalid(0, "particle saved/current quad counts differ"));
            }
            state.saved_quads.clone_from(&state.quads);
            return Ok(NativeReturn::Continue);
        }
        46 => state.object_id as u32,
        47 => effective_source(state).color,
        48 => {
            let source = effective_source(state);
            write_floats(vm, &[args.word(0)?, args.word(1)?], &source.position_2d)?;
            return Ok(NativeReturn::Continue);
        }
        49 => {
            let source = effective_source(state);
            write_floats(
                vm,
                &[args.word(0)?, args.word(1)?, args.word(2)?],
                &source.position_3d,
            )?;
            return Ok(NativeReturn::Continue);
        }
        50 => {
            let mode = args.word(0)?;
            state.render_mode = if mode < 6 { mode } else { 0 };
            return Ok(NativeReturn::Continue);
        }
        _ => return Err(invalid(index as usize, "particle native dispatch is invalid")),
    };
    Ok(NativeReturn::Value(value))
}

fn effective_source(state: &ParticleState) -> ParticleSource {
    if state.object_id == -1 {
        ParticleSource::default()
    } else {
        state.source.unwrap_or_default()
    }
}

fn current_quad(state: &ParticleState) -> Result<usize> {
    let index = state.current_quad as usize;
    if index >= state.quads.len()
        || index >= state.saved_quads.len()
        || index >= state.offsets.len()
        || index >= state.flags.len()
    {
        Err(invalid(index, "particle current quad is outside its backing arrays"))
    } else {
        Ok(index)
    }
}

fn write_words(vm: &mut Vm, addresses: &[u32], words: &[u32]) -> Result<()> {
    for (&address, word) in addresses.iter().zip(words) {
        vm.write(address, &word.to_le_bytes())?;
    }
    Ok(())
}

fn write_floats(vm: &mut Vm, addresses: &[u32], values: &[f32]) -> Result<()> {
    for (&address, value) in addresses.iter().zip(values) {
        vm.write(address, &value.to_bits().to_le_bytes())?;
    }
    Ok(())
}

fn read_quad(vm: &Vm, address: u32) -> Result<[ParticleVertex; 4]> {
    let bytes = vm.read(address, QUAD_BYTES)?;
    let mut output = [ParticleVertex::default(); 4];
    for (vertex, raw) in output.iter_mut().zip(bytes.chunks_exact(VERTEX_BYTES)) {
        let mut words = [0; 7];
        for (word, bytes) in words.iter_mut().zip(raw.chunks_exact(4)) {
            let mut encoded = [0; 4];
            encoded.copy_from_slice(bytes);
            *word = u32::from_le_bytes(encoded);
        }
        *vertex = ParticleVertex {
            position: [words[0], words[1], words[2]].map(f32::from_bits),
            rhw: f32::from_bits(words[3]),
            color: words[4],
            uv: [words[5], words[6]].map(f32::from_bits),
        };
    }
    Ok(output)
}

fn write_quad(vm: &mut Vm, address: u32, quad: [ParticleVertex; 4]) -> Result<()> {
    let mut encoded = [0; QUAD_BYTES];
    for (target, vertex) in encoded.chunks_exact_mut(VERTEX_BYTES).zip(quad) {
        let words = [
            vertex.position[0].to_bits(), vertex.position[1].to_bits(),
            vertex.position[2].to_bits(), vertex.rhw.to_bits(), vertex.color,
            vertex.uv[0].to_bits(), vertex.uv[1].to_bits(),
        ];
        for (target, word) in target.chunks_exact_mut(4).zip(words) {
            target.copy_from_slice(&word.to_le_bytes());
        }
    }
    vm.write(address, &encoded)
}

fn quiet_nan(bits: u32) -> Option<u32> {
    let is_nan = bits & 0x7f80_0000 == 0x7f80_0000 && bits & 0x007f_ffff != 0;
    is_nan.then_some(bits | 0x0040_0000)
}

fn checked_float(value: f32) -> f32 {
    if value.is_nan() { f32::from_bits(0xffc0_0000) } else { value }
}

fn float_binary(a: f32, b: f32, operation: impl FnOnce(f32, f32) -> f32) -> f32 {
    if let Some(bits) = quiet_nan(a.to_bits()).or_else(|| quiet_nan(b.to_bits())) {
        return f32::from_bits(bits);
    }
    checked_float(operation(a, b))
}

fn add(a: f32, b: f32) -> f32 {
    float_binary(a, b, |a, b| a + b)
}

fn sub(a: f32, b: f32) -> f32 {
    float_binary(a, b, |a, b| a - b)
}

fn mul(a: f32, b: f32) -> f32 {
    float_binary(a, b, |a, b| a * b)
}

/// Native/matrix division follows IEEE-754, independently of the VM's
/// divide-by-zero opcode rule.
fn div(a: f32, b: f32) -> f32 {
    float_binary(a, b, |a, b| a / b)
}

fn max_zero(value: f32) -> f32 {
    if value < 0.0 { 0.0 } else { value }
}

fn math_unary(bits: u32, function: fn(f64) -> f64) -> u32 {
    if let Some(nan) = quiet_nan(bits) {
        return nan;
    }
    checked_float(function(f64::from(f32::from_bits(bits))) as f32).to_bits()
}

fn math_binary(a: u32, b: u32, function: fn(f64, f64) -> f64) -> u32 {
    if let Some(nan) = quiet_nan(a).or_else(|| quiet_nan(b)) {
        return nan;
    }
    let a = f64::from(f32::from_bits(a));
    let b = f64::from(f32::from_bits(b));
    checked_float(function(a, b) as f32).to_bits()
}

fn particle_math(index: u32, first: u32, second: u32) -> u32 {
    match index {
        6 => math_unary(first, f64::sin),
        7 => math_unary(first, f64::cos),
        8 => math_unary(first, f64::tan),
        9 => math_unary(first, f64::asin),
        10 => math_unary(first, f64::acos),
        11 => math_unary(first, f64::atan),
        12 => math_binary(first, second, f64::atan2),
        13 => math_unary(first, f64::ceil),
        14 => {
            let bits = first & 0x7fff_ffff;
            quiet_nan(bits).unwrap_or(bits)
        }
        15 => math_unary(first, f64::floor),
        16 => math_binary(first, second, |a, b| a % b),
        17 => math_unary(first, f64::sqrt),
        18 => math_binary(first, second, f64::powf),
        19 => div(mul(f32::from_bits(first), std::f32::consts::PI), 180.0).to_bits(),
        20 => div(mul(f32::from_bits(first), 180.0), std::f32::consts::PI).to_bits(),
        _ => unreachable!("only particle math indices 6 through 20 are routed here"),
    }
}

fn float_to_i32(value: f32) -> i32 {
    if !(-2_147_483_648.0..2_147_483_648.0).contains(&value) {
        i32::MIN
    } else {
        value.trunc() as i32
    }
}

fn linear_interpolation(values: [f32; 5]) -> f32 {
    let [start, end, first_time, last_time, now] = values;
    if now <= first_time {
        start
    } else if now >= last_time {
        end
    } else {
        let fraction = div(sub(now, first_time), sub(last_time, first_time));
        add(start, mul(sub(end, start), fraction))
    }
}

fn triangle_wave(input: f32, period: f32) -> f32 {
    if let Some(nan) = quiet_nan(input.to_bits()).or_else(|| quiet_nan(period.to_bits())) {
        return f32::from_bits(nan);
    }
    let input = f32::from_bits(input.to_bits() & 0x7fff_ffff);
    let period = f32::from_bits(period.to_bits() & 0x7fff_ffff);
    let remainder = f32::from_bits(math_binary(input.to_bits(), period.to_bits(), |a, b| a % b));
    let slope = sub(mul(div(remainder, period), 4.0), 2.0);
    checked_float(sub(f32::from_bits(slope.to_bits() & 0x7fff_ffff), 1.0))
}

fn matrix_multiply(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    std::array::from_fn(|index| {
        let row = index / 4;
        let column = index % 4;
        let a = mul(left[row * 4], right[column]);
        let b = mul(left[row * 4 + 1], right[column + 4]);
        let c = mul(left[row * 4 + 2], right[column + 8]);
        let d = mul(left[row * 4 + 3], right[column + 12]);
        add(add(add(a, b), c), d)
    })
}

fn transform(vector: [f32; 4], matrix: [f32; 16]) -> [f32; 4] {
    std::array::from_fn(|column| {
        let a = mul(vector[0], matrix[column]);
        let b = mul(vector[1], matrix[column + 4]);
        let c = mul(vector[2], matrix[column + 8]);
        let d = mul(vector[3], matrix[column + 12]);
        add(add(add(a, b), c), d)
    })
}

/// Adjugate/cofactor inversion keeps the original matrix on ordered zero
/// determinant. The reciprocal is refined once from an SSE estimate.
fn matrix_inverse(matrix: [f32; 16]) -> Option<[f32; 16]> {
    let mut cofactors = [0.0; 16];
    for row in 0..4 {
        for column in 0..4 {
            let mut minor = [0.0; 9];
            let mut cursor = 0;
            for source_row in 0..4 {
                if source_row == row {
                    continue;
                }
                for source_column in 0..4 {
                    if source_column != column {
                        minor[cursor] = matrix[source_row * 4 + source_column];
                        cursor += 1;
                    }
                }
            }
            let diagonal = mul(minor[0], sub(mul(minor[4], minor[8]), mul(minor[5], minor[7])));
            let middle = mul(minor[1], sub(mul(minor[3], minor[8]), mul(minor[5], minor[6])));
            let opposite = mul(minor[2], sub(mul(minor[3], minor[7]), mul(minor[4], minor[6])));
            let cofactor = add(sub(diagonal, middle), opposite);
            cofactors[row * 4 + column] = if (row + column) & 1 == 0 {
                cofactor
            } else {
                f32::from_bits(cofactor.to_bits() ^ 0x8000_0000)
            };
        }
    }
    let determinant = add(
        add(mul(matrix[0], cofactors[0]), mul(matrix[1], cofactors[1])),
        add(mul(matrix[2], cofactors[2]), mul(matrix[3], cofactors[3])),
    );
    if determinant == 0.0 {
        return None;
    }
    let inverse = refined_reciprocal(determinant);
    Some(std::array::from_fn(|index| {
        let row = index / 4;
        let column = index % 4;
        mul(cofactors[column * 4 + row], inverse)
    }))
}

fn reciprocal_estimate(value: f32) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        // SSE is part of the x86-64 baseline.
        unsafe {
            use std::arch::x86_64::{_mm_cvtss_f32, _mm_rcp_ss, _mm_set_ss};
            _mm_cvtss_f32(_mm_rcp_ss(_mm_set_ss(value)))
        }
    }
    #[cfg(target_arch = "x86")]
    {
        if std::is_x86_feature_detected!("sse") {
            unsafe {
                use std::arch::x86::{_mm_cvtss_f32, _mm_rcp_ss, _mm_set_ss};
                return _mm_cvtss_f32(_mm_rcp_ss(_mm_set_ss(value)));
            }
        }
        portable_reciprocal_estimate(value)
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        portable_reciprocal_estimate(value)
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn portable_reciprocal_estimate(value: f32) -> f32 {
    let reciprocal = div(1.0, value);
    if !reciprocal.is_normal() {
        return reciprocal;
    }
    let rounded = reciprocal.to_bits().wrapping_add(0x400) & !0x7ff;
    f32::from_bits(rounded)
}

fn refined_reciprocal(value: f32) -> f32 {
    let estimate = reciprocal_estimate(value);
    sub(add(estimate, estimate), mul(mul(value, estimate), estimate))
}

fn project_vertex(
    vertex: ParticleVertex,
    offset: [f32; 4],
    canvas: ParticleCanvas,
    view: &ParticleProjection,
    opacity: u32,
) -> ParticleVertex {
    let center = canvas.viewport.map(|size| (size >> 1) as f32);
    let local = [
        sub(vertex.position[0], offset[0]),
        sub(vertex.position[1], offset[1]),
        sub(vertex.position[2], offset[2]),
        sub(vertex.rhw, offset[3]),
    ];
    let mut local = transform(local, IDENTITY);
    local[0] = mul(sub(local[0], center[0]), view.coordinate_scale[0]);
    local[1] = mul(sub(local[1], center[1]), view.coordinate_scale[1]);
    local = transform(local, view.coordinate_matrix);
    local[2] = sub(FAR_DEPTH, add(local[2], canvas.depth_offset));
    let distance = if local[2] == canvas.camera_depth {
        1.0
    } else {
        sub(local[2], canvas.camera_depth)
    };
    let scale = max_zero(div(FOCAL_LENGTH, distance));
    let mut perspective = [0.0; 16];
    perspective[0] = scale;
    perspective[5] = scale;
    perspective[10] = 1.0 / FAR_DEPTH;
    perspective[15] = div(1.0, distance);
    let mut projected = transform(local, perspective);
    projected[0] = add(projected[0], center[0]);
    projected[1] = add(projected[1], center[1]);
    projected = transform(projected, view.canvas_matrix);
    projected[0] = sub(add(projected[0], view.translation[0]), view.coordinate_offset[0]);
    projected[1] = sub(add(projected[1], view.translation[1]), view.coordinate_offset[1]);
    let alpha = (vertex.color >> 24).wrapping_mul(opacity);
    let color = (vertex.color & 0x00ff_ffff) | (((alpha >> 8) & 255) << 24);
    ParticleVertex {
        position: [projected[0], projected[1], projected[2]],
        rhw: projected[3],
        color,
        uv: vertex.uv,
    }
}
