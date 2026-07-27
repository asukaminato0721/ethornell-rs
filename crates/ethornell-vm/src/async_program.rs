use crate::{
    GraphApi, SoundApi, SysApi, Value, Vm, VmRunOptions, VmStopReason, ADDRESS_MASK,
    LOCAL_MEMORY_BASE,
};
use ethornell_script::BpProgram;
use std::sync::Arc;

// BGI system programs are cooperative coroutines. In particular, mtnmngr runs
// several 80-entry queues for every 4 ms tick before reaching Sys80_5F (Yield).
const ASYNC_COOPERATIVE_STEP_LIMIT: usize = 2_000_000;

#[derive(Debug)]
pub(crate) struct AsyncProgramTask {
    key: String,
    vm: Box<Vm>,
    pub(crate) runnable: bool,
    completed: bool,
}

impl Vm {
    pub(crate) fn async_program_is_active(&self, program: Value) -> i32 {
        if matches!(program, Value::Int(_) | Value::Ptr(_)) {
            let thread_id = program.as_i32();
            if thread_id == 0 || thread_id == self.native_thread_id {
                return 1;
            }
        }
        let Some(key) = self.async_program_key(&program) else {
            return 0;
        };
        self.async_tasks
            .iter()
            .find(|task| task.key == key)
            .map_or(0, |task| i32::from(!task.completed))
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

        self.flush_shared_heap();
        let mut vm = Vm::new();
        vm.shared_heap = Arc::clone(&self.shared_heap);
        vm.sync_shared_heap();
        let shared_end = shared_memory_end(self);
        vm.memory[..shared_end].copy_from_slice(&self.memory[..shared_end]);
        vm.mem_values = self
            .mem_values
            .iter()
            .filter(|(addr, _)| **addr < LOCAL_MEMORY_BASE)
            .map(|(addr, value)| (*addr, value.clone()))
            .collect();
        vm.heap_ptr = self.heap_ptr;
        vm.record_tables = self.record_tables.clone();
        vm.indexed_record_tables = self.indexed_record_tables.clone();
        vm.script_records = self.script_records.clone();
        vm.string_hash_tables = self.string_hash_tables.clone();
        vm.read_flags = self.read_flags.clone();
        vm.global_config = self.global_config.clone();
        vm.global_user_data = self.global_user_data.clone();
        vm.loaded_bcs_ranges = self.loaded_bcs_ranges.clone();
        vm.timing = self.timing.clone();
        vm.rng_seed = self.rng_seed;
        vm.start(&program);
        vm.native_thread_id = self.next_program_instance_id.min(i32::MAX as u64) as i32;
        vm.mediation_programs = self.mediation_programs.clone();
        if !args.is_empty() {
            vm.pending_program_messages.extend(args);
        }
        self.async_tasks.retain(|task| task.key != key);
        self.async_tasks.push(AsyncProgramTask {
            key: key.clone(),
            vm: Box::new(vm),
            runnable: true,
            completed: false,
        });
        if trace_events {
            tracing::info!(program = key, "VM async program created");
        }
    }

    pub(crate) fn post_async_program_message(
        &mut self,
        program: Value,
        message: Value,
        trace_events: bool,
    ) -> bool {
        if matches!(program, Value::Int(_) | Value::Ptr(_)) {
            let thread_id = program.as_i32();
            if thread_id == self.native_thread_id {
                self.pending_program_messages.push_back(message);
                return true;
            }
            if thread_id == 0 {
                self.pending_root_program_messages.push_back(message);
                return true;
            }
        }
        let Some(key) = self.async_program_key(&program) else {
            if trace_events {
                tracing::warn!(
                    ?program,
                    ?message,
                    "VM program message target is not a program handle"
                );
            }
            return false;
        };
        let Some(task) = self
            .async_tasks
            .iter_mut()
            .find(|task| task.key == key && !task.completed)
        else {
            if trace_events {
                tracing::warn!(program = key, ?message, "VM program message target missing");
            }
            return false;
        };
        if trace_events {
            tracing::debug!(program = key, ?message, "VM program message posted");
        }
        task.vm.pending_program_messages.push_back(message);
        true
    }

