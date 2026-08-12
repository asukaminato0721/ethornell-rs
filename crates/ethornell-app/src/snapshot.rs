use crate::{
    graph::{RuntimeGraphDrawItem, RuntimeGraphLayer},
    RuntimeTraceApi,
};
use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use ethornell_core::{composite_native_rgba, Result};
use ethornell_image::{write_rgba_png, DecodedImage};
use std::{path::Path, sync::OnceLock};

const SNAPSHOT_WIDTH: u32 = 1280;
const SNAPSHOT_HEIGHT: u32 = 720;

pub(crate) struct RuntimeFrameComposition {
    pub(crate) framebuffer: DecodedImage,
    pub(crate) render_tree_nodes: usize,
    pub(crate) framebuffer_hash: u64,
}

pub(crate) fn write_runtime_snapshot(api: &RuntimeTraceApi, path: &Path) -> Result<()> {
    let out = compose_runtime_frame(api, true);
    write_composed_runtime_snapshot(api, &out, path)
}

pub(crate) fn write_composed_runtime_snapshot(
    api: &RuntimeTraceApi,
    framebuffer: &DecodedImage,
    path: &Path,
) -> Result<()> {
    if let Some(directory) = std::env::var_os("ETHORNELL_HEADLESS_DUMP_IMAGES_DIR") {
        let directory = std::path::PathBuf::from(directory);
        let selected_keys = std::env::var("ETHORNELL_HEADLESS_DUMP_IMAGE_KEYS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .map(str::to_owned)
                    .collect::<std::collections::BTreeSet<_>>()
            });
        std::fs::create_dir_all(&directory)?;
        for (key, image) in &api.graph_images {
            if selected_keys
                .as_ref()
                .is_some_and(|selected| !selected.contains(key.as_str()))
            {
                continue;
            }
            let name = key
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .collect::<String>();
            write_rgba_png(image, &directory.join(format!("{name}.png")))?;
        }
    }
    write_rgba_png(framebuffer, path)
}

pub(crate) fn runtime_frame_size(api: &RuntimeTraceApi) -> (u32, u32) {
    let width = u32::try_from(api.screen_width)
        .ok()
        .filter(|width| *width > 0)
        .unwrap_or(SNAPSHOT_WIDTH);
    let height = u32::try_from(api.screen_height)
        .ok()
        .filter(|height| *height > 0)
        .unwrap_or(SNAPSHOT_HEIGHT);
    (width, height)
}

pub(crate) fn compose_runtime_frame(api: &RuntimeTraceApi, include_text: bool) -> DecodedImage {
    compose_runtime_frame_through_priority(api, include_text, None)
}

pub(crate) fn compose_runtime_frame_with_diagnostics(
    api: &RuntimeTraceApi,
    include_text: bool,
) -> RuntimeFrameComposition {
    let render_tree_nodes = api.graph_output_draw_items().len();
    let framebuffer = compose_runtime_frame(api, include_text);
    let framebuffer_hash = stable_framebuffer_hash(&framebuffer);
    RuntimeFrameComposition {
        framebuffer,
        render_tree_nodes,
        framebuffer_hash,
    }
}

