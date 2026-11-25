//! Ordinary string wrappers recovered directly from versioned PE bytes.
use std::collections::BTreeMap;

use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, cp932_char_width, native_state_error, read_u32, trim_nul};

const MAX_EXPANDED_BYTES: usize = 64 * 1024 * 1024;

fn string_address(base: u32, offset: usize) -> Result<u32> {
    let offset = u32::try_from(offset)
        .map_err(|_| native_state_error("string native offset exceeds address space"))?;
    base.checked_add(offset)
        .ok_or_else(|| native_state_error("string native address overflow"))
}

fn string_byte(vm: &Vm, address: u32) -> Result<u8> {
    Ok(vm.read(address, 1)?[0])
}

/// 600a00 / 577450 inspect a second byte only for the two lead-byte ranges.
/// A query or an in-place prefix operation does not require a whole C string.
fn string_char_width(vm: &Vm, address: u32) -> Result<usize> {
    let first = string_byte(vm, address)?;
    if matches!(first, 0x81..=0x9f | 0xe0..=0xef)
        && matches!(string_byte(vm, string_address(address, 1)?)?,
            0x40..=0x7e | 0x80..=0xfc)
    {
        Ok(2)
    } else {
        Ok(1)
    }
}

fn string_length(vm: &Vm, address: u32) -> Result<usize> {
    let length = vm.read_string(address)?.len();
    if length > MAX_EXPANDED_BYTES {
        return Err(native_state_error("string native exceeds storage limit"));
    }
    Ok(length)
}

/// Success is separate from the numeric value: the engine accepts bare 0x
/// as zero, but rejects leading whitespace and a decimal digit run plus '.'.
/// Accumulation itself reuses the recovered DWORD-wrapping format parser.
fn string_integer_prefix(bytes: &[u8]) -> Option<u32> {
    let bytes = trim_nul(bytes);
    let sign = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let digits = bytes.get(sign..)?;
    if !digits.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    if !digits.starts_with(b"0x") && !digits.starts_with(b"0X") {
        let count = digits.iter().take_while(|byte| byte.is_ascii_digit()).count();
        if digits.get(count) == Some(&b'.') { return None; }
    }
    Some(cs2_formats::config::parse_integer_prefix(bytes) as u32)
}

/// The wrappers read fixed fields at their parameter pointer, independently
/// of the call instruction's stack-pop count. Short calls use existing VM
/// stack storage, never zero-filled or invented parameters.
fn string_packet(args: &[u8], vm: &Vm, length: usize) -> Result<Vec<u8>> {
    if args.len() >= length {
        Ok(args[..length].to_vec())
    } else {
        Ok(vm.native_argument_storage(length)?.to_vec())
    }
}

fn string_set_contains(vm: &Vm, address: u32, byte: u8) -> Result<bool> {
    for offset in 0..MAX_EXPANDED_BYTES {
        let candidate = string_byte(vm, string_address(address, offset)?)?;
        if candidate == 0 {
            return Ok(false);
        }
        if candidate == byte {
            return Ok(true);
        }
    }
    Err(native_state_error("string native delimiter set exceeds storage limit"))
}

/// These receivers copy live VM bytes, including the first NUL. In particular,
/// an aliased destination is allowed to change subsequent source reads.
fn copy_string_forward(vm: &mut Vm, target: u32, source: u32) -> Result<()> {
    for offset in 0..MAX_EXPANDED_BYTES {
        let byte = string_byte(vm, string_address(source, offset)?)?;
        vm.write(string_address(target, offset)?, &[byte])?;
        if byte == 0 {
            return Ok(());
        }
    }
    Err(native_state_error("string native copy does not terminate within storage limit"))
}

/// Matches the bounded forward DWORD/remainder copies in 15, 44 and 45. A full
/// source snapshot would change the original ordering for overlapping spans.
fn copy_string_words(vm: &mut Vm, target: u32, source: u32, length: usize) -> Result<()> {
    if length > MAX_EXPANDED_BYTES {
        return Err(native_state_error("string native copy exceeds storage limit"));
    }
    let words = length / 4 * 4;
    for offset in (0..words).step_by(4) {
        let value = vm.read(string_address(source, offset)?, 4)?;
        vm.write(string_address(target, offset)?, &value)?;
    }
    for offset in words..length {
        let value = string_byte(vm, string_address(source, offset)?)?;
        vm.write(string_address(target, offset)?, &[value])?;
    }
    Ok(())
}

enum ParenthesizedOutput<'a> {
    Vm(u32),
    Scratch(&'a mut Vec<u8>),
}

/// 43 writes back into VM storage; 519 calls the SAME PE helper with an
/// independent output buffer, after clearing its final VM destination.
struct ParenthesizedTable<'a> {
    vm: &'a mut Vm,
    source: u32,
    separators: Option<u32>,
    output: ParenthesizedOutput<'a>,
}

