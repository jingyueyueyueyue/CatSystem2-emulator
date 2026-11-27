//! Optional, bounded diagnostics for the work done by one profiled host step.
use std::{cell::RefCell, collections::BTreeMap, fmt::Write, time::{Duration, Instant}};

#[path = "audio_profile.rs"]
pub(super) mod audio;

const SLOW_US: u64 = 5_000;
const MAX_TASKS: usize = 12;
const MAX_LISTS: usize = 4;
const MAX_TEXT: usize = 2;
const MAX_KCS_TASKS: usize = 3;
const MAX_KCS_CODES: usize = 64;
const MAX_WIRE_BYTES: usize = 3600;

thread_local! {
    static DETAIL: RefCell<Option<HostPerfDetails>> = const { RefCell::new(None) };
    static TEXT: RefCell<Option<TextSample>> = const { RefCell::new(None) };
    static KCS: RefCell<Option<KcsSample>> = const { RefCell::new(None) };
}

#[derive(Default)]
pub struct HostPerfDetails {
    tasks: Vec<TaskSample>,
    lists: Vec<ListSample>,
    text: Vec<TextSample>,
    text_summary: TextSummary,
    kcs: Vec<KcsSample>,
    audio: Vec<audio::Sample>,
    active_list: Option<ListSample>,
    image_source: Option<ImageSourceSample>,
    active_image_source: Option<ImageSourceSample>,
    sample_lists: bool,
    gpu_output: GpuOutputSample,
    emote_gpu: EmoteGpuSample,
    emote_scene: EmoteSceneSample,
    emote_fallbacks: BTreeMap<&'static str, usize>,
    fast_forward_state: Option<bool>,
    #[cfg(windows)]
    movie_scale: Option<(bool, Duration)>,
    warmup_notes: Vec<String>,
    audio_async_notes: Vec<String>,
    visual_notes: Vec<(&'static str, String)>,
    visual_dropped: usize,
}

struct Capture(Option<HostPerfDetails>);

impl Drop for Capture {
    fn drop(&mut self) {
        DETAIL.with_borrow_mut(|slot| *slot = self.0.take());
    }
}

impl HostPerfDetails {
    pub fn capture<T>(sample_lists: bool, operation: impl FnOnce() -> T) -> (T, Self) {
        let previous = DETAIL.with(|slot| slot.replace(Some(Self { sample_lists, ..Self::default() })));
        let _restore = Capture(previous);
        let result = operation();
        let detail = DETAIL.with(|slot| slot.take().unwrap_or_default());
        (result, detail)
    }

