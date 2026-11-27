//! Retained frame effects sharing the graphics window's single callback.
//! Ordinary wrappers and object messages have separate descriptor tables.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, Color, RgbaFrame};
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{NativeReturn, Vm};

use super::{RenderTaskStore, V401, YUKIKOI, check_work, pixels};
use crate::{LoggingHost, native_state_error, read_u32};

#[path = "frame_effect_geometry.rs"]
mod geometry;

#[path = "frame_effect_resample.rs"]
mod resample;

const MAX_LEVELS: u32 = 65_536;
const MAX_RETAINED_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Query,
    Prepare,
    CaptureSecond,
    Play,
    Flag,
    Consume,
    Delegate,
}

#[derive(Clone, Copy)]
struct Descriptor {
    slot: u32,
    kind: u32,
    operation: Operation,
    input_words: usize,
    record_words: usize,
    old_message: u32,
    current_message: u32,
}

const DESCRIPTORS: &[Descriptor] = &[
    Descriptor {
        slot: 853,
        kind: 50,
        operation: Operation::Prepare,
        input_words: 6,
        record_words: 5,
        old_message: 490,
        current_message: 525,
    },
    Descriptor {
        slot: 854,
        kind: 50,
        operation: Operation::CaptureSecond,
        input_words: 1,
        record_words: 0,
        old_message: 491,
        current_message: 526,
    },
    Descriptor {
        slot: 855,
        kind: 50,
        operation: Operation::Play,
        input_words: 2,
        record_words: 0,
        old_message: 492,
        current_message: 527,
    },
    Descriptor {
        slot: 856,
        kind: 51,
        operation: Operation::Prepare,
        input_words: 7,
        record_words: 6,
        old_message: 493,
        current_message: 528,
    },
    Descriptor {
        slot: 857,
        kind: 51,
        operation: Operation::Play,
        input_words: 2,
        record_words: 0,
        old_message: 494,
        current_message: 529,
    },
    Descriptor {
        slot: 860,
        kind: 52,
        operation: Operation::Prepare,
        input_words: 8,
        record_words: 7,
        old_message: 495,
        current_message: 530,
    },
    Descriptor {
        slot: 861,
        kind: 52,
        operation: Operation::Play,
        input_words: 2,
        record_words: 0,
        old_message: 496,
        current_message: 531,
    },
    Descriptor {
        slot: 967,
        kind: 57,
        operation: Operation::Prepare,
        input_words: 7,
        record_words: 6,
        old_message: 385,
        current_message: 396,
    },
    Descriptor {
        slot: 968,
        kind: 57,
        operation: Operation::Play,
        input_words: 2,
        record_words: 0,
        old_message: 386,
        current_message: 397,
    },
    Descriptor {
        slot: 969,
        kind: 57,
        operation: Operation::Flag,
        input_words: 1,
        record_words: 0,
        old_message: 387,
        current_message: 398,
    },
    Descriptor {
        slot: 970,
        kind: 58,
        // Both PE receivers jump directly to their consumed/zero return.
        // Only973 falls through to the base receiver.
        operation: Operation::Consume,
        input_words: 1,
        record_words: 0,
        old_message: 388,
        current_message: 399,
    },
    Descriptor {
        slot: 971,
        kind: 58,
        operation: Operation::Prepare,
        input_words: 10,
        record_words: 9,
        old_message: 389,
        current_message: 400,
    },
    Descriptor {
        slot: 972,
        kind: 58,
        operation: Operation::Play,
        input_words: 2,
        record_words: 0,
        old_message: 390,
        current_message: 401,
    },
    Descriptor {
        slot: 973,
        kind: 58,
        operation: Operation::Delegate,
        input_words: 1,
        record_words: 0,
        old_message: 391,
        current_message: 402,
    },
];

#[derive(Default)]
pub(super) struct FrameEffects {
    tasks: BTreeMap<u32, Effect>,
}

#[derive(Clone)]
struct Level {
    image: Arc<RgbaFrame>,
    block: u32,
    at: u32,
    alpha: u8,
    side: usize,
}

