//! Portable ANM execution state. Restoring never steps instructions or RNG.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageAnimationSnapshot {
    pub version: u32,
    pub timeline: Option<TimelineSnapshot>,
    pub display: [i32; 5],
    pub playing: i32,
    pub reset_time_on_update: bool,
    pub time: u32,
    pub frame_change: Option<i32>,
    pub pending_reset_frame_alias: bool,
    pub force_next_update: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimelineSnapshot {
    pub variables: Vec<i32>,
    pub instruction_pointer: usize,
    pub display: [i32; 5],
    pub max_image_id: i32,
    /// Opcode followed by eight (kind, value) pairs, including unused words.
    pub instructions: Vec<[u32; 17]>,
    pub finished: bool,
    pub speed_bits: u32,
    pub origin: u32,
    pub duration: i32,
    pub pending_remaining: Option<i32>,
    pub display_before_round: [i32; 5],
}

fn words(d: DisplayState) -> [i32; 5] { [d.frame_id, d.blend, d.visible, d.x, d.y] }
fn display(d: [i32; 5]) -> DisplayState {
    DisplayState { frame_id: d[0], blend: d[1], visible: d[2], x: d[3], y: d[4] }
}

impl ImageAnimation {
    pub fn execution_snapshot(&self) -> ImageAnimationSnapshot {
        ImageAnimationSnapshot {
            version: 1,
            timeline: self.timeline.as_ref().map(|t| TimelineSnapshot {
                variables: t.interpreter.variables.to_vec(),
                instruction_pointer: t.interpreter.instruction_pointer,
                display: words(t.interpreter.display), max_image_id: t.interpreter.max_image_id,
                instructions: t.interpreter.instructions.iter().map(|i| {
                    let mut row = [0; 17]; row[0] = i.opcode;
                    for (n, o) in i.operands.iter().enumerate() {
                        row[1 + n * 2] = o.kind; row[2 + n * 2] = o.value;
                    }
                    row
                }).collect(),
                finished: t.interpreter.finished, speed_bits: t.speed.to_bits(),
                origin: t.origin, duration: t.duration,
                pending_remaining: t.pending.map(|p| p.remaining),
                display_before_round: words(t.display_before_round),
            }),
            display: words(self.display_cache), playing: self.playing,
            reset_time_on_update: self.reset_time_on_update, time: self.time,
            frame_change: self.frame_change, pending_reset_frame_alias: self.pending_reset_frame_alias,
            force_next_update: self.force_next_update,
        }
    }

    pub fn from_execution_snapshot(s: &ImageAnimationSnapshot) -> AnmResult<Self> {
        if s.version != 1 { return Err(AnmError::InvalidSnapshot("version")); }
        let timeline = s.timeline.as_ref().map(|t| -> AnmResult<Timeline> {
            if t.variables.len() != VARIABLE_COUNT || t.instructions.len() > 1_000_000
                || t.instruction_pointer > t.instructions.len()
            { return Err(AnmError::InvalidSnapshot("variables, program size or instruction pointer")); }
            // Operands are data: retain malformed/unreached instructions so
            // execution raises the same error when they are actually reached.
            Ok(Timeline {
                interpreter: AnmInterpreter {
                    variables: std::array::from_fn(|i| t.variables[i]),
                    instruction_pointer: t.instruction_pointer, display: display(t.display),
                    max_image_id: t.max_image_id,
                    instructions: t.instructions.iter().map(|row| AnmInstruction {
                        opcode: row[0], operands: std::array::from_fn(|i| AnmOperand {
                            kind: row[1 + i * 2], value: row[2 + i * 2],
                        }),
                    }).collect(), finished: t.finished,
                },
                speed: f32::from_bits(t.speed_bits), origin: t.origin, duration: t.duration,
                pending: t.pending_remaining.map(|remaining| PendingRound { remaining }),
                display_before_round: display(t.display_before_round),
            })
        }).transpose()?;
        Ok(Self {
            timeline, display_cache: display(s.display), playing: s.playing,
            reset_time_on_update: s.reset_time_on_update, time: s.time,
            frame_change: s.frame_change, pending_reset_frame_alias: s.pending_reset_frame_alias,
            force_next_update: s.force_next_update,
        })
    }
}
