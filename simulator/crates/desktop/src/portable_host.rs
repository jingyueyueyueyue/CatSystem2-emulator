//! Window-system-independent entry points used by mobile and embedded hosts.
//! Events pass through the same player/FES queues as the Windows window loop.
use anyhow::Result;
use cs2_runtime::{AudioHost, InputEvent, InputKey};
use std::cell::RefCell;
use std::time::{Duration, Instant};
use super::{HeadlessPlayer, PlayerFramePhase};

#[derive(Default)]
#[repr(C)]
pub struct HostFrameTiming {
    pub phase_ns: [u64; HostPhase::ComposeResourceRedraw as usize + 1],
}

#[derive(Clone, Copy)]
pub(super) enum HostPhase {
    Script,
    Read,
    Decode,
    Patch,
    Compose,
    ScriptInput,
    ScriptRoot,
    ScriptChildren,
    ScriptFes,
    ScriptList,
    ScriptListRaster,
    ScriptMotionRaster,
    ScriptButton,
    ScriptRead,
    ScriptParse,
    OpenScan,
    OpenInitialize,
    OpenStartup,
    ComposeLayers,
    ComposeText,
    ComposeEmote,
    ComposeTransition,
    ComposePlane,
    ComposeKx,
    ComposeResourceRedraw,
}

#[derive(Default)]
struct TimingState {
    sample: HostFrameTiming,
    active: Option<(HostPhase, Instant)>,
}

thread_local! {
    static FRAME_TIMING: RefCell<Option<TimingState>> = const { RefCell::new(None) };
}

struct TimingCapture(Option<TimingState>);

impl Drop for TimingCapture {
    fn drop(&mut self) {
        FRAME_TIMING.with(|state| *state.borrow_mut() = self.0.take());
    }
}

impl HostFrameTiming {
    pub fn capture<T>(operation: impl FnOnce() -> T) -> (T, Self) {
        let previous = FRAME_TIMING.with(|state| state.replace(Some(TimingState::default())));
        let _capture = TimingCapture(previous);
        let result = operation();
        let sample = FRAME_TIMING.with(|state| state.take().unwrap_or_default().sample);
        (result, sample)
    }
}

impl TimingState {
    fn switch(&mut self, next: Option<HostPhase>) -> Option<HostPhase> {
        let now = Instant::now();
        let previous = self.active.map(|(phase, start)| {
            let slot = &mut self.sample.phase_ns[phase as usize];
            *slot += now.duration_since(start).as_nanos() as u64;
            phase
        });
        self.active = next.map(|phase| (phase, now));
        previous
    }
}

pub(super) struct HostPhaseTimer(Option<Option<HostPhase>>);

impl Drop for HostPhaseTimer {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            FRAME_TIMING.with(|state| {
                if let Some(timing) = state.borrow_mut().as_mut() {
                    timing.switch(previous);
                }
            });
        }
    }
}

fn measure_phase(phase: HostPhase, script_only: bool) -> HostPhaseTimer {
    HostPhaseTimer(FRAME_TIMING.with(|state| {
        let mut state = state.borrow_mut();
        let timing = state.as_mut()?;
        if script_only && !timing.active.is_some_and(|(parent, _)| {
            matches!(parent, HostPhase::Script)
                || (HostPhase::ScriptInput as usize..=HostPhase::ScriptButton as usize).contains(&(parent as usize))
        }) {
            return None;
        }
        Some(timing.switch(Some(phase)))
    }))
}

pub(super) fn measure_host_phase(phase: HostPhase) -> HostPhaseTimer {
    measure_phase(phase, false)
}

pub(super) fn measure_host_operation<T>(phase: HostPhase, operation: impl FnOnce() -> T) -> T {
    let _timing = measure_host_phase(phase);
    operation()
}

pub(super) fn measure_script_detail(phase: HostPhase) -> HostPhaseTimer {
    measure_phase(phase, true)
}

impl HeadlessPlayer {
    /// Enable the hardware raster backend on this interactive host thread.
    pub fn enable_host_gpu_raster() {
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        super::gpu_raster::enable();
    }

    /// Release DX12 objects before the Windows thread begins TLS teardown.
    #[cfg(windows)]
    pub fn disable_host_gpu_raster() {
        #[cfg(feature = "gpu-raster")]
        super::gpu_raster::disable();
    }

    /// Latest completed hardware raster batch on this host thread.
    pub fn latest_host_gpu_timing() -> Option<(u64, u64)> {
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        return super::gpu_raster::latest_gpu_timing();
        #[cfg(not(all(feature = "gpu-raster", any(windows, target_os = "android"))))]
        None
    }

