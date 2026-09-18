use ethornell_image::DecodedImage;

#[derive(Clone, Debug)]
pub(crate) struct RuntimeColorLut {
    channels: [[u8; 256]; 3],
}

impl RuntimeColorLut {
    pub(crate) fn from_control_points(points: [[i32; 2]; 3]) -> Option<Self> {
        if points
            .iter()
            .any(|point| !(1..255).contains(&point[0]) || !(0..=255).contains(&point[1]))
        {
            return None;
        }
        // sub_40A070 consumes the descriptor from the last pair to the first:
        // native bitmap words are BGRA, while DecodedImage is RGBA.
        let points = [points[2], points[1], points[0]];
        Some(Self {
            channels: points.map(|point| spline_channel(point[0], point[1])),
        })
    }

    pub(crate) fn apply(
        &self,
        source: &DecodedImage,
        color_before: i32,
        amount_before: i32,
        color_after: i32,
        amount_after: i32,
    ) -> DecodedImage {
        let mut output = source.clone();
        let before = packed_color(color_before);
        let after = packed_color(color_after);
        let amount_before = amount_before.clamp(0, 256);
        let amount_after = amount_after.clamp(0, 256);

        for pixel in output.rgba.as_chunks_mut::<4>().0 {
            let luminance =
                (i32::from(pixel[0]) * 77 + i32::from(pixel[1]) * 150 + i32::from(pixel[2]) * 29)
                    >> 8;
            for channel in 0..3 {
                let original = i32::from(pixel[channel]);
                let toned = luminance * i32::from(before[channel]) / 255;
                let mixed = lerp_256(original, toned, amount_before).clamp(0, 255) as usize;
                let mapped = i32::from(self.channels[channel][mixed]);
                let tinted = mapped * i32::from(after[channel]) / 255;
                pixel[channel] = lerp_256(mapped, tinted, amount_after).clamp(0, 255) as u8;
            }
        }
        output
    }
}

fn packed_color(value: i32) -> [u8; 3] {
    let value = value as u32;
    [
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ]
}

fn lerp_256(from: i32, to: i32, amount: i32) -> i32 {
    from + (((to - from) * amount) >> 8)
}

fn spline_channel(control_x: i32, control_y: i32) -> [u8; 256] {
    let mut result = [0_u8; 256];
    let knot_x = [0.0, f64::from(control_x), 255.0];
    let knot_y = [0.0, f64::from(control_y), 255.0];
    let widths = [knot_x[1] - knot_x[0], knot_x[2] - knot_x[1]];
    let mut curvature = [0.0_f64; 3];
    curvature[1] = (((knot_y[2] - knot_y[1]) / widths[1]) - ((knot_y[1] - knot_y[0]) / widths[0]))
        * 3.0
        / ((widths[1] + widths[0]) * 2.0);

    for (input, output) in result.iter_mut().enumerate() {
        let input = input as f64;
        let segment = usize::from(input >= knot_x[1]);
        let offset = input - knot_x[segment];
        let value = offset
            * ((knot_y[segment + 1] - knot_y[segment]) / widths[segment]
                - (2.0 * curvature[segment] + curvature[segment + 1]) * widths[segment] / 3.0
                + (((curvature[segment + 1] - curvature[segment]) / (3.0 * widths[segment]))
                    * offset
                    + curvature[segment])
                    * offset)
            + knot_y[segment];
        *output = native_round(value).clamp(0, 255) as u8;
    }
    result
}

fn native_round(value: f64) -> i32 {
    if value < 0.0 {
        (value - 0.5) as i32
    } else {
        (value + 0.5) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_identity_control_points_preserve_pixels() {
        let lut = RuntimeColorLut::from_control_points([[128, 128]; 3]).unwrap();
        let source = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![24, 90, 201, 77],
        };
        let output = lut.apply(&source, 0x00ff_ffff, 0, 0x00ff_ffff, 0);
        assert_eq!(output.rgba, source.rgba);
    }

    #[test]
    fn native_curve_descriptor_rejects_endpoint_control_x() {
        assert!(RuntimeColorLut::from_control_points([[0, 0], [128, 128], [128, 128]]).is_none());
        assert!(
            RuntimeColorLut::from_control_points([[255, 255], [128, 128], [128, 128]]).is_none()
        );
    }
}
