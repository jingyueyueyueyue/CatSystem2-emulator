use super::{RASTER, Raster, buffer, device_frame, report};
use anyhow::{Context, Result, bail};
use cs2_renderer::Canvas;
use std::cell::Cell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    static DISPLAY_STATS_ENABLED: Cell<bool> = const { Cell::new(false) };
}

struct PendingDisplay {
    session: u64,
    id: u32,
    submitted_ns: u64,
    inputs: Vec<(u64, u64)>,
}

struct DisplayTiming {
    api: Option<ash::google::display_timing::Device>,
    generation: u64,
    next_id: u32,
    frames_since_poll: u8,
    last_poll_ns: u64,
    pending: VecDeque<PendingDisplay>,
    displayed: u64,
    latest_ns: u64,
    queue_to_display_ns: u64,
    unresolved: u64,
    query_failed: bool,
}

impl DisplayTiming {
    fn new(device: &wgpu::Device) -> Self {
        let api = if device.features().contains(wgpu::Features::VULKAN_GOOGLE_DISPLAY_TIMING) {
            // The feature enables the device functions on the presenter's Vulkan device.
            unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }.map(|hal| {
                ash::google::display_timing::Device::new(hal.shared_instance().raw_instance(), hal.raw_device())
            })
        } else {
            None
        };
        let mut timing = Self {
            api, generation: 0, next_id: 1, frames_since_poll: 0, last_poll_ns: 0,
            pending: VecDeque::new(),
            displayed: 0, latest_ns: 0, queue_to_display_ns: 0, unresolved: 0, query_failed: false,
        };
        timing.reset();
        timing
    }

    fn reset(&mut self) {
        static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
        self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        for pending in self.pending.drain(..) {
            unresolved_inputs(&pending, "surface_reset");
        }
        self.frames_since_poll = 0;
        self.last_poll_ns = 0;
        self.displayed = 0;
        self.latest_ns = 0;
        self.queue_to_display_ns = 0;
        self.unresolved = 0;
        self.query_failed = false;
    }

    fn supported(&self) -> bool {
        self.api.is_some() && !self.query_failed
    }

    fn on_submit(&mut self, surface: &wgpu::Surface<'_>, session: u64, inputs: &[(u64, u64)]) {
        if !self.supported() {
            unresolved_direct(session, inputs, "display_timing_unsupported");
            return;
        }
        if inputs.is_empty() && !DISPLAY_STATS_ENABLED.with(Cell::get) { return }
        self.frames_since_poll += 1;
        let now = monotonic_ns();
        // Bound the interval at low submission rates as well as the frame count.
        if self.frames_since_poll >= 4 || now.saturating_sub(self.last_poll_ns) >= 100_000_000 {
            self.poll(surface);
        }
        if self.query_failed {
            unresolved_direct(session, inputs, "display_timing_query_failed");
            return;
        }
        let Some(hal) = (unsafe { surface.as_hal::<wgpu::hal::api::Vulkan>() }) else {
            unresolved_direct(session, inputs, "non_vulkan_surface");
            return;
        };
        let id = self.next_id;
        self.next_id = id.wrapping_add(1).max(1);
        // The acquired surface frame keeps this swapchain configured. Zero
        // desired time tags the frame without requesting a scheduling delay.
        hal.set_next_present_time(ash::vk::PresentTimeGOOGLE::default().present_id(id));
        if self.pending.len() == 32 {
            let pending = self.pending.pop_front().unwrap();
            unresolved_inputs(&pending, "present_queue_overflow");
            self.unresolved += 1;
        }
        self.pending.push_back(PendingDisplay {
            session, id, submitted_ns: monotonic_ns(), inputs: inputs.to_vec(),
        });
    }

    fn poll(&mut self, surface: &wgpu::Surface<'_>) {
        self.frames_since_poll = 0;
        if self.pending.is_empty() || !self.supported() { return; }
        self.last_poll_ns = monotonic_ns();
        let Some(hal) = (unsafe { surface.as_hal::<wgpu::hal::api::Vulkan>() }) else { return; };
        let Some(swapchain) = hal.raw_swapchain() else { return; };
        // No other thread presents or reconfigures this surface.
        let result = unsafe { self.api.as_ref().unwrap().get_past_presentation_timing(swapchain) };
        match result {
            Ok(rows) => {
                for row in rows {
                    let Some(index) = self.pending.iter().position(|pending| pending.id == row.present_id) else { continue; };
                    if row.actual_present_time == 0 { continue; }
                    let pending = self.pending.remove(index).unwrap();
                    self.displayed += 1;
                    if row.actual_present_time > self.latest_ns {
                        self.latest_ns = row.actual_present_time;
                        self.queue_to_display_ns = row.actual_present_time.saturating_sub(pending.submitted_ns);
                    }
                    displayed_inputs(&pending, row.actual_present_time);
                }
            }
            Err(error) => {
                self.unresolved += self.pending.len() as u64;
                for pending in self.pending.drain(..) {
                    unresolved_inputs(&pending, "display_timing_query_failed");
                }
                self.query_failed = true;
                report(&format!("vulkan_display_timing=query_failed reason={error}"));
            }
        }
        let now = monotonic_ns();
        while self.pending.front().is_some_and(|pending| now.saturating_sub(pending.submitted_ns) > 2_500_000_000) {
            let pending = self.pending.pop_front().unwrap();
            unresolved_inputs(&pending, "present_timeout");
            self.unresolved += 1;
        }
    }

    fn snapshot(&self) -> [u64; 7] {
        // Values follow Cs2DisplaySource in egl_presenter.h.
        let source = if !self.supported() { 5 } else if self.displayed == 0 { 6 } else { 7 };
        [source, self.generation, self.displayed, self.latest_ns, self.queue_to_display_ns,
            self.unresolved, self.pending.len() as u64]
    }
}

