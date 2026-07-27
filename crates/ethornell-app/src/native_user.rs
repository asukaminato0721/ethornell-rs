use super::*;

#[derive(Debug, Default)]
pub(super) struct NativeUserState {
    pub(super) text: String,
    title: String,
    visible: bool,
    ime_visible: bool,
    font_scale: i32,
    color: i32,
    selection_start: i32,
    selection_end: i32,
    next_resource_id: i32,
    resource_ids: BTreeMap<String, i32>,
}

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_user(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        if group != 0xB0 {
            return None;
        }

        let value = match id {
            0x00 => {
                let args = pop_args(stack, 3);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            0x04 => {
                let args = pop_args(stack, 3);
                self.pending_input_descriptor =
                    args.get(1).map(value_to_i32).filter(|value| *value != 0);
                ethornell_vm::Value::None
            }
            // sub_478200 constructs the cooperative host drawing procedure.
            0x08 => {
                let args = pop_args(stack, 7);
                let handle = self.alloc_object();
                self.graph_process_handles.insert(handle);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::Int(handle)
            }
            // The native helper owns an auxiliary drawing window. Portable
            // frontends represent it as an ordinary shared RuntimeSurface.
            0x10 => {
                let title = pop_string_value(stack).unwrap_or_default();
                let height = pop_int_value(stack).unwrap_or_default();
                let width = pop_int_value(stack).unwrap_or_default();
                let y = pop_int_value(stack).unwrap_or_default();
                let x = pop_int_value(stack).unwrap_or_default();
                let handle = self.alloc_object();
                let mut surface =
                    RuntimeSurface::display(handle, width.max(1) as f32, height.max(1) as f32);
                surface.x = x as f32;
                surface.y = y as f32;
                self.graph_surfaces.insert(handle, surface);
                self.native_user.title = title;
                self.native_user.visible = true;
                ethornell_vm::Value::Int(handle)
            }
            0x11 => {
                let handle = pop_int_value(stack).unwrap_or_default();
                self.graph_surfaces.remove(&handle);
                ethornell_vm::Value::None
            }
            0x14 => {
                let visible = pop_int_value(stack).unwrap_or_default() != 0;
                let handle = pop_int_value(stack).unwrap_or_default();
                if let Some(surface) = self.graph_surfaces.get_mut(&handle) {
                    surface.enabled = visible;
                }
                self.native_user.visible = visible;
                ethornell_vm::Value::None
            }
            0x15 => {
                self.native_user.title = pop_string_value(stack).unwrap_or_default();
                let _handle = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            0x16 => {
                let y = pop_int_value(stack).unwrap_or_default();
                let x = pop_int_value(stack).unwrap_or_default();
                let handle = pop_int_value(stack).unwrap_or_default();
                if let Some(surface) = self.graph_surfaces.get_mut(&handle) {
                    surface.x = x as f32;
                    surface.y = y as f32;
                }
                ethornell_vm::Value::None
            }
            0x17 => {
                let handle = pop_int_value(stack).unwrap_or_default();
                let (x, y) = self
                    .graph_surfaces
                    .get(&handle)
                    .map(|surface| (surface.x as i32, surface.y as i32))
                    .or_else(|| {
                        self.mouse_pos
                            .map(|(x, y)| (x.round() as i32, y.round() as i32))
                    })
                    .unwrap_or_default();
                stack.push(ethornell_vm::Value::Int(x));
                ethornell_vm::Value::Int(y)
            }
            0x18 => {
                let args = pop_args(stack, 2);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            0x19 => {
                let args = pop_args(stack, 6);
                self.apply_native_bitmap_operation(
                    &args,
                    crate::native_graph::NativeBitmapOperation::Composite,
                );
                ethornell_vm::Value::None
            }
            0x1A => {
                let args = pop_args(stack, 10);
                self.render_native_text_args(&args);
                ethornell_vm::Value::Int(
                    args.iter()
                        .map(value_to_i32)
                        .find(|value| *value > 0)
                        .unwrap_or_default(),
                )
            }
            0x1C => {
                self.native_user.text = pop_string_value(stack).unwrap_or_default();
                let _handle = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            // In-process IME/edit control state. No host window is created.
            0x20 => {
                let args = pop_args(stack, 8);
                self.native_user.ime_visible = true;
                if let Some(text) = args.iter().find_map(value_to_optional_string) {
                    self.native_user.text = text;
                }
                ethornell_vm::Value::None
            }
            0x21 => {
                self.native_user.ime_visible = false;
                ethornell_vm::Value::Int(0)
            }
            0x22 => {
                let scale = pop_int_value(stack).unwrap_or_default();
                let valid = (25..=200).contains(&scale);
                if valid {
                    self.native_user.font_scale = scale;
                }
                ethornell_vm::Value::Int(i32::from(valid))
            }
            0x23 => ethornell_vm::Value::Int(i32::from(self.native_user.ime_visible)),
            0x24 => {
                self.native_user.ime_visible = pop_int_value(stack).unwrap_or_default() != 0;
                ethornell_vm::Value::None
            }
            0x25 => {
                self.native_user.color = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            0x26 => {
                self.native_user.text = pop_string_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            // The VM writes the current text into the caller buffer.
            0x27 => {
                let _destination = stack.pop();
                ethornell_vm::Value::Int(self.native_user.text.len() as i32)
            }
            0x28 => {
                self.native_user.selection_start = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            0x29 => {
                self.native_user.selection_end = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            // Message boxes are deliberately non-modal. Informational/OK
            // calls accept; question calls choose the wrapper's false branch.
            0x81 => {
                let _message = pop_string_value(stack).unwrap_or_default();
                let _topmost = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(0)
            }
            0x82 => {
                let message = pop_string_value(stack).unwrap_or_default();
                let kind = pop_int_value(stack).unwrap_or_default();
                let _topmost = pop_int_value(stack).unwrap_or_default();
                tracing::info!(message, kind, "suppressed native message box");
                ethornell_vm::Value::Int(i32::from(kind == 1))
            }
            0x83 => {
                self.native_user.title = pop_string_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            // Native common dialogs return zero/cancel without opening a host
            // window in both normal and GUI-headless frontends.
            0x84 | 0x85 | 0x86 | 0x87 | 0x8C | 0x8F => {
                let count = match id {
                    0x84 | 0x8C => 4,
                    0x86 => 5,
                    0x8F => 6,
                    0x85 => 9,
                    0x87 => 12,
                    _ => unreachable!(),
                };
                let _args = pop_args(stack, count);
                ethornell_vm::Value::Int(0)
            }
            // Portable printing reports unavailable without invoking a system
            // print dialog.
            0xA0 => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::Int(0)
            }
            0xA1 => {
                let _arg = stack.pop();
                ethornell_vm::Value::Int(0)
            }
            0xA2 | 0xA3 => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            // Native string/resource registry.
            0xC0 => {
                let name = pop_string_value(stack).unwrap_or_default();
                let id = if let Some(id) = self.native_user.resource_ids.get(&name) {
                    *id
                } else {
                    let id = self.native_user.next_resource_id;
                    self.native_user.next_resource_id =
                        self.native_user.next_resource_id.saturating_add(1);
                    self.native_user.resource_ids.insert(name, id);
                    id
                };
                ethornell_vm::Value::Int(id)
            }
            0xC2 => {
                let name = pop_string_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(i32::from(self.manager.find(&name).is_some()))
            }
            0xC3 => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            0xC6 => {
                let charset = pop_int_value(stack).unwrap_or_default();
                let _name = pop_string_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(i32::from((0..=255).contains(&charset)))
            }
            // Desktop wallpaper mutation is intentionally unavailable.
            0xF0 => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::None
            }
            _ => return None,
        };
        Some(Ok(value))
    }
}