pub(crate) fn stable_framebuffer_hash(framebuffer: &DecodedImage) -> u64 {
    // A fixed FNV-1a digest keeps frame diagnostics stable across platforms and
    // process invocations without introducing a rendering dependency.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in framebuffer
        .width
        .to_le_bytes()
        .into_iter()
        .chain(framebuffer.height.to_le_bytes())
        .chain(framebuffer.rgba.iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(crate) fn compose_runtime_frame_through_priority(
    api: &RuntimeTraceApi,
    include_text: bool,
    maximum_priority: Option<i32>,
) -> DecodedImage {
    let (width, height) = runtime_frame_size(api);
    let mut out = DecodedImage {
        width,
        height,
        rgba: vec![0; width as usize * height as usize * 4],
    };
    for px in out.rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&[0, 0, 0, 255]);
    }

    let draw_items = if maximum_priority.is_some() {
        api.graph_draw_items()
    } else {
        api.graph_output_draw_items()
    };
    for item in draw_items {
        if maximum_priority.is_some_and(|maximum| item.z > maximum) {
            continue;
        }
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
            item.blend_mode,
            item.clip,
        );
    }

    if include_text && std::env::var_os("ETHORNELL_SNAPSHOT_HIDE_TEXT").is_none() {
        for node in api.text_nodes.values() {
            if maximum_priority.is_none_or(|maximum| node.z <= maximum)
                && api.should_draw_text_node(node)
            {
                let mut positioned = node.clone();
                if let Some(surface) = node
                    .target_surface
                    .filter(|surface| api.graph_surfaces.contains_key(surface))
                {
                    let (surface_x, surface_y) = api.surface_world_position(surface);
                    positioned.x += surface_x;
                    positioned.y += surface_y;
                    positioned.color[3] *= api.surface_display_opacity(surface);
                }
                let clip = node
                    .target_surface
                    .and_then(|surface| api.surface_chain_clip(surface));
                draw_text_node(
                    &mut out,
                    &positioned,
                    api.formatted_text_layout(&positioned),
                    api.graph_defaults.shadow_parameters(),
                    clip,
                );
            }
        }
    }

    out
}

pub(crate) fn compose_graph_object(
    api: &RuntimeTraceApi,
    object: i32,
    target_size: Option<(u32, u32)>,
) -> Option<DecodedImage> {
    let surface = api.graph_surfaces.get(&object);
    let selected_layers = api
        .graph_layers
        .iter()
        .filter(|(id, layer)| {
            surface.is_some_and(|_| layer.target_surface == Some(object))
                || **id == object
                || layer.owner_object == Some(object)
        })
        .collect::<Vec<_>>();
    let selected_text = api
        .text_nodes
        .values()
        .filter(|node| {
            surface.is_some_and(|_| node.target_surface == Some(object))
                || node.owner_object == Some(object)
        })
        .collect::<Vec<_>>();

    let (origin_x, origin_y, content_width, content_height) = if let Some(surface) = surface {
        let (world_x, world_y) = api.surface_world_position(surface.id);
        (
            world_x,
            world_y,
            surface.viewport_width.max(surface.width).max(1.0),
            surface.viewport_height.max(surface.height).max(1.0),
        )
    } else {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (layer_id, layer) in &selected_layers {
            let (x, y, _) = api.layer_world_transform(**layer_id, layer);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + (layer.width * layer.scale_x).abs());
            max_y = max_y.max(y + (layer.height * layer.scale_y).abs());
        }
        for node in &selected_text {
            let (surface_x, surface_y) = node
                .target_surface
                .filter(|id| api.graph_surfaces.contains_key(id))
                .map(|id| api.surface_world_position(id))
                .unwrap_or_default();
            let x = surface_x + node.x;
            let y = surface_y + node.y;
            let line_count = node.text.lines().count().max(1) as f32;
            let max_line_chars = node
                .text
                .lines()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or_default() as f32;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + (max_line_chars * node.size * 0.6).max(node.size));
            max_y = max_y.max(y + (line_count * node.size * 1.35).max(node.size));
        }
        if !min_x.is_finite() || !min_y.is_finite() {
            return None;
        }
        (
            min_x,
            min_y,
            (max_x - min_x).ceil().max(1.0),
            (max_y - min_y).ceil().max(1.0),
        )
    };
    let (width, height) = target_size
        .filter(|(width, height)| *width > 0 && *height > 0)
        .unwrap_or((content_width.ceil() as u32, content_height.ceil() as u32));
    let mut out = DecodedImage {
        width: width.max(1),
        height: height.max(1),
        rgba: vec![0; width.max(1) as usize * height.max(1) as usize * 4],
    };

    if let Some(surface) = surface {
        if let Some(resource_id) = surface.resource_id {
            if let Some((key, region)) = api.resource_image_region(resource_id) {
                if let Some(source) = api.graph_images.get(key) {
                    let viewport_x = surface.viewport_x.max(0.0);
                    let viewport_y = surface.viewport_y.max(0.0);
                    let draw_width = surface
                        .viewport_width
                        .min(region.width - viewport_x)
                        .max(0.0);
                    let draw_height = surface
                        .viewport_height
                        .min(region.height - viewport_y)
                        .max(0.0);
                    composite_nearest(
                        &mut out,
                        source,
                        0.0,
                        0.0,
                        draw_width,
                        draw_height,
                        region.x + viewport_x,
                        region.y + viewport_y,
                        draw_width,
                        draw_height,
                        surface.opacity,
                        api.graph_object_properties
                            .get(&surface.id)
                            .map(|properties| properties.blend_mode)
                            .unwrap_or(128),
                        None,
                    );
                }
            }
        }
    }

    let mut draw_items = selected_layers
        .into_iter()
        .filter_map(|(id, layer)| capture_draw_item(api, *id, layer, origin_x, origin_y))
        .collect::<Vec<_>>();
    draw_items.sort_by_key(crate::graph::native_draw_order);
    for item in draw_items {
        let Some(source) = api.graph_images.get(&item.key) else {
            continue;
        };
        composite_nearest(
            &mut out,
            source,
            item.x,
            item.y,
            item.width,
            item.height,
            item.src_x,
            item.src_y,
            item.src_width,
            item.src_height,
            item.opacity,
            item.blend_mode,
            item.clip,
        );
    }

    for node in selected_text {
        if !node.enabled || !api.object_enabled_for_layer(node.owner_object) || node.text.is_empty()
        {
            continue;
        }
        let mut positioned = node.clone();
        if let Some(surface) = node
            .target_surface
            .filter(|surface| api.graph_surfaces.contains_key(surface))
        {
            let (surface_x, surface_y) = api.surface_world_position(surface);
            positioned.x += surface_x;
            positioned.y += surface_y;
        }
        positioned.x -= origin_x;
        positioned.y -= origin_y;
        let clip = node
            .target_surface
            .and_then(|surface| api.surface_chain_clip(surface))
            .map(|clip| crate::graph::RuntimeClipRect {
                x: clip.x - origin_x,
                y: clip.y - origin_y,
                width: clip.width,
                height: clip.height,
            });
        draw_text_node(
            &mut out,
            &positioned,
            api.formatted_text_layout(&positioned),
            api.graph_defaults.shadow_parameters(),
            clip,
        );
    }

    Some(out)
}

