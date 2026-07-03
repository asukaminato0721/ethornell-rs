use crate::RuntimeTraceApi;
use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use ethornell_core::Result;
use ethornell_image::{write_rgba_png, DecodedImage};
use std::{path::Path, sync::OnceLock};

const SNAPSHOT_WIDTH: u32 = 1280;
const SNAPSHOT_HEIGHT: u32 = 720;

pub(crate) fn write_runtime_snapshot(api: &RuntimeTraceApi, path: &Path) -> Result<()> {
    let mut out = DecodedImage {
        width: SNAPSHOT_WIDTH,
        height: SNAPSHOT_HEIGHT,
        rgba: vec![0; SNAPSHOT_WIDTH as usize * SNAPSHOT_HEIGHT as usize * 4],
    };
    for px in out.rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&[0, 0, 0, 255]);
    }

    for item in api.graph_draw_items() {
        let Some(src) = api.graph_images.get(&item.key) else {
            continue;
        };
        composite_nearest(
            &mut out,
            src,
            item.x,
            item.y,
            item.width,
            item.height,
            item.src_x,
            item.src_y,
            item.src_width,
            item.src_height,
            item.opacity,
        );
    }

    for node in api.text_nodes.values() {
        if api.should_draw_text_node(node) {
            draw_text_node(&mut out, node);
        }
    }

    write_rgba_png(&out, path)
}

fn draw_text_node(dst: &mut DecodedImage, node: &crate::text::RuntimeTextNode) {
    let Some(font) = snapshot_font() else {
        draw_fallback_text(dst, node);
        return;
    };
    let scale = PxScale::from(node.size.max(12.0));
    let scaled = font.as_scaled(scale);
    let line_height = (node.size * 1.35).max(18.0);
    let max_x = (SNAPSHOT_WIDTH as f32 - 36.0).max(node.x + 64.0);
    let mut x = node.x;
    let mut y = node.y + scaled.ascent();

    for ch in node.text.chars() {
        if ch == '\n' {
            x = node.x;
            y += line_height;
            continue;
        }
        let glyph_id = font.glyph_id(ch);
        let advance = scaled.h_advance(glyph_id).max(node.size * 0.5);
        if x + advance > max_x {
            x = node.x;
            y += line_height;
        }
        let glyph = glyph_id.with_scale_and_position(scale, point(x, y));
        draw_glyph(
            dst,
            font,
            glyph.clone(),
            [0.0, 0.0, 0.0, node.color[3] * 0.75],
            2,
            2,
        );
        draw_glyph(dst, font, glyph, node.color, 0, 0);
        x += advance;
    }
}

fn snapshot_font() -> Option<&'static FontArc> {
    static FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    FONT.get_or_init(|| {
        let candidates = [
            std::env::var("ETHORNELL_SNAPSHOT_FONT").ok(),
            Some("/System/Library/Fonts/Supplemental/Arial Unicode.ttf".to_string()),
            Some("/System/Library/Fonts/SFNS.ttf".to_string()),
        ];
        candidates
            .into_iter()
            .flatten()
            .filter_map(|path| std::fs::read(path).ok())
            .find_map(|bytes| FontArc::try_from_vec(bytes).ok())
    })
    .as_ref()
}

fn draw_glyph(
    dst: &mut DecodedImage,
    font: &FontArc,
    glyph: ab_glyph::Glyph,
    color: [f32; 4],
    dx: i32,
    dy: i32,
) {
    let Some(outlined) = font.outline_glyph(glyph) else {
        return;
    };
    let bounds = outlined.px_bounds();
    outlined.draw(|gx, gy, coverage| {
        let x = bounds.min.x as i32 + gx as i32 + dx;
        let y = bounds.min.y as i32 + gy as i32 + dy;
        if x < 0 || y < 0 || x >= dst.width as i32 || y >= dst.height as i32 {
            return;
        }
        let alpha = (coverage * color[3]).clamp(0.0, 1.0);
        let src = [
            (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            (alpha * 255.0).round() as u8,
        ];
        let i = ((y as u32 * dst.width + x as u32) * 4) as usize;
        blend_pixel(&mut dst.rgba[i..i + 4], &src, 1.0);
    });
}

fn draw_fallback_text(dst: &mut DecodedImage, node: &crate::text::RuntimeTextNode) {
    let mut x = node.x as i32;
    let mut y = node.y as i32;
    let char_w = (node.size * 0.55).max(8.0) as i32;
    let char_h = (node.size * 0.85).max(12.0) as i32;
    for ch in node.text.chars() {
        if ch == '\n' || x + char_w >= SNAPSHOT_WIDTH as i32 - 36 {
            x = node.x as i32;
            y += (node.size * 1.35).max(18.0) as i32;
            if ch == '\n' {
                continue;
            }
        }
        fill_rect(dst, x, y, char_w - 2, char_h, node.color);
        x += char_w;
    }
}

fn fill_rect(dst: &mut DecodedImage, x: i32, y: i32, width: i32, height: i32, color: [f32; 4]) {
    for py in y.max(0)..(y + height).min(dst.height as i32) {
        for px in x.max(0)..(x + width).min(dst.width as i32) {
            let src = [
                (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
            ];
            let i = ((py as u32 * dst.width + px as u32) * 4) as usize;
            blend_pixel(&mut dst.rgba[i..i + 4], &src, 1.0);
        }
    }
}

fn composite_nearest(
    dst: &mut DecodedImage,
    src: &DecodedImage,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    src_x: f32,
    src_y: f32,
    src_width: f32,
    src_height: f32,
    opacity: f32,
) {
    if src.width == 0
        || src.height == 0
        || width <= 0.0
        || height <= 0.0
        || src_width <= 0.0
        || src_height <= 0.0
        || opacity <= 0.0
    {
        return;
    }
    let alpha_scale = opacity.clamp(0.0, 1.0);
    let x0 = x.floor().max(0.0) as i32;
    let y0 = y.floor().max(0.0) as i32;
    let x1 = (x + width).ceil().min(dst.width as f32) as i32;
    let y1 = (y + height).ceil().min(dst.height as f32) as i32;
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    for dy in y0..y1 {
        let v = ((dy as f32 + 0.5 - y) / height).clamp(0.0, 0.999_999);
        let sy = (src_y + v * src_height).clamp(0.0, src.height.saturating_sub(1) as f32) as u32;
        for dx in x0..x1 {
            let u = ((dx as f32 + 0.5 - x) / width).clamp(0.0, 0.999_999);
            let sx = (src_x + u * src_width).clamp(0.0, src.width.saturating_sub(1) as f32) as u32;
            let src_i = ((sy * src.width + sx) * 4) as usize;
            let dst_i = ((dy as u32 * dst.width + dx as u32) * 4) as usize;
            blend_pixel(
                &mut dst.rgba[dst_i..dst_i + 4],
                &src.rgba[src_i..src_i + 4],
                alpha_scale,
            );
        }
    }
}

fn blend_pixel(dst: &mut [u8], src: &[u8], alpha_scale: f32) {
    let src_a = (src[3] as f32 / 255.0) * alpha_scale;
    if src_a <= 0.0 {
        return;
    }
    let inv_a = 1.0 - src_a;
    dst[0] = ((src[0] as f32 * src_a) + (dst[0] as f32 * inv_a)).round() as u8;
    dst[1] = ((src[1] as f32 * src_a) + (dst[1] as f32 * inv_a)).round() as u8;
    dst[2] = ((src[2] as f32 * src_a) + (dst[2] as f32 * inv_a)).round() as u8;
    dst[3] = 255;
}
