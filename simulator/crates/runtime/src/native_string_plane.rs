use crate::pe_resources::Section;

const POSITION_WRAPPER: &[u8] = b"\x8b\x44\x24\x0c\x8b\x10\x83\xec\x0c\x85\xd2\x74\x3b\xd9\x40\x04\x8b\x4c\x24\x14\x56\xd9\x5c\x24\x04\xd9\x40\x08\x8b\x31\x57\xd9\x5c\x24\x0c\xd9\x40\x0c\x8b\x40\x10\x8d\x7c\x24\x20\
    \xd9\x5c\x24\x10\x57\x6a\x01\x8d\x7c\x24\x10\x57\x50\x8b\x46\x0c\x6a\x2d\x52\x8b\x51\x04\x52\xff\xd0\x5f\x5e\xb8\x02\x00\x00\x00\x83\xc4\x0c\xc2\x10\x00";
const BASE_XY_WRAPPER: &[u8] = b"\x83\xec\x0c\x57\x8b\x7c\x24\x1c\x8b\x07\x85\xc0\x74\x56\x56\x8b\x74\x24\x1c\x8b\x16\x8b\x52\x0c\x8d\x4c\x24\x20\x51\x6a\x01\x8d\x4c\x24\x10\x51\x8b\x4f\x0c\x51\x6a\x37\x50\x8b\x46\x04\x50\x8b\xce\xff\xd2\
    \xd9\x47\x04\x8b\x06\xd9\x5c\x24\x08\xd9\x47\x08\x8d\x4c\x24\x20\x51\xd9\x5c\x24\x10\x8b\x4f\x0c\x6a\x01\x8d\x54\x24\x10\x52\x8b\x17\x51\x8b\x4e\x04\x6a\x2b\x52\x8b\x50\x0c\x51\x8b\xce\xff\xd2\x5e\xb8\x02\x00\x00\x00\x5f\x83\xc4\x0c\xc2\x10\x00";
const BASE_Z_WRAPPER: &[u8] = b"\x83\xec\x0c\x57\x8b\x7c\x24\x1c\x8b\x07\x85\xc0\x74\x4b\x56\x8b\x74\x24\x1c\x8b\x16\x8b\x52\x0c\x8d\x4c\x24\x20\x51\x6a\x01\x8d\x4c\x24\x10\x51\x6a\x00\x6a\x37\x50\x8b\x46\x04\x50\x8b\xce\xff\xd2\
    \xd9\x47\x04\x8b\x06\xd9\x5c\x24\x10\x8b\x40\x0c\x8d\x4c\x24\x20\x51\x8b\x0f\x6a\x01\x8d\x54\x24\x10\x52\x8b\x56\x04\x6a\x00\x6a\x2b\x51\x52\x8b\xce\xff\xd0\x5e\xb8\x02\x00\x00\x00\x5f\x83\xc4\x0c\xc2\x10\x00";
const ALPHA_WRAPPER: &[u8] = b"\x8b\x44\x24\x0c\x8b\x10\x85\xd2\x74\x3f\x8b\x40\x04\x89\x44\x24\x0c\x85\xc0\x7d\x04\x33\xc0\xeb\x0c\x3d\xff\x00\x00\x00\x7e\x09\xb8\xff\x00\x00\x00\x89\x44\x24\x0c\
    \x8b\x4c\x24\x08\x56\x8b\x31\x57\x8d\x7c\x24\x14\x57\x6a\x01\x50\x8b\x41\x04\x6a\x00\x6a\x39\x52\x8b\x56\x0c\x50\xff\xd2\x5f\x5e\xb8\x02\x00\x00\x00\xc2\x10\x00";
const STRING_PLANE_NAME: &[u8] = b".?AVkcTaskStringPlane2@@\0";
const SET: u32 = 43;
const POSITION: u32 = 45;
const QUERY: u32 = 55;
const ALPHA: u32 = 57;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct StringPlaneCommands {
    pub base: bool,
    pub position: bool,
    pub alpha: bool,
}

pub(super) fn commands(
    entries: &[(u32, &[u8])], data: &[u8], sections: &[Section], image_base: u32,
) -> StringPlaneCommands {
    let wrapper_matches = |index: usize, expected: &[u8]| {
        entries.get(index).is_some_and(|&(_, body)| body.starts_with(expected))
    };
    let base = wrapper_matches(85, BASE_XY_WRAPPER) && wrapper_matches(88, BASE_Z_WRAPPER);
    let position = wrapper_matches(84, POSITION_WRAPPER);
    let alpha = wrapper_matches(94, ALPHA_WRAPPER);
    if !base && !position && !alpha {
        return StringPlaneCommands::default();
    }
    let receivers = string_plane_receivers(data, sections, image_base).unwrap_or_default();
    StringPlaneCommands {
        base: base && receivers.base,
        position: position && receivers.position,
        alpha: alpha && receivers.alpha,
    }
}