fn capture_draw_item(
    api: &RuntimeTraceApi,
    layer_id: i32,
    layer: &RuntimeGraphLayer,
    origin_x: f32,
    origin_y: f32,
) -> Option<RuntimeGraphDrawItem> {
    if !layer.enabled || !api.object_enabled_for_layer(layer.owner_object) {
        return None;
    }
    let object_opacity =
        api.object_chain_opacity(layer.owner_object.filter(|object| *object != layer_id));
    let node_properties = api.graph_object_properties.get(&layer_id);
    let owner_properties = layer
        .owner_object
        .and_then(|object| api.graph_object_properties.get(&object));
    let node_opacity = if api.graph_transition_nodes.contains_key(&layer_id) {
        1.0
    } else {
        node_properties
            .map(crate::graph::RuntimeGraphObjectProperties::opacity)
            .unwrap_or(1.0)
    };
    let blend_mode = node_properties
        .or(owner_properties)
        .map(|properties| properties.blend_mode)
        .unwrap_or(128);
    let opacity = layer.opacity * node_opacity * object_opacity;
    if opacity <= 0.001 {
        return None;
    }
    let draw_key = api
        .message_control_layer_draw_key(layer_id, layer)
        .unwrap_or(layer.key.as_str());
    let image = api.graph_images.get(draw_key)?;
    if layer.width <= 0.0 || layer.height <= 0.0 {
        return None;
    }
    let width = (layer.width * layer.scale_x)
        .abs()
        .min(image.width as f32 * layer.scale_x.abs().max(1.0));
    let height = (layer.height * layer.scale_y)
        .abs()
        .min(image.height as f32 * layer.scale_y.abs().max(1.0));
    let src_x = layer.src_x.clamp(0.0, image.width.saturating_sub(1) as f32);
    let src_y = layer
        .src_y
        .clamp(0.0, image.height.saturating_sub(1) as f32);
    let src_width = width.min(image.width as f32 - src_x).max(0.0);
    let src_height = height.min(image.height as f32 - src_y).max(0.0);
    let (world_x, world_y, world_z) = api.layer_world_transform(layer_id, layer);
    (src_width > 0.0 && src_height > 0.0).then(|| RuntimeGraphDrawItem {
        owner_object: layer.owner_object.or(Some(layer_id)),
        key: draw_key.to_string(),
        x: world_x - origin_x,
        y: world_y - origin_y,
        width,
        height,
        src_x,
        src_y,
        src_width,
        src_height,
        opacity,
        rotation_degrees: layer.rotation_degrees,
        clip: api
            .layer_display_clip(layer)
            .map(|clip| crate::graph::RuntimeClipRect {
                x: clip.x - origin_x,
                y: clip.y - origin_y,
                width: clip.width,
                height: clip.height,
            }),
        z: world_z,
        blend_mode,
        order_serial: api.display_order_serial(layer_id, layer.owner_object),
        hit_id: layer.hit_id,
    })
}