#[derive(Clone)]
struct Effect {
    kind: u32,
    available: bool,
    registered: bool,
    active: u32,
    size: [u32; 2],
    words: Vec<u32>,
    now: u32,
    started: u32,
    end: u32,
    captures: [Option<Arc<RgbaFrame>>; 2],
    levels: Vec<Level>,
    // Time is the original cache key, including duplicate/odd middle times.
    cached_time: Option<u32>,
    selected_side: usize,
    work: [Option<Arc<RgbaFrame>>; 2],
    geometry: Option<geometry::Geometry>,
    current_geometry: Option<bool>,
    scale_profile: Option<resample::Profile>,
    flag: bool,
}

pub(super) fn supports(index: u32) -> bool {
    DESCRIPTORS.iter().any(|entry| entry.slot == index)
}

pub(super) fn supports_kind(kind: u32) -> bool {
    matches!(kind, 50..=52 | 57 | 58)
}

impl FrameEffects {
    pub(super) fn contains(&self, id: u32) -> bool {
        self.tasks.contains_key(&id)
    }

    pub(super) fn participates(&self, id: u32) -> bool {
        self.tasks.get(&id).is_some_and(|task| task.registered)
    }

    pub(super) fn update(&mut self, id: u32, now: u32) -> Result<()> {
        if let Some(task) = self.tasks.get_mut(&id) {
            if task.active != 0 && matches!(task.kind, 57 | 58) {
                let mut geometry = task
                    .geometry
                    .clone()
                    .context("flip geometry owner is absent")?;
                geometry.advance(now, task.started, task.end)?;
                task.geometry = Some(geometry);
            }
            task.now = now;
        }
        Ok(())
    }

    pub(super) fn remove(&mut self, id: u32) -> bool {
        self.tasks.remove(&id).is_some_and(|task| task.registered)
    }

    pub(super) fn compose(
        &mut self,
        id: u32,
        target: &mut Canvas,
        sources: [Option<&RgbaFrame>; 2],
        draw: &mut impl FnMut(&mut Canvas, f32, f32) -> Result<()>,
    ) -> Result<()> {
        let original = self
            .tasks
            .get(&id)
            .context("frame effect owner is absent")?;
        let mut task = original.clone();
        if task.size != [target.width(), target.height()] {
            bail!("frame effect surface size changed without recreating its owner");
        }
        let far = task.depth()?;
        let mut frame = Canvas::new(task.size[0], task.size[1])?;
        frame.clear(Color::rgba(0, 0, 0, 255));
        draw(&mut frame, 0., far)?;
        if let Some(geometry) = task.geometry.as_mut() {
            if task.kind == 52 && task.active != 0 {
                geometry.advance(task.now, task.started, task.end)?;
            }
            geometry.draw(&mut frame, sources)?;
        } else {
            let alpha = if task.active != 0 {
                // The mosaic receiver compares the relative time to its stored
                // absolute endpoint, rather than to duration.
                let elapsed = task.now.wrapping_sub(task.started).min(task.end);
                task.select_level(elapsed)?
            } else {
                task.selected_side = 0;
                255
            };
            let image = task.work[task.selected_side]
                .as_deref()
                .or(task.captures[task.selected_side].as_deref())
                .context("frame effect has no completed capture for its selected side")?;
            pixels::composite(&mut frame, image, alpha, true)?;
        }
        draw(&mut frame, far, 1_000_000.)?;
        task.check_capacity()?;
        super::verify_deferred_output(&frame)?;
        if task.now >= task.end {
            task.active = 0;
        }
        // Commit both pixels and activity only after both live ranges succeed.
        *target = frame;
        self.tasks.insert(id, task);
        Ok(())
    }
}

