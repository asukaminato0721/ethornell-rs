use ethornell_script::{
    calls::{known_call_arg_count, known_call_returns_value, known_call_stack_output_count},
    known_call_name, BpInstruction, BpOpcode, BpOperand, BpProgram,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

mod async_program;
mod debug;
mod extended_opcodes;
mod input;
pub mod native_ownership;
mod profile;
mod records;
mod scenario;
mod time;
mod user_data;

const SYSTEM_PROGRAM_TABLE: u32 = 273_280;
const SYSTEM_PROGRAM_SLOTS: usize = 32;
const SYSTEM_PROGRAM_STRIDE: u32 = 16;
const SYSTEM_PROGRAM_DESCRIPTOR_BASE: u32 = 0x4000_0000;
const ADDRESS_MASK: u32 = 0x01ff_ffff;
const LOCAL_MEMORY_BASE: u32 = 0x0080_0000;
const HEAP_OFFSET_BASE: u32 = 0x0020_0000;
const HEAP_MEMORY_BASE: usize = (LOCAL_MEMORY_BASE + HEAP_OFFSET_BASE) as usize;
const INITIAL_MEMORY_SIZE: usize = 16 * 1024 * 1024;
const MAX_MEMORY_SIZE: usize = 64 * 1024 * 1024;
const OPERAND_STACK_CAPACITY: usize = 4096;
static NEXT_VM_TRACE_ID: AtomicU64 = AtomicU64::new(1);

pub type VmResult<T> = std::result::Result<T, VmError>;

#[derive(Debug, thiserror::Error)]
pub enum VmError {
    #[error("stack underflow")]
    StackUnderflow,
    #[error("unsupported instruction: 0x{0:02x}")]
    UnsupportedInstruction(u8),
    #[error("unknown dispatch group=0x{group:02x} id=0x{id:02x}")]
    UnknownDispatch { group: u8, id: u16 },
    #[error("memory access out of bounds addr=0x{addr:08x} size={size}")]
    MemoryOutOfBounds { addr: u32, size: usize },
    #[error("{0}")]
    Runtime(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i32),
    Str(String),
    Ptr(u32),
    Func { program_index: usize, offset: u32 },
    Program(Arc<BpProgram>),
    None,
}

impl Value {
    fn as_i32(&self) -> i32 {
        match self {
            Value::Int(v) => *v,
            Value::Ptr(v) => *v as i32,
            Value::Func { offset, .. } => *offset as i32,
            Value::Str(_) | Value::Program(_) | Value::None => 0,
        }
    }
}

fn measure_text_width_value(value: &Value, font_size: i32, max_width: i32) -> i32 {
    let text = match value {
        Value::Str(text) => text.as_str(),
        Value::Int(0) | Value::Ptr(0) | Value::None => "",
        Value::Int(_) | Value::Ptr(_) | Value::Func { .. } | Value::Program(_) => {
            return max_width.max(font_size);
        }
    };
    let mut width = 0i32;
    for ch in text.chars() {
        width += if ch.is_ascii() {
            (font_size + 1) / 2
        } else {
            font_size
        };
    }
    if max_width > 0 {
        width.min(max_width)
    } else {
        width
    }
}

#[derive(Debug, Clone)]
pub struct VmRunOptions {
    pub max_steps: usize,
    pub trace: bool,
    pub fail_on_stub: bool,
    pub collect_diagnostics: bool,
}

impl Default for VmRunOptions {
    fn default() -> Self {
        Self {
            max_steps: 10_000,
            trace: false,
            fail_on_stub: false,
            collect_diagnostics: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmStopReason {
    Completed,
    MaxSteps,
    WaitingForInput,
    WaitingForAnimation,
    UnknownOpcode,
    UnknownDispatch,
    Error,
}

#[derive(Debug, Clone)]
pub struct VmRunReport {
    pub steps: usize,
    pub pc: usize,
    pub offset: Option<u32>,
    pub program: String,
    pub stop_reason: VmStopReason,
    pub calls: BTreeMap<String, usize>,
    pub stubs: BTreeMap<String, usize>,
    pub recent_trace: Vec<String>,
}

pub trait SysApi {
    fn call_sys(&mut self, group: u8, id: u16, stack: &mut Vec<Value>) -> VmResult<Value>;

    fn observe_dispatch(&mut self, _group: u8, _id: u16) {}

    fn take_runtime_stub(&mut self) -> bool {
        false
    }

    fn register_graphic_resource(
        &mut self,
        _namespace: i32,
        _resource_id: i32,
        _path: &str,
    ) -> bool {
        false
    }

    fn post_queued_event(&mut self, _code: i32, _parameter: i32) {}

    fn poll_queued_event(&mut self) -> Option<[i32; 3]> {
        None
    }

    fn dispatch_object_event(
        &mut self,
        _object: i32,
        _count: i32,
        _descriptor: &[Value],
    ) -> VmResult<()> {
        Ok(())
    }

    fn load_file_bytes(&mut self, _archive: &str, _file: &str) -> Option<Vec<u8>> {
        None
    }

    fn file_exists(&mut self, archive: &str, file: &str) -> bool {
        self.load_file_bytes(archive, file).is_some()
    }

    fn file_size(&mut self, archive: &str, file: &str) -> i32 {
        self.load_file_bytes(archive, file)
            .map(|bytes| bytes.len() as i32)
            .unwrap_or(-1)
    }

    fn write_file_bytes(&mut self, _path: &str, _bytes: &[u8]) -> bool {
        false
    }

    fn read_user_file_bytes(&mut self, _path: &str) -> Option<Vec<u8>> {
        None
    }

    fn enumerate_user_files(
        &mut self,
        _pattern: &str,
        _recursive: bool,
        _max_count: usize,
    ) -> Vec<String> {
        Vec::new()
    }

    fn enumerate_user_directories(&mut self, _pattern: &str, _max_count: usize) -> Vec<String> {
        Vec::new()
    }

    fn take_dropped_file(&mut self) -> Option<String> {
        None
    }

    fn native_save_header(&mut self, _slot: i32) -> Option<[u8; 64]> {
        None
    }

    fn registered_object_value(&mut self, _object: i32) -> Option<i32> {
        None
    }

    fn host_user_name(&mut self) -> String {
        String::new()
    }

    fn host_computer_name(&mut self) -> String {
        String::new()
    }

    fn keyboard_state(&mut self) -> [u8; 256] {
        [0; 256]
    }

    fn pointer_position(&mut self, _index: i32) -> Option<(i32, i32)> {
        None
    }

    fn runtime_command_line(&mut self) -> String {
        String::new()
    }

    fn delete_file(&mut self, _root: &str, _file: &str) -> bool {
        false
    }

    fn user_data_root(&mut self, _kind: i32) -> Option<String> {
        None
    }

    fn save_global_user_data(&mut self) -> bool {
        false
    }

    fn load_program(&mut self, archive: &str, file: &str) -> Option<BpProgram> {
        let script_name = Some(format!("{archive}:{file}"));
        self.load_file_bytes(archive, file)
            .map(|bytes| ethornell_script::parse_bp_program(script_name, &bytes))
    }

    fn load_program_ex(
        &mut self,
        archive: &str,
        file: &str,
        _params: &[Value],
    ) -> Option<BpProgram> {
        self.load_program(archive, file)
    }

    fn free_program(&mut self, _program: Value) {}

    fn read_input_state(&mut self, _descriptor: i32) -> i32 {
        0
    }

    fn query_input_class_state(&mut self, _scope: i32) -> i32 {
        0
    }

    fn query_input_descriptor_state(&mut self, _class_mask: i32) -> i32 {
        0
    }

    fn set_input_master_gate(&mut self, _value: i32) {}

    fn set_input_latched_state(&mut self, _value: i32) {}

    fn take_frame_yield(&mut self) -> bool {
        false
    }
}

pub trait GraphApi {
    fn call_graph(&mut self, group: u8, id: u16, stack: &mut Vec<Value>) -> VmResult<Value>;

    fn query_graph_effect_result(&self, _process: i32) -> Option<i32> {
        None
    }

    fn invoke_graph_effect_process(&mut self, _process: i32) -> GraphEffectInvocation {
        GraphEffectInvocation {
            status: 4,
            duration_ms: 0,
        }
    }

    fn cache_graph_blob(&mut self, _namespace: &str, _name: &str, _bytes: &[u8]) -> bool {
        false
    }

    fn create_bitmap_from_rgb(
        &mut self,
        _bitmap: i32,
        _width: i32,
        _height: i32,
        _format: i32,
        _pixels: &[u8],
    ) -> bool {
        false
    }

    fn read_bitmap_pixels(&mut self, _bitmap: i32, _capacity: usize) -> Option<Vec<u8>> {
        None
    }

    fn set_bitmap_dimensions(&mut self, _bitmap: i32, _width: i32, _height: i32) -> bool {
        false
    }

    fn query_bitmap_info(&mut self, _bitmap: i32) -> Option<BitmapInfo> {
        None
    }

    fn call_graph_spline_control(
        &mut self,
        args: &[Value],
        _points: &[[i32; 4]],
    ) -> VmResult<Value> {
        let mut stack = args.to_vec();
        self.call_graph(0x90, 0x29, &mut stack)
    }

    fn take_graph_procedure_schedule(&mut self) -> Option<GraphProcedureSchedule> {
        None
    }

    fn configure_graph_input_object(&mut self, _object: i32, _descriptor: GraphInputDescriptor) {}

    fn configure_graph_surface_controls(
        &mut self,
        _surface: i32,
        _descriptor: GraphInputDescriptor,
    ) {
    }

    fn collect_ruby_substitutions(&mut self, _source: &str) -> (String, i32) {
        (String::new(), 0)
    }

    fn poll_object_state(&mut self, _object: i32) -> i32 {
        0
    }

    fn poll_object_event(&mut self, _object: i32) -> i32 {
        0
    }

    fn poll_object_event_payload(&mut self, object: i32) -> (i32, i32) {
        (self.poll_object_event(object), 0)
    }

    fn poll_object_state_record(&mut self, object: i32) -> [i32; 6] {
        [self.poll_object_state(object), 0, 0, 0, 0, 0]
    }

    fn poll_object_event_record(&mut self, object: i32) -> [i32; 3] {
        let (event, payload) = self.poll_object_event_payload(object);
        [event, payload, 0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphEffectInvocation {
    pub status: i32,
    pub duration_ms: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphInputDescriptor {
    pub initial_group: i32,
    pub flags: [i32; 7],
    pub regions: Vec<GraphInputRegion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphInputRegion {
    pub group: i32,
    pub index: i32,
    pub ordinal: i32,
    pub enabled_depth: i32,
    pub selected: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub normal_resource: i32,
    pub selected_resource: i32,
    pub mask_resource: i32,
    pub flags: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitmapInfo {
    pub width: u32,
    pub height: u32,
    pub format: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphProcedureSchedule {
    pub duration_ms: i32,
    pub input_enabled: bool,
    pub input_descriptor: i32,
    pub wait_for_input: bool,
    pub completion: GraphProcedureCompletion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphProcedureCompletion {
    None,
    ControlProgress,
    MessageInterrupted,
}

#[derive(Debug, Clone, Copy)]
struct PendingGraphProcedure {
    started_ms: i32,
    duration_ms: i32,
    input_enabled: bool,
    input_descriptor: i32,
    wait_for_input: bool,
    completion: GraphProcedureCompletion,
}

pub trait SoundApi {
    fn call_sound(&mut self, group: u8, id: u16, stack: &mut Vec<Value>) -> VmResult<Value>;

    fn observe_user(&mut self, _group: u8, _id: u16, _stack: &[Value]) {}

    fn call_user(
        &mut self,
        _group: u8,
        _id: u16,
        _stack: &mut Vec<Value>,
    ) -> VmResult<Option<Value>> {
        Ok(None)
    }

    fn current_user_text(&self) -> String {
        String::new()
    }

    fn configure_user_polygon(&mut self, _handle: i32, _mode: i32, _points: &[[i32; 3]]) -> i32 {
        1
    }

    fn query_user_polygon(&self, _handle: i32, _index: i32) -> Option<[i32; 3]> {
        None
    }
}

fn empty_loaded_program(name: String) -> BpProgram {
    placeholder_loaded_program(
        name,
        "ret",
        "generated empty program after runtime load failure",
    )
}

fn native_file_hash(bytes: &[u8]) -> [u32; 2] {
    native_file_hash_update([0, 0], bytes)
}

fn native_file_hash_update(initial: [u32; 2], bytes: &[u8]) -> [u32; 2] {
    let mut hash = initial[0];
    let mut tail = initial[1].to_le_bytes();
    for &byte in bytes {
        hash = hash.wrapping_mul(233).wrapping_add(u32::from(byte));
        let low = hash as u8;
        tail[0] = low.wrapping_add(tail[0]);
        tail[1] = low ^ tail[1];
        tail[2] = tail[2].wrapping_add(byte);
        tail[3] = byte ^ tail[3];
    }
    [hash, u32::from_le_bytes(tail)]
}

fn wide_string_similarity(left: &[u16], right: &[u16]) -> i32 {
    if left.is_empty() || right.is_empty() {
        return 0;
    }
    let mut previous = vec![0usize; right.len() + 1];
    let mut current = vec![0usize; right.len() + 1];
    for &left_value in left {
        for (index, &right_value) in right.iter().enumerate() {
            current[index + 1] = if left_value == right_value {
                previous[index] + 1
            } else {
                current[index].max(previous[index + 1])
            };
        }
        std::mem::swap(&mut previous, &mut current);
        current.fill(0);
    }
    previous[right.len()].min(i32::MAX as usize) as i32
}

fn placeholder_loaded_program(
    name: String,
    opcode_name: &'static str,
    warning: &'static str,
) -> BpProgram {
    let mut labels = HashMap::new();
    labels.insert(0x10, 0);
    BpProgram {
        script_name: Some(name),
        functions: Vec::new(),
        strings: Vec::new(),
        instructions: vec![BpInstruction {
            offset: 0x10,
            opcode: BpOpcode::Known {
                code: 0x17,
                name: opcode_name,
            },
            opcode_hex: "0x17".into(),
            opcode_name: opcode_name.into(),
            operands: Vec::new(),
            known_call: None,
            raw: vec![0x17],
            warning: Some(warning.into()),
        }],
        labels,
        warnings: vec![warning.into()],
    }
}

#[derive(Debug, Default)]
pub struct Vm {
    trace_id: u64,
    pub stack: Vec<Value>,
    operand_slots: Vec<Value>,
    operand_slots_synced_len: usize,
    pub pc: usize,
    pub call_stack: Vec<(usize, usize, usize)>,
    pub programs: Vec<BpProgram>,
    pub current_program: usize,
    pub memory: Vec<u8>,
    pub mem_values: HashMap<u32, Value>,
    pub mem_ptr: u32,
    pub heap_ptr: u32,
    heap_allocations: BTreeMap<u32, u32>,
    heap_free_blocks: Vec<(u32, u32)>,
    shared_heap: Arc<Mutex<SharedHeapState>>,
    shared_heap_generation: u64,
    shared_heap_dirty: Vec<std::ops::Range<usize>>,
    pub halted: bool,
    pub calls: BTreeMap<String, usize>,
    pub stubs: BTreeMap<String, usize>,
    program_cache: BTreeMap<String, usize>,
    program_free_stack: Vec<usize>,
    mediation_programs: BTreeMap<u8, BpProgram>,
    record_tables: BTreeMap<u32, records::RecordTableState>,
    indexed_record_tables: BTreeMap<u32, records::IndexedRecordState>,
    script_records: BTreeMap<u32, String>,
    string_hash_tables: BTreeMap<i32, Vec<String>>,
    read_flags: BTreeMap<String, ReadFlagBits>,
    global_config: Vec<u8>,
    global_user_data: Vec<u8>,
    loaded_bcs_ranges: Vec<scenario::LoadedBcsRange>,
    async_tasks: Vec<async_program::AsyncProgramTask>,
    suppress_async_pump_once: bool,
    timing: time::VmTime,
    rng_seed: u32,
    recent_trace: VecDeque<String>,
    yield_requested: bool,
    wait_blocked: bool,
    wait_input_scope: Option<i32>,
    pending_graph_procedure: Option<PendingGraphProcedure>,
    pending_program_messages: VecDeque<Value>,
    pending_root_program_messages: VecDeque<Value>,
    pending_program_callbacks: VecDeque<[Value; 3]>,
    pending_root_program_callbacks: VecDeque<[Value; 3]>,
    native_thread_id: i32,
    next_program_instance_id: u64,
    collect_diagnostics: bool,
    extended_opcodes: extended_opcodes::ExtendedOpcodeState,
}

#[derive(Debug, Default)]
struct SharedHeapState {
    bytes: Vec<u8>,
    values: HashMap<u32, Value>,
    generation: u64,
    journal: Vec<SharedHeapJournalEntry>,
}

#[derive(Debug, Clone)]
struct SharedHeapJournalEntry {
    generation: u64,
    range: std::ops::Range<usize>,
}

#[derive(Debug, Clone, Default)]
struct ReadFlagBits {
    bit_len: u32,
    bytes: Vec<u8>,
}

impl ReadFlagBits {
    fn new(bit_len: usize) -> Self {
        let bit_len = u32::try_from(bit_len).unwrap_or(u32::MAX);
        let byte_len = usize::try_from(bit_len.div_ceil(8)).unwrap_or_default();
        Self {
            bit_len,
            bytes: vec![0; byte_len],
        }
    }

    fn contains(&self, bit: u32) -> Option<bool> {
        if bit >= self.bit_len {
            return None;
        }
        let byte = *self.bytes.get((bit / 8) as usize)?;
        Some(byte & (1 << (bit & 7)) != 0)
    }

    fn resize(&mut self, bit_len: usize) -> bool {
        let Ok(bit_len) = u32::try_from(bit_len) else {
            return false;
        };
        if bit_len == 0 {
            return false;
        }
        let Ok(byte_len) = usize::try_from(bit_len.div_ceil(8)) else {
            return false;
        };
        self.bytes.resize(byte_len, 0);
        self.bit_len = bit_len;
        if bit_len & 7 != 0 {
            let last_mask = (1u16 << (bit_len & 7)) as u8 - 1;
            if let Some(last) = self.bytes.last_mut() {
                *last &= last_mask;
            }
        }
        true
    }

    fn set(&mut self, bit: u32, enabled: bool) -> bool {
        if bit >= self.bit_len {
            return false;
        }
        let byte = &mut self.bytes[(bit / 8) as usize];
        let mask = 1 << (bit & 7);
        if enabled {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
        true
    }

    fn set_range(&mut self, start: u32, length: u32, enabled: bool) -> bool {
        let Some(end) = start.checked_add(length) else {
            return false;
        };
        if length == 0 || start >= self.bit_len || end > self.bit_len {
            return false;
        }

        let mut cursor = start;
        while cursor < end && cursor & 7 != 0 {
            self.set(cursor, enabled);
            cursor += 1;
        }
        let fill = if enabled { u8::MAX } else { 0 };
        while cursor.saturating_add(8) <= end {
            self.bytes[(cursor / 8) as usize] = fill;
            cursor += 8;
        }
        while cursor < end {
            self.set(cursor, enabled);
            cursor += 1;
        }
        true
    }
}

impl Vm {
    pub fn new() -> Self {
        Self {
            trace_id: NEXT_VM_TRACE_ID.fetch_add(1, Ordering::Relaxed),
            operand_slots: vec![Value::Int(0); OPERAND_STACK_CAPACITY],
            memory: vec![0; INITIAL_MEMORY_SIZE],
            heap_ptr: 0x0020_0000,
            rng_seed: 1,
            global_config: vec![0; user_data::GLOBAL_CONFIG_SIZE],
            global_user_data: vec![0; user_data::GLOBAL_USER_DATA_SIZE],
            ..Self::default()
        }
    }

    pub fn advance_time_ms(&mut self, milliseconds: u64) {
        self.timing.advance(milliseconds);
    }

    pub fn run<A>(
        &mut self,
        program: &BpProgram,
        api: &mut A,
        options: &VmRunOptions,
    ) -> VmRunReport
    where
        A: SysApi + GraphApi + SoundApi,
    {
        self.start(program);
        self.run_loaded(api, options)
    }

    pub fn start(&mut self, program: &BpProgram) {
        self.stack.clear();
        self.operand_slots.fill(Value::Int(0));
        self.operand_slots_synced_len = 0;
        self.programs.clear();
        self.programs.push(program.clone());
        self.program_cache.clear();
        if let Some(name) = program.script_name.clone() {
            self.program_cache.insert(name, 0);
        }
        self.current_program = 0;
        self.pc = 0;
        self.call_stack.clear();
        self.program_free_stack.clear();
        self.mediation_programs.clear();
        self.halted = false;
        self.wait_blocked = false;
        self.wait_input_scope = None;
        self.pending_graph_procedure = None;
        self.pending_program_messages.clear();
        self.pending_root_program_messages.clear();
        self.pending_program_callbacks.clear();
        self.pending_root_program_callbacks.clear();
        self.timing.reset();
    }

    pub fn run_loaded<A>(&mut self, api: &mut A, options: &VmRunOptions) -> VmRunReport
    where
        A: SysApi + GraphApi + SoundApi,
    {
        self.collect_diagnostics = options.collect_diagnostics;
        // `stack` is public for embedders and tests. Reconcile it once at the
        // host slice boundary; instruction dispatch maintains the ring
        // incrementally after this point.
        self.operand_slots_synced_len = 0;
        self.sync_operand_slots();
        let mut steps = 0usize;
        let mut stop_reason = VmStopReason::Completed;
        let trace_stack = std::env::var_os("TRACE_STACK").is_some();
        let trace_stack_vm = std::env::var("TRACE_STACK_VM")
            .ok()
            .and_then(|value| value.parse::<u64>().ok());
        let trace_stack_program = std::env::var("TRACE_STACK_PROGRAM").ok();
        let trace_stack_offset_min = trace_u32_env("TRACE_STACK_OFFSET_MIN");
        let trace_stack_offset_max = trace_u32_env("TRACE_STACK_OFFSET_MAX");
        let mut instruction_profile = debug::InstructionProfile::from_env(self.trace_id);
        let trace_events =
            std::env::var_os("TRACE_VM_EVENTS").is_some() || std::env::var_os("DEBUG").is_some();
        if !std::mem::take(&mut self.suppress_async_pump_once) {
            self.pump_async_programs(api, trace_events);
        }
        let graph_procedure_waiting = self.poll_graph_procedure(api, trace_events);
        let timing_procedure_waiting = self.poll_wait_timing_procedure(api, trace_events);
        if graph_procedure_waiting || timing_procedure_waiting {
            stop_reason = VmStopReason::WaitingForAnimation;
        }
        while steps < options.max_steps
            && !graph_procedure_waiting
            && !timing_procedure_waiting
            && !self.halted
            && self
                .programs
                .get(self.current_program)
                .and_then(|program| program.instructions.get(self.pc))
                .is_some()
        {
            let program_index = self.current_program;
            let inst_ptr =
                &self.programs[program_index].instructions[self.pc] as *const BpInstruction;
            // Dispatch may append to `programs`, but it never mutates or removes
            // an existing program's instruction buffer. The boxed Vec storage
            // containing this instruction therefore remains stable for the
            // duration of this iteration.
            let inst = unsafe { &*inst_ptr };
            let stack_before = self.stack.len();
            let pc_before = self.pc;
            instruction_profile.record(self, program_index, pc_before);
            if options.trace {
                println!(
                    "pc={} off=0x{:08X} {} {:?} stack_top={:?}",
                    self.pc,
                    inst.offset,
                    inst.opcode_name,
                    inst.operands,
                    self.stack.last()
                );
            }
            if trace_events || options.trace || options.fail_on_stub {
                self.push_trace(format!(
                    "program={} pc={} off=0x{:08X} {} {:?}",
                    self.program_name(program_index),
                    self.pc,
                    inst.offset,
                    inst.opcode_name,
                    inst.operands
                ));
            }
            self.trace_mtn_body_dispatch(inst.offset);
            match self.dispatch_program(
                program_index,
                inst,
                api,
                options.fail_on_stub,
                trace_events,
            ) {
                Ok(()) => {
                    if trace_stack {
                        let stack_after = self.stack.len();
                        let delta = stack_after as isize - stack_before as isize;
                        let program_name = self
                            .programs
                            .get(program_index)
                            .and_then(|program| program.script_name.as_deref())
                            .unwrap_or("<unknown>");
                        let program_matches = trace_stack_program
                            .as_deref()
                            .is_none_or(|filter| program_name.contains(filter));
                        let offset_matches = trace_stack_offset_min
                            .is_none_or(|minimum| inst.offset >= minimum)
                            && trace_stack_offset_max.is_none_or(|maximum| inst.offset <= maximum);
                        if trace_stack_vm.is_none_or(|trace_id| trace_id == self.trace_id)
                            && program_matches
                            && offset_matches
                            && (delta != 0 || inst.opcode_name.starts_with("sys"))
                        {
                            eprintln!(
                                "TRACE_STACK vm={} program={} pc={} off=0x{:08X} op={} {:?} stack {} -> {} ({:+}) next_pc={} top={:?}",
                                self.trace_id,
                                program_name,
                                pc_before,
                                inst.offset,
                                inst.opcode_name,
                                inst.operands,
                                stack_before,
                                stack_after,
                                delta,
                                self.pc,
                                self.stack
                                    .iter()
                                    .rev()
                                    .take(4)
                                    .map(value_summary)
                                    .collect::<Vec<_>>()
                            );
                        }
                    }
                    steps += 1;
                    self.trace_msgwnd_loop(inst.offset as u32);
                    // Native graph procedures suspend the calling interpreter at
                    // the syscall boundary. Continuing here lets a later graph
                    // call replace the pending procedure before it is ever polled.
                    if self.pending_graph_procedure.is_some() {
                        stop_reason = VmStopReason::WaitingForAnimation;
                        break;
                    }
                    if self.wait_blocked {
                        stop_reason = VmStopReason::WaitingForAnimation;
                        break;
                    }
                    if std::mem::take(&mut self.yield_requested) || api.take_frame_yield() {
                        stop_reason = VmStopReason::WaitingForAnimation;
                        break;
                    }
                }
                Err(VmError::UnsupportedInstruction(_)) => {
                    stop_reason = VmStopReason::UnknownOpcode;
                    steps += 1;
                    break;
                }
                Err(VmError::UnknownDispatch { .. }) => {
                    stop_reason = VmStopReason::UnknownDispatch;
                    steps += 1;
                    break;
                }
                Err(err) => {
                    self.push_trace(format!("error: {err}"));
                    tracing::error!(
                        vm = self.trace_id,
                        program = self.program_name(program_index),
                        pc = pc_before,
                        offset = format_args!("0x{:08X}", inst.offset),
                        %err,
                        "VM instruction failed"
                    );
                    self.halted = true;
                    stop_reason = VmStopReason::Error;
                    steps += 1;
                    break;
                }
            }
        }
        if steps >= options.max_steps {
            stop_reason = VmStopReason::MaxSteps;
        }
        instruction_profile.report(self, steps);
        VmRunReport {
            steps,
            pc: self.pc,
            offset: self
                .programs
                .get(self.current_program)
                .and_then(|program| program.instructions.get(self.pc))
                .map(|i| i.offset as u32),
            program: self.program_name(self.current_program).to_string(),
            stop_reason,
            calls: options
                .collect_diagnostics
                .then(|| self.calls.clone())
                .unwrap_or_default(),
            stubs: options
                .collect_diagnostics
                .then(|| self.stubs.clone())
                .unwrap_or_default(),
            recent_trace: options
                .collect_diagnostics
                .then(|| self.recent_trace.iter().cloned().collect())
                .unwrap_or_default(),
        }
    }

    pub fn dispatch<A>(&mut self, instruction: &BpInstruction, api: &mut A) -> VmResult<()>
    where
        A: SysApi + GraphApi + SoundApi,
    {
        self.operand_slots_synced_len = 0;
        let old_program = self.current_program;
        let old_pc = self.pc;
        self.programs.push(BpProgram {
            script_name: None,
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![instruction.clone()],
            labels: Default::default(),
            warnings: Vec::new(),
        });
        self.current_program = self.programs.len() - 1;
        self.pc = 0;
        let result = self.dispatch_program(self.current_program, instruction, api, false, false);
        self.programs.pop();
        self.current_program = old_program;
        self.pc = old_pc;
        result
    }

    fn dispatch_program<A>(
        &mut self,
        program_index: usize,
        instruction: &BpInstruction,
        api: &mut A,
        fail_on_stub: bool,
        trace_events: bool,
    ) -> VmResult<()>
    where
        A: SysApi + GraphApi + SoundApi,
    {
        self.sync_operand_slots();
        let code = instruction.opcode.code();
        let mut next_pc = self.pc + 1;
        match instruction.opcode {
            BpOpcode::Known {
                name: "push_byte", ..
            } => {
                self.push_value(Value::Int(read_op_i32(instruction)));
            }
            BpOpcode::Known {
                name: "push_word" | "push_dword",
                ..
            } => {
                self.push_value(Value::Int(read_op_i32(instruction)));
            }
            BpOpcode::Known {
                name: "push_base_offset",
                ..
            } => {
                self.push_value(Value::Ptr(
                    0x1200_0000u32 | self.mem_ptr.saturating_sub(read_op_u32(instruction)),
                ));
            }
            BpOpcode::Known {
                name: "push_string",
                ..
            } => {
                if let Some(BpOperand::String(text)) = instruction.operands.first() {
                    self.push_value(Value::Str(text.clone()));
                } else if let Some(BpOperand::Offset(offset)) = instruction.operands.first() {
                    self.push_value(Value::Ptr(0x1000_0000 | *offset));
                }
            }
            BpOpcode::Known {
                name: "push_offset",
                ..
            } => {
                self.push_value(Value::Func {
                    program_index,
                    offset: read_op_u32(instruction),
                });
            }
            BpOpcode::Known {
                name: "load_base", ..
            } => {
                // sub_473880 pushes the raw frame-memory offset. Only opcode
                // 0x04 turns base-relative offsets into dereferenceable pointers.
                self.push_value(Value::Int(self.mem_ptr as i32));
            }
            BpOpcode::Known {
                name: "store_base", ..
            } => {
                let previous = self.mem_ptr;
                let next = self.pop_int()? as u32;
                if next > previous {
                    let size = (next - previous) as usize;
                    let ptr = 0x1200_0000u32 | previous;
                    let range = self.resolve_write_range(ptr, size)?;
                    self.memory[range].fill(0);
                    self.clear_shadow_values(ptr, size);
                }
                tracing::trace!(
                    target: "vm_frames",
                    program = self.program_name(program_index),
                    pc = self.pc,
                    offset = format_args!("0x{:08X}", instruction.offset),
                    previous = format_args!("0x{previous:08X}"),
                    next = format_args!("0x{next:08X}"),
                    "VM store_base"
                );
                self.mem_ptr = next;
            }
            BpOpcode::Known { name: "load", .. } => {
                let width = read_op_u8(instruction);
                let ptr = self.pop_ptr()?;
                let value = self.read_value(ptr, width)?;
                self.push_value(value);
            }
            BpOpcode::Known { name: "move", .. } => {
                let width = read_op_u8(instruction);
                let value = self.pop_value()?;
                let ptr = self.pop_ptr()?;
                self.write_value(ptr, width, &value)?;
                // sub_473710 writes through the pointer and calls sub_4450D0
                // to return the assigned value on the operand ring.
                self.push_value(value);
            }
            BpOpcode::Known {
                name: "move_arg", ..
            } => {
                let width = read_op_u8(instruction);
                let ptr = self.pop_ptr()?;
                let value = self.pop_value()?;
                tracing::trace!(
                    target: "vm_operands",
                    program = self.program_name(program_index),
                    pc = self.pc,
                    offset = format_args!("0x{:08X}", instruction.offset),
                    width,
                    ptr = format_args!("0x{ptr:08X}"),
                    value = %value_summary(&value),
                    "VM move_arg"
                );
                if std::env::var_os("TRACE_SCRMAIN_DISPATCH").is_some()
                    && instruction.offset == 0x3f4
                    && self.program_name(program_index).contains("scrmain._bp")
                {
                    tracing::warn!(
                        vm = self.trace_id,
                        mem_ptr = format_args!("0x{:08X}", self.mem_ptr),
                        ptr = format_args!("0x{ptr:08X}"),
                        value = %value_summary(&value),
                        stack = self.stack.len(),
                        "TRACE_SCRMAIN_DISPATCH result"
                    );
                }
                self.write_value(ptr, width, &value)?;
            }
            BpOpcode::Known {
                name: "copy_inline",
                ..
            } => {
                let ptr = self.pop_ptr()?;
                let bytes = instruction
                    .operands
                    .first()
                    .and_then(|operand| match operand {
                        BpOperand::Raw(bytes) => Some(bytes.as_slice()),
                        _ => None,
                    })
                    .unwrap_or_default();
                for (offset, byte) in bytes.iter().copied().enumerate() {
                    self.write_int(ptr.wrapping_add(offset as u32), 0, u32::from(byte))?;
                }
            }
            BpOpcode::Known {
                name: "copy_stack", ..
            } => {
                let width = read_op_u8(instruction);
                let count = instruction.raw.get(2).copied().unwrap_or_default() as usize;
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.pop_value()?);
                }
                let dst = self.pop_ptr()?;
                if trace_events && count <= 12 {
                    tracing::info!(
                        pc = self.pc,
                        offset = format_args!("0x{:08X}", instruction.offset),
                        dst = format_args!("0x{dst:08X}"),
                        width,
                        count,
                        values = ?values.iter().rev().map(value_summary).collect::<Vec<_>>(),
                        "VM copy_stack"
                    );
                }
                let stride = 1u32 << width.min(2);
                for (idx, value) in values.into_iter().rev().enumerate() {
                    self.write_value(dst.wrapping_add(stride * idx as u32), width, &value)?;
                }
            }
            BpOpcode::Known { name: "jmp", .. } => {
                let dest = self.pop_int()? as u32;
                next_pc = self.jump_target_index(program_index, dest)?;
            }
            BpOpcode::Known { name: "jc", .. } => {
                let kind = read_op_u8(instruction);
                let dest = self.pop_int()? as u32;
                let value = self.pop_int()?;
                let take = match kind {
                    0 => value != 0,
                    1 => value == 0,
                    2 => value > 0,
                    3 => value >= 0,
                    4 => value <= 0,
                    5 => value < 0,
                    _ => true,
                };
                if trace_vm_branches_enabled() {
                    tracing::debug!(
                        program = self
                            .programs
                            .get(program_index)
                            .and_then(|program| program.script_name.as_deref())
                            .unwrap_or("<anonymous>"),
                        pc = self.pc,
                        offset = format_args!("0x{:08X}", instruction.offset),
                        kind,
                        value,
                        dest = format_args!("0x{dest:08X}"),
                        take,
                        "VM jc"
                    );
                }
                if take {
                    next_pc = self.jump_target_index(program_index, dest)?;
                }
            }
            BpOpcode::Known { name: "call", .. } => match {
                let target = self.pop_value()?;
                if std::env::var_os("TRACE_SCRMAIN_DISPATCH").is_some()
                    && instruction.offset == 0x3f0
                    && self.program_name(program_index).contains("scrmain._bp")
                {
                    tracing::warn!(
                        vm = self.trace_id,
                        mem_ptr = format_args!("0x{:08X}", self.mem_ptr),
                        target = %value_summary(&target),
                        stack = self.stack.len(),
                        "TRACE_SCRMAIN_DISPATCH call"
                    );
                }
                target
            } {
                Value::Func {
                    program_index: dest_program_index,
                    offset,
                } => {
                    if offset == 0 {
                        if trace_events {
                            tracing::info!(
                                pc = self.pc,
                                offset = format_args!("0x{:08X}", instruction.offset),
                                dest_program = dest_program_index,
                                "VM null function call ignored"
                            );
                        }
                        self.pc = next_pc;
                        return Ok(());
                    }
                    if trace_events {
                        tracing::info!(
                            pc = self.pc,
                            offset = format_args!("0x{:08X}", instruction.offset),
                            dest = format_args!("0x{offset:08X}"),
                            dest_program = dest_program_index,
                            stack_top = ?self.stack_summary(8),
                            "VM call"
                        );
                    }
                    tracing::trace!(
                        target: "vm_frames",
                        caller = self.program_name(program_index),
                        caller_pc = self.pc,
                        caller_offset = format_args!("0x{:08X}", instruction.offset),
                        callee = self.program_name(dest_program_index),
                        destination = format_args!("0x{offset:08X}"),
                        mem_ptr = format_args!("0x{:08X}", self.mem_ptr),
                        stack_top = ?self.stack_summary(12),
                        "VM call frame"
                    );
                    self.write_return_addr(program_index, next_pc)?;
                    self.call_stack
                        .push((self.current_program, next_pc, self.stack.len()));
                    self.current_program = dest_program_index;
                    next_pc = self.jump_target_index(dest_program_index, offset)?;
                }
                Value::Program(program) => {
                    let dest_program_index =
                        self.program_index_for_loaded_program((*program).clone());
                    self.program_free_stack.push(dest_program_index);
                    if trace_events {
                        tracing::info!(
                            pc = self.pc,
                            offset = format_args!("0x{:08X}", instruction.offset),
                            dest_program = dest_program_index,
                            stack_top = ?self.stack_summary(8),
                            "VM call program"
                        );
                    }
                    self.write_return_addr(self.current_program, next_pc)?;
                    self.call_stack
                        .push((self.current_program, next_pc, self.stack.len()));
                    self.current_program = dest_program_index;
                    next_pc = self.jump_target_index(self.current_program, 0x10)?;
                }
                value => {
                    let dest = value.as_i32() as u32;
                    if dest == 0 {
                        if trace_events {
                            tracing::info!(
                                pc = self.pc,
                                offset = format_args!("0x{:08X}", instruction.offset),
                                "VM null call ignored"
                            );
                        }
                        self.pc = next_pc;
                        return Ok(());
                    }
                    if trace_events {
                        tracing::info!(
                            pc = self.pc,
                            offset = format_args!("0x{:08X}", instruction.offset),
                            dest = format_args!("0x{dest:08X}"),
                            stack_top = ?self.stack_summary(8),
                            "VM call"
                        );
                    }
                    self.write_return_addr(program_index, next_pc)?;
                    self.call_stack
                        .push((self.current_program, next_pc, self.stack.len()));
                    next_pc = self.jump_target_index(program_index, dest)?;
                }
            },
            BpOpcode::Known { name: "ret", .. } => {
                if self.mem_ptr == 0 {
                    self.halted = true;
                } else if let Some((program_id, fallback_ret, stack_base)) = self.call_stack.pop() {
                    let ret_offset = self.read_return_addr()?;
                    if trace_call_frames_enabled(self.trace_id) {
                        eprintln!(
                            "TRACE_CALL_FRAME vm={} callee={} caller={} stack_base={} stack_return={} delta={:+}",
                            self.trace_id,
                            self.program_name(self.current_program),
                            self.program_name(program_id),
                            stack_base,
                            self.stack.len(),
                            self.stack.len() as isize - stack_base as isize
                        );
                    }
                    if trace_events {
                        tracing::info!(
                            pc = self.pc,
                            offset = format_args!("0x{:08X}", instruction.offset),
                            return_program = program_id,
                            return_offset = format_args!("0x{ret_offset:08X}"),
                            fallback_pc = fallback_ret,
                            stack_top = ?self.stack_summary(8),
                            "VM ret"
                        );
                    }
                    self.current_program = program_id;
                    next_pc = self
                        .jump_target_index(program_id, ret_offset)
                        .unwrap_or(fallback_ret);
                } else {
                    self.halted = true;
                }
            }
            BpOpcode::Known { name, .. } if matches!(name, "eq" | "neq") => {
                let right = self.pop_value()?;
                let left = self.pop_value()?;
                let equal = self.values_equal(&left, &right)?;
                let value = match name {
                    "eq" => equal as i32,
                    "neq" => (!equal) as i32,
                    _ => 0,
                };
                self.push_value(Value::Int(value));
            }
            BpOpcode::Known { name, .. }
                if matches!(
                    name,
                    "add"
                        | "sub"
                        | "mul"
                        | "div"
                        | "mod"
                        | "and"
                        | "or"
                        | "xor"
                        | "leq"
                        | "geq"
                        | "lt"
                        | "gt"
                        | "dnotzero"
                        | "dnotzero2"
                        | "shl"
                        | "shr"
                        | "sar"
                ) =>
            {
                let right = self.pop_int()?;
                let left = self.pop_int()?;
                let value = match name {
                    "add" => left.wrapping_add(right),
                    "sub" => left.wrapping_sub(right),
                    "mul" => left.wrapping_mul(right),
                    "div" => {
                        if right == 0 {
                            -1
                        } else {
                            left / right
                        }
                    }
                    "mod" => {
                        if right == 0 {
                            -1
                        } else {
                            left % right
                        }
                    }
                    "and" => left & right,
                    "or" => left | right,
                    "xor" => left ^ right,
                    "shl" => left.wrapping_shl((right & 0x1f) as u32),
                    "shr" => ((left as u32) >> (right & 0x1f)) as i32,
                    "sar" => left >> (right & 0x1f),
                    "leq" => (left <= right) as i32,
                    "geq" => (left >= right) as i32,
                    "lt" => (left < right) as i32,
                    "gt" => (left > right) as i32,
                    "dnotzero" => ((left != 0) && (right != 0)) as i32,
                    "dnotzero2" => ((left != 0) || (right != 0)) as i32,
                    _ => 0,
                };
                self.push_value(Value::Int(value));
            }
            BpOpcode::Known { name: "not", .. } => {
                let value = self.pop_int()?;
                self.push_value(Value::Int(!value));
            }
            BpOpcode::Known {
                name: "bool_zero", ..
            } => {
                let value = self.pop_int()?;
                self.push_value(Value::Int((value == 0) as i32));
            }
            BpOpcode::Known {
                name: "ternary", ..
            } => {
                let false_value = self.pop_value()?;
                let true_value = self.pop_value()?;
                let condition = self.pop_int()?;
                self.push_value(if condition != 0 {
                    true_value
                } else {
                    false_value
                });
            }
            BpOpcode::Known { name: "muldiv", .. } => {
                let divisor = self.pop_int()?;
                let multiplier = self.pop_int()?;
                let multiplicand = self.pop_int()?;
                let value = if divisor == 0 {
                    -1
                } else {
                    ((multiplicand as i64 * multiplier as i64) / divisor as i64) as i32
                };
                self.push_value(Value::Int(value));
            }
            BpOpcode::Known { name: "atan2", .. } => {
                let y = self.pop_int()?;
                let x = self.pop_int()?;
                let mut degrees = (y as f64).atan2(x as f64).to_degrees();
                if degrees < 0.0 {
                    degrees += 360.0;
                }
                self.push_value(Value::Int((degrees * 65_536.0) as i32));
            }
            BpOpcode::Known {
                name: "vec3_length",
                ..
            } => {
                let z = self.pop_int()? as f64;
                let y = self.pop_int()? as f64;
                let x = self.pop_int()? as f64;
                self.push_value(Value::Int(
                    (x.mul_add(x, y.mul_add(y, z * z)).sqrt()) as i32,
                ));
            }
            BpOpcode::Known {
                name: "sin" | "cos",
                ..
            } => {
                let angle = self.pop_int()? as f64;
                let radians = angle * ((std::f64::consts::PI / 180.0) / 65_536.0);
                let value = if instruction.opcode.name() == "sin" {
                    radians.sin()
                } else {
                    radians.cos()
                };
                self.push_value(Value::Int((value * 65_536.0) as i32));
            }
            BpOpcode::Known {
                name:
                    "qword_add" | "qword_sub" | "qword_mul" | "qword_div" | "qword_mod",
                ..
            } => {
                self.execute_qword_arithmetic(code)?;
            }
            BpOpcode::Known { name: "memcpy", .. } => {
                let size = self.pop_int()?.max(0) as usize;
                let src = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                let src = self.normalize_scenario_descriptor_src(src, size);
                let src_range = self.resolve_range(src, size)?;
                let dst_range = self.resolve_write_range(dst, size)?;
                let tmp = self.memory[src_range].to_vec();
                self.memory[dst_range].copy_from_slice(&tmp);
                self.trace_watch_write(dst, size, src, "memcpy");
                self.copy_shadow_values(src, dst, size);
            }
            BpOpcode::Known { name: "memclr", .. } => {
                let size = self.pop_int()?.max(0) as usize;
                let ptr = self.pop_ptr()?;
                let range = self.resolve_write_range(ptr, size)?;
                self.memory[range].fill(0);
                self.trace_watch_write(ptr, size, 0, "memclr");
                self.clear_shadow_values(ptr, size);
                self.restore_script_records(ptr, size)?;
            }
            BpOpcode::Known { name: "memset", .. } => {
                let value = self.pop_int()? as u8;
                let size = self.pop_int()?.max(0) as usize;
                let ptr = self.pop_ptr()?;
                let range = self.resolve_write_range(ptr, size)?;
                self.memory[range].fill(value);
                self.trace_watch_write(ptr, size, value as u32, "memset");
                self.clear_shadow_values(ptr, size);
                self.restore_script_records(ptr, size)?;
            }
            BpOpcode::Known { name: "memcmp", .. } => {
                let size = self.pop_int()?.max(0) as usize;
                let right = self.pop_value()?;
                let left = self.pop_value()?;
                let equal = self.value_as_fixed_bytes(left, size)?
                    == self.value_as_fixed_bytes(right, size)?;
                self.push_value(Value::Int(i32::from(equal)));
            }
            BpOpcode::Known {
                name: "memrepeat", ..
            } => {
                let src = self.pop_ptr()?;
                let count = self.pop_int()?.max(0) as usize;
                let size = self.pop_int()?.max(0) as usize;
                let dst = self.pop_ptr()?;
                let src_range = self.resolve_range(src, size)?;
                let block = self.memory[src_range].to_vec();
                let total = size.saturating_mul(count);
                let dst_range = self.resolve_write_range(dst, total)?;
                for chunk in self.memory[dst_range]
                    .chunks_exact_mut(size.max(1))
                    .take(count)
                {
                    if size != 0 {
                        chunk.copy_from_slice(&block);
                    }
                }
                self.clear_shadow_values(dst, total);
            }
            BpOpcode::Known {
                name: "memfind", ..
            } => {
                let needle = self.pop_ptr()?;
                let count = self.pop_int()?.max(0) as usize;
                let size = self.pop_int()?.max(0) as usize;
                let haystack = self.pop_ptr()?;
                let needle_range = self.resolve_range(needle, size)?;
                let needle = self.memory[needle_range].to_vec();
                let total = size.saturating_mul(count);
                let haystack_range = self.resolve_range(haystack, total)?;
                let found = if size == 0 || count == 0 {
                    -1
                } else {
                    self.memory[haystack_range]
                        .chunks_exact(size)
                        .position(|block| block == needle)
                        .map(|index| index as i32)
                        .unwrap_or(-1)
                };
                self.push_value(Value::Int(found));
            }
            BpOpcode::Known {
                name: "strfind", ..
            } => {
                let needle = self.pop_string_lossy()?;
                let haystack = self.pop_string_lossy()?;
                let (needle, _, _) = encoding_rs::SHIFT_JIS.encode(&needle);
                let (haystack, _, _) = encoding_rs::SHIFT_JIS.encode(&haystack);
                let found = if needle.is_empty() {
                    0
                } else {
                    haystack
                        .windows(needle.len())
                        .position(|window| window == needle.as_ref())
                        .map(|offset| offset as i32)
                        .unwrap_or(-1)
                };
                self.push_value(Value::Int(found));
            }
            BpOpcode::Known {
                name: "strreplace", ..
            } => {
                let replacement = self.pop_string_lossy()?;
                let needle = self.pop_string_lossy()?;
                let haystack = self.pop_string_lossy()?;
                let dst = self.pop_ptr()?;
                self.write_c_string(dst, &haystack.replace(&needle, &replacement))?;
            }
            BpOpcode::Known { name: "strlen", .. } => {
                let text = self.pop_string_lossy()?;
                let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode(&text);
                self.push_value(Value::Int(encoded.len() as i32));
            }
            BpOpcode::Known { name: "streq", .. } => {
                let right = self.pop_string_lossy()?;
                let left = self.pop_string_lossy()?;
                self.push_value(Value::Int((left == right) as i32));
            }
            BpOpcode::Known { name: "strcpy", .. } => {
                let right = self.pop_value()?;
                let left = self.pop_ptr()?;
                self.copy_c_string_value(left, right)?;
            }
            BpOpcode::Known {
                name: "strconcat", ..
            } => {
                let right = self.pop_string_lossy()?;
                let left = self.pop_string_lossy()?;
                let dst = self.pop_ptr()?;
                self.write_c_string(dst, &format!("{left}{right}"))?;
            }
            BpOpcode::Known {
                name: "getchar", ..
            } => {
                let ptr = self.pop_ptr()?;
                let start = Self::memory_addr(ptr) as usize;
                let first = self.memory.get(start).copied().unwrap_or_default();
                let is_two_byte = matches!(first, 0x81..=0x9f | 0xe0..=0xfc);
                let ch = if is_two_byte {
                    u16::from_be_bytes([
                        first,
                        self.memory.get(start + 1).copied().unwrap_or_default(),
                    ]) as i32
                } else {
                    first as i32
                };
                self.push_value(Value::Int(ch));
                self.push_value(Value::Int(is_two_byte as i32));
                self.push_value(Value::Int(is_sjis_delimiter(ch as u16) as i32));
            }
            BpOpcode::Known {
                name: "tolower", ..
            } => {
                let ptr = self.pop_ptr()?;
                let text = self.read_c_string(ptr)?.to_ascii_lowercase();
                self.write_c_string(ptr, &text)?;
            }
            BpOpcode::Known {
                name: "quote_string",
                ..
            } => {
                self.execute_quote_string()?;
            }
            BpOpcode::Known {
                name: "sprintf", ..
            } => {
                let fmt = self.pop_string_lossy()?;
                let dst = self.pop_ptr()?;
                let rendered = self.render_sprintf(&fmt);
                self.write_c_string(dst, &rendered)?;
            }
            BpOpcode::Known { name: "malloc", .. } => {
                let size = self.pop_int()?.max(0) as u32;
                let ptr = self.alloc_heap(size);
                self.push_value(Value::Ptr(ptr));
            }
            BpOpcode::Known { name: "free", .. } => {
                let ptr = self.pop_ptr()?;
                let freed = self.free_heap(ptr);
                self.push_value(Value::Int(i32::from(freed)));
            }
            BpOpcode::Known {
                name: "set_memory_mode",
                ..
            } => {
                self.execute_set_memory_mode()?;
            }
            BpOpcode::Known {
                name: "addmemboundary",
                ..
            } => {
                let _name = self.pop_string_lossy()?;
                let _size = self.pop_int()?;
                let _start = self.pop_int()?;
                self.push_value(Value::Int(1));
            }
            BpOpcode::Known {
                name: "engine_state",
                ..
            } => {
                let selector = self.pop_int()?;
                // sub_443180 exposes native renderer/debug fields. They have no
                // portable host equivalent; the observed dbgmngr selector 0 is
                // the idle wheel/state counter and starts at zero.
                let value = if matches!(selector, 0..=7 | 16 | 17) {
                    0
                } else {
                    -1
                };
                self.push_value(Value::Int(value));
            }
            BpOpcode::Known {
                name: "confirm", ..
            } => {
                let _message = self.pop_string_lossy().unwrap_or_default();
                self.push_value(Value::Int(1));
            }
            BpOpcode::Known {
                name: "message_box",
                ..
            } => {
                let message = self
                    .pop_string_lossy()
                    .unwrap_or_else(|_| "<message>".into());
                tracing::warn!(%message, "BP message_box");
            }
            BpOpcode::Known { name: "assert", .. } => {
                let value = self.pop_int()?;
                if value == 0 {
                    return Err(VmError::Runtime("BP assert failed".into()));
                }
            }
            BpOpcode::Known {
                name: "dumpmem", ..
            } => {
                let _size = self.pop_int().unwrap_or_default();
                let _ptr = self.pop_ptr().unwrap_or_default();
            }
            BpOpcode::Known {
                name: "modal_list",
                ..
            } => {
                let result = self.execute_modal_list()?;
                self.push_value(Value::Int(result));
            }
            BpOpcode::Known {
                name: "resource_transform",
                ..
            } => {
                self.execute_resource_transform()?;
            }
            BpOpcode::Known {
                name: "clipboard_set",
                ..
            } => {
                let result = self.execute_clipboard_set()?;
                self.push_value(Value::Int(result));
            }
            BpOpcode::Known {
                name: "resource_blend",
                ..
            } => {
                self.execute_resource_blend()?;
            }
            BpOpcode::Known {
                name: "sys1" | "sys2",
                ..
            } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default() as u16;
                self.note_call("sys", code, id);
                api.observe_dispatch(code, id);
                if known_call_name(code, id).is_none() && fail_on_stub {
                    self.note_stub("sys", code, id);
                    return Err(VmError::UnknownDispatch { group: code, id });
                }
                let result = if (code, id) == (0x81, 0x35) {
                    let file = self.pop_string_lossy()?;
                    let archive = self.pop_string_lossy()?;
                    let size = api
                        .load_file_bytes(&archive, &file)
                        .map(|bytes| bytes.len() as i32)
                        .unwrap_or_default();
                    Value::Int(size)
                } else if (code, id) == (0x81, 0x30) {
                    let length = self.pop_int()?.max(0) as usize;
                    let offset = self.pop_int()?.max(0) as usize;
                    let file = self.pop_string_lossy()?;
                    let archive = self.pop_string_lossy()?;
                    let buffer = self.pop_ptr()?;
                    let written =
                        self.write_loaded_file(api, buffer, &archive, &file, offset, Some(length))?;
                    Value::Int(i32::from(written as usize != length))
                } else if (code, id) == (0x80, 0x12) {
                    let input_descriptor_arg = self.pop_value()?;
                    let input_descriptor = match input_descriptor_arg {
                        Value::Ptr(ptr) => self.read_value(ptr, 2)?.as_i32(),
                        value => value.as_i32(),
                    };
                    Value::Int(api.read_input_state(input_descriptor))
                } else if (code, id) == (0x80, 0x0a) {
                    let destination = self.pop_ptr()?;
                    // sub_487EB0 copies the 16-dword D3D capability cache.
                    // Portable backends expose the stable screen-related
                    // prefix and leave unsupported native capability bits off.
                    let capabilities = [0i32; 16];
                    for (index, value) in capabilities.into_iter().enumerate() {
                        self.write_int(
                            destination.wrapping_add(index as u32 * 4),
                            2,
                            value as u32,
                        )?;
                    }
                    Value::None
                } else if (code, id) == (0x80, 0x1d) {
                    let input_class = self.pop_int()?;
                    let _registration_mask = self.pop_int()?;
                    Value::Int(api.query_input_class_state(input_class))
                } else if (code, id) == (0x80, 0x1a) {
                    let scope = self.pop_int()?;
                    Value::Int(api.query_input_class_state(scope))
                } else if (code, id) == (0x80, 0x1c) {
                    let class_mask = self.pop_int()?;
                    Value::Int(api.query_input_descriptor_state(class_mask))
                } else if (code, id) == (0x80, 0x18) {
                    let value = self.pop_int()?;
                    api.set_input_master_gate(value);
                    Value::None
                } else if (code, id) == (0x80, 0x19) {
                    let value = self.pop_int()?;
                    api.set_input_latched_state(value);
                    Value::None
                } else if (code, id) == (0x80, 0x25) {
                    let max_count = self.pop_int()?.max(0) as usize;
                    let recursive = self.pop_int()? != 0;
                    let pattern = self.pop_string_lossy()?;
                    let capacity = self.pop_int()?.max(0) as usize;
                    let destination = self.pop_ptr()?;
                    let files = api.enumerate_user_files(&pattern, recursive, max_count);
                    let required = files
                        .iter()
                        .map(|file| encoding_rs::SHIFT_JIS.encode(file).0.len() + 1)
                        .sum::<usize>();
                    if destination != 0 && required > capacity {
                        Value::Int(-1)
                    } else {
                        let mut cursor = destination;
                        if destination != 0 {
                            self.clear_shadow_values(destination, required);
                            for file in &files {
                                self.write_c_string_raw(cursor, file)?;
                                cursor = cursor.saturating_add(
                                    (encoding_rs::SHIFT_JIS.encode(file).0.len() + 1) as u32,
                                );
                            }
                        }
                        Value::Int(files.len().min(i32::MAX as usize) as i32)
                    }
                } else if (code, id) == (0x80, 0x26) {
                    let max_count = self.pop_int()?.max(0) as usize;
                    let pattern = self.pop_string_lossy()?;
                    let capacity = self.pop_int()?.max(0) as usize;
                    let destination = self.pop_ptr()?;
                    let directories = api.enumerate_user_directories(&pattern, max_count);
                    let required = directories
                        .iter()
                        .map(|directory| encoding_rs::SHIFT_JIS.encode(directory).0.len() + 1)
                        .sum::<usize>();
                    if destination != 0 && required > capacity {
                        Value::Int(-1)
                    } else {
                        let mut cursor = destination;
                        if destination != 0 {
                            self.clear_shadow_values(destination, required);
                            for directory in &directories {
                                self.write_c_string_raw(cursor, directory)?;
                                cursor = cursor.saturating_add(
                                    (encoding_rs::SHIFT_JIS.encode(directory).0.len() + 1) as u32,
                                );
                            }
                        }
                        Value::Int(directories.len().min(i32::MAX as usize) as i32)
                    }
                } else if (code, id) == (0x80, 0x6d) {
                    let destination = self.pop_ptr()?;
                    if let Some(path) = api.take_dropped_file() {
                        self.write_c_string(destination, &path)?;
                        Value::Int(1)
                    } else {
                        Value::Int(0)
                    }
                } else if (code, id) == (0x80, 0x7a) {
                    let destination = self.pop_ptr()?;
                    let slot = self.pop_int()?;
                    if let Some(header) = api.native_save_header(slot) {
                        let range = self.resolve_range(destination, header.len())?;
                        self.memory[range].copy_from_slice(&header);
                        self.clear_shadow_values(destination, header.len());
                        Value::Int(0)
                    } else {
                        Value::Int(1)
                    }
                } else if (code, id) == (0x80, 0xa9) {
                    let destination = self.pop_ptr()?;
                    let object = self.pop_int()?;
                    if let Some(value) = api.registered_object_value(object) {
                        self.write_int(destination, 2, value as u32)?;
                        Value::Int(1)
                    } else {
                        Value::Int(0)
                    }
                } else if (code, id) == (0x80, 0xe9) {
                    let path = self.pop_string_lossy()?;
                    let destination = self.pop_ptr()?;
                    if let Some(bytes) = api.read_user_file_bytes(&path) {
                        let hash = native_file_hash(&bytes);
                        self.write_int(destination, 2, hash[0])?;
                        self.write_int(destination.wrapping_add(4), 2, hash[1])?;
                        Value::Int(1)
                    } else {
                        Value::Int(0)
                    }
                } else if (code, id) == (0x80, 0xfa) {
                    let _name = self.pop_string_lossy()?;
                    let destination = self.pop_ptr()?;
                    if let Some(root) = api.user_data_root(0) {
                        self.write_c_string(destination, root.trim_end_matches(['/', '\\']))?;
                        Value::Int(1)
                    } else {
                        Value::Int(0)
                    }
                } else if (code, id) == (0x80, 0xfb) {
                    let destination = self.pop_ptr()?;
                    let root = api.user_data_root(0).unwrap_or_else(|| ".".into());
                    self.write_c_string(destination, &root)?;
                    Value::None
                } else if (code, id) == (0x80, 0xfc) {
                    let _description = self.pop_string_lossy()?;
                    let _command = self.pop_string_lossy()?;
                    let _extension = self.pop_string_lossy()?;
                    let _class_name = self.pop_string_lossy()?;
                    let _icon = self.pop_string_lossy()?;
                    // File-association registration is a Windows shell
                    // operation. Returning native failure is the portable,
                    // side-effect-free result.
                    Value::Int(0)
                } else if (code, id) == (0x80, 0x80) {
                    let loaded = self.load_global_user_data(api)?;
                    self.push_value(Value::Int(loaded.window_x));
                    self.push_value(Value::Int(loaded.window_y));
                    Value::Int(loaded.status)
                } else if (code, id) == (0x80, 0x81) {
                    Value::Int(i32::from(self.save_global_user_data(api)))
                } else if (code, id) == (0x80, 0x40) {
                    let file = self.pop_string_lossy()?;
                    let archive = self.pop_string_lossy()?;
                    let mut program = api
                        .load_program(&archive, &file)
                        .unwrap_or_else(|| empty_loaded_program(format!("{archive}:{file}")));
                    self.assign_program_instance(&mut program);
                    Value::Program(Arc::new(program))
                } else if (code, id) == (0x80, 0x44) {
                    let mut params = Vec::with_capacity(3);
                    for _ in 0..3 {
                        params.push(self.pop_value()?);
                    }
                    let file = self.pop_string_lossy()?;
                    let archive = self.pop_string_lossy()?;
                    let mut program = api
                        .load_program_ex(&archive, &file, &params)
                        .unwrap_or_else(|| empty_loaded_program(format!("{archive}:{file}")));
                    self.assign_program_instance(&mut program);
                    self.start_async_program_with_args(
                        Value::Program(Arc::new(program.clone())),
                        Vec::new(),
                        trace_events,
                    );
                    Value::Program(Arc::new(program))
                } else if (code, id) == (0x80, 0x41) {
                    let program = if matches!(self.stack.last(), Some(Value::Program(_))) {
                        self.pop_value().unwrap_or(Value::None)
                    } else {
                        self.free_next_called_program(trace_events)
                    };
                    let freed_program = self.free_loaded_program_value(program, trace_events);
                    api.free_program(freed_program);
                    Value::Int(self.call_stack.len() as i32)
                } else if (code, id) == (0x80, 0x46) {
                    // sub_488D80 -> sub_42D560 reads CThread+8. Loaded BP
                    // modules do not change this value; every LoadProgramEx
                    // CThread receives one stable scheduler identifier.
                    Value::Int(self.native_thread_id)
                } else if (code, id) == (0x80, 0x47) {
                    let program = self.pop_value()?;
                    Value::Int(self.async_program_is_active(program))
                } else if (code, id) == (0x80, 0x48) {
                    let message = self.pop_value()?;
                    let program = self.pop_value()?;
                    self.post_async_program_message(program, message, trace_events);
                    Value::None
                } else if (code, id) == (0x80, 0x49) {
                    let message_ptr = self.pop_ptr()?;
                    if let Some(message) = self.pending_program_messages.pop_front() {
                        self.write_value(message_ptr, 2, &message)?;
                        Value::Int(1)
                    } else {
                        Value::Int(0)
                    }
                } else if (code, id) == (0x80, 0x4a) {
                    let messages_ptr = self.pop_value()?;
                    let argc = self.pop_int()?.max(0).min(64) as usize;
                    let program_value = self.pop_value()?;
                    let messages_addr = messages_ptr.as_i32() as u32;
                    if trace_events {
                        tracing::debug!(
                            program = ?value_summary(&program_value),
                            argc,
                            messages_addr = format_args!("0x{messages_addr:08X}"),
                            "VM program message batch"
                        );
                    }
                    for index in 0..argc {
                        let message =
                            self.read_value(messages_addr.saturating_add((index * 4) as u32), 2)?;
                        self.post_async_program_message(
                            program_value.clone(),
                            message,
                            trace_events,
                        );
                    }
                    Value::None
                } else if (code, id) == (0x80, 0x4c) {
                    let arg3 = self.pop_value()?;
                    let arg2 = self.pop_value()?;
                    let arg1 = self.pop_value()?;
                    let program = self.pop_value()?;
                    let active = self.post_async_program_callback(
                        program.clone(),
                        [arg1.clone(), arg2.clone(), arg3.clone()],
                        trace_events,
                    );
                    if trace_events {
                        tracing::debug!(
                            program = ?value_summary(&program),
                            args = ?[value_summary(&arg1), value_summary(&arg2), value_summary(&arg3)],
                            active,
                            "VM program callback invocation"
                        );
                    }
                    Value::Int(i32::from(active))
                } else if (code, id) == (0x80, 0x5e) {
                    let program = self.pop_value()?;
                    // Native handler status 3 asks the interpreter scheduler to
                    // switch to this thread immediately. The portable scheduler
                    // activates it for subsequent cooperative slices and ends the
                    // caller's current slice at the same syscall boundary.
                    self.switch_to_async_program(program, trace_events);
                    self.yield_requested = true;
                    Value::None
                } else if (code, id) == (0x80, 0x5f) {
                    tracing::debug!(
                        target: "vm_yield",
                        tick_ms = self.timing.tick_count(),
                        program = self.program_name(program_index),
                        pc = self.pc,
                        offset = format_args!("0x{:08X}", instruction.offset),
                        stack_top = ?self.stack_summary(8),
                        "cooperative VM yield"
                    );
                    self.yield_requested = true;
                    Value::None
                } else if (code, id) == (0x80, 0x30) {
                    let file = self.pop_string_lossy()?;
                    let archive = self.pop_string_lossy()?;
                    let buffer = self.pop_ptr()?;
                    let written = self.write_loaded_file(api, buffer, &archive, &file, 0, None)?;
                    Value::Int(written)
                } else if (code, id) == (0x80, 0x32) {
                    let length = self.pop_int()?.max(0) as usize;
                    let buffer = self.pop_ptr()?;
                    let path = self.pop_string_lossy()?;
                    let range = self.resolve_range(buffer, length)?;
                    let bytes = self.memory[range].to_vec();
                    let ok = api.write_file_bytes(&path, &bytes);
                    if trace_events {
                        tracing::info!(
                            pc = self.pc,
                            offset = format_args!("0x{:08X}", instruction.offset),
                            path,
                            buffer = format_args!("0x{buffer:08X}"),
                            length,
                            ok,
                            "VM WriteFileBytes"
                        );
                    }
                    Value::Int(i32::from(ok))
                } else if (code, id) == (0x80, 0x34) {
                    let file = self.pop_string_lossy()?;
                    let archive = self.pop_string_lossy()?;
                    Value::Int(i32::from(api.file_exists(&archive, &file)))
                } else if (code, id) == (0x80, 0x35) {
                    let file = self.pop_string_lossy()?;
                    let archive = self.pop_string_lossy()?;
                    Value::Int(api.file_size(&archive, &file))
                } else if (code, id) == (0x80, 0x31) {
                    Value::Int(self.sys_read_profile_string()?)
                } else if (code, id) == (0x80, 0x33) {
                    let file = self.pop_string_lossy()?;
                    let root = self.pop_string_lossy()?;
                    Value::Int(i32::from(api.delete_file(&root, &file)))
                } else if (code, id) == (0x80, 0x3d) {
                    let kind = self.pop_int()?;
                    let ptr = self.pop_ptr()?;
                    let root = api.user_data_root(kind).unwrap_or_else(|| ".".into());
                    self.write_c_string(ptr, &root)?;
                    Value::Int(1)
                } else if (code, id) == (0x80, 0x50) {
                    let enabled = self.pop_int().unwrap_or_default();
                    tracing::info!(enabled, "Sys50SystemWaitState");
                    Value::None
                } else if (code, id) == (0x81, 0x0f) {
                    Value::Int(0)
                } else if (code, id) == (0x81, 0x18) {
                    let _enabled = self.pop_value()?;
                    Value::Int(0)
                } else if (code, id) == (0x80, 0xa0) {
                    let destination = self.pop_ptr()?;
                    if let Some(event) = api.poll_queued_event() {
                        for (index, value) in event.into_iter().enumerate() {
                            self.write_int(
                                destination.wrapping_add(index as u32 * 4),
                                2,
                                value as u32,
                            )?;
                        }
                        Value::Int(1)
                    } else {
                        Value::Int(0)
                    }
                } else if (code, id) == (0x80, 0xa1) {
                    let parameter = self.pop_int()?;
                    let event_code = self.pop_int()?;
                    api.post_queued_event(event_code, parameter);
                    Value::None
                } else if (code, id) == (0x80, 0xac) {
                    let descriptor = self.pop_value()?;
                    let count = self.pop_int().unwrap_or_default();
                    let object = self.pop_value()?.as_i32();
                    let descriptor_values =
                        self.read_descriptor_values(descriptor.clone(), count.max(0) as usize)?;
                    api.dispatch_object_event(object, count, &descriptor_values)?;
                    if trace_events {
                        tracing::info!(
                            pc = self.pc,
                            offset = format_args!("0x{:08X}", instruction.offset),
                            object,
                            count,
                            descriptor = ?descriptor,
                            values = ?descriptor_values.iter().map(value_summary).collect::<Vec<_>>(),
                            "VM DispatchObjectEvent"
                        );
                    }
                    Value::None
                } else if (code, id) == (0x80, 0xdc) {
                    let path = self.pop_string_lossy()?;
                    let namespace = self.pop_int()?;
                    let values = self.string_hash_tables.entry(namespace).or_default();
                    let resource_id = values
                        .iter()
                        .position(|value| value == &path)
                        .map(|index| index as i32)
                        .unwrap_or_else(|| {
                            let index = values.len() as i32;
                            values.push(path.clone());
                            index
                        });
                    let found = api.register_graphic_resource(namespace, resource_id, &path);
                    if trace_events {
                        tracing::info!(
                            namespace,
                            resource_id,
                            path,
                            found,
                            "VM intern graphic resource"
                        );
                    }
                    Value::Int(resource_id)
                } else if let Some(result) = self.try_builtin_sys_with_api(api, code, id)? {
                    if trace_events
                        && matches!(id, 0x47 | 0x48 | 0x9d | 0xac | 0xd0 | 0xd2 | 0xd4 | 0xdd)
                    {
                        tracing::info!(
                            pc = self.pc,
                            offset = format_args!("0x{:08X}", instruction.offset),
                            group = format_args!("0x{code:02X}"),
                            id = format_args!("0x{id:02X}"),
                            result = ?result,
                            stack_top = ?self.stack_summary(8),
                            "VM builtin sys"
                        );
                    }
                    result
                } else {
                    if trace_events {
                        let known = known_call_name(code, id).is_some();
                        if known {
                            tracing::debug!(
                                program = self.program_name(program_index),
                                pc = self.pc,
                                offset = format_args!("0x{:08X}", instruction.offset),
                                group = format_args!("0x{code:02X}"),
                                id = format_args!("0x{id:02X}"),
                                stack_len = self.stack.len(),
                                stack_top = ?self.stack_summary(8),
                                "VM runtime sys fallback"
                            );
                        } else {
                            tracing::warn!(
                                program = self.program_name(program_index),
                                pc = self.pc,
                                offset = format_args!("0x{:08X}", instruction.offset),
                                group = format_args!("0x{code:02X}"),
                                id = format_args!("0x{id:02X}"),
                                stack_len = self.stack.len(),
                                stack_top = ?self.stack_summary(8),
                                "VM runtime sys fallback"
                            );
                        }
                    }
                    self.normalize_sys_string_args(code, id)?;
                    let mut call_stack = self.take_dispatch_call_frame(code, id)?;
                    let result = api.call_sys(code, id, &mut call_stack)?;
                    let result = self.settle_native_call_outputs(code, id, &mut call_stack, result);
                    self.audit_native_args_consumed(
                        "sys",
                        code,
                        id,
                        call_stack.len(),
                        program_index,
                        fail_on_stub,
                    )?;
                    result
                };
                if api.take_runtime_stub() {
                    self.note_stub("sys", code, id);
                    if fail_on_stub {
                        return Err(VmError::UnknownDispatch { group: code, id });
                    }
                }
                self.audit_native_return("sys", code, id, &result, program_index, fail_on_stub)?;
                if result != Value::None {
                    self.push_value(result);
                }
            }
            BpOpcode::Known {
                name: "script_load",
                ..
            } => {
                let file = self.pop_string_lossy()?;
                let archive = self.pop_string_lossy()?;
                let slot = self.pop_int()?;
                self.note_call("script", 0xff, 0xf0);
                if !(0..0xf0).contains(&slot) {
                    return Err(VmError::UnknownDispatch {
                        group: 0xff,
                        id: slot as u16,
                    });
                }
                let mut program = api
                    .load_program(&archive, &file)
                    .unwrap_or_else(|| empty_loaded_program(format!("{archive}:{file}")));
                self.assign_program_instance(&mut program);
                self.mediation_programs.insert(slot as u8, program);
                tracing::info!(slot, archive, file, "BP mediation program registered");
            }
            BpOpcode::Known {
                name: "script_free",
                ..
            } => {
                let slot = self.pop_int()?;
                self.note_call("script", 0xff, 0xf1);
                if let Some(program) = self.mediation_programs.remove(&(slot as u8)) {
                    api.free_program(Value::Program(Arc::new(program)));
                }
            }
            BpOpcode::Known {
                name: "script_ret", ..
            } => {
                if self.mem_ptr == 0 {
                    self.halted = true;
                } else if let Some((program_id, fallback_ret, stack_base)) = self.call_stack.pop() {
                    let ret_offset = self.read_return_addr()?;
                    if trace_call_frames_enabled(self.trace_id) {
                        eprintln!(
                            "TRACE_CALL_FRAME vm={} callee={} caller={} stack_base={} stack_return={} delta={:+}",
                            self.trace_id,
                            self.program_name(self.current_program),
                            self.program_name(program_id),
                            stack_base,
                            self.stack.len(),
                            self.stack.len() as isize - stack_base as isize
                        );
                    }
                    self.current_program = program_id;
                    next_pc = self
                        .jump_target_index(program_id, ret_offset)
                        .unwrap_or(fallback_ret);
                } else {
                    self.halted = true;
                }
            }
            BpOpcode::Known {
                name: "script_call",
                ..
            } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default();
                self.note_call("script", 0xff, id as u16);
                let Some(program) = self.mediation_programs.get(&id).cloned() else {
                    self.note_stub("script", 0xff, id as u16);
                    return Err(VmError::UnknownDispatch {
                        group: 0xff,
                        id: id as u16,
                    });
                };
                let dest_program_index = self.program_index_for_loaded_program(program);
                if std::env::var_os("TRACE_MEDIATION_ARGS").is_some() {
                    tracing::warn!(
                        vm = self.trace_id,
                        caller = self.program_name(self.current_program),
                        caller_pc = self.pc,
                        caller_offset = format_args!("0x{:08X}", instruction.offset),
                        slot = id,
                        callee = self.program_name(dest_program_index),
                        stack = self.stack.len(),
                        stack_top = ?self.stack_summary(12),
                        "TRACE_MEDIATION_ARGS"
                    );
                }
                if trace_events {
                    tracing::info!(
                        slot = id,
                        program = self.program_name(dest_program_index),
                        stack_top = ?self.stack_summary(8),
                        "BP mediation program call"
                    );
                }
                self.write_return_addr(self.current_program, next_pc)?;
                self.call_stack
                    .push((self.current_program, next_pc, self.stack.len()));
                self.current_program = dest_program_index;
                next_pc = self.jump_target_index(dest_program_index, 0x10)?;
            }
            BpOpcode::Known {
                name: "grp1" | "grp2" | "grp3",
                ..
            } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default() as u16;
                self.note_call("graph", code, id);
                api.observe_dispatch(code, id);
                if known_call_name(code, id).is_none() && fail_on_stub {
                    self.note_stub("graph", code, id);
                    return Err(VmError::UnknownDispatch { group: code, id });
                }
                let result = if (code, id) == (0x90, 0xbc) {
                    let object = self.pop_value()?;
                    let state_buffer = self.pop_ptr()?;
                    let state = api.poll_object_state_record(object.as_i32());
                    for (index, value) in state.into_iter().enumerate() {
                        self.write_int(
                            state_buffer.wrapping_add((index * 4) as u32),
                            2,
                            value as u32,
                        )?;
                    }
                    Value::Int(i32::from(object.as_i32() != 0))
                } else if (code, id) == (0x90, 0xbf) {
                    let object = self.pop_value()?;
                    let event_buffer = self.pop_ptr()?;
                    let event = api.poll_object_event_record(object.as_i32());
                    if input::clears_title_pending_callback(event[0], event[1]) {
                        self.write_value(input::TITLE_PENDING_CALLBACK_ADDR, 2, &Value::Int(0))?;
                    }
                    for (index, value) in event.into_iter().enumerate() {
                        self.write_int(
                            event_buffer.wrapping_add((index * 4) as u32),
                            2,
                            value as u32,
                        )?;
                    }
                    Value::Int(i32::from(object.as_i32() != 0))
                } else if (code, id) == (0x90, 0x14) {
                    let pixels = self.pop_ptr()?;
                    let format = self.pop_int()?;
                    let height = self.pop_int()?;
                    let width = self.pop_int()?;
                    let bitmap = self.pop_int()?;
                    let byte_count = (width.max(0) as usize)
                        .checked_mul(height.max(0) as usize)
                        .and_then(|pixels| pixels.checked_mul(3))
                        .ok_or_else(|| VmError::Runtime("bitmap RGB size overflow".into()))?;
                    let range = self.resolve_range(pixels, byte_count)?;
                    let bytes = self.memory[range].to_vec();
                    api.create_bitmap_from_rgb(bitmap, width, height, format, &bytes);
                    Value::None
                } else if (code, id) == (0x90, 0x15) {
                    let bitmap = self.pop_int()?;
                    let capacity = self.pop_int()?.max(0) as usize;
                    let written = self.pop_ptr()?;
                    let destination = self.pop_ptr()?;
                    let bytes = api
                        .read_bitmap_pixels(bitmap, capacity)
                        .filter(|bytes| bytes.len() <= capacity);
                    let count = bytes.as_ref().map(Vec::len).unwrap_or_default();
                    if let Some(bytes) = bytes {
                        let range = self.resolve_write_range(destination, bytes.len())?;
                        self.memory[range].copy_from_slice(&bytes);
                        self.clear_shadow_values(destination, bytes.len());
                    }
                    self.write_int(written, 2, count as u32)?;
                    Value::None
                } else if (code, id) == (0x90, 0x16) {
                    let bitmap = self.pop_value()?.as_i32();
                    let destination = self.pop_ptr()?;
                    let info = api.query_bitmap_info(bitmap);
                    // sub_407F20 writes six DWORDs through hidden ESI. The
                    // public handler then clears field zero and returns found.
                    self.write_int(destination, 2, 0)?;
                    if let Some(info) = info {
                        self.write_int(destination.wrapping_add(4), 2, 0)?;
                        self.write_int(destination.wrapping_add(8), 2, info.width)?;
                        self.write_int(destination.wrapping_add(12), 2, info.height)?;
                        self.write_int(destination.wrapping_add(16), 2, info.format)?;
                        self.write_int(destination.wrapping_add(20), 2, 0)?;
                    }
                    Value::Int(i32::from(info.is_some()))
                } else if (code, id) == (0x91, 0x03) {
                    // sub_480680 -> sub_401ED0 -> sub_439930 writes a sized
                    // binary payload into the native two-key graph cache.
                    let size = self.pop_int()?.max(0) as usize;
                    let source = self.pop_ptr()?;
                    let name = self.pop_string_lossy()?;
                    let namespace = self.pop_string_lossy()?;
                    let range = self.resolve_range(source, size)?;
                    let bytes = self.memory[range].to_vec();
                    Value::Int(if api.cache_graph_blob(&namespace, &name, &bytes) {
                        0
                    } else {
                        -1
                    })
                } else if (code, id) == (0x91, 0xF1) {
                    // sub_485100 pops the process handle, converts the first
                    // script argument to an output pointer, then starts the
                    // media process. sub_44D110 writes its duration in
                    // milliseconds through that pointer on success.
                    let process = self.pop_int()?;
                    let duration = self.pop_ptr()?;
                    let invocation = api.invoke_graph_effect_process(process);
                    if invocation.status == 0 {
                        self.write_int(duration, 2, invocation.duration_ms as u32)?;
                    }
                    Value::Int(invocation.status)
                } else if (code, id) == (0x91, 0xF7) {
                    // sub_4853F0 -> sub_407FB0 writes the process result
                    // through the converted second argument.
                    let process = self.pop_int()?;
                    let destination = self.pop_ptr()?;
                    let result = api.query_graph_effect_result(process);
                    if let Some(result) = result {
                        self.write_int(destination, 2, result as u32)?;
                    }
                    Value::Int(i32::from(result.is_some()))
                } else if (code, id) == (0x92, 0x12) {
                    // funcs_486FEE[0x12] -> sub_4857F0 -> sub_402440.
                    // The native bitmap table has 0x4000 addressable slots;
                    // this writes the two metadata DWORDs at +0x28/+0x2c.
                    let height = self.pop_int()?;
                    let width = self.pop_int()?;
                    let bitmap = self.pop_int()?;
                    Value::Int(i32::from(api.set_bitmap_dimensions(bitmap, width, height)))
                } else if (code, id) == (0x92, 0x16) {
                    // sub_485910 pops the bitmap then the destination pointer,
                    // and sub_402470 writes exactly two DWORDs.
                    let bitmap = self.pop_int()?;
                    let destination = self.pop_ptr()?;
                    let info = api.query_bitmap_info(bitmap);
                    if let Some(info) = info {
                        self.write_int(destination, 2, info.width)?;
                        self.write_int(destination.wrapping_add(4), 2, info.height)?;
                    }
                    Value::Int(i32::from(info.is_some()))
                } else if (code, id) == (0x91, 0x9f) {
                    let source = self.pop_string_lossy()?;
                    let destination = self.pop_ptr()?;
                    self.write_c_string(destination, &strip_native_markup_tags(&source))?;
                    Value::None
                } else if (code, id) == (0x91, 0x3e) {
                    let mut call_stack = self.take_dispatch_call_frame(code, id)?;
                    let result = api.call_graph(code, id, &mut call_stack)?;
                    self.write_int(1072, 2, result.as_i32() as u32)?;
                    Value::None
                } else if (code, id) == (0x91, 0x9b) {
                    self.handle_text_measure_call()?;
                    Value::Int(0)
                } else if (code, id) == (0x91, 0x9e) {
                    // sub_437EE0 extracts every non-empty <l>...</l> payload
                    // into consecutive zero-padded 128-byte records.
                    let source = self.pop_string_lossy()?;
                    let destination = self.pop_ptr()?;
                    let labels = extract_native_labels(&source);
                    for (index, label) in labels.iter().enumerate() {
                        let address = destination.wrapping_add((index * 128) as u32);
                        let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode(label);
                        let size = encoded.len().min(95);
                        let range = self.resolve_write_range(address, 128)?;
                        self.memory[range.clone()].fill(0);
                        self.memory[range.start..range.start + size]
                            .copy_from_slice(&encoded[..size]);
                        self.clear_shadow_values(address, 128);
                    }
                    Value::Int(labels.len() as i32)
                } else if (code, id) == (0x91, 0x95) {
                    let source = self.pop_string_lossy()?;
                    let dest = self.pop_ptr()?;
                    let (records, count) = api.collect_ruby_substitutions(&source);
                    self.write_c_string(dest, &records)?;
                    if trace_events && (!source.is_empty() || count != 0) {
                        tracing::debug!(
                            source,
                            dest,
                            count,
                            records,
                            "GraphCollectRubySubstitutions"
                        );
                    }
                    Value::Int(count)
                } else if (code, id) == (0x90, 0xb6) {
                    let descriptor_ptr = self.pop_ptr()?;
                    let object = self.pop_int()?;
                    let descriptor = self.read_compact_graph_input_descriptor(descriptor_ptr)?;
                    api.configure_graph_surface_controls(object, descriptor);
                    Value::Int(0)
                } else if (code, id) == (0x90, 0xba) {
                    let descriptor_ptr = self.pop_ptr()?;
                    let object = self.pop_int()?;
                    let descriptor = self.read_compact_graph_input_descriptor(descriptor_ptr)?;
                    api.configure_graph_input_object(object, descriptor);
                    Value::Int(0)
                } else if (code, id) == (0x91, 0xba) {
                    let descriptor_ptr = self.pop_ptr()?;
                    let object = self.pop_int()?;
                    let descriptor = self.read_graph_input_descriptor(descriptor_ptr)?;
                    api.configure_graph_input_object(object, descriptor);
                    Value::Int(0)
                } else if (code, id) == (0x92, 0x9e) {
                    // sub_437EB0 copies an optional native 128-byte record
                    // table. The portable renderer has no process-global
                    // table, but it must still clear the caller's first slot.
                    let destination = self.pop_ptr()?;
                    let range = self.resolve_write_range(destination, 128)?;
                    self.memory[range].fill(0);
                    self.clear_shadow_values(destination, 128);
                    Value::Int(0)
                } else if (code, id) == (0xa0, 0x86) {
                    // sub_48D910 writes the MCI mode through the converted
                    // destination pointer and returns whether the query ran.
                    let destination = self.pop_ptr()?;
                    self.write_int(destination, 2, u32::MAX)?;
                    Value::Int(0)
                } else if (code, id) == (0x90, 0xb7) {
                    let descriptor_ptr = self.pop_ptr()?;
                    let surface = self.pop_int()?;
                    let descriptor = self.read_graph_input_descriptor(descriptor_ptr)?;
                    api.configure_graph_surface_controls(surface, descriptor);
                    Value::Int(0)
                } else if (code, id) == (0x90, 0xbe) {
                    let dest = self.pop_ptr()?;
                    let object = self.pop_value()?;
                    let mut call_stack = vec![object, Value::Ptr(dest)];
                    let value = api.call_graph(code, id, &mut call_stack)?.as_i32();
                    self.write_int(dest, 2, value as u32)?;
                    Value::None
                } else {
                    self.normalize_graph_string_args(code, id)?;
                    let mut call_stack = self.take_dispatch_call_frame(code, id)?;
                    let result = if (code, id) == (0x90, 0x29) {
                        let points = self.read_spline_control_points(&call_stack)?;
                        let result = api.call_graph_spline_control(&call_stack, &points)?;
                        call_stack.clear();
                        result
                    } else {
                        api.call_graph(code, id, &mut call_stack)?
                    };
                    let result = self.settle_native_call_outputs(code, id, &mut call_stack, result);
                    self.audit_native_args_consumed(
                        "graph",
                        code,
                        id,
                        call_stack.len(),
                        program_index,
                        fail_on_stub,
                    )?;
                    if let Some(schedule) = api.take_graph_procedure_schedule() {
                        self.pending_graph_procedure = Some(PendingGraphProcedure {
                            started_ms: self.timing.tick_count(),
                            duration_ms: schedule.duration_ms.max(1),
                            input_enabled: schedule.input_enabled,
                            input_descriptor: schedule.input_descriptor,
                            wait_for_input: schedule.wait_for_input,
                            completion: schedule.completion,
                        });
                        if trace_events {
                            tracing::info!(
                                duration_ms = schedule.duration_ms.max(1),
                                input_enabled = schedule.input_enabled,
                                input_descriptor = schedule.input_descriptor,
                                "VM graph procedure scheduled"
                            );
                        }
                    }
                    if (code, id) == (0x92, 0x14) {
                        let pending = self.read_int(1644, 2)?.saturating_sub(1);
                        self.write_int(1644, 2, pending)?;
                        if trace_events {
                            tracing::info!(pending, "VM graph preload completed");
                        }
                    }
                    result
                };
                if api.take_runtime_stub() {
                    self.note_stub("graph", code, id);
                    if fail_on_stub {
                        return Err(VmError::UnknownDispatch { group: code, id });
                    }
                }
                self.audit_native_return("graph", code, id, &result, program_index, fail_on_stub)?;
                if result != Value::None {
                    self.push_value(result);
                }
            }
            BpOpcode::Known { name: "snd1", .. } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default() as u16;
                self.note_call("snd", code, id);
                api.observe_dispatch(code, id);
                if trace_sound_calls_enabled() {
                    tracing::info!(
                        vm = self.trace_id,
                        program = self.program_name(program_index),
                        offset = format_args!("0x{:08X}", instruction.offset),
                        group = format_args!("0x{code:02X}"),
                        id = format_args!("0x{id:02X}"),
                        stack_top = ?self.stack_summary(8),
                        "VM sound callsite"
                    );
                }
                if known_call_name(code, id).is_none() && fail_on_stub {
                    self.note_stub("snd", code, id);
                    return Err(VmError::UnknownDispatch { group: code, id });
                }
                self.normalize_sound_string_args(code, id)?;
                let mut call_stack = self.take_dispatch_call_frame(code, id)?;
                let result = api.call_sound(code, id, &mut call_stack)?;
                self.audit_native_args_consumed(
                    "sound",
                    code,
                    id,
                    call_stack.len(),
                    program_index,
                    fail_on_stub,
                )?;
                if api.take_runtime_stub() {
                    self.note_stub("sound", code, id);
                    if fail_on_stub {
                        return Err(VmError::UnknownDispatch { group: code, id });
                    }
                }
                self.audit_native_return("sound", code, id, &result, program_index, fail_on_stub)?;
                if result != Value::None {
                    self.push_value(result);
                }
            }
            BpOpcode::Known {
                name: "usr1" | "usr2",
                ..
            } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default() as u16;
                self.note_call("user", code, id);
                api.observe_dispatch(code, id);
                if known_call_name(code, id).is_none() && fail_on_stub {
                    self.note_stub("user", code, id);
                    return Err(VmError::UnknownDispatch { group: code, id });
                }
                self.normalize_user_string_args(code, id)?;
                let direct_result = if (code, id) == (0xb0, 0x27) {
                    let destination = self.pop_ptr()?;
                    let text = api.current_user_text();
                    self.write_c_string(destination, &text)?;
                    let length = encoding_rs::SHIFT_JIS.encode(&text).0.len() as i32;
                    Some(Value::Int(length))
                } else if (code, id) == (0xb0, 0xa3) {
                    let _printer = self.pop_int()?;
                    let destination = self.pop_ptr()?;
                    self.write_int(destination, 2, 0)?;
                    self.write_int(destination.wrapping_add(4), 2, 0)?;
                    Some(Value::Int(0))
                } else if (code, id) == (0xc0, 0xc2) {
                    let count = self.pop_int()?.max(0) as usize;
                    let source = self.pop_ptr()?;
                    let mode = self.pop_int()?;
                    let handle = self.pop_int()?;
                    let mut points = Vec::with_capacity(count);
                    for index in 0..count {
                        let address = source.wrapping_add((index * 16) as u32);
                        points.push([
                            self.read_int(address, 2)? as i32,
                            self.read_int(address.wrapping_add(4), 2)? as i32,
                            self.read_int(address.wrapping_add(8), 2)? as i32,
                        ]);
                    }
                    Some(Value::Int(
                        api.configure_user_polygon(handle, mode, &points),
                    ))
                } else if (code, id) == (0xc0, 0xc3) {
                    let index = self.pop_int()?;
                    let handle = self.pop_int()?;
                    let destination = self.pop_ptr()?;
                    let point = api.query_user_polygon(handle, index);
                    if let Some(point) = point {
                        for (slot, value) in point.into_iter().enumerate() {
                            self.write_int(
                                destination.wrapping_add((slot * 4) as u32),
                                2,
                                value as u32,
                            )?;
                        }
                    }
                    Some(Value::Int(if point.is_some() { 0 } else { 4 }))
                } else {
                    None
                };
                let mut call_stack = if direct_result.is_some() {
                    Vec::new()
                } else {
                    self.take_dispatch_call_frame(code, id)?
                };
                api.observe_user(code, id, &call_stack);
                let host_result = if let Some(result) = direct_result {
                    Some(result)
                } else {
                    api.call_user(code, id, &mut call_stack)?
                };
                if let Some(result) = host_result {
                    let result = self.settle_native_call_outputs(code, id, &mut call_stack, result);
                    self.audit_native_args_consumed(
                        "user",
                        code,
                        id,
                        call_stack.len(),
                        program_index,
                        fail_on_stub,
                    )?;
                    self.audit_native_return(
                        "user",
                        code,
                        id,
                        &result,
                        program_index,
                        fail_on_stub,
                    )?;
                    if result != Value::None {
                        self.push_value(result);
                    }
                } else {
                    let caller_stack = std::mem::take(&mut self.stack);
                    let caller_synced_len = self.operand_slots_synced_len;
                    let caller_slots = std::mem::replace(
                        &mut self.operand_slots,
                        vec![Value::Int(0); OPERAND_STACK_CAPACITY],
                    );
                    self.stack = call_stack;
                    self.operand_slots_synced_len = 0;
                    self.sync_operand_slots();
                    let builtin_result = self.try_builtin_user(code, id);
                    let mut local_stack = std::mem::replace(&mut self.stack, caller_stack);
                    self.operand_slots = caller_slots;
                    self.operand_slots_synced_len = caller_synced_len;
                    builtin_result?;
                    if known_call_returns_value(code, id) {
                        let result = local_stack
                            .pop()
                            .filter(|value| *value != Value::None)
                            .unwrap_or(Value::None);
                        self.audit_native_return(
                            "user",
                            code,
                            id,
                            &result,
                            program_index,
                            fail_on_stub,
                        )?;
                        if result != Value::None {
                            self.push_value(result);
                        }
                    }
                }
            }
            BpOpcode::Known {
                name: "legacy_3d",
                ..
            } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default();
                self.note_call("legacy3d", code, u16::from(id));
                if let Some(result) = self.execute_legacy_3d(id)? {
                    self.push_value(result);
                }
            }
            BpOpcode::Known {
                name: "debug_inspect",
                ..
            } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default();
                self.note_call("debug", code, u16::from(id));
                self.execute_debug_inspect(id)?;
            }
            BpOpcode::Unknown(code) => {
                self.note_stub("opcode", code, 0);
                if fail_on_stub {
                    return Err(VmError::UnsupportedInstruction(code));
                }
            }
            BpOpcode::Known { code, .. } => {
                self.note_stub("opcode", code, 0);
                if fail_on_stub {
                    return Err(VmError::UnsupportedInstruction(code));
                }
            }
        }
        self.trim_operand_stack();
        self.pc = next_pc;
        Ok(())
    }

    fn trim_operand_stack(&mut self) {
        if self.operand_slots.len() != OPERAND_STACK_CAPACITY {
            self.operand_slots = vec![Value::Int(0); OPERAND_STACK_CAPACITY];
            self.operand_slots_synced_len = 0;
        }
        if self.stack.len() >= OPERAND_STACK_CAPACITY {
            let values = std::mem::take(&mut self.stack);
            self.operand_slots_synced_len = 0;
            for value in values {
                self.push_value(value);
            }
        } else {
            self.sync_operand_slots();
        }
    }

    fn poll_graph_procedure<A>(&mut self, api: &mut A, trace_events: bool) -> bool
    where
        A: SysApi + GraphApi + SoundApi,
    {
        let Some(procedure) = self.pending_graph_procedure else {
            return false;
        };
        let callback_interrupted = self.pending_program_callbacks.drain(..).any(|callback| {
            let code = callback[0].as_i32();
            match procedure.completion {
                GraphProcedureCompletion::ControlProgress => {
                    code == 1 && (callback[1].as_i32() != 0 || procedure.input_enabled)
                }
                GraphProcedureCompletion::MessageInterrupted => matches!(code, 1 | 258),
                GraphProcedureCompletion::None => false,
            }
        });
        let input = if procedure.input_enabled {
            api.read_input_state(procedure.input_descriptor)
        } else {
            0
        };
        let elapsed_ms = self
            .timing
            .tick_count()
            .saturating_sub(procedure.started_ms)
            .max(0);
        if !callback_interrupted
            && input == 0
            && (procedure.wait_for_input || elapsed_ms < procedure.duration_ms)
        {
            return true;
        }

        let progress = if procedure.duration_ms <= 0 {
            1000
        } else {
            elapsed_ms
                .saturating_mul(1000)
                .checked_div(procedure.duration_ms)
                .unwrap_or(1000)
                .clamp(0, 1000)
        };
        let completion_reason = if input == 0 && !callback_interrupted {
            -1
        } else {
            1
        };
        if procedure.completion == GraphProcedureCompletion::ControlProgress {
            self.push_value(Value::Int(progress));
            self.push_value(Value::Int(completion_reason));
        } else if procedure.completion == GraphProcedureCompletion::MessageInterrupted {
            self.push_value(Value::Int(i32::from(input != 0)));
        }
        self.pending_graph_procedure = None;
        if trace_events {
            tracing::info!(
                progress,
                completion_reason,
                elapsed_ms,
                "VM graph procedure completed"
            );
        }
        false
    }

    fn poll_wait_timing_procedure<A>(&mut self, api: &mut A, trace_events: bool) -> bool
    where
        A: SysApi,
    {
        if !self.wait_blocked {
            return false;
        }

        let callback_interrupted = self
            .pending_program_callbacks
            .drain(..)
            .any(|callback| callback[0].as_i32() == 1);
        let interrupted = callback_interrupted
            || self
                .wait_input_scope
                .is_some_and(|scope| api.query_input_class_state(scope) != 0);
        if !interrupted && self.timing.remaining_wait_ms() > 0 {
            return true;
        }

        self.wait_blocked = false;
        self.wait_input_scope = None;
        self.push_value(Value::Int(i32::from(interrupted)));
        if trace_events {
            tracing::info!(interrupted, "VM WaitTimingEx procedure completed");
        }
        false
    }

    fn take_dispatch_call_frame(&mut self, group: u8, id: u16) -> VmResult<Vec<Value>> {
        let Some(argc) = known_call_arg_count(group, id) else {
            return Ok(Vec::new());
        };
        let mut args = Vec::with_capacity(argc);
        for _ in 0..argc {
            args.push(self.pop_value()?);
        }
        args.reverse();
        Ok(args)
    }

    fn settle_native_call_outputs(
        &mut self,
        group: u8,
        id: u16,
        call_stack: &mut Vec<Value>,
        result: Value,
    ) -> Value {
        let output_count = known_call_stack_output_count(group, id);
        if output_count <= 1 {
            return result;
        }

        let stack_output_count = output_count - usize::from(result != Value::None);
        if call_stack.len() < stack_output_count {
            return result;
        }
        let mut outputs = call_stack.split_off(call_stack.len() - stack_output_count);
        if result != Value::None {
            for value in outputs {
                self.push_value(value);
            }
            return result;
        }

        let result = outputs.pop().unwrap_or(Value::None);
        for value in outputs {
            self.push_value(value);
        }
        result
    }

    fn read_spline_control_points(&self, args: &[Value]) -> VmResult<Vec<[i32; 4]>> {
        let count = args
            .get(1)
            .map(Value::as_i32)
            .unwrap_or_default()
            .clamp(0, 256) as usize;
        let ptr = args.get(2).map(Value::as_i32).unwrap_or_default() as u32;
        let mut points = Vec::with_capacity(count);
        for point_index in 0..count {
            let base = ptr.saturating_add((point_index * 16) as u32);
            points.push([
                self.read_int(base, 2)? as i32,
                self.read_int(base.saturating_add(4), 2)? as i32,
                self.read_int(base.saturating_add(8), 2)? as i32,
                self.read_int(base.saturating_add(12), 2)? as i32,
            ]);
        }
        Ok(points)
    }

    fn audit_native_return(
        &mut self,
        kind: &str,
        group: u8,
        id: u16,
        result: &Value,
        program_index: usize,
        fail_on_stub: bool,
    ) -> VmResult<()> {
        if !native_return_audit_enabled()
            || !known_call_returns_value(group, id)
            || *result != Value::None
        {
            return Ok(());
        }

        let key = format!("native-return:{kind}:0x{group:02X}:0x{id:02X}");
        let first_hit = !self.stubs.contains_key(&key);
        *self.stubs.entry(key).or_default() += 1;
        if first_hit {
            tracing::warn!(
                program = self.program_name(program_index),
                pc = self.pc,
                group = format_args!("0x{group:02X}"),
                id = format_args!("0x{id:02X}"),
                kind,
                call = known_call_name(group, id).unwrap_or("unknown"),
                "native ABI return value omitted"
            );
        }
        if fail_on_stub {
            return Err(VmError::UnknownDispatch { group, id });
        }
        Ok(())
    }

    fn audit_native_args_consumed(
        &mut self,
        kind: &str,
        group: u8,
        id: u16,
        remaining: usize,
        program_index: usize,
        fail_on_stub: bool,
    ) -> VmResult<()> {
        if remaining == 0 {
            return Ok(());
        }
        let key = format!("native-args:{kind}:0x{group:02X}:0x{id:02X}");
        let first_hit = !self.stubs.contains_key(&key);
        *self.stubs.entry(key).or_default() += 1;
        if first_hit {
            tracing::warn!(
                program = self.program_name(program_index),
                pc = self.pc,
                group = format_args!("0x{group:02X}"),
                id = format_args!("0x{id:02X}"),
                kind,
                call = known_call_name(group, id).unwrap_or("unknown"),
                remaining,
                "native ABI arguments left unconsumed"
            );
        }
        if fail_on_stub {
            return Err(VmError::UnknownDispatch { group, id });
        }
        Ok(())
    }

    fn assign_program_instance(&mut self, program: &mut BpProgram) {
        self.next_program_instance_id = self.next_program_instance_id.wrapping_add(1).max(1);
        let name = program
            .script_name
            .get_or_insert_with(|| "<anonymous>".to_string());
        name.push_str(&format!("#instance={}", self.next_program_instance_id));
    }

    fn program_index_for_loaded_program(&mut self, program: BpProgram) -> usize {
        if let Some(name) = program.script_name.as_ref() {
            if let Some(index) = self.program_cache.get(name).copied() {
                return index;
            }
        }
        let index = self.programs.len();
        if let Some(name) = program.script_name.clone() {
            self.program_cache.insert(name, index);
        }
        self.programs.push(program);
        index
    }

    fn free_next_called_program(&mut self, trace_events: bool) -> Value {
        while let Some(index) = self.program_free_stack.pop() {
            let Some(program) = self.programs.get(index) else {
                continue;
            };
            if trace_events {
                tracing::info!(
                    program_index = index,
                    program = self.program_name(index),
                    "VM FreeProgram resolved last called program"
                );
            }
            return Value::Program(Arc::new(program.clone()));
        }
        Value::None
    }

    fn free_loaded_program_value(&mut self, program: Value, trace_events: bool) -> Value {
        let Some(index) = self.program_index_for_program_value(&program) else {
            if trace_events {
                tracing::info!(
                    program = ?value_summary(&program),
                    "VM FreeProgram ignored non-loaded program"
                );
            }
            return program;
        };
        self.free_loaded_program_index(index, trace_events)
    }

    fn free_loaded_program_index(&mut self, index: usize, trace_events: bool) -> Value {
        let Some(program) = self.programs.get(index).cloned() else {
            return Value::None;
        };
        if index == 0 {
            if trace_events {
                tracing::warn!(
                    program = self.program_name(index),
                    "VM FreeProgram ignored root program"
                );
            }
            return Value::Program(Arc::new(program));
        }

        let name = program
            .script_name
            .clone()
            .unwrap_or_else(|| format!("<program#{index}>"));
        if let Some(script_name) = program.script_name.as_ref() {
            if self.program_cache.get(script_name).copied() == Some(index) {
                self.program_cache.remove(script_name);
            }
        }
        if trace_events {
            tracing::info!(
                program_index = index,
                program = name,
                "VM FreeProgram released handle"
            );
        }
        Value::Program(Arc::new(program))
    }

    fn program_index_for_program_value(&self, value: &Value) -> Option<usize> {
        let program = match value {
            Value::Program(program) => Some(program.as_ref()),
            Value::Ptr(ptr) => self
                .mem_values
                .get(&Self::value_key(*ptr))
                .and_then(|value| match value {
                    Value::Program(program) => Some(program.as_ref()),
                    _ => None,
                }),
            Value::Int(ptr) => {
                self.mem_values
                    .get(&Self::value_key(*ptr as u32))
                    .and_then(|value| match value {
                        Value::Program(program) => Some(program.as_ref()),
                        _ => None,
                    })
            }
            Value::Str(_) | Value::Func { .. } | Value::None => None,
        }?;

        if let Some(name) = program.script_name.as_ref() {
            if let Some(index) = self.program_cache.get(name).copied() {
                if self
                    .programs
                    .get(index)
                    .and_then(|loaded| loaded.script_name.as_ref())
                    == Some(name)
                {
                    return Some(index);
                }
            }
        }
        self.programs
            .iter()
            .enumerate()
            .find_map(|(index, loaded)| (loaded == program).then_some(index))
    }

    fn push_value(&mut self, value: Value) {
        if self.operand_slots.len() != OPERAND_STACK_CAPACITY {
            self.operand_slots = vec![Value::Int(0); OPERAND_STACK_CAPACITY];
            self.operand_slots_synced_len = 0;
        }
        let sp = self.stack.len();
        self.operand_slots[sp] = value.clone();
        if sp + 1 == OPERAND_STACK_CAPACITY {
            self.stack.clear();
            self.operand_slots_synced_len = 0;
        } else {
            self.stack.push(value);
            self.operand_slots_synced_len = self.stack.len();
        }
    }

    fn pop_value(&mut self) -> VmResult<Value> {
        if let Some(value) = self.stack.pop() {
            self.operand_slots_synced_len = self.operand_slots_synced_len.min(self.stack.len());
            return Ok(value);
        }
        if self.operand_slots.len() != OPERAND_STACK_CAPACITY {
            self.operand_slots = vec![Value::Int(0); OPERAND_STACK_CAPACITY];
            self.operand_slots_synced_len = 0;
        }
        let value = self.operand_slots[OPERAND_STACK_CAPACITY - 1].clone();
        self.stack
            .extend_from_slice(&self.operand_slots[..OPERAND_STACK_CAPACITY - 1]);
        self.operand_slots_synced_len = self.stack.len();
        Ok(value)
    }

    fn sync_operand_slots(&mut self) {
        if self.operand_slots.len() != OPERAND_STACK_CAPACITY {
            self.operand_slots = vec![Value::Int(0); OPERAND_STACK_CAPACITY];
            self.operand_slots_synced_len = 0;
        }
        if self.stack.len() >= OPERAND_STACK_CAPACITY {
            self.trim_operand_stack();
            return;
        }
        let start = self.operand_slots_synced_len.min(self.stack.len());
        for index in start..self.stack.len() {
            self.operand_slots[index] = self.stack[index].clone();
        }
        self.operand_slots_synced_len = self.stack.len();
    }

    fn replace_stack_value(&mut self, index: usize, value: Value) {
        self.stack[index] = value.clone();
        if index < OPERAND_STACK_CAPACITY {
            self.operand_slots[index] = value;
            self.operand_slots_synced_len = self.operand_slots_synced_len.max(index + 1);
        }
    }

    fn pop_int(&mut self) -> VmResult<i32> {
        Ok(self.pop_value()?.as_i32())
    }

    fn pop_ptr(&mut self) -> VmResult<u32> {
        Ok(Self::translate_system_descriptor(self.pop_int()? as u32))
    }

    fn resolve_range(&self, ptr: u32, size: usize) -> VmResult<std::ops::Range<usize>> {
        let addr = Self::memory_addr(ptr);
        let start = addr as usize;
        let end = start
            .checked_add(size)
            .ok_or(VmError::MemoryOutOfBounds { addr, size })?;
        if end > self.memory.len() {
            return Err(VmError::MemoryOutOfBounds { addr, size });
        }
        Ok(start..end)
    }

    pub(crate) fn resolve_write_range(
        &mut self,
        ptr: u32,
        size: usize,
    ) -> VmResult<std::ops::Range<usize>> {
        let addr = Self::memory_addr(ptr);
        let start = addr as usize;
        let end = start
            .checked_add(size)
            .ok_or(VmError::MemoryOutOfBounds { addr, size })?;
        if end > MAX_MEMORY_SIZE {
            return Err(VmError::MemoryOutOfBounds { addr, size });
        }
        if end > self.memory.len() {
            let grown = end
                .next_power_of_two()
                .max(INITIAL_MEMORY_SIZE)
                .min(MAX_MEMORY_SIZE);
            self.memory.resize(grown, 0);
        }
        if start >= HEAP_MEMORY_BASE && size != 0 {
            self.mark_shared_heap_dirty(start..end);
        }
        Ok(start..end)
    }

    fn translate_system_descriptor(ptr: u32) -> u32 {
        let slot = ptr.wrapping_sub(SYSTEM_PROGRAM_DESCRIPTOR_BASE);
        if slot < SYSTEM_PROGRAM_SLOTS as u32 {
            SYSTEM_PROGRAM_TABLE + slot * SYSTEM_PROGRAM_STRIDE
        } else {
            ptr
        }
    }

    fn read_int(&self, ptr: u32, width: u8) -> VmResult<u32> {
        if let Some(value) = self.scenario_guarded_read_int(ptr, width) {
            let size = 1usize << width.min(2);
            self.trace_watch_read(
                ptr,
                size,
                &Value::Int(value as i32),
                "scenario_guarded_read_int",
            );
            return Ok(value);
        }
        let size = 1usize << width.min(2);
        let range = self.resolve_range(ptr, size)?;
        let bytes = &self.memory[range];
        let value = match width {
            0 => bytes[0] as u32,
            1 => u16::from_le_bytes([bytes[0], bytes[1]]) as u32,
            _ => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        };
        self.trace_watch_read(ptr, size, &Value::Int(value as i32), "read_int");
        Ok(value)
    }

    fn write_int(&mut self, ptr: u32, width: u8, value: u32) -> VmResult<()> {
        let size = 1usize << width.min(2);
        let range = self.resolve_write_range(ptr, size)?;
        let bytes = value.to_le_bytes();
        self.memory[range].copy_from_slice(&bytes[..size]);
        // Raw native writes replace the complete value at this address. A
        // stale Ptr/Str/Func shadow must never survive and override the bytes
        // on a later load2. write_value re-adds a typed shadow after this call
        // when the assigned value itself is tagged.
        self.clear_shadow_values(ptr, size);
        self.trace_watch_write(ptr, size, value, "write_int");
        Ok(())
    }

    fn write_return_addr(&mut self, program_index: usize, next_pc: usize) -> VmResult<()> {
        let return_offset = self
            .programs
            .get(program_index)
            .and_then(|program| program.instructions.get(next_pc))
            .map(|instruction| instruction.offset as u32)
            .unwrap_or_default();
        self.write_int(0x1200_0000 | self.mem_ptr, 2, return_offset)?;
        self.mem_ptr = self.mem_ptr.saturating_add(4);
        Ok(())
    }

    fn read_return_addr(&mut self) -> VmResult<u32> {
        self.mem_ptr = self.mem_ptr.saturating_sub(4);
        self.read_int(0x1200_0000 | self.mem_ptr, 2)
    }

    fn read_value(&self, ptr: u32, width: u8) -> VmResult<Value> {
        let addr = Self::value_key(ptr);
        if width >= 2 {
            if let Some(value) = self.mem_values.get(&addr) {
                let size = 1usize << width.min(2);
                self.trace_watch_read(ptr, size, value, "read_value_tagged");
                return Ok(value.clone());
            }
        }
        Ok(Value::Int(self.read_int(ptr, width)? as i32))
    }

    fn read_descriptor_values(&self, descriptor: Value, count: usize) -> VmResult<Vec<Value>> {
        let ptr = match descriptor {
            Value::Ptr(ptr) => ptr,
            Value::Int(value) if value != 0 => value as u32,
            _ => return Ok(Vec::new()),
        };
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            values.push(self.read_value(ptr.saturating_add((index * 4) as u32), 2)?);
        }
        Ok(values)
    }

    fn read_graph_input_descriptor(&self, ptr: u32) -> VmResult<GraphInputDescriptor> {
        const GROUP_STRIDE: u32 = 64;
        const REGION_STRIDE: u32 = 196;
        const MAX_GROUPS: usize = 256;
        const MAX_REGIONS_PER_GROUP: usize = 256;

        let group_count = (self.read_int(ptr, 2)? as i32).clamp(0, MAX_GROUPS as i32) as usize;
        let groups_ptr = self.read_pointer_field(ptr.wrapping_add(4))?;
        tracing::debug!(
            descriptor = format_args!("0x{ptr:08X}"),
            group_count,
            groups = format_args!("0x{groups_ptr:08X}"),
            "read graph input descriptor"
        );
        let mut flags = [0; 7];
        for (index, flag) in flags.iter_mut().enumerate() {
            *flag = self.read_int(ptr.wrapping_add(12 + (index * 4) as u32), 2)? as i32;
        }
        let mut descriptor = GraphInputDescriptor {
            initial_group: self.read_int(ptr.wrapping_add(8), 2)? as i32,
            flags,
            regions: Vec::new(),
        };
        if groups_ptr == 0 {
            return Ok(descriptor);
        }

        let mut ordinal = 0i32;
        for group in 0..group_count {
            let group_ptr = groups_ptr.wrapping_add(group as u32 * GROUP_STRIDE);
            let region_count =
                (self.read_int(group_ptr, 2)? & 0xffff).min(MAX_REGIONS_PER_GROUP as u32) as usize;
            let regions_ptr = self.read_pointer_field(group_ptr.wrapping_add(8))?;
            let selected_index = self.read_int(group_ptr.wrapping_add(12), 2)? as i32;
            tracing::debug!(
                group,
                group_ptr = format_args!("0x{group_ptr:08X}"),
                region_count,
                regions = format_args!("0x{regions_ptr:08X}"),
                "read graph input group"
            );
            if regions_ptr == 0 {
                continue;
            }
            for index in 0..region_count {
                let region_ptr = regions_ptr.wrapping_add(index as u32 * REGION_STRIDE);
                let enabled_depth = self.read_int(region_ptr.wrapping_add(4), 2)? as i32;
                let x = self.read_int(region_ptr.wrapping_add(8), 2)? as i32;
                let y = self.read_int(region_ptr.wrapping_add(12), 2)? as i32;
                let width = self.read_int(region_ptr.wrapping_add(16), 2)? as i32;
                let height = self.read_int(region_ptr.wrapping_add(20), 2)? as i32;
                let normal_resource = self.read_int(region_ptr.wrapping_add(32), 2)? as i32;
                let selected_resource = self.read_int(region_ptr.wrapping_add(40), 2)? as i32;
                let mask_resource = self.read_int(region_ptr.wrapping_add(48), 2)? as i32;
                let region_flags = self.read_int(region_ptr.wrapping_add(192), 2)? as i32;
                tracing::debug!(
                    group,
                    index,
                    region_ptr = format_args!("0x{region_ptr:08X}"),
                    x,
                    y,
                    width,
                    height,
                    normal_resource,
                    selected_resource,
                    mask_resource,
                    flags = format_args!("0x{region_flags:08X}"),
                    "read graph input region"
                );
                if (width > 0 && height > 0) || normal_resource >= 0 {
                    descriptor.regions.push(GraphInputRegion {
                        group: group as i32,
                        index: index as i32,
                        ordinal,
                        enabled_depth,
                        selected: index as i32 == selected_index,
                        x,
                        y,
                        width,
                        height,
                        normal_resource,
                        selected_resource,
                        mask_resource,
                        flags: region_flags,
                    });
                }
                ordinal = ordinal.saturating_add(1);
            }
        }
        Ok(descriptor)
    }

    fn read_compact_graph_input_descriptor(&self, ptr: u32) -> VmResult<GraphInputDescriptor> {
        // funcs_48065E[0xBA] -> sub_47EF00 -> sub_46C8E0 -> sub_46C750
        // copies a 32-byte root, 52-byte groups, and 60-byte regions.
        const GROUP_STRIDE: u32 = 52;
        const REGION_STRIDE: u32 = 60;
        const MAX_GROUPS: usize = 256;
        const MAX_REGIONS_PER_GROUP: usize = 256;

        let group_count = (self.read_int(ptr, 2)? as i32).clamp(0, MAX_GROUPS as i32) as usize;
        let groups_ptr = self.read_pointer_field(ptr.wrapping_add(4))?;
        let mut flags = [0; 7];
        for (index, flag) in flags.iter_mut().enumerate() {
            *flag = self.read_int(ptr.wrapping_add(12 + (index * 4) as u32), 2)? as i32;
        }
        let mut descriptor = GraphInputDescriptor {
            initial_group: self.read_int(ptr.wrapping_add(8), 2)? as i32,
            flags,
            regions: Vec::new(),
        };
        if groups_ptr == 0 {
            return Ok(descriptor);
        }

        let mut ordinal = 0i32;
        for group in 0..group_count {
            let group_ptr = groups_ptr.wrapping_add(group as u32 * GROUP_STRIDE);
            let region_count =
                (self.read_int(group_ptr, 2)? & 0xffff).min(MAX_REGIONS_PER_GROUP as u32) as usize;
            let regions_ptr = self.read_pointer_field(group_ptr.wrapping_add(4))?;
            let selected_index = self.read_int(group_ptr.wrapping_add(8), 2)? as i32;
            if regions_ptr == 0 {
                continue;
            }
            for index in 0..region_count {
                let region_ptr = regions_ptr.wrapping_add(index as u32 * REGION_STRIDE);
                let enabled_depth = self.read_int(region_ptr, 2)? as i32;
                let x = self.read_int(region_ptr.wrapping_add(4), 2)? as i32;
                let y = self.read_int(region_ptr.wrapping_add(8), 2)? as i32;
                let normal_resource = self.read_int(region_ptr.wrapping_add(12), 2)? as i32;
                let selected_resource = self.read_int(region_ptr.wrapping_add(16), 2)? as i32;
                let mask_resource = self.read_int(region_ptr.wrapping_add(24), 2)? as i32;
                let region_flags = self.read_int(region_ptr.wrapping_add(56), 2)? as i32;
                descriptor.regions.push(GraphInputRegion {
                    group: group as i32,
                    index: index as i32,
                    ordinal,
                    enabled_depth,
                    selected: index as i32 == selected_index,
                    x,
                    y,
                    width: 0,
                    height: 0,
                    normal_resource,
                    selected_resource,
                    mask_resource,
                    flags: region_flags,
                });
                ordinal = ordinal.saturating_add(1);
            }
        }
        Ok(descriptor)
    }

    fn read_pointer_field(&self, ptr: u32) -> VmResult<u32> {
        Ok(match self.read_value(ptr, 2)? {
            Value::Ptr(value) => value,
            Value::Int(value) if value != 0 => value as u32,
            _ => 0,
        })
    }

    fn handle_text_measure_call(&mut self) -> VmResult<()> {
        let _flags = self.pop_value()?;
        let _style = self.pop_value()?;
        let _scale = self.pop_value()?;
        let font_size = self.pop_value()?.as_i32().max(1);
        let max_width = self.pop_value()?.as_i32().max(0);
        let text = self.pop_value()?;
        let dest = self.pop_ptr()?;
        let measured = measure_text_width_value(&text, font_size, max_width);
        self.write_int(dest, 2, measured as u32)?;
        Ok(())
    }

    fn write_value(&mut self, ptr: u32, width: u8, value: &Value) -> VmResult<()> {
        let addr = Self::value_key(ptr);
        self.write_int(ptr, width, value.as_i32() as u32)?;
        if width >= 2 {
            match value {
                Value::Int(_) | Value::None => {
                    self.mem_values.remove(&addr);
                }
                Value::Ptr(_) | Value::Str(_) | Value::Func { .. } | Value::Program(_) => {
                    self.mem_values.insert(addr, value.clone());
                }
            }
        } else {
            self.mem_values.remove(&addr);
        }
        Ok(())
    }

    fn trace_watch_write(&self, ptr: u32, size: usize, value: u32, source: &'static str) {
        self.trace_watch_access(
            ptr,
            size,
            Some(value),
            None,
            source,
            "VM watched memory write",
        );
    }

    fn trace_watch_read(&self, ptr: u32, size: usize, value: &Value, source: &'static str) {
        self.trace_watch_access(
            ptr,
            size,
            None,
            Some(value),
            source,
            "VM watched memory read",
        );
    }

    fn trace_watch_access(
        &self,
        ptr: u32,
        size: usize,
        write_value: Option<u32>,
        read_value: Option<&Value>,
        source: &'static str,
        message: &'static str,
    ) {
        let watches = trace_watch_addresses();
        if watches.is_empty() {
            return;
        }
        let start = Self::memory_addr(ptr);
        let end = start.saturating_add(size as u32);
        let hit = watches
            .iter()
            .copied()
            .find(|watch| *watch >= start && *watch < end);
        if let Some(watch) = hit {
            let watch_value = (watch as usize)
                .checked_add(4)
                .filter(|end| *end <= self.memory.len())
                .map(|end| {
                    let bytes = &self.memory[end - 4..end];
                    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                });
            let program = self
                .programs
                .get(self.current_program)
                .and_then(|program| program.script_name.as_deref())
                .unwrap_or("<anonymous>");
            let offset = self
                .programs
                .get(self.current_program)
                .and_then(|program| program.instructions.get(self.pc))
                .map(|instruction| instruction.offset);
            tracing::warn!(
                pc = self.pc,
                offset = offset.map(|offset| format!("0x{offset:08X}")).as_deref(),
                program,
                ptr = format_args!("0x{ptr:08X}"),
                addr = format_args!("0x{start:08X}"),
                size,
                write_value = write_value.map(|value| format!("0x{value:08X}")).as_deref(),
                read_value = read_value.map(value_summary).as_deref(),
                watch = format_args!("0x{watch:08X}"),
                watch_value = watch_value.map(|value| format!("0x{value:08X}")).as_deref(),
                source,
                "{message}"
            );
        }
    }

    fn alloc_heap(&mut self, size: u32) -> u32 {
        let size = size.max(4).saturating_add(3) & !3;
        let offset = if let Some(index) = self
            .heap_free_blocks
            .iter()
            .position(|(_, available)| *available >= size)
        {
            let (offset, available) = self.heap_free_blocks[index];
            if available == size {
                self.heap_free_blocks.remove(index);
            } else {
                self.heap_free_blocks[index] = (offset + size, available - size);
            }
            offset
        } else {
            let offset = self.heap_ptr;
            let Some(next) = offset
                .checked_add(size)
                .filter(|next| *next <= ADDRESS_MASK)
            else {
                return 0;
            };
            self.heap_ptr = next;
            offset
        };
        let ptr = 0x1200_0000 | offset;
        let Ok(range) = self.resolve_write_range(ptr, size as usize) else {
            return 0;
        };
        self.memory[range].fill(0);
        self.clear_shadow_values(ptr, size as usize);
        self.heap_allocations.insert(offset, size);
        ptr
    }

    fn free_heap(&mut self, ptr: u32) -> bool {
        if ptr == 0 {
            return true;
        }
        let offset = ptr & ADDRESS_MASK;
        let Some(size) = self.heap_allocations.remove(&offset) else {
            return false;
        };
        self.clear_shadow_values(ptr, size as usize);
        self.heap_free_blocks.push((offset, size));
        self.heap_free_blocks.sort_unstable_by_key(|block| block.0);
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(self.heap_free_blocks.len());
        for (offset, size) in self.heap_free_blocks.drain(..) {
            if let Some((previous_offset, previous_size)) = merged.last_mut() {
                if previous_offset.saturating_add(*previous_size) == offset {
                    *previous_size = previous_size.saturating_add(size);
                    continue;
                }
            }
            merged.push((offset, size));
        }
        self.heap_free_blocks = merged;
        true
    }

    fn rand_msvc(&mut self) -> i32 {
        self.rng_seed = self.rng_seed.wrapping_mul(214013).wrapping_add(2531011);
        ((self.rng_seed >> 16) & 0x7fff) as i32
    }

    fn rand_msvc_wide(&mut self) -> i32 {
        let high = self.rand_msvc() << 8;
        let mid = self.rand_msvc();
        let value = (high ^ mid) << 8;
        value ^ self.rand_msvc()
    }

    fn read_c_string(&self, ptr: u32) -> VmResult<String> {
        let bytes = self.read_c_string_bytes(ptr)?;
        let (text, _, _) = encoding_rs::SHIFT_JIS.decode(&bytes);
        Ok(text.into_owned())
    }

    fn read_c_string_bytes(&self, ptr: u32) -> VmResult<Vec<u8>> {
        let start = Self::memory_addr(ptr) as usize;
        if start >= self.memory.len() {
            return Err(VmError::MemoryOutOfBounds { addr: ptr, size: 1 });
        }
        let end = self.memory[start..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|offset| start + offset)
            .unwrap_or(self.memory.len());
        Ok(self.memory[start..end].to_vec())
    }

    fn read_wide_c_string(&self, ptr: u32) -> VmResult<Vec<u16>> {
        let start = Self::memory_addr(ptr) as usize;
        if start >= self.memory.len() {
            return Err(VmError::MemoryOutOfBounds { addr: ptr, size: 2 });
        }
        let mut values = Vec::new();
        let mut cursor = start;
        while cursor.saturating_add(2) <= self.memory.len() {
            let value = u16::from_le_bytes([self.memory[cursor], self.memory[cursor + 1]]);
            if value == 0 {
                return Ok(values);
            }
            values.push(value);
            cursor += 2;
        }
        Err(VmError::MemoryOutOfBounds {
            addr: ptr.wrapping_add((cursor.saturating_sub(start)) as u32),
            size: 2,
        })
    }

    fn c_string_byte_len(&self, ptr: u32) -> VmResult<usize> {
        let start = Self::memory_addr(ptr) as usize;
        if start >= self.memory.len() {
            return Err(VmError::MemoryOutOfBounds { addr: ptr, size: 1 });
        }
        Ok(self.memory[start..]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(self.memory.len() - start))
    }

    fn write_c_string(&mut self, ptr: u32, text: &str) -> VmResult<()> {
        self.write_c_string_raw(ptr, text)?;
        let len = encoding_rs::SHIFT_JIS
            .encode(text)
            .0
            .len()
            .saturating_add(1);
        self.restore_script_records(ptr, len)?;
        Ok(())
    }

    fn copy_c_string_value(&mut self, dst: u32, source: Value) -> VmResult<()> {
        match source {
            Value::Str(text) => self.write_c_string(dst, &text),
            Value::Int(ptr) if ptr != 0 => {
                let src = Self::translate_system_descriptor(ptr as u32);
                let size = self.c_string_byte_len(src)?.saturating_add(1);
                self.copy_buffer(dst, src, size)
            }
            Value::Ptr(ptr) if ptr != 0 => {
                let src = Self::translate_system_descriptor(ptr);
                let size = self.c_string_byte_len(src)?.saturating_add(1);
                self.copy_buffer(dst, src, size)
            }
            Value::Func { offset, .. } if offset != 0 => {
                let size = self.c_string_byte_len(offset)?.saturating_add(1);
                self.copy_buffer(dst, offset, size)
            }
            Value::Int(_) | Value::Ptr(_) | Value::Func { .. } | Value::None => {
                self.write_c_string(dst, "")
            }
            Value::Program(_) => Err(VmError::Runtime(
                "program value cannot be copied as a C string".into(),
            )),
        }
    }

    fn write_c_string_raw(&mut self, ptr: u32, text: &str) -> VmResult<()> {
        let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode(text);
        let bytes = encoded.as_ref();
        let range = self.resolve_write_range(ptr, bytes.len().saturating_add(1))?;
        let start = range.start;
        self.memory[start..start + bytes.len()].copy_from_slice(bytes);
        self.memory[start + bytes.len()] = 0;
        self.trace_watch_write(ptr, bytes.len().saturating_add(1), 0, "write_c_string");
        Ok(())
    }

    fn copy_buffer(&mut self, dst: u32, src: u32, size: usize) -> VmResult<()> {
        let src_range = self.resolve_range(src, size)?;
        let dst_range = self.resolve_write_range(dst, size)?;
        let tmp = self.memory[src_range].to_vec();
        self.memory[dst_range].copy_from_slice(&tmp);
        self.copy_shadow_values(src, dst, size);
        Ok(())
    }

    fn write_loaded_file<A>(
        &mut self,
        api: &mut A,
        buffer: u32,
        archive: &str,
        file: &str,
        offset: usize,
        length: Option<usize>,
    ) -> VmResult<i32>
    where
        A: SysApi,
    {
        let Some(bytes) = api.load_file_bytes(archive, file) else {
            tracing::debug!(
                archive,
                file,
                offset,
                length,
                written = 0,
                available = 0,
                "VM ReadResourceToBuffer missing"
            );
            return Ok(0);
        };
        if offset >= bytes.len() {
            tracing::debug!(
                archive,
                file,
                offset,
                length,
                written = 0,
                available = bytes.len(),
                "VM ReadResourceToBuffer out of range"
            );
            return Ok(0);
        }
        let available = bytes.len() - offset;
        let count = length.unwrap_or(available).min(available);
        let range = self.resolve_write_range(buffer, count)?;
        self.memory[range].copy_from_slice(&bytes[offset..offset + count]);
        self.clear_shadow_values(buffer, count);
        if offset == 0 {
            self.scenario_loaded_file_preprocess(file, buffer, &bytes[..count])?;
        }
        tracing::debug!(
            archive,
            file,
            offset,
            length,
            written = count,
            available = bytes.len(),
            "VM ReadResourceToBuffer"
        );
        Ok(count as i32)
    }

    #[cfg(test)]
    fn try_builtin_sys(&mut self, group: u8, id: u16) -> VmResult<Option<Value>> {
        self.try_builtin_sys_with_api(&mut TraceApi, group, id)
    }

    fn try_builtin_sys_with_api<A: SysApi>(
        &mut self,
        api: &mut A,
        group: u8,
        id: u16,
    ) -> VmResult<Option<Value>> {
        let result = match (group, id) {
            (0x81, 0x07) => {
                let index = self.pop_int()?;
                let destination = self.pop_ptr()?;
                if let Some((x, y)) = api.pointer_position(index) {
                    self.write_int(destination, 2, x as u32)?;
                    self.write_int(destination.wrapping_add(4), 2, y as u32)?;
                    Value::Int(1)
                } else {
                    Value::Int(0)
                }
            }
            (0x81, 0x08) => {
                let destination = self.pop_ptr()?;
                self.write_c_string(destination, &api.host_user_name())?;
                Value::None
            }
            (0x81, 0x09) => {
                let destination = self.pop_ptr()?;
                self.write_c_string(destination, &api.host_computer_name())?;
                Value::None
            }
            (0x81, 0x0a) => {
                let text = self.pop_ptr()?;
                let normalized = self
                    .read_c_string(text)?
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                self.write_c_string(text, &normalized)?;
                Value::Int(1)
            }
            (0x81, 0x0b) => {
                let _source = self.pop_ptr()?;
                let destination = self.pop_ptr()?;
                let _version = self.pop_ptr()?;
                for index in 0..4 {
                    self.write_int(destination.wrapping_add(index * 4), 2, 0)?;
                }
                Value::None
            }
            (0x81, 0x0c) => {
                let service_pack = self.pop_ptr()?;
                let version = self.pop_ptr()?;
                for (index, value) in [10u32, 0, 0, 2].into_iter().enumerate() {
                    self.write_int(version.wrapping_add(index as u32 * 4), 2, value)?;
                }
                self.write_c_string(service_pack, "")?;
                Value::None
            }
            (0x81, 0x0d) => {
                let available = self.pop_ptr()?;
                let total = self.pop_ptr()?;
                let total_mb = (self.memory.len() / (1024 * 1024)).min(u32::MAX as usize) as u32;
                let available_mb = (self.memory.len().saturating_sub(self.heap_ptr as usize)
                    / (1024 * 1024))
                    .min(u32::MAX as usize) as u32;
                self.write_int(total, 2, total_mb)?;
                self.write_int(available, 2, available_mb)?;
                Value::None
            }
            (0x81, 0x11) => {
                let destination = self.pop_ptr()?;
                let state = api.keyboard_state();
                let range = self.resolve_range(destination, state.len())?;
                self.memory[range].copy_from_slice(&state);
                self.clear_shadow_values(destination, state.len());
                Value::None
            }
            (0x81, 0x17) => {
                let count = self.pop_int()?.max(0) as usize;
                let start = self.pop_int()?;
                let distances = self.pop_ptr()?;
                let points = self.pop_ptr()?;
                let mut written = 0usize;
                for index in 0..count {
                    let Some((x, y)) = api.pointer_position(start.saturating_add(index as i32))
                    else {
                        break;
                    };
                    self.write_int(points.wrapping_add(index as u32 * 8), 2, x as u32)?;
                    self.write_int(points.wrapping_add(index as u32 * 8 + 4), 2, y as u32)?;
                    let distance = if index == 0 {
                        -1
                    } else {
                        let (previous_x, previous_y) = api
                            .pointer_position(start.saturating_add(index as i32 - 1))
                            .unwrap_or((x, y));
                        let dx = x.saturating_sub(previous_x);
                        let dy = y.saturating_sub(previous_y);
                        (((i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)) as f64)
                            .sqrt()) as i32
                    };
                    self.write_int(distances.wrapping_add(index as u32 * 4), 2, distance as u32)?;
                    written += 1;
                }
                Value::Int(written.min(i32::MAX as usize) as i32)
            }
            (0x81, 0x1d) => {
                let _device = self.pop_int()?;
                let destination = self.pop_ptr()?;
                let (x, y) = api.pointer_position(0).unwrap_or_default();
                for (index, value) in [x, y, 0, 0, -1, 0].into_iter().enumerate() {
                    self.write_int(destination.wrapping_add(index as u32 * 4), 2, value as u32)?;
                }
                Value::Int(0)
            }
            (0x81, 0x6b) => {
                let destination = self.pop_ptr()?;
                let command_line = api.runtime_command_line();
                if destination != 0 {
                    self.write_c_string(destination, &command_line)?;
                }
                Value::Int(
                    encoding_rs::SHIFT_JIS
                        .encode(&command_line)
                        .0
                        .len()
                        .saturating_add(1)
                        .min(i32::MAX as usize) as i32,
                )
            }
            (0x81, 0xb0) => {
                let left = self.pop_ptr()?;
                let right = self.pop_ptr()?;
                let left = self.read_wide_c_string(left)?;
                let right = self.read_wide_c_string(right)?;
                Value::Int(wide_string_similarity(&left, &right))
            }
            (0x81, 0xb7) => {
                let source = self.pop_ptr()?;
                let destination = self.pop_ptr()?;
                let source_bytes = self.read_c_string_bytes(source)?;
                let (decoded, _, _) = encoding_rs::SHIFT_JIS.decode(&source_bytes);
                let encoded = decoded.encode_utf16().collect::<Vec<_>>();
                for (index, value) in encoded.iter().copied().enumerate() {
                    self.write_int(destination.wrapping_add(index as u32 * 2), 1, value as u32)?;
                }
                self.write_int(destination.wrapping_add(encoded.len() as u32 * 2), 1, 0)?;
                Value::Int(encoded.len().min(i32::MAX as usize) as i32)
            }
            (0x81, 0xe9) => {
                let length = self.pop_int()?.max(0) as usize;
                let source = self.pop_ptr()?;
                let destination = self.pop_ptr()?;
                let range = self.resolve_range(source, length)?;
                let bytes = self.memory[range].to_vec();
                let initial = [
                    self.read_int(destination, 2)?,
                    self.read_int(destination.wrapping_add(4), 2)?,
                ];
                let hash = native_file_hash_update(initial, &bytes);
                self.write_int(destination, 2, hash[0])?;
                self.write_int(destination.wrapping_add(4), 2, hash[1])?;
                Value::None
            }
            (0x81, 0xea) => {
                let length = self.pop_int()?.max(0) as usize;
                let source = self.pop_ptr()?;
                let destination = self.pop_ptr()?;
                let source_range = self.resolve_range(source, length)?;
                let bytes = self.memory[source_range].to_vec();
                let destination_range = self.resolve_range(destination, length)?;
                self.memory[destination_range].copy_from_slice(&bytes);
                self.clear_shadow_values(destination, length);
                Value::None
            }
            (0x81, 0x60) => {
                let _height = self.pop_int()?;
                let _width = self.pop_int()?;
                let _flags = self.pop_int()?;
                Value::None
            }
            (0x81, 0x64) => {
                let _height = self.pop_int()?;
                let _width = self.pop_int()?;
                Value::None
            }
            (0x81, 0x0e) | (0x81, 0x18) | (0x81, 0x62) | (0x81, 0x6f) => {
                let _arg = self.pop_value()?;
                Value::None
            }
            (0x80, 0x52) => {
                let _mode = self.pop_int()?;
                Value::None
            }
            (0x80, 0x06) => {
                let _mode = self.pop_value()?;
                Value::Int(0)
            }
            (0x80, 0x00) => {
                self.rng_seed = self.pop_int()? as u32;
                Value::None
            }
            (0x80, 0x01) => Value::Int(self.rand_msvc()),
            (0x80, 0x02) => {
                let max = self.pop_int()?;
                if max > 0 {
                    Value::Int(self.rand_msvc_wide().rem_euclid(max))
                } else {
                    Value::Int(0)
                }
            }
            (0x80, 0x11) => {
                let _key_code = self.pop_value()?;
                Value::Int(0)
            }
            (0x80, 0x16) => Value::Int(0),
            (0x80, 0x5c) => {
                let arg3 = self.pop_value()?;
                let arg2 = self.pop_value()?;
                let arg1 = self.pop_value()?;
                let duration_ms = arg1.as_i32().max(0);
                let input_enabled = arg2.as_i32() != 0;
                let input_scope = arg3.as_i32();
                self.timing.begin_wait(duration_ms);
                self.wait_blocked = true;
                self.wait_input_scope = input_enabled.then_some(input_scope);
                if self.collect_diagnostics {
                    tracing::debug!(
                        duration_ms,
                        input_enabled,
                        input_scope,
                        "VM WaitTimingEx procedure"
                    );
                }
                Value::None
            }
            (0x80, 0x4b) => {
                let destination = self.pop_ptr()?;
                let count = self.pop_int()?.max(0).min(256) as usize;
                let mut received = 0usize;
                while received < count {
                    let Some(value) = self.pending_program_messages.pop_front() else {
                        break;
                    };
                    let index = received;
                    self.write_value(destination.saturating_add(index as u32 * 4), 2, &value)?;
                    received += 1;
                }
                Value::Int(received as i32)
            }
            (0x80, 0x62) => {
                let _descriptor = self.pop_value()?;
                let _mode = self.pop_value()?;
                Value::None
            }
            (0x80, 0x46) => self
                .programs
                .get(self.current_program)
                .cloned()
                .map(|program| Value::Program(Arc::new(program)))
                .unwrap_or(Value::None),
            (0x80, 0x5a) => Value::None,
            (0x80, 0x6a) => Value::None,
            (0x80, 0xac) => {
                let _descriptor = self.pop_value()?;
                let _count = self.pop_value()?;
                let _object = self.pop_value()?;
                Value::None
            }
            (0x80, 0xc0) => {
                let length = self.pop_int()?;
                let source = self.pop_ptr()?;
                let destination = self.pop_ptr()?;
                Value::Int(self.encode_user_data_buffer(destination, source, length)?)
            }
            (0x80, 0xc1) => {
                let src = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                let written = self.decode_sdc_records(src, dst)?;
                tracing::debug!(
                    src = format_args!("0x{src:08X}"),
                    dst = format_args!("0x{dst:08X}"),
                    written,
                    "SdcDecodeRecords"
                );
                Value::Int(written)
            }
            (0x80, 0xc4) => {
                let record_count = self.pop_int()?;
                let record_size = self.pop_int()?;
                let source = self.pop_ptr()?;
                let destination = self.pop_ptr()?;
                Value::Int(self.encode_user_data_structs(
                    destination,
                    source,
                    record_size,
                    record_count,
                )?)
            }
            (0x80, 0xc5) => {
                let src = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                Value::Int(self.decode_sdc_struct_array(src, dst)?)
            }
            (0x80, 0xd2) => {
                let src = self.pop_value()?;
                let key = self.pop_value()?;
                let handle = self.pop_ptr()?;
                let src = src.as_i32() as u32;
                self.sys_record_table_copy(handle, key, src)?;
                Value::Int(0)
            }
            (0x80, 0xd3) => {
                let _arg2 = self.pop_value()?;
                let _arg1 = self.pop_value()?;
                Value::Int(0)
            }
            (0x80, 0x84) => {
                let name = self.pop_string_lossy()?;
                let values = self.string_hash_tables.entry(i32::MIN).or_default();
                if !values.iter().any(|value| value == &name) {
                    values.push(name);
                }
                Value::Int(1)
            }
            (0x80, 0x85) => {
                let name = self.pop_string_lossy()?;
                Value::Int(i32::from(
                    self.string_hash_tables
                        .get(&i32::MIN)
                        .is_some_and(|values| values.iter().any(|value| value == &name)),
                ))
            }
            (0x80, 0x8a) => {
                let length = self.pop_int()?;
                let enabled = self.pop_int()? != 0;
                let offset = self.pop_int()?;
                let name = self.pop_string_lossy()?;
                let status = match (
                    self.read_flags.get_mut(&name),
                    u32::try_from(offset),
                    u32::try_from(length),
                ) {
                    (None, _, _) => 1,
                    (Some(_), Err(_), _) => 2,
                    (Some(_), _, Err(_) | Ok(0)) => 3,
                    (Some(_), _, Ok(length)) if length > 65_536 => 3,
                    (Some(flags), Ok(offset), Ok(length)) => {
                        if flags.set_range(offset, length, enabled) {
                            0
                        } else {
                            3
                        }
                    }
                };
                Value::Int(status)
            }
            (0x80, 0x8b) => {
                let offset = self.pop_int()?;
                let output = self.pop_ptr()?;
                let name = self.pop_string_lossy()?;
                let (status, enabled) = match (self.read_flags.get(&name), u32::try_from(offset)) {
                    (None, _) => (1, false),
                    (Some(_), Err(_)) => (2, false),
                    (Some(flags), Ok(offset)) => match flags.contains(offset) {
                        Some(enabled) => (0, enabled),
                        None => (2, false),
                    },
                };
                self.write_int(output, 2, u32::from(enabled))?;
                Value::Int(status)
            }
            (0x80, 0xda) => {
                let src = self.pop_ptr()?;
                let table_id = self.pop_int()?;
                let count = self.pop_int()?.max(0) as usize;
                if count == 0 {
                    self.string_hash_tables.remove(&table_id);
                    Value::Int(1)
                } else {
                    let mut cursor = src;
                    let mut values = Vec::with_capacity(count);
                    for _ in 0..count {
                        let text = self.read_c_string(cursor)?;
                        let byte_len = self.c_string_byte_len(cursor)?;
                        values.push(text);
                        cursor = cursor.saturating_add(byte_len as u32 + 1);
                    }
                    self.string_hash_tables.insert(table_id, values);
                    Value::Int(1)
                }
            }
            (0x80, 0xd9) => {
                let table_id = self.pop_int()?;
                Value::Int(
                    self.string_hash_tables
                        .get(&table_id)
                        .map_or(0, |values| values.len().min(i32::MAX as usize) as i32),
                )
            }
            (0x80, 0xdb) => {
                // sub_48A890/sub_495640: serialize all NUL-terminated strings
                // in a namespace, or only report the required byte count for dst=0.
                let table_id = self.pop_int()?;
                let destination = self.pop_ptr()?;
                let values = self
                    .string_hash_tables
                    .get(&table_id)
                    .cloned()
                    .unwrap_or_default();
                let mut cursor = destination;
                let mut total = 0usize;
                for value in values {
                    let byte_len = encoding_rs::SHIFT_JIS.encode(&value).0.len() + 1;
                    if destination != 0 {
                        self.write_c_string_raw(cursor, &value)?;
                        cursor = cursor.saturating_add(byte_len as u32);
                    }
                    total = total.saturating_add(byte_len);
                }
                Value::Int(total.min(i32::MAX as usize) as i32)
            }
            (0x80, 0x04) => Value::Int(self.timing.tick_count()),
            (0x80, 0x05) => {
                let ptr = self.pop_ptr()?;
                let counter = self.timing.performance_counter();
                self.write_int(ptr, 2, counter as u32)?;
                self.write_int(ptr.wrapping_add(4), 2, (counter >> 32) as u32)?;
                Value::Int(1)
            }
            (0x80, 0x0c) => {
                let ptr = self.pop_ptr()?;
                self.write_system_time(ptr)?;
                Value::None
            }
            (0x80, 0x0d) => {
                self.push_value(Value::Int(self.memory.len() as i32));
                Value::Int((self.memory.len().saturating_sub(self.heap_ptr as usize)) as i32)
            }
            (0x80, 0x0f) => Value::Int(0),
            (0x80, 0x14) => {
                let _descriptor = self.pop_value()?;
                Value::Int(1)
            }
            (0x80, 0x17) => Value::Int(0),
            (0x80, 0x1b) => {
                let _descriptor = self.pop_value()?;
                let _size = self.pop_int()?;
                Value::None
            }
            (0x80, 0x20) => {
                let size = self.pop_int()?.max(0) as u32;
                Value::Ptr(self.alloc_heap(size))
            }
            (0x80, 0x21) => {
                let ptr = self.pop_ptr()?;
                Value::Int(i32::from(self.free_heap(ptr)))
            }
            (0x80, 0x33) => {
                let _mode = self.pop_value()?;
                let _key = self.pop_value()?;
                Value::None
            }
            (0x80, 0x36) => {
                let _enabled = self.pop_value()?;
                Value::None
            }
            (0x80, 0x37) => {
                let _path = self.pop_string_lossy()?;
                Value::None
            }
            (0x80, 0x38) => {
                let _records = self.pop_value()?;
                let _name = self.pop_value()?;
                Value::Int(0)
            }
            (0x80, 0x39) => {
                let _path = self.pop_string_lossy()?;
                Value::None
            }
            (0x80, 0x3a) => {
                let _mode = self.pop_value()?;
                let _dst = self.pop_value()?;
                Value::Int(1)
            }
            (0x80, 0x3b) => {
                let _flags = self.pop_value()?;
                let _out_path = self.pop_value()?;
                let _title = self.pop_value()?;
                let _pattern = self.pop_value()?;
                let _filter = self.pop_value()?;
                let _initial_dir = self.pop_value()?;
                Value::Int(0)
            }
            (0x80, 0x3d) => {
                let _kind = self.pop_value()?;
                let ptr = self.pop_ptr()?;
                self.write_value(ptr, 2, &Value::Str(String::new()))?;
                Value::None
            }
            (0x80, 0x45) => Value::None,
            (0x80, 0x80) => {
                self.push_value(Value::Int(0));
                self.push_value(Value::Int(0));
                Value::Int(0)
            }
            (0x80, 0x88) => {
                let length = self.pop_int()?.max(0) as usize;
                let script_name = self.pop_string_lossy()?;
                self.scenario_code_preprocess(&script_name, length)?;
                let created = if length == 0 || script_name.is_empty() {
                    false
                } else if let Some(flags) = self.read_flags.get_mut(&script_name) {
                    flags.resize(length)
                } else {
                    self.read_flags
                        .insert(script_name, ReadFlagBits::new(length));
                    true
                };
                Value::Int(i32::from(created))
            }
            (0x80, 0x89) => {
                let enabled = self.pop_int()? != 0;
                let offset = self.pop_int()?;
                let name = self.pop_string_lossy()?;
                let status = match (self.read_flags.get_mut(&name), u32::try_from(offset)) {
                    (None, _) => 1,
                    (Some(_), Err(_)) => 2,
                    (Some(flags), Ok(offset)) => {
                        if flags.set(offset, enabled) {
                            0
                        } else {
                            2
                        }
                    }
                };
                Value::Int(status)
            }
            (0x80, 0xd0) => {
                let record_size = self.pop_int()?.max(0) as u32;
                let slot = self.pop_ptr()?;
                self.sys_record_table_open(slot, record_size)?
            }
            (0x80, 0xd1) => {
                let handle = self.pop_ptr()?;
                self.sys_record_table_close(handle);
                Value::Int(0)
            }
            (0x80, 0xd4) => {
                let mode = self.pop_int()?;
                let selector = self.pop_value()?;
                let handle = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                self.sys_record_table_fetch(dst, handle, selector, mode)?
            }
            (0x80, 0x82) => {
                let length = self.pop_int()?;
                let src = self.pop_ptr()?;
                let offset = self.pop_int()?;
                self.copy_to_global_data(offset, src, length)?;
                Value::None
            }
            (0x80, 0x83) => {
                let length = self.pop_int()?;
                let offset = self.pop_int()?;
                let dst = self.pop_ptr()?;
                self.copy_from_global_data(dst, offset, length)?;
                Value::None
            }
            (0x80, 0x98) => {
                let record_size = self.pop_int()?.max(0) as u32;
                let capacity = self.pop_int()?.max(0) as u32;
                let slot = self.pop_ptr()?;
                Value::Int(self.sys_indexed_record_open(slot, capacity, record_size)?)
            }
            (0x80, 0x9a) => {
                let handle = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                Value::Int(self.sys_indexed_record_count(dst, handle)?)
            }
            (0x80, 0x9d) => {
                let index = self.pop_int()?.max(0) as u32;
                let handle = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                Value::Int(self.sys_indexed_record_load(dst, handle, index)?)
            }
            (0x80, 0x9e) => {
                let count = self.pop_int()?.max(0) as u32;
                let start = self.pop_int()?.max(0) as u32;
                let handle = self.pop_ptr()?;
                Value::Int(self.sys_indexed_record_remove(handle, start, count))
            }
            (0x80, 0x99) => {
                let handle = self.pop_ptr()?;
                Value::Int(self.sys_indexed_record_close(handle))
            }
            (0x80, 0x9c) => {
                let src = self.pop_ptr()?;
                let handle = self.pop_ptr()?;
                Value::Int(self.sys_indexed_record_push(handle, src)?)
            }
            (0x80, 0xdd) => {
                let index = self.pop_int()?;
                let table_id = self.pop_int()?;
                let dst = self.pop_ptr()?;
                let Some(values) = self.string_hash_tables.get(&table_id) else {
                    return Ok(Some(Value::Int(i32::MIN + 1)));
                };
                let Some(text) = usize::try_from(index)
                    .ok()
                    .and_then(|index| values.get(index))
                    .cloned()
                else {
                    return Ok(Some(Value::Int(i32::MIN + 2)));
                };
                self.write_c_string(dst, &text)?;
                tracing::debug!(table_id, index, text, "StringHashTableGet");
                Value::Int(0)
            }
            (0x80, 0x58) | (0x80, 0x67) | (0x80, 0x68) => {
                let _arg = self.pop_value()?;
                Value::None
            }
            (0x80, 0x70) => {
                let shift = self.pop_int()?;
                Value::Int(i32::from(self.allocate_global_config(shift)))
            }
            (0x80, 0x71) => {
                self.global_config.fill(0);
                Value::None
            }
            (0x80, 0x74) => {
                let _arg = self.pop_value()?;
                Value::None
            }
            (0x80, 0xe8) => {
                let ptr = self.pop_ptr()?;
                self.write_c_string(ptr, "Tayutama2TV")?;
                Value::None
            }
            (0x80, 0xfd) => Value::Int(0),
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    fn try_builtin_user(&mut self, group: u8, id: u16) -> VmResult<()> {
        match (group, id) {
            (0xb0, 0x02) => {}
            (0xb0, 0x06) => {}
            (0xb0, 0x03) => {
                let _arg2 = self.pop_value()?;
                let _arg1 = self.pop_value()?;
            }
            (0xb0, 0x05) => {
                let _arg = self.pop_value()?;
            }
            (0xb0, 0x80) => {
                let _message = self.pop_string_lossy()?;
            }
            (0xb0, 0xc7) => {
                let _font_name = self.pop_string_lossy()?;
                let _slot = self.pop_int()?;
            }
            (0xb0, 0xc4) => {
                let _value = self.pop_int()?;
                self.push_value(Value::Int(0));
            }
            (0xb0, 0xc1) => {
                let _font = self.pop_int()?;
                let _text = self.pop_string_lossy()?;
                self.push_value(Value::Int(1));
            }
            (0xc0, 0x00) => {
                let _height = self.pop_value()?;
                let _width = self.pop_value()?;
            }
            (0xc0, 0x01) => {
                let _target = self.pop_value()?;
            }
            (0xc0, 0x04) => {
                let _enabled = self.pop_value()?;
                let _target = self.pop_value()?;
            }
            (0xc0, 0x05) => {
                for _ in 0..6 {
                    let _arg = self.pop_value()?;
                }
            }
            (0xc0, 0x09) => {
                let _interval = self.pop_value()?;
                let _target = self.pop_value()?;
            }
            (0xc0, 0x0a) => {
                let _capacity = self.pop_value()?;
                let _target = self.pop_value()?;
            }
            (0xc0, 0x0b) => {
                for _ in 0..10 {
                    let _arg = self.pop_value()?;
                }
            }
            (0xc0, 0x0c) => {
                let _interval = self.pop_value()?;
                let _target = self.pop_value()?;
            }
            (0xc0, 0x0d) => {
                let _duration = self.pop_value()?;
                let _target = self.pop_value()?;
            }
            (0xc0, 0x0f) => {
                let _target = self.pop_value()?;
            }
            (0xc0, 0x18) => {
                for _ in 0..7 {
                    let _arg = self.pop_value()?;
                }
            }
            (0xc0, 0x1f) => {
                let _arg = self.pop_value()?;
            }
            (0xc0, 0x28) => {
                for _ in 0..4 {
                    let _arg = self.pop_value()?;
                }
            }
            (0xc0, 0x29) => {
                for _ in 0..16 {
                    let _arg = self.pop_value()?;
                }
            }
            (0xc0, 0x2d) => {
                for _ in 0..18 {
                    let _arg = self.pop_value()?;
                }
            }
            (0xc0, 0x41) => {
                let _target = self.pop_value()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn normalize_sys_string_args(&mut self, group: u8, id: u16) -> VmResult<()> {
        let positions_from_top: &[usize] = match (group, id) {
            (0x80, 0x34) | (0x80, 0x35) | (0x80, 0x40) => &[0, 1],
            (0x80, 0x28)
            | (0x80, 0x29)
            | (0x80, 0x2a)
            | (0x80, 0x2c)
            | (0x80, 0x66)
            | (0x80, 0xe3) => &[0],
            (0x80, 0x2d) => &[1],
            (0x80, 0x2f) => &[0, 1],
            (0x80, 0x44) => &[3, 4],
            (0x80, 0xdc) => &[0],
            _ => return Ok(()),
        };
        for &from_top in positions_from_top {
            let Some(index) = self.stack.len().checked_sub(1 + from_top) else {
                continue;
            };
            let text = self.value_as_string_lossy(self.stack[index].clone())?;
            self.replace_stack_value(index, Value::Str(text));
        }
        Ok(())
    }

    fn normalize_sound_string_args(&mut self, group: u8, id: u16) -> VmResult<()> {
        let positions_from_top: &[usize] = match (group, id) {
            (0xa0, 0x11) => &[2, 3],
            // funcs_487CFE[0x12] -> sub_487280 converts source arguments
            // 2/3/4 through sub_48DF50. They are archive, diagnostic name,
            // and the actual resource name in script order.
            (0xa0, 0x12) => &[3, 4, 5],
            (0xa0, 0x10) => &[1],
            (0xa0, 0x23) => &[2, 3],
            (0xa0, 0x27) => &[3, 4],
            (0xa0, 0xC0) => &[0],
            (0xa0, 0x20) => &[0, 1],
            (0xa0, 0x21) => &[2, 3],
            _ => return Ok(()),
        };
        for &from_top in positions_from_top {
            let Some(index) = self.stack.len().checked_sub(1 + from_top) else {
                continue;
            };
            if std::env::var_os("TRACE_SOUND_ARGS").is_some() {
                self.trace_sound_arg(group, id, from_top, index);
            }
            let text = if matches!(
                (group, id, from_top),
                (0xa0, 0x11, 2) | (0xa0, 0x12, 3 | 4) | (0xa0, 0x21, 2)
            ) {
                self.value_as_sound_file_string(self.stack[index].clone())?
            } else {
                self.value_as_string_lossy(self.stack[index].clone())?
            };
            self.replace_stack_value(index, Value::Str(text));
        }
        Ok(())
    }

    fn normalize_user_string_args(&mut self, group: u8, id: u16) -> VmResult<()> {
        let positions_from_top: &[usize] = match (group, id) {
            (0xb0, 0x10) | (0xb0, 0x15) | (0xb0, 0x1C) | (0xb0, 0x26) => &[0],
            (0xb0, 0x1A) => &[6],
            (0xb0, 0x81) | (0xb0, 0x82) | (0xb0, 0x83) => &[0],
            (0xb0, 0x84) => &[1, 2, 3],
            (0xb0, 0xC0) | (0xb0, 0xC2) => &[0],
            (0xb0, 0xC3) | (0xb0, 0xC6) => &[0, 1],
            (0xb0, 0xF0) => &[2],
            (0xc0, 0x06) => &[0, 1],
            (0xc0, 0xF0) => &[1],
            _ => return Ok(()),
        };
        for &from_top in positions_from_top {
            let Some(index) = self.stack.len().checked_sub(1 + from_top) else {
                continue;
            };
            let text = self.value_as_string_lossy(self.stack[index].clone())?;
            self.replace_stack_value(index, Value::Str(text));
        }
        Ok(())
    }

    fn normalize_graph_string_args(&mut self, group: u8, id: u16) -> VmResult<()> {
        let positions_from_top: &[usize] = match (group, id) {
            // GraphLoadResource has the same (target, archive, resource)
            // string contract as preload; both strings may live in frame slots.
            (0x90, 0x10) => &[0, 1],
            // Native sub_47DF50 converts the second pop before looking it up
            // in the target graph object.
            (0x90, 0x89) => &[1],
            (0x90, 0x90) => &[4],
            (0x90, 0x56) => &[0, 3],
            // sub_4844F0/sub_484650 consume the text pointer at these exact
            // stack positions. sub_484C40 has two independent text inputs.
            (0x91, 0x91) => &[3],
            (0x91, 0x93) => &[4],
            (0x91, 0x9D) => &[9, 11],
            (0x91, 0x9E) => &[0, 1],
            (0x91, 0xBF) => &[0],
            (0x91, 0xF0) => &[2],
            (0x91, 0xF4) => &[1],
            (0x92, 0x1C) => &[6],
            (0x92, 0x1D) => &[7],
            (0x92, 0x1F) => &[0],
            (0x92, 0x9B) => &[1],
            // sub_486D30 pops five arguments in reverse order. Its third and
            // fourth pops are converted through sub_48DF50 before the graph
            // effect resource is created.
            (0x92, 0xF2) => &[2, 3],
            // Native GraphPreloadResource receives (archive, resource). Both
            // values are script pointers and are popped in reverse order.
            (0x92, 0x14) => &[0, 1],
            // Native sub_4863E0 pops 15 arguments. Its fourteenth pop is
            // converted by sub_48DF50 before constructing CProcDspMsgEx.
            (0x92, 0x90) => &[13],
            // Native sub_4867D0 converts the fourth argument from the bottom
            // through sub_48DF50. There are 21 arguments in total.
            (0x92, 0x9c) => &[17],
            _ => return Ok(()),
        };
        for &from_top in positions_from_top {
            let Some(index) = self.stack.len().checked_sub(1 + from_top) else {
                continue;
            };
            let original = self.stack[index].clone();
            let text = self.value_as_text_descriptor_string(original.clone())?;
            if std::env::var_os("TRACE_RESOURCE_ARGS").is_some() {
                let ptr = match original {
                    Value::Int(value) if value != 0 => Some(value as u32),
                    Value::Ptr(ptr) if ptr != 0 => Some(ptr),
                    _ => None,
                };
                tracing::warn!(
                    program = self.program_name(self.current_program),
                    pc = self.pc,
                    group = format_args!("0x{group:02X}"),
                    id = format_args!("0x{id:02X}"),
                    from_top,
                    value = %value_summary(&original),
                    text,
                    dump = ptr.map(|ptr| self.memory_preview(ptr, 96)).unwrap_or_default(),
                    "TRACE_RESOURCE_ARG"
                );
            }
            if is_plausible_text_payload(&text) {
                self.replace_stack_value(index, Value::Str(text));
            }
        }
        Ok(())
    }

    fn trace_sound_arg(&self, group: u8, id: u16, from_top: usize, index: usize) {
        let value = self.stack.get(index).cloned().unwrap_or(Value::None);
        let ptr = match value {
            Value::Int(value) => Some(value as u32),
            Value::Ptr(ptr) => Some(ptr),
            _ => None,
        };
        let direct = self
            .stack
            .get(index)
            .cloned()
            .and_then(|value| self.value_as_string_lossy(value).ok())
            .unwrap_or_default();
        let dump = ptr
            .map(|ptr| self.memory_preview(ptr, 96))
            .unwrap_or_default();
        tracing::warn!(
            group = format_args!("0x{group:02X}"),
            id = format_args!("0x{id:02X}"),
            from_top,
            index,
            value = ?self.stack.get(index),
            direct,
            dump,
            "TRACE_SOUND_ARG"
        );
    }

    fn memory_preview(&self, ptr: u32, size: usize) -> String {
        let addr = Self::memory_addr(ptr) as usize;
        let Some(bytes) = self
            .memory
            .get(addr..addr.saturating_add(size).min(self.memory.len()))
        else {
            return String::new();
        };
        bytes
            .chunks(16)
            .enumerate()
            .map(|(row, chunk)| {
                let hex = chunk
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let ascii: String = chunk
                    .iter()
                    .map(|byte| match *byte {
                        0x20..=0x7e => *byte as char,
                        _ => '.',
                    })
                    .collect();
                format!("+{:02X}: {hex:<47} {ascii}", row * 16)
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn values_equal(&self, left: &Value, right: &Value) -> VmResult<bool> {
        match (left, right) {
            (Value::Str(left), Value::Str(right)) => Ok(left == right),
            (Value::Str(_), Value::Int(0) | Value::Ptr(0) | Value::None)
            | (Value::Int(0) | Value::Ptr(0) | Value::None, Value::Str(_)) => Ok(false),
            (Value::Str(left), Value::Ptr(ptr)) | (Value::Ptr(ptr), Value::Str(left)) => {
                Ok(self.read_c_string(*ptr).ok().as_deref() == Some(left.as_str()))
            }
            (Value::Str(left), Value::Int(ptr)) | (Value::Int(ptr), Value::Str(left)) => {
                Ok(self.read_c_string(*ptr as u32).ok().as_deref() == Some(left.as_str()))
            }
            _ => Ok(left.as_i32() == right.as_i32()),
        }
    }

    fn value_as_string_lossy(&self, value: Value) -> VmResult<String> {
        match value {
            Value::Str(text) => Ok(text),
            Value::Ptr(ptr) => self.read_c_string(ptr),
            Value::Int(value) => match self.read_c_string(value as u32) {
                _ if self
                    .mem_values
                    .get(&Self::value_key(value as u32))
                    .and_then(|stored| match stored {
                        Value::Str(text) => Some(text),
                        _ => None,
                    })
                    .is_some() =>
                {
                    Ok(
                        match self
                            .mem_values
                            .get(&Self::value_key(value as u32))
                            .expect("checked above")
                        {
                            Value::Str(text) => text.clone(),
                            _ => String::new(),
                        },
                    )
                }
                Ok(text) if !text.is_empty() => Ok(text),
                _ => Ok(format!("0x{value:08X}")),
            },
            Value::Func { offset, .. } => Ok(format!("0x{offset:08X}")),
            Value::Program(_) | Value::None => Ok(String::new()),
        }
    }

    fn value_as_fixed_bytes(&self, value: Value, size: usize) -> VmResult<Vec<u8>> {
        match value {
            Value::Str(text) => {
                let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode(&text);
                let mut bytes = Vec::with_capacity(size);
                bytes.extend_from_slice(&encoded);
                bytes.push(0);
                bytes.resize(size, 0);
                bytes.truncate(size);
                Ok(bytes)
            }
            Value::Int(ptr) => {
                let range =
                    self.resolve_range(Self::translate_system_descriptor(ptr as u32), size)?;
                Ok(self.memory[range].to_vec())
            }
            Value::Ptr(ptr) => {
                let range = self.resolve_range(Self::translate_system_descriptor(ptr), size)?;
                Ok(self.memory[range].to_vec())
            }
            Value::None => Ok(vec![0; size]),
            Value::Func { offset, .. } => {
                let range = self.resolve_range(offset, size)?;
                Ok(self.memory[range].to_vec())
            }
            Value::Program(_) => Err(VmError::Runtime(
                "program value cannot be used as a memory block".into(),
            )),
        }
    }

    fn value_as_text_descriptor_string(&self, value: Value) -> VmResult<String> {
        let direct = self.value_as_string_lossy(value.clone())?;
        if is_plausible_text_payload(&direct) {
            return Ok(direct);
        }
        let ptr = match value {
            Value::Int(value) if value != 0 => value as u32,
            Value::Ptr(ptr) if ptr != 0 => ptr,
            _ => return Ok(direct),
        };
        const DESCRIPTOR_OFFSETS: [i32; 21] = [
            0_i32, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, -4, -8, -12, -16,
        ];
        for offset in DESCRIPTOR_OFFSETS {
            let Some(addr) = ptr.checked_add_signed(offset) else {
                continue;
            };
            if let Some(text) = self.shadow_string_at(addr) {
                self.trace_text_descriptor_hit(ptr, offset, addr, &text, "shadow");
                return Ok(text);
            }
            if let Ok(Value::Ptr(nested)) = self.read_value(addr, 2) {
                if nested != 0 {
                    if let Some(text) = self.shadow_string_at(nested) {
                        self.trace_text_descriptor_hit(ptr, offset, nested, &text, "nested_shadow");
                        return Ok(text);
                    }
                    if let Ok(text) = self.read_c_string(nested) {
                        if is_plausible_text_payload(&text) {
                            self.trace_text_descriptor_hit(
                                ptr,
                                offset,
                                nested,
                                &text,
                                "nested_cstr",
                            );
                            return Ok(text);
                        }
                    }
                }
            }
            if let Ok(nested) = self.read_int(addr, 2) {
                if nested != 0 {
                    if let Some(text) = self.shadow_string_at(nested) {
                        self.trace_text_descriptor_hit(
                            ptr,
                            offset,
                            nested,
                            &text,
                            "nested_int_shadow",
                        );
                        return Ok(text);
                    }
                    if let Ok(text) = self.read_c_string(nested) {
                        if is_plausible_text_payload(&text) {
                            self.trace_text_descriptor_hit(
                                ptr,
                                offset,
                                nested,
                                &text,
                                "nested_int_cstr",
                            );
                            return Ok(text);
                        }
                    }
                }
            }
        }
        if is_damaged_text_payload(&direct) {
            return Ok(direct);
        }
        // Descriptor fields take precedence over raw byte scanning. Otherwise
        // an invalid leading conversion such as "&#65533;" can expose a
        // plausible-looking suffix at +4 before a valid nested pointer at +8.
        for offset in DESCRIPTOR_OFFSETS {
            let Some(addr) = ptr.checked_add_signed(offset) else {
                continue;
            };
            if let Ok(text) = self.read_c_string(addr) {
                if is_plausible_text_payload(&text) {
                    self.trace_text_descriptor_hit(ptr, offset, addr, &text, "cstr");
                    return Ok(text);
                }
            }
        }
        Ok(direct)
    }

    fn shadow_string_at(&self, addr: u32) -> Option<String> {
        self.mem_values
            .get(&Self::value_key(addr))
            .and_then(|value| match value {
                Value::Str(text) if is_plausible_text_payload(text) => Some(text.clone()),
                _ => None,
            })
    }

    fn trace_text_descriptor_hit(
        &self,
        ptr: u32,
        offset: i32,
        addr: u32,
        text: &str,
        source: &'static str,
    ) {
        if std::env::var_os("TRACE_TEXT_ARGS").is_some() {
            tracing::debug!(
                ptr = format_args!("0x{ptr:08X}"),
                offset,
                addr = format_args!("0x{addr:08X}"),
                source,
                text,
                "TextDescriptorString"
            );
        }
    }

    fn value_as_sound_file_string(&self, value: Value) -> VmResult<String> {
        let direct = self.value_as_string_lossy(value.clone())?;
        if !direct.starts_with("0x") {
            return Ok(direct);
        }
        let ptr = match value {
            Value::Int(value) => value as u32,
            Value::Ptr(ptr) => ptr,
            _ => return Ok(direct),
        };
        for offset in [0x5c_u32, 0x60, 0x58] {
            let addr = ptr.saturating_add(offset);
            if let Some(Value::Str(text)) = self.mem_values.get(&Self::value_key(addr)) {
                if !text.is_empty() {
                    tracing::debug!(
                        ptr = format_args!("0x{ptr:08X}"),
                        offset = format_args!("0x{offset:X}"),
                        text,
                        "SoundDescriptorFileString"
                    );
                    return Ok(text.clone());
                }
            }
            if let Ok(text) = self.read_c_string(addr) {
                if !text.is_empty()
                    && text.len() <= 64
                    && text
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
                {
                    tracing::debug!(
                        ptr = format_args!("0x{ptr:08X}"),
                        offset = format_args!("0x{offset:X}"),
                        text,
                        "SoundDescriptorFileString"
                    );
                    return Ok(text);
                }
            }
        }
        Ok(direct)
    }

    fn write_system_time(&mut self, ptr: u32) -> VmResult<()> {
        let range = self.resolve_write_range(ptr, 16)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let seconds = now.as_secs();
        let days = (seconds / 86_400) as i64;
        let (year, month, day) = civil_from_days(days);
        let millis = now.subsec_millis() as u16;
        let fields = [
            year as u16,
            month as u16,
            ((seconds / 86_400 + 4) % 7) as u16,
            day as u16,
            ((seconds / 3_600) % 24) as u16,
            ((seconds / 60) % 60) as u16,
            (seconds % 60) as u16,
            millis,
        ];
        for (idx, field) in fields.into_iter().enumerate() {
            self.memory[range.start + idx * 2..range.start + idx * 2 + 2]
                .copy_from_slice(&field.to_le_bytes());
        }
        Ok(())
    }

    fn pop_string_lossy(&mut self) -> VmResult<String> {
        match self.pop_value()? {
            Value::Str(text) => Ok(text),
            Value::Ptr(ptr) => self.read_c_string(ptr),
            Value::Int(value) => {
                if let Some(Value::Str(text)) = self.mem_values.get(&Self::value_key(value as u32))
                {
                    Ok(text.clone())
                } else {
                    self.read_c_string(value as u32)
                }
            }
            Value::Func { offset, .. } => self.read_c_string(offset),
            Value::Program(_) | Value::None => Ok(String::new()),
        }
    }

    fn render_sprintf(&mut self, fmt: &str) -> String {
        let mut output = String::new();
        let mut chars = fmt.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                output.push(ch);
                continue;
            }
            if chars.peek() == Some(&'%') {
                chars.next();
                output.push('%');
                continue;
            }
            let mut zero_pad = false;
            while matches!(chars.peek(), Some('-' | '+' | ' ' | '#')) {
                chars.next();
            }
            if chars.peek() == Some(&'0') {
                zero_pad = true;
                chars.next();
            }
            let mut width = 0usize;
            if chars.peek() == Some(&'*') {
                width = self.pop_int().unwrap_or_default().max(0) as usize;
                chars.next();
            } else {
                while let Some(digit) = chars.peek().and_then(|ch| ch.to_digit(10)) {
                    width = width.saturating_mul(10).saturating_add(digit as usize);
                    chars.next();
                }
            }
            if chars.peek() == Some(&'.') {
                chars.next();
                if chars.peek() == Some(&'*') {
                    let _precision = self.pop_int().unwrap_or_default();
                    chars.next();
                } else {
                    while chars.peek().and_then(|ch| ch.to_digit(10)).is_some() {
                        chars.next();
                    }
                }
            }
            while matches!(chars.peek(), Some('h' | 'l' | 'j' | 'z' | 't' | 'L')) {
                chars.next();
            }
            let spec = chars.next().unwrap_or('%');
            match spec {
                'd' | 'i' | 'u' | 'x' | 'X' => {
                    let value = self.pop_int().unwrap_or_default();
                    let rendered = if spec == 'x' {
                        format!("{value:x}")
                    } else if spec == 'X' {
                        format!("{value:X}")
                    } else {
                        value.to_string()
                    };
                    if width > rendered.len() {
                        let pad = if zero_pad { '0' } else { ' ' };
                        output.extend(std::iter::repeat_n(pad, width - rendered.len()));
                    }
                    output.push_str(&rendered);
                }
                's' => {
                    let value = self.pop_string_lossy().unwrap_or_default();
                    output.push_str(&value);
                }
                other => {
                    output.push('%');
                    output.push(other);
                }
            }
        }
        output
    }

    fn jump_target(&self, program: &BpProgram, target: u32) -> VmResult<usize> {
        program.labels.get(&target).copied().ok_or_else(|| {
            VmError::Runtime(format!("jump target 0x{target:08X} is not an instruction"))
        })
    }

    fn jump_target_index(&self, program_index: usize, target: u32) -> VmResult<usize> {
        let program = self
            .programs
            .get(program_index)
            .ok_or_else(|| VmError::Runtime(format!("program #{program_index} is not loaded")))?;
        self.jump_target(program, target)
    }

    fn note_call(&mut self, kind: &str, group: u8, id: u16) {
        if !self.collect_diagnostics {
            return;
        }
        let label = known_call_name(group, id).unwrap_or("unknown");
        *self
            .calls
            .entry(format!("{kind}:0x{group:02X}:0x{id:02X}:{label}"))
            .or_default() += 1;
    }

    fn note_stub(&mut self, kind: &str, group: u8, id: u16) {
        *self
            .stubs
            .entry(format!("{kind}:0x{group:02X}:0x{id:02X}"))
            .or_default() += 1;
    }

    fn push_trace(&mut self, line: String) {
        if self.recent_trace.len() >= 30 {
            self.recent_trace.pop_front();
        }
        self.recent_trace.push_back(line);
    }

    fn program_name(&self, program_index: usize) -> &str {
        self.programs
            .get(program_index)
            .and_then(|program| program.script_name.as_deref())
            .unwrap_or("<anonymous>")
    }

    fn stack_summary(&self, count: usize) -> Vec<String> {
        self.stack
            .iter()
            .rev()
            .take(count)
            .map(value_summary)
            .collect()
    }

    pub(crate) fn memory_addr(ptr: u32) -> u32 {
        match ptr >> 24 {
            0x12 | 0x13 => LOCAL_MEMORY_BASE.saturating_add(ptr & ADDRESS_MASK),
            _ => ptr & ADDRESS_MASK,
        }
    }

    pub(crate) fn value_key(ptr: u32) -> u32 {
        Self::memory_addr(ptr)
    }
}

fn strip_native_markup_tags(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        let tag = &rest[start..];
        let valid_tag = tag
            .as_bytes()
            .get(1)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'/');
        if valid_tag {
            if let Some(end) = tag.find('>') {
                rest = &tag[end + 1..];
                continue;
            }
        }
        output.push('<');
        rest = &tag[1..];
    }
    output.push_str(rest);
    output
}

fn extract_native_labels(source: &str) -> Vec<String> {
    let lower = source.to_ascii_lowercase();
    let mut labels = Vec::new();
    let mut offset = 0;
    while let Some(relative_start) = lower[offset..].find("<l>") {
        let start = offset + relative_start + 3;
        let Some(relative_end) = lower[start..].find("</l>") else {
            break;
        };
        let end = start + relative_end;
        if end != start {
            labels.push(source[start..end].to_string());
        }
        offset = end + 4;
    }
    labels
}

fn trace_u32_env(key: &str) -> Option<u64> {
    let value = std::env::var(key).ok()?;
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|hex| u64::from_str_radix(hex, 16).ok())
        .unwrap_or_else(|| value.parse().ok())
}

fn native_return_audit_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ETHORNELL_NATIVE_RETURN_AUDIT").is_some())
}

fn trace_sound_calls_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("TRACE_SOUND_CALLS").is_some())
}

fn trace_call_frames_enabled(trace_id: u64) -> bool {
    static CONFIG: OnceLock<(bool, Option<u64>)> = OnceLock::new();
    let (enabled, filter) = CONFIG.get_or_init(|| {
        (
            std::env::var_os("TRACE_CALL_FRAMES").is_some(),
            std::env::var("TRACE_CALL_FRAMES_VM")
                .ok()
                .and_then(|value| value.parse::<u64>().ok()),
        )
    });
    *enabled && filter.is_none_or(|filter| filter == trace_id)
}

fn trace_vm_branches_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("TRACE_VM_BRANCHES").is_some())
}

