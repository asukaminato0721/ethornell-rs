#[derive(Debug, Clone, Copy)]
pub(crate) struct GraphKnobState {
    pub(crate) target: i32,
    pub(crate) base_x: f32,
    pub(crate) base_y: f32,
    pub(crate) enabled: bool,
    pub(crate) relative_mode: i32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) extent_x: i32,
    pub(crate) extent_y: i32,
    pub(crate) bounds_width: i32,
    pub(crate) bounds_height: i32,
    pub(crate) event_pending: bool,
    pub(crate) event_x: i32,
    pub(crate) event_y: i32,
    pub(crate) event_kind: i32,
    pub(crate) changed: bool,
    /// Mouse-to-thumb offset captured by CDspObjKnob::BeginDrag
    /// (target sub_4212A0). These are screen-space pixels because the target
    /// stores the pointer position minus the controlled object's composite
    /// position at capture time.
    pub(crate) drag_offset_x: f32,
    pub(crate) drag_offset_y: f32,
}

impl GraphKnobState {
    pub(crate) fn new(target: i32, base_x: f32, base_y: f32) -> Self {
        Self {
            target,
            base_x,
            base_y,
            enabled: false,
            relative_mode: 1,
            x: 0,
            y: 0,
            extent_x: 0,
            extent_y: 0,
            bounds_width: 0,
            bounds_height: 0,
            event_pending: false,
            event_x: 0,
            event_y: 0,
            event_kind: 0,
            changed: false,
            drag_offset_x: 0.0,
            drag_offset_y: 0.0,
        }
    }

    pub(crate) fn take_vertical_event(&mut self) -> i32 {
        let value = if self.event_pending && self.event_kind != 0 {
            self.event_y
        } else {
            0
        };
        self.event_pending = false;
        self.event_x = 0;
        self.event_y = 0;
        self.event_kind = 0;
        value
    }

    pub(crate) fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Portable equivalent of sub_421520, used by the native mouse-wheel
    /// watch list. Wheel input changes the logical Y coordinate by exactly
    /// one step and records a one-shot event regardless of whether the move
    /// succeeded. The event-kind field is nonzero only when SetPosition
    /// rejected the requested step at the range boundary; Graph90:DA then
    /// returns the unconsumed vertical delta so an outer control can handle it.
    pub(crate) fn wheel_step(
        &mut self,
        delta_y: i32,
        target_width: f32,
        target_height: f32,
    ) -> bool {
        let accepted = self.set_position(
            self.x,
            self.y.wrapping_add(delta_y),
            target_width,
            target_height,
        );
        self.event_pending = true;
        self.event_x = 0;
        self.event_y = delta_y;
        self.event_kind = i32::from(!accepted);
        accepted
    }

    pub(crate) fn set_extent(&mut self, extent_x: i32, extent_y: i32) -> bool {
        // CDspObjKnob::SetDivisionCount (sub_4211E0) rejects either negative
        // coordinate and otherwise stores both values unchanged.
        if extent_x < 0 || extent_y < 0 {
            return false;
        }
        self.extent_x = extent_x;
        self.extent_y = extent_y;
        true
    }

    pub(crate) fn set_base_position(&mut self, x: i32, y: i32) {
        self.base_x = x as f32;
        self.base_y = y as f32;
    }

    pub(crate) fn set_bounds(
        &mut self,
        width: i32,
        height: i32,
        target_width: f32,
        target_height: f32,
    ) -> bool {
        // sub_421200 treats the supplied values as inclusive content sizes.
        // A content rectangle smaller than the knob object's own rectangle is
        // rejected without changing the previous range.
        let target_width = target_width.round().max(1.0) as i32;
        let target_height = target_height.round().max(1.0) as i32;
        if width < target_width || height < target_height {
            return false;
        }
        self.bounds_width = width;
        self.bounds_height = height;
        true
    }

    pub(crate) fn set_position(
        &mut self,
        x: i32,
        y: i32,
        target_width: f32,
        target_height: f32,
    ) -> bool {
        let (max_x, max_y) = self.logical_limits(target_width, target_height);
        let mut accepted = true;

        // sub_421430 validates and stores X before validating Y. Preserve that
        // partial-update behavior, but only move the child when both pass.
        if (0..=max_x).contains(&x) {
            self.x = x;
        } else {
            accepted = false;
        }
        if (0..=max_y).contains(&y) {
            self.y = y;
        } else {
            accepted = false;
        }
        accepted
    }