impl RenderTaskStore {
    fn needs_effect_capture(&self, id: u32, operation: Operation, record: Option<&[u8]>) -> bool {
        self.effects.tasks.get(&id).is_some_and(|task| {
            task.available && task.kind == 50 && match operation {
                Operation::CaptureSecond => true,
                Operation::Prepare => record.and_then(|bytes| bytes.get(8..12))
                    .is_some_and(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()) >= 2),
                _ => false,
            }
        })
    }

    pub(crate) fn prepare_effect_auxiliary(
        &mut self,
        count: u32,
        size: [u32; 2],
        policy: Option<i32>,
    ) -> Result<bool> {
        let Some(limit) = policy.filter(|_| !size.contains(&0)) else {
            // 546e10/5c5500 return before looking at +618 when the
            // graphics context/backbuffer gate is closed. An unsuccessful
            // new owner must not discard an older owner's retained targets.
            return Ok(false);
        };
        // Existing auxiliary surfaces survive a smaller configured limit;
        // the device checks the limit only when it must grow the pool.
        if count > self.auxiliary_count
            && limit > 0
            && size.iter().any(|&value| value > limit as u32)
        {
            self.release_auxiliary_targets();
            return Ok(false);
        }
        self.prepare_auxiliary_targets(count, size, limit)?;
        Ok(true)
    }

    pub(super) fn frame_effect_active(&self, id: u32) -> Option<u32> {
        self.effects.tasks.get(&id).map(|task| {
            if matches!(task.kind, 52 | 57) && !task.available {
                0
            } else {
                task.active
            }
        })
    }

    pub(super) fn register_frame_effect(
        &mut self,
        id: u32,
        kind: u32,
        size: [u32; 2],
        profile: Option<&cs2_runtime::native_profile::FeatureProfile>,
    ) -> Result<()> {
        check_work(size)?;
        let mut task = Effect {
            kind,
            available: !size.contains(&0),
            registered: kind != 50 && !size.contains(&0),
            active: 0,
            size,
            words: vec![0, 1_000_000.0f32.to_bits(), 0, 1, 0, 0],
            now: 0,
            started: 0,
            end: 0,
            captures: [None, None],
            levels: Vec::new(),
            cached_time: None,
            selected_side: 0,
            work: [None, None],
            geometry: if matches!(kind, 52 | 57 | 58) && !size.contains(&0) {
                Some(geometry::Geometry::new(kind, size)?)
            } else {
                None
            },
            current_geometry: match profile.map(|profile| profile.pe_sha256) {
                Some(V401) => Some(false),
                Some(YUKIKOI) => Some(true),
                _ => None,
            },
            scale_profile: profile.and_then(|profile| match (kind, profile.pe_sha256) {
                // Hatsumira WMosaic uses the V401 format4 kernels (0x534880,
                // 0x534e20) and format32 hard expansion (0x4c2cee).
                (50, super::HATSUMIRA) => Some(resample::Profile::V401),
                (_, sha) => resample::Profile::from_sha(sha),
            }),
            flag: false,
        };
        // FMosaic owns a constructor-time copy; WMosaic captures at prepare.
        if task.available && kind == 51 {
            self.prepare_auxiliary_targets(1, size, 0)?;
            task.captures[0] = self.auxiliary_target(0).cloned().map(Arc::new);
            task.work[0] = task.captures[0].clone();
            self.release_auxiliary_targets();
        }
        if task.available && matches!(kind, 52 | 57 | 58) {
            self.prepare_auxiliary_targets(if kind == 58 { 2 } else { 1 }, size, 0)?;
        }
        task.check_capacity()?;
        self.effects.tasks.insert(id, task);
        if kind != 50 && self.effects.participates(id) {
            self.callback_owner = Some(id);
        }
        Ok(())
    }

    fn effect_command(
        &mut self,
        id: u32,
        operation: Operation,
        scalar: u32,
        record: Option<&[u8]>,
        now: u32,
        capture_policy: Option<i32>,
        output: &mut u32,
        random: &mut impl FnMut() -> f32,
    ) -> Result<()> {
        let mut task = self
            .effects
            .tasks
            .get(&id)
            .context("frame effect has no concrete graphics owner")?
            .clone();
        if operation == Operation::Query {
            *output = if matches!(task.kind, 52 | 57) && !task.available {
                0
            } else {
                task.active
            };
            return Ok(());
        }
        if !task.available {
            return Ok(());
        }
        task.now = now;
        let mut claim_callback = false;
        let mut release_auxiliary = false;
        match operation {
            Operation::Prepare => {
                let count = match task.kind {
                    50 => 5,
                    51 | 57 => 6,
                    52 => 7,
                    58 => 9,
                    _ => unreachable!(),
                };
                task.words = words(record, count)?;
                if task.kind == 50 && task.words[2] < 2 {
                    task.available = false;
                    self.effects.tasks.insert(id, task);
                    return Ok(());
                }
                task.depth()?;
                if task.active != 0 {
                    task.restart(now);
                }
                if let Some(geometry) = task.geometry.as_mut() {
                    geometry.configure(&task.words, task.current_geometry, random)?;
                } else if task.kind == 50 {
                    if !self.prepare_effect_auxiliary(2, task.size, capture_policy)? {
                        task.available = false;
                        self.effects.tasks.insert(id, task);
                        return Ok(());
                    }
                    task.captures[0] = self.auxiliary_target(0).cloned().map(Arc::new);
                    if task.captures[0].is_none() {
                        task.available = false;
                    } else {
                        task.levels = build_levels(&task, 0, task.words[2] >> 1)?;
                        task.work[0] = task.captures[0].clone();
                        task.registered = true;
                        claim_callback = true;
                    }
                } else {
                    task.levels = build_levels(&task, 0, task.words[2])?;
                    task.select_level(0)?;
                }
            }
            Operation::CaptureSecond => {
                if !self.prepare_effect_auxiliary(2, task.size, capture_policy)? {
                    task.available = false;
                    self.effects.tasks.insert(id, task);
                    return Ok(());
                }
                task.captures[1] = self.auxiliary_target(1).cloned().map(Arc::new);
                release_auxiliary = true;
                if task.captures[1].is_none() {
                    task.available = false;
                } else {
                    let mut second = build_levels(&task, 1, task.words[2] >> 1)?;
                    let length = second.len();
                    let half = task.words[2] >> 1;
                    // The original adjusts paired timestamps only. With an
                    // odd number of levels the unpaired middle is unchanged.
                    for index in 0..length / 2 {
                        let other = length - 1 - index;
                        let at = second[index].at;
                        second[index].at = second[other].at.wrapping_add(half);
                        second[other].at = at.wrapping_add(half);
                    }
                    second.reverse();
                    check_retained(&task.levels, &second)?;
                    task.levels.extend(second);
                    task.work[1] = task.captures[1].clone();
                    task.select_level(0)?;
                    // Both capture calls install the window callback. A
                    // different render task may have claimed it since prepare.
                    task.registered = true;
                    claim_callback = true;
                }
            }
            Operation::Play => {
                task.active = scalar;
                if scalar != 0 {
                    task.restart(now);
                    if let Some(geometry) = task.geometry.as_mut() {
                        geometry.restart();
                    }
                }
                task.cached_time = None;
            }
            Operation::Flag => task.flag = true,
            Operation::Consume => {}
            Operation::Delegate => {}
            Operation::Query => unreachable!(),
        }
        task.check_capacity()?;
        self.effects.tasks.insert(id, task);
        if release_auxiliary {
            self.release_auxiliary_targets();
        }
        if claim_callback {
            self.callback_owner = Some(id);
        }
        Ok(())
    }
}