    pub fn host_device_frame_revision(&self) -> Option<u64> {
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        return super::gpu_raster::frame_revision(&self.canvas);
        #[cfg(not(all(feature = "gpu-raster", any(windows, target_os = "android"))))]
        None
    }

    /// The caller keeps a native window reference for this call.
    #[cfg(target_os = "android")]
    pub fn android_surface_candidate(window: std::ptr::NonNull<std::ffi::c_void>) -> bool {
        #[cfg(feature = "gpu-raster")]
        return super::gpu_raster::surface_candidate(window);
        #[cfg(not(feature = "gpu-raster"))]
        { let _ = window; false }
    }

    /// The caller must hold a live ANativeWindow. The presenter acquires its own
    /// reference before retaining a surface.
    #[cfg(target_os = "android")]
    pub unsafe fn present_android_surface(
        &self, window: std::ptr::NonNull<std::ffi::c_void>, session: u64, input_mono_us: &[(u64, u64)],
    ) -> i32 {
        #[cfg(feature = "gpu-raster")]
        return super::gpu_raster::present_canvas(window, &self.canvas, session, input_mono_us);
        #[cfg(not(feature = "gpu-raster"))]
        { let _ = (window, input_mono_us); 1 }
    }

    #[cfg(target_os = "android")]
    pub fn detach_android_surface() {
        #[cfg(feature = "gpu-raster")]
        super::gpu_raster::detach();
    }

    #[cfg(target_os = "android")]
    pub fn set_android_vulkan_display_stats(enabled: bool) {
        #[cfg(feature = "gpu-raster")]
        super::gpu_raster::set_display_stats_enabled(enabled);
        #[cfg(not(feature = "gpu-raster"))]
        let _ = enabled;
    }

    #[cfg(target_os = "android")]
    pub fn android_vulkan_display_stats() -> Option<[u64; 7]> {
        #[cfg(feature = "gpu-raster")]
        return super::gpu_raster::display_stats();
        #[cfg(not(feature = "gpu-raster"))]
        None
    }

    pub fn disable_script_evidence_capture(&mut self) {
        self.capture_script_evidence = false;
    }

    pub fn set_host_windowed_mode(&mut self, mode: u32) {
        self.data_services.configuration.set_actual_window_mode(mode);
        self.data_services.fes_settings.windowed_mode = Some(mode);
        self.data_services.fes_settings.set_presented_fullscreen(u32::from(mode == 0));
    }

    pub fn frame_in_progress(&self) -> bool {
        self.frame_phase != PlayerFramePhase::Start
    }

    pub fn quit_requested(&self) -> bool {
        self.state.input.quit_requested || self.data_services.host_quit_requested
    }

    /// Borrow the completed CPU frame. Hosts on the engine thread can copy it
    /// without allocating and hashing a diagnostic FrameSnapshot every frame.
    pub fn frame_rgba(&self) -> &[u8] {
        self.canvas.pixels()
    }

    pub(super) fn message_plane_rgb_probe(
        &self,
        object: &cs2_runtime::message_object::MessageObject,
        node: &super::SceneNode,
        submitted: &cs2_renderer::Canvas,
    ) -> Option<((usize, usize), [i32; 2], (usize, usize))> {
        let (registered_size, registered_offset) = object.registered_geometry()?;
        let size = if node.initialized { node.size } else { registered_size.map(|val| val as f32) };
        if !node.visible || node.scale[..2] != [1.0, 1.0] || node.color != u32::MAX
            || node.overlay_color != 0 || size != [submitted.width() as f32, submitted.height() as f32]
            || node.position.iter().any(|coordinate| !coordinate.is_finite())
        {
            return None;
        }
        let current_offset = object.configuration()?.canvas_offset();
        let draw_origin = [0, 1].map(|axis| (node.position[axis] as i32).wrapping_add(registered_offset[axis]));
        let mut tested = 0;
        let mut matched = 0;
        let mut texture_clipped = 0;
        let mut viewport_clipped = 0;
        for (index, pixel) in submitted.pixels().chunks_exact(4).enumerate().filter(|(_, pixel)| pixel[3] == 255) {
            let local = [(index % submitted.width() as usize) as i64, (index / submitted.width() as usize) as i64];
            let shifted = [0, 1].map(|axis| local[axis] + i64::from(current_offset[axis]) - i64::from(registered_offset[axis]));
            if !(0..i64::from(submitted.width())).contains(&shifted[0])
                || !(0..i64::from(submitted.height())).contains(&shifted[1])
            {
                texture_clipped += 1;
                continue;
            }
            let screen = [0, 1].map(|axis| draw_origin[axis].wrapping_add(shifted[axis] as i32));
            if let Ok(output) = self.canvas.pixel(screen[0], screen[1]) {
                tested += 1;
                matched += usize::from([output.r, output.g, output.b] == pixel[..3]);
            } else {
                viewport_clipped += 1;
            }
        }
        Some(((matched, tested), draw_origin, (texture_clipped, viewport_clipped)))
    }

