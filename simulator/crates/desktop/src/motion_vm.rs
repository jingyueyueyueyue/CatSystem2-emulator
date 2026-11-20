//! Motion KCS execution, independently implemented from the format/behavior
//! documentation. This module has no dependency on the reference engine.

use cs2_script::{Host, Instruction, KcsProgram, NativeReturn, ScriptError, Step, Vm};

#[path = "motion_vm_state.rs"]
pub(crate) mod checkpoint;

type Result<T> = cs2_script::Result<T>;

pub(super) const EXTERNAL: u32 = 0x8000_0000;
pub(super) const EXTERNAL_SIZE: usize = 0x4a8;
const ELAPSED: usize = 0x90;
const RUNNING: usize = 0x94;
const FINISHING: usize = 0x98;
const PARAMETERS: usize = 0x9c;
const SAVE_POINTER: usize = 0x49c;
const SAVE_LENGTH: usize = 0x4a0;
const EXTERNAL_VALUE: usize = 0x4a4;
const LIMIT: usize = 64 * 1024 * 1024;
const INDEFINITE: f32 = f32::from_bits(0xffc0_0000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExternalLayout {
    Default,
    Campanella,
}

impl ExternalLayout {
    fn offset(self, modern: usize) -> usize {
        modern - if self == Self::Campanella { 4 } else { 0 }
    }

    fn address(self, modern: usize) -> u32 {
        EXTERNAL + self.offset(modern) as u32
    }

    fn size(self) -> usize {
        self.offset(EXTERNAL_SIZE)
    }

    fn property_count(self) -> usize {
        self.offset(ELAPSED) / 4
    }
}

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[derive(Clone, Debug)]
pub(super) struct MotionProgram {
    program: KcsProgram,
}

impl MotionProgram {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        Self::from_program(KcsProgram::parse(bytes)?)
    }

    pub(super) fn from_program(program: KcsProgram) -> Result<Self> {
        let size = program.data.len().checked_add(program.stack_size);
        if size.is_none_or(|n| n > LIMIT) {
            return Err(invalid("motion program storage exceeds 64 MiB"));
        }
        Ok(Self { program })
    }

    pub(super) fn program(&self) -> &KcsProgram {
        &self.program
    }
}

