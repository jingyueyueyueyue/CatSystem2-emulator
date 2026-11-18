use cs2_formats::{AnmFile, AnmInstruction, AnmOperand};
use thiserror::Error;

pub type AnmResult<T> = std::result::Result<T, AnmError>;

const VARIABLE_COUNT: usize = 64;

#[path = "anm_snapshot.rs"]
pub mod snapshot;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AnmError {
    #[error("invalid ANM execution snapshot: {0}")]
    InvalidSnapshot(&'static str),
    #[error("unsupported ANM opcode {opcode} at instruction {instruction}")]
    InvalidOpcode { instruction: usize, opcode: u32 },
    #[error("invalid ANM operand kind {kind} at instruction {instruction}, operand {operand}")]
    InvalidOperandKind {
        instruction: usize,
        operand: usize,
        kind: u32,
    },
    #[error("ANM variable index {index} is outside 0..{VARIABLE_COUNT}")]
    VariableOutOfBounds { index: i32 },
    #[error(
        "ANM jump target {target} at instruction {instruction} is outside 0..={instruction_count}"
    )]
    JumpOutOfBounds {
        instruction: usize,
        target: u32,
        instruction_count: usize,
    },
    #[error("ANM random range requires an injected random source")]
    RandomSourceRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayState {
    pub frame_id: i32,
    pub blend: i32,
    pub visible: i32,
    pub x: i32,
    pub y: i32,
}

impl Default for DisplayState {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl DisplayState {
    pub const DEFAULT: Self = Self {
        frame_id: 0,
        blend: 255,
        visible: 1,
        x: 0,
        y: 0,
    };