fn trace_watch_addresses() -> &'static [u32] {
    static ADDRESSES: OnceLock<Vec<u32>> = OnceLock::new();
    ADDRESSES.get_or_init(|| {
        std::env::var("TRACE_WATCH_ADDR")
            .ok()
            .into_iter()
            .flat_map(|spec| {
                spec.split(',')
                    .filter_map(|part| {
                        let part = part.trim();
                        if part.is_empty() {
                            None
                        } else if let Some(hex) =
                            part.strip_prefix("0x").or_else(|| part.strip_prefix("0X"))
                        {
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            part.parse::<u32>().ok()
                        }
                    })
                    .map(Vm::memory_addr)
                    .collect::<Vec<_>>()
            })
            .collect()
    })
}

fn value_summary(value: &Value) -> String {
    match value {
        Value::Int(value) => format!("Int({value})"),
        Value::Ptr(ptr) => format!("Ptr(0x{ptr:08X})"),
        Value::Str(text) => format!("Str({text:?})"),
        Value::Func {
            program_index,
            offset,
        } => format!("Func(program={program_index}, offset=0x{offset:08X})"),
        Value::Program(program) => {
            let name = program.script_name.as_deref().unwrap_or("<anonymous>");
            format!("Program({name})")
        }
        Value::None => "None".into(),
    }
}

