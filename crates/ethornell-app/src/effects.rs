use ethornell_image::DecodedImage;
use ethornell_vm::Value;
use std::collections::BTreeMap;

const MAX_WAVE_TABLES: i32 = 8;

const RAIN_HANDLE_BASE: i32 = 0xC100_0000u32 as i32;
const MAX_RAIN_EFFECTS: usize = 8;

#[derive(Debug)]
pub(crate) struct RuntimeEffects {
    rain: BTreeMap<i32, RainEffect>,
    rain_step_ms: i32,
    rain_native_step: i32,
    rain_elapsed_ms: i64,
    rain_tick: u64,
    wave_tables: BTreeMap<i32, WaveTable>,
    vector_maps: BTreeMap<i32, VectorMap>,
    displacement_maps: BTreeMap<i32, DisplacementMap>,
}

impl Default for RuntimeEffects {
    fn default() -> Self {
        Self {
            rain: BTreeMap::new(),
            rain_step_ms: 16,
            rain_native_step: 0,
            rain_elapsed_ms: 0,
            rain_tick: 0,
            wave_tables: BTreeMap::new(),
            vector_maps: BTreeMap::new(),
            displacement_maps: BTreeMap::new(),
        }
    }
}

impl RuntimeEffects {
    /// System92:00 receives the target wrapper's reverse-pop order:
    /// [block_count, block_height, amplitude, quarter_period, slot].
    pub(crate) fn configure_compact_wave_table(&mut self, popped: &[Value]) -> bool {
        let values = popped.iter().map(value_to_i32).collect::<Vec<_>>();
        let [block_count, block_height, amplitude, quarter_period, slot] = values.as_slice() else {
            return false;
        };
        if !(0..MAX_WAVE_TABLES).contains(slot) {
            return false;
        }
        self.wave_tables.insert(
            *slot,
            WaveTable::compact(*quarter_period, *amplitude, *block_height, *block_count),
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
        if id == 0x41 {
            let handle = args.first().map(value_to_i32).unwrap_or_default();
            self.rain.remove(&handle);
            return EffectCall::Handled(None);
        }
        if id == 0x4f {
            // Native reverse-pop order is [frequency, native_step_value].
            let frequency = args.first().map(value_to_i32).unwrap_or_default();
            let native_step_value = args.get(1).map(value_to_i32).unwrap_or_default();
            if (1..=1000).contains(&frequency) {
                self.rain_step_ms = 1000 / frequency;
                self.rain_native_step = native_step_value;
                self.rain_elapsed_ms = 0;
            }
            return EffectCall::Handled(None);
        }
        if !(0x42..=0x4e).contains(&id) {
            return EffectCall::Unhandled;
        }

        let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
        // Every rain mutator takes the handle as its first script argument,
        // hence it is last after the native wrapper's reverse pops.
        let Some((&handle, parameters)) = values.split_last() else {
            return EffectCall::Handled(None);
        };
        let Some(effect) = self.rain.get_mut(&handle) else {
            return EffectCall::Handled(None);
        };
        let mut payload = parameters.to_vec();
        payload.reverse();
        match id {
            0x42 => {
                effect.initialized = true;
                effect.drop_count = payload.first().copied().unwrap_or_default().max(0) as usize;
            }
            0x43 => effect.texture = payload.first().copied().unwrap_or(-1),
            0x44 => effect.enabled = payload.first().copied().unwrap_or_default() != 0,
            0x45 => {
                if let [x, y, z, blend, alpha] = payload.as_slice() {
                    effect.display = [*x, *y, *z, *blend, *alpha];
                }
            }
            0x46 => copy_values(&mut effect.volume_bounds, &payload),
            0x47 => effect.drop_width_fixed = payload.first().copied().unwrap_or_default() << 8,
            0x48 => effect.drop_height_fixed = payload.first().copied().unwrap_or_default() << 8,
            0x49 => effect.color = payload.first().copied().unwrap_or_default(),
            0x4a => effect.density = payload.first().copied().unwrap_or_default(),
            0x4b => effect.speed = payload.first().copied().unwrap_or_default(),
            0x4c => copy_values(&mut effect.origin, &payload),
            0x4d => copy_values(&mut effect.direction, &payload),
            0x4e => effect.length = payload.first().copied().unwrap_or_default(),
            _ => {}
        }
        EffectCall::Handled(None)
    }

    pub(crate) fn tick(&mut self, elapsed_ms: u64) {
        self.rain_elapsed_ms = self
            .rain_elapsed_ms
            .saturating_add(i64::try_from(elapsed_ms).unwrap_or(i64::MAX));
        let interval = i64::from(self.rain_step_ms.max(1));
        let steps = self.rain_elapsed_ms / interval;
        if steps > 0 {
            self.rain_elapsed_ms %= interval;
            self.rain_tick = self
                .rain_tick
                .wrapping_add(u64::try_from(steps).unwrap_or(u64::MAX));
        }
    }

    pub(crate) fn rain_draw_instances(&self) -> Vec<RainDrawInstance> {
        let mut instances = Vec::new();
        for effect in self.rain.values() {
            if !effect.initialized || !effect.enabled || effect.texture < 0 {
                continue;
            }
            let viewport_width = effect.viewport[0].max(1) as f32;
            let viewport_height = effect.viewport[1].max(1) as f32;
            let width = (effect.drop_width_fixed as f32 / 256.0).abs().max(1.0);
            let height = (effect.drop_height_fixed as f32 / 256.0).abs().max(1.0);
            let count = effect
                .drop_count
                .max(effect.density.max(0) as usize)
                .min(512);
            let speed = effect.speed.max(1) as u64;
            for index in 0..count {
                let seed = mix64(effect.handle as u64 ^ (index as u64).wrapping_mul(0x9E37_79B9));
                let x_fraction = (seed & 0xffff) as f32 / 65535.0;
                let start = ((seed >> 16) & 0xffff) as u64;
                let travel = self
                    .rain_tick
                    .wrapping_mul(speed)
                    .wrapping_add(start)
                    .wrapping_add(index as u64 * 13);
                let y_fraction = (travel % 65536) as f32 / 65535.0;
                let direction_x = effect.direction[0] as f32 / 256.0;
                let direction_y = effect.direction[1] as f32 / 256.0;
                instances.push(RainDrawInstance {
                    resource_id: effect.texture,
                    x: effect.display[0] as f32
                        + effect.origin[0] as f32 / 256.0
                        + x_fraction * viewport_width
                        + direction_x * y_fraction,
                    y: effect.display[1] as f32
                        + effect.origin[1] as f32 / 256.0
                        + y_fraction * viewport_height
                        + direction_y * y_fraction,
                    width,
                    height,
                    opacity: (1.0 - effect.display[4].clamp(0, 256) as f32 / 256.0).clamp(0.0, 1.0),
                    z: effect.display[2],
                    blend_mode: effect.display[3],
                    order_serial: ((effect.handle as u32 as u64) << 16) | index as u64,
                });
            }
        }
        instances
    }

    /// System92:01 reverse-pop order:
    /// [block_count, block_height, trail_levels, lead_levels,
    ///  amplitude, quarter_period, slot].
    pub(crate) fn configure_wave_table(&mut self, popped: &[Value]) -> bool {
        let values = popped.iter().map(value_to_i32).collect::<Vec<_>>();
        let [block_count, block_height, trail_levels, lead_levels, amplitude, quarter_period, slot] =
            values.as_slice()
        else {
            return false;
        };
        if !(0..MAX_WAVE_TABLES).contains(slot) {
            return false;
        }
        self.wave_tables.insert(
            *slot,
            WaveTable::extended(
                *quarter_period,
                *amplitude,
                *lead_levels,
                *trail_levels,
                *block_height,
                *block_count,
            ),
        );
        true
    }

    pub(crate) fn create_vector_map(&mut self, bitmap: i32, width: u32, height: u32) {
        self.vector_maps
            .insert(bitmap, VectorMap::blank(width, height));
    }

    pub(crate) fn create_displacement_map(&mut self, bitmap: i32, width: u32, height: u32) {
        self.displacement_maps
            .insert(bitmap, DisplacementMap::blank(width, height));
    }

    pub(crate) fn remove_bitmap(&mut self, bitmap: i32) {
        self.vector_maps.remove(&bitmap);
        self.displacement_maps.remove(&bitmap);
    }

    pub(crate) fn clone_bitmap(&mut self, destination: i32, source: i32) {
        match self.vector_maps.get(&source).cloned() {
            Some(map) => {
                self.vector_maps.insert(destination, map);
            }
            None => {
                self.vector_maps.remove(&destination);
            }
        }
        match self.displacement_maps.get(&source).cloned() {
            Some(map) => {
                self.displacement_maps.insert(destination, map);
            }
            None => {
                self.displacement_maps.remove(&destination);
            }
        }
    }

    pub(crate) fn clear_vector_map(&mut self, bitmap: i32) -> bool {
        let mut cleared = false;
        if let Some(map) = self.vector_maps.get_mut(&bitmap) {
            map.samples.fill(VectorSample::default());
            cleared = true;
        }
        if let Some(map) = self.displacement_maps.get_mut(&bitmap) {
            map.samples.fill([0, 0]);
            cleared = true;
        }
        cleared
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
        if !(0..=1).contains(&direction) {
            return false;
        }
        map.generate_radial(direction, center_x, center_y, period);
        true
    }

    pub(crate) fn generate_axis_vector_map(&mut self, bitmap: i32, mode: i32) -> bool {
        let Some(map) = self.vector_maps.get_mut(&bitmap) else {
            return false;
        };
        if !(0..=3).contains(&mode) {
            return false;
        }
        map.generate_axis(mode);
        true
    }

    pub(crate) fn copy_vector_map(
        &mut self,
        destination: i32,
        source: i32,
        x: i32,
        y: i32,
    ) -> bool {
        let mut copied = false;
        if let Some(source) = self.vector_maps.get(&source).cloned() {
            if let Some(destination) = self.vector_maps.get_mut(&destination) {
                destination.blit(&source, x, y);
                copied = true;
            }
        }
        if let Some(source) = self.displacement_maps.get(&source).cloned() {
            if let Some(destination) = self.displacement_maps.get_mut(&destination) {
                destination.blit(&source, x, y);
                copied = true;
            }
        }
        copied
    }

    pub(crate) fn configure_displacement_map(&mut self, effect: u16, popped: &[Value]) -> bool {
        let mut values = popped.iter().map(value_to_i32).collect::<Vec<_>>();
        values.reverse();
        let Some((&bitmap, parameters)) = values.split_first() else {
            return false;
        };
        let Some(map) = self.displacement_maps.get_mut(&bitmap) else {
            return false;
        };
        match effect {
            0x10 => map.zoom(parameters),
            0x11 => map.diffuse(parameters),
            0x12 => map.radial_cosine(parameters),
            0x13 => map.perspective_bend(parameters),
            0x14 => map.curvature(parameters),
            0x15 => map.radial_lens(parameters),
            0x16 => map.wave(parameters),
            0x17 => map.radial_warp(parameters),
            _ => return false,
        }
        true
    }

    pub(crate) fn warp_displacement_map(
        &self,
        bitmap: i32,
        source: &DecodedImage,
    ) -> Option<DecodedImage> {
        self.displacement_maps.get(&bitmap)?.warp(source)
    }

    #[cfg(test)]
    fn wave_table(&self, slot: i32) -> Option<&WaveTable> {
        self.wave_tables.get(&slot)
    }

    #[cfg(test)]
    fn vector_map(&self, bitmap: i32) -> Option<&VectorMap> {
        self.vector_maps.get(&bitmap)
    }

    #[cfg(test)]
    fn displacement_map(&self, bitmap: i32) -> Option<&DisplacementMap> {
        self.displacement_maps.get(&bitmap)
    }
}

#[derive(Debug, Clone)]
struct WaveTable {
    samples: Vec<[i16; 2]>,
}

impl WaveTable {
    fn compact(quarter_period: i32, amplitude: i32, block_height: i32, block_count: i32) -> Self {
        let cycle_len = quarter_period.max(1) as usize * 4;
        let block_height = block_height.max(1) as usize;
        let block_count = block_count.max(1) as usize;
        let mut samples = Vec::with_capacity(
            cycle_len
                .saturating_mul(block_height)
                .saturating_mul(block_count),
        );
        for _ in 0..block_count {
            append_sine_cycle(&mut samples, cycle_len, amplitude, 0);
            for _ in 1..block_height {
                samples.resize(samples.len().saturating_add(cycle_len), [0, 0]);
            }
        }
        Self { samples }
    }

