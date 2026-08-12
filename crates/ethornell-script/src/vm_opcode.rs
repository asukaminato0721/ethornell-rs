//! Canonical BP VM opcode reference embedded in the source tree.
//!
//! This table is intentionally evidence graded. `high` means the target
//! executable or its dispatch table establishes the operation. `medium` means
//! the current implementation and script call sites agree but more native
//! annotation is desirable. `low` means only registration or a provisional
//! implementation name is known; those entries must not be treated as proven
//! reverse engineering results.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BpOpcodeSpec {
    pub code: u8,
    pub symbol: &'static str,
    pub stack_effect: &'static str,
    pub immediate: &'static str,
    pub description: &'static str,
    pub confidence: &'static str,
    pub evidence: &'static str,
}

/// Complete non-null opcode table for the target interpreter.
///
/// Stack effects are written in native operand-ring terms. `implementation
/// defined` marks a contract that still needs direct target recovery.
pub const BP_OPCODE_SPECS: &[BpOpcodeSpec] = &[
    // 0x00 push_byte: Sign-extend an 8-bit literal into one BP value.
    BpOpcodeSpec {
        code: 0x00,
        symbol: "push_byte",
        stack_effect: "0 -> 1",
        immediate: "i8 immediate",
        description: "Sign-extend an 8-bit literal and push it as one BP value.",
        confidence: "high",
        evidence: "native opcode table and interpreter implementation",
    },
    // 0x01 push_word: Sign-extend a 16-bit literal into one BP value.
    BpOpcodeSpec {
        code: 0x01,
        symbol: "push_word",
        stack_effect: "0 -> 1",
        immediate: "i16 immediate",
        description: "Sign-extend a 16-bit literal and push it as one BP value.",
        confidence: "high",
        evidence: "native opcode table and interpreter implementation",
    },
    // 0x02 push_dword: Push 32 bit literal as one BP value.
    BpOpcodeSpec {
        code: 0x02,
        symbol: "push_dword",
        stack_effect: "0 -> 1",
        immediate: "u32 immediate",
        description: "Push 32 bit literal as one BP value.",
        confidence: "high",
        evidence: "native opcode table and interpreter implementation",
    },
    // 0x04 push_base_offset: Subtract a signed displacement from the raw frame base and tag it.
    BpOpcodeSpec {
        code: 0x04,
        symbol: "push_base_offset",
        stack_effect: "0 -> 1",
        immediate: "i16 frame displacement",
        description: "Push (raw_frame_base - displacement) tagged as a base-relative BP pointer.",
        confidence: "high",
        evidence: "native pointer resolver and opcode handler",
    },
    // 0x05 push_string: Push a tagged string pointer at a signed code-relative displacement.
    BpOpcodeSpec {
        code: 0x05,
        symbol: "push_string",
        stack_effect: "0 -> 1",
        immediate: "i16 displacement from the opcode address",
        description: "Push a tagged pointer to NUL-terminated Shift-JIS bytes in BP code memory.",
        confidence: "high",
        evidence: "native handler and script corpus",
    },
    // 0x06 push_offset: Push a raw code offset at a signed code-relative displacement.
    BpOpcodeSpec {
        code: 0x06,
        symbol: "push_offset",
        stack_effect: "0 -> 1",
        immediate: "i16 displacement from the opcode address",
        description: "Push the raw BP code offset selected by a signed relative displacement.",
        confidence: "high",
        evidence: "target sub_473650 and BP call corpus",
    },
    // 0x08 load: Pop an address and load a value of the selected width.
    BpOpcodeSpec {
        code: 0x08,
        symbol: "load",
        stack_effect: "1 -> 1",
        immediate: "width selector",
        description: "Pop an address and load a value of the selected width.",
        confidence: "high",
        evidence: "native load handler",
    },
    // 0x09 move: Store through a destination pointer and push the assigned value back.
    BpOpcodeSpec {
        code: 0x09,
        symbol: "move",
        stack_effect: "2 -> 1",
        immediate: "width selector",
        description: "Pop value then destination, store by width, and push the assigned value.",
        confidence: "high",
        evidence: "native assignment handler",
    },
    // 0x0A move_arg: Store a value through an argument or frame relative destination.
    BpOpcodeSpec {
        code: 0x0A,
        symbol: "move_arg",
        stack_effect: "2 -> 0",
        immediate: "width selector",
        description: "Store a value through an argument or frame relative destination.",
        confidence: "medium",
        evidence: "current VM and script call sites",
    },
    // 0x0B copy_inline: Copy an inline byte payload into one destination pointer.
    BpOpcodeSpec {
        code: 0x0B,
        symbol: "copy_inline",
        stack_effect: "1 -> 0",
        immediate: "u8 byte count followed by that many payload bytes",
        description: "Pop one destination pointer and copy the inline payload into BP memory.",
        confidence: "high",
        evidence: "target sub_473790 and instruction-stream reader",
    },
    // 0x0C copy_stack: Store a fixed count of stack values into consecutive destination slots.
    BpOpcodeSpec {
        code: 0x0C,
        symbol: "copy_stack",
        stack_effect: "count + 1 -> 0",
        immediate: "u8 width selector, u8 value count",
        description: "Pop count values and one destination, then store consecutive typed slots.",
        confidence: "high",
        evidence: "target sub_4737C0",
    },
    // 0x10 load_base: Push the current raw frame-memory offset.
    BpOpcodeSpec {
        code: 0x10,
        symbol: "load_base",
        stack_effect: "0 -> 1",
        immediate: "none",
        description: "Push the current raw frame-memory offset without applying a pointer tag.",
        confidence: "high",
        evidence: "native CThread frame helpers",
    },
    // 0x11 store_base: Replace the current raw frame-memory offset.
    BpOpcodeSpec {
        code: 0x11,
        symbol: "store_base",
        stack_effect: "1 -> 0",
        immediate: "none",
        description: "Pop and validate the new raw frame-memory offset.",
        confidence: "high",
        evidence: "native CThread frame helpers",
    },
    // 0x14 jmp: Unconditional control flow transfer.
    BpOpcodeSpec {
        code: 0x14,
        symbol: "jmp",
        stack_effect: "1 -> 0",
        immediate: "none",
        description: "Pop a raw BP code offset and transfer control to it.",
        confidence: "high",
        evidence: "native dispatcher",
    },
    // 0x15 jc: Pop target and condition, then apply one of six signed predicates.
    BpOpcodeSpec {
        code: 0x15,
        symbol: "jc",
        stack_effect: "2 -> 0",
        immediate: "u8 predicate selector (0..5)",
        description: "Pop target then condition and branch when the selected predicate is true.",
        confidence: "high",
        evidence: "native dispatcher",
    },
    // 0x16 call: Enter a BP subroutine and push a return record.
    BpOpcodeSpec {
        code: 0x16,
        symbol: "call",
        stack_effect: "1 -> call frame",
        immediate: "none",
        description: "Pop a BP subroutine offset, create a return record, and enter the callee.",
        confidence: "high",
        evidence: "native CThread return stack",
    },
    // 0x17 ret: Return from a BP subroutine; with no return record the program ends.
    BpOpcodeSpec {
        code: 0x17,
        symbol: "ret",
        stack_effect: "frame dependent",
        immediate: "none",
        description: "Return from a BP subroutine; with no return record the program ends.",
        confidence: "high",
        evidence: "native CThread return stack",
    },
    // 0x20 add: Wrapping 32 bit integer addition.
    BpOpcodeSpec {
        code: 0x20,
        symbol: "add",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Wrapping 32 bit integer addition.",
        confidence: "high",
        evidence: "native arithmetic handler",
    },
    // 0x21 sub: Wrapping 32 bit integer subtraction.
    BpOpcodeSpec {
        code: 0x21,
        symbol: "sub",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Wrapping 32 bit integer subtraction.",
        confidence: "high",
        evidence: "native arithmetic handler",
    },
    // 0x22 mul: Wrapping 32 bit integer multiplication.
    BpOpcodeSpec {
        code: 0x22,
        symbol: "mul",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Wrapping 32 bit integer multiplication.",
        confidence: "high",
        evidence: "native arithmetic handler",
    },
    // 0x23 div: Signed integer division; target zero divisor behavior must remain target compatible.
    BpOpcodeSpec {
        code: 0x23,
        symbol: "div",
        stack_effect: "2 -> 1",
        immediate: "none",
        description:
            "Signed integer division; target zero divisor behavior must remain target compatible.",
        confidence: "medium",
        evidence: "current VM and call corpus",
    },
    // 0x24 mod: Signed integer remainder.
    BpOpcodeSpec {
        code: 0x24,
        symbol: "mod",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Signed integer remainder.",
        confidence: "medium",
        evidence: "current VM and call corpus",
    },
    // 0x25 bit_and: Bitwise AND.
    BpOpcodeSpec {
        code: 0x25,
        symbol: "and",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Bitwise AND.",
        confidence: "high",
        evidence: "native arithmetic handler",
    },
    // 0x26 bit_or: Bitwise OR.
    BpOpcodeSpec {
        code: 0x26,
        symbol: "or",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Bitwise OR.",
        confidence: "high",
        evidence: "native arithmetic handler",
    },
    // 0x27 bit_xor: Bitwise XOR.
    BpOpcodeSpec {
        code: 0x27,
        symbol: "xor",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Bitwise XOR.",
        confidence: "high",
        evidence: "native arithmetic handler",
    },
    // 0x28 bit_not: Bitwise complement.
    BpOpcodeSpec {
        code: 0x28,
        symbol: "not",
        stack_effect: "1 -> 1",
        immediate: "none",
        description: "Bitwise complement.",
        confidence: "high",
        evidence: "native arithmetic handler",
    },
    // 0x29 shift_left: Shift left by the low five bits of the count.
    BpOpcodeSpec {
        code: 0x29,
        symbol: "shl",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Shift left by the low five bits of the count.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x2A shift_right_logical: Logical right shift.
    BpOpcodeSpec {
        code: 0x2A,
        symbol: "shr",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Logical right shift.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x2B shift_right_arithmetic: Arithmetic right shift.
    BpOpcodeSpec {
        code: 0x2B,
        symbol: "sar",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Arithmetic right shift.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x30 equal: Push 1 when operands compare equal, otherwise 0.
    BpOpcodeSpec {
        code: 0x30,
        symbol: "eq",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Push 1 when operands compare equal, otherwise 0.",
        confidence: "high",
        evidence: "native comparison handler",
    },
    // 0x31 not_equal: Push 1 when operands differ, otherwise 0.
    BpOpcodeSpec {
        code: 0x31,
        symbol: "neq",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Push 1 when operands differ, otherwise 0.",
        confidence: "high",
        evidence: "native comparison handler",
    },
    // 0x32 less_or_equal: Signed less than or equal comparison.
    BpOpcodeSpec {
        code: 0x32,
        symbol: "leq",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Signed less than or equal comparison.",
        confidence: "high",
        evidence: "native comparison handler",
    },
    // 0x33 greater_or_equal: Signed greater than or equal comparison.
    BpOpcodeSpec {
        code: 0x33,
        symbol: "geq",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Signed greater than or equal comparison.",
        confidence: "high",
        evidence: "native comparison handler",
    },
    // 0x34 less_than: Signed less than comparison.
    BpOpcodeSpec {
        code: 0x34,
        symbol: "lt",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Signed less than comparison.",
        confidence: "high",
        evidence: "native comparison handler",
    },
    // 0x35 greater_than: Signed greater than comparison.
    BpOpcodeSpec {
        code: 0x35,
        symbol: "gt",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Signed greater than comparison.",
        confidence: "high",
        evidence: "native comparison handler",
    },
    // 0x38 boolean_and: Logical AND: each operand is tested for nonzero and the result is 0 or 1.
    BpOpcodeSpec {
        code: 0x38,
        symbol: "boolean_and",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Logical AND: each operand is tested for nonzero and the result is 0 or 1.",
        confidence: "high",
        evidence: "target opcode table slot 0x38 and native handler",
    },
    // 0x39 boolean_or: Logical OR: each operand is tested for nonzero and the result is 0 or 1.
    BpOpcodeSpec {
        code: 0x39,
        symbol: "boolean_or",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Logical OR: each operand is tested for nonzero and the result is 0 or 1.",
        confidence: "high",
        evidence: "target opcode table slot 0x39 and native handler",
    },
    // 0x3A boolean_not: Push 1 when the operand is zero, otherwise 0.
    BpOpcodeSpec {
        code: 0x3A,
        symbol: "bool_zero",
        stack_effect: "1 -> 1",
        immediate: "none",
        description: "Push 1 when the operand is zero, otherwise 0.",
        confidence: "high",
        evidence: "native boolean handler",
    },
    // 0x40 select: Pop condition, true value and false value and push the selected value.
    BpOpcodeSpec {
        code: 0x40,
        symbol: "ternary",
        stack_effect: "3 -> 1",
        immediate: "none",
        description: "Pop condition, true value and false value and push the selected value.",
        confidence: "medium",
        evidence: "current VM and script call sites",
    },
    // 0x42 mul_div: Compute multiplicand times multiplier divided by divisor using a wide intermediate.
    BpOpcodeSpec {
        code: 0x42,
        symbol: "muldiv",
        stack_effect: "3 -> 1",
        immediate: "none",
        description:
            "Compute multiplicand times multiplier divided by divisor using a wide intermediate.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x43 atan2_fixed: Return atan2 angle in 16.16 degree representation.
    BpOpcodeSpec {
        code: 0x43,
        symbol: "atan2",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Return atan2 angle in 16.16 degree representation.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x44 vec3_length: Return Euclidean length of three integer components.
    BpOpcodeSpec {
        code: 0x44,
        symbol: "vec3_length",
        stack_effect: "3 -> 1",
        immediate: "none",
        description: "Return Euclidean length of three integer components.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x48 sin_fixed: Sine using a 16.16 degree angle and result scale.
    BpOpcodeSpec {
        code: 0x48,
        symbol: "sin",
        stack_effect: "1 -> 1",
        immediate: "none",
        description: "Sine using a 16.16 degree angle and result scale.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x49 cos_fixed: Cosine using a 16.16 degree angle and result scale.
    BpOpcodeSpec {
        code: 0x49,
        symbol: "cos",
        stack_effect: "1 -> 1",
        immediate: "none",
        description: "Cosine using a 16.16 degree angle and result scale.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x50 qword_add: 64 bit addition using the BP operand representation.
    BpOpcodeSpec {
        code: 0x50,
        symbol: "qword_add",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "64 bit addition using the BP operand representation.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x51 qword_sub: 64 bit subtraction using the BP operand representation.
    BpOpcodeSpec {
        code: 0x51,
        symbol: "qword_sub",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "64 bit subtraction using the BP operand representation.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x52 qword_mul: 64 bit multiplication using the BP operand representation.
    BpOpcodeSpec {
        code: 0x52,
        symbol: "qword_mul",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "64 bit multiplication using the BP operand representation.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x53 qword_div: 64 bit division using the BP operand representation.
    BpOpcodeSpec {
        code: 0x53,
        symbol: "qword_div",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "64 bit division using the BP operand representation.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x54 qword_mod: 64 bit remainder using the BP operand representation.
    BpOpcodeSpec {
        code: 0x54,
        symbol: "qword_mod",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "64 bit remainder using the BP operand representation.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x60 memory_copy: Pop destination, source and byte count and copy bytes.
    BpOpcodeSpec {
        code: 0x60,
        symbol: "memcpy",
        stack_effect: "3 -> 0",
        immediate: "none",
        description: "Pop destination, source and byte count and copy bytes.",
        confidence: "high",
        evidence: "native memory handler",
    },
    // 0x61 memory_clear: Pop address and byte count and fill with zero.
    BpOpcodeSpec {
        code: 0x61,
        symbol: "memclr",
        stack_effect: "2 -> 0",
        immediate: "none",
        description: "Pop address and byte count and fill with zero.",
        confidence: "high",
        evidence: "native memory handler",
    },
    // 0x62 memory_set: Pop address, byte count and fill byte.
    BpOpcodeSpec {
        code: 0x62,
        symbol: "memset",
        stack_effect: "3 -> 0",
        immediate: "none",
        description: "Pop address, byte count and fill byte.",
        confidence: "high",
        evidence: "native memory handler",
    },
    // 0x63 memory_equal: Compare two fixed size memory ranges and push boolean equality. It is not libc memcmp.
    BpOpcodeSpec {
        code: 0x63,
        symbol: "memory_equal",
        stack_effect: "3 -> 1",
        immediate: "none",
        description:
            "Compare two fixed size memory ranges and push boolean equality. It is not libc memcmp.",
        confidence: "high",
        evidence: "target opcode table slot 0x63 and native handler",
    },
    // 0x64 memory_repeat_copy: Repeat one source block into a destination range.
    BpOpcodeSpec {
        code: 0x64,
        symbol: "memrepeat",
        stack_effect: "4 -> 0",
        immediate: "none",
        description: "Repeat one source block into a destination range.",
        confidence: "high",
        evidence: "native handler annotation",
    },
    // 0x65 memory_find_block: Find a fixed byte block and return its offset or failure value.
    BpOpcodeSpec {
        code: 0x65,
        symbol: "memfind",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Find a fixed byte block and return its offset or failure value.",
        confidence: "high",
        evidence: "native handler annotation",
    },
    // 0x66 string_find_offset: Find a substring and return a byte offset or failure value.
    BpOpcodeSpec {
        code: 0x66,
        symbol: "strfind",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Find a substring and return a byte offset or failure value.",
        confidence: "high",
        evidence: "native handler annotation",
    },
    // 0x67 string_replace: Replace string content using BP managed buffers.
    BpOpcodeSpec {
        code: 0x67,
        symbol: "strreplace",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Replace string content using BP managed buffers.",
        confidence: "medium",
        evidence: "current VM and external reference; target details pending",
    },
    // 0x68 string_length: Return byte length of a NUL terminated string.
    BpOpcodeSpec {
        code: 0x68,
        symbol: "strlen",
        stack_effect: "1 -> 1",
        immediate: "none",
        description: "Return byte length of a NUL terminated string.",
        confidence: "high",
        evidence: "native string handler",
    },
    // 0x69 string_equal: Compare two NUL terminated strings for equality.
    BpOpcodeSpec {
        code: 0x69,
        symbol: "streq",
        stack_effect: "2 -> 1",
        immediate: "none",
        description: "Compare two NUL terminated strings for equality.",
        confidence: "high",
        evidence: "native string handler",
    },
    // 0x6A string_copy: Copy native string bytes to destination.
    BpOpcodeSpec {
        code: 0x6A,
        symbol: "strcpy",
        stack_effect: "2 -> implementation defined",
        immediate: "none",
        description: "Copy native string bytes to destination.",
        confidence: "high",
        evidence: "target opcode handler annotation",
    },
    // 0x6B string_concat: Concatenate string values into destination.
    BpOpcodeSpec {
        code: 0x6B,
        symbol: "strconcat",
        stack_effect: "3 -> implementation defined",
        immediate: "none",
        description: "Concatenate string values into destination.",
        confidence: "medium",
        evidence: "current VM implementation",
    },
    // 0x6C get_character: Decode one character and produce character value plus cursor information.
    BpOpcodeSpec {
        code: 0x6C,
        symbol: "getchar",
        stack_effect: "1 -> 3",
        immediate: "none",
        description: "Decode one character and produce character value plus cursor information.",
        confidence: "high",
        evidence: "recovered stack contract",
    },
    // 0x6D to_lower: Lowercase or normalize a character in place; this opcode has no BP stack output.
    BpOpcodeSpec {
        code: 0x6D,
        symbol: "tolower",
        stack_effect: "1 -> 0",
        immediate: "none",
        description:
            "Lowercase or normalize a character in place; this opcode has no BP stack output.",
        confidence: "high",
        evidence: "recovered stack contract",
    },
    // 0x6E quote_string: Escape or quote a string for formatted output.
    BpOpcodeSpec {
        code: 0x6E,
        symbol: "quote_string",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Escape or quote a string for formatted output.",
        confidence: "low",
        evidence: "current name; native semantics need further recovery",
    },
    // 0x6F format_string: Format text into a BP destination buffer.
    BpOpcodeSpec {
        code: 0x6F,
        symbol: "sprintf",
        stack_effect: "variable -> implementation defined",
        immediate: "format embedded in stack values",
        description: "Format text into a BP destination buffer.",
        confidence: "medium",
        evidence: "current VM and script call sites",
    },
    // 0x70 allocate: Allocate BP heap memory and push an encoded pointer.
    BpOpcodeSpec {
        code: 0x70,
        symbol: "malloc",
        stack_effect: "1 -> 1",
        immediate: "none",
        description: "Allocate BP heap memory and push an encoded pointer.",
        confidence: "medium",
        evidence: "current VM memory model",
    },
    // 0x71 free_allocation: Release BP heap memory and push success status.
    BpOpcodeSpec {
        code: 0x71,
        symbol: "free",
        stack_effect: "1 -> 1",
        immediate: "none",
        description: "Release BP heap memory and push success status.",
        confidence: "high",
        evidence: "recovered stack contract",
    },
    // 0x74 set_memory_mode: Configure BP memory behavior. Exact target fields remain unresolved.
    BpOpcodeSpec {
        code: 0x74,
        symbol: "set_memory_mode",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Configure BP memory behavior. Exact target fields remain unresolved.",
        confidence: "low",
        evidence: "registered handler only",
    },
    // 0x75 add_memory_boundary: Register an address boundary and return a status or handle.
    BpOpcodeSpec {
        code: 0x75,
        symbol: "addmemboundary",
        stack_effect: "3 -> 1",
        immediate: "none",
        description: "Register an address boundary and return a status or handle.",
        confidence: "high",
        evidence: "recovered stack contract",
    },
    // 0x77 engine_state: Query or modify an engine state value.
    BpOpcodeSpec {
        code: 0x77,
        symbol: "engine_state",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Query or modify an engine state value.",
        confidence: "low",
        evidence: "registered handler; semantics unresolved",
    },
    // 0x78 confirm: Native confirmation or diagnostic interaction.
    BpOpcodeSpec {
        code: 0x78,
        symbol: "confirm",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Native confirmation or diagnostic interaction.",
        confidence: "low",
        evidence: "registered handler; semantics unresolved",
    },
    // 0x79 message_box: Native diagnostic or message display.
    BpOpcodeSpec {
        code: 0x79,
        symbol: "message_box",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Native diagnostic or message display.",
        confidence: "low",
        evidence: "registered handler; semantics unresolved",
    },
    // 0x7A assert: Native assertion operation.
    BpOpcodeSpec {
        code: 0x7A,
        symbol: "assert",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Native assertion operation.",
        confidence: "low",
        evidence: "registered handler; semantics unresolved",
    },
    // 0x7B dump_memory: Native memory diagnostic operation.
    BpOpcodeSpec {
        code: 0x7B,
        symbol: "dumpmem",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Native memory diagnostic operation.",
        confidence: "low",
        evidence: "registered handler; semantics unresolved",
    },
    // 0x7C modal_list: Native modal selection operation.
    BpOpcodeSpec {
        code: 0x7C,
        symbol: "modal_list",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Native modal selection operation.",
        confidence: "low",
        evidence: "registered handler; semantics unresolved",
    },
    // 0x7D resource_transform: Native resource transformation operation.
    BpOpcodeSpec {
        code: 0x7D,
        symbol: "resource_transform",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Native resource transformation operation.",
        confidence: "low",
        evidence: "registered handler; semantics unresolved",
    },
    // 0x7E clipboard_set: Write text or data to the host clipboard.
    BpOpcodeSpec {
        code: 0x7E,
        symbol: "clipboard_set",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Write text or data to the host clipboard.",
        confidence: "low",
        evidence: "registered handler; semantics unresolved",
    },
    // 0x7F resource_blend: Native resource blend operation.
    BpOpcodeSpec {
        code: 0x7F,
        symbol: "resource_blend",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Native resource blend operation.",
        confidence: "low",
        evidence: "registered handler; semantics unresolved",
    },
    // 0x80 dispatch_system80: Dispatch through the System80 native handler table.
    BpOpcodeSpec {
        code: 0x80,
        symbol: "sys1",
        stack_effect: "ABI table controlled",
        immediate: "u8 secondary id",
        description: "Dispatch through the System80 native handler table.",
        confidence: "high",
        evidence: "target dispatch table",
    },
    // 0x81 dispatch_system81: Dispatch through the System81 native handler table.
    BpOpcodeSpec {
        code: 0x81,
        symbol: "sys2",
        stack_effect: "ABI table controlled",
        immediate: "u8 secondary id",
        description: "Dispatch through the System81 native handler table.",
        confidence: "high",
        evidence: "target dispatch table",
    },
    // 0x90 dispatch_graph90: Dispatch through the Graph90 native handler table.
    BpOpcodeSpec {
        code: 0x90,
        symbol: "grp1",
        stack_effect: "ABI table controlled",
        immediate: "u8 secondary id",
        description: "Dispatch through the Graph90 native handler table.",
        confidence: "high",
        evidence: "target dispatch table",
    },
    // 0x91 dispatch_graph91: Dispatch through the Graph91 native handler table.
    BpOpcodeSpec {
        code: 0x91,
        symbol: "grp2",
        stack_effect: "ABI table controlled",
        immediate: "u8 secondary id",
        description: "Dispatch through the Graph91 native handler table.",
        confidence: "high",
        evidence: "target dispatch table",
    },
    // 0x92 dispatch_graph92: Dispatch through the Graph92 native handler table.
    BpOpcodeSpec {
        code: 0x92,
        symbol: "grp3",
        stack_effect: "ABI table controlled",
        immediate: "u8 secondary id",
        description: "Dispatch through the Graph92 native handler table.",
        confidence: "high",
        evidence: "target dispatch table",
    },
    // 0xA0 dispatch_soundA0: Dispatch through the SoundA0 native handler table.
    BpOpcodeSpec {
        code: 0xA0,
        symbol: "snd1",
        stack_effect: "ABI table controlled",
        immediate: "u8 secondary id",
        description: "Dispatch through the SoundA0 native handler table.",
        confidence: "high",
        evidence: "target dispatch table",
    },
    // 0xB0 dispatch_userB0: Dispatch through the UserB0 native handler table.
    BpOpcodeSpec {
        code: 0xB0,
        symbol: "usr1",
        stack_effect: "ABI table controlled",
        immediate: "u8 secondary id",
        description: "Dispatch through the UserB0 native handler table.",
        confidence: "high",
        evidence: "target dispatch table",
    },
    // 0xC0 dispatch_userC0: Dispatch through the UserC0 native handler table.
    BpOpcodeSpec {
        code: 0xC0,
        symbol: "usr2",
        stack_effect: "ABI table controlled",
        immediate: "u8 secondary id",
        description: "Dispatch through the UserC0 native handler table.",
        confidence: "high",
        evidence: "target dispatch table",
    },
    // 0xD0 legacy_3d_dispatch: Dispatch a legacy 3D extension opcode.
    BpOpcodeSpec {
        code: 0xD0,
        symbol: "legacy_3d",
        stack_effect: "table controlled",
        immediate: "u8 secondary id",
        description: "Dispatch a legacy 3D extension opcode.",
        confidence: "medium",
        evidence: "current VM ABI table",
    },
    // 0xE0 debug_inspect: Debug or inspection extension.
    BpOpcodeSpec {
        code: 0xE0,
        symbol: "debug_inspect",
        stack_effect: "implementation defined",
        immediate: "none",
        description: "Debug or inspection extension.",
        confidence: "low",
        evidence: "registered handler only",
    },
    // 0xFF script_extension: Dispatch a script extension call.
    BpOpcodeSpec {
        code: 0xFF,
        symbol: "script_extension",
        stack_effect: "extension controlled",
        immediate: "extension selector",
        description: "Dispatch a script extension call.",
        confidence: "low",
        evidence: "registered handler only",
    },
];

pub fn lookup_bp_opcode(code: u8) -> Option<&'static BpOpcodeSpec> {
    BP_OPCODE_SPECS.iter().find(|spec| spec.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_no_duplicate_opcode_values() {
        let mut codes = BP_OPCODE_SPECS
            .iter()
            .map(|spec| spec.code)
            .collect::<Vec<_>>();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), BP_OPCODE_SPECS.len());
    }

    #[test]
    fn target_boolean_and_memory_compare_names_are_explicit() {
        assert_eq!(lookup_bp_opcode(0x38).unwrap().symbol, "boolean_and");
        assert_eq!(lookup_bp_opcode(0x39).unwrap().symbol, "boolean_or");
        assert_eq!(lookup_bp_opcode(0x63).unwrap().symbol, "memory_equal");
    }
}
