use ethornell_script::{known_call_name, BpInstruction, BpOpcode, BpOperand, BpProgram};
use std::collections::{BTreeMap, HashMap, VecDeque};

mod async_program;
mod debug;
mod input;
mod profile;
mod records;
mod scenario;
mod time;

const SYSTEM_PROGRAM_TABLE: u32 = 273_280;
const SYSTEM_PROGRAM_SLOTS: usize = 32;
const SYSTEM_PROGRAM_STRIDE: u32 = 16;
const SYSTEM_PROGRAM_DESCRIPTOR_BASE: u32 = 0x4000_0000;
const ADDRESS_MASK: u32 = 0x01ff_ffff;
const LOCAL_MEMORY_BASE: u32 = 0x0080_0000;
const INITIAL_MEMORY_SIZE: usize = 64 * 1024 * 1024;

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
    Program(Box<BpProgram>),
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

#[derive(Debug, Clone)]
pub struct VmRunOptions {
    pub max_steps: usize,
    pub trace: bool,
    pub fail_on_stub: bool,
}

impl Default for VmRunOptions {
    fn default() -> Self {
        Self {
            max_steps: 10_000,
            trace: false,
            fail_on_stub: false,
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

    fn take_frame_yield(&mut self) -> bool {
        false
    }
}

pub trait GraphApi {
    fn call_graph(&mut self, group: u8, id: u16, stack: &mut Vec<Value>) -> VmResult<Value>;

    fn poll_object_state(&mut self, _object: i32) -> i32 {
        0
    }

    fn poll_object_event(&mut self, _object: i32) -> i32 {
        0
    }

    fn poll_object_event_payload(&mut self, object: i32) -> (i32, i32) {
        (self.poll_object_event(object), 0)
    }
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
}

fn empty_loaded_program(name: String) -> BpProgram {
    placeholder_loaded_program(
        name,
        "ret",
        "generated empty program after runtime load failure",
    )
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
    pub stack: Vec<Value>,
    pub pc: usize,
    pub call_stack: Vec<(usize, usize)>,
    pub programs: Vec<BpProgram>,
    pub current_program: usize,
    pub memory: Vec<u8>,
    pub mem_values: BTreeMap<u32, Value>,
    pub mem_ptr: u32,
    pub heap_ptr: u32,
    pub halted: bool,
    pub calls: BTreeMap<String, usize>,
    pub stubs: BTreeMap<String, usize>,
    program_cache: BTreeMap<String, usize>,
    program_free_stack: Vec<usize>,
    record_tables: BTreeMap<u32, records::RecordTableState>,
    indexed_record_tables: BTreeMap<u32, records::IndexedRecordState>,
    script_records: BTreeMap<u32, String>,
    loaded_bcs_ranges: Vec<scenario::LoadedBcsRange>,
    async_tasks: Vec<async_program::AsyncProgramTask>,
    timing: time::VmTime,
    recent_trace: VecDeque<String>,
    yield_requested: bool,
}

impl Vm {
    pub fn new() -> Self {
        Self {
            memory: vec![0; INITIAL_MEMORY_SIZE],
            heap_ptr: 0x0020_0000,
            ..Self::default()
        }
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
        self.halted = false;
        self.timing.reset();
    }

    pub fn run_loaded<A>(&mut self, api: &mut A, options: &VmRunOptions) -> VmRunReport
    where
        A: SysApi + GraphApi + SoundApi,
    {
        let mut steps = 0usize;
        let mut stop_reason = VmStopReason::Completed;
        let trace_stack = std::env::var_os("TRACE_STACK").is_some();
        let trace_events =
            std::env::var_os("TRACE_VM_EVENTS").is_some() || std::env::var_os("DEBUG").is_some();
        self.pump_async_programs(api, trace_events);
        while steps < options.max_steps
            && !self.halted
            && self
                .programs
                .get(self.current_program)
                .and_then(|program| program.instructions.get(self.pc))
                .is_some()
        {
            let program_index = self.current_program;
            let inst = self.programs[program_index].instructions[self.pc].clone();
            let stack_before = self.stack.len();
            let pc_before = self.pc;
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
            self.push_trace(format!(
                "program={} pc={} off=0x{:08X} {} {:?}",
                self.program_name(program_index),
                self.pc,
                inst.offset,
                inst.opcode_name,
                inst.operands
            ));
            match self.dispatch_program(
                program_index,
                &inst,
                api,
                options.fail_on_stub,
                trace_events,
            ) {
                Ok(()) => {
                    if trace_stack {
                        let stack_after = self.stack.len();
                        let delta = stack_after as isize - stack_before as isize;
                        if delta != 0 || inst.opcode_name.starts_with("sys") {
                            let program_name = self
                                .programs
                                .get(program_index)
                                .and_then(|program| program.script_name.as_deref())
                                .unwrap_or("<unknown>");
                            eprintln!(
                                "TRACE_STACK program={} pc={} off=0x{:08X} op={} {:?} stack {} -> {} ({:+}) next_pc={} top={:?}",
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
            calls: self.calls.clone(),
            stubs: self.stubs.clone(),
            recent_trace: self.recent_trace.iter().cloned().collect(),
        }
    }

    pub fn dispatch<A>(&mut self, instruction: &BpInstruction, api: &mut A) -> VmResult<()>
    where
        A: SysApi + GraphApi + SoundApi,
    {
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
        let code = instruction.opcode.code();
        let mut next_pc = self.pc + 1;
        match instruction.opcode {
            BpOpcode::Known {
                name: "push_byte", ..
            } => {
                self.stack.push(Value::Int(read_op_i32(instruction)));
            }
            BpOpcode::Known {
                name: "push_word" | "push_dword",
                ..
            } => {
                self.stack.push(Value::Int(read_op_i32(instruction)));
            }
            BpOpcode::Known {
                name: "push_base_offset",
                ..
            } => {
                self.stack.push(Value::Ptr(
                    0x1200_0000u32 | self.mem_ptr.saturating_sub(read_op_u32(instruction)),
                ));
            }
            BpOpcode::Known {
                name: "push_string",
                ..
            } => {
                if let Some(BpOperand::String(text)) = instruction.operands.first() {
                    self.stack.push(Value::Str(text.clone()));
                } else if let Some(BpOperand::Offset(offset)) = instruction.operands.first() {
                    self.stack.push(Value::Ptr(0x1000_0000 | *offset));
                }
            }
            BpOpcode::Known {
                name: "push_offset",
                ..
            } => {
                self.stack.push(Value::Func {
                    program_index,
                    offset: read_op_u32(instruction),
                });
            }
            BpOpcode::Known {
                name: "load_base", ..
            } => self.stack.push(Value::Ptr(0x1200_0000 | self.mem_ptr)),
            BpOpcode::Known {
                name: "store_base", ..
            } => {
                self.mem_ptr = self.pop_int()? as u32;
            }
            BpOpcode::Known { name: "load", .. } => {
                let width = read_op_u8(instruction);
                let ptr = self.pop_ptr()?;
                let value = self.read_value(ptr, width)?;
                self.stack.push(value);
            }
            BpOpcode::Known { name: "move", .. } => {
                let width = read_op_u8(instruction);
                let value = self.pop_value()?;
                let ptr = self.pop_ptr()?;
                self.write_value(ptr, width, &value)?;
            }
            BpOpcode::Known {
                name: "move_arg", ..
            } => {
                let width = read_op_u8(instruction);
                let ptr = self.pop_ptr()?;
                let value = self.pop_value()?;
                self.write_value(ptr, width, &value)?;
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
                if std::env::var_os("TRACE_VM_BRANCHES").is_some() {
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
            BpOpcode::Known { name: "call", .. } => match self.pop_value()? {
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
                    self.write_return_addr(program_index, next_pc)?;
                    self.call_stack.push((self.current_program, next_pc));
                    self.current_program = dest_program_index;
                    next_pc = self.jump_target_index(dest_program_index, offset)?;
                }
                Value::Program(program) => {
                    let dest_program_index = self.program_index_for_loaded_program(*program);
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
                    self.call_stack.push((self.current_program, next_pc));
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
                    self.call_stack.push((self.current_program, next_pc));
                    next_pc = self.jump_target_index(program_index, dest)?;
                }
            },
            BpOpcode::Known { name: "ret", .. } => {
                if self.mem_ptr == 0 {
                    self.halted = true;
                } else if let Some((program_id, fallback_ret)) = self.call_stack.pop() {
                    let ret_offset = self.read_return_addr()?;
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
                self.stack.push(Value::Int(value));
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
                self.stack.push(Value::Int(value));
            }
            BpOpcode::Known { name: "not", .. } => {
                let value = self.pop_int()?;
                self.stack.push(Value::Int(!value));
            }
            BpOpcode::Known {
                name: "bool_zero", ..
            } => {
                let value = self.pop_int()?;
                self.stack.push(Value::Int((value == 0) as i32));
            }
            BpOpcode::Known {
                name: "ternary", ..
            } => {
                let false_value = self.pop_value()?;
                let true_value = self.pop_value()?;
                let condition = self.pop_int()?;
                self.stack.push(if condition != 0 {
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
                self.stack.push(Value::Int(value));
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
                self.stack.push(Value::Int((value * 65_536.0) as i32));
            }
            BpOpcode::Known { name: "memcpy", .. } => {
                let size = self.pop_int()?.max(0) as usize;
                let src = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                let src = self.normalize_scenario_descriptor_src(src, size);
                let src_range = self.resolve_range(src, size)?;
                let dst_range = self.resolve_range(dst, size)?;
                let tmp = self.memory[src_range].to_vec();
                self.memory[dst_range].copy_from_slice(&tmp);
                self.trace_watch_write(dst, size, src, "memcpy");
                self.copy_shadow_values(src, dst, size);
            }
            BpOpcode::Known { name: "memclr", .. } => {
                let size = self.pop_int()?.max(0) as usize;
                let ptr = self.pop_ptr()?;
                let range = self.resolve_range(ptr, size)?;
                self.memory[range].fill(0);
                self.trace_watch_write(ptr, size, 0, "memclr");
                self.clear_shadow_values(ptr, size);
                self.restore_script_records(ptr, size)?;
            }
            BpOpcode::Known { name: "memset", .. } => {
                let value = self.pop_int()? as u8;
                let size = self.pop_int()?.max(0) as usize;
                let ptr = self.pop_ptr()?;
                let range = self.resolve_range(ptr, size)?;
                self.memory[range].fill(value);
                self.trace_watch_write(ptr, size, value as u32, "memset");
                self.clear_shadow_values(ptr, size);
                self.restore_script_records(ptr, size)?;
            }
            BpOpcode::Known { name: "memcmp", .. } => {
                let size = self.pop_int()?.max(0) as usize;
                let src = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                let src_range = self.resolve_range(src, size)?;
                let dst_range = self.resolve_range(dst, size)?;
                let ordering = self.memory[dst_range]
                    .iter()
                    .zip(self.memory[src_range].iter())
                    .find_map(|(left, right)| match left.cmp(right) {
                        std::cmp::Ordering::Equal => None,
                        std::cmp::Ordering::Less => Some(-1),
                        std::cmp::Ordering::Greater => Some(1),
                    })
                    .unwrap_or(0);
                self.stack.push(Value::Int(ordering));
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
                self.stack.push(Value::Int(encoded.len() as i32));
            }
            BpOpcode::Known { name: "streq", .. } => {
                let right = self.pop_string_lossy()?;
                let left = self.pop_string_lossy()?;
                self.stack.push(Value::Int((left == right) as i32));
            }
            BpOpcode::Known { name: "strcpy", .. } => {
                let right = self.pop_string_lossy()?;
                let left = self.pop_ptr()?;
                self.write_c_string(left, &right)?;
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
                self.stack.push(Value::Int(ch));
                self.stack.push(Value::Int(is_two_byte as i32));
                self.stack
                    .push(Value::Int(is_sjis_delimiter(ch as u16) as i32));
            }
            BpOpcode::Known {
                name: "tolower", ..
            } => {
                let ptr = self.pop_ptr()?;
                let text = self.read_c_string(ptr)?.to_ascii_lowercase();
                self.write_c_string(ptr, &text)?;
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
                self.stack.push(Value::Ptr(ptr));
            }
            BpOpcode::Known { name: "free", .. } => {
                let _ptr = self.pop_ptr()?;
                self.stack.push(Value::Int(1));
            }
            BpOpcode::Known {
                name: "addmemboundary",
                ..
            } => {
                let _name = self.pop_string_lossy()?;
                let _size = self.pop_int()?;
                let _start = self.pop_int()?;
                self.stack.push(Value::Int(1));
            }
            BpOpcode::Known {
                name: "confirm", ..
            } => {
                let _message = self.pop_string_lossy().unwrap_or_default();
                self.stack.push(Value::Int(1));
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
                name: "sys1" | "sys2",
                ..
            } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default() as u16;
                self.note_call("sys", code, id);
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
                    Value::Int(written)
                } else if (code, id) == (0x80, 0x12) {
                    let input_descriptor_arg = self.pop_value()?;
                    let input_descriptor = match input_descriptor_arg {
                        Value::Ptr(ptr) => self.read_value(ptr, 2)?.as_i32(),
                        value => value.as_i32(),
                    };
                    Value::Int(api.read_input_state(input_descriptor))
                } else if (code, id) == (0x80, 0x81) {
                    Value::None
                } else if (code, id) == (0x80, 0x40) {
                    let file = self.pop_string_lossy()?;
                    let archive = self.pop_string_lossy()?;
                    let program = api
                        .load_program(&archive, &file)
                        .unwrap_or_else(|| empty_loaded_program(format!("{archive}:{file}")));
                    Value::Program(Box::new(program))
                } else if (code, id) == (0x80, 0x44) {
                    let mut params = Vec::with_capacity(3);
                    for _ in 0..3 {
                        params.push(self.pop_value()?);
                    }
                    let file = self.pop_string_lossy()?;
                    let archive = self.pop_string_lossy()?;
                    let program = api
                        .load_program_ex(&archive, &file, &params)
                        .unwrap_or_else(|| empty_loaded_program(format!("{archive}:{file}")));
                    Value::Program(Box::new(program))
                } else if (code, id) == (0x80, 0x41) {
                    let program = if matches!(self.stack.last(), Some(Value::Program(_))) {
                        self.stack.pop().unwrap_or(Value::None)
                    } else {
                        self.free_next_called_program(trace_events)
                    };
                    let freed_program = self.free_loaded_program_value(program, trace_events);
                    api.free_program(freed_program);
                    Value::None
                } else if (code, id) == (0x80, 0x47) {
                    let program = self.pop_value()?;
                    Value::Int(self.async_program_is_complete(program, api, trace_events))
                } else if (code, id) == (0x80, 0x48) {
                    let _mode = self.pop_value()?;
                    let program = self.pop_value()?;
                    self.start_async_program(program, trace_events);
                    Value::None
                } else if (code, id) == (0x80, 0x49) {
                    let _program_or_token = self.pop_value()?;
                    Value::Int(0)
                } else if (code, id) == (0x80, 0x4a) {
                    let args_ptr = self.pop_value()?;
                    let argc = self.pop_int()?.max(0).min(64) as usize;
                    let program_value = self.pop_value()?;
                    if let Some(program) = self.value_program(program_value) {
                        let args_addr = args_ptr.as_i32() as u32;
                        let mut args = Vec::with_capacity(argc);
                        for index in 0..argc {
                            args.push(
                                self.read_value(args_addr.saturating_add((index * 4) as u32), 2)?,
                            );
                        }
                        let dest_program_index =
                            self.program_index_for_loaded_program(program.clone());
                        if trace_events {
                            tracing::info!(
                                pc = self.pc,
                                offset = format_args!("0x{:08X}", instruction.offset),
                                dest_program = dest_program_index,
                                argc,
                                args_ptr = format_args!("0x{:08X}", args_addr),
                                stack_top = ?self.stack_summary(8),
                                "VM ProgramDispatchWithArgs"
                            );
                        }
                        self.start_async_program_with_args(
                            Value::Program(Box::new(program)),
                            args,
                            trace_events,
                        );
                    } else if trace_events {
                        tracing::warn!(
                            pc = self.pc,
                            offset = format_args!("0x{:08X}", instruction.offset),
                            argc,
                            args_ptr = ?args_ptr,
                            "VM ProgramDispatchWithArgs missing program"
                        );
                    }
                    Value::None
                } else if (code, id) == (0x80, 0x5e) {
                    let _program = self.pop_value()?;
                    Value::None
                } else if (code, id) == (0x80, 0x5f) {
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
                } else if (code, id) == (0x80, 0x3d) {
                    let _kind = self.pop_value()?;
                    let ptr = self.pop_ptr()?;
                    self.write_c_string(ptr, "")?;
                    Value::None
                } else if (code, id) == (0x80, 0x50) {
                    let enabled = self.pop_int().unwrap_or_default();
                    self.write_int(1616, 2, enabled as u32)?;
                    self.write_int(1620, 2, 0)?;
                    tracing::info!(enabled, "Sys50SystemWaitState");
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
                } else if let Some(result) = self.try_builtin_sys(code, id)? {
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
                    api.call_sys(code, id, &mut self.stack)?
                };
                if result != Value::None {
                    self.stack.push(result);
                }
            }
            BpOpcode::Known {
                name: "script_load",
                ..
            } => {
                let file = self.pop_string_lossy()?;
                let archive = self.pop_string_lossy()?;
                let slot = self.pop_value()?;
                self.note_call("script", 0xff, 0xf0);
                tracing::info!(?slot, archive, file, "BP script_load");
            }
            BpOpcode::Known {
                name: "script_free",
                ..
            } => {
                let _slot = self.pop_value()?;
                self.note_call("script", 0xff, 0xf1);
            }
            BpOpcode::Known {
                name: "script_ret", ..
            } => {
                if self.mem_ptr == 0 {
                    self.halted = true;
                } else if let Some((program_id, fallback_ret)) = self.call_stack.pop() {
                    let ret_offset = self.read_return_addr()?;
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
                let id = instruction.raw.get(1).copied().unwrap_or_default() as u16;
                self.note_call("script", 0xff, id);
                if fail_on_stub {
                    self.note_stub("script", 0xff, id);
                    return Err(VmError::UnknownDispatch { group: 0xff, id });
                }
            }
            BpOpcode::Known {
                name: "grp1" | "grp2" | "grp3",
                ..
            } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default() as u16;
                self.note_call("graph", code, id);
                if known_call_name(code, id).is_none() && fail_on_stub {
                    self.note_stub("graph", code, id);
                    return Err(VmError::UnknownDispatch { group: code, id });
                }
                let result = if (code, id) == (0x90, 0xbc) {
                    let object = self.pop_value()?;
                    let state_buffer = self.pop_ptr()?;
                    self.write_int(
                        state_buffer,
                        2,
                        api.poll_object_state(object.as_i32()) as u32,
                    )?;
                    Value::None
                } else if (code, id) == (0x90, 0xbf) {
                    let object = self.pop_value()?;
                    let event_buffer = self.pop_ptr()?;
                    let (event, payload) = api.poll_object_event_payload(object.as_i32());
                    if input::clears_title_pending_callback(event, payload) {
                        self.write_value(input::TITLE_PENDING_CALLBACK_ADDR, 2, &Value::Int(0))?;
                    }
                    self.write_int(event_buffer, 2, event as u32)?;
                    self.write_int(event_buffer.wrapping_add(4), 2, payload as u32)?;
                    Value::None
                } else if (code, id) == (0x91, 0x3e) {
                    let result = api.call_graph(code, id, &mut self.stack)?;
                    self.write_int(1072, 2, result.as_i32() as u32)?;
                    Value::None
                } else if (code, id) == (0x91, 0xba) {
                    let dest = self.pop_ptr()?;
                    let layer_value = self.pop_value()?;
                    let value = layer_value.as_i32();
                    self.write_int(dest, 2, value as u32)?;
                    if self.next_instructions_store_call_result(program_index) {
                        Value::Int(value)
                    } else {
                        Value::None
                    }
                } else {
                    self.normalize_graph_string_args(code, id)?;
                    api.call_graph(code, id, &mut self.stack)?
                };
                if result != Value::None {
                    self.stack.push(result);
                }
            }
            BpOpcode::Known { name: "snd1", .. } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default() as u16;
                self.note_call("snd", code, id);
                if known_call_name(code, id).is_none() && fail_on_stub {
                    self.note_stub("snd", code, id);
                    return Err(VmError::UnknownDispatch { group: code, id });
                }
                self.normalize_sound_string_args(code, id)?;
                let result = api.call_sound(code, id, &mut self.stack)?;
                if result != Value::None {
                    self.stack.push(result);
                }
            }
            BpOpcode::Known {
                name: "usr1" | "usr2",
                ..
            } => {
                let id = instruction.raw.get(1).copied().unwrap_or_default() as u16;
                self.note_call("user", code, id);
                if known_call_name(code, id).is_none() && fail_on_stub {
                    self.note_stub("user", code, id);
                    return Err(VmError::UnknownDispatch { group: code, id });
                }
                api.observe_user(code, id, &self.stack);
                if let Some(result) = api.call_user(code, id, &mut self.stack)? {
                    if result != Value::None {
                        self.stack.push(result);
                    }
                } else {
                    self.try_builtin_user(code, id)?;
                }
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
        self.pc = next_pc;
        Ok(())
    }

    fn next_instructions_store_call_result(&self, program_index: usize) -> bool {
        let Some(program) = self.programs.get(program_index) else {
            return false;
        };
        let next = self.pc + 1;
        matches!(
            (
                program.instructions.get(next),
                program.instructions.get(next + 1),
            ),
            (
                Some(BpInstruction {
                    opcode: BpOpcode::Known {
                        name: "push_base_offset",
                        ..
                    },
                    ..
                }),
                Some(BpInstruction {
                    opcode: BpOpcode::Known {
                        name: "move_arg",
                        ..
                    },
                    ..
                }),
            )
        )
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
            return Value::Program(Box::new(program.clone()));
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
            return Value::Program(Box::new(program));
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
        Value::Program(Box::new(program))
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

    fn pop_value(&mut self) -> VmResult<Value> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
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

    fn translate_system_descriptor(ptr: u32) -> u32 {
        let slot = ptr.wrapping_sub(SYSTEM_PROGRAM_DESCRIPTOR_BASE);
        if slot < SYSTEM_PROGRAM_SLOTS as u32 {
            SYSTEM_PROGRAM_TABLE + slot * SYSTEM_PROGRAM_STRIDE
        } else {
            ptr
        }
    }

    fn register_system_program_stack_suffix(
        &mut self,
        arg1: Value,
        arg2: Value,
        arg3: Value,
    ) -> VmResult<()> {
        let mut programs = self
            .stack
            .iter()
            .rev()
            .take_while(|value| matches!(value, Value::Program(_)))
            .take(8)
            .cloned()
            .collect::<Vec<_>>();
        if programs.is_empty() {
            return Ok(());
        }
        programs.reverse();
        for (index, program) in programs.into_iter().enumerate() {
            let base = SYSTEM_PROGRAM_TABLE + (index as u32) * SYSTEM_PROGRAM_STRIDE;
            self.write_value(base, 2, &Value::Int(0))?;
            self.write_value(base + 4, 2, &program)?;
            self.write_value(base + 8, 2, &Value::Int(0))?;
            self.write_value(base + 12, 2, &Value::Int(0))?;
        }
        if std::env::var_os("DEBUG").is_some() || std::env::var_os("TRACE_VM_EVENTS").is_some() {
            tracing::info!(
                args = ?[value_summary(&arg1), value_summary(&arg2), value_summary(&arg3)],
                stack_top = ?self.stack_summary(12),
                "Sys5CTriple registered system programs"
            );
        }
        Ok(())
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
        let range = self.resolve_range(ptr, size)?;
        let bytes = value.to_le_bytes();
        self.memory[range].copy_from_slice(&bytes[..size]);
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
        let Some(spec) = std::env::var_os("TRACE_WATCH_ADDR") else {
            return;
        };
        let Some(spec) = spec.to_str() else {
            return;
        };
        let start = Self::memory_addr(ptr);
        let end = start.saturating_add(size as u32);
        let hit = spec
            .split(',')
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
            .map(Self::memory_addr)
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
        let ptr = 0x1200_0000 | self.heap_ptr;
        let next = self.heap_ptr.saturating_add(size.saturating_add(3) & !3);
        let end = Self::memory_addr(0x1200_0000 | next) as usize;
        if end > self.memory.len() {
            self.memory.resize(end, 0);
        }
        self.heap_ptr = next;
        ptr
    }

    fn read_c_string(&self, ptr: u32) -> VmResult<String> {
        let start = Self::memory_addr(ptr) as usize;
        if start >= self.memory.len() {
            return Err(VmError::MemoryOutOfBounds { addr: ptr, size: 1 });
        }
        let end = self.memory[start..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|offset| start + offset)
            .unwrap_or(self.memory.len());
        let (text, _, _) = encoding_rs::SHIFT_JIS.decode(&self.memory[start..end]);
        Ok(text.into_owned())
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

    fn write_c_string_raw(&mut self, ptr: u32, text: &str) -> VmResult<()> {
        let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode(text);
        let bytes = encoded.as_ref();
        let range = self.resolve_range(ptr, bytes.len().saturating_add(1))?;
        let start = range.start;
        self.memory[start..start + bytes.len()].copy_from_slice(bytes);
        self.memory[start + bytes.len()] = 0;
        self.trace_watch_write(ptr, bytes.len().saturating_add(1), 0, "write_c_string");
        Ok(())
    }

    fn copy_buffer(&mut self, dst: u32, src: u32, size: usize) -> VmResult<()> {
        let src_range = self.resolve_range(src, size)?;
        let dst_range = self.resolve_range(dst, size)?;
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
        let range = self.resolve_range(buffer, count)?;
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

    fn try_builtin_sys(&mut self, group: u8, id: u16) -> VmResult<Option<Value>> {
        let result = match (group, id) {
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
            (0x81, 0x0e) | (0x81, 0x18) | (0x81, 0x62) | (0x81, 0x63) | (0x81, 0x6f) => {
                let _arg = self.pop_value()?;
                Value::None
            }
            (0x80, 0x52) => {
                let _mode = self.pop_int()?;
                Value::None
            }
            (0x80, 0x00) => {
                let _seed = self.pop_int()?;
                Value::None
            }
            (0x80, 0x11) => {
                let _key_code = self.pop_value()?;
                Value::Int(0)
            }
            (0x80, 0x16) => Value::Int(0),
            (0x80, 0x60) => {
                let _arg3 = self.pop_value()?;
                let _arg2 = self.pop_value()?;
                let _arg1 = self.pop_value()?;
                Value::None
            }
            (0x80, 0x5c) => {
                let arg3 = self.pop_value()?;
                let arg2 = self.pop_value()?;
                let arg1 = self.pop_value()?;
                self.register_system_program_stack_suffix(arg1, arg2, arg3)?;
                Value::None
            }
            (0x80, 0x62) => {
                let _descriptor = self.pop_value()?;
                let _mode = self.pop_value()?;
                Value::None
            }
            (0x80, 0x64) => {
                let _enabled = self.pop_value()?;
                Value::None
            }
            (0x80, 0x66) => {
                let _arg = self.pop_value()?;
                Value::None
            }
            (0x80, 0x46) => Value::None,
            (0x80, 0x5a) => Value::None,
            (0x80, 0x6a) => Value::None,
            (0x80, 0xac) => {
                let _descriptor = self.pop_value()?;
                let _count = self.pop_value()?;
                let _object = self.pop_value()?;
                Value::None
            }
            (0x80, 0xc0) => {
                let _length = self.pop_int()?.max(0) as usize;
                let _state = self.pop_value()?;
                let dst = self.pop_ptr()?;
                self.write_int(dst, 2, 0)?;
                Value::Int(4)
            }
            (0x80, 0xc1) => {
                let src = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                let starts_with_sdc = self
                    .resolve_range(src, b"SDC FORMAT 1.00".len())
                    .map(|range| self.memory[range].starts_with(b"SDC FORMAT 1.00"))
                    .unwrap_or(false);
                if starts_with_sdc {
                    let status = self.write_sdc_script_records(dst)?;
                    tracing::debug!(
                        src = format_args!("0x{src:08X}"),
                        dst = format_args!("0x{dst:08X}"),
                        status,
                        "SdcDecodeRecords"
                    );
                    Value::Int(status)
                } else {
                    Value::Int(0)
                }
            }
            (0x80, 0xc4) => {
                let out_limit = self.pop_int()?.max(0) as usize;
                let src_size = self.pop_int()?.max(0) as usize;
                let src = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                let count = src_size.min(out_limit);
                if count > 0 {
                    self.copy_buffer(dst, src, count)?;
                }
                Value::Int(count as i32)
            }
            (0x80, 0xc5) => {
                let src = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                self.copy_buffer(dst, src, 620)?;
                Value::Int(1)
            }
            (0x80, 0xd2) => {
                let src = self.pop_value()?;
                let dst = self.pop_value()?;
                let handle = self.pop_ptr()?;
                let src = src.as_i32() as u32;
                let dst = dst.as_i32() as u32;
                self.sys_record_table_copy(handle, dst, src)?;
                Value::None
            }
            (0x80, 0xda) => {
                let _arg3 = self.pop_value()?;
                let _arg2 = self.pop_value()?;
                let _arg1 = self.pop_value()?;
                Value::None
            }
            (0x80, 0xd9) => {
                let _mode = self.pop_value()?;
                let _length = self.pop_value()?;
                let ptr = self.alloc_heap(4);
                self.write_int(ptr, 2, 0)?;
                Value::Ptr(ptr)
            }
            (0x80, 0xdb) => {
                let mode = self.pop_value()?;
                let state = self.pop_value()?;
                if state.as_i32() == 0 && mode.as_i32() == 0 {
                    Value::Int(4096)
                } else {
                    Value::Int(0)
                }
            }
            (0x80, 0x04) => Value::Int(self.timing.tick_count()),
            (0x80, 0x0c) => {
                let ptr = self.pop_ptr()?;
                self.write_system_time(ptr)?;
                Value::None
            }
            (0x80, 0x0d) => {
                self.stack.push(Value::Int(self.memory.len() as i32));
                Value::Int((self.memory.len().saturating_sub(self.heap_ptr as usize)) as i32)
            }
            (0x80, 0x0f) => Value::Int(0),
            (0x80, 0x14) => {
                let _descriptor = self.pop_value()?;
                Value::Int(1)
            }
            (0x80, 0x17) => Value::Int(0),
            (0x80, 0x18) | (0x80, 0x19) => {
                let milliseconds = self.pop_value()?.as_i32();
                self.timing.observe_wait(milliseconds);
                Value::None
            }
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
                let _ptr = self.pop_ptr()?;
                Value::Int(1)
            }
            (0x80, 0x28) => {
                let _path = self.pop_string_lossy()?;
                Value::Int(1)
            }
            (0x80, 0x2a) => {
                let _path = self.pop_string_lossy()?;
                Value::Int(1)
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
            (0x80, 0x3d) => {
                let _kind = self.pop_value()?;
                let ptr = self.pop_ptr()?;
                self.write_value(ptr, 2, &Value::Str(String::new()))?;
                Value::None
            }
            (0x80, 0x80) => {
                let _arg = self.pop_value()?;
                self.stack.push(Value::Int(0));
                self.stack.push(Value::Int(0));
                Value::Int(0)
            }
            (0x80, 0x88) => {
                let length = self.pop_int()?.max(0) as usize;
                let script_name = self.pop_string_lossy()?;
                self.scenario_code_preprocess(&script_name, length)?;
                Value::None
            }
            (0x80, 0xd0) => {
                let record_size = self.pop_int()?.max(0) as u32;
                let slot = self.pop_ptr()?;
                self.sys_record_table_open(slot, record_size)?
            }
            (0x80, 0xd1) => {
                let handle = self.pop_ptr()?;
                self.sys_record_table_close(handle);
                Value::None
            }
            (0x80, 0xd4) => {
                let mode = self.pop_int()?;
                let selector = self.pop_value()?;
                let handle = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                self.sys_record_table_fetch(dst, handle, selector, mode)?
            }
            (0x80, 0x82) | (0x80, 0x83) => {
                let _arg3 = self.pop_value()?;
                let _arg2 = self.pop_value()?;
                let _arg1 = self.pop_value()?;
                Value::None
            }
            (0x80, 0x98) => {
                let record_size = self.pop_int()?.max(0) as u32;
                let capacity = self.pop_int()?.max(0) as u32;
                let slot = self.pop_ptr()?;
                self.sys_indexed_record_open(slot, capacity, record_size)?;
                Value::None
            }
            (0x80, 0x9a) => {
                let handle = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                self.sys_indexed_record_count(dst, handle)?;
                Value::None
            }
            (0x80, 0x9d) => {
                let index = self.pop_int()?.max(0) as u32;
                let handle = self.pop_ptr()?;
                let dst = self.pop_ptr()?;
                self.sys_indexed_record_load(dst, handle, index)?;
                Value::None
            }
            (0x80, 0x99) => {
                let handle = self.pop_ptr()?;
                self.sys_indexed_record_close(handle);
                Value::None
            }
            (0x80, 0xa8) => {
                let _arg2 = self.pop_value()?;
                let _arg1 = self.pop_value()?;
                Value::None
            }
            (0x80, 0x9c) => {
                for _ in 0..4 {
                    let _arg = self.pop_value()?;
                }
                Value::None
            }
            (0x80, 0xdd) => {
                for _ in 0..3 {
                    let _arg = self.pop_value()?;
                }
                Value::Int(0)
            }
            (0x80, 0x58)
            | (0x80, 0x67)
            | (0x80, 0x68)
            | (0x80, 0x70)
            | (0x80, 0x74)
            | (0x80, 0xaf) => {
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
                self.stack.push(Value::Int(0));
            }
            (0xb0, 0xc1) => {
                let _text = self.pop_int()?;
                self.stack.push(Value::Int(1));
            }
            (0xc0, 0x00) => {
                let _height = self.pop_value()?;
                let _width = self.pop_value()?;
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
            _ => {}
        }
        Ok(())
    }

    fn normalize_sys_string_args(&mut self, group: u8, id: u16) -> VmResult<()> {
        let positions_from_top: &[usize] = match (group, id) {
            (0x80, 0x34) | (0x80, 0x35) | (0x80, 0x40) => &[0, 1],
            (0x80, 0x44) => &[3, 4],
            _ => return Ok(()),
        };
        for &from_top in positions_from_top {
            let Some(index) = self.stack.len().checked_sub(1 + from_top) else {
                continue;
            };
            let text = self.value_as_string_lossy(self.stack[index].clone())?;
            self.stack[index] = Value::Str(text);
        }
        Ok(())
    }

    fn normalize_sound_string_args(&mut self, group: u8, id: u16) -> VmResult<()> {
        let positions_from_top: &[usize] = match (group, id) {
            (0xa0, 0x11) => &[2, 3],
            (0xa0, 0x20) => &[0, 1],
            _ => return Ok(()),
        };
        for &from_top in positions_from_top {
            let Some(index) = self.stack.len().checked_sub(1 + from_top) else {
                continue;
            };
            if std::env::var_os("TRACE_SOUND_ARGS").is_some() {
                self.trace_sound_arg(group, id, from_top, index);
            }
            let text = if (group, id, from_top) == (0xa0, 0x11, 2) {
                self.value_as_sound_file_string(self.stack[index].clone())?
            } else {
                self.value_as_string_lossy(self.stack[index].clone())?
            };
            self.stack[index] = Value::Str(text);
        }
        Ok(())
    }

    fn normalize_graph_string_args(&mut self, group: u8, id: u16) -> VmResult<()> {
        let positions_from_top: &[usize] = match (group, id) {
            (0x90, 0x56) => &[0, 3],
            (0x92, 0x9c) => &[0],
            _ => return Ok(()),
        };
        for &from_top in positions_from_top {
            let Some(index) = self.stack.len().checked_sub(1 + from_top) else {
                continue;
            };
            let text = self.value_as_text_descriptor_string(self.stack[index].clone())?;
            if !text.is_empty() && !text.starts_with("0x") {
                self.stack[index] = Value::Str(text);
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

    fn value_as_text_descriptor_string(&self, value: Value) -> VmResult<String> {
        let direct = self.value_as_string_lossy(value.clone())?;
        if !direct.is_empty() && !direct.starts_with("0x") {
            return Ok(direct);
        }
        let ptr = match value {
            Value::Int(value) if value != 0 => value as u32,
            Value::Ptr(ptr) if ptr != 0 => ptr,
            _ => return Ok(direct),
        };
        for offset in [
            0_i32, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, -4, -8, -12, -16,
        ] {
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
        let range = self.resolve_range(ptr, 16)?;
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
    if trimmed.is_empty() || trimmed.starts_with("0x") {
        return false;
    }
    trimmed
        .chars()
        .any(|ch| !ch.is_control() || ch == '\n' || ch == '\r' || ch == '\t')
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
            (0x90, 0x13) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0x16) => {
                let _source = stack.pop();
                let _target = stack.pop();
                return Ok(Value::Int(0));
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
                for _ in 0..10 {
                    let _arg = stack.pop();
                }
            }
            (0x90, 0x18) => {
                for _ in 0..6 {
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
            (0x90, 0x32) => {
                let _value = stack.pop();
                let _object = stack.pop();
            }
            (0x90, 0x3c) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x90, 0x30) => {
                let _enabled = stack.pop();
                let _timeline = stack.pop();
            }
            (0x90, 0x50) => {
                return Ok(Value::Int(1));
            }
            (0x90, 0x54) => {
                let _enabled = stack.pop();
                let _node = stack.pop();
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
            (0x90, 0x5c) => {
                for _ in 0..17 {
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
            (0x90, 0x00)
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
            (0x90, 0x95) => {
                let _b = stack.pop();
                let _a = stack.pop();
            }
            (0x90, 0xb7) => {
                let _state = stack.pop();
                let _surface = stack.pop();
            }
            (0x90, 0xb9) => {
                let _object = stack.pop();
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
            (0x90, 0xd9) => {
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
            (0x91, 0x3e) => {
                let _mode = stack.pop();
                let _x = stack.pop();
                let _layer = stack.pop();
                return Ok(Value::Int(0));
            }
            (0x91, 0x38) => {
                let _mode = stack.pop();
                let _layer = stack.pop();
                let _buffer = stack.pop();
            }
            (0x91, 0x1f) => {
                let _arg2 = stack.pop();
                let _arg1 = stack.pop();
            }
            (0x91, 0x19) => {
                for _ in 0..11 {
                    let _arg = stack.pop();
                }
            }
            (0x91, 0x98) => {
                for _ in 0..7 {
                    let _arg = stack.pop();
                }
            }
            (0x92, 0x97) => {
                for _ in 0..7 {
                    let _arg = stack.pop();
                }
            }
            (0x92, 0x88) => {
                let _value = stack.pop();
                let _surface = stack.pop();
            }
            (0x92, 0x91) => {
                for _ in 0..5 {
                    let _arg = stack.pop();
                }
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
                let value = match stack.pop() {
                    Some(Value::Int(value)) => value,
                    Some(Value::Ptr(value)) => value as i32,
                    _ => 0,
                };
                stack.push(Value::Int(value));
                stack.push(Value::Int(value));
                return Ok(Value::Int(value));
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
