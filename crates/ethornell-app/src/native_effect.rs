use super::*;

const PARTICLE_HANDLE_BASE: i32 = 0xC000_0000u32 as i32;
const MAX_PARTICLE_SCREENS: usize = 8;
const MAX_SPLINE_POINTS: usize = 100;

#[derive(Debug, Default)]
pub(super) struct NativeEffectState {
    particle_screens: BTreeMap<i32, NativeParticleScreen>,
    particle_banks: BTreeMap<u16, Vec<ethornell_vm::Value>>,
    particle_interpolation_mode: i32,
    splines: BTreeMap<i32, NativeSpline>,
    next_spline_handle: i32,
}

#[derive(Debug, Clone)]
struct NativeParticleScreen {
    handle: i32,
    width: i32,
    height: i32,
    enabled: bool,
    display: [i32; 5],
    frame_count: usize,
    frame_tables: [Vec<i32>; 2],
    native_field_340: i32,
    auto_update_interval_ms: i32,
    auto_update_elapsed_ms: i64,
    commit_count: u64,
    emitter_layout: Vec<i32>,
    emission_percent: i32,
    advance_count: u64,
    emitter_configuration: Vec<i32>,
    object_state: BTreeMap<u16, Vec<ethornell_vm::Value>>,
}

impl NativeParticleScreen {
    fn new(handle: i32, width: i32, height: i32) -> Self {
        Self {
            handle,
            width,
            height,
            enabled: true,
            display: [0, 0, 0, 128, 0],
            frame_count: 0,
            frame_tables: [Vec::new(), Vec::new()],
            native_field_340: 0,
            auto_update_interval_ms: 0,
            auto_update_elapsed_ms: 0,
            commit_count: 0,
            emitter_layout: Vec::new(),
            emission_percent: 0,
            advance_count: 0,
            emitter_configuration: Vec::new(),
            object_state: BTreeMap::new(),
        }
    }

    fn commit(&mut self) {
        self.commit_count = self.commit_count.saturating_add(1);
    }

