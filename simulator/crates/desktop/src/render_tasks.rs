//! CPU work surfaces for the registered render-task protocol.
//! v4.01 receivers: 004d1ad0, 004d23a0, 004d3330, 004d3b50, 004d44d0.
//! Ordinary indices are not message numbers: Yukikoi's messages are shifted
//! by eleven. The semantic entry below never manufactures VM pointers.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, Color, ImageDrawOptions, Point, RgbaFrame};
use cs2_runtime::native_profile::ConstantNativeReturn;
use cs2_runtime::native_table::RenderArgumentOrder;
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{NativeReturn, Vm};

use crate::{LoggingHost, native_state_error, read_u32};
use crate::css_model::UTATE_SHA;

#[path = "render_task_pixels.rs"]
mod pixels;
#[path = "frame_effects.rs"]
mod frame_effects;
#[path = "render_surface_pass.rs"]
mod surface_pass;
use surface_pass::SurfacePass;
use crate::particle_external::CAMPANELLA_SHA;

const MAX_WORK_BYTES: usize = 128 * 1024 * 1024;
const V401: &str = "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f";
const AUGUST: &str = "4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94";
const DJIBRIL: &str = "012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f";
const GRISAIA_FRUIT: &str = "9f3a5ad0ed22074420685f1e2a3bc92436d68289c7bac854d5f7d7331dc3ef67";
const GRISAIA_LABYRINTH: &str = "ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5";
const GRISAIA_EDEN: &str = "c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c";
const HAKUCHUUMU: &str = "1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5";
const HATSUMIRA: &str = "c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78";
const ISLAND: &str = "474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1";
const KAMIKAZE: &str = "f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e";
const KANMI: &str = "c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26";
const KOKONATSU: &str = "c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3";
const NEKOPARA: &str = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc";
const NEWTON: &str = "3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18";
const NEWTON_LOCAL: &str = "6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de";
const REBELLIONS_DRAW_PE: &str = "a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3";
const REBELLIONS_DRAW_SCRIPT: &str = "533ecc0aec6ae5c0b6b2e21f56ee1d3e4637f5eb655ac0b746a133c06f323407";
const TSUNABAN: &str = "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8";
const SUDDEN_DRAW_SCRIPT: &str = "1153e10b75c189b7908858d0165c6b2020a09d859aa2005704a4e710ea293a95";
const SUDDEN_SCRIPT_CODE: &str = "633d3138fa814497e50b3bc70a1aad901441a8e613b3f2cfdb3f42325c8d93a8";
const SUDDEN_WIPE_PE: &str = "d158c8915d4982c82badc631b4f92e1f140b246df5929ff2d9924d242c66765c";
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
    unprofiled_duration_first_draw: Option<bool>,
    unverified_argument_order_reported: [bool; 2],
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
    pending_wipe: Option<WipeStep>,
    opacity: u8,
    rendered_once: bool,
    warmup_frames: Option<u32>,
}

#[derive(Clone, Copy)]
pub(crate) enum WipeStep {
    Hard(u32),
    Soft(i32),
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
                pending_wipe: None,
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
        ).and_then(|()| verify_deferred_output(canvas));
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

