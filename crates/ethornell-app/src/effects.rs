use ethornell_vm::Value;
use std::collections::BTreeMap;

const MAX_WAVE_TABLES: i32 = 8;

const RAIN_HANDLE_BASE: i32 = 0xC100_0000u32 as i32;
const MAX_RAIN_EFFECTS: usize = 8;

#[derive(Debug, Default)]
pub(crate) struct RuntimeEffects {
    rain: BTreeMap<i32, RainEffect>,
    wave_tables: BTreeMap<i32, WaveTable>,
    vector_maps: BTreeMap<i32, VectorMap>,
}

impl RuntimeEffects {
    pub(crate) fn configure_compact_wave_table(&mut self, popped: &[Value]) -> bool {
        let values = popped.iter().map(value_to_i32).collect::<Vec<_>>();
        let [slot, period, trail, lead, amplitude] = values.as_slice() else {
            return false;
        };
        if !(0..MAX_WAVE_TABLES).contains(slot) {
            return false;
        }
        self.wave_tables.insert(
            *slot,
            WaveTable::new(*period, *amplitude, *lead, *trail, 1, 1),
        );
        true
    }

    pub(crate) fn call(&mut self, group: u8, id: u16, args: &[Value]) -> EffectCall {
        if group != 0xc0 {
            return EffectCall::Unhandled;
        }
        if id == 0x40 {
            let Some(slot) = (0..MAX_RAIN_EFFECTS)
                .find(|slot| !self.rain.contains_key(&(RAIN_HANDLE_BASE | *slot as i32)))
            else {
                return EffectCall::Handled(Some(0));
            };
            let handle = RAIN_HANDLE_BASE | slot as i32;
            self.rain.insert(handle, RainEffect::new(handle, args));
            return EffectCall::Handled(Some(handle));
        }
        if matches!(id, 0x10 | 0x1a | 0x1b | 0x20) {
            return EffectCall::Handled(None);
        }
        if !(0x42..=0x4e).contains(&id) {
            return EffectCall::Unhandled;
        }

        let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
        let Some(handle) = values
            .iter()
            .copied()
            .find(|value| self.rain.contains_key(value))
        else {
            return EffectCall::Handled(None);
        };
        let effect = self.rain.get_mut(&handle).expect("rain handle disappeared");
        let payload = values
            .iter()
            .copied()
            .filter(|value| *value != handle)
            .collect::<Vec<_>>();
        match id {
            0x42 => effect.initialized = true,
            0x43 => effect.texture = payload.first().copied().unwrap_or(-1),
            0x44 => effect.enabled = payload.first().copied().unwrap_or_default() != 0,
            0x45 => copy_values(&mut effect.bounds, &payload),
            0x46 => effect.drop_width = payload.first().copied().unwrap_or_default(),
            0x47 => effect.drop_height = payload.first().copied().unwrap_or_default(),
            0x48 => effect.drop_color = payload.first().copied().unwrap_or_default(),
            0x49 => effect.density = payload.first().copied().unwrap_or_default(),
            0x4a => effect.speed = payload.first().copied().unwrap_or_default(),
            0x4b => effect.angle = payload.first().copied().unwrap_or_default(),
            0x4c => copy_values(&mut effect.origin, &payload),
            0x4d => copy_values(&mut effect.direction, &payload),
            0x4e => effect.length = payload.first().copied().unwrap_or_default(),
            _ => {}
        }
        EffectCall::Handled(None)
    }

    pub(crate) fn configure_wave_table(&mut self, popped: &[Value]) -> bool {
        let values = popped.iter().map(value_to_i32).collect::<Vec<_>>();
        let [slot, period, amplitude, lead, trail, repeat_x, repeat_y] = values.as_slice() else {
            return false;
        };
        if !(0..MAX_WAVE_TABLES).contains(slot) {
            return false;
        }
        self.wave_tables.insert(
            *slot,
            WaveTable::new(*period, *amplitude, *lead, *trail, *repeat_x, *repeat_y),
        );
        true
    }

    pub(crate) fn create_vector_map(&mut self, bitmap: i32, width: u32, height: u32) {
        self.vector_maps
            .insert(bitmap, VectorMap::blank(width, height));
    }

