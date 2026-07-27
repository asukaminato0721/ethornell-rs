use std::cmp::Reverse;
use std::collections::BTreeMap;

use ethornell_image::DecodedImage;

pub(crate) const NATIVE_DISPLAY_Z: i32 = 100;
// The stock sysprg modules query slot 3790 as the host-provided screen bitmap
// without creating it through 90:11. The native bitmap manager supplies this
// slot before BP execution; save, message, and system scripts all depend on it.
pub(crate) const NATIVE_SCREEN_BITMAP: i32 = 3790;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeGraphObjectProperties {
    pub(crate) blend_mode: i32,
    pub(crate) alpha_parameter: i32,
    pub(crate) mask_alpha: i32,
    pub(crate) alpha_multiplier: i32,
    pub(crate) format_resource: Option<i32>,
    pub(crate) properties: BTreeMap<u32, (i32, i32)>,
}

impl Default for RuntimeGraphObjectProperties {
    fn default() -> Self {
        Self {
            // CDspObj::CDspObj (sub_41A400) installs these exact defaults.
            blend_mode: 128,
            alpha_parameter: 0,
            mask_alpha: 0,
            alpha_multiplier: 256,
            format_resource: None,
            properties: BTreeMap::new(),
        }
    }
}

impl RuntimeGraphObjectProperties {
    pub(crate) fn set_property(&mut self, property: u32, value: i32, extra: i32) {
        match property {
            1 => self.blend_mode = value,
            2 => self.alpha_parameter = value,
            _ => {
                self.properties.insert(property, (value, extra));
            }
        }
    }

