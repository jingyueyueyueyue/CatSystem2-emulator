use thiserror::Error;

use crate::util::{MAX_BUFFER, MAX_ITEMS, c_string, inflate_exact, u32_at};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KcsHeader {
    pub version: u32,
    pub program_version: u32,
    pub code_length: u32,
    pub string_init_offset: u32,
    pub string_init_count: u32,
    pub string_init_length: u32,
    pub data_length: u32,
    pub stack_length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KcsInstruction {
    pub offset: u32,
    pub opcode: u8,
    pub operands: Vec<[u8; 5]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KcsFile {
    pub header: KcsHeader,
    pub code: Vec<u8>,
    pub instructions: Vec<KcsInstruction>,
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum KcsError {
    #[error("KCS file is truncated")]
    Truncated,
    #[error("KCS file exceeds the format reader limit")]
    TooLarge,
    #[error("invalid KCS signature")]
    InvalidSignature,
    #[error("KCS compressed body is invalid")]
    InvalidCompression,
    #[error("KCS instruction stream is invalid at offset {0:#x}")]
    InvalidInstruction(u32),
    #[error("KCS header is invalid")]
    InvalidHeader,
    #[error("KCS data table is invalid")]
    InvalidData,
}

impl KcsFile {
    pub fn parse(data: &[u8]) -> Result<Self, KcsError> {
        if data.len() > MAX_BUFFER {
            return Err(KcsError::TooLarge);
        }
        if data.get(..4) != Some(b"KCS\0") {
            return Err(KcsError::InvalidSignature);
        }
        if u32_at(data, 4).ok_or(KcsError::Truncated)? != 2
            || u32_at(data, 12).ok_or(KcsError::Truncated)? != 52
        {
            return Err(KcsError::InvalidHeader);
        }

        let code_length = u32_at(data, 16).ok_or(KcsError::Truncated)? as usize;
        let string_init_offset = u32_at(data, 20).ok_or(KcsError::Truncated)?;
        let string_init_count = u32_at(data, 24).ok_or(KcsError::Truncated)? as usize;
        let string_init_length = u32_at(data, 28).ok_or(KcsError::Truncated)? as usize;
        let data_length = u32_at(data, 32).ok_or(KcsError::Truncated)? as usize;
        let stack_length = u32_at(data, 36).ok_or(KcsError::Truncated)? as usize;
        let data_offset = u32_at(data, 40).ok_or(KcsError::Truncated)? as usize;
        let capacity = u32_at(data, 48).ok_or(KcsError::Truncated)? as usize;
        let body_length = code_length
            .checked_add(string_init_length)
            .ok_or(KcsError::InvalidHeader)?;

        if string_init_offset as usize
            != 52usize
                .checked_add(code_length)
                .ok_or(KcsError::InvalidHeader)?
            || data_offset
                != 52usize
                    .checked_add(code_length)
                    .and_then(|value| value.checked_add(string_init_length))
                    .ok_or(KcsError::InvalidHeader)?
            || capacity > MAX_BUFFER
            || body_length > capacity
            || string_init_count > MAX_ITEMS
            || string_init_count
                .checked_mul(8)
                .is_none_or(|length| length > string_init_length)
            || data_length
                .checked_add(stack_length)
                .is_none_or(|length| length > MAX_BUFFER)
            || data_offset > u32::MAX as usize
        {
            return Err(KcsError::InvalidHeader);
        }

        let compressed_body = data.get(52..).ok_or(KcsError::Truncated)?;
        if compressed_body.is_empty() && body_length != 0 {
            return Err(KcsError::Truncated);
        }
        let body =
            inflate_exact(compressed_body, body_length).ok_or(KcsError::InvalidCompression)?;
        let code = body[..code_length].to_vec();
        let init = &body[code_length..];

        let mut initial_data = vec![0; data_length];
        let init_table_length = string_init_count
            .checked_mul(8)
            .ok_or(KcsError::InvalidData)?;
        for index in 0..string_init_count {
            let source = u32_at(init, index * 8).ok_or(KcsError::Truncated)? as usize;
            let target = u32_at(init, index * 8 + 4).ok_or(KcsError::Truncated)? as usize;
            if source < init_table_length {
                return Err(KcsError::InvalidData);
            }
            let string = c_string(init, source).ok_or(KcsError::InvalidData)?;
            if source
                .checked_add(string.len())
                .is_none_or(|end| end > init.len())
            {
                return Err(KcsError::InvalidData);
            }
            let target_end = target
                .checked_add(string.len() + 1)
                .ok_or(KcsError::InvalidData)?;
            if target_end > initial_data.len() {
                return Err(KcsError::InvalidData);
            }
            initial_data[target..target + string.len()].copy_from_slice(string);
            initial_data[target + string.len()] = 0;
        }

        let instructions = parse_instructions(&code)?;
        validate_static_targets(&code, &instructions)?;
        Ok(Self {
            header: KcsHeader {
                version: 2,
                program_version: u32_at(data, 8).ok_or(KcsError::Truncated)?,
                code_length: code_length as u32,
                string_init_offset,
                string_init_count: string_init_count as u32,
                string_init_length: string_init_length as u32,
                data_length: data_length as u32,
                stack_length: stack_length as u32,
            },
            code,
            instructions,
            data: initial_data,
        })
    }
}

pub fn operand_count(opcode: u8) -> Option<usize> {
    Some(match opcode {
        0x00 | 0x2d | 0x33 | 0x3a..=0x3c => 0,
        0x08
        | 0x0c
        | 0x16..=0x1a
        | 0x25..=0x29
        | 0x2e..=0x2f
        | 0x31..=0x32
        | 0x36..=0x39
        | 0x3d
        | 0x42
        | 0x44 => 1,
        0x01..=0x07
        | 0x09..=0x0b
        | 0x0d..=0x15
        | 0x1b..=0x24
        | 0x2a..=0x2c
        | 0x30
        | 0x34
        | 0x3e
        | 0x41
        | 0x43 => 2,
        0x35 => 3,
        _ => return None,
    })
}

fn parse_instructions(code: &[u8]) -> Result<Vec<KcsInstruction>, KcsError> {
    let mut instructions = Vec::new();
    let mut offset = 0usize;
    while offset < code.len() {
        if instructions.len() == MAX_ITEMS {
            return Err(KcsError::InvalidInstruction(offset as u32));
        }
        let instruction_offset = offset as u32;
        let opcode = code[offset];
        offset += 1;
        let count =
            operand_count(opcode).ok_or(KcsError::InvalidInstruction(instruction_offset))?;
        let operand_bytes = count
            .checked_mul(5)
            .ok_or(KcsError::InvalidInstruction(instruction_offset))?;
        if offset
            .checked_add(operand_bytes)
            .is_none_or(|end| end > code.len())
        {
            return Err(KcsError::InvalidInstruction(instruction_offset));
        }
        let mut operands = Vec::with_capacity(count);
        for _ in 0..count {
            let mut operand = [0; 5];
            operand.copy_from_slice(&code[offset..offset + 5]);
            operands.push(operand);
            offset += 5;
        }
        instructions.push(KcsInstruction {
            offset: instruction_offset,
            opcode,
            operands,
        });
    }
    Ok(instructions)
}

fn validate_static_targets(code: &[u8], instructions: &[KcsInstruction]) -> Result<(), KcsError> {
    for instruction in instructions {
        let target_operand = match instruction.opcode {
            0x27..=0x2a => Some(0),
            0x34 => Some(1),
            0x35 => Some(2),
            _ => None,
        };
        let Some(target_operand) = target_operand else {
            continue;
        };
        let operand = instruction.operands[target_operand];
        if operand[0] != 3 {
            continue;
        }
        let target = u32::from_le_bytes(operand[1..5].try_into().unwrap()) as usize;
        if target != code.len()
            && instructions
                .binary_search_by_key(&target, |candidate| candidate.offset as usize)
                .is_err()
        {
            return Err(KcsError::InvalidInstruction(instruction.offset));
        }
    }
    Ok(())
}
