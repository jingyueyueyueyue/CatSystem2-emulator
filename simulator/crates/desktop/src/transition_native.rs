//! Type 26/27/28 canvas transitions and ordinary 196/197/198/200/203.
//! Sources are borrowed wrapper handles; work pixels and preparation state
//! belong to the transition. No original executable or graphics API is used.
use std::collections::{BTreeMap, VecDeque};

use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, FramePixelIdentity, ImageDrawOptions, Point, Rect, RgbaFrame};
use cs2_script::{NativeReturn, Vm};

use super::{LoggingHost, native_state_error, read_u32};
use super::plane_native::native_image_offset;

#[path = "transition_state.rs"]
mod checkpoint;

#[derive(Default)]
pub(super) struct TransitionStore {
    tasks: BTreeMap<u32, Transition>,
    queued: BTreeMap<u32, VecDeque<(u32, Vec<u8>)>>,
}

struct Transition {
    kind: u32,
    words: Vec<u32>,
    duration: u32,
    started: u32,
    speed: f32,
    active: bool,
    size: [u32; 2],
    pixels: Vec<u8>,
    output: Point,
    local: [u32; 2],
    submitted: bool,
    rendered: Option<(Point, RgbaFrame)>,
    rendered_sample: Option<TransitionSample>,
}

struct TransitionSample {
    inputs: [Option<FramePixelIdentity>; 3],
    region: [u32; 4],
    progress: u32,
    ended: bool,
    full: bool,
}

impl TransitionSample {
    fn matches(&self, inputs: [Option<&RgbaFrame>; 3], region: [u32; 4], progress: u32, ended: bool, full: bool) -> bool {
        self.region == region && self.progress == progress && self.ended == ended && self.full == full
            && self.inputs.iter().zip(inputs).all(|(old, current)| match (old, current) {
                (Some(old), Some(current)) => old.matches(current),
                (None, None) => true,
                _ => false,
            })
    }
}