    pub(crate) fn post_async_program_callback(
        &mut self,
        program: Value,
        callback: [Value; 3],
        trace_events: bool,
    ) -> bool {
        if matches!(program, Value::Int(_) | Value::Ptr(_)) {
            let thread_id = program.as_i32();
            if thread_id == self.native_thread_id {
                return self.enqueue_native_callback(callback, trace_events);
            }
            if thread_id == 0 {
                self.pending_root_program_callbacks.push_back(callback);
                return true;
            }
        }
        let Some(key) = self.async_program_key(&program) else {
            if trace_events {
                tracing::warn!(
                    ?program,
                    ?callback,
                    "VM callback target is not a thread handle"
                );
            }
            return false;
        };
        let Some(task) = self
            .async_tasks
            .iter_mut()
            .find(|task| task.key == key && !task.completed)
        else {
            if trace_events {
                tracing::warn!(program = key, ?callback, "VM callback target missing");
            }
            return false;
        };
        task.vm.enqueue_native_callback(callback, trace_events)
    }

    fn enqueue_native_callback(&mut self, callback: [Value; 3], trace_events: bool) -> bool {
        let active = self.wait_blocked || self.pending_graph_procedure.is_some();
        if active {
            if trace_events {
                tracing::debug!(?callback, "VM native procedure callback queued");
            }
            self.pending_program_callbacks.push_back(callback);
        }
        active
    }

    pub(crate) fn switch_to_async_program(&mut self, program: Value, trace_events: bool) -> bool {
        if matches!(program, Value::Int(_) | Value::Ptr(_)) {
            let thread_id = program.as_i32();
            if thread_id == 0 || thread_id == self.native_thread_id {
                return true;
            }
        }
        let Some(key) = self.async_program_key(&program) else {
            if trace_events {
                tracing::warn!(?program, "VM program switch target is not a program handle");
            }
            return false;
        };
        let Some(task) = self
            .async_tasks
            .iter_mut()
            .find(|task| task.key == key && !task.completed)
        else {
            if trace_events {
                tracing::warn!(program = key, "VM program switch target missing");
            }
            return false;
        };
        task.runnable = true;
        if trace_events || std::env::var_os("TRACE_ASYNC_PROGRAMS").is_some() {
            tracing::info!(program = key, "VM async program activated");
        }
        true
    }

