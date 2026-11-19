//! CPU work surfaces for the registered render-task protocol.
//! v4.01 receivers: 004d1ad0, 004d23a0, 004d3330, 004d3b50, 004d44d0.
//! Ordinary indices are not message numbers: Yukikoi's messages are shifted
//! by eleven. The semantic entry below never manufactures VM pointers.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, Color, ImageDrawOptions, Point, RgbaFrame};
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{NativeReturn, Vm};

use crate::{LoggingHost, native_state_error, read_u32};

#[path = "render_task_pixels.rs"]
mod pixels;
#[path = "frame_effects.rs"]
mod frame_effects;
#[path = "render_surface_pass.rs"]
mod surface_pass;
use surface_pass::SurfacePass;

const MAX_WORK_BYTES: usize = 128 * 1024 * 1024;
const V401: &str = "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f";
const GRISAIA_LABYRINTH: &str = "ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5";
const KOKONATSU: &str = "c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3";
const YUKIKOI: &str = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderTaskOperation {
    Query,
    ConfigureBlur,
    EnableBlur,
    ConfigureMosaic,
    EnableMosaic,
    ConfigureShade,
    EnableShade,
    ConfigureDraw,
    EnableDraw,
    SetDrawFlag,
    ConfigureWipe,
    EnableWipe,
}

#[derive(Default)]
pub(crate) struct RenderTaskStore {
    tasks: BTreeMap<u32, RenderTask>,
    effects: frame_effects::FrameEffects,
    callback_owner: Option<u32>,
    auxiliary: Option<RgbaFrame>,
    auxiliary_second: Option<RgbaFrame>,
    auxiliary_count: u32,
}

#[derive(Clone)]
struct RenderTask {
    kind: u32,
    available: bool,
    active: u32,
    size: [u32; 2],
    raw_configuration: [u32; 22],
    configured: bool,
    mosaic_rect: Option<[u32; 4]>,
    fresh_feedback: bool,
    /// 004d1bd1/004d1ed2: destination slot this callback, toggled at stage5.
    auxiliary_slot: u32,
    now: u32,
    started: u32,
    end: u32,
    depth: f32,
    duration: u32,
    draw_flag: bool,
    captured: Option<RgbaFrame>,
    mask: Option<RgbaFrame>,
    work: Option<RgbaFrame>,
    opacity: u8,
    rendered_once: bool,
    warmup_frames: Option<u32>,
}

impl RenderTaskStore {
    /// This software backend's largest positive axis at its existing RGBA
    /// allocation budget. The legacy field is initialized from device caps;
    /// D3D/tex_size is a separate image-reduction enum, not a pixel limit.
    pub(crate) fn auxiliary_dimension_limit(&self) -> i32 {
        (MAX_WORK_BYTES / 4) as i32
    }

    pub(crate) fn auxiliary_target(&self,index:u32)->Option<&RgbaFrame> {
        if index>=self.auxiliary_count {return None;}
        match index {0=>self.auxiliary.as_ref(),1=>self.auxiliary_second.as_ref(),_=>None}
    }

    pub(crate) fn release_auxiliary_targets(&mut self) {
        self.auxiliary=None;
        self.auxiliary_second=None;
        self.auxiliary_count=0;
    }

    pub(crate) fn prepare_auxiliary_targets(&mut self,count:u32,size:[u32;2],limit:i32)->Result<()> {
        if count>2 || count<=self.auxiliary_count {return Ok(());}
        self.release_auxiliary_targets();
        if limit>0 && size.iter().any(|&value|value>limit as u32) {
            bail!("auxiliary target exceeds backend texture dimension limit");
        }
        check_work(size)?;
        let length=size[0] as usize*size[1] as usize*4;
        let first=RgbaFrame::from_rgba8(size[0],size[1],vec![0;length])?;
        let second=(count==2).then(||first.clone());
        self.auxiliary=Some(first);
        self.auxiliary_second=second;
        self.auxiliary_count=count;
        Ok(())
    }

    pub(crate) fn replace_auxiliary_target(&mut self,index:u32,frame:RgbaFrame)->Result<()> {
        let target=match index {
            0 if self.auxiliary_count>0=>&mut self.auxiliary,
            1 if self.auxiliary_count>1=>&mut self.auxiliary_second,
            _=>bail!("auxiliary target slot does not exist"),
        };
        *target=Some(frame);Ok(())
    }

    pub(crate) fn supports(index: u32) -> bool {
        matches!(index, 520..=532) || frame_effects::supports(index)
    }

    pub(crate) fn supports_kind(kind: u32) -> bool {
        matches!(kind, 35..=39) || frame_effects::supports_kind(kind)
    }