fn is_plausible_text_payload(text: &str) -> bool {
    let trimmed = text.trim_matches('\0');
    if trimmed.is_empty() || trimmed.starts_with("0x") || is_damaged_text_payload(trimmed) {
        return false;
    }
    trimmed
        .chars()
        .any(|ch| !ch.is_control() || ch == '\n' || ch == '\r' || ch == '\t')
}

fn is_damaged_text_payload(text: &str) -> bool {
    text.contains('\u{fffd}') || text.contains("&#65533;") || text.contains("&#xFFFD;")
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    ((y + (m <= 2) as i64) as i32, m as u32, d as u32)
}

fn is_sjis_delimiter(c: u16) -> bool {
    matches!(
        c,
        0x002c
            | 0x002e
            | 0x00a4
            | 0x00a1
            | 0x003a
            | 0x003b
            | 0x003f
            | 0x0021
            | 0x00de
            | 0x00df
            | 0x00a5
            | 0x8141
            | 0x8142
            | 0x8143
            | 0x8144
            | 0x8146
            | 0x8147
            | 0x8148
            | 0x8149
            | 0x814a
            | 0x814b
            | 0x815d
            | 0x005d
            | 0x007d
            | 0x0029
            | 0x816a
            | 0x816c
            | 0x816e
            | 0x8170
            | 0x8172
            | 0x8174
            | 0x8176
            | 0x8178
            | 0x817a
            | 0x8165
            | 0x8167
    )
}

