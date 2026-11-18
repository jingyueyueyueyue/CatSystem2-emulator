use crate::{
    OrdinaryTask, RuntimeError, RuntimeResult, RuntimeState, TaskCommand,
    TaskCommandBuffers, TaskCommandOutcome, TaskMessage,
};
use cs2_script::{Host, NativeReturn, ResumeStep, Step, Vm};
use std::collections::BTreeMap;

pub type NativeHandler =
    Box<dyn FnMut(&[u8], &mut Vm, &mut RuntimeState) -> RuntimeResult<NativeReturn> + Send>;

#[derive(Default)]
pub struct NativeCallRegistry {
    handlers: BTreeMap<u32, NativeHandler>,
}

impl NativeCallRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        index: u32,
        handler: impl FnMut(&[u8], &mut Vm, &mut RuntimeState) -> RuntimeResult<NativeReturn>
        + Send
        + 'static,
    ) -> Option<NativeHandler> {
        self.handlers.insert(index, Box::new(handler))
    }

    pub fn unregister(&mut self, index: u32) -> bool {
        self.handlers.remove(&index).is_some()
    }

    pub fn contains(&self, index: u32) -> bool {
        self.handlers.contains_key(&index)
    }

    pub fn indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.handlers.keys().copied()
    }

    pub fn run(
        &mut self,
        vm: &mut Vm,
        state: &mut RuntimeState,
        instruction_budget: usize,
    ) -> RuntimeResult<Step> {
        let mut host = RegistryHost {
            handlers: &mut self.handlers,
            state,
            failure: None,
        };
        let result = vm.run(&mut host, instruction_budget);
        match host.failure.take() {
            Some(error) => Err(error),
            None => result.map_err(RuntimeError::Script),
        }
    }

    pub fn resume(
        &mut self,
        vm: &mut Vm,
        state: &mut RuntimeState,
        instruction_budget: usize,
    ) -> RuntimeResult<ResumeStep> {
        self.with_host(state, |host| {
            vm.resume(host, instruction_budget).map_err(RuntimeError::Script)
        })
    }

    pub fn update_task(
        &mut self,
        task: &mut OrdinaryTask,
        state: &mut RuntimeState,
        now: u32,
        increment: u32,
        instruction_budget: usize,
    ) -> RuntimeResult<bool> {
        self.with_host(state, |host| task.update(now, increment, host, instruction_budget))
    }

    pub fn update_task_body(
        &mut self,
        task: &mut OrdinaryTask,
        state: &mut RuntimeState,
        increment: u32,
        instruction_budget: usize,
    ) -> RuntimeResult<bool> {
        self.with_host(state, |host| task.update_body(increment, host, instruction_budget))
    }

    pub fn dispatch_task_message(
        &mut self,
        task: &mut OrdinaryTask,
        state: &mut RuntimeState,
        message: &mut TaskMessage,
        output: &mut u32,
        instruction_budget: usize,
    ) -> RuntimeResult<bool> {
        self.with_host(state, |host| {
            task.dispatch_message(message, output, host, instruction_budget)
        })
    }

    pub fn request_task_delete(
        &mut self,
        task: &mut OrdinaryTask,
        state: &mut RuntimeState,
        instruction_budget: usize,
    ) -> RuntimeResult<bool> {
        self.with_host(state, |host| task.request_delete(host, instruction_budget))
    }

    pub fn dispatch_task_control(
        &mut self,
        task: &mut OrdinaryTask,
        object_type: u32,
        state: &mut RuntimeState,
        command: TaskCommand,
        output: &mut u32,
        buffers: &mut TaskCommandBuffers<'_>,
        instruction_budget: usize,
    ) -> RuntimeResult<TaskCommandOutcome> {
        self.with_host(state, |host| task.dispatch_control_with_buffers(
            object_type, command, output, buffers, host, instruction_budget,
        ))
    }

    fn with_host<T>(
        &mut self,
        state: &mut RuntimeState,
        call: impl FnOnce(&mut RegistryHost<'_>) -> RuntimeResult<T>,
    ) -> RuntimeResult<T> {
        let mut host = RegistryHost {
            handlers: &mut self.handlers,
            state,
            failure: None,
        };
        let result = call(&mut host);
        match host.failure.take() {
            Some(error) => Err(error),
            None => result,
        }
    }
}

struct RegistryHost<'a> {
    handlers: &'a mut BTreeMap<u32, NativeHandler>,
    state: &'a mut RuntimeState,
    failure: Option<RuntimeError>,
}

impl Host for RegistryHost<'_> {
    fn call(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let Some(handler) = self.handlers.get_mut(&index) else {
            self.failure = Some(RuntimeError::UnknownNativeCall(index));
            return Ok(NativeReturn::Retry);
        };
        match handler(arguments, vm, self.state) {
            Ok(result) => Ok(result),
            Err(error) => {
                self.failure = Some(RuntimeError::NativeCall {
                    index,
                    message: error.to_string(),
                });
                Ok(NativeReturn::Retry)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_script::{KcsProgram, NativeReturn, Step};

    fn operand(flags: u8, value: u32, code: &mut Vec<u8>) {
        code.push(flags);
        code.extend_from_slice(&value.to_le_bytes());
    }

    fn program_with_native_call(index: u32) -> KcsProgram {
        let mut code = vec![0x25];
        operand(0x23, 7, &mut code);
        code.push(0x2b);
        operand(0x23, index, &mut code);
        operand(0x23, 4, &mut code);
        code.push(0x3b);
        KcsProgram::from_code(&code, Vec::new(), 16).unwrap()
    }

    #[test]
    fn dispatches_registered_native_and_preserves_return_value() {
        let mut registry = NativeCallRegistry::new();
        registry.register(12, |arguments, _vm, state| {
            assert_eq!(arguments, &7_u32.to_le_bytes());
            state
                .variables
                .insert("called".into(), crate::RuntimeValue::U32(1));
            Ok(NativeReturn::Value(42))
        });
        let mut vm = Vm::from_program(program_with_native_call(12)).unwrap();
        let mut state = RuntimeState::default();

        assert_eq!(registry.run(&mut vm, &mut state, 16).unwrap(), Step::Halted);
        assert_eq!(vm.stack_bits().collect::<Vec<_>>(), vec![42]);
        assert_eq!(
            state.variables.get("called"),
            Some(&crate::RuntimeValue::U32(1))
        );
    }

    #[test]
    fn missing_native_is_an_explicit_runtime_error() {
        let mut registry = NativeCallRegistry::new();
        let mut vm = Vm::from_program(program_with_native_call(99)).unwrap();
        let error = registry
            .run(&mut vm, &mut RuntimeState::default(), 16)
            .unwrap_err();
        assert!(matches!(error, RuntimeError::UnknownNativeCall(99)));
    }

    #[test]
    fn preserves_structured_vm_errors() {
        let mut registry = NativeCallRegistry::new();
        let mut vm = Vm::new();
        let error = registry
            .run(&mut vm, &mut RuntimeState::default(), 16)
            .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::Script(cs2_script::ScriptError::NoProgram)
        ));
    }
}
