#[derive(Debug, Clone, Copy)]
pub(crate) struct GraphScrollState {
    pub(crate) target: i32,
    pub(crate) base_x: f32,
    pub(crate) base_y: f32,
    pub(crate) mode: i32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) extent_x: i32,
    pub(crate) extent_y: i32,
    pub(crate) bounds_width: i32,
    pub(crate) bounds_height: i32,
}

impl GraphScrollState {
    pub(crate) fn new(target: i32, base_x: f32, base_y: f32) -> Self {
        Self {
            target,
            base_x,
            base_y,
            mode: 0,
            x: 0,
            y: 0,
            extent_x: 0,
            extent_y: 0,
            bounds_width: 0,
            bounds_height: 0,
        }
    }

    pub(crate) fn display_position(self, target_width: f32, target_height: f32) -> (f32, f32) {
        (
            self.base_x
                + native_scroll_offset(self.x, self.extent_x, self.bounds_width, target_width),
            self.base_y
                + native_scroll_offset(self.y, self.extent_y, self.bounds_height, target_height),
        )
    }
}

// CDspObjKnob::SetPosition (sub_421430) uses a 16.16 step calculated by
// sub_4215E0/sub_421640. Preserve that integer rounding before rendering.
fn native_scroll_offset(value: i32, extent: i32, bounds: i32, target_size: f32) -> f32 {
    let available = (bounds.saturating_sub(target_size.round() as i32)).max(0);
    if extent <= 0 {
        return value.clamp(0, available) as f32;
    }
    if extent <= 1 || available == 0 {
        return 0.0;
    }
    let step = (((available as i64) << 16) / i64::from(extent - 1)).max(1);
    (((step + 1) * i64::from(value.clamp(0, extent - 1))) >> 16) as f32
}

#[cfg(test)]
mod tests {
    use super::native_scroll_offset;

    #[test]
    fn native_knob_range_reaches_both_pixel_bounds() {
        assert_eq!(native_scroll_offset(0, 64, 406, 40.0), 0.0);
        assert_eq!(native_scroll_offset(63, 64, 406, 40.0), 366.0);
    }

    #[test]
    fn unscaled_knob_position_is_clamped_to_available_pixels() {
        assert_eq!(native_scroll_offset(42, 0, 100, 20.0), 42.0);
        assert_eq!(native_scroll_offset(99, 0, 100, 20.0), 80.0);
    }
}