    pub fn host_composition_diagnostic(&self) -> String {
        use std::fmt::Write;

        let planes = &self.data_services.planes;
        let mut out = String::new();
        for node in self.scene_nodes.values().filter(|node| node.kind == 2 && node.initialized) {
            let source = planes.source_position(node.id, 1);
            let sample = source.and_then(|origin| {
                let x = (origin[0] + node.size[0] / 2.0) as i32;
                let y = (origin[1] + node.size[1] / 2.0) as i32;
                self.graphics_canvas.pixel(x, y).ok().or_else(|| {
                    planes.vram_overflow.as_ref()?.pixel(x.try_into().ok()?, y.try_into().ok()?)
                })
            });
            let depth = super::plane_native::plane_depth(node, planes);
            let shape = planes.state(node.id).map(|state| state.shape);
            let _ = writeln!(out,
                "plane {} {} parent={} visible={} shape={shape:?} position={:?} size={:?} depth={depth} color={:#010x} source={source:?} sample={sample:?}",
                node.id, node.name, node.parent, node.visible, node.position, node.size, node.color);
        }
        for (&id, object) in self.data_services.messages.objects() {
            let Some(node) = self.scene_nodes.get(&id) else { continue };
            let Some(surface) = self.data_services.message_device.surface(id as i32) else { continue };
            let task = self.data_services.tasks.message_task_state(id);
            let geometry = object.registered_geometry();
            let submitted_alpha = surface.submitted.pixels().chunks_exact(4)
                .filter(|pixel| pixel[3] != 0).count();
            let projected = self.project_message_plane(node).ok();
            let projected_position = projected.as_ref().map(|plane| plane.position);
            let opaque_final_rgb_probe = projected.as_deref()
                .and_then(|node| self.message_plane_rgb_probe(object, node, surface.submitted));
            let opaque_final_rgb_matches = opaque_final_rgb_probe.map(|probe| probe.0);
            let draw_origin = opaque_final_rgb_probe.map(|probe| probe.1);
            let opaque_texture_viewport_clipped = opaque_final_rgb_probe.map(|probe| probe.2);
            let _ = writeln!(out,
                "message plane {id} {} kind={} parent={} visible={} position={:?} size={:?} scale={:?} color={:#010x} overlay={:#010x} task={task:?} geometry={geometry:?} generation={} submitted_alpha={submitted_alpha} opaque_final_rgb_matches={opaque_final_rgb_matches:?} projected_position={projected_position:?} draw_origin={draw_origin:?} opaque_texture_viewport_clipped={opaque_texture_viewport_clipped:?}",
                node.name, node.kind, node.parent, node.visible, node.position, node.size,
                node.scale, node.color, node.overlay_color, surface.generation);
        }
        for (id, list) in &self.data_services.tasks.fes.platform.lists {
            let output = self.image_handles.get(&list.output);
            let _ = writeln!(out, "list {id} size={:?} offset={:?} enabled={} submit={} force={} nonzero_alpha={:?}",
                list.size, list.offset, list.enabled, list.submit, list.force,
                output.map(|frame| frame.pixels().chunks_exact(4).filter(|pixel| pixel[3] != 0).count()));
            for ((owner, name), layer) in list.layers.iter().filter(|(_, layer)| layer.options.visible) {
                let _ = writeln!(out, "layer {owner} {name:?} position={:?} size=[{}, {}] depth={} opacity={} clip={:?} nonzero_alpha={}",
                    layer.options.position, layer.frame.width(), layer.frame.height(), layer.priority,
                    layer.options.opacity, layer.options.clip,
                    layer.frame.pixels().chunks_exact(4).filter(|pixel| pixel[3] != 0).count());
            }
        }
        out
    }