fn read_op_u8(instruction: &BpInstruction) -> u8 {
    instruction.raw.get(1).copied().unwrap_or_default()
}

fn read_op_u32(instruction: &BpInstruction) -> u32 {
    match instruction.operands.first() {
        Some(BpOperand::U8(v)) => *v as u32,
        Some(BpOperand::U16(v)) => *v as u32,
        Some(BpOperand::U32(v)) => *v,
        Some(BpOperand::I32(v)) => *v as u32,
        Some(BpOperand::Offset(v)) => *v,
        _ => 0,
    }
}

fn read_op_i32(instruction: &BpInstruction) -> i32 {
    match instruction.operands.first() {
        Some(BpOperand::U8(v)) => *v as i8 as i32,
        Some(BpOperand::U16(v)) => *v as i16 as i32,
        Some(BpOperand::U32(v)) => *v as i32,
        Some(BpOperand::I32(v)) => *v,
        Some(BpOperand::Offset(v)) => *v as i32,
        _ => 0,
    }
}

#[derive(Debug, Default)]
pub struct TraceApi;

impl SysApi for TraceApi {
    fn call_sys(&mut self, group: u8, id: u16, stack: &mut Vec<Value>) -> VmResult<Value> {
        match (group, id) {
            (0x80, 0x40) => {
                let _file = stack.pop();
                let _archive = stack.pop();
                return Ok(Value::Ptr(0x2f));
            }
            (0x80, 0x44) => {
                for _ in 0..3 {
                    let _arg = stack.pop();
                }
                let _file = stack.pop();
                let _archive = stack.pop();
                return Ok(Value::Ptr(0x30));
            }
            (0x80, 0x41) => {
                let _program = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x34) => {
                let _file = stack.pop();
                let _archive = stack.pop();
                return Ok(Value::Int(0));
            }
            (0x80, 0x35) => {
                let _file = stack.pop();
                let _archive = stack.pop();
                return Ok(Value::Int(-1));
            }
            (0x80, 0x1b) => {
                let _descriptor = stack.pop();
                let _size = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x08) => {
                stack.push(Value::Int(0));
                stack.push(Value::Int(0));
                return Ok(Value::None);
            }
            (0x80, 0x1f) => {
                for _ in 0..6 {
                    let _arg = stack.pop();
                }
                return Ok(Value::None);
            }
            (0x80, 0x60) => {
                let _arg3 = stack.pop();
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x5c) => {
                let _arg3 = stack.pop();
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x62) => {
                let _descriptor = stack.pop();
                let _mode = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x64) => {
                let _enabled = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x46) => {
                return Ok(Value::None);
            }
            (0x80, 0x28) | (0x80, 0x2a) => {
                let _path = stack.pop();
                return Ok(Value::Int(1));
            }
            (0x80, 0x31) => {
                for _ in 0..5 {
                    let _ = stack.pop();
                }
                return Ok(Value::Int(0));
            }
            (0x80, 0x13) => {
                return Ok(Value::Int(1));
            }
            (0x80, 0x33) => {
                let _mode = stack.pop();
                let _key = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x36) | (0x80, 0x37) => {
                let _arg = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x3d) => {
                let _kind = stack.pop();
                let _ptr = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x80) => {
                let _arg = stack.pop();
                return Ok(Value::Int(0));
            }
            (0x80, 0x81) => {
                return Ok(Value::None);
            }
            (0x80, 0x82) | (0x80, 0x83) => {
                let _arg3 = stack.pop();
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x84) => {
                let _value = stack.pop();
                return Ok(Value::Int(1));
            }
            (0x80, 0x98) => {
                let _arg3 = stack.pop();
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x9d) => {
                let _arg3 = stack.pop();
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x99) => {
                let _handle = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0xa8) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0xa1) => {
                let _arg = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0xac) => {
                let _descriptor = stack.pop();
                let _count = stack.pop();
                let _object = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0xd0) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::Int(1));
            }
            (0x80, 0xd2) => {
                let _arg3 = stack.pop();
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0xda) => {
                let _arg3 = stack.pop();
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0xc5) => {
                let _dst = stack.pop();
                let _src = stack.pop();
                return Ok(Value::Int(1));
            }
            (0x80, 0x5a) => {
                return Ok(Value::None);
            }
            (0x80, 0x61) => {
                return Ok(Value::Int(0));
            }
            (0x80, 0x50) => {
                let _enabled = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0x58)
            | (0x80, 0x67)
            | (0x80, 0x68)
            | (0x80, 0x70)
            | (0x80, 0x74)
            | (0x80, 0xc1)
            | (0x80, 0xaf)
            | (0x81, 0x0e)
            | (0x81, 0x18)
            | (0x81, 0x62)
            | (0x81, 0x63)
            | (0x81, 0x6f) => {
                let _arg = stack.pop();
                return Ok(Value::None);
            }
            (0x81, 0x64) => {
                let _height = stack.pop();
                let _width = stack.pop();
                return Ok(Value::None);
            }
            (0x81, 0x35) => {
                let _file = stack.pop();
                let _archive = stack.pop();
                return Ok(Value::Int(1));
            }
            (0x81, 0x30) => {
                for _ in 0..5 {
                    let _ = stack.pop();
                }
                return Ok(Value::Int(0));
            }
            (0x80, 0xe8) => {
                let _ptr = stack.pop();
                return Ok(Value::None);
            }
            (0x80, 0xfd) => {
                return Ok(Value::Int(0));
            }
            _ => {}
        }
        Ok(Value::None)
    }
}

