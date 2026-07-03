use crate::calls::instruction_call_key;
use crate::{BpInstruction, BpOperand, BpProgram};

#[derive(Debug, Clone)]
pub struct DecompileOptions {
    pub limit: Option<usize>,
    pub show_stack: bool,
}

impl Default for DecompileOptions {
    fn default() -> Self {
        Self {
            limit: None,
            show_stack: false,
        }
    }
}

pub fn decompile_bp(program: &BpProgram, options: &DecompileOptions) -> String {
    let mut state = DecompileState {
        stack: Vec::new(),
        lines: Vec::new(),
        show_stack: options.show_stack,
        temp_counter: 0,
    };

    if let Some(name) = &program.script_name {
        state.lines.push(format!("// script {name}"));
    }
    for function in &program.functions {
        state
            .lines
            .push(format!("// function sub_{:08X}", function.offset));
    }

    for instruction in program
        .instructions
        .iter()
        .take(options.limit.unwrap_or(usize::MAX))
    {
        state.visit(instruction);
    }

    state.lines.join("\n")
}

struct DecompileState {
    stack: Vec<String>,
    lines: Vec<String>,
    show_stack: bool,
    temp_counter: usize,
}

impl DecompileState {
    fn visit(&mut self, instruction: &BpInstruction) {
        let offset = instruction.offset;
        let name = instruction.opcode_name.as_str();
        match name {
            "push_byte" | "push_word" | "push_dword" => {
                self.push(format!("{}", operand_i32(instruction).unwrap_or_default()));
            }
            "push_base_offset" => {
                let value = operand_u32(instruction).unwrap_or_default();
                self.push(format!("base[-0x{value:X}]"));
            }
            "push_string" => {
                self.push(match instruction.operands.first() {
                    Some(BpOperand::String(text)) => format!("{text:?}"),
                    Some(BpOperand::Offset(value)) => format!("str_0x{value:08X}"),
                    _ => "\"<bad-string>\"".to_string(),
                });
            }
            "push_offset" => {
                let value = operand_u32(instruction).unwrap_or_default();
                self.push(format!("sub_0x{value:08X}"));
            }
            "load_base" => self.push("base".to_string()),
            "store_base" => {
                let value = self.pop();
                self.emit(offset, format!("base = {value};"));
            }
            "load" => {
                let width = operand_u32(instruction).unwrap_or_default();
                let ptr = self.pop();
                self.push(format!("load{width}({ptr})"));
            }
            "move" => {
                let width = operand_u32(instruction).unwrap_or_default();
                let value = self.pop();
                let ptr = self.pop();
                self.emit(offset, format!("store{width}({ptr}, {value});"));
            }
            "move_arg" => {
                let width = operand_u32(instruction).unwrap_or_default();
                let ptr = self.pop();
                let value = self.pop();
                self.emit(offset, format!("store_arg{width}({ptr}, {value});"));
            }
            "copy_stack" => {
                let width = operand_u32(instruction).unwrap_or_default();
                let count = instruction
                    .operands
                    .get(1)
                    .and_then(operand_as_u32)
                    .unwrap_or_default() as usize;
                let mut values = Vec::new();
                for _ in 0..count {
                    values.push(self.pop());
                }
                values.reverse();
                let dst = self.pop();
                self.emit(
                    offset,
                    format!("copy_stack{width}({dst}, [{}]);", values.join(", ")),
                );
            }
            "jmp" => {
                let dest = self.pop();
                self.emit(offset, format!("goto {dest};"));
            }
            "jc" => {
                let kind = operand_u32(instruction).unwrap_or_default();
                let dest = self.pop();
                let condition = self.pop();
                self.emit(
                    offset,
                    format!("if {} goto {dest};", condition_for_jc(kind, &condition)),
                );
            }
            "call" => {
                let dest = self.pop();
                self.emit(offset, format!("call {dest};"));
            }
            "ret" | "script_ret" => self.emit(offset, "return;".to_string()),
            "add" | "sub" | "mul" | "div" | "mod" | "and" | "or" | "xor" | "shl" | "shr"
            | "sar" | "eq" | "neq" | "leq" | "geq" | "lt" | "gt" | "dnotzero" | "dnotzero2" => {
                let right = self.pop();
                let left = self.pop();
                self.push(format!("({left} {} {right})", binary_symbol(name)));
            }
            "not" => {
                let value = self.pop();
                self.push(format!("~({value})"));
            }
            "bool_zero" => {
                let value = self.pop();
                self.push(format!("({value} == 0)"));
            }
            "ternary" => {
                let false_value = self.pop();
                let true_value = self.pop();
                let condition = self.pop();
                self.push(format!("({condition} ? {true_value} : {false_value})"));
            }
            "muldiv" => {
                let divisor = self.pop();
                let multiplier = self.pop();
                let multiplicand = self.pop();
                self.push(format!("muldiv({multiplicand}, {multiplier}, {divisor})"));
            }
            "sin" | "cos" => {
                let value = self.pop();
                self.push(format!("{name}({value})"));
            }
            "memcpy" | "memclr" | "memset" | "memcmp" | "strreplace" | "strlen" | "streq"
            | "strcpy" | "strconcat" | "getchar" | "tolower" | "sprintf" | "malloc" | "free"
            | "addmemboundary" | "confirm" | "message_box" | "assert" | "dumpmem" => {
                self.emit_builtin(offset, name);
            }
            "sys1" | "sys2" | "grp1" | "grp2" | "grp3" | "snd1" | "usr1" | "usr2" => {
                self.emit_dispatch(offset, instruction);
            }
            "script_load" | "script_free" | "script_call" => {
                self.emit(offset, format!("{name}();"));
            }
            _ => self.emit(
                offset,
                format!("{}({});", name, format_operands(&instruction.operands)),
            ),
        }
    }

