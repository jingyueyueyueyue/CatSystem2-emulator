//! Name-table file matching and byte templates, recovered from the original PE.
//! These are not KCS printf formats. Input/output may alias VM storage.
use cs2_script::{Result, Vm};

use super::native_state_error;

const MAX_WORK: usize = 64 * 1024 * 1024;

pub(super) fn matches(input: &[u8], pattern: &[u8]) -> Result<bool> {
    if input.len() >= 256 || pattern.len() >= 256 {
        return Err(native_state_error("native 639 name exceeds its 255-byte scratch buffer"));
    }
    let input = &input[..input.len().min(pattern.len())];
    let text_extension = extension(input);
    let pattern_extension = extension(pattern);
    // 0x5ffffd/0x600007 subtract extension pointers from their bases.
    // Preserve that unsigned length (and the comparator's strlen clamp),
    // rather than replacing it with an intended basename length.
    let text_length = input.len().min(0u32.wrapping_sub(text_extension as u32) as usize);
    let pattern_length = pattern.len().min(0u32.wrapping_sub(pattern_extension as u32) as usize);
    Ok(segment(&input[..text_length], &pattern[..pattern_length], pattern)
        && segment(&input[text_extension..], &pattern[pattern_extension..], &pattern[pattern_extension..]))
}

fn extension(bytes: &[u8]) -> usize {
    for index in (0..bytes.len()).rev() {
        match bytes[index] {
            b'/' | b'\\' => return bytes.len(),
            b'.' => return index + 1,
            _ => {}
        }
    }
    bytes.len()
}

fn segment(text: &[u8], pattern: &[u8], complete_pattern: &[u8]) -> bool {
    if complete_pattern == b"*" { return true; }
    for (index, &byte) in text.iter().enumerate() {
        match pattern.get(index) {
            Some(b'*') => return true,
            Some(b'?') => {}
            Some(&expected) if byte == expected => {}
            _ => return false,
        }
    }
    text.len() == pattern.len()
}

pub(super) fn format(
    vm: &mut Vm, input: u32, output: u32, replacement: &[u8], template: &[u8],
) -> Result<()> {
    let mut writer = Writer { vm, input, output, length: 0, remaining: MAX_WORK };
    let mut cursor = 0;
    while let Some(&byte) = template.get(cursor).filter(|&&byte| byte != 0) {
        if byte != b'%' {
            writer.emit(byte)?;
        } else {
            match template.get(cursor + 1).copied().unwrap_or(0) {
                b'f' => {
                    for &byte in replacement.iter().take_while(|&&byte| byte != 0) {
                        writer.emit(byte)?;
                    }
                    cursor += 1;
                }
                b'c' => {
                    // The source position is the current *output* byte count.
                    let byte = writer.input_byte(writer.length)?;
                    writer.emit(byte)?;
                    cursor += 1;
                }
                b's' => {
                    writer.copy_input(0)?;
                    cursor += 1;
                }
                b'r' => {
                    writer.copy_input(writer.length)?;
                    cursor += 1;
                }
                b'n' => {
                    let mut index = 0;
                    loop {
                        let byte = writer.input_byte(index)?;
                        if byte == 0 || byte == b',' || byte.is_ascii_digit() { break; }
                        index += 1;
                    }
                    loop {
                        let byte = writer.input_byte(index)?;
                        if !byte.is_ascii_digit() { break; }
                        writer.emit(byte)?;
                        index += 1;
                    }
                    cursor += 1;
                }
                _ => writer.emit(b'%')?,
            }
        }
        cursor += 1;
    }
    writer.store(writer.output, writer.length, &[0])?;
    writer.append_comma_suffix()
}

struct Writer<'a> {
    vm: &'a mut Vm,
    input: u32,
    output: u32,
    length: usize,
    remaining: usize,
}

impl Writer<'_> {
    fn work(&mut self, count: usize) -> Result<()> {
        self.remaining = self.remaining.checked_sub(count).ok_or_else(|| native_state_error(
            "native 640 byte-operation limit exceeded (possible overlapping string feedback)",
        ))?;
        Ok(())
    }

    fn address(base: u32, index: usize) -> Result<u32> {
        u32::try_from(index).ok().and_then(|index| base.checked_add(index))
            .ok_or_else(|| native_state_error("native 640 VM address overflow"))
    }

    fn byte(&mut self, base: u32, index: usize) -> Result<u8> {
        self.work(1)?;
        Ok(self.vm.read(Self::address(base, index)?, 1)?[0])
    }

    fn input_byte(&mut self, index: usize) -> Result<u8> {
        self.byte(self.input, index)
    }

    fn store(&mut self, base: u32, index: usize, bytes: &[u8]) -> Result<()> {
        self.work(bytes.len())?;
        self.vm.write(Self::address(base, index)?, bytes)
    }

    fn emit(&mut self, byte: u8) -> Result<()> {
        self.store(self.output, self.length, &[byte])?;
        self.length += 1;
        Ok(())
    }

    fn copy_input(&mut self, mut index: usize) -> Result<()> {
        loop {
            let byte = self.input_byte(index)?;
            if byte == 0 { return Ok(()); }
            self.emit(byte)?;
            index += 1;
        }
    }

    fn length_at(&mut self, base: u32, start: usize) -> Result<usize> {
        let mut length = 0;
        while self.byte(base, start + length)? != 0 { length += 1; }
        Ok(length)
    }

    fn append_comma_suffix(&mut self) -> Result<()> {
        let mut byte = self.input_byte(0)?;
        if (byte as i8) <= 0 { return Ok(()); }
        let mut index = 0;
        loop {
            if byte == b',' { break; }
            index += 1;
            byte = self.input_byte(index)?;
            // 0x5a7383..0x5a7388 compares byte index against the signed
            // current character, rather than comparing against strlen.
            if index as i32 >= i32::from(byte as i8) { return Ok(()); }
        }
        let length = self.length_at(self.input, index)? + 1;
        let end = self.length_at(self.output, 0)?;
        let mut copied = 0;
        // REP MOVSD followed by REP MOVSB has DWORD-sized alias feedback.
        while length - copied >= 4 {
            self.work(4)?;
            let bytes = self.vm.read(Self::address(self.input, index + copied)?, 4)?;
            self.store(self.output, end + copied, &bytes)?;
            copied += 4;
        }
        while copied < length {
            let byte = self.input_byte(index + copied)?;
            self.store(self.output, end + copied, &[byte])?;
            copied += 1;
        }
        Ok(())
    }
}