    pub(super) fn register(
        &mut self,
        id: u32,
        kind: u32,
        canvas: Option<&Canvas>,
        profile: Option<&cs2_runtime::native_profile::FeatureProfile>,
    ) -> cs2_script::Result<()> {
        if frame_effects::supports_kind(kind) {
            let size = canvas.map_or([0; 2], |canvas| [canvas.width(), canvas.height()]);
            return self.register_frame_effect(id, kind, size, profile).map_err(|_|
                native_state_error("frame effect registration cannot allocate its work surface"));
        }
        if !matches!(kind, 35..=39) {
            return Ok(());
        }
        let size = canvas.map_or([0; 2], |canvas| [canvas.width(), canvas.height()]);
        let available = !size.contains(&0);
        let mut initial = None;
        if available {
            check_work(size).map_err(|_| {
                native_state_error("render task work surface exceeds its allocation limit")
            })?;
            let count = if matches!(kind, 35 | 36) { 2 } else { 1 };
            if kind == 39 {
                // 004d3f04..004d3fa9 copies auxiliary slot0 to independent
                // CPU storage, then releases the pool. The supplied canvas
                // supplies dimensions; its current pixels are not this source.
                self.prepare_auxiliary_targets(1, size, 0).map_err(|_| {
                    native_state_error("render Wipe auxiliary allocation failed")
                })?;
                initial = self.auxiliary_target(0).cloned();
                if initial.is_none() {
                    return Err(native_state_error("render Wipe auxiliary source is absent"));
                }
                self.release_auxiliary_targets();
            } else {
                if self.auxiliary_count < count || self.auxiliary.is_none() {
                    self.auxiliary = Some(black_frame(size).map_err(|_| {
                        native_state_error("render auxiliary surface allocation failed")
                    })?);
                    self.auxiliary_count = count;
                    self.auxiliary_second = if count>1 {
                        Some(black_frame(size).map_err(|_|native_state_error(
                            "second render auxiliary surface allocation failed"))?)
                    } else {None};
                }
                if kind == 38 {
                    initial = self.auxiliary.clone();
                }
            }
        }
        let mut raw_configuration = [0; 22];
        if kind == 35 {
            for index in [0, 5, 10, 15] {
                raw_configuration[index] = 1.0f32.to_bits();
            }
            raw_configuration[21] = 1_000_000.0f32.to_bits();
        }
        self.tasks.insert(
            id,
            RenderTask {
                kind,
                available,
                active: 0,
                size,
                raw_configuration,
                configured: false,
                mosaic_rect: None,
                fresh_feedback: true,
                auxiliary_slot: 0,
                now: 0,
                started: 0,
                end: 0,
                depth: 1_000_000.,
                duration: 0,
                draw_flag: false,
                captured: initial,
                mask: None,
                work: None,
                opacity: 255,
                rendered_once: false,
                warmup_frames: (kind == 38 && profile.is_some_and(|p| p.pe_sha256 == YUKIKOI))
                    .then_some(0),
            },
        );
        // 00556de0 stores one callback/data pair in the logical window.
        // Creating another render task replaces it even if active is zero.
        if available {
            self.callback_owner = Some(id);
        }
        Ok(())
    }

    pub(super) fn update(&mut self, id: u32, now: u32) -> Result<()> {
        self.effects.update(id, now)?;
        if let Some(task) = self.tasks.get_mut(&id) {
            task.now = now;
        }
        Ok(())
    }

    pub(super) fn release(&mut self, id: u32) {
        let effect = self.effects.remove(id);
        if self.tasks.remove(&id).is_some_and(|task| task.available) || effect {
            // The original unregister call clears the pair unconditionally;
            // deleting an older task does not restore its previous callback.
            self.callback_owner = None;
            self.auxiliary = None;
            self.auxiliary_second = None;
            self.auxiliary_count = 0;
        }
    }

    pub(super) fn delete_frame_effect(&mut self, id: u32) {
        if self.effects.contains(id) {
            self.release(id);
        }
    }

    pub(crate) fn has_composition(&self) -> bool {
        if self.callback_owner.is_some_and(|id| self.effects.participates(id)) {
            return true;
        }
        self.callback_owner
            .and_then(|id| self.tasks.get(&id))
            .is_some_and(RenderTask::participates)
    }

