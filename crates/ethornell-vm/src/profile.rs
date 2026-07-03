use crate::{Value, Vm, VmResult};

impl Vm {
    pub(crate) fn sys_read_profile_string(&mut self) -> VmResult<i32> {
        let max_len = self.pop_int()?.max(0) as usize;
        let default = self.pop_value()?;
        let section = self.pop_value()?;
        let _mode = self.pop_value()?;
        let dst = self.pop_ptr()?;

        let key = self.profile_key_string(section.clone());
        let default_text = self.profile_default_string(default.clone());
        let Some(text) = resolve_profile_default(&key, &default_text) else {
            tracing::info!(
                dst = format_args!("0x{dst:08X}"),
                max_len,
                key,
                default = default_text,
                "ReadProfileString missing"
            );
            return Ok(1);
        };
        let clipped = clip_sjis_c_string(&text, max_len);
        self.write_c_string(dst, &clipped)?;
        tracing::info!(
            dst = format_args!("0x{dst:08X}"),
            max_len,
            key,
            default = default_text,
            result = clipped,
            "ReadProfileString"
        );
        Ok(0)
    }
}

impl Vm {
    fn profile_key_string(&self, value: Value) -> String {
        self.value_as_string_lossy(value.clone())
            .ok()
            .filter(|text| !text.starts_with("0x"))
            .unwrap_or_else(|| value_label(&value))
    }

    fn profile_default_string(&self, value: Value) -> String {
        match value {
            Value::Int(0) | Value::Ptr(0) | Value::None => String::new(),
            Value::Int(_) | Value::Ptr(_) | Value::Str(_) => self
                .value_as_string_lossy(value.clone())
                .ok()
                .filter(|text| !text.starts_with("0x"))
                .unwrap_or_else(|| value_label(&value)),
            Value::Func { .. } | Value::Program(_) => value_label(&value),
        }
    }
}

fn resolve_profile_default(key: &str, default_text: &str) -> Option<String> {
    match key {
        _ if default_text.is_empty() => None,
        _ => Some(default_text.to_string()),
    }
}

fn clip_sjis_c_string(text: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        let encoded = ch.encode_utf8(&mut buf);
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode(encoded);
        if used + sjis.len() >= max_len {
            break;
        }
        used += sjis.len();
        out.push(ch);
    }
    out
}

fn value_label(value: &Value) -> String {
    match value {
        Value::Int(value) => format!("0x{value:08X}"),
        Value::Ptr(value) => format!("0x{value:08X}"),
        Value::Func { offset, .. } => format!("0x{offset:08X}"),
        Value::Str(text) => text.clone(),
        Value::Program(program) => program
            .script_name
            .as_deref()
            .unwrap_or("<program>")
            .to_string(),
        Value::None => String::new(),
    }
}