impl ParenthesizedTable<'_> {
    fn source_byte(&self, offset: u32) -> Result<u8> {
        string_byte(self.vm, self.source.wrapping_add(offset))
    }

    fn output_byte(&self, offset: u32) -> Result<u8> {
        match &self.output {
            ParenthesizedOutput::Vm(base) => string_byte(self.vm, base.wrapping_add(offset)),
            ParenthesizedOutput::Scratch(bytes) => bytes.get(offset as usize).copied()
                .ok_or_else(|| native_state_error("parenthesized table reads unwritten output")),
        }
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<()> {
        match &mut self.output {
            ParenthesizedOutput::Vm(base) => self.vm.write(base.wrapping_add(offset), bytes),
            ParenthesizedOutput::Scratch(output) => {
                let start = offset as usize;
                let end = start.checked_add(bytes.len()).filter(|&end| end <= MAX_EXPANDED_BYTES)
                    .ok_or_else(|| native_state_error("parenthesized table exceeds storage limit"))?;
                if start > output.len() {
                    return Err(native_state_error("parenthesized table skips unwritten output"));
                }
                if end > output.len() { output.resize(end, 0); }
                output[start..end].copy_from_slice(bytes);
                Ok(())
            }
        }
    }

    fn separates(&self, byte: u8) -> Result<bool> {
        match self.separators {
            Some(address) => string_set_contains(self.vm, address, byte),
            None => Ok(byte == b' '),
        }
    }
}

/// 43/519's nested copier (Yukikoi 5ff410 / v401 577dc0). Every nesting level
/// writes WORD0 when it returns, before its parent reads the next source byte.
/// One shared offset and depth preserve this order without host recursion or
/// a snapshot of the input. Quotes and CP932 trail bytes receive no exemption.
fn copy_parenthesized_live(table: &mut ParenthesizedTable<'_>, target: u32, source: u32) -> Result<u32> {
    if table.source_byte(source)? != b'(' {
        return Ok(0);
    }
    table.write(target, b"(")?;
    let mut offset = 1_u32;
    let mut depth = 1_u32;
    for _ in 0..MAX_EXPANDED_BYTES {
        let byte = table.source_byte(source.wrapping_add(offset))?;
        match byte {
            b'(' => {
                table.write(target.wrapping_add(offset), b"(")?;
                depth += 1;
                offset = offset.wrapping_add(1);
            }
            0 | b')' => {
                if byte == b')' {
                    table.write(target.wrapping_add(offset), b")")?;
                    offset = offset.wrapping_add(1);
                }
                table.write(target.wrapping_add(offset), &[0, 0])?;
                depth -= 1;
                if depth == 0 {
                    return Ok(offset);
                }
            }
            _ => {
                table.write(target.wrapping_add(offset), &[byte])?;
                offset = offset.wrapping_add(1);
            }
        }
    }
    Err(native_state_error("native43/519 nested copy exceeds storage limit"))
}

/// 43's caller uses DWORD offsets, including the subtraction by three for an
/// unclosed (: group. Resolve each resulting address through the VM instead
/// of saturating the offset or manufacturing a closing parenthesis.
fn split_parenthesized_live(
    vm: &mut Vm, target: u32, source: u32, separators: u32,
) -> Result<()> {
    split_parenthesized_table(&mut ParenthesizedTable {
        vm, source, separators: Some(separators), output: ParenthesizedOutput::Vm(target),
    })
}

fn split_parenthesized_table(table: &mut ParenthesizedTable<'_>) -> Result<()> {
    let mut input = 0_u32;
    let mut output = 0_u32;
    let mut previous = 0_u8;
    let mut separated = true;
    for _ in 0..MAX_EXPANDED_BYTES {
        let byte = table.source_byte(input)?;
        if byte == 0 {
            table.write(output, &[0, 0])?;
            return Ok(());
        }
        if byte == b'(' {
            // Capture this marker before a separating zero can alias it.
            let flatten = table.source_byte(input.wrapping_add(1))? == b':';
            if previous.is_ascii_alphabetic() && !separated {
                table.write(output, &[0])?;
                output = output.wrapping_add(1);
            }
            let target_at = output;
            let copied = copy_parenthesized_live(table, target_at, input)?;
            // An injected separator may already have overwritten this '('.
            // The copier then returns zero; the outer live NUL check ends
            // normally, including the original (: offset subtraction below.
            if flatten {
                if copied > 3 {
                    // Forward REP MOVSB reads already-written output bytes,
                    // not the original source or an independently parsed body.
                    for at in 0..copied - 3 {
                        let byte = table.output_byte(target_at.wrapping_add(at).wrapping_add(2))?;
                        table.write(target_at.wrapping_add(at), &[byte])?;
                    }
                }
                output = output.wrapping_add(copied);
                if copied >= 2 {
                    table.write(output.wrapping_sub(3), &[0])?;
                }
                output = output.wrapping_sub(3);
            } else {
                output = output.wrapping_add(copied);
            }
            input = input.wrapping_add(copied);
            separated = false;
            // Both PE receivers retain the preceding ordinary-byte state.
        } else {
            if table.separates(byte)? {
                if !separated {
                    table.write(output, &[0])?;
                    output = output.wrapping_add(1);
                    separated = true;
                    previous = 0;
                }
            } else {
                table.write(output, &[byte])?;
                output = output.wrapping_add(1);
                separated = false;
                // The saved byte is reloaded after the write in both PEs.
                previous = table.source_byte(input)?;
            }
            input = input.wrapping_add(1);
        }
    }
    Err(native_state_error("native43/519 splitting exceeds storage limit"))
}