    pub fn rows(mut self, session: u64, frame: u64, call: u64) -> String {
        let mut rows = String::new();
        let mut text_summary = self.text_summary.row(session, frame, call, self.text.len());
        let text_reserve = text_summary.as_ref().map_or(0, |row| {
            row.len() + " detail_wire_dropped=\n".len() + MAX_TEXT.ilog10() as usize + 1
        });
        if !self.visual_notes.is_empty() {
            let mut progress = String::new();
            let mut dropped = self.visual_dropped;
            for (kind, note) in &self.visual_notes {
                let row = format!("{kind} session={session} frame={frame} call={call} {note}\n");
                if progress.len() + row.len() <= 2600 { progress.push_str(&row); }
                else { dropped += 1; }
            }
            let coverage = format!("visual_coverage session={session} frame={frame} call={call} kept={} dropped={dropped}",
                self.visual_notes.len() + self.visual_dropped - dropped);
            append_row_reserving(&mut rows, &coverage, text_reserve);
            append_row_reserving(&mut rows, progress.trim_end(), text_reserve);
        }
        if let Some(row) = self.gpu_output.row(session, frame, call) {
            let _ = append_row_reserving(&mut rows, &row, text_reserve);
        }
        for (reason, count) in self.emote_fallbacks {
            let row = format!("emote_fallback session={session} frame={frame} call={call} reason={reason} count={count}");
            append_row_reserving(&mut rows, &row, text_reserve);
        }
        if let Some(active) = self.fast_forward_state {
            let row = format!("fast_forward_state session={session} frame={frame} call={call} active={active}");
            append_row_reserving(&mut rows, &row, text_reserve);
        }
        #[cfg(windows)]
        if let Some((reused, elapsed)) = self.movie_scale {
            let row = format!("movie_scale session={session} frame={frame} call={call} reused={} elapsed_us={}",
                u8::from(reused), elapsed.as_micros());
            append_row_reserving(&mut rows, &row, text_reserve);
        }
        let scene = self.emote_scene;
        if scene.updated + scene.held + scene.static_held != 0 {
            let row = format!(
                "emote_scene session={session} frame={frame} call={call} updated={} held={} static_held={} stale={} max_lag={} max_age_us={}",
                scene.updated, scene.held, scene.static_held, scene.stale, scene.max_lag,
                scene.max_age.as_micros(),
            );
            append_row_reserving(&mut rows, &row, text_reserve);
        }
        if self.emote_gpu.poses != 0 {
            let sample = self.emote_gpu;
            let [evaluate, prepare, encode, readback, repair] = sample.elapsed.map(|time| time.as_micros());
            let collect = evaluate + prepare;
            let [scanlines, pack] = sample.collect_phases.map(|time| time.as_micros());
            let row = format!(
                "emote_gpu session={session} frame={frame} call={call} poses={} draws={} batches={} pixels={} repaired={} sync_waits={} sync_poses={} collect_us={collect} evaluate_us={evaluate} prepare_us={prepare} scanlines_cpu_us={scanlines} pack_cpu_us={pack} encode_us={encode} readback_us={readback} repair_us={repair} span_hits={} span_misses={} span_cache_peak_bytes={} parallel_wall_us={} parallel_join_us={} parallel_copy_bytes={}",
                sample.poses, sample.draws, sample.batches, sample.pixels, sample.repaired,
                sample.sync_waits, sample.sync_poses, sample.span_hits, sample.span_misses, sample.span_cache_peak_bytes,
                sample.parallel_wall.as_micros(), sample.parallel_join.as_micros(), sample.parallel_copy_bytes,
            );
            append_row_reserving(&mut rows, &row, text_reserve);
        }
        for note in &self.warmup_notes {
            append_row_reserving(&mut rows,
                &format!("resource_warmup session={session} frame={frame} call={call} {note}"), text_reserve);
        }
        for note in &self.audio_async_notes {
            append_row_reserving(&mut rows,
                &format!("audio_async session={session} frame={frame} call={call} {note}"), text_reserve);
        }
        self.tasks.sort_by_key(|sample| std::cmp::Reverse(sample.us));
        self.lists.sort_by_key(|sample| std::cmp::Reverse(sample.us));
        for task in &self.tasks {
            let mut row = format!(
                "task_detail session={session} frame={frame} call={call} task={} kind={} scope={} elapsed_us={}",
                task.id, task.kind, task.scope, task.us,
            );
            if let Some(code) = task.code { let _ = write!(row, " code={code}"); }
            if let Some(offset) = task.offset { let _ = write!(row, " offset={offset}"); }
            if task.name_len != 0 {
                let name = std::str::from_utf8(&task.name[..task.name_len]).unwrap_or("?");
                let _ = write!(row, " name={name}");
            }
            append_row_reserving(&mut rows, &row, text_reserve);
        }
        self.kcs.sort_by_key(|sample| std::cmp::Reverse(sample.elapsed));
        for sample in &self.kcs {
            let other = sample.elapsed.saturating_sub(sample.native.elapsed);
            let mut row = format!(
                "kcs_detail session={session} frame={frame} call={call} task={} elapsed_us={} native_us={} other_us={} native_calls={} native_log_us={}",
                sample.id, sample.elapsed.as_micros(), sample.native.elapsed.as_micros(),
                other.as_micros(), sample.native.calls, sample.native.log_time.as_micros(),
            );
            if let Some(start) = sample.start_pc { let _ = write!(row, " start_pc={start:#x}"); }
            if let Some(end) = sample.end_pc { let _ = write!(row, " end_pc={end:#x}"); }
            let mut codes: Vec<_> = sample.codes.iter().collect();
            codes.sort_by_key(|(_, code)| std::cmp::Reverse(code.elapsed));
            for (&index, code) in codes.into_iter().take(5) {
                let _ = write!(row, " n{index}={}/{}/{}", code.calls,
                    code.elapsed.as_micros(), code.log_time.as_micros());
            }
            append_row_reserving(&mut rows, &row, text_reserve);
        }
        if let Some(sample) = &self.image_source {
            let mut row = format!(
                "fes_img_summary session={session} frame={frame} call={call} task={} elapsed_us={} dirty={} skipped={} cache_hits={}",
                sample.owner, sample.us, sample.dirty, sample.skipped, sample.cache_hits,
            );
            for (stage, (calls, us)) in IMAGE_SOURCE_STAGES.iter().zip(sample.stages) {
                if calls != 0 { let _ = write!(row, " {stage}={calls}/{us}"); }
            }
            append_row_reserving(&mut rows, &row, text_reserve);
        }
        self.text.sort_by_key(|sample| std::cmp::Reverse(sample.elapsed));
        let mut detail_wire_dropped = 0;
        for sample in &self.text {
            let name = std::str::from_utf8(&sample.name[..sample.name_len]).unwrap_or("?");
            let mut row = format!(
                "text_detail session={session} frame={frame} call={call} task={} bank={} name={name} elapsed_us={}",
                sample.owner, sample.bank, sample.elapsed.as_micros(),
            );
            for (stage, measurement) in TEXT_STAGES.iter().zip(sample.stages) {
                if measurement.calls != 0 {
                    let _ = write!(row, " {stage}_us={} {stage}_calls={}",
                        measurement.elapsed.as_micros(), measurement.calls);
                }
            }
            for (name, count) in TEXT_FALLBACK.into_iter().zip(sample.fallback) {
                if count != 0 { let _ = write!(row, " {name}={count}"); }
            }
            if let Some((_, index, name, length)) = sample.cold_face.as_ref() {
                let name = std::str::from_utf8(&name[..*length]).unwrap_or("?");
                let _ = write!(row, " slowest_face={name}:{index}");
            }
            if !append_row_reserving(&mut rows, &row, text_reserve) {
                detail_wire_dropped += 1;
            }
        }
        if let Some(row) = &mut text_summary {
            let _ = writeln!(row, " detail_wire_dropped={detail_wire_dropped}");
            rows.insert_str(0, row);
        }
        audio::write_rows(&mut rows, self.audio, session, frame, call);
        for list in &self.lists {
            let mut row = format!(
                "list_gpu session={session} frame={frame} call={call} list={} size={}x{} dirty_rects={} elapsed_us={} prepare_us={} draw_us={} commit_us={} submit_us={} draws={}",
                list.id, list.size[0], list.size[1], list.rects, list.us,
                list.stages[0], list.stages[1], list.stages[2], list.stages[3],
                list.counts.iter().map(|entry| entry.calls).sum::<u32>(),
            );
            for (name, stats) in GPU_DECISIONS.iter().zip(&list.counts) {
                if stats.calls != 0 {
                    let _ = write!(row, " {name}={}/{}", stats.calls, stats.max_pixels);
                }
            }
            append_row(&mut rows, &row);
        }
        rows
    }
}

// Portable notes travel through the existing host profile wire; the platform
// bridge owns logcat output. Keep both row count and message length bounded.
pub(super) fn enabled() -> bool {
    DETAIL.with_borrow(Option::is_some)
}

pub(super) fn visual_note(kind: &'static str, note: String) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot {
            if detail.visual_notes.len() < 12 && note.len() <= 640 {
                detail.visual_notes.push((kind, note));
            } else { detail.visual_dropped += 1; }
        }
    });
}