    fn extended(
        quarter_period: i32,
        amplitude: i32,
        lead_levels: i32,
        trail_levels: i32,
        block_height: i32,
        block_count: i32,
    ) -> Self {
        let cycle_len = quarter_period.max(1) as usize * 4;
        let lead_levels = lead_levels.max(0) as usize;
        let trail_levels = trail_levels.max(0) as usize;
        let block_height = block_height.max(1) as usize;
        let block_count = block_count.max(1) as usize;
        let cycles_per_row = lead_levels.saturating_add(trail_levels).saturating_add(1);
        let row_len = cycle_len.saturating_mul(cycles_per_row);
        let mut samples = Vec::with_capacity(
            row_len
                .saturating_mul(block_height)
                .saturating_mul(block_count),
        );
        for _ in 0..block_count {
            for _ in 1..block_height {
                samples.resize(samples.len().saturating_add(row_len), [0, 0]);
            }
            for shift in (1..=lead_levels).rev() {
                append_sine_cycle(&mut samples, cycle_len, amplitude, shift);
            }
            append_sine_cycle(&mut samples, cycle_len, amplitude, 0);
            for shift in 1..=trail_levels {
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

#[derive(Debug, Clone)]
struct DisplacementMap {
    width: u32,
    height: u32,
    samples: Vec<[i16; 2]>,
}

impl DisplacementMap {
    fn blank(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            samples: vec![[0, 0]; width as usize * height as usize],
        }
    }

    fn zoom(&mut self, parameters: &[i32]) {
        let [center_x, center_y, width_scale, height_scale, ..] = parameters else {
            return;
        };
        let width = self.width.max(1) as i64;
        let height = self.height.max(1) as i64;
        for y in 0..self.height as i64 {
            for x in 0..self.width as i64 {
                let dx = 16 * (i64::from(*center_x) - x) + 16 * x * i64::from(*width_scale) / width;
                let dy =
                    16 * (i64::from(*center_y) - y) + 16 * y * i64::from(*height_scale) / height;
                self.set(x as u32, y as u32, dx, dy);
            }
        }
    }

    fn diffuse(&mut self, parameters: &[i32]) {
        let Some(radius) = parameters.first().copied() else {
            return;
        };
        let radius = radius.unsigned_abs().min(i16::MAX as u32) as i32;
        let span = radius.saturating_mul(2).saturating_add(1).max(1) as u32;
        for y in 0..self.height {
            for x in 0..self.width {
                let seed = x
                    .wrapping_mul(0x9E37_79B9)
                    .wrapping_add(y.wrapping_mul(0x85EB_CA6B));
                let dx = radius - (mix32(seed) % span) as i32;
                let dy = radius - (mix32(seed ^ 0xA5A5_5A5A) % span) as i32;
                self.set(x, y, i64::from(dx), i64::from(dy));
            }
        }
    }

    fn radial_cosine(&mut self, parameters: &[i32]) {
        let [center_x, center_y, wavelength, phase, amplitude, ..] = parameters else {
            return;
        };
        if *wavelength == 0 {
            return;
        }
        let mut scalar = vec![0_i64; (self.width as usize + 1) * (self.height as usize + 1)];
        let stride = self.width as usize + 1;
        for y in 0..=self.height {
            for x in 0..=self.width {
                let dx = f64::from(*center_x) - f64::from(x);
                let dy = f64::from(*center_y) - f64::from(y);
                let distance = (dx * dx + dy * dy).sqrt();
                scalar[y as usize * stride + x as usize] = ((1.0
                    - ((distance - f64::from(*phase)) * std::f64::consts::TAU
                        / f64::from(*wavelength))
                    .cos())
                    * f64::from(*amplitude))
                    as i64;
            }
        }
        self.gradient_from_scalar(&scalar, 65_536);
    }

    fn perspective_bend(&mut self, parameters: &[i32]) {
        let [center_x, center_y, angle_fixed, perspective, ..] = parameters else {
            return;
        };
        let angle = f64::from(*angle_fixed) * std::f64::consts::PI / (180.0 * 65_536.0);
        let sin_angle = angle.sin();
        let cos_angle = angle.cos();
        if cos_angle <= 0.0 {
            self.samples.fill([0, 0]);
            return;
        }
        let projected_height = f64::from(self.height) / cos_angle;
        for y in 0..self.height {
            let local_y = f64::from(y as i32 - *center_y);
            for x in 0..self.width {
                let local_x = f64::from(x as i32 - *center_x);
                let distance = (local_x * local_x + local_y * local_y).sqrt();
                let polar = local_y.atan2(local_x);
                let source_x = (1.0 - (180.0 - polar.to_degrees()) / 360.0)
                    * f64::from(16 * self.width.saturating_sub(1))
                    / 16.0;
                let denominator = f64::from(*perspective) + distance * sin_angle;
                let source_y = if denominator.abs() < f64::EPSILON {
                    f64::from(y)
                } else {
                    (projected_height + f64::from(*perspective)) * distance / denominator
                };
                self.set(
                    x,
                    y,
                    ((source_x - f64::from(x)) * 16.0) as i64,
                    ((source_y - f64::from(y)) * 16.0) as i64,
                );
            }
        }
    }

    fn curvature(&mut self, parameters: &[i32]) {
        let [center_x, center_y, bend_angle_fixed, radius, ..] = parameters else {
            return;
        };
        if *bend_angle_fixed == 0 || *radius <= 0 {
            return;
        }
        let angle = f64::from(*bend_angle_fixed) * std::f64::consts::PI / (180.0 * 65_536.0);
        let sin_angle = angle.sin();
        let cos_angle = angle.cos();
        let radius = f64::from(*radius);
        for y in 0..self.height {
            let local_y = f64::from(y as i32 - *center_y);
            for x in 0..self.width {
                let local_x = f64::from(x as i32 - *center_x);
                let distance = (local_x * local_x + local_y * local_y).sqrt();
                if distance <= 0.0 || distance >= radius {
                    self.set(x, y, 0, 0);
                    continue;
                }
                let q = distance * sin_angle / radius;
                if q.abs() >= 1.0 {
                    self.set(x, y, 0, 0);
                    continue;
                }
                let root = (1.0 - q * q).sqrt();
                if root <= f64::EPSILON {
                    self.set(x, y, 0, 0);
                    continue;
                }
                let factor = (root - cos_angle) * q / root;
                self.set(
                    x,
                    y,
                    (-local_x * 16.0 * factor) as i64,
                    (-local_y * 16.0 * factor) as i64,
                );
            }
        }
    }

    fn radial_lens(&mut self, parameters: &[i32]) {
        let [center_x, center_y, strength, radius, ..] = parameters else {
            return;
        };
        if *strength == 0 || *radius <= 0 {
            return;
        }
        let mut scalar = vec![0_i64; (self.width as usize + 1) * (self.height as usize + 1)];
        let stride = self.width as usize + 1;
        for y in 0..=self.height {
            for x in 0..=self.width {
                let dx = f64::from(*center_x) - f64::from(x);
                let dy = f64::from(*center_y) - f64::from(y);
                let distance = (dx * dx + dy * dy).sqrt();
                scalar[y as usize * stride + x as usize] = if distance >= f64::from(*radius) {
                    0
                } else {
                    (4_194_304.0
                        - (distance * std::f64::consts::PI / (2.0 * f64::from(*radius))).sin()
                            * 4_194_304.0) as i64
                };
            }
        }
        self.gradient_from_scalar(&scalar, (0x4000_0000_i64 / i64::from(*strength)).max(1));
    }

    fn wave(&mut self, parameters: &[i32]) {
        let [x_period, x_phase, x_amplitude, y_period, y_phase, y_amplitude, ..] = parameters
        else {
            return;
        };
        let (x_period, x_amplitude) = if *x_period == 0 {
            (1, 0)
        } else {
            (*x_period, *x_amplitude)
        };
        let (y_period, y_amplitude) = if *y_period == 0 {
            (1, 0)
        } else {
            (*y_period, *y_amplitude)
        };
        for y in 0..self.height {
            let dx = ((f64::from(y as i32 + *x_phase) * std::f64::consts::TAU
                / f64::from(x_period))
            .sin()
                * f64::from(x_amplitude)
                * 16.0) as i64;
            for x in 0..self.width {
                let dy = ((f64::from(x as i32 + *y_phase) * std::f64::consts::TAU
                    / f64::from(y_period))
                .sin()
                    * f64::from(y_amplitude)
                    * 16.0) as i64;
                self.set(x, y, dx, dy);
            }
        }
    }

    fn radial_warp(&mut self, parameters: &[i32]) {
        let [source_origin_x, source_origin_y, warp_center_x, warp_center_y, radius_bias, ..] =
            parameters
        else {
            return;
        };
        let radius = (((*warp_center_x - *source_origin_x) as f64).powi(2)
            + ((*warp_center_y - *source_origin_y) as f64).powi(2))
        .sqrt()
            + f64::from(*radius_bias);
        if radius <= 0.0 {
            return;
        }
        for y in 0..self.height {
            let local_y = f64::from(y as i32 - *warp_center_y);
            for x in 0..self.width {
                let local_x = f64::from(x as i32 - *warp_center_x);
                let distance = (local_x * local_x + local_y * local_y).sqrt();
                let radial_scale = (distance * std::f64::consts::FRAC_PI_2 / radius).sin();
                let source_x = radial_scale * local_x + f64::from(*source_origin_x);
                let source_y = local_y + f64::from(*source_origin_y);
                self.set(
                    x,
                    y,
                    ((source_x - f64::from(x)) * 16.0) as i64,
                    ((source_y - f64::from(y)) * 16.0) as i64,
                );
            }
        }
    }

    fn gradient_from_scalar(&mut self, scalar: &[i64], divisor: i64) {
        let stride = self.width as usize + 1;
        for y in 0..self.height {
            for x in 0..self.width {
                let index = y as usize * stride + x as usize;
                let current = scalar[index];
                let right = scalar[index + 1];
                let below = scalar[index + stride];
                let dx =
                    (right.saturating_mul(right) - current.saturating_mul(current)) / (2 * divisor);
                let dy =
                    (below.saturating_mul(below) - current.saturating_mul(current)) / (2 * divisor);
                self.set(
                    x,
                    y,
                    dx.clamp(-i64::from(i16::MAX), i64::from(i16::MAX)),
                    dy.clamp(-i64::from(i16::MAX), i64::from(i16::MAX)),
                );
            }
        }
    }

    fn set(&mut self, x: u32, y: u32, dx: i64, dy: i64) {
        self.samples[y as usize * self.width as usize + x as usize] = [
            dx.clamp(i16::MIN as i64, i16::MAX as i64) as i16,
            dy.clamp(i16::MIN as i64, i16::MAX as i64) as i16,
        ];
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
                self.samples[target_y as usize * self.width as usize + target_x as usize] =
                    source.samples[source_y as usize * source.width as usize + source_x as usize];
            }
        }
    }

    fn warp(&self, source: &DecodedImage) -> Option<DecodedImage> {
        if source.width == 0 || source.height == 0 {
            return None;
        }
        let width = self.width.min(source.width);
        let height = self.height.min(source.height);
        let mut output = DecodedImage {
            width,
            height,
            rgba: vec![0; width as usize * height as usize * 4],
        };
        for y in 0..height {
            for x in 0..width {
                let [dx, dy] = self.samples[y as usize * self.width as usize + x as usize];
                let source_x =
                    (x as i32 + i32::from(dx) / 16).clamp(0, source.width as i32 - 1) as u32;
                let source_y =
                    (y as i32 + i32::from(dy) / 16).clamp(0, source.height as i32 - 1) as u32;
                let source_offset =
                    (source_y as usize * source.width as usize + source_x as usize) * 4;
                let destination_offset = (y as usize * width as usize + x as usize) * 4;
                output.rgba[destination_offset..destination_offset + 4]
                    .copy_from_slice(&source.rgba[source_offset..source_offset + 4]);
            }
        }
        Some(output)
    }
}

fn mix32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7FEB_352D);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846C_A68B);
    value ^ (value >> 16)
}

