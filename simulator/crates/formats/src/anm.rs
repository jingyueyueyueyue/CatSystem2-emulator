use crate::util::{MAX_BUFFER, MAX_ITEMS, u32_at};
use thiserror::Error;

pub const ANM_HEADER_SIZE: usize = 32;
pub const ANM_INSTRUCTION_SIZE: usize = 68;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnmVersion {
    FixedInstructionTable,
    LegacyEmpty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnmOperand {
    pub kind: u32,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnmInstruction {
    pub opcode: u32,
    pub operands: [AnmOperand; 8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnmFile {
    pub version: AnmVersion,
    pub reserved: u32,
    pub opaque_header: [u8; 20],
    pub instructions: Vec<AnmInstruction>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnmError {
    #[error("invalid ANM signature")]
    InvalidSignature,
    #[error("ANM file is truncated")]
    Truncated,
    #[error("ANM instruction table is invalid")]
    InvalidTable,
    #[error("ANM file exceeds the format reader limit")]
    TooLarge,
}

impl AnmFile {
    pub fn parse(data: &[u8]) -> Result<Self, AnmError> {
        if data.len() < 4 {
            return Err(AnmError::Truncated);
        }
        if &data[..4] != b"ANM\0" {
            return Err(AnmError::InvalidSignature);
        }
        if data.len() < ANM_HEADER_SIZE {
            return Err(AnmError::Truncated);
        }
        if data.len() > MAX_BUFFER {
            return Err(AnmError::TooLarge);
        }
        let reserved = u32_at(data, 4).ok_or(AnmError::Truncated)?;
        let count = usize::try_from(u32_at(data, 8).ok_or(AnmError::Truncated)?)
            .map_err(|_| AnmError::InvalidTable)?;
        if count == 0 {
            // Older CatSystem2 builds emit a 32-byte empty animation with no
            // instruction table. A zero count in any other-sized file is
            // malformed rather than an alternate table layout.
            if data.len() != ANM_HEADER_SIZE {
                return Err(AnmError::InvalidTable);
            }
            return Ok(Self {
                version: AnmVersion::LegacyEmpty,
                reserved,
                opaque_header: data[12..ANM_HEADER_SIZE]
                    .try_into()
                    .map_err(|_| AnmError::Truncated)?,
                instructions: Vec::new(),
            });
        }
        if count > MAX_ITEMS {
            return Err(AnmError::InvalidTable);
        }
        let table_length = count
            .checked_mul(ANM_INSTRUCTION_SIZE)
            .and_then(|length| ANM_HEADER_SIZE.checked_add(length))
            .ok_or(AnmError::InvalidTable)?;
        if data.len() < table_length {
            return Err(AnmError::Truncated);
        }

        let mut instructions = Vec::with_capacity(count);
        for index in 0..count {
            let offset = ANM_HEADER_SIZE
                .checked_add(
                    index
                        .checked_mul(ANM_INSTRUCTION_SIZE)
                        .ok_or(AnmError::InvalidTable)?,
                )
                .ok_or(AnmError::InvalidTable)?;
            let record_end = offset
                .checked_add(ANM_INSTRUCTION_SIZE)
                .ok_or(AnmError::InvalidTable)?;
            let record = data.get(offset..record_end).ok_or(AnmError::Truncated)?;
            let opcode = u32_at(record, 0).ok_or(AnmError::Truncated)?;
            let mut operands = [AnmOperand::default(); 8];
            for (operand_index, operand) in operands.iter_mut().enumerate() {
                let operand_offset = 4usize
                    .checked_add(operand_index.checked_mul(8).ok_or(AnmError::InvalidTable)?)
                    .ok_or(AnmError::InvalidTable)?;
                *operand = AnmOperand {
                    kind: u32_at(record, operand_offset).ok_or(AnmError::Truncated)?,
                    value: u32_at(record, operand_offset + 4).ok_or(AnmError::Truncated)?,
                };
            }
            instructions.push(AnmInstruction { opcode, operands });
        }
        Ok(Self {
            version: AnmVersion::FixedInstructionTable,
            reserved,
            opaque_header: data[12..ANM_HEADER_SIZE]
                .try_into()
                .map_err(|_| AnmError::Truncated)?,
            instructions,
        })
    }
}