#[test]
fn visual_progress_wire_reports_truncation_and_stays_bounded() {
    let (_, details) = HostPerfDetails::capture(false, || {
        for _ in 0..30 { visual_note("visual_pose", "x".repeat(600)); }
    });
    let rows = details.rows(91, 1, 1);
    assert!(rows.starts_with("visual_coverage session=91"));
    assert!(!rows.contains("dropped=0"));
    assert!(rows.len() < MAX_WIRE_BYTES);
    assert!(!rows.contains('\0'));
}

pub(super) fn resource_warmup_note(note: &str) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot && detail.warmup_notes.len() < 4 {
            detail.warmup_notes.push(note.chars().take(192)
                .map(|character| if character.is_control() { ' ' } else { character }).collect());
        }
    });
}

pub(super) fn audio_async_note(note: &str) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot && detail.audio_async_notes.len() < 8 {
            detail.audio_async_notes.push(note.chars().take(192)
                .map(|character| if character.is_control() { ' ' } else { character }).collect());
        }
    });
}

// Report issued transfer bytes, including failed submissions, with elapsed
// CPU wall time for the synchronous wait and copy.
#[derive(Default)]
struct ReadbackSample {
    calls: u64,
    failures: u64,
    bytes: u64,
    elapsed: Duration,
}

#[derive(Default)]
struct GpuOutputSample {
    device: ReadbackSample,
    packed: ReadbackSample,
    emote: ReadbackSample,
    textured_group_rejects: u64,
    textured_single_rejects: u64,
}

#[derive(Default)]
struct EmoteGpuSample {
    poses: usize,
    draws: usize,
    batches: usize,
    pixels: u64,
    repaired: usize,
    sync_waits: usize,
    sync_poses: usize,
    elapsed: [Duration; 5],
    collect_phases: [Duration; 2],
    span_hits: usize,
    span_misses: usize,
    span_cache_peak_bytes: usize,
    parallel_wall: Duration,
    parallel_join: Duration,
    parallel_copy_bytes: usize,
}

#[derive(Default)]
struct EmoteSceneSample {
    updated: usize,
    held: usize,
    static_held: usize,
    stale: usize,
    max_lag: u64,
    max_age: Duration,
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn emote_fallback(reason: &'static str) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot { *detail.emote_fallbacks.entry(reason).or_default() += 1; }
    });
}

pub(super) fn fast_forward_state(active: bool) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot { detail.fast_forward_state = Some(active); }
    });
}

#[cfg(windows)]
pub(super) fn movie_scale(reused: bool, elapsed: Duration) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot {
            detail.movie_scale = Some((reused, elapsed));
        }
    });
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn emote_scene_pose(updated: bool, changing: bool, tick_lag: u64, age: Duration) {
    DETAIL.with_borrow_mut(|slot| {
        let Some(detail) = slot else { return };
        let sample = &mut detail.emote_scene;
        if updated {
            sample.updated += 1;
        } else if changing {
            sample.held += 1;
        } else {
            sample.static_held += 1;
            return;
        }
        sample.stale += usize::from(tick_lag > 2 || age > Duration::from_millis(50));
        sample.max_lag = sample.max_lag.max(tick_lag);
        sample.max_age = sample.max_age.max(age);
    });
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn emote_prep_parallel(wall: Duration, join: Duration, copy_bytes: usize) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot {
            let sample = &mut detail.emote_gpu;
            sample.parallel_wall += wall;
            sample.parallel_join += join;
            sample.parallel_copy_bytes += copy_bytes;
        }
    });
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn emote_span_cache(hits: usize, misses: usize, bytes: usize) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot {
            let sample = &mut detail.emote_gpu;
            sample.span_hits += hits;
            sample.span_misses += misses;
            sample.span_cache_peak_bytes = sample.span_cache_peak_bytes.max(bytes);
        }
    });
}

