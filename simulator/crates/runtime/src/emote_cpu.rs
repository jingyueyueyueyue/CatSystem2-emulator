//! Independent PSB motion sampling and CPU texture/mesh rasterization.
//!
//! Strict mode rejects unsupported stencil/skin/controller features. Explicit
//! diagnostic previews return their warnings beside the pixels and never count
//! as native draw success or engine equivalence.

use super::psb::{PsbDocument, PsbValue};
use super::{
    CpuBezierPatch, CpuM2LipBinding, CpuM2LipCurve, CpuM2LipSidecarKind, CpuM2LiveOwner, CpuM2Load,
    CpuM2TimedVariables, CpuMeshGrid, CpuNative907State, CpuOriginalTaskClock, CpuStencilBuffer,
    CpuStencilWrite, CpuVariableTimelineInfo, CpuVariableTimelines, EmoteBackend, EmoteBuffer,
    EmoteCommand, EmoteError, EmoteOperation, EmotePlaybackInfo, EmoteRasterFrame, EmoteReply,
    EmoteResource, EmoteTick, MAX_FRAME_BYTES, Result, decode_emote_texture, emote_layer_affine,
    emote_native_descriptor, m2plane_tint_argb, sample_emote_layer,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock, Weak};

const MAX_TEXTURE_BYTES: usize = 128 * 1024 * 1024;
const MAX_DRAW_LAYERS: usize = 16384;
const MAX_DRAW_TRIANGLES: usize = 2_000_000;

fn unsupported(why: impl Into<String>) -> EmoteError {
    EmoteError::Backend(why.into())
}
fn field<'a>(value: &'a PsbValue, key: &str) -> Result<&'a PsbValue> {
    value
        .get(key)
        .ok_or_else(|| unsupported(format!("PSB missing {key}")))
}
fn num(value: &PsbValue, key: &str, default: f64) -> Result<f64> {
    match value.get(key) {
        None | Some(PsbValue::Null) => Ok(default),
        Some(v) => v
            .number()
            .filter(|n| n.is_finite())
            .ok_or_else(|| unsupported(format!("non-numeric/non-finite {key}"))),
    }
}
fn string<'a>(value: &'a PsbValue, key: &str) -> Option<&'a str> {
    value.get(key).and_then(PsbValue::text)
}
fn numbers(value: Option<&PsbValue>) -> Result<Vec<f64>> {
    match value {
        None | Some(PsbValue::Null) => Ok(Vec::new()),
        Some(PsbValue::List(items)) => items
            .iter()
            .map(|v| {
                v.number()
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| unsupported("non-finite vector"))
            })
            .collect(),
        Some(PsbValue::Integers(items)) => Ok(items.iter().map(|&v| f64::from(v)).collect()),
        _ => Err(unsupported("expected vector/list")),
    }
}

/// Driver3279d..32839 initializes the parameter cursor;33140..331ee
/// recomputes it after a variable write. Discretization truncates the raw
/// variable before clamping, and descending ranges preserve their direction.
fn parameter_cursor(parameter: &PsbValue, value: f64) -> Result<f64> {
    let begin = num(parameter, "rangeBegin", 0.0)? as f32;
    let end = num(parameter, "rangeEnd", 0.0)? as f32;
    let division = num(parameter, "division", 0.0)? as f32;
    let mut value = value as f32;
    if ![begin, end, division, value].into_iter().all(f32::is_finite) {
        return Err(unsupported("parameter cursor outside original finite f32 domain"));
    }
    if begin == end || division <= 0.0 {
        return Ok(0.0);
    }
    let discrete = match parameter.get("discretization") {
        None | Some(PsbValue::Null) | Some(PsbValue::Bool(false)) => false,
        Some(PsbValue::Bool(true)) => true,
        Some(v) => v.number().filter(|v| v.is_finite())
            .ok_or_else(|| unsupported("parameter discretization flag"))? != 0.0,
    };
    if discrete {
        // CVTTSS2SI produces i32::MIN on finite out-of-range operands.
        let integer = if (-2147483648.0..2147483648.0).contains(&value) {
            value.trunc() as i32
        } else {
            i32::MIN
        };
        value = integer as f32;
    }
    let clamped = value.clamp(begin.min(end), begin.max(end));
    let cursor = ((clamped - begin) * division) / (end - begin);
    if !cursor.is_finite() {
        return Err(unsupported("nonfinite original parameter cursor"));
    }
    Ok(cursor as f64)
}

#[derive(Debug, Clone, Copy)]
pub struct CpuViewport {
    pub width: u32,
    pub height: u32,
    pub origin_x: f64,
    pub origin_y: f64,
    pub scale: f64,
    /// Host supplies the original time-to-frame scale; never inferred from PE.
    pub time_units_per_frame: f64,
}