impl GraphApi for TraceApi {
    fn call_graph(&mut self, group: u8, id: u16, stack: &mut Vec<Value>) -> VmResult<Value> {
        match (group, id) {
            (0x90, 0x06) => {
                let _y = stack.pop();
                let _x = stack.pop();
            }
            (0x90, 0x0c) | (0x90, 0x4c) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0x05) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0x13) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0x16) => {
                let _source = stack.pop();
                let _target = stack.pop();
                return Ok(Value::Int(0));
            }
            (0x90, 0x17) => {
                let _mode = stack.pop();
                let value = stack.pop().unwrap_or(Value::Int(0));
                return Ok(Value::Int(i32::from(value.as_i32() != 0)));
            }
            (0x90, 0x11) => {
                for _ in 0..4 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x10) => {
                for _ in 0..3 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x1f) => {
                // funcs_48065E[0x1f] (sub_47A590) pops exactly six values.
                for _ in 0..6 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x18) => {
                for _ in 0..6 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x1e) => {
                for _ in 0..8 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x20) => {
                for _ in 0..5 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x22) => {
                for _ in 0..7 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x23) => {
                for _ in 0..10 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x32) => {
                let _value = stack.pop();
                let _object = stack.pop();
            }
            (0x90, 0x3c) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0x3d) => {
                let value = stack.pop().unwrap_or(Value::Int(0));
                return Ok(Value::Int(i32::from(value.as_i32() != 0)));
            }
            (0x90, 0x30) => {
                let _enabled = stack.pop();
                let _timeline = stack.pop();
            }
            (0x90, 0x31) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0x50) => {
                return Ok(Value::Int(1));
            }
            (0x90, 0x53) => {
                for _ in 0..8 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x54) => {
                let _enabled = stack.pop();
                let _node = stack.pop();
            }
            (0x90, 0x55) => {
                let _value = stack.pop();
                let _target = stack.pop();
            }
            (0x90, 0x56) => {
                for _ in 0..7 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x58) => {
                for _ in 0..9 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x5a) => {
                for _ in 0..10 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x5c) => {
                for _ in 0..17 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x5d) => {
                for _ in 0..12 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x60) => {
                return Ok(Value::Int(2));
            }
            (0x90, 0x61) => {
                let _object = stack.pop();
            }
            (0x90, 0x64) => {
                let _enabled = stack.pop();
                let _object = stack.pop();
            }
            (0x90, 0x65) => {
                for _ in 0..4 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x66) => {
                for _ in 0..7 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x80) => {
                let _height = stack.pop();
                let _width = stack.pop();
                return Ok(Value::Int(3));
            }
            (0x90, 0x83) => {
                let _surface = stack.pop();
                let _buffer = stack.pop();
            }
            (0x90, 0x84) => {
                let _enabled = stack.pop();
                let _surface = stack.pop();
            }
            (0x90, 0x85) => {
                for _ in 0..7 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x86) => {
                for _ in 0..4 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x87) => {
                let _value = stack.pop();
                let _surface = stack.pop();
            }
            (0x90, 0x88) => {
                for _ in 0..5 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x89) => {
                for _ in 0..12 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x90) => {
                for _ in 0..6 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x00)
            | (0x90, 0x01)
            | (0x90, 0x02)
            | (0x90, 0x03)
            | (0x90, 0x07)
            | (0x90, 0x08)
            | (0x90, 0x0d)
            | (0x90, 0x12)
            | (0x91, 0x0d) => {
                let _arg = stack.pop();
            }
            (0x90, 0x94) | (0x90, 0x9c) | (0x90, 0x9f) | (0x90, 0xaf) => {
                let _arg = stack.pop();
            }
            (0x90, 0x96) | (0x90, 0x97) | (0x91, 0x06) => {
                let _y = stack.pop();
                let _x = stack.pop();
            }
            (0x91, 0x89) => {
                let _value = stack.pop();
                let _target = stack.pop();
            }
            (0x91, 0x94) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x91, 0x9b) => {
                let _flags = stack.pop();
                let _style = stack.pop();
                let _scale = stack.pop();
                let font_size = stack.pop().map(|value| value.as_i32()).unwrap_or(24);
                let max_width = stack.pop().map(|value| value.as_i32()).unwrap_or(0);
                let text = stack.pop().unwrap_or(Value::None);
                let _dest = stack.pop();
                let measured = measure_text_width_value(&text, font_size.max(1), max_width);
                return Ok(Value::Int(measured));
            }
            (0x90, 0x95) => {
                let _b = stack.pop();
                let _a = stack.pop();
            }
            (0x90, 0xf6) => {
                for _ in 0..5 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0xb7) => {
                let _state = stack.pop();
                let _surface = stack.pop();
            }
            (0x90, 0xb6) => {
                let _descriptor = stack.pop();
                let _object = stack.pop();
            }
            (0x90, 0xb8) => {
                let _object = stack.pop();
            }
            (0x90, 0xb9) => {
                let _object = stack.pop();
            }
            (0x90, 0xba) => {
                let _descriptor = stack.pop();
                let _object = stack.pop();
            }
            (0x90, 0xbe) => {
                let _dest = stack.pop();
                let _object = stack.pop();
                return Ok(Value::Int(-1));
            }
            (0x90, 0xd0) => {
                let _target = stack.pop();
            }
            (0x90, 0xd1) => {
                let _target = stack.pop();
            }
            (0x90, 0xd4) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0xd5) | (0x90, 0xd6) | (0x90, 0xd8) => {
                let _arg3 = stack.pop();
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0xd7) => {
                let _handle = stack.pop();
                stack.push(Value::Int(0));
                stack.push(Value::Int(0));
            }
            (0x90, 0xd9) => {
                let _arg3 = stack.pop();
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0xbc) => {
                let _object = stack.pop();
                let _state_buffer = stack.pop();
            }
            (0x90, 0xbf) => {
                let _object = stack.pop();
                let _event_buffer = stack.pop();
            }
            (0x90, 0xcc) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0xcd) => {
                for _ in 0..8 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0xe0) => {
                return Ok(Value::Int(4));
            }
            (0x90, 0xe1) => {
                let _timeline = stack.pop();
                return Ok(Value::Int(0));
            }
            (0x90, 0xe4) => {
                let _enabled = stack.pop();
                let _timeline = stack.pop();
            }
            (0x90, 0xe5) => {
                for _ in 0..4 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0xe8) => {
                for _ in 0..4 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0xe9) => {
                let _value = stack.pop();
                let _timeline = stack.pop();
                return Ok(Value::Int(0));
            }
            (0x91, 0x0e) => {
                for _ in 0..5 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x10) | (0x91, 0x13) | (0x91, 0x15) => {
                for _ in 0..5 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x11) => {
                let _value = stack.pop();
                let _target = stack.pop();
            }
            (0x91, 0x12) => {
                for _ in 0..6 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x16) => {
                for _ in 0..7 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x3e) => {
                let _output = stack.pop();
                let _mode = stack.pop();
                let _x = stack.pop();
                let _layer = stack.pop();
                return Ok(Value::None);
            }
            (0x91, 0x40) => {
                for _ in 0..9 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x38) => {
                let _mode = stack.pop();
                let _layer = stack.pop();
                let _buffer = stack.pop();
            }
            (0x91, 0x55) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x91, 0x60) | (0x91, 0x61) => {
                let _handle = stack.pop();
            }
            (0x91, 0x64) => {
                let _enabled = stack.pop();
                let _handle = stack.pop();
            }
            (0x91, 0x65) => {
                for _ in 0..6 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x66) => {
                for _ in 0..4 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x1f) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x91, 0x1d) => {
                for _ in 0..8 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x33) => {
                for _ in 0..4 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x19) => {
                for _ in 0..11 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x98) => {
                for _ in 0..6 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x9c) => {
                for _ in 0..14 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x9f) => while stack.pop().is_some() {},
            (0x92, 0x97) => {
                for _ in 0..7 {
                    let _arg = stack.pop();
                }
            }
            (0x92, 0x90) => {
                for _ in 0..15 {
                    let _arg = stack.pop();
                }
            }
            (0x92, 0x88) => {
                let _value = stack.pop();
                let _surface = stack.pop();
            }
            (0x92, 0x14) => {
                for _ in 0..2 {
                    let _arg = stack.pop();
                }
            }
            (0x92, 0x15) => {}
            (0x92, 0x16) => {
                let _resource = stack.pop();
                let _target = stack.pop();
                return Ok(Value::Int(1));
            }
            (0x92, 0x19) => {
                let _target = stack.pop();
            }
            (0x92, 0x91) => {
                for _ in 0..5 {
                    let _arg = stack.pop();
                }
            }
            (0x92, 0x8e) => {
                let _target = stack.pop();
            }
            (0x90, 0x0e) => {
                for _ in 0..5 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x9a) => {
                let _value = stack.pop();
                let _property = stack.pop();
            }
            (0x91, 0x8d) => {
                let _surface = stack.pop();
                stack.push(Value::Int(0));
                stack.push(Value::Int(0));
                return Ok(Value::Int(0));
            }
            (0x91, 0x8e) => {
                let _target = stack.pop();
                return Ok(Value::Int(0));
            }
            (0x91, 0xb8) => {
                let _layer = stack.pop();
                return Ok(Value::Int(0));
            }
            (0x91, 0xba) => {
                let _dest = stack.pop();
                let layer = stack.pop().unwrap_or(Value::Int(0));
                return Ok(Value::Int(layer.as_i32()));
            }
            (0x91, 0xf1) => {
                let _handle = stack.pop();
                let _duration_or_dest = stack.pop();
            }
            (0x91, 0xf2) | (0x91, 0xf6) => {
                let _handle = stack.pop();
            }
            (0x92, 0xf1) => {
                for _ in 0..6 {
                    let _arg = stack.pop();
                }
                return Ok(Value::Int(0));
            }
            (0x92, 0xf2) => {
                for _ in 0..13 {
                    let _arg = stack.pop();
                }
                return Ok(Value::Int(0));
            }
            (0x92, 0xf4) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
                return Ok(Value::Int(1));
            }
            _ => {}
        }
        Ok(Value::None)
    }
}