impl GpuOutputSample {
    fn row(&self, session: u64, frame: u64, call: u64) -> Option<String> {
        if self.device.calls == 0 && self.packed.calls == 0 && self.emote.calls == 0
            && self.textured_group_rejects == 0 && self.textured_single_rejects == 0
        {
            return None;
        }
        Some(format!(
            "gpu_output session={session} frame={frame} call={call} device_calls={} device_failed={} device_bytes={} device_us={} packed_calls={} packed_failed={} packed_bytes={} packed_us={} emote_calls={} emote_failed={} emote_bytes={} emote_us={} textured_group_rejects={} textured_single_rejects={}",
            self.device.calls, self.device.failures, self.device.bytes, self.device.elapsed.as_micros(),
            self.packed.calls, self.packed.failures, self.packed.bytes, self.packed.elapsed.as_micros(),
            self.emote.calls, self.emote.failures, self.emote.bytes, self.emote.elapsed.as_micros(),
            self.textured_group_rejects, self.textured_single_rejects,
        ))
    }
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
#[derive(Clone, Copy)]
pub(super) enum ReadbackKind {
    Device,
    Packed,
    Emote,
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) struct ReadbackTimer {
    kind: ReadbackKind,
    bytes: u64,
    started: Instant,
    success: bool,
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
impl ReadbackTimer {
    pub(super) fn start(kind: ReadbackKind, bytes: u64) -> Option<Self> {
        DETAIL.with_borrow(Option::is_some)
            .then(|| Self { kind, bytes, started: Instant::now(), success: false })
    }

    pub(super) fn complete(mut self) {
        self.success = true;
    }
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
impl Drop for ReadbackTimer {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        DETAIL.with_borrow_mut(|slot| {
            let Some(detail) = slot else { return };
            let sample = match self.kind {
                ReadbackKind::Device => &mut detail.gpu_output.device,
                ReadbackKind::Packed => &mut detail.gpu_output.packed,
                ReadbackKind::Emote => &mut detail.gpu_output.emote,
            };
            sample.calls += 1;
            sample.failures += u64::from(!self.success);
            sample.bytes += self.bytes;
            sample.elapsed += elapsed;
        });
    }
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn emote_gpu_start() -> Option<Instant> {
    DETAIL.with_borrow(Option::is_some).then(Instant::now)
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn emote_gpu_sync(poses: usize) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot {
            detail.emote_gpu.sync_waits += 1;
            detail.emote_gpu.sync_poses += poses;
        }
    });
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn emote_gpu_pose(
    milestones: [Instant; 5], collect_phases: [Duration; 2], draws: usize, batches: usize,
    pixels: u64, repaired: usize,
) {
    let [start, evaluated, collected, encoded, downloaded] = milestones;
    let elapsed = [evaluated - start, collected - evaluated, encoded - collected, downloaded - encoded, downloaded.elapsed()];
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot {
            let sample = &mut detail.emote_gpu;
            sample.poses += 1;
            sample.draws += draws;
            sample.batches += batches;
            sample.pixels += pixels;
            sample.repaired += repaired;
            for (total, phase) in sample.collect_phases.iter_mut().zip(collect_phases) {
                *total += phase;
            }
            for (total, elapsed) in sample.elapsed.iter_mut().zip(elapsed) {
                *total += elapsed;
            }
        }
    });
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn emote_gpu_encode_elapsed(elapsed: Duration) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(detail) = slot {
            detail.emote_gpu.elapsed[2] += elapsed;
        }
    });
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn textured_triangle_rejected(group: bool) {
    DETAIL.with_borrow_mut(|slot| {
        let Some(detail) = slot else { return };
        if group {
            detail.gpu_output.textured_group_rejects += 1;
        } else {
            detail.gpu_output.textured_single_rejects += 1;
        }
    });
}

fn append_row(rows: &mut String, row: &str) {
    append_row_reserving(rows, row, 0);
}

fn append_row_reserving(rows: &mut String, row: &str, reserve: usize) -> bool {
    if rows.len() + row.len() + reserve >= MAX_WIRE_BYTES {
        return false;
    }
    rows.push_str(row);
    rows.push('\n');
    true
}

fn keep_slowest<T>(records: &mut Vec<T>, limit: usize, record: T, elapsed: impl Fn(&T) -> u64) {
    if records.len() < limit {
        records.push(record);
    } else if let Some((index, old)) = records.iter().enumerate().min_by_key(|(_, item)| elapsed(item))
        && elapsed(&record) > elapsed(old)
    {
        records[index] = record;
    }
}

#[derive(Clone, Copy)]
pub(super) enum TextStage {
    Configure, UserMaps, DecodeUser, Clear, Prepare, Layout, FontSelect, FontFallback,
    FaceLoad, FaceRead, FaceDecode, GlyphMeasure, GlyphRaster, GlyphEffects, DecodeInline,
    Metadata, FontCatalog, Playback, SurfaceSync, Projection, NovelProjection, History,
}

