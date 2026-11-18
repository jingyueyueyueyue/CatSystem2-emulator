# cs2-script

This crate contains the standalone KCS instruction decoder/interpreter,
integer expression evaluator, and CST/FES script interpreters. It does not
load game executables or implement renderer, audio, object, or resource
services. Those effects are emitted as explicit host events.

## KCS execution

`ACTUAL_OPCODES` enumerates the 67 non-null dispatch slots established by the
CatSystem2 2.6.1.90 reference matrix. The reference sample corpus uses 53 of
those slots. `0x3f` and `0x40` are null entries, not executable instructions.
The interpreter has an execution arm for every listed slot. Operand and
instruction layout are documented by `operand_count` and `KcsProgram`.
The decoder re-exports the formats crate's operand-count table so the two
container readers cannot disagree about instruction boundaries. Container
inflation requires the complete zlib stream, its exact declared output, and
no unused compressed input.

The VM implements the verified integer, float, comparison, conversion,
addressing, branch, call-frame, native-call status, and debug-metadata rules.
Native behavior is supplied by the caller through `Host`; a missing host
contract is not represented as a fabricated return value. Invalid types,
addresses, stack operations, and call frames return `ScriptError`.

`Vm::run` remains the low-level entry for motion/particle tables.
`Vm::resume` and `Vm::dispatch_message` implement ordinary-task recovery,
including native 71's delayed 0/1 result, mode-zero suspension, and message
writeback from the latest receiver. `ResumeStep::Yield` retains an unfinished
budget continuation; continue through the same entry with the same message.
`ResumeStep::Completed` separately records the task's activity and native
yield event. A fresh entry at implicit code end is inactive; reaching that end
during execution remains active until the next entry.

## Structured scripts

`CstDocument::inputs` iterates the parsed input ranges and their records.
`FesDocument::cursor` traverses each parsed section followed by its lines in
source order. `CstInterpreter` adds input-range progression, message/name
records, conditions, assignments, strings, choices, scene call/next/return,
and timed/input waits. `FesProgram` and `FesInterpreter` add comment and macro
handling, block conditions, `next`/`call`/`break`/`exit`, expression statements,
call frames, declarations, and resumable command/wait/frame events.

`ExpressionState` owns shared values, FES local values, string variables, and
host-provided named values. `eval_expression` is the strict single-expression
entry point; `eval_expression_fallback` preserves the separately verified FES
whole-line fallback behavior. Both reject invalid indices, unclosed groups,
unsupported operators, and arithmetic failures.

`FesEvent::Command` and `CstEvent::Command` are the boundary for commands whose
behavior belongs to runtime/renderer/audio/resource services. The script crate
does not silently skip those commands. A caller must resume the interpreter
after consuming an event and may use `CstInterpreter::load_scene` for a scene
transfer.

Use `FesInterpreter::pending_command_parameters` or
`FesProgram::command_parameters` for host commands. String expansion before
macros and again after parameter extraction is significant: spaces introduced
by a macro's string reference stay inside one argument. The expanded event
source remains useful for logging, but cannot reconstruct those boundaries.
CST scene calls retain the caller's name and CSTL table; bind the callee's
table after `load_scene`, and return restores the original selection.
With a bound CSTL language, an empty name or an absent message fragment emits
empty localized text. Choice text keeps its separate fallback to CST when
the localized option is empty. An unavailable language can use the explicitly
configured fallback language; otherwise the original CST bytes are retained.
FES `strvar` formats at most four evaluated DWORD arguments. Integer formatting
supports signed widths and precisions, h/hh, l/I32, and little-endian pairs for
ll/I64; the formatted output and each field must stay below 2,560 bytes.
Missing arguments and unsupported conversions preserve the old destination
and return a diagnostic.

## Evidence boundary

KCS coverage is based on the local KatzeSystem2 2.6.1.90 documentation and
reference implementation. The 67-slot execution test verifies that every
established slot reaches a defined arm; individual behavior tests cover
numeric, memory, stack, frame, branching, and native-call contracts. It is not
a claim that all game-level native functions, scheduler behavior, or every
engine-version variation is implemented by this crate.

Opcode values outside the established set return `UnsupportedOpcode` when
passed to an operation helper or the VM dispatch boundary. The null slots
`0x3f` and `0x40` are rejected during decoding as `InvalidOpcode`.
`NullHost` rejects native calls with `UnimplementedNativeCall`; real native
behavior must be supplied by the caller.
