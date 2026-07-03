use crate::{Value, Vm, VmResult};
use ethornell_script::bcs::{parse_bcs, BcsCommand, BcsValue};

const BCS_MAGIC: &[u8] = b"BurikoCompiledScriptVer1.00\0";
const BCS_RET_OPCODE: u32 = 0x1b;
const SCRIPT_CODE_BASE_SLOT: u32 = 0x0004_ca78;
const SCRIPT_CURRENT_OFFSET_SLOT: u32 = 0x0004_cc70;

#[derive(Debug, Clone)]
pub(crate) struct LoadedBcsRange {
    script_name: String,
    base: u32,
    code_start: u32,
    code_end: u32,
    total_len: u32,
}

impl Vm {
    pub(crate) fn scenario_loaded_file_preprocess(
        &mut self,
        script_name: &str,
        base: u32,
        bytes: &[u8],
    ) -> VmResult<()> {
        let Some(program) = parse_bcs(bytes) else {
            return Ok(());
        };

        self.remember_loaded_bcs_range(script_name, base, bytes.len(), &program);
        let direct_strings = self.shadow_bcs_string_refs(base, &program.commands);
        let (sound_strings, text_strings, visual_strings) =
            self.shadow_bcs_command_refs(base, &program.commands);
        tracing::info!(
            script_name,
            bytes = bytes.len(),
            base = format_args!("0x{base:08X}"),
            code_start = format_args!("0x{:X}", program.code_start),
            code_end = format_args!("0x{:X}", program.code_end),
            namespaces = program.namespaces.len(),
            subs = program.subs.len(),
            commands = program.commands.len(),
            direct_strings,
            sound_strings,
            text_strings,
            visual_strings,
            "ScenarioLoadedFilePreprocess"
        );
        Ok(())
    }

    pub(crate) fn scenario_code_preprocess(
        &mut self,
        script_name: &str,
        body_length: usize,
    ) -> VmResult<()> {
        let Some((base, bytes)) = self.find_loaded_bcs(body_length) else {
            tracing::warn!(
                script_name,
                body_length,
                "ScenarioCodePreprocess BCS not found"
            );
            return Ok(());
        };
        let Some(program) = parse_bcs(&bytes) else {
            tracing::warn!(
                script_name,
                body_length,
                base = format_args!("0x{base:08X}"),
                "ScenarioCodePreprocess parse failed"
            );
            return Ok(());
        };

        self.remember_loaded_bcs_range(script_name, base, bytes.len(), &program);
        let direct_strings = self.shadow_bcs_string_refs(base, &program.commands);
        let (sound_strings, text_strings, visual_strings) =
            self.shadow_bcs_command_refs(base, &program.commands);

        tracing::info!(
            script_name,
            body_length,
            base = format_args!("0x{base:08X}"),
            code_start = format_args!("0x{:X}", program.code_start),
            code_end = format_args!("0x{:X}", program.code_end),
            namespaces = program.namespaces.len(),
            subs = program.subs.len(),
            commands = program.commands.len(),
            direct_strings,
            sound_strings,
            text_strings,
            visual_strings,
            "ScenarioCodePreprocess"
        );
        Ok(())
    }

    pub(crate) fn scenario_guarded_read_int(&self, ptr: u32, width: u8) -> Option<u32> {
        if width < 2 {
            return None;
        }

        let code_base = self.read_raw_u32(SCRIPT_CODE_BASE_SLOT)?;
        let code_base_addr = Self::memory_addr(code_base);
        let current_offset = self.read_raw_u32(SCRIPT_CURRENT_OFFSET_SLOT)?;
        if Self::memory_addr(ptr) != Self::memory_addr(code_base.wrapping_add(current_offset)) {
            return None;
        }

        let range = self.loaded_bcs_ranges.iter().find(|range| {
            code_base_addr >= range.base
                && code_base_addr < range.base.saturating_add(range.total_len)
                && current_offset >= range.code_end
                && current_offset < range.total_len
        })?;

        if std::env::var_os("DEBUG").is_some() {
            tracing::warn!(
                script_name = range.script_name.as_str(),
                ptr = format_args!("0x{ptr:08X}"),
                code_base = format_args!("0x{code_base:08X}"),
                current_offset = format_args!("0x{current_offset:08X}"),
                code_start = format_args!("0x{:X}", range.code_start),
                code_end = format_args!("0x{:X}", range.code_end),
                "ScenarioPastCodeReadReturningRet"
            );
        }
        Some(BCS_RET_OPCODE)
    }