const TEXT_STAGES: [&str; TextStage::History as usize + 1] = [
    "configure", "user_maps", "decode_user", "clear", "prepare", "layout", "font_select",
    "font_fallback", "face_load", "face_read", "face_decode", "glyph_measure",
    "glyph_raster", "glyph_effects", "decode_inline", "metadata", "font_catalog", "playback",
    "surface_sync", "projection", "novel_projection", "history",
];
const TEXT_FALLBACK: [&str; 3] = ["font_cache_hits", "font_rank_hits", "font_rank_builds"];

#[derive(Clone, Copy, Default)]
struct TextMeasurement {
    elapsed: Duration,
    calls: u32,
}

impl TextMeasurement {
    fn record(&mut self, elapsed: Duration) {
        self.elapsed += elapsed;
        self.calls = self.calls.saturating_add(1);
    }
}

#[derive(Default)]
struct TextSummary {
    bank_work: TextMeasurement,
    bank_below_threshold: u32,
    projections: TextProjections,
    stages: [TextMeasurement; TEXT_STAGES.len()],
    fallback: [u32; TEXT_FALLBACK.len()],
}

impl TextSummary {
    fn record_bank(&mut self, sample: &TextSample) {
        self.bank_work.record(sample.elapsed);
        if sample.elapsed.as_micros() < u128::from(SLOW_US) {
            self.bank_below_threshold = self.bank_below_threshold.saturating_add(1);
        }
        self.projections.add(&sample.projections);
        for (total, stage) in self.stages.iter_mut().zip(sample.stages) {
            total.elapsed += stage.elapsed;
            total.calls = total.calls.saturating_add(stage.calls);
        }
        for (total, count) in self.fallback.iter_mut().zip(sample.fallback) {
            *total = total.saturating_add(count);
        }
    }

    fn row(&self, session: u64, frame: u64, call: u64, kept: usize) -> Option<String> {
        if self.bank_work.calls == 0 && self.stages.iter().all(|stage| stage.calls == 0) {
            return None;
        }
        let bank_calls = self.bank_work.calls;
        let bank_us = self.bank_work.elapsed.as_micros();
        let below = self.bank_below_threshold;
        let detail_limit_dropped = bank_calls.saturating_sub(below).saturating_sub(kept as u32);
        let mut row = format!(
            "text_summary session={session} frame={frame} call={call} bank_calls={bank_calls} bank_us={bank_us} detail_below_threshold={below} detail_kept={kept} detail_limit_dropped={detail_limit_dropped}"
        );
        for (stage, measurement) in TEXT_STAGES.iter().zip(self.stages) {
            if measurement.calls != 0 {
                let _ = write!(row, " {stage}={}/{}", measurement.calls, measurement.elapsed.as_micros());
            }
        }
        for (name, count) in TEXT_FALLBACK.into_iter().zip(self.fallback) {
            if count != 0 {
                let _ = write!(row, " {name}={count}");
            }
        }
        self.projections.write(&mut row);
        Some(row)
    }
}

#[derive(Default)]
struct TextProjections {
    reused: u32,
    patched: u32,
    replaced: u32,
    patch_pixels: u64,
    eligible_updates: u32,
}

impl TextProjections {
    fn add(&mut self, other: &Self) {
        self.reused = self.reused.saturating_add(other.reused);
        self.patched = self.patched.saturating_add(other.patched);
        self.replaced = self.replaced.saturating_add(other.replaced);
        self.patch_pixels = self.patch_pixels.saturating_add(other.patch_pixels);
        self.eligible_updates = self.eligible_updates.saturating_add(other.eligible_updates);
    }

    fn write(&self, row: &mut String) {
        if self.reused != 0 || self.patched != 0 || self.replaced != 0 {
            let _ = write!(row,
                " projections_reused={} projections_patched={} projections_replaced={} projection_patch_pixels={} eligible_projection_updates={}",
                self.reused, self.patched, self.replaced, self.patch_pixels, self.eligible_updates);
        }
    }
}

pub(super) enum TextProjection {
    Reused,
    Patched(u64),
    Replaced,
}

struct TextSample {
    owner: u32,
    bank: i32,
    name: [u8; 40],
    name_len: usize,
    elapsed: Duration,
    stages: [TextMeasurement; TEXT_STAGES.len()],
    fallback: [u32; 3],
    cold_face: Option<(Duration, u32, [u8; 40], usize)>,
    projections: TextProjections,
}

fn detail_name(name: &[u8]) -> ([u8; 40], usize) {
    let mut bytes = [0; 40];
    for (out, &byte) in bytes.iter_mut().zip(name) {
        *out = if byte.is_ascii_alphanumeric() || b"_.:-/".contains(&byte) { byte } else { b'_' };
    }
    (bytes, name.len().min(40))
}

pub(super) struct TextWork(Option<(Instant, Option<TextSample>)>);

