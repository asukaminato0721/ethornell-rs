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
pub mod native_abi;

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
        0x0b => "copy_inline",
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
        0x43 => "atan2",
        0x44 => "vec3_length",
        0x48 => "sin",
        0x49 => "cos",
        0x50 => "qword_add",
        0x51 => "qword_sub",
        0x52 => "qword_mul",
        0x53 => "qword_div",
        0x54 => "qword_mod",
        0x60 => "memcpy",
        0x61 => "memclr",
        0x62 => "memset",
        0x63 => "memcmp",
        0x64 => "memrepeat",
        0x65 => "memfind",
        0x66 => "strfind",
        0x67 => "strreplace",
        0x68 => "strlen",
        0x69 => "streq",
        0x6a => "strcpy",
        0x6b => "strconcat",
        0x6c => "getchar",
        0x6d => "tolower",
        0x6e => "quote_string",
        0x6f => "sprintf",
        0x70 => "malloc",
        0x71 => "free",
        0x74 => "set_memory_mode",
        0x75 => "addmemboundary",
        0x77 => "engine_state",
        0x78 => "confirm",
        0x79 => "message_box",
        0x7a => "assert",
        0x7b => "dumpmem",
        0x7c => "modal_list",
        0x7d => "resource_transform",
        0x7e => "clipboard_set",
        0x7f => "resource_blend",
        0x80 => "sys1",
        0x81 => "sys2",
        0x90 => "grp1",
        0x91 => "grp2",
        0x92 => "grp3",
        0xa0 => "snd1",
        0xb0 => "usr1",
        0xc0 => "usr2",
        0xd0 => "legacy_3d",
        0xe0 => "debug_inspect",
        0xff => "script_extension",
        _ => return None,
    })
}

/// Non-null entries in the shipped interpreter table at `0x506300`.
pub const NATIVE_REGISTERED_OPCODES: &[u8] = &[
    0x00, 0x01, 0x02, 0x04, 0x05, 0x06, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x10, 0x11, 0x14, 0x15,
    0x16, 0x17, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x30,
    0x31, 0x32, 0x33, 0x34, 0x35, 0x38, 0x39, 0x3a, 0x40, 0x42, 0x43, 0x44, 0x48, 0x49, 0x50,
    0x51, 0x52, 0x53, 0x54, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a,
    0x6b, 0x6c, 0x6d, 0x6e, 0x6f, 0x70, 0x71, 0x74, 0x75, 0x77, 0x78, 0x79, 0x7a, 0x7b, 0x7c,
    0x7d, 0x7e, 0x7f, 0x80, 0x81, 0x90, 0x91, 0x92, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0, 0xff,
];