fn string_plane_receivers(data: &[u8], sections: &[Section], image_base: u32) -> Option<StringPlaneCommands> {
    let data_sections: Vec<_> = sections.iter().copied().filter(|section| section.is_readable_data())
        .filter_map(|section| Some((section.virtual_address.checked_add(image_base)?,
            section.file_bytes_from(data, section.virtual_address)?))).collect();
    let descriptor = data_sections.iter().find_map(|(address, bytes)| {
        let offset = bytes.windows(STRING_PLANE_NAME.len()).position(|word| word == STRING_PLANE_NAME)?;
        address.checked_add(u32::try_from(offset.checked_sub(8)?).ok()?)
    })?;
    let locator = data_sections.iter().find_map(|(address, bytes)| {
        bytes.chunks_exact(4).enumerate().find_map(|(index, word)| {
            let offset = (index * 4).checked_sub(12)?;
            if word != descriptor.to_le_bytes() || bytes.get(offset..offset + 12)? != [0; 12] {
                return None;
            }
            address.checked_add(u32::try_from(offset).ok()?)
        })
    })?;
    // The MSVC RTTI locator precedes the vtable. Slot 6 receives object commands.
    let receiver = data_sections.iter().find_map(|(_, bytes)| {
        bytes.chunks_exact(4).position(|word| word == locator.to_le_bytes())
            .and_then(|word| read_word(bytes, (word + 7) * 4))
    })?;
    let code = |va: u32| {
        let rva = va.checked_sub(image_base)?;
        sections.iter().filter(|section| section.executable())
            .find_map(|section| section.file_bytes_from(data, rva))
    };
    let handling = code(receiver)?.get(0x32..)?;
    if handling.get(..10)? != b"\x8b\x45\x0c\x3d\x80\x01\x00\x00\x0f\x8f"
        || handling.get(14..16)? != b"\x0f\x84"
        || handling.get(20..28)? != b"\x8d\x48\xfd\x83\xf9\x39\x0f\x87"
        || handling.get(32..35)? != b"\x0f\xb6\x89" || handling.get(39..42)? != b"\xff\x24\x8d"
    {
        return None;
    }
    let case_target = |dispatch: &[u8], map: usize, table: usize, command: u32| {
        let selector = *code(read_word(dispatch, map)?)?.get((command - 3) as usize)?;
        read_word(code(read_word(dispatch, table)?)?, usize::from(selector) * 4)
    };
    let setter = case_target(handling, 35, 42, SET)?;
    let query = case_target(handling, 35, 42, QUERY)?;
    let delegated = |case: u32, expected: &[u8]| {
        let body = code(case)?;
        let displacement = read_word(body.strip_prefix(expected)?, 0)? as i32;
        case.checked_add(expected.len() as u32 + 4)?.checked_add_signed(displacement)
    };
    let base = delegated(setter, b"\x8b\x4d\x1c\x8b\x55\x18\x51\x8b\x4d\x14\x52\x8b\x55\x10\x51\x52\x50\x8b\x45\x08\x50\x8b\xce\xe8")?;
    if delegated(query, b"\x8b\x55\x1c\x8b\x4d\x18\x52\x8b\x55\x14\x51\x8b\x4d\x10\x52\x8b\x55\x08\x51\x50\x52\x8b\xce\xe8")? != base {
        return None;
    }
    let dispatch = code(base)?.get(0xf..)?;
    if dispatch.get(..17)? != b"\x8b\x7d\x0c\x8d\x47\xfd\x8b\xd9\x89\x5c\x24\x10\x83\xf8\x4e\x0f\x87"
        || dispatch.get(21..24)? != b"\x0f\xb6\x80" || dispatch.get(28..31)? != b"\xff\x24\x85"
    {
        return None;
    }
    let set_body = code(case_target(dispatch, 24, 31, SET)?)?;
    let query_body = code(case_target(dispatch, 24, 31, QUERY)?)?;
    let base = set_body.starts_with(b"\x83\xbb\x84\x00\x00\x00\x00\x8b\x75\x14\x74\x1f\x83\x7d\x10\x00")
        && query_body.starts_with(b"\x8b\x45\x10\x8b\x5b\x04\x50\x8b\x45\x14\x8d\x48\x08\x51\x8d\x50\x04\x52\x50\x8b\x44\x24\x20\x8b\x70\x5c\xe8");
    // Position and alpha share the base setter's forwarding entry.
    let position = case_target(handling, 35, 42, POSITION) == Some(setter)
        && case_target(dispatch, 24, 31, POSITION).and_then(code).is_some_and(|body| {
            body.starts_with(b"\x83\xbb\x84\x00\x00\x00\x00\x8b\x7d\x14\x74\x1f\x83\x7d\x10\x00")
                && body.get(43..).is_some_and(|update| update.starts_with(
                    b"\x8b\x4d\x10\xd9\x47\x08\x8b\x54\x24\x10\x8b\x5b\x04\x8b\x72\x5c\x51\x83\xec\x0c\xd9\x5c\x24\x08\xd9\x47\x04",
                ))
        });
    let alpha = case_target(handling, 35, 42, ALPHA) == Some(setter)
        && case_target(dispatch, 24, 31, ALPHA).and_then(code).is_some_and(|body| {
            body.starts_with(b"\x8b\x4b\x04\x8b\x5b\x5c\x8d\x44\x24\x30\x50\x8d\x44\x24\x10\xe8")
                && body.get(20..24) == Some(b"\x85\xc0\x0f\x84")
                && body.get(28..).is_some_and(|update| update.starts_with(
                    b"\x8b\x44\x24\x0c\x8b\x4d\x14\x25\xff\xff\xff\x00\xc1\xe1\x18\x0b\xc1\x6a\x01\x50",
                ))
        });
    Some(StringPlaneCommands { base, position, alpha })
}

