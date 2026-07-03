use crate::{GraphApi, SoundApi, SysApi, Value, Vm, VmRunOptions, VmStopReason};
use ethornell_script::BpProgram;

#[derive(Debug)]
pub(crate) struct AsyncProgramTask {
    key: String,
    vm: Box<Vm>,
    completed: bool,
}

#[derive(Debug)]
struct AsyncSharedState {
    memory: Vec<u8>,
    mem_values: std::collections::BTreeMap<u32, Value>,
    heap_ptr: u32,
    mem_ptr: u32,
    record_tables: std::collections::BTreeMap<u32, crate::records::RecordTableState>,
    indexed_record_tables: std::collections::BTreeMap<u32, crate::records::IndexedRecordState>,
    script_records: std::collections::BTreeMap<u32, String>,
    loaded_bcs_ranges: Vec<crate::scenario::LoadedBcsRange>,
    timing: crate::time::VmTime,
}

impl AsyncSharedState {
    fn capture(vm: &Vm) -> Self {
        Self {
            memory: vm.memory.clone(),
            mem_values: vm.mem_values.clone(),
            heap_ptr: vm.heap_ptr,
            mem_ptr: vm.mem_ptr,
            record_tables: vm.record_tables.clone(),
            indexed_record_tables: vm.indexed_record_tables.clone(),
            script_records: vm.script_records.clone(),
            loaded_bcs_ranges: vm.loaded_bcs_ranges.clone(),
            timing: vm.timing.clone(),
        }
    }
}

impl Vm {
    pub(crate) fn async_program_is_complete<A>(
        &mut self,
        program: Value,
        api: &mut A,
        trace_events: bool,
    ) -> i32
    where
        A: SysApi + GraphApi + SoundApi,
    {
        let Some(key) = self.async_program_key(&program) else {
            return 1;
        };

        let (shared_state, completed) = {
            let Some(task) = self.async_tasks.iter_mut().find(|task| task.key == key) else {
                return 1;
            };
            if task.completed {
                return 1;
            }

            let options = VmRunOptions {
                max_steps: 2_000,
                trace: false,
                fail_on_stub: false,
            };
            let report = task.vm.run_loaded(api, &options);
            if trace_events {
                tracing::info!(
                    program = task.key,
                    steps = report.steps,
                    pc = report.pc,
                    offset = ?report.offset,
                    reason = ?report.stop_reason,
                    "VM async program pump"
                );
            }
            let completed = matches!(
                report.stop_reason,
                VmStopReason::Completed
                    | VmStopReason::Error
                    | VmStopReason::UnknownOpcode
                    | VmStopReason::UnknownDispatch
            );
            if completed {
                task.completed = true;
            }
            (AsyncSharedState::capture(&task.vm), completed)
        };
        self.apply_async_shared_state(shared_state);
        if completed {
            1
        } else {
            0
        }
    }

    pub(crate) fn start_async_program(&mut self, program: Value, trace_events: bool) {
        self.start_async_program_with_args(program, Vec::new(), trace_events);
    }

    pub(crate) fn start_async_program_with_args(
        &mut self,
        program: Value,
        args: Vec<Value>,
        trace_events: bool,
    ) {
        let Some(program) = self.value_program(program) else {
            return;
        };
        let key = program_key(&program);
        if let Some(task) = self.async_tasks.iter_mut().find(|task| task.key == key) {
            if !task.completed {
                return;
            }
        }

        let mut vm = Vm::new();
        vm.memory = self.memory.clone();
        vm.mem_values = self.mem_values.clone();
        vm.heap_ptr = self.heap_ptr;
        vm.record_tables = self.record_tables.clone();
        vm.indexed_record_tables = self.indexed_record_tables.clone();
        vm.script_records = self.script_records.clone();
        vm.loaded_bcs_ranges = self.loaded_bcs_ranges.clone();
        vm.timing = self.timing.clone();
        vm.start(&program);
        for arg in args.into_iter().rev() {
            vm.stack.push(arg);
        }
        self.async_tasks.retain(|task| task.key != key);
        self.async_tasks.push(AsyncProgramTask {
            key: key.clone(),
            vm: Box::new(vm),
            completed: false,
        });
        if trace_events {
            tracing::info!(program = key, "VM async program started");
        }
    }

    pub(crate) fn pump_async_programs<A>(&mut self, api: &mut A, trace_events: bool)
    where
        A: SysApi + GraphApi + SoundApi,
    {
        let mut shared_states = Vec::new();
        for task in self.async_tasks.iter_mut().filter(|task| !task.completed) {
            let options = VmRunOptions {
                max_steps: 2_000,
                trace: false,
                fail_on_stub: false,
            };
            let report = task.vm.run_loaded(api, &options);
            if trace_events {
                tracing::info!(
                    program = task.key,
                    steps = report.steps,
                    pc = report.pc,
                    offset = ?report.offset,
                    reason = ?report.stop_reason,
                    "VM async program pump"
                );
            }
            if matches!(
                report.stop_reason,
                VmStopReason::Completed
                    | VmStopReason::Error
                    | VmStopReason::UnknownOpcode
                    | VmStopReason::UnknownDispatch
            ) {
                task.completed = true;
            }
            shared_states.push(AsyncSharedState::capture(&task.vm));
        }
        for shared_state in shared_states {
            self.apply_async_shared_state(shared_state);
        }
    }

    fn apply_async_shared_state(&mut self, state: AsyncSharedState) {
        self.memory = state.memory;
        self.mem_values = state.mem_values;
        self.heap_ptr = self.heap_ptr.max(state.heap_ptr);
        self.mem_ptr = self.mem_ptr.max(state.mem_ptr);
        self.record_tables = state.record_tables;
        self.indexed_record_tables = state.indexed_record_tables;
        self.script_records = state.script_records;
        self.loaded_bcs_ranges = state.loaded_bcs_ranges;
        self.timing = state.timing;
    }

    fn async_program_key(&self, value: &Value) -> Option<String> {
        self.value_program(value.clone())
            .map(|program| program_key(&program))
    }

    pub(crate) fn value_program(&self, value: Value) -> Option<BpProgram> {
        match value {
            Value::Program(program) => Some(*program),
            Value::Ptr(ptr) => self
                .mem_values
                .get(&Self::value_key(ptr))
                .and_then(|value| {
                    if let Value::Program(program) = value {
                        Some((**program).clone())
                    } else {
                        None
                    }
                }),
            Value::Int(ptr) => {
                self.mem_values
                    .get(&Self::value_key(ptr as u32))
                    .and_then(|value| {
                        if let Value::Program(program) = value {
                            Some((**program).clone())
                        } else {
                            None
                        }
                    })
            }
            Value::Str(_) | Value::Func { .. } | Value::None => None,
        }
    }
}

fn program_key(program: &BpProgram) -> String {
    program
        .script_name
        .as_deref()
        .unwrap_or("<anonymous>")
        .to_string()
}
