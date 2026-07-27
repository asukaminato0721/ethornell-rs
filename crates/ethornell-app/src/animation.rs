use crate::graph::{RuntimeGraphLayer, RuntimeSurface};
use ethornell_vm::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScheduledObjectControl {
    pub(crate) target_object: i32,
    pub(crate) target_x: i32,
    pub(crate) target_y: i32,
    pub(crate) position_curve: i32,
    pub(crate) target_z: i32,
    pub(crate) z_curve: i32,
    pub(crate) target_alpha: i32,
    pub(crate) update_denominator: i32,
    pub(crate) update_numerator: i32,
    pub(crate) input_descriptor: i32,
    pub(crate) input_enabled: bool,
    pub(crate) duration_ms: i32,
}

impl ScheduledObjectControl {
    // Native 0x90:28 (sub_47AC80) pops these fields in this order before
    // forwarding them to sub_491D60/sub_431D90/sub_431E80.
    pub(crate) fn from_popped_args(args: &[Value]) -> Option<Self> {
        let source = args.iter().rev().collect::<Vec<_>>();
        Some(Self {
            target_object: value_to_i32(source.first()?)?,
            target_x: value_to_i32(source.get(1)?)?,
            target_y: value_to_i32(source.get(2)?)?,
            position_curve: value_to_i32(source.get(3)?)?,
            target_z: value_to_i32(source.get(4)?)?,
            z_curve: value_to_i32(source.get(5)?)?,
            target_alpha: value_to_i32(source.get(6)?)?,
            duration_ms: value_to_i32(source.get(7)?)?.max(1),
            update_denominator: value_to_i32(source.get(8)?)?,
            update_numerator: value_to_i32(source.get(9)?)?,
            input_enabled: value_to_i32(source.get(10)?)? != 0,
            input_descriptor: value_to_i32(source.get(11)?)?,
        })
    }