fn verify_deferred_output(canvas: &Canvas) -> Result<()> {
    #[cfg(all(windows, feature = "gpu-raster"))]
    if crate::gpu_raster::DeferredSceneReadback::active() {
        canvas.try_pixels()?;
    }
    let _ = canvas;
    Ok(())
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
        let companion = self.data_services.startup_companion_natives.as_ref();
        let companion_order = match (index, kind, companion) {
            (527, 38, Some(companion)) => companion.draw_order,
            (530, 39, Some(companion)) => companion.wipe_order,
            _ => None,
        };
        let render_order = if matches!(index, 527 | 530)
            && let Some(table) = self.data_services.ordinary_native_table.as_ref()
        {
            match table.render_argument_order(index) {
                Some(order) => Some(order),
                None if table.constant(index) == Some(ConstantNativeReturn::Continue) => {
                    return Ok(NativeReturn::Continue);
                }
                None => {
                    return Err(self.task_error(id, cs2_runtime::RuntimeError::Host(
                        format!("ordinary native {index} has an unsupported render argument layout"),
                    )));
                }
            }
        } else {
            companion_order
        };
        let sudden_wipe = kind == 39 && vm.program_code_sha256() == Some(SUDDEN_SCRIPT_CODE);
        let companion_draw = kind == 38 && companion.is_some_and(|companion| companion.draw_order.is_some());
        let companion_wipe = kind == 39 && companion.is_some_and(|companion| companion.wipe_order.is_some());
        let mut output = 0;
        let bytes = match index {
            521 => {
                let mut record = vm.read(read_u32(args, 4)?, 64)?;
                record.extend_from_slice(&args[8..32]);
                record
            }
            527 => match render_order {
                Some(order) => draw_configuration(args, order == RenderArgumentOrder::DurationFirst),
                None => self.native_draw_configuration(args)?,
            }.to_vec(),
            523 | 525 => args[4..].to_vec(),
            530 if render_order.is_some() => {
                wipe_configuration(args, render_order == Some(RenderArgumentOrder::DurationFirst)).to_vec()
            }
            530 => {
                let pe = self.css_task_sha().filter(|&sha| duration_first_wipe_profile(Some(sha)))
                    .or_else(|| self.data_services.native_profile.map(|profile| profile.pe_sha256))
                    .or_else(|| {
                        // The verified Kanmi sscript passes [task, duration, mode, depth, mask] at 0xb478e.
                        (kind == 39 && self.data_services.particle_message_pe_sha256 == Some(KANMI)
                            && vm.program_code_sha256() == Some("7a068090a8d37c7a446a74e98448458e3d6ed75d94bb1a00022eaeb07d7a3921"))
                            .then_some(KANMI)
                    })
                    .or_else(|| (sudden_wipe && self.verified_sudden_draw_source()).then_some(SUDDEN_WIPE_PE))
                    .or_else(|| self.verified_rebellions_draw_source().then_some(REBELLIONS_DRAW_PE));
                let duration_first = duration_first_wipe_profile(pe);
                if !duration_first {
                    self.report_unverified_render_argument_order(index);
                }
                wipe_configuration(args, duration_first).to_vec()
            }
            _ => Vec::new(),
        };
        let scalar = if matches!(index, 522 | 524 | 526 | 528 | 531) {
            read_u32(args, 4)?
        } else {
            0
        };
        if index == 527 && matches!(self.css_task_sha(), Some(NEWTON | NEWTON_LOCAL)) {
            self.capture_newton_draw_source(id, &bytes)
                .map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?;
        }
        self.render_task_command_bytes(
            id,
            operation,
            scalar,
            (!bytes.is_empty()).then_some(bytes.as_slice()),
            &mut output,
        )
        .map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?;
        let pe = self.css_task_sha();
        let draw_trace = match pe {
            Some(CAMPANELLA_SHA) if index != 520 || output == 0 => Some("campanella-rdraw-native"),
            Some(KAMIKAZE) if index != 520 || output == 0 => Some(if kind == 39 {
                "kamikaze-rwipe-native"
            } else {
                "kamikaze-rdraw-native"
            }),
            Some(KOKONATSU) if index != 520 || output == 0 => Some("kokonatsu-rdraw-native"),
            Some(TSUNABAN) if index != 520 || output == 0 => Some("08-render-native"),
            Some(UTATE_SHA) if index != 520 || output == 0 => Some("utate-rdraw-native"),
            _ if sudden_wipe && (index != 520 || output == 0) => Some("sudden-rwipe-native"),
            _ if companion_wipe && (index != 520 || output == 0) => Some("companion-rwipe-native"),
            _ if companion_draw && (index != 520 || output == 0) => Some("companion-rdraw-native"),
            _ if self.data_services.native_profile.is_some_and(|profile| profile.pe_sha256 == NEKOPARA) =>
                Some("neko-rdraw-native"),
            _ => None,
        };
        let draw_call = kind == 38 && matches!(index, 520 | 527 | 528);
        let wipe_call = (sudden_wipe || companion_wipe || matches!(pe, Some(KAMIKAZE | TSUNABAN)))
            && kind == 39 && matches!(index, 520 | 530 | 531);
        if (draw_call || wipe_call)
            && self.data_services.story_trace
            && let Some(trace) = draw_trace
            && let Some(task) = self.data_services.tasks.render.tasks.get(&id)
        {
            self.events.push(super::LogicEvent {
                frame: self.frame,
                kind: trace.into(),
                detail: format!(
                    "index={index} task={id} scalar={scalar} query={} mode_bits={:#010x} \
                     depth={} depth_bits={:#010x} duration={} now={} started={} end={} \
                     active={} available={} rendered={} opacity={}",
                    (index == 520).then_some(output).map_or("-".into(), |value| value.to_string()),
                    task.raw_configuration[0], task.depth, task.depth.to_bits(),
                    task.duration, task.now, task.started, task.end,
                    task.active, task.available, task.rendered_once, task.opacity,
                ),
            });
        }
        Ok(if index == 520 {
            NativeReturn::Value(output)
        } else {
            NativeReturn::Continue
        })
    }

    fn capture_newton_draw_source(&mut self, id: u32, record: &[u8]) -> Result<()> {
        let Some(task) = self.data_services.tasks.render.tasks.get(&id)
            .filter(|task| task.kind == 38 && task.available) else { return Ok(()); };
        let depth = f32::from_bits(dword(record, 4)?);
        if !(0. ..=1_000_000.).contains(&depth) { return Ok(()); }
        let mut source = Canvas::new(task.size[0], task.size[1])?;
        // This script prepares Draw before publishing the changed list layers.
        self.draw_scene_depth_range(&mut source, 0., depth)?;
        self.data_services.tasks.render.replace_auxiliary_target(0, source.into_frame())
    }

    fn native_draw_configuration(&mut self, args: &[u8]) -> cs2_script::Result<[u8; 12]> {
        let duration_first = if matches!(
            self.css_task_sha(),
            Some(CAMPANELLA_SHA | DJIBRIL | KAMIKAZE | TSUNABAN | UTATE_SHA)
        ) {
            // These wrappers take [task, duration, mode, depth] and send
            // [mode, depth, duration]. Utate native527 is at VA 0x44d2a0.
            true
        } else if let Some(profile) = self.data_services.native_profile {
            duration_first_draw_profile(profile.pe_sha256)
        } else {
            match self.data_services.tasks.render.unprofiled_duration_first_draw {
                Some(verified) => verified,
                None => {
                    let verified = self.css_task_sha() == Some(GRISAIA_FRUIT)
                        || self.verified_kanmi_sound_loop_source()?
                        || self.verified_sudden_draw_source()
                        || self.verified_rebellions_draw_source();
                    self.data_services.tasks.render.unprofiled_duration_first_draw = Some(verified);
                    verified
                }
            }
        };
        if !duration_first {
            self.report_unverified_render_argument_order(527);
        }
        Ok(draw_configuration(args, duration_first))
    }

    fn report_unverified_render_argument_order(&mut self, index: u32) {
        let reported = &mut self.data_services.tasks.render.unverified_argument_order_reported
            [usize::from(index == 530)];
        if *reported {
            return;
        }
        *reported = true;
        self.events.push(super::LogicEvent {
            frame: self.frame,
            kind: "render-native-argument-order-unverified".into(),
            detail: format!("ordinary native {index}: no registered wrapper or verified companion; using record order"),
        });
    }

    fn verified_rebellions_draw_source(&self) -> bool {
        const PE_SIZE: usize = 4_350_976;
        let Some(runtime) = self.resources.resolve_plain_file("REBELLIONS.exe").ok().flatten()
            .filter(|runtime| runtime.size == PE_SIZE as u64)
            .and_then(|runtime| self.resources.read_candidate_bounded(runtime, PE_SIZE).ok().flatten())
        else { return false; };
        rebellions_draw_source_matches(&crate::particle_plugin_digest::sha256(&runtime), || {
            let names = self.resources.category_names(b"kcs", "sscript").ok()?;
            let script = names.iter().find_map(|name| self.resources.find(name))?;
            Some(crate::particle_plugin_digest::sha256(&self.resources.read_candidate(script).ok()?))
        })
    }

    fn verified_sudden_draw_source(&self) -> bool {
        if !self.verified_sudden_sound_loop_source().unwrap_or(false) {
            return false;
        }
        self.resources.category_names(b"kcs", "sscript.kcs").ok()
            .and_then(|names| names.iter().find_map(|name| self.resources.find(name)))
            .and_then(|candidate| self.resources.read_candidate(candidate).ok())
            .is_some_and(|bytes| crate::particle_plugin_digest::sha256(&bytes) == SUDDEN_DRAW_SCRIPT)
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
                    task.pending_wipe = None;
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
                task.pending_wipe = Some(wipe_step(&task)?);
            }
            if !draw_wipe_on_gpu(&task, &mut live)? {
                if let Some(step) = task.pending_wipe.take() {
                    apply_wipe_step(&mut task, step)?;
                }
                pixels::composite(
                    &mut live,
                    task.work.as_ref().context("Wipe work surface is absent")?,
                    255,
                    false,
                )?;
            }
        }
        // Commit the low-depth result before repainting the live foreground.
        *destination = live;
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
    if task.kind == 37 {
        middle.clear(Color::rgba(0, 0, 0, 255));
    }
    render_chain(tasks, surfaces, preceding, &mut middle, from, to, draw, depth + 1)?;
    middle.try_pixels()?;
    let source = middle.into_frame();
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
    let mut options = ImageDrawOptions::new(Point::new(0, 0));
    options.scale_x = task.size[0] as f32 / mask.width() as f32;
    options.scale_y = task.size[1] as f32 / mask.height() as f32;
    target.copy_frame(mask, &options)?;
    task.mask = Some(target.into_frame());
    Ok(())
}