#[test]
fn ordinary_568_supplies_the_wbreak_fragments() -> Result<()> {
    use cs2_script::Host;

    let temp = crate::tests::TempDir::new();
    let mut player = crate::HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("profile"))?;
    let previous = Color::rgb(192, 64, 32);
    let current = Color::rgb(24, 80, 160);
    let size = [player.canvas.width(), player.canvas.height()];
    let depth = 45_000.0f32.to_bits();
    let effect = player.with_host(|host| {
        let plane = host.create_task_object(2, 0, b"background", b"")?;
        let node = host.scene_nodes.get_mut(&plane).unwrap();
        node.size = size.map(|axis| axis as f32);
        node.position = [0., 0., 5000.];
        node.color = 0xffc0_4020;
        node.visible = true;
        node.initialized = true;

        let mut vm = Vm::new();
        let capture = [0, depth].map(u32::to_le_bytes).concat();
        host.call(568, &capture, &mut vm)?;
        host.scene_nodes.get_mut(&plane).unwrap().color = 0xff18_50a0;

        let effect = host.create_task_object(52, 0, b"wbreak", b"")?;
        let prepare = [effect, 58, 0, depth, 8, 8, 15.0f32.to_bits(), 30.0f32.to_bits()];
        host.call(860, &prepare.map(u32::to_le_bytes).concat(), &mut vm)?;
        host.call(861, &[effect, 1].map(u32::to_le_bytes).concat(), &mut vm)?;
        Ok(effect)
    })?;

    player.render_scene()?;
    assert!(player.canvas.pixels().chunks_exact(4).all(|pixel| {
        pixel == [previous.r, previous.g, previous.b, 255]
    }), "each fragment must sample the previous scene without seams at the start");

    player.with_host(|host| host.data_services.tasks.render.update(effect, 20))?;
    player.render_scene()?;
    assert!(player.canvas.pixels().chunks_exact(4).any(|pixel| {
        pixel[0] > current.r && pixel[0] < previous.r
    }), "moving shards must blend the previous and current scene");

    player.with_host(|host| host.data_services.tasks.render.update(effect, 58))?;
    player.render_scene()?;
    assert!(player.canvas.pixels().chunks_exact(4).all(|pixel| {
        pixel == [current.r, current.g, current.b, 255]
    }), "the completed transition must show the current scene");
    Ok(())
}