fn read_word(bytes: &[u8], start: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(start..start.checked_add(4)?)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use crate::native_table::OrdinaryNativeTable;

    #[test]
    fn string_plane_base_wrappers_match_the_installed_pe() -> std::io::Result<()> {
        let Some(path) = std::env::var_os("CS2_CAMPANELLA_PE") else { return Ok(()); };
        let pe = std::fs::read(path)?;
        let has_base_wrappers = |data: &[u8]| OrdinaryNativeTable::from_pe(data)
            .is_some_and(|table| table.string_plane_has_base_wrappers());
        assert!(has_base_wrappers(&pe));
        for (pattern, offset) in [(super::BASE_XY_WRAPPER, 35), (super::BASE_Z_WRAPPER, 34),
            (super::STRING_PLANE_NAME, 5)]
        {
            let start = pe.windows(pattern.len()).position(|window| window == pattern).unwrap();
            let mut changed = pe.clone();
            changed[start + offset] ^= 1;
            assert!(!has_base_wrappers(&changed));
        }
        Ok(())
    }

    #[test]
    fn string_plane_alpha_wrapper_matches_the_installed_pe() -> std::io::Result<()> {
        let Some(path) = std::env::var_os("CS2_CAMPANELLA_PE") else { return Ok(()); };
        let pe = std::fs::read(path)?;
        let has_alpha_wrapper = |data: &[u8]| OrdinaryNativeTable::from_pe(data)
            .is_some_and(|table| table.string_plane_has_alpha_wrapper());
        assert!(has_alpha_wrapper(&pe));
        let start = pe.windows(super::ALPHA_WRAPPER.len())
            .position(|window| window == super::ALPHA_WRAPPER).unwrap();
        let mut changed = pe;
        changed[start + 62] ^= 1;
        assert!(!has_alpha_wrapper(&changed));
        assert!(OrdinaryNativeTable::from_pe(&changed).unwrap().string_plane_has_base_wrappers());
        Ok(())
    }

    #[test]
    fn string_plane_position_wrapper_matches_the_installed_pe() -> std::io::Result<()> {
        let Some(path) = std::env::var_os("CS2_CAMPANELLA_PE") else { return Ok(()); };
        let pe = std::fs::read(path)?;
        let has_position_wrapper = |data: &[u8]| OrdinaryNativeTable::from_pe(data)
            .is_some_and(|table| table.string_plane_has_position_wrapper());
        assert!(has_position_wrapper(&pe));
        let start = pe.windows(super::POSITION_WRAPPER.len())
            .position(|window| window == super::POSITION_WRAPPER).unwrap();
        let mut changed = pe;
        changed[start + 62] ^= 1;
        assert!(!has_position_wrapper(&changed));
        assert!(OrdinaryNativeTable::from_pe(&changed).unwrap().string_plane_has_base_wrappers());
        Ok(())
    }
}
