//! Preserve each transition's work surface and owned message queue. Preparing
//! never prepares sources, samples a transition, submits pixels or reads a clock.
use super::*;
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_runtime::object_graph::{self as wire, ImageFrameSnapshot};
use crate::image_manager::{capture_frame, restore_frame};

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Task {
    words: Vec<u32>, duration: u32, started: u32, speed: u32, active: bool,
    size: [u32; 2], pixels: Vec<u8>, output: [i32; 2], local: [u32; 2],
    submitted: bool, rendered: Option<([i32; 2], ImageFrameSnapshot)>,
}
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot {
    version: u32, tasks: BTreeMap<u32, Task>,
    queued: BTreeMap<u32, VecDeque<(u32, Vec<u8>)>>,
}

impl TransitionStore {
    pub(crate) fn capture_checkpoint(&self, kinds: &BTreeMap<u32, u32>,
        images: &BTreeMap<u32, RgbaFrame>) -> Result<Vec<u8>> {
        self.validate_checkpoint(kinds, images)?;
        Ok(wire::encode_component(&Snapshot { version: 1,
            tasks: self.tasks.iter().map(|(&id, t)| (id, Task {
                words: t.words.clone(), duration: t.duration, started: t.started,
                speed: t.speed.to_bits(), active: t.active, size: t.size,
                pixels: t.pixels.clone(), output: [t.output.x, t.output.y], local: t.local,
                submitted: t.submitted, rendered: t.rendered.as_ref()
                    .map(|(at, frame)| ([at.x, at.y], capture_frame(frame))),
            })).collect(), queued: self.queued.clone(),
        })?)
    }

    pub(crate) fn prepare_checkpoint(bytes: &[u8], kinds: &BTreeMap<u32, u32>,
        images: &BTreeMap<u32, RgbaFrame>) -> Result<Self> {
        let saved: Snapshot = wire::decode_component(bytes)?;
        if saved.version != 1 || saved.tasks.len() > 1_000_000 || saved.queued.len() > 1_000_000 {
            bail!("unsupported transition checkpoint version or task count");
        }
        let store = Self { tasks: saved.tasks.into_iter().map(|(id, t)| Ok((id, Transition {
            kind: *kinds.get(&id).context("transition checkpoint has no original task")?,
            words: t.words, duration: t.duration, started: t.started,
            speed: f32::from_bits(t.speed), active: t.active, size: t.size, pixels: t.pixels,
            output: Point::new(t.output[0], t.output[1]), local: t.local, submitted: t.submitted,
            rendered_sample: None,
            rendered: t.rendered.map(|(at, frame)| Ok::<_, anyhow::Error>((
                Point::new(at[0], at[1]), restore_frame(&frame)?))).transpose()?,
        }))).collect::<Result<_>>()?, queued: saved.queued };
        store.validate_checkpoint(kinds, images)?;
        Ok(store)
    }