    /// Compose actual source/foreground depth ranges. The platform callback
    /// must repaint its normal scene into `canvas`, selecting lower <= Z <
    /// upper. It must not draw these render tasks recursively itself.
    ///
    /// Work images remain owned by the graphics service. A failed callback
    /// retains its previous work/feedback state.
    pub(crate) fn compose(
        &mut self,
        canvas: &mut Canvas,
        draw_range: &mut impl FnMut(&mut Canvas, f32, f32) -> Result<()>,
    ) -> Result<()> {
        if let Some(id) = self.callback_owner.filter(|id| self.effects.contains(*id)) {
            return if self.effects.participates(id) {
                self.effects.compose(id, canvas, [self.auxiliary.as_ref(), self.auxiliary_second.as_ref()], draw_range)
            } else {
                draw_range(canvas, 0., 1_000_000.)
            };
        }
        let Some(owner) = self
            .callback_owner
            .filter(|id| self.tasks.get(id).is_some_and(RenderTask::participates))
        else {
            return Ok(());
        };
        // Rendering may fail after one of the callbacks. Keep all retained
        // feedback/capture state at the last completed frame in that case.
        let previous = self.tasks[&owner].clone();
        if self.tasks[&owner].kind == 38 {
            self.tasks.get_mut(&owner).unwrap().captured = self.auxiliary.clone();
        }
        let mut surfaces = SurfacePass::new(
            [self.auxiliary.as_ref(), self.auxiliary_second.as_ref()],
            self.auxiliary_count,
        );
        let mut calls = 0usize;
        let mut bounded_draw = |canvas: &mut Canvas, lower: f32, upper: f32| {
            calls += 1;
            if calls > 4096 {
                bail!("render depth-range callback budget exhausted");
            }
            draw_range(canvas, lower, upper)
        };
        let result = render_chain(
            &mut self.tasks,
            &mut surfaces,
            &[owner],
            canvas,
            0.,
            1_000_000.,
            &mut bounded_draw,
            0,
        );
        let [first, second] = surfaces.finish();
        if result.is_err() {
            self.tasks.insert(owner, previous);
        } else {
            if let Some(frame) = first { self.auxiliary = Some(frame); }
            if let Some(frame) = second { self.auxiliary_second = Some(frame); }
        }
        result
    }
}

impl RenderTask {
    fn participates(&self) -> bool {
        self.available
            && match self.kind {
                35 | 37 => self.active != 0,
                36 => self.active != 0 && self.configured,
                38 | 39 => true,
                _ => false,
            }
    }
}