fn wipe_step(task: &RenderTask) -> Result<WipeStep> {
    let elapsed = task
        .now
        .wrapping_sub(task.started)
        .min(task.end)
        .min(task.duration);
    if task.duration == 0 { bail!("render Wipe duration is zero"); }
    Ok(if task.raw_configuration[4] != 0 {
        let threshold = 256u32.wrapping_sub(elapsed.wrapping_shl(8) / task.duration);
        let previous = match task.pending_wipe { Some(WipeStep::Hard(value)) => value, _ => 256 };
        WipeStep::Hard(threshold.min(previous))
    } else {
        WipeStep::Soft((elapsed.wrapping_shl(9) / task.duration) as i32)
    })
}

fn draw_wipe_on_gpu(task: &RenderTask, live: &mut Canvas) -> Result<bool> {
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    if let Some(step) = task.pending_wipe {
        let source = task.work.as_ref().context("Wipe work surface is absent")?;
        let mask = task.mask.as_ref().context("render Wipe mask disappeared")?;
        if source.dimensions() == live.dimensions() && mask.dimensions() == live.dimensions()
            && task.size.into_iter().all(|axis| (1..=4096).contains(&axis))
            && task.size[0] * task.size[1] <= 1 << 23
        {
            if matches!(step, WipeStep::Hard(0) | WipeStep::Soft(511..)) {
                return Ok(true);
            }
            use crate::gpu_raster::{Draw, RasterRect, draw};
            return Ok(draw(live, Some(source), RasterRect { origin: [0; 2], size: task.size },
                Draw::Wipe { mask, step }));
        }
    }
    let _ = (task, live);
    Ok(false)
}

fn apply_wipe_pixels(task: &mut RenderTask, elapsed: u32, hard: bool) -> Result<()> {
    let duration = task.duration;
    if duration == 0 { bail!("render Wipe duration is zero"); }
    let step = if hard {
        WipeStep::Hard(256u32.wrapping_sub(elapsed.wrapping_shl(8) / duration))
    } else {
        WipeStep::Soft((elapsed.wrapping_shl(9) / duration) as i32)
    };
    apply_wipe_step(task, step)
}

