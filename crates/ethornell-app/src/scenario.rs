#[cfg(test)]
use ethornell_script::bcs::BcsSymbol;
use ethornell_script::bcs::{parse_bcs, BcsCommand, BcsProgram, BcsValue};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone)]
pub(crate) enum ScenarioAction {
    Sound {
        file: String,
    },
    Bgm {
        file: String,
    },
    Sprite {
        file: String,
        wait_frames: u32,
        slot: Option<i32>,
        x: Option<i32>,
        z: Option<i32>,
        opacity: Option<f32>,
        body_layer: Option<&'static str>,
    },
    TransformSprite {
        slot: i32,
        wait_frames: u32,
        x: Option<i32>,
        y: Option<i32>,
        opacity: Option<f32>,
    },
    HideSprite {
        slot: Option<i32>,
        wait_frames: u32,
    },
    Message {
        speaker: Option<String>,
        text: String,
    },
    Wait {
        frames: u32,
    },
    WaitForInput,
    ClearSprite,
    LoadScript {
        file: String,
        symbol: Option<String>,
        transfer: ScriptTransfer,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptTransfer {
    Call,
    Jump,
}

#[derive(Debug)]
pub(crate) struct ScenarioPlayback {
    program: BcsProgram,
    address_to_pc: HashMap<u32, usize>,
    pc: usize,
    operand_stack: Vec<BcsValue>,
    memory: BcsMemory,
    globals: BTreeMap<i32, BcsValue>,
    call_stack: Vec<usize>,
    pending_arg_count: Option<usize>,
    resolved_symbol: Option<ResolvedSymbol>,
    decoder: ScenarioActionBuilder,
    wait_frames: u32,
    waiting_for_input: bool,
    completed: bool,
    wait_return_ready: bool,
    awaiting_external_script: Option<PendingExternalScript>,
    external_call_stack: Vec<ExternalBcsFrame>,
}

#[derive(Debug)]
struct ExternalBcsFrame {
    program: BcsProgram,
    address_to_pc: HashMap<u32, usize>,
    pc: usize,
    operand_stack: Vec<BcsValue>,
    memory: BcsMemory,
    call_stack: Vec<usize>,
    pending_arg_count: Option<usize>,
    resolved_symbol: Option<ResolvedSymbol>,
}

#[derive(Debug, Clone)]
struct PendingExternalScript {
    symbol: Option<String>,
    transfer: ScriptTransfer,
}

#[derive(Debug, Clone)]
enum ResolvedSymbol {
    Local(u32),
    External {
        file: String,
        symbol: Option<String>,
    },
}

/// The BURIKO core VM stores locals in an addressable frame, not in a map of
/// compiler variable offsets. Keeping the frame explicit lets the same opcode
/// path serve library scripts and scenario scripts.
#[derive(Debug, Default)]
struct BcsMemory {
    pointer: i32,
    values: BTreeMap<i32, BcsValue>,
}

impl BcsMemory {
    fn read(&self, address: i32) -> BcsValue {
        self.values
            .get(&address)
            .cloned()
            .unwrap_or(BcsValue::Int(0))
    }

    fn write(&mut self, address: i32, value: BcsValue) {
        self.values.insert(address, value);
    }
}

impl ScenarioPlayback {
    pub(crate) fn from_bcs(bytes: &[u8]) -> Option<Self> {
        Self::from_program(parse_bcs(bytes)?)
    }

    fn from_program(program: BcsProgram) -> Option<Self> {
        if program.commands.is_empty() {
            return None;
        }
        let address_to_pc = program
            .commands
            .iter()
            .enumerate()
            .map(|(pc, command)| (command.addr, pc))
            .collect();
        Some(Self {
            program,
            address_to_pc,
            pc: 0,
            operand_stack: Vec::new(),
            memory: BcsMemory::default(),
            globals: BTreeMap::new(),
            call_stack: Vec::new(),
            pending_arg_count: None,
            resolved_symbol: None,
            decoder: ScenarioActionBuilder::default(),
            wait_frames: 0,
            waiting_for_input: false,
            completed: false,
            wait_return_ready: false,
            awaiting_external_script: None,
            external_call_stack: Vec::new(),
        })
    }

    pub(crate) fn instruction_count(&self) -> usize {
        self.program.commands.len()
    }

    pub(crate) fn is_waiting_for_input(&self) -> bool {
        self.waiting_for_input
    }

    pub(crate) fn append_bcs(&mut self, bytes: &[u8]) -> Option<usize> {
        self.append_program(parse_bcs(bytes)?)
    }

    fn append_program(&mut self, program: BcsProgram) -> Option<usize> {
        let command_count = program.commands.len();
        let pending = self.awaiting_external_script.take()?;
        let mut next = Self::from_program(program)?;
        if let Some(symbol) = pending.symbol.as_deref() {
            let address = next
                .program
                .symbols
                .iter()
                .find(|candidate| candidate.name == symbol)
                .map(|candidate| candidate.addr)?;
            next.pc = next.address_to_pc.get(&address).copied()?;
        }
        if pending.transfer == ScriptTransfer::Call {
            self.external_call_stack.push(ExternalBcsFrame {
                program: std::mem::replace(&mut self.program, next.program),
                address_to_pc: std::mem::replace(&mut self.address_to_pc, next.address_to_pc),
                pc: std::mem::replace(&mut self.pc, next.pc),
                operand_stack: std::mem::take(&mut self.operand_stack),
                memory: std::mem::take(&mut self.memory),
                call_stack: std::mem::take(&mut self.call_stack),
                pending_arg_count: self.pending_arg_count.take(),
                resolved_symbol: self.resolved_symbol.take(),
            });
        } else {
            self.program = next.program;
            self.address_to_pc = next.address_to_pc;
            self.pc = next.pc;
            self.operand_stack.clear();
            self.memory = BcsMemory::default();
            self.call_stack.clear();
            self.pending_arg_count = None;
            self.resolved_symbol = None;
        }
        self.operand_stack = next.operand_stack;
        self.memory = next.memory;
        self.call_stack = next.call_stack;
        self.pending_arg_count = next.pending_arg_count;
        self.completed = false;
        Some(command_count)
    }

    pub(crate) fn cancel_external_script(&mut self) {
        self.awaiting_external_script = None;
    }

    pub(crate) fn tick(&mut self, input_advance: bool) -> (Option<ScenarioAction>, bool) {
        let mut input_consumed = false;

        for _ in 0..2_048 {
            if self.waiting_for_input {
                if !input_advance {
                    return (None, input_consumed);
                }
                self.waiting_for_input = false;
                self.wait_return_ready = true;
                input_consumed = true;
            }

            if self.wait_frames > 0 {
                self.wait_frames -= 1;
                return (None, input_consumed);
            }

            if self.completed {
                return (None, input_consumed);
            }

            if !self.decoder.actions.is_empty() {
                let action = self.decoder.actions.remove(0);
                match action {
                    ScenarioAction::Wait { frames } => {
                        self.wait_frames = frames;
                        return (None, input_consumed);
                    }
                    ScenarioAction::WaitForInput => {
                        if input_advance {
                            input_consumed = true;
                            continue;
                        }
                        self.waiting_for_input = true;
                        return (None, input_consumed);
                    }
                    action => return (Some(action), input_consumed),
                }
            }

            let Some(command) = self.program.commands.get(self.pc).cloned() else {
                self.completed = true;
                return (None, input_consumed);
            };
            self.pc += 1;
            self.trace_command(&command);
            self.execute_command(&command);
        }

        tracing::warn!(
            pc = self.pc,
            "BCS VM instruction budget exhausted for one frame"
        );
        (None, input_consumed)
    }

    fn execute_command(&mut self, command: &BcsCommand) {
        match command.name {
            Some("push_dword" | "push_string") => {
                self.operand_stack.extend(command.args.iter().cloned());
            }
            Some("push_offset") => {
                let resolved = self.resolved_symbol.take();
                self.operand_stack
                    .extend(
                        command
                            .args
                            .iter()
                            .cloned()
                            .map(|value| match (&resolved, value) {
                                (Some(ResolvedSymbol::Local(base)), BcsValue::Addr(offset)) => {
                                    BcsValue::Addr(base.wrapping_add(offset as u32) as i32)
                                }
                                (_, value) => value,
                            }),
                    );
            }
            Some("push_base_offset") => {
                self.operand_stack
                    .extend(command.args.iter().map(|value| match value {
                        BcsValue::BaseOffset(offset) => {
                            BcsValue::MemoryAddr(self.memory.pointer.wrapping_sub(*offset))
                        }
                        value => value.clone(),
                    }));
            }
            Some("nargs") => {
                self.pending_arg_count = command
                    .args
                    .iter()
                    .filter_map(value_i32)
                    .last()
                    .and_then(|value| usize::try_from(value).ok());
            }
            Some("jmp") => self.jump_to_stack_address(),
            Some("jc") => {
                let target = command
                    .args
                    .iter()
                    .rev()
                    .find_map(value_i32)
                    .unwrap_or_else(|| self.pop_address());
                let condition = self.pop_i32();
                if condition != 0 {
                    self.jump_to(target);
                }
            }
            Some("call") => {
                let target = self.pop_address();
                self.call_stack.push(self.pc);
                self.jump_to(target);
            }
            Some("ret") => {
                if let Some(return_pc) = self.call_stack.pop() {
                    self.pc = return_pc;
                } else if let Some(frame) = self.external_call_stack.pop() {
                    self.program = frame.program;
                    self.address_to_pc = frame.address_to_pc;
                    self.pc = frame.pc;
                    self.operand_stack = frame.operand_stack;
                    self.memory = frame.memory;
                    self.call_stack = frame.call_stack;
                    self.pending_arg_count = frame.pending_arg_count;
                    self.resolved_symbol = frame.resolved_symbol;
                } else {
                    self.completed = true;
                }
            }
            Some("get_stack_pointer" | "load_base") => self
                .operand_stack
                .push(BcsValue::MemoryAddr(self.memory.pointer)),
            Some("set_stack_pointer" | "store_base") => {
                self.memory.pointer = self.pop_i32();
            }
            Some("write_mem_copy" | "move") => {
                let value = self.operand_stack.pop().unwrap_or(BcsValue::Int(0));
                let address = self.pop_memory_address();
                self.memory.write(address, value.clone());
                self.operand_stack.push(value);
            }
            Some("write_mem" | "move_arg") => {
                let address = self.pop_memory_address();
                let value = self.operand_stack.pop().unwrap_or(BcsValue::Int(0));
                self.memory.write(address, value);
            }
            Some("read_mem" | "load") => {
                let address = self.pop_memory_address();
                self.operand_stack.push(self.memory.read(address));
            }
            Some("global_set") => {
                let mut args = self.take_pending_args();
                let (index, value) = if args.len() >= 2 {
                    let value = args.pop().unwrap_or(BcsValue::Int(0));
                    let index = args.pop().and_then(|value| value_i32(&value)).unwrap_or(0);
                    (index, value)
                } else {
                    let value = self.operand_stack.pop().unwrap_or(BcsValue::Int(0));
                    (self.pop_i32(), value)
                };
                self.globals.insert(index, value);
            }
            Some("global_get") => {
                let args = self.take_pending_args();
                let index = args
                    .last()
                    .and_then(value_i32)
                    .unwrap_or_else(|| self.pop_i32());
                self.operand_stack.push(
                    self.globals
                        .get(&index)
                        .cloned()
                        .unwrap_or(BcsValue::Int(0)),
                );
            }
            Some(
                "add" | "sub" | "mul" | "div" | "mod" | "and" | "or" | "xor" | "shl" | "shr"
                | "sar",
            ) => {
                self.execute_binary_operator(command.name.unwrap_or_default());
            }
            Some("not" | "bool_zero") => {
                let value = self.pop_i32();
                let value = if command.name == Some("not") {
                    !value
                } else {
                    i32::from(value == 0)
                };
                self.operand_stack.push(BcsValue::Int(value));
            }
            Some("ternary") => {
                let false_value = self.operand_stack.pop().unwrap_or(BcsValue::Int(0));
                let true_value = self.operand_stack.pop().unwrap_or(BcsValue::Int(0));
                let condition = self.pop_i32();
                self.operand_stack.push(if condition != 0 {
                    true_value
                } else {
                    false_value
                });
            }
            Some("sin" | "cos") => {
                let angle = self.pop_i32() as f64 / 65_536.0;
                let value = if command.name == Some("sin") {
                    angle.sin()
                } else {
                    angle.cos()
                };
                self.operand_stack
                    .push(BcsValue::Int((value * 65_536.0) as i32));
            }
            Some("eq" | "neq" | "leq" | "geq" | "lt" | "gt" | "bool_and" | "bool_or") => {
                self.execute_comparison(command.name.unwrap_or_default());
            }
            Some("source_line" | "cmd0xe3" | "cmd0xe2" | "end_if") => {
                let _ = self.take_pending_args();
            }
            Some("check_translator_note") => {
                let _ = self.take_pending_args();
                self.operand_stack.push(BcsValue::Int(0));
            }
            Some("reg_exception_handler") => {
                let _ = self.pop_address();
            }
            Some("unreg_exception_handler") => {}
            Some("resolve_symbol") => self.resolve_symbol(),
            // Reverse trace in main shows 0xE5 feeds `eq 0` before `_WaitReturnValued`.
            // Treat it as the most recent wait result until the native ABI is fully recovered.
            _ if command.opcode == 0x0e5 => {
                let _ = self.take_pending_args();
                self.operand_stack
                    .push(BcsValue::Int(i32::from(std::mem::take(
                        &mut self.wait_return_ready,
                    ))));
            }
            _ => self.invoke_engine_command(command),
        }
    }

    fn trace_command(&self, command: &BcsCommand) {
        if std::env::var_os("DEBUG").is_none() {
            return;
        }
        tracing::info!(
            target: "bcs_vm",
            pc = self.pc.saturating_sub(1),
            addr = format_args!("0x{:08X}", command.addr),
            opcode = format_args!("0x{:03X}", command.opcode),
            name = command.name.unwrap_or("unknown"),
            stack_depth = self.operand_stack.len(),
            local_count = self.memory.values.len(),
            call_depth = self.call_stack.len(),
            external_call_depth = self.external_call_stack.len(),
            "BCS VM execute"
        );
    }

    fn invoke_engine_command(&mut self, command: &BcsCommand) {
        if matches!(command.opcode, 0x0f0 | 0x0f3) {
            self.invoke_external_script(command.opcode);
            return;
        }
        let args = if command.opcode == 0x1c {
            self.take_script_call_context()
        } else {
            self.take_pending_args()
        };
        self.decoder.observe_invocation(command, &args);
        if self
            .decoder
            .actions
            .iter()
            .any(|action| matches!(action, ScenarioAction::LoadScript { .. }))
        {
            // Script transfer opcodes are handled above; legacy observations
            // remain action-only and must not create an incomplete request.
        }
    }

    fn resolve_symbol(&mut self) {
        let Some(symbol_value) = self.operand_stack.pop() else {
            return;
        };
        let external_file = self.operand_stack.last().and_then(|value| match value {
            BcsValue::Str(file) if looks_like_script_name(file) => Some(file.clone()),
            _ => None,
        });
        if let Some(file) = external_file {
            self.operand_stack.pop();
            let symbol = match symbol_value {
                BcsValue::Str(symbol) => Some(symbol),
                BcsValue::Int(0) | BcsValue::Addr(0) => None,
                _ => None,
            };
            self.resolved_symbol = Some(ResolvedSymbol::External { file, symbol });
            return;
        }

        let BcsValue::Str(symbol) = symbol_value else {
            self.resolved_symbol = None;
            return;
        };
        self.resolved_symbol = self
            .program
            .symbols
            .iter()
            .find(|candidate| candidate.name == symbol)
            .map(|candidate| ResolvedSymbol::Local(candidate.addr));
        if self.resolved_symbol.is_none() {
            tracing::warn!(symbol, "BCS local symbol missing from resolver table");
        }
    }

    fn invoke_external_script(&mut self, opcode: u32) {
        let target = match self.resolved_symbol.take() {
            Some(ResolvedSymbol::External { file, symbol }) => Some((file, symbol)),
            _ => {
                let args = self.take_script_jump_context();
                let file = strings(&args)
                    .into_iter()
                    .find(|text| looks_like_script_name(text))
                    .map(str::to_string);
                file.map(|file| (file, None))
            }
        };
        let Some((file, symbol)) = target else {
            tracing::warn!(
                opcode = format_args!("0x{opcode:03X}"),
                "BCS script transfer target missing"
            );
            return;
        };
        // Both external-script forms preserve the caller. `main` provides the
        // minimal proof for F3: its entry executes `F3 MakerLogo` and the very
        // next instruction is `ret`, which returns control to title._bp.
        let transfer = ScriptTransfer::Call;
        self.awaiting_external_script = Some(PendingExternalScript {
            symbol: symbol.clone(),
            transfer,
        });
        self.decoder.actions.push(ScenarioAction::LoadScript {
            file,
            symbol,
            transfer,
        });
    }

    fn take_pending_args(&mut self) -> Vec<BcsValue> {
        let Some(count) = self.pending_arg_count.take() else {
            return Vec::new();
        };
        let split = self.operand_stack.len().saturating_sub(count);
        self.operand_stack.split_off(split)
    }

    fn take_script_call_context(&mut self) -> Vec<BcsValue> {
        let Some(function_index) = self
            .operand_stack
            .iter()
            .rposition(|value| matches!(value, BcsValue::Str(text) if text.starts_with('_')))
        else {
            return Vec::new();
        };
        let start = self
            .operand_stack
            .iter()
            .take(function_index)
            .rposition(|value| matches!(value, BcsValue::Str(text) if text.ends_with(".txt")))
            .map(|index| (index + 2).min(function_index))
            .unwrap_or_else(|| function_index.saturating_sub(40));
        self.operand_stack.split_off(start)
    }

    fn take_script_jump_context(&mut self) -> Vec<BcsValue> {
        if self.pending_arg_count.is_some() {
            return self.take_pending_args();
        }
        let Some(script_index) = self.operand_stack.iter().rposition(
            |value| matches!(value, BcsValue::Str(text) if looks_like_script_name(text)),
        ) else {
            return Vec::new();
        };
        if self.operand_stack.len().saturating_sub(script_index) > 4 {
            return Vec::new();
        }
        self.operand_stack.split_off(script_index)
    }

    fn jump_to_stack_address(&mut self) {
        let address = self.pop_address();
        self.jump_to(address);
    }

    fn jump_to(&mut self, address: i32) {
        let address = address as u32;
        if let Some(pc) = self.address_to_pc.get(&address).copied() {
            self.pc = pc;
        } else {
            tracing::warn!(
                address = format_args!("0x{address:08X}"),
                "BCS VM jump target missing"
            );
            self.completed = true;
        }
    }

    fn pop_address(&mut self) -> i32 {
        self.operand_stack
            .pop()
            .and_then(|value| value_i32(&value))
            .unwrap_or_default()
    }

    fn pop_i32(&mut self) -> i32 {
        self.pop_address()
    }

    fn pop_memory_address(&mut self) -> i32 {
        match self.operand_stack.pop() {
            Some(BcsValue::MemoryAddr(address)) => address,
            Some(BcsValue::BaseOffset(offset)) => self.memory.pointer.wrapping_sub(offset),
            Some(value) => value_i32(&value).unwrap_or_default(),
            None => 0,
        }
    }

    fn execute_binary_operator(&mut self, operator: &str) {
        let right = self.pop_i32();
        let left = self.pop_i32();
        let value = match operator {
            "add" => left.wrapping_add(right),
            "sub" => left.wrapping_sub(right),
            "mul" => left.wrapping_mul(right),
            "div" => {
                if right == 0 {
                    0
                } else {
                    left.wrapping_div(right)
                }
            }
            "mod" => {
                if right == 0 {
                    0
                } else {
                    left.wrapping_rem(right)
                }
            }
            "and" => left & right,
            "or" => left | right,
            "xor" => left ^ right,
            "shl" => left.wrapping_shl(right as u32),
            "shr" => ((left as u32) >> (right as u32)) as i32,
            "sar" => left >> (right as u32),
            _ => 0,
        };
        self.operand_stack.push(BcsValue::Int(value));
    }

    fn execute_comparison(&mut self, operator: &str) {
        let right = self.pop_i32();
        let left = self.pop_i32();
        let value = match operator {
            "eq" => left == right,
            "neq" => left != right,
            "leq" => left <= right,
            "geq" => left >= right,
            "lt" => left < right,
            "gt" => left > right,
            "bool_and" => left != 0 && right != 0,
            "bool_or" => left != 0 || right != 0,
            _ => false,
        };
        self.operand_stack.push(BcsValue::Int(i32::from(value)));
    }
}

#[derive(Debug, Default)]
struct ScenarioActionBuilder {
    actions: Vec<ScenarioAction>,
    scheduled_actions: Vec<ScenarioAction>,
    characters: BTreeMap<i32, CharacterState>,
}

impl ScenarioActionBuilder {
    fn observe_invocation(&mut self, command: &BcsCommand, args: &[BcsValue]) {
        if command.opcode == 0x1c {
            self.observe_script_call(args);
        } else if matches!(
            command.name,
            Some("sprite" | "bg" | "bg_transition" | "bg240")
        ) {
            self.observe_visual_command(args);
        } else if matches!(command.name, Some("sprite_hide" | "sprite_hide_all")) {
            self.observe_sprite_hide(args);
        } else if matches!(command.name, Some("wait")) {
            self.observe_wait(args);
        } else if command.name == Some("cmd0x120") {
            if ints(args).last().copied().unwrap_or_default() != 0 {
                self.actions.push(ScenarioAction::Wait { frames: 0 });
            }
        } else if matches!(command.name, Some("say" | "msg")) {
            self.observe_message(args);
        } else if matches!(command.name, Some("sound" | "sound_1a0" | "snd")) {
            self.observe_sound_command(command, args);
        } else if matches!(command.name, Some("exec_script")) || command.opcode == 0xf3 {
            self.observe_script_jump(args);
        }
    }

    fn observe_script_call(&mut self, args: &[BcsValue]) {
        let Some(function) = last_string(args).filter(|text| text.starts_with('_')) else {
            return;
        };
        match function {
            "_PlaySE" => {
                if let Some(file) = strings(args)
                    .into_iter()
                    .rev()
                    .find(|text| text.starts_with("se_"))
                {
                    self.actions.push(ScenarioAction::Sound {
                        file: file.to_string(),
                    });
                }
            }
            "_PlayVoice" => {
                if let Some(file) = strings(args)
                    .into_iter()
                    .rev()
                    .find(|text| looks_like_resource_name(text))
                {
                    self.actions.push(ScenarioAction::Sound {
                        file: file.to_string(),
                    });
                }
            }
            "_PlayBGM" => {
                if let Some(file) = strings(args)
                    .into_iter()
                    .rev()
                    .find(|text| looks_like_resource_name(text))
                {
                    self.actions.push(ScenarioAction::Bgm {
                        file: file.to_string(),
                    });
                }
            }
            "_DrawScene" => {
                if let Some(file) = visual_resource_name(args) {
                    self.actions.push(ScenarioAction::ClearSprite);
                    self.push_sprite_with_hints(
                        file.to_string(),
                        duration_frames(args).unwrap_or(1),
                        draw_scene_hints(args),
                    );
                }
            }
            "_SetBSsize" => {
                let ints = ints(args);
                if let Some((slot, size)) = last_two(&ints) {
                    self.characters.entry(slot).or_default().size = Some(size);
                }
            }
            "_SetClothes" => {
                let ints = ints(args);
                if let Some((slot, clothes)) = last_two(&ints) {
                    self.characters.entry(slot).or_default().clothes = Some(clothes);
                }
            }
            "_BS_P" | "_BS" => {
                if let Some(sprite) = self.character_sprite_from_bs(args) {
                    self.schedule_sprite_with_hints(
                        sprite.file,
                        sprite.wait_frames,
                        VisualHints {
                            slot: Some(sprite.slot),
                            x: sprite.x,
                            z: Some(70 + sprite.slot),
                            opacity: None,
                            body_layer: sprite.body_layer,
                        },
                    );
                }
            }
            "_MBS_P" | "_MBS" | "_FBS_P" | "_FBS" => {
                if let Some(transform) = transform_hints(args) {
                    self.scheduled_actions
                        .push(ScenarioAction::TransformSprite {
                            slot: transform.slot,
                            wait_frames: transform.wait_frames,
                            x: transform.x,
                            y: transform.y,
                            opacity: transform.opacity,
                        });
                } else if let Some(frames) = duration_frames(args) {
                    self.actions.push(ScenarioAction::Wait { frames });
                }
            }
            "_SPR_P" => {
                if let Some(file) = visual_resource_name(args) {
                    let hints = visual_hints(args);
                    self.schedule_sprite_with_hints(
                        file.to_string(),
                        duration_frames(args).unwrap_or(1),
                        hints,
                    );
                }
            }
            "_SPR" => {
                if let Some(file) = visual_resource_name(args) {
                    let hints = visual_hints(args);
                    self.push_sprite_with_hints(
                        file.to_string(),
                        duration_frames(args).unwrap_or(1),
                        hints,
                    );
                }
            }
            "_FSPR_P" | "_FSPR" => {
                if let Some(transform) = transform_hints(args) {
                    self.scheduled_actions
                        .push(ScenarioAction::TransformSprite {
                            slot: transform.slot,
                            wait_frames: transform.wait_frames,
                            x: transform.x,
                            y: transform.y,
                            opacity: transform.opacity,
                        });
                }
            }
            "_MSPR_P" | "_MSPR" => {
                if let Some(transform) = transform_hints(args) {
                    self.scheduled_actions
                        .push(ScenarioAction::TransformSprite {
                            slot: transform.slot,
                            wait_frames: transform.wait_frames,
                            x: transform.x,
                            y: transform.y,
                            opacity: transform.opacity,
                        });
                } else if let Some(frames) = duration_frames(args) {
                    self.actions.push(ScenarioAction::Wait { frames });
                }
            }
            "_Exec_P" | "_Exec" | "_ExecuteScheduledControl" => {
                let scheduled_wait = self.flush_scheduled_actions();
                if let Some(frames) = duration_frames(args).or(scheduled_wait) {
                    self.actions.push(ScenarioAction::Wait { frames });
                }
            }
            "_FadeScene" => {
                let frames = duration_frames(args).unwrap_or(1);
                self.actions.push(ScenarioAction::HideSprite {
                    slot: None,
                    wait_frames: frames,
                });
                self.actions.push(ScenarioAction::Wait { frames });
            }
            "_Wait" | "_WaitReturnValued" => {
                if function == "_Wait" {
                    if let Some(frames) = duration_frames(args) {
                        self.actions.push(ScenarioAction::Wait { frames });
                    }
                } else {
                    self.actions.push(ScenarioAction::WaitForInput);
                }
            }
            _ => {}
        }
    }

    fn observe_visual_command(&mut self, args: &[BcsValue]) {
        if let Some(file) = visual_resource_name(args) {
            self.push_sprite_with_hints(
                file.to_string(),
                duration_frames(args).unwrap_or(1),
                visual_hints(args),
            );
        }
    }

    fn observe_sprite_hide(&mut self, args: &[BcsValue]) {
        let hints = visual_hints(args);
        let frames = duration_frames(args).unwrap_or(1);
        self.actions.push(ScenarioAction::HideSprite {
            slot: hints.slot,
            wait_frames: frames,
        });
        self.actions.push(ScenarioAction::Wait { frames });
    }

    fn observe_wait(&mut self, args: &[BcsValue]) {
        if let Some(frames) = duration_frames(args) {
            self.actions.push(ScenarioAction::Wait { frames });
        }
    }

    fn observe_message(&mut self, args: &[BcsValue]) {
        let mut speaker = None;
        for text in strings(args) {
            if is_message_text(text) {
                if is_message_marker(text) {
                    continue;
                }
                if is_speaker_label(text) {
                    speaker = Some(text.to_string());
                    continue;
                }
                self.actions.push(ScenarioAction::Message {
                    speaker: speaker.take(),
                    text: text.to_string(),
                });
                if should_wait_after_message(text) {
                    self.actions.push(ScenarioAction::WaitForInput);
                }
            }
        }
    }

    fn observe_sound_command(&mut self, command: &BcsCommand, args: &[BcsValue]) {
        let Some(file) = strings(args)
            .into_iter()
            .rev()
            .find(|text| looks_like_resource_name(text))
        else {
            return;
        };
        if matches!(command.name, Some("sound" | "sound_1a0"))
            && file.to_ascii_lowercase().starts_with("bgm")
        {
            self.actions.push(ScenarioAction::Bgm {
                file: file.to_string(),
            });
        } else {
            self.actions.push(ScenarioAction::Sound {
                file: file.to_string(),
            });
        }
    }

    fn push_sprite_with_hints(&mut self, file: String, frames: u32, hints: VisualHints) {
        self.actions.push(ScenarioAction::Sprite {
            file,
            wait_frames: frames,
            slot: hints.slot,
            x: hints.x,
            z: hints.z,
            opacity: hints.opacity,
            body_layer: hints.body_layer,
        });
        self.actions.push(ScenarioAction::Wait { frames });
    }

    fn schedule_sprite_with_hints(&mut self, file: String, frames: u32, hints: VisualHints) {
        self.scheduled_actions.push(ScenarioAction::Sprite {
            file,
            wait_frames: frames,
            slot: hints.slot,
            x: hints.x,
            z: hints.z,
            opacity: hints.opacity,
            body_layer: hints.body_layer,
        });
    }

    fn flush_scheduled_actions(&mut self) -> Option<u32> {
        if self.scheduled_actions.is_empty() {
            return None;
        }
        let wait_frames = self
            .scheduled_actions
            .iter()
            .filter_map(action_wait_frames)
            .max();
        self.actions.append(&mut self.scheduled_actions);
        wait_frames
    }

    fn observe_script_jump(&mut self, args: &[BcsValue]) {
        let Some(file) = strings(args)
            .into_iter()
            .rev()
            .find(|text| looks_like_script_name(text))
        else {
            return;
        };
        self.actions.push(ScenarioAction::LoadScript {
            file: file.to_string(),
            symbol: None,
            transfer: ScriptTransfer::Jump,
        });
    }

    fn character_sprite_from_bs(&self, args: &[BcsValue]) -> Option<CharacterSpriteCall> {
        let slot = value_i32(args.first()?)?;
        let body = args.get(1).and_then(value_i32).unwrap_or(1);
        let names = args
            .iter()
            .filter_map(|value| match value {
                BcsValue::Str(text)
                    if !text.starts_with('_')
                        && !text.ends_with(".txt")
                        && !text.eq_ignore_ascii_case("macro_story") =>
                {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let pose = names.first().copied().unwrap_or("a");
        let face = names.get(1).copied()?;
        let state = self.characters.get(&slot).copied().unwrap_or_default();
        let character = character_name(slot)?;
        let size = size_prefix(state.size.unwrap_or(2));
        let clothes = state.clothes.unwrap_or(1).clamp(1, 9);
        let wait_frames = duration_frames(args).unwrap_or(1);
        Some(CharacterSpriteCall {
            file: format!(
                "{size}_{character}_{clothes}_d_{}{}",
                body.clamp(1, 9),
                face
            ),
            wait_frames,
            slot,
            x: None,
            body_layer: character_body_layer_for_pose(pose),
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct CharacterState {
    size: Option<i32>,
    clothes: Option<i32>,
}

#[derive(Debug, Clone)]
struct CharacterSpriteCall {
    file: String,
    wait_frames: u32,
    slot: i32,
    x: Option<i32>,
    body_layer: Option<&'static str>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct VisualHints {
    pub(crate) slot: Option<i32>,
    pub(crate) x: Option<i32>,
    pub(crate) z: Option<i32>,
    pub(crate) opacity: Option<f32>,
    pub(crate) body_layer: Option<&'static str>,
}

fn strings(values: &[BcsValue]) -> Vec<&str> {
    let mut out = Vec::new();
    for value in values {
        collect_strings(value, &mut out);
    }
    out
}

fn ints(values: &[BcsValue]) -> Vec<i32> {
    values.iter().filter_map(value_i32).collect()
}

fn last_two(values: &[i32]) -> Option<(i32, i32)> {
    Some((*values.get(values.len().checked_sub(2)?)?, *values.last()?))
}

fn collect_strings<'a>(value: &'a BcsValue, out: &mut Vec<&'a str>) {
    match value {
        BcsValue::Str(text) => out.push(text.as_str()),
        BcsValue::Mul(left, right) => {
            collect_strings(left, out);
            collect_strings(right, out);
        }
        BcsValue::CheckNote(value) => collect_strings(value, out),
        _ => {}
    }
}

fn last_string(values: &[BcsValue]) -> Option<&str> {
    strings(values).into_iter().last()
}

fn visual_resource_name(values: &[BcsValue]) -> Option<&str> {
    strings(values).into_iter().rev().find(|text| {
        looks_like_resource_name(text)
            && !text.starts_with("se_")
            && !text.starts_with("bgm")
            && !text.starts_with("BGM")
    })
}

fn character_name(slot: i32) -> Option<&'static str> {
    match slot {
        1 => Some("koh"),
        3 => Some("hih"),
        15 => Some("tay"),
        _ => None,
    }
}

fn size_prefix(size: i32) -> &'static str {
    match size {
        0 | 1 => "L",
        3 => "M",
        4 => "S",
        _ => "LL",
    }
}

fn character_body_layer_for_pose(pose: &str) -> Option<&'static str> {
    match pose {
        "a" => Some("ax11"),
        "d" => Some("ax12"),
        "c" => Some("ax14"),
        _ => None,
    }
}

fn visual_hints(values: &[BcsValue]) -> VisualHints {
    let file_index = values
        .iter()
        .rposition(|value| matches!(value, BcsValue::Str(text) if looks_like_resource_name(text)));
    let ints = values.iter().filter_map(value_i32).collect::<Vec<_>>();
    if let Some(file_index) = file_index {
        let before_file = values[..file_index]
            .iter()
            .filter_map(value_i32)
            .collect::<Vec<_>>();
        let after_file = values[file_index + 1..]
            .iter()
            .filter_map(value_i32)
            .collect::<Vec<_>>();
        let duration = duration_frames(values).and_then(|frames| i32::try_from(frames * 16).ok());
        let slot = before_file
            .last()
            .copied()
            .filter(|slot| (0..=99).contains(slot))
            .or_else(|| {
                after_file
                    .last()
                    .copied()
                    .filter(|slot| (0..=99).contains(slot))
            });
        let z = after_file.iter().rev().copied().find(|value| {
            Some(*value) != slot
                && Some(*value) != duration
                && *value != 256
                && (2..=899).contains(value)
        });
        return VisualHints {
            slot,
            x: after_file.iter().copied().find(|x| {
                Some(*x) != duration
                    && Some(*x) != slot
                    && *x != 0
                    && *x != 1
                    && *x != 256
                    && (-1280..=1280).contains(x)
            }),
            z,
            opacity: None,
            body_layer: None,
        };
    }

    let slot = ints
        .iter()
        .rev()
        .copied()
        .find(|value| (0..=99).contains(value));
    let x = file_index.and_then(|index| {
        values
            .iter()
            .skip(index + 1)
            .filter_map(value_i32)
            .find(|value| (-640..=640).contains(value))
    });
    let z = ints
        .iter()
        .rev()
        .copied()
        .find(|value| (1..=160).contains(value));
    VisualHints {
        slot,
        x,
        z,
        opacity: None,
        body_layer: None,
    }
}

fn draw_scene_hints(values: &[BcsValue]) -> VisualHints {
    let mut hints = visual_hints(values);
    hints.slot = None;
    hints.x = None;
    hints.z = Some(10);
    hints.opacity = None;
    hints
}

#[derive(Debug, Clone, Copy)]
struct TransformHints {
    slot: i32,
    wait_frames: u32,
    x: Option<i32>,
    y: Option<i32>,
    opacity: Option<f32>,
}

fn transform_hints(values: &[BcsValue]) -> Option<TransformHints> {
    let ints = values.iter().filter_map(value_i32).collect::<Vec<_>>();
    let slot = ints
        .first()
        .copied()
        .filter(|slot| (0..=99).contains(slot))?;
    let wait_frames = duration_frames(values).unwrap_or(1);
    let duration_ms = duration_millis(values);
    let positive_opacity =
        ints.iter().rev().copied().find(|value| {
            (2..=256).contains(value) && *value != slot && Some(*value) != duration_ms
        });
    let opacity = positive_opacity.map(script_opacity);
    let coord_values = ints
        .iter()
        .skip(1)
        .copied()
        .filter(|value| {
            Some(*value) != duration_ms
                && Some(*value) != positive_opacity
                && *value != i32::MIN + 1
                && (-1280..=1280).contains(value)
        })
        .collect::<Vec<_>>();
    let x = coord_values
        .iter()
        .copied()
        .find(|value| *value != -1 && *value != 0 && *value != 1 && *value != 256);
    Some(TransformHints {
        slot,
        wait_frames,
        x,
        y: None,
        opacity,
    })
}

fn script_opacity(value: i32) -> f32 {
    (value as f32 / 256.0).clamp(0.0, 1.0)
}

fn action_wait_frames(action: &ScenarioAction) -> Option<u32> {
    match action {
        ScenarioAction::Sprite { wait_frames, .. }
        | ScenarioAction::HideSprite { wait_frames, .. }
        | ScenarioAction::TransformSprite { wait_frames, .. } => Some(*wait_frames),
        ScenarioAction::Wait { frames } => Some(*frames),
        _ => None,
    }
}

fn looks_like_resource_name(text: &str) -> bool {
    !text.is_empty()
        && !text.ends_with(".txt")
        && !text.starts_with('_')
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn looks_like_script_name(text: &str) -> bool {
    matches!(text, "_GM" | "_00")
        || (!text.is_empty()
            && !text.ends_with(".txt")
            && !text.starts_with('_')
            && text
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
}

fn duration_frames(values: &[BcsValue]) -> Option<u32> {
    duration_millis(values).map(|duration| (duration as u32).div_ceil(16).max(1))
}

fn duration_millis(values: &[BcsValue]) -> Option<i32> {
    values
        .iter()
        .filter_map(value_i32)
        .filter(|value| (16..=30_000).contains(value))
        .last()
}

fn value_i32(value: &BcsValue) -> Option<i32> {
    match value {
        BcsValue::Int(value)
        | BcsValue::Addr(value)
        | BcsValue::BaseOffset(value)
        | BcsValue::MemoryAddr(value) => Some(*value),
        BcsValue::Mul(left, right) => Some(value_i32(left)? * value_i32(right)?),
        BcsValue::CheckNote(value) => value_i32(value),
        BcsValue::Line { .. } | BcsValue::Arg2 | BcsValue::Str(_) => None,
    }
}

fn is_message_text(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('_')
        && !text.ends_with(".txt")
        && !text.starts_with("se_")
        && !text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_message_marker(text: &str) -> bool {
    matches!(text, "空")
}

fn is_speaker_label(text: &str) -> bool {
    let char_count = text.chars().count();
    char_count <= 12
        && !text.contains('\n')
        && !text.chars().any(|ch| {
            matches!(
                ch,
                '「' | '」'
                    | '『'
                    | '』'
                    | '（'
                    | '）'
                    | '。'
                    | '、'
                    | '，'
                    | '．'
                    | '！'
                    | '？'
                    | '!'
                    | '?'
                    | '…'
                    | 'ー'
            )
        })
}

fn should_wait_after_message(text: &str) -> bool {
    !is_message_marker(text) && !is_speaker_label(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(
        addr: u32,
        opcode: u32,
        name: Option<&'static str>,
        args: Vec<BcsValue>,
    ) -> BcsCommand {
        BcsCommand {
            file_offset: addr as usize,
            addr,
            opcode,
            name,
            args,
            string_refs: Vec::new(),
        }
    }

    fn program(commands: Vec<BcsCommand>) -> BcsProgram {
        BcsProgram {
            header_size: 0,
            namespaces: Vec::new(),
            subs: Vec::new(),
            symbols: Vec::new(),
            code_start: 0,
            code_end: 0,
            executable_end: 0,
            resolver_end: None,
            commands,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn follows_jump_instead_of_precollecting_skipped_actions() {
        let program = program(vec![
            command(0x1c, 0, Some("push_dword"), vec![BcsValue::Addr(0x4c)]),
            command(0x24, 0x18, Some("jmp"), Vec::new()),
            command(
                0x2c,
                0,
                Some("push_string"),
                vec![BcsValue::Str("「skipped」".into())],
            ),
            command(0x34, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x3c, 0x140, Some("say"), Vec::new()),
            command(0x44, 0x1b, Some("ret"), Vec::new()),
            command(
                0x4c,
                0,
                Some("push_string"),
                vec![BcsValue::Str("「visited」".into())],
            ),
            command(0x54, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x5c, 0x140, Some("say"), Vec::new()),
            command(0x64, 0x1b, Some("ret"), Vec::new()),
        ]);
        let mut playback = ScenarioPlayback::from_program(program).expect("program");

        let (action, consumed) = playback.tick(false);

        assert!(!consumed);
        assert!(matches!(
            action,
            Some(ScenarioAction::Message { text, .. }) if text == "「visited」"
        ));
    }

    #[test]
    fn conditional_jump_uses_its_encoded_target_not_the_operand_stack() {
        let program = program(vec![
            command(0x1c, 0, Some("push_dword"), vec![BcsValue::Int(1)]),
            command(0x24, 0x19, Some("jc"), vec![BcsValue::Addr(0x54)]),
            command(
                0x2c,
                0,
                Some("push_string"),
                vec![BcsValue::Str("「fallthrough」".into())],
            ),
            command(0x34, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x3c, 0x140, Some("say"), Vec::new()),
            command(0x44, 0x1b, Some("ret"), Vec::new()),
            command(
                0x54,
                0,
                Some("push_string"),
                vec![BcsValue::Str("「taken」".into())],
            ),
            command(0x5c, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x64, 0x140, Some("say"), Vec::new()),
            command(0x6c, 0x1b, Some("ret"), Vec::new()),
        ]);
        let mut playback = ScenarioPlayback::from_program(program).expect("program");

        assert!(matches!(
            playback.tick(false).0,
            Some(ScenarioAction::Message { text, .. }) if text == "「taken」"
        ));
    }

    #[test]
    fn external_script_return_restores_calling_program() {
        let parent = program(vec![
            command(
                0x1c,
                0,
                Some("push_string"),
                vec![BcsValue::Str("child".into())],
            ),
            command(0x24, 0, Some("push_dword"), vec![BcsValue::Int(0)]),
            command(0x2c, 0x0f0, Some("exec_script"), Vec::new()),
            command(
                0x34,
                0,
                Some("push_string"),
                vec![BcsValue::Str("「returned」".into())],
            ),
            command(0x3c, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x44, 0x140, Some("say"), Vec::new()),
            command(0x4c, 0x1b, Some("ret"), Vec::new()),
        ]);
        let child = program(vec![command(0x1c, 0x1b, Some("ret"), Vec::new())]);
        let mut playback = ScenarioPlayback::from_program(parent).expect("parent");

        assert!(matches!(
            playback.tick(false).0,
            Some(ScenarioAction::LoadScript { file, transfer: ScriptTransfer::Call, .. }) if file == "child"
        ));
        assert!(playback.append_program(child).is_some());
        assert!(matches!(
            playback.tick(false).0,
            Some(ScenarioAction::Message { text, .. }) if text == "「returned」"
        ));
    }

    #[test]
    fn legacy_external_script_return_restores_calling_program() {
        let parent = program(vec![
            command(
                0x1c,
                0,
                Some("push_string"),
                vec![BcsValue::Str("MakerLogo".into())],
            ),
            command(0x24, 0, Some("push_dword"), vec![BcsValue::Int(0)]),
            command(0x2c, 0x0f3, Some("exec_script_legacy"), Vec::new()),
            command(0x34, 0x1b, Some("ret"), Vec::new()),
        ]);
        let child = program(vec![command(0x1c, 0x1b, Some("ret"), Vec::new())]);
        let mut playback = ScenarioPlayback::from_program(parent).expect("parent");

        assert!(matches!(
            playback.tick(false).0,
            Some(ScenarioAction::LoadScript { file, transfer: ScriptTransfer::Call, .. })
                if file == "MakerLogo"
        ));
        assert!(playback.append_program(child).is_some());
        assert!(playback.tick(false).0.is_none());
        assert!(playback.completed);
    }

    #[test]
    fn f6_resolves_a_local_label_before_call() {
        let mut program = program(vec![
            command(
                0x1c,
                0x03,
                Some("push_string"),
                vec![BcsValue::Str("__entry".into())],
            ),
            command(0x24, 0x0f6, Some("resolve_symbol"), Vec::new()),
            command(0x28, 0x01, Some("push_offset"), vec![BcsValue::Addr(0)]),
            command(0x30, 0x1a, Some("call"), Vec::new()),
            command(0x34, 0x1b, Some("ret"), Vec::new()),
            command(
                0x50,
                0x03,
                Some("push_string"),
                vec![BcsValue::Str("「resolved」".into())],
            ),
            command(0x58, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x60, 0x140, Some("say"), Vec::new()),
            command(0x68, 0x1b, Some("ret"), Vec::new()),
        ]);
        program.symbols.push(BcsSymbol {
            name: "__entry".into(),
            addr: 0x50,
            table_offset: 0,
        });
        let mut playback = ScenarioPlayback::from_program(program).expect("program");

        assert!(matches!(
            playback.tick(false).0,
            Some(ScenarioAction::Message { text, .. }) if text == "「resolved」"
        ));
    }

    #[test]
    fn executes_frame_memory_reads_writes_and_a_conditional_branch() {
        let program = program(vec![
            command(0x1c, 0x10, Some("get_stack_pointer"), Vec::new()),
            command(0x20, 0, Some("push_dword"), vec![BcsValue::Int(8)]),
            command(0x28, 0x20, Some("add"), Vec::new()),
            command(0x2c, 0x11, Some("set_stack_pointer"), Vec::new()),
            command(0x30, 0, Some("push_dword"), vec![BcsValue::Int(123)]),
            command(
                0x38,
                0x2,
                Some("push_base_offset"),
                vec![BcsValue::BaseOffset(4)],
            ),
            command(0x40, 0x0a, Some("write_mem"), vec![BcsValue::Int(2)]),
            command(
                0x44,
                0x2,
                Some("push_base_offset"),
                vec![BcsValue::BaseOffset(4)],
            ),
            command(0x4c, 0x08, Some("read_mem"), vec![BcsValue::Int(2)]),
            command(0x54, 0, Some("push_dword"), vec![BcsValue::Int(123)]),
            command(0x5c, 0x30, Some("eq"), Vec::new()),
            command(0x60, 0x19, Some("jc"), vec![BcsValue::Addr(0x90)]),
            command(
                0x68,
                0x3,
                Some("push_string"),
                vec![BcsValue::Str("「wrong branch」".into())],
            ),
            command(0x70, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x78, 0x140, Some("say"), Vec::new()),
            command(0x80, 0x1b, Some("ret"), Vec::new()),
            command(
                0x90,
                0x3,
                Some("push_string"),
                vec![BcsValue::Str("「frame memory works」".into())],
            ),
            command(0x98, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0xa0, 0x140, Some("say"), Vec::new()),
            command(0xa8, 0x1b, Some("ret"), Vec::new()),
        ]);
        let mut playback = ScenarioPlayback::from_program(program).expect("program");

        assert!(matches!(
            playback.tick(false).0,
            Some(ScenarioAction::Message { text, .. }) if text == "「frame memory works」"
        ));
    }

    #[test]
    fn global_set_and_get_follow_the_compiler_stack_contract() {
        let program = program(vec![
            command(0x1c, 0, Some("push_dword"), vec![BcsValue::Int(9)]),
            command(0x24, 0, Some("push_dword"), vec![BcsValue::Int(9)]),
            command(0x2c, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x34, 0xe1, Some("global_get"), Vec::new()),
            command(0x3c, 0, Some("push_dword"), vec![BcsValue::Int(1)]),
            command(0x44, 0x20, Some("add"), Vec::new()),
            command(0x4c, 0x3f, Some("nargs"), vec![BcsValue::Int(2)]),
            command(0x54, 0xe0, Some("global_set"), Vec::new()),
            command(0x5c, 0, Some("push_dword"), vec![BcsValue::Int(9)]),
            command(0x64, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x6c, 0xe1, Some("global_get"), Vec::new()),
            command(0x74, 0, Some("push_dword"), vec![BcsValue::Int(1)]),
            command(0x7c, 0x30, Some("eq"), Vec::new()),
            command(0x84, 0x19, Some("jc"), vec![BcsValue::Addr(0xa4)]),
            command(0x8c, 0x1b, Some("ret"), Vec::new()),
            command(
                0xa4,
                0x3,
                Some("push_string"),
                vec![BcsValue::Str("「global state works」".into())],
            ),
            command(0xac, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0xb4, 0x140, Some("say"), Vec::new()),
            command(0xbc, 0x1b, Some("ret"), Vec::new()),
        ]);
        let mut playback = ScenarioPlayback::from_program(program).expect("program");

        assert!(matches!(
            playback.tick(false).0,
            Some(ScenarioAction::Message { text, .. }) if text == "「global state works」"
        ));
        assert_eq!(playback.globals.get(&9), Some(&BcsValue::Int(1)));
        assert!(playback.operand_stack.is_empty());
        assert!(playback.pending_arg_count.is_none());
    }

    #[test]
    fn scheduler_yield_resumes_on_the_next_frame() {
        let program = program(vec![
            command(0x1c, 0, Some("push_dword"), vec![BcsValue::Int(1)]),
            command(0x24, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x2c, 0x120, Some("cmd0x120"), Vec::new()),
            command(
                0x34,
                0x3,
                Some("push_string"),
                vec![BcsValue::Str("「after yield」".into())],
            ),
            command(0x3c, 0x3f, Some("nargs"), vec![BcsValue::Int(1)]),
            command(0x44, 0x140, Some("say"), Vec::new()),
            command(0x4c, 0x1b, Some("ret"), Vec::new()),
        ]);
        let mut playback = ScenarioPlayback::from_program(program).expect("program");

        assert!(playback.tick(false).0.is_none());
        assert!(matches!(
            playback.tick(false).0,
            Some(ScenarioAction::Message { text, .. }) if text == "「after yield」"
        ));
    }
}