impl TextWork {
    pub(super) fn new(owner: u32, bank: i32, name: &[u8]) -> Self {
        Self(DETAIL.with_borrow(|slot| slot.is_some()).then(|| {
            let (name, name_len) = detail_name(name);
            (Instant::now(), TEXT.with_borrow_mut(|slot| slot.replace(TextSample {
                owner, bank, name, name_len, elapsed: Duration::ZERO,
                stages: Default::default(), fallback: [0; 3], cold_face: None,
                projections: TextProjections::default(),
            })))
        }))
    }

    pub(super) fn enabled(&self) -> bool {
        self.0.is_some()
    }

    pub(super) fn projection(&self, projection: TextProjection, eligible: bool) {
        TEXT.with_borrow_mut(|slot| {
            let Some(sample) = slot.as_mut() else { return };
            let counts = &mut sample.projections;
            match projection {
                TextProjection::Reused => counts.reused += 1,
                TextProjection::Patched(pixels) => {
                    counts.patched += 1;
                    counts.patch_pixels += pixels;
                }
                TextProjection::Replaced => counts.replaced += 1,
            }
            if eligible && !matches!(projection, TextProjection::Reused) {
                counts.eligible_updates += 1;
            }
        });
    }
}

impl Drop for TextWork {
    fn drop(&mut self) {
        let Some((start, previous)) = self.0.take() else { return };
        let elapsed = start.elapsed();
        let sample = TEXT.with_borrow_mut(|slot| std::mem::replace(slot, previous));
        let Some(mut sample) = sample else { return };
        sample.elapsed = elapsed;
        DETAIL.with_borrow_mut(|slot| {
            let Some(detail) = slot else { return };
            detail.text_summary.record_bank(&sample);
            if elapsed.as_micros() >= u128::from(SLOW_US) {
                keep_slowest(&mut detail.text, MAX_TEXT, sample, |sample| sample.elapsed.as_micros() as u64);
            }
        });
    }
}

#[cfg(any(windows, target_os = "android"))]
pub(super) enum TextFallbackEvent {
    SelectionHit, RankHit, RankBuild,
}

#[cfg(any(windows, target_os = "android"))]
pub(super) fn text_fallback_event(event: TextFallbackEvent) {
    TEXT.with_borrow_mut(|slot| {
        if let Some(sample) = slot {
            let counter = &mut sample.fallback[event as usize];
            *counter = counter.saturating_add(1);
        }
    });
}

/// Text work outside an individual message bank.
pub(super) struct TextHostTimer(Option<Instant>, TextStage);

impl TextHostTimer {
    pub(super) fn new(stage: TextStage) -> Self {
        Self(DETAIL.with_borrow(|slot| slot.is_some()).then(Instant::now), stage)
    }
}

impl Drop for TextHostTimer {
    fn drop(&mut self) {
        let Some(start) = self.0 else { return };
        let elapsed = start.elapsed();
        DETAIL.with_borrow_mut(|slot| {
            if let Some(detail) = slot {
                detail.text_summary.stages[self.1 as usize].record(elapsed);
            }
        });
    }
}

pub(super) struct TextTimer(Option<Instant>, TextStage, Option<(u32, [u8; 40], usize)>);

impl TextTimer {
    pub(super) fn new(stage: TextStage) -> Self {
        Self(TEXT.with_borrow(|slot| slot.is_some()).then(Instant::now), stage, None)
    }

    pub(super) fn face(mut self, name: &[u8], index: u32) -> Self {
        if self.0.is_some() {
            let (name, len) = detail_name(name);
            self.2 = Some((index, name, len));
        }
        self
    }
}

impl Drop for TextTimer {
    fn drop(&mut self) {
        let Some(start) = self.0 else { return };
        let elapsed = start.elapsed();
        TEXT.with_borrow_mut(|slot| {
            if let Some(text) = slot {
                let measurement = &mut text.stages[self.1 as usize];
                measurement.record(elapsed);
                if let Some((index, name, len)) = self.2
                    && text.cold_face.as_ref().is_none_or(|(longest, ..)| elapsed > *longest)
                {
                    text.cold_face = Some((elapsed, index, name, len));
                }
            }
        });
    }
}

struct TaskSample {
    id: u32,
    kind: i32,
    scope: &'static str,
    code: Option<i64>,
    offset: Option<usize>,
    name: [u8; 40],
    name_len: usize,
    us: u64,
}

#[derive(Default)]
struct KcsCodeSample {
    calls: u32,
    elapsed: Duration,
    log_time: Duration,
}

impl KcsCodeSample {
    fn record(&mut self, elapsed: Duration, log_time: Duration) {
        self.calls = self.calls.saturating_add(1);
        self.elapsed += elapsed;
        self.log_time += log_time;
    }
}

#[derive(Default)]
struct KcsSample {
    id: u32,
    start_pc: Option<usize>,
    end_pc: Option<usize>,
    elapsed: Duration,
    native: KcsCodeSample,
    codes: BTreeMap<u32, KcsCodeSample>,
    depth: usize,
}

pub(super) struct KcsBody(Option<(Instant, Option<KcsSample>)>);