fn monotonic_ns() -> u64 {
    let mut now = std::mem::MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, now.as_mut_ptr()) } != 0 { return 0; }
    let now = unsafe { now.assume_init() };
    now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64
}

fn photon_report(message: &str) {
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: std::ffi::c_int, tag: *const std::ffi::c_char,
            text: *const std::ffi::c_char) -> std::ffi::c_int;
    }
    if let Ok(message) = std::ffi::CString::new(message) {
        unsafe { __android_log_write(4, c"CS2Perf".as_ptr(), message.as_ptr()); }
    }
}

fn unresolved_direct(session: u64, inputs: &[(u64, u64)], reason: &str) {
    for &(input_id, input) in inputs {
        photon_report(&format!(
            "input_photon_unresolved session={session} input_id={input_id} input_mono_us={input} reason={reason}"
        ));
    }
}

fn unresolved_inputs(pending: &PendingDisplay, reason: &str) {
    unresolved_direct(pending.session, &pending.inputs, reason);
}

fn displayed_inputs(pending: &PendingDisplay, actual_present_ns: u64) {
    let submit_to_photon_us = actual_present_ns.saturating_sub(pending.submitted_ns) / 1000;
    for &(input_id, input) in &pending.inputs {
        let Some(input_ns) = input.checked_mul(1000) else {
            unresolved_direct(pending.session, &[(input_id, input)], "clock_overflow");
            continue;
        };
        if actual_present_ns < input_ns {
            unresolved_direct(pending.session, &[(input_id, input)], "clock_order");
            continue;
        }
        photon_report(&format!(
            "input_photon session={} input_id={input_id} input_mono_us={input} present_id={} actual_present_ns={actual_present_ns} input_to_photon_us={} submit_to_photon_us={submit_to_photon_us}",
            pending.session, pending.id, (actual_present_ns - input_ns) / 1000,
        ));
    }
}

#[link(name = "android")]
unsafe extern "C" {
    fn ANativeWindow_acquire(window: *mut c_void);
    fn ANativeWindow_release(window: *mut c_void);
    fn ANativeWindow_getWidth(window: *mut c_void) -> i32;
    fn ANativeWindow_getHeight(window: *mut c_void) -> i32;
}

