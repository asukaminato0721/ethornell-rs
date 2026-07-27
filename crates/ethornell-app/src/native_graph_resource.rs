use super::*;
use crate::native_graph::NativeBitmapOperation;
use ethornell_vm::GraphApi;

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_graph_resource(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        if group != 0x92 {
            return None;
        }

        let value = match id {
            // sub_4854A0 -> sub_409AF0 installs one of eight compact sine
            // displacement tables.
            0x00 => {
                let args = pop_args(stack, 5);
                let configured = self.effects.configure_compact_wave_table(&args);
                tracing::debug!(?args, configured, "GraphConfigureCompactWaveTable");
                ethornell_vm::Value::None
            }
            // sub_404E70 initializes every format-6 vector-map sample.
            0x11 => {
                let mode = pop_int_value(stack).unwrap_or_default();
                let bitmap = pop_int_value(stack).unwrap_or_default();
                let generated = if (0..4).contains(&mode) {
                    self.effects.generate_ripple_map(bitmap, mode & 1, 0, 0, 1)
                } else {
                    false
                };
                tracing::debug!(bitmap, mode, generated, "GraphInitializeVectorMap");
                ethornell_vm::Value::None
            }
            // sub_4024A0 replaces matching RGB/RGBA pixels in the selected
            // native bitmap and returns its native status family.
            0x13 => {
                let replacement = pop_int_value(stack).unwrap_or_default();
                let needle = pop_int_value(stack).unwrap_or_default();
                let bitmap = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(self.replace_native_bitmap_color(
                    bitmap,
                    needle as u32,
                    replacement as u32,
                ))
            }
            0x1A => {
                let args = pop_args(stack, 6);
                self.apply_native_bitmap_operation(&args, NativeBitmapOperation::Composite);
                ethornell_vm::Value::None
            }
            // sub_403600/sub_403680 are immediate text rasterization paths
            // that return the updated cursor/output coordinate.
            0x1C | 0x1D => {
                let count = if id == 0x1C { 10 } else { 11 };
                let args = pop_args(stack, count);
                self.render_native_text_args(&args);
                let cursor = args
                    .iter()
                    .map(value_to_i32)
                    .find(|value| *value > 0)
                    .unwrap_or_default();
                ethornell_vm::Value::Int(cursor)
            }
            // sub_4020D0 loads an in-memory BMP payload into a bitmap slot.
            0x1F => {
                let file = pop_string_value(stack).unwrap_or_default();
                let bitmap = pop_int_value(stack).unwrap_or_default();
                let loaded = self.load_graph_image_resource(bitmap, "", &file);
                ethornell_vm::Value::Int(if loaded { 0 } else { -1 })
            }
            0x8A => {
                let args = pop_args(stack, 2);
                self.apply_graph_layer_property(&args);
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
                let font_name = pop_string_value(stack).unwrap_or_default();
                let value = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults.text_style.font_name =
                    (!font_name.is_empty()).then_some(font_name);
                self.graph_default_priority = value;
                ethornell_vm::Value::None
            }
            // sub_437FA0 validates and installs native font face, dimensions,
            // weight, and italic state; wrappers map its HRESULT family.
            0x9D => {
                let args = pop_args(stack, 5);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                let font_name = args.iter().find_map(value_to_optional_string);
                if let Some(name) = font_name.filter(|name| !name.is_empty()) {
                    self.graph_defaults.text_style.font_name = Some(name);
                }
                if let Some(size) = values
                    .iter()
                    .copied()
                    .find(|value| (1..=200).contains(value))
                {
                    self.text_state.font_size = size as f32;
                }
                ethornell_vm::Value::Int(0)
            }
            // The VM owns and clears the destination record buffer.
            0x9E => {
                let _destination = stack.pop();
                ethornell_vm::Value::Int(0)
            }
            0x9F => {
                self.graph_default_priority = pop_int_value(stack).unwrap_or_default();
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

    fn replace_native_bitmap_color(&mut self, bitmap: i32, needle: u32, replacement: u32) -> i32 {
        let Some(mut image) = self.graph_bitmap_image(bitmap) else {
            return 1;
        };
        let needle_rgb = [
            (needle & 0xff) as u8,
            ((needle >> 8) & 0xff) as u8,
            ((needle >> 16) & 0xff) as u8,
        ];
        let replacement_rgb = [
            (replacement & 0xff) as u8,
            ((replacement >> 8) & 0xff) as u8,
            ((replacement >> 16) & 0xff) as u8,
        ];
        for pixel in image.rgba.chunks_exact_mut(4) {
            if pixel[..3] == needle_rgb {
                pixel[..3].copy_from_slice(&replacement_rgb);
                if needle & 0xff00_0000 != 0 {
                    pixel[3] = (replacement >> 24) as u8;
                }
            }
        }
        let key = format!("runtime:color-replace:{bitmap}");
        self.store_graph_image(key.clone(), image);
        self.graph_resources
            .insert(bitmap, RuntimeGraphResource::whole(key));
        0
    }
}