    pub(crate) fn opacity(&self) -> f32 {
        // CDspObj's drawable predicate (sub_41AE30) rejects a fully masked
        // object before asking GetMaskAlpha for its blend-specific value.
        if self.mask_alpha >= 256 {
            return 0.0;
        }
        // CDspObj::GetMaskAlpha (sub_41B770) returns transparency in 1/256
        // units. Preserve its integer arithmetic before converting for wgpu.
        let transparency = match self.blend_mode {
            1 | 0x20..=0x24 => {
                256 - ((self.alpha_multiplier
                    * (256 - self.alpha_parameter)
                    * (256 - self.mask_alpha))
                    >> 16)
            }
            2 | 3 | 4 | 0xc0 | 0xc1 => {
                (self.alpha_parameter * self.alpha_multiplier * (256 - self.mask_alpha)) >> 16
            }
            _ => self.alpha_parameter,
        };
        (1.0 - transparency as f32 / 256.0).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeGraphResource {
    pub(crate) key: String,
    pub(crate) source_rect: Option<RuntimeClipRect>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeGraphTransitionNode {
    pub(crate) primary_resource: i32,
    pub(crate) secondary_resource: i32,
    pub(crate) alpha_parameter: i32,
}

impl RuntimeGraphResource {
    pub(crate) fn whole(key: String) -> Self {
        Self {
            key,
            source_rect: None,
        }
    }

    pub(crate) fn subregion(&self, x: i32, y: i32, width: i32, height: i32) -> Self {
        let (base_x, base_y) = self
            .source_rect
            .map(|rect| (rect.x, rect.y))
            .unwrap_or_default();
        Self {
            key: self.key.clone(),
            source_rect: Some(RuntimeClipRect {
                x: base_x + x as f32,
                y: base_y + y as f32,
                width: width.max(0) as f32,
                height: height.max(0) as f32,
            }),
        }
    }
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
    pub(crate) transform_z: i32,
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

    pub(crate) fn screen_z(&self) -> i32 {
        self.z.saturating_add(self.transform_z)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeSurface {
    pub(crate) id: i32,
    pub(crate) display_attached: bool,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) parent_surface: Option<i32>,
    pub(crate) local_x: f32,
    pub(crate) local_y: f32,
    pub(crate) viewport_x: f32,
    pub(crate) viewport_y: f32,
    pub(crate) viewport_width: f32,
    pub(crate) viewport_height: f32,
    pub(crate) resource_id: Option<i32>,
    pub(crate) enabled: bool,
    pub(crate) opacity: f32,
    pub(crate) z: i32,
}

impl RuntimeSurface {
    pub(crate) fn bitmap(id: i32, width: f32, height: f32) -> Self {
        Self {
            id,
            display_attached: false,
            width,
            height,
            x: 0.0,
            y: 0.0,
            parent_surface: None,
            local_x: 0.0,
            local_y: 0.0,
            viewport_x: 0.0,
            viewport_y: 0.0,
            viewport_width: width,
            viewport_height: height,
            resource_id: None,
            enabled: true,
            opacity: 1.0,
            z: NATIVE_DISPLAY_Z,
        }
    }

    pub(crate) fn display(id: i32, width: f32, height: f32) -> Self {
        Self {
            display_attached: true,
            ..Self::bitmap(id, width, height)
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

pub(crate) fn native_draw_order(item: &RuntimeGraphDrawItem) -> (i32, Reverse<i32>) {
    (item.z, Reverse(item.hit_id))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeClipRect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

pub(crate) fn crop_decoded_image(image: &DecodedImage, region: RuntimeClipRect) -> DecodedImage {
    let x = region.x.max(0.0) as u32;
    let y = region.y.max(0.0) as u32;
    let width = (region.width.max(0.0) as u32).min(image.width.saturating_sub(x));
    let height = (region.height.max(0.0) as u32).min(image.height.saturating_sub(y));
    let mut cropped = DecodedImage {
        width,
        height,
        rgba: vec![0; width as usize * height as usize * 4],
    };
    for row in 0..height as usize {
        let src_start = ((y as usize + row) * image.width as usize + x as usize) * 4;
        let dst_start = row * width as usize * 4;
        let byte_len = width as usize * 4;
        cropped.rgba[dst_start..dst_start + byte_len]
            .copy_from_slice(&image.rgba[src_start..src_start + byte_len]);
    }
    cropped
}

pub(crate) fn blit_decoded_image(
    destination: &mut DecodedImage,
    source: &DecodedImage,
    destination_x: i32,
    destination_y: i32,
    mode: i32,
) {
    for source_y in 0..source.height as i32 {
        let target_y = destination_y + source_y;
        if !(0..destination.height as i32).contains(&target_y) {
            continue;
        }
        for source_x in 0..source.width as i32 {
            let target_x = destination_x + source_x;
            if !(0..destination.width as i32).contains(&target_x) {
                continue;
            }
            let source_index = (source_y as usize * source.width as usize + source_x as usize) * 4;
            let target_index =
                (target_y as usize * destination.width as usize + target_x as usize) * 4;
            let source_pixel = &source.rgba[source_index..source_index + 4];
            let target_pixel = &mut destination.rgba[target_index..target_index + 4];
            if mode == 128 {
                target_pixel.copy_from_slice(source_pixel);
                continue;
            }

            let source_alpha = source_pixel[3] as u32;
            let inverse_alpha = 255 - source_alpha;
            for channel in 0..3 {
                target_pixel[channel] = ((source_pixel[channel] as u32 * source_alpha
                    + target_pixel[channel] as u32 * inverse_alpha
                    + 127)
                    / 255) as u8;
            }
            target_pixel[3] = (source_alpha + (target_pixel[3] as u32 * inverse_alpha + 127) / 255)
                .min(255) as u8;
        }
    }
}

pub(crate) fn blit_decoded_image_parameter(
    destination: &mut DecodedImage,
    source: &DecodedImage,
    destination_x: i32,
    destination_y: i32,
    mode: i32,
    alpha_parameter: i32,
) {
    let global_alpha = alpha_parameter.clamp(0, 256) as u32;
    for source_y in 0..source.height as i32 {
        let target_y = destination_y + source_y;
        if !(0..destination.height as i32).contains(&target_y) {
            continue;
        }
        for source_x in 0..source.width as i32 {
            let target_x = destination_x + source_x;
            if !(0..destination.width as i32).contains(&target_x) {
                continue;
            }
            let source_index = (source_y as usize * source.width as usize + source_x as usize) * 4;
            let target_index =
                (target_y as usize * destination.width as usize + target_x as usize) * 4;
            let source_pixel = &source.rgba[source_index..source_index + 4];
            let target_pixel = &mut destination.rgba[target_index..target_index + 4];
            if mode == 128 && global_alpha == 256 {
                target_pixel.copy_from_slice(source_pixel);
                continue;
            }

            let source_alpha = u32::from(source_pixel[3]) * global_alpha / 256;
            let inverse_alpha = 255 - source_alpha;
            for channel in 0..3 {
                target_pixel[channel] = ((u32::from(source_pixel[channel]) * source_alpha
                    + u32::from(target_pixel[channel]) * inverse_alpha
                    + 127)
                    / 255) as u8;
            }
            target_pixel[3] = (source_alpha
                + (u32::from(target_pixel[3]) * inverse_alpha + 127) / 255)
                .min(255) as u8;
        }
    }
}

pub(crate) fn crossfade_decoded_images(
    primary: &DecodedImage,
    secondary: &DecodedImage,
    alpha_parameter: i32,
) -> DecodedImage {
    let width = primary.width.min(secondary.width);
    let height = primary.height.min(secondary.height);
    let secondary_weight = alpha_parameter.clamp(0, 256) as u32;
    let primary_weight = 256 - secondary_weight;
    let mut rgba = vec![0; width as usize * height as usize * 4];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let primary_offset = (y * primary.width as usize + x) * 4;
            let secondary_offset = (y * secondary.width as usize + x) * 4;
            let destination_offset = (y * width as usize + x) * 4;
            for channel in 0..4 {
                let value = u32::from(primary.rgba[primary_offset + channel]) * primary_weight
                    + u32::from(secondary.rgba[secondary_offset + channel]) * secondary_weight;
                rgba[destination_offset + channel] = ((value + 128) >> 8) as u8;
            }
        }
    }
    DecodedImage {
        width,
        height,
        rgba,
    }
}

pub(crate) fn scale_decoded_image_fixed(
    source: &DecodedImage,
    scale_x: i32,
    scale_y: i32,
) -> Option<DecodedImage> {
    let width = ((u64::from(source.width) * scale_x.max(0) as u64) >> 16) as u32;
    let height = ((u64::from(source.height) * scale_y.max(0) as u64) >> 16) as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let mut rgba = vec![0; width as usize * height as usize * 4];
    for y in 0..height {
        let source_y = ((u64::from(y) << 16) / scale_y as u64)
            .min(u64::from(source.height.saturating_sub(1))) as u32;
        for x in 0..width {
            let source_x = ((u64::from(x) << 16) / scale_x as u64)
                .min(u64::from(source.width.saturating_sub(1))) as u32;
            let source_offset = (source_y as usize * source.width as usize + source_x as usize) * 4;
            let destination_offset = (y as usize * width as usize + x as usize) * 4;
            rgba[destination_offset..destination_offset + 4]
                .copy_from_slice(&source.rgba[source_offset..source_offset + 4]);
        }
    }
    Some(DecodedImage {
        width,
        height,
        rgba,
    })
}

pub(crate) fn blend_decoded_image_parameter(
    destination: &mut DecodedImage,
    source: &DecodedImage,
    alpha_parameter: i32,
) {
    let source_weight = alpha_parameter.clamp(0, 256) as u32;
    let destination_weight = 256 - source_weight;
    let width = destination.width.min(source.width);
    let height = destination.height.min(source.height);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let destination_offset = (y * destination.width as usize + x) * 4;
            let source_offset = (y * source.width as usize + x) * 4;
            for channel in 0..4 {
                let value = u32::from(destination.rgba[destination_offset + channel])
                    * destination_weight
                    + u32::from(source.rgba[source_offset + channel]) * source_weight;
                destination.rgba[destination_offset + channel] = ((value + 128) >> 8) as u8;
            }
        }
    }
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
    pub(crate) normal_resource: i32,
    pub(crate) selected_resource: i32,
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
            normal_resource: -1,
            selected_resource: -1,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeImageNodeArgs {
    pub(crate) node_id: i32,
    pub(crate) resource_id: i32,
    pub(crate) origin_x: i32,
    pub(crate) origin_y: i32,
    pub(crate) origin_z: i32,
}

impl NativeImageNodeArgs {
    pub(crate) fn from_popped(values: &[i32]) -> Option<Self> {
        // funcs_48065E[0x5c] (sub_47CC10) pops 17 values. In native
        // script order the node, coordinates, and bitmap are arguments
        // 1, 2, 3, and 5, so their reverse-pop slots are fixed.
        Some(Self {
            node_id: *values.get(16)?,
            resource_id: *values.get(12)?,
            origin_x: *values.get(15)?,
            origin_y: *values.get(14)?,
            origin_z: *values.get(13)?,
        })
    }

    pub(crate) fn screen_position(
        self,
        width: f32,
        height: f32,
        viewport_width: f32,
        viewport_height: f32,
    ) -> (f32, f32) {
        // sub_427170 sends arguments 2/3/4 to CDspObjVirtual::SetPosition
        // as 16.16 values. sub_429AF0 then offsets that object position by
        // half the viewport before the image object applies its own extent.
        let mut x = viewport_width * 0.5 + fixed_16_to_f32(self.origin_x) - width * 0.5;
        let mut y = viewport_height * 0.5 + fixed_16_to_f32(self.origin_y) - height * 0.5;
        if width >= viewport_width {
            x = 0.0;
        }
        if height >= viewport_height {
            y = 0.0;
        }
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        blend_decoded_image_parameter, blit_decoded_image, blit_decoded_image_parameter,
        crossfade_decoded_images, native_draw_order, scale_decoded_image_fixed,
        NativeImageNodeArgs, RuntimeGraphDrawItem, RuntimeGraphObjectProperties,
        RuntimeGraphResource,
    };
    use ethornell_image::DecodedImage;

    #[test]
    fn image_node_uses_native_fixed_argument_slots() {
        let popped = [
            1936,
            256,
            1,
            1,
            0,
            640,
            0,
            0,
            0,
            0,
            0,
            -1,
            6398,
            0,
            -23_592_960,
            -41_943_040,
            6,
        ];

        assert_eq!(
            NativeImageNodeArgs::from_popped(&popped),
            Some(NativeImageNodeArgs {
                node_id: 6,
                resource_id: 6398,
                origin_x: -41_943_040,
                origin_y: -23_592_960,
                origin_z: 0,
            })
        );
        assert_eq!(NativeImageNodeArgs::from_popped(&popped[..16]), None);
    }

    #[test]
    fn image_node_position_uses_native_fixed_point_object_coordinates() {
        let popped = [
            287,
            0,
            32,
            1,
            1,
            640,
            0,
            0,
            8,
            0,
            0,
            -1,
            43,
            0,
            -11_796_480,
            0,
            21,
        ];

        let args = NativeImageNodeArgs::from_popped(&popped).unwrap();
        assert_eq!(
            (args.origin_x, args.origin_y, args.origin_z),
            (0, -11_796_480, 0)
        );
        assert_eq!(
            args.screen_position(1280.0, 720.0, 1280.0, 720.0),
            (0.0, 0.0)
        );
    }

    #[test]
    fn bitmap_subregions_compose_atlas_coordinates() {
        let atlas = RuntimeGraphResource::whole("sysgrp.arc:atlas".into());
        let button = atlas.subregion(100, 40, 160, 50);
        let hover = button.subregion(0, 50, 160, 50);

        let button_rect = button.source_rect.unwrap();
        assert_eq!((button_rect.x, button_rect.y), (100.0, 40.0));
        assert_eq!((button_rect.width, button_rect.height), (160.0, 50.0));
        let hover_rect = hover.source_rect.unwrap();
        assert_eq!((hover_rect.x, hover_rect.y), (100.0, 90.0));
        assert_eq!((hover_rect.width, hover_rect.height), (160.0, 50.0));
    }

    #[test]
    fn native_bitmap_copy_resets_then_alpha_composites_the_target() {
        let mut target = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        };
        let base = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![20, 40, 60, 255],
        };
        let overlay = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![220, 140, 60, 128],
        };

        blit_decoded_image(&mut target, &base, 0, 0, 128);
        assert_eq!(target.rgba, [20, 40, 60, 255]);
        blit_decoded_image(&mut target, &overlay, 0, 0, 1);
        assert_eq!(target.rgba, [120, 90, 60, 255]);
    }

    #[test]
    fn parameterized_blit_applies_native_256_alpha() {
        let mut target = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        };
        let source = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![200, 100, 50, 255],
        };

        blit_decoded_image_parameter(&mut target, &source, 0, 0, 1, 128);
        assert_eq!(target.rgba, [100, 50, 25, 255]);
    }