struct Window(NonNull<c_void>);

impl Drop for Window {
    fn drop(&mut self) {
        unsafe { ANativeWindow_release(self.0.as_ptr()) }
    }
}

pub(super) struct Presenter {
    // Drop the surface (the BufferQueue producer) before releasing the window.
    surface: wgpu::Surface<'static>,
    window: Window,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    logical_buffer: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    config: wgpu::SurfaceConfiguration,
    display_timing: DisplayTiming,
}

impl Presenter {
    fn size(window: NonNull<c_void>) -> Option<[u32; 2]> {
        let [width, height] = unsafe {
            [ANativeWindow_getWidth(window.as_ptr()), ANativeWindow_getHeight(window.as_ptr())]
        };
        (width > 0 && height > 0).then_some([width as u32, height as u32])
    }

    fn new(raster: &Raster, window: NonNull<c_void>, [width, height]: [u32; 2]) -> Result<Self> {
        unsafe { ANativeWindow_acquire(window.as_ptr()) };
        let window = Window(window);
        let surface = unsafe { raster.instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: wgpu::rwh::RawDisplayHandle::Android(wgpu::rwh::AndroidDisplayHandle::new()),
            raw_window_handle: wgpu::rwh::RawWindowHandle::AndroidNdk(wgpu::rwh::AndroidNdkWindowHandle::new(window.0)),
        }) }.context("create Android Vulkan window surface")?;
        let capabilities = surface.get_capabilities(&raster.adapter);
        let format = capabilities.formats.iter().copied()
            .find(|format| matches!(format, wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm))
            .or_else(|| capabilities.formats.iter().copied().find(wgpu::TextureFormat::is_srgb))
            .context("Vulkan game surface has no RGBA/BGRA color format")?;
        let alpha_mode = capabilities.alpha_modes.iter().copied()
            .find(|mode| *mode == wgpu::CompositeAlphaMode::Opaque)
            .or_else(|| capabilities.alpha_modes.first().copied()).context("Vulkan surface has no alpha mode")?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, format, width, height,
            present_mode: wgpu::PresentMode::Fifo, desired_maximum_frame_latency: 2,
            alpha_mode, view_formats: Vec::new(),
        };
        let device = &raster.device;
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(device, &config);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("CS2 window presentation"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_raster_present.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("CS2 Vulkan game window"),
            layout: None,
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vertex"),
                buffers: &[], compilation_options: Default::default() },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fragment"),
                targets: &[Some(format.into())], compilation_options: Default::default() }),
            multiview: None, cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            bail!("configure Android Vulkan presentation: {error}");
        }
        let layout = pipeline.get_bind_group_layout(0);
        let logical_buffer = buffer(device, 4, super::UPLOAD);
        let uniforms = buffer(device, 32, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        report(&format!("present=android_surface backend=vulkan width={width} height={height} format={format:?}"));
        let display_timing = DisplayTiming::new(device);
        Ok(Self { surface, window, pipeline, layout, logical_buffer, uniforms, config, display_timing })
    }

    fn render(&mut self, raster: &Raster, canvas: &Canvas, session: u64, inputs: &[(u64, u64)]) -> Result<bool> {
        let Some([width, height]) = Self::size(self.window.0) else { return Ok(false) };
        if [width, height] != [self.config.width, self.config.height] {
            self.config.width = width;
            self.config.height = height;
            raster.device.push_error_scope(wgpu::ErrorFilter::Validation);
            self.surface.configure(&raster.device, &self.config);
            if let Some(error) = pollster::block_on(raster.device.pop_error_scope()) {
                bail!("resize Android Vulkan window: {error}");
            }
            self.display_timing.reset();
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Timeout) => return Ok(false),
            Err(wgpu::SurfaceError::Outdated) => {
                self.config.width = 0;
                return Ok(false);
            }
            Err(error) => return Err(error).context("acquire Android Vulkan window frame"),
        };
        let size = u64::from(canvas.width()) * u64::from(canvas.height()) * 4;
        if size > u64::from(raster.device.limits().max_storage_buffer_binding_size) {
            bail!("game frame exceeds Vulkan presentation storage limit");
        }
        let frame_on_device = device_frame(canvas).filter(|frame|
            std::sync::Arc::ptr_eq(&frame.failed, &raster.failed) && !frame.failed.load(Ordering::Relaxed));
        let pixels = if let Some(frame) = frame_on_device {
            &*frame.buffer
        } else {
            if self.logical_buffer.size() < size {
                self.logical_buffer = buffer(&raster.device, size, super::UPLOAD);
            }
            raster.queue.write_buffer(&self.logical_buffer, 0, canvas.pixels());
            &self.logical_buffer
        };
        let params = [canvas.width(), canvas.height(), width, height, u32::from(self.config.format.is_srgb()), 0, 0, 0];
        let params: Vec<_> = params.into_iter().flat_map(u32::to_le_bytes).collect();
        raster.queue.write_buffer(&self.uniforms, 0, &params);
        let binding = raster.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: pixels.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.uniforms.as_entire_binding() },
            ],
        });
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = raster.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None, depth_stencil_attachment: None, occlusion_query_set: None, timestamp_writes: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &binding, &[]);
            pass.draw(0..3, 0..1);
        }
        raster.queue.submit(Some(encoder.finish()));
        self.display_timing.on_submit(&self.surface, session, inputs);
        frame.present();
        Ok(true)
    }
}

