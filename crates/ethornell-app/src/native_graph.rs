use super::*;

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_graph(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        let value = match (group, id) {
            (0x90, 0x0A) => {
                let y = pop_int_value(stack).unwrap_or_default() as f32;
                let x = pop_int_value(stack).unwrap_or_default() as f32;
                self.graph_global_offset = (x, y);
                ethornell_vm::Value::None
            }
            (0x90, 0x0B) => {
                let object = pop_int_value(stack).unwrap_or_default();
                if object > 0 {
                    self.set_current_graph_object(object);
                }
                ethornell_vm::Value::None
            }
            (0x90, 0x0F) => {
                self.graph_driver_mode = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::None
            }
            (0x90, 0x1A) => {
                let args = pop_args(stack, 6);
                self.apply_native_bitmap_operation(&args, NativeBitmapOperation::Composite);
                ethornell_vm::Value::None
            }
            (0x90, 0x1B) => {
                let args = pop_args(stack, 4);
                self.apply_native_bitmap_operation(&args, NativeBitmapOperation::Copy);
                ethornell_vm::Value::None
            }
            (0x90, 0x1C) => {
                let args = pop_args(stack, 10);
                self.apply_native_bitmap_operation(&args, NativeBitmapOperation::Scale);
                ethornell_vm::Value::None
            }
            (0x90, 0x1D) => {
                let args = pop_args(stack, 4);
                self.apply_native_bitmap_operation(&args, NativeBitmapOperation::Copy);
                ethornell_vm::Value::None
            }
            (0x90, 0x21) => {
                let args = pop_args(stack, 9);
                self.schedule_native_graph_procedure(&args);
                self.apply_native_transition_args(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x24) => {
                let args = pop_args(stack, 12);
                self.schedule_native_graph_procedure(&args);
                self.apply_native_transition_args(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x2C) => {
                let args = pop_args(stack, 9);
                self.schedule_native_graph_procedure(&args);
                self.apply_native_transition_args(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x3F) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                if !self.graph_handle_exists(handle) {
                    tracing::debug!(handle, "native graph handle query missed");
                }
                ethornell_vm::Value::None
            }
            (0x90, 0x41) => {
                let args = pop_args(stack, 3);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                if let [child, parent, _mode] = values.as_slice() {
                    self.graph_object_layers
                        .entry(*parent)
                        .or_default()
                        .insert(*child);
                    self.graph_bindings.insert(*child, *parent);
                }
                ethornell_vm::Value::None
            }
            (0x90, 0x42) => {
                let args = pop_args(stack, 6);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x44) => {
                let args = pop_args(stack, 3);
                let count = args.first().map(value_to_i32).unwrap_or_default();
                let target = args.last().map(value_to_i32).unwrap_or_default();
                if let Some(layer) = self.graph_layers.get_mut(&target) {
                    layer.z = count;
                }
                ethornell_vm::Value::None
            }
            (0x90, 0x45) => {
                let args = pop_args(stack, 5);
                self.apply_graph_object_property(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x46) => {
                let args = pop_args(stack, 3);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x48) => {
                let args = pop_args(stack, 5);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x49) => {
                let args = pop_args(stack, 3);
                self.apply_graph_object_property(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x59) => {
                let args = pop_args(stack, 13);
                self.schedule_native_graph_procedure(&args);
                self.apply_native_transition_args(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x5B) => {
                let args = pop_args(stack, 10);
                self.apply_graph_object_effect(&args);
                self.apply_native_transition_args(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x70) => {
                let handle = self.alloc_object();
                self.set_current_graph_object(handle);
                ethornell_vm::Value::Int(handle)
            }
            (0x90, 0x71) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                self.remove_graph_object(handle);
                ethornell_vm::Value::None
            }
            (0x90, 0x74) => {
                let child = pop_int_value(stack).unwrap_or_default();
                let parent = pop_int_value(stack).unwrap_or_default();
                if child > 0 && parent > 0 {
                    self.graph_bindings.insert(child, parent);
                    self.graph_object_layers
                        .entry(parent)
                        .or_default()
                        .insert(child);
                }
                ethornell_vm::Value::None
            }
            (0x90, 0x75) => {
                let args = pop_args(stack, 7);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x76) => {
                let args = pop_args(stack, 5);
                self.apply_graph_object_property(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x78) => {
                let args = pop_args(stack, 4);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x79) => {
                let args = pop_args(stack, 6);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x7A) => {
                let args = pop_args(stack, 2);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0x91) => {
                let value = pop_int_value(stack).unwrap_or_default();
                let target = pop_int_value(stack).unwrap_or_default();
                self.graph_object_properties
                    .entry(target)
                    .or_default()
                    .mask_alpha = value;
                ethornell_vm::Value::None
            }
            (0x90, 0x92) => {
                let target = pop_int_value(stack).unwrap_or_default();
                self.text_nodes.remove(&target);
                self.graph_layers.remove(&target);
                ethornell_vm::Value::None
            }
            (0x90, 0x9E) => {
                let source = pop_int_value(stack).unwrap_or_default();
                let destination = pop_int_value(stack).unwrap_or_default();
                self.copy_graph_backing(source, destination);
                ethornell_vm::Value::None
            }
            (0x90, 0xA0) => {
                let args = pop_args(stack, 8);
                self.render_native_text_args(&args);
                self.schedule_native_graph_procedure(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0xA1) => {
                let args = pop_args(stack, 6);
                self.render_native_text_args(&args);
                self.schedule_native_graph_procedure(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0xA2) | (0x90, 0xA3) => {
                let args = pop_args(stack, 14);
                self.render_native_text_args(&args);
                self.schedule_native_graph_procedure(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0xA4) => {
                let value = pop_int_value(stack).unwrap_or_default();
                let target = pop_int_value(stack).unwrap_or_default();
                if let Some(node) = self.text_nodes.get_mut(&target) {
                    node.enabled = value != 0;
                }
                ethornell_vm::Value::None
            }
            (0x90, 0xA5) => {
                let target = pop_int_value(stack).unwrap_or_default();
                self.text_nodes.remove(&target);
                ethornell_vm::Value::None
            }
            (0x90, 0xA6) => {
                let args = pop_args(stack, 4);
                self.apply_graph_layer_property(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0xA7) => {
                let args = pop_args(stack, 2);
                self.render_native_text_args(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0xB0) | (0x90, 0xB1) => {
                let args = pop_args(stack, 7);
                self.apply_native_transition_args(&args);
                self.schedule_native_graph_procedure(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0xB5) => {
                let args = pop_args(stack, 3);
                self.apply_graph_object_effect(&args);
                ethornell_vm::Value::None
            }
            // VM writes the output word for 0xBD.
            (0x90, 0xBD) => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            (0x90, 0xC0) => {
                let file = pop_string_value(stack).unwrap_or_default();
                let archive = pop_string_value(stack).unwrap_or_default();
                let target = pop_int_value(stack).unwrap_or_default();
                self.load_graph_image_resource(target, &archive, &file);
                ethornell_vm::Value::None
            }
            (0x90, 0xC2) => {
                let args = pop_args(stack, 3);
                self.apply_native_bitmap_operation(&args, NativeBitmapOperation::Composite);
                ethornell_vm::Value::None
            }
            (0x90, 0xC3) => {
                let source = pop_int_value(stack).unwrap_or_default();
                let destination = pop_int_value(stack).unwrap_or_default();
                self.copy_graph_backing(source, destination);
                ethornell_vm::Value::None
            }
            (0x90, 0xC4) => {
                let _args = pop_args(stack, 3);
                ethornell_vm::Value::Int(0)
            }
            (0x90, 0xC5) => {
                let file = pop_string_value(stack).unwrap_or_default();
                let _arg1 = pop_int_value(stack).unwrap_or_default();
                let _arg2 = pop_int_value(stack).unwrap_or_default();
                let target = pop_int_value(stack).unwrap_or_default();
                let loaded = self.load_graph_image_resource(target, "", &file);
                ethornell_vm::Value::Int(if loaded { 0 } else { -1 })
            }
            (0x90, 0xC6) | (0x90, 0xC7) => {
                let args = pop_args(stack, 4);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                ethornell_vm::Value::Int(i32::from(
                    values.get(1) == values.get(2)
                        && values.first().copied().unwrap_or_default() >= 0,
                ))
            }
            (0x90, 0xC8) => {
                let args = pop_args(stack, 18);
                self.apply_native_transition_args(&args);
                ethornell_vm::Value::None
            }
            (0x90, 0xCA) => {
                let source = pop_int_value(stack).unwrap_or_default();
                let destination = pop_int_value(stack).unwrap_or_default();
                self.copy_graph_backing(source, destination);
                ethornell_vm::Value::None
            }
            (0x90, 0xCE) => {
                let args = pop_args(stack, 5);
                self.apply_native_bitmap_operation(&args, NativeBitmapOperation::Composite);
                ethornell_vm::Value::None
            }
            (0x90, 0xDC) => {
                let value = pop_int_value(stack).unwrap_or_default();
                let handle = pop_int_value(stack).unwrap_or_default();
                if let Some(state) = self.graph_scroll_states.get_mut(&handle) {
                    state.mode = value;
                }
                ethornell_vm::Value::None
            }
            (0x90, 0xF8) => {
                self.graph_special_watches.clear();
                self.graph_special_events.clear();
                ethornell_vm::Value::None
            }
            (0x90, 0xFA) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                if handle > 0 {
                    self.graph_special_watches.insert(handle);
                }
                ethornell_vm::Value::None
            }
            (0x90, 0xFB) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                self.graph_special_watches.remove(&handle);
                ethornell_vm::Value::None
            }
            (0x90, 0xFC) => ethornell_vm::Value::Int(
                self.graph_special_watches
                    .iter()
                    .next()
                    .copied()
                    .unwrap_or(-1),
            ),
            (0x90, 0xFD) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(
                    self.graph_bindings
                        .get(&handle)
                        .copied()
                        .unwrap_or_default(),
                )
            }
            _ => return None,
        };
        Some(Ok(value))
    }

    pub(super) fn schedule_native_graph_procedure(&mut self, args: &[ethornell_vm::Value]) {
        let duration_ms = args
            .iter()
            .map(value_to_i32)
            .filter(|value| (1..=120_000).contains(value))
            .min()
            .unwrap_or(1);
        self.pending_graph_procedure_schedule = Some(ethornell_vm::GraphProcedureSchedule {
            duration_ms,
            input_enabled: false,
            input_descriptor: 0,
            wait_for_input: false,
            completion: ethornell_vm::GraphProcedureCompletion::None,
        });
    }

    pub(super) fn render_native_text_args(&mut self, args: &[ethornell_vm::Value]) {
        if args
            .iter()
            .any(|value| matches!(value, ethornell_vm::Value::Str(text) if !text.is_empty()))
        {
            self.render_graph_text(args);
        }
    }

    fn apply_native_transition_args(&mut self, args: &[ethornell_vm::Value]) {
        let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
        let Some(target) = values
            .iter()
            .rev()
            .copied()
            .find(|value| self.graph_handle_exists(*value))
        else {
            return;
        };
        let alpha = values
            .iter()
            .copied()
            .find(|value| (0..=256).contains(value))
            .unwrap_or(256);
        let opacity = alpha as f32 / 256.0;
        for layer in self.graph_target_layers(target) {
            if let Some(layer) = self.graph_layers.get_mut(&layer) {
                layer.opacity = opacity;
            }
        }
        if let Some(surface) = self.graph_surfaces.get_mut(&target) {
            surface.opacity = opacity;
        }
    }

    pub(super) fn apply_native_bitmap_operation(
        &mut self,
        args: &[ethornell_vm::Value],
        operation: NativeBitmapOperation,
    ) {
        let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
        let handles = values
            .iter()
            .rev()
            .copied()
            .filter(|value| self.graph_resources.contains_key(value))
            .take(2)
            .collect::<Vec<_>>();
        let [destination, source] = handles.as_slice() else {
            return;
        };
        match operation {
            NativeBitmapOperation::Copy => self.copy_graph_backing(*source, *destination),
            NativeBitmapOperation::Composite => {
                let x = values.get(1).copied().unwrap_or_default();
                let y = values.first().copied().unwrap_or_default();
                self.composite_graph_bitmap(*destination, *source, x, y, 0, 256);
            }
            NativeBitmapOperation::Scale => {
                let scale_x = values
                    .iter()
                    .copied()
                    .find(|value| value.unsigned_abs() >= 0x100)
                    .unwrap_or(0x1_0000);
                let scale_y = scale_x;
                if let Some(image) = self
                    .graph_bitmap_image(*source)
                    .and_then(|image| scale_decoded_image_fixed(&image, scale_x, scale_y))
                {
                    let width = image.width;
                    let height = image.height;
                    let key = format!("runtime:native-scale:{destination}");
                    self.store_graph_image(key.clone(), image);
                    self.graph_resources
                        .insert(*destination, RuntimeGraphResource::whole(key));
                    self.bitmap_dimensions.insert(*destination, (width, height));
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum NativeBitmapOperation {
    Copy,
    Composite,
    Scale,
}