    pub(crate) fn logical_limits(self, target_width: f32, target_height: f32) -> (i32, i32) {
        (
            native_knob_limit(self.extent_x, self.bounds_width, target_width),
            native_knob_limit(self.extent_y, self.bounds_height, target_height),
        )
    }

    pub(crate) fn display_position(self, target_width: f32, target_height: f32) -> (f32, f32) {
        (
            self.base_x
                + native_knob_offset(self.x, self.extent_x, self.bounds_width, target_width),
            self.base_y
                + native_knob_offset(self.y, self.extent_y, self.bounds_height, target_height),
        )
    }

    /// Portable equivalent of target sub_4212A0.  The native `relative_mode`
    /// flag (CDspObjKnob+0x140) decides whether a drag preserves the point
    /// inside the thumb at which the user grabbed it.
    pub(crate) fn begin_drag(&mut self, mouse_x: f32, mouse_y: f32, target_x: f32, target_y: f32) {
        if self.relative_mode != 0 {
            self.drag_offset_x = mouse_x - target_x;
            self.drag_offset_y = mouse_y - target_y;
        } else {
            self.drag_offset_x = 0.0;
            self.drag_offset_y = 0.0;
        }
    }

    /// Portable equivalent of target sub_421300. `origin_x/y` is the
    /// screen-space position at which logical (0,0) places the controlled
    /// sprite. The target clamps in pixel space first and then converts back
    /// to the logical precision domain using the same 16.16 step and rounding
    /// bias as sub_4215E0/sub_421640.
    pub(crate) fn drag_to(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        origin_x: f32,
        origin_y: f32,
        target_width: f32,
        target_height: f32,
    ) -> bool {
        let desired_x = mouse_x - self.drag_offset_x - origin_x;
        let desired_y = mouse_y - self.drag_offset_y - origin_y;
        let logical_x = native_knob_logical_from_pixel(
            desired_x,
            self.extent_x,
            self.bounds_width,
            target_width,
        );
        let logical_y = native_knob_logical_from_pixel(
            desired_y,
            self.extent_y,
            self.bounds_height,
            target_height,
        );
        if logical_x == self.x && logical_y == self.y {
            return false;
        }
        // The values were produced by the target's own clamp/range mapping, so
        // this call should be accepted unless the object was reconfigured in
        // the middle of the host event. Preserve SetPosition's validation.
        if self.set_position(logical_x, logical_y, target_width, target_height) {
            self.changed = true;
            true
        } else {
            false
        }
    }
}

// CDspObjKnob::SetPosition (sub_421430) uses a 16.16 step calculated by
// sub_4215E0/sub_421640. Preserve that integer rounding before rendering.
fn native_knob_offset(value: i32, extent: i32, bounds: i32, target_size: f32) -> f32 {
    let available = bounds.saturating_sub(target_size.round().max(1.0) as i32);
    if extent <= 0 {
        return value as f32;
    }
    if extent == 1 || available <= 0 {
        return 0.0;
    }
    let step = native_knob_step(extent, available);
    (((step + 1) * i64::from(value)) >> 16) as f32
}

fn native_knob_logical_from_pixel(pixel: f32, extent: i32, bounds: i32, target_size: f32) -> i32 {
    let available = bounds.saturating_sub(target_size.round().max(1.0) as i32);
    if available <= 0 {
        return 0;
    }
    let pixel = pixel.round().clamp(0.0, available as f32) as i32;
    match extent {
        i32::MIN..=0 => pixel,
        1 => 0,
        _ => {
            let step = native_knob_step(extent, available);
            let round_bias = available / (2 * extent - 2);
            ((((i64::from(pixel + round_bias)) << 16) / step)
                .clamp(0, i64::from(native_knob_limit(extent, bounds, target_size)))) as i32
        }
    }
}