    pub(crate) fn remove_bitmap(&mut self, bitmap: i32) {
        self.vector_maps.remove(&bitmap);
    }

    pub(crate) fn clear_vector_map(&mut self, bitmap: i32) -> bool {
        let Some(map) = self.vector_maps.get_mut(&bitmap) else {
            return false;
        };
        map.samples.fill(VectorSample::default());
        true
    }

    pub(crate) fn generate_ripple_map(
        &mut self,
        bitmap: i32,
        direction: i32,
        center_x: i32,
        center_y: i32,
        period: i32,
    ) -> bool {
        let Some(map) = self.vector_maps.get_mut(&bitmap) else {
            return false;
        };
        map.generate_radial(direction, center_x, center_y, period);
        true
    }

    pub(crate) fn copy_vector_map(
        &mut self,
        destination: i32,
        source: i32,
        x: i32,
        y: i32,
    ) -> bool {
        let Some(source) = self.vector_maps.get(&source).cloned() else {
            return false;
        };
        let Some(destination) = self.vector_maps.get_mut(&destination) else {
            return false;
        };
        destination.blit(&source, x, y);
        true
    }

    #[cfg(test)]
    fn wave_table(&self, slot: i32) -> Option<&WaveTable> {
        self.wave_tables.get(&slot)
    }

    #[cfg(test)]
    fn vector_map(&self, bitmap: i32) -> Option<&VectorMap> {
        self.vector_maps.get(&bitmap)
    }
}

#[derive(Debug, Clone)]
struct WaveTable {
    samples: Vec<[i16; 2]>,
}

impl WaveTable {
    fn new(
        period: i32,
        amplitude: i32,
        lead: i32,
        trail: i32,
        repeat_x: i32,
        repeat_y: i32,
    ) -> Self {
        let period = period.max(1) as usize;
        let lead = lead.max(0) as usize;
        let trail = trail.max(0) as usize;
        let repeat_x = repeat_x.max(1) as usize;
        let repeat_y = repeat_y.max(1) as usize;
        let cycle_len = period.saturating_mul(4);
        let block_len = cycle_len.saturating_mul(lead.saturating_add(trail).saturating_add(1));
        let capacity = block_len.saturating_mul(repeat_x).saturating_mul(repeat_y);
        let mut samples = Vec::with_capacity(capacity);

        for _ in 0..repeat_y {
            for _ in 1..repeat_x {
                samples.resize(samples.len().saturating_add(block_len), [0, 0]);
            }
            for shift in (1..=lead).rev() {
                append_sine_cycle(&mut samples, cycle_len, amplitude, shift);
            }
            append_sine_cycle(&mut samples, cycle_len, amplitude, 0);
            for shift in 1..=trail {
                append_sine_cycle(&mut samples, cycle_len, amplitude, shift);
            }
        }
        Self { samples }
    }
}

