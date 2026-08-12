use super::*;

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_graph_ext(
        &mut self,
        call: &mut ethornell_vm::NativeCallFrame,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        let (group, id) = (call.group(), call.id());
        if group != 0x91 {
            return None;
        }
        if id <= 0x1F {
            return self.dispatch_system91_03_1f(call);
        }
        if matches!(
            id,
            0x31 | 0x33 | 0x36 | 0x37 | 0x38 | 0x3d | 0x3e | 0x3f | 0x40..=0x4a
        ) {
            return self.dispatch_system91_31_4a(call);
        }
        if matches!(
            id,
            0x55 | 0x60 | 0x61 | 0x64..=0x69 | 0x70 | 0x71 | 0x73..=0x76 | 0x78..=0x7f
        ) {
            return self.dispatch_system91_55_7f(call);
        }
        if matches!(id, 0x88..=0x8e | 0x90..=0x9f) {
            return self.dispatch_system91_88_9f(call);
        }
        if matches!(id, 0xb8 | 0xba | 0xbb | 0xbf | 0xdb | 0xf0..=0xf7) {
            return self.dispatch_system91_b8_f7(call);
        }
        None
    }
}

pub(crate) fn count_native_labels(source: &str) -> i32 {
    let lower = source.to_ascii_lowercase();
    let mut tail = lower.as_str();
    let mut count = 0_i32;
    while let Some(start) = tail.find("<l>") {
        let content = &tail[start + 3..];
        let Some(end) = content.find("</l>") else {
            break;
        };
        if end != 0 {
            count = count.saturating_add(1);
        }
        tail = &content[end + 4..];
    }
    count
}

#[cfg(test)]
mod tests {
    use super::count_native_labels;

    #[test]
    fn native_label_extraction_is_case_insensitive() {
        assert_eq!(count_native_labels("a<L>one</L>b<l>two</l>"), 2);
        assert_eq!(count_native_labels("<l></l>"), 0);
    }
}