fn native_knob_limit(extent: i32, bounds: i32, target_size: f32) -> i32 {
    let available = bounds.saturating_sub(target_size.round().max(1.0) as i32);
    if available <= 0 {
        return 0;
    }
    match extent {
        i32::MIN..=0 => available,
        1 => 0,
        _ => {
            let step = native_knob_step(extent, available);
            (((i64::from(available)) << 16) / step).clamp(0, i64::from(i32::MAX)) as i32
        }
    }
}

fn native_knob_step(extent: i32, available: i32) -> i64 {
    let mut step = (i64::from(available)) << 16;
    // Native sub_4215E0/421640 only divides for counts above two.
    if extent > 2 {
        step /= i64::from(extent - 1);
    }
    step.max(1)
}

#[cfg(test)]
mod tests {
    use super::{
        native_knob_limit, native_knob_logical_from_pixel, native_knob_offset, GraphKnobState,
    };

    #[test]
    fn native_knob_range_reaches_both_pixel_bounds() {
        assert_eq!(native_knob_offset(0, 64, 406, 40.0), 0.0);
        assert_eq!(native_knob_offset(63, 64, 406, 40.0), 366.0);
    }

    #[test]
    fn unscaled_knob_position_is_clamped_to_available_pixels() {
        assert_eq!(native_knob_offset(42, 0, 100, 20.0), 42.0);
        assert_eq!(native_knob_limit(0, 100, 20.0), 80);
    }

    #[test]
    fn one_record_backlog_has_only_the_zero_logical_position() {
        let mut state = GraphKnobState::new(7, 0.0, 0.0);
        assert!(state.set_extent(0, 1));
        assert!(state.set_bounds(27, 589, 27.0, 566.0));
        assert_eq!(state.logical_limits(27.0, 566.0), (0, 0));
        assert!(state.set_position(0, 0, 27.0, 566.0));
        assert!(!state.set_position(0, 1, 27.0, 566.0));
        assert_eq!((state.x, state.y), (0, 0));
    }

    #[test]
    fn native_bounds_reject_content_smaller_than_the_target() {
        let mut state = GraphKnobState::new(7, 0.0, 0.0);
        assert!(!state.set_bounds(26, 565, 27.0, 566.0));
        assert_eq!((state.bounds_width, state.bounds_height), (0, 0));
    }
    #[test]
    fn inverse_drag_mapping_matches_native_precision_endpoints() {
        assert_eq!(native_knob_logical_from_pixel(0.0, 64, 406, 40.0), 0);
        assert_eq!(native_knob_logical_from_pixel(366.0, 64, 406, 40.0), 63);
        assert_eq!(native_knob_logical_from_pixel(-100.0, 64, 406, 40.0), 0);
        assert_eq!(native_knob_logical_from_pixel(999.0, 64, 406, 40.0), 63);
    }

    #[test]
    fn config_slider_precision_round_trips_horizontal_and_vertical_endpoints() {
        // cnfgsldctrl._bp uses 101 logical horizontal positions over a
        // 406-pixel content range. cnfgclrwnd._bp uses 257 vertical logical
        // positions for the color slider. Keep both endpoint mappings stable.
        let horizontal_available = 406 - 40;
        assert_eq!(native_knob_offset(100, 101, 406, 40.0), horizontal_available as f32);
        assert_eq!(
            native_knob_logical_from_pixel(horizontal_available as f32, 101, 406, 40.0),
            100
        );

        let vertical_available = 212 - 20;
        assert_eq!(native_knob_offset(256, 257, 212, 20.0), vertical_available as f32);
        assert_eq!(
            native_knob_logical_from_pixel(vertical_available as f32, 257, 212, 20.0),
            256
        );
    }

    #[test]
    fn relative_drag_preserves_grab_offset_and_marks_changed() {
        let mut state = GraphKnobState::new(7, 0.0, 0.0);
        assert!(state.set_extent(64, 0));
        assert!(state.set_bounds(406, 40, 40.0, 40.0));
        state.begin_drag(110.0, 20.0, 100.0, 0.0);
        assert!(state.drag_to(210.0, 20.0, 100.0, 0.0, 40.0, 40.0));
        assert!(state.x > 0);
        assert!(state.changed);
    }

}
