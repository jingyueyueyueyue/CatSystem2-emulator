//! Read-only syntax inspection. Never invokes the interpreter/expression evaluator.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FesStaticCommand {
    pub offset: usize,
    pub parameters: Vec<Vec<u8>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FesStaticLookahead {
    pub commands: Vec<FesStaticCommand>,
    pub stopped: &'static str,
}
impl FesInterpreter {
    /// At most twenty statically literal statements after the real cursor.
    /// Unknown host verbs are barriers: they can write variables or invoke
    /// callbacks which change the next resource. No speculative VM is run.
    pub fn static_lookahead(&self, limit: usize) -> FesStaticLookahead {
        let mut scan = FesStaticLookahead { commands: Vec::new(), stopped: "block-end" };
        if self.initialization_pending() { scan.stopped = "initialization"; return scan; }
        let Some(block) = self.program.blocks.get(self.block) else { return scan; };
        for line in block.lines.iter().skip(self.pc) {
            if scan.commands.len() >= limit.min(20) { scan.stopped = "command-limit"; break; }
            let Ok(bytes) = expand_fes_macros(&line.bytes, &self.program.macros) else {
                scan.stopped = "macro"; break;
            };
            // Recognize syntax without evaluating even a constant condition.
            if !matches!(split_fes_condition(&bytes), Ok(None)) {
                scan.stopped = "control-flow"; break;
            }
            let Ok(parameters) = fes_parameters(&bytes) else { scan.stopped = "syntax"; break; };
            if parameters.is_empty() { continue; }
            if parameters.iter().any(|word| word.iter().any(|byte|
                matches!(byte, b'$' | b'%' | b'@' | b'(' | b')' | b'=' | b'?' | b'\\')))
            { scan.stopped = "dynamic-parameter"; break; }
            let wait = parameters[0] == b"wait" && parameters.len() == 2
                && parameters[1].iter().all(u8::is_ascii_digit);
            let load = parameters.len() >= 3 && parameters[1].eq_ignore_ascii_case(b"load")
                && parameters.iter().skip(3).all(|word| !word.is_empty()
                    && word.iter().all(|byte| byte.is_ascii_digit() || *byte == b'-'));
            if !wait && !load { scan.stopped = "unknown-or-callback"; break; }
            scan.commands.push(FesStaticCommand { offset: line.offset, parameters });
        }
        scan
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn interpreter(lines: &str) -> FesInterpreter {
        FesInterpreter::new(FesProgram::from_text(format!("#START\n{lines}").as_bytes()).unwrap()).unwrap()
    }
    #[test]
    fn static_lookahead_limits_twenty_effective_commands_without_mutating() {
        let source = (0..24).map(|i| format!("pic load image{i}\n")).collect::<String>();
        let vm = interpreter(&source);
        let before = vm.position();
        let scan = vm.static_lookahead(20);
        assert_eq!(scan.commands.len(), 20);
        assert_eq!(scan.commands[19].parameters[2], b"image19");
        assert_eq!(before, vm.position());
        assert!(vm.pending().is_none());
    }
    #[test]
    fn static_lookahead_stops_before_control_writes_random_and_dynamic_parameters() {
        for barrier in ["if (1) pic load conditional", "call OTHER", "next OTHER", "$0=rand(99)", "pic load $str0", "pic load photo $0", "snd play", "unknown effect"] {
            let vm = interpreter(&format!("pic load first\n{barrier}\npic load unreachable\n"));
            let scan = vm.static_lookahead(20);
            assert_eq!(scan.commands.len(), 1, "barrier: {barrier}");
            assert_eq!(vm.local_values()[0], 0);
            assert_eq!(vm.call_depth(), 0);
        }
    }
    #[test]
    fn static_lookahead_starts_at_actual_cursor_and_uses_fes_parameter_lexer() {
        let mut vm = interpreter("pic load old\nwait 10\npic load \"folder/my image\"\n");
        assert!(vm.enter_statement(b"START", 2));
        let scan = vm.static_lookahead(20);
        assert_eq!(scan.commands.len(), 2);
        assert_eq!(scan.commands[1].parameters[2], b"folder/my image");
    }
}