    fn validate_checkpoint(&self, kinds: &BTreeMap<u32, u32>,
        images: &BTreeMap<u32, RgbaFrame>) -> Result<()> {
        if self.tasks.len() > 1_000_000 || self.queued.len() > 1_000_000 {
            bail!("transition checkpoint exceeds task bounds");
        }
        let mut bytes = 0usize;
        for (&id, task) in &self.tasks {
            if kinds.get(&id) != Some(&task.kind) || !matches!(task.kind, 26 | 27)
                || task.words.len() * 4 != record_size(task.kind)
                || task.duration != (task.words[0] as i32).max(1) as u32
                || ![1.0f32, 1.5, 2.0, 3.0, 4.0].contains(&task.speed)
                || task.submitted && task.rendered.is_none() {
                bail!("transition checkpoint has no matching fade or wipe receiver/work state");
            }
            let output_index = if task.kind == 26 {
                let mask = images.get(&task.words[7]).context("wipe checkpoint mask wrapper was released")?;
                validate_local(task, mask)?;
                if task.local != [0; 2] && task.local != [task.words[12], task.words[13]] {
                    bail!("wipe checkpoint local origin differs from its work record");
                }
                8
            } else {
                if task.local != [0; 2] || task.words[9..11].iter().any(|word| (*word as i32) < 0)
                    || task.size[0] > task.words[9] || task.size[1] > task.words[10] {
                    bail!("fade checkpoint work region differs from its retained record");
                }
                7
            };
            if task.output != Point::new(task.words[output_index] as i32, task.words[output_index + 1] as i32) {
                bail!("transition checkpoint destination differs from its retained record");
            }
            let expected = (task.size[0] as usize).checked_mul(task.size[1] as usize)
                .and_then(|n| n.checked_mul(4)).context("transition checkpoint pixel count overflow")?;
            if expected != task.pixels.len() || expected > wire::MAX_GRAPH_BYTES {
                bail!("transition checkpoint work pixels differ from its exact dimensions");
            }
            for (handle, origin) in [(task.words[1], [task.words[2], task.words[3]]),
                (task.words[4], [task.words[5], task.words[6]])] {
                let frame = images.get(&handle).context("transition checkpoint source wrapper was released")?;
                validate_region(frame, origin, task.size)?;
            }
            bytes = bytes.checked_add(expected).context("transition checkpoint pixel accounting overflow")?;
            if let Some((at, frame)) = &task.rendered {
                let full = *at == task.output && frame.dimensions() == (task.size[0], task.size[1]);
                let mut partial = false;
                if task.kind == 26 {
                    let mask = images.get(&task.words[7]).context("wipe checkpoint mask wrapper was released")?;
                    let [x, y] = [task.words[12], task.words[13]];
                    let region = (task.size[0].saturating_sub(x).min(mask.width()),
                        task.size[1].saturating_sub(y).min(mask.height()));
                    partial = *at == Point::new(task.output.x.wrapping_add(x as i32), task.output.y.wrapping_add(y as i32))
                        && frame.dimensions() == region;
                }
                if !full && !partial {
                    bail!("transition checkpoint rendered frame differs from its actual output dimensions");
                }
                // A failed canvas copy can leave the last submission older
                // than the newly sampled work pixels. Preserve both buffers.
                bytes = bytes.checked_add(frame.pixels().len()).context("transition checkpoint byte overflow")?;
            }
        }
        for (&id, queue) in &self.queued {
            let kind = kinds.get(&id).copied().filter(|kind| matches!(kind, 26 | 27))
                .context("transition checkpoint queue lost its fade or wipe receiver")?;
            if queue.len() > 4096 { bail!("transition checkpoint command queue exceeds 4096 entries"); }
            for (code, record) in queue {
                let size = match code { 263..=267 if *code != 266 => 0,
                    266 => 20, 280 | 281 => record_size(kind),
                    _ => bail!("transition checkpoint contains an unknown queued command"), };
                if record.len() != size { bail!("transition checkpoint queued owned record is truncated"); }
                bytes = bytes.checked_add(record.len()).context("transition checkpoint queue byte overflow")?;
            }
        }
        if bytes > wire::MAX_GRAPH_BYTES { bail!("transition checkpoint exceeds graph byte limit"); }
        Ok(())
    }

    pub(crate) fn validate_draw_checkpoint(&self,
        links: impl Iterator<Item = (u32, u32, u32)>, kinds: &BTreeMap<u32, u32>,
        parents: &BTreeMap<u32, u32>) -> Result<()> {
        for (owner, id, kind) in links {
            if kinds.get(&owner) != Some(&34) || kinds.get(&id) != Some(&kind)
                || parents.get(&id) != Some(&owner) {
                bail!("DRAW checkpoint native transition lost its original FES parent");
            }
            // The native receiver is independently mutable. A later raw
            // prepare/replace/stop may differ from DRAW's retained initial
            // record, or clear its task. Preserve both states, not a replay.
        }
        Ok(())
    }
}
