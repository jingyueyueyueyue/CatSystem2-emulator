//! Copy policy for ordinary native 35 and self-directed 72/73.
use cs2_script::{Result, ScriptError, Vm};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NativeMemoryCopyMode {
    /// Documented baseline policy for a system context with MMX enabled.
    #[default]
    Mmx,
    Dword,
    Crt,
}

pub fn native_copy_memory(
    vm: &mut Vm,
    destination: u32,
    source: u32,
    length: usize,
    mode: NativeMemoryCopyMode,
) -> Result<()> {
    if length > 64 * 1024 * 1024 {
        return Err(ScriptError::InvalidStructuredText {
            offset: 0,
            message: "ordinary memory copy exceeds 64 MiB",
        });
    }
    if length == 0 {
        return Ok(());
    }
    let block = match mode {
        NativeMemoryCopyMode::Mmx if length % 8 == 0 => 8,
        NativeMemoryCopyMode::Mmx | NativeMemoryCopyMode::Dword if length % 4 == 0 => 4,
        _ => {
            let bytes = vm.read(source, length)?;
            return vm.write(destination, &bytes);
        }
    };
    for offset in (0..length).step_by(block) {
        let source = offset_address(source, offset)?;
        let destination = offset_address(destination, offset)?;
        // Snapshot one block, not the whole source. Forward overlap may
        // change bytes subsequently read by the next block.
        let bytes = vm.read(source, block)?;
        vm.write(destination, &bytes)?;
    }
    Ok(())
}

/// Distinct VMs can still alias the same external allocation. Preserve block
/// feedback for that case instead of accidentally making a whole-copy snapshot.
pub fn native_copy_between_vms(
    destination_vm: &mut Vm,
    destination: u32,
    source_vm: &Vm,
    source: u32,
    length: usize,
    mode: NativeMemoryCopyMode,
) -> Result<()> {
    if length > 64 * 1024 * 1024 {
        return Err(ScriptError::InvalidStructuredText {
            offset: 0,
            message: "ordinary cross-VM copy exceeds 64 MiB",
        });
    }
    if length == 0 {
        return Ok(());
    }
    let shared = destination & source & 0x8000_0000 != 0
        && destination_vm
            .external_binding()
            .same_storage(&source_vm.external_binding());
    let block = match mode {
        NativeMemoryCopyMode::Mmx if shared && length % 8 == 0 => 8,
        NativeMemoryCopyMode::Mmx | NativeMemoryCopyMode::Dword if shared && length % 4 == 0 => 4,
        _ => {
            let bytes = source_vm.read(source, length)?;
            return destination_vm.write(destination, &bytes);
        }
    };
    for offset in (0..length).step_by(block) {
        let bytes = source_vm.read(offset_address(source, offset)?, block)?;
        destination_vm.write(offset_address(destination, offset)?, &bytes)?;
    }
    Ok(())
}

fn offset_address(address: u32, offset: usize) -> Result<u32> {
    let offset = u32::try_from(offset).map_err(|_| ScriptError::MemoryOutOfBounds {
        address,
        length: offset,
    })?;
    address
        .checked_add(offset)
        .filter(|next| next >> 31 == address >> 31)
        .ok_or(ScriptError::MemoryOutOfBounds {
            address,
            length: offset as usize,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_script::KcsProgram;

    #[test]
    fn forward_overlap_exposes_block_policy_in_both_vm_storage_regions() {
        for base in [0, 0x8000_0000] {
            for mode in [
                NativeMemoryCopyMode::Mmx,
                NativeMemoryCopyMode::Dword,
                NativeMemoryCopyMode::Crt,
            ] {
                let mut vm =
                    Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 32], 16).unwrap())
                        .unwrap();
                vm.bind_external(vec![0; 32]).unwrap();
                let source: Vec<u8> = (0..24).collect();
                vm.write(base, &source).unwrap();
                native_copy_memory(&mut vm, base + 4, base, 16, mode).unwrap();
                let result = vm.read(base + 4, 16).unwrap();
                match mode {
                    NativeMemoryCopyMode::Mmx => {
                        assert_eq!(result, [0, 1, 2, 3, 4, 5, 6, 7, 4, 5, 6, 7, 12, 13, 14, 15])
                    }
                    NativeMemoryCopyMode::Dword => {
                        assert_eq!(result, [0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3])
                    }
                    NativeMemoryCopyMode::Crt => assert_eq!(result, source[..16]),
                }
            }
        }
    }

    #[test]
    fn cross_vm_copy_uses_the_identity_of_shared_external_storage() {
        let mut source =
            Vm::from_program(KcsProgram::from_code(&[0x3b], Vec::new(), 16).unwrap()).unwrap();
        source.bind_external((0u8..24).collect()).unwrap();
        let mut destination =
            Vm::from_program(KcsProgram::from_code(&[0x3b], Vec::new(), 16).unwrap()).unwrap();
        destination
            .bind_shared_external(source.external_binding())
            .unwrap();
        native_copy_between_vms(
            &mut destination,
            0x8000_0004,
            &source,
            0x8000_0000,
            16,
            NativeMemoryCopyMode::Dword,
        )
        .unwrap();
        assert_eq!(
            source.read(0x8000_0004, 16).unwrap(),
            [0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3]
        );
    }
}