pub(crate) fn draw_text_node(
    dst: &mut DecodedImage,
    node: &crate::text::RuntimeTextNode,
    layout: crate::text::RuntimeFormattedTextLayout,
    shadow: Option<(i32, i32, i32)>,
    clip: Option<crate::graph::RuntimeClipRect>,
) {
    let Some(font) = snapshot_font() else {
        draw_fallback_text(dst, node, clip);
        return;
    };
    let scale = PxScale::from(node.size.max(12.0));
    let scaled = font.as_scaled(scale);
    let line_height = layout.line_height;
    let max_x = (dst.width as f32 - 36.0).max(node.x + 64.0);
    let mut x = node.x;
    let mut y = node.y + layout.body_y_offset + scaled.ascent();
    let mut char_index = 0usize;

    for ch in node.text.chars() {
        if ch == '\n' {
            x = node.x;
            y += line_height;
            char_index += 1;
            continue;
        }
        let style = runtime_text_style_at(&node.style_spans, char_index);
        let color = runtime_text_style_color(node.color, style);
        let bold = style.is_some_and(|style| style.style.bold);
        let italic = style.is_some_and(|style| style.style.italic);
        let glyph_id = font.glyph_id(ch);
        let advance = scaled.h_advance(glyph_id).max(node.size * 0.5);
        if x + advance > max_x {
            x = node.x;
            y += line_height;
        }
        let glyph = glyph_id.with_scale_and_position(scale, point(x, y));
        if let Some((shadow_x, shadow_y, shadow_alpha)) = resolve_text_shadow(shadow, node.size) {
            draw_glyph(
                dst,
                font,
                glyph.clone(),
                [0.0, 0.0, 0.0, color[3] * shadow_alpha],
                shadow_x,
                shadow_y,
                bold,
                italic,
                clip,
            );
        }
        draw_glyph(dst, font, glyph, color, 0, 0, bold, italic, clip);
        x += advance;
        char_index += 1;
    }

    for (ruby, x, y, size) in crate::text::ruby_draw_runs(node, layout) {
        draw_text_run(dst, font, &ruby, x, y, size, node.color, shadow, clip);
    }
}