    fn remember_loaded_bcs_range(
        &mut self,
        script_name: &str,
        base: u32,
        total_len: usize,
        program: &ethornell_script::bcs::BcsProgram,
    ) {
        let base = Self::memory_addr(base);
        self.loaded_bcs_ranges
            .retain(|range| !(range.base == base && range.script_name == script_name));
        self.loaded_bcs_ranges.push(LoadedBcsRange {
            script_name: script_name.to_string(),
            base,
            code_start: program.code_start as u32,
            code_end: program.code_end as u32,
            total_len: total_len as u32,
        });
    }

    fn shadow_bcs_command_refs(
        &mut self,
        bcs_base: u32,
        commands: &[BcsCommand],
    ) -> (usize, usize, usize) {
        let mut sound_strings = 0usize;
        let mut text_strings = 0usize;
        let mut visual_strings = 0usize;
        for command in commands {
            match command.name {
                Some("sound") | Some("sound_1a0") | Some("snd") => {
                    sound_strings += self.shadow_command_strings(bcs_base, command, 6);
                }
                Some("say") | Some("msg") => {
                    text_strings += self.shadow_command_strings(bcs_base, command, 16);
                }
                Some("bg")
                | Some("bg240")
                | Some("bg_transition")
                | Some("sprite")
                | Some("grp") => {
                    visual_strings += self.shadow_command_strings(bcs_base, command, 16);
                }
                _ => {}
            }
        }
        (sound_strings, text_strings, visual_strings)
    }

    fn shadow_bcs_string_refs(&mut self, bcs_base: u32, commands: &[BcsCommand]) -> usize {
        let mut written = 0usize;
        for command in commands {
            for string_ref in &command.string_refs {
                if string_ref.text.is_empty() {
                    continue;
                }
                let addr = bcs_base.saturating_add(string_ref.offset);
                self.mem_values
                    .insert(Self::value_key(addr), Value::Str(string_ref.text.clone()));
                written += 1;
            }
        }
        written
    }

    fn find_loaded_bcs(&self, body_length: usize) -> Option<(u32, Vec<u8>)> {
        let mut search_from = 0usize;
        while search_from < self.memory.len() {
            let haystack = &self.memory[search_from..];
            let Some(relative) = find_bytes(haystack, BCS_MAGIC) else {
                break;
            };
            let start = search_from + relative;
            let header_offset = start.checked_add(BCS_MAGIC.len())?;
            let header_size = read_u32(&self.memory, header_offset)? as usize;
            let body_start = header_size.checked_add(0x1c)?;
            let total_len = body_start.checked_add(body_length)?;
            if start.checked_add(total_len)? <= self.memory.len() {
                let bytes = self.memory[start..start + total_len].to_vec();
                if parse_bcs(&bytes).is_some() {
                    return Some((start as u32, bytes));
                }
            }
            search_from = start.saturating_add(BCS_MAGIC.len());
        }
        None
    }

    fn shadow_command_strings(
        &mut self,
        bcs_base: u32,
        command: &BcsCommand,
        field_bias: u32,
    ) -> usize {
        let mut written = 0usize;
        let strings = command
            .string_refs
            .iter()
            .map(|string_ref| string_ref.text.as_str())
            .chain(command.args.iter().filter_map(value_text))
            .filter(|text| !text.is_empty() && !text.starts_with('\u{1b}'))
            .collect::<Vec<_>>();

        for (index, text) in strings.into_iter().enumerate() {
            let addr = bcs_base
                .saturating_add(command.file_offset as u32)
                .saturating_add(field_bias)
                .saturating_add((index as u32).saturating_mul(4));
            self.mem_values
                .insert(Self::value_key(addr), Value::Str(text.to_string()));
            written += 1;
        }
        for string_ref in &command.string_refs {
            if !string_ref.text.is_empty() {
                let addr = bcs_base.saturating_add(string_ref.offset);
                self.mem_values
                    .insert(Self::value_key(addr), Value::Str(string_ref.text.clone()));
                written += 1;
            }
        }
        written
    }

    fn read_raw_u32(&self, ptr: u32) -> Option<u32> {
        let start = Self::memory_addr(ptr) as usize;
        let bytes = self.memory.get(start..start.checked_add(4)?)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }
}

fn value_text(value: &BcsValue) -> Option<&str> {
    match value {
        BcsValue::Str(text) => Some(text.as_str()),
        _ => None,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn read_u32(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = buf.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}
