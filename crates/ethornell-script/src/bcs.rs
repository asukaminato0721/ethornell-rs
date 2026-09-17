use encoding_rs::SHIFT_JIS;
use serde::Serialize;

const MAGIC: &[u8] = b"BurikoCompiledScriptVer1.00\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BcsProgram {
    pub header_size: usize,
    pub namespaces: Vec<String>,
    pub subs: Vec<BcsSub>,
    pub symbols: Vec<BcsSymbol>,
    pub code_start: usize,
    pub code_end: usize,
    pub executable_end: usize,
    pub resolver_end: Option<usize>,
    pub commands: Vec<BcsCommand>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BcsSub {
    pub name: String,
    pub addr: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BcsSymbol {
    pub name: String,
    pub addr: u32,
    pub table_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BcsCommand {
    pub file_offset: usize,
    pub addr: u32,
    pub opcode: u32,
    pub name: Option<&'static str>,
    pub args: Vec<BcsValue>,
    pub string_refs: Vec<BcsStringRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BcsStringRef {
    pub offset: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum BcsValue {
    Int(i32),
    Addr(i32),
    BaseOffset(i32),
    MemoryAddr(i32),
    Line { file: String, line: i32 },
    Arg2,
    Str(String),
    Mul(Box<BcsValue>, Box<BcsValue>),
    CheckNote(Box<BcsValue>),
}

pub fn parse_bcs(buf: &[u8]) -> Option<BcsProgram> {
    if !buf.starts_with(MAGIC) || buf.len() < MAGIC.len() + 4 {
        return None;
    }

    let header_size = read_u32(buf, MAGIC.len())? as usize;
    let body_start = header_size.checked_add(0x1c)?;
    if body_start.checked_add(28)? > buf.len() {
        return None;
    }

    let (namespaces, subs, mut warnings) = parse_symbol_header(buf, header_size);
    let (commands, code_end, executable_end) =
        parse_commands(buf, body_start, &subs, &mut warnings);
    let (symbols, resolver_end) = parse_resolver_table(buf, body_start);

    Some(BcsProgram {
        header_size,
        namespaces,
        subs,
        symbols,
        code_start: body_start,
        code_end,
        executable_end,
        resolver_end,
        commands,
        warnings,
    })
}

/// New-format scenario files keep label relocations in an opcode-shaped table
/// after the executable entry block. Each F7 record pairs a relative code
/// offset with a string; F9/F4 terminates the table. The final record is the
/// localized "label not found" diagnostic, so only label-shaped names are
/// exported.
fn parse_resolver_table(buf: &[u8], body_start: usize) -> (Vec<BcsSymbol>, Option<usize>) {
    const RECORD_SIZE: usize = 7 * 4;
    let mut best = Vec::new();
    let mut best_end = None;

    for start in (body_start..buf.len().saturating_sub(RECORD_SIZE)).step_by(4) {
        if read_u32(buf, start) != Some(0xf7) {
            continue;
        }
        let mut pos = start;
        let mut symbols = Vec::new();
        let mut terminated = false;
        while pos + 8 <= buf.len() {
            if read_u32(buf, pos) == Some(0xf9) && read_u32(buf, pos + 4) == Some(0xf4) {
                terminated = true;
                break;
            }
            if pos + RECORD_SIZE > buf.len()
                || read_u32(buf, pos) != Some(0xf7)
                || read_u32(buf, pos + 4) != Some(0x01)
                || read_u32(buf, pos + 12) != Some(0x19)
                || read_u32(buf, pos + 16) != Some(0x01)
                || read_u32(buf, pos + 20) != Some(0x03)
            {
                break;
            }
            let Some(relative_addr) = read_u32(buf, pos + 8) else {
                break;
            };
            let Some(string_offset) =
                read_u32(buf, pos + 24).and_then(|offset| body_start.checked_add(offset as usize))
            else {
                break;
            };
            let Some((name, _)) = read_sjis_z(buf, string_offset) else {
                break;
            };
            if name.starts_with("__") {
                symbols.push(BcsSymbol {
                    name,
                    addr: relative_addr.saturating_add(0x1c),
                    table_offset: pos,
                });
            }
            pos += RECORD_SIZE;
        }
        if terminated && symbols.len() > best.len() {
            best = symbols;
            best_end = Some(pos + 8);
        }
    }

    best.sort_by(|left, right| left.name.cmp(&right.name).then(left.addr.cmp(&right.addr)));
    best.dedup_by(|left, right| left.name == right.name && left.addr == right.addr);
    (best, best_end)
}

fn parse_symbol_header(buf: &[u8], header_size: usize) -> (Vec<String>, Vec<BcsSub>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut namespaces = Vec::new();
    let mut subs = Vec::new();
    let Some(count) = read_u32(buf, MAGIC.len() + 4) else {
        return (namespaces, subs, warnings);
    };
    let mut pos = MAGIC.len() + 8;
    for _ in 0..count.min(4096) {
        let Some((name, next)) = read_sjis_z(buf, pos) else {
            warnings.push("truncated namespace table".into());
            return (namespaces, subs, warnings);
        };
        namespaces.push(name);
        pos = next;
    }
    let Some(sub_count) = read_u32(buf, pos) else {
        return (namespaces, subs, warnings);
    };
    pos += 4;
    for _ in 0..sub_count.min(65536) {
        let Some((name, next)) = read_sjis_z(buf, pos) else {
            warnings.push("truncated sub table".into());
            return (namespaces, subs, warnings);
        };
        pos = next;
        let Some(addr) = read_u32(buf, pos) else {
            warnings.push("truncated sub address".into());
            return (namespaces, subs, warnings);
        };
        pos += 4;
        subs.push(BcsSub { name, addr });
        if pos > header_size + 0x1c {
            warnings.push("sub table exceeded declared header size".into());
            break;
        }
    }
    (namespaces, subs, warnings)
}

fn parse_commands(
    buf: &[u8],
    body_start: usize,
    subs: &[BcsSub],
    warnings: &mut Vec<String>,
) -> (Vec<BcsCommand>, usize, usize) {
    let mut commands = Vec::new();
    let mut queued_starts = vec![body_start];
    queued_starts.extend(subs.iter().filter_map(|sub| {
        body_start
            .checked_add(sub.addr as usize)
            .filter(|start| *start >= body_start && start.saturating_add(4) <= buf.len())
    }));
    let mut parsed_starts = std::collections::BTreeSet::new();
    let mut parsed_ranges = Vec::new();
    let mut entry_code_end = None;
    let mut executable_end = body_start;

    while let Some(start) = queued_starts.pop() {
        if !parsed_starts.insert(start)
            || parsed_ranges
                .iter()
                .any(|(range_start, range_end)| start >= *range_start && start < *range_end)
        {
            continue;
        }
        let (chunk, chunk_end) = parse_command_chunk(buf, start, body_start, warnings);
        let scanned_end = chunk
            .last()
            .map(|command| command.file_offset.saturating_add(4))
            .unwrap_or(chunk_end)
            .max(start.saturating_add(4));
        parsed_ranges.push((start, scanned_end));
        executable_end = executable_end.max(chunk_end);
        if start == body_start {
            entry_code_end = Some(chunk_end);
        }
        let mut targets = Vec::new();
        for (index, command) in chunk.iter().enumerate() {
            if command.name == Some("jc") {
                targets.extend(command.args.iter().filter_map(value_address));
            }
            if matches!(command.name, Some("jmp" | "call"))
                && let Some(previous) = index.checked_sub(1).and_then(|index| chunk.get(index))
                && previous.name == Some("push_offset")
            {
                targets.extend(previous.args.iter().filter_map(value_address));
            }
        }
        for target in targets {
            let Some(relative) = target.checked_sub(0x1c) else {
                continue;
            };
            let Some(target_start) = body_start.checked_add(relative as usize) else {
                continue;
            };
            if target_start >= body_start
                && target_start + 4 <= buf.len()
                && !parsed_starts.contains(&target_start)
                && !parsed_ranges.iter().any(|(range_start, range_end)| {
                    target_start >= *range_start && target_start < *range_end
                })
            {
                queued_starts.push(target_start);
            }
        }
        commands.extend(chunk);
    }

    commands.sort_by_key(|command| command.file_offset);
    commands.dedup_by_key(|command| command.file_offset);
    (
        commands,
        entry_code_end.unwrap_or(body_start),
        executable_end,
    )
}

fn parse_command_chunk(
    buf: &[u8],
    start: usize,
    body_start: usize,
    warnings: &mut Vec<String>,
) -> (Vec<BcsCommand>, usize) {
    let mut pos = start;
    let mut data_start = buf.len();
    let mut last_ret_end = None;
    let mut args = Vec::new();
    let mut string_refs = Vec::new();
    let mut commands = Vec::new();
    while pos + 4 <= data_start {
        let file_offset = pos;
        let opcode = read_u32(buf, pos).unwrap_or_default();
        if opcode > 0x3ff {
            warnings.push(format!(
                "opcode 0x{opcode:08X} outside BCS dispatch range at 0x{file_offset:08X}"
            ));
            break;
        }
        pos += 4;
        match opcode {
            0 => {
                if pos + 4 <= data_start {
                    let value = read_i32(buf, pos).unwrap_or_default();
                    pos += 4;
                    args.push(BcsValue::Int(value));
                    push_command(
                        &mut commands,
                        file_offset,
                        body_start,
                        opcode,
                        &mut args,
                        &mut string_refs,
                    );
                }
            }
            1 => {
                if pos + 4 <= data_start {
                    let value = read_i32(buf, pos).unwrap_or_default();
                    pos += 4;
                    args.push(BcsValue::Addr(value));
                    push_command(
                        &mut commands,
                        file_offset,
                        body_start,
                        opcode,
                        &mut args,
                        &mut string_refs,
                    );
                }
            }
            2 => {
                if pos + 4 <= data_start {
                    let value = read_i32(buf, pos).unwrap_or_default();
                    pos += 4;
                    args.push(BcsValue::BaseOffset(value));
                    push_command(
                        &mut commands,
                        file_offset,
                        body_start,
                        opcode,
                        &mut args,
                        &mut string_refs,
                    );
                }
            }
            3 => {
                if pos + 4 <= data_start {
                    let offset = read_u32(buf, pos).unwrap_or_default();
                    pos += 4;
                    let string_file_offset = body_start.saturating_add(offset as usize);
                    if let Some((text, _)) = read_sjis_z(buf, string_file_offset) {
                        string_refs.push(BcsStringRef {
                            offset: string_file_offset as u32,
                            text: text.clone(),
                        });
                        args.push(BcsValue::Str(text));
                        data_start = data_start.min(string_file_offset);
                    } else {
                        warnings.push(format!("invalid string offset 0x{offset:08X}"));
                        args.clear();
                        string_refs.clear();
                    }
                    push_command(
                        &mut commands,
                        file_offset,
                        body_start,
                        opcode,
                        &mut args,
                        &mut string_refs,
                    );
                }
            }
            0x08 | 0x09 | 0x0a | 0x17 | 0x7e => {
                read_int_args(buf, &mut pos, data_start, 1, &mut args);
                push_command(
                    &mut commands,
                    file_offset,
                    body_start,
                    opcode,
                    &mut args,
                    &mut string_refs,
                );
            }
            0x7b => {
                read_int_args(buf, &mut pos, data_start, 3, &mut args);
                push_command(
                    &mut commands,
                    file_offset,
                    body_start,
                    opcode,
                    &mut args,
                    &mut string_refs,
                );
            }
            0x7f => {
                if pos + 8 <= data_start {
                    let offset = read_u32(buf, pos).unwrap_or_default();
                    let line = read_i32(buf, pos + 4).unwrap_or_default();
                    pos += 8;
                    let file_offset = body_start.saturating_add(offset as usize);
                    if let Some((file, _)) = read_sjis_z(buf, file_offset) {
                        args.push(BcsValue::Line { file, line });
                        data_start = data_start.min(file_offset);
                    } else {
                        args.push(BcsValue::Int(line));
                    }
                }
                push_command(
                    &mut commands,
                    file_offset,
                    body_start,
                    opcode,
                    &mut args,
                    &mut string_refs,
                );
            }
            0x19 => {
                if pos + 4 <= data_start {
                    let value = read_i32(buf, pos).unwrap_or_default();
                    args.push(BcsValue::Addr(value));
                    pos += 4;
                }
                push_command(
                    &mut commands,
                    file_offset,
                    body_start,
                    opcode,
                    &mut args,
                    &mut string_refs,
                );
            }
            0x22 => {
                if let (Some(right), Some(left)) = (args.pop(), args.pop()) {
                    args.push(BcsValue::Mul(Box::new(left), Box::new(right)));
                } else {
                    push_command(
                        &mut commands,
                        file_offset,
                        body_start,
                        opcode,
                        &mut args,
                        &mut string_refs,
                    );
                }
            }
            0x3f => {
                if pos + 4 <= data_start {
                    let value = read_i32(buf, pos).unwrap_or_default();
                    pos += 4;
                    args.push(BcsValue::Int(value));
                }
                push_command(
                    &mut commands,
                    file_offset,
                    body_start,
                    opcode,
                    &mut args,
                    &mut string_refs,
                );
            }
            0xe7 => {
                if let Some(value) = args.pop() {
                    args.push(BcsValue::CheckNote(Box::new(value)));
                } else {
                    push_command(
                        &mut commands,
                        file_offset,
                        body_start,
                        opcode,
                        &mut args,
                        &mut string_refs,
                    );
                }
            }
            _ => {
                push_command(
                    &mut commands,
                    file_offset,
                    body_start,
                    opcode,
                    &mut args,
                    &mut string_refs,
                );
            }
        }

        if opcode == 0x1b {
            last_ret_end = Some(pos);
        }
    }

    let code_end = last_ret_end.unwrap_or_else(|| data_start.min(buf.len()));
    commands.retain(|command| command.file_offset < code_end);
    (commands, code_end)
}

fn push_command(
    commands: &mut Vec<BcsCommand>,
    file_offset: usize,
    body_start: usize,
    opcode: u32,
    args: &mut Vec<BcsValue>,
    string_refs: &mut Vec<BcsStringRef>,
) {
    commands.push(BcsCommand {
        file_offset,
        addr: file_offset.saturating_sub(body_start).saturating_add(0x1c) as u32,
        opcode,
        name: command_name(opcode),
        args: std::mem::take(args),
        string_refs: std::mem::take(string_refs),
    });
}

pub fn command_name(opcode: u32) -> Option<&'static str> {
    Some(match opcode {
        0x000 => "push_dword",
        0x001 => "push_offset",
        0x002 => "push_base_offset",
        0x003 => "push_string",
        0x008 => "read_mem",
        0x009 => "write_mem_copy",
        0x00a => "write_mem",
        0x010 => "get_stack_pointer",
        0x011 => "set_stack_pointer",
        0x017 => "cmd0x17",
        0x018 => "jmp",
        0x019 => "jc",
        0x01a => "call",
        0x01b => "ret",
        0x01c => "script_call",
        0x01e => "reg_exception_handler",
        0x01f => "unreg_exception_handler",
        0x020 => "add",
        0x021 => "sub",
        0x022 => "mul",
        0x023 => "div",
        0x024 => "mod",
        0x025 => "and",
        0x026 => "or",
        0x027 => "xor",
        0x028 => "not",
        0x029 => "shl",
        0x02a => "shr",
        0x02b => "sar",
        0x030 => "eq",
        0x031 => "neq",
        0x032 => "leq",
        0x033 => "geq",
        0x034 => "lt",
        0x035 => "gt",
        0x038 => "bool_and",
        0x039 => "bool_or",
        0x03a => "bool_zero",
        0x03f => "nargs",
        0x040 => "ternary",
        0x048 => "sin",
        0x049 => "cos",
        0x07b => "cmd0x7b",
        0x07e => "cmd0x7e",
        0x07f => "line",
        0x0e0 => "global_set",
        0x0e1 => "global_get",
        0x0e2 => "cmd0xe2",
        0x0e3 => "cmd0xe3",
        0x0e6 => "end_if",
        0x0e7 => "check_translator_note",
        0x0f0 => "exec_script",
        // Runtime evidence: `main` pushes a script resource and mode, then 0x0F3
        // transfers to `MakerLogo`. This is distinct from the documented 0x0F0 form.
        0x0f3 => "exec_script_legacy",
        0x0f4 => "cmd0xf4",
        0x0f6 => "resolve_symbol",
        0x0f7 => "resolver_entry",
        0x0f9 => "resolver_end",
        0x0fe => "source_line",
        0x110 => "wait",
        0x120 => "cmd0x120",
        0x121 => "cmd0x121",
        0x126 => "cmd0x126",
        0x140 => "say",
        0x14c => "set_font",
        0x151 => "cmd0x151",
        0x180 => "sound",
        0x185 => "img_hide",
        0x186 => "fx_smth1",
        0x1a0 => "sound_1a0",
        0x1b1 => "cmd0x1b1",
        0x1b2 => "char_act",
        0x1b4 => "set_script_file",
        0x1b6 => "set_voice_seq",
        0x1bf => "play_movie",
        0x230 => "cmd0x230",
        0x240 => "bg240",
        0x260 => "bg",
        0x261 => "bg_transition",
        0x268 => "fade_to_black",
        0x269 => "transition",
        0x280 => "sprite",
        0x288 => "sprite_hide",
        0x28a => "sprite_hide_all",
        0x340 => "cmd0x340",
        _ if (0x100..0x140).contains(&opcode) => "sys",
        _ if (0x140..0x160).contains(&opcode) => "msg",
        _ if (0x160..0x180).contains(&opcode) => "slct",
        _ if (0x180..0x200).contains(&opcode) => "snd",
        _ if (0x200..0x400).contains(&opcode) => "grp",
        _ => return None,
    })
}

fn read_u32(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = buf.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_i32(buf: &[u8], offset: usize) -> Option<i32> {
    let bytes = buf.get(offset..offset + 4)?;
    Some(i32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_int_args(
    buf: &[u8],
    pos: &mut usize,
    data_start: usize,
    count: usize,
    args: &mut Vec<BcsValue>,
) {
    for _ in 0..count {
        if pos.saturating_add(4) > data_start {
            break;
        }
        args.push(BcsValue::Int(read_i32(buf, *pos).unwrap_or_default()));
        *pos += 4;
    }
}

fn read_sjis_z(buf: &[u8], offset: usize) -> Option<(String, usize)> {
    let rest = buf.get(offset..)?;
    let len = rest.iter().position(|byte| *byte == 0)?;
    let (text, _, _) = SHIFT_JIS.decode(&rest[..len]);
    Some((text.into_owned(), offset + len + 1))
}

fn value_address(value: &BcsValue) -> Option<i32> {
    match value {
        BcsValue::Addr(value) => Some(*value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(code: &[u32], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&12u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        for value in code {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(data);
        bytes.resize(bytes.len().max(MAGIC.len() + 12 + 28), 0);
        bytes
    }

    #[test]
    fn stops_before_non_opcode_data_and_keeps_the_last_ret() {
        let bytes = fixture(&[0x0, 42, 0x1b], &[0x78, 0x56, 0x34, 0x12, 0x1b, 0, 0, 0]);
        let program = parse_bcs(&bytes).expect("BCS fixture");

        assert_eq!(program.commands.len(), 2);
        assert_eq!(
            program.commands.last().map(|command| command.opcode),
            Some(0x1b)
        );
        assert_eq!(
            program.code_end,
            program.commands.last().unwrap().file_offset + 4
        );
        assert!(
            program
                .warnings
                .iter()
                .any(|warning| warning.contains("outside BCS dispatch range"))
        );
    }

    #[test]
    fn reads_the_documented_memory_width_operands() {
        let bytes = fixture(&[0x2, 12, 0x08, 2, 0x1b], &[0x78, 0x56, 0x34, 0x12]);
        let program = parse_bcs(&bytes).expect("BCS fixture");

        assert_eq!(program.commands.len(), 3);
        assert_eq!(program.commands[1].opcode, 0x08);
        assert_eq!(program.commands[1].args, vec![BcsValue::Int(2)]);
    }

    #[test]
    fn follows_a_jump_across_embedded_data_without_extending_entry_code() {
        let bytes = fixture(
            &[
                0x01,
                0x50,
                0x18,
                0x1b, // push target, jump, entry-block ret
                0x1234_5678,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0, // embedded data
                0x00,
                7,
                0x1b, // target at BCS address 0x50
            ],
            &[],
        );
        let program = parse_bcs(&bytes).expect("BCS fixture");

        assert_eq!(program.commands.len(), 5);
        assert_eq!(
            program
                .commands
                .iter()
                .map(|command| command.addr)
                .collect::<Vec<_>>(),
            vec![0x1c, 0x24, 0x28, 0x50, 0x58]
        );
        assert_eq!(program.code_end, program.code_start + 16);
        assert_eq!(program.commands[3].args, vec![BcsValue::Int(7)]);
    }

    #[test]
    fn consumes_version_specific_operands_without_creating_fake_opcodes() {
        let bytes = fixture(&[0x17, 0xe0, 0x7b, 0xe1, 0xe4, 0xe5, 0x7e, 0xfe, 0x1b], &[]);
        let program = parse_bcs(&bytes).expect("BCS fixture");

        assert_eq!(
            program
                .commands
                .iter()
                .map(|command| command.opcode)
                .collect::<Vec<_>>(),
            vec![0x17, 0x7b, 0x7e, 0x1b]
        );
        assert_eq!(program.commands[0].args, vec![BcsValue::Int(0xe0)]);
        assert_eq!(
            program.commands[1].args,
            vec![
                BcsValue::Int(0xe1),
                BcsValue::Int(0xe4),
                BcsValue::Int(0xe5)
            ]
        );
        assert_eq!(program.commands[2].args, vec![BcsValue::Int(0xfe)]);
    }

    #[test]
    fn parses_the_f7_scenario_label_resolver_table() {
        let mut bytes = fixture(&[0x1b, 0, 0], &[]);
        let body_start = 40usize;
        let label_offset = 96usize;
        bytes.resize(body_start + label_offset + 32, 0);
        let table = [
            0xf7,
            0x01,
            0x0c,
            0x19,
            0x01,
            0x03,
            label_offset as u32,
            0xf9,
            0xf4,
        ];
        let table_start = body_start + 12;
        for (index, value) in table.into_iter().enumerate() {
            let offset = table_start + index * 4;
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[body_start + label_offset..body_start + label_offset + 14]
            .copy_from_slice(b"__trial_entry\0");

        let program = parse_bcs(&bytes).expect("BCS fixture");

        assert_eq!(program.symbols.len(), 1);
        assert_eq!(program.symbols[0].name, "__trial_entry");
        assert_eq!(program.symbols[0].addr, 0x28);
    }
}