fn draw_text_run(
    dst: &mut DecodedImage,
    font: &FontArc,
    text: &str,
    mut x: f32,
    y: f32,
    size: f32,
    color: [f32; 4],
    shadow: Option<(i32, i32, i32)>,
    clip: Option<crate::graph::RuntimeClipRect>,
) {
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let baseline = y + scaled.ascent();
    for ch in text.chars() {
        let glyph_id = font.glyph_id(ch);
        let advance = scaled.h_advance(glyph_id).max(size * 0.5);
        let glyph = glyph_id.with_scale_and_position(scale, point(x, baseline));
        if let Some((shadow_x, shadow_y, shadow_alpha)) = resolve_text_shadow(shadow, size) {
            draw_glyph(
                dst,
                font,
                glyph.clone(),
                [0.0, 0.0, 0.0, color[3] * shadow_alpha],
                shadow_x,
                shadow_y,
                false,
                false,
                clip,
            );
        }
        draw_glyph(dst, font, glyph, color, 0, 0, false, false, clip);
        x += advance;
    }
}

fn resolve_text_shadow(
    shadow: Option<(i32, i32, i32)>,
    font_height: f32,
) -> Option<(i32, i32, f32)> {
    let (x_percent, y_percent, concentration) = shadow?;
    let height = font_height.max(0.0).round() as i32;
    Some((
        height.saturating_mul(x_percent) / 100,
        height.saturating_mul(y_percent) / 100,
        (256 - concentration.clamp(0, 256)) as f32 / 256.0,
    ))
}

fn snapshot_font() -> Option<&'static FontArc> {
    static FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    FONT.get_or_init(|| {
        if let Some(font) = std::env::var("ETHORNELL_SNAPSHOT_FONT")
            .ok()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| FontArc::try_from_vec(bytes).ok())
        {
            return Some(font);
        }
        ethornell_render::load_system_cjk_font().ok()
    })
    .as_ref()
}

pub(crate) fn measure_text_advance(text: &str, size: f32, spacing: f32) -> i32 {
    let glyph_count = text.chars().count();
    if glyph_count == 0 {
        return 0;
    }
    let width = if let Some(font) = snapshot_font() {
        let scaled = font.as_scaled(PxScale::from(size.max(1.0)));
        text.chars()
            .map(|character| scaled.h_advance(font.glyph_id(character)))
            .sum::<f32>()
    } else {
        text.chars()
            .map(|character| {
                if character.is_ascii() {
                    size * 0.5
                } else {
                    size
                }
            })
            .sum::<f32>()
    };
    (width + spacing * glyph_count as f32).round() as i32
}