impl VectorMap {
    fn blank(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            samples: vec![VectorSample::default(); width as usize * height as usize],
        }
    }

    fn generate_radial(&mut self, mode: i32, center_x: i32, center_y: i32, period: i32) {
        let divisor = if period > 0 {
            period.saturating_mul(4) as u32
        } else {
            u32::MAX
        };
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                let dx = x - center_x;
                let dy = y - center_y;
                let distance_sq = i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy);
                let distance = (distance_sq as f64).sqrt();
                let (base_x, base_y) = if distance == 0.0 {
                    (0_i16, 0_i16)
                } else {
                    (
                        (f64::from(dx) * 32767.0 / distance) as i16,
                        (f64::from(dy) * 32767.0 / distance) as i16,
                    )
                };
                let (vector_x, vector_y) = if mode == 0 {
                    (base_x, base_y)
                } else {
                    (base_y, base_x)
                };
                let raw_phase = (distance * 4.0) as u32;
                let phase = (raw_phase % divisor) as u16;
                self.samples[y as usize * self.width as usize + x as usize] = VectorSample {
                    x: vector_x,
                    y: vector_y,
                    phase,
                };
            }
        }
    }

    fn generate_axis(&mut self, mode: i32) {
        for y in 0..self.height {
            for x in 0..self.width {
                let (vector_x, vector_y, phase_axis) = match mode {
                    0 => (i16::MAX, 0, y),
                    1 => (0, i16::MAX, x),
                    2 => (i16::MAX, 0, x),
                    3 => (0, i16::MAX, y),
                    _ => unreachable!("validated System92:11 mode"),
                };
                self.samples[y as usize * self.width as usize + x as usize] = VectorSample {
                    x: vector_x,
                    y: vector_y,
                    phase: phase_axis.wrapping_mul(4) as u16,
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct RainDrawInstance {
    pub(crate) resource_id: i32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) opacity: f32,
    pub(crate) z: i32,
    pub(crate) blend_mode: i32,
    pub(crate) order_serial: u64,
}

#[derive(Debug)]
struct RainEffect {
    handle: i32,
    initialized: bool,
    enabled: bool,
    texture: i32,
    viewport: [i32; 2],
    display: [i32; 5],
    volume_bounds: [i32; 6],
    drop_count: usize,
    drop_width_fixed: i32,
    drop_height_fixed: i32,
    color: i32,
    density: i32,
    speed: i32,
    origin: [i32; 3],
    direction: [i32; 3],
    length: i32,
}

impl RainEffect {
    fn new(handle: i32, args: &[Value]) -> Self {
        let mut creation = args.iter().map(value_to_i32).collect::<Vec<_>>();
        creation.reverse();
        let mut effect = Self {
            handle,
            initialized: false,
            enabled: false,
            texture: -1,
            viewport: [0; 2],
            display: [0, 0, 0, 128, 0],
            volume_bounds: [-6000, -4000, -2000, 6000, 4000, 2000],
            drop_count: 0,
            drop_width_fixed: 60 << 8,
            drop_height_fixed: 350 << 8,
            color: -1,
            density: 20,
            speed: 50,
            origin: [0; 3],
            direction: [0; 3],
            length: 100,
        };
        copy_values(&mut effect.viewport, &creation);
        effect
    }
}

fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
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
    use super::{RuntimeEffects, VectorSample};
    use ethornell_image::DecodedImage;
    use ethornell_vm::Value;

    #[test]
    fn rain_calls_preserve_script_argument_order_and_equal_handle_values() {
        let mut effects = RuntimeEffects::default();
        let handle = match effects.call(0xc0, 0x40, &[Value::Int(720), Value::Int(1280)]) {
            super::EffectCall::Handled(Some(handle)) => handle,
            result => panic!("rain creation failed: {result:?}"),
        };
        let bounds = [-10_000, -4_000, -2_000, 10_000, 4_000, 16_000];
        let mut popped = bounds
            .iter()
            .rev()
            .copied()
            .map(Value::Int)
            .collect::<Vec<_>>();
        popped.push(Value::Int(handle));
        assert!(matches!(
            effects.call(0xc0, 0x46, &popped),
            super::EffectCall::Handled(None)
        ));
        let rain = effects.rain.get(&handle).unwrap();
        assert_eq!(rain.viewport, [1280, 720]);
        assert_eq!(rain.volume_bounds, bounds);
    }

    #[test]
    fn native_wave_table_uses_reverse_pop_order_and_exact_cycle_length() {
        let mut effects = RuntimeEffects::default();
        // Native reverse-pop order: block_count, block_height,
        // trail_levels, lead_levels, amplitude, quarter_period, slot.
        let popped = [
            Value::Int(2),
            Value::Int(3),
            Value::Int(1),
            Value::Int(2),
            Value::Int(64),
            Value::Int(1),
            Value::Int(2),
        ];
        assert!(effects.configure_wave_table(&popped));
        let table = effects.wave_table(2).unwrap();
        assert_eq!(table.samples.len(), 2 * 3 * 4 * (2 + 1 + 1));
        assert!(table.samples.iter().any(|sample| sample[0] != 0));
    }

    #[test]
    fn compact_wave_table_uses_target_block_layout() {
        let mut effects = RuntimeEffects::default();
        let popped = [
            Value::Int(2),
            Value::Int(3),
            Value::Int(64),
            Value::Int(1),
            Value::Int(4),
        ];
        assert!(effects.configure_compact_wave_table(&popped));
        let table = effects.wave_table(4).unwrap();
        assert_eq!(table.samples.len(), 2 * 3 * 4);
        assert!(table.samples[..4].iter().any(|sample| sample[0] != 0));
        assert!(table.samples[4..12].iter().all(|sample| *sample == [0, 0]));
    }

    #[test]
    fn format_six_axis_modes_match_target_components_and_phase_axes() {
        let mut effects = RuntimeEffects::default();
        effects.create_vector_map(3, 2, 2);
        assert!(effects.generate_axis_vector_map(3, 1));
        let map = effects.vector_map(3).unwrap();
        assert_eq!(
            map.samples[0],
            VectorSample {
                x: 0,
                y: 32767,
                phase: 0
            }
        );
        assert_eq!(
            map.samples[1],
            VectorSample {
                x: 0,
                y: 32767,
                phase: 4
            }
        );
        assert_eq!(
            map.samples[2],
            VectorSample {
                x: 0,
                y: 32767,
                phase: 0
            }
        );
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

    #[test]
    fn format_four_curvature_and_radial_warp_are_registered() {
        let mut effects = RuntimeEffects::default();
        effects.create_displacement_map(9, 5, 5);
        let curvature = [
            Value::Int(8),
            Value::Int(45 << 16),
            Value::Int(2),
            Value::Int(2),
            Value::Int(9),
        ];
        assert!(effects.configure_displacement_map(0x14, &curvature));
        assert!(effects
            .displacement_map(9)
            .unwrap()
            .samples
            .iter()
            .any(|sample| *sample != [0, 0]));

        let radial_warp = [
            Value::Int(4),
            Value::Int(3),
            Value::Int(3),
            Value::Int(1),
            Value::Int(1),
            Value::Int(9),
        ];
        assert!(effects.configure_displacement_map(0x17, &radial_warp));
        assert!(effects
            .displacement_map(9)
            .unwrap()
            .samples
            .iter()
            .any(|sample| *sample != [0, 0]));
    }

    #[test]
    fn format_four_clone_replaces_destination_map() {
        let mut effects = RuntimeEffects::default();
        effects.create_displacement_map(3, 2, 1);
        effects.create_displacement_map(4, 1, 1);
        let zoom = [
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(3),
        ];
        assert!(effects.configure_displacement_map(0x10, &zoom));
        effects.clone_bitmap(4, 3);
        assert_eq!(
            effects.displacement_map(4).unwrap().samples,
            effects.displacement_map(3).unwrap().samples
        );
    }

    #[test]
    fn format_four_zoom_map_warps_the_source_in_sixteenth_pixels() {
        let mut effects = RuntimeEffects::default();
        effects.create_displacement_map(7, 3, 1);
        let popped = [
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(7),
        ];
        assert!(effects.configure_displacement_map(0x10, &popped));
        assert_eq!(
            effects.displacement_map(7).unwrap().samples,
            vec![[0, 0], [-16, 0], [-32, 0]]
        );

        let source = DecodedImage {
            width: 3,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255],
        };
        let warped = effects.warp_displacement_map(7, &source).unwrap();
        assert_eq!(
            warped.rgba,
            vec![255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255]
        );
    }
}
