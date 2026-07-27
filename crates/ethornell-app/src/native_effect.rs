use super::*;

#[derive(Debug, Default)]
pub(super) struct NativeEffectState {
    pub(super) polygons: BTreeMap<i32, NativePolygon>,
}

#[derive(Debug, Clone)]
pub(super) struct NativePolygon {
    pub(super) mode: i32,
    pub(super) points: Vec<[i32; 3]>,
}

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_effect(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        if group != 0xC0 {
            return None;
        }

        let value = match id {
            0x06 => {
                let args = pop_args(stack, 4);
                self.apply_graph_object_property(&args);
                ethornell_vm::Value::None
            }
            0x08 => {
                let handle = pop_int_value(stack).unwrap_or_default();
                self.graph_process_handles.remove(&handle);
                self.remove_graph_object(handle);
                ethornell_vm::Value::None
            }
            // sub_490DC0 installs the twelve-field native effect descriptor.
            0x10 => {
                let args = pop_args(stack, 12);
                self.apply_graph_object_effect(&args);
                self.schedule_native_graph_procedure(&args);
                ethornell_vm::Value::None
            }
            0x1A => {
                let args = pop_args(stack, 8);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            0x1B => {
                let args = pop_args(stack, 3);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            0x20 => {
                let args = pop_args(stack, 4);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            0x24 => {
                let args = pop_args(stack, 2);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            0x25 => {
                let args = pop_args(stack, 11);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            0x2C => {
                let args = pop_args(stack, 2);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            // sub_492150 and sub_492170..280 own the native rain/particle
            // object. RuntimeEffects stores every recovered field.
            0x40 | 0x42..=0x4E => {
                let count = match id {
                    0x40 | 0x42 | 0x43 => 2,
                    0x44 => 6,
                    0x45 => 7,
                    0x4C | 0x4D => 4,
                    _ => 2,
                };
                let args = pop_args(stack, count);
                match self.effects.call(group, id, &args) {
                    crate::effects::EffectCall::Handled(result) => result
                        .map(ethornell_vm::Value::Int)
                        .unwrap_or(ethornell_vm::Value::None),
                    crate::effects::EffectCall::Unhandled => ethornell_vm::Value::None,
                }
            }
            // sub_492280 validates 1..1000 and derives a millisecond step.
            0x4F => {
                let interval = pop_int_value(stack).unwrap_or_default();
                let _diagnostic = pop_int_value(stack).unwrap_or_default();
                if (1..=1_000).contains(&interval) {
                    self.animation_queue_remaining = self
                        .animation_queue_remaining
                        .max((1_000_u32 / interval as u32).max(1));
                }
                ethornell_vm::Value::None
            }
            // Native polygon/list manager.
            0xC0 => {
                let handle = self.alloc_object();
                self.native_effect.polygons.insert(
                    handle,
                    NativePolygon {
                        mode: 0,
                        points: Vec::new(),
                    },
                );
                ethornell_vm::Value::Int(handle)
            }
            0xC1 => {
                let handle = pop_int_value(stack).unwrap_or_default();
                let status = if self.native_effect.polygons.remove(&handle).is_some() {
                    0
                } else {
                    1
                };
                ethornell_vm::Value::Int(status)
            }
            // VM-owned pointer calls.
            0xC2 => {
                let _args = pop_args(stack, 4);
                ethornell_vm::Value::Int(1)
            }
            0xC3 => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::Int(4)
            }
            // sub_4066C0 decodes a BWEF resource table. Invalid/unsupported
            // payloads use the native bad-resource result.
            0xF0 => {
                let _args = pop_args(stack, 5);
                ethornell_vm::Value::Int(-2_147_483_647)
            }
            _ => return None,
        };
        Some(Ok(value))
    }
}