fn draw_glyph(
    dst: &mut DecodedImage,
    font: &FontArc,
    glyph: ab_glyph::Glyph,
    color: [f32; 4],
    dx: i32,
    dy: i32,
    bold: bool,
    italic: bool,
    clip: Option<crate::graph::RuntimeClipRect>,
) {
    let Some(outlined) = font.outline_glyph(glyph) else {
        return;
    };
    let bounds = outlined.px_bounds();
    outlined.draw(|gx, gy, coverage| {
        let italic_dx = if italic {
            ((bounds.height() - gy as f32).max(0.0) * 0.20).round() as i32
        } else {
            0
        };
        let x = bounds.min.x as i32 + gx as i32 + dx + italic_dx;
        let y = bounds.min.y as i32 + gy as i32 + dy;
        for bold_dx in 0..=i32::from(bold) {
            let x = x + bold_dx;
            if x < 0
                || y < 0
                || x >= dst.width as i32
                || y >= dst.height as i32
                || !point_in_clip(x, y, clip)
            {
                continue;
            }
            let alpha = (coverage * color[3]).clamp(0.0, 1.0);
            let src = [
                (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                (alpha * 255.0).round() as u8,
            ];
            let i = ((y as u32 * dst.width + x as u32) * 4) as usize;
            blend_pixel_native(&mut dst.rgba[i..i + 4], &src, 1.0, 0);
        }
    });
}

fn runtime_text_style_at(
    styles: &[crate::text_anim::RuntimeTextStyleSpan],
    char_index: usize,
) -> Option<&crate::text_anim::RuntimeTextStyleSpan> {
    styles
        .iter()
        .find(|style| style.start_char <= char_index && char_index < style.end_char)
}

fn runtime_text_style_color(
    base: [f32; 4],
    style: Option<&crate::text_anim::RuntimeTextStyleSpan>,
) -> [f32; 4] {
    style
        .and_then(|style| style.style.packed_rgb)
        .map(|packed| {
            [
                ((packed >> 16) & 0xff) as f32 / 255.0,
                ((packed >> 8) & 0xff) as f32 / 255.0,
                (packed & 0xff) as f32 / 255.0,
                base[3],
            ]
        })
        .unwrap_or(base)
}

fn draw_fallback_text(
    dst: &mut DecodedImage,
    node: &crate::text::RuntimeTextNode,
    clip: Option<crate::graph::RuntimeClipRect>,
) {
    let mut x = node.x as i32;
    let mut y = node.y as i32;
    let char_w = (node.size * 0.55).max(8.0) as i32;
    let char_h = (node.size * 0.85).max(12.0) as i32;
    for (char_index, ch) in node.text.chars().enumerate() {
        if ch == '\n' || x + char_w >= dst.width as i32 - 36 {
            x = node.x as i32;
            y += (node.size * 1.35).max(18.0) as i32;
            if ch == '\n' {
                continue;
            }
        }
        let style = runtime_text_style_at(&node.style_spans, char_index);
        fill_rect(
            dst,
            x,
            y,
            char_w - 2 + i32::from(style.is_some_and(|style| style.style.bold)),
            char_h,
            runtime_text_style_color(node.color, style),
            clip,
        );
        x += char_w;
    }
}

fn fill_rect(
    dst: &mut DecodedImage,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: [f32; 4],
    clip: Option<crate::graph::RuntimeClipRect>,
) {
    for py in y.max(0)..(y + height).min(dst.height as i32) {
        for px in x.max(0)..(x + width).min(dst.width as i32) {
            if !point_in_clip(px, py, clip) {
                continue;
            }
            let src = [
                (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
            ];
            let i = ((py as u32 * dst.width + px as u32) * 4) as usize;
            blend_pixel_native(&mut dst.rgba[i..i + 4], &src, 1.0, 0);
        }
    }
}

fn point_in_clip(x: i32, y: i32, clip: Option<crate::graph::RuntimeClipRect>) -> bool {
    clip.is_none_or(|clip| {
        let x = x as f32;
        let y = y as f32;
        x >= clip.x && y >= clip.y && x < clip.x + clip.width && y < clip.y + clip.height
    })
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
    blend_mode: i32,
    clip: Option<crate::graph::RuntimeClipRect>,
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
    let clip_x0 = clip.map(|clip| clip.x).unwrap_or(0.0);
    let clip_y0 = clip.map(|clip| clip.y).unwrap_or(0.0);
    let clip_x1 = clip
        .map(|clip| clip.x + clip.width)
        .unwrap_or(dst.width as f32);
    let clip_y1 = clip
        .map(|clip| clip.y + clip.height)
        .unwrap_or(dst.height as f32);
    let x0 = x.floor().max(0.0).max(clip_x0) as i32;
    let y0 = y.floor().max(0.0).max(clip_y0) as i32;
    let x1 = (x + width).ceil().min(dst.width as f32).min(clip_x1) as i32;
    let y1 = (y + height).ceil().min(dst.height as f32).min(clip_y1) as i32;
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
            blend_pixel_native(
                &mut dst.rgba[dst_i..dst_i + 4],
                &src.rgba[src_i..src_i + 4],
                alpha_scale,
                blend_mode,
            );
        }
    }
}

fn blend_pixel_native(dst: &mut [u8], src: &[u8], alpha_scale: f32, blend_mode: i32) {
    let destination: &mut [u8; 4] = dst.try_into().expect("RGBA destination pixel");
    let source: [u8; 4] = src.try_into().expect("RGBA source pixel");
    composite_native_rgba(destination, source, alpha_scale, blend_mode);
}