fn append_sine_cycle(samples: &mut Vec<[i16; 2]>, cycle_len: usize, amplitude: i32, shift: usize) {
    let divisor = 2_f64.powi(shift.min(30) as i32);
    for index in 0..cycle_len {
        let phase = index as f64 * std::f64::consts::TAU / cycle_len as f64;
        let value = (phase.sin() * amplitude as f64 / divisor) as i16;
        samples.push([value, value]);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct VectorSample {
    x: i16,
    y: i16,
    phase: u16,
}

#[derive(Debug, Clone)]
struct VectorMap {
    width: u32,
    height: u32,
    samples: Vec<VectorSample>,
}

impl VectorMap {
    fn blank(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            samples: vec![VectorSample::default(); width as usize * height as usize],
        }
    }

    fn generate_radial(&mut self, direction: i32, center_x: i32, center_y: i32, period: i32) {
        let phase_period = (period > 0).then_some(period.saturating_mul(4) as u32);
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                let dx = x - center_x;
                let dy = y - center_y;
                let distance =
                    ((i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)) as f64).sqrt();
                let (vector_x, vector_y) = if distance == 0.0 {
                    (0, 0)
                } else if direction == 0 {
                    (
                        (32767.0 * center_y.saturating_sub(y) as f64 / distance) as i16,
                        (32767.0 * center_x.saturating_sub(x) as f64 / distance) as i16,
                    )
                } else {
                    (
                        (32767.0 * center_x.saturating_sub(x) as f64 / distance) as i16,
                        (32767.0 * center_y.saturating_sub(y) as f64 / distance) as i16,
                    )
                };
                let phase = phase_period
                    .map(|period| ((distance * 4.0) as u32 % period) as u16)
                    .unwrap_or((distance * 4.0) as u16);
                self.samples[(y as usize * self.width as usize) + x as usize] = VectorSample {
                    x: vector_x,
                    y: vector_y,
                    phase,
                };
            }
        }
    }

    fn blit(&mut self, source: &Self, destination_x: i32, destination_y: i32) {
        for source_y in 0..source.height as i32 {
            let target_y = destination_y + source_y;
            if !(0..self.height as i32).contains(&target_y) {
                continue;
            }
            for source_x in 0..source.width as i32 {
                let target_x = destination_x + source_x;
                if !(0..self.width as i32).contains(&target_x) {
                    continue;
                }
                let source_index = source_y as usize * source.width as usize + source_x as usize;
                let target_index = target_y as usize * self.width as usize + target_x as usize;
                self.samples[target_index] = source.samples[source_index];
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum EffectCall {
    Unhandled,
    Handled(Option<i32>),
}

#[derive(Debug)]
struct RainEffect {
    #[allow(dead_code)]
    handle: i32,
    initialized: bool,
    enabled: bool,
    texture: i32,
    bounds: [i32; 6],
    drop_width: i32,
    drop_height: i32,
    drop_color: i32,
    density: i32,
    speed: i32,
    angle: i32,
    origin: [i32; 3],
    direction: [i32; 3],
    length: i32,
}

impl RainEffect {
    fn new(handle: i32, args: &[Value]) -> Self {
        let mut effect = Self {
            handle,
            initialized: false,
            enabled: false,
            texture: -1,
            bounds: [-6000, -4000, -2000, 6000, 4000, 2000],
            drop_width: 0,
            drop_height: 60 << 8,
            drop_color: 350 << 8,
            density: 20,
            speed: 50,
            angle: 1,
            origin: [0; 3],
            direction: [0; 3],
            length: 100,
        };
        let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
        if let Some(value) = values.first() {
            effect.drop_width = *value;
        }
        effect
    }
}

fn value_to_i32(value: &Value) -> i32 {
    match value {
        Value::Int(value) => *value,
        Value::Ptr(value) => *value as i32,
        Value::Func { offset, .. } => *offset as i32,
        Value::Str(_) | Value::Program(_) | Value::None => 0,
    }
}

fn copy_values<const N: usize>(target: &mut [i32; N], values: &[i32]) {
    for (target, value) in target.iter_mut().zip(values.iter().copied()) {
        *target = value;
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeEffects;
    use ethornell_vm::Value;

    #[test]
    fn native_wave_table_uses_reverse_pop_order_and_exact_cycle_length() {
        let mut effects = RuntimeEffects::default();
        let popped = [
            Value::Int(2),
            Value::Int(3),
            Value::Int(64),
            Value::Int(1),
            Value::Int(2),
            Value::Int(1),
            Value::Int(1),
        ];
        assert!(effects.configure_wave_table(&popped));
        let table = effects.wave_table(2).unwrap();
        assert_eq!(table.samples.len(), 3 * 4 * (1 + 2 + 1));
        assert!(table.samples.iter().any(|sample| sample[0] != 0));
    }

    #[test]
    fn format_six_ripple_maps_generate_and_blit_pixels() {
        let mut effects = RuntimeEffects::default();
        effects.create_vector_map(1, 3, 3);
        effects.create_vector_map(2, 4, 4);
        assert!(effects.generate_ripple_map(1, 0, 1, 1, 4));
        assert!(effects.copy_vector_map(2, 1, 1, 1));

        let source = effects.vector_map(1).unwrap();
        let destination = effects.vector_map(2).unwrap();
        assert_eq!(source.samples[4], destination.samples[2 * 4 + 2]);
        assert_ne!(source.samples[0], Default::default());
    }
}