/// 45's helper searches every byte, unlike ordinary49's CP932 boundaries.
fn find_live_string(vm: &Vm, source: u32, needle: u32) -> Result<Option<usize>> {
    for offset in 0..MAX_EXPANDED_BYTES {
        if string_byte(vm, string_address(source, offset)?)? == 0 {
            return Ok(None);
        }
        let mut matched = 0;
        loop {
            let expected = string_byte(vm, string_address(needle, matched)?)?;
            if expected == 0 {
                return Ok(Some(offset));
            }
            if matched == MAX_EXPANDED_BYTES {
                return Err(native_state_error("string native search exceeds storage limit"));
            }
            let actual = string_byte(vm, string_address(source, offset + matched)?)?;
            if actual == 0 || actual != expected {
                break;
            }
            matched += 1;
        }
    }
    Err(native_state_error("string native search does not terminate within storage limit"))
}

/// The byte-only shared-string receiver at Yukikoi 0x699c30. It is separate
/// from KCS address formatting and from history attachment lookup (336/337).
/// A parsed reference consumes its optional semicolon even when absent/empty;
/// replacement bytes are C strings and are not scanned recursively.
pub(super) fn expand_shared_strings(
    input: &[u8], strings: &BTreeMap<u32, Vec<u8>>,
) -> Result<Vec<u8>> {
    expand_shared_strings_observed(input, strings, |_, _| {})
}

fn expand_shared_strings_observed(
    input: &[u8], strings: &BTreeMap<u32, Vec<u8>>,
    mut observe: impl FnMut(u32, &[u8]),
) -> Result<Vec<u8>> {
    let input = trim_nul(input);
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        if input[cursor..].starts_with(b"$str") {
            if let Some((length, id)) = super::cst_context_native::integer_prefix(&input[cursor + 4..]) {
                if let Some(value) = strings.get(&id) {
                    let value = trim_nul(value);
                    observe(id, value);
                    if output.len().checked_add(value.len())
                        .is_none_or(|length| length > MAX_EXPANDED_BYTES)
                    {
                        return Err(native_state_error("shared string expansion exceeds storage limit"));
                    }
                    output.extend_from_slice(value);
                }
                cursor += 4 + length;
                cursor += usize::from(input.get(cursor) == Some(&b';'));
                continue;
            }
        }
        if output.len() == MAX_EXPANDED_BYTES {
            return Err(native_state_error("shared string expansion exceeds storage limit"));
        }
        output.push(input[cursor]);
        cursor += 1;
    }
    Ok(output)
}