impl SoundApi for TraceApi {
    fn call_sound(&mut self, group: u8, id: u16, stack: &mut Vec<Value>) -> VmResult<Value> {
        match (group, id) {
            (0xa0, 0x11) => {
                for _ in 0..5 {
                    let _arg = stack.pop();
                }
            }
            (0xa0, 0x14) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0xa0, 0x16) => {
                let _duration = stack.pop();
                let _volume = stack.pop();
                let _channel = stack.pop();
            }
            (0xa0, 0x20) => {
                let _file = stack.pop();
                let _archive = stack.pop();
                let _slot = stack.pop();
            }
            (0xa0, 0x21) => {
                for _ in 0..6 {
                    let _arg = stack.pop();
                }
            }
            (0xa0, 0x22) => {
                let _channel = stack.pop();
            }
            (0xa0, 0x24) => {
                let _fade = stack.pop();
                let _volume = stack.pop();
                let _slot = stack.pop();
                return Ok(Value::Int(0));
            }
            (0xa0, 0x25) => {
                let _channel = stack.pop();
            }
            (0xa0, 0x26) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0xa0, 0x08) | (0xa0, 0x09) => {
                let _value = stack.pop();
                let _channel = stack.pop();
            }
            _ => {}
        }
        Ok(Value::None)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        empty_loaded_program, strip_native_markup_tags, GraphApi, GraphProcedureCompletion,
        GraphProcedureSchedule, PendingGraphProcedure, SoundApi, SysApi, TraceApi, Value, Vm,
        VmRunOptions, VmStopReason,
    };
    use ethornell_script::{BpInstruction, BpOpcode, BpOperand, BpProgram};
    use std::sync::Arc;

    #[derive(Default)]
    struct SchedulingApi {
        schedule: Option<GraphProcedureSchedule>,
        system_events: std::collections::VecDeque<[i32; 3]>,
        bitmap_dimensions: std::collections::BTreeMap<i32, (u32, u32)>,
        bitmap_pixels: std::collections::BTreeMap<i32, Vec<u8>>,
        input_class_state: i32,
        last_input_scope: Option<i32>,
    }

    impl SysApi for SchedulingApi {
        fn call_sys(
            &mut self,
            _group: u8,
            _id: u16,
            _stack: &mut Vec<Value>,
        ) -> super::VmResult<Value> {
            Ok(Value::None)
        }

        fn post_queued_event(&mut self, code: i32, parameter: i32) {
            self.system_events.push_back([0, code, parameter]);
        }

        fn poll_queued_event(&mut self) -> Option<[i32; 3]> {
            self.system_events.pop_front()
        }

        fn query_input_class_state(&mut self, scope: i32) -> i32 {
            self.last_input_scope = Some(scope);
            self.input_class_state
        }
    }

    impl GraphApi for SchedulingApi {
        fn create_bitmap_from_rgb(
            &mut self,
            bitmap: i32,
            _width: i32,
            _height: i32,
            _format: i32,
            pixels: &[u8],
        ) -> bool {
            self.bitmap_pixels.insert(bitmap, pixels.to_vec());
            true
        }

        fn read_bitmap_pixels(&mut self, bitmap: i32, capacity: usize) -> Option<Vec<u8>> {
            self.bitmap_pixels
                .get(&bitmap)
                .filter(|pixels| pixels.len() <= capacity)
                .cloned()
        }

        fn call_graph(
            &mut self,
            _group: u8,
            _id: u16,
            stack: &mut Vec<Value>,
        ) -> super::VmResult<Value> {
            let _object = stack.pop();
            self.schedule = Some(GraphProcedureSchedule {
                duration_ms: 1,
                input_enabled: false,
                input_descriptor: 0,
                wait_for_input: false,
                completion: GraphProcedureCompletion::None,
            });
            Ok(Value::None)
        }

        fn take_graph_procedure_schedule(&mut self) -> Option<GraphProcedureSchedule> {
            self.schedule.take()
        }

        fn query_bitmap_info(&mut self, bitmap: i32) -> Option<super::BitmapInfo> {
            if let Some(&(width, height)) = self.bitmap_dimensions.get(&bitmap) {
                return Some(super::BitmapInfo {
                    width,
                    height,
                    format: 2,
                });
            }
            (bitmap == 3792).then_some(super::BitmapInfo {
                width: 128,
                height: 32,
                format: 2,
            })
        }

        fn set_bitmap_dimensions(&mut self, bitmap: i32, width: i32, height: i32) -> bool {
            if !(0..0x4000).contains(&bitmap) {
                return false;
            }
            self.bitmap_dimensions
                .insert(bitmap, (width as u32, height as u32));
            true
        }
    }

    impl SoundApi for SchedulingApi {
        fn call_sound(
            &mut self,
            _group: u8,
            _id: u16,
            _stack: &mut Vec<Value>,
        ) -> super::VmResult<Value> {
            Ok(Value::None)
        }
    }

    struct MediationApi {
        program: BpProgram,
    }

    impl SysApi for MediationApi {
        fn call_sys(
            &mut self,
            _group: u8,
            _id: u16,
            _stack: &mut Vec<Value>,
        ) -> super::VmResult<Value> {
            Ok(Value::None)
        }

        fn load_program(&mut self, _archive: &str, _file: &str) -> Option<BpProgram> {
            Some(self.program.clone())
        }
    }

    impl GraphApi for MediationApi {
        fn call_graph(
            &mut self,
            _group: u8,
            _id: u16,
            _stack: &mut Vec<Value>,
        ) -> super::VmResult<Value> {
            Ok(Value::None)
        }
    }

    impl SoundApi for MediationApi {
        fn call_sound(
            &mut self,
            _group: u8,
            _id: u16,
            _stack: &mut Vec<Value>,
        ) -> super::VmResult<Value> {
            Ok(Value::None)
        }
    }

    fn test_instruction(
        offset: u64,
        code: u8,
        name: &'static str,
        raw: Vec<u8>,
        operands: Vec<BpOperand>,
    ) -> BpInstruction {
        BpInstruction {
            offset,
            opcode: BpOpcode::Known { code, name },
            opcode_hex: format!("0x{code:02X}"),
            opcode_name: name.into(),
            operands,
            known_call: None,
            raw,
            warning: None,
        }
    }

    #[test]
    fn native_move_returns_the_assigned_value_and_inline_copy_writes_payload() {
        let destination = 0x2400u32;
        let program = BpProgram {
            script_name: Some("native-memory-opcodes".into()),
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![
                test_instruction(
                    0x10,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(destination)],
                ),
                test_instruction(
                    0x15,
                    0x01,
                    "push_word",
                    vec![0x01, 0x34, 0x12],
                    vec![BpOperand::U16(0x1234)],
                ),
                test_instruction(0x18, 0x09, "move", vec![0x09, 2], vec![BpOperand::U8(2)]),
                test_instruction(
                    0x1a,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(destination + 4)],
                ),
                test_instruction(
                    0x1f,
                    0x0b,
                    "copy_inline",
                    vec![0x0b, 3, 0xaa, 0xbb, 0xcc],
                    vec![BpOperand::Raw(vec![0xaa, 0xbb, 0xcc])],
                ),
                test_instruction(0x24, 0x17, "ret", vec![0x17], Vec::new()),
            ],
            labels: Default::default(),
            warnings: Vec::new(),
        };
        let mut api = SchedulingApi::default();
        let mut vm = Vm::new();

        let report = vm.run(&program, &mut api, &VmRunOptions::default());

        assert_eq!(report.stop_reason, VmStopReason::Completed);
        assert_eq!(vm.read_int(destination, 2).unwrap(), 0x1234);
        assert_eq!(vm.read_int(destination + 4, 0).unwrap(), 0xaa);
        assert_eq!(vm.read_int(destination + 5, 0).unwrap(), 0xbb);
        assert_eq!(vm.read_int(destination + 6, 0).unwrap(), 0xcc);
        assert_eq!(vm.stack, [Value::Int(0x1234)]);
    }

    #[test]
    fn raw_native_write_replaces_a_stale_pointer_shadow() {
        let mut vm = Vm::new();
        let destination = 0x2480;
        vm.write_value(destination, 2, &Value::Ptr(0x1200_4321))
            .unwrap();
        assert_eq!(
            vm.read_value(destination, 2).unwrap(),
            Value::Ptr(0x1200_4321)
        );

        vm.write_int(destination, 2, 10_466).unwrap();

        assert_eq!(vm.read_value(destination, 2).unwrap(), Value::Int(10_466));
    }

    #[test]
    fn bitmap_query_writes_the_native_six_dword_record() {
        let destination = 0x2600u32;
        let program = BpProgram {
            script_name: Some("bitmap-info-record".into()),
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![
                test_instruction(
                    0x10,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(destination)],
                ),
                test_instruction(
                    0x15,
                    0x01,
                    "push_word",
                    vec![0x01],
                    vec![BpOperand::U16(3792)],
                ),
                test_instruction(0x18, 0x90, "grp1", vec![0x90, 0x16], Vec::new()),
                test_instruction(0x1a, 0x17, "ret", vec![0x17], Vec::new()),
            ],
            labels: Default::default(),
            warnings: Vec::new(),
        };
        let mut api = SchedulingApi::default();
        let mut vm = Vm::new();

        let report = vm.run(&program, &mut api, &VmRunOptions::default());

        assert_eq!(report.stop_reason, VmStopReason::Completed);
        assert_eq!(vm.read_int(destination, 2).unwrap(), 0);
        assert_eq!(vm.read_int(destination + 8, 2).unwrap(), 128);
        assert_eq!(vm.read_int(destination + 12, 2).unwrap(), 32);
        assert_eq!(vm.read_int(destination + 16, 2).unwrap(), 2);
        assert_eq!(vm.stack, [Value::Int(1)]);
    }

    #[test]
    fn bitmap_dimension_metadata_round_trips_through_group_92() {
        let destination = 0x2680u32;
        let bitmap = 43u16;
        let program = BpProgram {
            script_name: Some("bitmap-dimension-metadata".into()),
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![
                test_instruction(
                    0x10,
                    0x01,
                    "push_word",
                    vec![0x01],
                    vec![BpOperand::U16(bitmap)],
                ),
                test_instruction(
                    0x13,
                    0x01,
                    "push_word",
                    vec![0x01],
                    vec![BpOperand::U16(640)],
                ),
                test_instruction(
                    0x16,
                    0x01,
                    "push_word",
                    vec![0x01],
                    vec![BpOperand::U16(360)],
                ),
                test_instruction(0x19, 0x92, "grp3", vec![0x92, 0x12], Vec::new()),
                test_instruction(
                    0x1b,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(destination)],
                ),
                test_instruction(
                    0x20,
                    0x01,
                    "push_word",
                    vec![0x01],
                    vec![BpOperand::U16(bitmap)],
                ),
                test_instruction(0x23, 0x92, "grp3", vec![0x92, 0x16], Vec::new()),
                test_instruction(0x25, 0x17, "ret", vec![0x17], Vec::new()),
            ],
            labels: Default::default(),
            warnings: Vec::new(),
        };
        let mut api = SchedulingApi::default();
        let mut vm = Vm::new();

        let report = vm.run(&program, &mut api, &VmRunOptions::default());

        assert_eq!(report.stop_reason, VmStopReason::Completed);
        assert_eq!(vm.read_int(destination, 2).unwrap(), 640);
        assert_eq!(vm.read_int(destination + 4, 2).unwrap(), 360);
        assert_eq!(vm.stack, [Value::Int(1), Value::Int(1)]);
    }

    #[test]
    fn bitmap_rgb_syscalls_round_trip_through_vm_memory() {
        let source = 0x2800u32;
        let destination = 0x2900u32;
        let written = 0x2a00u32;
        let pixels = [10, 20, 30, 40, 50, 60];
        let program = BpProgram {
            script_name: Some("bitmap-rgb-roundtrip".into()),
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![
                test_instruction(0x10, 0x01, "push_word", vec![0x01], vec![BpOperand::U16(7)]),
                test_instruction(0x13, 0x01, "push_word", vec![0x01], vec![BpOperand::U16(2)]),
                test_instruction(0x16, 0x01, "push_word", vec![0x01], vec![BpOperand::U16(1)]),
                test_instruction(0x19, 0x01, "push_word", vec![0x01], vec![BpOperand::U16(1)]),
                test_instruction(
                    0x1c,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(source)],
                ),
                test_instruction(0x21, 0x90, "grp1", vec![0x90, 0x14], Vec::new()),
                test_instruction(
                    0x23,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(destination)],
                ),
                test_instruction(
                    0x28,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(written)],
                ),
                test_instruction(0x2d, 0x01, "push_word", vec![0x01], vec![BpOperand::U16(6)]),
                test_instruction(0x30, 0x01, "push_word", vec![0x01], vec![BpOperand::U16(7)]),
                test_instruction(0x33, 0x90, "grp1", vec![0x90, 0x15], Vec::new()),
                test_instruction(0x35, 0x17, "ret", vec![0x17], Vec::new()),
            ],
            labels: Default::default(),
            warnings: Vec::new(),
        };
        let mut api = SchedulingApi::default();
        let mut vm = Vm::new();
        vm.resolve_write_range(source, pixels.len())
            .map(|range| vm.memory[range].copy_from_slice(&pixels))
            .unwrap();

        let report = vm.run(&program, &mut api, &VmRunOptions::default());

        assert_eq!(report.stop_reason, VmStopReason::Completed);
        assert_eq!(vm.read_int(written, 2).unwrap(), pixels.len() as u32);
        let range = vm.resolve_range(destination, pixels.len()).unwrap();
        assert_eq!(&vm.memory[range], &pixels);
    }

    #[test]
    fn expanding_the_local_base_zeroes_a_reused_frame() {
        let program = BpProgram {
            script_name: Some("zero-reused-frame".into()),
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![
                test_instruction(
                    0x10,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(0x120)],
                ),
                test_instruction(0x15, 0x11, "store_base", vec![0x11], Vec::new()),
            ],
            labels: Default::default(),
            warnings: Vec::new(),
        };
        let mut api = SchedulingApi::default();
        let mut vm = Vm::new();
        vm.start(&program);
        vm.mem_ptr = 0x100;
        vm.write_value(0x1200_0118, 2, &Value::Ptr(0x1000_1234))
            .unwrap();

        let report = vm.run_loaded(&mut api, &VmRunOptions::default());

        assert_eq!(report.stop_reason, VmStopReason::Completed);
        assert_eq!(vm.read_int(0x1200_0118, 2).unwrap(), 0);
        assert!(!vm.mem_values.contains_key(&Vm::value_key(0x1200_0118)));
    }

    #[test]
    fn native_heap_reuses_and_coalesces_freed_blocks() {
        let mut vm = Vm::new();
        let first = vm.alloc_heap(64);
        let second = vm.alloc_heap(96);
        assert!(vm.free_heap(first));
        assert!(vm.free_heap(second));

        let combined = vm.alloc_heap(160);

        assert_eq!(combined, first);
        assert!(!vm.free_heap(second));
        assert!(vm.free_heap(combined));
    }

    #[test]
    fn native_markup_strip_matches_sub_438070() {
        assert_eq!(
            strip_native_markup_tags("序章<Ruby>本文</Ruby><1>保持"),
            "序章本文<1>保持"
        );
        assert_eq!(strip_native_markup_tags("未閉合<Tag"), "未閉合<Tag");
    }

    #[test]
    fn mediation_program_registration_and_call_preserve_the_native_stack() {
        let mediation = BpProgram {
            script_name: Some("loadbmpdx._bp".into()),
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![
                test_instruction(0x10, 0x10, "load_base", vec![0x10], Vec::new()),
                test_instruction(
                    0x11,
                    0x01,
                    "push_word",
                    vec![0x01, 12, 0],
                    vec![BpOperand::U16(12)],
                ),
                test_instruction(0x14, 0x20, "add", vec![0x20], Vec::new()),
                test_instruction(0x15, 0x11, "store_base", vec![0x11], Vec::new()),
                test_instruction(
                    0x16,
                    0x04,
                    "push_base_offset",
                    vec![0x04, 4, 0],
                    vec![BpOperand::U16(4)],
                ),
                test_instruction(
                    0x19,
                    0x0a,
                    "move_arg",
                    vec![0x0a, 2],
                    vec![BpOperand::U8(2)],
                ),
                test_instruction(
                    0x1b,
                    0x04,
                    "push_base_offset",
                    vec![0x04, 8, 0],
                    vec![BpOperand::U16(8)],
                ),
                test_instruction(
                    0x1e,
                    0x0a,
                    "move_arg",
                    vec![0x0a, 2],
                    vec![BpOperand::U8(2)],
                ),
                test_instruction(
                    0x20,
                    0x04,
                    "push_base_offset",
                    vec![0x04, 12, 0],
                    vec![BpOperand::U16(12)],
                ),
                test_instruction(
                    0x23,
                    0x0a,
                    "move_arg",
                    vec![0x0a, 2],
                    vec![BpOperand::U8(2)],
                ),
                test_instruction(
                    0x25,
                    0x04,
                    "push_base_offset",
                    vec![0x04, 12, 0],
                    vec![BpOperand::U16(12)],
                ),
                test_instruction(0x28, 0x08, "load", vec![0x08, 2], vec![BpOperand::U8(2)]),
                test_instruction(0x2a, 0x10, "load_base", vec![0x10], Vec::new()),
                test_instruction(
                    0x2b,
                    0x01,
                    "push_word",
                    vec![0x01, 12, 0],
                    vec![BpOperand::U16(12)],
                ),
                test_instruction(0x2e, 0x21, "sub", vec![0x21], Vec::new()),
                test_instruction(0x2f, 0x11, "store_base", vec![0x11], Vec::new()),
                test_instruction(0x30, 0x17, "ret", vec![0x17], Vec::new()),
            ],
            labels: std::iter::once((0x10, 0)).collect(),
            warnings: Vec::new(),
        };
        let main = BpProgram {
            script_name: Some("mediation-main".into()),
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![
                test_instruction(0x10, 0x10, "load_base", vec![0x10], Vec::new()),
                test_instruction(
                    0x11,
                    0x00,
                    "push_byte",
                    vec![0x00, 4],
                    vec![BpOperand::U8(4)],
                ),
                test_instruction(0x13, 0x20, "add", vec![0x20], Vec::new()),
                test_instruction(0x14, 0x11, "store_base", vec![0x11], Vec::new()),
                test_instruction(
                    0x15,
                    0x00,
                    "push_byte",
                    vec![0x00, 0x40],
                    vec![BpOperand::U8(0x40)],
                ),
                test_instruction(
                    0x17,
                    0x05,
                    "push_string",
                    vec![0x05],
                    vec![BpOperand::String("sysprg.arc".into())],
                ),
                test_instruction(
                    0x1a,
                    0x05,
                    "push_string",
                    vec![0x05],
                    vec![BpOperand::String("loadbmpdx._bp".into())],
                ),
                test_instruction(
                    0x1d,
                    0xff,
                    "script_load",
                    vec![0xff, 0xf0],
                    vec![BpOperand::U8(0xf0)],
                ),
                test_instruction(
                    0x1f,
                    0x01,
                    "push_word",
                    vec![0x01, 9, 3],
                    vec![BpOperand::U16(777)],
                ),
                test_instruction(
                    0x22,
                    0x05,
                    "push_string",
                    vec![0x05],
                    vec![BpOperand::String("data02xxx.arc".into())],
                ),
                test_instruction(
                    0x25,
                    0x05,
                    "push_string",
                    vec![0x05],
                    vec![BpOperand::String("bg50d_a".into())],
                ),
                test_instruction(
                    0x28,
                    0xff,
                    "script_call",
                    vec![0xff, 0x40],
                    vec![BpOperand::U8(0x40)],
                ),
                test_instruction(0x2a, 0x10, "load_base", vec![0x10], Vec::new()),
                test_instruction(
                    0x2b,
                    0x00,
                    "push_byte",
                    vec![0x00, 4],
                    vec![BpOperand::U8(4)],
                ),
                test_instruction(0x2d, 0x21, "sub", vec![0x21], Vec::new()),
                test_instruction(0x2e, 0x11, "store_base", vec![0x11], Vec::new()),
                test_instruction(0x2f, 0x17, "ret", vec![0x17], Vec::new()),
            ],
            labels: Default::default(),
            warnings: Vec::new(),
        };
        let mut api = MediationApi { program: mediation };
        let mut vm = Vm::new();

        let report = vm.run(
            &main,
            &mut api,
            &VmRunOptions {
                collect_diagnostics: true,
                ..Default::default()
            },
        );

        assert_eq!(report.stop_reason, VmStopReason::Completed);
        assert_eq!(vm.stack, [Value::Int(777)]);
        assert!(vm
            .calls
            .keys()
            .any(|key| key.starts_with("script:0xFF:0x40:")));
    }

    #[test]
    fn graph_preload_normalizes_both_native_string_arguments() {
        let mut vm = Vm::new();
        vm.write_c_string(0x3000, "data02xxx.arc").unwrap();
        vm.write_c_string(0x3100, "bg50d_a").unwrap();
        vm.stack.extend([Value::Ptr(0x3000), Value::Ptr(0x3100)]);

        vm.normalize_graph_string_args(0x92, 0x14).unwrap();

        assert_eq!(
            vm.stack,
            [
                Value::Str("data02xxx.arc".into()),
                Value::Str("bg50d_a".into())
            ]
        );
    }

    #[test]
    fn graph_load_resolves_dynamic_strings_stored_in_frame_slots() {
        let mut vm = Vm::new();
        let local_name = 0x1200_3000;
        vm.write_value(local_name, 2, &Value::Str("bg50d_a".into()))
            .unwrap();
        vm.stack.extend([
            Value::Int(43),
            Value::Str("data02xxx.arc".into()),
            Value::Ptr(local_name),
        ]);

        vm.normalize_graph_string_args(0x90, 0x10).unwrap();

        assert_eq!(
            vm.stack,
            [
                Value::Int(43),
                Value::Str("data02xxx.arc".into()),
                Value::Str("bg50d_a".into())
            ]
        );
    }

    #[test]
    fn graph_load_skips_a_damaged_direct_string_for_nested_descriptor_text() {
        let mut vm = Vm::new();
        let descriptor = 0x1200_3000;
        let resource_name = 0x1200_3100;
        vm.write_c_string(descriptor, "&#65533;&#65533;").unwrap();
        vm.write_c_string(resource_name, "bg_WHITE").unwrap();
        vm.write_int(descriptor + 8, 2, resource_name).unwrap();
        vm.stack.extend([
            Value::Int(43),
            Value::Str("data02xxx.arc".into()),
            Value::Ptr(descriptor),
        ]);

        vm.normalize_graph_string_args(0x90, 0x10).unwrap();

        assert_eq!(vm.stack.last(), Some(&Value::Str("bg_WHITE".into())));
    }

    #[test]
    fn strcpy_preserves_native_bytes_for_pointer_sources() {
        let source = 0x3000;
        let destination = 0x3100;
        let mut vm = Vm::new();
        vm.write_int(source, 0, 0x81).unwrap();
        vm.write_int(source + 1, 0, 0).unwrap();
        vm.stack
            .extend([Value::Ptr(destination), Value::Ptr(source)]);

        vm.dispatch(
            &test_instruction(0x10, 0x6a, "strcpy", vec![0x6a], Vec::new()),
            &mut TraceApi,
        )
        .unwrap();

        assert_eq!(vm.read_int(destination, 0).unwrap(), 0x81);
        assert_eq!(vm.read_int(destination + 1, 0).unwrap(), 0);
        assert!(!vm.memory_preview(destination, 16).contains("&#65533;"));
    }

    #[test]
    fn native_system_event_queue_preserves_two_argument_abi() {
        let destination = 0x2000u32;
        let program = BpProgram {
            script_name: Some("system-event-test".into()),
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![
                test_instruction(
                    0x10,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(0x4001)],
                ),
                test_instruction(
                    0x15,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(100)],
                ),
                test_instruction(0x1a, 0x80, "sys1", vec![0x80, 0xa1], Vec::new()),
                test_instruction(
                    0x1c,
                    0x02,
                    "push_dword",
                    vec![0x02],
                    vec![BpOperand::U32(destination)],
                ),
                test_instruction(0x21, 0x80, "sys1", vec![0x80, 0xa0], Vec::new()),
                test_instruction(0x23, 0x17, "ret", vec![0x17], Vec::new()),
            ],
            labels: Default::default(),
            warnings: Vec::new(),
        };
        let mut api = SchedulingApi::default();
        let mut vm = Vm::new();

        let report = vm.run(&program, &mut api, &VmRunOptions::default());

        assert_eq!(report.stop_reason, VmStopReason::Completed);
        assert_eq!(vm.stack, [Value::Int(1)]);
        assert_eq!(vm.read_int(destination, 2).unwrap(), 0);
        assert_eq!(vm.read_int(destination + 4, 2).unwrap(), 0x4001);
        assert_eq!(vm.read_int(destination + 8, 2).unwrap(), 100);
    }

    #[test]
    fn graph_procedure_suspends_before_following_instruction() {
        let program = BpProgram {
            script_name: Some("procedure-boundary-test".into()),
            functions: Vec::new(),
            strings: Vec::new(),
            instructions: vec![
                test_instruction(
                    0x10,
                    0x01,
                    "push_byte",
                    vec![0x01, 7],
                    vec![BpOperand::U8(7)],
                ),
                test_instruction(0x12, 0x90, "grp1", vec![0x90, 0xb9], Vec::new()),
                test_instruction(
                    0x14,
                    0x01,
                    "push_byte",
                    vec![0x01, 99],
                    vec![BpOperand::U8(99)],
                ),
            ],
            labels: Default::default(),
            warnings: Vec::new(),
        };
        let options = VmRunOptions {
            max_steps: 100,
            ..Default::default()
        };
        let mut api = SchedulingApi::default();
        let mut vm = Vm::new();

        let report = vm.run(&program, &mut api, &options);
        assert_eq!(report.stop_reason, VmStopReason::WaitingForAnimation);
        assert_eq!(report.steps, 2);
        assert_eq!(report.pc, 2);
        assert!(vm.stack.is_empty());

        let report = vm.run_loaded(&mut api, &options);
        assert_eq!(report.stop_reason, VmStopReason::WaitingForAnimation);
        assert_eq!(report.steps, 0);
        assert_eq!(report.pc, 2);

        vm.advance_time_ms(1);
        let report = vm.run_loaded(&mut api, &options);
        assert_eq!(report.stop_reason, VmStopReason::Completed);
        assert_eq!(report.steps, 1);
        assert_eq!(vm.stack, [Value::Int(99)]);
    }

    #[test]
    fn graph_procedure_pushes_native_completion_values() {
        let mut vm = Vm::new();
        vm.pending_graph_procedure = Some(PendingGraphProcedure {
            started_ms: vm.timing.tick_count(),
            duration_ms: 1,
            input_enabled: false,
            input_descriptor: 1808,
            wait_for_input: false,
            completion: super::GraphProcedureCompletion::ControlProgress,
        });
        vm.advance_time_ms(16);

        assert!(!vm.poll_graph_procedure(&mut TraceApi, false));
        assert_eq!(vm.stack, [Value::Int(1000), Value::Int(-1)]);
        assert!(vm.pending_graph_procedure.is_none());
    }

    #[test]
    fn message_procedure_resumes_without_control_values() {
        let mut vm = Vm::new();
        vm.pending_graph_procedure = Some(PendingGraphProcedure {
            started_ms: vm.timing.tick_count(),
            duration_ms: 1,
            input_enabled: false,
            input_descriptor: 0,
            wait_for_input: false,
            completion: super::GraphProcedureCompletion::None,
        });
        vm.advance_time_ms(16);

        assert!(!vm.poll_graph_procedure(&mut TraceApi, false));
        assert!(vm.stack.is_empty());
        assert!(vm.pending_graph_procedure.is_none());
    }

    #[test]
    fn message_procedure_pushes_native_interruption_flag() {
        let mut vm = Vm::new();
        vm.pending_graph_procedure = Some(PendingGraphProcedure {
            started_ms: vm.timing.tick_count(),
            duration_ms: 1,
            input_enabled: false,
            input_descriptor: 0,
            wait_for_input: false,
            completion: super::GraphProcedureCompletion::MessageInterrupted,
        });
        vm.advance_time_ms(16);

        assert!(!vm.poll_graph_procedure(&mut TraceApi, false));
        assert_eq!(vm.stack, [Value::Int(0)]);
    }

    #[test]
    fn wait_timing_ex_times_out_and_pushes_zero() {
        let mut vm = Vm::new();
        vm.stack
            .extend([Value::Int(100), Value::Int(1), Value::Int(1808)]);
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut TraceApi, 0x80, 0x5c)
                .unwrap(),
            Some(Value::None)
        );
        let mut api = SchedulingApi::default();

        assert!(vm.poll_wait_timing_procedure(&mut api, false));
        vm.advance_time_ms(100);
        assert!(!vm.poll_wait_timing_procedure(&mut api, false));
        assert_eq!(vm.stack, [Value::Int(0)]);
        assert_eq!(api.last_input_scope, Some(1808));
    }

    #[test]
    fn wait_timing_ex_is_interrupted_by_its_input_class() {
        let mut vm = Vm::new();
        vm.stack
            .extend([Value::Int(60_001), Value::Int(1), Value::Int(1808)]);
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut TraceApi, 0x80, 0x5c)
                .unwrap(),
            Some(Value::None)
        );
        let mut api = SchedulingApi {
            input_class_state: 1,
            ..Default::default()
        };

        assert!(!vm.poll_wait_timing_procedure(&mut api, false));
        assert_eq!(vm.stack, [Value::Int(1)]);
        assert_eq!(api.last_input_scope, Some(1808));
    }

    #[test]
    fn native_program_callback_interrupts_wait_timing_only_for_code_one() {
        let mut vm = Vm::new();
        vm.stack
            .extend([Value::Int(60_001), Value::Int(0), Value::Int(1808)]);
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut TraceApi, 0x80, 0x5c)
                .unwrap(),
            Some(Value::None)
        );

        assert!(vm.post_async_program_callback(
            Value::Int(0),
            [Value::Int(0), Value::Int(0), Value::Int(0)],
            false,
        ));
        assert!(vm.poll_wait_timing_procedure(&mut TraceApi, false));

        assert!(vm.post_async_program_callback(
            Value::Int(0),
            [Value::Int(1), Value::Int(0), Value::Int(0)],
            false,
        ));
        assert!(!vm.poll_wait_timing_procedure(&mut TraceApi, false));
        assert_eq!(vm.stack, [Value::Int(1)]);
    }

    #[test]
    fn native_program_callback_fails_without_an_installed_procedure() {
        let mut vm = Vm::new();
        assert!(!vm.post_async_program_callback(
            Value::Int(0),
            [Value::Int(1), Value::Int(0), Value::Int(0)],
            false,
        ));
        assert!(vm.pending_program_callbacks.is_empty());
    }

    #[test]
    fn wait_timing_ex_without_input_ignores_input_state() {
        let mut vm = Vm::new();
        vm.stack
            .extend([Value::Int(20), Value::Int(0), Value::Int(1808)]);
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut TraceApi, 0x80, 0x5c)
                .unwrap(),
            Some(Value::None)
        );
        let mut api = SchedulingApi {
            input_class_state: 1,
            ..Default::default()
        };

        assert!(vm.poll_wait_timing_procedure(&mut api, false));
        assert_eq!(api.last_input_scope, None);
        vm.advance_time_ms(20);
        assert!(!vm.poll_wait_timing_procedure(&mut api, false));
        assert_eq!(vm.stack, [Value::Int(0)]);
    }

    #[test]
    fn resource_names_receive_stable_ids_and_boolean_lookup_results() {
        let mut vm = Vm::new();

        vm.stack.push(Value::Str("mm05_100001".into()));
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut TraceApi, 0x80, 0x84)
                .unwrap(),
            Some(Value::Int(1))
        );
        vm.stack.push(Value::Str("mm05_100001".into()));
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut TraceApi, 0x80, 0x84)
                .unwrap(),
            Some(Value::Int(1))
        );
        vm.stack.push(Value::Str("mm05_100002".into()));
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut TraceApi, 0x80, 0x84)
                .unwrap(),
            Some(Value::Int(1))
        );

        vm.stack.push(Value::Str("mm05_100001".into()));
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut TraceApi, 0x80, 0x85)
                .unwrap(),
            Some(Value::Int(1))
        );
        vm.stack.push(Value::Str("missing".into()));
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut TraceApi, 0x80, 0x85)
                .unwrap(),
            Some(Value::Int(0))
        );
    }

    #[test]
    fn native_string_hash_table_round_trips_script_names() {
        let mut vm = Vm::new();
        let source = 0x3000;
        let destination = 0x3100;
        vm.write_c_string(source, "SetupForOmake").unwrap();

        vm.stack
            .extend([Value::Int(1), Value::Int(77), Value::Ptr(source)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0xda).unwrap(), Some(Value::Int(1)));

        vm.stack
            .extend([Value::Ptr(destination), Value::Int(77), Value::Int(0)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0xdd).unwrap(), Some(Value::Int(0)));
        assert_eq!(vm.read_c_string(destination).unwrap(), "SetupForOmake");
    }

    #[test]
    fn native_string_table_serialization_matches_namespace_contents() {
        let mut vm = Vm::new();
        let source = 0x3000;
        let destination = 0x3100;
        vm.write_c_string(source, "first").unwrap();
        vm.write_c_string(source + 6, "second").unwrap();

        vm.stack
            .extend([Value::Int(2), Value::Int(0), Value::Ptr(source)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0xda).unwrap(), Some(Value::Int(1)));

        vm.stack.extend([Value::Ptr(0), Value::Int(0)]);
        assert_eq!(
            vm.try_builtin_sys(0x80, 0xdb).unwrap(),
            Some(Value::Int(13))
        );

        vm.stack.extend([Value::Ptr(destination), Value::Int(0)]);
        assert_eq!(
            vm.try_builtin_sys(0x80, 0xdb).unwrap(),
            Some(Value::Int(13))
        );
        assert_eq!(vm.read_c_string(destination).unwrap(), "first");
        assert_eq!(vm.read_c_string(destination + 6).unwrap(), "second");
    }

    #[test]
    fn native_operand_stack_push_wraps_the_pointer_to_slot_zero() {
        let mut vm = Vm::new();
        vm.stack
            .extend((0..(super::OPERAND_STACK_CAPACITY + 3)).map(|value| Value::Int(value as i32)));
        vm.trim_operand_stack();

        assert_eq!(vm.stack.len(), 3);
        assert_eq!(vm.stack.first(), Some(&Value::Int(4096)));
        assert_eq!(vm.stack.last(), Some(&Value::Int(4098)));
    }

    #[test]
    fn native_operand_stack_empty_pop_wraps_to_the_last_stale_slot() {
        let mut vm = Vm::new();
        vm.operand_slots[super::OPERAND_STACK_CAPACITY - 1] = Value::Int(777);

        assert_eq!(vm.pop_value().unwrap(), Value::Int(777));
        assert_eq!(vm.stack.len(), super::OPERAND_STACK_CAPACITY - 1);
        assert_eq!(vm.pop_value().unwrap(), Value::Int(0));
        assert_eq!(vm.stack.len(), super::OPERAND_STACK_CAPACITY - 2);
    }

    #[test]
    fn native_double_not_opcodes_match_testcase_pe_dispatch_table() {
        // testcase's 0x506300 opcode table maps 0x38 to sub_473E70 (AND)
        // and 0x39 to sub_473EB0 (OR). The older openbgi reference differs.
        let and_instruction = test_instruction(0x10, 0x38, "dnotzero", vec![0x38], Vec::new());
        let or_instruction = test_instruction(0x11, 0x39, "dnotzero2", vec![0x39], Vec::new());
        let mut vm = Vm::new();

        vm.stack.extend([Value::Int(0), Value::Int(-1)]);
        vm.dispatch(&and_instruction, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(0)]);

        vm.stack.extend([Value::Int(0), Value::Int(-1)]);
        vm.dispatch(&or_instruction, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(0), Value::Int(1)]);
    }

    #[test]
    fn memcmp_opcode_returns_boolean_equality() {
        let instruction = test_instruction(0x10, 0x63, "memcmp", vec![0x63], Vec::new());
        let mut vm = Vm::new();
        vm.memory[0x100..0x103].copy_from_slice(b"mf2");
        vm.memory[0x200..0x203].copy_from_slice(b"mf2");

        vm.stack
            .extend([Value::Ptr(0x100), Value::Ptr(0x200), Value::Int(3)]);
        vm.dispatch(&instruction, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(1)]);

        vm.memory[0x202] = b'3';
        vm.stack
            .extend([Value::Ptr(0x100), Value::Ptr(0x200), Value::Int(3)]);
        vm.dispatch(&instruction, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(1), Value::Int(0)]);
    }

    #[test]
    fn memcmp_opcode_compares_string_literals_as_shift_jis_c_strings() {
        let instruction = test_instruction(0x10, 0x63, "memcmp", vec![0x63], Vec::new());
        let mut vm = Vm::new();
        let magic = "BurikoCompiledScriptVer1.00";
        let size = magic.len() + 1;
        vm.memory[0x100..0x100 + magic.len()].copy_from_slice(magic.as_bytes());
        vm.memory[0x100 + magic.len()] = 0;

        vm.stack.extend([
            Value::Ptr(0x100),
            Value::Str(magic.into()),
            Value::Int(size as i32),
        ]);
        vm.dispatch(&instruction, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(1)]);
    }

    #[test]
    fn native_string_find_returns_shift_jis_byte_offset() {
        let instruction = test_instruction(0x10, 0x66, "strfind", vec![0x66], Vec::new());
        let mut vm = Vm::new();

        vm.stack
            .extend([Value::Str("前:後".into()), Value::Str(":".into())]);
        vm.dispatch(&instruction, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(2)]);

        vm.stack
            .extend([Value::Str("Main".into()), Value::Str(":".into())]);
        vm.dispatch(&instruction, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(2), Value::Int(-1)]);
    }

    #[test]
    fn native_fixed_block_find_returns_block_index() {
        let instruction = test_instruction(0x10, 0x65, "memfind", vec![0x65], Vec::new());
        let mut vm = Vm::new();
        vm.memory[0x100..0x10c].copy_from_slice(b"aaaabbbbcccc");
        vm.memory[0x200..0x204].copy_from_slice(b"bbbb");
        vm.stack.extend([
            Value::Ptr(0x100),
            Value::Int(4),
            Value::Int(3),
            Value::Ptr(0x200),
        ]);

        vm.dispatch(&instruction, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(1)]);
    }

    #[test]
    fn native_vector_math_opcodes_match_fixed_degree_contract() {
        let atan2 = test_instruction(0x10, 0x43, "atan2", vec![0x43], Vec::new());
        let length = test_instruction(0x11, 0x44, "vec3_length", vec![0x44], Vec::new());
        let mut vm = Vm::new();

        vm.stack.extend([Value::Int(0), Value::Int(-1)]);
        vm.dispatch(&atan2, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(270 * 65_536)]);

        vm.stack
            .extend([Value::Int(3), Value::Int(4), Value::Int(12)]);
        vm.dispatch(&length, &mut TraceApi).unwrap();
        assert_eq!(vm.stack, [Value::Int(270 * 65_536), Value::Int(13)]);
    }

    #[test]
    fn load_program_adds_a_module_without_creating_a_native_thread() {
        let mut vm = Vm::new();
        vm.stack.extend([
            Value::Str("sysprg.arc".into()),
            Value::Str("worker._bp".into()),
        ]);
        let instruction = test_instruction(0, 0x80, "sys1", vec![0x80, 0x40], Vec::new());
        let program = empty_loaded_program("worker._bp".into());

        vm.dispatch(&instruction, &mut MediationApi { program })
            .unwrap();

        assert!(vm.async_tasks.is_empty());
        assert!(matches!(vm.stack.last(), Some(Value::Program(_))));
    }

    #[test]
    fn load_program_ex_creates_an_immediately_scheduled_native_thread() {
        let mut vm = Vm::new();
        vm.stack.extend([
            Value::Str("sysprg.arc".into()),
            Value::Str("worker._bp".into()),
            Value::Int(1024),
            Value::Int(8192),
            Value::Int(8192),
        ]);
        let instruction = test_instruction(0, 0x80, "sys1", vec![0x80, 0x44], Vec::new());
        let program = empty_loaded_program("worker._bp".into());

        vm.dispatch(&instruction, &mut MediationApi { program })
            .unwrap();

        assert_eq!(vm.async_tasks.len(), 1);
        assert!(matches!(vm.stack.last(), Some(Value::Program(_))));
        assert!(vm.async_tasks[0].runnable);
        let handle = vm.stack.last().cloned().unwrap();
        assert!(vm.post_async_program_message(handle.clone(), Value::Int(0x40ff_ffff), false));
        assert!(vm.async_tasks[0].runnable);
        assert!(vm.switch_to_async_program(handle, false));
        assert!(vm.async_tasks[0].runnable);
    }

    #[test]
    fn operand_slot_sync_keeps_program_values_as_shared_handles() {
        let mut vm = Vm::new();
        let program = Arc::new(empty_loaded_program("shared".into()));
        vm.push_value(Value::Program(program.clone()));
        vm.sync_operand_slots();

        let Value::Program(stack_program) = &vm.stack[0] else {
            panic!("program value missing from operand stack");
        };
        let Value::Program(slot_program) = &vm.operand_slots[0] else {
            panic!("program value missing from operand slot ring");
        };
        assert!(Arc::ptr_eq(&program, stack_program));
        assert!(Arc::ptr_eq(&program, slot_program));
    }

    #[test]
    fn native_program_messages_are_fifo_and_report_the_received_count() {
        let mut vm = Vm::new();
        vm.pending_program_messages
            .extend([Value::Int(0x40ff_ffff), Value::Int(0x4100_0001)]);
        vm.stack.extend([Value::Int(2), Value::Ptr(0x1000)]);

        assert_eq!(vm.try_builtin_sys(0x80, 0x4b).unwrap(), Some(Value::Int(2)));
        assert_eq!(vm.read_int(0x1000, 2).unwrap(), 0x40ff_ffff);
        assert_eq!(vm.read_int(0x1004, 2).unwrap(), 0x4100_0001);

        vm.stack.extend([Value::Int(2), Value::Ptr(0x1000)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x4b).unwrap(), Some(Value::Int(0)));
    }

    #[test]
    fn native_program_lookup_does_not_report_a_missing_task_as_active() {
        let vm = Vm::new();
        assert_eq!(
            vm.async_program_is_active(Value::Program(Arc::new(empty_loaded_program(
                "missing".into()
            )))),
            0
        );
    }

    #[test]
    fn read_flag_range_round_trips_and_clears_bits() {
        let mut vm = Vm::new();
        vm.stack.extend([Value::Str("main".into()), Value::Int(16)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x88).unwrap(), Some(Value::Int(1)));
        vm.stack.extend([
            Value::Str("main".into()),
            Value::Int(10),
            Value::Int(1),
            Value::Int(3),
        ]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x8a).unwrap(), Some(Value::Int(0)));

        vm.stack.extend([
            Value::Str("main".into()),
            Value::Ptr(0x1000),
            Value::Int(11),
        ]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x8b).unwrap(), Some(Value::Int(0)));
        assert_eq!(vm.read_int(0x1000, 2).unwrap(), 1);

        vm.stack.extend([
            Value::Str("main".into()),
            Value::Int(11),
            Value::Int(0),
            Value::Int(1),
        ]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x8a).unwrap(), Some(Value::Int(0)));
        vm.stack.extend([
            Value::Str("main".into()),
            Value::Ptr(0x1000),
            Value::Int(11),
        ]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x8b).unwrap(), Some(Value::Int(0)));
        assert_eq!(vm.read_int(0x1000, 2).unwrap(), 0);

        vm.stack
            .extend([Value::Str("main".into()), Value::Int(15), Value::Int(1)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x89).unwrap(), Some(Value::Int(0)));
        vm.stack.extend([
            Value::Str("main".into()),
            Value::Ptr(0x1000),
            Value::Int(15),
        ]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x8b).unwrap(), Some(Value::Int(0)));
        assert_eq!(vm.read_int(0x1000, 2).unwrap(), 1);

        vm.stack.extend([
            Value::Str("main".into()),
            Value::Int(15),
            Value::Int(1),
            Value::Int(2),
        ]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x8a).unwrap(), Some(Value::Int(3)));
    }

    #[test]
    fn read_flag_resize_preserves_existing_bits_and_enforces_new_length() {
        let mut vm = Vm::new();
        vm.stack.extend([Value::Str("main".into()), Value::Int(16)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x88).unwrap(), Some(Value::Int(1)));
        vm.stack
            .extend([Value::Str("main".into()), Value::Int(15), Value::Int(1)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x89).unwrap(), Some(Value::Int(0)));

        vm.stack.extend([Value::Str("main".into()), Value::Int(24)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x88).unwrap(), Some(Value::Int(1)));
        vm.stack.extend([
            Value::Str("main".into()),
            Value::Ptr(0x1000),
            Value::Int(15),
        ]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x8b).unwrap(), Some(Value::Int(0)));
        assert_eq!(vm.read_int(0x1000, 2).unwrap(), 1);

        vm.stack.extend([Value::Str("main".into()), Value::Int(12)]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x88).unwrap(), Some(Value::Int(1)));
        vm.stack.extend([
            Value::Str("main".into()),
            Value::Ptr(0x1000),
            Value::Int(15),
        ]);
        assert_eq!(vm.try_builtin_sys(0x80, 0x8b).unwrap(), Some(Value::Int(2)));
    }
}
