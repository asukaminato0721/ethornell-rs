use byteorder::{LittleEndian, ReadBytesExt};
use encoding_rs::SHIFT_JIS;
use ethornell_core::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;

pub mod bcs;
pub mod calls;
pub mod decompile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ScriptFormat {
    Bp,
    BurikoCompiledScriptV1,
    HeaderlessScenario,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum BpOpcode {
    Known { code: u8, name: &'static str },
    Unknown(u8),
}

impl BpOpcode {
    pub fn from_byte(code: u8) -> Self {
        match opcode_name(code) {
            Some(name) => Self::Known { code, name },
            None => Self::Unknown(code),
        }
    }

    pub fn code(&self) -> u8 {
        match self {
            Self::Known { code, .. } | Self::Unknown(code) => *code,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Known { name, .. } => name,
            Self::Unknown(_) => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum BpOperand {
    U8(u8),
    U16(u16),
    U32(u32),
    I32(i32),
    Offset(u32),
    String(String),
    Raw(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BpInstruction {
    pub offset: u64,
    pub opcode: BpOpcode,
    pub opcode_hex: String,
    pub opcode_name: String,
    pub operands: Vec<BpOperand>,
    pub known_call: Option<&'static str>,
    pub raw: Vec<u8>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BpProgram {
    pub script_name: Option<String>,
    pub functions: Vec<BpFunction>,
    pub strings: Vec<String>,
    pub instructions: Vec<BpInstruction>,
    pub labels: HashMap<u32, usize>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BpFunction {
    pub name: Option<String>,
    pub offset: u32,
}

#[derive(Debug, Clone, Copy)]
struct CodeRange {
    start: usize,
    end: usize,
}

pub fn detect_script_format(path: &Path, buf: &[u8]) -> ScriptFormat {
    if path
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with("._bp")
    {
        ScriptFormat::Bp
    } else if buf.starts_with(b"BurikoCompiledScriptVer1.00") {
        ScriptFormat::BurikoCompiledScriptV1
    } else if !buf.is_empty() && path.extension().is_none() {
        ScriptFormat::HeaderlessScenario
    } else {
        ScriptFormat::Unknown
    }
}

pub fn opcode_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0x00 => "push_byte",
        0x01 => "push_word",
        0x02 => "push_dword",
        0x04 => "push_base_offset",
        0x05 => "push_string",
        0x06 => "push_offset",
        0x08 => "load",
        0x09 => "move",
        0x0a => "move_arg",
        0x0c => "copy_stack",
        0x10 => "load_base",
        0x11 => "store_base",
        0x14 => "jmp",
        0x15 => "jc",
        0x16 => "call",
        0x17 => "ret",
        0x20 => "add",
        0x21 => "sub",
        0x22 => "mul",
        0x23 => "div",
        0x24 => "mod",
        0x25 => "and",
        0x26 => "or",
        0x27 => "xor",
        0x28 => "not",
        0x29 => "shl",
        0x2a => "shr",
        0x2b => "sar",
        0x30 => "eq",
        0x31 => "neq",
        0x32 => "leq",
        0x33 => "geq",
        0x34 => "lt",
        0x35 => "gt",
        0x38 => "dnotzero",
        0x39 => "dnotzero2",
        0x3a => "bool_zero",
        0x40 => "ternary",
        0x42 => "muldiv",
        0x48 => "sin",
        0x49 => "cos",
        0x60 => "memcpy",
        0x61 => "memclr",
        0x62 => "memset",
        0x63 => "memcmp",
        0x67 => "strreplace",
        0x68 => "strlen",
        0x69 => "streq",
        0x6a => "strcpy",
        0x6b => "strconcat",
        0x6c => "getchar",
        0x6d => "tolower",
        0x6f => "sprintf",
        0x70 => "malloc",
        0x71 => "free",
        0x75 => "addmemboundary",
        0x78 => "confirm",
        0x79 => "message_box",
        0x7a => "assert",
        0x7b => "dumpmem",
        0x80 => "sys1",
        0x81 => "sys2",
        0x90 => "grp1",
        0x91 => "grp2",
        0x92 => "grp3",
        0xa0 => "snd1",
        0xb0 => "usr1",
        0xc0 => "usr2",
        _ => return None,
    })
}

pub fn known_call_name(group: u8, id: u16) -> Option<&'static str> {
    match (group, id) {
        (0x80, 0x00) => Some("Srand"),
        (0x80, 0x04) => Some("GetTickCount"),
        (0x80, 0x0c) => Some("GetLocalTime"),
        (0x80, 0x0d) => Some("GetMemoryInfo"),
        (0x80, 0x0f) => Some("PollSystem"),
        (0x80, 0x11) => Some("ReadKeyState"),
        (0x80, 0x12) => Some("ReadInputState"),
        (0x80, 0x14) => Some("TimerOrMessageCheck"),
        (0x80, 0x17) => Some("PollWindowActive"),
        (0x80, 0x16) => Some("PollInputMessage"),
        (0x80, 0x18) => Some("WaitMilliseconds"),
        (0x80, 0x19) => Some("StartMillisecondsWait"),
        (0x80, 0x1a) => Some("Sys80_1A"),
        (0x80, 0x1b) => Some("RegisterMemoryClass"),
        (0x80, 0x1c) => Some("SystemSetMemoryClass"),
        (0x80, 0x20) => Some("Alloc"),
        (0x80, 0x21) => Some("Free"),
        (0x80, 0x28) => Some("CreateDirectory"),
        (0x80, 0x2a) => Some("DirectoryExists"),
        (0x80, 0x2b) => Some("Sys80_2B"),
        (0x80, 0x2d) => Some("SetFileAttributes"),
        (0x80, 0x30) => Some("ReadFileBytes"),
        (0x80, 0x31) => Some("ReadProfileString"),
        (0x80, 0x32) => Some("WriteFileBytes"),
        (0x80, 0x33) => Some("FreeProfileString"),
        (0x80, 0x34) => Some("FileExists"),
        (0x80, 0x35) => Some("GetFileSize"),
        (0x80, 0x36) => Some("CommitResourceSearchPaths"),
        (0x80, 0x37) => Some("AddResourceSearchPath"),
        (0x80, 0x3d) => Some("GetUserDataRoot"),
        (0x80, 0x40) => Some("LoadProgram"),
        (0x80, 0x41) => Some("FreeProgram"),
        (0x80, 0x44) => Some("LoadProgramEx"),
        (0x80, 0x46) => Some("SystemInit46"),
        (0x80, 0x47) => Some("ProgramIsComplete"),
        (0x80, 0x48) => Some("ProgramStartAsync"),
        (0x80, 0x49) => Some("ProgramShouldStop"),
        (0x80, 0x4a) => Some("ProgramDispatchWithArgs"),
        (0x80, 0x4b) => Some("Sys80_4B"),
        (0x80, 0x4c) => Some("Sys80_4C"),
        (0x80, 0x50) => Some("SystemWaitState"),
        (0x80, 0x52) => Some("SetHeadlessWindowMode"),
        (0x80, 0x58) => Some("SetTimer"),
        (0x80, 0x5a) => Some("PumpMessages"),
        (0x80, 0x5c) => Some("Sys5CTriple"),
        (0x80, 0x5e) => Some("ProgramMarkStarted"),
        (0x80, 0x5f) => Some("Yield"),
        (0x80, 0x60) => Some("Sys60Triple"),
        (0x80, 0x62) => Some("CommitMemoryClasses"),
        (0x80, 0x64) => Some("SystemInit64"),
        (0x80, 0x66) => Some("Sys66"),
        (0x80, 0x67) => Some("Sys67"),
        (0x80, 0x68) => Some("CaptureCloseEvent"),
        (0x80, 0x69) => Some("Sys80_69"),
        (0x80, 0x6a) => Some("Sys6A"),
        (0x80, 0x70) => Some("AllocGlobalMem"),
        (0x80, 0x74) => Some("Sys74"),
        (0x80, 0x80) => Some("Sys80Triple"),
        (0x80, 0x81) => Some("FrameBoundary"),
        (0x80, 0x82) => Some("Sys82Block"),
        (0x80, 0x83) => Some("Sys83Block"),
        (0x80, 0x88) => Some("ScenarioCodePreprocess"),
        (0x80, 0x8a) => Some("AnimationQueueBegin"),
        (0x80, 0x8b) => Some("AnimationQueueStep"),
        (0x80, 0x98) => Some("IndexedRecordOpen"),
        (0x80, 0x99) => Some("Sys99Release"),
        (0x80, 0x9a) => Some("IndexedRecordCount"),
        (0x80, 0x9c) => Some("Sys9CQuad"),
        (0x80, 0x9d) => Some("IndexedRecordLoad"),
        (0x80, 0xa8) => Some("SysA8Pair"),
        (0x80, 0xac) => Some("DispatchObjectEvent"),
        (0x80, 0xaf) => Some("SysAF"),
        (0x80, 0xc0) => Some("UserDataEncodeFlush"),
        (0x80, 0xc1) => Some("SysC1Buffer"),
        (0x80, 0xc4) => Some("UserDataEncodeChunk"),
        (0x80, 0xc5) => Some("SysC5Buffers"),
        (0x80, 0xd0) => Some("SysD0Pair"),
        (0x80, 0xd1) => Some("SysD1RecordClose"),
        (0x80, 0xd2) => Some("SysD2Record"),
        (0x80, 0xd4) => Some("SysD4RecordFetch"),
        (0x80, 0xd9) => Some("UserDataEncodeTrailer"),
        (0x80, 0xda) => Some("SysDATriple"),
        (0x80, 0xdb) => Some("UserDataEncodeState"),
        (0x80, 0xdd) => Some("SysDDTriple"),
        (0x80, 0xe8) => Some("GetGameId"),
        (0x80, 0xfd) => Some("IsLauncher"),
        (0x81, 0x0e) => Some("Sys2_0E"),
        (0x81, 0x0f) => Some("AnimationQueuePoll"),
        (0x81, 0x18) => Some("Sys2_18"),
        (0x81, 0x30) => Some("ReadResourceToBuffer"),
        (0x81, 0x35) => Some("OpenResourceHandle"),
        (0x81, 0x60) => Some("ConfigureScreen"),
        (0x81, 0x62) => Some("Sys2_62"),
        (0x81, 0x63) => Some("Sys2_63"),
        (0x81, 0x64) => Some("ConfigureScreenSize"),
        (0x81, 0x6f) => Some("Sys2_6F"),
        (0x90, 0x00) => Some("GraphInit"),
        (0x90, 0x02) => Some("GraphDelay"),
        (0x90, 0x03) => Some("GraphSetMemoryLimit"),
        (0x90, 0x04) => Some("GraphSetDrawContext"),
        (0x90, 0x06) => Some("GraphSetCenter"),
        (0x90, 0x07) => Some("GraphSetDefaultDuration"),
        (0x90, 0x08) => Some("GraphSetEnabled"),
        (0x90, 0x09) => Some("GraphSys09"),
        (0x90, 0x0c) => Some("GraphSystemInit"),
        (0x90, 0x0d) => Some("GraphSetFlag"),
        (0x90, 0x0e) => Some("GraphConfigureViewport"),
        (0x90, 0x10) => Some("GraphLoadResource"),
        (0x90, 0x11) => Some("GraphSys11"),
        (0x90, 0x12) => Some("GraphSys12"),
        (0x90, 0x13) => Some("GraphSys13"),
        (0x90, 0x16) => Some("GraphBindResource"),
        (0x90, 0x18) => Some("GraphObjectApply"),
        (0x90, 0x19) => Some("GraphObjectSetVisibleOrState"),
        (0x90, 0x1f) => Some("GraphObjectConfig"),
        (0x90, 0x20) => Some("GraphObjectTransition"),
        (0x90, 0x22) => Some("GraphNodeApplyTransition"),
        (0x90, 0x28) => Some("GraphSys28"),
        (0x90, 0x29) => Some("GraphSys29"),
        (0x90, 0x30) => Some("GraphTimelineSetEnabled"),
        (0x90, 0x31) => Some("GraphSys31"),
        (0x90, 0x32) => Some("GraphObjectCommit"),
        (0x90, 0x33) => Some("GraphSurfaceSetPosition"),
        (0x90, 0x34) => Some("GraphSys34"),
        (0x90, 0x35) => Some("GraphSys35"),
        (0x90, 0x36) => Some("GraphSys36"),
        (0x90, 0x37) => Some("GraphSys37"),
        (0x90, 0x38) => Some("GraphSys38"),
        (0x90, 0x39) => Some("GraphSurfaceSetVisible"),
        (0x90, 0x3a) => Some("GraphSetNodeResource"),
        (0x90, 0x3c) => Some("GraphSys3C"),
        (0x90, 0x43) => Some("GraphRectTransition"),
        (0x90, 0x4c) => Some("GraphSystemInit2"),
        (0x90, 0x50) => Some("GraphCreateNode"),
        (0x90, 0x51) => Some("GraphNodeRelease"),
        (0x90, 0x54) => Some("GraphNodeSetEnabled"),
        (0x90, 0x56) => Some("GraphNodeSetText"),
        (0x90, 0x57) => Some("GraphSys57"),
        (0x90, 0x58) => Some("GraphNodeConfigureLayout"),
        (0x90, 0x5c) => Some("GraphNodeConfigureImage"),
        (0x90, 0x60) => Some("GraphCreateObject"),
        (0x90, 0x61) => Some("GraphObjectUpdate"),
        (0x90, 0x64) => Some("GraphObjectSetEnabled"),
        (0x90, 0x65) => Some("GraphObjectConfigure"),
        (0x90, 0x80) => Some("GraphCreateSurface"),
        (0x90, 0x81) => Some("GraphSurfaceFlush"),
        (0x90, 0x82) => Some("GraphSurfaceSetVisibleFast"),
        (0x90, 0x83) => Some("GraphSurfaceBindBuffer"),
        (0x90, 0x84) => Some("GraphSurfaceSetEnabled"),
        (0x90, 0x85) => Some("GraphSurfaceSetRegion"),
        (0x90, 0x86) => Some("GraphSurfaceConfigure"),
        (0x90, 0x87) => Some("GraphSurfaceSetOption"),
        (0x90, 0x88) => Some("GraphSurfaceSetViewport"),
        (0x90, 0x94) => Some("GraphSys94"),
        (0x90, 0x95) => Some("GraphSys95"),
        (0x90, 0x96) => Some("GraphSys96"),
        (0x90, 0x97) => Some("GraphSys97"),
        (0x90, 0x98) => Some("GraphSys98"),
        (0x90, 0x99) => Some("GraphSys99"),
        (0x90, 0x9a) => Some("GraphSys9A"),
        (0x90, 0x9b) => Some("GraphSys9B"),
        (0x90, 0x9c) => Some("GraphSys9C"),
        (0x90, 0x9d) => Some("GraphSys9D"),
        (0x90, 0x9f) => Some("GraphSys9F"),
        (0x90, 0xaf) => Some("GraphSysAF"),
        (0x90, 0xb7) => Some("GraphSurfaceBindState"),
        (0x90, 0xb9) => Some("GraphObjectFinalize"),
        (0x90, 0xbc) => Some("GraphPollObjectState"),
        (0x90, 0xbf) => Some("GraphPollObjectEvent"),
        (0x90, 0xd0) => Some("GraphSysD0"),
        (0x90, 0xd1) => Some("GraphSysD1"),
        (0x90, 0xd4) => Some("GraphSysD4"),
        (0x90, 0xd5) => Some("GraphSysD5"),
        (0x90, 0xd6) => Some("GraphSysD6"),
        (0x90, 0xd8) => Some("GraphSysD8"),
        (0x90, 0xd9) => Some("GraphSysD9"),
        (0x90, 0xdb) => Some("GraphSysDB"),
        (0x90, 0xdd) => Some("GraphDriverInit"),
        (0x90, 0xe0) => Some("GraphCreateTimeline"),
        (0x90, 0xe1) => Some("GraphTimelinePoll"),
        (0x90, 0xe4) => Some("GraphTimelineSetEnabled"),
        (0x90, 0xe5) => Some("GraphTimelineConfigure"),
        (0x90, 0xe8) => Some("GraphTimelineAttach"),
        (0x90, 0xe9) => Some("GraphTimelineQuery"),
        (0x90, 0xf1) => Some("GraphSysF1"),
        (0x90, 0xf2) => Some("GraphSysF2"),
        (0x90, 0xf5) => Some("GraphSysF5"),
        (0x90, 0xf7) => Some("GraphSysF7"),
        (0x91, 0x0d) => Some("GraphSetLayerEnabled"),
        (0x91, 0x0e) => Some("GraphConfigureLayer"),
        (0x91, 0x06) => Some("GraphLayerSys06"),
        (0x91, 0x19) => Some("GraphLayerTransform"),
        (0x91, 0x1e) => Some("GraphLayerSys1E"),
        (0x91, 0x1f) => Some("GraphLayerSys1F"),
        (0x91, 0x36) => Some("GraphLayerSys36"),
        (0x91, 0x38) => Some("GraphLayerSys38"),
        (0x91, 0x3e) => Some("GraphLayerHitTest"),
        (0x91, 0x3f) => Some("GraphLayerSys3F"),
        (0x91, 0x48) => Some("GraphLayerSys48"),
        (0x91, 0x49) => Some("GraphLayerSys49"),
        (0x91, 0x4a) => Some("GraphLayerSys4A"),
        (0x91, 0x88) => Some("ConfigureFormatInfo"),
        (0x91, 0x89) => Some("GraphLayerFormatOption"),
        (0x91, 0x8b) => Some("GraphLayerSys8B"),
        (0x91, 0x8c) => Some("GraphLayerSys8C"),
        (0x91, 0x8d) => Some("GraphLayerSys8D"),
        (0x91, 0x94) => Some("GraphLayerSys94"),
        (0x91, 0x95) => Some("GraphLayerSys95"),
        (0x91, 0x96) => Some("GraphLayerSys96"),
        (0x91, 0x98) => Some("GraphLayerSys98"),
        (0x91, 0x9a) => Some("GraphSetLayerProperty"),
        (0x91, 0xb8) => Some("GraphLayerGetValue"),
        (0x91, 0xba) => Some("GraphLayerStoreValue"),
        (0x92, 0x12) => Some("GraphTextSys12"),
        (0x92, 0x18) => Some("GraphTextSys18"),
        (0x92, 0x17) => Some("GraphTextSetColorState"),
        (0x92, 0x88) => Some("GraphTextSys88"),
        (0x92, 0x8c) => Some("GraphTextSys8C"),
        (0x92, 0x89) => Some("GraphTextSys89"),
        (0x92, 0x91) => Some("GraphTextSys91"),
        (0x92, 0x97) => Some("GraphTextSys97"),
        (0x92, 0x9c) => Some("RenderText"),
        (0xa0, 0x08) => Some("SoundSys08"),
        (0xa0, 0x09) => Some("SoundSys09"),
        (0xa0, 0x11) => Some("SoundPlayBgm"),
        (0xa0, 0x12) => Some("SoundSys12"),
        (0xa0, 0x14) => Some("SoundControlBgm"),
        (0xa0, 0x15) => Some("SoundSys15"),
        (0xa0, 0x16) => Some("SoundFadeVolume"),
        (0xa0, 0x19) => Some("SoundFadeOrStop"),
        (0xa0, 0x20) => Some("SoundLoadSlot"),
        (0xa0, 0x21) => Some("SoundPlayEx"),
        (0xa0, 0x22) => Some("SoundChannelQuery"),
        (0xa0, 0x24) => Some("SoundPlaySlot"),
        (0xa0, 0x25) => Some("SoundChannelStopOrQuery"),
        (0xa0, 0x26) => Some("SoundSlotRelease"),
        (0xb0, 0x02) => Some("UserSys02"),
        (0xb0, 0x03) => Some("UserSys03"),
        (0xb0, 0x05) => Some("UserSys05"),
        (0xb0, 0x80) => Some("UserMessage"),
        (0xb0, 0xc1) => Some("UserMeasureText"),
        (0xb0, 0xc4) => Some("UserSetFontOption"),
        (0xb0, 0xc7) => Some("UserSetFontName"),
        (0xc0, 0x00) => Some("User2SetScreenSize"),
        (0xc0, 0x04) => Some("User2SetEnabled"),
        (0xc0, 0x05) => Some("User2ConfigureRegion"),
        (0xc0, 0x09) => Some("User2SetInterval"),
        (0xc0, 0x0a) => Some("User2SetCapacity"),
        (0xc0, 0x0b) => Some("User2SetLayout"),
        (0xc0, 0x0c) => Some("User2SetRepeatInterval"),
        (0xc0, 0x0d) => Some("User2SetDuration"),
        (0xc0, 0x18) => Some("User2ConfigureAnimation"),
        (0xc0, 0x1f) => Some("User2_1F"),
        (0xc0, 0x28) => Some("User2ConfigureBounds"),
        (0xc0, 0x29) => Some("User2ConfigureStyle"),
        (0xc0, 0x2d) => Some("User2ConfigureAdvancedStyle"),
        (0xc0, 0x4f) => Some("User2SetStep"),
        _ => None,
    }
}

pub fn disassemble_bp(buf: &[u8]) -> Vec<BpInstruction> {
    parse_bp_program(None, buf).instructions
}

pub fn parse_bp_program(script_name: Option<String>, buf: &[u8]) -> BpProgram {
    let range = detect_code_range(buf);
    let mut cursor = Cursor::new(&buf[range.start..range.end]);
    let mut instructions = Vec::new();
    let mut labels = HashMap::new();
    let mut strings = Vec::new();
    let mut warnings = Vec::new();

    while (cursor.position() as usize) < range.end - range.start {
        let relative = cursor.position() as usize;
        let offset = (range.start + relative) as u64;
        let opcode_byte = match cursor.read_u8() {
            Ok(op) => op,
            Err(_) => break,
        };
        let mut raw = vec![opcode_byte];
        let mut operands = Vec::new();
        let mut warning = None;
        let mut known_call = None;
        let mut opcode_name_override = None;

        match opcode_byte {
            0x00 | 0x08 | 0x09 | 0x0a => {
                if let Some(value) = read_u8_operand(&mut cursor, &mut raw, &mut warning) {
                    operands.push(BpOperand::U8(value));
                }
            }
            0x01 => {
                if let Some(value) = read_u16_operand(&mut cursor, &mut raw, &mut warning) {
                    operands.push(BpOperand::U16(value));
                }
            }
            0x02 => {
                if let Some(value) = read_u32_operand(&mut cursor, &mut raw, &mut warning) {
                    operands.push(BpOperand::U32(value));
                }
            }
            0x04 | 0x05 | 0x06 => {
                if let Some(value) = read_u16_operand(&mut cursor, &mut raw, &mut warning) {
                    if opcode_byte == 0x04 {
                        operands.push(BpOperand::U16(value));
                    } else if opcode_byte == 0x05 {
                        let target = (offset as i64 + value as i16 as i64) as usize;
                        if let Some(text) = read_c_string(buf, target) {
                            strings.push(text.clone());
                            operands.push(BpOperand::String(text));
                        } else {
                            operands.push(BpOperand::Offset(target as u32));
                            warning = Some(format!("unresolved string reference 0x{target:08x}"));
                        }
                    } else {
                        let target = (offset as i64 + value as i16 as i64) as u32;
                        operands.push(BpOperand::Offset(target));
                    }
                }
            }
            0x0b => {
                if let Some(count) = read_u8_operand(&mut cursor, &mut raw, &mut warning) {
                    let mut bytes = Vec::new();
                    for _ in 0..count {
                        if let Some(byte) = read_u8_operand(&mut cursor, &mut raw, &mut warning) {
                            bytes.push(byte);
                        }
                    }
                    operands.push(BpOperand::Raw(bytes));
                }
            }
            0x0c => {
                if let Some(width) = read_u8_operand(&mut cursor, &mut raw, &mut warning) {
                    operands.push(BpOperand::U8(width));
                }
                if let Some(count) = read_u8_operand(&mut cursor, &mut raw, &mut warning) {
                    operands.push(BpOperand::U8(count));
                }
            }
            0x15 | 0x80 | 0x81 | 0x90 | 0x91 | 0x92 | 0xa0 | 0xb0 | 0xc0 => {
                if let Some(value) = read_u8_operand(&mut cursor, &mut raw, &mut warning) {
                    operands.push(BpOperand::U8(value));
                    known_call = known_call_name(opcode_byte, value as u16);
                }
            }
            0xff => {
                if let Some(value) = read_u8_operand(&mut cursor, &mut raw, &mut warning) {
                    operands.push(BpOperand::U8(value));
                    opcode_name_override = Some(match value {
                        0xf0 => "script_load",
                        0xf1 => "script_free",
                        0xf8 => "script_ret",
                        _ => "script_call",
                    });
                }
            }
            0x14 | 0x16 | 0x17 => {}
            _ => {}
        }

        let opcode = if let Some(name) = opcode_name_override {
            BpOpcode::Known {
                code: opcode_byte,
                name,
            }
        } else {
            BpOpcode::from_byte(opcode_byte)
        };
        let instruction_index = instructions.len();
        labels.insert(offset as u32, instruction_index);
        if let Some(warning_text) = &warning {
            warnings.push(format!("0x{offset:08X}: {warning_text}"));
        }
        instructions.push(BpInstruction {
            offset,
            opcode_hex: format!("0x{opcode_byte:02X}"),
            opcode_name: opcode.name().to_string(),
            opcode,
            operands,
            known_call,
            raw,
            warning,
        });
    }

    let mut functions = Vec::new();
    for inst in &instructions {
        for operand in &inst.operands {
            if let BpOperand::Offset(offset) = operand {
                if labels.contains_key(offset) {
                    functions.push(BpFunction {
                        name: Some(format!("sub_{offset:08X}")),
                        offset: *offset,
                    });
                }
            }
        }
    }
    functions.sort_by_key(|f| f.offset);
    functions.dedup_by_key(|f| f.offset);

    BpProgram {
        script_name,
        functions,
        strings,
        instructions,
        labels,
        warnings,
    }
}

fn detect_code_range(buf: &[u8]) -> CodeRange {
    if buf.len() >= 8 {
        let header_size = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let instr_size = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
        if header_size >= 8
            && header_size <= buf.len()
            && instr_size <= buf.len()
            && header_size + instr_size == buf.len()
        {
            let nominal_end = header_size + instr_size;
            let end = detect_bp_code_end(buf, header_size, nominal_end);
            return CodeRange {
                start: header_size,
                end,
            };
        }
    }
    CodeRange {
        start: 0,
        end: buf.len(),
    }
}

fn detect_bp_code_end(buf: &[u8], start: usize, nominal_end: usize) -> usize {
    let Some(last_ret_relative) = buf[start..nominal_end]
        .iter()
        .rposition(|byte| *byte == 0x17)
    else {
        return nominal_end;
    };
    let mut end = start + last_ret_relative + 1;
    while end < nominal_end && buf[end] == 0 {
        end += 1;
    }
    end
}

fn read_u8_operand(
    cursor: &mut Cursor<&[u8]>,
    raw: &mut Vec<u8>,
    warning: &mut Option<String>,
) -> Option<u8> {
    match cursor.read_u8() {
        Ok(value) => {
            raw.push(value);
            Some(value)
        }
        Err(_) => {
            *warning = Some("truncated u8 operand".into());
            None
        }
    }
}

fn read_u16_operand(
    cursor: &mut Cursor<&[u8]>,
    raw: &mut Vec<u8>,
    warning: &mut Option<String>,
) -> Option<u16> {
    let mut bytes = [0u8; 2];
    match cursor.read_exact(&mut bytes) {
        Ok(()) => {
            raw.extend_from_slice(&bytes);
            Some(u16::from_le_bytes(bytes))
        }
        Err(_) => {
            *warning = Some("truncated u16 operand".into());
            None
        }
    }
}

fn read_u32_operand(
    cursor: &mut Cursor<&[u8]>,
    raw: &mut Vec<u8>,
    warning: &mut Option<String>,
) -> Option<u32> {
    match cursor.read_u32::<LittleEndian>() {
        Ok(value) => {
            raw.extend_from_slice(&value.to_le_bytes());
            Some(value)
        }
        Err(_) => {
            *warning = Some("truncated u32 operand".into());
            None
        }
    }
}

fn read_c_string(buf: &[u8], offset: usize) -> Option<String> {
    if offset >= buf.len() {
        return None;
    }
    let end = buf[offset..]
        .iter()
        .position(|&b| b == 0)
        .map(|pos| offset + pos)?;
    let (decoded, _, had_errors) = SHIFT_JIS.decode(&buf[offset..end]);
    if had_errors {
        None
    } else {
        Some(decoded.to_string())
    }
}

pub fn disassemble_file(path: &Path) -> Result<Vec<BpInstruction>> {
    let bytes = std::fs::read(path)?;
    Ok(disassemble_bp(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_required_opcode_names() {
        assert_eq!(opcode_name(0x00), Some("push_byte"));
        assert_eq!(opcode_name(0x91), Some("grp2"));
        assert_eq!(opcode_name(0xff), None);
    }

    #[test]
    fn decodes_conservatively() {
        let instructions = disassemble_bp(&[0x00, 0x7f, 0xff, 0x17]);
        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[0].raw, vec![0x00, 0x7f]);
        assert_eq!(instructions[1].raw, vec![0xff, 0x17]);
        assert_eq!(instructions[1].opcode.name(), "script_call");
    }

    #[test]
    fn short_input_does_not_panic() {
        let instructions = disassemble_bp(&[0x02, 0x01]);
        assert_eq!(instructions.len(), 1);
        assert!(instructions[0].warning.is_some());
    }

    #[test]
    fn known_call_table_is_seeded() {
        assert_eq!(known_call_name(0x91, 0x88), Some("ConfigureFormatInfo"));
        assert_eq!(known_call_name(0x92, 0x9c), Some("RenderText"));
    }
}
