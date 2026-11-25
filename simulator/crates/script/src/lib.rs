pub mod anm;
pub mod cstl;
pub mod cst_history;
pub mod cst_snapshot;
pub mod data_snapshot;
pub mod shared_numeric_snapshot;
pub mod expression;
pub mod message;
pub mod scene;
mod vm_snapshot;
mod vm_identity;
mod lookahead;
pub use lookahead::StaticNativeCall;
mod sha256;
pub use sha256::sha256;
pub use vm_snapshot::{VmExecutionSnapshot, VmSnapshotGroup};

// Container validation and VM decoding must use one instruction-length table.
pub use cs2_formats::kcs::operand_count;
pub use anm::{
    AnmError, AnmInterpreter, AnmResult, DisplayState, ImageAnimation, RandomSource, Timeline,
};
pub use cstl::{CstlError, CstlLanguageSelection, CstlLocalization, CstlMessage, CstlTable};
pub use expression::{
    Expression, ExpressionState, eval_expression, eval_expression_fallback,
    eval_expression_parameter, eval_kcs_expression_parameter,
};
pub use message::{
    MessageGlyph, MessageLayout, MessageLayoutSettings, MessageReveal, MessageRevealUpdate,
    MessageSchedule, MessageTimingSettings,
};
pub use scene::{
    CstChoice, CstEvent, CstInterpreter, CstSceneTransfer, CstStep, CstTextEncoding, CstTextToken,
    FesBlock, FesEvent, FesExecutionContext, FesInitializationPlan, FesInterpreter, FesKeyBlock,
    FesKeyBlockBinding, FesKeyBlockTable, FesObjectDefinition, FesObjectInitialization, FesProgram,
    FesStep, FesSubcommand, FesSubcommandArgument, FesSubcommandParameter, FesSubcommandTable,
    parse_cst_text, parse_cst_text_with_encoding,
};

use flate2::{Decompress, FlushDecompress, Status};
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{Arc, OnceLock, RwLock, RwLockReadGuard},
};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ScriptError>;

const KCS_HEADER_LENGTH: usize = 52;
const MAX_SCRIPT_BYTES: usize = 256 * 1024 * 1024;
const MAX_SCRIPT_ITEMS: usize = 1_000_000;
const EXTERNAL_ADDRESS_BIT: u32 = 0x8000_0000;
const NATIVE_CALL_OPCODE: u8 = 0x2b;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ScriptError {
    #[error("truncated data at {offset:#x}, need {needed} bytes")]
    Truncated { offset: usize, needed: usize },
    #[error("invalid {kind} signature")]
    InvalidSignature { kind: &'static str },
    #[error("invalid {kind} header: {message}")]
    InvalidHeader {
        kind: &'static str,
        message: &'static str,
    },
    #[error("invalid compressed {kind} body: {message}")]
    InvalidCompression { kind: &'static str, message: String },
    #[error("invalid KCS opcode {opcode:#04x} at {offset:#x}")]
    InvalidOpcode { offset: usize, opcode: u8 },
    #[error("invalid KCS instruction at {offset:#x}")]
    InvalidInstruction { offset: usize },
    #[error("invalid KCS jump target {target:#x}")]
    InvalidJump { target: usize },
    #[error("invalid KCS type flags {flags:#04x}")]
    InvalidTypeFlags { flags: u8 },
    #[error("KCS memory access out of bounds at {address:#x} for {length} bytes")]
    MemoryOutOfBounds { address: u32, length: usize },
    #[error("invalid KCS memory region at {base:#x} for {length} bytes: {message}")]
    InvalidMemoryRegion {
        base: u32,
        length: usize,
        message: &'static str,
    },
    #[error("KCS shared external storage lock was poisoned")]
    ExternalStoragePoisoned,
    #[error("KCS stack underflow")]
    StackUnderflow,
    #[error("KCS stack overflow")]
    StackOverflow,
    #[error("KCS call parameter underflow")]
    CallParameterUnderflow,
    #[error("KCS invalid call frame")]
    InvalidCallFrame,
    #[error(
        "KCS call frame mismatch: {reason}, frame_base={frame_base}, frame_top={frame_top}, parameter_bytes={parameter_bytes}, result_bytes={result_bytes}, saved={saved}, expected_base={expected_base}, saved_base={saved_base}, expected_parameters={expected_parameters}"
    )]
    CallFrameMismatch {
        reason: &'static str,
        frame_base: usize,
        frame_top: usize,
        parameter_bytes: usize,
        result_bytes: usize,
        saved: usize,
        expected_base: usize,
        saved_base: usize,
        expected_parameters: usize,
    },
    #[error("KCS return value underflow")]
    ReturnValueUnderflow,
    #[error("KCS local frame is invalid")]
    InvalidLocalFrame,
    #[error("KCS signed integer arithmetic overflow")]
    IntegerOverflow,
    #[error("KCS opcode {opcode:#04x} at {offset:#x} is not implemented")]
    UnimplementedOpcode { offset: usize, opcode: u8 },
    #[error("KCS native call {index} is not implemented")]
    UnimplementedNativeCall { index: u32 },
    #[error("KCS opcode {opcode:#04x} has no verified behavior at {offset:#x}")]
    UnsupportedOpcode { offset: usize, opcode: u8 },
    #[error("KCS program is not loaded")]
    NoProgram,
    #[error("invalid structured script data at {offset:#x}: {message}")]
    InvalidStructuredText {
        offset: usize,
        message: &'static str,
    },
    #[error("invalid script expression at {offset:#x}: {message}")]
    InvalidExpression {
        offset: usize,
        message: String,
    },
    #[error("invalid scene control at {offset:#x}: {message}")]
    InvalidSceneControl {
        offset: usize,
        message: &'static str,
    },
    #[error("host call failed: {message}")]
    HostCallFailed { message: String },
    #[error("host call yielded")]
    HostYield { event: ExecutionEvent },
}

