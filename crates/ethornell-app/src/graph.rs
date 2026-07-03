use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeGraphResource {
    pub(crate) key: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeGraphLayer {
    pub(crate) hit_id: i32,
    pub(crate) owner_object: Option<i32>,
    pub(crate) key: String,
    pub(crate) target_surface: Option<i32>,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) src_x: f32,
    pub(crate) src_y: f32,
    pub(crate) opacity: f32,
    pub(crate) z: i32,
    pub(crate) enabled: bool,
    pub(crate) transform_x: f32,
    pub(crate) transform_y: f32,
    pub(crate) scale_x: f32,
    pub(crate) scale_y: f32,
    pub(crate) rotation_degrees: f32,
    pub(crate) clip: Option<RuntimeClipRect>,
}

impl RuntimeGraphLayer {
    pub(crate) fn screen_x(&self, surfaces: &BTreeMap<i32, RuntimeSurface>) -> f32 {
        self.target_surface
            .and_then(|surface| surfaces.get(&surface))
            .map(|surface| surface.x + self.x)
            .unwrap_or(self.x)
    }

    pub(crate) fn screen_y(&self, surfaces: &BTreeMap<i32, RuntimeSurface>) -> f32 {
        self.target_surface
            .and_then(|surface| surfaces.get(&surface))
            .map(|surface| surface.y + self.y)
            .unwrap_or(self.y)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeSurface {
    pub(crate) id: i32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) viewport_width: f32,
    pub(crate) viewport_height: f32,
    pub(crate) resource_id: Option<i32>,
    pub(crate) enabled: bool,
}

impl RuntimeSurface {
    pub(crate) fn new(id: i32, width: f32, height: f32) -> Self {
        Self {
            id,
            width,
            height,
            x: 0.0,
            y: 0.0,
            viewport_width: width,
            viewport_height: height,
            resource_id: None,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeGraphDrawItem {
    pub(crate) key: String,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) src_x: f32,
    pub(crate) src_y: f32,
    pub(crate) src_width: f32,
    pub(crate) src_height: f32,
    pub(crate) opacity: f32,
    pub(crate) rotation_degrees: f32,
    pub(crate) clip: Option<RuntimeClipRect>,
    pub(crate) z: i32,
    pub(crate) hit_id: i32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeClipRect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeUserControl {
    pub(crate) id: i32,
    pub(crate) owner_id: i32,
    pub(crate) payload: i32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) enabled: bool,
    pub(crate) title_only: bool,
}

impl Default for RuntimeUserControl {
    fn default() -> Self {
        Self {
            id: 0,
            owner_id: 0,
            payload: 0,
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            enabled: true,
            title_only: false,
        }
    }
}

impl RuntimeUserControl {
    pub(crate) fn contains(&self, point: (f32, f32), title_active: bool) -> bool {
        self.enabled
            && (!self.title_only || title_active)
            && point.0 >= self.x
            && point.0 < self.x + self.width
            && point.1 >= self.y
            && point.1 < self.y + self.height
    }
}

pub(crate) fn fixed_16_to_f32(value: i32) -> f32 {
    value as f32 / 65_536.0
}