    /// Interactive hosts retain recent diagnostic context instead of keeping
    /// every native call for an entire multi-hour game in memory. CLI evidence
    /// collection leaves its original, complete event stream unchanged.
    pub fn limit_host_diagnostics(&mut self, event_limit: usize, log_limit: usize) {
        if self.events.len() > event_limit.saturating_mul(2) {
            self.events.drain(..self.events.len().saturating_sub(event_limit));
        }
        if self.log.len() > log_limit.saturating_mul(2) {
            self.log.drain(..self.log.len().saturating_sub(log_limit));
        }
    }

    pub fn apply_host_input(&mut self, event: InputEvent) -> Result<()> {
        if self.frame_in_progress() && !matches!(event, InputEvent::FocusChanged(_) | InputEvent::QuitRequested) {
            if self.data_services.pending_host_input.len() >= 4096 {
                anyhow::bail!("platform input queue exceeds 4096 events");
            }
            self.data_services.pending_host_input.push_back(event);
            return Ok(());
        }
        if let InputEvent::KeyDown(InputKey::MouseButton(button)) = &event
            && self.state.input.mouse_button_pressed(*button)
        {
            return Ok(());
        }
        let focus = if let InputEvent::FocusChanged(value) = &event { Some(*value) } else { None };
        self.observe_host_device_input(&event)?;
        match &event {
            InputEvent::KeyDown(InputKey::MouseButton(button)) => {
                let code = match button { 0 => Some(2), 1 => Some(3), _ => None };
                if let Some(code) = code {
                    self.queue_message([code, self.state.input.cursor_x as u32, self.state.input.cursor_y as u32, 0]);
                    self.queue_host_fes_event(code, &event);
                }
            }
            InputEvent::KeyUp(InputKey::MouseButton(button)) => {
                if let Some(code) = match button { 0 => Some(4), 1 => Some(5), _ => None } {
                    self.queue_host_fes_event(code, &event);
                }
            }
            InputEvent::KeyDown(InputKey::MouseWheel(delta)) if *delta != 0 => {
                self.queue_host_fes_event(if *delta > 0 { 6 } else { 7 }, &event);
            }
            InputEvent::KeyDown(InputKey::Keyboard(key)) if !self.state.input.key_pressed(*key) => {
                if matches!(key, 13 | 32) {
                    self.queue_message([2, 0, 0, 0]);
                    self.queue_host_fes_event(2, &event);
                } else if let Some(code) = arrow_event(*key) {
                    self.queue_host_fes_event(code, &event);
                }
            }
            InputEvent::KeyUp(InputKey::Keyboard(key)) => {
                if let Some(code) = arrow_event(*key) { self.queue_host_fes_event(code + 4, &event); }
            }
            _ => {}
        }
        self.state.apply_input(event);
        if let Some(focused) = focus {
            let suspended = self.data_services.configuration.window_sound_suspended(focused);
            self.set_window_audio_suspended(suspended)?;
        }
        Ok(())
    }

    pub(super) fn apply_pending_host_input(&mut self) -> Result<()> {
        while let Some(event) = self.data_services.pending_host_input.pop_front() {
            self.apply_host_input(event)?;
        }
        Ok(())
    }

    /// One UI iteration may only finish part of a script frame. Device polling
    /// continues, while input edge bits are committed only after that frame
    /// really finishes. The next frame consumes events queued during a yield.
    pub fn advance_host_frame(&mut self, instruction_budget: usize) -> Result<bool> {
        {
            let _timing = measure_host_phase(HostPhase::Script);
            if !self.advance_posted_window_commands(instruction_budget)? { return Ok(false); }
        }
        if self.quit_requested() { return Ok(false); }
        self.movie_host.poll()?;
        self.audio_host.poll()?;
        let before = self.state.clock.frame;
        let continued = self.frame_in_progress();
        self.run_frames(1, instruction_budget)?;
        let completed = !self.frame_in_progress() && (continued || self.state.clock.frame != before);
        if completed { self.state.input.commit(); }
        Ok(completed)
    }

    pub fn advance_host_frame_with_retries(&mut self, instruction_budget: usize) -> Result<bool> {
        let start = Instant::now();
        if self.advance_host_frame(instruction_budget)? { return Ok(true); }
        for _ in 0..3 {
            if !self.frame_in_progress() || self.quit_requested() || self.data_services.host_pause.paused
                || start.elapsed() >= Duration::from_millis(2)
            { break; }
            if self.advance_host_frame(instruction_budget)? { return Ok(true); }
        }
        Ok(false)
    }
}

fn arrow_event(key: u32) -> Option<u32> {
    match key { 38 => Some(11), 40 => Some(12), 37 => Some(13), 39 => Some(14), _ => None }
}