impl ScriptError {
    pub fn event(&self) -> Option<ExecutionEvent> {
        match self {
            Self::HostYield { event } => Some(*event),
            _ => None,
        }
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let raw = bytes
        .get(offset..offset.saturating_add(4))
        .filter(|raw| raw.len() == 4)
        .ok_or(ScriptError::Truncated { offset, needed: 4 })?;
    Ok(u32::from_le_bytes(raw.try_into().unwrap()))
}

fn read_range(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    bytes
        .get(offset..offset.saturating_add(length))
        .filter(|range| range.len() == length)
        .ok_or(ScriptError::Truncated {
            offset,
            needed: length,
        })
}

fn read_zstring(bytes: &[u8], offset: usize) -> Result<&[u8]> {
    let bytes = bytes
        .get(offset..)
        .ok_or(ScriptError::Truncated { offset, needed: 1 })?;
    let length =
        bytes
            .iter()
            .position(|&byte| byte == 0)
            .ok_or(ScriptError::InvalidStructuredText {
                offset,
                message: "unterminated string",
            })?;
    Ok(&bytes[..length])
}

fn inflate_exact(kind: &'static str, compressed: &[u8], expected: usize) -> Result<Vec<u8>> {
    if expected > MAX_SCRIPT_BYTES {
        return Err(ScriptError::InvalidHeader {
            kind,
            message: "decompressed body exceeds size limit",
        });
    }
    // One spare output byte distinguishes an exact stream from an oversized
    // body. Reading only `expected` bytes could miss a truncated checksum.
    let mut output = vec![0; expected + 1];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(compressed, &mut output, FlushDecompress::Finish)
        .map_err(|error| ScriptError::InvalidCompression {
            kind,
            message: error.to_string(),
        })?;
    if status != Status::StreamEnd
        || decoder.total_in() != compressed.len() as u64
        || decoder.total_out() != expected as u64
    {
        return Err(ScriptError::InvalidCompression {
            kind,
            message: format!(
                "stream {status:?}, consumed {}/{}, decoded {}/{expected}",
                decoder.total_in(), compressed.len(), decoder.total_out(),
            ),
        });
    }
    output.truncate(expected);
    Ok(output)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeFlags(u8);

impl TypeFlags {
    pub const BYTE: Self = Self(1);
    pub const WORD: Self = Self(2);
    pub const DWORD: Self = Self(3);
    pub const FLOAT: Self = Self(4);
    pub const SIGNED: u8 = 0x20;
    pub const FROM_STACK: u8 = 0x40;
    pub const ADDRESS: u8 = 0x10;
    pub const DEREFERENCE_DWORD: u8 = 0x80;

    pub const fn new(flags: u8) -> Self {
        Self(flags)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn scalar(self) -> u8 {
        self.0 & 0x0f
    }

    pub const fn is_signed(self) -> bool {
        self.0 & Self::SIGNED != 0
    }

    pub const fn from_stack(self) -> bool {
        self.0 & Self::FROM_STACK != 0
    }

    pub const fn is_address(self) -> bool {
        self.0 & Self::ADDRESS != 0
    }

    pub const fn is_float(self) -> bool {
        // Arithmetic/comparison dispatch tests bit 0x04. Width decoding is
        // separate and still validates the complete scalar nibble.
        self.0 & Self::FLOAT.0 != 0
    }

    pub const fn dereferences_dword(self) -> bool {
        self.0 & Self::DEREFERENCE_DWORD != 0
    }

    pub fn width(self) -> Result<usize> {
        if self.dereferences_dword() {
            return Ok(4);
        }
        match self.scalar() {
            1 => Ok(1),
            2 => Ok(2),
            3 | 4 => Ok(4),
            _ => Err(ScriptError::InvalidTypeFlags { flags: self.0 }),
        }
    }
}

impl From<u8> for TypeFlags {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operand {
    pub flags: TypeFlags,
    pub payload: u32,
}

impl Operand {
    pub const fn new(flags: u8, payload: u32) -> Self {
        Self {
            flags: TypeFlags::new(flags),
            payload,
        }
    }

    pub const fn immediate(payload: u32) -> Self {
        Self::new(3, payload)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub offset: usize,
    pub opcode: u8,
    pub operands: Vec<Operand>,
}

/// The executable opcode set established by the 2.6.1.90 reference corpus.
pub const ACTUAL_OPCODES: &[u8] = &[
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
    0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x41,
    0x42, 0x43, 0x44,
];

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KcsHeader {
    pub format_version: u32,
    pub program_version: u32,
    pub header_length: u32,
    pub code_length: u32,
    pub string_init_offset: u32,
    pub string_init_count: u32,
    pub string_init_length: u32,
    pub data_length: u32,
    pub stack_length: u32,
    pub data_offset: u32,
    pub reserved: u32,
    pub decompressed_capacity: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KcsProgram {
    pub header: KcsHeader,
    pub code: Vec<u8>,
    pub data: Vec<u8>,
    pub instructions: Vec<Instruction>,
    pub stack_size: usize,
    instruction_pages: Vec<InstructionPage>,
}

const INSTRUCTION_PAGE_BYTES: usize = u64::BITS as usize;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct InstructionPage {
    starts: u64,
    first: u32,
}

impl KcsProgram {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < KCS_HEADER_LENGTH {
            return Err(ScriptError::Truncated {
                offset: 0,
                needed: KCS_HEADER_LENGTH,
            });
        }
        if bytes.len() > MAX_SCRIPT_BYTES {
            return Err(ScriptError::InvalidHeader {
                kind: "KCS",
                message: "container exceeds size limit",
            });
        }
        if bytes.get(..4) != Some(b"KCS\0") {
            return Err(ScriptError::InvalidSignature { kind: "KCS" });
        }
        let header = KcsHeader {
            format_version: read_u32(bytes, 4)?,
            program_version: read_u32(bytes, 8)?,
            header_length: read_u32(bytes, 12)?,
            code_length: read_u32(bytes, 16)?,
            string_init_offset: read_u32(bytes, 20)?,
            string_init_count: read_u32(bytes, 24)?,
            string_init_length: read_u32(bytes, 28)?,
            data_length: read_u32(bytes, 32)?,
            stack_length: read_u32(bytes, 36)?,
            data_offset: read_u32(bytes, 40)?,
            reserved: read_u32(bytes, 44)?,
            decompressed_capacity: read_u32(bytes, 48)?,
        };
        if header.format_version != 2 || header.header_length as usize != KCS_HEADER_LENGTH {
            return Err(ScriptError::InvalidHeader {
                kind: "KCS",
                message: "unsupported format version or header length",
            });
        }
        let code_length = header.code_length as usize;
        let init_length = header.string_init_length as usize;
        let body_length =
            code_length
                .checked_add(init_length)
                .ok_or(ScriptError::InvalidHeader {
                    kind: "KCS",
                    message: "body length overflows",
                })?;
        if header.string_init_offset as usize != KCS_HEADER_LENGTH + code_length
            || header.data_offset as usize != KCS_HEADER_LENGTH + body_length
            || body_length > header.decompressed_capacity as usize
            || header.string_init_count as usize > init_length / 8
            || header.string_init_count as usize > MAX_SCRIPT_ITEMS
            || (header.data_length as usize)
                .checked_add(header.stack_length as usize)
                .is_none_or(|length| length > MAX_SCRIPT_BYTES)
        {
            return Err(ScriptError::InvalidHeader {
                kind: "KCS",
                message: "body offsets or sizes are inconsistent",
            });
        }
        let body = inflate_exact("KCS", &bytes[KCS_HEADER_LENGTH..], body_length)?;
        let code = body[..code_length].to_vec();
        let init = &body[code_length..];
        let mut data = vec![0; header.data_length as usize];
        for index in 0..header.string_init_count as usize {
            let record = index * 8;
            let source = read_u32(init, record)? as usize;
            let target = read_u32(init, record + 4)? as usize;
            if source < header.string_init_count as usize * 8 {
                return Err(ScriptError::InvalidHeader {
                    kind: "KCS",
                    message: "string initializer points into its table",
                });
            }
            let string = read_zstring(init, source).map_err(|_| ScriptError::InvalidHeader {
                kind: "KCS",
                message: "string initializer is not terminated",
            })?;
            let end = target
                .checked_add(string.len() + 1)
                .ok_or(ScriptError::InvalidHeader {
                    kind: "KCS",
                    message: "string initializer target overflows",
                })?;
            if end > data.len() {
                return Err(ScriptError::InvalidHeader {
                    kind: "KCS",
                    message: "string initializer exceeds data area",
                });
            }
            data[target..target + string.len()].copy_from_slice(string);
            data[target + string.len()] = 0;
        }
        Self::from_code_with_header(code, data, header.stack_length as usize, header)
    }

    pub fn from_code(code: &[u8], data: Vec<u8>, stack_size: usize) -> Result<Self> {
        if code.len() > MAX_SCRIPT_BYTES
            || data
                .len()
                .checked_add(stack_size)
                .is_none_or(|length| length > MAX_SCRIPT_BYTES)
        {
            return Err(ScriptError::InvalidHeader {
                kind: "KCS",
                message: "program storage exceeds size limit",
            });
        }
        let header = KcsHeader {
            format_version: 2,
            program_version: 0,
            header_length: KCS_HEADER_LENGTH as u32,
            code_length: code.len() as u32,
            string_init_offset: (KCS_HEADER_LENGTH + code.len()) as u32,
            string_init_count: 0,
            string_init_length: 0,
            data_length: data.len() as u32,
            stack_length: stack_size as u32,
            data_offset: (KCS_HEADER_LENGTH + code.len()) as u32,
            reserved: 0,
            decompressed_capacity: code.len() as u32,
        };
        Self::from_code_with_header(code.to_vec(), data, stack_size, header)
    }

    fn from_code_with_header(
        mut code: Vec<u8>,
        data: Vec<u8>,
        stack_size: usize,
        header: KcsHeader,
    ) -> Result<Self> {
        let mut instructions = decode_instructions(&code)?;
        widen_legacy_cst_command_buffer(&mut code, &mut instructions, stack_size);
        let mut instruction_pages =
            vec![InstructionPage::default(); code.len().div_ceil(INSTRUCTION_PAGE_BYTES)];
        for (index, instruction) in instructions.iter().enumerate() {
            let page = &mut instruction_pages[instruction.offset / INSTRUCTION_PAGE_BYTES];
            if page.starts == 0 {
                page.first = index as u32;
            }
            page.starts |= 1 << (instruction.offset % INSTRUCTION_PAGE_BYTES);
        }
        data.len()
            .checked_add(stack_size)
            .ok_or(ScriptError::StackOverflow)?;
        let program = Self { header, code, data, instructions, stack_size, instruction_pages };
        for instruction in &program.instructions {
            let target_index = match instruction.opcode {
                0x27..=0x2a => Some(0),
                0x34 => Some(1),
                0x35 => Some(2),
                _ => None,
            };
            if let Some(index) = target_index {
                let operand = instruction.operands[index];
                if operand.flags.bits() == 3
                    && !program.is_instruction_boundary(operand.payload as usize)
                {
                    return Err(ScriptError::InvalidJump {
                        target: operand.payload as usize,
                    });
                }
            }
        }
        Ok(program)
    }

    pub fn is_instruction_boundary(&self, offset: usize) -> bool {
        offset == self.code.len() || self.instruction_index(offset).is_some()
    }

    pub fn instruction_at(&self, offset: usize) -> Option<&Instruction> {
        self.instruction_index(offset).and_then(|index| self.instructions.get(index))
    }

    fn instruction_index(&self, offset: usize) -> Option<usize> {
        let page = self.instruction_pages.get(offset / INSTRUCTION_PAGE_BYTES)?;
        let bit = 1u64 << (offset % INSTRUCTION_PAGE_BYTES);
        if page.starts & bit == 0 {
            return None;
        }
        // The number of earlier starts in this page is the decoded index's
        // distance from its first instruction.
        Some(page.first as usize + (page.starts & (bit - 1)).count_ones() as usize)
    }
}

fn widen_legacy_cst_command_buffer(
    code: &mut [u8],
    instructions: &mut [Instruction],
    stack_size: usize,
) {
    const COMMAND_CAPACITY: u32 = 0x1000;
    if stack_size < COMMAND_CAPACITY as usize * 2 {
        return;
    }
    let imm = Operand::immediate;
    let is = |instruction: &Instruction, opcode, operands: &[Operand]| {
        instruction.opcode == opcode && instruction.operands == operands
    };
    for start in 1..instructions.len() {
        if !is(&instructions[start - 1], 0x31, &[imm(4)])
            || !is(&instructions[start], 0x31, &[imm(0x80)])
        {
            continue;
        }
        let tail = &instructions[start + 1..];
        let Some(end) = tail.iter().position(|i| matches!(i.opcode, 0x2c | 0x31 | 0x32))
        else { continue };
        if tail[end].opcode != 0x2c {
            continue;
        }
        let body = &tail[..end];
        let Some(case) = body.iter().find(|i| {
            i.opcode == 0x34 && i.operands[0] == imm(0x30) && i.operands[1].flags == TypeFlags::DWORD
        }) else { continue };
        let Ok(branch) = body.binary_search_by_key(&(case.operands[1].payload as usize), |i| i.offset)
        else { continue };
        let Some(read) = body.get(branch..branch + 19) else { continue };
        let context = read[0].operands.first().copied().unwrap_or(imm(0));
        let is_context_field = |instruction, offset| {
            is(instruction, 0x3e, &[context, Operand::new(0x23, offset)])
        };
        let local = imm(0x14);
        if context.flags != TypeFlags::new(0x06)
            || !is(&read[0], 0x3e, &[context, imm(0)])
            || !is(&read[1], 0x25, &[Operand::new(0x53, 0)])
            || !is(&read[2], 0x3d, &[local])
            || !is_context_field(&read[3], 0x24)
            || !is(&read[4], 0x25, &[Operand::new(0x73, 0x24)])
            || !is_context_field(&read[5], 0x28)
            || !is(&read[6], 0x25, &[Operand::new(0x73, 0x28)])
            || !is(&read[7], 0x2b, &[imm(327), imm(16)])
            || !is(&read[8], 0x3c, &[])
            || !is(&read[9], 0x3d, &[local])
            || !is(&read[10], 0x2b, &[imm(278), imm(4)])
            || !is(&read[11], 0x3c, &[])
            || !is(&read[12], 0x3d, &[imm(0)])
            || !is(&read[13], 0x25, &[Operand::new(0xf1, 0)])
            || !is(&read[14], 0x3d, &[local])
            || read[15..17].iter().any(|i| i.opcode != 0x25 || i.operands[0].flags != TypeFlags::DWORD)
            || !is(&read[17], 0x2b, &[imm(45), imm(16)])
            || !is(&read[18], 0x3c, &[])
        {
            continue;
        }
        // The command read and in-place expansion share the last frame local.
        // Reserve the legacy global command buffer's size so argument pushes
        // and subsequent calls cannot overwrite the temporary string.
        let instruction = &mut instructions[start];
        instruction.operands[0] = imm(COMMAND_CAPACITY);
        let operand = instruction.offset + 2;
        code[operand..operand + 4].copy_from_slice(&COMMAND_CAPACITY.to_le_bytes());
    }
}

fn decode_instructions(code: &[u8]) -> Result<Vec<Instruction>> {
    let mut instructions = Vec::new();
    let mut cursor = 0;
    while cursor < code.len() {
        let offset = cursor;
        let opcode = code[cursor];
        cursor += 1;
        let count = operand_count(opcode).ok_or(ScriptError::InvalidOpcode { offset, opcode })?;
        let mut operands = Vec::with_capacity(count);
        for _ in 0..count {
            let raw = read_range(code, cursor, 5)
                .map_err(|_| ScriptError::InvalidInstruction { offset })?;
            operands.push(Operand::new(
                raw[0],
                u32::from_le_bytes(raw[1..5].try_into().unwrap()),
            ));
            cursor += 5;
        }
        instructions.push(Instruction {
            offset,
            opcode,
            operands,
        });
        if instructions.len() > MAX_SCRIPT_ITEMS {
            return Err(ScriptError::InvalidHeader {
                kind: "KCS",
                message: "instruction count exceeds limit",
            });
        }
    }
    Ok(instructions)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    U32(u32),
    I32(i32),
    F32(f32),
    Address(u32),
}

impl Value {
    pub const fn bits(self) -> u32 {
        match self {
            Self::U32(value) => value,
            Self::I32(value) => value as u32,
            Self::F32(value) => value.to_bits(),
            Self::Address(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallFrame {
    pub return_pc: usize,
    pub base: usize,
    pub top: usize,
    pub parameter_bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DebugState {
    pub line: u32,
    pub source: Vec<u8>,
    pub scopes: Vec<u32>,
    pub variables: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeReturn {
    Yield,
    Value(u32),
    Continue,
    YieldValue(u32),
    YieldVoid,
    Retry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionEvent {
    pub native_index: u32,
    pub status: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Yield,
    Halted,
    Event(ExecutionEvent),
}

/// Ordinary-task completion is distinct from the low-level fetch-loop result.
/// In particular, reaching the end during a call is active until the next
/// update, while an explicit 3B stops immediately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeStep {
    /// Instruction budget or an unfinished native Retry. The same invocation
    /// (including its message, if any) must resume before any fresh entry.
    Yield,
    Paused,
    Completed {
        active: bool,
        event: Option<ExecutionEvent>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ExecutionControl {
    mode: u32,
    call_return: Option<usize>,
    receiver: Option<u32>,
    waiting: bool,
    message: Option<[u32; 4]>,
    message_started_at_end: bool,
    resume_started_at_end: Option<bool>,
}

impl Default for ExecutionControl {
    fn default() -> Self {
        Self {
            mode: 1,
            call_return: None,
            receiver: None,
            waiting: false,
            message: None,
            message_started_at_end: false,
            resume_started_at_end: None,
        }
    }
}

pub trait Host {
    fn call(&mut self, index: u32, arguments: &[u8], vm: &mut Vm) -> Result<NativeReturn>;
    /// A cooperative host-iteration budget. Default hosts remain deterministic.
    fn should_yield(&self) -> bool { false }

    /// Renew an exhausted instruction slice while the host deadline permits.
    /// Native retry and wait events return to the caller before this boundary.
    fn renew_instruction_budget(&self, _budget: usize) -> bool { false }
}

impl<F> Host for F
where
    F: FnMut(u32, &[u8], &mut Vm) -> Result<NativeReturn>,
{
    fn call(&mut self, index: u32, arguments: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        self(index, arguments, vm)
    }
}

#[derive(Default)]
pub struct NullHost;

impl Host for NullHost {
    fn call(&mut self, index: u32, _arguments: &[u8], _vm: &mut Vm) -> Result<NativeReturn> {
        Err(ScriptError::UnimplementedNativeCall { index })
    }
}

/// An owned copy of one independent low-address allocation, excluding the
/// program's original data/stack memory and the high-bit external domain.
///
/// Persist both fields: pointers in VM memory may refer to this exact base.
/// This is a storage snapshot, not a complete execution/continuation snapshot.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VmRegionSnapshot {
    pub base: u32,
    pub bytes: Vec<u8>,
}

/// Shared external bytes. Cloning preserves identity; `snapshot` makes an
/// independent copy for persistence. Locks never escape a VM read/write.
#[derive(Clone, Debug)]
pub struct SharedExternalStorage {
    bytes: Arc<RwLock<Vec<u8>>>,
}

impl Default for SharedExternalStorage {
    fn default() -> Self {
        Self { bytes: Arc::new(RwLock::new(Vec::new())) }
    }
}

impl SharedExternalStorage {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() > MAX_SCRIPT_BYTES {
            return Err(ScriptError::InvalidHeader {
                kind: "KCS",
                message: "external storage exceeds size limit",
            });
        }
        Ok(Self { bytes: Arc::new(RwLock::new(bytes)) })
    }

    /// Snapshotting may be done while VMs share the binding. A coherent
    /// multi-VM save still requires the host to suspend the VM group.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        Ok(self.read_guard()?.clone())
    }

    pub fn len(&self) -> Result<usize> {
        Ok(self.read_guard()?.len())
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.read_guard()?.is_empty())
    }

    pub fn same_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.bytes, &other.bytes)
    }

    fn read_guard(&self) -> Result<RwLockReadGuard<'_, Vec<u8>>> {
        self.bytes.read().map_err(|_| ScriptError::ExternalStoragePoisoned)
    }
}

#[derive(Clone, Copy)]
enum VmStorage {
    Local,
    External,
    Region(u32),
}

#[derive(Default)]
pub struct Vm {
    native_context_identity: Option<u32>,
    pub stack: Vec<Value>,
    pub instruction_pointer: usize,
    program: Option<Arc<KcsProgram>>,
    program_code_sha256: OnceLock<String>,
    memory: Vec<u8>,
    external: SharedExternalStorage,
    regions: BTreeMap<u32, Vec<u8>>,
    region_bytes: usize,
    stack_start: usize,
    sp: usize,
    frame_top: usize,
    frame_base: usize,
    frame_floors: Vec<usize>,
    debug_variable_floors: Vec<usize>,
    frames: Vec<CallFrame>,
    switch_value: u32,
    debug: DebugState,
    native_arguments: Option<(usize, usize)>,
    control: ExecutionControl,
}

impl Vm {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_program(program: KcsProgram) -> Result<Self> {
        let mut vm = Self::new();
        vm.load_program(program, Vec::new())?;
        Ok(vm)
    }

    pub fn from_kcs(bytes: &[u8]) -> Result<Self> {
        Self::from_program(KcsProgram::parse(bytes)?)
    }

    pub fn load_program(&mut self, program: KcsProgram, external: Vec<u8>) -> Result<()> {
        let external = SharedExternalStorage::new(external)?;
        self.load_program_with_shared_external(program, external)
    }

    /// Loads a program against an existing shared binding without copying its
    /// bytes. As with `load_program`, clears independent allocation regions.
    pub fn load_program_with_shared_external(
        &mut self,
        program: KcsProgram,
        external: SharedExternalStorage,
    ) -> Result<()> {
        self.initialize_program(Arc::new(program), external)
    }

    fn initialize_program(
        &mut self,
        program: Arc<KcsProgram>,
        external: SharedExternalStorage,
    ) -> Result<()> {
        external.len()?;
        let stack_start = program.data.len();
        let memory_length = stack_start
            .checked_add(program.stack_size)
            .ok_or(ScriptError::StackOverflow)?;
        if memory_length > MAX_SCRIPT_BYTES {
            return Err(ScriptError::InvalidHeader {
                kind: "KCS",
                message: "program storage exceeds size limit",
            });
        }
        self.memory = program.data.clone();
        self.memory.resize(memory_length, 0);
        self.external = external;
        self.regions.clear();
        self.region_bytes = 0;
        self.program = Some(program);
        self.program_code_sha256 = OnceLock::new();
        self.stack_start = stack_start;
        self.sp = stack_start;
        self.frame_top = stack_start;
        self.frame_base = stack_start;
        self.frame_floors.clear();
        self.frame_floors.push(stack_start);
        self.debug_variable_floors.clear();
        self.frames.clear();
        self.stack.clear();
        self.instruction_pointer = 0;
        self.switch_value = 0;
        self.debug = DebugState::default();
        self.native_arguments = None;
        self.control = ExecutionControl::default();
        Ok(())
    }

    pub fn program(&self) -> Option<&KcsProgram> {
        self.program.as_deref()
    }

    pub fn debug(&self) -> &DebugState {
        &self.debug
    }

    pub fn frames(&self) -> &[CallFrame] {
        &self.frames
    }

    /// Copies only the original D+S bytes, preserving the existing save API.
    /// Independent blocks must also be captured with [`Self::region_snapshots`];
    /// external storage remains available separately via [`Self::external_bytes`].
    pub fn memory_snapshot(&self) -> Option<Vec<u8>> {
        self.program.as_ref().map(|_| self.memory.clone())
    }

    /// Copies `bytes` into a new independent low-address region.
    ///
    /// The allocator chooses the first sufficiently large gap at or after
    /// D+S; when D+S is zero it starts at one so zero remains the motion
    /// no-data sentinel. Released gaps can be reused. There is no alignment
    /// rounding, and this deterministic policy is not a claim about the
    /// original engine's heap layout.
    ///
    /// Empty data is rejected: motion hosts should skip allocation and store
    /// a zero address/length pair. Live bytes are limited to 256 MiB in total,
    /// independently of D+S and external storage. Neither the operation stack
    /// nor its capacity changes. `reset_execution` preserves these blocks;
    /// `load_program` and the full `reset` discard them.
    pub fn allocate_region(&mut self, bytes: &[u8]) -> Result<u32> {
        self.program.as_ref().ok_or(ScriptError::NoProgram)?;
        let length = bytes.len();
        if length == 0 || self.regions.len() >= MAX_SCRIPT_ITEMS {
            return Err(ScriptError::InvalidMemoryRegion {
                base: 0,
                length,
                message: "empty allocation or region count limit exceeded",
            });
        }
        let total = self.region_bytes
            .checked_add(length)
            .filter(|&total| total <= MAX_SCRIPT_BYTES)
            .ok_or(ScriptError::InvalidMemoryRegion {
                base: 0,
                length,
                message: "total region storage exceeds size limit",
            })?;
        let mut base = self.memory.len().max(1) as u64;
        for (&next_base, region) in &self.regions {
            if base + length as u64 <= u64::from(next_base) {
                break;
            }
            base = u64::from(next_base) + region.len() as u64;
        }
        let end = base.checked_add(length as u64).ok_or(
            ScriptError::InvalidMemoryRegion {
                base: 0,
                length,
                message: "region address overflows",
            },
        )?;
        if end > u64::from(EXTERNAL_ADDRESS_BIT) {
            return Err(ScriptError::InvalidMemoryRegion {
                base: base as u32,
                length,
                message: "region crosses into the external address domain",
            });
        }
        let base = base as u32;
        self.regions.insert(base, bytes.to_vec());
        self.region_bytes = total;
        Ok(base)
    }

    /// Releases one exact allocation base. Interior addresses, zero, external
    /// addresses and already released blocks return a diagnostic.
    ///
    /// Existing pointers are not rewritten. Subsequent reads/writes through
    /// an unmapped pointer fail; a later allocation may reuse the address.
    pub fn release_region(&mut self, base: u32) -> Result<()> {
        self.program.as_ref().ok_or(ScriptError::NoProgram)?;
        let region = self.regions.remove(&base).ok_or(ScriptError::InvalidMemoryRegion {
            base,
            length: 0,
            message: "address is not a live allocation base",
        })?;
        self.region_bytes -= region.len();
        Ok(())
    }

    /// Captures current bytes in ascending address order. The original D+S
    /// image is intentionally not concatenated with the independent blocks.
    pub fn region_snapshots(&self) -> Vec<VmRegionSnapshot> {
        self.regions.iter().map(|(&base, bytes)| VmRegionSnapshot {
            base,
            bytes: bytes.clone(),
        }).collect()
    }

    /// Atomically replaces all independent blocks at their saved addresses.
    ///
    /// Load the matching program first, then restore D+S, these blocks and
    /// external bytes separately. Validation rejects empty/overlapping blocks,
    /// overlap with D+S, addresses in the external domain, arithmetic overflow,
    /// and storage/count limits. On error the old mapping is unchanged.
    ///
    /// This is a quiescent storage operation; it cannot replace blocks during
    /// an ordinary-task budget continuation or an active native callback.
    /// It neither restores PC/frames nor rebinds external memory.
    pub fn restore_regions(&mut self, snapshots: &[VmRegionSnapshot]) -> Result<()> {
        self.program.as_ref().ok_or(ScriptError::NoProgram)?;
        if self.has_pending_execution() || self.native_arguments.is_some() {
            return Err(ScriptError::InvalidSceneControl {
                offset: self.instruction_pointer,
                message: "cannot restore regions during execution continuation or native call",
            });
        }
        if snapshots.len() > MAX_SCRIPT_ITEMS {
            return Err(ScriptError::InvalidMemoryRegion {
                base: 0,
                length: 0,
                message: "region count exceeds limit",
            });
        }
        let mut ordered = snapshots.iter().collect::<Vec<_>>();
        ordered.sort_unstable_by_key(|region| region.base);
        let mut previous_end = self.memory.len().max(1) as u64;
        let mut total = 0usize;
        for region in &ordered {
            let length = region.bytes.len();
            let invalid = |message| ScriptError::InvalidMemoryRegion {
                base: region.base,
                length,
                message,
            };
            if length == 0 {
                return Err(invalid("empty allocation"));
            }
            if region.base & EXTERNAL_ADDRESS_BIT != 0 {
                return Err(invalid("region base is in the external address domain"));
            }
            let base = u64::from(region.base);
            if base < previous_end {
                return Err(invalid("region overlaps D+S or another allocation"));
            }
            total = total.checked_add(length)
                .filter(|&total| total <= MAX_SCRIPT_BYTES)
                .ok_or_else(|| invalid("total region storage exceeds size limit"))?;
            let end = base.checked_add(length as u64)
                .filter(|&end| end <= u64::from(EXTERNAL_ADDRESS_BIT))
                .ok_or_else(|| invalid("region end overflows the low address domain"))?;
            previous_end = end;
        }
        let regions = ordered.into_iter()
            .map(|region| (region.base, region.bytes.clone()))
            .collect();
        self.regions = regions;
        self.region_bytes = total;
        Ok(())
    }

    pub fn stack_pointer(&self) -> usize {
        self.sp
    }

    pub fn frame_base(&self) -> usize {
        self.frame_base
    }

    pub fn frame_top(&self) -> usize {
        self.frame_top
    }

    pub fn stack_bits(&self) -> impl Iterator<Item = u32> + '_ {
        self.stack.iter().map(|value| value.bits())
    }

    pub fn stack_bytes(&self) -> &[u8] {
        let start = self.stack_start.min(self.sp);
        let end = self.sp.min(self.memory.len());
        &self.memory[start..end]
    }

    pub fn native_arguments(&self) -> Option<&[u8]> {
        self.native_arguments
            .and_then(|(start, end)| self.memory.get(start..end))
    }

    /// Read the allocated VM bytes beginning at the current native parameter
    /// pointer. Some original wrappers read a fixed structure beyond the KCS
    /// call's declared pop count. Retain those stack bytes without extending
    /// the argument count, touching external storage, or exposing a host pointer.
    pub fn native_argument_storage(&self, length: usize) -> Result<&[u8]> {
        let (start, _) = self.native_arguments.ok_or(ScriptError::CallParameterUnderflow)?;
        let end = start.checked_add(length)
            .filter(|&end| end <= self.memory.len())
            .ok_or(ScriptError::MemoryOutOfBounds { address: start as u32, length })?;
        Ok(&self.memory[start..end])
    }

    pub fn execution_mode(&self) -> u32 {
        self.control.mode
    }

    pub fn call_return_position(&self) -> Option<usize> {
        self.control.call_return
    }

    pub fn set_execution_mode(&mut self, mode: u32) -> Result<()> {
        if self.has_pending_execution() {
            return Err(ScriptError::InvalidSceneControl {
                offset: self.instruction_pointer,
                message: "cannot change execution mode during budget continuation",
            });
        }
        self.control.mode = mode;
        self.control.call_return = None;
        Ok(())
    }

    pub fn has_pending_execution(&self) -> bool {
        self.control.message.is_some() || self.control.resume_started_at_end.is_some()
    }

    pub fn has_pending_ordinary_resume(&self) -> bool {
        self.control.resume_started_at_end.is_some()
    }

    pub fn waiting_for_message(&self) -> bool {
        self.control.waiting
    }

    /// Native 71 registers a VM address, not a process pointer. Its host
    /// handler returns YieldVoid; the eventual 0/1 is supplied by resume.
    pub fn wait_for_message(&mut self, address: u32) -> Result<()> {
        self.storage_location(address, 16)?;
        self.control.receiver = Some(address);
        self.control.waiting = true;
        Ok(())
    }

    /// Returns an independent byte snapshot; use external_binding to share
    /// storage identity with another VM. Lock poisoning remains an error.
    pub fn external_bytes(&self) -> Result<Vec<u8>> {
        self.external.snapshot()
    }

    pub fn external_binding(&self) -> SharedExternalStorage {
        self.external.clone()
    }

    pub fn bind_external(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.bind_shared_external(SharedExternalStorage::new(bytes)?)
    }

    /// Rebinds only this VM. Other VMs retain the old shared storage object.
    pub fn bind_shared_external(&mut self, bytes: SharedExternalStorage) -> Result<()> {
        if self.has_pending_execution() {
            return Err(ScriptError::InvalidSceneControl {
                offset: self.instruction_pointer,
                message: "cannot replace external memory during budget continuation",
            });
        }
        bytes.len()?;
        self.external = bytes;
        Ok(())
    }

    fn at_code_end(&self) -> Result<bool> {
        Ok(self.instruction_pointer
            == self.program.as_ref().ok_or(ScriptError::NoProgram)?.code.len())
    }

    fn ordinary_result(&self, step: Step, started_at_end: bool) -> ResumeStep {
        match step {
            Step::Yield => ResumeStep::Yield,
            // Retry has rewound the native instruction and restored its
            // arguments. Its host operation still owns this invocation,
            // unlike an explicit native yield which completes this slice.
            Step::Event(ExecutionEvent { status: 5, .. }) => ResumeStep::Yield,
            Step::Event(event) => ResumeStep::Completed {
                active: true,
                event: Some(event),
            },
            Step::Halted => ResumeStep::Completed {
                active: !started_at_end && self.program.as_ref()
                    .is_some_and(|program| self.instruction_pointer == program.code.len()),
                event: None,
            },
        }
    }

    /// Resume an ordinary task without a message. A mode-zero task preserves
    /// its receiver and stack; synchronous dispatch intentionally bypasses it.
    pub fn resume(&mut self, host: &mut impl Host, budget: usize) -> Result<ResumeStep> {
        if self.control.message.is_some() {
            return Err(ScriptError::InvalidSceneControl {
                offset: self.instruction_pointer,
                message: "message dispatch must finish before ordinary resume",
            });
        }
        if self.control.mode == 0 {
            return Ok(ResumeStep::Paused);
        }
        if budget == 0 {
            return Ok(ResumeStep::Yield);
        }
        let started = match self.control.resume_started_at_end {
            Some(started) => started,
            None => {
                let started = self.at_code_end()?;
                if self.control.waiting {
                    self.push_word(0)?;
                    self.control.waiting = false;
                }
                self.control.resume_started_at_end = Some(started);
                started
            }
        };
        let step = self.run_inner(host, budget)?;
        let result = self.ordinary_result(step, started);
        if result != ResumeStep::Yield {
            self.control.resume_started_at_end = None;
        }
        Ok(result)
    }

    /// Deliver exactly once across budget yields and native retries. The
    /// caller keeps the same message until completion; only its second DWORD
    /// is written back once the suspended invocation actually completes.
    pub fn dispatch_message(
        &mut self,
        message: &mut [u32; 4],
        host: &mut impl Host,
        budget: usize,
    ) -> Result<Option<ResumeStep>> {
        if self.control.resume_started_at_end.is_some() {
            return Err(ScriptError::InvalidSceneControl {
                offset: self.instruction_pointer,
                message: "ordinary resume must finish before message dispatch",
            });
        }
        if let Some(pending) = self.control.message {
            if pending != *message {
                return Err(ScriptError::InvalidSceneControl {
                    offset: self.instruction_pointer,
                    message: "budget continuation requires the same task message",
                });
            }
        } else {
            if !self.control.waiting {
                return Ok(None);
            }
            if budget == 0 {
                return Ok(Some(ResumeStep::Yield));
            }
            let started = self.at_code_end()?;
            let address = self.control.receiver.ok_or(ScriptError::InvalidCallFrame)?;
            self.storage_location(address, 16)?;
            self.sp.checked_add(4)
                .filter(|end| *end <= self.memory.len())
                .ok_or(ScriptError::StackOverflow)?;
            let bytes = message.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
            self.write(address, &bytes)?;
            self.push_word(1)?;
            self.control.waiting = false;
            self.control.message = Some(*message);
            self.control.message_started_at_end = started;
        }
        let step = self.run_inner(host, budget)?;
        let result = self.ordinary_result(step, self.control.message_started_at_end);
        if result != ResumeStep::Yield {
            let address = self.control.receiver.ok_or(ScriptError::InvalidCallFrame)?;
            message[1] = self.read_u32(address.checked_add(4).ok_or(
                ScriptError::MemoryOutOfBounds { address, length: 16 },
            )?)?;
            self.control.message = None;
        }
        Ok(Some(result))
    }

    /// Start a fresh invocation of the loaded program without reinitializing
    /// any memory. PC becomes zero; SP, frame base and frame top return to the
    /// initial stack boundary. All caller frames and per-frame bookkeeping are
    /// discarded, as are message waits, budget continuations and pause/step
    /// control (execution mode returns to 1).
    ///
    /// Program/code, every byte of mutable local storage (including retired
    /// stack bytes), independent regions, external storage, switch value and all
    /// debug metadata are preserved in place. This does not restore a suspended caller: it always
    /// clears the entire call chain. Motion/particle hosts should call it only
    /// when starting a new invocation, never while continuing a budget yield.
    ///
    /// A budget continuation may deliberately be abandoned by this method.
    /// Resetting from inside an active native callback is rejected because its
    /// enclosing call instruction still owns argument/return processing.
    pub fn reset_execution(&mut self) -> Result<()> {
        self.program.as_ref().ok_or(ScriptError::NoProgram)?;
        if self.native_arguments.is_some() {
            return Err(ScriptError::InvalidSceneControl {
                offset: self.instruction_pointer,
                message: "cannot reset execution from inside an active native call",
            });
        }
        self.instruction_pointer = 0;
        self.sp = self.stack_start;
        self.frame_base = self.stack_start;
        self.frame_top = self.stack_start;
        self.frames.clear();
        self.frame_floors.clear();
        self.frame_floors.push(self.stack_start);
        self.debug_variable_floors.clear();
        self.stack.clear();
        self.native_arguments = None;
        self.control = ExecutionControl::default();
        Ok(())
    }

    /// Reload the initial program data. Use reset_execution for a new invocation
    /// that must preserve mutable local memory and debug/switch state.
    pub fn reset(&mut self) -> Result<()> {
        let program = self.program.clone().ok_or(ScriptError::NoProgram)?;
        let external = self.external.clone();
        self.initialize_program(program, external)
    }

    pub fn read(&self, address: u32, length: usize) -> Result<Vec<u8>> {
        self.with_read_bytes(address, length, <[u8]>::to_vec)
    }

    fn with_read_bytes<T>(
        &self,
        address: u32,
        length: usize,
        read: impl FnOnce(&[u8]) -> T,
    ) -> Result<T> {
        if address & EXTERNAL_ADDRESS_BIT != 0 {
            let bytes = self.external.read_guard()?;
            let offset = (address & !EXTERNAL_ADDRESS_BIT) as usize;
            let end = offset.checked_add(length)
                .filter(|&end| end <= bytes.len())
                .ok_or(ScriptError::MemoryOutOfBounds { address, length })?;
            return Ok(read(&bytes[offset..end]));
        }
        let (storage, offset) = self.storage(address, length)?;
        Ok(read(&storage[offset..offset + length]))
    }

    pub fn read_u32(&self, address: u32) -> Result<u32> {
        self.with_read_bytes(address, 4, |bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn read_string(&self, address: u32) -> Result<Vec<u8>> {
        if address & EXTERNAL_ADDRESS_BIT != 0 {
            let bytes = self.external.read_guard()?;
            let offset = (address & !EXTERNAL_ADDRESS_BIT) as usize;
            let tail = bytes.get(offset..)
                .ok_or(ScriptError::MemoryOutOfBounds { address, length: 0 })?;
            let length = tail.iter().position(|&byte| byte == 0)
                .ok_or(ScriptError::InvalidStructuredText {
                    offset: address as usize,
                    message: "unterminated KCS string",
                })?;
            return Ok(tail[..length].to_vec());
        }
        let (storage, offset) = self.storage(address, 0)?;
        let bytes = &storage[offset..];
        let length =
            bytes
                .iter()
                .position(|&byte| byte == 0)
                .ok_or(ScriptError::InvalidStructuredText {
                    offset: address as usize,
                    message: "unterminated KCS string",
                })?;
        Ok(bytes[..length].to_vec())
    }

    pub fn write(&mut self, address: u32, bytes: &[u8]) -> Result<()> {
        if address & EXTERNAL_ADDRESS_BIT != 0 {
            let mut storage = self.external.bytes.write()
                .map_err(|_| ScriptError::ExternalStoragePoisoned)?;
            let offset = (address & !EXTERNAL_ADDRESS_BIT) as usize;
            let end = offset.checked_add(bytes.len())
                .filter(|&end| end <= storage.len())
                .ok_or(ScriptError::MemoryOutOfBounds { address, length: bytes.len() })?;
            storage[offset..end].copy_from_slice(bytes);
            return Ok(());
        }
        // Resolve the whole access before writing; adjacent allocations are
        // separate buffers and never combine into one readable/writable span.
        let (location, offset) = self.storage_location(address, bytes.len())?;
        let end = offset + bytes.len();
        let storage = match location {
            VmStorage::Local => &mut self.memory,
            VmStorage::External => {
                return Err(ScriptError::MemoryOutOfBounds { address, length: bytes.len() });
            }
            VmStorage::Region(base) => self.regions.get_mut(&base)
                .ok_or(ScriptError::MemoryOutOfBounds { address, length: bytes.len() })?,
        };
        storage[offset..end].copy_from_slice(bytes);
        if matches!(location, VmStorage::Local)
            && offset < self.sp && end > self.stack_start
        {
            self.sync_stack_view();
        }
        Ok(())
    }

    pub fn run(&mut self, host: &mut impl Host, budget: usize) -> Result<Step> {
        if self.has_pending_execution() {
            return Err(ScriptError::InvalidSceneControl {
                offset: self.instruction_pointer,
                message: "low-level run cannot interrupt ordinary-task continuation",
            });
        }
        self.run_inner(host, budget)
    }

    fn run_inner(&mut self, host: &mut impl Host, budget: usize) -> Result<Step> {
        // A native call can replace the program. Keep the current instruction
        // alive across that call and refresh the program before the next fetch.
        let mut active_program = self.program.clone();
        let mut instruction_cursor: Option<usize> = None;
        let mut instruction_step = 0;
        while instruction_step < budget {
            let program = active_program.as_ref().ok_or(ScriptError::NoProgram)?;
            if self.instruction_pointer == program.code.len() {
                return Ok(Step::Halted);
            }
            let instruction = match instruction_cursor
                .and_then(|index| program.instructions.get(index))
                .filter(|instruction| instruction.offset == self.instruction_pointer)
            {
                Some(instruction) => instruction,
                None => {
                    let index = program.instruction_index(self.instruction_pointer)
                        .ok_or(ScriptError::InvalidInstruction {
                            offset: self.instruction_pointer,
                        })?;
                    instruction_cursor = Some(index);
                    program.instructions.get(index)
                        .filter(|instruction| instruction.offset == self.instruction_pointer)
                        .ok_or(ScriptError::InvalidInstruction { offset: self.instruction_pointer })?
                }
            };
            if instruction.opcode == 0x3b {
                return Ok(Step::Halted);
            }
            // Leave PC/stack/native arguments untouched at a cooperative yield.
            if (instruction_step % 64 == 0 || instruction.opcode == NATIVE_CALL_OPCODE)
                && host.should_yield()
            { return Ok(Step::Yield); }
            let opcode = instruction.opcode;
            let next_offset = instruction.offset + 1 + instruction.operands.len() * 5;
            let next_index = instruction_cursor.map(|index| index + 1);
            self.instruction_pointer = next_offset;
            if let Some(event) = self.execute(instruction, host)? {
                return Ok(Step::Event(event));
            }
            if opcode == NATIVE_CALL_OPCODE
                && !self.program.as_ref().is_some_and(|current| Arc::ptr_eq(program, current))
            {
                active_program = self.program.clone();
                instruction_cursor = None;
            } else if self.instruction_pointer == next_offset {
                instruction_cursor = next_index;
            } else {
                instruction_cursor = None;
            }
            instruction_step += 1;
            if instruction_step == budget && host.renew_instruction_budget(budget) {
                instruction_step = 0;
            }
        }
        Ok(Step::Yield)
    }

    fn execute(
        &mut self,
        instruction: &Instruction,
        host: &mut impl Host,
    ) -> Result<Option<ExecutionEvent>> {
        let a = &instruction.operands;
        match instruction.opcode {
            0x00 => {}
            0x01..=0x07 | 0x09..=0x0b | 0x0d..=0x14 => {
                let left = self.value(a[0])?;
                let right = self.value(a[1])?;
                self.push_word(binary(
                    instruction.opcode,
                    left,
                    right,
                    a[0].flags,
                    a[1].flags,
                    instruction.offset,
                )?)?;
            }
            0x08 | 0x0c | 0x1a => {
                let value = self.value(a[0])?;
                let result = match instruction.opcode {
                    0x08 => !value,
                    0x0c => u32::from(value == 0),
                    0x1a if a[0].flags.is_float() => value ^ 0x8000_0000,
                    _ => value.wrapping_neg(),
                };
                self.push_word(result)?;
            }
            0x15 => {
                let target = self.raw(a[0])?;
                let value = self.value(a[1])?;
                let width = match a[0].flags.bits() {
                    0x56 => 4,
                    _ => a[0].flags.width()?,
                };
                self.write(target, &value.to_le_bytes()[..width])?;
                self.push_word(match width {
                    1 => value & 0xff,
                    2 => value & 0xffff,
                    _ => value,
                })?;
            }
            0x16..=0x19 => {
                let target = self.raw(a[0])?;
                let old = self.read_u32(target)?;
                let width = TypeFlags::new(a[0].flags.bits() & 0x7f).width()?;
                let subtract = matches!(instruction.opcode, 0x17 | 0x19);
                let result = if a[0].flags.is_float() {
                    float_binary(
                        if subtract { 2 } else { 1 },
                        old,
                        1.0f32.to_bits(),
                        instruction.offset,
                    )?
                } else {
                    old.wrapping_add(if subtract { u32::MAX } else { 1 })
                };
                self.write(target, &result.to_le_bytes()[..width])?;
                self.push_word(if instruction.opcode >= 0x18 {
                    old
                } else {
                    self.read_u32(target)?
                })?;
            }
            0x1b..=0x24 => {
                let target = self.raw(a[0])?;
                let right = self.value(a[1])?;
                let left = self.read_u32(target)?;
                let result = binary(
                    instruction.opcode,
                    left,
                    right,
                    a[0].flags,
                    a[1].flags,
                    instruction.offset,
                )?;
                self.write(target, &result.to_le_bytes()[..a[0].flags.width()?])?;
                self.push_word(result)?;
            }
            0x25 => {
                if a[0].flags.is_address() {
                    let address = self.raw(a[0])?;
                    self.push_word(self.read_u32(address)?)?;
                } else if !a[0].flags.from_stack() {
                    self.push_word(a[0].payload)?;
                }
            }
            0x26 => {
                let target = self.raw(a[0])?;
                let value = self.pop_word()?;
                let width = TypeFlags::new(a[0].flags.bits() & 0x7f).width()?;
                self.write(target, &value.to_le_bytes()[..width])?;
            }
            0x27..=0x29 => {
                let target = self.value(a[0])? as usize;
                let jump = if instruction.opcode == 0x29 {
                    true
                } else {
                    let value = self.pop_word()?;
                    if instruction.opcode == 0x27 {
                        value == 0
                    } else {
                        value != 0
                    }
                };
                self.set_sp(self.frame_top)?;
                if jump {
                    self.jump(target)?;
                }
            }
            0x2a => {
                let target = self.value(a[0])? as usize;
                let parameter_bytes = self.value(a[1])? as usize;
                let available = self
                    .sp
                    .checked_sub(self.frame_top)
                    .ok_or(ScriptError::CallParameterUnderflow)?;
                if parameter_bytes > available {
                    return Err(ScriptError::CallParameterUnderflow);
                }
                let program = self.program.as_ref().ok_or(ScriptError::NoProgram)?;
                if !program.is_instruction_boundary(target) {
                    return Err(ScriptError::InvalidJump { target });
                }
                self.sp.checked_add(12)
                    .filter(|top| *top <= self.memory.len())
                    .ok_or(ScriptError::StackOverflow)?;
                let base = self.sp - parameter_bytes;
                let caller_base = self.frame_base;
                let return_pc = self.instruction_pointer;
                if self.control.mode == 3 {
                    self.control.mode = 1;
                    self.control.call_return = Some(return_pc);
                }
                self.push_word(return_pc as u32)?;
                self.push_word(self.frame_base as u32)?;
                self.push_word(self.frame_top as u32)?;
                self.frame_base = base;
                self.frame_top = self.sp;
                self.frame_floors.push(self.frame_top);
                self.debug_variable_floors.push(self.debug.variables.len());
                self.frames.push(CallFrame {
                    return_pc,
                    base: caller_base,
                    top: self.frame_top,
                    parameter_bytes,
                });
                self.instruction_pointer = target;
            }
            NATIVE_CALL_OPCODE => {
                let index = self.value(a[0])?;
                let parameter_bytes = self.value(a[1])? as usize;
                let available = self
                    .sp
                    .checked_sub(self.frame_top)
                    .ok_or(ScriptError::CallParameterUnderflow)?;
                if parameter_bytes > available {
                    return Err(ScriptError::CallParameterUnderflow);
                }
                let old_sp = self.sp;
                self.sp -= parameter_bytes;
                self.sync_stack_view();
                let arguments = self.memory[self.sp..old_sp].to_vec();
                let previous = self.native_arguments.replace((self.sp, old_sp));
                let result = host.call(index, &arguments, self);
                self.native_arguments = previous;
                let result = result?;
                let status = native_status(result);
                match result {
                    NativeReturn::Value(value) | NativeReturn::YieldValue(value) => {
                        self.push_word(value)?;
                    }
                    NativeReturn::Yield
                    | NativeReturn::Continue
                    | NativeReturn::YieldVoid
                    | NativeReturn::Retry => {}
                }
                if result == NativeReturn::Retry {
                    self.sp = old_sp;
                    self.sync_stack_view();
                    self.instruction_pointer = instruction.offset;
                }
                if matches!(result, NativeReturn::Value(_) | NativeReturn::Continue) {
                    return Ok(None);
                } else {
                    return Ok(Some(ExecutionEvent {
                        native_index: index,
                        status,
                    }));
                }
            }
            0x2c => {
                self.return_from_call(a[0], a[1])?;
            }
            0x2d => {
                self.pop_word()?;
            }
            0x2e => {
                let value = self.raw(a[0])?;
                self.push_word(value)?;
            }
            0x2f => {
                let address = self.raw(a[0])?;
                self.push_word(self.read_u32(address)?)?;
            }
            0x30 => {
                let target = self.raw(a[0])?;
                let length = self.value(a[1])? as usize;
                let available = self
                    .sp
                    .checked_sub(self.frame_top)
                    .ok_or(ScriptError::StackUnderflow)?;
                if length > available {
                    return Err(ScriptError::StackUnderflow);
                }
                let source = self.memory[self.sp - length..self.sp].to_vec();
                self.write(target, &source)?;
            }
            0x31 | 0x32 => {
                let length = self.value(a[0])? as usize;
                let top = if instruction.opcode == 0x31 {
                    self.frame_top.checked_add(length)
                } else {
                    self.frame_top.checked_sub(length)
                }
                .filter(|&value| value >= self.stack_start && value <= self.memory.len())
                .ok_or(ScriptError::InvalidLocalFrame)?;
                if top < *self.frame_floors.last().unwrap() {
                    return Err(ScriptError::InvalidLocalFrame);
                }
                self.frame_top = top;
                self.set_sp(top)?;
            }
            0x33 => {
                self.switch_value = self.pop_word()?;
            }
            0x34 => {
                let value = self.value(a[0])?;
                let target = self.value(a[1])? as usize;
                if value == self.switch_value {
                    self.jump(target)?;
                }
            }
            0x35 => {
                let left = self.value(a[0])?;
                let right = self.value(a[1])?;
                let target = self.value(a[2])? as usize;
                if (left.min(right)..=left.max(right)).contains(&self.switch_value) {
                    self.jump(target)?;
                }
            }
            0x36..=0x39 => {
                let value = self.value(a[0])?;
                self.push_word(convert(instruction.opcode, value, instruction.offset)?)?;
            }
            0x3a => {
                let value = self.pop_word()?;
                self.push_word(value)?;
                self.push_word(value)?;
            }
            0x3c => {
                self.set_sp(self.frame_top)?;
            }
            0x3d => {
                let value = self.value(a[0])?;
                self.push_word(value.wrapping_add(self.frame_base as u32))?;
            }
            0x3e => {
                let mut value = self.raw(a[0])?;
                if a[0].flags.is_address() {
                    value = self.read_u32(value)?;
                }
                let right = self.value(a[1])?;
                self.push_word(value.wrapping_add(right))?;
            }
            0x41 => {
                self.debug.line = self.value(a[0])?;
                let source_address = self.value(a[1])?;
                self.debug.source = self.read_string(source_address)?;
            }
            0x42 => {
                let address = self.value(a[0])?;
                self.read(address, 0)?;
                if self.debug.scopes.len() >= MAX_SCRIPT_ITEMS {
                    return Err(ScriptError::InvalidStructuredText {
                        offset: instruction.offset,
                        message: "KCS debug scope count exceeds limit",
                    });
                }
                self.debug.scopes.push(address);
            }
            0x43 => {
                let first = self.value(a[0])?;
                let count = self.value(a[1])? as usize;
                if count > MAX_SCRIPT_ITEMS.saturating_sub(self.debug.variables.len()) {
                    return Err(ScriptError::InvalidStructuredText {
                        offset: instruction.offset,
                        message: "KCS debug variable count exceeds limit",
                    });
                }
                self.debug
                    .variables
                    .extend((0..count).map(|index| first.wrapping_add(index as u32)));
            }
            0x44 => {
                let count = self.value(a[0])? as usize;
                let length = self
                    .debug
                    .variables
                    .len()
                    .checked_sub(count)
                    .ok_or(ScriptError::StackUnderflow)?;
                self.debug.variables.truncate(length);
            }
            opcode => {
                return Err(ScriptError::UnsupportedOpcode {
                    offset: instruction.offset,
                    opcode,
                });
            }
        }
        Ok(None)
    }

    fn return_from_call(&mut self, result: Operand, parameters: Operand) -> Result<()> {
        if self.frames.is_empty() {
            return Err(ScriptError::InvalidCallFrame);
        }
        let result_length = self.value(result)? as usize;
        let parameter_length = self.value(parameters)? as usize;
        let available = self
            .sp
            .checked_sub(self.frame_top)
            .ok_or(ScriptError::ReturnValueUnderflow)?;
        if result_length > available {
            return Err(ScriptError::ReturnValueUnderflow);
        }
        let result_range = self.sp - result_length..self.sp;
        let saved = self
            .frame_base
            .checked_add(parameter_length)
            .ok_or(ScriptError::InvalidCallFrame)?;
        let frame_top = *self
            .frame_floors
            .last()
            .ok_or(ScriptError::InvalidCallFrame)?;
        if saved.checked_add(12) != Some(frame_top) {
            return Err(ScriptError::CallFrameMismatch {
                reason: "saved frame header does not match frame top",
                frame_base: self.frame_base,
                frame_top,
                parameter_bytes: parameter_length,
                result_bytes: result_length,
                saved,
                expected_base: 0,
                saved_base: 0,
                expected_parameters: parameter_length,
            });
        }
        let return_pc = self.read_local_u32(saved)? as usize;
        let base = self.read_local_u32(saved + 4)? as usize;
        let top = self.read_local_u32(saved + 8)? as usize;
        let destination = self.frame_base;
        if base > self.memory.len()
            || top > self.memory.len()
            || destination.saturating_add(result_length) > self.memory.len()
        {
            return Err(ScriptError::CallFrameMismatch {
                reason: "saved memory range is outside the VM",
                frame_base: self.frame_base,
                frame_top,
                parameter_bytes: parameter_length,
                result_bytes: result_length,
                saved,
                expected_base: 0,
                saved_base: base,
                expected_parameters: parameter_length,
            });
        }
        if !self.program.as_ref().ok_or(ScriptError::NoProgram)?
            .is_instruction_boundary(return_pc)
        {
            return Err(ScriptError::InvalidJump { target: return_pc });
        }
        self.frame_base = base;
        self.frame_top = top;
        self.sp = destination + result_length;
        self.frames.pop();
        self.frame_floors.pop();
        self.debug.scopes.pop();
        if let Some(floor) = self.debug_variable_floors.pop() {
            self.debug.variables.truncate(floor);
        }
        self.memory.copy_within(result_range, destination);
        self.sync_stack_view();
        self.instruction_pointer = return_pc;
        Ok(())
    }

    fn raw(&mut self, operand: Operand) -> Result<u32> {
        if operand.flags.from_stack() {
            self.pop_word()
        } else {
            Ok(operand.payload)
        }
    }

    fn value(&mut self, operand: Operand) -> Result<u32> {
        let raw = self.raw(operand)?;
        if !operand.flags.is_address() {
            return Ok(raw);
        }
        let width = operand.flags.width()?;
        self.with_read_bytes(raw, width, |bytes| match width {
            1 if operand.flags.is_signed() => (bytes[0] as i8 as i32) as u32,
            1 => bytes[0] as u32,
            2 if operand.flags.is_signed() => {
                i16::from_le_bytes(bytes.try_into().unwrap()) as i32 as u32
            }
            2 => u16::from_le_bytes(bytes.try_into().unwrap()) as u32,
            _ => u32::from_le_bytes(bytes.try_into().unwrap()),
        })
    }

    fn jump(&mut self, target: usize) -> Result<()> {
        let program = self.program.as_ref().ok_or(ScriptError::NoProgram)?;
        if !program.is_instruction_boundary(target) {
            return Err(ScriptError::InvalidJump { target });
        }
        self.instruction_pointer = target;
        Ok(())
    }

    fn push_word(&mut self, value: u32) -> Result<()> {
        let old_sp = self.sp;
        let end = self
            .sp
            .checked_add(4)
            .filter(|&end| end <= self.memory.len())
            .ok_or(ScriptError::StackOverflow)?;
        self.memory[self.sp..end].copy_from_slice(&value.to_le_bytes());
        self.sp = end;
        if old_sp >= self.stack_start && (old_sp - self.stack_start) % 4 == 0 {
            self.stack.push(Value::U32(value));
        } else {
            self.sync_stack_view();
        }
        Ok(())
    }

    fn pop_word(&mut self) -> Result<u32> {
        if self.sp < 4 {
            return Err(ScriptError::StackUnderflow);
        }
        self.sp -= 4;
        let value = u32::from_le_bytes(self.memory[self.sp..self.sp + 4].try_into().unwrap());
        if self.sp >= self.stack_start && (self.sp - self.stack_start) % 4 == 0 {
            self.stack.pop();
        } else {
            self.sync_stack_view();
        }
        Ok(value)
    }

    fn set_sp(&mut self, value: usize) -> Result<()> {
        if value > self.memory.len() {
            return Err(ScriptError::StackOverflow);
        }
        self.sp = value;
        self.sync_stack_view();
        Ok(())
    }

    fn sync_stack_view(&mut self) {
        let start = self.stack_start.min(self.sp);
        let end = self.sp.min(self.memory.len());
        self.stack.clear();
        self.stack.extend(
            self.memory[start..end]
                .chunks_exact(4)
                .map(|word| Value::U32(u32::from_le_bytes(word.try_into().unwrap()))),
        );
    }

    fn read_local_u32(&self, offset: usize) -> Result<u32> {
        let bytes = self
            .memory
            .get(offset..offset.saturating_add(4))
            .filter(|bytes| bytes.len() == 4)
            .ok_or(ScriptError::MemoryOutOfBounds {
                address: offset as u32,
                length: 4,
            })?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn storage(&self, address: u32, length: usize) -> Result<(&[u8], usize)> {
        let (location, offset) = self.storage_location(address, length)?;
        let storage = match location {
            VmStorage::Local => &self.memory,
            // External access must own its read guard for the entire copy;
            // never manufacture a borrowed slice after releasing the lock.
            VmStorage::External => {
                return Err(ScriptError::MemoryOutOfBounds { address, length });
            }
            VmStorage::Region(base) => self.regions.get(&base)
                .ok_or(ScriptError::MemoryOutOfBounds { address, length })?,
        };
        Ok((storage, offset))
    }

    fn storage_location(&self, address: u32, length: usize) -> Result<(VmStorage, usize)> {
        let invalid = || ScriptError::MemoryOutOfBounds { address, length };
        let fits = |offset: usize, capacity: usize| {
            offset.checked_add(length).is_some_and(|end| end <= capacity)
        };
        if address & EXTERNAL_ADDRESS_BIT != 0 {
            let offset = (address & !EXTERNAL_ADDRESS_BIT) as usize;
            return if fits(offset, self.external.len()?) {
                Ok((VmStorage::External, offset))
            } else {
                Err(invalid())
            };
        }
        let offset = address as usize;
        if offset < self.memory.len() {
            return if fits(offset, self.memory.len()) {
                Ok((VmStorage::Local, offset))
            } else {
                Err(invalid())
            };
        }
        // Prefer a block beginning exactly at D+S over the local one-past
        // endpoint, including zero-length probes used by read_string.
        if let Some((&base, bytes)) = self.regions.range(..=address).next_back() {
            let relative = (address - base) as usize;
            if fits(relative, bytes.len()) {
                return Ok((VmStorage::Region(base), relative));
            }
        }
        // Retain the old empty-range behavior at the local storage endpoint.
        // An access containing any bytes must start inside one real buffer.
        if length == 0 && offset == self.memory.len() {
            return Ok((VmStorage::Local, offset));
        }
        Err(invalid())
    }
}

fn native_status(result: NativeReturn) -> u8 {
    match result {
        NativeReturn::Yield => 0,
        NativeReturn::Value(_) => 1,
        NativeReturn::Continue => 2,
        NativeReturn::YieldValue(_) => 3,
        NativeReturn::YieldVoid => 4,
        NativeReturn::Retry => 5,
    }
}

fn binary(
    opcode: u8,
    left: u32,
    right: u32,
    left_flags: TypeFlags,
    right_flags: TypeFlags,
    offset: usize,
) -> Result<u32> {
    let flags = left_flags.bits() | right_flags.bits();
    let signed = flags & TypeFlags::SIGNED != 0;
    let base = match opcode {
        0x1b => 5,
        0x1c => 6,
        0x1d => 3,
        0x1e => 1,
        0x1f => 2,
        0x20 => 4,
        0x21 => 10,
        0x22 => 11,
        0x23 => 9,
        0x24 => 7,
        _ => opcode,
    };
    if (1..=4).contains(&base) && flags & 4 != 0 {
        return Ok(if matches!(opcode, 0x1d | 0x1e) {
            float_binary(base as u8, right, left, offset)?
        } else if opcode == 0x20 && f32::from_bits(right) == 0.0 {
            right
        } else {
            float_binary(base as u8, left, right, offset)?
        });
    }
    if (0x0f..=0x14).contains(&base) {
        return Ok(if left_flags.is_float() {
            compare(base, f32::from_bits(left), f32::from_bits(right), offset)?
        } else if signed {
            compare(base, left as i32, right as i32, offset)?
        } else {
            compare(base, left, right, offset)?
        });
    }
    Ok(match base {
        1 => left.wrapping_add(right),
        2 => left.wrapping_sub(right),
        3 => left.wrapping_mul(right),
        4 | 5 if right == 0 => 0,
        4 | 5 if signed => {
            let left = left as i32;
            let right = right as i32;
            if left == i32::MIN && right == -1 {
                return Err(ScriptError::IntegerOverflow);
            }
            if base == 4 {
                (left / right) as u32
            } else {
                (left % right) as u32
            }
        }
        4 if opcode != 0x20 => left / right,
        4 | 5 => left % right,
        6 => left & right,
        7 => left | right,
        9 => left ^ right,
        10 => left.wrapping_shl(right & 31),
        11 if opcode == 0x22 && signed => (left as i32).wrapping_shr(right & 31) as u32,
        11 => left.wrapping_shr(right & 31),
        13 => u32::from(left != 0 && right != 0),
        14 => u32::from(left != 0 || right != 0),
        _ => return Err(ScriptError::UnsupportedOpcode { offset, opcode }),
    })
}

fn compare<T: PartialOrd + PartialEq>(opcode: u8, left: T, right: T, offset: usize) -> Result<u32> {
    let result = match opcode {
        0x0f => left > right,
        0x10 => left < right,
        0x11 => left >= right,
        0x12 => left <= right,
        0x13 => left == right,
        0x14 => left != right,
        _ => return Err(ScriptError::UnsupportedOpcode { offset, opcode }),
    };
    Ok(u32::from(result))
}

fn float_binary(opcode: u8, left: u32, right: u32, offset: usize) -> Result<u32> {
    if !(1..=4).contains(&opcode) {
        return Err(ScriptError::UnsupportedOpcode { offset, opcode });
    }
    if opcode == 4 && f32::from_bits(right) == 0.0 {
        return Ok(0);
    }
    let left_float = f32::from_bits(left);
    let right_float = f32::from_bits(right);
    if left_float.is_nan() {
        return Ok(left | 0x0040_0000);
    }
    if right_float.is_nan() {
        return Ok(right | 0x0040_0000);
    }
    let result = match opcode {
        1 => left_float + right_float,
        2 => left_float - right_float,
        3 => left_float * right_float,
        4 => left_float / right_float,
        _ => unreachable!(),
    };
    Ok(if result.is_nan() {
        0xffc0_0000
    } else {
        result.to_bits()
    })
}

fn convert(opcode: u8, value: u32, offset: usize) -> Result<u32> {
    Ok(match opcode {
        0x36 => (value as f32).to_bits(),
        0x37 => (value as i32 as f32).to_bits(),
        0x38 => {
            let exponent = ((value >> 23) & 255) as i32 - 127;
            if !(0..55).contains(&exponent) {
                return Ok(0);
            }
            let significand = (value & 0x007f_ffff) | 0x0080_0000;
            let result = if exponent >= 23 {
                significand.wrapping_shl((exponent - 23) as u32)
            } else {
                significand >> (23 - exponent)
            };
            if value >> 31 != 0 {
                result.wrapping_neg()
            } else {
                result
            }
        }
        0x39 => {
            let value = f32::from_bits(value);
            if !(-2147483648.0..2147483648.0).contains(&value) {
                0x8000_0000
            } else {
                (value as i32) as u32
            }
        }
        _ => return Err(ScriptError::UnsupportedOpcode { offset, opcode }),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CstRecord {
    pub kind: u8,
    pub text: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CstDocument {
    pub records: Vec<CstRecord>,
    pub inputs: Vec<Range<usize>>,
}

impl CstDocument {
    pub fn input_records(&self, input: usize) -> Option<&[CstRecord]> {
        let range = self.inputs.get(input)?;
        self.records.get(range.clone())
    }

    pub fn record_kind(&self, input: i32, record: i32) -> u32 {
        if input < 0 {
            return self.inputs.len() as u32;
        }
        let Some(range) = self.inputs.get(input as usize) else {
            return u32::MAX;
        };
        if record < 0 {
            return range.len() as u32;
        }
        let Some(index) = range.start.checked_add(record as usize) else {
            return u32::MAX;
        };
        if index >= range.end {
            return u32::MAX;
        }
        self.records
            .get(index)
            .map_or(u32::MAX, |record| u32::from(record.kind))
    }

    pub fn inputs(&self) -> CstInputCursor<'_> {
        CstInputCursor {
            document: self,
            next_input: 0,
        }
    }

    /// Live script loading shares the format reader's verified compression
    /// variants, checksum checks and bounded record-table validation.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Self::parse_with_diagnostics(bytes).map(|(document, _)| document)
    }

    /// Preserve recovery diagnostics without changing this document's fields
    /// or breaking existing callers that construct records directly.
    pub fn parse_with_diagnostics(
        bytes: &[u8],
    ) -> Result<(Self, Vec<cs2_formats::CstDiagnostic>)> {
        use cs2_formats::CstError;
        let file = cs2_formats::CstFile::parse(bytes).map_err(|error| match error {
            CstError::InvalidSignature => ScriptError::InvalidSignature { kind: "CST" },
            CstError::TooLarge => ScriptError::InvalidHeader {
                kind: "CST", message: "container exceeds canonical 64 MiB limit",
            },
            CstError::InvalidCompression | CstError::UnsupportedVariant(_)
                | CstError::AmbiguousXorKey => ScriptError::InvalidCompression {
                    kind: "CST", message: error.to_string(),
                },
            CstError::Truncated => ScriptError::InvalidStructuredText {
                offset: bytes.len(), message: "CST container or field is truncated",
            },
            CstError::OutOfBounds => ScriptError::InvalidStructuredText {
                offset: 0, message: "CST table or stored payload is out of bounds",
            },
            CstError::InvalidRecord => ScriptError::InvalidStructuredText {
                offset: 0, message: "invalid CST record prefix",
            },
            CstError::InvalidInputs => ScriptError::InvalidStructuredText {
                offset: 0, message: "CST input ranges are not contiguous",
            },
        })?;
        let records = file.records.into_iter().map(|record| CstRecord {
            kind: record.kind, text: record.text,
        }).collect();
        Ok((Self { records, inputs: file.inputs }, file.diagnostics))
    }
}

pub struct CstInputCursor<'a> {
    document: &'a CstDocument,
    next_input: usize,
}

impl<'a> Iterator for CstInputCursor<'a> {
    type Item = (usize, &'a [CstRecord]);

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.next_input;
        let records = self.document.input_records(index)?;
        self.next_input += 1;
        Some((index, records))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesLine {
    pub offset: usize,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesSection {
    pub header: Vec<u8>,
    pub lines: Vec<FesLine>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesDocument {
    pub sections: Vec<FesSection>,
}

impl FesDocument {
    pub fn cursor(&self) -> FesCursor {
        FesCursor::default()
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_SCRIPT_BYTES {
            return Err(ScriptError::InvalidHeader {
                kind: "FES",
                message: "container exceeds size limit",
            });
        }
        if bytes.get(..4) != Some(b"FES\0") {
            return Err(ScriptError::InvalidSignature { kind: "FES" });
        }
        let compressed = read_u32(bytes, 4)? as usize;
        let uncompressed = read_u32(bytes, 8)? as usize;
        if bytes.len() != 16 + compressed {
            return Err(ScriptError::InvalidStructuredText {
                offset: 4,
                message: "FES length mismatch",
            });
        }
        let body = inflate_exact("FES", read_range(bytes, 16, compressed)?, uncompressed)?;
        let mut sections = Vec::new();
        let mut offset = 0;
        for raw in body.split_inclusive(|&byte| byte == b'\n') {
            let line_offset = offset;
            offset += raw.len();
            let line = raw.strip_suffix(b"\n").unwrap_or(raw);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let trimmed = trim_ascii(line);
            if let Some(header) = trimmed.strip_prefix(b"#") {
                if sections.len() >= MAX_SCRIPT_ITEMS {
                    return Err(ScriptError::InvalidStructuredText {
                        offset: line_offset,
                        message: "FES section count exceeds limit",
                    });
                }
                sections.push(FesSection {
                    header: header.to_vec(),
                    lines: Vec::new(),
                });
            } else if !trimmed.is_empty() {
                let leading = line.len() - line.trim_ascii_start().len();
                if let Some(section) = sections.last_mut() {
                    if section.lines.len() >= MAX_SCRIPT_ITEMS {
                        return Err(ScriptError::InvalidStructuredText {
                            offset: line_offset,
                            message: "FES line count exceeds limit",
                        });
                    }
                    section.lines.push(FesLine {
                        offset: line_offset + leading,
                        bytes: trimmed.to_vec(),
                    });
                }
            }
        }
        Ok(Self { sections })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FesItem<'a> {
    Section {
        index: usize,
        section: &'a FesSection,
    },
    Line {
        section_index: usize,
        line_index: usize,
        line: &'a FesLine,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FesCursor {
    section_index: usize,
    line_index: usize,
    section_pending: bool,
}

impl FesCursor {
    pub fn next<'a>(&mut self, document: &'a FesDocument) -> Option<FesItem<'a>> {
        loop {
            let section = document.sections.get(self.section_index)?;
            if !self.section_pending {
                self.section_pending = true;
                self.line_index = 0;
                return Some(FesItem::Section {
                    index: self.section_index,
                    section,
                });
            }
            if let Some(line) = section.lines.get(self.line_index) {
                let item = FesItem::Line {
                    section_index: self.section_index,
                    line_index: self.line_index,
                    line,
                };
                self.line_index += 1;
                return Some(item);
            }
            self.section_index += 1;
            self.section_pending = false;
        }
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_work_budget_yields_between_real_native_calls_and_resumes_in_order() {
        struct Gate { calls: usize, gate: bool }
        impl Host for Gate {
            fn call(&mut self, _: u32, _: &[u8], _: &mut Vm) -> Result<NativeReturn> {
                self.calls += 1;
                Ok(NativeReturn::Continue)
            }
            fn should_yield(&self) -> bool { self.gate && self.calls != 0 }
        }
        let mut code = Vec::new();
        for _ in 0..2 {
            code.push(NATIVE_CALL_OPCODE);
            for value in [0, 0] { let operand = Operand::immediate(value);
                code.push(operand.flags.bits()); code.extend_from_slice(&operand.payload.to_le_bytes()); }
        }
        code.push(0x3b);
        let mut vm = Vm::from_program(KcsProgram::from_code(&code, vec![0; 64], 32).unwrap()).unwrap();
        let mut host = Gate { calls: 0, gate: true };
        assert_eq!(vm.run(&mut host, 1000).unwrap(), Step::Yield);
        assert_eq!(host.calls, 1);
        assert_eq!(vm.instruction_pointer, 11);
        host.gate = false;
        assert_eq!(vm.run(&mut host, 1000).unwrap(), Step::Halted);
        assert_eq!(host.calls, 2);
    }

    #[test]
    fn compressed_containers_require_the_checksum_and_complete_input() {
        let empty = [0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        assert_eq!(inflate_exact("KCS", &empty, 0).unwrap(), Vec::<u8>::new());
        assert!(inflate_exact("KCS", &empty[..7], 0).is_err());
        let mut trailing = empty.to_vec();
        trailing.push(0);
        assert!(inflate_exact("KCS", &trailing, 0).is_err());
    }

    #[test]
    fn ordinary_message_continuation_writes_back_from_the_latest_receiver() {
        let mut code = Vec::new();
        instruction(&mut code, 0x25, &[immediate(0)]);
        instruction(&mut code, 0x2b, &[immediate(71), immediate(4)]);
        instruction(&mut code, 0x26, &[immediate(32)]);
        instruction(&mut code, 0x25, &[immediate(16)]);
        instruction(&mut code, 0x2b, &[immediate(71), immediate(4)]);
        code.push(0x3b);
        let program = KcsProgram::from_code(&code, vec![0; 36], 32).unwrap();
        let mut vm = Vm::from_program(program).unwrap();
        vm.write(20, &0x1234_5678u32.to_le_bytes()).unwrap();
        let mut host = |index, arguments: &[u8], vm: &mut Vm| {
            assert_eq!(index, 71);
            vm.wait_for_message(u32::from_le_bytes(arguments[..4].try_into().unwrap()))?;
            Ok(NativeReturn::YieldVoid)
        };
        assert!(matches!(vm.resume(&mut host, 8).unwrap(),
            ResumeStep::Completed { active: true, event: Some(ExecutionEvent { status: 4, .. }) }));
        let mut message = [9, 12, 3, 4];
        assert_eq!(vm.dispatch_message(&mut message, &mut host, 1).unwrap(), Some(ResumeStep::Yield));
        assert_eq!(vm.read_u32(32).unwrap(), 1);
        assert_eq!(vm.dispatch_message(&mut message, &mut host, 1).unwrap(), Some(ResumeStep::Yield));
        assert!(matches!(vm.dispatch_message(&mut message, &mut host, 1).unwrap(),
            Some(ResumeStep::Completed { active: true, .. })));
        assert_eq!(message, [9, 0x1234_5678, 3, 4]);
        assert!(vm.waiting_for_message());
        assert_eq!(vm.resume(&mut host, 8).unwrap(),
            ResumeStep::Completed { active: false, event: None });
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn ordinary_pause_preserves_wait_and_synchronous_message_bypasses_it() {
        let program = KcsProgram::from_code(&[0x3b], vec![0; 16], 16).unwrap();
        let mut vm = Vm::from_program(program).unwrap();
        vm.wait_for_message(0).unwrap();
        vm.set_execution_mode(0).unwrap();
        assert_eq!(vm.resume(&mut NullHost, 8).unwrap(), ResumeStep::Paused);
        assert!(vm.waiting_for_message());
        assert!(vm.stack_bytes().is_empty());
        let mut message = [1, 2, 3, 4];
        assert_eq!(vm.dispatch_message(&mut message, &mut NullHost, 8).unwrap(),
            Some(ResumeStep::Completed { active: false, event: None }));
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn ordinary_implicit_end_has_one_final_active_update() {
        let mut vm = Vm::from_program(KcsProgram::from_code(&[0], Vec::new(), 0).unwrap()).unwrap();
        assert_eq!(vm.resume(&mut NullHost, 1).unwrap(), ResumeStep::Yield);
        assert_eq!(vm.resume(&mut NullHost, 1).unwrap(),
            ResumeStep::Completed { active: true, event: None });
        assert_eq!(vm.resume(&mut NullHost, 1).unwrap(),
            ResumeStep::Completed { active: false, event: None });
    }
    use std::{fs, path::PathBuf};

    fn instruction(code: &mut Vec<u8>, opcode: u8, operands: &[Operand]) {
        code.push(opcode);
        for operand in operands {
            code.push(operand.flags.bits());
            code.extend_from_slice(&operand.payload.to_le_bytes());
        }
    }

    fn immediate(value: u32) -> Operand {
        Operand::immediate(value)
    }

    fn program(code: &[u8]) -> KcsProgram {
        KcsProgram::from_code(code, vec![0; 128], 512).unwrap()
    }

    #[test]
    fn legacy_cst_command_survives_argument_push_after_read() {
        let imm = Operand::immediate;
        let local = imm(0x14);
        let context = Operand::new(0x06, 0);
        let mut code = Vec::new();
        instruction(&mut code, 0x25, &[imm(64)]);
        instruction(&mut code, 0x2a, &[imm(18), imm(4)]);
        code.push(0x3b);
        instruction(&mut code, 0x31, &[imm(4)]);
        let buffer = code.len();
        instruction(&mut code, 0x31, &[imm(0x80)]);
        instruction(&mut code, 0x25, &[imm(0x30)]);
        code.push(0x33);
        let branch = code.len() as u32 + 11;
        instruction(&mut code, 0x34, &[imm(0x30), imm(branch)]);
        instruction(&mut code, 0x3e, &[context, imm(0)]);
        instruction(&mut code, 0x25, &[Operand::new(0x53, 0)]);
        instruction(&mut code, 0x3d, &[local]);
        for offset in [0x24, 0x28] {
            instruction(&mut code, 0x3e, &[context, Operand::new(0x23, offset)]);
            instruction(&mut code, 0x25, &[Operand::new(0x73, offset)]);
        }
        instruction(&mut code, 0x2b, &[imm(327), imm(16)]);
        code.push(0x3c);
        instruction(&mut code, 0x3d, &[local]);
        let expand = code.len();
        instruction(&mut code, 0x2b, &[imm(278), imm(4)]);
        code.push(0x3c);
        instruction(&mut code, 0x3d, &[imm(0)]);
        instruction(&mut code, 0x25, &[Operand::new(0xf1, 0)]);
        instruction(&mut code, 0x3d, &[local]);
        for address in [4, 8] {
            instruction(&mut code, 0x25, &[imm(address)]);
        }
        instruction(&mut code, 0x2b, &[imm(45), imm(16)]);
        code.push(0x3c);
        instruction(&mut code, 0x2c, &[imm(0), imm(4)]);

        let mut command = b"bg 4 quake".to_vec();
        command.extend(b" 5,-10,-4".repeat(26));
        command.push(0);
        let mut vm = Vm::from_program(KcsProgram::from_code(&code, vec![0; 512], 0x2800).unwrap()).unwrap();
        let mut host = |index, args: &[u8], vm: &mut Vm| {
            let address = |offset| u32::from_le_bytes(args[offset..offset + 4].try_into().unwrap());
            match index {
                327 => vm.write(address(4), &command)?,
                278 => assert_eq!(vm.read_string(address(0))?, command[..command.len() - 1]),
                45 => {
                    let mut text = vm.read_string(address(4))?;
                    text.push(0);
                    vm.write(address(0), &text)?;
                }
                _ => unreachable!(),
            }
            Ok(NativeReturn::Continue)
        };
        assert_eq!(vm.run(&mut host, 100).unwrap(), Step::Halted);
        assert_eq!(vm.read(64, command.len()).unwrap(), command);

        code[expand + 2..expand + 6].copy_from_slice(&279_u32.to_le_bytes());
        let unrelated = KcsProgram::from_code(&code, vec![0; 512], 0x2800).unwrap();
        assert_eq!(unrelated.instruction_at(buffer).unwrap().operands, [imm(0x80)]);
    }

    fn sample_root() -> Option<PathBuf> {
        if let Some(root) = std::env::var_os("CS2_SAMPLE_ROOT") {
            let root = PathBuf::from(root);
            let catalog = root.join("games");
            if catalog.is_dir() {
                return Some(catalog);
            }
            return root.is_dir().then_some(root);
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..")
            .join("games");
        root.is_dir().then_some(root)
    }

    fn workspace_file(relative: &str) -> Option<PathBuf> {
        sample_root().map(|root| root.join(relative))
    }

    fn collect_kcs_files(directory: &std::path::Path, output: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_kcs_files(&path, output);
            } else if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("kcs"))
            {
                output.push(path);
            }
        }
    }

    #[test]
    fn opcode_table_matches_kcs_dispatch_range() {
        assert_eq!(operand_count(0x00), Some(0));
        assert_eq!(operand_count(0x01), Some(2));
        assert_eq!(operand_count(0x25), Some(1));
        assert_eq!(operand_count(0x35), Some(3));
        assert_eq!(operand_count(0x44), Some(1));
        assert_eq!(operand_count(0x3f), None);
        assert_eq!(operand_count(0x40), None);
        assert_eq!(operand_count(0x45), None);
        assert_eq!(ACTUAL_OPCODES.len(), 67);
        assert!(
            ACTUAL_OPCODES
                .iter()
                .all(|opcode| operand_count(*opcode).is_some())
        );
        assert!(!ACTUAL_OPCODES.contains(&0x3f));
        assert!(!ACTUAL_OPCODES.contains(&0x40));
    }

    #[test]
    fn type_flags_expose_width_and_modes() {
        let flags = TypeFlags::new(0x71);
        assert_eq!(flags.scalar(), 1);
        assert!(flags.is_signed());
        assert!(flags.from_stack());
        assert!(flags.is_address());
        assert_eq!(flags.width().unwrap(), 1);
        assert_eq!(TypeFlags::new(0x84).width().unwrap(), 4);
        assert!(TypeFlags::new(0).width().is_err());
    }

    #[test]
    fn decoder_reads_fixed_five_byte_operands() {
        let mut code = Vec::new();
        instruction(&mut code, 0x01, &[immediate(2), immediate(3)]);
        code.push(0x3b);
        let program = program(&code);
        assert_eq!(program.instructions.len(), 2);
        assert_eq!(program.instructions[0].offset, 0);
        assert_eq!(program.instructions[0].operands[1].payload, 3);
        assert!(program.is_instruction_boundary(11));
        assert!(KcsProgram::from_code(&[0x01, 3], vec![], 16).is_err());
    }

    #[test]
    fn arithmetic_comparison_and_conversion_execute() {
        let mut code = Vec::new();
        instruction(&mut code, 0x01, &[immediate(7), immediate(5)]);
        instruction(&mut code, 0x13, &[immediate(12), immediate(12)]);
        instruction(&mut code, 0x36, &[immediate(2)]);
        code.push(0x3b);
        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = NullHost;
        assert_eq!(vm.run(&mut host, 64).unwrap(), Step::Halted);
        assert_eq!(
            vm.stack_bits().collect::<Vec<_>>(),
            vec![12, 1, 2.0f32.to_bits()]
        );
    }

    #[test]
    fn signed_float_arithmetic_comparison_and_conversion_execute() {
        let mut code = Vec::new();
        instruction(
            &mut code,
            0x01,
            &[
                Operand::new(4, 1.5f32.to_bits()),
                Operand::new(4, 2.25f32.to_bits()),
            ],
        );
        instruction(
            &mut code,
            0x10,
            &[Operand::new(0x23, u32::MAX), Operand::new(0x23, 1)],
        );
        instruction(&mut code, 0x39, &[Operand::new(4, 1.75f32.to_bits())]);
        instruction(
            &mut code,
            0x04,
            &[Operand::new(0x23, (-7i32) as u32), Operand::new(0x23, 2)],
        );
        code.push(0x3b);

        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = NullHost;
        assert_eq!(vm.run(&mut host, 64).unwrap(), Step::Halted);
        assert_eq!(
            vm.stack_bits().collect::<Vec<_>>(),
            vec![3.75f32.to_bits(), 1, 1, (-3i32) as u32]
        );
    }

    #[test]
    fn conditional_branch_discards_temporary_stack_values() {
        let mut code = Vec::new();
        instruction(&mut code, 0x25, &[immediate(0)]);
        instruction(&mut code, 0x27, &[immediate(18)]);
        instruction(&mut code, 0x25, &[immediate(99)]);
        instruction(&mut code, 0x25, &[immediate(1)]);
        code.push(0x3b);
        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = NullHost;
        assert_eq!(vm.run(&mut host, 64).unwrap(), Step::Halted);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn sequential_fetch_resumes_at_budget_and_rebuilds_after_jump() {
        let mut code = Vec::new();
        instruction(&mut code, 0x25, &[immediate(7)]);
        instruction(&mut code, 0x25, &[immediate(8)]);
        instruction(&mut code, 0x29, &[immediate(24)]);
        instruction(&mut code, 0x25, &[immediate(99)]);
        instruction(&mut code, 0x25, &[immediate(1)]);
        code.push(0x3b);
        let mut vm = Vm::from_program(program(&code)).unwrap();

        assert_eq!(vm.run(&mut NullHost, 2).unwrap(), Step::Yield);
        assert_eq!(vm.instruction_pointer, 12);
        assert_eq!(vm.run(&mut NullHost, 2).unwrap(), Step::Yield);
        assert_eq!(vm.instruction_pointer, 30);
        assert_eq!(vm.run(&mut NullHost, 2).unwrap(), Step::Halted);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn code_call_and_return_restore_the_call_frame() {
        let mut code = Vec::new();
        instruction(&mut code, 0x25, &[immediate(7)]);
        instruction(&mut code, 0x2a, &[immediate(30), immediate(4)]);
        code.push(0x3b);
        while code.len() < 30 {
            code.push(0x00);
        }
        instruction(&mut code, 0x31, &[immediate(4)]);
        instruction(&mut code, 0x25, &[immediate(42)]);
        instruction(&mut code, 0x2c, &[immediate(4), immediate(4)]);
        code.push(0x3b);
        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = NullHost;
        assert_eq!(vm.run(&mut host, 256).unwrap(), Step::Halted);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![42]);
        assert!(vm.frames().is_empty());
    }

    #[test]
    fn nested_call_restores_the_caller_base_separately_from_argument_base() {
        let mut code = Vec::new();
        instruction(&mut code, 0x2a, &[immediate(20), immediate(0)]);
        code.push(0x3b);
        while code.len() < 20 {
            code.push(0x00);
        }
        instruction(&mut code, 0x31, &[immediate(8)]);
        instruction(&mut code, 0x25, &[immediate(7)]);
        instruction(&mut code, 0x2a, &[immediate(60), immediate(4)]);
        instruction(&mut code, 0x2c, &[immediate(4), immediate(0)]);
        code.push(0x3b);
        while code.len() < 60 {
            code.push(0x00);
        }
        instruction(&mut code, 0x25, &[immediate(42)]);
        instruction(&mut code, 0x2c, &[immediate(4), immediate(4)]);
        code.push(0x3b);

        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = NullHost;
        assert_eq!(vm.run(&mut host, 128).unwrap(), Step::Halted);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![42]);
        assert!(vm.frames().is_empty());
    }

    #[test]
    fn return_uses_the_saved_frame_header_after_its_top_was_overwritten() {
        let mut code = Vec::new();
        instruction(&mut code, 0x2a, &[immediate(20), immediate(0)]);
        code.push(0x3b);
        while code.len() < 20 {
            code.push(0x00);
        }
        instruction(&mut code, 0x2c, &[immediate(0), immediate(0)]);
        code.push(0x3b);

        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = NullHost;
        assert_eq!(vm.run(&mut host, 1).unwrap(), Step::Yield);
        vm.write(136, &64u32.to_le_bytes()).unwrap();
        assert_eq!(vm.run(&mut host, 8).unwrap(), Step::Halted);
        assert!(vm.frames().is_empty());
        assert_eq!(vm.frame_top(), 64);
    }

    #[test]
    fn every_reference_opcode_reaches_a_defined_execution_path() {
        for &opcode in ACTUAL_OPCODES {
            let mut code = Vec::new();
            let operands = vec![immediate(0); operand_count(opcode).unwrap()];
            instruction(&mut code, opcode, &operands);
            code.push(0x3b);
            let mut vm = Vm::from_program(program(&code)).unwrap();
            let mut host = NullHost;
            let result = vm.run(&mut host, 8);
            assert!(
                !matches!(
                    result,
                    Err(ScriptError::UnimplementedOpcode { .. })
                        | Err(ScriptError::UnsupportedOpcode { .. })
                ) || opcode == 0x2b,
                "opcode {opcode:#04x} reached an unsupported path: {result:?}"
            );
            if opcode == 0x2b {
                assert!(matches!(
                    result,
                    Err(ScriptError::UnimplementedNativeCall { .. })
                ));
            }
        }
    }

    #[test]
    fn memory_address_width_sign_extension_and_external_region_are_explicit() {
        let mut vm = Vm::from_program(program(&[0x3b])).unwrap();
        vm.write(4, &[0x80, 0xff, 0x34, 0x12]).unwrap();
        assert_eq!(vm.value(Operand::new(0x31, 4)).unwrap(), 0xffff_ff80);
        assert_eq!(vm.value(Operand::new(0x12, 4)).unwrap(), 0xff80);
        assert_eq!(vm.value(Operand::new(0x13, 4)).unwrap(), 0x1234_ff80);
        assert_eq!(
            vm.read(0x8000_0000, 1),
            Err(ScriptError::MemoryOutOfBounds {
                address: 0x8000_0000,
                length: 1
            })
        );
        vm.load_program(program(&[0x3b]), vec![0xa5, 0x5a]).unwrap();
        assert_eq!(vm.read(0x8000_0000, 2).unwrap(), [0xa5, 0x5a]);
        assert_eq!(
            vm.read(0x8000_0001, 2),
            Err(ScriptError::MemoryOutOfBounds {
                address: 0x8000_0001,
                length: 2
            })
        );
    }

    #[test]
    fn assignment_to_stack_computed_0x56_target_writes_and_pushes_a_dword() {
        let mut code = Vec::new();
        instruction(&mut code, 0x3e, &[Operand::new(0x13, 4), immediate(8)]);
        instruction(&mut code, 0x15, &[Operand::new(0x56, 0), immediate(0x8877_6655)]);
        code.push(0x3b);

        let mut vm = Vm::from_program(program(&code)).unwrap();
        vm.write(4, &16u32.to_le_bytes()).unwrap();
        vm.write(23, &[0xa5, 0, 0, 0, 0, 0x5a]).unwrap();

        assert_eq!(vm.run(&mut NullHost, 8).unwrap(), Step::Halted);
        assert_eq!(vm.read(23, 6).unwrap(), [0xa5, 0x55, 0x66, 0x77, 0x88, 0x5a]);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), [0x8877_6655]);
        assert!(matches!(TypeFlags::new(0x56).width(), Err(ScriptError::InvalidTypeFlags { flags: 0x56 })));
    }

    #[test]
    fn stack_from_operand_and_local_frame_bounds_are_checked() {
        let mut code = Vec::new();
        instruction(&mut code, 0x25, &[immediate(0x1234_5678)]);
        instruction(&mut code, 0x2e, &[Operand::new(0x43, 0)]);
        code.push(0x3b);
        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = NullHost;
        assert_eq!(vm.run(&mut host, 8).unwrap(), Step::Halted);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), [0x1234_5678]);
        assert_eq!(vm.stack_pointer(), 132);

        let mut bad_frame = Vec::new();
        instruction(&mut bad_frame, 0x32, &[immediate(4)]);
        bad_frame.push(0x3b);
        let mut vm = Vm::from_program(program(&bad_frame)).unwrap();
        assert_eq!(vm.run(&mut host, 8), Err(ScriptError::InvalidLocalFrame));
    }

    #[test]
    fn arithmetic_and_conversion_boundaries_report_errors_instead_of_fallback_values() {
        assert_eq!(
            binary(
                0x20,
                i32::MIN as u32,
                u32::MAX,
                TypeFlags::new(0x23),
                TypeFlags::new(0x23),
                0x2a
            ),
            Err(ScriptError::IntegerOverflow)
        );
        assert_eq!(
            binary(0x45, 1, 2, TypeFlags::new(3), TypeFlags::new(3), 0x77),
            Err(ScriptError::UnsupportedOpcode {
                offset: 0x77,
                opcode: 0x45
            })
        );
        assert_eq!(
            convert(0x40, 1, 0x88),
            Err(ScriptError::UnsupportedOpcode {
                offset: 0x88,
                opcode: 0x40
            })
        );
        assert_eq!(
            compare(0x45, 1u32, 2u32, 0x99),
            Err(ScriptError::UnsupportedOpcode {
                offset: 0x99,
                opcode: 0x45
            })
        );
    }

    #[test]
    fn floating_point_nan_payload_and_zero_division_match_the_reference_contract() {
        let nan_left = 0x7fa1_2345;
        let nan_right = 0xffa5_4321;
        assert_eq!(
            float_binary(1, nan_left, 1.0f32.to_bits(), 0).unwrap(),
            nan_left | 0x0040_0000
        );
        assert_eq!(
            float_binary(1, 1.0f32.to_bits(), nan_right, 0).unwrap(),
            nan_right | 0x0040_0000
        );
        assert_eq!(
            float_binary(4, 1.0f32.to_bits(), 0x8000_0000, 0).unwrap(),
            0
        );
        assert_eq!(float_binary(4, 1.0f32.to_bits(), 0, 0).unwrap(), 0);
        assert_eq!(
            binary(
                0x20,
                1.0f32.to_bits(),
                0x8000_0000,
                TypeFlags::new(4),
                TypeFlags::new(4),
                0x11
            )
            .unwrap(),
            0x8000_0000
        );
    }

    #[test]
    fn cst_inputs_and_fes_sections_have_ordered_structural_cursors() {
        let cst = CstDocument {
            records: vec![
                CstRecord {
                    kind: 1,
                    text: b"a".to_vec(),
                },
                CstRecord {
                    kind: 2,
                    text: b"b".to_vec(),
                },
                CstRecord {
                    kind: 3,
                    text: b"c".to_vec(),
                },
            ],
            inputs: vec![0..2, 2..3],
        };
        let inputs = cst.inputs().collect::<Vec<_>>();
        assert_eq!(inputs.len(), 2);
        assert_eq!(
            inputs[0]
                .1
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(inputs[1].1[0].text, b"c");
        assert!(cst.input_records(2).is_none());

        let fes = FesDocument {
            sections: vec![
                FesSection {
                    header: b"one".to_vec(),
                    lines: vec![FesLine {
                        offset: 4,
                        bytes: b"x=1".to_vec(),
                    }],
                },
                FesSection {
                    header: b"two".to_vec(),
                    lines: vec![],
                },
            ],
        };
        let mut cursor = fes.cursor();
        assert!(matches!(
            cursor.next(&fes),
            Some(FesItem::Section { index: 0, .. })
        ));
        assert!(matches!(
            cursor.next(&fes),
            Some(FesItem::Line {
                section_index: 0,
                line_index: 0,
                ..
            })
        ));
        assert!(matches!(
            cursor.next(&fes),
            Some(FesItem::Section { index: 1, .. })
        ));
        assert_eq!(cursor.next(&fes), None);
    }

    struct TestHost {
        result: NativeReturn,
        calls: Vec<(u32, Vec<u8>)>,
    }

    impl Host for TestHost {
        fn call(&mut self, index: u32, arguments: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
            self.calls.push((index, arguments.to_vec()));
            assert_eq!(vm.native_arguments().unwrap(), arguments);
            Ok(self.result)
        }
    }

    #[test]
    fn native_host_receives_arguments_and_returns_value() {
        let mut code = Vec::new();
        instruction(&mut code, 0x25, &[immediate(7)]);
        instruction(&mut code, 0x2b, &[immediate(12), immediate(4)]);
        code.push(0x3b);
        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = TestHost {
            result: NativeReturn::Value(8),
            calls: Vec::new(),
        };
        assert_eq!(vm.run(&mut host, 64).unwrap(), Step::Halted);
        assert_eq!(host.calls, vec![(12, 7u32.to_le_bytes().to_vec())]);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![8]);
    }

    #[test]
    fn native_continue_fetches_from_a_replaced_program_counter() {
        let mut code = Vec::new();
        instruction(
            &mut code,
            NATIVE_CALL_OPCODE,
            &[immediate(30), immediate(0)],
        );
        instruction(&mut code, 0x25, &[immediate(99)]);
        instruction(&mut code, 0x25, &[immediate(42)]);
        code.push(0x3b);
        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = |index: u32, _arguments: &[u8], vm: &mut Vm| {
            assert_eq!(index, 30);
            vm.instruction_pointer = 17;
            Ok(NativeReturn::Continue)
        };

        assert_eq!(vm.run(&mut host, 8).unwrap(), Step::Halted);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![42]);
    }

    #[test]
    fn native_continue_fetches_from_a_replaced_program() {
        let mut code = Vec::new();
        instruction(
            &mut code,
            NATIVE_CALL_OPCODE,
            &[immediate(31), immediate(0)],
        );
        instruction(&mut code, 0x25, &[immediate(99)]);
        code.push(0x3b);
        let mut replacement = Vec::new();
        instruction(&mut replacement, 0x25, &[immediate(42)]);
        replacement.push(0x3b);
        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = |index: u32, _arguments: &[u8], vm: &mut Vm| {
            assert_eq!(index, 31);
            vm.load_program(program(&replacement), Vec::new())?;
            Ok(NativeReturn::Continue)
        };

        assert_eq!(vm.run(&mut host, 8).unwrap(), Step::Halted);
        assert_eq!(vm.program().unwrap().code, replacement);
        assert_eq!(vm.instruction_pointer, 6);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![42]);
    }

    #[test]
    fn instruction_lookup_preserves_boundaries_across_pages() {
        let mut code = vec![0; 63];
        instruction(&mut code, 0x29, &[immediate(70)]);
        code.extend([0, 0x3b]);
        let parsed = program(&code);
        assert_eq!(parsed.instruction_at(63).unwrap().opcode, 0x29);
        assert!((64..69).all(|offset| !parsed.is_instruction_boundary(offset)));
        assert_eq!(parsed.instruction_at(69).unwrap().opcode, 0);
        assert!(parsed.is_instruction_boundary(code.len()));
        assert!(parsed.instruction_at(code.len()).is_none());
        let mut vm = Vm::from_program(parsed).unwrap();
        let mut host = TestHost { result: NativeReturn::Continue, calls: Vec::new() };
        assert_eq!(vm.run(&mut host, 128).unwrap(), Step::Halted);
        assert_eq!(vm.instruction_pointer, 70);

        code[65..69].copy_from_slice(&64u32.to_le_bytes());
        assert!(matches!(KcsProgram::from_code(&code, Vec::new(), 0),
            Err(ScriptError::InvalidJump { target: 64 })));
    }

    #[test]
    fn native_retry_rewinds_the_call_instruction() {
        let mut code = Vec::new();
        instruction(&mut code, 0x2b, &[immediate(9), immediate(0)]);
        code.push(0x3b);
        let mut vm = Vm::from_program(program(&code)).unwrap();
        let mut host = TestHost {
            result: NativeReturn::Retry,
            calls: Vec::new(),
        };
        assert_eq!(
            vm.run(&mut host, 8).unwrap(),
            Step::Event(ExecutionEvent {
                native_index: 9,
                status: 5
            })
        );
        assert_eq!(vm.instruction_pointer, 0);
    }

    #[test]
    fn native_statuses_have_distinct_yield_return_and_retry_effects() {
        for (result, status, yields, values) in [
            (NativeReturn::Yield, 0, true, vec![]),
            (NativeReturn::Value(7), 1, false, vec![7]),
            (NativeReturn::Continue, 2, false, vec![]),
            (NativeReturn::YieldValue(8), 3, true, vec![8]),
            (NativeReturn::YieldVoid, 4, true, vec![]),
            (NativeReturn::Retry, 5, true, vec![]),
        ] {
            let mut code = Vec::new();
            instruction(&mut code, 0x2b, &[immediate(19), immediate(0)]);
            code.push(0x3b);
            let mut vm = Vm::from_program(program(&code)).unwrap();
            let mut host = TestHost {
                result,
                calls: Vec::new(),
            };
            let step = vm.run(&mut host, 8).unwrap();
            if yields {
                assert_eq!(
                    step,
                    Step::Event(ExecutionEvent {
                        native_index: 19,
                        status
                    })
                );
            } else {
                assert_eq!(step, Step::Halted);
            }
            assert_eq!(vm.stack_bits().collect::<Vec<_>>(), values);
            assert_eq!(
                vm.instruction_pointer,
                if result == NativeReturn::Retry { 0 } else { 11 }
            );
            assert_eq!(vm.native_arguments(), None);
        }
    }

    #[test]
    fn real_kcs_samples_decode() {
        let Some(root) = sample_root() else {
            return;
        };
        for relative in [
            "cs2_full_v401/system/ptcl/ase.kcs",
            "cs2_full_v401/system/ptcl/rain.kcs",
            "\u{725b}\u{987f}\u{4e0e}\u{82f9}\u{679c}\u{6811}/kcs/sscript.kcs",
        ] {
            let bytes = fs::read(root.join(relative)).unwrap();
            let program = KcsProgram::parse(&bytes).unwrap();
            assert_eq!(program.header.format_version, 2);
            assert!(!program.instructions.is_empty());
            assert_eq!(program.header.code_length as usize, program.code.len());
        }
    }

    #[test]
    fn every_loose_kcs_sample_decodes() {
        let Some(_) = sample_root() else {
            return;
        };
        let mut files = Vec::new();
        for root in [
            workspace_file("cs2_full_v401"),
            workspace_file("\u{725b}\u{987f}\u{4e0e}\u{82f9}\u{679c}\u{6811}"),
        ] {
            if let Some(root) = root {
                collect_kcs_files(&root, &mut files);
            }
        }
        assert!(files.len() >= 100);
        for path in &files {
            let bytes = fs::read(path).unwrap();
            KcsProgram::parse(&bytes).unwrap_or_else(|error| {
                panic!("failed to parse KCS sample {}: {error}", path.display())
            });
        }
    }

    #[test]
    fn real_cst_and_fes_samples_parse_into_records_and_sections() {
        let Some(cst_path) = workspace_file("cs2_full_v401/system/scene/start.cst") else {
            return;
        };
        let cst = CstDocument::parse(&fs::read(cst_path).unwrap()).unwrap();
        assert!(!cst.records.is_empty());

        let Some(fes_path) = workspace_file("cs2_full_v401/system/fes/title.fes") else {
            return;
        };
        let fes = FesDocument::parse(&fs::read(fes_path).unwrap()).unwrap();
        assert!(!fes.sections.is_empty());
    }
}