pub fn known_call_name(group: u8, id: u16) -> Option<&'static str> {
    match (group, id) {
        (0x80, 0x00) => Some("Srand"),
        (0x80, 0x01) => Some("Rand"),
        (0x80, 0x02) => Some("RandMax"),
        (0x80, 0x04) => Some("GetTickCount"),
        (0x80, 0x05) => Some("QueryPerformanceCounter"),
        (0x80, 0x06) => Some("SystemFlushOrSync"),
        (0x80, 0x07) => Some("ReadPerformanceMetric"),
        (0x80, 0x08) => Some("ReadCursorPoint"),
        (0x80, 0x0c) => Some("GetLocalTime"),
        (0x80, 0x0d) => Some("GetMemoryInfo"),
        (0x80, 0x0f) => Some("PollSystem"),
        (0x80, 0x11) => Some("ReadKeyState"),
        (0x80, 0x12) => Some("ReadInputState"),
        (0x80, 0x13) => Some("PumpWaitState"),
        (0x80, 0x14) => Some("TimerOrMessageCheck"),
        (0x80, 0x15) => Some("ShowCursor"),
        (0x80, 0x17) => Some("PollWindowActive"),
        (0x80, 0x16) => Some("PollInputMessage"),
        (0x80, 0x18) => Some("SetInputMasterGate"),
        (0x80, 0x19) => Some("SetInputLatchedState"),
        (0x80, 0x1a) => Some("QueryInputClassState"),
        (0x80, 0x1b) => Some("RegisterMemoryClass"),
        (0x80, 0x1c) => Some("QueryInputClassLevel"),
        (0x80, 0x1f) => Some("ConfigureUiMotion"),
        (0x80, 0x20) => Some("Alloc"),
        (0x80, 0x21) => Some("Free"),
        (0x80, 0x25) => Some("EnumerateFiles"),
        (0x80, 0x28) => Some("CreateDirectory"),
        (0x80, 0x29) => Some("RemoveDirectory"),
        (0x80, 0x2a) => Some("DirectoryExists"),
        (0x80, 0x2b) => Some("Sys80_2B"),
        (0x80, 0x2c) => Some("GetFileAttributes"),
        (0x80, 0x2d) => Some("SetFileAttributes"),
        (0x80, 0x2f) => Some("CopyFile"),
        (0x80, 0x30) => Some("ReadFileBytes"),
        (0x80, 0x31) => Some("ReadProfileString"),
        (0x80, 0x32) => Some("WriteFileBytes"),
        (0x80, 0x33) => Some("DeleteFile"),
        (0x80, 0x34) => Some("FileExists"),
        (0x80, 0x35) => Some("GetFileSize"),
        (0x80, 0x36) => Some("CommitResourceSearchPaths"),
        (0x80, 0x37) => Some("AddResourceSearchPath"),
        (0x80, 0x38) => Some("RegisterResourceRecord"),
        (0x80, 0x39) => Some("CommitUserDataRoot"),
        (0x80, 0x3a) => Some("QueryShortcutInfo"),
        (0x80, 0x3b) => Some("OpenFileDialog"),
        (0x80, 0x3d) => Some("GetUserDataRoot"),
        (0x80, 0x3f) => Some("ConfigureArchiveRoot"),
        (0x80, 0x40) => Some("LoadProgram"),
        (0x80, 0x45) => Some("ProgramFrameComplete"),
        (0x80, 0x41) => Some("FreeProgram"),
        (0x80, 0x44) => Some("LoadProgramEx"),
        (0x80, 0x46) => Some("CurrentProgramId"),
        (0x80, 0x47) => Some("ProgramExists"),
        (0x80, 0x48) => Some("ProgramPostMessage"),
        (0x80, 0x49) => Some("ProgramReceiveMessage"),
        (0x80, 0x4a) => Some("ProgramPostMessages"),
        (0x80, 0x4b) => Some("ProgramReceiveMessages"),
        (0x80, 0x4c) => Some("ProgramInvokeCallback"),
        (0x80, 0x50) => Some("SystemWaitState"),
        (0x80, 0x52) => Some("SetHeadlessWindowMode"),
        (0x80, 0x58) => Some("SetTimer"),
        (0x80, 0x59) => Some("AdvanceDeadlineAndPoll"),
        (0x80, 0x5a) => Some("PumpMessages"),
        (0x80, 0x5c) => Some("WaitTimingEx"),
        (0x80, 0x5e) => Some("ProgramSwitchTo"),
        (0x80, 0x5f) => Some("Yield"),
        (0x80, 0x60) => Some("ConfigureDisplayMode"),
        (0x80, 0x61) => Some("QueryDisplayMode"),
        (0x80, 0x62) => Some("CommitMemoryClasses"),
        (0x80, 0x64) => Some("SetWindowVisible"),
        (0x80, 0x65) => Some("MinimizeMainWindow"),
        (0x80, 0x66) => Some("SetWindowTitle"),
        (0x80, 0x67) => Some("SetCursorKind"),
        (0x80, 0x68) => Some("CaptureCloseEvent"),
        (0x80, 0x69) => Some("RequestWindowClose"),
        (0x80, 0x6a) => Some("Sys6A"),
        (0x80, 0x70) => Some("AllocGlobalMem"),
        (0x80, 0x74) => Some("Sys74"),
        (0x80, 0x80) => Some("LoadGlobalUserData"),
        (0x80, 0x81) => Some("SaveGlobalUserData"),
        (0x80, 0x82) => Some("WriteGlobalDataBlock"),
        (0x80, 0x83) => Some("ReadGlobalDataBlock"),
        (0x80, 0x84) => Some("RegisterResourceName"),
        (0x80, 0x85) => Some("ResourceNameExists"),
        (0x80, 0x88) => Some("ScenarioCodePreprocess"),
        (0x80, 0x8a) => Some("SetReadFlagRange"),
        (0x80, 0x8b) => Some("GetReadFlag"),
        (0x80, 0x98) => Some("IndexedRecordOpen"),
        (0x80, 0x99) => Some("IndexedRecordClose"),
        (0x80, 0x9a) => Some("IndexedRecordCount"),
        (0x80, 0x9c) => Some("IndexedRecordPush"),
        (0x80, 0x9d) => Some("IndexedRecordLoad"),
        (0x80, 0x9e) => Some("IndexedRecordRemove"),
        (0x80, 0xa0) => Some("PollQueuedEvent"),
        (0x80, 0xa1) => Some("PostQueuedEvent"),
        (0x80, 0xa8) => Some("SetRegisteredObjectState"),
        (0x80, 0xac) => Some("DispatchObjectEvent"),
        (0x80, 0xaf) => Some("SetSystemModeFlag"),
        (0x80, 0xc0) => Some("UserDataEncodeFlush"),
        (0x80, 0xc1) => Some("SysC1Buffer"),
        (0x80, 0xc4) => Some("UserDataEncodeChunk"),
        (0x80, 0xc5) => Some("SysC5Buffers"),
        (0x80, 0xd0) => Some("RecordTableOpen"),
        (0x80, 0xd1) => Some("RecordTableClose"),
        (0x80, 0xd2) => Some("RecordTableInsert"),
        (0x80, 0xd3) => Some("SysD3RecordProbe"),
        (0x80, 0xd4) => Some("RecordTableFetch"),
        (0x80, 0xd8) => Some("DispatchPendingCallbacks"),
        (0x80, 0xd9) => Some("UserDataEncodeTrailer"),
        (0x80, 0xda) => Some("InstallStringNamespace"),
        (0x80, 0xdb) => Some("SerializeStringNamespace"),
        (0x80, 0xdc) => Some("InternStringInNamespace"),
        (0x80, 0xdd) => Some("GetStringNamespaceEntry"),
        (0x80, 0xe0) => Some("LaunchProcess"),
        (0x80, 0xe1) => Some("RestartWithCommand"),
        (0x80, 0xe2) => Some("LaunchProcessWithArgs"),
        (0x80, 0xe3) => Some("ShellOpen"),
        (0x80, 0xed) => Some("SystemExtensionQuery"),
        (0x80, 0xee) => Some("SystemExtensionReset"),
        (0x80, 0xef) => Some("SystemExtensionSet"),
        (0x80, 0xe8) => Some("GetGameId"),
        (0x80, 0xf0) => Some("ShowInputDialog"),
        (0x80, 0xf2) => Some("ShowInstallerDialog"),
        (0x80, 0xf7) => Some("ValidateOrCreateUserPath"),
        (0x80, 0xf8) => Some("ReadInstalledFolder"),
        (0x80, 0xfd) => Some("IsLauncher"),
        (0x81, 0x0e) => Some("Sys2_0E"),
        (0x81, 0x0f) => Some("IsMainWindowMinimized"),
        (0x81, 0x18) => Some("RegisterTouchInput"),
        (0x81, 0x19) => Some("CopyTouchRecords"),
        (0x81, 0x2f) => Some("TestPathWritable"),
        (0x81, 0x30) => Some("ReadResourceToBuffer"),
        (0x81, 0x31) => Some("InternetRead"),
        (0x81, 0x35) => Some("OpenResourceHandle"),
        (0x81, 0x36) => Some("EnumerateDriveTypes"),
        (0x81, 0x37) => Some("GetDiskFreeMegabytes"),
        (0x81, 0x60) => Some("ConfigureScreen"),
        (0x81, 0x62) => Some("Sys2_62"),
        (0x81, 0x63) => Some("SetConfigInputMode"),
        (0x81, 0x64) => Some("ConfigureScreenSize"),
        (0x81, 0x6f) => Some("Sys2_6F"),
        (0x81, 0xf7) => Some("ValidateOrCreateUserPathEx"),
        (0x90, 0x00) => Some("GraphInit"),
        (0x90, 0x01) => Some("GraphShutdownOrReset"),
        (0x90, 0x02) => Some("GraphDelay"),
        (0x90, 0x03) => Some("GraphSetMemoryLimit"),
        (0x90, 0x04) => Some("GraphSetDrawContext"),
        (0x90, 0x05) => Some("GraphSelectWorkBuffer"),
        (0x90, 0x06) => Some("GraphSetCenter"),
        (0x90, 0x07) => Some("GraphSetDefaultDuration"),
        (0x90, 0x08) => Some("GraphSetEnabled"),
        (0x90, 0x09) => Some("GraphSetDefaultPriority"),
        (0x90, 0x0c) => Some("GraphSystemInit"),
        (0x90, 0x0d) => Some("GraphSetFlag"),
        (0x90, 0x0e) => Some("GraphConfigureViewport"),
        (0x90, 0x10) => Some("GraphLoadResource"),
        (0x90, 0x11) => Some("GraphCreateBitmap"),
        (0x90, 0x12) => Some("GraphReleaseBitmap"),
        (0x90, 0x13) => Some("GraphClearBitmap"),
        (0x90, 0x14) => Some("GraphCreateBitmapFromRgb"),
        (0x90, 0x15) => Some("GraphReadBitmapPixels"),
        (0x90, 0x16) => Some("BitmapQueryInfo"),
        (0x90, 0x17) => Some("GraphValidateBitmap"),
        (0x90, 0x18) => Some("GraphObjectApply"),
        (0x90, 0x19) => Some("GraphObjectSetVisibleOrState"),
        (0x90, 0x1e) => Some("GraphBlitRectFast"),
        (0x90, 0x1f) => Some("GraphConfigureBitmapRegion"),
        (0x90, 0x20) => Some("GraphObjectTransition"),
        (0x90, 0x22) => Some("GraphNodeApplyTransition"),
        (0x90, 0x23) => Some("GraphNodeTransitionEx"),
        (0x90, 0x28) => Some("GraphScheduleObjectControl"),
        (0x90, 0x29) => Some("GraphScheduleSplineControl"),
        (0x90, 0x30) => Some("GraphTimelineSetEnabled"),
        (0x90, 0x31) => Some("GraphSetObjectEnabled"),
        (0x90, 0x32) => Some("GraphObjectCommit"),
        (0x90, 0x33) => Some("GraphSetObjectPosition"),
        (0x90, 0x34) => Some("GraphSetObjectMaskAlpha"),
        (0x90, 0x35) => Some("GraphSetObjectScaleX"),
        (0x90, 0x36) => Some("GraphSetObjectPosition"),
        (0x90, 0x37) => Some("GraphSetObjectOrigin"),
        (0x90, 0x38) => Some("GraphSetObjectProperty"),
        (0x90, 0x39) => Some("GraphSurfaceSetVisible"),
        (0x90, 0x3a) => Some("GraphSetNodeResource"),
        (0x90, 0x3c) => Some("GraphSetObjectFormat"),
        (0x90, 0x3d) => Some("GraphHandleExists"),
        (0x90, 0x40) => Some("GraphSetPrimaryBitmap"),
        (0x90, 0x43) => Some("GraphRectTransition"),
        (0x90, 0x47) => Some("GraphConfigureEffectResource"),
        (0x90, 0x4a) => Some("GraphConfigureBlitSources"),
        (0x90, 0x4c) => Some("GraphSystemInit2"),
        (0x90, 0x4d) => Some("GraphGetRenderTarget"),
        (0x90, 0x50) => Some("GraphCreateNode"),
        (0x90, 0x51) => Some("GraphNodeRelease"),
        (0x90, 0x53) => Some("GraphRefreshObjectRect"),
        (0x90, 0x54) => Some("GraphNodeSetEnabled"),
        (0x90, 0x55) => Some("GraphLayerSetProperty"),
        (0x90, 0x56) => Some("GraphNodeConfigure"),
        (0x90, 0x57) => Some("GraphRenderObjectToTarget"),
        (0x90, 0x58) => Some("GraphNodeConfigureLayout"),
        (0x90, 0x5a) => Some("GraphNodeDrawStateEx"),
        (0x90, 0x5c) => Some("GraphNodeConfigureImage"),
        (0x90, 0x5d) => Some("GraphNodeTransitionPattern"),
        (0x90, 0x60) => Some("GraphCreateObject"),
        (0x90, 0x61) => Some("GraphObjectUpdate"),
        (0x90, 0x64) => Some("GraphObjectSetEnabled"),
        (0x90, 0x65) => Some("GraphObjectConfigure"),
        (0x90, 0x66) => Some("GraphObjectApplyEffect"),
        (0x90, 0x80) => Some("GraphCreateSurface"),
        (0x90, 0x81) => Some("GraphSurfaceFlush"),
        (0x90, 0x82) => Some("GraphSurfaceSetVisibleFast"),
        (0x90, 0x83) => Some("GraphSurfaceBindBuffer"),
        (0x90, 0x84) => Some("GraphSurfaceSetEnabled"),
        (0x90, 0x85) => Some("GraphSurfaceSetRegion"),
        (0x90, 0x86) => Some("GraphSurfaceConfigure"),
        (0x90, 0x87) => Some("GraphSurfaceSetOption"),
        (0x90, 0x88) => Some("GraphSurfaceSetViewport"),
        (0x90, 0x89) => Some("GraphTextLayoutRun"),
        (0x90, 0x90) => Some("GraphTextDrawControl"),
        (0x90, 0x94) => Some("GraphSetGlyphRevealDelay"),
        (0x90, 0x95) => Some("GraphSetTextRevealAnimation"),
        (0x90, 0x96) => Some("GraphSetTextSettleAnimation"),
        (0x90, 0x97) => Some("GraphSetTextAutoAdvance"),
        (0x90, 0x98) => Some("GraphConfigureCaretFrames"),
        (0x90, 0x99) => Some("GraphSetCaretFrameDelay"),
        (0x90, 0x9a) => Some("GraphSetCaretPosition"),
        (0x90, 0x9b) => Some("GraphSetMessageStartDelay"),
        (0x90, 0x9c) => Some("GraphSetTextShadowEnabled"),
        (0x90, 0x9d) => Some("GraphSetTextShadowParameters"),
        (0x90, 0x9f) => Some("GraphSetInstantTextReveal"),
        (0x90, 0xaf) => Some("GraphSetForegroundInputGuard"),
        (0x90, 0xb6) => Some("GraphObjectBindInputTable"),
        (0x90, 0xb7) => Some("GraphConfigureSurfaceControls"),
        (0x90, 0xb8) => Some("GraphObjectBeginInput"),
        (0x90, 0xb9) => Some("GraphObjectFinalize"),
        (0x90, 0xb4) => Some("GraphCompositeSpriteBatch"),
        (0x90, 0xba) => Some("GraphObjectAttachInput"),
        (0x90, 0xbc) => Some("GraphPollObjectState"),
        (0x90, 0xbe) => Some("GraphObjectResolveInput"),
        (0x90, 0xbf) => Some("GraphPollObjectEvent"),
        (0x90, 0xcc) => Some("GraphColorAdjustPrepare"),
        (0x90, 0xcd) => Some("GraphColorAdjustedBlit"),
        (0x90, 0xd0) => Some("GraphCreateKnobObject"),
        (0x90, 0xd1) => Some("GraphReleaseScrollState"),
        (0x90, 0xd4) => Some("GraphScrollSetMode"),
        (0x90, 0xd5) => Some("GraphScrollCommit"),
        (0x90, 0xd6) => Some("GraphScrollSetPosition"),
        (0x90, 0xd7) => Some("GraphScrollGetPosition"),
        (0x90, 0xd8) => Some("GraphScrollSetExtent"),
        (0x90, 0xd9) => Some("GraphScrollSetBounds"),
        (0x90, 0xda) => Some("GraphQuerySpecialHandle"),
        (0x90, 0xdb) => Some("GraphTakeSpecialEvent"),
        (0x90, 0xde) => Some("GraphWatchSpecialHandle"),
        (0x90, 0xdf) => Some("GraphUnwatchSpecialHandle"),
        (0x90, 0xdd) => Some("GraphSetDriverMode"),
        (0x90, 0xe0) => Some("GraphCreateTimeline"),
        (0x90, 0xe1) => Some("GraphTimelinePoll"),
        (0x90, 0xe4) => Some("GraphTimelineSetEnabled"),
        (0x90, 0xe5) => Some("GraphTimelineConfigure"),
        (0x90, 0xe8) => Some("GraphTimelineAttach"),
        (0x90, 0xe9) => Some("GraphTimelineQuery"),
        (0x90, 0xf1) => Some("GraphShutdownDriver"),
        (0x90, 0xf2) => Some("GraphQueryDriverStatus"),
        (0x90, 0xf3) => Some("MovieSetVolume"),
        (0x90, 0xf0) => Some("MovieOpenBlocking"),
        (0x90, 0xf4) => Some("MovieCreateLoader"),
        (0x90, 0xf5) => Some("GraphReleaseProcess"),
        (0x90, 0xf6) => Some("GraphTransitionEvaluate"),
        (0x90, 0xf7) => Some("GraphAttachProcess"),
        (0x91, 0x0d) => Some("GraphSetLayerEnabled"),
        (0x91, 0x0e) => Some("GraphConfigureLayer"),
        (0x91, 0x06) => Some("GraphSetGlobalDisplayOffset"),
        (0x91, 0x10) => Some("GraphEffectSetZoomRect"),
        (0x91, 0x11) => Some("GraphEffectSetDiffuse"),
        (0x91, 0x12) => Some("GraphEffectSetColorTone"),
        (0x91, 0x13) => Some("GraphEffectSetRotation"),
        (0x91, 0x15) => Some("GraphEffectSetWave"),
        (0x91, 0x16) => Some("GraphEffectSetClipRect"),
        (0x91, 0x19) => Some("GraphLayerTransform"),
        (0x91, 0x1b) => Some("GraphBlendBitmapsScaled"),
        (0x91, 0x1c) => Some("GraphCreateScaledBitmap"),
        (0x91, 0x1d) => Some("GraphLayerColorBlend"),
        (0x91, 0x1e) => Some("GraphLayerSys1E"),
        (0x91, 0x1f) => Some("GraphCloneBitmap"),
        (0x91, 0x33) => Some("GraphObjectSetFixedPosition"),
        (0x91, 0x36) => Some("GraphObjectSetTransformVector3"),
        (0x91, 0x37) => Some("GraphObjectSetBaseVector3"),
        (0x91, 0x38) => Some("GraphConfigureResourceProperty"),
        (0x91, 0x3e) => Some("GraphAttachSurface"),
        (0x91, 0x3f) => Some("GraphDetachSurface"),
        (0x91, 0x40) => Some("GraphLayerAffineTransform"),
        (0x91, 0x48) => Some("GraphLayerSys48"),
        (0x91, 0x49) => Some("GraphLayerSys49"),
        (0x91, 0x4a) => Some("GraphLayerSys4A"),
        (0x91, 0x55) => Some("GraphLayerLink"),
        (0x91, 0x60) => Some("GraphTempLayerCreate"),
        (0x91, 0x61) => Some("GraphTempLayerRelease"),
        (0x91, 0x64) => Some("GraphTempLayerSetEnabled"),
        (0x91, 0x65) => Some("GraphTempLayerBlit"),
        (0x91, 0x66) => Some("GraphTempLayerCopy"),
        (0x91, 0x88) => Some("ConfigureFormatInfo"),
        (0x91, 0x89) => Some("GraphLayerFormatOption"),
        (0x91, 0x8b) => Some("GraphTextSetWritingMode"),
        (0x91, 0x8c) => Some("GraphTextSetCursorPosition"),
        (0x91, 0x8d) => Some("GraphTextGetCursorPosition"),
        (0x91, 0x8e) => Some("GraphTextCursorReachedBoundary"),
        (0x91, 0x94) => Some("GraphUpdateRubySubstitution"),
        (0x91, 0x95) => Some("GraphCollectRubySubstitutions"),
        (0x91, 0x96) => Some("GraphRegisterRubySubstitutions"),
        (0x91, 0x98) => Some("GraphConfigureTextLayoutDefaults"),
        (0x91, 0x9a) => Some("GraphSetLayerProperty"),
        (0x91, 0x9b) => Some("GraphMeasureText"),
        (0x91, 0x9c) => Some("GraphDrawTextEx"),
        (0x91, 0x9f) => Some("StripMarkupTags"),
        (0x91, 0xb8) => Some("GraphCreateSurfaceInputObject"),
        (0x91, 0xba) => Some("GraphConfigureInputRegions"),
        (0x91, 0xdb) => Some("GraphCurrentSpecialHandle"),
        (0x91, 0xf0) => Some("GraphEffectProcessCreate"),
        (0x91, 0xf1) => Some("GraphEffectProcessInvoke"),
        (0x91, 0xf2) => Some("GraphEffectProcessCancel"),
        (0x91, 0xf3) => Some("GraphEffectProcessState"),
        (0x91, 0xf4) => Some("GraphEffectProcessCreateRect"),
        (0x91, 0xf5) => Some("GraphEffectProcessPoll"),
        (0x91, 0xf6) => Some("GraphEffectProcessRelease"),
        (0x91, 0xf7) => Some("GraphEffectProcessResult"),
        (0x92, 0x01) => Some("GraphConfigureWaveTable"),
        (0x92, 0x10) => Some("GraphGenerateRippleMap"),
        (0x92, 0x12) => Some("GraphSetBitmapDimensions"),
        (0x92, 0x14) => Some("GraphPreloadResource"),
        (0x92, 0x15) => Some("GraphResourceFlushQueue"),
        (0x92, 0x16) => Some("GraphGetBitmapDimensions"),
        (0x92, 0x18) => Some("GraphTextSys18"),
        (0x92, 0x19) => Some("GraphResourceCommit"),
        (0x92, 0x1e) => Some("GraphTextFillRect"),
        (0x92, 0x17) => Some("GraphTextSetColorState"),
        (0x92, 0x88) => Some("GraphSetTextObjectValue"),
        (0x92, 0x8c) => Some("GraphTextSys8C"),
        (0x92, 0x8d) => Some("GraphApplyEffectResource"),
        (0x92, 0x8e) => Some("GraphTextBegin"),
        (0x92, 0x89) => Some("GraphDrawResourceToSurface"),
        (0x92, 0x90) => Some("GraphTextDrawStyled"),
        (0x92, 0x91) => Some("GraphDrawFormattedText"),
        (0x92, 0x97) => Some("GraphConfigureTextStyleDefaults"),
        (0x92, 0x9c) => Some("RenderText"),
        (0x92, 0xf0) => Some("MovieOpen"),
        (0x92, 0xf1) => Some("GraphTransitionLoad"),
        (0x92, 0xf2) => Some("GraphTransitionConfigure"),
        (0x92, 0xf4) => Some("GraphTransitionPoll"),
        (0x92, 0xf5) => Some("MovieGetState"),
        (0xa0, 0x08) => Some("SoundSetGroupVolume"),
        (0xa0, 0x09) => Some("SoundSetChannelVolume"),
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
        (0xa0, 0x28) => Some("SoundCreateRegistrationProcess"),
        (0xb0, 0x08) => Some("DebugCreateRenderer"),
        (0xb0, 0x10) => Some("DebugMeasureText"),
        (0xb0, 0x11) => Some("DebugReleaseRenderer"),
        (0xb0, 0x14) => Some("DebugSetRendererState"),
        (0xb0, 0x17) => Some("DebugGetRendererPoint"),
        (0xb0, 0x19) => Some("DebugDrawText"),
        (0xb0, 0x1c) => Some("DebugWriteLine"),
        (0xb0, 0x82) => Some("UserMessageBox"),
        (0xb0, 0x8c) => Some("UserModalDialog"),
        (0xc0, 0x10) => Some("ParticleConfigureObject"),
        (0xc0, 0x1a) => Some("ParticleLoadFrames"),
        (0xc0, 0x1b) => Some("ParticleCommitFrames"),
        (0xc0, 0x20) => Some("ParticleSetObjectState"),
        (0xc0, 0x40) => Some("RainCreate"),
        (0xc0, 0x42) => Some("RainInitialize"),
        (0xc0, 0x43) => Some("RainSetTexture"),
        (0xc0, 0x44) => Some("RainSetEnabled"),
        (0xc0, 0x45) => Some("RainSetBounds"),
        (0xc0, 0x46) => Some("RainSetDropWidth"),
        (0xc0, 0x47) => Some("RainSetDropHeight"),
        (0xc0, 0x48) => Some("RainSetDropColor"),
        (0xc0, 0x49) => Some("RainSetDensity"),
        (0xc0, 0x4a) => Some("RainSetSpeed"),
        (0xc0, 0x4b) => Some("RainSetAngle"),
        (0xc0, 0x4c) => Some("RainSetOrigin"),
        (0xc0, 0x4d) => Some("RainSetDirection"),
        (0xc0, 0x4e) => Some("RainSetLength"),
        (0xb0, 0x02) => Some("UserInputInitializeDefault"),
        (0xb0, 0x03) => Some("UserInputInitializeHost"),
        (0xb0, 0x05) => Some("UserSys05"),
        (0xb0, 0x06) => Some("UserInputAllowed"),
        (0xb0, 0x80) => Some("UserMessage"),
        (0xb0, 0xc1) => Some("UserMeasureText"),
        (0xb0, 0xc4) => Some("UserSetFontOption"),
        (0xb0, 0xc7) => Some("UserSetFontName"),
        (0xc0, 0x00) => Some("UserCreateScreenGraphObject"),
        (0xc0, 0x01) => Some("User2Release"),
        (0xc0, 0x04) => Some("User2SetEnabled"),
        (0xc0, 0x05) => Some("User2ConfigureRegion"),
        (0xc0, 0x09) => Some("User2SetInterval"),
        (0xc0, 0x0a) => Some("User2SetCapacity"),
        (0xc0, 0x0b) => Some("User2SetLayout"),
        (0xc0, 0x0c) => Some("User2SetRepeatInterval"),
        (0xc0, 0x0d) => Some("User2SetDuration"),
        (0xc0, 0x0f) => Some("User2Commit"),
        (0xc0, 0x18) => Some("User2ConfigureAnimation"),
        (0xc0, 0x1f) => Some("User2_1F"),
        (0xc0, 0x28) => Some("User2ConfigureBounds"),
        (0xc0, 0x29) => Some("User2ConfigureStyle"),
        (0xc0, 0x2d) => Some("User2ConfigureAdvancedStyle"),
        (0xc0, 0x41) => Some("User2ReleaseEx"),
        (0xc0, 0x4f) => Some("User2SetStep"),
        _ => native_abi::generic_name(group, id),
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
            0x15 | 0x80 | 0x81 | 0x90 | 0x91 | 0x92 | 0xa0 | 0xb0 | 0xc0 | 0xd0 | 0xe0 => {
                if let Some(value) = read_u8_operand(&mut cursor, &mut raw, &mut warning) {
                    operands.push(BpOperand::U8(value));
                    if !matches!(opcode_byte, 0xd0 | 0xe0) {
                        known_call = known_call_name(opcode_byte, value as u16);
                    }
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
        assert_eq!(opcode_name(0x43), Some("atan2"));
        assert_eq!(opcode_name(0x44), Some("vec3_length"));
        assert_eq!(opcode_name(0x77), Some("engine_state"));
        assert_eq!(opcode_name(0xff), Some("script_extension"));
    }

    #[test]
    fn names_every_opcode_registered_by_the_native_interpreter() {
        assert_eq!(NATIVE_REGISTERED_OPCODES.len(), 89);
        assert!(NATIVE_REGISTERED_OPCODES
            .iter()
            .all(|opcode| opcode_name(*opcode).is_some()));
    }

    #[test]
    fn secondary_opcode_domains_consume_their_selector() {
        let instructions = disassemble_bp(&[0xd0, 0x40, 0xe0, 0x80, 0x17]);
        assert_eq!(instructions.len(), 3);
        assert_eq!(instructions[0].raw, vec![0xd0, 0x40]);
        assert_eq!(instructions[0].opcode.name(), "legacy_3d");
        assert_eq!(instructions[1].raw, vec![0xe0, 0x80]);
        assert_eq!(instructions[1].opcode.name(), "debug_inspect");
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