    #[test]
    fn native_transition_alpha_crossfades_both_bitmaps() {
        let primary = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![240, 80, 0, 255],
        };
        let secondary = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![0, 40, 200, 0],
        };

        assert_eq!(
            crossfade_decoded_images(&primary, &secondary, 0).rgba,
            primary.rgba
        );
        assert_eq!(
            crossfade_decoded_images(&primary, &secondary, 256).rgba,
            secondary.rgba
        );
        assert_eq!(
            crossfade_decoded_images(&primary, &secondary, 128).rgba,
            [120, 60, 100, 128]
        );
    }

    #[test]
    fn native_object_mask_combines_without_overwriting_layer_animation_alpha() {
        let mut properties = RuntimeGraphObjectProperties::default();
        assert_eq!(properties.opacity(), 1.0);

        properties.set_property(1, 1, 0);
        properties.mask_alpha = 128;
        assert!((properties.opacity() - 0.5).abs() < f32::EPSILON);

        properties.set_property(2, 64, 0);
        assert!((properties.opacity() - 0.375).abs() < f32::EPSILON);
    }

    #[test]
    fn native_additive_mode_uses_the_recovered_integer_formula() {
        let mut properties = RuntimeGraphObjectProperties::default();
        properties.set_property(1, 2, 0);
        properties.set_property(2, 128, 0);
        properties.mask_alpha = 0;
        assert!((properties.opacity() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn native_full_mask_hides_every_blend_mode() {
        let mut properties = RuntimeGraphObjectProperties::default();
        properties.mask_alpha = 256;
        assert_eq!(properties.opacity(), 0.0);

        properties.set_property(1, 2, 0);
        properties.set_property(2, 256, 0);
        assert_eq!(properties.opacity(), 0.0);
    }

    #[test]
    fn equal_depth_objects_are_composed_newest_first() {
        let item = |hit_id| RuntimeGraphDrawItem {
            key: String::new(),
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            src_x: 0.0,
            src_y: 0.0,
            src_width: 1.0,
            src_height: 1.0,
            opacity: 1.0,
            rotation_degrees: 0.0,
            clip: None,
            z: 0,
            hit_id,
        };
        let mut items = [item(27), item(28)];
        items.sort_by_key(native_draw_order);
        assert_eq!(items.map(|item| item.hit_id), [28, 27]);
    }

    #[test]
    fn native_fixed_scale_and_parameter_blend_match_endpoints() {
        let source = DecodedImage {
            width: 2,
            height: 1,
            rgba: vec![10, 20, 30, 255, 200, 210, 220, 128],
        };
        let scaled = scale_decoded_image_fixed(&source, 32_768, 65_536).unwrap();
        assert_eq!((scaled.width, scaled.height), (1, 1));
        assert_eq!(scaled.rgba, [10, 20, 30, 255]);

        let mut destination = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![110, 120, 130, 0],
        };
        blend_decoded_image_parameter(&mut destination, &scaled, 128);
        assert_eq!(destination.rgba, [60, 70, 80, 128]);
    }
}
