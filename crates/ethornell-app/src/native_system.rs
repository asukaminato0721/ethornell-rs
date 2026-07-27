use super::*;

#[derive(Default)]
pub(super) struct NativeSystemState {
    input_mapping_mode: i32,
    power_wait_mode: i32,
    pub(super) drag_drop_enabled: bool,
    cursor_button_mode: i32,
    event_base: i32,
    event_serial: i32,
    named_threads: BTreeMap<String, i32>,
    pub(super) save_headers: BTreeMap<i32, [u8; 64]>,
    uninstaller_product: String,
}

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_system(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        if group != 0x80 {
            return None;
        }

        let value = match id {
            // sub_487E90/sub_45FEF0 returns the native renderer's last
            // presentation status. A completed portable frame has status zero.
            0x09 => ethornell_vm::Value::Int(0),
            // VM-owned output: the 64-byte graphics capability record.
            0x0A => {
                let _destination = stack.pop();
                ethornell_vm::Value::None
            }
            // sub_461EB0 queries native graphics memory usage. The portable
            // renderer does not reserve an independently queryable D3D heap.
            0x0B => ethornell_vm::Value::Int(0),
            // sub_49A240 is the WM_ACTIVATE/minimize latch.
            0x0E => ethornell_vm::Value::Int(i32::from(self.pending_window_minimize)),
            0x10 => {
                self.native_system.input_mapping_mode = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            // VM-owned input and output-pointer calls.
            0x1D => {
                let _ = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            0x1E => {
                let mode = pop_int_value(stack).unwrap_or_default();
                let accepted = (0..=1).contains(&mode);
                if accepted {
                    self.native_system.cursor_button_mode = mode;
                }
                ethornell_vm::Value::Int(i32::from(accepted))
            }
            0x24 => {
                let recursive = pop_int_value(stack).unwrap_or_default() != 0;
                let pattern = pop_string_value(stack).unwrap_or_default();
                let count = self
                    .enumerate_native_paths(&pattern, recursive, 0, false)
                    .len()
                    .min(i32::MAX as usize) as i32;
                ethornell_vm::Value::Int(count)
            }
            // VM owns the destination buffer for 0x26.
            0x26 => {
                let _ = pop_args(stack, 4);
                ethornell_vm::Value::Int(0)
            }
            0x27 => {
                let from = pop_string_value(stack).unwrap_or_default();
                let to = pop_string_value(stack).unwrap_or_default();
                let ok = match (
                    runtime_file_path(&self.manager, &from),
                    runtime_file_path(&self.manager, &to),
                ) {
                    (Some(from), Some(to)) => {
                        if let Some(parent) = to.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        std::fs::rename(from, to).is_ok()
                    }
                    _ => false,
                };
                tracing::info!(from, to, ok, "MoveFile");
                ethornell_vm::Value::Int(i32::from(ok))
            }
            // Native sub_4667A0 is a retry/quit dialog around file existence.
            // Headless and GUI share the non-modal result and never spawn it.
            0x3C => {
                let _message = pop_string_value(stack).unwrap_or_default();
                let _caption = pop_string_value(stack).unwrap_or_default();
                let path = pop_string_value(stack).unwrap_or_default();
                let exists =
                    runtime_file_path(&self.manager, &path).is_some_and(|path| path.is_file());
                ethornell_vm::Value::Int(i32::from(exists))
            }
            0x3E => {
                let path = pop_string_value(stack).unwrap_or_default();
                let exists =
                    runtime_file_path(&self.manager, &path).is_some_and(|path| path.is_dir());
                ethornell_vm::Value::Int(i32::from(exists))
            }
            0x53 => {
                self.native_system.power_wait_mode = 0;
                ethornell_vm::Value::None
            }
            // CProcWaitWndMsg: one cooperative frame/message-pump boundary.
            0x54 => {
                let _message_mask = pop_int_value(stack).unwrap_or_default();
                self.frame_yield_requested = true;
                ethornell_vm::Value::None
            }
            0x5D => {
                self.native_system.power_wait_mode = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            0x63 => {
                let resize_enabled = pop_int_value(stack).unwrap_or_default() == 0;
                self.input_requires_focus = resize_enabled;
                ethornell_vm::Value::None
            }
            // sub_4650F0 selects a new bootstrap pair and returns interpreter
            // status 5. The VM intercepts actual program replacement.
            0x6B => {
                let file = pop_string_value(stack).unwrap_or_default();
                let archive = pop_string_value(stack).unwrap_or_default();
                tracing::info!(archive, file, "SelectBootstrapProgram");
                ethornell_vm::Value::None
            }
            0x6C => {
                self.native_system.drag_drop_enabled =
                    pop_int_value(stack).unwrap_or_default() != 0;
                ethornell_vm::Value::None
            }
            // VM owns the output string.
            0x6D => {
                let _destination = stack.pop();
                ethornell_vm::Value::Int(0)
            }
            0x6E => {
                self.native_system.cursor_button_mode = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            0x6F => {
                let aspect_mismatch = self.screen_width > 0
                    && self.screen_height > 0
                    && self.screen_width * 3 != self.screen_height * 4;
                ethornell_vm::Value::Int(i32::from(aspect_mismatch))
            }
            0x78 => {
                let label = pop_string_value(stack).unwrap_or_default();
                let slot = pop_int_value(stack).unwrap_or_default();
                let mut header = [0u8; 64];
                let encoded = encoding_rs::SHIFT_JIS.encode(&label).0;
                let length = encoded.len().min(header.len().saturating_sub(1));
                header[..length].copy_from_slice(&encoded[..length]);
                self.native_system.save_headers.insert(slot, header);
                ethornell_vm::Value::None
            }
            0x79 => {
                let slot = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(if self.native_system.save_headers.contains_key(&slot) {
                    0
                } else {
                    1
                })
            }
            0x7A => {
                let _destination = stack.pop();
                let slot = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(if self.native_system.save_headers.contains_key(&slot) {
                    0
                } else {
                    1
                })
            }
            0x7B => {
                let slot = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(if self.native_system.save_headers.contains_key(&slot) {
                    0
                } else {
                    1
                })
            }
            0x90 => {
                self.native_system.event_base = pop_int_value(stack).unwrap_or_default();
                self.native_system.event_serial = 0;
                self.queued_system_events.clear();
                ethornell_vm::Value::None
            }
            0x91 => ethornell_vm::Value::Int(
                self.queued_system_events.len().min(i32::MAX as usize) as i32,
            ),
            0x94 => {
                let _descriptor = pop_args(stack, 13);
                self.native_system.event_serial = self.native_system.event_serial.saturating_add(1);
                ethornell_vm::Value::None
            }
            0x95 | 0x97 => {
                let _descriptor = stack.pop();
                let event = pop_int_value(stack).unwrap_or_default();
                let exists = self
                    .queued_system_events
                    .iter()
                    .any(|queued| queued[1] == event);
                ethornell_vm::Value::Int(i32::from(exists))
            }
            0x96 => {
                let _descriptor = stack.pop();
                ethornell_vm::Value::None
            }
            0xA9 => {
                let _destination = stack.pop();
                let object = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(i32::from(self.graph_input_objects.contains_key(&object)))
            }
            0xB0 => {
                let flags = pop_int_value(stack).unwrap_or_default();
                let name = pop_string_value(stack).unwrap_or_default();
                let inserted = !self.native_system.named_threads.contains_key(&name);
                if inserted {
                    self.native_system.named_threads.insert(name, flags);
                }
                ethornell_vm::Value::Int(i32::from(inserted))
            }
            0xB1 => {
                let name = pop_string_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(i32::from(
                    self.native_system.named_threads.remove(&name).is_some(),
                ))
            }
            0xB4 => {
                let _flags = pop_int_value(stack).unwrap_or_default();
                let _name = pop_string_value(stack).unwrap_or_default();
                self.frame_yield_requested = true;
                ethornell_vm::Value::None
            }
            0xB5 => {
                let name = pop_string_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(
                    self.native_system
                        .named_threads
                        .get(&name)
                        .copied()
                        .unwrap_or(i32::MIN + 1),
                )
            }
            0xB6 => {
                let flags = pop_int_value(stack).unwrap_or_default();
                let name = pop_string_value(stack).unwrap_or_default();
                let result = if let Some(value) = self.native_system.named_threads.get_mut(&name) {
                    *value = flags;
                    0
                } else {
                    i32::MIN + 1
                };
                ethornell_vm::Value::Int(result)
            }
            0xCF => {
                let _args = pop_args(stack, 3);
                self.frame_yield_requested = true;
                ethornell_vm::Value::None
            }
            0xDE => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::Int(i32::MIN + 1)
            }
            // VM owns the destination vector for external file loading.
            0xE9 => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            0xEA => {
                self.native_system.uninstaller_product =
                    pop_string_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            // Native installer dialogs and registry integration have no
            // portable UI side effect. Their documented failure result keeps
            // scripts on the same error branch without opening a popup.
            0xF1 => {
                let _args = pop_args(stack, 6);
                ethornell_vm::Value::Int(0)
            }
            0xF3 => {
                let _args = pop_args(stack, 8);
                ethornell_vm::Value::Int(0)
            }
            0xF4 => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            0xF5 => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            0xF6 => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::None
            }
            0xF9 => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            // VM owns output buffers for 0xFA/0xFB/0xFC.
            0xFA => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            0xFB => {
                let _destination = stack.pop();
                ethornell_vm::Value::None
            }
            0xFC => {
                let _args = pop_args(stack, 5);
                ethornell_vm::Value::Int(0)
            }
            0xFE => ethornell_vm::Value::Int(1),
            _ => return None,
        };
        Some(Ok(value))
    }

    pub(super) fn enumerate_native_paths(
        &self,
        pattern: &str,
        recursive: bool,
        max_count: usize,
        directories: bool,
    ) -> Vec<String> {
        let normalized = pattern.replace('\\', std::path::MAIN_SEPARATOR_STR);
        let pattern_path = Path::new(&normalized);
        let wildcard = pattern_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("*");
        let parent = pattern_path.parent().unwrap_or_else(|| Path::new(""));
        let Some(root) = runtime_file_path(&self.manager, &parent.to_string_lossy()) else {
            return Vec::new();
        };
        let mut output = Vec::new();
        enumerate_native_directory(
            &root,
            &root,
            wildcard,
            recursive,
            max_count,
            directories,
            &mut output,
        );
        output
    }
}

fn enumerate_native_directory(
    root: &Path,
    directory: &Path,
    pattern: &str,
    recursive: bool,
    max_count: usize,
    directories: bool,
    output: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries.filter_map(|entry| entry.ok()).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        if max_count != 0 && output.len() >= max_count {
            return;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if directories && windows_wildcard_matches(pattern, &name) {
                let relative = entry.path();
                let relative = relative.strip_prefix(root).unwrap_or(&relative);
                output.push(
                    relative
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "\\"),
                );
            }
            if recursive && name != "." && name != ".." {
                enumerate_native_directory(
                    root,
                    &entry.path(),
                    pattern,
                    true,
                    max_count,
                    directories,
                    output,
                );
            }
        } else if !directories && windows_wildcard_matches(pattern, &name) {
            let relative = entry.path();
            let relative = relative.strip_prefix(root).unwrap_or(&relative);
            output.push(
                relative
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "\\"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn registered_group_80_handlers_are_explicitly_owned() {
        let source = include_str!("native_system.rs");
        for id in [
            0x09u16, 0x0A, 0x0B, 0x0E, 0x10, 0x1D, 0x1E, 0x24, 0x26, 0x27, 0x3C, 0x3E, 0x53, 0x54,
            0x5D, 0x63, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x78, 0x79, 0x7A, 0x7B, 0x90, 0x91, 0x94,
            0x95, 0x96, 0x97, 0xA9, 0xB0, 0xB1, 0xB4, 0xB5, 0xB6, 0xCF, 0xDE, 0xE9, 0xEA, 0xF1,
            0xF3, 0xF4, 0xF5, 0xF6, 0xF9, 0xFA, 0xFB, 0xFC, 0xFE,
        ] {
            assert!(
                source.contains(&format!("0x{id:02X}")),
                "missing explicit group 80 handler 0x{id:02X}"
            );
        }
    }
}