    fn push(&mut self, value: String) {
        self.stack.push(value);
    }

    fn pop(&mut self) -> String {
        self.stack.pop().unwrap_or_else(|| {
            let name = format!("stack_underflow_{}", self.temp_counter);
            self.temp_counter += 1;
            name
        })
    }

    fn emit(&mut self, offset: u64, body: String) {
        if self.show_stack {
            self.lines.push(format!(
                "{offset:08X}: {body:<72} // stack=[{}]",
                self.stack.join(", ")
            ));
        } else {
            self.lines.push(format!("{offset:08X}: {body}"));
        }
    }

    fn emit_builtin(&mut self, offset: u64, name: &str) {
        let arity = match name {
            "strlen" | "getchar" | "tolower" | "malloc" | "free" | "confirm" | "assert" => 1,
            "memclr" | "streq" | "strcpy" | "message_box" | "dumpmem" => 2,
            "memcpy" | "memset" | "memcmp" | "strconcat" | "sprintf" => 3,
            "strreplace" => 4,
            "addmemboundary" => 3,
            _ => 0,
        };
        let mut args = self.pop_args(arity);
        if matches!(
            name,
            "memcmp" | "strlen" | "streq" | "getchar" | "malloc" | "confirm"
        ) {
            let temp = self.next_temp();
            self.emit(offset, format!("{temp} = {name}({});", args.join(", ")));
            self.push(temp);
            return;
        }
        self.emit(offset, format!("{name}({});", args.join(", ")));
        args.clear();
    }

    fn emit_dispatch(&mut self, offset: u64, instruction: &BpInstruction) {
        let Some(key) = instruction_call_key(instruction) else {
            self.emit(offset, format!("{}(<bad-id>);", instruction.opcode_name));
            return;
        };
        let name = key
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("call_{:02X}_{:02X}", key.group, key.id));
        let args = key
            .arg_count()
            .map(|count| self.pop_args(count))
            .unwrap_or_default();
        let arg_text = if args.is_empty() {
            String::new()
        } else {
            args.join(", ")
        };
        let returns_value = key.returns_value();
        let temp = returns_value.then(|| self.next_temp());
        let statement = if let Some(temp) = &temp {
            format!(
                "{temp} = {}({}) /* group=0x{:02X} id=0x{:02X} argc={} stack_top=[{}] */;",
                name,
                arg_text,
                key.group,
                key.id,
                key.arg_count()
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "?".to_string()),
                self.stack
                    .iter()
                    .rev()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            format!(
                "{}({}) /* group=0x{:02X} id=0x{:02X} argc={} stack_top=[{}] */;",
                name,
                arg_text,
                key.group,
                key.id,
                key.arg_count()
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "?".to_string()),
                self.stack
                    .iter()
                    .rev()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        self.emit(offset, statement);
        if let Some(temp) = temp {
            self.push(temp);
        }
    }

    fn pop_args(&mut self, arity: usize) -> Vec<String> {
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop());
        }
        args.reverse();
        args
    }

    fn next_temp(&mut self) -> String {
        let temp = format!("t{}", self.temp_counter);
        self.temp_counter += 1;
        temp
    }
}

fn operand_i32(instruction: &BpInstruction) -> Option<i32> {
    instruction
        .operands
        .first()
        .and_then(|operand| match operand {
            BpOperand::U8(value) => Some(*value as i8 as i32),
            BpOperand::U16(value) => Some(*value as i16 as i32),
            BpOperand::U32(value) => Some(*value as i32),
            BpOperand::I32(value) => Some(*value),
            _ => None,
        })
}

fn operand_u32(instruction: &BpInstruction) -> Option<u32> {
    instruction.operands.first().and_then(operand_as_u32)
}

fn operand_as_u32(operand: &BpOperand) -> Option<u32> {
    match operand {
        BpOperand::U8(value) => Some(*value as u32),
        BpOperand::U16(value) => Some(*value as u32),
        BpOperand::U32(value) => Some(*value),
        BpOperand::I32(value) => Some(*value as u32),
        BpOperand::Offset(value) => Some(*value),
        _ => None,
    }
}

fn format_operands(operands: &[BpOperand]) -> String {
    operands
        .iter()
        .map(|operand| format!("{operand:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn binary_symbol(name: &str) -> &'static str {
    match name {
        "add" => "+",
        "sub" => "-",
        "mul" => "*",
        "div" => "/",
        "mod" => "%",
        "and" => "&",
        "or" => "|",
        "xor" => "^",
        "shl" => "<<",
        "shr" => ">>",
        "sar" => ">>>",
        "eq" => "==",
        "neq" => "!=",
        "leq" => "<=",
        "geq" => ">=",
        "lt" => "<",
        "gt" => ">",
        "dnotzero" => "&&",
        "dnotzero2" => "||",
        _ => "?",
    }
}

fn condition_for_jc(kind: u32, condition: &str) -> String {
    match kind {
        0 => format!("({condition} != 0)"),
        1 => format!("({condition} == 0)"),
        2 => format!("({condition} > 0)"),
        3 => format!("({condition} >= 0)"),
        4 => format!("({condition} <= 0)"),
        5 => format!("({condition} < 0)"),
        _ => format!("jc{kind}({condition})"),
    }
}
