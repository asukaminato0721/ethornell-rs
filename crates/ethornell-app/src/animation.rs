use crate::graph::RuntimeGraphLayer;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(crate) struct LayerAnimationSystem {
    tracks: Vec<LayerAnimation>,
}

impl LayerAnimationSystem {
    pub(crate) fn fade_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.animate(
            layer_id,
            LayerAnimationProperty::Opacity,
            from,
            to,
            duration_frames,
        );
    }

    pub(crate) fn move_x_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.animate(
            layer_id,
            LayerAnimationProperty::TransformX,
            from,
            to,
            duration_frames,
        );
    }

    pub(crate) fn move_y_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.animate(
            layer_id,
            LayerAnimationProperty::TransformY,
            from,
            to,
            duration_frames,
        );
    }

    pub(crate) fn scale_x_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.animate(
            layer_id,
            LayerAnimationProperty::ScaleX,
            from,
            to,
            duration_frames,
        );
    }

    pub(crate) fn scale_y_to(&mut self, layer_id: i32, from: f32, to: f32, duration_frames: u32) {
        self.animate(
            layer_id,
            LayerAnimationProperty::ScaleY,
            from,
            to,
            duration_frames,
        );
    }

    fn animate(
        &mut self,
        layer_id: i32,
        property: LayerAnimationProperty,
        from: f32,
        to: f32,
        duration_frames: u32,
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
    ) -> Vec<LayerAnimationEvent> {
        let mut events = Vec::new();
        let mut index = 0;
        while index < self.tracks.len() {
            let track = &mut self.tracks[index];
            track.elapsed_frames = track.elapsed_frames.saturating_add(1);
            let t = (track.elapsed_frames as f32 / track.duration_frames as f32).clamp(0.0, 1.0);
            let eased = ease_out_quad(t);
            let value = track.from + (track.to - track.from) * eased;
            if let Some(layer) = layers.get_mut(&track.layer_id) {
                match track.property {
                    LayerAnimationProperty::Opacity => layer.opacity = value.clamp(0.0, 1.0),
                    LayerAnimationProperty::TransformX => layer.transform_x = value,
                    LayerAnimationProperty::TransformY => layer.transform_y = value,
                    LayerAnimationProperty::ScaleX => layer.scale_x = value.max(0.001),
                    LayerAnimationProperty::ScaleY => layer.scale_y = value.max(0.001),
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
}

fn ease_out_quad(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}