fn apply_wipe_step(task: &mut RenderTask, step: WipeStep) -> Result<()> {
    let mask = task.mask.as_ref().context("render Wipe mask disappeared")?;
    let source = task
        .captured
        .as_ref()
        .context("render Wipe capture disappeared")?;
    let mut pixels = task.work.as_ref().unwrap_or(source).pixels().to_vec();
    if pixels.len() != mask.pixels().len() {
        bail!("render Wipe mask/work sizes differ");
    }
    if let WipeStep::Hard(threshold) = step {
        // 004d4830..004d492b clears alpha monotonically when the original
        // BGRA first byte reaches the unsigned 256-step threshold.
        for (pixel, mask) in pixels
            .chunks_exact_mut(4)
            .zip(mask.pixels().chunks_exact(4))
        {
            if u32::from(mask[2]) >= threshold {
                pixel[3] = 0;
            }
        }
    } else if let WipeStep::Soft(progress) = step {
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

fn black_frame(size: [u32; 2]) -> Result<RgbaFrame> {
    let mut canvas = Canvas::new(size[0], size[1])?;
    canvas.clear(Color::rgba(0, 0, 0, 255));
    Ok(canvas.into_frame())
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

fn duration_first_draw_profile(pe_sha256: &str) -> bool {
    matches!(
        pe_sha256,
        V401 | AUGUST | DJIBRIL | GRISAIA_FRUIT | GRISAIA_LABYRINTH | GRISAIA_EDEN | HAKUCHUUMU | HATSUMIRA
            | ISLAND | KANMI | KOKONATSU | NEKOPARA | NEWTON | NEWTON_LOCAL | YUKIKOI
    )
}

fn rebellions_draw_source_matches(
    runtime_sha256: &str,
    script_sha256: impl FnOnce() -> Option<String>,
) -> bool {
    runtime_sha256 == REBELLIONS_DRAW_PE
        && script_sha256().as_deref() == Some(REBELLIONS_DRAW_SCRIPT)
}

fn draw_configuration(args: &[u8], duration_first: bool) -> [u8; 12] {
    let mut record = [0; 12];
    if duration_first {
        record[..8].copy_from_slice(&args[8..16]);
        record[8..].copy_from_slice(&args[4..8]);
    } else {
        record.copy_from_slice(&args[4..16]);
    }
    record
}

fn duration_first_wipe_profile(pe_sha256: Option<&str>) -> bool {
    matches!(
        pe_sha256,
        Some(V401 | AUGUST | CAMPANELLA_SHA | DJIBRIL | GRISAIA_FRUIT | GRISAIA_LABYRINTH | GRISAIA_EDEN
            | HAKUCHUUMU | HATSUMIRA | ISLAND | KAMIKAZE | KANMI
            | KOKONATSU | NEKOPARA | NEWTON | NEWTON_LOCAL | REBELLIONS_DRAW_PE | SUDDEN_WIPE_PE
            | TSUNABAN | UTATE_SHA | YUKIKOI)
    )
}

fn wipe_configuration(args: &[u8], duration_first: bool) -> [u8; 20] {
    let mut record = [0; 20];
    record[..12].copy_from_slice(&draw_configuration(args, duration_first));
    record[12..16].copy_from_slice(&args[16..20]);
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(windows, feature = "gpu-raster"))]
    #[test]
    fn deferred_sampled_effect_rejects_lost_source_and_preserves_feedback() -> Result<()> {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

        #[derive(Debug)]
        struct Readback {
            calls: Arc<AtomicUsize>,
            succeeds: bool,
        }

        impl cs2_renderer::CanvasReadback for Readback {
            fn read_rgba(&self, output: &mut [u8]) -> bool {
                self.calls.fetch_add(1, Ordering::Relaxed);
                for pixel in output.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[10, 20, 30, 255]);
                }
                self.succeeds
            }

            fn as_any(&self) -> &dyn std::any::Any { self }
        }

        let _deferred = crate::gpu_raster::DeferredSceneReadback::begin();
        let mut render = RenderTaskStore::default();
        let mut canvas = Canvas::new(2, 1)?;
        render.register(1, 35, Some(&canvas), None)?;
        let blur = render.tasks.get_mut(&1).unwrap();
        blur.active = 1;
        blur.raw_configuration[20] = 10f32.to_bits();
        blur.raw_configuration[21] = 20f32.to_bits();
        blur.configured = true;
        let previous = render.auxiliary.as_ref().unwrap().clone();

        for (source_succeeds, output_succeeds) in [(false, false), (true, false), (true, true)] {
            canvas.clear(Color::rgb(0, 0, 0));
            let source = Arc::new(AtomicUsize::new(0));
            let output = Arc::new(AtomicUsize::new(0));
            let result = render.compose(&mut canvas, &mut |target, lower, _| {
                let (calls, succeeds) = if lower == 10. {
                    (&source, source_succeeds)
                } else if lower == 20. {
                    (&output, output_succeeds)
                } else {
                    return Ok(());
                };
                target.set_deferred_device_pixels(Arc::new(Readback { calls: calls.clone(), succeeds }));
                Ok(())
            });
            assert_eq!(source.load(Ordering::Relaxed), 1);
            assert_eq!(output.load(Ordering::Relaxed), usize::from(source_succeeds));
            let blur = &render.tasks[&1];
            if output_succeeds {
                result?;
                assert_eq!(canvas.pixel(0, 0)?, Color::rgb(10, 20, 30));
                assert!(!blur.fresh_feedback);
                assert_eq!(blur.auxiliary_slot, 1);
                assert!(blur.work.is_some());
            } else {
                assert!(matches!(
                    result.unwrap_err().downcast_ref::<cs2_renderer::RendererError>(),
                    Some(cs2_renderer::RendererError::DeviceReadbackFailed)
                ));
                assert!(blur.fresh_feedback);
                assert_eq!(blur.auxiliary_slot, 0);
                assert!(blur.work.is_none());
                assert_eq!(render.auxiliary.as_ref().unwrap().pixels(), previous.pixels());
            }
        }
        Ok(())
    }

    #[test]
    fn ordinary_568_captures_previous_pixels_for_draw() -> Result<()> {
        let temp = crate::tests::TempDir::new();
        let mut player = crate::HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("profile"))?;
        let previous = Color::rgb(192, 64, 32);
        let current = Color::rgb(24, 80, 160);
        let depth = 45_000f32;
        let draw = player.with_host(|host| {
            let plane = host.create_task_object(2, 0, b"background", b"")?;
            let node = host.scene_nodes.get_mut(&plane).unwrap();
            node.size = [16., 16.];
            node.position = [0., 0., 5000.];
            node.color = 0xffc0_4020;
            node.visible = true;
            node.initialized = true;

            let mut vm = Vm::new();
            let capture_range = [0f32, depth].map(f32::to_le_bytes).concat();
            cs2_script::Host::call(host, 568, &capture_range, &mut vm)?;
            host.scene_nodes.get_mut(&plane).unwrap().color = 0xff18_50a0;

            let draw = host.create_task_object(38, 0, b"rdraw", b"")?;
            let record = [0, depth.to_bits(), 20].map(u32::to_le_bytes).concat();
            host.render_task_command_bytes(draw, RenderTaskOperation::ConfigureDraw, 0, Some(&record), &mut 0)?;
            host.render_task_command_bytes(draw, RenderTaskOperation::EnableDraw, 1, None, &mut 0)?;
            Ok(draw)
        })?;

        player.render_scene()?;
        assert_eq!(player.canvas.pixel(1, 1)?, previous);
        player.with_host(|host| host.data_services.tasks.render.update(draw, 20))?;
        player.render_scene()?;
        assert_eq!(player.canvas.pixel(1, 1)?, current);
        Ok(())
    }

    #[test]
    fn tsunaban_and_kamikaze_wipe_use_duration_before_float_arguments() {
        // Native 530 at VA 0x445640 and 0x479810 writes the duration at record +8.
        let mode = 1.0f32.to_bits();
        let depth = 45_000f32.to_bits();
        let args = [11u32, 30, mode, depth, 12].map(u32::to_le_bytes).concat();
        let expected = [mode, depth, 30, 12, 0].map(u32::to_le_bytes).concat();
        for runtime in [TSUNABAN, KAMIKAZE] {
            assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(runtime))).as_slice(), expected);
        }
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(None))[8..12], depth.to_le_bytes());
    }

    #[test]
    fn sudden_map028_wipe_preserves_duration_and_mask() {
        // Verified companion slot 530 at 0047a080 prepares the 0166 record.
        let depth = 45_000f32.to_bits();
        let args = [64u32, 178, 0, depth, 17].map(u32::to_le_bytes).concat();
        let expected = [0, depth, 178, 17, 0].map(u32::to_le_bytes).concat();
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(SUDDEN_WIPE_PE))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(None))[8..12], depth.to_le_bytes());
    }

    #[test]
    fn august_cinema_wipe_uses_recorded_530_abi() {
        let mode = 1.0f32.to_bits();
        let split_depth = 45_000f32.to_bits();
        let args = [13u32, 38, mode, split_depth, 345].map(u32::to_le_bytes).concat();
        let expected = [mode, split_depth, 38, 345, 0].map(u32::to_le_bytes).concat();
        let unchanged = [38, mode, split_depth, 345, 0].map(u32::to_le_bytes).concat();
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(AUGUST))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(V401))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(None)).as_slice(), unchanged);
    }

    #[test]
    fn august_and_grisaia_fruit_draw_use_recorded_527_abi() {
        let split_depth = 45_000f32.to_bits();
        for (runtime, task, duration) in [(AUGUST, 30u32, 18u32), (GRISAIA_FRUIT, 29, 58)] {
            let args = [task, duration, 0, split_depth].map(u32::to_le_bytes).concat();
            let expected = [0, split_depth, duration].map(u32::to_le_bytes).concat();
            assert_eq!(draw_configuration(&args, duration_first_draw_profile(runtime)).as_slice(), expected);
        }
    }

    #[test]
    fn rebellions_draw_requires_both_527_fingerprints() {
        let split_depth = 45_000f32.to_bits();
        let args = [12u32, 58, 0, split_depth].map(u32::to_le_bytes).concat();
        let expected = [0, split_depth, 58].map(u32::to_le_bytes).concat();
        let verified = rebellions_draw_source_matches(REBELLIONS_DRAW_PE, || Some(REBELLIONS_DRAW_SCRIPT.into()));
        assert_eq!(draw_configuration(&args, verified).as_slice(), expected);
        for verified in [
            duration_first_draw_profile(REBELLIONS_DRAW_PE),
            rebellions_draw_source_matches(REBELLIONS_DRAW_PE, || Some(SUDDEN_DRAW_SCRIPT.into())),
            rebellions_draw_source_matches(REBELLIONS_DRAW_PE, || None),
            rebellions_draw_source_matches(YUKIKOI, || Some(REBELLIONS_DRAW_SCRIPT.into())),
        ] {
            assert_eq!(draw_configuration(&args, verified), args[4..]);
        }
    }

    #[test]
    fn kanmi_wipe02c_uses_recorded_530_abi() {
        let args = [28u32, 98, 0, 0x472f_c800, 260].map(u32::to_le_bytes).concat();
        let expected = [0u32, 0x472f_c800, 98, 260, 0].map(u32::to_le_bytes).concat();
        let unchanged = [98u32, 0, 0x472f_c800, 260, 0].map(u32::to_le_bytes).concat();
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(KANMI))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(None)).as_slice(), unchanged);
    }

    #[test]
    fn hatsumira_wipe04b_uses_recorded_530_abi() {
        let args = [13u32, 13, 0, 0x472f_c800, 1231].map(u32::to_le_bytes).concat();
        let expected = [0u32, 0x472f_c800, 13, 1231, 0].map(u32::to_le_bytes).concat();
        let unchanged = [13u32, 0, 0x472f_c800, 1231, 0].map(u32::to_le_bytes).concat();
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(HATSUMIRA))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(V401))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(None)).as_slice(), unchanged);
    }

    #[test]
    fn hakuchuumu_cinema_wipe_uses_recorded_530_abi() {
        let mode = 1.0f32.to_bits();
        let split_depth = 45_000f32.to_bits();
        let args = [65u32, 38, mode, split_depth, 259].map(u32::to_le_bytes).concat();
        let expected = [mode, split_depth, 38, 259, 0].map(u32::to_le_bytes).concat();
        let unchanged = [38, mode, split_depth, 259, 0].map(u32::to_le_bytes).concat();
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(HAKUCHUUMU))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(V401))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(None)).as_slice(), unchanged);
    }

    #[test]
    fn hakuchuumu_cinema_draw_uses_recorded_527_abi() {
        let split_depth = 45_000f32.to_bits();
        let args = [13u32, 298, 0, split_depth].map(u32::to_le_bytes).concat();
        let expected = [0, split_depth, 298].map(u32::to_le_bytes).concat();
        assert_eq!(
            draw_configuration(&args, duration_first_draw_profile(HAKUCHUUMU)).as_slice(),
            expected
        );
    }

    #[test]
    fn kokonatsu_chapter_draw_uses_recorded_527_abi() {
        let args = [99u32, 118, 0, 0x472f_c800].map(u32::to_le_bytes).concat();
        let expected = [0u32, 0x472f_c800, 118].map(u32::to_le_bytes).concat();
        let record = draw_configuration(&args, duration_first_draw_profile(KOKONATSU));
        assert_eq!(record.as_slice(), expected);
    }

    #[test]
    fn nekopara_draw_reorders_duration_and_depth() {
        // 2.6.1.76 VA 00642e40 moves arg +4 to record +8.
        let depth = 45_000f32.to_bits();
        let args = [54u32, 120, 0, depth].map(u32::to_le_bytes).concat();
        let expected = [0, depth, 120].map(u32::to_le_bytes).concat();
        assert_eq!(draw_configuration(&args, duration_first_draw_profile(NEKOPARA)).as_slice(), expected);
    }

    #[test]
    fn nekopara_scene01_wipe_uses_script_duration_and_depth() {
        let depth = 45_000f32.to_bits();
        let args = [12u32, 58, 0, depth, 1332].map(u32::to_le_bytes).concat();
        let expected = [0, depth, 58, 1332, 0].map(u32::to_le_bytes).concat();
        let unchanged = [58, 0, depth, 1332, 0].map(u32::to_le_bytes).concat();
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(NEKOPARA))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(KOKONATSU))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(V401))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(None)).as_slice(), unchanged);
    }

    #[test]
    fn yukikoi_wipe04_uses_recorded_530_abi() {
        let args = [39u32, 28, 0, 0x472f_c800, 265].map(u32::to_le_bytes).concat();
        let expected = [0u32, 0x472f_c800, 28, 265, 0].map(u32::to_le_bytes).concat();
        let unchanged = [28u32, 0, 0x472f_c800, 265, 0].map(u32::to_le_bytes).concat();
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(YUKIKOI))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(V401))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(None)).as_slice(), unchanged);
    }

    #[test]
    fn labyrinth_draw_uses_duration_before_mode_and_depth() {
        let args = [30u32, 58, 0, 45_000f32.to_bits()].map(u32::to_le_bytes).concat();
        let record = draw_configuration(&args, duration_first_draw_profile(GRISAIA_LABYRINTH));
        assert_eq!(dword(&record, 0).unwrap(), 0);
        assert_eq!(f32::from_bits(dword(&record, 4).unwrap()), 45_000.);
        assert_eq!(dword(&record, 8).unwrap(), 58);
        assert_eq!(draw_configuration(&args, duration_first_draw_profile(V401)), record);
        assert_eq!(draw_configuration(&args, false), args[4..]);
    }

    #[test]
    fn island_rdraw60_composes_through_script_duration() -> Result<()> {
        let duration = 58u32;
        let split_depth = 45_000f32;
        let recorded_args = [12, duration, 0, split_depth.to_bits()]
            .map(u32::to_le_bytes)
            .concat();
        let expected = [0, split_depth.to_bits(), duration]
            .map(u32::to_le_bytes)
            .concat();
        assert_eq!(
            draw_configuration(&recorded_args, duration_first_draw_profile(ISLAND)).as_slice(),
            expected
        );
        let other_island = "7bd9f6dcc7d62868d4ad9cc68d5580452e71bd907540ec4f993d960d051d2c45";
        assert!(!duration_first_draw_profile(other_island));

        let game = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games/ISLAND");
        if !game.is_dir() {
            return Ok(());
        }
        let profile = std::env::temp_dir().join(format!("cs2-island-rdraw60-{}", std::process::id()));
        let mut player = crate::HeadlessPlayer::open_with_save_root(&game, profile)?;
        player.with_host(|host| {
            assert_eq!(host.data_services.native_profile.map(|profile| profile.pe_sha256), Some(ISLAND));
            let id = host.create_task_object(38, 0, b"rdraw", b"")?;
            let mut args = recorded_args;
            args[..4].copy_from_slice(&id.to_le_bytes());
            let mut vm = Vm::new();
            assert_eq!(host.render_tasks_native(527, &args, &mut vm)?, NativeReturn::Continue);
            let origin = host.data_services.tasks.task_clock(id).unwrap().origin;
            let activation = origin.wrapping_add(41);
            host.data_services.tasks.set_task_clock(id, activation)?;
            let enable = [id, 1].map(u32::to_le_bytes).concat();
            assert_eq!(host.render_tasks_native(528, &enable, &mut vm)?, NativeReturn::Continue);
            let task = &host.data_services.tasks.render.tasks[&id];
            assert_eq!((task.duration, task.depth, task.started, task.end), (duration, split_depth, 41, 99));
            let [width, height] = task.size;
            let mut frame = Canvas::new(width, height)?;
            let foreground = Color::rgba(0, 128, 255, 255);
            let query = id.to_le_bytes();
            for elapsed in [0, duration / 2, duration - 1, duration] {
                host.data_services.tasks.set_task_clock(id, activation.wrapping_add(elapsed))?;
                host.data_services.tasks.render.update(id, 41 + elapsed)?;
                assert_eq!(host.render_tasks_native(520, &query, &mut vm)?, NativeReturn::Value(1));
                let mut ranges = Vec::new();
                host.data_services.tasks.render.compose(&mut frame, &mut |canvas, lower, upper| {
                    ranges.push((lower, upper));
                    if (lower..upper).contains(&10.) {
                        canvas.clear(Color::rgba(255, 255, 255, 255));
                    }
                    if (lower..upper).contains(&50_000.) {
                        canvas.fill_rect(cs2_renderer::Rect::new(1, 0, 1, 1), foreground)?;
                    }
                    Ok(())
                })?;
                assert_eq!(ranges, [(0., split_depth), (split_depth, 1_000_000.)]);
                assert_eq!(frame.pixel(1, 0)?, foreground);
                assert_eq!(frame.pixel(0, 0)?.r, (elapsed * 255 / duration) as u8);
                assert_eq!(
                    host.render_tasks_native(520, &query, &mut vm)?,
                    NativeReturn::Value(u32::from(elapsed < duration))
                );
            }
            Ok(())
        })?;
        Ok(())
    }

    #[test]
    fn island_wipe05b_uses_recorded_530_abi() {
        let depth = 45_000f32.to_bits();
        let args = [12u32, 28, 0, depth, 1181].map(u32::to_le_bytes).concat();
        let expected = [0u32, depth, 28, 1181, 0].map(u32::to_le_bytes).concat();
        let unchanged = [28u32, 0, depth, 1181, 0].map(u32::to_le_bytes).concat();
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(ISLAND))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(V401))).as_slice(), expected);
        assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(None)).as_slice(), unchanged);
    }

    #[test]
    fn yukikoi_rdraw60_keeps_warmup_and_composes_to_completion() -> Result<()> {
        let split_depth = 45_000f32;
        let recorded = [119u32, 58, 0, 0x472f_c800].map(u32::to_le_bytes).concat();
        let expected = [0, split_depth.to_bits(), 58].map(u32::to_le_bytes).concat();
        assert_eq!(draw_configuration(&recorded, duration_first_draw_profile(YUKIKOI)).as_slice(), expected);

        let game = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games/Yukikoi Melt");
        if !game.is_dir() {
            return Ok(());
        }
        let profile = std::env::temp_dir().join(format!("cs2-yukikoi-rdraw60-{}", std::process::id()));
        let mut player = crate::HeadlessPlayer::open_with_save_root(&game, profile)?;
        player.with_host(|host| {
            assert_eq!(host.data_services.native_profile.map(|profile| profile.pe_sha256), Some(YUKIKOI));
            let id = host.create_task_object(38, 0, b"rdraw", b"")?;
            let mut args = recorded;
            args[..4].copy_from_slice(&id.to_le_bytes());
            let mut vm = Vm::new();
            assert_eq!(host.render_tasks_native(527, &args, &mut vm)?, NativeReturn::Continue);

            let started = 41u32;
            let activation = host.data_services.tasks.task_clock(id).unwrap().origin.wrapping_add(started);
            host.data_services.tasks.set_task_clock(id, activation)?;
            let enable = [id, 1].map(u32::to_le_bytes).concat();
            assert_eq!(host.render_tasks_native(528, &enable, &mut vm)?, NativeReturn::Continue);
            let task = &host.data_services.tasks.render.tasks[&id];
            assert_eq!((task.duration, task.depth, task.warmup_frames), (58, split_depth, Some(0)));
            let [width, height] = task.size;
            let mut frame = Canvas::new(width, height)?;
            let foreground = Color::rgba(0, 128, 255, 255);
            let query = id.to_le_bytes();
            for (elapsed, red) in [(0, 0), (1, 0), (2, 0), (31, 127), (59, 250), (60, 255)] {
                host.data_services.tasks.set_task_clock(id, activation.wrapping_add(elapsed))?;
                host.data_services.tasks.render.update(id, started + elapsed)?;
                assert_eq!(host.render_tasks_native(520, &query, &mut vm)?, NativeReturn::Value(1));
                let mut ranges = Vec::new();
                host.data_services.tasks.render.compose(&mut frame, &mut |canvas, lower, upper| {
                    ranges.push((lower, upper));
                    if (lower..upper).contains(&10.) {
                        canvas.clear(Color::rgba(255, 255, 255, 255));
                    }
                    if (lower..upper).contains(&50_000.) {
                        canvas.fill_rect(cs2_renderer::Rect::new(1, 0, 1, 1), foreground)?;
                    }
                    Ok(())
                })?;
                assert_eq!(ranges, [(0., split_depth), (split_depth, 1_000_000.)]);
                assert_eq!(frame.pixel(0, 0)?.r, red);
                assert_eq!(frame.pixel(1, 0)?, foreground);
                assert_eq!(host.render_tasks_native(520, &query, &mut vm)?, NativeReturn::Value(u32::from(elapsed < 60)));
            }
            let task = &host.data_services.tasks.render.tasks[&id];
            assert_eq!((task.started, task.end), (started + 2, started + 60));
            Ok(())
        })?;
        Ok(())
    }

    #[test]
    fn eden_post_op_draw_uses_duration_before_mode_and_depth() {
        let args = [12u32, 28, 0, 45_000f32.to_bits()].map(u32::to_le_bytes).concat();
        let record = draw_configuration(&args, duration_first_draw_profile(GRISAIA_EDEN));
        assert_eq!(dword(&record, 0).unwrap(), 0);
        assert_eq!(f32::from_bits(dword(&record, 4).unwrap()), 45_000.);
        assert_eq!(dword(&record, 8).unwrap(), 28);
    }

    #[test]
    fn happiness2_registered_companion_render_uses_draw_and_wipe_abi() -> Result<()> {
        use cs2_script::Host;

        let game = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games/Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}");
        if !game.is_dir() {
            return Ok(());
        }
        let profile = std::env::temp_dir().join(format!("cs2-happiness2-draw-{}", std::process::id()));
        let mut player = crate::HeadlessPlayer::open_with_save_root(&game, profile)?;
        player.with_host(|host| {
            assert!(host.data_services.native_profile.is_none());
            let previous = host.create_task_object(38, 0, b"rdraw", b"")?;
            assert_eq!(host.data_services.tasks.render.callback_owner, Some(previous));
            assert_eq!(host.delete_task_native(&previous.to_le_bytes())?, NativeReturn::Continue);
            assert!(!host.data_services.tasks.render.has_composition());
            host.data_services.particle_message_pe_sha256 = None;
            let id = host.create_task_object(38, 0, b"rdraw", b"")?;
            assert!(host.update_ordinary_children(0, 1, 1, 20_000)?);
            assert!(!host.task_parents.contains_key(&previous));
            assert_eq!(host.data_services.tasks.render.callback_owner, Some(id));
            let duration = 48u32;
            let split_depth = 45_000f32;
            let args = [id, duration, 0, split_depth.to_bits()]
                .map(u32::to_le_bytes)
                .concat();
            assert_eq!(host.native_draw_configuration(&args)?, args[4..]);

            let mut vm = Vm::new();
            assert_eq!(
                host.call(527, &args, &mut vm)?,
                NativeReturn::Continue
            );
            let origin = host.data_services.tasks.task_clock(id).unwrap().origin;
            let started = 41u32;
            let activation = origin.wrapping_add(started);
            host.data_services.tasks.set_task_clock(id, activation)?;
            let enable = [id, 1].map(u32::to_le_bytes).concat();
            assert_eq!(
                host.render_tasks_native(528, &enable, &mut vm)?,
                NativeReturn::Continue
            );
            let task = &host.data_services.tasks.render.tasks[&id];
            assert_eq!(task.raw_configuration[..3], [0, split_depth.to_bits(), duration]);
            assert_eq!((task.started, task.end), (started, started + duration));

            let [width, height] = task.size;
            let mut frame = Canvas::new(width, height)?;
            let foreground = Color::rgba(0, 128, 255, 255);
            let query = id.to_le_bytes();
            for elapsed in [0, duration / 2, duration - 1, duration] {
                host.data_services.tasks.set_task_clock(id, activation.wrapping_add(elapsed))?;
                host.data_services.tasks.render.update(id, started + elapsed)?;
                assert_eq!(host.render_tasks_native(520, &query, &mut vm)?, NativeReturn::Value(1));
                host.data_services.tasks.render.compose(&mut frame, &mut |canvas, lower, upper| {
                    if (lower..upper).contains(&10.) {
                        canvas.clear(Color::rgba(255, 255, 255, 255));
                    }
                    if (lower..upper).contains(&50_000.) {
                        canvas.fill_rect(cs2_renderer::Rect::new(1, 0, 1, 1), foreground)?;
                    }
                    Ok(())
                })?;
                assert_eq!(frame.pixel(0, 0)?.r, (elapsed * 255 / duration) as u8);
                assert_eq!(frame.pixel(1, 0)?, foreground);
                assert_eq!(
                    host.render_tasks_native(520, &query, &mut vm)?,
                    NativeReturn::Value(u32::from(elapsed < duration))
                );
            }

            let wipe = host.create_task_object(39, 0, b"rwipe", b"")?;
            let mask = 506;
            host.image_handles.insert(mask, RgbaFrame::from_rgba8(1, 1, vec![200, 200, 200, 255])?);
            let wipe_duration = 58;
            let wipe_args = [wipe, wipe_duration, 0, split_depth.to_bits(), mask]
                .map(u32::to_le_bytes).concat();
            assert_eq!(host.call(530, &wipe_args, &mut vm)?, NativeReturn::Continue);
            let task = &host.data_services.tasks.render.tasks[&wipe];
            assert_eq!(task.raw_configuration[..5], [0, split_depth.to_bits(), wipe_duration, mask, 0]);
            assert_eq!(task.duration, wipe_duration);

            let before = task.raw_configuration;
            let invalid_mask = [wipe, 2, 0, split_depth.to_bits(), 0].map(u32::to_le_bytes).concat();
            assert_eq!(host.call(530, &invalid_mask, &mut vm)?, NativeReturn::Continue);
            assert_eq!(host.data_services.tasks.render.tasks[&wipe].raw_configuration, before);

            host.data_services.startup_companion_natives = None;
            let record_order = [wipe, 0, split_depth.to_bits(), 43, mask]
                .map(u32::to_le_bytes).concat();
            assert_eq!(host.call(530, &record_order, &mut vm)?, NativeReturn::Continue);
            assert_eq!(host.data_services.tasks.render.tasks[&wipe].duration, 43);
            Ok(())
        })
    }

    #[test]
    fn kanmi_sc01_draw_uses_duration_before_mode_and_depth() {
        let args = [28u32, 98, 0, 0x472f_c800].map(u32::to_le_bytes).concat();
        let expected = [0u32, 0x472f_c800, 98].map(u32::to_le_bytes).concat();
        assert_eq!(draw_configuration(&args, duration_first_draw_profile(KANMI)).as_slice(), expected);
    }

    #[test]
    fn kanmi_sc01_draw_finishes_on_verified_package_without_native_profile() {
        let game = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games")
            .join("\u{5c1a}\u{6709}\u{4f73}\u{871c}\u{4f34}\u{604b}\u{5fc3}");
        if !game.is_dir() {
            return;
        }
        let profile = std::env::temp_dir().join(format!("cs2-kanmi-draw-{}", std::process::id()));
        let mut player = crate::HeadlessPlayer::open_with_save_root(&game, profile).unwrap();
        player.with_host(|host| {
            assert!(host.data_services.native_profile.is_none());
            let id = host.create_task_object(38, 0, b"rdraw", b"")?;
            let args = [id, 98, 0, 45_000f32.to_bits()].map(u32::to_le_bytes).concat();
            let mut vm = Vm::new();
            assert_eq!(host.render_tasks_native(527, &args, &mut vm)?, NativeReturn::Continue);
            assert_eq!(host.data_services.tasks.render.unprofiled_duration_first_draw, Some(true));
            let enable = [id, 1].map(u32::to_le_bytes).concat();
            assert_eq!(host.render_tasks_native(528, &enable, &mut vm)?, NativeReturn::Continue);
            let task = &host.data_services.tasks.render.tasks[&id];
            assert_eq!(task.depth, 45_000.);
            assert_eq!(task.duration, 98);
            let [width, height] = task.size;
            let deadline = task.end;
            let mut frame = Canvas::new(width, height)?;
            let sky = Color::rgba(64, 128, 255, 255);
            for (now, expected_active) in [(deadline - 1, 1), (deadline, 0)] {
                let render = &mut host.data_services.tasks.render;
                render.update(id, now)?;
                render.compose(&mut frame, &mut |canvas, lower, upper| {
                    if (lower..upper).contains(&10.) {
                        canvas.clear(sky);
                    }
                    Ok(())
                })?;
                assert_eq!(host.render_tasks_native(520, &id.to_le_bytes(), &mut vm)?,
                    NativeReturn::Value(expected_active));
            }
            assert_eq!(frame.pixel(0, 0)?, sky);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn newton_draw_record_uses_verified_pe_order() {
        let args = [30u32, 238, 0, 45_000f32.to_bits()].map(u32::to_le_bytes).concat();
        let expected = [0u32, 45_000f32.to_bits(), 238].map(u32::to_le_bytes).concat();
        for pe in [NEWTON, NEWTON_LOCAL] {
            assert_eq!(draw_configuration(&args, duration_first_draw_profile(pe)).as_slice(), expected);
        }
        for unverified in [
            "35ec21bcfcee0f324206e945eef1c082e35fd8af594ea0e738a46cd941f5b3f3",
            "d158c8915d4982c82badc631b4f92e1f140b246df5929ff2d9924d242c66765c",
        ] {
            assert_eq!(draw_configuration(&args, duration_first_draw_profile(unverified)), args[4..]);
        }
    }

    #[test]
    fn newton_wipe_record_uses_verified_pe_order() {
        // Both 2.6.1.78 native tables point slot 530 to 0x608230.
        let depth = 45_000f32.to_bits();
        let args = [30u32, 90, 0, depth, 1231].map(u32::to_le_bytes).concat();
        let expected = [0u32, depth, 90, 1231, 0].map(u32::to_le_bytes).concat();
        for pe in [NEWTON, NEWTON_LOCAL] {
            assert_eq!(wipe_configuration(&args, duration_first_wipe_profile(Some(pe))).as_slice(), expected);
        }
    }

    #[test]
    fn verified_draw_profiles_compose_through_the_recorded_duration() -> Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        for (case_index, (game, expected_profile, duration)) in [
            ("\u{725b}\u{987f}\u{4e0e}\u{82f9}\u{679c}\u{6811}", Some(NEWTON), 238u32),
            ("\u{7a81}\u{7136}\u{4e4b}\u{95f4}\u{53d1}\u{73b0}\u{6211}\u{5df2}\u{604b}\u{4e0a}\u{4f60}", None, 238),
            ("\u{767d}\u{663c}\u{5922}\u{306e}\u{9752}\u{5199}\u{771f}", Some(HAKUCHUUMU), 298),
            (concat!("\u{672a}\u{6765}\u{30e9}\u{30b8}\u{30aa}\u{3068}\u{4eba}\u{5de5}\u{9ce9}",
                "\u{ff08}AUGUST\u{ff09}"), Some(AUGUST), 18),
            ("\u{7070}\u{8272}\u{7684}\u{679c}\u{5b9e}", None, 58),
            ("\u{79d8}\u{5bc6}\u{6e38}\u{620f} \u{53cd}\u{53db}", None, 58),
        ].into_iter().enumerate() {
            let game = games.join(game);
            if !game.is_dir() {
                continue;
            }
            let profile = std::env::temp_dir().join(format!("cs2-{duration}-draw-{}-{case_index}",
                std::process::id()));
            let mut player = crate::HeadlessPlayer::open_with_save_root(&game, profile)?;
            player.with_host(|host| {
                assert_eq!(host.data_services.native_profile.map(|profile| profile.pe_sha256), expected_profile);
                let id = host.create_task_object(38, 0, b"rdraw", b"")?;
                let args = [id, duration, 0, 45_000f32.to_bits()].map(u32::to_le_bytes).concat();
                let mut vm = Vm::new();
                assert_eq!(host.render_tasks_native(527, &args, &mut vm)?, NativeReturn::Continue);
                if let Some(table) = host.data_services.ordinary_native_table.as_ref() {
                    assert_eq!(
                        table.render_argument_order(527),
                        Some(RenderArgumentOrder::DurationFirst),
                        "{}",
                        game.display(),
                    );
                    assert_eq!(host.data_services.tasks.render.unprofiled_duration_first_draw, None);
                } else if expected_profile.is_none() {
                    assert_eq!(host.data_services.tasks.render.unprofiled_duration_first_draw, Some(true));
                }
                let origin = host.data_services.tasks.task_clock(id).unwrap().origin;
                let activation_tick = origin.wrapping_add(41);
                host.data_services.tasks.set_task_clock(id, activation_tick)?;
                let enable = [id, 1].map(u32::to_le_bytes).concat();
                assert_eq!(host.render_tasks_native(528, &enable, &mut vm)?, NativeReturn::Continue);
                let task = &host.data_services.tasks.render.tasks[&id];
                assert_eq!(task.depth, 45_000.);
                assert_eq!(task.duration, duration);
                assert_eq!(task.started, 41);
                assert_eq!(task.end, 41 + duration);
                let start = task.started;
                let [width, height] = task.size;
                let mut frame = Canvas::new(width, height)?;
                let background = Color::rgba(255, 255, 255, 255);
                let foreground = Color::rgba(0, 128, 255, 255);
                let query = id.to_le_bytes();
                for elapsed in [0, duration / 2, duration - 1, duration] {
                    host.data_services.tasks.set_task_clock(id, activation_tick.wrapping_add(elapsed))?;
                    host.data_services.tasks.render.update(id, start.wrapping_add(elapsed))?;
                    assert_eq!(host.render_tasks_native(520, &query, &mut vm)?, NativeReturn::Value(1));
                    let mut ranges = Vec::new();
                    host.data_services.tasks.render.compose(&mut frame, &mut |canvas, lower, upper| {
                        ranges.push((lower, upper));
                        if (lower..upper).contains(&10.) {
                            canvas.clear(background);
                        }
                        if (lower..upper).contains(&50_000.) {
                            canvas.fill_rect(cs2_renderer::Rect::new(1, 0, 1, 1), foreground)?;
                        }
                        Ok(())
                    })?;
                    assert_eq!(ranges, [(0., 45_000.), (45_000., 1_000_000.)]);
                    assert_eq!(frame.pixel(1, 0)?, foreground);
                    let red = frame.pixel(0, 0)?.r;
                    assert_eq!(red, (elapsed * 255 / duration) as u8);
                    assert_eq!(host.render_tasks_native(520, &query, &mut vm)?,
                        NativeReturn::Value(u32::from(elapsed < duration)));
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}