impl KcsBody {
    pub(super) fn new(id: u32, start_pc: Option<usize>) -> Self {
        Self(DETAIL.with_borrow(|detail| detail.is_some()).then(|| {
            let replaced = KCS.with(|slot| slot.replace(Some(KcsSample {
                id, start_pc, ..KcsSample::default()
            })));
            (Instant::now(), replaced)
        }))
    }

    pub(super) fn end_pc(&self, pc: Option<usize>) {
        if self.0.is_some() {
            KCS.with_borrow_mut(|slot| {
                if let Some(sample) = slot { sample.end_pc = pc; }
            });
        }
    }
}

impl Drop for KcsBody {
    fn drop(&mut self) {
        let Some((start, replaced)) = self.0.take() else { return };
        let elapsed = start.elapsed();
        let Some(mut sample) = KCS.with(|slot| slot.replace(replaced)) else { return };
        if elapsed.as_micros() >= SLOW_US as u128 {
            sample.elapsed = elapsed;
            DETAIL.with_borrow_mut(|slot| {
                if let Some(detail) = slot {
                    keep_slowest(&mut detail.kcs, MAX_KCS_TASKS, sample,
                        |sample| sample.elapsed.as_micros() as u64);
                }
            });
        }
    }
}

pub(super) struct SlowTask {
    timing: Option<(Instant, TaskSample)>,
    kcs_code: Option<u32>,
    log_time: Duration,
}

impl SlowTask {
    pub(super) fn new(id: u32, kind: impl FnOnce() -> i32, scope: &'static str) -> Self {
        let enabled = DETAIL.with_borrow(|slot| slot.is_some());
        Self {
            timing: enabled.then(|| (Instant::now(), TaskSample {
                id, kind: kind(), scope, code: None, offset: None, name: [0; 40], name_len: 0, us: 0,
            })),
            kcs_code: None,
            log_time: Duration::ZERO,
        }
    }

    pub(super) fn kcs_native(id: u32, kind: impl FnOnce() -> i32, index: u32, pc: usize) -> Self {
        let mut result = Self::new(id, kind, "kcs_native").code(index);
        if let Some((_, sample)) = &mut result.timing {
            sample.offset = Some(pc);
            result.kcs_code = KCS.with_borrow_mut(|slot| {
                let body = slot.as_mut()?;
                body.depth += 1;
                Some(index)
            });
        }
        result
    }

    pub(super) fn native_log_finished(&mut self) {
        if self.kcs_code.is_some() && let Some((start, _)) = &self.timing {
            self.log_time = start.elapsed();
        }
    }

    pub(super) fn code(mut self, code: impl Into<i64>) -> Self {
        if let Some((_, sample)) = &mut self.timing { sample.code = Some(code.into()); }
        self
    }

    pub(super) fn named(mut self, name: &[u8], offset: Option<usize>) -> Self {
        if let Some((_, sample)) = &mut self.timing {
            sample.offset = offset;
            sample.name_len = name.len().min(sample.name.len());
            for (out, &byte) in sample.name.iter_mut().zip(name) {
                *out = if byte.is_ascii_alphanumeric() || b"_.:-/".contains(&byte) { byte } else { b'_' };
            }
        }
        self
    }
}

impl Drop for SlowTask {
    fn drop(&mut self) {
        let Some((start, mut sample)) = self.timing.take() else { return };
        let elapsed = start.elapsed();
        if let Some(index) = self.kcs_code {
            KCS.with_borrow_mut(|slot| {
                let Some(body) = slot else { return };
                body.depth -= 1;
                // The outer call already includes any reentrant native work.
                if body.depth == 0 {
                    body.native.record(elapsed, self.log_time);
                    if body.codes.len() < MAX_KCS_CODES || body.codes.contains_key(&index) {
                        body.codes.entry(index).or_default().record(elapsed, self.log_time);
                    }
                }
            });
        }
        sample.us = elapsed.as_micros() as u64;
        let threshold = match sample.scope {
            "emote_render_rgba" | "emote_blend" => 1_000,
            _ => SLOW_US,
        };
        if sample.us >= threshold {
            DETAIL.with_borrow_mut(|slot| {
                if let Some(detail) = slot {
                    keep_slowest(&mut detail.tasks, MAX_TASKS, sample, |sample| sample.us);
                }
            });
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum GpuDecision {
    SpanGpu, AffineGpu, SpanSmall, AffineSmall, BackendUnavailable, DispatchLimits,
    DispatchFailed, OpaqueCopy, AlphaMask, DrawMask, DrawBlend, DrawTransform, CopyGpu,
}

const GPU_DECISIONS: [&str; GpuDecision::CopyGpu as usize + 1] = [
    "span_gpu", "affine_gpu", "span_under_131072", "affine_under_32768",
    "backend_unavailable", "dispatch_limits", "dispatch_failed", "cpu_opaque_copy",
    "cpu_alpha_mask", "cpu_draw_mask", "cpu_draw_blend", "cpu_draw_transform", "copy_gpu",
];

#[derive(Clone, Copy, Default)]
struct DrawCount {
    calls: u32,
    max_pixels: u64,
}

struct ListSample {
    id: u32,
    size: [u32; 2],
    rects: usize,
    counts: [DrawCount; GPU_DECISIONS.len()],
    us: u64,
    stages: [u64; 4],
}

pub(super) struct ListRaster(Option<(Instant, Option<ListSample>)>);

impl ListRaster {
    pub(super) fn begin(id: u32, size: [u32; 2], rects: usize) -> Self {
        Self(DETAIL.with_borrow_mut(|slot| {
            let detail = slot.as_mut()?;
            let list = ListSample { id, size, rects, counts: Default::default(), us: 0, stages: [0; 4] };
            Some((Instant::now(), detail.active_list.replace(list)))
        }))
    }

    pub(super) fn end_stage(&self, stage: usize) {
        let Some((start, _)) = &self.0 else { return };
        let now = start.elapsed().as_micros() as u64;
        DETAIL.with_borrow_mut(|slot| {
            if let Some(list) = slot.as_mut().and_then(|detail| detail.active_list.as_mut()) {
                list.stages[stage] = now.saturating_sub(list.stages[..stage].iter().sum());
            }
        });
    }
}

impl Drop for ListRaster {
    fn drop(&mut self) {
        let Some((start, previous)) = self.0.take() else { return };
        DETAIL.with_borrow_mut(|slot| {
            let Some(detail) = slot else { return };
            let Some(mut list) = std::mem::replace(&mut detail.active_list, previous) else { return };
            list.us = start.elapsed().as_micros() as u64;
            if list.us >= SLOW_US || detail.sample_lists {
                keep_slowest(&mut detail.lists, MAX_LISTS, list, |sample| sample.us);
            }
        });
    }
}

pub(super) fn list_gpu_decision(decision: GpuDecision, pixels: u64) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(list) = slot.as_mut().and_then(|detail| detail.active_list.as_mut()) {
            let stats = &mut list.counts[decision as usize];
            stats.calls = stats.calls.saturating_add(1);
            stats.max_pixels = stats.max_pixels.max(pixels);
        }
    });
}

