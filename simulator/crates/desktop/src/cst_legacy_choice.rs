use cs2_runtime::{ResourceIndex, ResourceLocation};
use cs2_script::{CstDocument, CstRecord, KcsProgram, Operand};

use super::resource_preload::SourcedResource;

const COMMAND: u8 = 0x30;
const LEGACY_SELECT: &[u8] = b"select";
const FSELECT: &[u8] = b"fselect";

pub(super) fn restore_legacy_choices(
    resources: &ResourceIndex,
    resource: &SourcedResource,
    document: &mut CstDocument,
    program: Option<&KcsProgram>,
) -> Vec<usize> {
    let ResourceLocation::Archive { path, .. } = &resource.source.location else { return Vec::new(); };
    let Some(relative) = path.strip_prefix(resources.root()).ok()
        .and_then(|path| path.to_str()) else { return Vec::new(); };
    let name = relative.to_ascii_lowercase();
    if !name.strip_prefix("update").and_then(|name| name.strip_suffix(".int"))
        .is_some_and(|number| number.len() == 2 && number.bytes().all(|byte| byte.is_ascii_digit()))
        || !document.records.iter().any(|row| is_command(row, FSELECT))
        || !program.is_some_and(uses_legacy_select)
    {
        return Vec::new();
    }
    let Some(base) = resources.archive_candidates(&resource.source.name).find(|candidate| {
        matches!(&candidate.location, ResourceLocation::Archive { path, .. }
            if path.strip_prefix(resources.root()).is_ok_and(|path|
                path.as_os_str().eq_ignore_ascii_case("scene.int")))
    }) else { return Vec::new(); };
    let Some(original) = resources.read_candidate_bounded(base, 64 * 1024 * 1024).ok().flatten()
        .and_then(|bytes| CstDocument::parse(&bytes).ok()) else { return Vec::new(); };
    restore_matching_choice_groups(document, &original)
}

fn uses_legacy_select(program: &KcsProgram) -> bool {
    // The old sscript command map is initialized with PUSH map, key and
    // dispatch value, followed by native 517 (insert into integer map).
    // Reject programs with an initialized fselect command key anywhere.
    if program.data.windows(b"fselect\0".len()).any(|key| key == b"fselect\0") {
        return false;
    }
    program.instructions.windows(4).any(|registration| {
        let [map, key, value, insert] = registration else { return false; };
        if [map.opcode, key.opcode, value.opcode, insert.opcode] != [0x25, 0x25, 0x25, 0x2b]
            || !matches!(map.operands.as_slice(), [operand] if operand.flags.bits() == 0x13)
            || value.operands != [Operand::immediate(27)]
            || insert.operands != [Operand::immediate(517), Operand::immediate(12)]
        {
            return false;
        }
        let [key] = key.operands.as_slice() else { return false; };
        key.flags.bits() == 3 && program.data.get(key.payload as usize..)
            .is_some_and(|bytes| bytes.starts_with(b"select\0"))
    })
}

fn is_command(record: &CstRecord, command: &[u8]) -> bool {
    record.kind == COMMAND && record.text.trim_ascii() == command
}

fn restore_matching_choice_groups(document: &mut CstDocument, original: &CstDocument) -> Vec<usize> {
    if document.inputs.len() != original.inputs.len() {
        return Vec::new();
    }
    let mut restored = Vec::new();
    for (input, range) in document.inputs.iter().enumerate() {
        let Some([marker, choices @ ..]) = document.records.get_mut(range.clone()) else { continue; };
        let Some([base_marker, base_choices @ ..]) = original.input_records(input) else { continue; };
        if is_command(marker, FSELECT) && is_command(base_marker, LEGACY_SELECT)
            && !choices.is_empty() && choices.len() == base_choices.len()
            && choices.iter().zip(base_choices).all(|(choice, base)| {
                choice_target(choice).is_some_and(|target| choice_target(base) == Some(target))
            })
        {
            marker.text = LEGACY_SELECT.to_vec();
            restored.push(input);
        }
    }
    restored
}

fn choice_target(record: &CstRecord) -> Option<(&[u8], &[u8])> {
    if record.kind != COMMAND { return None; }
    let mut fields = record.text.split(|byte| byte.is_ascii_whitespace()).filter(|part| !part.is_empty());
    let id = fields.next()?;
    let target = fields.next()?;
    if id.iter().all(u8::is_ascii_digit) && target.iter().all(|byte|
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) && fields.next().is_some()
    {
        Some((id, target))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_choice_requires_registered_command_and_no_fselect_key() {
        let mut code = Vec::new();
        for (opcode, operands) in [
            (0x25, vec![Operand::new(0x13, 0)]),
            (0x25, vec![Operand::immediate(0)]),
            (0x25, vec![Operand::immediate(27)]),
            (0x2b, vec![Operand::immediate(517), Operand::immediate(12)]),
        ] {
            code.push(opcode);
            for operand in operands {
                code.push(operand.flags.bits());
                code.extend_from_slice(&operand.payload.to_le_bytes());
            }
        }
        let mut program = KcsProgram::from_code(&code, b"select\0".to_vec(), 1024).unwrap();
        assert!(uses_legacy_select(&program));
        program.data.extend_from_slice(b"fselect\0");
        assert!(!uses_legacy_select(&program));
        program.data.truncate(b"select\0".len());
        program.instructions.pop();
        assert!(!uses_legacy_select(&program));
    }

    #[test]
    fn legacy_choice_restores_only_a_bare_marker_with_matching_original_routes() {
        let make_document = |rows: &[&[u8]], start| CstDocument {
            records: rows.iter().map(|bytes| CstRecord { kind: COMMAND, text: bytes.to_vec() }).collect(),
            inputs: vec![start..rows.len()],
        };
        let original = make_document(&[b"select", b"1 route original", b"2 route other"], 0);
        let localized = make_document(&[b"other", b"fselect", b"1 route translated", b"2 route localized"], 1);
        let mut restored = localized.clone();
        assert_eq!(restore_matching_choice_groups(&mut restored, &original), [0]);
        assert_eq!(restored.records[1].text, b"select");
        assert_eq!(restored.records[2..], localized.records[2..]);

        for (record, replacement) in [(1, b"fselect menu 0".as_slice()), (2, b"1 different changed"),
            (3, b"1 route duplicate")]
        {
            let mut invalid = localized.clone();
            invalid.records[record].text = replacement.to_vec();
            let expected = invalid.clone();
            assert!(restore_matching_choice_groups(&mut invalid, &original).is_empty());
            assert_eq!(invalid, expected);
        }
    }
}
