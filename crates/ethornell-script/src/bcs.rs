use encoding_rs::SHIFT_JIS;
use serde::Serialize;

const MAGIC: &[u8] = b"BurikoCompiledScriptVer1.00\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BcsProgram {
    pub header_size: usize,
    pub namespaces: Vec<String>,
    pub subs: Vec<BcsSub>,
    pub code_start: usize,
    pub code_end: usize,
    pub commands: Vec<BcsCommand>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BcsSub {
    pub name: String,
    pub addr: u32,
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
    let (code_start, code_end) = choose_code_range(buf, body_start);
    let commands = parse_commands(buf, code_start, code_end, body_start, &mut warnings);

    Some(BcsProgram {
        header_size,
        namespaces,
        subs,
        code_start,
        code_end,
        commands,
        warnings,
    })
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

fn choose_code_range(buf: &[u8], body_start: usize) -> (usize, usize) {
    let start = body_start.min(buf.len());
    let end = find_last_ret_end(buf, start).unwrap_or(buf.len());
    (start, end.max(start).min(buf.len()))
}

fn find_last_ret_end(buf: &[u8], start: usize) -> Option<usize> {
    let mut pos = start;
    let mut last = None;
    while pos + 4 <= buf.len() {
        if read_u32(buf, pos) == Some(0x1b) {
            last = Some(pos + 4);
        }
        pos += 4;
    }
    last
}

fn parse_commands(
    buf: &[u8],
    start: usize,
    end: usize,
    body_start: usize,
    warnings: &mut Vec<String>,
) -> Vec<BcsCommand> {
    let mut pos = start;
    let mut args = Vec::new();
    let mut string_refs = Vec::new();
    let mut commands = Vec::new();
    while pos + 4 <= end {
        let file_offset = pos;
        let opcode = read_u32(buf, pos).unwrap_or_default();
        pos += 4;
        match opcode {
            0 => {
                if let Some(value) = read_i32(buf, pos) {
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
                if let Some(value) = read_i32(buf, pos) {
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
                if let Some(value) = read_i32(buf, pos) {
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
                if let Some(offset) = read_u32(buf, pos) {
                    pos += 4;
                    let string_file_offset = body_start.saturating_add(offset as usize);
                    if let Some((text, _)) = read_sjis_z(buf, string_file_offset) {
                        string_refs.push(BcsStringRef {
                            offset: string_file_offset as u32,
                            text: text.clone(),
                        });
                        args.push(BcsValue::Str(text));
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
            0x7f => {
                if pos + 8 <= end {
                    let offset = read_u32(buf, pos).unwrap_or_default();
                    let line = read_i32(buf, pos + 4).unwrap_or_default();
                    pos += 8;
                    let file_offset = body_start.saturating_add(offset as usize);
                    if let Some((file, _)) = read_sjis_z(buf, file_offset) {
                        args.push(BcsValue::Line { file, line });
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
                if pos + 4 <= end {
                    if let Some(value) = read_i32(buf, pos) {
                        args.push(BcsValue::Addr(value));
                    }
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
                if let Some(value) = read_i32(buf, pos) {
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
    }
    commands
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
        0x008 => "load",
        0x009 => "move",
        0x00a => "move_arg",
        0x010 => "load_base",
        0x011 => "store_base",
        0x018 => "jmp",
        0x019 => "jc",
        0x01a => "call",
        0x01b => "ret",
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
        0x07f => "line",
        0x0e2 => "cmd0xe2",
        0x0e3 => "cmd0xe3",
        0x0e6 => "end_if",
        0x0e7 => "check_translator_note",
        0x0f0 => "exec_script",
        0x0f4 => "cmd0xf4",
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

fn read_sjis_z(buf: &[u8], offset: usize) -> Option<(String, usize)> {
    let rest = buf.get(offset..)?;
    let len = rest.iter().position(|byte| *byte == 0)?;
    let (text, _, _) = SHIFT_JIS.decode(&rest[..len]);
    Some((text.into_owned(), offset + len + 1))
}