impl LoggingHost<'_> {
    /// Factory bridge for kinds35..39. The original constructor's +70/+74
    /// is the gated allocator result, not merely the presence of a canvas.
    /// Keep registration/rendering on this store; no extra window callback.
    pub(crate) fn register_retained_render_task(
        &mut self,
        id: u32,
        kind: u32,
    ) -> cs2_script::Result<()> {
        if !matches!(kind, 35..=39) {
            return Err(native_state_error("render-task factory bridge has the wrong kind"));
        }
        let size = self.back_buffer.or(self.canvas.as_deref())
            .map_or([0; 2], |canvas| [canvas.width(), canvas.height()]);
        let policy = self.frame_effect_capture_policy()
            .map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?;
        let available = self.data_services.tasks.render.prepare_effect_auxiliary(
            if matches!(kind, 35 | 36) { 2 } else { 1 }, size, policy,
        ).map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?;
        self.data_services.tasks.render.register(
            id, kind,
            if available { self.back_buffer.or(self.canvas.as_deref()) } else { None },
            self.data_services.native_profile,
        )
    }

    pub(crate) fn render_tasks_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        if frame_effects::supports(index) {
            return self.frame_effect_native(index, args);
        }
        let count = match index {
            520 | 529 | 532 => 4,
            521 => 32,
            523 => 40,
            525 | 527 => 16,
            522 | 524 | 526 | 528 | 531 => 8,
            530 => 20,
            _ => return Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        };
        if args.len() != count {
            return Err(native_state_error(
                "render native argument count is invalid",
            ));
        }
        let id = read_u32(args, 0)?;
        if id == 0 {
            return Ok(if index == 520 {
                NativeReturn::Value(0)
            } else {
                NativeReturn::Continue
            });
        }
        let kind = self
            .data_services
            .tasks
            .message_task_state(id)
            .ok_or_else(|| native_state_error("render native target is absent"))?
            .0;
        if index == 520 && frame_effects::supports_kind(kind) {
            if self.data_services.tasks.message_task_state(id)
                .is_some_and(|(_, deletion, _)| deletion != 0)
            {
                return Ok(NativeReturn::Value(0));
            }
            return self.data_services.tasks.render.frame_effect_active(id)
                .map(NativeReturn::Value)
                .ok_or_else(|| native_state_error("frame effect query has no concrete graphics owner"));
        }
        if !matches!(kind, 35..=39) {
            return Err(native_state_error(
                "render native target needs its concrete render receiver",
            ));
        }
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_some_and(|(_, deletion, _)| deletion != 0)
        {
            return Ok(if index == 520 {
                NativeReturn::Value(0)
            } else {
                NativeReturn::Continue
            });
        }
        if index == 532 {
            // VA00527f60 sends384; Wipe's004d44d0 receiver has no384 case.
            // This wrapper delegates/propagates and does not capture or start.
            if kind != 39 {
                return Err(native_state_error(
                    "render532 requires the known type39 base delegation",
                ));
            }
            return Ok(NativeReturn::Continue);
        }
        let operation = match index {
            520 => RenderTaskOperation::Query,
            521 => RenderTaskOperation::ConfigureBlur,
            522 => RenderTaskOperation::EnableBlur,
            523 => RenderTaskOperation::ConfigureMosaic,
            524 => RenderTaskOperation::EnableMosaic,
            525 => RenderTaskOperation::ConfigureShade,
            526 => RenderTaskOperation::EnableShade,
            527 => RenderTaskOperation::ConfigureDraw,
            528 => RenderTaskOperation::EnableDraw,
            529 => RenderTaskOperation::SetDrawFlag,
            530 => RenderTaskOperation::ConfigureWipe,
            531 => RenderTaskOperation::EnableWipe,
            _ => unreachable!(),
        };
        if index == 530 && read_u32(args, 16)? == 0 {
            // The wrapper skips dispatch entirely when the mask is zero.
            return Ok(NativeReturn::Continue);
        }
        let mut output = 0;
        let bytes = match index {
            521 => {
                let mut record = vm.read(read_u32(args, 4)?, 64)?;
                record.extend_from_slice(&args[8..32]);
                record
            }
            527 => draw_configuration(
                args,
                self.data_services.native_profile.map(|profile| profile.pe_sha256),
            ).to_vec(),
            523 | 525 => args[4..].to_vec(),
            530 => {
                let mut record = Vec::with_capacity(20);
                if self
                    .data_services
                    .native_profile
                    .is_some_and(|profile| profile.pe_sha256 == KOKONATSU)
                {
                    record.extend_from_slice(&args[8..16]);
                    record.extend_from_slice(&args[4..8]);
                    record.extend_from_slice(&args[16..20]);
                } else {
                    record.extend_from_slice(&args[4..20]);
                }
                record.extend_from_slice(&0u32.to_le_bytes());
                record
            }
            _ => Vec::new(),
        };
        let scalar = if matches!(index, 522 | 524 | 526 | 528 | 531) {
            read_u32(args, 4)?
        } else {
            0
        };
        self.render_task_command_bytes(
            id,
            operation,
            scalar,
            (!bytes.is_empty()).then_some(bytes.as_slice()),
            &mut output,
        )
        .map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?;
        Ok(if index == 520 {
            NativeReturn::Value(output)
        } else {
            NativeReturn::Continue
        })
    }

    /// Semantic producer API for FES/platform code, with owned record bytes.
    pub(crate) fn render_task_command_bytes(
        &mut self,
        id: u32,
        operation: RenderTaskOperation,
        scalar: u32,
        bytes: Option<&[u8]>,
        output: &mut u32,
    ) -> Result<()> {
        if operation == RenderTaskOperation::Query
            && let Some(active) = self.data_services.tasks.render.frame_effect_active(id)
        {
            *output = active;
            return Ok(());
        }
        let now = self
            .data_services
            .tasks
            .task_clock(id)
            .context("render task clock is absent")?
            .elapsed();
        let mask = if operation == RenderTaskOperation::ConfigureWipe {
            let bytes = bytes.context("render Wipe needs its 20-byte configuration")?;
            if bytes.len() < 20 {
                bail!("render Wipe configuration is truncated");
            }
            let handle = dword(bytes, 12)?;
            // Native530 has already translated an ordinary wrapper to its
            // underlying surface. Here the typed producer supplies its real
            // host wrapper identity, never an address in a temporary VM.
            let mask = self
                .image_handles
                .get(&handle)
                .context("render Wipe mask is not a live ordinary image wrapper")?;
            if mask.metadata().bit_depth != 32 {
                bail!("render Wipe requires the original 32-bit mask image format");
            }
            Some(mask.clone())
        } else {
            None
        };
        let original = self
            .data_services
            .tasks
            .render
            .tasks
            .get(&id)
            .context("render task was not registered with the graphics service")?;
        if operation == RenderTaskOperation::Query {
            *output = if original.kind == 38 && !original.available {
                0
            } else {
                original.active
            };
            return Ok(());
        }
        let required_kind = match operation {
            RenderTaskOperation::ConfigureBlur | RenderTaskOperation::EnableBlur => 35,
            RenderTaskOperation::ConfigureMosaic | RenderTaskOperation::EnableMosaic => 36,
            RenderTaskOperation::ConfigureShade | RenderTaskOperation::EnableShade => 37,
            RenderTaskOperation::ConfigureDraw
            | RenderTaskOperation::EnableDraw
            | RenderTaskOperation::SetDrawFlag => 38,
            RenderTaskOperation::ConfigureWipe | RenderTaskOperation::EnableWipe => 39,
            RenderTaskOperation::Query => unreachable!(),
        };
        if original.kind != required_kind {
            bail!("render command does not belong to this concrete task type");
        }
        if !original.available {
            return Ok(());
        }
        // Validate/allocate before replacing the live record.
        let mut task = original.clone();
        task.now = now;
        match operation {
            RenderTaskOperation::ConfigureBlur
            | RenderTaskOperation::ConfigureMosaic
            | RenderTaskOperation::ConfigureShade => {
                let length = match task.kind {
                    35 => 88,
                    36 => 36,
                    _ => 12,
                };
                let bytes = bytes
                    .filter(|bytes| bytes.len() >= length)
                    .context("render sampling configuration is absent or truncated")?;
                for (target, bytes) in task
                    .raw_configuration
                    .iter_mut()
                    .zip(bytes[..length].chunks_exact(4))
                {
                    *target = u32::from_le_bytes(bytes.try_into().unwrap());
                }
                if task.kind == 37 {
                    task.raw_configuration[0] = (task.raw_configuration[0] as i32).min(4) as u32;
                }
                pixels::validate_configuration(task.kind, &task.raw_configuration)?;
                if task.kind == 36 && task.mosaic_rect.is_none() {
                    // 004d248b calls the geometry initializer only before
                    // the first successful configuration (field +1b0).
                    task.mosaic_rect = Some(std::array::from_fn(|i| task.raw_configuration[i]));
                }
                task.configured = true;
            }
            RenderTaskOperation::EnableBlur
            | RenderTaskOperation::EnableMosaic
            | RenderTaskOperation::EnableShade => {
                task.active = scalar;
                if task.kind == 35 {
                    task.fresh_feedback = true;
                }
            }
            RenderTaskOperation::ConfigureDraw | RenderTaskOperation::ConfigureWipe => {
                let length = if task.kind == 38 { 12 } else { 20 };
                let bytes = bytes
                    .filter(|bytes| bytes.len() >= length)
                    .context("render preparation record is absent or truncated")?;
                let words: Vec<_> = bytes[..length]
                    .chunks_exact(4)
                    .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
                    .collect();
                task.raw_configuration[..words.len()].copy_from_slice(&words);
                task.depth = f32::from_bits(words[1]);
                task.duration = words[2];
                if !task.depth.is_finite() || task.depth < 0. || task.depth > 1_000_000. {
                    bail!("render split depth is outside the recoverable logical range");
                }
                if task.active != 0 {
                    task.started = now;
                    task.end = now.wrapping_add(task.duration);
                    if let Some(warmup) = task.warmup_frames.as_mut() {
                        *warmup = 0;
                        task.duration = task.duration.max(1);
                    }
                }
                if task.kind == 39 {
                    if task.duration == 0 {
                        bail!(
                            "render Wipe duration zero reaches original integer division by zero"
                        );
                    }
                    task.mask = mask;
                    task.work = None;
                    // The independent converted mask is prepared here, while
                    // the captured source remains a separate retained image.
                    prepare_mask(&mut task)?;
                    apply_wipe_pixels(&mut task, 0, false)?;
                }
                task.configured = true;
            }
            RenderTaskOperation::EnableDraw | RenderTaskOperation::EnableWipe => {
                if task.kind == 39 && scalar != 0 && task.mask.is_none() {
                    bail!("render Wipe cannot start before its real mask is prepared");
                }
                task.active = scalar;
                if scalar != 0 {
                    task.started = now;
                    task.end = now.wrapping_add(task.duration);
                    if let Some(warmup) = task.warmup_frames.as_mut() {
                        *warmup = 0;
                    }
                }
            }
            RenderTaskOperation::SetDrawFlag => {
                // Both identified receivers set +7c, but neither callback
                // reads it. It cannot be used to invent a frame capture.
                task.draw_flag = true;
            }
            RenderTaskOperation::Query => unreachable!(),
        }
        self.data_services.tasks.render.tasks.insert(id, task);
        Ok(())
    }

    pub(crate) fn receive_render_task_command(
        &mut self,
        command: TaskCommand,
        buffers: &mut TaskCommandBuffers<'_>,
        output: &mut u32,
    ) -> cs2_script::Result<Option<TaskCommandOutcome>> {
        if self.data_services.tasks.message_task_state(command.target as u32)
            .is_some_and(|(kind, _, _)| frame_effects::supports_kind(kind))
        {
            return self.receive_frame_effect_command(command, buffers, output);
        }
        if command.code < 350 {
            return Ok(None);
        }
        let id = command.target as u32;
        let Some(kind) = self
            .data_services
            .tasks
            .message_task_state(id)
            .map(|value| value.0)
            .filter(|kind| matches!(kind, 35..=39))
        else {
            return Ok(None);
        };
        let base = render_message_base(self.data_services.native_profile).ok_or_else(|| {
            native_state_error("raw render-task messages need an identified PE message table")
        })?;
        let Some(offset) = command.code.checked_sub(base) else {
            return Ok(None);
        };
        let operation = match (kind, offset) {
            (_, 0) => RenderTaskOperation::Query,
            (35, 1) => RenderTaskOperation::ConfigureBlur,
            (35, 2) => RenderTaskOperation::EnableBlur,
            (36, 3) => RenderTaskOperation::ConfigureMosaic,
            (36, 4) => RenderTaskOperation::EnableMosaic,
            (37, 5) => RenderTaskOperation::ConfigureShade,
            (37, 6) => RenderTaskOperation::EnableShade,
            (38, 7) => RenderTaskOperation::ConfigureDraw,
            (38, 8) => RenderTaskOperation::EnableDraw,
            (38, 9) => RenderTaskOperation::SetDrawFlag,
            (39, 10) => RenderTaskOperation::ConfigureWipe,
            (39, 11) => RenderTaskOperation::EnableWipe,
            _ => return Ok(None),
        };
        self.render_task_command_bytes(
            id,
            operation,
            command.data[1],
            buffers.data1.as_deref(),
            output,
        )
        .map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?;
        Ok(Some(TaskCommandOutcome {
            done: true,
            propagate: false,
        }))
    }

    pub(super) fn dispatch_render_task_message(
        &mut self,
        target: u32,
        packet: [u32; 4],
        queued: bool,
        output: &mut u32,
        vm: Option<&mut Vm>,
    ) -> cs2_script::Result<Option<TaskCommandOutcome>> {
        if self.data_services.tasks.message_task_state(target)
            .is_some_and(|(kind, _, _)| frame_effects::supports_kind(kind))
        {
            return self.dispatch_frame_effect_message(target, packet, queued, output, vm);
        }
        if packet[1] < 350 {
            return Ok(None);
        }
        let Some(base) = render_message_base(self.data_services.native_profile) else {
            return Err(native_state_error(
                "raw render-task message layout is not identified",
            ));
        };
        let Some(offset) = packet[1].checked_sub(base) else {
            return Ok(None);
        };
        let Some(kind) = self
            .data_services
            .tasks
            .message_task_state(target)
            .map(|value| value.0)
        else {
            return Ok(None);
        };
        let size = match (kind, offset) {
            (_, 0) | (35, 2) | (36, 4) | (37, 6) | (38, 8..=9) | (39, 11) => 0,
            (35, 1) => 88,
            (36, 3) => 36,
            (37, 5) => 12,
            (38, 7) => 12,
            (39, 10) => 20,
            _ => return Ok(None),
        };
        let command = TaskCommand {
            target: target as i32,
            sender: packet[0] as i32,
            code: packet[1],
            data: [packet[2], packet[3], 0],
        };
        if !queued && let Some(mut buffers) = self.pending_owned_task_buffers(command) {
            return self.route_task_command_buffers(command, &mut buffers, output, 4096);
        }
        let bytes = if size == 0 {
            None
        } else {
            Some(
                vm.ok_or_else(|| {
                    native_state_error("render configuration needs its sender VM or owned buffer")
                })?
                .read(packet[3], size)?,
            )
        };
        let mut buffers = super::OwnedTaskBuffers::new(None, bytes)?;
        if queued {
            let found = self.queue_task_command_buffers(command, &buffers, output)?;
            Ok(found.then_some(TaskCommandOutcome {
                done: true,
                propagate: false,
            }))
        } else {
            self.route_task_command_buffers(command, &mut buffers, output, 4096)
        }
    }
}