#[cfg(test)]
mod native_compositor_tests {
    use super::{composite_nearest, stable_framebuffer_hash};
    use crate::graph::RuntimeClipRect;
    use ethornell_image::DecodedImage;

    fn pixel(r: u8, g: u8, b: u8, a: u8) -> DecodedImage {
        DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![r, g, b, a],
        }
    }

    #[test]
    fn additive_mode_accumulates_source_color() {
        let mut destination = pixel(10, 20, 30, 255);
        let source = pixel(100, 80, 60, 128);
        composite_nearest(
            &mut destination,
            &source,
            0.0,
            0.0,
            1.0,
            1.0,
            0.0,
            0.0,
            1.0,
            1.0,
            1.0,
            2,
            None,
        );
        assert_eq!(destination.rgba, [60, 60, 60, 255]);
    }

    #[test]
    fn subtractive_mode_removes_source_color() {
        let mut destination = pixel(100, 100, 100, 255);
        let source = pixel(80, 40, 20, 128);
        composite_nearest(
            &mut destination,
            &source,
            0.0,
            0.0,
            1.0,
            1.0,
            0.0,
            0.0,
            1.0,
            1.0,
            1.0,
            3,
            None,
        );
        assert_eq!(destination.rgba, [60, 80, 90, 255]);
    }

    #[test]
    fn compositor_respects_surface_clip() {
        let mut destination = DecodedImage {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 0, 0, 0, 255],
        };
        let source = pixel(255, 255, 255, 255);
        composite_nearest(
            &mut destination,
            &source,
            0.0,
            0.0,
            2.0,
            1.0,
            0.0,
            0.0,
            1.0,
            1.0,
            1.0,
            1,
            Some(RuntimeClipRect {
                x: 1.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }),
        );
        assert_eq!(destination.rgba, [0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn framebuffer_hash_covers_geometry_and_pixels() {
        let black = pixel(0, 0, 0, 255);
        let mut white = pixel(0, 0, 0, 255);
        white.rgba[0] = 255;
        let wider = DecodedImage {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 0, 0, 0, 255],
        };

        assert_eq!(
            stable_framebuffer_hash(&black),
            stable_framebuffer_hash(&black)
        );
        assert_ne!(
            stable_framebuffer_hash(&black),
            stable_framebuffer_hash(&white)
        );
        assert_ne!(
            stable_framebuffer_hash(&black),
            stable_framebuffer_hash(&wider)
        );
    }
}

#[cfg(test)]
mod styled_text_snapshot_tests {
    use super::draw_text_node;
    use crate::text::{RuntimeFormattedTextLayout, RuntimeTextNode};
    use crate::text_anim::{RuntimeTextStyle, RuntimeTextStyleSpan};
    use ethornell_image::DecodedImage;

    #[test]
    fn cpu_framebuffer_applies_the_same_per_character_color_span() {
        let node = RuntimeTextNode {
            text: "AB".into(),
            enabled: true,
            owner_object: None,
            screen_attached: true,
            target_surface: None,
            x: 8.0,
            y: 8.0,
            size: 32.0,
            line_height: 40.0,
            formatted_layout: false,
            color: [1.0; 4],
            z: 0,
            ruby_spans: Vec::new(),
            style_spans: vec![RuntimeTextStyleSpan {
                start_char: 1,
                end_char: 2,
                style: RuntimeTextStyle {
                    packed_rgb: Some(0xff0000),
                    bold: true,
                    italic: true,
                },
            }],
        };
        let layout = RuntimeFormattedTextLayout::plain(&node);
        let mut image = DecodedImage {
            width: 128,
            height: 64,
            rgba: vec![0; 128 * 64 * 4],
        };
        draw_text_node(&mut image, &node, layout, None, None);
        assert!(image
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 200 && pixel[1] > 200 && pixel[2] > 200 && pixel[3] > 0));
        assert!(image
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 200 && pixel[1] < 20 && pixel[2] < 20 && pixel[3] > 0));
    }
}
