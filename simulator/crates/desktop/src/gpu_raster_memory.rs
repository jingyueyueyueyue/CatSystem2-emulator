use super::{RASTER, Raster, RasterStatus, enable, report, set_raster_status};
use std::cell::Cell;
use std::io::{self, Read};
use std::time::{Duration, Instant};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
const TRIM_INTERVAL: Duration = Duration::from_secs(2);
const TRIM_AT: usize = 16_384;
const RETIRE_AT: usize = 32_768;
const RESUME_BELOW: usize = 12_288;
const RECOVERY_DELAY: Duration = Duration::from_secs(2);
const FAILED_RETRY_DELAY: Duration = Duration::from_secs(10);
const DRAIN_TIMEOUT: Duration = Duration::from_millis(40);

thread_local! {
    static RECOVER_AT: Cell<Option<Instant>> = const { Cell::new(None) };
}

#[derive(Default)]
pub(super) struct MemoryMaintenance {
    sampled_at: Option<Instant>,
    trimmed_at: Option<Instant>,
    pub(super) retired: bool,
}

pub(in crate::gpu_raster) fn map_count(mut maps: impl Read) -> io::Result<usize> {
    let mut count = 0;
    let mut buffer = [0; 8192];
    loop {
        let len = maps.read(&mut buffer)?;
        if len == 0 {
            return Ok(count);
        }
        count += buffer[..len].iter().filter(|&&byte| byte == b'\n').count();
        if count >= RETIRE_AT {
            return Ok(count);
        }
    }
}

pub(crate) fn maintain(at_frame_boundary: bool) {
    let retired = RASTER.with_borrow_mut(|slot| {
        if slot.as_mut().is_some_and(Raster::maintain_android_memory) {
            set_raster_status(RasterStatus::Disabled);
            slot.take();
            return true;
        }
        false
    });
    if retired {
        RECOVER_AT.set(Some(Instant::now() + RECOVERY_DELAY));
    } else if at_frame_boundary {
        recover();
    }
}

pub(super) fn clear_recovery() {
    RECOVER_AT.set(None);
}

fn current_map_count() -> io::Result<usize> {
    std::fs::File::open("/proc/self/maps").and_then(map_count)
}

fn recover() {
    let now = Instant::now();
    if !RECOVER_AT.get().is_some_and(|deadline| now >= deadline) {
        return;
    }
    // Give the presenter and outstanding snapshots time to release the old
    // device. Leave headroom for a new device and swapchain before allocating.
    RECOVER_AT.set(Some(now + SAMPLE_INTERVAL));
    let Ok(maps) = current_map_count() else { return };
    if maps >= RESUME_BELOW {
        return;
    }
    if enable() {
        report(&format!("memory_recovery maps={maps} backend=vulkan"));
    } else {
        RECOVER_AT.set(Some(Instant::now() + FAILED_RETRY_DELAY));
        report(&format!("memory_recovery maps={maps} retry=scheduled"));
    }
}

impl Raster {
    fn maintain_android_memory(&mut self) -> bool {
        let now = Instant::now();
        if self.memory.sampled_at.is_some_and(|at| now.duration_since(at) < SAMPLE_INTERVAL) {
            return false;
        }
        self.memory.sampled_at = Some(now);

        let Ok(maps) = current_map_count() else {
            return false;
        };
        if maps < TRIM_AT {
            return false;
        }
        if maps < RETIRE_AT
            && self.memory.trimmed_at.is_some_and(|at| now.duration_since(at) < TRIM_INTERVAL)
        {
            return false;
        }
        self.memory.trimmed_at = Some(now);

        // Drop only derived GPU data. In-flight commands and published frames
        // retain their own references until the queue releases them.
        self.sources.clear();
        self.source_bytes = 0;
        self.atlas = None;
        self.previous_planes = None;
        self.spares.clear();
        self.full_target = None;
        if let Some(emote) = &mut self.emote {
            emote.trim_android_caches();
        }
        let drained = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(DRAIN_TIMEOUT),
        }).is_ok();
        purge_unused_heap_pages();
        let remaining = current_map_count().unwrap_or(maps);
        report(&format!("memory_reclaim maps_before={maps} maps_after={remaining} drained={drained}"));
        if remaining < RETIRE_AT {
            return false;
        }

        // The Adreno driver can abort in malloc at the kernel's mapping limit
        // without returning a Vulkan error. Stop allocating on this device
        // while there is still room to finish and display the current scene.
        self.memory.retired = true;
        report(&format!("memory_reclaim maps={remaining} backend=cpu reason=mapping_pressure"));
        true
    }
}

fn purge_unused_heap_pages() {
    unsafe extern "C" {
        fn mallopt(option: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
    }
    // Bionic M_PURGE is available from API 28. Older versions reject the
    // request, which is independent of the GPU cache eviction above.
    unsafe { mallopt(-101, 0); }
}