#[derive(Clone, Copy)]
pub(super) enum ImageSourceStage {
    Expand, Route, CacheLookup, CacheTouch, Ingress, Decode, Release, Attach,
    RawDirectory, RawDecode, RawRegister,
}

const IMAGE_SOURCE_STAGES: [&str; ImageSourceStage::RawRegister as usize + 1] = [
    "expand", "route", "cache_lookup", "cache_touch", "ingress", "decode", "release", "attach",
    "raw_directory", "raw_decode", "raw_register",
];

struct ImageSourceSample {
    owner: u32,
    us: u64,
    dirty: u32,
    skipped: u32,
    cache_hits: u32,
    stages: [(u32, u64); IMAGE_SOURCE_STAGES.len()],
}

pub(super) struct ImageSourceTrace(Option<(Instant, Option<ImageSourceSample>)>);

impl ImageSourceTrace {
    pub(super) fn begin(owner: u32) -> Self {
        Self(DETAIL.with_borrow_mut(|slot| {
            let detail = slot.as_mut()?;
            let sample = ImageSourceSample { owner, us: 0, dirty: 0, skipped: 0, cache_hits: 0, stages: Default::default() };
            Some((Instant::now(), detail.active_image_source.replace(sample)))
        }))
    }

    pub(super) fn branch(&self, dirty: bool) {
        if self.0.is_none() { return; }
        with_image_source(|sample| {
            if dirty { sample.dirty += 1; } else { sample.skipped += 1; }
        });
    }
}

impl Drop for ImageSourceTrace {
    fn drop(&mut self) {
        let Some((start, previous)) = self.0.take() else { return; };
        DETAIL.with_borrow_mut(|slot| {
            let Some(detail) = slot.as_mut() else { return; };
            let Some(mut sample) = std::mem::replace(&mut detail.active_image_source, previous) else { return; };
            sample.us = start.elapsed().as_micros() as u64;
            if (sample.dirty != 0 || sample.us >= SLOW_US)
                && detail.image_source.as_ref().is_none_or(|previous| sample.us > previous.us)
            {
                detail.image_source = Some(sample);
            }
        });
    }
}

fn with_image_source(operation: impl FnOnce(&mut ImageSourceSample)) {
    DETAIL.with_borrow_mut(|slot| {
        if let Some(sample) = slot.as_mut().and_then(|detail| detail.active_image_source.as_mut()) {
            operation(sample);
        }
    });
}

pub(super) struct ImageSourceStep(Option<Instant>, ImageSourceStage);

impl ImageSourceStep {
    pub(super) fn new(stage: ImageSourceStage) -> Self {
        Self(DETAIL.with_borrow(|slot| {
            slot.as_ref().and_then(|detail| detail.active_image_source.as_ref()).map(|_| Instant::now())
        }), stage)
    }

    pub(super) fn cache_hit() {
        with_image_source(|sample| sample.cache_hits += 1);
    }
}

impl Drop for ImageSourceStep {
    fn drop(&mut self) {
        let Some(start) = self.0 else { return; };
        let elapsed = start.elapsed().as_micros() as u64;
        with_image_source(|sample| {
            let (calls, us) = &mut sample.stages[self.1 as usize];
            *calls += 1;
            *us += elapsed;
        });
    }
}