    pub(crate) fn procedure_schedule(self) -> ethornell_vm::GraphProcedureSchedule {
        ethornell_vm::GraphProcedureSchedule {
            duration_ms: self.duration_ms,
            input_enabled: self.input_enabled,
            input_descriptor: self.input_descriptor,
            wait_for_input: false,
            completion: ethornell_vm::GraphProcedureCompletion::ControlProgress,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct GraphAnimationRegistry {
    records: BTreeMap<i32, GraphAnimationRecord>,
}

impl GraphAnimationRegistry {
    pub(crate) fn start(&mut self, handle: i32, duration: i32, args: &[Value]) {
        if handle <= 0 {
            return;
        }
        let duration_frames = duration_to_frames(duration);
        self.records.insert(
            handle,
            GraphAnimationRecord {
                handle,
                duration_frames,
                remaining_frames: duration_frames,
                active: duration_frames > 0,
                released: false,
                args: args.iter().filter_map(value_to_i32).collect(),
            },
        );
    }

    pub(crate) fn cancel(&mut self, handle: i32) {
        if let Some(record) = self.records.get_mut(&handle) {
            record.remaining_frames = 0;
            record.active = false;
        }
    }

    pub(crate) fn release(&mut self, handle: i32) {
        if let Some(record) = self.records.get_mut(&handle) {
            record.remaining_frames = 0;
            record.active = false;
            record.released = true;
        }
    }

    pub(crate) fn evaluate(&mut self, args: &[Value]) -> Option<GraphAnimationSnapshot> {
        let values = args.iter().filter_map(value_to_i32).collect::<Vec<_>>();
        let handle = values
            .iter()
            .copied()
            .find(|value| self.records.contains_key(value))
            .or_else(|| values.iter().copied().find(|value| *value > 0))?;
        let record = self
            .records
            .entry(handle)
            .or_insert_with(|| GraphAnimationRecord::new(handle));
        if record.active && record.remaining_frames > 0 {
            record.remaining_frames -= 1;
        }
        if record.remaining_frames == 0 {
            record.active = false;
        }
        Some(record.snapshot())
    }

    pub(crate) fn tick(&mut self) -> Vec<i32> {
        let mut finished = Vec::new();
        for record in self.records.values_mut() {
            let was_active = record.active;
            if record.active && record.remaining_frames > 0 {
                record.remaining_frames -= 1;
            }
            if record.remaining_frames == 0 {
                record.active = false;
            }
            if was_active && !record.active {
                finished.push(record.handle);
            }
        }
        finished
    }
}

#[derive(Debug, Clone)]
struct GraphAnimationRecord {
    handle: i32,
    duration_frames: u32,
    remaining_frames: u32,
    active: bool,
    released: bool,
    args: Vec<i32>,
}

impl GraphAnimationRecord {
    fn new(handle: i32) -> Self {
        Self {
            handle,
            duration_frames: 1,
            remaining_frames: 0,
            active: false,
            released: false,
            args: Vec::new(),
        }
    }

    fn snapshot(&self) -> GraphAnimationSnapshot {
        GraphAnimationSnapshot {
            handle: self.handle,
            duration_frames: self.duration_frames,
            remaining_frames: self.remaining_frames,
            active: self.active,
            released: self.released,
            args: self.args.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GraphAnimationSnapshot {
    pub(crate) handle: i32,
    pub(crate) duration_frames: u32,
    pub(crate) remaining_frames: u32,
    pub(crate) active: bool,
    pub(crate) released: bool,
    pub(crate) args: Vec<i32>,
}

#[derive(Debug, Default)]
pub(crate) struct LayerAnimationSystem {
    tracks: Vec<LayerAnimation>,
}

impl LayerAnimationSystem {
    pub(crate) fn fade_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.fade_to_eased(layer_id, from, to, duration_frames, 5);
    }

    pub(crate) fn fade_to_eased(
        &mut self,
        layer_id: i32,
        from: f32,
        to: f32,
        duration_frames: u32,
        curve: i32,
    ) {
        self.animate(
            layer_id,
            LayerAnimationProperty::Opacity,
            from,
            to,
            duration_frames,
            curve,
        );
    }

    pub(crate) fn move_x_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.animate(
            layer_id,
            LayerAnimationProperty::TransformX,
            from,
            to,
            duration_frames,
            5,
        );
    }

    pub(crate) fn move_y_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.animate(
            layer_id,
            LayerAnimationProperty::TransformY,
            from,
            to,
            duration_frames,
            5,
        );
    }

    pub(crate) fn scale_x_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.animate(
            layer_id,
            LayerAnimationProperty::ScaleX,
            from,
            to,
            duration_frames,
            5,
        );
    }

    pub(crate) fn scale_y_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.animate(
            layer_id,
            LayerAnimationProperty::ScaleY,
            from,
            to,
            duration_frames,
            5,
        );
    }

    pub(crate) fn base_vector_to(
        &mut self,
        layer_id: i32,
        from: (f32, f32, i32),
        to: (f32, f32, i32),
        duration_frames: u32,
        position_curve: i32,
        z_curve: i32,
    ) {
        self.animate(
            layer_id,
            LayerAnimationProperty::BaseX,
            from.0,
            to.0,
            duration_frames,
            position_curve,
        );
        self.animate(
            layer_id,
            LayerAnimationProperty::BaseY,
            from.1,
            to.1,
            duration_frames,
            position_curve,
        );
        self.animate(
            layer_id,
            LayerAnimationProperty::BaseZ,
            from.2 as f32,
            to.2 as f32,
            duration_frames,
            z_curve,
        );
    }

    fn animate(
        &mut self,
        layer_id: i32,
        property: LayerAnimationProperty,
        from: f32,
        to: f32,
        duration_frames: u32,
        curve: i32,
    ) {
        if duration_frames == 0 {
            return;
        }
        self.tracks
            .retain(|track| !(track.layer_id == layer_id && track.property == property));
        self.tracks.push(LayerAnimation {
            layer_id,
            property,
            from,
            to,
            duration_frames,
            elapsed_frames: 0,
            curve,
        });
    }

    pub(crate) fn clear_layer(&mut self, layer_id: i32) {
        self.tracks.retain(|track| track.layer_id != layer_id);
    }

    pub(crate) fn clear_layers<'a>(&mut self, layer_ids: impl IntoIterator<Item = &'a i32>) {
        let ids = layer_ids.into_iter().copied().collect::<Vec<_>>();
        self.tracks
            .retain(|track| !ids.iter().any(|id| *id == track.layer_id));
    }

    pub(crate) fn active_count(&self) -> usize {
        self.tracks.len()
    }

