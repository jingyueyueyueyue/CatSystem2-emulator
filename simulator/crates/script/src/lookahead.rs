//! Conservative inspection of literal argument pushes preceding one native call.
use super::*;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticNativeCall { pub index: u32, pub arguments: Vec<u8> }
impl Vm {
    /// Inspect one native boundary; every call is a barrier because its result
    /// or callbacks can affect subsequent argument construction.
    pub fn static_native_lookahead(&self) -> Option<StaticNativeCall> {
        let program = self.program.as_ref()?;
        let start = program.instructions.binary_search_by_key(&self.instruction_pointer, |i| i.offset).ok()?;
        let available = self.sp.checked_sub(self.frame_top)?;
        if available > 4096 { return None; }
        let mut arguments = self.memory.get(self.frame_top..self.sp)?.to_vec();
        for instruction in program.instructions.iter().skip(start).take(128) {
            let immediate = |operand: &Operand| (operand.flags.bits() == 3).then_some(operand.payload);
            match instruction.opcode {
                0x00 => {},
                0x2e => {
                    if arguments.len() >= 4096 { return None; }
                    arguments.extend_from_slice(&immediate(instruction.operands.first()?)?.to_le_bytes());
                }
                NATIVE_CALL_OPCODE => {
                    let index = immediate(instruction.operands.first()?)?;
                    let bytes = immediate(instruction.operands.get(1)?)? as usize;
                    let start = arguments.len().checked_sub(bytes)?;
                    return Some(StaticNativeCall { index, arguments: arguments[start..].to_vec() });
                }
                _ => return None,
            }
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn push(code: &mut Vec<u8>, value: u32) {
        code.extend_from_slice(&[0x2e, 3]); code.extend_from_slice(&value.to_le_bytes());
    }
    fn call(code: &mut Vec<u8>, index: u32, bytes: u32) {
        code.extend_from_slice(&[0x2b, 3]); code.extend_from_slice(&index.to_le_bytes());
        code.push(3); code.extend_from_slice(&bytes.to_le_bytes());
    }
    #[test]
    fn static_native_lookahead_reads_literal_native_arguments_without_running() {
        let mut code = Vec::new();
        push(&mut code, 12); push(&mut code, 34); call(&mut code, 435, 8);
        let vm = Vm::from_program(KcsProgram::from_code(&code, vec![0;64], 512).unwrap()).unwrap();
        let before = vm.instruction_pointer;
        assert_eq!(vm.static_native_lookahead(), Some(StaticNativeCall { index: 435,
            arguments: [12u32.to_le_bytes(),34u32.to_le_bytes()].concat() }));
        assert_eq!(before, vm.instruction_pointer);
        assert!(vm.stack.is_empty());
    }
    #[test]
    fn static_native_lookahead_stops_at_write_branch_and_nonliteral_push() {
        for barrier in [vec![0x3b], vec![0x2e,0x43,0,0,0,0], vec![0x29,3,0,0,0,0]] {
            let mut code=barrier;
            call(&mut code,435,0);
            let vm=Vm::from_program(KcsProgram::from_code(&code,vec![0;64],512).unwrap()).unwrap();
            assert!(vm.static_native_lookahead().is_none());
        }
    }
}