#[test]
fn wbreak_prepare_does_not_require_a_flip_receiver_version() {
    let mut render = RenderTaskStore::default();
    let effect_id = 108;
    let wbreak_words = [
        0,
        45_000.0f32.to_bits(),
        58,
        8,
        8,
        15.0f32.to_bits(),
        3.0f32.to_bits(),
    ];
    let wbreak_record = wbreak_words.map(u32::to_le_bytes).concat();
    render
        .register_frame_effect(effect_id, 52, [32, 24], None)
        .unwrap();
    render
        .effect_command(
            effect_id,
            Operation::Prepare,
            0,
            Some(&wbreak_record),
            0,
            Some(0),
            &mut 0,
            &mut || 0.5,
        )
        .unwrap();

    let flip_id = 109;
    let flip_record = [0; 36];
    render
        .register_frame_effect(flip_id, 58, [32, 24], None)
        .unwrap();
    let error = render
        .effect_command(
            flip_id,
            Operation::Prepare,
            0,
            Some(&flip_record),
            0,
            Some(0),
            &mut 0,
            &mut || 0.5,
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "effect geometry needs an identified receiver version"
    );
}

impl Effect {
    fn check_capacity(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        let mut retained = 0usize;
        for image in self
            .captures
            .iter()
            .chain(&self.work)
            .filter_map(Option::as_ref)
            .chain(self.levels.iter().map(|level| &level.image))
        {
            if seen.insert(Arc::as_ptr(image) as usize) {
                retained = retained
                    .checked_add(image.pixels().len())
                    .context("frame effect retained pixel capacity overflow")?;
                if retained > MAX_RETAINED_BYTES {
                    bail!("frame effect captures, levels and work exceed 256 MiB");
                }
            }
        }
        Ok(())
    }

    fn depth(&self) -> Result<f32> {
        let far = f32::from_bits(self.words[1]);
        if !far.is_finite() {
            bail!("frame effect split depth is non-finite");
        }
        Ok(far)
    }

    fn restart(&mut self, now: u32) {
        self.started = now;
        self.end = now.wrapping_add(self.words[2]);
    }

    fn select_level(&mut self, elapsed: u32) -> Result<u8> {
        let index = if self.cached_time.is_none() {
            (!self.levels.is_empty()).then_some(0)
        } else {
            self.levels
                .iter()
                .position(|entry| entry.at > elapsed)
                .map(|index| index.saturating_sub(1))
        };
        let Some(level) = index.map(|index| &self.levels[index]) else {
            return Ok(if self.kind == 51 { 0 } else { 255 });
        };
        let alpha = level.alpha;
        if self.cached_time != Some(level.at) {
            let work = expand_level(level, self.size, self.words[4] == 0,
                self.work[level.side].as_deref(), self.scale_profile)?;
            self.work[level.side] = Some(Arc::new(work));
            self.selected_side = level.side;
            self.cached_time = Some(level.at);
        }
        Ok(alpha)
    }
}