impl LoggingHost<'_> {
    pub(super) fn transition_command_bytes(
        &mut self, target: u32, code: u32, bytes: Option<&[u8]>, output: &mut u32,
    ) -> Result<()> {
        self.apply_transition_command(target, code, bytes, output)
    }

    pub(super) fn transition_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let target = read_u32(args, 0)?;
        let Some(kind) = self.scene_nodes.get(&target).map(|node| node.kind) else {
            return if index == 200 {
                Err(native_state_error("native 200 transition query has no receiver"))
            } else { Ok(NativeReturn::Continue) };
        };
        if self.data_services.tasks.message_task_state(target)
            .is_some_and(|(_, deletion, _)| deletion != 0)
        { return Ok(if index == 200 { NativeReturn::Value(0) } else { NativeReturn::Continue }); }
        if index == 196 && !(17..=31).contains(&kind) { return Ok(NativeReturn::Continue); }
        if !matches!(kind, 26..=28) {
            return Err(native_state_error("transition task type needs its independent record/effect"));
        }
        let code = match index {
            196 => if kind == 27 { 281 } else { 280 },
            197 => 263, 198 => 264, 200 => 265, 203 => 267,
            _ => return Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        };
        let input = if index == 196 {
            Some(vm.read(read_u32(args, 4)?, record_size(kind))?)
        } else { None };
        let mut output = 0;
        self.apply_transition_command(target, code, input.as_deref(), &mut output)
            .map_err(|error| self.transition_error(target, error))?;
        Ok(if index == 200 { NativeReturn::Value(output) } else { NativeReturn::Continue })
    }

    pub(super) fn dispatch_transition_message(
        &mut self, target: u32, packet: [u32; 4], queued: bool,
        output: &mut u32, vm: Option<&mut Vm>,
    ) -> cs2_script::Result<bool> {
        let Some(kind) = self.scene_nodes.get(&target).map(|node| node.kind) else { return Ok(false); };
        if !matches!(kind, 26..=28) { return Ok(false); }
        let code = packet[1];
        if !matches!(code, 263..=267 | 280 | 281) { return Ok(false); }
        let size = match code {
            280 | 281 => record_size(kind),
            266 => if kind == 28 { 8 } else { 20 },
            _ => 0,
        };
        let input = if size != 0 {
            let vm = vm.ok_or_else(|| native_state_error("transition command needs a VM or owned record"))?;
            vm.read(packet[3], size)?
        } else { Vec::new() };
        if queued {
            let queue = self.data_services.transitions.queued.entry(target).or_default();
            if queue.len() >= 4096 { return Err(native_state_error("transition message queue exceeded")); }
            queue.push_back((code, input));
            return Ok(true);
        }
        self.apply_transition_command(target, code, (!input.is_empty()).then_some(input.as_slice()), output)
            .map_err(|error| self.transition_error(target, error))?;
        Ok(true)
    }

    fn apply_transition_command(
        &mut self, target: u32, code: u32, input: Option<&[u8]>, output: &mut u32,
    ) -> Result<()> {
        let kind = self.scene_nodes.get(&target).context("transition task disappeared")?.kind;
        if self.data_services.tasks.message_task_state(target)
            .is_some_and(|(_, deleted, _)| deleted != 0)
        { return Ok(()); }
        let now = self.data_services.tasks.task_clock(target).map_or(0, |clock| clock.elapsed());
        if matches!(code, 280 | 281) {
            if code != if kind == 27 { 281 } else { 280 } {
                bail!("transition prepare command does not match its task type");
            }
            self.data_services.transitions.tasks.remove(&target);
            let input = input.context("transition preparation record is absent")?;
            if input.len() < record_size(kind) { bail!("transition preparation record is truncated"); }
            let words: Vec<_> = input[..record_size(kind)].chunks_exact(4)
                .map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
            let handles: &[usize] = match kind { 26 => &[1, 4, 7], 27 => &[1, 4], 28 => &[1, 4], _ => unreachable!() };
            if handles.iter().any(|&index| words[index] == 0) { return Ok(()); }
            let first = self.image_handles.get(&words[1]).context("transition first source is not an image wrapper")?.clone();
            let size = match kind {
                26 => {
                    let second = self.image_handles.get(&words[4]).context("wipe second source disappeared")?;
                    [first.width().min(second.width()), first.height().min(second.height())]
                }
                27 => {
                    let second = self.image_handles.get(&words[4]).context("fade second source disappeared")?;
                    let mut size = [words[9], words[10]];
                    for (axis, limit) in [first.width(), first.height()].into_iter().enumerate() {
                        if (size[axis] as i32) < 0 { bail!("negative fade dimensions"); }
                        let first_at = words[2 + axis] as i32;
                        let second_at = words[5 + axis] as i32;
                        let second_limit = if axis == 0 { second.width() } else { second.height() };
                        if first_at < 0 || second_at < 0 || first_at as u32 > limit || second_at as u32 > second_limit {
                            bail!("fade source origin lies outside its image");
                        }
                        size[axis] = size[axis].min(limit - first_at as u32).min(second_limit - second_at as u32);
                    }
                    size
                }
                28 => [words[7], words[8]],
                _ => unreachable!(),
            };
            let length = (size[0] as usize).checked_mul(size[1] as usize)
                .and_then(|n| n.checked_mul(4)).filter(|&n| n <= 128 * 1024 * 1024)
                .context("transition work surface exceeds 128 MiB")?;
            let mut pixels = Vec::new();
            pixels.try_reserve_exact(length)?;
            pixels.resize(length, 0);
            let mut task = Transition {
                kind, duration: (words[0] as i32).max(1) as u32, words,
                started: 0, speed: 1., active: false, size, pixels,
                output: Point::default(), local: [0; 2], submitted: false, rendered: None, rendered_sample: None,
            };
            task.output = match kind {
                26 => Point::new(task.words[8] as i32, task.words[9] as i32),
                27 => Point::new(task.words[7] as i32, task.words[8] as i32),
                28 => Point::new(task.words[5] as i32, task.words[6] as i32),
                _ => unreachable!(),
            };
            task.local = match kind {
                26 => [task.words[12], task.words[13]],
                28 => [task.words[10], task.words[11]],
                _ => [0; 2],
            };
            self.prepare_transition_sources(&mut task)?;
            self.data_services.transitions.tasks.insert(target, task);
            return Ok(());
        }
        if code == 265 {
            *output = self.data_services.transitions.tasks.get(&target).map_or(0, |task| u32::from(task.active));
            return Ok(());
        }
        let Some(mut task) = self.data_services.transitions.tasks.remove(&target) else { return Ok(()); };
        let result = (|| -> Result<()> {
            match code {
                263 => {
                    let mode = self.system_config_blocks[2].get(..4)
                        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap())).unwrap_or(0);
                    task.speed = match mode { 1 => 1.5, 2 => 2., 3 => 3., 4 => 4., _ => 1. };
                    task.started = now;
                    task.active = true;
                    task.submitted = false;
                }
                264 | 267 if code == 264 || task.active => {
                    self.sample_transition(&mut task, now, true, false)?;
                    task.active = false;
                }
                266 => {
                    let input = input.context("transition replacement source record is absent")?;
                    task.words[1] = read_u32(input, 4)?;
                    if task.kind != 28 { task.words[4] = read_u32(input, 16)?; }
                    if task.kind == 27 {
                        let first = self.image_handles.get(&task.words[1]).context("fade replacement first source missing")?;
                        let second = self.image_handles.get(&task.words[4]).context("fade replacement second source missing")?;
                        if first.dimensions() != second.dimensions() { bail!("fade replacement sources differ in full dimensions"); }
                    }
                }
                267 => {}
                _ => bail!("transition command has no receiver"),
            }
            Ok(())
        })();
        self.data_services.transitions.tasks.insert(target, task);
        result
    }

    fn prepare_transition_sources(&mut self, task: &mut Transition) -> Result<()> {
        let first_handle = task.words[1];
        let first = self.image_handles.get(&first_handle).context("transition first image missing")?.clone();
        let first_top_down = self.is_list_output_wrapper(first_handle);
        validate_region(&first, [task.words[2], task.words[3]], task.size)?;
        if task.kind == 28 {
            let mask = self.image_handles.get(&task.words[4]).context("alpha wipe mask missing")?;
            validate_local(task, mask)?;
            let row_bytes = task.size[0] as usize * 4;
            for row in 0..task.size[1] {
                let source = native_image_offset(&first, task.words[2], task.words[3] + row, first_top_down);
                let target = row as usize * row_bytes;
                task.pixels[target..target + row_bytes].copy_from_slice(&first.pixels()[source..source + row_bytes]);
            }
            if task.words[9] != 0 {
                for pixel in task.pixels.chunks_exact_mut(4) { pixel[3] = 0; }
            }
            return Ok(());
        }
        let mut first_pixels = first.pixels().to_vec();
        let second_handle = task.words[4];
        let second = self.image_handles.get(&second_handle).context("transition second image missing")?.clone();
        let second_top_down = self.is_list_output_wrapper(second_handle);
        validate_region(&second, [task.words[5], task.words[6]], task.size)?;
        if task.kind == 26 {
            validate_local(task, self.image_handles.get(&task.words[7]).context("wipe mask missing")?)?;
        }
        let shared = first_handle == second_handle
            || self.data_services.image_wrapper_pixels.get(&first_handle)
                .is_some_and(|pixel| self.data_services.image_wrapper_pixels.get(&second_handle) == Some(pixel));
        let mut second_pixels = if shared { Vec::new() } else { second.pixels().to_vec() };
        for row in 0..task.size[1] {
            for column in 0..task.size[0] {
                let a = native_image_offset(&first, task.words[2] + column, task.words[3] + row, first_top_down);
                let b = native_image_offset(&second, task.words[5] + column, task.words[6] + row, second_top_down);
                let new_pixel: [u8; 4] = if shared { first_pixels[b..b + 4].try_into().unwrap() }
                    else { second_pixels[b..b + 4].try_into().unwrap() };
                if first_pixels[a + 3] == 0 {
                    first_pixels[a..a + 3].copy_from_slice(&new_pixel[..3]);
                } else if task.kind == 27 && new_pixel[3] == 0 {
                    let rgb: [u8; 3] = first_pixels[a..a + 3].try_into().unwrap();
                    if shared { first_pixels[b..b + 3].copy_from_slice(&rgb); }
                    else { second_pixels[b..b + 3].copy_from_slice(&rgb); }
                }
                let target = (row as usize * task.size[0] as usize + column as usize) * 4;
                task.pixels[target..target + 4].copy_from_slice(&first_pixels[a..a + 4]);
            }
        }
        self.replace_image_wrapper_pixels(first_handle, rebuild(&first, first_pixels)?)?;
        if !shared && task.kind == 27 { self.replace_image_wrapper_pixels(second_handle, rebuild(&second, second_pixels)?)?; }
        Ok(())
    }

    pub(super) fn advance_transition_task(&mut self, id: u32) -> cs2_script::Result<()> {
        while let Some((code, bytes)) = self.data_services.transitions.queued.get_mut(&id).and_then(VecDeque::pop_front) {
            if let Err(error) = self.apply_transition_command(id, code, (!bytes.is_empty()).then_some(bytes.as_slice()), &mut 0) {
                self.data_services.transitions.queued.entry(id).or_default().push_front((code, bytes));
                return Err(self.transition_error(id, error));
            }
        }
        let Some(mut task) = self.data_services.transitions.tasks.remove(&id) else { return Ok(()); };
        let now = self.data_services.tasks.task_clock(id).map_or(0, |clock| clock.elapsed());
        let result = if task.active { self.sample_transition(&mut task, now, false, true) } else { Ok(()) };
        self.data_services.transitions.tasks.insert(id, task);
        result.map_err(|error| self.transition_error(id, error))
    }

    fn sample_transition(&mut self, task: &mut Transition, now: u32, force: bool, natural: bool) -> Result<()> {
        let elapsed = if force { task.duration } else {
            ((now.wrapping_sub(task.started) as f32 * task.speed).trunc() as u64) as u32
        };
        let ended = elapsed >= task.duration;
        let first_top_down = self.is_list_output_wrapper(task.words[1]);
        let second_top_down = task.kind != 28 && self.is_list_output_wrapper(task.words[4]);
        let first = self.image_handles.get(&task.words[1]).context("transition first source was released")?;
        validate_region(first, [task.words[2], task.words[3]], task.size)?;
        let second = if task.kind != 28 {
            let second = self.image_handles.get(&task.words[4]).context("transition second source was released")?;
            validate_region(second, [task.words[5], task.words[6]], task.size)?;
            Some(second)
        } else { None };
        let mask = if task.kind != 27 {
            Some(self.image_handles.get(&task.words[if task.kind == 26 { 7 } else { 4 }])
                .context("transition mask was released")?)
        } else { None };
        let region = if ended && task.kind != 28 { [0, 0, task.size[0], task.size[1]] }
            else if let Some(mask) = mask {
                validate_local(task, mask)?;
                [task.local[0], task.local[1],
                    (task.size[0] - task.local[0]).min(mask.width()),
                    (task.size[1] - task.local[1]).min(mask.height())]
            } else { [0, 0, task.size[0], task.size[1]] };
        let progress = if ended { 256 }
            else { elapsed.wrapping_shl(if task.kind == 27 { 8 } else { 9 }) / task.duration };
        let full = ended && (task.kind != 28 || natural);
        let inputs = [Some(first), second, mask];
        if let Some((at, frame)) = &task.rendered
            && task.rendered_sample.as_ref().is_some_and(|sample| sample.matches(inputs, region, progress, ended, full))
        {
            if ended { task.active = false; }
            if full { task.local = [0; 2]; }
            self.submit_transition_frame(*at, frame)?;
            task.submitted = true;
            return Ok(());
        }
        task.rendered_sample = None;
        let row_bytes = region[2] as usize * 4;
        for row in 0..region[3] {
            let [x, y] = [region[0], region[1] + row];
            let target = (y as usize * task.size[0] as usize + x as usize) * 4;
            let output = &mut task.pixels[target..target + row_bytes];
            let a = native_image_offset(first, task.words[2] + x, task.words[3] + y, first_top_down);
            let first = &first.pixels()[a..a + row_bytes];
            let mask_row = mask.filter(|_| !ended).map(|mask| {
                let start = native_image_offset(mask, 0, row, false);
                &mask.pixels()[start..start + row_bytes]
            });
            if let Some(second) = second {
                let b = native_image_offset(second, task.words[5] + x, task.words[6] + y, second_top_down);
                let second = &second.pixels()[b..b + row_bytes];
                if ended {
                    output.copy_from_slice(second);
                } else if let Some(mask) = mask_row {
                    for (((output, first), second), mask) in output.chunks_exact_mut(4)
                        .zip(first.chunks_exact(4)).zip(second.chunks_exact(4)).zip(mask.chunks_exact(4))
                    {
                        let weight = mask_weight(progress, mask[2]);
                        for i in 0..4 {
                            output[i] = blend_channel(first[i], second[i], weight);
                        }
                    }
                } else {
                    let weight = progress.min(256);
                    for ((output, &first), &second) in output.iter_mut().zip(first).zip(second) {
                        *output = blend_channel(first, second, weight);
                    }
                }
            } else {
                for (column, (output, first)) in output.chunks_exact_mut(4).zip(first.chunks_exact(4)).enumerate() {
                    let weight = mask_row.map_or(progress.min(256), |mask| mask_weight(progress, mask[column * 4 + 2]));
                    let alpha = ((u32::from(first[3]) * weight) >> 8) as u8;
                    output[3] = if task.words[9] != 0 { alpha } else { !alpha };
                }
            }
        }
        if ended { task.active = false; }
        if full { task.local = [0; 2]; }
        let (at, frame) = if full || task.kind == 27 {
            (task.output, RgbaFrame::from_rgba8(task.size[0], task.size[1], task.pixels.clone())?)
        } else {
            let mut pixels = Vec::with_capacity(region[2] as usize * region[3] as usize * 4);
            for row in 0..region[3] {
                let start = ((region[1] + row) as usize * task.size[0] as usize + region[0] as usize) * 4;
                pixels.extend_from_slice(&task.pixels[start..start + region[2] as usize * 4]);
            }
            (Point::new(task.output.x.wrapping_add(region[0] as i32), task.output.y.wrapping_add(region[1] as i32)),
                RgbaFrame::from_rgba8(region[2], region[3], pixels)?)
        };
        let sample = TransitionSample {
            inputs: inputs.map(|frame| frame.map(RgbaFrame::pixel_identity)), region, progress, ended, full,
        };
        self.submit_transition_frame(at, &frame)?;
        task.rendered = Some((at, frame));
        task.rendered_sample = Some(sample);
        task.submitted = true;
        Ok(())
    }

    fn submit_transition_frame(&mut self, at: Point, frame: &RgbaFrame) -> Result<()> {
        if let Some(canvas) = self.canvas.as_mut() {
            // The canvas planes also sample the part of the VRAM atlas outside the viewport.
            self.data_services.canvas_native.copy_list_rectangle(
                frame, canvas, &mut self.data_services.planes.vram_overflow,
                Rect::new(0, 0, frame.width(), frame.height()), [at.x, at.y],
            )?;
        }
        Ok(())
    }

    fn transition_error(&mut self, id: u32, error: anyhow::Error) -> cs2_script::ScriptError {
        self.events.push(super::LogicEvent {
            frame: self.frame, kind: "transition-error".into(), detail: format!("task={id}: {error}"),
        });
        native_state_error("transition operation failed; see transition-error")
    }
}