fn render_message_base(
    profile: Option<&cs2_runtime::native_profile::FeatureProfile>,
) -> Option<u32> {
    match profile?.pe_sha256 {
        V401 => Some(372),
        YUKIKOI => Some(383),
        _ => None,
    }
}

fn render_chain(
    tasks: &mut BTreeMap<u32, RenderTask>,
    surfaces: &mut SurfacePass<'_>,
    ids: &[u32],
    destination: &mut Canvas,
    lower: f32,
    upper: f32,
    draw: &mut impl FnMut(&mut Canvas, f32, f32) -> Result<()>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        bail!("render callback recursion exceeded");
    }
    let Some((&id, preceding)) = ids.split_last() else {
        return if lower < upper {
            draw(destination, lower, upper)
        } else {
            Ok(())
        };
    };
    let mut task = tasks
        .remove(&id)
        .context("render task disappeared during composition")?;
    let result = (|| {
        if matches!(task.kind, 35..=37) {
            return render_sampled_range(
                &mut task,
                tasks,
                surfaces,
                preceding,
                destination,
                lower,
                upper,
                draw,
                depth,
            );
        }
        if task.depth < lower || task.depth > upper {
            return render_chain(tasks, surfaces, preceding, destination, lower, upper, draw, depth + 1);
        }
        if task.size != [destination.width(), destination.height()] {
            bail!("render work surface size changed without rebuilding its graphics registration");
        }
        check_work(task.size)?;
        let mut live = Canvas::new(task.size[0], task.size[1])?;
        live.clear(Color::rgba(0, 0, 0, 255));
        render_chain(
            tasks,
            surfaces,
            preceding,
            &mut live,
            lower,
            task.depth,
            draw,
            depth + 1,
        )?;
        if task.warmup_frames.is_some_and(|count| count <= 2) {
            // Yukikoi 00552234 resets the fade origin during the first
            // three render callbacks. The v4.01 callback has no such stage.
            task.started = task.now;
            task.end = task.now.wrapping_add(task.duration);
        }
        if task.kind == 38 {
            if task.active != 0 {
                // 004d3d53 compares the relative elapsed value with the
                // stored absolute endpoint; preserve that DWORD behavior.
                let elapsed = task.now.wrapping_sub(task.started).min(task.end);
                let fraction = elapsed.wrapping_mul(255) / task.duration.max(1);
                task.opacity = 255 - fraction.min(255) as u8;
            }
            pixels::composite(
                &mut live,
                task.captured
                    .as_ref()
                    .context("Draw auxiliary texture is absent")?,
                task.opacity,
                true,
            )?;
        } else {
            if task.work.is_none() {
                task.work = task.captured.clone();
            }
            if task.active != 0 {
                wipe_pixels(&mut task)?;
            }
            pixels::composite(
                &mut live,
                task.work.as_ref().context("Wipe work surface is absent")?,
                255,
                false,
            )?;
        }
        // Commit the low-depth result before repainting the live foreground.
        destination.copy_frame(&snapshot(&live)?, &ImageDrawOptions::new(Point::new(0, 0)))?;
        render_chain(
            tasks,
            surfaces,
            preceding,
            destination,
            task.depth,
            upper,
            draw,
            depth + 1,
        )?;
        task.rendered_once = true;
        if let Some(warmup) = task.warmup_frames.as_mut() {
            *warmup = warmup.saturating_add(1);
        }
        // Original stage5 clears activity after, rather than before, drawing
        // the endpoint frame. Stopping retains the last work pixels/alpha.
        if task.now >= task.end {
            task.active = 0;
        }
        Ok(())
    })();
    tasks.insert(id, task);
    result
}