    fn tick(&mut self, elapsed_ms: u64) {
        if self.auto_update_interval_ms <= 0 || !self.enabled {
            return;
        }
        self.auto_update_elapsed_ms = self
            .auto_update_elapsed_ms
            .saturating_add(i64::try_from(elapsed_ms).unwrap_or(i64::MAX));
        let interval = i64::from(self.auto_update_interval_ms.max(1));
        let commits = self.auto_update_elapsed_ms / interval;
        if commits > 0 {
            self.auto_update_elapsed_ms %= interval;
            self.commit_count = self
                .commit_count
                .saturating_add(u64::try_from(commits).unwrap_or(u64::MAX));
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct NativeSpline {
    duration: i32,
    points: Vec<[i32; 3]>,
}

impl NativeEffectState {
    fn allocate_particle_screen(&mut self, width: i32, height: i32) -> Option<i32> {
        let slot = (0..MAX_PARTICLE_SCREENS).find(|slot| {
            !self
                .particle_screens
                .contains_key(&(PARTICLE_HANDLE_BASE | *slot as i32))
        })?;
        let handle = PARTICLE_HANDLE_BASE | slot as i32;
        self.particle_screens
            .insert(handle, NativeParticleScreen::new(handle, width, height));
        Some(handle)
    }

    fn release_particle_screen(&mut self, handle: i32) -> bool {
        self.particle_screens.remove(&handle).is_some()
    }

    fn particle_screen_mut(&mut self, handle: i32) -> Option<&mut NativeParticleScreen> {
        self.particle_screens.get_mut(&handle)
    }

    pub(super) fn configure_particle_frame_tables(
        &mut self,
        handle: i32,
        first: &[i32],
        second: &[i32],
    ) -> bool {
        let Some(screen) = self.particle_screens.get_mut(&handle) else {
            return false;
        };
        screen.frame_count = first.len().max(second.len());
        screen.frame_tables = [first.to_vec(), second.to_vec()];
        true
    }

    pub(super) fn tick(&mut self, elapsed_ms: u64) {
        for screen in self.particle_screens.values_mut() {
            screen.tick(elapsed_ms);
        }
    }

    pub(super) fn create_spline(&mut self) -> i32 {
        let handle = self.next_spline_handle;
        self.next_spline_handle = self.next_spline_handle.wrapping_add(1);
        self.splines.insert(
            handle,
            NativeSpline {
                duration: 0,
                points: Vec::new(),
            },
        );
        handle
    }

    pub(super) fn release_spline(&mut self, handle: i32) -> i32 {
        if self.splines.remove(&handle).is_some() {
            0
        } else {
            1
        }
    }

    pub(super) fn spline_exists(&self, handle: i32) -> bool {
        self.splines.contains_key(&handle)
    }

    pub(super) fn configure_spline(
        &mut self,
        handle: i32,
        duration: i32,
        points: &[[i32; 3]],
    ) -> i32 {
        if points.len() < 2 {
            return 2;
        }
        if duration < 2 {
            return 3;
        }
        let Some(spline) = self.splines.get_mut(&handle) else {
            return 1;
        };
        spline.duration = duration;
        spline.points.clear();
        spline
            .points
            .extend(points.iter().take(MAX_SPLINE_POINTS).copied());
        0
    }

    pub(super) fn sample_spline(
        &self,
        handle: i32,
        time: i32,
    ) -> std::result::Result<[i32; 3], i32> {
        let Some(spline) = self.splines.get(&handle) else {
            return Err(1);
        };
        if time < 0 || time >= spline.duration {
            return Err(4);
        }
        if spline.duration <= 0 || spline.points.is_empty() {
            return Err(-1);
        }
        let t = f64::from(time) / f64::from(spline.duration);
        let mut output = [0; 3];
        for axis in 0..3 {
            let values = spline
                .points
                .iter()
                .map(|point| f64::from(point[axis]))
                .collect::<Vec<_>>();
            output[axis] = sample_natural_cubic(&values, t) as i32;
        }
        Ok(output)
    }
}

fn sample_natural_cubic(values: &[f64], normalized: f64) -> f64 {
    match values {
        [] => 0.0,
        [value] => *value,
        [first, second] => first + (second - first) * normalized.clamp(0.0, 1.0),
        _ => {
            let segments = values.len() - 1;
            let scaled = normalized.clamp(0.0, 1.0) * segments as f64;
            let segment = (scaled.floor() as usize).min(segments - 1);
            let local = scaled - segment as f64;

            // The target CSpline builds a natural cubic spline independently
            // for x/y/z. Equal knot spacing lets us solve the tridiagonal
            // second-derivative system with this compact Thomas pass.
            let n = values.len();
            let mut lower = vec![0.0; n];
            let mut diagonal = vec![1.0; n];
            let mut upper = vec![0.0; n];
            let mut rhs = vec![0.0; n];
            for index in 1..n - 1 {
                lower[index] = 1.0;
                diagonal[index] = 4.0;
                upper[index] = 1.0;
                rhs[index] = 6.0 * (values[index + 1] - 2.0 * values[index] + values[index - 1]);
            }
            for index in 1..n {
                let factor = lower[index] / diagonal[index - 1];
                diagonal[index] -= factor * upper[index - 1];
                rhs[index] -= factor * rhs[index - 1];
            }
            let mut second = vec![0.0; n];
            for index in (0..n).rev() {
                second[index] = if index + 1 < n {
                    (rhs[index] - upper[index] * second[index + 1]) / diagonal[index]
                } else {
                    rhs[index] / diagonal[index]
                };
            }
            let a = 1.0 - local;
            let b = local;
            a * values[segment]
                + b * values[segment + 1]
                + ((a * a * a - a) * second[segment] + (b * b * b - b) * second[segment + 1]) / 6.0
        }
    }
}

impl RuntimeTraceApi {
    pub(super) fn tick_native_effect_state(&mut self, elapsed_ms: u64) {
        self.native_effect.tick(elapsed_ms);
        self.effects.tick(elapsed_ms);
    }

    pub(super) fn dispatch_native_effect(
        &mut self,
        call: &mut ethornell_vm::NativeCallFrame,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        let (group, id) = (call.group(), call.id());
        let stack = call.args_mut();
        if group != 0xC0 {
            return None;
        }

        let value = match id {
            0x00 => {
                let popped = pop_args(stack, 2);
                let height = popped.first().map(value_to_i32).unwrap_or_default();
                let width = popped.get(1).map(value_to_i32).unwrap_or_default();
                let handle = self
                    .native_effect
                    .allocate_particle_screen(width, height)
                    .unwrap_or_default();
                if handle != 0 {
                    self.display_tree.register_inferred(handle);
                    self.graph_object_enabled.insert(handle, true);
                    self.graph_object_properties.entry(handle).or_default();
                }
                ethornell_vm::Value::Int(handle)
            }
            0x01 => {
                let handle = pop_int_value(stack).unwrap_or_default();
                if self.native_effect.release_particle_screen(handle) {
                    self.remove_graph_object(handle);
                }
                ethornell_vm::Value::None
            }
            0x04 => {
                let enabled = pop_int_value(stack).unwrap_or_default() != 0;
                let handle = pop_int_value(stack).unwrap_or_default();
                if let Some(screen) = self.native_effect.particle_screen_mut(handle) {
                    screen.enabled = enabled;
                    self.set_graph_object_enabled(handle, enabled);
                }
                ethornell_vm::Value::None
            }
            0x05 => {
                let mut source = pop_args(stack, 6);
                source.reverse();
                let values = source.iter().map(value_to_i32).collect::<Vec<_>>();
                if let [handle, x, y, z, blend, alpha] = values.as_slice()
                    && is_native_blend_mode(*blend)
                    && (0..=256).contains(alpha)
                    && let Some(screen) = self.native_effect.particle_screen_mut(*handle)
                {
                    screen.display = [*x, *y, *z, *blend, *alpha];
                    let properties = self.graph_object_properties.entry(*handle).or_default();
                    properties.blend_mode = *blend;
                    properties.set_alpha_parameter(*alpha);
                    self.display_tree.set_chain_depth(*handle, *z);
                }
                ethornell_vm::Value::None
            }
            // VM-owned because the two frame tables are raw DWORD arrays.
            0x06 => {
                let _ = pop_args(stack, 4);
                ethornell_vm::Value::None
            }
            0x08 => {
                let handle = pop_int_value(stack).unwrap_or_default();
                if let Some(screen) = self.native_effect.particle_screen_mut(handle) {
                    screen.commit();
                }
                ethornell_vm::Value::None
            }
            0x09 => {
                let interval = pop_int_value(stack).unwrap_or_default();
                let handle = pop_int_value(stack).unwrap_or_default();
                if let Some(screen) = self.native_effect.particle_screen_mut(handle) {
                    screen.auto_update_interval_ms = interval.max(0);
                    screen.auto_update_elapsed_ms = 0;
                }
                ethornell_vm::Value::None
            }
            0x0A => {
                let value = pop_int_value(stack).unwrap_or_default();
                let handle = pop_int_value(stack).unwrap_or_default();
                if let Some(screen) = self.native_effect.particle_screen_mut(handle) {
                    screen.native_field_340 = value;
                }
                ethornell_vm::Value::None
            }
            0x0B => {
                let mut source = pop_args(stack, 10);
                source.reverse();
                let handle = source.first().map(value_to_i32).unwrap_or_default();
                if let Some(screen) = self.native_effect.particle_screen_mut(handle) {
                    screen.emitter_layout = source.iter().skip(1).map(value_to_i32).collect();
                }
                ethornell_vm::Value::None
            }
            0x0C => {
                let percent = pop_int_value(stack).unwrap_or_default();
                let handle = pop_int_value(stack).unwrap_or_default();
                if (0..=100).contains(&percent)
                    && let Some(screen) = self.native_effect.particle_screen_mut(handle)
                {
                    screen.emission_percent = percent;
                }
                ethornell_vm::Value::None
            }
            0x0D => {
                let count = pop_int_value(stack).unwrap_or_default();
                let handle = pop_int_value(stack).unwrap_or_default();
                if let Some(screen) = self.native_effect.particle_screen_mut(handle) {
                    screen.advance_count = screen
                        .advance_count
                        .saturating_add(u64::try_from(count.max(0)).unwrap_or(u64::MAX));
                }
                ethornell_vm::Value::None
            }
            0x0F => {
                let handle = pop_int_value(stack).unwrap_or_default();
                if let Some(screen) = self.native_effect.particle_screen_mut(handle) {
                    screen.commit_count = 0;
                    screen.advance_count = 0;
                    screen.auto_update_elapsed_ms = 0;
                }
                ethornell_vm::Value::None
            }
            0x10 => {
                let mut source = pop_args(stack, 12);
                source.reverse();
                let handle = source.first().map(value_to_i32).unwrap_or_default();
                if let Some(screen) = self.native_effect.particle_screen_mut(handle) {
                    screen.emitter_configuration =
                        source.iter().skip(1).map(value_to_i32).collect();
                }
                ethornell_vm::Value::None
            }
            0x18 | 0x1A | 0x1B | 0x20 | 0x24 | 0x25 | 0x28 | 0x29 | 0x2C | 0x2D => {
                let count = match id {
                    0x18 => 7,
                    0x1A => 8,
                    0x1B => 3,
                    0x20 | 0x28 => 4,
                    0x24 | 0x2C => 2,
                    0x25 => 11,
                    0x29 => 16,
                    0x2D => 18,
                    _ => unreachable!(),
                };
                let mut source = pop_args(stack, count);
                source.reverse();
                self.native_effect.particle_banks.insert(id, source);
                ethornell_vm::Value::None
            }
            0x1F => {
                let mode = pop_int_value(stack).unwrap_or_default();
                if (0..=2).contains(&mode) {
                    self.native_effect.particle_interpolation_mode = mode;
                }
                ethornell_vm::Value::None
            }
            0x40..=0x4F => {
                let count = match id {
                    0x40 => 2,
                    0x41 => 1,
                    0x42..=0x44 | 0x47..=0x4B | 0x4E | 0x4F => 2,
                    0x45 => 6,
                    0x46 => 7,
                    0x4C | 0x4D => 4,
                    _ => unreachable!(),
                };
                let args = pop_args(stack, count);
                match self.effects.call(group, id, &args) {
                    crate::effects::EffectCall::Handled(result) => result
                        .map(ethornell_vm::Value::Int)
                        .unwrap_or(ethornell_vm::Value::None),
                    crate::effects::EffectCall::Unhandled => ethornell_vm::Value::None,
                }
            }
            0xC0 => ethornell_vm::Value::Int(self.native_effect.create_spline()),
            0xC1 => {
                let handle = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(self.native_effect.release_spline(handle))
            }
            // C2/C3 are VM-owned because their target contracts dereference
            // caller memory. Reaching the host path indicates malformed routing.
            0xC2 => {
                let _ = pop_args(stack, 4);
                ethornell_vm::Value::Int(1)
            }
            0xC3 => {
                let _ = pop_args(stack, 3);
                ethornell_vm::Value::Int(4)
            }
            // F0 is VM-owned so it can write the BWEF table and count directly.
            0xF0 => {
                let _ = pop_args(stack, 5);
                ethornell_vm::Value::Int(i32::MIN + 1)
            }
            _ => return None,
        };
        Some(Ok(value))
    }
}

fn is_native_blend_mode(value: i32) -> bool {
    matches!(
        value,
        0x00..=0x09 | 0x20..=0x27 | 0x40 | 0x41 | 0x80 | 0xC0 | 0xC1 | 0xF0 | 0xFF
    )
}

#[cfg(test)]
mod tests {
    use super::{NativeEffectState, PARTICLE_HANDLE_BASE, sample_natural_cubic};

    #[test]
    fn particle_screen_uses_target_tag_and_eight_slots() {
        let mut state = NativeEffectState::default();
        for slot in 0..8 {
            assert_eq!(
                state.allocate_particle_screen(1280, 720),
                Some(PARTICLE_HANDLE_BASE | slot)
            );
        }
        assert_eq!(state.allocate_particle_screen(1, 1), None);
        assert!(state.release_particle_screen(PARTICLE_HANDLE_BASE | 3));
        assert_eq!(
            state.allocate_particle_screen(640, 480),
            Some(PARTICLE_HANDLE_BASE | 3)
        );
    }

    #[test]
    fn spline_matches_target_status_and_natural_cubic_endpoints() {
        let mut state = NativeEffectState::default();
        let handle = state.create_spline();
        assert_eq!(
            state.configure_spline(handle, 1, &[[0, 0, 0], [1, 1, 1]]),
            3
        );
        assert_eq!(state.configure_spline(99, 10, &[[0, 0, 0], [1, 1, 1]]), 1);
        assert_eq!(
            state.configure_spline(handle, 10, &[[0, 0, 0], [10, 20, 30], [20, 40, 60]]),
            0
        );
        assert_eq!(state.sample_spline(handle, 0), Ok([0, 0, 0]));
        assert_eq!(state.sample_spline(handle, 10), Err(4));
        assert!((sample_natural_cubic(&[0.0, 10.0, 20.0], 0.5) - 10.0).abs() < 0.001);
        assert_eq!(state.release_spline(handle), 0);
        assert_eq!(state.release_spline(handle), 1);
    }
}