impl CpuViewport {
    pub(super) fn check(self) -> Result<()> {
        let bytes = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|&n| n <= MAX_FRAME_BYTES);
        if self.width == 0
            || self.height == 0
            || bytes.is_none()
            || ![
                self.origin_x,
                self.origin_y,
                self.scale,
                self.time_units_per_frame,
            ]
            .iter()
            .all(|v| v.is_finite())
            || self.scale <= 0.0
            || self.time_units_per_frame <= 0.0
        {
            return Err(unsupported(
                "invalid explicit E-mote CPU viewport/time scale",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CpuTexture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl CpuTexture {
    fn check(&self) -> Result<()> {
        let size = self
            .width
            .checked_mul(self.height)
            .and_then(|n| n.checked_mul(4))
            .filter(|&n| n != 0 && n <= MAX_TEXTURE_BYTES)
            .ok_or(EmoteError::Capacity)?;
        if self.width == 0 || self.height == 0 || self.rgba.len() != size {
            return Err(unsupported("invalid CPU texture storage"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CpuMotionModel {
    pub document: PsbDocument,
    pub textures: BTreeMap<String, CpuTexture>,
    pub base_object: String,
    pub base_motion: String,
}

impl CpuMotionModel {
    pub fn load(resource: &EmoteResource) -> Result<Self> {
        let document = PsbDocument::from_resource(resource)?;
        let spec = string(&document.root, "spec").unwrap_or("");
        if !matches!(spec, "win" | "common") {
            return Err(unsupported(format!(
                "PSB texture platform {spec:?} not decoded"
            )));
        }
        let mut textures = BTreeMap::new();
        let mut total = 0usize;
        let sources = field(&document.root, "source")?
            .object()
            .ok_or_else(|| unsupported("PSB source dictionary"))?;
        for (name, source) in sources {
            let Some(texture) = source.get("texture") else {
                continue;
            };
            let width = dimension(texture, "width")?;
            let height = dimension(texture, "height")?;
            let size = width
                .checked_mul(height)
                .and_then(|v| v.checked_mul(4))
                .ok_or(EmoteError::Capacity)?;
            total = total
                .checked_add(size)
                .filter(|&v| v <= MAX_TEXTURE_BYTES)
                .ok_or(EmoteError::Capacity)?;
            let kind = string(texture, "type").ok_or_else(|| unsupported("texture format"))?;
            let chunk = document.resource(field(texture, "pixel")?)?;
            let rgba = decode_emote_texture(kind, spec, width, height, chunk)?;
            textures.insert(
                name.clone(),
                CpuTexture {
                    width,
                    height,
                    rgba,
                },
            );
        }
        let base = document.root.get("metadata").and_then(|m| m.get("base"));
        let base_object = base
            .and_then(|b| string(b, "chara"))
            .unwrap_or("")
            .to_owned();
        let base_motion = base
            .and_then(|b| string(b, "motion"))
            .unwrap_or("")
            .to_owned();
        if base_object.is_empty() || base_motion.is_empty() {
            return Err(unsupported(
                "PSB metadata.base missing; select an explicit object/motion",
            ));
        }
        let model = Self {
            document,
            textures,
            base_object,
            base_motion,
        };
        // Reject a broken metadata.base before replacing the current player.
        // A successful import must at least select an existing default motion.
        model.motion(&model.base_object, &model.base_motion)?;
        Ok(model)
    }

    pub fn motions(&self) -> Result<Vec<(String, String)>> {
        let objects = field(&self.document.root, "object")?
            .object()
            .ok_or_else(|| unsupported("PSB object map"))?;
        let mut list = Vec::new();
        for (name, object) in objects {
            if let Some(motions) = object.get("motion").and_then(PsbValue::object) {
                list.extend(motions.keys().map(|motion| (name.clone(), motion.clone())));
            }
        }
        Ok(list)
    }

    pub fn stencil_bindings(
        &self,
        object: &str,
        motion: &str,
    ) -> Result<Vec<super::CpuStencilBinding>> {
        super::resolve_emote_stencil_bindings(self.motion(object, motion)?)
    }

    fn motion(&self, object: &str, motion: &str) -> Result<&PsbValue> {
        self.document
            .root
            .get("object")
            .and_then(|v| v.get(object))
            .and_then(|v| v.get("motion"))
            .and_then(|v| v.get(motion))
            .ok_or_else(|| unsupported(format!("PSB motion {object}/{motion} is absent")))
    }
}

fn dimension(value: &PsbValue, key: &str) -> Result<usize> {
    let number = num(value, key, 0.0)?;
    if number < 1.0 || number.fract() != 0.0 || number > 16384.0 {
        return Err(unsupported("invalid PSB texture dimension"));
    }
    Ok(number as usize)
}

/// Pixel-aligned RL stream: literal command n emits n+1 pixels; high-bit command
/// n emits one pixel n+3 times. Exact size/stream consumption is mandatory.
pub fn decode_rgba_rle(bytes: &[u8], expected: usize) -> Result<Vec<u8>> {
    if expected > MAX_TEXTURE_BYTES || expected % 4 != 0 {
        return Err(EmoteError::Capacity);
    }
    let mut output = Vec::with_capacity(expected);
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let code = bytes[cursor];
        cursor += 1;
        let count = if code & 128 != 0 {
            usize::from(code & 127) + 3
        } else {
            usize::from(code) + 1
        };
        let emitted = count.checked_mul(4).ok_or(EmoteError::Capacity)?;
        if output
            .len()
            .checked_add(emitted)
            .filter(|&n| n <= expected)
            .is_none()
        {
            return Err(unsupported("RGBA RL expands beyond declared texture size"));
        }
        let take = if code & 128 != 0 { 4 } else { emitted };
        let end = cursor
            .checked_add(take)
            .filter(|&n| n <= bytes.len())
            .ok_or_else(|| unsupported("truncated RGBA RL stream"))?;
        let data = &bytes[cursor..end];
        if code & 128 != 0 {
            for _ in 0..count {
                output.extend_from_slice(data);
            }
        } else {
            output.extend_from_slice(data);
        }
        cursor = end;
    }
    if output.len() != expected {
        return Err(unsupported("RGBA RL output length mismatch"));
    }
    Ok(output)
}

#[derive(Debug, Clone)]
pub struct CpuPlayer {
    pub model: Arc<CpuMotionModel>,
    pub object: String,
    pub motion: String,
    pub frame: f64,
    pub variables: BTreeMap<String, f64>,
    pub enabled: bool,
    pub controller_a_raw: u32,
    pub controller_b_raw: u32,
    /// Original driver default1.0; host may propagate the actual quality setter.
    pub mesh_quality_scale: f32,
    root_affine: [f64; 6],
    driver_transform: Option<super::CpuDriverTransform>,
    variable_timelines: CpuVariableTimelines,
    m2_timed_variables: CpuM2TimedVariables,
    ///Parser-created reference clock owners, separate from M2 controllers.
    ///Deep Clone preserves the player's existing transactional update model.
    reference_timelines: super::timeline::CpuReferenceTimelines,
}

impl CpuPlayer {
    pub fn new(model: CpuMotionModel) -> Self {
        Self::from_shared_model(Arc::new(model))
    }
    pub fn from_shared_model(model: Arc<CpuMotionModel>) -> Self {
        Self {
            object: model.base_object.clone(),
            motion: model.base_motion.clone(),
            model,
            frame: 0.0,
            variables: BTreeMap::new(),
            enabled: true,
            controller_a_raw: 0,
            controller_b_raw: 0,
            mesh_quality_scale: 1.0,
            root_affine: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            driver_transform: None,
            variable_timelines: CpuVariableTimelines::default(),
            m2_timed_variables: CpuM2TimedVariables::default(),
            reference_timelines: super::timeline::CpuReferenceTimelines::default(),
        }
    }
    pub fn select_motion(&mut self, object: &str, motion: &str, frame: f64) -> Result<()> {
        self.model.motion(object, motion)?;
        if !frame.is_finite() {
            return Err(unsupported("non-finite motion time"));
        }
        let mut next = self.clone();
        next.object = object.to_owned();
        next.motion = motion.to_owned();
        next.frame = frame;
        next.reset_reference_timelines();
        next.update_reference_timelines()?;
        *self = next;
        Ok(())
    }
    pub fn set_variable(&mut self, name: &str, value: f64) -> Result<()> {
        if name.len() > 4095 || !value.is_finite() {
            return Err(unsupported("invalid motion variable"));
        }
        if let Some(previous) = self.variables.get_mut(name) {
            *previous = value;
        } else {
            self.variables.insert(name.to_owned(), value);
        }
        self.m2_timed_variables.cancel(name);
        Ok(())
    }
    /// Bind the actual host object's fully resolved affine transform. Profile
    /// selection/parser values alone are not silently promoted to this matrix.
    pub fn set_root_affine(&mut self, affine: [f64; 6]) -> Result<()> {
        if affine.iter().any(|n| !n.is_finite()) {
            return Err(unsupported("non-finite host object transform"));
        }
        self.root_affine = affine;
        Ok(())
    }

    pub fn timeline_labels(&self) -> Result<Vec<String>> {
        CpuVariableTimelines::labels(&self.model.document.root)
    }
    pub fn active_timelines(&self) -> Vec<CpuVariableTimelineInfo> {
        self.variable_timelines.active()
    }
    pub fn is_timeline_playing(&self, label: &str) -> bool {
        self.variable_timelines.is_playing(label)
    }
    /// Evaluates actual additive tracks; unrecovered instant targets
    /// fail before replacing any currently active timeline.
    pub fn play_timeline(&mut self, label: &str, flags: u32) -> Result<()> {
        let mut next = self.clone();
        next.variable_timelines
            .play(&self.model.document.root, label, flags)?;
        next.consume_controller_targets()?;
        *self = next;
        Ok(())
    }
    fn consume_controller_targets(&mut self) -> Result<()> {
        for (name, value) in self.variable_timelines.take_immediate_writes() {
            self.m2_timed_variables.cancel(&name);
            self.variables.insert(name, f64::from(value));
        }
        let targets = self.variable_timelines.take_controller_targets();
        if !targets.is_empty() {
            let current = self.controller_variables()?;
            self.m2_timed_variables.submit_targets(
                &self.model.document.root,
                &current,
                &mut self.variables,
                &targets,
            )?;
        }
        Ok(())
    }
    pub fn stop_timeline(&mut self, label: &str) {
        self.variable_timelines.stop(label);
    }
    pub fn effective_variables(&self) -> Result<BTreeMap<String, f64>> {
        let mut values = self.variable_timelines
            .evaluate(&self.controller_variables()?)?;
        //13530 ->1c930: clamp the evaluated driver values after additive
        //timeline/offset lanes; do not feed these outputs back into owners.
        super::clamp::apply(&self.model.document.root, &mut values)?;
        Ok(values)
    }
    fn controller_variables(&self) -> Result<BTreeMap<String, f64>> {
        let mut variables = self.variables.clone();
        super::evaluate_emote_immediate_controls(&self.model.document.root, &mut variables)?;
        super::timeline::apply_mouth_outputs(&self.model.document.root, &mut variables)?;
        super::timeline::apply_transition_outputs(&self.model.document.root, &mut variables)?;
        self.m2_timed_variables
            .apply_selector_outputs(&self.variables, &mut variables);
        //Selector options are real controller base values. Driver20170 adds
        //active difference tracks after controller outputs; do not overwrite
        //a genuine fade option difference with its selector base afterward.
        Ok(variables)
    }
    pub fn advance_frames(&mut self, delta: f32) -> Result<()> {
        let frame = self.frame as f32 + delta;
        if !frame.is_finite() {
            return Err(unsupported("motion time overflow"));
        }
        let mut next = self.clone();
        next.variable_timelines.advance(delta)?;
        next.consume_controller_targets()?;
        next.m2_timed_variables
            .advance(delta, &mut next.variables)?;
        next.frame = f64::from(frame);
        *self = next;
        Ok(())
    }

    fn consume_m2_values(&mut self, owner: &mut CpuM2LiveOwner) -> Result<()> {
        let engine_zero = owner.state.engine_enable_10 == Some(0);
        self.consume_m2_values_with_mode(owner, engine_zero)
    }

    fn consume_m2_values_with_mode(&mut self, owner: &mut CpuM2LiveOwner,
        engine_zero: bool) -> Result<()> {
        let current = self.controller_variables()?;
        let (timeline_busy, names) = self.variable_timelines.owner_busy_inputs();
        owner.value_table.begin_update(
            &self.model.document.root, &current, &mut self.variables,
            &mut self.m2_timed_variables, engine_zero,
            timeline_busy, &names,
        )
    }

    fn driver_busy(&self) -> bool {
        let (nonloop_busy, names) = self.variable_timelines.owner_busy_inputs();
        nonloop_busy || self.m2_timed_variables.has_active_except(&names)
    }

    ///Virtualf4 (3520->12db0), then virtualfc(1.0), without the M2 clock.
    fn finish_pending(&mut self, owner: &mut CpuM2LiveOwner) -> Result<()> {
        self.consume_m2_values(owner)?;
        self.variable_timelines.finish_all()?;
        self.consume_controller_targets()?;
        self.m2_timed_variables.finish_all(&mut self.variables);
        self.advance_frames(1.0)?;
        self.consume_m2_values(owner)?;
        owner.value_table.clear_chains();
        owner.pending_timed.clear();
        self.consume_m2_values(owner)?;
        self.update_reference_timelines()
    }

    ///Render-side sampling hook for every completed player tick, including
    ///ticks with no draw. Call after actual variables/frame have been updated.
    ///It builds no pixels and runs no M2 controller a second time. The host's
    ///advance integration owns this call; draw does not invent missed ticks.
    pub fn update_reference_timelines(&mut self) -> Result<()> {
        let variables = self.effective_variables()?;
        let mut references = self.reference_timelines.clone();
        references.begin(&self.object, &self.motion);
        let mut context = RenderContext {
            player: self, variables: &variables,
            viewport: CpuViewport { width: 1, height: 1, origin_x: 0.0, origin_y: 0.0,
                                    scale: 1.0, time_units_per_frame: 1.0 },
            pixels: Vec::new(), warnings: BTreeSet::new(), layers: 0, triangles: 0,
            sampled_pixels: 0, active: BTreeSet::new(), references,
            reference_path: Vec::new(), reference_update: true,
            reference_raw_time: self.frame,
        };
        if self.enabled {
            context.collect_motion(&self.object, &self.motion, self.frame,
                Transform { m: self.root_affine, opacity: 255, z: 0.0 },
                &[], 0, &[], &mut Vec::new())?;
        }
        context.references.finish();
        context.references.mark_tick(self.frame as f32, &variables);
        self.reference_timelines = context.references;
        Ok(())
    }

    ///Use when the host actually rebuilds/seeks the root player's layer tree.
    ///An ordinary reference's same-name selection must not call this reset.
    pub fn reset_reference_timelines(&mut self) {
        self.reference_timelines = super::timeline::CpuReferenceTimelines::default();
    }

    pub fn render_preview(&self, viewport: CpuViewport) -> Result<DiagnosticFrame> {
        viewport.check()?;
        let variables = self.effective_variables()?;
        self.render_resolved(viewport, &variables, Vec::new())
    }

    fn render_resolved(
        &self, viewport: CpuViewport, variables: &BTreeMap<String, f64>, mut pixels: Vec<u8>,
    ) -> Result<DiagnosticFrame> {
        let mut references = self.reference_timelines.clone();
        references.begin(&self.object, &self.motion);
        let byte_count = viewport.width as usize * viewport.height as usize * 4;
        pixels.clear();
        pixels.reserve_exact(byte_count);
        pixels.resize(byte_count, 0);
        let mut context = RenderContext {
            player: self,
            variables,
            viewport,
            pixels,
            warnings: BTreeSet::new(),
            layers: 0,
            triangles: 0,
            sampled_pixels: 0,
            active: BTreeSet::new(),
            references,
            reference_path: Vec::new(),
            reference_update: false,
            reference_raw_time: self.frame,
        };
        // A static initial pose cannot stand in for an enabled controller,
        // including on models that happen not to contain stencil layers.
        if let Some(metadata) = self.model.document.root.get("metadata") {
            for kind in [
                "bustControl",
                "hairControl",
                "loopControl",
                "mirrorControl",
                "partsControl",
                "stereovisionControl",
            ] {
                if metadata
                    .get(kind)
                    .and_then(PsbValue::list)
                    .is_some_and(|entries| {
                        entries.iter().any(|entry| {
                            entry.get("enabled").and_then(PsbValue::number) != Some(0.0)
                        })
                    })
                {
                    context.warnings.insert(format!(
                        "full metadata {kind} controller evaluation is not recovered"
                    ));
                }
            }
        }
        context.warnings.extend(super::m2selector::graph_render_diagnostics(
            &self.model.document.root,
        )?);
        if self.enabled {
            context.motion(
                &self.object,
                &self.motion,
                self.frame,
                Transform {
                    m: self.root_affine,
                    opacity: 255,
                    z: 0.0,
                },
                &[],
                0,
            )?;
        }
        //Draws sample a private copy. Only the explicit per-tick update hook
        //commits visibility/release, so a second color/mask/render pass cannot
        //consume time or destroy holders absent from its primitive schedule.
        let frame = EmoteRasterFrame::new(viewport.width, viewport.height, [0, 0], context.pixels)?;
        Ok(DiagnosticFrame {
            frame,
            warnings: context.warnings.into_iter().collect(),
            layers: context.layers,
            triangles: context.triangles,
            sampled_pixels: context.sampled_pixels,
        })
    }

    pub fn render_strict(&self, viewport: CpuViewport) -> Result<EmoteRasterFrame> {
        self.render_preview(viewport)?.into_strict()
    }
}

pub struct DiagnosticFrame {
    pub frame: EmoteRasterFrame,
    /// Nonempty warnings forbid treating this preview as successful native draw.
    pub warnings: Vec<String>,
    pub layers: usize,
    pub triangles: usize,
    pub sampled_pixels: usize,
}

impl DiagnosticFrame {
    pub fn into_strict(self) -> Result<EmoteRasterFrame> {
        check_strict_warnings(&self.warnings)?;
        Ok(self.frame)
    }
}

fn check_strict_warnings(warnings: &[String]) -> Result<()> {
    if warnings.is_empty() {
        Ok(())
    } else {
        Err(unsupported(format!("E-mote CPU incomplete: {}", warnings.join("; "))))
    }
}

#[derive(Clone, Copy)]
struct Transform {
    m: [f64; 6],
    ///Native DWORD284 holds a byte result after every hierarchy multiply.
    opacity: u8,
    /// Default XY coordinate mode carries f32 depth independently of XY.
    z: f32,
}
impl Transform {
    fn identity() -> Self {
        Self {
            m: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            opacity: 255,
            z: 0.0,
        }
    }
    fn point(self, p: [f64; 2]) -> [f64; 2] {
        [
            self.m[0] * p[0] + self.m[2] * p[1] + self.m[4],
            self.m[1] * p[0] + self.m[3] * p[1] + self.m[5],
        ]
    }
    fn then(self, child: Self) -> Self {
        let a = self.m;
        let b = child.m;
        Self {
            m: [
                a[0] * b[0] + a[2] * b[1],
                a[1] * b[0] + a[3] * b[1],
                a[0] * b[2] + a[2] * b[3],
                a[1] * b[2] + a[3] * b[3],
                a[0] * b[4] + a[2] * b[5] + a[4],
                a[1] * b[4] + a[3] * b[5] + a[5],
            ],
            opacity: (u32::from(self.opacity) * u32::from(child.opacity) / 255) as u8,
            z: self.z + child.z,
        }
    }
}

#[derive(Clone)]
struct Warp {
    transform: Transform,
    width: f64,
    height: f64,
    ox: f64,
    oy: f64,
    patch: CpuBezierPatch,
    division: u32,
    ///3f6ed..3f705: meshSyncChildMask bit8 produces active parent c0.
    ///An own patch can draw without being a warp for its descendants.
    propagate_to_children: bool,
}

/// Effective2c4 chain, outermost first. A c1 barrier is retained even when
///its layer has no active c0 patch; skipped ancestors still move the anchor.
#[derive(Clone)]
struct MeshAncestor {
    owner: usize,
    warp: Option<Warp>,
    stops_geometry: bool,
}

/// Raw ancestor BP entries, nearest first, ending at the first layer whose
/// meshCombine is false. This is distinct from the effective geometry chain:
/// combined ancestors must not be applied a second time as sequential warps.
#[derive(Clone)]
struct PatchAncestor {
    owner: usize,
    raw: CpuBezierPatch,
}
impl Warp {
    fn apply(&self, global: [f64; 2]) -> Result<[f64; 2]> {
        let m = self.transform.m;
        let determinant = m[0] * m[3] - m[1] * m[2];
        if determinant.abs() < 1e-12 {
            return Err(unsupported("singular Bezier patch transform"));
        }
        let x = global[0] - m[4];
        let y = global[1] - m[5];
        let local = [
            (m[3] * x - m[2] * y) / determinant,
            (-m[1] * x + m[0] * y) / determinant,
        ];
        let u = (local[0] + self.ox) / self.width;
        let v = (local[1] + self.oy) / self.height;
        let p = self.patch.evaluate(u, v)?;
        Ok(self
            .transform
            .point([p[0] * self.width - self.ox, p[1] * self.height - self.oy]))
    }
}

///3ffdd..401c0: deform geometry and its undecorated anchor until c1 stops
///the parent walk. Remaining active ancestors deform only that anchor; add
///their resulting delta uniformly to all vertices (401d2..4020f). The own
///patch deforms geometry first, but not the anchor initialized at3f90e/3f92d.
fn mesh_raster_plan(
    chain: &[MeshAncestor],
    mesh_owner: usize,
    mut anchor: [f64; 2],
) -> Result<(Vec<&Warp>, [f64; 2])> {
    let mut geometry = Vec::new();
    let mut stopped_anchor = None;
    for ancestor in chain.iter().rev() {
        if let Some(warp) = &ancestor.warp {
            if stopped_anchor.is_none() {
                geometry.push(warp);
            }
            if ancestor.owner != mesh_owner {
                anchor = warp.apply(anchor)?;
            }
        }
        if ancestor.stops_geometry && stopped_anchor.is_none() {
            stopped_anchor = Some(anchor);
        }
    }
    let delta = stopped_anchor
        .map(|before: [f64; 2]| [anchor[0] - before[0], anchor[1] - before[1]])
        .unwrap_or([0.0, 0.0]);
    if delta.iter().any(|value| !value.is_finite()) {
        return Err(unsupported("non-finite inherited mesh anchor correction"));
    }
    Ok((geometry, delta))
}

struct RenderContext<'a> {
    player: &'a CpuPlayer,
    variables: &'a BTreeMap<String, f64>,
    viewport: CpuViewport,
    pixels: Vec<u8>,
    warnings: BTreeSet<String>,
    layers: usize,
    triangles: usize,
    sampled_pixels: usize,
    active: BTreeSet<(String, String)>,
    references: super::timeline::CpuReferenceTimelines,
    reference_path: Vec<usize>,
    reference_update: bool,
    ///Caller player+100, distinct from sampled+104.41087 uses the raw
    ///clock when selecting a new reference on an unparameterized layer.
    reference_raw_time: f64,
}

struct DrawLayer {
    kind: i32,
    content: PsbValue,
    transform: Transform,
    warps: Vec<MeshAncestor>,
    ///41afd/43cbd copy the referencing layer's effective2c4 into the
    ///referenced motion root. Its own BP/c1 is not part of that pointer.
    motion_parent_warps: Vec<MeshAncestor>,
    mesh_owner: usize,
    time: f64,
    frame_begin: f32,
    reference: Option<super::timeline::CpuReferenceSample>,
    depth: usize,
    mesh_grid: Option<CpuMeshGrid>,
    stencil_owners: Vec<usize>,
}

/// Physical owners are outermost first within each scope. Scopes are linked
///from the nearest motion outward, preserving primitive ownership and geometry.
///402cd/41b12 transfer the caller tail;44b22/44dea make primitive+78 links.
#[derive(Clone, Copy)]
struct StencilScope<'a> {
    owners: &'a [usize],
    pending: &'a [Option<Arc<DrawLayer>>],
    bindings: &'a [super::CpuStencilBinding],
    ranges: &'a OnceLock<Vec<Vec<Arc<DrawLayer>>>>,
}

/// One expanded motion keeps its actual physical-index/source bindings alive
///until the final primitive sort and draw, including across reference roots.
struct MotionPrimitives {
    pending: Vec<Option<Arc<DrawLayer>>>,
    bindings: Vec<super::CpuStencilBinding>,
    ///44b39 captures actual primitive pointers before44f69 replaces each
    ///composite owner's range. Geometry has no back-reference to this scope:
    ///nested ranges share samples without retaining cyclic owner graphs.
    ranges: OnceLock<Vec<Vec<Arc<DrawLayer>>>>,
}

#[derive(Clone)]
struct OwnedStencilScope {
    owners: Vec<usize>,
    motion: Arc<MotionPrimitives>,
}

struct ColorPrimitive {
    index: usize,
    motion: Arc<MotionPrimitives>,
    stencils: Vec<OwnedStencilScope>,
}

impl RenderContext<'_> {
    ///35124 binds a motion's own parameter record to player+2d0;
    ///3abf1..3ac2a consumes that cursor instead of advancing ordinary time.
    ///This is independent of the referencing layer's parameter selection.
    ///Color collection and both composite-mask entry paths share this clock.
    fn motion_frame(&self, motion: &PsbValue, time: f64) -> Result<f64> {
        let params = motion.get("parameter").and_then(PsbValue::list).unwrap_or(&[]);
        if let Some(frame) = self.bound_parameter_clock(motion, params)? {
            return Ok(frame);
        }
        let end = num(motion, "lastTime", 0.0)?;
        let begin = num(motion, "loopTime", -1.0)?;
        if begin >= 0.0 && end > begin && time >= end {
            Ok(begin + (time - begin) % (end - begin))
        } else {
            Ok(time.min(end))
        }
    }

    fn bound_parameter_clock(&self, node: &PsbValue, params: &[PsbValue]) -> Result<Option<f64>> {
        let index = match node.get("parameterize") {
            None | Some(PsbValue::Null) => return Ok(None),
            Some(value) => value.number().ok_or_else(|| unsupported("parameterize integer index"))?,
        };
        if !index.is_finite() || index.fract() != 0.0 || index < 0.0 || index >= params.len() as f64 {
            return Err(unsupported("parameterize index outside motion table"));
        }
        let parameter = &params[index as usize];
        let id = string(parameter, "id").ok_or_else(|| unsupported("parameter id"))?;
        parameter_cursor(parameter, *self.variables.get(id).unwrap_or(&0.0)).map(Some)
    }

    ///Resolve each physical reference once before primitive ordering. Its
    ///captured sample is reused by color and composite-mask draws; neither
    ///pass can restart or advance the holder as a side effect of rasterizing.
    fn bind_references(&mut self, pending: &mut [Option<DrawLayer>]) -> Result<()> {
        for (index, draw) in pending.iter_mut().enumerate() {
            let Some(draw) = draw.as_mut().filter(|draw| draw.kind == 3) else {
                continue;
            };
            let (Some(object), Some(name)) =
                (string(&draw.content, "src"), string(&draw.content, "icon")) else {
                continue;
            };
            let motion = self.player.model.motion(object, name)?;
            let params = motion.get("parameter").and_then(PsbValue::list).unwrap_or(&[]);
            let parameter = self.bound_parameter_clock(motion, params)?.map(|v| v as f32);
            let end = num(motion, "lastTime", 0.0)? as f32;
            let loop_time = num(motion, "loopTime", -1.0)?;
            let settings = draw.content.get("motion");
            let offset = settings.map(|m| num(m, "timeOffset", 0.0)).transpose()?.unwrap_or(0.0) as f32;
            let mask = settings.map(|m| num(m, "mask", 0.0)).transpose()?.unwrap_or(0.0);
            if loop_time >= 0.0 || mask != 0.0 {
                return Err(unsupported("reference loop/nonzero motion flags outside recovered actual resource family"));
            }
            if parameter.is_none() && !self.reference_update
                && !self.references.has_tick(self.player.frame as f32, self.variables)
            {
                self.warnings.insert(
                    "ordinary reference owner requires update_reference_timelines after each player tick".into());
            }
            let mut path = self.reference_path.clone();
            path.push(index);
            let initial = (draw.time as f32 - draw.frame_begin) + offset;
            draw.reference = Some(self.references.select(path, object, name, initial, end,
                self.player.frame as f32, parameter)?);
        }
        Ok(())
    }

    fn motion(
        &mut self,
        object: &str,
        name: &str,
        time: f64,
        parent: Transform,
        warps: &[MeshAncestor],
        depth: usize,
    ) -> Result<()> {
        let mut primitives = Vec::new();
        self.collect_motion(object, name, time, parent, warps, depth, &[], &mut primitives)?;
        //44c13 stores world26c in primitive+24.4810/486a sorts the complete
        //expanded color list with2dd20: ascending f32 depth. Its secondary
        //key is the common player priority (35310/42e04), equal within this
        //CpuPlayer. Keep equal-depth primitive insertion order, as the native
        //stable merge does. Groups themselves emit no textured primitive.
        primitives.sort_by(|a, b| {
            let az = a.motion.pending[a.index].as_ref().unwrap().transform.z;
            let bz = b.motion.pending[b.index].as_ref().unwrap().transform.z;
            az.partial_cmp(&bz).unwrap_or(std::cmp::Ordering::Equal)
        });
        //A frame's owner/source samples are immutable. Reuse the actual
        //linked plane for the same complete chain, instead of resampling
        //every captured motion for every color sprite in that chain.
        let mut planes: BTreeMap<Vec<(usize, usize)>, Arc<(CpuStencilBuffer, u8)>> = BTreeMap::new();
        for primitive in primitives {
            let draw = primitive.motion.pending[primitive.index].as_ref().unwrap();
            let scopes: Vec<_> = primitive.stencils.iter().map(|scope| StencilScope {
                owners: &scope.owners,
                pending: &scope.motion.pending,
                bindings: &scope.motion.bindings,
                ranges: &scope.motion.ranges,
            }).collect();
            let key: Vec<_> = primitive.stencils.iter().flat_map(|scope| {
                let identity = Arc::as_ptr(&scope.motion) as usize;
                scope.owners.iter().rev().map(move |owner| (identity, *owner))
            }).collect();
            let clip = if let Some(plane) = planes.get(&key) {
                Some(plane.clone())
            } else if let Some(plane) = self.literal_stencil_clip(&scopes)? {
                let plane = Arc::new(plane);
                planes.insert(key, plane.clone());
                Some(plane)
            } else {
                None
            };
            let mut pass = SpritePass::Color(
                clip.as_deref().map(|(buffer, reference)| (buffer, *reference)),
            );
            self.sprite(
                &draw.content, draw.transform, &draw.warps, draw.mesh_grid,
                draw.mesh_owner, &mut pass,
            )?;
        }
        Ok(())
    }

    fn collect_motion(
        &mut self,
        object: &str,
        name: &str,
        time: f64,
        parent: Transform,
        warps: &[MeshAncestor],
        depth: usize,
        inherited_stencils: &[OwnedStencilScope],
        primitives: &mut Vec<ColorPrimitive>,
    ) -> Result<Vec<Arc<DrawLayer>>> {
        if depth > 64 || !self.active.insert((object.to_owned(), name.to_owned())) {
            return Err(unsupported("recursive/cyclic PSB motion reference"));
        }
        let motion = self.player.model.motion(object, name)?.clone();
        let frame = self.motion_frame(&motion, time)?;
        let layers = field(&motion, "layer")?
            .list()
            .ok_or_else(|| unsupported("motion layer list"))?;
        let params = motion
            .get("parameter")
            .and_then(PsbValue::list)
            .unwrap_or(&[]);
        //For a parameter-driven motion both+100/+104 equal that cursor;
        //ordinary owners retain the raw clock from the captured reference.
        let previous_raw = self.reference_raw_time;
        self.reference_raw_time = self.bound_parameter_clock(&motion, params)?.unwrap_or(previous_raw);
        // PSB priority indexes the entire preorder layer tree, not only the
        // roots in motion.layer. Transform collection and draw ordering are
        // separate; otherwise sprites may be omitted or children drawn twice.
        let mut pending = Vec::new();
        for layer in layers {
            self.layer(
                layer,
                frame,
                parent,
                parent.opacity,
                warps,
                &[],
                params,
                depth + 1,
                &[],
                &mut pending,
            )?;
        }
        self.bind_references(&mut pending)?;
        let bindings = super::resolve_emote_stencil_bindings(&motion)?;
        let order = if self.reference_update {
            //Native player update visits physical references even if their
            //motion currently emits no primitive in the priority schedule.
            (0..pending.len()).collect()
        } else {
            priority_order(&motion, frame, pending.len())?
        };
        let scope = Arc::new(MotionPrimitives {
            pending: pending.into_iter().map(|draw| draw.map(Arc::new)).collect(),
            bindings,
            ranges: OnceLock::new(),
        });
        let mut ranges = vec![Vec::new(); scope.pending.len()];
        let mut emitted = vec![false; scope.pending.len()];
        let mut captured = Vec::new();
        //449f7/44a33 count the priority list;44a50 reads it from its end.
        //Reference expansion stays in that insertion order before the one
        //global stable depth sort; do not sort each nested motion separately.
        for index in order.into_iter().rev() {
            let Some(draw) = scope.pending[index].as_ref() else {
                continue;
            };
            if !matches!(draw.kind, 0 | 3 | 12) {
                continue;
            }
            emitted[index] = true;
            let mut scopes = Vec::with_capacity(inherited_stencils.len() + 1);
            if !draw.stencil_owners.is_empty() {
                scopes.push(OwnedStencilScope {
                    owners: draw.stencil_owners.clone(),
                    motion: scope.clone(),
                });
            }
            scopes.extend_from_slice(inherited_stencils);
            if draw.kind == 3 {
                //41b12/43cd2 copy the caller's2d4 tail, not the caller
                //itself, into the referenced root.44b22 gives the captured
                //group primitive that same tail. Being an active owner does
                //not add this group to its referenced content's color chain;
                //its physical children below still receive it in layer().
                if let Some(reference) = &draw.reference {
                    let previous = std::mem::replace(&mut self.reference_path, reference.path.clone());
                    let caller_raw = std::mem::replace(&mut self.reference_raw_time, reference.raw_frame);
                    let result = self.collect_motion(
                        &reference.object,
                        &reference.motion,
                        reference.frame,
                        draw.transform,
                        &draw.motion_parent_warps,
                        draw.depth + 1,
                        &scopes,
                        primitives,
                    );
                    self.reference_path = previous;
                    self.reference_raw_time = caller_raw;
                    let range = result?;
                    captured.extend(range.iter().cloned());
                    ranges[index] = range;
                }
            } else if !self.reference_update {
                //44e2a appends the actual primitive to a caller's capture,
                //including blank/type12 entries.50d0 filters nontextured
                //entries at consumption; it does not invent their bounds.
                ranges[index].push(draw.clone());
                captured.push(draw.clone());
                if draw.kind == 0 && string(&draw.content, "src") != Some("blank") {
                    primitives.push(ColorPrimitive { index, motion: scope.clone(), stencils: scopes });
                }
            }
        }
        if !self.reference_update {
            //44f30 visits composite owners after reference capture.44f69
            //clears the SAME range, inserts its own primitive, then4501b
            //copies each source group's CURRENT range. Preserve source order
            //and duplicates, including any earlier owner rebuild; never
            //recursively evaluate a source motion again during a mask pass.
            for binding in &scope.bindings {
                if binding.stencil_type & 4 == 0 || !emitted[binding.owner_index] {
                    continue;
                }
                let owner = scope.pending[binding.owner_index].as_ref()
                    .ok_or_else(|| unsupported("emitted composite owner has no primitive"))?;
                ranges[binding.owner_index] = vec![owner.clone()];
                for (ordinal, &source) in binding.source_indices.iter().enumerate() {
                    //The resolver includes a sprite owner's own entry first;
                    //44f69 already inserted that primitive once above.
                    if ordinal == 0 && source == binding.owner_index && owner.kind == 0 {
                        continue;
                    }
                    if !emitted[source] {
                        continue;
                    }
                    let Some(draw) = scope.pending[source].as_ref() else {
                        continue;
                    };
                    if draw.kind == 3 {
                        let shared = ranges[source].clone();
                        ranges[binding.owner_index].extend(shared);
                    } else if draw.kind == 0 {
                        ranges[binding.owner_index].push(draw.clone());
                    }
                }
            }
            //44e2a/44b58 append to the caller capture in primitive insertion
            //order. Only the independent global COLOR list is depth-sorted.
        }
        scope.ranges.set(ranges)
            .map_err(|_| unsupported("motion primitive ranges already published"))?;
        self.active.remove(&(object.to_owned(), name.to_owned()));
        self.reference_raw_time = previous_raw;
        Ok(captured)
    }

    fn layer(
        &mut self,
        layer: &PsbValue,
        time: f64,
        parent: Transform,
        motion_opacity: u8,
        warps: &[MeshAncestor],
        mesh_chain: &[PatchAncestor],
        params: &[PsbValue],
        depth: usize,
        stencil_owners: &[usize],
        pending: &mut Vec<Option<DrawLayer>>,
    ) -> Result<()> {
        let preorder_index = pending.len();
        pending.push(None);
        self.layers += 1;
        if self.layers > MAX_DRAW_LAYERS || depth > 128 {
            return Err(unsupported("CPU motion layer budget"));
        }
        let kind = num(layer, "type", 2.0)? as i32;
        // 44b71..44b7d admits only0/10/12 to the primitive path (3 has
        // its own reference path). Actual NEKO type1 shape/anchor markers
        // are leaves: they occupy a physical/priority index but emit no
        // color or stencil geometry. Do not substitute their shape bounds.
        if kind == 1 && layer.get("children").and_then(PsbValue::list).is_none_or(|v| v.is_empty()) {
            return Ok(());
        }
        if !matches!(kind, 0 | 2 | 3 | 12) {
            self.warnings
                .insert(format!("layer type {kind} is not recovered"));
            hide_children(layer, pending)?;
            return Ok(());
        }
        if kind == 3 && num(layer, "motionIndependentLayerInherit", 0.0)? != 0.0 {
            self.warnings.insert("independent reference layer inheritance is not recovered".into());
        }
        if num(layer, "coordinate", 0.0)? != 0.0 {
            self.warnings
                .insert("nonlocal coordinate mode is not recovered".into());
        }
        let parameter_time = self.bound_parameter_clock(layer, params)?;
        let at = parameter_time.unwrap_or(time);
        let sample = sample_emote_layer(layer, at)?;
        self.warnings.extend(sample.warnings);
        let Some(mut content) = sample.content else {
            hide_children(layer, pending)?;
            return Ok(());
        };
        if kind == 12 && string(&content, "src") != Some("blank") {
            self.warnings.insert(
                "textured type12 target/reversal state is not recovered".into(),
            );
        }
        if depth == 1 {
            if let Some(transform) = self.player.driver_transform {
                transform.apply(&mut content)?;
            }
        }
        let coord = numbers(content.get("coord"))?;
        let opacity = super::timeline::frame_opacity(&content)?;
        let local = Transform {
            m: emote_layer_affine(layer, &content)?,
            opacity,
            //3eaea..3eb84: in XY mode, parent affine transforms X/Y;
            //Z is added independently and later orders color primitives.
            z: coord.get(2).copied().unwrap_or(0.0) as f32,
        };
        let mut transform = parent.then(local);
        //3eb9b..3ebd0: bit400 selects the effective physical parent's
        //DWORD284; without it the normal motion uses its implicit root,
        //not that physical parent. Nested references copy their caller's
        //effective opacity into that root at41929..41953.
        let inherit_mask = num(layer, "inheritMask", 0x020007fc_u32 as f64)?;
        if inherit_mask.fract() != 0.0 || !(0.0..=u32::MAX as f64).contains(&inherit_mask) {
            return Err(unsupported("invalid layer inheritMask DWORD"));
        }
        if inherit_mask as u32 & 0x400 == 0 {
            transform.opacity = (u32::from(motion_opacity) * u32::from(opacity) / 255) as u8;
        }
        if !transform.z.is_finite() {
            return Err(unsupported("non-finite original f32 layer depth"));
        }
        let mut own_warp = None;
        let mut own_raw = None;
        let mesh_sync = num(layer, "meshSyncChildMask", 0.0)?;
        if mesh_sync.fract() != 0.0 || !(0.0..=u32::MAX as f64).contains(&mesh_sync) {
            return Err(unsupported("invalid meshSyncChildMask DWORD"));
        }
        let mesh_sync = mesh_sync as u32;
        if mesh_sync & !8 != 0 {
            self.warnings
                .insert("meshSyncChildMask bits other than8 remain unrecovered".into());
        }
        let mesh_combine =
            num(layer, "meshTransform", 0.0)? != 0.0 && num(layer, "meshCombine", 0.0)? != 0.0;
        let inherited_mesh = layer
            .get("inheritMask")
            .and_then(PsbValue::number)
            .map(|mask| mask as u32 & 0x02000000 != 0)
            .unwrap_or(true);
        let stops_geometry = !inherited_mesh && !warps.is_empty();
        if let Some(mesh) = content.get("mesh") {
            let points = numbers(mesh.get("bp"))?;
            let coefficients = numbers(mesh.get("cc"))?;
            if !coefficients.is_empty() {
                self.warnings
                    .insert("mesh BP interpolation curve cc is not recovered".into());
            }
            if !points.is_empty() {
                if points.len() != 32 {
                    self.warnings
                        .insert(format!("mesh bp length{} unsupported", points.len()));
                } else {
                    let dims = self.mesh_dimensions(&content)?;
                    let raw = CpuBezierPatch::from_interleaved(&points)?;
                    let mut patch = raw.clone();
                    if mesh_combine {
                        //3f876 copies own raw BP;3f880..3f8ca adds each
                        //ancestor's raw1c BP through2bdb0, stopping after the
                        //first ancestor with meshCombine==0, active or not.
                        for ancestor in mesh_chain {
                            patch.combine_offsets(&ancestor.raw)?;
                        }
                    }
                    own_raw = Some(raw);
                    own_warp = Some(Warp {
                        transform,
                        width: dims[0],
                        height: dims[1],
                        ox: dims[2],
                        oy: dims[3],
                        patch,
                        division: mesh_division(layer)?,
                        propagate_to_children: mesh_sync & 8 != 0,
                    });
                    if num(layer, "meshTransform", 0.0)? != 1.0 {
                        self.warnings
                            .insert("BP with non-one meshTransform is not recovered".into());
                    }
                }
            }
        }
        let mut draw_warps = warps.to_vec();
        if own_warp.is_some() {
            if mesh_combine {
                //3f889..3f897 rewires2c4 past the consumed active ancestors.
                //Their raw offsets are already in this patch.
                draw_warps.retain(|parent| {
                    parent.warp.is_none() || !mesh_chain.iter().any(|a| a.owner == parent.owner)
                });
            }
        }
        //The new motion has an independent physical/raw BP tree. The
        //driver transfers only the current effective parent pointer2c4,
        //after any consumed-parent rewiring, before adding this layer's
        //own BP/c1. Keep original owner identities and world transforms.
        let motion_parent_warps = draw_warps.clone();
        //402e2 tests sampled_frame+10 (hidden), at layer+b0+index*d4+10.
        //It does not test layer+2c0 (the propagated mesh activity flag).
        //Hidden samples returned above; a visible BP owner remains eligible
        //and its real deformed primitive feeds the same stencil schedule.
        if own_warp.is_some() || stops_geometry {
            draw_warps.push(MeshAncestor {
                owner: self.layers,
                warp: own_warp.clone(),
                stops_geometry,
            });
        }
        let mut child_mesh_chain = if mesh_combine {
            mesh_chain.to_vec()
        } else {
            Vec::new()
        };
        if let Some(raw) = own_raw {
            child_mesh_chain.insert(
                0,
                PatchAncestor {
                    owner: self.layers,
                    raw,
                },
            );
        }
        let mut mesh_grid = None;
        if kind == 0
            && string(&content, "src") != Some("blank")
            && draw_warps.iter().any(|ancestor| ancestor.warp.is_some())
        {
            if num(layer, "meshTransform", 0.0)? == 1.0 {
                let dims = self.mesh_dimensions(&content)?;
                let division = num(layer, "meshDivision", -1.0)?;
                if division < 0.0
                    || division.fract() != 0.0
                    || division > 65535.0
                    || dims[0].fract() != 0.0
                    || dims[1].fract() != 0.0
                {
                    return Err(unsupported("invalid original meshDivision/icon dimensions"));
                }
                mesh_grid = Some(CpuMeshGrid::from_division(
                    dims[0] as u32,
                    dims[1] as u32,
                    division as u32,
                    self.player.mesh_quality_scale,
                )?);
            } else {
                let ancestor = draw_warps
                    .iter()
                    .rev()
                    .find_map(|ancestor| ancestor.warp.as_ref())
                    .ok_or_else(|| unsupported("missing active mesh ancestor"))?;
                let dims = self.mesh_dimensions(&content)?;
                if [dims[0], dims[1], ancestor.width, ancestor.height]
                    .iter()
                    .any(|v| v.fract() != 0.0 || *v > u32::MAX as f64)
                {
                    return Err(unsupported("invalid inherited mesh integer dimensions"));
                }
                mesh_grid = Some(CpuMeshGrid::from_inherited_division(
                    dims[0] as u32,
                    dims[1] as u32,
                    ancestor.width as u32,
                    ancestor.height as u32,
                    ancestor.division,
                    self.player.mesh_quality_scale,
                )?);
            }
        }
        pending[preorder_index] = Some(DrawLayer {
            kind,
            content,
            transform,
            warps: draw_warps.clone(),
            motion_parent_warps,
            mesh_owner: self.layers,
            //40f82 uses a layer's parameter+28;40f87..40f8f otherwise
            //selects its player raw+100, not sampled/clamped+104.
            time: parameter_time.unwrap_or(self.reference_raw_time),
            frame_begin: sample.frame_begin.ok_or_else(|| unsupported("visible reference sample has no key start"))?,
            reference: None,
            depth,
            mesh_grid,
            stencil_owners: stencil_owners.to_vec(),
        });
        let mut child_stencils = stencil_owners.to_vec();
        if num(layer, "stencilType", 0.0)? != 0.0 {
            child_stencils.push(preorder_index);
        }
        //3f5db selects a physical parent only when its c0/c1 is active;
        //otherwise descendants use that parent's effective2c4 chain. Keep
        //the own patch for drawing, but pass it on only with recovered bit8.
        let mut child_warps = draw_warps;
        if let Some(own) = child_warps
            .last_mut()
            .filter(|entry| entry.owner == self.layers)
        {
            if own
                .warp
                .as_ref()
                .is_some_and(|warp| !warp.propagate_to_children)
            {
                own.warp = None;
            }
            if own.warp.is_none() && !own.stops_geometry {
                child_warps.pop();
            }
        }
        if let Some(children) = layer.get("children").and_then(PsbValue::list) {
            //3e828..3e850 skips physical parents carrying bit400000 before
            //choosing the opacity parent. Keep this layer's own draw alpha,
            //but pass the earlier effective alpha across such a container.
            let mut child_parent = transform;
            if inherit_mask as u32 & 0x400000 != 0 {
                child_parent.opacity = parent.opacity;
            }
            for child in children {
                self.layer(
                    child,
                    //3d6f8..3d712 selects each layer's own parameter+28
                    //or the motion clock+104. A physical parent's parameter
                    //cursor affects its transform, not a child's clock.
                    time,
                    child_parent,
                    motion_opacity,
                    &child_warps,
                    &child_mesh_chain,
                    params,
                    depth + 1,
                    &child_stencils,
                    pending,
                )?;
            }
        }
        Ok(())
    }

    ///50d0..52b4 consumes captured primitive pointers, ignoring their color
    ///stencil links. Sampling/restarting referenced players in this pass would
    ///lose the source's captured geometry identity and its owner rebuild.
    fn stencil_source_range(
        &mut self,
        draws: &[Arc<DrawLayer>],
        plane: &mut CpuStencilBuffer,
        reference: u8,
        operation: CpuStencilWrite,
    ) -> Result<()> {
        for draw in draws {
            //44f69 includes a group owner's own nontextured primitive;
            //38f75 marks blank sprites nontextured as well. Neither can be
            //turned into an opaque rectangle or another reference expansion.
            if !matches!(draw.kind, 0 | 12) || string(&draw.content, "src") == Some("blank") {
                continue;
            }
            let mut pass = SpritePass::Mask {
                buffer: &mut *plane,
                reference,
                operation,
            };
            self.sprite(
                &draw.content,
                draw.transform,
                &draw.warps,
                draw.mesh_grid,
                draw.mesh_owner,
                &mut pass,
            )?;
        }
        Ok(())
    }

    /// Local and caller type1/2/5/6 owners form one actual linked schedule.
    /// Each scope retains its own per-motion indices and real source geometry.
    fn literal_stencil_clip(
        &mut self,
        scopes: &[StencilScope<'_>],
    ) -> Result<Option<(CpuStencilBuffer, u8)>> {
        let mut selected = Vec::new();
        for scope in scopes {
            for owner in scope.owners.iter().rev() {
                let binding = scope
                    .bindings
                    .iter()
                    .find(|binding| binding.owner_index == *owner)
                    .ok_or_else(|| unsupported("linked stencil owner has no geometry binding"))?;
                selected.push((binding, scope));
            }
        }
        if selected.is_empty() {
            return Ok(None);
        }
        let kinds: Vec<_> = selected
            .iter()
            .map(|(binding, _)| binding.stencil_type)
            .collect();
        let plan = super::stencil::linked_stencil_plan(&kinds)?;
        let mut plane = CpuStencilBuffer::new(self.viewport.width, self.viewport.height)?;
        plane.clear(plan.clear);
        for (owner, reference, operation) in plan.passes {
            let (binding, scope) = selected[owner];
            if binding.stencil_type & 4 != 0 {
                let ranges = scope.ranges.get()
                    .ok_or_else(|| unsupported("composite primitive ranges not published"))?;
                let range = ranges.get(binding.owner_index)
                    .ok_or_else(|| unsupported("composite primitive range index"))?;
                self.stencil_source_range(range, &mut plane, reference, operation)?;
                continue;
            }
            for &source in &binding.source_indices {
                let Some(draw) = scope.pending.get(source).and_then(Option::as_ref) else {
                    continue;
                };
                if draw.kind != 0 {
                    return Err(unsupported("invalid composite-mask source layer kind"));
                }
                if string(&draw.content, "src") == Some("blank") {
                    // 540b skips an own primitive whose kind byte is nonzero.
                    // Keep the linked plane/reference: an empty positive mask
                    // must still reject color; it must not remove the clip.
                    continue;
                }
                let mut pass = SpritePass::Mask {
                    buffer: &mut plane,
                    reference,
                    operation,
                };
                self.sprite(
                    &draw.content,
                    draw.transform,
                    &draw.warps,
                    draw.mesh_grid,
                    draw.mesh_owner,
                    &mut pass,
                )?;
            }
        }
        Ok(Some((plane, plan.color_reference)))
    }

    fn mesh_dimensions(&self, content: &PsbValue) -> Result<[f64; 4]> {
        let source = string(content, "src").ok_or_else(|| unsupported("BP source"))?;
        let name = string(content, "icon").ok_or_else(|| unsupported("BP icon"))?;
        let dims = if source == "blank" {
            // 38f8d/38fa4 use integer conversion for width/height;
            // 38fbb/38fd2 store origins as f32, not unrestricted f64.
            // Malformed CRT prefix-conversion inputs remain strict errors.
            let fields: Vec<_> = name.split(':').collect();
            if fields.len() != 4 {
                return Err(unsupported("blank BP descriptor field count"));
            }
            let width = fields[0].parse::<i32>()
                .map_err(|_| unsupported("blank BP integer width"))?;
            let height = fields[1].parse::<i32>()
                .map_err(|_| unsupported("blank BP integer height"))?;
            let ox = fields[2].parse::<f32>()
                .map_err(|_| unsupported("blank BP float originX"))?;
            let oy = fields[3].parse::<f32>()
                .map_err(|_| unsupported("blank BP float originY"))?;
            [width as f64, height as f64, ox as f64, oy as f64]
        } else {
            let icon = self
                .player
                .model
                .document
                .root
                .get("source")
                .and_then(|v| v.get(source))
                .and_then(|v| v.get("icon"))
                .and_then(|v| v.get(name))
                .ok_or_else(|| unsupported("nonblank BP atlas icon"))?;
            [
                num(icon, "width", 0.0)?,
                num(icon, "height", 0.0)?,
                num(icon, "originX", 0.0)?,
                num(icon, "originY", 0.0)?,
            ]
        };
        if dims.iter().any(|n| !n.is_finite()) || dims[0] <= 0.0 || dims[1] <= 0.0 {
            return Err(unsupported("invalid BP dimensions"));
        }
        Ok(dims)
    }

    fn sprite(
        &mut self,
        content: &PsbValue,
        transform: Transform,
        warps: &[MeshAncestor],
        mesh_grid: Option<CpuMeshGrid>,
        mesh_owner: usize,
        pass: &mut SpritePass<'_>,
    ) -> Result<()> {
        let source = string(content, "src").ok_or_else(|| unsupported("sprite source"))?;
        let name = string(content, "icon").ok_or_else(|| unsupported("sprite icon"))?;
        let src = self
            .player
            .model
            .document
            .root
            .get("source")
            .and_then(|v| v.get(source))
            .ok_or_else(|| unsupported(format!("missing texture source {source}")))?;
        let icon = src
            .get("icon")
            .and_then(|v| v.get(name))
            .ok_or_else(|| unsupported(format!("missing texture icon {source}/{name}")))?;
        let texture = self
            .player
            .model
            .textures
            .get(source)
            .ok_or_else(|| unsupported("missing decoded texture"))?;
        let width = num(icon, "width", 0.0)?;
        let height = num(icon, "height", 0.0)?;
        let ox = num(icon, "originX", 0.0)?;
        let oy = num(icon, "originY", 0.0)?;
        let left = num(icon, "left", 0.0)?;
        let top = num(icon, "top", 0.0)?;
        if width <= 0.0
            || height <= 0.0
            || left < 0.0
            || top < 0.0
            || left + width > texture.width as f64
            || top + height > texture.height as f64
        {
            return Err(unsupported("icon texture rectangle"));
        }
        let is_mask = matches!(&*pass, SpritePass::Mask { .. });
        if !is_mask && num(content, "bm", 0.0)? != 0.0 {
            self.warnings.insert("sprite blend mode unsupported".into());
        }
        // 2c079/2c085 return the other operand for the native neutral
        // DWORD; this is RGB128/alpha255, not RGB255/alpha255. All52
        // explicit colors in ca01_l_body_1 use this identity. Other colors
        // still require inherited/per-corner color ownership and quantizing.
        if !is_mask && num(content, "color", 0x808080ff_u32 as f64)? != 0x808080ff_u32 as f64 {
            self.warnings
                .insert("nonneutral sprite color multiplier unsupported".into());
        }
        //52a1 and55ca submit four opaque-white colors7cbd0 for the mask.
        //Layer color, opacity and color blending do not attenuate texture
        //alpha in this pass; the actual texture still supplies GREATER64.
        let raster_opacity = if is_mask { 1.0 } else { f64::from(transform.opacity) / 255.0 };
        let (geometry, anchor_delta) =
            mesh_raster_plan(warps, mesh_owner, transform.point([0.0, 0.0]))?;
        let grid = if !warps.iter().any(|ancestor| ancestor.warp.is_some()) {
            CpuMeshGrid {
                columns: 1,
                rows: 1,
            }
        } else {
            mesh_grid.ok_or_else(|| unsupported("active mesh has no recovered subdivision grid"))?
        };
        let count = (grid.columns as usize)
            .checked_mul(grid.rows as usize)
            .and_then(|n| n.checked_mul(2))
            .and_then(|n| self.triangles.checked_add(n))
            .filter(|&n| n <= MAX_DRAW_TRIANGLES)
            .ok_or(EmoteError::Capacity)?;
        let _ = count;
        //Native58720/2f080 materializes one shared node grid before emitting
        //row strips. Sample each ancestor warp once per node, then reuse the
        //same XY/UV for adjacent cells in both real color and mask passes.
        let (stride, vertex_count) = grid.raster_layout()?;
        let mut vertices = Vec::with_capacity(vertex_count);
        for y in 0..=grid.rows {
            for x in 0..=grid.columns {
                let u = x as f64 / grid.columns as f64;
                let q = y as f64 / grid.rows as f64;
                let mut point = transform.point([u * width - ox, q * height - oy]);
                for warp in &geometry {
                    point = warp.apply(point)?;
                }
                point[0] += anchor_delta[0];
                point[1] += anchor_delta[1];
                vertices.push([
                    self.viewport.origin_x + point[0] * self.viewport.scale,
                    self.viewport.origin_y + point[1] * self.viewport.scale,
                    left + u * width,
                    top + q * height,
                ]);
            }
        }
        for y in 0..grid.rows {
            for x in 0..grid.columns {
                let first = y as usize * stride + x as usize;
                let v = [vertices[first], vertices[first + 1],
                         vertices[first + stride], vertices[first + stride + 1]];
                raster(
                    &mut self.pixels,
                    self.viewport.width,
                    self.viewport.height,
                    [v[0], v[1], v[2]],
                    texture,
                    raster_opacity,
                    &mut self.sampled_pixels,
                    pass.raster_pass(),
                )?;
                raster(
                    &mut self.pixels,
                    self.viewport.width,
                    self.viewport.height,
                    [v[2], v[1], v[3]],
                    texture,
                    raster_opacity,
                    &mut self.sampled_pixels,
                    pass.raster_pass(),
                )?;
                self.triangles += 2;
            }
        }
        Ok(())
    }
}

fn mesh_division(layer: &PsbValue) -> Result<u32> {
    let division = num(layer, "meshDivision", -1.0)?;
    if division < 0.0 || division.fract() != 0.0 || division > 65535.0 {
        return Err(unsupported("invalid original meshDivision"));
    }
    Ok(division as u32)
}

fn hide_children(layer: &PsbValue, pending: &mut Vec<Option<DrawLayer>>) -> Result<()> {
    fn walk(layer: &PsbValue, pending: &mut Vec<Option<DrawLayer>>, depth: usize) -> Result<()> {
        if pending.len() >= MAX_DRAW_LAYERS || depth > 128 {
            return Err(unsupported("hidden layer budget"));
        }
        pending.push(None);
        if let Some(children) = layer.get("children").and_then(PsbValue::list) {
            for child in children {
                walk(child, pending, depth + 1)?;
            }
        }
        Ok(())
    }
    if let Some(children) = layer.get("children").and_then(PsbValue::list) {
        for child in children {
            walk(child, pending, 0)?;
        }
    }
    Ok(())
}

fn priority_order(motion: &PsbValue, time: f64, count: usize) -> Result<Vec<usize>> {
    let list = motion
        .get("priority")
        .and_then(PsbValue::list)
        .unwrap_or(&[]);
    let selected = list
        .iter()
        //34e42 parses priority/count174.3b617/3b61c and3c54a limit its
        //current cursor to count-2 as well: the terminal null priority is
        //a boundary, not an instruction to erase all color/mask sources.
        .take(if list.len() >= 2 { list.len() - 1 } else { list.len() })
        .filter(|f| num(f, "time", 0.0).is_ok_and(|n| n <= time))
        .last();
    if let Some(content) = selected.and_then(|v| v.get("content")) {
        if matches!(content, PsbValue::Null) {
            return Ok(Vec::new());
        }
        let indexes = numbers(Some(content))?;
        let mut seen = BTreeSet::new();
        let order: Vec<usize> = indexes.into_iter().map(|v| v as usize).collect();
        if order.iter().any(|&i| i >= count || !seen.insert(i)) {
            return Err(unsupported("priority layer indices"));
        }
        return Ok(order);
    }
    Ok((0..count).collect())
}

fn edge(a: [f64; 4], b: [f64; 4], x: f64, y: f64) -> f64 {
    (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
}

enum StencilRaster<'a> {
    Write {
        buffer: &'a mut CpuStencilBuffer,
        reference: u8,
        operation: CpuStencilWrite,
    },
    Test {
        buffer: &'a CpuStencilBuffer,
        reference: u8,
    },
}

enum SpritePass<'a> {
    Color(Option<(&'a CpuStencilBuffer, u8)>),
    Mask {
        buffer: &'a mut CpuStencilBuffer,
        reference: u8,
        operation: CpuStencilWrite,
    },
}

impl SpritePass<'_> {
    fn raster_pass(&mut self) -> Option<StencilRaster<'_>> {
        match self {
            Self::Color(None) => None,
            Self::Color(Some((buffer, reference))) => Some(StencilRaster::Test {
                buffer,
                reference: *reference,
            }),
            Self::Mask {
                buffer,
                reference,
                operation,
            } => Some(StencilRaster::Write {
                buffer,
                reference: *reference,
                operation: *operation,
            }),
        }
    }
}

/// Actual textured CPU mask primitive, sharing the model rasterizer's triangle
/// coverage and bilinear sampler. No intermediate alpha precomposition.
/// Vertex layout is[x,y,atlas_pixel_x,atlas_pixel_y]. This does not schedule PSB
/// masks; callers must supply the recovered reference and operation sequence.
pub fn rasterize_emote_stencil_triangle(
    buffer: &mut CpuStencilBuffer,
    vertices: [[f64; 4]; 3],
    texture: &CpuTexture,
    opacity: f64,
    reference: u8,
    operation: CpuStencilWrite,
) -> Result<usize> {
    let width = buffer.width();
    let height = buffer.height();
    let mut written = 0usize;
    raster(
        &mut [],
        width,
        height,
        vertices,
        texture,
        opacity,
        &mut written,
        Some(StencilRaster::Write {
            buffer,
            reference,
            operation,
        }),
    )?;
    Ok(written)
}

fn raster(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    mut v: [[f64; 4]; 3],
    texture: &CpuTexture,
    opacity: f64,
    samples: &mut usize,
    mut stencil: Option<StencilRaster<'_>>,
) -> Result<()> {
    texture.check()?;
    if !opacity.is_finite() {
        return Err(unsupported("non-finite raster opacity"));
    }
    if v.iter().flatten().any(|n| !n.is_finite()) {
        return Err(unsupported("non-finite mesh vertex"));
    }
    let mut area = edge(v[0], v[1], v[2][0], v[2][1]);
    if area.abs() < 1e-12 {
        return Ok(());
    }
    if area < 0.0 {
        v.swap(1, 2);
        area = -area;
    }
    let xmin = v
        .iter()
        .map(|p| p[0])
        .fold(f64::INFINITY, f64::min)
        .ceil()
        .clamp(0.0, f64::from(width)) as usize;
    let xmax = v
        .iter()
        .map(|p| p[0])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .clamp(0.0, f64::from(width)) as usize;
    let ymin = v
        .iter()
        .map(|p| p[1])
        .fold(f64::INFINITY, f64::min)
        .ceil()
        .clamp(0.0, f64::from(height)) as usize;
    let ymax = v
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .clamp(0.0, f64::from(height)) as usize;
    let top_left = |a: [f64; 4], b: [f64; 4]| b[1] < a[1] || (b[1] == a[1] && b[0] > a[0]);
    let inclusive = [
        top_left(v[1], v[2]),
        top_left(v[2], v[0]),
        top_left(v[0], v[1]),
    ];
    for y in ymin..ymax {
        for x in xmin..xmax {
            let es = [
                edge(v[1], v[2], x as f64, y as f64),
                edge(v[2], v[0], x as f64, y as f64),
                edge(v[0], v[1], x as f64, y as f64),
            ];
            if (0..3).any(|i| es[i] < 0.0 || (es[i] == 0.0 && !inclusive[i])) {
                continue;
            }
            let w = es.map(|n| n / area);
            let tx = (0..3).map(|i| w[i] * v[i][2]).sum::<f64>();
            let ty = (0..3).map(|i| w[i] * v[i][3]).sum::<f64>();
            let color = sample_texture(texture, tx, ty);
            let alpha = (color[3] / 255.0 * opacity).clamp(0.0, 1.0);
            match stencil.as_mut() {
                Some(StencilRaster::Write {
                    buffer,
                    reference,
                    operation,
                }) => {
                    if buffer.write_fragment(
                        x as u32,
                        y as u32,
                        (alpha * 255.0).round() as u8,
                        *reference,
                        *operation,
                    ) {
                        *samples += 1;
                    }
                    continue;
                }
                Some(StencilRaster::Test { buffer, reference }) => {
                    if !buffer.test_equal(x as u32, y as u32, *reference) {
                        continue;
                    }
                }
                None => {}
            }
            let offset = (y * width as usize + x) * 4;
            let destination_alpha = f64::from(pixels[offset + 3]) / 255.0;
            let combined_alpha = alpha + destination_alpha * (1.0 - alpha);
            if combined_alpha == 0.0 {
                continue;
            }
            // EmoteRasterFrame/Canvas exchange straight RGBA. Retaining a
            // premultiplied RGB here would apply alpha twice at submission.
            for c in 0..3 {
                pixels[offset + c] = ((color[c] * alpha
                    + f64::from(pixels[offset + c]) * destination_alpha * (1.0 - alpha))
                    / combined_alpha)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            pixels[offset + 3] = (combined_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
            *samples += 1;
        }
    }
    Ok(())
}
fn sample_texture(texture: &CpuTexture, x: f64, y: f64) -> [f64; 4] {
    let x = (x - 0.5).clamp(0.0, (texture.width - 1) as f64);
    let y = (y - 0.5).clamp(0.0, (texture.height - 1) as f64);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;
    let mut out = [0.0; 4];
    for (px, py, w) in [
        (x0, y0, (1.0 - fx) * (1.0 - fy)),
        ((x0 + 1).min(texture.width - 1), y0, fx * (1.0 - fy)),
        (x0, (y0 + 1).min(texture.height - 1), (1.0 - fx) * fy),
        (
            (x0 + 1).min(texture.width - 1),
            (y0 + 1).min(texture.height - 1),
            fx * fy,
        ),
    ] {
        let off = (py * texture.width + px) * 4;
        for c in 0..4 {
            out[c] += f64::from(texture.rgba[off + c]) * w;
        }
    }
    out
}

/// Host resource resolution remains external. It must return actual .psz/.psb
/// bytes for an exact full load specification, or a diagnostic.
pub trait CpuEmoteResources: Send {
    fn read(&mut self, load_spec: &[u8], mode: u32) -> Result<(Vec<u8>, EmoteResource)>;
}

#[derive(PartialEq, Eq)]
struct RasterPose {
    size: [u32; 2],
    mapping: [u64; 3],
    time: u64,
    enabled: bool,
    mesh_quality: u32,
    affine: [u64; 6],
    driver_transform: Option<[u32; 4]>,
}

impl RasterPose {
    fn new(player: &CpuPlayer, viewport: CpuViewport) -> Self {
        Self {
            size: [viewport.width, viewport.height],
            mapping: [viewport.origin_x, viewport.origin_y, viewport.scale].map(f64::to_bits),
            time: player.frame.to_bits(),
            enabled: player.enabled,
            mesh_quality: player.mesh_quality_scale.to_bits(),
            affine: player.root_affine.map(f64::to_bits),
            driver_transform: player.driver_transform.map(super::CpuDriverTransform::render_bits),
        }
    }
}

struct CachedRaster {
    model: Weak<CpuMotionModel>,
    object: String,
    motion: String,
    pose: RasterPose,
    variables: BTreeMap<String, f64>,
    references: super::timeline::CpuReferenceTimelines,
    frame: EmoteRasterFrame,
}

impl CachedRaster {
    fn matches(&self, player: &CpuPlayer, pose: &RasterPose, variables: &BTreeMap<String, f64>) -> bool {
        std::ptr::eq(self.model.as_ptr(), Arc::as_ptr(&player.model))
            && self.object == player.object
            && self.motion == player.motion
            && self.pose == *pose
            && super::timeline::same_variable_bits(&self.variables, variables)
            && self.references.same_render_state(&player.reference_timelines)
    }
}

struct CpuTask {
    player: Option<CpuPlayer>,
    raster_cache: Option<CachedRaster>,
    resource_name: Vec<u8>,
    resource_generation: u64,
    /// Constructor237d90 explicitly initializes M2Plane+388 toffffffff.
    base_argb: u32,
    /// None means the919 override has not been received. Resource replacement
    /// preserves task color; it does not reset a native field as a side effect.
    color_percentages: Option<[u32; 3]>,
    original_clock: CpuOriginalTaskClock,
    native907: Option<CpuNative907State>,
    m2_owner: Option<CpuM2LiveOwner>,
    m2_profile: Vec<u8>,
    m2_player_name: Vec<u8>,
    m2_timeline: String,
    ///1745d0 stores the raw word;1778f0 consumes it on the next rebuild only.
    texture_filter_raw: u32,
    /// Empty170be0 wrappers still own10c/14c/150. Only requests received
    /// before a driver exists are staged here; live requests use its banks.
    unloaded_control_requests: [Option<u32>; 3],
    ///170e46 constructs+fc before the driver. Keep start/stop fields here
    ///until the first real M2 owner binds the manager's spline config.
    unloaded_spline: Option<super::m2spline::CpuM2SplineOwner>,
    surface: super::surface::CpuM2SurfaceOwner,
}

impl CpuTask {
    fn new() -> Self {
        Self {
            player: None,
            raster_cache: None,
            resource_name: Vec::new(),
            resource_generation: 0,
            base_argb: 0xffffffff,
            color_percentages: None,
            original_clock: CpuOriginalTaskClock::default(),
            native907: None,
            m2_owner: None,
            m2_profile: Vec::new(),
            m2_player_name: Vec::new(),
            m2_timeline: String::new(),
            texture_filter_raw: 0,
            unloaded_control_requests: [None; 3],
            unloaded_spline: None,
            surface: super::surface::CpuM2SurfaceOwner::default(),
        }
    }
    /// Call once when constructing the first real M2 owner, after its config
    /// and variables exist. These are stored wrapper requests, never a fake
    /// player or empty replacement bank. Later rebuilds inherit the live owner.
    fn apply_unloaded_control_requests(
        &self, owner: &mut CpuM2LiveOwner, variables: &mut BTreeMap<String, f64>,
        current: &BTreeMap<String, f64>,
    ) -> Result<()> {
        if let Some(raw) = self.unloaded_control_requests[0] {
            owner.apply_907(raw, variables, current)?;
        }
        if let Some(raw) = self.unloaded_control_requests[1] {
            owner.set_blink_enable(raw, variables);
        }
        if let Some(raw) = self.unloaded_control_requests[2] {
            owner.set_lip_enable(raw, variables);
        }
        Ok(())
    }
    fn apply_spline_request(&mut self, label: Option<&[u8]>) {
        let spline = if let Some(owner) = self.m2_owner.as_mut() {
            &mut owner.spline
        } else if self.player.is_none() {
            //174d40/174d90 require only the registered wrapper's+fc.
            //No config lookup or curve evaluation happens on these setters.
            self.unloaded_spline.get_or_insert_with(Default::default)
        } else {
            //A raw PSB player has no recovered M2 wrapper binding here.
            return;
        };
        if let Some(label) = label { spline.start(label); }
        else { spline.stop(); }
    }
    fn apply_unloaded_spline_request(&self, owner: &mut CpuM2LiveOwner) {
        if let Some(spline) = &self.unloaded_spline {
            owner.spline.inherit_playback_from(spline);
        }
    }
    fn tint_argb(&self) -> u32 {
        match self.color_percentages {
            Some(values) => m2plane_tint_argb(self.base_argb, values),
            None => self.base_argb,
        }
    }
    fn append_owner_warnings(&self, warnings: &mut Vec<String>) {
        if let Some(owner) = &self.m2_owner {
            warnings.extend(owner.pending_effects.iter().cloned());
            warnings.extend(owner.sidecar_diagnostics());
        }
    }

    fn render_model(&mut self, viewport: CpuViewport, scratch: &mut Vec<u8>) -> Result<EmoteRasterFrame> {
        if let Some(frame) = self.surface.frame() {
            return Ok(frame.clone());
        }
        let player = self.player.as_ref().ok_or_else(|| unsupported("E-mote player resource is not loaded"))?;
        viewport.check()?;
        let variables = player.effective_variables()?;
        let pose = RasterPose::new(player, viewport);
        if let Some(cached) = self.raster_cache.as_ref().filter(|cached| cached.matches(player, &pose, &variables)) {
            let mut warnings = Vec::new();
            self.append_owner_warnings(&mut warnings);
            check_strict_warnings(&warnings)?;
            return Ok(cached.frame.clone());
        }
        self.raster_cache = None;
        let mut preview = player.render_resolved(viewport, &variables, std::mem::take(scratch))?;
        self.append_owner_warnings(&mut preview.warnings);
        let frame = preview.into_strict()?;
        self.raster_cache = Some(CachedRaster {
            model: Arc::downgrade(&player.model),
            object: player.object.clone(),
            motion: player.motion.clone(),
            pose,
            variables,
            references: player.reference_timelines.clone(),
            frame: frame.clone(),
        });
        Ok(frame)
    }
    fn surface_enabled(&self) -> bool {
        self.m2_owner.as_ref().is_some_and(|owner| {
            let state = &owner.state;
            state.requested_raw_10c.unwrap_or(1) != 0
                && state.engine_enable_10 != Some(0)
                && state.scene_override_138.is_none_or(|scene|
                    scene.active != 0 && scene.engine_enable != 0)
        })
    }
}

pub struct CpuEmoteBackend {
    pub viewport: CpuViewport,
    players: BTreeMap<u32, CpuTask>,
    resources: Option<Box<dyn CpuEmoteResources>>,
    mesh_quality_scale: f32,
    original_task_clock: bool,
    models: BTreeMap<(Vec<u8>, String, u32), Weak<CpuMotionModel>>,
    raster_scratch: Vec<u8>,
}
impl CpuEmoteBackend {
    pub fn new(viewport: CpuViewport) -> Result<Self> {
        viewport.check()?;
        Ok(Self {
            viewport,
            players: BTreeMap::new(),
            resources: None,
            mesh_quality_scale: 1.0,
            original_task_clock: false,
            models: BTreeMap::new(),
            raster_scratch: Vec::new(),
        })
    }
    /// Byte-proven174040->driver3540->12bb0 clock path. The host passes
    /// task64-task58 in EmoteTick.task_elapsed; increment is not milliseconds.
    /// The first original update advances1 and default motion speed is1.
    pub fn new_original_task_clock(mut viewport: CpuViewport) -> Result<Self> {
        viewport.time_units_per_frame = super::NEKO_ORIGINAL_TASK_UNITS_PER_FRAME;
        let mut backend = Self::new(viewport)?;
        backend.original_task_clock = true;
        Ok(backend)
    }
    pub fn set_resources(&mut self, resources: Box<dyn CpuEmoteResources>) {
        self.resources = Some(resources);
    }
    fn shared_model(
        &mut self,
        name: &[u8],
        resource: &EmoteResource,
        texture_filter_raw: u32,
    ) -> Result<Arc<CpuMotionModel>> {
        let mut digest = crate::sha256::Sha256::default();
        digest.update(resource.bytes());
        let filter = if matches!(texture_filter_raw, 1..=3) { texture_filter_raw } else { 0 };
        let key = (name.to_vec(), digest.finish(), filter);
        if let Some(model) = self.models.get(&key).and_then(Weak::upgrade) {
            return Ok(model);
        }
        self.models.retain(|_, model| model.strong_count() != 0);
        let mut model = CpuMotionModel::load(resource)?;
        for texture in model.textures.values_mut() {
            super::texture::apply_m2_texture_filter(&mut texture.rgba, filter);
        }
        let model = Arc::new(model);
        self.models.insert(key, Arc::downgrade(&model));
        Ok(model)
    }
    /// Mirrors the actual driver quality control; default is the byte-proven1.
    /// Rejects unbounded settings and updates already loaded and future models.
    pub fn set_mesh_quality_scale(&mut self, quality: f32) -> Result<()> {
        if !quality.is_finite() || quality <= 0.0 || quality > 16.0 {
            return Err(unsupported("invalid mesh quality scale"));
        }
        self.mesh_quality_scale = quality;
        for state in self.players.values_mut() {
            if let Some(player) = &mut state.player {
                player.mesh_quality_scale = quality;
            }
        }
        Ok(())
    }
    pub fn player(&self, task: u32) -> Result<&CpuPlayer> {
        self.players
            .get(&task)
            .and_then(|state| state.player.as_ref())
            .ok_or_else(|| unsupported("E-mote player resource is not loaded"))
    }
    pub fn player_mut(&mut self, task: u32) -> Result<&mut CpuPlayer> {
        self.players
            .get_mut(&task)
            .and_then(|state| state.player.as_mut())
            .ok_or_else(|| unsupported("E-mote player resource is not loaded"))
    }

    /// The exact uniform vertex ARGB calculated by native919 and task color.
    pub fn tint_argb(&self, task: u32) -> Result<u32> {
        self.players
            .get(&task)
            .map(CpuTask::tint_argb)
            .ok_or(EmoteError::UnknownTask(task))
    }

    /// Manager surfaces precede the final quad's919/task tint.
    fn model_preview(&self, task: u32) -> Result<DiagnosticFrame> {
        if let Some(frame) = self.players.get(&task).and_then(|t| t.surface.frame()) {
            // The manager presents captured pixels during its hold; it does
            // not traverse the replacement PSB again on each draw. Both
            // endpoints enter the surface owner only through strict capture.
            return Ok(DiagnosticFrame {
                frame: frame.clone(), warnings: Vec::new(),
                layers: 0, triangles: 0, sampled_pixels: 0,
            });
        }
        let mut preview = self.player(task)?.render_preview(self.viewport)?;
        if let Some(state) = self.players.get(&task) {
            state.append_owner_warnings(&mut preview.warnings);
        }
        Ok(preview)
    }

    fn capture_surface(&self, task: u32) -> Result<EmoteRasterFrame> {
        let state = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?;
        if let Some(frame) = state.surface.capture() { return Ok(frame.clone()); }
        self.model_preview(task)?.into_strict()
    }

    ///Manager pre-update17add5/17afe7: finish the existing hold before
    ///capturing a bare545 request, without advancing any driver/owner clock.
    ///The task updater must call this in place of its direct surface.hold.
    fn prepare_m2_surface_update(&mut self, task: u32, now: u32) -> Result<bool> {
        let (pending, enabled, needs_capture) = {
            let state = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?;
            if state.surface.hold(now) { return Ok(true); }
            (state.surface.pending(), state.surface.capture_enabled(state.surface_enabled()),
                state.surface.needs_capture())
        };
        if !pending { return Ok(false); }
        if !enabled {
            self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?
                .surface.discard_pending();
        } else if needs_capture {
            let previous = self.capture_surface(task)?;
            self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?
                .surface.request(None, previous, false);
        }
        Ok(false)
    }

    pub fn render_preview(&self, task: u32) -> Result<DiagnosticFrame> {
        let mut preview = self.model_preview(task)?;
        Self::tint_surface(self.tint_argb(task)?, &mut preview.frame);
        Ok(preview)
    }

    fn tint_surface(argb: u32, frame: &mut EmoteRasterFrame) {
        if argb == 0xffff_ffff {
            return;
        }
        let tint = [
            ((argb >> 16) & 255) as u8,
            ((argb >> 8) & 255) as u8,
            (argb & 255) as u8,
            (argb >> 24) as u8,
        ];
        // M2Plane draws the driver's already-composited texture on a quad;
        // its uniform vertex color is applied after internal model rendering.
        for pixel in frame.rgba_mut().chunks_exact_mut(4) {
            for c in 0..4 {
                pixel[c] = ((u32::from(pixel[c]) * u32::from(tint[c]) + 127) / 255) as u8;
            }
        }
    }
}
impl EmoteBackend for CpuEmoteBackend {
    fn debug_m2_text_cp932(&self) -> Result<Vec<u8>> {
        let mut text = super::m2live::CpuM2DebugText::default();
        for (&task, state) in &self.players {
            let Some(owner) = &state.m2_owner else { continue; };
            // The bank formatter keeps signed task and UTF-8 variable order;
            // the live driver supplies the samples rather than config seeds.
            let current = state.player.as_ref()
                .map(CpuPlayer::controller_variables).transpose()?;
            let registered = owner.debug_registered_values(current.as_ref());
            if current.as_ref().is_some_and(|current|
                registered.iter().any(|(name, _)| !current.contains_key(name))) {
                //133e0 may read a model variable after missing the driver's
                //base map. Do not publish a fabricated zero for that path.
                return Err(EmoteError::Unsupported { native: 933,
                    reason: "M2 debug registered name requires the model-variable fallback getter" });
            }
            let (character, _, invalid) = encoding_rs::SHIFT_JIS.decode(&state.m2_player_name);
            if invalid {
                return Err(EmoteError::InvalidResource("M2 debug character CP932"));
            }
            text.insert_bank(task, std::slice::from_ref(&state.resource_name),
                &character, &state.m2_timeline,
                registered)?;
        }
        text.into_cp932()
    }
    fn m2_action_busy(&self, task: u32) -> Result<bool> {
        let state = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?;
        Ok(state.m2_owner.as_ref().is_some_and(|owner| owner.action.busy()))
    }
    fn recompute_m2_controls(&mut self, settings: &[super::CpuM2MaterialSetting]) -> Result<()> {
        let mut prepared = Vec::new();
        for (&task, state) in &self.players {
            let (Some(owner), Some(player)) = (&state.m2_owner, &state.player) else { continue; };
            let mut owner = owner.clone();
            let mut player = player.clone();
            let current = player.controller_variables()?;
            owner.recompute_controls(settings, &mut player.variables, &current)?;
            player.update_reference_timelines()?;
            prepared.push((task, owner, player));
        }
        for (task, owner, player) in prepared {
            let state = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?;
            state.m2_owner = Some(owner);
            state.player = Some(player);
        }
        Ok(())
    }
    fn set_m2_force_position(&mut self, task: u32, position: [f32; 2], linked: bool) -> Result<()> {
        if !position.iter().all(|v| v.is_finite()) { return Err(EmoteError::InvalidResource("M2 force position")); }
        let owner = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?
            .m2_owner.as_mut().ok_or_else(|| unsupported("M2 force position requires live owner"))?;
        owner.force.profile_position = position;
        owner.force.position_linked = linked;
        Ok(())
    }
    fn set_m2_material_group(&mut self, setting: &super::CpuM2MaterialSetting) -> Result<()> {
        for task in self.players.values_mut() {
            if let Some(owner) = task.m2_owner.as_mut() {
                owner.set_material_group(setting);
            }
        }
        Ok(())
    }
    fn create_task(&mut self, task: u32) -> Result<()> {
        if self.players.contains_key(&task) {
            return Err(EmoteError::DuplicateTask(task));
        }
        self.players.insert(task, CpuTask::new());
        Ok(())
    }
    fn release_task(&mut self, task: u32) -> Result<()> {
        self.players
            .remove(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        Ok(())
    }
    fn import_resource(&mut self, task: u32, name: &[u8], resource: &EmoteResource) -> Result<()> {
        let generation = self
            .players
            .get(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .resource_generation
            .checked_add(1)
            .ok_or(EmoteError::Capacity)?;
        if name.len() > 4095 || name.contains(&0) {
            return Err(unsupported("invalid E-mote resource identity"));
        }
        let filter = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?.texture_filter_raw;
        let model = self.shared_model(name, resource, filter)?;
        let mut player = CpuPlayer::from_shared_model(model);
        player.mesh_quality_scale = self.mesh_quality_scale;
        player.update_reference_timelines()?;
        let state = self
            .players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        state.player = Some(player);
        state.resource_name = name.to_vec();
        state.resource_generation = generation;
        // This API creates a new CPU player. Original170be0 initializes the
        // clock on each player, not once for the containing M2Plane task.
        // A future M2 manager reuse path must retain that player's own clock.
        state.original_clock = CpuOriginalTaskClock::default();
        state.native907 = None;
        state.m2_owner = None;
        state.m2_profile.clear();
        state.m2_player_name.clear();
        state.m2_timeline.clear();
        state.surface.clear();
        state.raster_cache = None;
        Ok(())
    }
    fn import_m2_resource(
        &mut self,
        task: u32,
        name: &[u8],
        resource: Option<&EmoteResource>,
        load: CpuM2Load,
    ) -> Result<()> {
        let state = self
            .players
            .get(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        if name.is_empty() || name.len() > 4095 || name.contains(&0) {
            return Err(unsupported("invalid M2 resource identity"));
        }
        let same_resource = state.player.is_some() && state.resource_name == name;
        let same_profile = state.m2_profile == load.profile_alias_cp932
            && state.m2_player_name == load.player_name_cp932;
        let selection = load
            .controller_config
            .pattern_selection(&load.pattern_selectors_cp932)?;
        let unchanged = same_resource
            && same_profile
            && state.m2_timeline == load.timeline_label
            && state
                .m2_owner
                .as_ref()
                .is_some_and(|o| o.selected_patterns == selection);
        if unchanged {
            //17a990/171d50/172610 preserve player, clock, active timelines and
            //blink timestamps when the complete current selection is equal.
            return Ok(());
        }
        //171a38 retains the resource owner independently of transition_raw.
        //171d50 rebuilds only blink/lip banks when the timeline changes;
        //the helper preserves the shared values, driver timers and forces.
        let profile_reuse = if same_resource
            && (!same_profile || state.m2_timeline != load.timeline_label)
        {
            let player = state.player.as_ref().ok_or_else(|| unsupported("M2 retained profile player missing"))?;
            let owner = state.m2_owner.as_ref().ok_or_else(|| unsupported("M2 retained profile owner missing"))?;
            Some(owner.prepare_profile_reuse(&load, &state.m2_player_name, &state.m2_timeline,
                &player.variables, &player.controller_variables()?, state.surface_enabled(),
                state.surface.default_duration())?)
        } else { None };
        let generation = state
            .resource_generation
            .checked_add(1)
            .ok_or(EmoteError::Capacity)?;
        let previous_owner = state.m2_owner.clone();
        let previous_resource_name = state.resource_name.clone();
        let previous_timeline = state.m2_timeline.clone();
        let texture_filter_raw = state.texture_filter_raw;
        //171a8d..171aa4 sets9c when both old/new source vectors exist and
        //the persistent wrapper's requested10c is nonzero.17592e releases
        //the old driver, but the manager still owns its last rendered RGBA.
        //1725aa then resets+a0 from constructor+a4 when rebuilding the
        //primary difference owner. An earlier545 duration does not survive.
        let replacing_resource = state.player.is_some() && !same_resource;
        let mut fade_duration = if replacing_resource
            && previous_owner.as_ref().is_some_and(|owner|
                owner.state.requested_raw_10c.unwrap_or(1) != 0)
        {
            Some(state.surface.default_duration())
        } else { None };
        if same_resource && same_profile && state.surface_enabled() {
            if let Some(previous) = previous_owner.as_ref() {
                for (dimension, to) in selection.iter().enumerate() {
                    let from = previous.selected_patterns.get(dimension).map(Vec::as_slice).unwrap_or(b"");
                    if from != to.as_slice() {
                        if from.is_empty() || to.is_empty() {
                            // 172aa6 uses constructor+a4, which545 does not
                            // overwrite, even when load's transition word is0.
                            fade_duration = Some(state.surface.default_duration());
                            continue;
                        }
                        let chain = load.controller_config.chains.select(dimension as u32, from, to)?;
                        if chain.kind == super::CpuM2ChainKind::Fade {
                            // 172ba8 writes+a0 for each changed Fade dimension;
                            // subsequent dimensions retain the last such value.
                            fade_duration = Some(chain.frames);
                        }
                    }
                }
            }
        }
        if let Some(prepared) = &profile_reuse {
            fade_duration = prepared.surface_duration;
        }
        let previous_surface = fade_duration.map(|duration|
            self.capture_surface(task).map(|frame| (duration, frame))).transpose()?;
        let mut player = if same_resource {
            state
                .player
                .as_ref()
                .ok_or_else(|| unsupported("M2 reused player missing"))?
                .clone()
        } else {
            let shared = self.shared_model(
                name,
                resource
                    .ok_or_else(|| unsupported("new M2 player requires actual PSZ/PSB bytes"))?,
                texture_filter_raw,
            )?;
            CpuPlayer::from_shared_model(shared)
        };
        player.mesh_quality_scale = self.mesh_quality_scale;
        player.set_root_affine(load.root_affine)?;
        let reusing_owner = profile_reuse.is_some()
            || same_resource && same_profile && previous_owner.is_some();
        let controller_current = player.controller_variables()?;
        let mut owner = if let Some(prepared) = profile_reuse {
            player.variables = prepared.variables;
            prepared.owner
        } else if reusing_owner {
            previous_owner
                .as_ref()
                .ok_or_else(|| unsupported("M2 reused owner missing"))?
                .reselect(
                    load.controller_config,
                    &load.pattern_selectors_cp932,
                    load.transition_raw,
                    &mut player.variables,
                    &controller_current,
                )?
        } else {
            player.m2_timed_variables = CpuM2TimedVariables::default();
            player.variables.extend(load.secondary_variables);
            CpuM2LiveOwner::new(
                load.controller_config,
                &load.pattern_selectors_cp932,
                &mut player.variables,
                &load.secondary_force_defaults,
            )?
        };
        //170eea starts bc/c0 at0;1743e0 changes them through message51.
        //The difference file's ac/b0 affects placement, never force input.
        if let Some(previous) = previous_owner.as_ref() {
            owner.force.profile_position = previous.force.profile_position;
            owner.force.position_linked = previous.force.position_linked;
            owner.inherit_program_owner(previous);
            owner.inherit_spline_owner(previous);
            if !reusing_owner {
                owner.force.inherit_overrides(&previous.force, &controller_current)?;
            }
        }
        if replacing_resource {
            owner.force.group_change(&load.secondary_config, &previous_resource_name, name);
        }
        if !reusing_owner {
            owner.spline.config = load.spline_config;
            owner.action = previous_owner.as_ref().map(|owner| owner.action.clone())
                .unwrap_or_else(|| super::m2action::CpuM2ActionOwner::new(load.action_config));
            owner.install_secondary(load.secondary_config, &player.variables)?;
            for setting in &load.material_settings {
                owner.set_material_group(setting);
            }
            owner.force.seed_history(
                owner.state.transition.as_mut()
                    .ok_or(EmoteError::InvalidResource("M2 force transition owner"))?,
                &player.controller_variables()?,
            )?;
        }
        if previous_owner.is_none() {
            let current = player.controller_variables()?;
            let registered = self.players
                .get(&task)
                .ok_or(EmoteError::UnknownTask(task))?;
            registered.apply_unloaded_control_requests(&mut owner, &mut player.variables, &current)?;
            registered.apply_unloaded_spline_request(&mut owner);
        }
        CpuM2TimedVariables::validate(
            &player.model.document.root,
            &player.controller_variables()?,
            &owner.pending_timed,
        )?;
        if let Some(previous) = previous_owner.as_ref().filter(|_| !reusing_owner) {
            owner.state.scene_override_138 = previous.state.scene_override_138;
            let current = player.controller_variables()?;
            owner.apply_907(
                previous.state.requested_raw_10c.unwrap_or(1),
                &mut player.variables,
                &current,
            )?;
            owner.blink_enable = previous.blink_enable;
            owner.lip_enable = previous.lip_enable;
            owner.requested_blink_enable = previous.requested_blink_enable;
            owner.requested_lip_enable = previous.requested_lip_enable;
            owner.scene_blink_enable = previous.scene_blink_enable;
            owner.scene_lip_enable = previous.scene_lip_enable;
            owner.retain_lip_evaluators_from(previous);
        }
        if previous_timeline != load.timeline_label || !same_resource {
            player.stop_timeline("");
            if player
                .timeline_labels()?
                .iter()
                .any(|label| label == &load.timeline_label)
            {
                player.play_timeline(&load.timeline_label, 2)?;
            }
            //An absent label is the original driver's stop-all/no-match path.
        }
        player.update_reference_timelines()?;
        let state = self
            .players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        // The captured surface above owns pixels, never the old driver. Drop
        // old timelines/mesh/texture references with the old player here;
        // the shared-model weak cache may still serve another live task.
        state.raster_cache = None;
        state.player = Some(player);
        state.resource_name = name.to_vec();
        state.resource_generation = generation;
        //The170be0 M2 wrapper/clock survives driver-resource replacement.
        state.m2_owner = Some(owner);
        //Retain the snapshot on a failed import; drop it only after publish.
        //Later replacements inherit the live+fc owner instead of replaying it.
        state.unloaded_spline = None;
        state.native907 = None;
        state.m2_profile = load.profile_alias_cp932;
        state.m2_player_name = load.player_name_cp932;
        state.m2_timeline = load.timeline_label;
        if replacing_resource { state.surface.resource_rebuilt(); }
        state.surface.selection_loaded();
        if let Some((duration, frame)) = previous_surface {
            state.surface.request(Some(duration), frame, true);
        }
        Ok(())
    }
    fn command(&mut self, task: u32, command: &EmoteCommand) -> Result<EmoteReply> {
        if !self.players.contains_key(&task) {
            return Err(EmoteError::UnknownTask(task));
        }
        let descriptor = emote_native_descriptor(command.native)
            .ok_or_else(|| unsupported("native absent from the E-mote descriptor table"))?;
        match descriptor.packet_operation(command)? {
            EmoteOperation::HitTestControls => {
                let state = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?;
                if let (Some(player), Some(owner)) = (&state.player, &state.m2_owner) {
                    let mut player = player.clone();
                    let mut owner = owner.clone();
                    //175620 maps enable=1 to0.0, enable=0 to1.0. These are
                    //#hittest variable values, not general engine enable.
                    let value = if command.command == 548 { 0.0 } else { 1.0 };
                    let current = player.controller_variables()?;
                    for name in &owner.config.hittest {
                        owner.value_table.write_absolute(name, value, &current);
                        owner.force.set_baseline(name, value, &current)?;
                    }
                    //1757e0 calls16d250(0), then virtualfc(1.0). Do not run
                    //the owner clock, blink, lip, action or spring a second time.
                    player.consume_m2_values_with_mode(&mut owner, false)?;
                    player.advance_frames(1.0)?;
                    player.update_reference_timelines()?;
                    //The CPU renderer traverses owned pixels lazily;17e150's
                    //device-cache refresh needs no original graphics device.
                    let state = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?;
                    state.player = Some(player);
                    state.m2_owner = Some(owner);
                }
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::QueryActionState => {
                //24cf9a consumes/formats the retained stack word before
                //dispatch, but553 carries no input string.1758c0 tests only
                //the actual wrapper180 current-step pointer, not a queued
                //label, generic driver-busy flag or a fabricated count.
                Ok(EmoteReply::Value(u32::from(self.m2_action_busy(task)?)))
            }
            EmoteOperation::SelectProgram => {
                let EmoteBuffer::Bytes(record) = &command.buffer1 else {
                    return Err(unsupported("M2 program requires an owned label record"));
                };
                if let Some(owner) = self.players.get_mut(&task).and_then(|state| state.m2_owner.as_mut()) {
                    owner.select_program_record80(record)?;
                }
                //239197/2391dd consume the record and return continue. A
                //nonempty %time/%set program remains an explicit diagnostic.
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::StartAction => {
                let EmoteBuffer::Bytes(bytes) = &command.buffer1 else {
                    return Err(unsupported("M2 action requires owned label bytes"));
                };
                let label = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
                if let Some(owner) = self.players.get_mut(&task).and_then(|state| state.m2_owner.as_mut()) {
                    owner.action.start(label);
                }
                //2392f2 clears eax after175880, regardless of select's result.
                Ok(EmoteReply::Value(0))
            }
            EmoteOperation::SkipAction | EmoteOperation::ClearActionSaved => {
                if let Some(owner) = self.players.get_mut(&task).and_then(|state| state.m2_owner.as_mut()) {
                    if descriptor.operation == EmoteOperation::SkipAction { owner.action.skip_to_restore(); }
                    else { owner.action.clear_saved(); }
                }
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::QueryAttributes => {
                let EmoteBuffer::ThreeWords(mut values) = command.buffer1 else {
                    return Err(unsupported("M2 attribute query requires three initialized words"));
                };
                let state = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?;
                if state.player.is_some() {
                    //174510 clears two words before testing wrapper+64;
                    //187760 clears all three before the signed exact lookup.
                    values[0] = 0;
                    values[1] = 0;
                    if let Some(owner) = state.m2_owner.as_ref() {
                        values = owner.config.attributes.get(&(command.data0 as i32))
                            .copied().unwrap_or([0; 3]);
                    }
                }
                Ok(EmoteReply::ThreeWords(values))
            }
            EmoteOperation::TextureFilter => {
                //237d7e ->17d160 ->17a8d0 creates selector0's empty wrapper
                //at task construction, before any PSB/player/live banks.
                //The actual sscript790337 sets134 before906 at790643;
                //1745d0 does not require a loaded driver or controller.
                //175900/1778f0 consume this word on a later real rebuild.
                let state = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?;
                state.texture_filter_raw = command.data1;
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::SplineLabel => {
                let name = if command.command == 543 {
                    let EmoteBuffer::Bytes(bytes) = &command.buffer1 else {
                        return Err(unsupported("M2 spline label requires owned bytes"));
                    };
                    Some(bytes.split(|&byte| byte == 0).next().unwrap_or_default())
                } else { None };
                if let Some(state) = self.players.get_mut(&task) {
                    state.apply_spline_request(name);
                }
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::FinishPending => {
                let state = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?;
                let mut player = state.player.clone();
                let mut owner = state.m2_owner.clone();
                if let (Some(player), Some(owner)) = (player.as_mut(), owner.as_mut()) {
                    player.finish_pending(owner)?;
                }
                let mut surface = state.surface.clone();
                surface.finish_blend();
                state.player = player;
                state.m2_owner = owner;
                state.surface = surface;
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::QueryBusy => {
                let state = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?;
                let busy = state.surface.blending()
                    || state.m2_owner.as_ref().is_some_and(|owner| owner.value_table.chain_count() != 0)
                    || state.player.as_ref().is_some_and(CpuPlayer::driver_busy);
                Ok(EmoteReply::Value(u32::from(busy)))
            }
            EmoteOperation::MaterialHold => {
                let name = if command.command == 539 { None } else {
                    let EmoteBuffer::Bytes(bytes) = &command.buffer0 else {
                        return Err(unsupported("M2 material hold requires an owned name"));
                    };
                    let bytes = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
                    let (name, _, invalid) = encoding_rs::SHIFT_JIS.decode(bytes);
                    if invalid { return Err(unsupported("M2 material hold name CP932")); }
                    Some(name.into_owned())
                };
                let state = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?;
                if let Some(owner) = state.m2_owner.as_mut() {
                    if let Some(name) = name {
                        if let Some(material) = owner.materials.get_mut(&name) {
                            material.set_held_raw(command.data1);
                        }
                    } else {
                        for material in owner.materials.values_mut() {
                            material.set_held_raw(0);
                        }
                    }
                }
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::VariableSpeed | EmoteOperation::VariableWeight
                | EmoteOperation::ResetVariableWeight => {
                let reset = descriptor.operation == EmoteOperation::ResetVariableWeight;
                let bits = if reset { 1.0f32.to_bits() } else {
                    let EmoteBuffer::FloatBits(bits) = command.buffer1 else {
                        return Err(unsupported("M2 owner scalar requires actual float bits"));
                    };
                    bits
                };
                //531 ignores its supplied name:16da50 resets the whole owner.
                let name = if reset { None } else { match &command.buffer0 {
                    EmoteBuffer::None => None,
                    EmoteBuffer::Bytes(bytes) => {
                        let bytes = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
                        let (name, _, invalid) = encoding_rs::SHIFT_JIS.decode(bytes);
                        if invalid { return Err(unsupported("M2 owner scalar name CP932")); }
                        Some(name.into_owned())
                    }
                    _ => return Err(unsupported("M2 owner scalar name buffer")),
                } };
                let state = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?;
                if let Some(owner) = state.m2_owner.as_mut() {
                    owner.value_table.set_scalar(name.as_deref(), f32::from_bits(bits),
                        reset || descriptor.operation == EmoteOperation::VariableWeight)?;
                }
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::QueryTouchCount => {
                let state = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?;
                let count = state.m2_owner.as_ref().map_or(0, |owner|
                    owner.touch_descriptor_count(command.data0, command.data1));
                Ok(EmoteReply::Value(count))
            }
            EmoteOperation::QueryTouchItem => {
                let EmoteBuffer::ThreeWords([color, table, row]) = command.buffer1 else {
                    return Err(unsupported("M2 touch query requires color/table/row"));
                };
                let state = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?;
                let fields = state.m2_owner.as_ref().map_or([None, None], |owner|
                    owner.touch_descriptor_item_cp932(color, table, row));
                Ok(EmoteReply::DescriptorColumns(fields))
            }
            EmoteOperation::QuerySecondaryPatterns => {
                //17dd00 first clears the output, then addresses selector0.
                //187400 reads registered defaults from secondary config+68;
                //it does not enumerate the currently selected patterns+78.
                let state = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?;
                let bytes = match &state.m2_owner {
                    Some(owner) => owner.secondary_default_pattern_query_cp932()?,
                    None => Vec::new(),
                };
                Ok(EmoteReply::ByteString(bytes))
            }
            EmoteOperation::SelectSecondary => {
                let EmoteBuffer::Bytes(bytes) = &command.buffer1 else {
                    return Err(unsupported("M2 secondary selection specification bytes"));
                };
                let spec = super::m2config::CpuM2PlaySpec::parse_unquoted(bytes)?;
                let state = self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?;
                //17db90/17c2c0 lookup is confined to this task and player
                //key. The wrapper consumes a missing player without creating
                //another resource, timeline or secondary controller owner.
                if spec.player != state.m2_player_name || state.m2_owner.is_none() {
                    return Ok(EmoteReply::Continue);
                }
                let player = state.player.as_ref().ok_or_else(|| unsupported("M2 secondary player missing"))?;
                let mut owner = state.m2_owner.as_ref().unwrap().clone();
                let current = player.controller_variables()?;
                let (changed, fade) = owner.select_secondary(
                    &spec.patterns, command.data0, state.surface.pending(), &current,
                )?;
                CpuM2TimedVariables::validate(&player.model.document.root, &current, &owner.pending_timed)?;
                let previous_surface = fade.map(|duration|
                    self.capture_surface(task).map(|frame| (duration, frame))).transpose()?;
                let state = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?;
                state.m2_owner = Some(owner);
                if changed {
                    state.surface.selection_loaded();
                }
                if let Some((duration, frame)) = previous_surface {
                    state.surface.request(Some(duration), frame, true);
                }
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::LoadResource => {
                let EmoteBuffer::Bytes(spec) = &command.buffer1 else {
                    return Err(unsupported("load specification bytes"));
                };
                let (name, resource) = self
                    .resources
                    .as_mut()
                    .ok_or_else(|| unsupported("PSZ resource provider not bound"))?
                    .read(spec, command.data0)?;
                self.import_resource(task, &name, &resource)?;
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::EnableControls => {
                let state = self
                    .players
                    .get_mut(&task)
                    .ok_or(EmoteError::UnknownTask(task))?;
                if state.player.is_none() && state.m2_owner.is_none() {
                    //174504 writes10c even with no60/f8 or bank vectors.
                    state.unloaded_control_requests[0] = Some(command.data1);
                    return Ok(EmoteReply::Continue);
                }
                let player = state
                    .player
                    .as_mut()
                    .ok_or_else(|| unsupported("E-mote player resource is not loaded"))?;
                let current = player.controller_variables()?;
                if let Some(owner) = state.m2_owner.as_mut() {
                    owner.apply_907(command.data1, &mut player.variables, &current)?;
                    return Ok(EmoteReply::Continue);
                }
                state.native907.as_mut().ok_or(EmoteError::Unsupported {
                    native:907, reason:"actual profile controller banks/scene override must be bound after resource load",
                })?.apply_with_current(command.data1,&mut player.variables,&current)?;
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::ColorPercentages => {
                let EmoteBuffer::ThreeWords(values) = &command.buffer1 else {
                    return Err(unsupported("three raw color percentage DWORDs required"));
                };
                self.players
                    .get_mut(&task)
                    .ok_or(EmoteError::UnknownTask(task))?
                    .color_percentages = Some(*values);
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::ControllerA | EmoteOperation::ControllerB => {
                let state = self
                    .players
                    .get_mut(&task)
                    .ok_or(EmoteError::UnknownTask(task))?;
                if state.player.is_none() && state.m2_owner.is_none() {
                    //174c35/174cc5 write14c/150 before traversing banks.
                    let index = if command.native == 927 { 1 } else { 2 };
                    state.unloaded_control_requests[index] = Some(command.data1);
                    return Ok(EmoteReply::Continue);
                }
                let owner = state.m2_owner.as_mut().ok_or(EmoteError::Unsupported {
                    native: command.native,
                    reason: "M2 controller command requires an actually imported live owner",
                })?;
                let player = state
                    .player
                    .as_mut()
                    .ok_or_else(|| unsupported("M2 player resource missing"))?;
                if command.native == 927 {
                    owner.set_blink_enable(command.data1, &mut player.variables);
                    player.controller_a_raw = command.data1;
                } else {
                    owner.set_lip_enable(command.data1, &mut player.variables);
                    player.controller_b_raw = command.data1;
                }
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::SurfaceFade => {
                let state = self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?;
                if state.player.is_none() {
                    return Err(unsupported("M2 surface request requires a loaded PSB player"));
                }
                state.surface.request_capture(command.data1);
                Ok(EmoteReply::Continue)
            }
            EmoteOperation::RecomputeControls | EmoteOperation::DebugBankText => Err(EmoteError::Unsupported {
                native: command.native, reason: "global M2 operation requires the manager receiver",
            }),
            EmoteOperation::Unrecovered(reason) => Err(EmoteError::Unsupported {
                native: command.native,
                reason,
            }),
        }
    }
    fn advance(&mut self, task: u32, tick: EmoteTick) -> Result<()> {
        if self
            .players
            .get(&task)
            .is_some_and(|s| s.m2_owner.is_some())
        {
            return Err(unsupported(
                "M2 update requires the real shared integer random stream",
            ));
        }
        self.advance_with_random(task, tick, &mut || {
            unreachable!("raw PSB update has no M2 random consumer")
        })
    }
    fn advance_with_random(
        &mut self,
        task: u32,
        tick: EmoteTick,
        random: &mut dyn FnMut() -> u32,
    ) -> Result<()> {
        if self.prepare_m2_surface_update(task, tick.task_elapsed)? {
            return Ok(());
        }
        let state = self
            .players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        let surface_pending = state.surface.pending();
        let surface_enabled = state.surface_enabled();
        let mut player = state
            .player
            .as_ref()
            .ok_or_else(|| unsupported("E-mote player resource is not loaded"))?
            .clone();
        let p = &mut player;
        let mut next_owner = state.m2_owner.clone();
        let mut clock = state.original_clock;
        let step = if self.original_task_clock {
            clock.advance(tick.task_elapsed)
        } else {
            (f64::from(tick.increment) / self.viewport.time_units_per_frame) as f32
        };
        if let Some(owner) = next_owner.as_mut() {
            if surface_pending && surface_enabled {
                // 17b0f2..17b151 disables the real controller owner for the
                // one update producing the new static surface, then restores
                // it after 174040. Preserve the original requested raw word.
                let raw = owner.state.requested_raw_10c.unwrap_or(1);
                let current = p.controller_variables()?;
                owner.apply_907(0, &mut p.variables, &current)?;
                owner.state.requested_raw_10c = Some(raw);
            }
            let immediate = owner.state.engine_enable_10 == Some(0);
            if immediate {
                p.m2_timed_variables.finish_all(&mut p.variables);
            }
            //173f0e prepares the distinct force accumulator before16d250.
            owner.force.prepare();
            p.consume_m2_values(owner)?;
            owner.pending_timed.clear();
            //193090 runs after the value table and before blink/lip. Its
            //1837a0 output uses the real zero-duration driver setter.
            let current = p.controller_variables()?;
            let force_targets = owner.advance_force(tick.task_elapsed, &current)?;
            p.m2_timed_variables.submit_targets(
                &p.model.document.root, &current, &mut p.variables, &force_targets,
            )?;
            //Original173f1f consumes the owner table before bank writes;
            //173fde/17401e stage the next update's absolute values.
            owner.advance_blink(tick.task_elapsed, &mut p.variables, random)?;
            owner.advance_lip(tick.lip_elapsed, &mut p.variables)?;
            //174083 invokes191910 before virtualfc. It waits for the same
            //live chain map and driverf0 timers used by the ordinary wait.
            let current = p.controller_variables()?;
            let action_targets = owner.action.advance(tick.task_elapsed, &current,
                owner.value_table.chain_count() != 0, p.driver_busy())?;
            p.m2_timed_variables.submit_targets(
                &p.model.document.root, &current, &mut p.variables, &action_targets,
            )?;
        }
        p.advance_frames(step)?;
        if surface_pending && surface_enabled {
            if let Some(owner) = next_owner.as_mut() {
                let current = p.controller_variables()?;
                owner.apply_907(owner.state.requested_raw_10c.unwrap_or(1), &mut p.variables, &current)?;
            }
        }
        //Commit reference clocks on every complete driver update, even when
        //this tick has no draw or only captures a manager surface.
        p.update_reference_timelines()?;
        let mut surface = state.surface.clone();
        if surface_pending {
            if next_owner.as_ref().is_some_and(|owner|
                !owner.pending_effects.is_empty() || owner.sidecar_diagnostics().next().is_some()) {
                return Err(unsupported("M2 surface capture requires complete live owner pixels"));
            }
            surface.begin(p.render_strict(self.viewport)?, surface_enabled)?;
        }
        state.player = Some(player);
        state.m2_owner = next_owner;
        state.original_clock = clock;
        state.surface = surface;
        Ok(())
    }
    fn bind_m2_lip_curve(
        &mut self,
        task: u32,
        dimension: u32,
        pattern: &[u8],
        curve: CpuM2LipCurve,
    ) -> Result<()> {
        self.players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .m2_owner
            .as_mut()
            .ok_or_else(|| unsupported("M2 lip link requires a loaded live owner"))?
            .bind_lip_curve(dimension, pattern, curve)
    }
    fn bind_m2_lpd(
        &mut self,
        task: u32,
        bytes: &[u8],
        origin_tick: u32,
        bindings: &[CpuM2LipBinding],
    ) -> Result<usize> {
        self.bind_m2_lip_sidecar(task, bytes, CpuM2LipSidecarKind::Lpd, origin_tick, bindings)
    }
    fn update_m2_waveform(
        &mut self,
        task: u32,
        samples: &[u32],
        original_extent: u32,
    ) -> Result<usize> {
        self.players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .m2_owner
            .as_mut()
            .ok_or_else(|| unsupported("waveform requires a loaded M2 owner"))?
            .update_waveform(samples, original_extent)
    }
    fn control_m2_waveform(&mut self, task: u32, immediate_stop: bool) -> Result<()> {
        let task = self
            .players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        task.m2_owner
            .as_mut()
            .ok_or_else(|| unsupported("waveform control requires M2 owner"))?
            .control_waveform(
                immediate_stop,
                &mut task
                    .player
                    .as_mut()
                    .ok_or_else(|| unsupported("waveform control requires loaded player"))?
                    .variables,
            );
        Ok(())
    }
    fn bind_m2_lip_sidecar(
        &mut self,
        task: u32,
        bytes: &[u8],
        kind: CpuM2LipSidecarKind,
        origin_tick: u32,
        bindings: &[CpuM2LipBinding],
    ) -> Result<usize> {
        let mut owner = self
            .players
            .get(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .m2_owner
            .as_ref()
            .ok_or_else(|| unsupported("lip sidecar requires a loaded live M2 owner"))?
            .clone();
        if bindings.is_empty() || bindings.len() > 4096 {
            return Err(EmoteError::InvalidResource("M2 lip binding count"));
        }
        let records: Vec<_> = bindings
            .iter()
            .cloned()
            .map(|binding| (binding, origin_tick))
            .collect();
        let result = super::m2presets::bind_sidecar_records(&mut owner, bytes, kind, &records);
        //Virtual open already cleared old arrays, even if the new bytes
        //failed. Preserve that mutation and its persistent strict diagnostic.
        self.players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .m2_owner = Some(owner);
        result?;
        Ok(bindings.len())
    }
    fn bind_m2_profile_lip_sidecar(
        &mut self,
        task: u32,
        bytes: &[u8],
        kind: CpuM2LipSidecarKind,
    ) -> Result<usize> {
        let mut owner = self
            .players
            .get(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .m2_owner
            .as_ref()
            .ok_or_else(|| unsupported("profile sidecar requires live M2 owner"))?
            .clone();
        let records = owner.profile_sidecar_bindings(kind)?;
        let result = super::m2presets::bind_sidecar_records(&mut owner, bytes, kind, &records);
        self.players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .m2_owner = Some(owner);
        result?;
        Ok(records.len())
    }
    fn render_rgba(&mut self, task: u32) -> Result<EmoteRasterFrame> {
        let state = self.players.get_mut(&task).ok_or_else(|| unsupported("E-mote player resource is not loaded"))?;
        let mut captured = state.render_model(self.viewport, &mut self.raster_scratch)?;
        // Keep the untinted manager surface; quad color must be applied only
        // once, after old/new surface composition, on every actual draw.
        if let Some(bytes) = state.surface.remember(captured.clone()).and_then(EmoteRasterFrame::try_recycle) {
            if bytes.capacity() > self.raster_scratch.capacity() {
                self.raster_scratch = bytes;
            }
        }
        Self::tint_surface(state.tint_argb(), &mut captured);
        Ok(captured)
    }
    fn playback_info(&self, task: u32) -> Result<EmotePlaybackInfo> {
        let state = self
            .players
            .get(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        let player = state
            .player
            .as_ref()
            .ok_or_else(|| unsupported("E-mote player resource is not loaded"))?;
        Ok(EmotePlaybackInfo {
            resource_name: state.resource_name.clone(),
            resource_generation: state.resource_generation,
            default_object: player.model.base_object.clone(),
            default_motion: player.model.base_motion.clone(),
            object: player.object.clone(),
            motion: player.motion.clone(),
            frame: player.frame,
            native907_banks_bound: state.native907.is_some() || state.m2_owner.is_some(),
            original_task_clock: self.original_task_clock,
            original_clock_fields: state.original_clock.raw_fields(),
            active_m2_timed_variables: player.m2_timed_variables.active_count(),
            active_timeline_labels: player
                .active_timelines()
                .into_iter()
                .map(|t| t.label)
                .collect(),
            m2_owner: state.m2_owner.as_ref().map(CpuM2LiveOwner::info),
        })
    }
    fn set_viewport(&mut self, mut viewport: CpuViewport) -> Result<()> {
        if self.original_task_clock {
            viewport.time_units_per_frame = super::NEKO_ORIGINAL_TASK_UNITS_PER_FRAME;
        }
        viewport.check()?;
        self.viewport = viewport;
        Ok(())
    }
    fn play_timeline(&mut self, task: u32, label: &str, flags: u32) -> Result<()> {
        let mut player = self.player(task)?.clone();
        player.play_timeline(label, flags)?;
        player.update_reference_timelines()?;
        *self.player_mut(task)? = player;
        Ok(())
    }
    fn stop_timeline(&mut self, task: u32, label: &str) -> Result<()> {
        let mut player = self.player(task)?.clone();
        player.stop_timeline(label);
        player.update_reference_timelines()?;
        *self.player_mut(task)? = player;
        Ok(())
    }
    fn set_base_argb(&mut self, task: u32, argb: u32) -> Result<()> {
        self.players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .base_argb = argb;
        Ok(())
    }
    fn bind_native907_state(&mut self, task: u32, binding: CpuNative907State) -> Result<()> {
        binding.validate()?;
        let state = self
            .players
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        if state.player.is_none() {
            return Err(unsupported(
                "bind native907 banks after actual resource/profile load",
            ));
        }
        state.native907 = Some(binding);
        Ok(())
    }
    fn set_root_affine(&mut self, task: u32, affine: [f64; 6]) -> Result<()> {
        self.player_mut(task)?.set_root_affine(affine)
    }
    fn set_driver_transform(&mut self, task: u32, transform: super::CpuDriverTransform) -> Result<()> {
        self.player_mut(task)?.driver_transform = Some(transform);
        Ok(())
    }
    fn set_surface_fade_default(&mut self, task: u32, duration: u32) -> Result<()> {
        self.players.get_mut(&task).ok_or(EmoteError::UnknownTask(task))?
            .surface.set_default(duration);
        Ok(())
    }
    fn surface_holds_tick(&self, task: u32, now: u32) -> Result<bool> {
        Ok(self.players.get(&task).ok_or(EmoteError::UnknownTask(task))?
            .surface.holds_at(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object<const N: usize>(fields: [(&str, PsbValue); N]) -> PsbValue {
        PsbValue::Object(fields.into_iter().map(|(name, value)| (name.into(), value)).collect())
    }

    fn backend() -> Result<CpuEmoteBackend> {
        let sprite = object([
            ("type", PsbValue::Integer(0)),
            ("transformOrder", PsbValue::Integers(vec![0, 1, 2, 3])),
            ("children", PsbValue::List(Vec::new())),
            ("parameterize", PsbValue::Integer(0)),
            ("frameList", PsbValue::List(vec![
                object([("content", object([("src", PsbValue::String("atlas".into())),
                    ("icon", PsbValue::String("dot".into()))]))]),
                object([("time", PsbValue::Integer(1)), ("type", PsbValue::Integer(0))]),
                object([("time", PsbValue::Integer(2)), ("type", PsbValue::Integer(0))]),
            ])),
        ]);
        let motion = object([
            ("layer", PsbValue::List(vec![sprite])),
            ("parameter", PsbValue::List(vec![object([
                ("id", PsbValue::String("mouth".into())), ("rangeEnd", PsbValue::Integer(1)),
                ("division", PsbValue::Integer(1)),
            ])])),
        ]);
        let icon = object([("width", PsbValue::Integer(1)), ("height", PsbValue::Integer(1))]);
        let root = object([
            ("source", object([("atlas", object([("icon", object([("dot", icon)]))]))])),
            ("object", object([("actor", object([("motion", object([("idle", motion)]))]))])),
        ]);
        let model = CpuMotionModel {
            document: PsbDocument::from_test_root(root),
            textures: [("atlas".into(), CpuTexture { width: 1, height: 1, rgba: vec![200, 100, 50, 255] })].into(),
            base_object: "actor".into(), base_motion: "idle".into(),
        };
        let mut backend = CpuEmoteBackend::new(CpuViewport { width: 3, height: 2,
            origin_x: 0.0, origin_y: 0.0, scale: 1.0, time_units_per_frame: 1.0 })?;
        backend.create_task(1)?;
        backend.players.get_mut(&1).unwrap().player = Some(CpuPlayer::new(model));
        Ok(backend)
    }

    #[test]
    fn sidecar_emote_exact_hit_and_live_input_invalidation() -> Result<()> {
        let mut backend = backend()?;
        let first = backend.render_rgba(1)?;
        assert_eq!(first.alpha_bounds(), Some([0, 0, 1, 1]));
        assert!(Arc::ptr_eq(&first.pixels, &backend.render_rgba(1)?.pixels));
        backend.set_base_argb(1, 0x00ff_ffff)?;
        assert_eq!(backend.render_rgba(1)?.alpha_bounds(), None);
        assert_eq!(first.alpha_bounds(), Some([0, 0, 1, 1]));
        backend.set_base_argb(1, 0xffff_ffff)?;
        assert!(Arc::ptr_eq(&first.pixels, &backend.render_rgba(1)?.pixels));

        backend.player_mut(1)?.set_variable("mouth", 1.0)?;
        assert_eq!(backend.render_rgba(1)?.alpha_bounds(), None);
        backend.player_mut(1)?.set_variable("mouth", 0.0)?;
        backend.viewport.origin_x = 1.0;
        let moved = backend.render_rgba(1)?;
        assert_eq!(moved.alpha_bounds(), Some([1, 0, 2, 1]));
        assert_eq!(moved.rgba(), backend.render_preview(1)?.frame.rgba());
        backend.player_mut(1)?.reference_timelines.mark_tick(1.0, &BTreeMap::new());
        let refreshed = backend.render_rgba(1)?;
        assert!(!Arc::ptr_eq(&moved.pixels, &refreshed.pixels));
        assert_eq!(moved.rgba(), refreshed.rgba());

        let mut model = (*backend.player(1)?.model).clone();
        model.textures.get_mut("atlas").unwrap().rgba[0] = 60;
        backend.player_mut(1)?.model = Arc::new(model);
        let replacement = backend.render_rgba(1)?;
        assert_ne!(replacement.rgba(), refreshed.rgba());
        assert_eq!(replacement.rgba(), backend.render_preview(1)?.frame.rgba());

        let mut owner = CpuM2LiveOwner::new(Default::default(), &[], &mut BTreeMap::new(), &BTreeMap::new())?;
        owner.pending_effects.push("live test warning".into());
        backend.players.get_mut(&1).unwrap().m2_owner = Some(owner);
        assert!(backend.render_rgba(1).unwrap_err().to_string().contains("live test warning"));
        Ok(())
    }

    #[test]
    fn sidecar_emote_recycles_cleared_pixels_without_overriding_held_surface() -> Result<()> {
        let mut backend = backend()?;
        let first = backend.render_rgba(1)?.rgba().as_ptr();
        backend.viewport.origin_x = 1.0;
        backend.render_rgba(1)?;
        assert_eq!(backend.raster_scratch.as_ptr(), first);
        backend.viewport.origin_x = 2.0;
        let rendered = backend.render_rgba(1)?;
        assert_eq!(rendered.rgba().as_ptr(), first);
        assert_eq!(rendered.alpha_bounds(), Some([2, 0, 3, 1]));

        let held = EmoteRasterFrame::new(3, 2, [0, 0], vec![0; 24])?;
        backend.players.get_mut(&1).unwrap().surface.request(Some(30), held.clone(), false);
        assert!(Arc::ptr_eq(&held.pixels, &backend.render_rgba(1)?.pixels));
        backend.players.get_mut(&1).unwrap().surface.discard_pending();
        assert!(Arc::ptr_eq(&rendered.pixels, &backend.render_rgba(1)?.pixels));
        Ok(())
    }
}