fn words(record: Option<&[u8]>, count: usize) -> Result<Vec<u32>> {
    let record = record
        .and_then(|bytes| bytes.get(..count * 4))
        .context("frame effect preparation record is absent or truncated")?;
    Ok(record
        .chunks_exact(4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
        .collect())
}

fn build_levels(task: &Effect, side: usize, duration: u32) -> Result<Vec<Level>> {
    let source = task.captures[side]
        .as_ref()
        .context("frame effect capture is absent")?;
    if [source.width(), source.height()] != task.size {
        bail!("frame effect capture dimensions differ from its registered surface");
    }
    let mut levels = vec![Level {
        image: source.clone(),
        block: 1,
        at: 0,
        alpha: 255,
        side,
    }];
    let maximum = task.words[3] as i32;
    if duration <= 1 || maximum == 1 {
        return Ok(levels);
    }
    if maximum < 1 {
        bail!("frame effect level has an invalid block divisor");
    }
    let block_at = |at: u32| -> Result<i32> {
        let fraction = at as f32 / duration as f32;
        let value = fraction * (maximum as f32 - 1.) + 1.;
        if !value.is_finite() || value < 1. || value >= i32::MAX as f32 {
            bail!("frame effect level has an invalid block divisor");
        }
        Ok(value.trunc() as i32)
    };
    let last = duration - 1;
    let final_block = block_at(last)?;
    let mut previous = 1i32;
    let mut bytes = source.pixels().len();
    let mut search_from = 1;
    while search_from < duration && previous < final_block {
        // With maximum>=1 the native rounded f32 expression is monotone.
        // Find its first changed integer using that same expression, rather
        // than approximating the timestamp by an inverse float formula.
        // Large durations with repeated blocks need no per-tick allocation
        // or iteration; their first timestamp and FMosaic alpha stay exact.
        let mut low = search_from;
        let mut high = last;
        while low < high {
            let middle = low + (high - low) / 2;
            if block_at(middle)? > previous {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        let at = low;
        let block = block_at(at)?;
        let fraction = at as f32 / duration as f32;
        previous = block;
        if levels.len() >= MAX_LEVELS as usize {
            bail!("frame effect exceeds retained-level count budget");
        }
        let size = task
            .size
            .map(|value| value.div_ceil(block as u32).saturating_add(1));
        check_work(size)?;
        let allocation = size[0] as usize * size[1] as usize * 4;
        bytes = bytes
            .checked_add(allocation)
            .context("frame effect level capacity overflow")?;
        if bytes > MAX_RETAINED_BYTES {
            bail!("frame effect levels exceed 256 MiB");
        }
        let image = resample::resize(&levels.last().unwrap().image, size, None,
            task.scale_profile.context("frame effect scaling needs an identified PE descriptor")?)?;
        levels.push(Level {
            image: Arc::new(image),
            block: block as u32,
            at,
            alpha: if task.kind == 51 {
                255u8.wrapping_sub((fraction * 255.).trunc() as u8)
            } else {
                255
            },
            side,
        });
        // at<=duration-1, so this addition is representable even when the
        // supplied duration is u32::MAX.
        search_from = at + 1;
    }
    Ok(levels)
}

fn check_retained(first: &[Level], second: &[Level]) -> Result<()> {
    let bytes = first
        .iter()
        .chain(second)
        .try_fold(0usize, |total, level| {
            total
                .checked_add(level.image.pixels().len())
                .context("frame effect capacity overflow")
        })?;
    if bytes > MAX_RETAINED_BYTES {
        bail!("frame effect levels exceed 256 MiB");
    }
    Ok(())
}

fn expand_level(
    level: &Level, size: [u32; 2], hard: bool, previous: Option<&RgbaFrame>,
    profile: Option<resample::Profile>,
) -> Result<RgbaFrame> {
    let profile = profile.context("frame effect expansion needs an identified PE descriptor")?;
    if !hard {
        return resample::resize(&level.image, size, previous, profile);
    }
    resample::expand_blocks(&level.image, size, level.block, profile)
}

fn message_descriptor(
    kind: u32,
    message: u32,
    hash: Option<&str>,
) -> cs2_script::Result<Option<Descriptor>> {
    let old = match hash {
        Some(V401) => true,
        Some(YUKIKOI) => false,
        _ => {
            return Err(native_state_error(
                "frame effect messages need an identified PE descriptor",
            ));
        }
    };
    if message == if old { 372 } else { 383 } {
        return Ok(Some(Descriptor {
            slot: 0,
            kind,
            operation: Operation::Query,
            input_words: 0,
            record_words: 0,
            old_message: 372,
            current_message: 383,
        }));
    }
    Ok(DESCRIPTORS
        .iter()
        .find(|entry| {
            entry.kind == kind
                && message
                    == if old {
                        entry.old_message
                    } else {
                        entry.current_message
                    }
        })
        .copied())
}

impl LoggingHost<'_> {
    pub(crate) fn frame_effect_capture_policy(&mut self) -> Result<Option<i32>> {
        self.ensure_canvas_state()?;
        self.ensure_configuration_state()?;
        let state = &self.data_services.canvas_native;
        if self.canvas.is_none()
            || !state.enabled
            || state.mode != 0
            || self.data_services.configuration.platform_value("D3D/backbuffer") == Some(0)
        {
            return Ok(None);
        }
        Ok(Some(self.data_services.tasks.render.auxiliary_dimension_limit()))
    }

    /// GraphicsFactory entry for the retained mosaic/fragment receivers.
    pub(crate) fn register_retained_frame_effect(
        &mut self,
        id: u32,
        kind: u32,
    ) -> cs2_script::Result<()> {
        if !matches!(kind, 50..=52) {
            return Err(native_state_error("retained frame-effect factory has the wrong kind"));
        }
        let mut size = self.back_buffer.or(self.canvas.as_deref())
            .map_or([0; 2], |canvas| [canvas.width(), canvas.height()]);
        if matches!(kind, 51 | 52) {
            let policy = self.frame_effect_capture_policy()
                .map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?;
            if !self.data_services.tasks.render.prepare_effect_auxiliary(1, size, policy)
                .map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?
            {
                size = [0; 2];
            }
        }
        self.data_services.tasks.render.register_frame_effect(
            id, kind, size, self.data_services.native_profile,
        ).map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))
    }

    pub(super) fn frame_effect_native(
        &mut self,
        index: u32,
        args: &[u8],
    ) -> cs2_script::Result<NativeReturn> {
        let descriptor = DESCRIPTORS
            .iter()
            .find(|entry| entry.slot == index)
            .ok_or(cs2_script::ScriptError::UnimplementedNativeCall { index })?;
        let id = read_u32(args, 0)?;
        if id == 0 {
            return Ok(NativeReturn::Continue);
        }
        let input = args
            .get(..descriptor.input_words * 4)
            .ok_or_else(|| native_state_error("frame effect native arguments are truncated"))?;
        let (kind, deletion, _) = self
            .data_services
            .tasks
            .message_task_state(id)
            .ok_or_else(|| native_state_error("frame effect target is absent"))?;
        if deletion != 0 {
            return Ok(NativeReturn::Continue);
        }
        if kind != descriptor.kind {
            return Err(native_state_error(
                "frame effect target has the wrong concrete receiver",
            ));
        }
        let mut record = Vec::new();
        if descriptor.operation == Operation::Prepare {
            let offsets: &[usize] = match descriptor.slot {
                853 => &[8, 12, 4, 16, 20],
                856 => &[8, 12, 4, 16, 20, 24],
                860 => &[8, 12, 4, 16, 20, 24, 28],
                967 => &[20, 24, 4, 8, 12, 16],
                971 => &[32, 36, 4, 8, 12, 16, 20, 24, 28],
                _ => return Err(native_state_error("frame effect wrapper record is not described")),
            };
            for &offset in offsets {
                record.extend_from_slice(&input[offset..offset + 4]);
            }
        }
        let scalar = if descriptor.operation == Operation::Play {
            read_u32(input, 4)?
        } else {
            0
        };
        let now = self
            .data_services
            .tasks
            .task_clock(id)
            .ok_or_else(|| native_state_error("frame effect clock is absent"))?
            .elapsed();
        let capture_policy = if self.data_services.tasks.render.needs_effect_capture(
            id, descriptor.operation, (!record.is_empty()).then_some(record.as_slice()),
        )
        {
            self.frame_effect_capture_policy()
                .map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?
        } else {
            Some(0)
        };
        let random = &mut self.script_random;
        self.data_services
            .tasks
            .render
            .effect_command(
                id,
                descriptor.operation,
                scalar,
                (!record.is_empty()).then_some(record.as_slice()),
                now,
                capture_policy,
                &mut 0,
                &mut || f32::from_bits(random.next_float_bits()),
            )
            .map_err(|cause| {
                self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string()))
            })?;
        Ok(NativeReturn::Continue)
    }

    pub(super) fn receive_frame_effect_command(
        &mut self,
        command: TaskCommand,
        buffers: &mut TaskCommandBuffers<'_>,
        output: &mut u32,
    ) -> cs2_script::Result<Option<TaskCommandOutcome>> {
        let id = command.target as u32;
        let Some((kind, _, _)) = self.data_services.tasks.message_task_state(id) else {
            return Ok(None);
        };
        if !supports_kind(kind) || command.code < 350 {
            return Ok(None);
        }
        let descriptor = message_descriptor(
            kind,
            command.code,
            self.data_services
                .native_profile
                .map(|profile| profile.pe_sha256),
        )?;
        let Some(descriptor) = descriptor else {
            return Ok(None);
        };
        let now = self
            .data_services
            .tasks
            .task_clock(id)
            .ok_or_else(|| native_state_error("frame effect clock is absent"))?
            .elapsed();
        let capture_policy = if self.data_services.tasks.render.needs_effect_capture(
            id, descriptor.operation, buffers.data1.as_deref(),
        )
        {
            self.frame_effect_capture_policy()
                .map_err(|cause| self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string())))?
        } else {
            Some(0)
        };
        let random = &mut self.script_random;
        self.data_services
            .tasks
            .render
            .effect_command(
                id,
                descriptor.operation,
                command.data[1],
                buffers.data1.as_deref(),
                now,
                capture_policy,
                output,
                &mut || f32::from_bits(random.next_float_bits()),
            )
            .map_err(|cause| {
                self.task_error(id, cs2_runtime::RuntimeError::Host(cause.to_string()))
            })?;
        Ok(Some(TaskCommandOutcome {
            done: true,
            propagate: descriptor.operation == Operation::Delegate,
        }))
    }

    pub(super) fn dispatch_frame_effect_message(
        &mut self,
        target: u32,
        packet: [u32; 4],
        queued: bool,
        output: &mut u32,
        vm: Option<&mut Vm>,
    ) -> cs2_script::Result<Option<TaskCommandOutcome>> {
        if packet[1] < 350 {
            return Ok(None);
        }
        let Some((kind, _, _)) = self.data_services.tasks.message_task_state(target) else {
            return Ok(None);
        };
        let Some(descriptor) = message_descriptor(
            kind,
            packet[1],
            self.data_services
                .native_profile
                .map(|profile| profile.pe_sha256),
        )?
        else {
            return Ok(None);
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
        let available = self
            .data_services
            .tasks
            .render
            .effects
            .tasks
            .get(&target)
            .is_some_and(|task| task.available);
        let bytes = if descriptor.record_words == 0 || !available {
            None
        } else {
            Some(
                vm.ok_or_else(|| {
                    native_state_error(
                        "frame effect configuration requires its sender VM or owned record",
                    )
                })?
                .read(packet[3], descriptor.record_words * 4)?,
            )
        };
        let mut buffers = super::super::OwnedTaskBuffers::new(None, bytes)?;
        if queued {
            if descriptor.operation == Operation::Query {
                return Err(native_state_error(
                    "queued frame effect query cannot retain a script output slot",
                ));
            }
            Ok(self
                .queue_task_command_buffers(command, &buffers, output)?
                .then_some(TaskCommandOutcome {
                    done: true,
                    propagate: false,
                }))
        } else {
            self.route_task_command_buffers(command, &mut buffers, output, 4096)
        }
    }
}
