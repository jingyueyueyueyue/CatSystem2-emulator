//! Recognize the ordinary wrapper and the task update it controls.

const STEP_HELPER: &[u8] = b"\xc7\x41\x64\x01\x00\x00\x00\xc3";
// Consume the flag in both pause states. Outside pause, adjust the origin by
// now - last - 1 so the next task update advances exactly one frame.
const STEP_UPDATE: &[u8] = b"\x83\x7b\x5c\x00\x74\x11\x8b\xc5\xc7\x43\x64\x00\x00\x00\x00\x2b\x43\x60\x01\x43\x58\xeb\x19\
\x83\x7b\x64\x00\x74\x10\x8b\xc5\xc7\x43\x64\x00\x00\x00\x00\x2b\x43\x60\x48\x01\x43\x58\x89\x6b\x68";
const BASE_TASK_CODE_WINDOW: usize = 0x1000;

pub(crate) fn supports_single_step<'a>(
    (rva, wrapper): (u32, &[u8]),
    code_at: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    // Load the current task, call its setter and return native status 2.
    let [0x8b, 0x4c, 0x24, 0x08, 0xe8, a, b, c, d, 0xb8, 2, 0, 0, 0, 0xc2, 0x10, 0] =
        wrapper.get(..17)?
    else {
        return None;
    };
    let helper = rva.checked_add(9)?.checked_add_signed(i32::from_le_bytes([*a, *b, *c, *d]))?;
    if !code_at(helper)?.starts_with(STEP_HELPER) {
        return None;
    }
    // The verified base-task implementations place the consumer in the
    // executable bytes immediately preceding the setter.
    let nearby = code_at(helper.checked_sub(BASE_TASK_CODE_WINDOW as u32)?)?
        .get(..BASE_TASK_CODE_WINDOW)?;
    let mut updates = nearby.windows(STEP_UPDATE.len()).filter(|&code| code == STEP_UPDATE);
    updates.next()?;
    updates.next().is_none().then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_step_requires_a_matching_wrapper_setter_and_unique_consumer() {
        let code_rva = 0x4000;
        let helper = BASE_TASK_CODE_WINDOW;
        let update = helper - 0x675;
        let wrapper = helper + 0x80;
        let mut code = vec![0; wrapper + 17];
        code[wrapper..].copy_from_slice(b"\x8b\x4c\x24\x08\xe8\0\0\0\0\xb8\x02\0\0\0\xc2\x10\0");
        let displacement = helper as i32 - (wrapper + 9) as i32;
        code[wrapper + 5..wrapper + 9].copy_from_slice(&displacement.to_le_bytes());
        code[helper..helper + STEP_HELPER.len()].copy_from_slice(STEP_HELPER);
        code[update..update + STEP_UPDATE.len()].copy_from_slice(STEP_UPDATE);
        let recognize = |code: &[u8]| {
            let code_at = |rva: u32| code.get(rva.checked_sub(code_rva)? as usize..);
            supports_single_step((code_rva + wrapper as u32, &code[wrapper..]), code_at).is_some()
        };
        assert!(recognize(&code));

        for offset in [wrapper + 5, wrapper + 15, helper + 3, update + 23] {
            code[offset] ^= 1;
            assert!(!recognize(&code), "changed byte at {offset:#x}");
            code[offset] ^= 1;
        }
        code[..STEP_UPDATE.len()].copy_from_slice(STEP_UPDATE);
        assert!(!recognize(&code));
    }
}