/// All words are raw x86 DWORDs, including floating-point parameters.
pub(super) struct MotionStart<'a> {
    pub name: &'a [u8],
    pub parameters: [u32; 256],
    pub external_value: u32,
    pub properties: [u32; 36],
    pub base_time: u32,
    pub saved_data: &'a [u8],
    pub snapshot_flags: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MotionState {
    pub properties: [u32; 36],
    pub elapsed: u32,
    pub running: u32,
    pub finishing: u32,
    pub parameters: [u32; 256],
    pub save_address: u32,
    pub save_length: u32,
    pub external_value: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MotionProgress {
    NotStarted,
    Yield,
    Complete(Step),
}

/// Share one service across ordinary scripts, motion tasks, and particles.
/// In particular, the integer and floating streams must not be reseeded per frame.
pub(super) trait MotionServices {
    fn random_integer(&mut self) -> u32;
    fn random_float_bits(&mut self) -> u32;
    fn debug(&mut self, text: &[u8]) -> Result<()>;
}

#[derive(Default)]
pub(super) struct MotionHost {
    pub random: MotionRandom,
    pub debug_messages: Vec<Vec<u8>>,
}

impl MotionServices for MotionHost {
    fn random_integer(&mut self) -> u32 {
        self.random.next_integer()
    }

    fn random_float_bits(&mut self) -> u32 {
        self.random.next_float_bits()
    }

    fn debug(&mut self, text: &[u8]) -> Result<()> {
        self.debug_messages.push(text.to_vec());
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum Invocation {
    Update,
    Finish([u32; 3]),
}

pub(super) struct MotionRunner {
    program: MotionProgram,
    vm: Vm,
    layout: ExternalLayout,
    short_display: u32,
    base_time: u32,
    stack_end: usize,
    storage_end: usize,
    invocation: Option<Invocation>,
    name: Vec<u16>,
    snapshot_flags: u32,
    snapshot_data: Option<(u32, usize)>,
}

impl MotionRunner {
    pub(super) fn start(bytes: &[u8], start: MotionStart<'_>) -> Result<Self> {
        Self::from_program(MotionProgram::parse(bytes)?, start)
    }

    pub(super) fn from_program(program: MotionProgram, start: MotionStart<'_>) -> Result<Self> {
        Self::with_external_layout(program, start, ExternalLayout::Default)
    }

    pub(super) fn with_external_layout(
        program: MotionProgram,
        start: MotionStart<'_>,
        layout: ExternalLayout,
    ) -> Result<Self> {
        let name = snapshot_name(start.name)?;
        let stack_end = program.program.data.len() + program.program.stack_size;
        if stack_end == 0 && !start.saved_data.is_empty() {
            return Err(invalid(
                "motion saved storage cannot use the null snapshot address",
            ));
        }
        let storage_end = stack_end
            .checked_add(start.saved_data.len())
            .filter(|&n| n <= LIMIT)
            .ok_or_else(|| invalid("motion saved storage exceeds 64 MiB"))?;
        let mut external = vec![0; layout.size()];
        store_words(&mut external, 0, &start.properties[..layout.property_count()]);
        store_words(&mut external, layout.offset(RUNNING), &[1]);
        store_words(&mut external, layout.offset(PARAMETERS), &start.parameters);
        store_words(&mut external, layout.offset(EXTERNAL_VALUE), &[start.external_value]);
        let mut vm = Vm::new();
        vm.load_program(program.program.clone(), external)?;
        let snapshot_data = if start.saved_data.is_empty() {
            None
        } else {
            let base = vm.allocate_region(start.saved_data)?;
            vm.write(layout.address(SAVE_POINTER), &words_as_bytes(&[
                base, start.saved_data.len() as u32,
            ]))?;
            Some((base, start.saved_data.len()))
        };
        Ok(Self {
            program,
            vm,
            layout,
            short_display: if layout == ExternalLayout::Campanella { start.properties[35] } else { 0 },
            base_time: start.base_time,
            stack_end,
            storage_end,
            invocation: None,
            name,
            snapshot_flags: start.snapshot_flags,
            snapshot_data,
        })
    }

    pub(super) fn state(&self) -> Result<MotionState> {
        let bytes = self.external_bytes()?;
        let layout = self.layout;
        let word = |modern| word_at(&bytes, layout.offset(modern));
        Ok(MotionState {
            properties: std::array::from_fn(|index| if index < layout.property_count() {
                word_at(&bytes, index * 4)
            } else {
                self.short_display
            }),
            elapsed: word(ELAPSED),
            running: word(RUNNING),
            finishing: word(FINISHING),
            parameters: std::array::from_fn(|index| word(PARAMETERS + index * 4)),
            save_address: word(SAVE_POINTER),
            save_length: word(SAVE_LENGTH),
            external_value: word(EXTERNAL_VALUE),
        })
    }

    pub(super) fn properties(&self) -> Result<[u32; 36]> {
        Ok(self.state()?.properties)
    }

    pub(super) fn is_pending(&self) -> bool {
        self.invocation.is_some()
    }

    pub(super) fn base_time(&self) -> u32 {
        self.base_time
    }

    pub(super) fn set_base_time(&mut self, time: u32) -> Result<()> {
        self.require_idle()?;
        self.base_time = time;
        Ok(())
    }

    pub(super) fn set_properties(&mut self, properties: &[u32; 36]) -> Result<()> {
        self.require_idle()?;
        self.vm.write(EXTERNAL, &words_as_bytes(&properties[..self.layout.property_count()]))?;
        if self.layout == ExternalLayout::Campanella {
            self.short_display = properties[35];
        }
        Ok(())
    }

    pub(super) fn set_parameters(&mut self, parameters: &[u32]) -> Result<()> {
        self.require_idle()?;
        let mut bytes = [0; 1024];
        store_words(&mut bytes, 0, &parameters[..parameters.len().min(256)]);
        self.vm.write(self.layout.address(PARAMETERS), &bytes)
    }

    pub(super) fn request_finish(&mut self) -> Result<()> {
        self.require_idle()?;
        self.vm.write(self.layout.address(FINISHING), &1_u32.to_le_bytes())
    }

    pub(super) fn read_memory(&self, address: u32, length: usize) -> Result<Vec<u8>> {
        self.check_visible(address, length)?;
        self.vm.read(address, length)
    }

    pub(super) fn write_memory(&mut self, address: u32, bytes: &[u8]) -> Result<()> {
        self.require_idle()?;
        self.check_visible(address, bytes.len())?;
        self.vm.write(address, bytes)
    }

    pub(super) fn external_bytes(&self) -> Result<Vec<u8>> {
        self.vm.read(EXTERNAL, self.layout.size())
    }

    /// Register a snapshot source separately from script-visible pointer fields.
    /// A zero address means no registered data, as in the documented task ABI.
    pub(super) fn register_snapshot_data(&mut self, address: u32, length: usize) -> Result<()> {
        self.require_idle()?;
        if length > LIMIT {
            return Err(invalid("motion snapshot data exceeds 64 MiB"));
        }
        if address != 0 {
            self.read_memory(address, length)?;
        }
        self.snapshot_data = (address != 0).then_some((address, length));
        self.vm.write(self.layout.address(SAVE_POINTER), &address.to_le_bytes())?;
        self.vm.write(self.layout.address(SAVE_LENGTH), &(length as u32).to_le_bytes())
    }

    /// `now` is controller-local time, after speed/origin conversion.
    /// Do not suppress this call merely because running is zero: stopped entries
    /// still contribute while another task in the same queue remains active.
    pub(super) fn update(
        &mut self,
        now: u32,
        services: &mut impl MotionServices,
        budget: usize,
    ) -> Result<MotionProgress> {
        if matches!(self.invocation, Some(Invocation::Finish(_))) {
            return Err(invalid(
                "finish continuation must complete before motion update",
            ));
        }
        if self.invocation.is_none() {
            if self.vm.read_u32(self.layout.address(FINISHING))? == 0 && now < self.base_time {
                return Ok(MotionProgress::NotStarted);
            }
            self.vm
                .write(self.layout.address(ELAPSED), &now.wrapping_sub(self.base_time).to_le_bytes())?;
            self.vm.reset_execution()?;
            self.invocation = Some(Invocation::Update);
        }
        let result = self.run_budget(services, budget);
        if !matches!(&result, Ok(MotionProgress::Yield)) {
            self.invocation = None;
        }
        result
    }

    /// An explicit finish can interrupt an incomplete update. It restores the
    /// three control words on completion/error, retaining other script effects.
    pub(super) fn finish(
        &mut self,
        services: &mut impl MotionServices,
        budget: usize,
    ) -> Result<MotionProgress> {
        if !matches!(self.invocation, Some(Invocation::Finish(_))) {
            let saved = [
                self.vm.read_u32(self.layout.address(ELAPSED))?,
                self.vm.read_u32(self.layout.address(RUNNING))?,
                self.vm.read_u32(self.layout.address(FINISHING))?,
            ];
            self.invocation = Some(Invocation::Finish(saved));
            self.vm.write(self.layout.address(ELAPSED), &u32::MAX.to_le_bytes())?;
            self.request_finish_word()?;
            if let Err(error) = self.vm.reset_execution() {
                self.restore_finish()?;
                return Err(error);
            }
        }
        let result = self.run_budget(services, budget);
        if !matches!(&result, Ok(MotionProgress::Yield)) {
            self.restore_finish()?;
        }
        result
    }

    fn request_finish_word(&mut self) -> Result<()> {
        self.vm.write(self.layout.address(FINISHING), &1_u32.to_le_bytes())
    }

    pub(super) fn snapshot(&self, now: u32, group: i32) -> Result<MotionSnapshot> {
        self.require_idle()?;
        let saved_data = match self.snapshot_data {
            Some((address, length)) => self.read_memory(address, length)?,
            None => Vec::new(),
        };
        Ok(MotionSnapshot {
            name: self.name.clone(),
            group,
            parameters: self.state()?.parameters,
            saved_data,
            time_delta: now.wrapping_sub(self.base_time),
            flags: self.snapshot_flags,
        })
    }

    fn require_idle(&self) -> Result<()> {
        if self.is_pending() {
            Err(invalid(
                "cannot mutate/snapshot motion during budget continuation",
            ))
        } else {
            Ok(())
        }
    }

    fn check_visible(&self, address: u32, length: usize) -> Result<()> {
        MotionMemory {
            end: self.storage_end,
            stack_end: self.stack_end,
            external_size: self.layout.size(),
        }
        .range(address, length)
    }

    fn restore_finish(&mut self) -> Result<()> {
        if let Some(Invocation::Finish(saved)) = self.invocation.take() {
            self.vm.write(self.layout.address(ELAPSED), &words_as_bytes(&saved))?;
        }
        Ok(())
    }

    fn run_budget(
        &mut self,
        services: &mut impl MotionServices,
        budget: usize,
    ) -> Result<MotionProgress> {
        for _ in 0..budget {
            let pc = self.vm.instruction_pointer;
            if pc == self.program.program.code.len() {
                return Ok(MotionProgress::Complete(Step::Halted));
            }
            let instruction = self
                .program
                .program
                .instruction_at(pc)
                .cloned()
                .ok_or(ScriptError::InvalidJump { target: pc })?;
            preflight(
                &self.vm,
                &instruction,
                self.program.program.data.len(),
                self.stack_end,
                self.storage_end,
                self.layout.size(),
            )?;
            let mut host = NativeHost {
                services,
                storage_end: self.storage_end,
                stack_end: self.stack_end,
                layout: self.layout,
            };
            let step = self.vm.run(&mut host, 1)?;
            if step != Step::Yield {
                return Ok(MotionProgress::Complete(step));
            }
            if self.vm.instruction_pointer == self.program.program.code.len() {
                return Ok(MotionProgress::Complete(Step::Halted));
            }
        }
        Ok(MotionProgress::Yield)
    }
}

fn word_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn store_words(bytes: &mut [u8], offset: usize, words: &[u32]) {
    for (index, value) in words.iter().enumerate() {
        bytes[offset + index * 4..offset + index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
}

fn words_as_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|value| value.to_le_bytes()).collect()
}

/// Match the low-level VM's operand-consumption order without modifying it.
/// Independent saved-data regions do not enlarge the operand stack.
fn preflight(
    vm: &Vm,
    instruction: &Instruction,
    stack_start: usize,
    stack_end: usize,
    storage_end: usize,
    external_size: usize,
) -> Result<()> {
    let memory = MotionMemory {
        end: storage_end,
        stack_end,
        external_size,
    };
    let mut sp = vm.stack_pointer();
    let pop = |sp: &mut usize| -> Result<u32> {
        // Vm::pop_word permits consuming initialized storage down to address zero.
        *sp = sp.checked_sub(4).ok_or(ScriptError::StackUnderflow)?;
        vm.read_u32(*sp as u32)
    };
    let mut values = Vec::new();
    let mut raw_values = Vec::new();
    for (index, operand) in instruction.operands.iter().copied().enumerate() {
        if instruction.opcode == 0x25 && operand.flags.from_stack() && !operand.flags.is_address() {
            values.push(0);
            raw_values.push(0);
            continue;
        }
        let raw = if operand.flags.from_stack() {
            pop(&mut sp)?
        } else {
            operand.payload
        };
        raw_values.push(raw);
        let raw_target = index == 0
            && matches!(instruction.opcode,
            0x15..=0x19 | 0x1b..=0x26 | 0x2e..=0x30 | 0x3e);
        let value = if operand.flags.is_address() && !raw_target {
            let width = operand.flags.width()?;
            let bytes = memory.read(vm, raw, width)?;
            match width {
                1 if operand.flags.is_signed() => (bytes[0] as i8 as i32) as u32,
                1 => bytes[0] as u32,
                2 if operand.flags.is_signed() => {
                    (i16::from_le_bytes(bytes.try_into().unwrap()) as i32) as u32
                }
                2 => u16::from_le_bytes(bytes.try_into().unwrap()) as u32,
                _ => u32::from_le_bytes(bytes.try_into().unwrap()),
            }
        } else {
            raw
        };
        values.push(value);
    }
    match instruction.opcode {
        0x15 => memory.range(raw_values[0], instruction.operands[0].flags.width()?)?,
        0x26 => memory.range(
            raw_values[0],
            cs2_script::TypeFlags::new(instruction.operands[0].flags.bits() & 0x7f).width()?,
        )?,
        0x16..=0x19 | 0x1b..=0x24 | 0x2f => memory.range(raw_values[0], 4)?,
        0x25 | 0x3e if instruction.operands[0].flags.is_address() => {
            memory.range(raw_values[0], 4)?
        }
        0x30 => memory.range(raw_values[0], values[1] as usize)?,
        0x41 => {
            memory.string(vm, values[1])?;
        }
        0x42 => memory.range(values[0], 0)?,
        _ => {}
    }
    let mut push = 0usize;
    match instruction.opcode {
        0x01..=0x24 | 0x2e..=0x2f | 0x36..=0x39 | 0x3d..=0x3e => push = 4,
        0x25 if instruction.operands[0].flags.is_address()
            || !instruction.operands[0].flags.from_stack() =>
        {
            push = 4
        }
        0x26..=0x28 | 0x2d | 0x33 => {
            pop(&mut sp)?;
        }
        0x2a => push = 12,
        0x2b => {
            sp = sp
                .checked_sub(values[1] as usize)
                .filter(|&n| n >= vm.frame_top())
                .ok_or(ScriptError::CallParameterUnderflow)?;
            if matches!(values[0], 1..=17 | 34..=35 | 39..=42) {
                push = 4;
            }
        }
        0x2c => {
            let header = vm
                .frame_base()
                .checked_add(values[1] as usize)
                .filter(|&n| n.checked_add(12).is_some_and(|end| end <= stack_end))
                .ok_or(ScriptError::InvalidCallFrame)?;
            if vm.read_u32((header + 4) as u32)? as usize > stack_end
                || vm.read_u32((header + 8) as u32)? as usize > stack_end
            {
                return Err(ScriptError::InvalidCallFrame);
            }
            sp = vm.frame_base();
            push = values[0] as usize;
        }
        0x31 | 0x32 => {
            sp = if instruction.opcode == 0x31 {
                vm.frame_top().checked_add(values[0] as usize)
            } else {
                vm.frame_top().checked_sub(values[0] as usize)
            }
            .filter(|&n| n >= stack_start)
            .ok_or(ScriptError::InvalidLocalFrame)?;
        }
        0x3a => {
            pop(&mut sp)?;
            push = 8;
        }
        _ => {}
    }
    if sp.checked_add(push).is_none_or(|end| end > stack_end) {
        return Err(ScriptError::StackOverflow);
    }
    Ok(())
}

/// Controller time follows the unsigned low-DWORD conversion: truncate and wrap.
/// NaN/infinity produce zero, unlike native 17's signed i32::MIN result.
/// Keep the multiply and add separately rounded to f32.
pub(super) fn controller_time(now: u32, origin: u32, speed_bits: u32) -> u32 {
    let speed = f32::from_bits(speed_bits);
    if !speed.is_finite() {
        return 0;
    }
    let product = (now.wrapping_sub(origin) as f32) * speed;
    let value = product + 0.5_f32;
    if !value.is_finite() {
        return 0;
    }
    (f64::from(value).trunc().rem_euclid(4_294_967_296.0)) as u32
}

#[derive(Clone, Debug)]
pub(super) struct MotionSnapshot {
    pub name: Vec<u16>,
    pub group: i32,
    pub parameters: [u32; 256],
    pub saved_data: Vec<u8>,
    pub time_delta: u32,
    pub flags: u32,
}

impl MotionSnapshot {
    /// One 0x454-byte fixed record followed by the unaligned saved-data bytes.
    pub(super) fn record(&self) -> Result<Vec<u8>> {
        let units = self
            .name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(self.name.len());
        if units > 31 {
            return Err(invalid("motion snapshot name exceeds 31 UTF-16 units"));
        }
        let size = 0x454usize
            .checked_add(self.saved_data.len())
            .filter(|&n| n <= LIMIT)
            .ok_or_else(|| invalid("motion snapshot exceeds 64 MiB"))?;
        let mut record = vec![0; size];
        store_words(&mut record, 0, &[size as u32]);
        for (index, unit) in self.name[..units].iter().enumerate() {
            record[4 + index * 2..6 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }
        store_words(&mut record, 0x44, &[self.group as u32]);
        store_words(&mut record, 0x48, &self.parameters);
        store_words(
            &mut record,
            0x448,
            &[self.saved_data.len() as u32, self.time_delta, self.flags],
        );
        record[0x454..].copy_from_slice(&self.saved_data);
        Ok(record)
    }
}

fn snapshot_name(name: &[u8]) -> Result<Vec<u16>> {
    let end = name
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(name.len());
    let name = &name[..end];
    if name.len() >= 260 {
        return Err(invalid("motion path exceeds 259 bytes"));
    }
    let mut cursor = 0;
    let mut last = 0;
    while cursor < name.len() {
        let byte = name[cursor];
        if ((0x81..=0x9f).contains(&byte) || (0xe0..=0xef).contains(&byte))
            && name
                .get(cursor + 1)
                .is_some_and(|b| (0x40..=0x7e).contains(b) || (0x80..=0xfc).contains(b))
        {
            cursor += 2;
        } else {
            if cursor != 0 && matches!(byte, b'/' | b'\\') {
                last = cursor + 1;
            }
            cursor += 1;
        }
    }
    let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(&name[last..]);
    if errors {
        return Err(invalid("motion path contains invalid CP932"));
    }
    let result: Vec<_> = text.encode_utf16().collect();
    if result.len() > 31 {
        return Err(invalid("motion name exceeds 31 UTF-16 units"));
    }
    Ok(result)
}

struct NativeHost<'a, S> {
    services: &'a mut S,
    storage_end: usize,
    stack_end: usize,
    layout: ExternalLayout,
}

impl<S: MotionServices> Host for NativeHost<'_, S> {
    fn call(&mut self, index: u32, arguments: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let count =
            native_argument_words(index).ok_or(ScriptError::UnimplementedNativeCall { index })?;
        if arguments.len() != count * 4 {
            return Err(invalid(
                "motion native argument byte count does not match its ABI",
            ));
        }
        let words: Vec<_> = arguments
            .chunks_exact(4)
            .map(|raw| u32::from_le_bytes(raw.try_into().unwrap()))
            .collect();
        let scalar = |index: usize| f32::from_bits(words[index]);
        let memory = MotionMemory {
            end: self.storage_end,
            stack_end: self.stack_end,
            external_size: self.layout.size(),
        };
        match index {
            0 => {
                let text = expand_format(&memory, vm, words[0], 0)?;
                self.services.debug(&text)?;
            }
            1..=15 => {
                let second = words.get(1).copied().unwrap_or(0);
                return Ok(NativeReturn::Value(scalar_math(index, words[0], second)));
            }
            16 => return Ok(NativeReturn::Value((words[0] as i32).wrapping_abs() as u32)),
            17 => {
                let rounded = f64::from(scalar(0) + 0.5_f32).floor() as f32;
                return Ok(NativeReturn::Value(trunc_i32(rounded) as u32));
            }
            18..=20 => {
                let input = [
                    memory.float(vm, words[0])?,
                    memory.float(vm, words[1])?,
                    memory.float(vm, words[2])?,
                ];
                let output = rotate_triple(input, scalar(3), (index - 18) as usize);
                for component in 0..3 {
                    memory.write(
                        vm,
                        words[component],
                        &output[component].to_bits().to_le_bytes(),
                    )?;
                }
            }
            21..=22 => {
                // Reads follow the preceding writes when component addresses alias.
                for component in 0..3 {
                    let previous = memory.float(vm, words[component])?;
                    let value = if index == 21 {
                        previous + scalar(component + 3)
                    } else {
                        previous * scalar(component + 3)
                    };
                    memory.write(vm, words[component], &value.to_bits().to_le_bytes())?;
                }
            }
            23..=25 | 43 => {
                let mut matrix = memory.array::<16>(vm, words[0])?;
                let xyz = [scalar(1), scalar(2), scalar(3)];
                match index {
                    23 => {
                        for (axis, angle) in xyz.into_iter().enumerate() {
                            matrix = multiply_matrix(matrix, rotation_axis(axis, angle));
                        }
                    }
                    24 => {
                        let mut translation = identity();
                        translation[12..15].copy_from_slice(&xyz);
                        matrix = multiply_matrix(matrix, translation);
                    }
                    25 => {
                        let mut scaling = identity();
                        for axis in 0..3 {
                            scaling[axis * 5] = xyz[axis];
                        }
                        matrix = multiply_matrix(matrix, scaling);
                    }
                    43 => matrix = multiply_matrix(matrix, yaw_pitch_roll(xyz)),
                    _ => unreachable!(),
                }
                memory.write_array(vm, words[0], &matrix)?;
            }
            26 => memory.write_array(vm, words[0], &identity())?,
            27 => {
                let original = memory.array::<16>(vm, words[0])?;
                if let Some(inverse) = inverse_matrix(original) {
                    memory.write_array(vm, words[0], &inverse)?;
                }
            }
            28 => {
                // MOVUPS-sized forward copies, deliberately not memmove.
                for block in 0..4 {
                    let data = memory.read(vm, offset(words[1], block * 16)?, 16)?;
                    memory.write(vm, offset(words[0], block * 16)?, &data)?;
                }
            }
            29 => {
                let target = memory.array::<16>(vm, words[0])?;
                let source = memory.array::<16>(vm, words[1])?;
                memory.write_array(vm, words[0], &multiply_matrix(target, source))?;
            }
            30 => {
                for block in 0..4 {
                    let address = offset(words[0], block * 16)?;
                    let left = memory.array::<4>(vm, address)?;
                    let right = memory.array::<4>(vm, offset(words[1], block * 16)?)?;
                    let difference = std::array::from_fn::<_, 4, _>(|i| left[i] - right[i]);
                    memory.write_array(vm, address, &difference)?;
                }
            }
            31 => {
                let vector = memory.array::<4>(vm, words[0])?;
                memory.write_array(vm, words[0], &normalize4(vector))?;
            }
            32..=33 => {
                for component in 0..4 {
                    let destination = offset(words[0], component * 4)?;
                    let left = memory.float(vm, destination)?;
                    let right = memory.float(vm, offset(words[1], component * 4)?)?;
                    let value = if index == 32 {
                        left + right
                    } else {
                        left - right
                    };
                    memory.write(vm, destination, &value.to_bits().to_le_bytes())?;
                }
            }
            34..=35 => {
                let vector = memory.array::<4>(vm, words[0])?;
                let squared = dot4(vector, vector);
                let length = if index == 34 {
                    f64::from(squared).sqrt() as f32
                } else {
                    squared
                };
                return Ok(NativeReturn::Value(finite_or_nan(length).to_bits()));
            }
            36 => {
                let factor = memory.float(vm, words[1])?;
                let vector = memory.array::<4>(vm, words[0])?.map(|value| value * factor);
                memory.write_array(vm, words[0], &vector)?;
            }
            37 => {
                let vector = memory.array::<4>(vm, words[0])?;
                let matrix = memory.array::<16>(vm, words[1])?;
                memory.write_array(vm, words[0], &transform(vector, matrix))?;
            }
            38 => {
                memory.write(vm, self.layout.address(RUNNING), &0_u32.to_le_bytes())?;
                return Ok(NativeReturn::YieldVoid);
            }
            39 => return Ok(NativeReturn::Value(self.services.random_integer())),
            40 => return Ok(NativeReturn::Value(self.services.random_float_bits())),
            41 => {
                return Ok(NativeReturn::Value(
                    interpolate([scalar(0), scalar(1), scalar(2), scalar(3), scalar(4)]).to_bits(),
                ));
            }
            42 => {
                return Ok(NativeReturn::Value(
                    triangle(scalar(0), scalar(1)).to_bits(),
                ));
            }
            44 => {
                let mut target = memory.array::<16>(vm, words[0])?;
                let reference = memory.array::<16>(vm, words[1])?;
                let angles = memory.array::<3>(vm, words[2])?;
                let x = normalize3([-reference[0], 0.0, -reference[2]]);
                let z = normalize3([-reference[8], -reference[9], -reference[10]]);
                for (axis, angle) in [x, [0.0, -1.0, 0.0], z].into_iter().zip(angles) {
                    target = multiply_matrix(target, axis_angle(axis, angle));
                }
                memory.write_array(vm, words[0], &target)?;
            }
            _ => unreachable!("all documented motion natives are handled"),
        }
        Ok(NativeReturn::Continue)
    }
}

pub(super) fn native_argument_words(index: u32) -> Option<usize> {
    match index {
        0..=6 | 8..=10 | 12 | 14..=17 | 26..=27 | 31 | 34..=35 => Some(1),
        7 | 11 | 13 | 28..=30 | 32..=33 | 36..=37 | 42 => Some(2),
        44 => Some(3),
        18..=20 | 23..=25 | 43 => Some(4),
        41 => Some(5),
        21..=22 => Some(6),
        38..=40 => Some(0),
        _ => None,
    }
}

struct MotionMemory {
    end: usize,
    stack_end: usize,
    external_size: usize,
}

impl MotionMemory {
    fn range(&self, address: u32, length: usize) -> Result<()> {
        let limit = if address & EXTERNAL != 0 {
            self.external_size
        } else if (address as usize) < self.stack_end {
            self.stack_end
        } else {
            self.end
        };
        if ((address & !EXTERNAL) as usize)
            .checked_add(length)
            .is_none_or(|end| end > limit)
        {
            return Err(ScriptError::MemoryOutOfBounds { address, length });
        }
        Ok(())
    }

    fn read(&self, vm: &Vm, address: u32, length: usize) -> Result<Vec<u8>> {
        self.range(address, length)?;
        vm.read(address, length)
    }

    fn write(&self, vm: &mut Vm, address: u32, bytes: &[u8]) -> Result<()> {
        self.range(address, bytes.len())?;
        vm.write(address, bytes)
    }

    fn float(&self, vm: &Vm, address: u32) -> Result<f32> {
        Ok(f32::from_bits(word_at(&self.read(vm, address, 4)?, 0)))
    }

    fn array<const N: usize>(&self, vm: &Vm, address: u32) -> Result<[f32; N]> {
        let bytes = self.read(vm, address, N * 4)?;
        Ok(std::array::from_fn(|index| {
            f32::from_bits(word_at(&bytes, index * 4))
        }))
    }

    fn write_array(&self, vm: &mut Vm, address: u32, floats: &[f32]) -> Result<()> {
        let bytes: Vec<_> = floats
            .iter()
            .flat_map(|value| value.to_bits().to_le_bytes())
            .collect();
        self.write(vm, address, &bytes)
    }

    fn string(&self, vm: &Vm, address: u32) -> Result<Vec<u8>> {
        self.range(address, 1)?;
        let available = if address & EXTERNAL != 0 {
            self.external_size
        } else if (address as usize) < self.stack_end {
            self.stack_end
        } else {
            self.end
        } - (address & !EXTERNAL) as usize;
        let mut text = Vec::new();
        while text.len() < available {
            let count = (available - text.len()).min(256);
            let bytes = vm.read(offset(address, text.len())?, count)?;
            if let Some(end) = bytes.iter().position(|&byte| byte == 0) {
                text.extend_from_slice(&bytes[..end]);
                return Ok(text);
            }
            text.extend_from_slice(&bytes);
        }
        Err(invalid("unterminated motion debug string"))
    }
}

fn offset(address: u32, bytes: usize) -> Result<u32> {
    let result = (address & !EXTERNAL)
        .checked_add(u32::try_from(bytes).map_err(|_| invalid("motion address offset overflows"))?)
        .filter(|&value| value & EXTERNAL == 0)
        .ok_or_else(|| invalid("motion address offset crosses memory regions"))?;
    Ok(result | (address & EXTERNAL))
}

fn quiet(value: f32) -> f32 {
    f32::from_bits(value.to_bits() | 0x0040_0000)
}

fn finite_or_nan(value: f32) -> f32 {
    if value.is_nan() { INDEFINITE } else { value }
}

fn scalar_math(index: u32, first: u32, second: u32) -> u32 {
    let a = f32::from_bits(first);
    let b = f32::from_bits(second);
    if index == 9 {
        return (first & 0x7fff_ffff) | if a.is_nan() { 0x0040_0000 } else { 0 };
    }
    if index == 14 {
        return ((a * std::f32::consts::PI) / 180.0_f32).to_bits();
    }
    if index == 15 {
        return ((a * 180.0_f32) / std::f32::consts::PI).to_bits();
    }
    let x = f64::from(a);
    let y = f64::from(b);
    let computed = match index {
        1 => x.sin(),
        2 => x.cos(),
        3 => x.tan(),
        4 => x.asin(),
        5 => x.acos(),
        6 => x.atan(),
        7 => x.atan2(y),
        8 => x.ceil(),
        10 => x.floor(),
        11 => x % y,
        12 => x.sqrt(),
        13 => x.powf(y),
        _ => unreachable!(),
    } as f32;
    if !computed.is_nan() {
        return computed.to_bits();
    }
    if a.is_nan() {
        return quiet(a).to_bits();
    }
    if matches!(index, 7 | 11 | 13) && b.is_nan() {
        return quiet(b).to_bits();
    }
    INDEFINITE.to_bits()
}

fn trunc_i32(value: f32) -> i32 {
    if !value.is_finite() || !(-2_147_483_648.0..2_147_483_648.0).contains(&f64::from(value)) {
        i32::MIN
    } else {
        value.trunc() as i32
    }
}

fn interpolate([start, end, first, last, now]: [f32; 5]) -> f32 {
    if now <= first {
        return start;
    }
    if now >= last {
        return end;
    }
    let fraction = (f64::from(now) - f64::from(first)) / (f64::from(last) - f64::from(first));
    finite_or_nan((f64::from(start) + (f64::from(end) - f64::from(start)) * fraction) as f32)
}

fn triangle(input: f32, period: f32) -> f32 {
    if input.is_nan() {
        return quiet(input);
    }
    if period.is_nan() {
        return quiet(period);
    }
    let period = f64::from(period).abs();
    let phase = f64::from(input).abs() % period;
    let rising = phase / (period * 0.5);
    finite_or_nan(if rising <= 1.0 {
        rising as f32
    } else {
        (2.0 - rising) as f32
    })
}

fn identity() -> [f32; 16] {
    let mut matrix = [0.0; 16];
    for index in [0, 5, 10, 15] {
        matrix[index] = 1.0;
    }
    matrix
}

fn multiply_matrix(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    std::array::from_fn(|index| {
        let row = (index / 4) * 4;
        let column = index % 4;
        let a = left[row] * right[column];
        let b = left[row + 1] * right[column + 4];
        let c = left[row + 2] * right[column + 8];
        let d = left[row + 3] * right[column + 12];
        (a + c) + (b + d)
    })
}

fn transform(vector: [f32; 4], matrix: [f32; 16]) -> [f32; 4] {
    std::array::from_fn(|column| {
        let first = vector[0] * matrix[column] + vector[2] * matrix[column + 8];
        let second = vector[1] * matrix[column + 4] + vector[3] * matrix[column + 12];
        first + second
    })
}

fn dot4(left: [f32; 4], right: [f32; 4]) -> f32 {
    let products: [f32; 4] = std::array::from_fn(|i| left[i] * right[i]);
    (products[0] + products[2]) + (products[1] + products[3])
}

fn normalize4(vector: [f32; 4]) -> [f32; 4] {
    let squared = dot4(vector, vector);
    let estimate = reciprocal_sqrt_estimate(squared);
    let product = squared * estimate;
    let correction = product * estimate;
    let factor = (estimate * (3.0_f32 - correction)) * 0.5_f32;
    vector.map(|component| finite_or_nan(component * factor))
}

fn normalize3(vector: [f32; 3]) -> [f32; 3] {
    let squared = (vector[0] * vector[0] + vector[2] * vector[2]) + vector[1] * vector[1];
    let estimate = reciprocal_sqrt_estimate(squared);
    let factor = (estimate * (3.0_f32 - (squared * estimate) * estimate)) * 0.5_f32;
    vector.map(|component| finite_or_nan(component * factor))
}

fn rotation_axis(axis: usize, angle: f32) -> [f32; 16] {
    let (sine, cosine) = f64::from(angle).sin_cos();
    let (sine, cosine) = (sine as f32, cosine as f32);
    let mut matrix = identity();
    match axis {
        0 => {
            matrix[5] = cosine;
            matrix[6] = sine;
            matrix[9] = -sine;
            matrix[10] = cosine;
        }
        1 => {
            matrix[0] = cosine;
            matrix[2] = -sine;
            matrix[8] = sine;
            matrix[10] = cosine;
        }
        2 => {
            matrix[0] = cosine;
            matrix[1] = sine;
            matrix[4] = -sine;
            matrix[5] = cosine;
        }
        _ => unreachable!(),
    }
    matrix
}

fn axis_angle([x, y, z]: [f32; 3], angle: f32) -> [f32; 16] {
    // The axis-angle path promotes to double, unlike the quaternion path.
    let (sin, cos) = f64::from(angle).sin_cos();
    let t = 1.0 - cos;
    let (x, y, z) = (f64::from(x), f64::from(y), f64::from(z));
    [
        (cos + x * x * t) as f32,
        (x * y * t + z * sin) as f32,
        (x * z * t - y * sin) as f32,
        0.0,
        (y * x * t - z * sin) as f32,
        (cos + y * y * t) as f32,
        (y * z * t + x * sin) as f32,
        0.0,
        (z * x * t + y * sin) as f32,
        (z * y * t - x * sin) as f32,
        (cos + z * z * t) as f32,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

fn rotate_triple(input: [f32; 3], angle: f32, axis: usize) -> [f32; 3] {
    let (sine, cosine) = f64::from(angle).sin_cos();
    let [x, y, z] = input.map(f64::from);
    match axis {
        0 => [
            input[0],
            finite_or_nan((y * cosine - z * sine) as f32),
            finite_or_nan((y * sine + z * cosine) as f32),
        ],
        1 => [
            finite_or_nan((x * cosine + z * sine) as f32),
            input[1],
            finite_or_nan((-x * sine + z * cosine) as f32),
        ],
        2 => [
            finite_or_nan((x * cosine - y * sine) as f32),
            finite_or_nan((x * sine + y * cosine) as f32),
            input[2],
        ],
        _ => unreachable!(),
    }
}

fn inverse_matrix(matrix: [f32; 16]) -> Option<[f32; 16]> {
    // Adjugate expansion avoids pivoting choices and preserves singular inputs.
    let mut adjugate = [0.0; 16];
    for row in 0..4 {
        for column in 0..4 {
            let mut minor = [0.0; 9];
            let mut count = 0;
            for source_row in 0..4 {
                if source_row == row {
                    continue;
                }
                for source_column in 0..4 {
                    if source_column != column {
                        minor[count] = matrix[source_row * 4 + source_column];
                        count += 1;
                    }
                }
            }
            let determinant = minor[0] * (minor[4] * minor[8] - minor[5] * minor[7])
                - minor[1] * (minor[3] * minor[8] - minor[5] * minor[6])
                + minor[2] * (minor[3] * minor[7] - minor[4] * minor[6]);
            adjugate[column * 4 + row] = if (row + column) % 2 == 0 {
                determinant
            } else {
                -determinant
            };
        }
    }
    let determinant = dot4(
        [matrix[0], matrix[1], matrix[2], matrix[3]],
        [adjugate[0], adjugate[4], adjugate[8], adjugate[12]],
    );
    if determinant == 0.0 {
        return None;
    }
    let estimate = reciprocal_estimate(determinant);
    let square = estimate * estimate;
    let reciprocal = (estimate + estimate) - determinant * square;
    Some(adjugate.map(|value| finite_or_nan(value * reciprocal)))
}

/// Polynomial coefficients are derived from the sine/cosine Taylor series.
/// Their bitwise equivalence to the original SSE polynomial is not established.
fn polynomial_sin_cos(angle: f32) -> (f32, f32) {
    if !angle.is_finite() {
        return (INDEFINITE, INDEFINITE);
    }
    let turns = (f64::from(angle) / std::f64::consts::TAU).round();
    let mut x = (f64::from(angle) - turns * std::f64::consts::TAU) as f32;
    let mut cosine_sign = 1.0_f32;
    if x > std::f32::consts::FRAC_PI_2 {
        x = std::f32::consts::PI - x;
        cosine_sign = -1.0;
    } else if x < -std::f32::consts::FRAC_PI_2 {
        x = -std::f32::consts::PI - x;
        cosine_sign = -1.0;
    }
    let squared = x * x;
    let sine = x
        * (1.0
            + squared
                * (-1.0 / 6.0
                    + squared
                        * (1.0 / 120.0
                            + squared
                                * (-1.0 / 5040.0
                                    + squared
                                        * (1.0 / 362880.0 + squared * (-1.0 / 39916800.0))))));
    let cosine = 1.0
        + squared
            * (-1.0 / 2.0
                + squared
                    * (1.0 / 24.0
                        + squared
                            * (-1.0 / 720.0
                                + squared
                                    * (1.0 / 40320.0
                                        + squared
                                            * (-1.0 / 3628800.0
                                                + squared * (1.0 / 479001600.0))))));
    (sine, cosine * cosine_sign)
}

fn yaw_pitch_roll([yaw, pitch, roll]: [f32; 3]) -> [f32; 16] {
    let (sy, cy) = polynomial_sin_cos(yaw * 0.5);
    let (sp, cp) = polynomial_sin_cos(pitch * 0.5);
    let (sr, cr) = polynomial_sin_cos(roll * 0.5);
    let x = cy * sp * cr + sy * cp * sr;
    let y = sy * cp * cr - cy * sp * sr;
    let z = cy * cp * sr - sy * sp * cr;
    let w = cy * cp * cr + sy * sp * sr;
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        0.0,
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        0.0,
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

#[cfg(target_arch = "x86_64")]
fn reciprocal_estimate(value: f32) -> f32 {
    use std::arch::x86_64::{_mm_cvtss_f32, _mm_rcp_ss, _mm_set_ss};
    // SSE/SSE2 are part of the x86-64 baseline. No foreign engine code is called.
    unsafe { _mm_cvtss_f32(_mm_rcp_ss(_mm_set_ss(value))) }
}

#[cfg(target_arch = "x86_64")]
fn reciprocal_sqrt_estimate(value: f32) -> f32 {
    use std::arch::x86_64::{_mm_cvtss_f32, _mm_rsqrt_ss, _mm_set_ss};
    unsafe { _mm_cvtss_f32(_mm_rsqrt_ss(_mm_set_ss(value))) }
}

#[cfg(target_arch = "x86")]
fn reciprocal_estimate(value: f32) -> f32 {
    if std::is_x86_feature_detected!("sse") {
        use std::arch::x86::{_mm_cvtss_f32, _mm_rcp_ss, _mm_set_ss};
        unsafe { _mm_cvtss_f32(_mm_rcp_ss(_mm_set_ss(value))) }
    } else {
        portable_estimate(1.0 / value)
    }
}

#[cfg(target_arch = "x86")]
fn reciprocal_sqrt_estimate(value: f32) -> f32 {
    if std::is_x86_feature_detected!("sse") {
        use std::arch::x86::{_mm_cvtss_f32, _mm_rsqrt_ss, _mm_set_ss};
        unsafe { _mm_cvtss_f32(_mm_rsqrt_ss(_mm_set_ss(value))) }
    } else {
        portable_estimate(1.0 / value.sqrt())
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn reciprocal_estimate(value: f32) -> f32 {
    portable_estimate(1.0 / value)
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn reciprocal_sqrt_estimate(value: f32) -> f32 {
    portable_estimate(1.0 / value.sqrt())
}

#[cfg(not(target_arch = "x86_64"))]
fn portable_estimate(value: f32) -> f32 {
    if !value.is_normal() {
        return value;
    }
    let bits = value.to_bits();
    let sign = bits & 0x8000_0000;
    let magnitude = bits & 0x7fff_ffff;
    let rounded = magnitude & !0x7ff;
    let estimate = if rounded == magnitude {
        rounded.saturating_sub(0x1000)
    } else {
        rounded
    };
    f32::from_bits(sign | estimate)
}

#[derive(Clone)]
pub(super) struct MotionRandom {
    integer: MtStream,
    floating: MtStream,
}

impl Default for MotionRandom {
    fn default() -> Self {
        Self::seeded(4357)
    }
}

impl MotionRandom {
    pub(super) fn seeded(seed: u32) -> Self {
        Self {
            integer: MtStream::seeded(seed),
            floating: MtStream::seeded(seed),
        }
    }

    pub(super) fn next_integer(&mut self) -> u32 {
        self.integer.sample()
    }

    pub(super) fn next_float_bits(&mut self) -> u32 {
        ((f64::from(self.floating.sample()) * (1.0 / 4_294_967_295.0)) as f32).to_bits()
    }
}

#[derive(Clone)]
struct MtStream {
    state: [u32; 624],
    cursor: usize,
}

impl MtStream {
    fn seeded(seed: u32) -> Self {
        let mut stream = Self {
            state: [0; 624],
            cursor: 624,
        };
        let mut accumulator = seed;
        for position in 0..624 {
            let upper = accumulator & 0xffff_0000;
            accumulator = accumulator.wrapping_mul(69_069).wrapping_add(1);
            stream.state[position] = upper | (accumulator >> 16);
            accumulator = accumulator.wrapping_mul(69_069).wrapping_add(1);
        }
        stream
    }

    fn sample(&mut self) -> u32 {
        if self.cursor == 624 {
            for position in 0..624 {
                let next = if position == 623 { 0 } else { position + 1 };
                let distant = if position < 227 {
                    position + 397
                } else {
                    position - 227
                };
                let combined =
                    (self.state[position] & 0x8000_0000) | (self.state[next] & 0x7fff_ffff);
                self.state[position] = self.state[distant]
                    ^ (combined >> 1)
                    ^ [0, 0x9908_b0df][(combined & 1) as usize];
            }
            self.cursor = 0;
        }
        let mut output = self.state[self.cursor];
        self.cursor += 1;
        output ^= output >> 11;
        output ^= (output << 7) & 0x9d2c_5680;
        output ^= (output << 15) & 0xefc6_0000;
        output ^ (output >> 18)
    }
}

fn expand_format(memory: &MotionMemory, vm: &Vm, address: u32, depth: usize) -> Result<Vec<u8>> {
    if depth >= 32 {
        return Err(invalid("motion debug format recursion exceeds 32 levels"));
    }
    let input = memory.string(vm, address)?;
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        if input[cursor] != b'%' {
            output.push(input[cursor]);
            cursor += 1;
            continue;
        }
        cursor += 1;
        if input.get(cursor) == Some(&b'%') {
            output.push(b'%');
            cursor += 1;
            continue;
        }
        let zero_pad = input.get(cursor) == Some(&b'0');
        let width = format_number(&input, &mut cursor)?.unwrap_or(0);
        let precision = if input.get(cursor) == Some(&b'.') {
            cursor += 1;
            Some(
                format_number(&input, &mut cursor)?
                    .ok_or_else(|| invalid("motion debug format precision is missing"))?,
            )
        } else {
            None
        };
        let kind = *input
            .get(cursor)
            .ok_or_else(|| invalid("incomplete motion debug format"))?;
        cursor += 1;
        if input.get(cursor) != Some(&b'[') {
            return Err(invalid("motion debug format requires a G/L/H address"));
        }
        cursor += 1;
        let region = *input
            .get(cursor)
            .ok_or_else(|| invalid("motion debug address has no region"))?;
        cursor += 1;
        let negative = input.get(cursor) == Some(&b'-');
        if negative || input.get(cursor) == Some(&b'+') {
            cursor += 1;
        }
        let displacement = format_number(&input, &mut cursor)?
            .ok_or_else(|| invalid("motion debug address has no byte offset"))?;
        if input.get(cursor) != Some(&b']') {
            return Err(invalid("motion debug format address is not terminated"));
        }
        cursor += 1;
        let base = match region {
            b'G' => 0,
            b'L' => vm.frame_base() as u32,
            b'H' => EXTERNAL,
            _ => return Err(invalid("motion debug format address region is invalid")),
        };
        let address = if negative {
            base.checked_sub(displacement as u32)
        } else {
            base.checked_add(displacement as u32)
        }
        .ok_or_else(|| invalid("motion debug format address overflows"))?;
        let mut formatted = match kind {
            b's' => {
                let mut string = expand_format(memory, vm, address, depth + 1)?;
                if let Some(precision) = precision {
                    string.truncate(precision);
                }
                string
            }
            b'c' => {
                if precision.is_some() {
                    return Err(invalid("motion character precision is invalid"));
                }
                vec![word_at(&memory.read(vm, address, 4)?, 0) as u8]
            }
            b'd' | b'i' | b'u' | b'x' | b'X' => {
                if precision.is_some() {
                    return Err(invalid("motion integer precision is unsupported"));
                }
                let word = word_at(&memory.read(vm, address, 4)?, 0);
                match kind {
                    b'd' | b'i' => (word as i32).to_string(),
                    b'u' => word.to_string(),
                    b'x' => format!("{word:x}"),
                    _ => format!("{word:X}"),
                }
                .into_bytes()
            }
            b'f' => {
                let value = memory.float(vm, address)?;
                let text = if value.is_nan() {
                    if value.to_bits() == 0xffc0_0000 {
                        "-nan(ind)".to_string()
                    } else if value.is_sign_negative() {
                        "-nan".to_string()
                    } else {
                        "nan".to_string()
                    }
                } else if value == f32::INFINITY {
                    "inf".to_string()
                } else if value == f32::NEG_INFINITY {
                    "-inf".to_string()
                } else {
                    format!("{:.*}", precision.unwrap_or(6), f64::from(value))
                };
                text.into_bytes()
            }
            _ => return Err(invalid("motion debug format conversion is unsupported")),
        };
        let padding = width.saturating_sub(formatted.len());
        if output
            .len()
            .checked_add(padding)
            .and_then(|n| n.checked_add(formatted.len()))
            .is_none_or(|n| n > LIMIT)
        {
            return Err(invalid("motion debug format exceeds storage limit"));
        }
        if padding != 0 {
            let fill = if zero_pad && kind != b's' && kind != b'c' {
                b'0'
            } else {
                b' '
            };
            if fill == b'0' && formatted.first() == Some(&b'-') {
                output.push(b'-');
                formatted.remove(0);
            }
            output.resize(output.len() + padding, fill);
        }
        output.extend_from_slice(&formatted);
        if let Some(end) = output.iter().position(|&byte| byte == 0) {
            output.truncate(end);
            break;
        }
    }
    Ok(output)
}

fn format_number(input: &[u8], cursor: &mut usize) -> Result<Option<usize>> {
    let first = *cursor;
    let mut value = 0usize;
    while let Some(&byte) = input.get(*cursor) {
        if !byte.is_ascii_digit() {
            break;
        }
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add((byte - b'0') as usize))
            .filter(|&v| v <= LIMIT)
            .ok_or_else(|| invalid("motion format numeric field exceeds storage limit"))?;
        *cursor += 1;
    }
    Ok((*cursor != first).then_some(value))
}
