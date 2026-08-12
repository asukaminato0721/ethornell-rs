use super::*;
use ethornell_vm::GraphApi;

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_graph_resource(
        &mut self,
        call: &mut ethornell_vm::NativeCallFrame,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        let (group, id) = (call.group(), call.id());
        if group != 0x92 {
            return None;
        }
        if id <= 0x1f {
            return self.dispatch_system92_00_1f(call);
        }
        let stack = call.args_mut();

        let value = match id {
            0x8A => {
                // sub_440B50 resolves the text object and forwards the value
                // to sub_42B3C0 (native object substructure at +0x164).
                let value = pop_int_value(stack).unwrap_or_default();
                let object = pop_int_value(stack).unwrap_or_default();
                self.graph_object_properties
                    .entry(object)
                    .or_default()
                    .properties
                    .insert(0x8a, (value, 0));
                tracing::debug!(object, value, "GraphSetTextObjectAuxiliaryValue");
                ethornell_vm::Value::None
            }
            // sub_432E40 configures one of the fixed font/image atlas slots.
            0x98 => {
                let args = pop_args(stack, 6);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                if let [height, width, y, x, source, slot] = values.as_slice() {
                    if *source == -1 {
                        self.graphic_resource_keys.remove(&(*slot, 0));
                    } else if *width > 0 && *height > 0 {
                        if let Some(key) = self.resolve_resource_key(*source).map(str::to_string) {
                            self.graphic_resource_keys.insert((*slot, 0), key);
                        }
                        self.bitmap_dimensions
                            .insert(*slot, (*width as u32, *height as u32));
                    }
                    tracing::debug!(slot, source, x, y, width, height, "GraphConfigureFontAtlas");
                }
                ethornell_vm::Value::None
            }
            0x9B => {
                unreachable!("Graph92:9B is handled by the VM output-pair pointer bridge")
            }
            // sub_486A50 pops italic, registry/context, creation field,
            // height and face selector. sub_463350 resolves the face from the
            // final selector plus registry/context before sub_437FA0 performs
            // platform-native font validation.
            0x9D => {
                let args = pop_args(stack, 5);
                let italic = args.first().map(value_to_i32).unwrap_or_default();
                let registry_context = args.get(1).map(value_to_i32).unwrap_or_default();
                let creation_field = args.get(2).map(value_to_i32).unwrap_or_default();
                let height = args.get(3).map(value_to_i32).unwrap_or_default();
                let face_selector = args.get(4).cloned().unwrap_or(ethornell_vm::Value::None);
                match &face_selector {
                    ethornell_vm::Value::Str(name) => {
                        self.graph_defaults.text_font_override.face_name =
                            (!name.is_empty()).then(|| name.clone());
                    }
                    ethornell_vm::Value::Int(0)
                    | ethornell_vm::Value::Ptr(0)
                    | ethornell_vm::Value::None => {
                        self.graph_defaults.text_font_override.face_name = None;
                    }
                    _ => {}
                }
                if (1..=200).contains(&height) {
                    self.text_state.font_size = height as f32;
                }
                self.graph_defaults.text_font_override.height = height;
                self.graph_defaults.text_font_override.creation_field_0 = creation_field;
                self.graph_defaults.text_font_override.creation_field_1 = registry_context;
                self.graph_defaults.text_font_override.italic = italic != 0;
                tracing::debug!(
                    ?face_selector,
                    height,
                    creation_field,
                    registry_context,
                    italic,
                    "GraphConfigureFontOverride"
                );
                // Exact GDI face lookup and HRESULT-to-script status mapping
                // remain platform-specific, so this stays Partial.
                ethornell_vm::Value::Int(0)
            }
            // The VM owns the destination record buffer and exact 128-byte
            // serialization. Reaching host dispatch would double-pop.
            0x9E => {
                unreachable!("Graph92:9E is handled by the VM fragment-record pointer bridge")
            }
            0x9F => {
                self.system92_text_render_override = pop_int_value(stack).unwrap_or_default();
                tracing::debug!(
                    value = self.system92_text_render_override,
                    "GraphSetTextRenderOverride"
                );
                ethornell_vm::Value::None
            }
            0xF6 => {
                let bitmap = pop_int_value(stack).unwrap_or_default();
                let _mode = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(if self.query_bitmap_info(bitmap).is_some() {
                    0
                } else {
                    4
                })
            }
            _ => return None,
        };
        Some(Ok(value))
    }

    pub(super) fn replace_native_bitmap_color(
        &mut self,
        bitmap: i32,
        needle: u32,
        replacement: u32,
    ) -> i32 {
        let Some(format) = self.bitmap_formats.get(&bitmap).copied() else {
            return 1;
        };
        let Some(bytes_per_pixel) =
            super::graph_bitmap_nodes::target_bitmap_bytes_per_pixel(format)
        else {
            return 2;
        };
        if bytes_per_pixel != 4 {
            return 2;
        }
        if !matches!(format, 1 | 2) {
            return 0;
        }
        let Some(mut image) = self.graph_bitmap_image(bitmap) else {
            return 1;
        };
        let unpack = |packed: u32| {
            [
                ((packed >> 16) & 0xff) as u8,
                ((packed >> 8) & 0xff) as u8,
                (packed & 0xff) as u8,
                (packed >> 24) as u8,
            ]
        };
        let needle_rgba = unpack(needle);
        let replacement_rgba = unpack(replacement);
        for pixel in image.rgba.chunks_exact_mut(4) {
            if format == 1 {
                if pixel[..3] == needle_rgba[..3] {
                    pixel[..3].copy_from_slice(&replacement_rgba[..3]);
                    pixel[3] = 0xff;
                }
            } else if needle_rgba[3] == 0 {
                if pixel[..3] == needle_rgba[..3] {
                    pixel[..3].copy_from_slice(&replacement_rgba[..3]);
                }
            } else if pixel == needle_rgba {
                pixel.copy_from_slice(&replacement_rgba);
            }
        }
        let key = format!("runtime:color-replace:{bitmap}");
        self.store_graph_image(key.clone(), image);
        self.graph_resources
            .insert(bitmap, RuntimeGraphResource::whole(key));
        0
    }
}