fn render_sampled_range(
    task: &mut RenderTask,
    tasks: &mut BTreeMap<u32, RenderTask>,
    surfaces: &mut SurfacePass<'_>,
    preceding: &[u32],
    destination: &mut Canvas,
    lower: f32,
    upper: f32,
    draw: &mut impl FnMut(&mut Canvas, f32, f32) -> Result<()>,
    depth: usize,
) -> Result<()> {
    let offset = match task.kind {
        35 => 20,
        36 => 6,
        _ => 1,
    };
    let start = f32::from_bits(task.raw_configuration[offset]);
    let end = if task.configured {
        f32::from_bits(task.raw_configuration[offset + 1])
    } else {
        1_000_000.
    };
    let from = lower.max(start);
    let to = upper.min(end);
    if from >= to {
        return render_chain(tasks, surfaces, preceding, destination, lower, upper, draw, depth + 1);
    }
    if task.size != [destination.width(), destination.height()] {
        bail!("render sampling work dimensions changed without graphics registration");
    }
    check_work(task.size)?;
    render_chain(tasks, surfaces, preceding, destination, lower, from, draw, depth + 1)?;
    let mut middle = Canvas::new(task.size[0], task.size[1])?;
    middle.clear(if task.kind == 37 {
        Color::rgba(0, 0, 0, 255)
    } else {
        Color::TRANSPARENT
    });
    render_chain(tasks, surfaces, preceding, &mut middle, from, to, draw, depth + 1)?;
    let source = snapshot(&middle)?;
    match task.kind {
        35 => {
            let previous = if task.fresh_feedback { None } else {
                Some(surfaces.source(1 - task.auxiliary_slot, task.size)?)
            };
            let work = pixels::blur(
                &source,
                previous,
                &task.raw_configuration,
                task.fresh_feedback,
            )?;
            task.work = Some(work.clone());
            surfaces.replace(task.auxiliary_slot, work.clone())?;
            task.fresh_feedback = false;
            pixels::composite(destination, &work, 255, false)?;
            task.auxiliary_slot = 1 - task.auxiliary_slot;
        }
        36 => {
            let work = pixels::mosaic(
                destination,
                &source,
                &task.raw_configuration,
                task.mosaic_rect
                    .context("Mosaic prepared rectangle is absent")?,
            )?;
            // 004d24c0: the live range is target0; the reduced pass is
            // target1. Exposing only reduced pixels as slot0 loses 859's
            // producer/consumer identity for a later Draw or frame effect.
            surfaces.replace(0, source)?;
            surfaces.replace(1, work.clone())?;
            task.work = Some(work);
        }
        37 => {
            pixels::shade(
                destination,
                &source,
                task.configured.then_some(task.raw_configuration[0] as i32),
            )?;
            surfaces.replace(0, source.clone())?;
            task.work = Some(source);
        }
        _ => unreachable!(),
    }
    render_chain(tasks, surfaces, preceding, destination, to, upper, draw, depth + 1)
}

