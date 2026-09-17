use crate::resource_lookup::ResolvedResource;
use ethornell_archive::ResourceManager;
use ethornell_core::Result;
use ethornell_image::{DecodedImage, decode_image};

pub(crate) fn decode_scenario_resource_image(
    manager: &ResourceManager,
    resource: &ResolvedResource,
    compose_character_base: bool,
    body_layer_hint: Option<&str>,
) -> Result<DecodedImage> {
    let image = decode_resource(manager, resource)?;
    if !compose_character_base {
        return Ok(image);
    }
    let Some(base_name) = character_base_name(&resource.resolved) else {
        return Ok(image);
    };
    let Some(base_entry) = manager.find(&base_name).or_else(|| {
        character_base_aliases(&resource.resolved)
            .into_iter()
            .find_map(|candidate| manager.find(&candidate))
    }) else {
        return Ok(image);
    };
    let base_bytes = manager.read_by_entry_decoded(&base_entry)?;
    let mut base = decode_image(&base_bytes)?;
    if base.width != image.width || base.height != image.height {
        return Ok(image);
    }
    for body_layer_name in character_body_layer_aliases(&resource.resolved, body_layer_hint) {
        let Some(body_entry) = manager.find(&body_layer_name) else {
            continue;
        };
        let body_bytes = manager.read_by_entry_decoded(&body_entry)?;
        let body = decode_image(&body_bytes)?;
        if base.width == body.width && base.height == body.height {
            alpha_composite(&mut base, &body);
            break;
        }
    }
    alpha_composite(&mut base, &image);
    Ok(base)
}

fn decode_resource(manager: &ResourceManager, resource: &ResolvedResource) -> Result<DecodedImage> {
    let bytes = manager.read_by_entry_decoded(&resource.entry)?;
    decode_image(&bytes)
}

fn character_base_name(name: &str) -> Option<String> {
    let (head, expression) = name.rsplit_once("_d_")?;
    if expression.ends_with("base") {
        return None;
    }
    let body = expression.chars().next()?.to_digit(10)?;
    Some(format!("{head}_d_{body}base"))
}

fn character_base_aliases(name: &str) -> Vec<String> {
    let Some((head, expression)) = name.rsplit_once("_d_") else {
        return Vec::new();
    };
    let Some(body) = expression.chars().next().and_then(|ch| ch.to_digit(10)) else {
        return Vec::new();
    };
    let mut aliases = Vec::new();
    for candidate_body in [body, 1, 2, 3] {
        let candidate = format!("{head}_d_{candidate_body}base");
        if !aliases.iter().any(|alias| alias == &candidate) {
            aliases.push(candidate);
        }
    }
    aliases
}

fn character_body_layer_aliases(name: &str, hint: Option<&str>) -> Vec<String> {
    let Some((head, expression)) = name.rsplit_once("_d_") else {
        return Vec::new();
    };
    let Some(body) = expression.chars().next().and_then(|ch| ch.to_digit(10)) else {
        return Vec::new();
    };
    let mut suffixes = Vec::new();
    if let Some(hint) = hint {
        suffixes.push(hint);
    }
    for fallback in ["ax12", "ax11", "ax14"] {
        if !suffixes.iter().any(|suffix| *suffix == fallback) {
            suffixes.push(fallback);
        }
    }
    suffixes
        .into_iter()
        .map(|suffix| format!("{head}_d_{body}{suffix}"))
        .collect()
}

fn alpha_composite(dst: &mut DecodedImage, src: &DecodedImage) {
    for (dst, src) in dst.rgba.chunks_exact_mut(4).zip(src.rgba.chunks_exact(4)) {
        let src_a = src[3] as f32 / 255.0;
        if src_a <= 0.0 {
            continue;
        }
        if src_a >= 1.0 {
            dst.copy_from_slice(src);
            continue;
        }
        let dst_a = dst[3] as f32 / 255.0;
        let out_a = src_a + dst_a * (1.0 - src_a);
        if out_a <= f32::EPSILON {
            dst.fill(0);
            continue;
        }
        for channel in 0..3 {
            let src_c = src[channel] as f32 / 255.0;
            let dst_c = dst[channel] as f32 / 255.0;
            let out_c = (src_c * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a;
            dst[channel] = (out_c * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}
