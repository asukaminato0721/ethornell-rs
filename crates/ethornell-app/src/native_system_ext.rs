use super::*;

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_system_ext(
        &mut self,
        call: &mut ethornell_vm::NativeCallFrame,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        let (group, id) = (call.group(), call.id());
        let stack = call.args_mut();
        let value = match (group, id) {
            // sub_498820 accepts a presentation interval in [50, 60000].
            (0x81, 0x04) => {
                let interval = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(i32::from((50..=60_000).contains(&interval)))
            }
            // VM-owned point/history output.
            (0x81, 0x07) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x08) | (0x81, 0x09) => {
                let _destination = stack.pop();
                ethornell_vm::Value::None
            }
            (0x81, 0x0A) => {
                let _destination = stack.pop();
                ethornell_vm::Value::Int(1)
            }
            (0x81, 0x0B) => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::None
            }
            (0x81, 0x0C) | (0x81, 0x0D) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::None
            }
            (0x81, 0x10) => {
                let descriptor = pop_int_value(stack).unwrap_or_default();
                let value = pop_int_value(stack).unwrap_or_default();
                let previous = i32::from(
                    !self.pending_input_consumed
                        && self.pending_input_descriptor == Some(descriptor)
                        && self.pending_input_state.unwrap_or_default() == value,
                );
                ethornell_vm::Value::Int(previous)
            }
            (0x81, 0x11) => {
                let _destination = stack.pop();
                ethornell_vm::Value::None
            }
            (0x81, 0x14) => {
                self.input_latched_state = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            (0x81, 0x16) => {
                let capacity = pop_int_value(stack).unwrap_or_default();
                let _interval = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(i32::from((0..=0x200).contains(&capacity)))
            }
            (0x81, 0x17) => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x1B) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x1D) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(i32::MIN + 1)
            }
            // sub_46E5D0 synthesizes a complete mouse press/release pair.
            (0x81, 0x1E) => {
                let button = pop_int_value(stack).unwrap_or_default();
                let descriptor = match button {
                    1 => Some(INPUT_DESCRIPTOR_MOUSE_LEFT),
                    _ => None,
                };
                if let Some(descriptor) = descriptor {
                    self.pending_input_descriptor = Some(descriptor);
                    self.pending_input_state = Some(0x6);
                    self.pending_input_consumed = false;
                }
                ethornell_vm::Value::Int(i32::from(descriptor.is_some()))
            }
            (0x81, 0x1F) => {
                // sub_48B7D0 -> sub_46E5A0 -> sub_4319F0 writes the exact
                // message/procedure auxiliary input mask dword_507690.
                let mask = pop_int_value(stack).unwrap_or_default();
                self.message_auxiliary_input_mask = mask;
                tracing::info!(
                    mask = format_args!("0x{mask:08X}"),
                    "SysSetMessageAuxiliaryInputMask"
                );
                ethornell_vm::Value::None
            }
            // 0x28..0x32 are native crypto/time/file helpers. VM owns every
            // output buffer and implements the portable codecs.
            (0x81, 0x28) => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x29) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x2A) => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x2B) => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x2C) => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x2D) => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x32) => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x38) => {
                let _args = pop_args(stack, 7);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x39) => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x3A) => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x3B) => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x3C) => {
                let _path = pop_string_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x3D) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x3E) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x44) => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x61) => ethornell_vm::Value::Int(self.system_mode_flag),
            (0x81, 0x65) => {
                let moved = pop_int_value(stack).unwrap_or_default();
                self.pending_window_minimize = moved != 0;
                ethornell_vm::Value::None
            }
            (0x81, 0x68) => {
                let value = pop_int_value(stack).unwrap_or_default();
                let previous = self.window_mode;
                self.window_mode = value;
                ethornell_vm::Value::Int(previous)
            }
            (0x81, 0x69) => {
                let enabled = pop_int_value(stack).unwrap_or_default() != 0;
                self.input_requires_focus = enabled;
                ethornell_vm::Value::None
            }
            (0x81, 0x6A) => {
                self.system_config_input_mode = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            (0x81, 0x6B) => {
                let _destination = stack.pop();
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0x6D) => ethornell_vm::Value::Int(0),
            (0x81, 0x6E) => ethornell_vm::Value::Int(0),
            (0x81, 0xB0) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(-1)
            }
            (0x81, 0xB7) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(-1)
            }
            // Touch/gesture APIs preserve their native "not available" result
            // on a backend without a touch packet source.
            (0x81, 0xD0) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0xD1) => {
                let _arg = stack.pop();
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0xD2) => {
                let _args = pop_args(stack, 5);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0xD3) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0xD4) => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0xD5) => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0xE0) => {
                let _args = pop_args(stack, 7);
                ethornell_vm::Value::Int(0)
            }
            (0x81, 0xE9) => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::None
            }
            (0x81, 0xEA) => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::None
            }
            (0x81, 0xEC) => {
                let _arg = stack.pop();
                ethornell_vm::Value::Int(self.system_config_input_mode)
            }
            (0x81, 0xED) => {
                self.system_config_input_mode = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(self.system_config_input_mode)
            }
            // Native 0xF2 installs an installer procedure. No popup is
            // permitted; preserve its cooperative scheduling boundary.
            (0x81, 0xF2) => {
                let _args = pop_args(stack, 13);
                self.frame_yield_requested = true;
                ethornell_vm::Value::None
            }
            _ => return None,
        };
        Some(Ok(value))
    }
}