pub(crate) fn set_display_stats_enabled(enabled: bool) {
    if DISPLAY_STATS_ENABLED.with(|state| state.replace(enabled)) == enabled { return; }
    RASTER.with_borrow_mut(|slot| {
        if let Some(presenter) = slot.as_mut().and_then(|raster| raster.presenter.as_mut()) {
            presenter.display_timing.reset();
        }
    });
}

pub(crate) fn display_stats() -> Option<[u64; 7]> {
    if !DISPLAY_STATS_ENABLED.with(Cell::get) { return None; }
    RASTER.with_borrow_mut(|slot| {
        let presenter = slot.as_mut()?.presenter.as_mut()?;
        presenter.display_timing.poll(&presenter.surface);
        Some(presenter.display_timing.snapshot())
    })
}

pub(crate) fn candidate(window: NonNull<c_void>) -> bool {
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else { return false };
        if raster.failed.load(Ordering::Relaxed) {
            raster.presenter = None;
            return false;
        }
        raster.failed_window != Some(window.as_ptr() as usize)
    })
}

pub(crate) fn present_canvas(
    window: NonNull<c_void>, canvas: &Canvas, session: u64, input_mono_us: &[(u64, u64)],
) -> i32 {
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else { return 1 };
        if raster.failed.load(Ordering::Relaxed) || raster.failed_window == Some(window.as_ptr() as usize) {
            raster.presenter = None;
            return 1;
        }
        let mut presenter = raster.presenter.take().filter(|presenter| presenter.window.0 == window);
        let result = (|| {
            if presenter.is_none() {
                let Some(size) = Presenter::size(window) else { return Ok(false) };
                presenter = Some(Presenter::new(raster, window, size)?);
            }
            presenter.as_mut().unwrap().render(raster, canvas, session, input_mono_us)
        })();
        match result {
            Ok(posted) => {
                raster.presenter = presenter;
                if posted { 0 } else { -2 }
            }
            Err(error) => {
                report(&format!("present=egl_fallback reason={error:#}"));
                // `presenter` is dropped before the caller reconnects EGL.
                raster.failed_window = Some(window.as_ptr() as usize);
                1
            }
        }
    })
}

pub(crate) fn detach() {
    RASTER.with_borrow_mut(|slot| {
        if let Some(raster) = slot.as_mut() {
            raster.presenter = None;
            raster.failed_window = None;
        }
    });
}
