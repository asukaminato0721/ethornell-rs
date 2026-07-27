use super::*;
use crate::native_graph::NativeBitmapOperation;
use ethornell_vm::GraphApi;

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_graph_ext(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        if group != 0x91 {
            return None;
        }

        let value = match id {
            // The VM owns the source pointer and performs the cache copy.
            0x03 => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(-1)
            }
            // sub_480700 stores the active graphics context selector.
            0x0B => {
                self.graph_driver_mode = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            // sub_480720 -> sub_42DC40 accepts exactly modes zero and one.
            0x0C => {
                let mode = pop_int_value(stack).unwrap_or_default();
                if (0..=1).contains(&mode) {
                    self.system_extension_state = mode;
                    ethornell_vm::Value::Int(1)
                } else {
                    ethornell_vm::Value::Int(0)
                }
            }
            // sub_480860 creates/configures the native font rasterizer.
            0x0F => {
                let args = pop_args(stack, 6);
                self.configure_native_font(&args);
                ethornell_vm::Value::None
            }
            // sub_480D00/sub_481020 generate format-4 displacement maps.
            0x14 | 0x17 => {
                let count = if id == 0x14 { 5 } else { 6 };
                let args = pop_args(stack, count);
                self.apply_native_bitmap_operation(&args, NativeBitmapOperation::Copy);
                ethornell_vm::Value::None
            }
            // sub_481100 is the ten-parameter bitmap rectangle compositor plus
            // its diagnostic argument.
            0x18 => {
                let args = pop_args(stack, 11);
                self.apply_native_bitmap_operation(&args, NativeBitmapOperation::Composite);
                ethornell_vm::Value::None
            }
            // sub_481400 applies one packed RGB value to a bitmap surface.
            0x1A => {
                let args = pop_args(stack, 3);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            // These calls mutate the active CDspObj/resource state.
            0x31 => {
                let args = pop_args(stack, 2);
                self.apply_graph_object_property(&args);
                ethornell_vm::Value::None
            }
            0x3D => {
                let args = pop_args(stack, 2);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            0x41 => {
                let target = pop_int_value(stack).unwrap_or_default();
                self.set_graph_object_enabled(target, false);
                ethornell_vm::Value::None
            }
            0x42..=0x47 => {
                let count = match id {
                    0x42 | 0x44 | 0x45 => 2,
                    0x43 => 3,
                    0x46 => 4,
                    0x47 => 5,
                    _ => unreachable!(),
                };
                let args = pop_args(stack, count);
                if matches!(id, 0x46 | 0x47) {
                    self.apply_graph_object_effect(&args);
                } else {
                    self.apply_graph_object_property(&args);
                }
                ethornell_vm::Value::None
            }
            // sub_43F560/sub_43F640/sub_43F6D0 configure and submit the
            // currently selected render primitive.
            0x67 | 0x68 | 0x69 => {
                let count = match id {
                    0x67 => 6,
                    0x68 => 9,
                    _ => 3,
                };
                let args = pop_args(stack, count);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            // The native manager exposes four temporary primitive builders.
            0x70 => {
                let args = pop_args(stack, 6);
                let handle = self.alloc_object();
                self.graph_process_handles.insert(handle);
                self.apply_graph_object_property(&args);
                ethornell_vm::Value::Int(handle)
            }
            0x71 => {
                let handle = pop_int_value(stack).unwrap_or_default();
                let existed = self.graph_process_handles.remove(&handle);
                self.remove_graph_object(handle);
                if !existed {
                    tracing::debug!(handle, "native primitive release missed");
                }
                ethornell_vm::Value::None
            }
            0x73 => {
                let args = pop_args(stack, 3);
                let handle = args.last().map(value_to_i32).unwrap_or_default();
                ethornell_vm::Value::Int(i32::from(
                    self.graph_process_handles.contains(&handle)
                        || self.graph_handle_exists(handle),
                ))
            }
            0x74 => {
                let args = pop_args(stack, 2);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            0x75 | 0x76 => {
                let args = pop_args(stack, 6);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            0x78 => {
                let args = pop_args(stack, 7);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            0x79 | 0x7A => {
                let args = pop_args(stack, 4);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            0x7B => {
                let args = pop_args(stack, 6);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            0x7C => {
                let args = pop_args(stack, 3);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            0x7D => {
                let args = pop_args(stack, 4);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            0x7E | 0x7F => {
                let args = pop_args(stack, 4);
                let handle = args.last().map(value_to_i32).unwrap_or_default();
                if self.graph_process_handles.contains(&handle) || self.graph_handle_exists(handle)
                {
                    self.apply_graph_object_effect(&args);
                    ethornell_vm::Value::Int(1)
                } else {
                    ethornell_vm::Value::Int(0)
                }
            }
            0x8A => {
                let args = pop_args(stack, 2);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            // 0x90/0x92 construct CProcDspMsg-style cooperative procedures.
            0x90 | 0x92 => {
                let count = if id == 0x90 { 10 } else { 11 };
                let args = pop_args(stack, count);
                self.render_native_text_args(&args);
                self.schedule_native_graph_procedure(&args);
                ethornell_vm::Value::None
            }
            // 0x91/0x93 append text immediately using the selected defaults.
            0x91 | 0x93 => {
                let count = if id == 0x91 { 5 } else { 6 };
                let args = pop_args(stack, count);
                self.render_native_text_args(&args);
                ethornell_vm::Value::None
            }
            // sub_4847A0 -> sub_434440 installs the six persistent text
            // style/default fields (the wrapper supplies two native defaults).
            0x97 => {
                let args = pop_args(stack, 5);
                let mut values = [0_i32; 6];
                for (slot, value) in values.iter_mut().zip(args.iter().rev()) {
                    *slot = value_to_i32(value);
                }
                self.graph_defaults.configure_text_style(None, values);
                ethornell_vm::Value::None
            }
            0x99 => {
                let divisor = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(i32::from(divisor != 0))
            }
            // sub_484C40 is the extended immediate text path and returns its
            // updated cursor/output coordinate.
            0x9D => {
                let args = pop_args(stack, 15);
                self.render_native_text_args(&args);
                let cursor = args
                    .iter()
                    .map(value_to_i32)
                    .find(|value| *value > 0)
                    .unwrap_or_default();
                ethornell_vm::Value::Int(cursor)
            }
            // sub_463340 extracts <l>...</l> labels. The VM has already
            // normalized both native string pointers.
            0x9E => {
                let source = pop_string_value(stack).unwrap_or_default();
                let _destination = pop_string_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(count_native_labels(&source))
            }
            0xBB => {
                let args = pop_args(stack, 4);
                let object = args.last().map(value_to_i32).unwrap_or_default();
                ethornell_vm::Value::Int(self.poll_object_event(object))
            }
            // sub_447BB0 copies a validated 24-DWORD renderer preset.
            0xBF => {
                let _preset = pop_string_value(stack).unwrap_or_default();
                let slot = pop_int_value(stack).unwrap_or_default();
                self.graph_driver_mode = slot;
                ethornell_vm::Value::None
            }
            // sub_485040 creates a cooperative graph-effect process. This
            // family reports status values; none of its arguments is a frame
            // duration.
            0xF0 => {
                let args = pop_args(stack, 4);
                let process = args.last().map(value_to_i32).unwrap_or_default();
                ethornell_vm::Value::Int(self.graph_effects.configure_placeholder(process))
            }
            // sub_485100 pops the process key first. The converted second pop
            // is the duration output pointer and is owned by the VM.
            0xF1 => {
                let args = pop_args(stack, 2);
                let process = args.first().map(value_to_i32).unwrap_or_default();
                ethornell_vm::Value::Int(self.graph_effects.invoke(process).status)
            }
            // sub_485170 discards its script argument and cancels the active
            // process through the global native graph-effect manager.
            0xF2 => {
                let _reserved = pop_int_value(stack);
                ethornell_vm::Value::Int(self.graph_effects.cancel_all())
            }
            // sub_4851D0 queries whether a process is still cooperative.
            0xF3 => {
                let args = pop_args(stack, 2);
                let process = args.get(1).map(value_to_i32).unwrap_or_default();
                ethornell_vm::Value::Int(self.graph_effects.state(process))
            }
            // sub_485240 creates a rectangular cooperative effect process.
            0xF4 => {
                let args = pop_args(stack, 5);
                let process = args.last().map(value_to_i32).unwrap_or_default();
                ethornell_vm::Value::Int(self.graph_effects.configure_placeholder(process))
            }
            // sub_485300 preserves the native 0/1/2/4 poll result family.
            0xF5 => {
                let process = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(self.graph_effects.state(process))
            }
            // sub_485380 releases the underlying cooperative operation.
            0xF6 => {
                let process = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(self.graph_effects.release(process))
            }
            // sub_4853F0 writes a process result through its second argument;
            // the VM owns that pointer write.
            0xF7 => {
                let args = pop_args(stack, 2);
                let process = args.first().map(value_to_i32).unwrap_or_default();
                ethornell_vm::Value::Int(i32::from(self.graph_effects.result(process).is_some()))
            }
            _ => return None,
        };
        Some(Ok(value))
    }

    fn configure_native_font(&mut self, args: &[ethornell_vm::Value]) {
        let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
        if let Some(size) = values
            .iter()
            .copied()
            .find(|value| (1..=200).contains(value))
        {
            self.text_state.font_size = size as f32;
        }
        if values.len() >= 6 {
            let mut style = [0_i32; 6];
            style.copy_from_slice(&values[..6]);
            self.graph_defaults.configure_text_style(None, style);
        }
    }
}

fn count_native_labels(source: &str) -> i32 {
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