fn prepare_mask(task: &mut RenderTask) -> Result<()> {
    let mask = task
        .mask
        .as_ref()
        .context("render Wipe mask source is absent")?;
    if mask.width() == 0 || mask.height() == 0 {
        bail!("render Wipe mask has no pixels");
    }
    check_work(task.size)?;
    // The original independent mask is scaled to the render surface.
    let mut target = Canvas::new(task.size[0], task.size[1])?;
    target.clear(Color::TRANSPARENT);
    let mut options = ImageDrawOptions::new(Point::new(0, 0));
    options.scale_x = task.size[0] as f32 / mask.width() as f32;
    options.scale_y = task.size[1] as f32 / mask.height() as f32;
    target.copy_frame(mask, &options)?;
    task.mask = Some(snapshot(&target)?);
    Ok(())
}

fn wipe_pixels(task: &mut RenderTask) -> Result<()> {
    let elapsed = task
        .now
        .wrapping_sub(task.started)
        .min(task.end)
        .min(task.duration);
    let hard = task.raw_configuration[4] != 0;
    apply_wipe_pixels(task, elapsed, hard)
}

fn apply_wipe_pixels(task: &mut RenderTask, elapsed: u32, hard: bool) -> Result<()> {
    let mask = task.mask.as_ref().context("render Wipe mask disappeared")?;
    let source = task
        .captured
        .as_ref()
        .context("render Wipe capture disappeared")?;
    let duration = task.duration;
    if duration == 0 {
        bail!("render Wipe duration is zero");
    }
    let mut pixels = task.work.as_ref().unwrap_or(source).pixels().to_vec();
    if pixels.len() != mask.pixels().len() {
        bail!("render Wipe mask/work sizes differ");
    }
    if hard {
        // 004d4830..004d492b clears alpha monotonically when the original
        // BGRA first byte reaches the unsigned 256-step threshold.
        let threshold = 256u32.wrapping_sub(elapsed.wrapping_shl(8) / duration);
        for (pixel, mask) in pixels
            .chunks_exact_mut(4)
            .zip(mask.pixels().chunks_exact(4))
        {
            if u32::from(mask[2]) >= threshold {
                pixel[3] = 0;
            }
        }
    } else {
        let progress = (elapsed.wrapping_shl(9) / duration) as i32;
        for (pixel, mask) in pixels
            .chunks_exact_mut(4)
            .zip(mask.pixels().chunks_exact(4))
        {
            // 004d4a40 builds 255-clamp(progress+BGRA.blue-256,0,255).
            pixel[3] = 255 - (progress + i32::from(mask[2]) - 256).clamp(0, 255) as u8;
        }
    }
    task.work = Some(RgbaFrame::from_rgba8(
        source.width(),
        source.height(),
        pixels,
    )?);
    Ok(())
}