    pub fn compose(self, position: (f32, f32), blend: i32, visible: i32) -> Self {
        let image_x = float_to_i32(position.0);
        let image_y = float_to_i32(position.1);
        let product = (self.blend as u32).wrapping_mul(blend as u32);
        let composed_blend = if product < 0xff00 {
            (product / 255) as i32
        } else {
            255
        };
        Self {
            frame_id: self.frame_id,
            blend: composed_blend,
            visible: if visible == 0 { 0 } else { self.visible },
            x: image_x.wrapping_add(self.x),
            y: image_y.wrapping_add(self.y),
        }
    }
}

fn float_to_i32(value: f32) -> i32 {
    if !value.is_finite() || value < i32::MIN as f32 || value >= 2_147_483_648.0 {
        i32::MIN
    } else {
        value.trunc() as i32
    }
}

pub trait RandomSource {
    fn next_u32(&mut self) -> u32;
}

impl<F> RandomSource for F
where
    F: FnMut() -> u32,
{
    fn next_u32(&mut self) -> u32 {
        self()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnmInterpreter {
    pub variables: [i32; VARIABLE_COUNT],
    pub instruction_pointer: usize,
    pub display: DisplayState,
    pub max_image_id: i32,
    instructions: Vec<AnmInstruction>,
    finished: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepOutcome {
    Continue,
    Timed(i32),
    End,
}

impl AnmInterpreter {
    pub fn new(file: AnmFile, max_image_id: u32) -> Self {
        Self::from_instructions(file.instructions, max_image_id)
    }

    pub fn from_instructions(instructions: Vec<AnmInstruction>, max_image_id: u32) -> Self {
        let finished = instructions.is_empty();
        Self {
            variables: [0; VARIABLE_COUNT],
            instruction_pointer: 0,
            display: DisplayState::DEFAULT,
            max_image_id: max_image_id as i32,
            instructions,
            finished,
        }
    }

    pub fn instructions(&self) -> &[AnmInstruction] {
        &self.instructions
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn restart(&mut self) {
        self.instruction_pointer = 0;
        self.display = DisplayState::DEFAULT;
        self.finished = self.instructions.is_empty();
    }

    fn value(&self, instruction: usize, index: usize, operand: AnmOperand) -> AnmResult<i32> {
        match operand.kind {
            0 | 2 => Ok(operand.value as i32),
            1 => {
                let variable = operand.value as usize;
                self.variables
                    .get(variable)
                    .copied()
                    .ok_or(AnmError::VariableOutOfBounds {
                        index: operand.value as i32,
                    })
            }
            kind => Err(AnmError::InvalidOperandKind {
                instruction,
                operand: index,
                kind,
            }),
        }
    }

    fn variable_index(&self, value: i32) -> AnmResult<usize> {
        if !(0..VARIABLE_COUNT as i32).contains(&value) {
            return Err(AnmError::VariableOutOfBounds { index: value });
        }
        Ok(value as usize)
    }

    fn jump_target(&self, instruction: usize, target: i32) -> AnmResult<usize> {
        let raw_target = target as u32;
        let target_index = raw_target as usize;
        if target < 0 || target_index > self.instructions.len() {
            return Err(AnmError::JumpOutOfBounds {
                instruction,
                target: raw_target,
                instruction_count: self.instructions.len(),
            });
        }
        Ok(target_index)
    }

    fn duration(min: i32, max: i32, random: &mut Option<&mut dyn RandomSource>) -> AnmResult<i32> {
        if min == max {
            return Ok(min);
        }
        let source = random.as_mut().ok_or(AnmError::RandomSourceRequired)?;
        let span = (max as u32).wrapping_sub(min as u32);
        let offset = (**source).next_u32() % span;
        Ok((min as u32).wrapping_add(offset) as i32)
    }

    fn step(&mut self, random: &mut Option<&mut dyn RandomSource>) -> AnmResult<StepOutcome> {
        let pc = self.instruction_pointer;
        let Some(instruction) = self.instructions.get(pc).cloned() else {
            self.finished = true;
            return Ok(StepOutcome::End);
        };
        let op = |index: usize| instruction.operands[index];
        let val = |this: &Self, index: usize| this.value(pc, index, op(index));
        let next = pc + 1;
        let mut branch = None;
        let outcome = match instruction.opcode {
            0 => {
                let frame_id = val(self, 0)?;
                let min = val(self, 1)?;
                let max = val(self, 2)?;
                let duration = Self::duration(min, max, random)?;
                self.display.frame_id = frame_id;
                StepOutcome::Timed(duration)
            }
            1 => {
                let variable = self.variable_index(val(self, 0)?)?;
                let min = val(self, 1)?;
                let max = val(self, 2)?;
                let value = Self::duration(min, max, random)?;
                self.variables[variable] = value;
                StepOutcome::Continue
            }
            2 => {
                let variable = self.variable_index(val(self, 0)?)?;
                if self.variables[variable] > 0 {
                    self.variables[variable] = self.variables[variable].wrapping_sub(1);
                } else {
                    branch = Some(self.jump_target(pc, val(self, 1)?)?);
                }
                StepOutcome::Continue
            }
            3 => {
                branch = Some(self.jump_target(pc, val(self, 0)?)?);
                StepOutcome::Continue
            }
            4..=10 => {
                let left = val(self, 0)?;
                let right = if instruction.opcode == 4 {
                    0
                } else {
                    val(self, 1)?
                };
                let target_operand = if instruction.opcode == 4 { 1 } else { 2 };
                let take = match instruction.opcode {
                    4 => left > 0,
                    5 => left == right,
                    6 => left != right,
                    7 => left > right,
                    8 => left < right,
                    9 => left >= right,
                    10 => left <= right,
                    _ => unreachable!(),
                };
                if take {
                    branch = Some(self.jump_target(pc, val(self, target_operand)?)?);
                }
                StepOutcome::Continue
            }
            11 => {
                let variable = self.variable_index(val(self, 0)?)?;
                self.variables[variable] = self.max_image_id;
                StepOutcome::Continue
            }
            12 => {
                self.display.blend = val(self, 0)?;
                StepOutcome::Continue
            }
            13 => {
                self.display.visible = val(self, 0)?;
                StepOutcome::Continue
            }
            14 => {
                let x = val(self, 0)?;
                let y = val(self, 1)?;
                self.display.x = x;
                self.display.y = y;
                StepOutcome::Continue
            }
            15 => {
                let min = val(self, 0)?;
                let max = val(self, 1)?;
                StepOutcome::Timed(Self::duration(min, max, random)?)
            }
            16 | 17 => {
                let variable = self.variable_index(val(self, 0)?)?;
                let value = val(self, 1)?;
                self.variables[variable] = if instruction.opcode == 16 {
                    self.variables[variable].wrapping_add(value)
                } else {
                    self.variables[variable].wrapping_sub(value)
                };
                StepOutcome::Continue
            }
            opcode => {
                return Err(AnmError::InvalidOpcode {
                    instruction: pc,
                    opcode,
                });
            }
        };
        self.instruction_pointer = branch.unwrap_or(next);
        if self.instruction_pointer >= self.instructions.len() {
            self.finished = false;
        }
        Ok(outcome)
    }
}

#[derive(Clone, Copy, Debug)]
struct PendingRound {
    remaining: i32,
}

#[derive(Clone, Debug)]
pub struct Timeline {
    pub interpreter: AnmInterpreter,
    pub speed: f32,
    pub origin: u32,
    pub duration: i32,
    pending: Option<PendingRound>,
    display_before_round: DisplayState,
}

impl Timeline {
    pub fn new(file: AnmFile, max_image_id: u32) -> Self {
        Self::from_instructions(file.instructions, max_image_id)
    }

    pub fn from_instructions(instructions: Vec<AnmInstruction>, max_image_id: u32) -> Self {
        Self {
            interpreter: AnmInterpreter::from_instructions(instructions, max_image_id),
            speed: 1.0,
            origin: 0,
            duration: 0,
            pending: None,
            display_before_round: DisplayState::DEFAULT,
        }
    }

    pub fn display(&self) -> DisplayState {
        self.interpreter.display
    }

    pub fn display_changed(&self) -> bool {
        self.display_before_round != self.interpreter.display
    }

    pub fn is_finished(&self) -> bool {
        self.interpreter.finished && self.pending.is_none()
    }

    pub fn has_pending_update(&self) -> bool {
        self.pending.is_some()
    }

    pub fn restart(&mut self) {
        self.interpreter.restart();
        self.duration = 0;
        self.pending = None;
        self.display_before_round = self.interpreter.display;
    }

    pub fn update(&mut self, now: u32, budget: usize) -> AnmResult<bool> {
        self.update_inner(now, budget, None)
    }

    pub fn update_with_random(
        &mut self,
        now: u32,
        budget: usize,
        random: &mut dyn RandomSource,
    ) -> AnmResult<bool> {
        self.update_inner(now, budget, Some(random))
    }

    pub fn update_frame(&mut self, now: u32, budget: usize) -> AnmResult<bool> {
        self.update_frame_inner(now, budget, None)
    }

    pub fn update_frame_with_random(
        &mut self,
        now: u32,
        budget: usize,
        random: &mut dyn RandomSource,
    ) -> AnmResult<bool> {
        self.update_frame_inner(now, budget, Some(random))
    }

    fn update_frame_inner(
        &mut self,
        now: u32,
        budget: usize,
        random: Option<&mut dyn RandomSource>,
    ) -> AnmResult<bool> {
        if self.pending.is_none() {
            let now = if now == u32::MAX { self.origin } else { now };
            if self.origin == 0 {
                self.origin = now;
            }
            self.update_inner(now, budget, random)
        } else {
            self.update_inner(now, budget, random)
        }
    }

    fn update_inner(
        &mut self,
        now: u32,
        budget: usize,
        mut random: Option<&mut dyn RandomSource>,
    ) -> AnmResult<bool> {
        if self.pending.is_none() {
            self.display_before_round = self.interpreter.display;
            if self.interpreter.finished {
                return Ok(true);
            }
            let raw_delta = now.wrapping_sub(self.origin) as i32;
            let elapsed = scaled_elapsed(raw_delta, self.speed);
            if elapsed < self.duration {
                return Ok(true);
            }
            self.pending = Some(PendingRound { remaining: elapsed });
        }

        let mut remaining_budget = budget;
        loop {
            if self.interpreter.instruction_pointer >= self.interpreter.instructions.len() {
                let pending = self.pending.as_mut().expect("pending update exists");
                if pending.remaining >= self.duration {
                    pending.remaining = pending.remaining.wrapping_sub(self.duration);
                    self.origin = self.origin.wrapping_add(self.duration as u32);
                    self.interpreter.finished = true;
                    self.pending = None;
                    return Ok(true);
                }
                self.pending = None;
                return Ok(true);
            }
            if remaining_budget == 0 {
                return Ok(false);
            }
            let outcome = self.interpreter.step(&mut random)?;
            remaining_budget -= 1;
            if let StepOutcome::Timed(new_duration) = outcome {
                let pending = self.pending.as_mut().expect("pending update exists");
                pending.remaining = pending.remaining.wrapping_sub(self.duration);
                self.origin = self.origin.wrapping_add(self.duration as u32);
                self.duration = new_duration;
                if pending.remaining < self.duration {
                    self.pending = None;
                    return Ok(true);
                }
            } else if outcome == StepOutcome::End {
                self.pending = None;
                return Ok(true);
            }
        }
    }
}

fn scaled_elapsed(raw_delta: i32, speed: f32) -> i32 {
    let scaled = raw_delta as f32 * speed;
    if !scaled.is_finite() || scaled < i32::MIN as f32 || scaled >= 2_147_483_648.0 {
        i32::MIN
    } else {
        scaled.trunc() as i32
    }
}

#[derive(Clone, Debug)]
pub struct ImageAnimation {
    timeline: Option<Timeline>,
    display_cache: DisplayState,
    pub playing: i32,
    pub reset_time_on_update: bool,
    pub time: u32,
    frame_change: Option<i32>,
    pending_reset_frame_alias: bool,
    force_next_update: bool,
}

impl Default for ImageAnimation {
    fn default() -> Self {
        Self {
            timeline: None,
            display_cache: DisplayState::DEFAULT,
            playing: 0,
            reset_time_on_update: false,
            time: 0,
            frame_change: None,
            pending_reset_frame_alias: false,
            force_next_update: false,
        }
    }
}

impl ImageAnimation {
    pub fn timeline(&self) -> Option<&Timeline> {
        self.timeline.as_ref()
    }

    pub fn timeline_mut(&mut self) -> Option<&mut Timeline> {
        self.timeline.as_mut()
    }

    pub fn display(&self) -> DisplayState {
        self.display_cache
    }

    pub fn frame_change(&mut self) -> Option<i32> {
        self.frame_change.take()
    }

    pub fn load(&mut self, bytes: &[u8], max_image_id: u32) -> Result<(), cs2_formats::AnmError> {
        self.timeline = None;
        self.frame_change = None;
        self.pending_reset_frame_alias = false;
        let file = AnmFile::parse(bytes)?;
        self.timeline = Some(Timeline::new(file, max_image_id));
        self.display_cache = DisplayState::DEFAULT;
        Ok(())
    }

    pub fn update(&mut self, now: u32, budget: usize, has_reset_frame: bool) -> AnmResult<bool> {
        self.update_inner(now, budget, has_reset_frame, None)
    }

    pub fn update_with_random(
        &mut self,
        now: u32,
        budget: usize,
        has_reset_frame: bool,
        random: &mut dyn RandomSource,
    ) -> AnmResult<bool> {
        self.update_inner(now, budget, has_reset_frame, Some(random))
    }

    fn update_inner(
        &mut self,
        now: u32,
        budget: usize,
        has_reset_frame: bool,
        random: Option<&mut dyn RandomSource>,
    ) -> AnmResult<bool> {
        let Some(timeline) = self.timeline.as_mut() else {
            self.frame_change = None;
            return Ok(true);
        };
        if !timeline.has_pending_update() && self.playing == 0 && !self.force_next_update {
            self.frame_change = None;
            return Ok(true);
        }
        let is_new_round = !timeline.has_pending_update();
        if is_new_round {
            self.frame_change = None;
            self.pending_reset_frame_alias = now == u32::MAX && has_reset_frame;
            if self.reset_time_on_update {
                let effective_now = if now == u32::MAX {
                    timeline.origin
                } else {
                    now
                };
                timeline.origin = effective_now;
                self.time = effective_now;
                self.reset_time_on_update = false;
            }
        }
        self.force_next_update = false;
        let completed = match random {
            Some(random) => timeline.update_frame_with_random(now, budget, random)?,
            None => timeline.update_frame(now, budget)?,
        };
        if completed {
            if timeline.display_changed() {
                let mut display = timeline.display();
                if self.pending_reset_frame_alias {
                    // Frame 10000 is the reserved reset image, independently
                    // of the maximum frame ID used by the `max` instruction.
                    display.frame_id = 10_000;
                }
                self.display_cache = display;
                self.frame_change = Some(display.frame_id);
            }
            self.pending_reset_frame_alias = false;
        }
        Ok(completed)
    }

    pub fn restart(&mut self, budget: usize, has_reset_frame: bool) -> AnmResult<bool> {
        self.restart_inner(budget, has_reset_frame, None)
    }

    pub fn restart_with_random(
        &mut self,
        budget: usize,
        has_reset_frame: bool,
        random: &mut dyn RandomSource,
    ) -> AnmResult<bool> {
        self.restart_inner(budget, has_reset_frame, Some(random))
    }

    fn restart_inner(
        &mut self,
        budget: usize,
        has_reset_frame: bool,
        random: Option<&mut dyn RandomSource>,
    ) -> AnmResult<bool> {
        let Some(timeline) = self.timeline.as_mut() else {
            self.frame_change = None;
            return Ok(true);
        };
        timeline.restart();
        self.frame_change = None;
        self.pending_reset_frame_alias = has_reset_frame;
        self.force_next_update = true;
        match random {
            Some(random) => self.update_inner(u32::MAX, budget, has_reset_frame, Some(random)),
            None => self.update_inner(u32::MAX, budget, has_reset_frame, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instruction(opcode: u32, operands: &[(u32, u32)]) -> AnmInstruction {
        let mut slots = [AnmOperand::default(); 8];
        for (slot, &(kind, value)) in slots.iter_mut().zip(operands) {
            *slot = AnmOperand { kind, value };
        }
        AnmInstruction {
            opcode,
            operands: slots,
        }
    }

    fn imm(value: i32) -> (u32, u32) {
        (0, value as u32)
    }

    fn var(index: u32) -> (u32, u32) {
        (1, index)
    }

    fn timeline(instructions: Vec<AnmInstruction>) -> Timeline {
        Timeline::from_instructions(instructions, 100)
    }

    #[test]
    fn all_confirmed_opcodes_execute_with_expected_state() {
        let instructions = vec![
            instruction(1, &[imm(0), imm(2), imm(2)]),
            instruction(16, &[imm(0), imm(3)]),
            instruction(17, &[imm(0), imm(1)]),
            instruction(11, &[imm(1)]),
            instruction(12, &[imm(300)]),
            instruction(13, &[imm(-2)]),
            instruction(14, &[imm(-4), imm(7)]),
            instruction(4, &[imm(1), imm(8)]),
            instruction(1, &[imm(2), imm(99), imm(99)]),
            instruction(5, &[var(0), imm(3), imm(10)]),
            instruction(1, &[imm(3), imm(99), imm(99)]),
            instruction(6, &[imm(0), var(0), imm(12)]),
            instruction(1, &[imm(4), imm(99), imm(99)]),
            instruction(7, &[var(0), imm(2), imm(14)]),
            instruction(1, &[imm(5), imm(99), imm(99)]),
            instruction(8, &[imm(1), imm(2), imm(16)]),
            instruction(1, &[imm(6), imm(99), imm(99)]),
            instruction(9, &[var(0), imm(3), imm(18)]),
            instruction(1, &[imm(7), imm(99), imm(99)]),
            instruction(10, &[var(0), imm(5), imm(20)]),
            instruction(1, &[imm(8), imm(99), imm(99)]),
            instruction(0, &[imm(42), imm(5), imm(5)]),
            instruction(15, &[imm(2), imm(2)]),
        ];
        let mut vm = AnmInterpreter::from_instructions(instructions, 0xffff_fffe);
        for _ in 0..7 {
            assert_eq!(vm.step(&mut None).unwrap(), StepOutcome::Continue);
        }
        for expected_pc in 8..=20 {
            let before = vm.instruction_pointer;
            vm.step(&mut None).unwrap();
            assert_eq!(vm.instruction_pointer, expected_pc, "from {before}");
        }
        vm.step(&mut None).unwrap();
        assert_eq!(vm.variables[0], 4);
        assert_eq!(vm.variables[1], -2);
        assert_eq!(vm.variables[2..9], [99; 7]);
        assert_eq!(vm.display.blend, 300);
        assert_eq!(vm.display.visible, -2);
        assert_eq!((vm.display.x, vm.display.y), (-4, 7));
        assert_eq!(vm.step(&mut None).unwrap(), StepOutcome::Timed(5));
        assert_eq!(vm.display.frame_id, 42);
        assert_eq!(vm.step(&mut None).unwrap(), StepOutcome::Timed(2));
    }

    #[test]
    fn loop_decrements_then_jumps_only_at_zero_or_below() {
        let mut vm = AnmInterpreter::from_instructions(
            vec![instruction(2, &[imm(0), imm(2)]), instruction(3, &[imm(2)])],
            0,
        );
        vm.variables[0] = 2;
        vm.step(&mut None).unwrap();
        assert_eq!(vm.variables[0], 1);
        assert_eq!(vm.instruction_pointer, 1);
        vm.step(&mut None).unwrap();
        assert_eq!(vm.instruction_pointer, 2);
        vm.instruction_pointer = 0;
        vm.variables[0] = 0;
        vm.step(&mut None).unwrap();
        assert_eq!(vm.variables[0], 0);
        assert_eq!(vm.instruction_pointer, 2);
    }

    #[test]
    fn variable_arithmetic_wraps_at_signed_32_bit_boundaries() {
        let mut vm = AnmInterpreter::from_instructions(
            vec![
                instruction(16, &[imm(0), imm(1)]),
                instruction(17, &[imm(1), imm(1)]),
            ],
            0,
        );
        vm.variables[0] = i32::MAX;
        vm.variables[1] = i32::MIN;
        vm.step(&mut None).unwrap();
        vm.step(&mut None).unwrap();
        assert_eq!(vm.variables[0], i32::MIN);
        assert_eq!(vm.variables[1], i32::MAX);
    }

    #[test]
    fn jumps_and_unknown_values_return_structured_errors() {
        let mut bad_jump = AnmInterpreter::from_instructions(vec![instruction(3, &[imm(4)])], 0);
        assert_eq!(
            bad_jump.step(&mut None),
            Err(AnmError::JumpOutOfBounds {
                instruction: 0,
                target: 4,
                instruction_count: 1
            })
        );
        let mut bad_opcode = AnmInterpreter::from_instructions(vec![instruction(18, &[])], 0);
        assert_eq!(
            bad_opcode.step(&mut None),
            Err(AnmError::InvalidOpcode {
                instruction: 0,
                opcode: 18
            })
        );
        let mut bad_kind = AnmInterpreter::from_instructions(vec![instruction(12, &[(3, 7)])], 0);
        assert_eq!(
            bad_kind.step(&mut None),
            Err(AnmError::InvalidOperandKind {
                instruction: 0,
                operand: 0,
                kind: 3
            })
        );
        let mut bad_variable =
            AnmInterpreter::from_instructions(vec![instruction(11, &[imm(64)])], 0);
        assert_eq!(
            bad_variable.step(&mut None),
            Err(AnmError::VariableOutOfBounds { index: 64 })
        );
    }

    #[test]
    fn random_ranges_use_wrapping_unsigned_span_and_skip_equal_endpoints() {
        let calls = std::cell::Cell::new(0);
        let mut random = || {
            calls.set(calls.get() + 1);
            7
        };
        let mut vm = AnmInterpreter::from_instructions(
            vec![
                instruction(1, &[imm(0), imm(4), imm(4)]),
                instruction(1, &[imm(1), imm(-2), imm(2)]),
                instruction(15, &[imm(8), imm(3)]),
            ],
            0,
        );
        vm.step(&mut Some(&mut random)).unwrap();
        assert_eq!(calls.get(), 0);
        vm.step(&mut Some(&mut random)).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(vm.variables[1], 1);
        assert_eq!(
            vm.step(&mut Some(&mut random)).unwrap(),
            StepOutcome::Timed(8 + (7u32 % 0xffff_fffb) as i32)
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn random_range_without_source_reports_error_and_can_resume_with_injected_source() {
        let mut timeline = timeline(vec![instruction(1, &[imm(0), imm(2), imm(5)])]);
        assert_eq!(timeline.update(0, 1), Err(AnmError::RandomSourceRequired));
        assert!(timeline.has_pending_update());
        let calls = std::cell::Cell::new(0);
        let mut random = || {
            calls.set(calls.get() + 1);
            2
        };
        assert!(timeline.update_with_random(100, 1, &mut random).unwrap());
        assert_eq!(timeline.interpreter.variables[0], 4);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_yielded_update_resumes_without_restarting_time_or_consuming_random_twice() {
        let mut timeline = timeline(vec![
            instruction(1, &[imm(0), imm(10), imm(20)]),
            instruction(12, &[imm(80)]),
            instruction(0, &[imm(7), imm(5), imm(5)]),
        ]);
        timeline.duration = 1;
        timeline.origin = 100;
        let mut calls = 0;
        let mut random = || {
            calls += 1;
            3
        };
        assert!(!timeline.update_with_random(120, 1, &mut random).unwrap());
        assert_eq!(calls, 1);
        assert_eq!(timeline.interpreter.variables[0], 13);
        assert_eq!(timeline.interpreter.instruction_pointer, 1);
        assert!(!timeline.update(999, 1).unwrap());
        assert_eq!(timeline.interpreter.instruction_pointer, 2);
        assert!(timeline.update(999, 1).unwrap());
        assert_eq!(calls, 1);
        assert_eq!(
            timeline.display(),
            DisplayState {
                frame_id: 7,
                blend: 80,
                ..DisplayState::DEFAULT
            }
        );
        assert!(timeline.display_changed());
    }

    #[test]
    fn timeline_handles_wrapping_clocks_scaled_time_and_restart_retention() {
        let mut timeline = timeline(vec![
            instruction(12, &[imm(33)]),
            instruction(0, &[imm(5), imm(2), imm(2)]),
        ]);
        timeline.origin = u32::MAX - 1;
        timeline.speed = 0.5;
        assert!(!timeline.update(u32::MAX, 1).unwrap());
        assert_eq!(timeline.display().blend, 33);
        assert!(timeline.update(2, 1).unwrap());
        assert_eq!(timeline.duration, 2);
        assert_eq!(timeline.display().frame_id, 5);
        assert!(timeline.update(2, 16).unwrap());
        assert!(timeline.is_finished());
        timeline.interpreter.variables[3] = -9;
        timeline.speed = 2.0;
        timeline.restart();
        assert_eq!(timeline.origin, 0);
        assert_eq!(timeline.speed, 2.0);
        assert_eq!(timeline.interpreter.variables[3], -9);
        assert_eq!(timeline.display(), DisplayState::DEFAULT);
        assert_eq!(timeline.duration, 0);
    }

    #[test]
    fn scaled_clock_overflow_and_non_finite_values_use_i32_minimum() {
        assert_eq!(scaled_elapsed(i32::MAX, 2.0), i32::MIN);
        assert_eq!(scaled_elapsed(1, f32::INFINITY), i32::MIN);
        assert_eq!(scaled_elapsed(1, f32::NAN), i32::MIN);
        assert_eq!(scaled_elapsed(i32::MIN, 1.0), i32::MIN);
        assert_eq!(scaled_elapsed(3, 0.5), 1);
    }

    #[test]
    fn final_timed_instruction_finishes_only_after_its_duration() {
        let mut timeline = timeline(vec![instruction(0, &[imm(8), imm(3), imm(3)])]);
        assert!(timeline.update(0, 4).unwrap());
        assert_eq!(timeline.display().frame_id, 8);
        assert!(!timeline.is_finished());
        assert!(timeline.update(2, 4).unwrap());
        assert!(!timeline.is_finished());
        assert!(timeline.update(3, 4).unwrap());
        assert!(timeline.is_finished());
    }

    fn anm_bytes(instructions: &[AnmInstruction]) -> Vec<u8> {
        let mut bytes = vec![0; 32 + instructions.len() * 68];
        bytes[..4].copy_from_slice(b"ANM\0");
        bytes[8..12].copy_from_slice(&(instructions.len() as u32).to_le_bytes());
        for (index, instruction) in instructions.iter().enumerate() {
            let offset = 32 + index * 68;
            bytes[offset..offset + 4].copy_from_slice(&instruction.opcode.to_le_bytes());
            for (operand_index, operand) in instruction.operands.iter().enumerate() {
                let operand_offset = offset + 4 + operand_index * 8;
                bytes[operand_offset..operand_offset + 4]
                    .copy_from_slice(&operand.kind.to_le_bytes());
                bytes[operand_offset + 4..operand_offset + 8]
                    .copy_from_slice(&operand.value.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn image_animation_pause_resume_load_failure_and_restart_match_object_contract() {
        let instructions = vec![instruction(0, &[imm(7), imm(1), imm(1)])];
        let mut animation = ImageAnimation::default();
        animation.load(&anm_bytes(&instructions), 100).unwrap();
        assert!(animation.update(50, 8, false).unwrap());
        assert_eq!(animation.display(), DisplayState::DEFAULT);
        animation.playing = 1;
        assert!(animation.update(50, 8, false).unwrap());
        assert_eq!(animation.display().frame_id, 7);
        assert_eq!(animation.frame_change(), Some(7));
        animation.playing = 0;
        animation.update(60, 8, false).unwrap();
        assert!(animation.restart(8, false).unwrap());
        assert_eq!(animation.playing, 0);
        assert_eq!(animation.display().frame_id, 7);
        assert!(animation.load(&[], 0).is_err());
        assert!(animation.timeline().is_none());
        assert_eq!(animation.display().frame_id, 7);
    }

    #[test]
    fn reset_frame_alias_is_applied_to_object_cache_but_not_timeline_state() {
        let mut animation = ImageAnimation::default();
        animation.playing = 1;
        animation
            .load(
                &anm_bytes(&[instruction(0, &[imm(3), imm(1), imm(1)])]),
                10001,
            )
            .unwrap();
        assert!(animation.update(u32::MAX, 16, true).unwrap());
        assert_eq!(animation.display().frame_id, 10000);
        assert_eq!(animation.timeline().unwrap().display().frame_id, 3);
        assert_eq!(animation.frame_change(), Some(10000));
    }

    #[test]
    fn display_composition_preserves_raw_animation_attributes_and_wraps_coordinates() {
        let state = DisplayState {
            frame_id: 9,
            blend: 300,
            visible: -2,
            x: i32::MAX,
            y: -5,
        };
        assert_eq!(
            state.compose((2.9, f32::NAN), 200, 1),
            DisplayState {
                frame_id: 9,
                blend: 235,
                visible: -2,
                x: i32::MIN + 1,
                y: i32::MAX - 4,
            }
        );
        assert_eq!(state.compose((0.0, 0.0), 10, 0).visible, 0);
    }
}