    pub(crate) fn pump_async_programs<A>(&mut self, api: &mut A, trace_events: bool)
    where
        A: SysApi + GraphApi + SoundApi,
    {
        self.flush_shared_heap();
        self.sync_shared_heap();
        let trace_async = trace_events || std::env::var_os("TRACE_ASYNC_PROGRAMS").is_some();
        let task_count = self.async_tasks.len();
        for index in 0..task_count {
            if self.async_tasks[index].completed || !self.async_tasks[index].runnable {
                continue;
            }
            let options = VmRunOptions {
                max_steps: ASYNC_COOPERATIVE_STEP_LIMIT,
                trace: false,
                fail_on_stub: false,
                collect_diagnostics: false,
            };
            let mut task = self.async_tasks.remove(index);
            task.vm.sync_shared_heap();
            transfer_async_shared_state(self, &mut task.vm);
            let mut scheduler_tasks = std::mem::take(&mut self.async_tasks);
            scheduler_tasks.append(&mut task.vm.async_tasks);
            task.vm.async_tasks = scheduler_tasks;
            task.vm.suppress_async_pump_once = true;
            let report = task.vm.run_loaded(api, &options);
            task.vm.flush_shared_heap();
            self.sync_shared_heap();
            let root_messages = std::mem::take(&mut task.vm.pending_root_program_messages);
            let root_callbacks = std::mem::take(&mut task.vm.pending_root_program_callbacks);
            if trace_async {
                let (
                    scenario_pending,
                    motion_active,
                    motion_pending,
                    body_active,
                    body_pending,
                    body_records,
                ) = async_queue_summary(&task.vm);
                let local_base = (task.vm.mem_ptr & ADDRESS_MASK) as usize;
                let body_slot = local_base
                    .checked_sub(0x90)
                    .map(|addr| read_u32(&task.vm.memory, LOCAL_MEMORY_BASE as usize + addr));
                let body_record_ptr = local_base
                    .checked_sub(0x50)
                    .map(|addr| read_u32(&task.vm.memory, LOCAL_MEMORY_BASE as usize + addr));
                tracing::info!(
                    program = task.key,
                    steps = report.steps,
                    pc = report.pc,
                    offset = ?report.offset,
                    reason = ?report.stop_reason,
                    mem_ptr = format_args!("{:#010x}", task.vm.mem_ptr),
                    body_slot,
                    body_record_ptr = ?body_record_ptr.map(|ptr| format!("{ptr:#010x}")),
                    scenario_pending,
                    motion_active,
                    motion_pending,
                    body_active,
                    body_pending,
                    body_records = ?body_records,
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
            let mut scheduler_tasks = std::mem::take(&mut task.vm.async_tasks);
            transfer_async_shared_state(&mut task.vm, self);
            self.pending_program_messages.extend(root_messages);
            for callback in root_callbacks {
                self.enqueue_native_callback(callback, trace_events);
            }
            scheduler_tasks.insert(index.min(scheduler_tasks.len()), task);
            self.async_tasks = scheduler_tasks;
        }
        self.async_tasks.retain(|task| !task.completed);
    }

    pub(crate) fn mark_shared_heap_dirty(&mut self, range: std::ops::Range<usize>) {
        if range.start >= range.end {
            return;
        }
        let mut start = range.start;
        let mut end = range.end;
        self.shared_heap_dirty.retain(|existing| {
            if existing.end < start || existing.start > end {
                true
            } else {
                start = start.min(existing.start);
                end = end.max(existing.end);
                false
            }
        });
        self.shared_heap_dirty.push(start..end);
    }

    fn flush_shared_heap(&mut self) {
        if self.shared_heap_dirty.is_empty() {
            return;
        }
        let ranges = std::mem::take(&mut self.shared_heap_dirty);
        let mut shared = self
            .shared_heap
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shared.generation = shared.generation.saturating_add(1);
        let generation = shared.generation;
        for range in ranges {
            if shared.bytes.len() < range.end {
                shared.bytes.resize(range.end, 0);
            }
            shared.bytes[range.clone()].copy_from_slice(&self.memory[range.clone()]);
            shared
                .values
                .retain(|addr, _| !range.contains(&(*addr as usize)));
            shared.values.extend(
                self.mem_values
                    .iter()
                    .filter(|(addr, _)| range.contains(&(**addr as usize)))
                    .map(|(addr, value)| (*addr, value.clone())),
            );
            shared
                .journal
                .push(super::SharedHeapJournalEntry { generation, range });
        }
        self.shared_heap_generation = generation;
    }

    fn sync_shared_heap(&mut self) {
        let shared = self
            .shared_heap
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shared_heap_generation == shared.generation {
            return;
        }
        let ranges = shared
            .journal
            .iter()
            .filter(|entry| entry.generation > self.shared_heap_generation)
            .map(|entry| entry.range.clone())
            .collect::<Vec<_>>();
        for range in ranges {
            if self.memory.len() < range.end {
                self.memory.resize(range.end, 0);
            }
            self.memory[range.clone()].copy_from_slice(&shared.bytes[range.clone()]);
            self.mem_values
                .retain(|addr, _| !range.contains(&(*addr as usize)));
            self.mem_values.extend(
                shared
                    .values
                    .iter()
                    .filter(|(addr, _)| range.contains(&(**addr as usize)))
                    .map(|(addr, value)| (*addr, value.clone())),
            );
        }
        self.shared_heap_generation = shared.generation;
    }

    fn async_program_key(&self, value: &Value) -> Option<String> {
        if matches!(value, Value::Int(_) | Value::Ptr(_)) {
            let thread_id = value.as_i32();
            if let Some(task) = self
                .async_tasks
                .iter()
                .find(|task| task.vm.native_thread_id == thread_id)
            {
                return Some(task.key.clone());
            }
        }
        self.value_program(value.clone())
            .map(|program| program_key(&program))
    }

    pub(crate) fn value_program(&self, value: Value) -> Option<BpProgram> {
        match value {
            Value::Program(program) => Some((*program).clone()),
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

fn shared_memory_end(vm: &Vm) -> usize {
    (vm.heap_ptr as usize)
        .min(LOCAL_MEMORY_BASE as usize)
        .min(vm.memory.len())
}

fn transfer_async_shared_state(source: &mut Vm, destination: &mut Vm) {
    let shared_end = shared_memory_end(source);
    if destination.memory.len() < shared_end {
        destination.memory.resize(shared_end, 0);
    }
    destination.memory[..shared_end].copy_from_slice(&source.memory[..shared_end]);

    let mut shared_values = std::collections::HashMap::new();
    for (addr, value) in std::mem::take(&mut source.mem_values) {
        if addr < LOCAL_MEMORY_BASE {
            shared_values.insert(addr, value);
        } else {
            source.mem_values.insert(addr, value);
        }
    }
    destination
        .mem_values
        .retain(|addr, _| *addr >= LOCAL_MEMORY_BASE);
    destination.mem_values.extend(shared_values);

    std::mem::swap(&mut source.heap_ptr, &mut destination.heap_ptr);
    std::mem::swap(&mut source.record_tables, &mut destination.record_tables);
    std::mem::swap(
        &mut source.indexed_record_tables,
        &mut destination.indexed_record_tables,
    );
    std::mem::swap(&mut source.script_records, &mut destination.script_records);
    std::mem::swap(
        &mut source.string_hash_tables,
        &mut destination.string_hash_tables,
    );
    std::mem::swap(
        &mut source.mediation_programs,
        &mut destination.mediation_programs,
    );
    std::mem::swap(&mut source.read_flags, &mut destination.read_flags);
    std::mem::swap(&mut source.global_config, &mut destination.global_config);
    std::mem::swap(
        &mut source.global_user_data,
        &mut destination.global_user_data,
    );
    std::mem::swap(
        &mut source.loaded_bcs_ranges,
        &mut destination.loaded_bcs_ranges,
    );
    std::mem::swap(&mut source.timing, &mut destination.timing);
    std::mem::swap(&mut source.rng_seed, &mut destination.rng_seed);
    std::mem::swap(
        &mut source.next_program_instance_id,
        &mut destination.next_program_instance_id,
    );
}

fn async_queue_summary(vm: &Vm) -> (u32, usize, usize, usize, usize, Vec<[u32; 7]>) {
    let scenario_pending = read_u32(&vm.memory, 1_644);
    let mut motion_active = 0usize;
    let mut motion_pending = 0usize;
    for slot in 0..80usize {
        let base = 11_280 + slot * 432;
        motion_active += usize::from(read_u32(&vm.memory, base) != 0);
        motion_pending += usize::from(read_u32(&vm.memory, base + 8) != 0);
    }
    let mut body_active = 0usize;
    let mut body_pending = 0usize;
    let mut body_records = Vec::new();
    for slot in 0..64usize {
        let base = 45_840 + slot * 3_208;
        let active = read_u32(&vm.memory, base);
        body_active += usize::from(active != 0);
        body_pending += usize::from(read_u32(&vm.memory, base + 4) != 0);
        if active != 0 {
            body_records.push([
                slot as u32,
                active,
                read_u32(&vm.memory, base + 4),
                read_u32(&vm.memory, base + 8),
                read_u32(&vm.memory, base + 12),
                read_u32(&vm.memory, base + 16),
                read_u32(&vm.memory, base + 20),
            ]);
        }
    }
    (
        scenario_pending,
        motion_active,
        motion_pending,
        body_active,
        body_pending,
        body_records,
    )
}

fn read_u32(memory: &[u8], addr: usize) -> u32 {
    memory
        .get(addr..addr.saturating_add(4))
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .unwrap_or_default()
}