    pub(crate) fn tick(
        &mut self,
        layers: &mut BTreeMap<i32, RuntimeGraphLayer>,
        surfaces: &mut BTreeMap<i32, RuntimeSurface>,
    ) -> Vec<LayerAnimationEvent> {
        let mut events = Vec::new();
        let mut index = 0;
        while index < self.tracks.len() {
            let track = &mut self.tracks[index];
            track.elapsed_frames = track.elapsed_frames.saturating_add(1);
            let t = (track.elapsed_frames as f32 / track.duration_frames as f32).clamp(0.0, 1.0);
            let eased = native_ease(track.curve, t);
            let value = track.from + (track.to - track.from) * eased;
            if let Some(layer) = layers.get_mut(&track.layer_id) {
                match track.property {
                    LayerAnimationProperty::BaseX => layer.x = value,
                    LayerAnimationProperty::BaseY => layer.y = value,
                    LayerAnimationProperty::BaseZ => layer.z = value.round() as i32,
                    LayerAnimationProperty::Opacity => layer.opacity = value.clamp(0.0, 1.0),
                    LayerAnimationProperty::TransformX => layer.transform_x = value,
                    LayerAnimationProperty::TransformY => layer.transform_y = value,
                    LayerAnimationProperty::ScaleX => layer.scale_x = value.max(0.001),
                    LayerAnimationProperty::ScaleY => layer.scale_y = value.max(0.001),
                }
            } else if let Some(surface) = surfaces.get_mut(&track.layer_id) {
                match track.property {
                    LayerAnimationProperty::BaseX => surface.x = value,
                    LayerAnimationProperty::BaseY => surface.y = value,
                    LayerAnimationProperty::BaseZ => surface.z = value.round() as i32,
                    LayerAnimationProperty::Opacity => surface.opacity = value.clamp(0.0, 1.0),
                    LayerAnimationProperty::TransformX
                    | LayerAnimationProperty::TransformY
                    | LayerAnimationProperty::ScaleX
                    | LayerAnimationProperty::ScaleY => {}
                }
            }
            if track.elapsed_frames >= track.duration_frames {
                events.push(LayerAnimationEvent::Finished {
                    layer_id: track.layer_id,
                    property: track.property,
                });
                self.tracks.swap_remove(index);
            } else {
                index += 1;
            }
        }
        events
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayerAnimationProperty {
    BaseX,
    BaseY,
    BaseZ,
    Opacity,
    TransformX,
    TransformY,
    ScaleX,
    ScaleY,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LayerAnimationEvent {
    Finished {
        layer_id: i32,
        property: LayerAnimationProperty,
    },
}

#[derive(Debug, Clone)]
struct LayerAnimation {
    layer_id: i32,
    property: LayerAnimationProperty,
    from: f32,
    to: f32,
    duration_frames: u32,
    elapsed_frames: u32,
    curve: i32,
}

// sub_41A690 maps a 24-bit normalized progress value to 16.16 fixed point.
fn native_ease(curve: i32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match curve {
        1 => (1.0 - (std::f32::consts::PI * t).cos()) * 0.5,
        2 => (std::f32::consts::FRAC_PI_2 * t).sin(),
        3 => 1.0 - (std::f32::consts::FRAC_PI_2 * (1.0 - t)).sin(),
        4 => t.powi(2),
        5 => 1.0 - (1.0 - t).powi(2),
        6 => t.powf(2.5),
        7 => 1.0 - (1.0 - t).powf(2.5),
        8 => t.powi(3),
        9 => 1.0 - (1.0 - t).powi(3),
        10 => t.powi(4),
        11 => 1.0 - (1.0 - t).powi(4),
        12 => t.powi(5),
        13 => 1.0 - (1.0 - t).powi(5),
        14 => t.powi(6),
        15 => 1.0 - (1.0 - t).powi(6),
        _ => t,
    }
}

fn duration_to_frames(duration: i32) -> u32 {
    if duration <= 0 {
        return 1;
    }
    if duration <= 32 {
        return duration as u32;
    }
    (duration as u32).div_ceil(16).max(1)
}

fn value_to_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Int(value) => Some(*value),
        Value::Ptr(value) => Some(*value as i32),
        Value::Func { offset, .. } => Some(*offset as i32),
        Value::Str(_) | Value::Program(_) | Value::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_uses_native_duration_and_input_slots() {
        let args = [
            Value::Int(1808),
            Value::Int(1),
            Value::Int(0),
            Value::Int(0),
            Value::Int(250),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
        ];
        let schedule = ScheduledObjectControl::from_popped_args(&args).unwrap();
        assert_eq!(schedule.target_object, 0);
        assert_eq!(schedule.target_x, 0);
        assert_eq!(schedule.target_y, 0);
        assert_eq!(schedule.position_curve, 0);
        assert_eq!(schedule.target_z, 0);
        assert_eq!(schedule.z_curve, 0);
        assert_eq!(schedule.target_alpha, 0);
        assert_eq!(schedule.input_descriptor, 1808);
        assert!(schedule.input_enabled);
        assert_eq!(schedule.duration_ms, 250);
        assert_eq!(schedule.update_denominator, 0);
        assert_eq!(schedule.update_numerator, 0);
        assert_eq!(
            schedule.procedure_schedule(),
            ethornell_vm::GraphProcedureSchedule {
                duration_ms: 250,
                input_enabled: true,
                input_descriptor: 1808,
                wait_for_input: false,
                completion: ethornell_vm::GraphProcedureCompletion::ControlProgress,
            }
        );
    }

    #[test]
    fn native_curve_modes_keep_exact_endpoints() {
        for curve in 0..=15 {
            assert!((native_ease(curve, 0.0) - 0.0).abs() < 0.000_01);
            assert!((native_ease(curve, 1.0) - 1.0).abs() < 0.000_01);
        }
        assert!((native_ease(4, 0.5) - 0.25).abs() < 0.000_01);
        assert!((native_ease(5, 0.5) - 0.75).abs() < 0.000_01);
    }
}