impl LoggingHost<'_> {
    /// Ordinary13..16 all read (target, source) independently of the pop count
    /// and return the target VM address. Even slots first format into a
    /// separate byte buffer; raw copies keep reading the original live VM.
    pub(super) fn string_copy_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        if !(13..=16).contains(&index) {
            return Err(native_state_error("unsupported string copy native"));
        }
        let packet = string_packet(args, vm, 8)?;
        let target = read_u32(&packet, 0)?;
        let source = read_u32(&packet, 4)?;
        if index == 13 {
            for offset in 0..MAX_EXPANDED_BYTES {
                let byte = string_byte(vm, string_address(source, offset)?)?;
                if byte == 0 {
                    vm.write(string_address(target, offset)?, &[0, 0])?;
                    return Ok(NativeReturn::Value(target));
                }
                vm.write(string_address(target, offset)?, &[byte])?;
            }
            return Err(native_state_error("native13 copy exceeds storage limit"));
        }
        if index == 15 {
            // Source length is captured before scanning the destination. The
            // original then copies forward DWORDs and leftover bytes, NUL
            // included, and rescans the result before writing its second NUL.
            let length = string_length(vm, source)?;
            let offset = string_length(vm, target)?;
            copy_string_words(vm, string_address(target, offset)?, source, length + 1)?;
        } else {
            let input = vm.read_string(source)?;
            let formatted = super::expand_kcs_format(vm, &input, 0)?;
            let text = trim_nul(&formatted);
            if text.len() > MAX_EXPANDED_BYTES {
                return Err(native_state_error("formatted string copy exceeds storage limit"));
            }
            let offset = if index == 16 { string_length(vm, target)? } else { 0 };
            for (at, &byte) in text.iter().enumerate() {
                vm.write(string_address(target, offset + at)?, &[byte])?;
            }
            if index == 14 {
                // Both 592dc2 and v401 523935 write WORD0, including empty
                // expansions; the old formatted-copy branch wrote only BYTE0.
                vm.write(string_address(target, text.len())?, &[0, 0])?;
                return Ok(NativeReturn::Value(target));
            }
            vm.write(string_address(target, offset + text.len())?, &[0])?;
        }
        let length = string_length(vm, target)?;
        vm.write(string_address(target, length)?, &[0, 0])?;
        Ok(NativeReturn::Value(target))
    }

    /// Fixed fields, rather than KCS pop counts, select the comparison and
    /// character receivers. No temporary source snapshot is needed for cuts.
    pub(super) fn string_scalar_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        if !(19..=24).contains(&index) {
            return Err(native_state_error("unsupported string scalar native"));
        }
        let packet = string_packet(args, vm, 8)?;
        let first = read_u32(&packet, 0)?;
        let second = read_u32(&packet, 4)?;
        if index <= 22 {
            let (left, right) = if index % 2 == 0 {
                // Both formatted operands have their own buffer. As in the
                // original C-string receiver, a formatted %c NUL ends them.
                let left = super::expand_kcs_format(vm, &vm.read_string(first)?, 0)?;
                let right = super::expand_kcs_format(vm, &vm.read_string(second)?, 0)?;
                (left, right)
            } else {
                // 19/21 pass arg1, arg0 to the comparison; 20/22 do not.
                let first = vm.read_string(first)?;
                let second = vm.read_string(second)?;
                (second, first)
            };
            let value = if index >= 21 {
                super::compare_kcs_strings_ascii_insensitive(trim_nul(&left), trim_nul(&right))
            } else {
                super::compare_kcs_strings(trim_nul(&left), trim_nul(&right))
            };
            return Ok(NativeReturn::Value(value as u32));
        }
        let mut offset = 0;
        let mut characters = 0;
        loop {
            let at = string_address(first, offset)?;
            if string_byte(vm, at)? == 0 || index == 24 && characters >= second {
                break;
            }
            if offset >= MAX_EXPANDED_BYTES {
                return Err(native_state_error("string native prefix exceeds storage limit"));
            }
            let width = string_char_width(vm, at)?;
            if index == 23 && offset + width >= second as usize {
                break;
            }
            offset += width;
            characters += 1;
        }
        vm.write(string_address(first, offset)?, &[0, 0])?;
        Ok(NativeReturn::Value(offset as u32))
    }

    /// 38 and 40 read fixed parameter structures even when their KCS call
    /// pops no bytes. 38's delimiter bytes live in its own 64-byte local area.
    pub(super) fn string_token_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let size = match index {
            38 => 16,
            40 => 4,
            _ => return Err(native_state_error("unsupported string token native")),
        };
        let packet = string_packet(args, vm, size)?;
        let target = read_u32(&packet, 0)?;
        if index == 40 {
            return Ok(NativeReturn::Value(u32::from(string_char_width(vm, target)? == 2)));
        }
        let source = read_u32(&packet, 4)?;
        let separators = read_u32(&packet, 8)?;
        let mut delimiters = Vec::with_capacity(64);
        let mut input = 0;
        loop {
            let byte = string_byte(vm, string_address(separators, input)?)?;
            if byte == 0 { break; }
            if delimiters.len() == 64 {
                return Err(native_state_error("native38 delimiters overwrite the original stack cookie"));
            }
            if byte == b'\\' && string_byte(vm, string_address(separators, input + 1)?)? == b't' {
                delimiters.push(b'\t');
                input += 2;
            } else {
                delimiters.push(byte);
                input += 1;
            }
        }
        let mut length = 0;
        loop {
            let byte = string_byte(vm, string_address(source, length)?)?;
            if byte == 0 { break; }
            // The wrapper reloads this field each iteration. Preserve a live
            // parameter slot if an overlapping destination reaches it.
            let capacity = if vm.native_arguments().is_some() {
                read_u32(vm.native_argument_storage(16)?, 12)?
            } else {
                read_u32(&packet, 12)?
            };
            if length as u32 >= capacity.wrapping_sub(2) || delimiters.contains(&byte) {
                break;
            }
            if length >= MAX_EXPANDED_BYTES {
                return Err(native_state_error("native38 copy exceeds storage limit"));
            }
            vm.write(string_address(target, length)?, &[byte])?;
            length += 1;
        }
        vm.write(string_address(target, length)?, &[0, 0])?;
        Ok(NativeReturn::Value(length as u32))
    }

    /// Ordinary25 and 46 use the same integer reader, with independent
    /// formatting/percentage wrappers. They do not use CRT whitespace rules.
    pub(super) fn string_number_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let size = match index {
            25 => 4,
            46 => 8,
            _ => return Err(native_state_error("unsupported string number native")),
        };
        let packet = string_packet(args, vm, size)?;
        let target = read_u32(&packet, 0)?;
        if index == 25 {
            let input = vm.read_string(target)?;
            let text = super::expand_kcs_format(vm, &input, 0)?;
            return Ok(NativeReturn::Value(string_integer_prefix(&text).unwrap_or(0)));
        }
        let base = read_u32(&packet, 4)?;
        // 592155 skips an unresolved pointer. A mapped, unterminated string
        // and a poisoned shared binding remain actual diagnostics.
        match vm.read(target, 0) {
            Ok(_) => {}
            Err(cs2_script::ScriptError::MemoryOutOfBounds { .. }) => {
                return Ok(NativeReturn::Continue);
            }
            Err(error) => return Err(error),
        }
        let source = vm.read_string(target)?;
        if source.last() == Some(&b'%') {
            if let Some(value) = string_integer_prefix(&source) {
                // Preserve the two f32 operations and signed conversion,
                // rather than an integer multiply/divide or a fused scale.
                let product = (value as i32 as f32) * (base as f32);
                let scaled = super::float_to_i32(product / 100.0);
                super::write_kcs_terminated(vm, target, scaled.to_string().as_bytes(), 1)?;
            }
        }
        Ok(NativeReturn::Continue)
    }

    /// Ordinary519 uses 43's parser with a separate output buffer, after
    /// clearing the VM destination. Its ordinal is read AFTER the split.
    pub(super) fn string_argument_native519(
        &mut self, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let packet = string_packet(args, vm, 8)?;
        let target = read_u32(&packet, 0)?;
        let source = read_u32(&packet, 4)?;
        let source_resolves = match vm.read(source, 0) {
            Ok(_) => true,
            Err(cs2_script::ScriptError::MemoryOutOfBounds { .. }) => false,
            Err(error) => return Err(error),
        };
        // 5a54b7 runs even when the source pointer is unresolved. An aliased
        // destination therefore changes the bytes which the parser observes.
        vm.write(target, &[0])?;
        let task = self.current_task_id;
        let origins = &mut self.data_services.cst_text_origins;
        origins.invalidate_returned(task, target);
        if !source_resolves {
            return Ok(NativeReturn::Value(0));
        }
        let codec = origins.sourced_vm_string(task, source, vm).map(|(_, codec)| codec);
        let mut words = Vec::new();
        split_parenthesized_table(&mut ParenthesizedTable {
            vm, source, separators: None,
            output: ParenthesizedOutput::Scratch(&mut words),
        })?;
        let ordinal = if vm.native_arguments().is_some() {
            read_u32(vm.native_argument_storage(12)?, 8)?
        } else {
            read_u32(args, 8)?
        };
        let mut selected = 0;
        let mut skipped = 0;
        while skipped < ordinal {
            let length = words[selected..].iter().position(|&byte| byte == 0)
                .ok_or_else(|| native_state_error("native519 table item is unterminated"))?;
            selected += length + 1;
            if words.get(selected).copied() == Some(0) { break; }
            if selected >= words.len() {
                return Err(native_state_error("native519 table lacks its final terminator"));
            }
            skipped += 1;
        }
        let length = words[selected..].iter().position(|&byte| byte == 0)
            .ok_or_else(|| native_state_error("native519 selected item is unterminated"))?;
        if length == 0 {
            // No second zero is written for an empty/missing item.
            return Ok(NativeReturn::Value(0));
        }
        let text = &words[selected..selected + length];
        for (offset, &byte) in text.iter().enumerate() {
            vm.write(string_address(target, offset)?, &[byte])?;
        }
        vm.write(string_address(target, length)?, &[0, 0])?;
        if let Some(codec) = codec && !text.is_ascii() {
            origins.record(task, target, text, codec);
        }
        Ok(NativeReturn::Value(1))
    }

    /// Ordinary598 extracts the name from a formatted comma/selector string.
    /// Its helper sees the first C string, not bytes after an embedded NUL.
    pub(super) fn string_resource_name_native598(
        &mut self, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let source = read_u32(&string_packet(args, vm, 4)?, 0)?;
        let formatted = super::expand_kcs_format(vm, &vm.read_string(source)?, 0)?;
        // 596d84 resolves the destination only after formatting. The format
        // wrapper always returns one; mapping/lock failures are not an empty
        // name and must remain errors.
        let packet = string_packet(args, vm, 8)?;
        let target = read_u32(&packet, 4)?;
        // Both fixed 598 helpers split only on ',' (2c00). Since ',' cannot
        // be a valid CP932 trail byte, the existing splitter also preserves
        // v401's byte-only result; no second resource-name parser is needed.
        let table = super::split_kcs_string_table(trim_nul(&formatted), b",");
        if table.len() > 256 {
            return Err(native_state_error("native598 comma-name table exceeds its 256-byte local storage"));
        }
        let mut parts = table.split(|&byte| byte == 0);
        let Some(name) = parts.next().filter(|part| !part.is_empty()) else {
            // 6effa0 clears its own local name, not the caller's VM output.
            return Ok(NativeReturn::Value(0));
        };
        if parts.take_while(|part| !part.is_empty()).count() > 64 {
            return Err(native_state_error("native598 selectors exceed its 64-DWORD local storage"));
        }
        // 596dc0 copies forward including a SINGLE terminator. Preserve
        // writes before any VM bounds/lock failure, and the following byte.
        for (offset, byte) in name.iter().copied().chain(std::iter::once(0)).enumerate() {
            vm.write(string_address(target, offset)?, &[byte])?;
        }
        Ok(NativeReturn::Value(1))
    }

    /// Ordinary41..45: raw byte tables and replacement, not KCS format
    /// expansion. The fixed Yukikoi and v401 wrappers have the same field order.
    pub(super) fn string_table_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let size = match index {
            41..=43 => 12,
            44 | 45 => 16,
            _ => return Err(native_state_error("unsupported string table native")),
        };
        let packet = string_packet(args, vm, size)?;
        let target = read_u32(&packet, 0)?;
        let source = read_u32(&packet, 4)?;
        match index {
            41 => {
                let separators = read_u32(&packet, 8)?;
                // 5ffb60 returns without writes when an initial VM address
                // resolves to NULL. A poisoned shared binding remains an error.
                for address in [target, source, separators] {
                    match vm.read(address, 0) {
                        Ok(_) => {}
                        Err(cs2_script::ScriptError::MemoryOutOfBounds { .. }) => {
                            return Ok(NativeReturn::Continue);
                        }
                        Err(error) => return Err(error),
                    }
                }
                // Only a registered 41 wrapper calling the recovered
                // byte-oriented helper opts out of CP932 pair handling.
                let pairs = self.data_services.ordinary_native_table.as_ref()
                    .is_none_or(|table| !table.string_split_uses_bytes());
                let mut input = 0;
                let mut output = 0;
                let mut separated = true;
                loop {
                    if input >= MAX_EXPANDED_BYTES || output >= MAX_EXPANDED_BYTES {
                        return Err(native_state_error("native41 exceeds storage limit"));
                    }
                    let at = string_address(source, input)?;
                    let byte = string_byte(vm, at)?;
                    if byte == 0 { break; }
                    if string_set_contains(vm, separators, byte)? {
                        if !separated {
                            vm.write(string_address(target, output)?, &[0])?;
                            output += 1;
                            separated = true;
                        }
                    } else {
                        if pairs && matches!(byte, 0x81..=0x9f | 0xe0..=0xef)
                            && matches!(string_byte(vm, string_address(at, 1)?)?,
                                0x40..=0x7e | 0x80..=0xfc)
                        {
                            vm.write(string_address(target, output)?, &[byte])?;
                            output += 1;
                            input += 1;
                        }
                        let byte = string_byte(vm, string_address(source, input)?)?;
                        vm.write(string_address(target, output)?, &[byte])?;
                        output += 1;
                        separated = false;
                    }
                    input += 1;
                }
                vm.write(string_address(target, output)?, &[0, 0])?;
                Ok(NativeReturn::Continue)
            }
            42 => {
                let ordinal = read_u32(&packet, 8)? as i32;
                let mut selected = source;
                for _ in 0..ordinal.max(0) {
                    let length = string_length(vm, selected)?;
                    if length == 0 { break; }
                    selected = string_address(selected, length + 1)?;
                }
                // 5925f0 copies even the empty/missing item; 592610 writes
                // WORD0; only then does 592618 inspect the live source byte.
                copy_string_forward(vm, target, selected)?;
                let length = string_length(vm, target)?;
                vm.write(string_address(target, length)?, &[0, 0])?;
                Ok(NativeReturn::Value(u32::from(string_byte(vm, selected)? != 0)))
            }
            43 => {
                let separators = read_u32(&packet, 8)?;
                for address in [target, source, separators] {
                    match vm.read(address, 0) {
                        Ok(_) => {}
                        Err(cs2_script::ScriptError::MemoryOutOfBounds { .. }) => {
                            return Ok(NativeReturn::Continue);
                        }
                        Err(error) => return Err(error),
                    }
                }
                split_parenthesized_live(vm, target, source, separators)?;
                Ok(NativeReturn::Continue)
            }
            44 => {
                let first = read_u32(&packet, 8)? as i32;
                let last = match read_u32(&packet, 12)? {
                    u32::MAX => i32::MAX,
                    value => value as i32,
                };
                // 592372 initializes only BYTE0, before consulting the table.
                // Empty ranges therefore preserve the following output byte.
                vm.write(target, &[0])?;
                let mut selected = source;
                let mut ordinal = 0;
                while ordinal < first {
                    let length = string_length(vm, selected)?;
                    if length == 0 { break; }
                    selected = string_address(selected, length + 1)?;
                    ordinal += 1;
                }
                while ordinal <= last {
                    let length = string_length(vm, selected)?;
                    if length == 0 { break; }
                    let output = string_length(vm, target)?;
                    if output.checked_add(length + 1).is_none_or(|size| size > MAX_EXPANDED_BYTES) {
                        return Err(native_state_error("native44 output exceeds storage limit"));
                    }
                    copy_string_words(vm, string_address(target, output)?, selected, length + 1)?;
                    let output = string_length(vm, target)?;
                    vm.write(string_address(target, output)?, b" \0")?;
                    // Original rescans the source after both output writes.
                    let length = string_length(vm, selected)?;
                    selected = string_address(selected, length + 1)?;
                    ordinal += 1;
                }
                Ok(NativeReturn::Continue)
            }
            45 => {
                let old = read_u32(&packet, 8)?;
                let replacement = read_u32(&packet, 12)?;
                let old_length = string_length(vm, old)?;
                let replacement_length = string_length(vm, replacement)?;
                let task = self.current_task_id;
                let origins = &mut self.data_services.cst_text_origins;
                // A low ASCII marker cannot be the trailing byte of a multibyte
                // character. Compare the result to check live buffer aliasing.
                let old_byte = string_byte(vm, old)?;
                let sourced = origins.sourced_vm_string(task, source, vm)
                    .filter(|_| old_length == 1 && old_byte < 0x40)
                    .and_then(|(input, codec)| {
                        let replacement = vm.read(replacement, replacement_length).ok()?;
                        replacement.is_ascii().then(|| (input, replacement, codec))
                    });
                origins.invalidate_returned(task, target);
                // 5ff603 clears output after reading both lengths, before the
                // first source search. This matters to real in-place buffers.
                vm.write(target, &[0])?;
                let mut consumed: usize = 0;
                let mut written: usize = 0;
                while let Some(offset) = find_live_string(vm, string_address(source, consumed)?, old)? {
                    if old_length == 0 {
                        return Err(native_state_error("native45 empty search would not advance"));
                    }
                    if offset.checked_add(replacement_length)
                        .and_then(|size| written.checked_add(size))
                        .is_none_or(|size| size > MAX_EXPANDED_BYTES)
                    {
                        return Err(native_state_error("native45 output exceeds storage limit"));
                    }
                    for at in 0..offset {
                        let byte = string_byte(vm, string_address(source, consumed + at)?)?;
                        vm.write(string_address(target, written)?, &[byte])?;
                        written += 1;
                    }
                    for at in 0..replacement_length {
                        let byte = string_byte(vm, string_address(replacement, at)?)?;
                        vm.write(string_address(target, written)?, &[byte])?;
                        written += 1;
                    }
                    consumed = consumed.checked_add(offset + old_length)
                        .filter(|&size| size <= MAX_EXPANDED_BYTES)
                        .ok_or_else(|| native_state_error("native45 source exceeds storage limit"))?;
                }
                // The original writes WORD0 before strcat's suffix copy. If
                // that suffix is nonempty only its final BYTE0 is copied; it
                // does not add a second zero at the new end of the string.
                vm.write(string_address(target, written)?, &[0, 0])?;
                let suffix = string_address(source, consumed)?;
                let length = string_length(vm, suffix)?;
                let output = string_length(vm, target)?;
                if output.checked_add(length + 1).is_none_or(|size| size > MAX_EXPANDED_BYTES) {
                    return Err(native_state_error("native45 output exceeds storage limit"));
                }
                copy_string_words(vm, string_address(target, output)?, suffix, length + 1)?;
                if let Some((input, replacement, codec)) = sourced
                    && let Ok(bytes) = vm.read_string(target)
                    && !bytes.is_ascii()
                    && input.iter().flat_map(|byte| {
                        if *byte == old_byte { replacement.as_slice() } else { std::slice::from_ref(byte) }
                    }).eq(&bytes)
                {
                    origins.record(task, target, &bytes, codec);
                }
                Ok(NativeReturn::Continue)
            }
            _ => unreachable!(),
        }
    }

    /// 0x59f7b0 resolves the SECOND argument as a VM address; 0x699e70
    /// looks up the first as an unsigned string id. Presence, including an
    /// empty value, returns one. Both paths terminate with a single zero.
    pub(super) fn read_shared_string_native277(
        &self, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let id = read_u32(args, 0)?;
        let target = read_u32(args, 4)?;
        let value = self.string_values.get(&id);
        let mut output = value.map_or_else(Vec::new, |value| trim_nul(value).to_vec());
        output.push(0);
        // Capture the complete source before a possibly overlapping VM write.
        // Bounds/lock failure remains an error and cannot become a missing id.
        vm.write(target, &output)?;
        Ok(NativeReturn::Value(u32::from(value.is_some())))
    }

    /// Native state 2 leaves the return slot intact. Copy input before writing
    /// so self expansion and a shared external VM binding use the same bytes.
    pub(super) fn expand_shared_string_native278(
        &mut self, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let target = read_u32(args, 0)?;
        let input = vm.read_string(target)?;
        let origins = &mut self.data_services.cst_text_origins;
        let mut codec = origins.returned(self.current_task_id, target, &input);
        let mut output = expand_shared_strings_observed(&input, self.string_values, |id, bytes| {
            if !bytes.is_ascii() && origins.shared(id, bytes) != codec {
                codec = None;
            }
        })?;
        let text_len = output.len();
        output.extend_from_slice(&[0, 0]);
        vm.write(target, &output)?;
        if let Some(codec) = codec {
            origins.record(self.current_task_id, target, &output[..text_len], codec);
        } else {
            origins.invalidate_returned(self.current_task_id, target);
        }
        Ok(NativeReturn::Continue)
    }

    /// Byte substring search at CP932 character boundaries. This native
    /// returns the haystack byte length for a miss, including an empty input.
    /// It neither formats the inputs nor returns a host pointer.
    pub(super) fn find_string_native(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let haystack = vm.read_string(read_u32(args, 0)?)?;
        let needle = vm.read_string(read_u32(args, 4)?)?;
        let mut cursor = 0;
        while cursor < haystack.len() {
            if haystack[cursor..].starts_with(&needle) {
                return Ok(NativeReturn::Value(cursor as u32));
            }
            cursor += cp932_char_width(&haystack[cursor..]);
        }
        Ok(NativeReturn::Value(haystack.len() as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeadlessPlayer;
    use cs2_runtime::native_table::OrdinaryNativeTable;
    use cs2_script::{Host, KcsProgram};
    use std::path::{Path, PathBuf};

    fn game(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games").join(path)
    }

    fn split_trail_byte(path: &str, expected: &[u8]) -> anyhow::Result<()> {
        let executable = game(path);
        if !executable.is_file() {
            return Ok(());
        }
        let table = OrdinaryNativeTable::from_pe(&std::fs::read(executable)?)
            .expect("ordinary registered table");
        let temp = crate::tests::TempDir::new();
        std::fs::write(temp.0.join("startup.xml"), concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><document><INT><int00>",
            "<name>image</name><int>image.int</int><ext>hg3</ext><dir>image</dir>",
            "</int00></INT></document>",
        ))?;
        let mut player = HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("profile"))?;
        player.with_host(|host| {
            host.data_services.ordinary_native_table = Some(table);
            let mut vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 64], 0)?)?;
            let target = vm.allocate_region(&[0; 8])?;
            let source = vm.allocate_region(&[0x82, 0xa0, 0])?;
            let separators = vm.allocate_region(&[0xa0, 0])?;
            let arguments = [target, source, separators].map(u32::to_le_bytes).concat();
            assert_eq!(host.call(41, &arguments, &mut vm)?, NativeReturn::Continue);
            assert_eq!(vm.read(target, expected.len())?, expected);
            Ok::<_, anyhow::Error>(())
        })?;
        Ok(())
    }

    #[test]
    fn registered_native41_preserves_the_original_splitter_byte_unit() -> anyhow::Result<()> {
        split_trail_byte("cs2_full_v401/system/cs2_open.exe", &[0x82, 0, 0, 0])?;
        split_trail_byte("Yukikoi Melt/YukikoiMelt.exe", &[0x82, 0xa0, 0, 0])?;
        Ok(())
    }
}