fn snapshot(canvas: &Canvas) -> Result<RgbaFrame> {
    Ok(RgbaFrame::from_rgba8(
        canvas.width(),
        canvas.height(),
        canvas.pixels().to_vec(),
    )?)
}
fn black_frame(size: [u32; 2]) -> Result<RgbaFrame> {
    let mut canvas = Canvas::new(size[0], size[1])?;
    canvas.clear(Color::rgba(0, 0, 0, 255));
    snapshot(&canvas)
}
fn check_work(size: [u32; 2]) -> Result<()> {
    let size = (size[0] as usize)
        .checked_mul(size[1] as usize)
        .and_then(|n| n.checked_mul(4));
    if size.is_none_or(|bytes| bytes > MAX_WORK_BYTES) {
        bail!("render work surface exceeds 128 MiB");
    }
    Ok(())
}
fn dword(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .context("render record DWORD is truncated")?
            .try_into()
            .unwrap(),
    ))
}

fn draw_configuration(args: &[u8], pe_sha256: Option<&str>) -> [u8; 12] {
    let mut record = [0; 12];
    if matches!(pe_sha256, Some(V401 | GRISAIA_LABYRINTH)) {
        record[..8].copy_from_slice(&args[8..16]);
        record[8..].copy_from_slice(&args[4..8]);
    } else {
        record.copy_from_slice(&args[4..16]);
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labyrinth_draw_uses_duration_before_mode_and_depth() {
        let args = [30u32, 58, 0, 45_000f32.to_bits()].map(u32::to_le_bytes).concat();
        let record = draw_configuration(&args, Some(GRISAIA_LABYRINTH));
        assert_eq!(dword(&record, 0).unwrap(), 0);
        assert_eq!(f32::from_bits(dword(&record, 4).unwrap()), 45_000.);
        assert_eq!(dword(&record, 8).unwrap(), 58);
        assert_eq!(draw_configuration(&args, Some(V401)), record);
        assert_eq!(draw_configuration(&args, None), args[4..]);
    }
}