pub(super) fn render(store: &TransitionStore, canvas: &mut Canvas) -> Result<()> {
    for task in store.tasks.values().filter(|task| task.submitted) {
        if let Some((at, frame)) = &task.rendered { canvas.copy_frame(frame, &ImageDrawOptions::new(*at))?; }
    }
    Ok(())
}

pub(super) fn release(store: &mut TransitionStore, id: u32) {
    store.tasks.remove(&id);
    store.queued.remove(&id);
}

fn mask_weight(progress: u32, blue: u8) -> u32 {
    progress.saturating_sub(u32::from(blue)).min(256)
}

fn blend_channel(first: u8, second: u8, weight: u32) -> u8 {
    ((u32::from(first) * (256 - weight) + u32::from(second) * weight) >> 8) as u8
}

fn record_size(kind: u32) -> usize { match kind { 26 => 56, 27 => 44, 28 => 48, _ => 0 } }
fn validate_region(image: &RgbaFrame, origin: [u32; 2], size: [u32; 2]) -> Result<()> {
    if origin[0] > image.width() || origin[1] > image.height()
        || size[0] > image.width() - origin[0] || size[1] > image.height() - origin[1]
    { bail!("transition source rectangle exceeds its image"); }
    Ok(())
}
fn validate_local(task: &Transition, _mask: &RgbaFrame) -> Result<()> {
    if task.local[0] > task.size[0] || task.local[1] > task.size[1] {
        bail!("transition local region begins outside the work surface");
    }
    Ok(())
}
fn rebuild(frame: &RgbaFrame, pixels: Vec<u8>) -> Result<RgbaFrame> {
    Ok(RgbaFrame::from_rgba8(frame.width(), frame.height(), pixels)?
        .with_raw_metadata(frame.metadata()).with_hg_attributes(frame.hg_attributes().clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeadlessPlayer;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn wipe_completes_full_canvas_after_sampling_smaller_mask() -> Result<()> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-wipe-{nonce}"));
        std::fs::create_dir(&root)?;
        let player = HeadlessPlayer::open_with_save_root(&root, root.join("save"));
        std::fs::remove_dir(&root)?;
        let mut player = player?;
        player.with_host(|host| {
            let first = [20, 40, 60, 64].repeat(6);
            let second = [80, 100, 120, 200].repeat(6);
            host.image_handles.insert(1, RgbaFrame::from_rgba8(3, 2, first.clone())?);
            host.image_handles.insert(2, RgbaFrame::from_rgba8(3, 2, second.clone())?);
            host.image_handles.insert(3, RgbaFrame::from_rgba8(2, 1, vec![0, 0, 0, 255, 0, 0, 128, 255])?);
            let mut task = Transition {
                kind: 26, words: vec![100, 1, 0, 0, 2, 0, 0, 3, 4, 8, 0, 0, 1, 1],
                duration: 100, started: 0, speed: 1., active: true,
                size: [3, 2], pixels: first, output: Point::new(4, 8),
                local: [1, 1], submitted: false, rendered: None, rendered_sample: None,
            };

            host.sample_transition(&mut task, 50, false, true)?;
            let (at, frame) = task.rendered.as_ref().unwrap();
            assert_eq!(*at, Point::new(5, 9));
            assert_eq!(frame.dimensions(), (2, 1));
            assert_eq!(frame.pixels(), [80, 100, 120, 200, 50, 70, 90, 132]);
            assert!(task.active);

            host.sample_transition(&mut task, 100, false, true)?;
            let (at, frame) = task.rendered.as_ref().unwrap();
            assert_eq!(*at, Point::new(4, 8));
            assert_eq!(frame.dimensions(), (3, 2));
            assert_eq!(frame.pixels(), second.as_slice());
            assert_eq!(task.local, [0, 0]);
            assert!(!task.active);
            Ok(())
        })
    }
}
