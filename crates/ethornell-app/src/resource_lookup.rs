use ethornell_archive::{ResourceEntry, ResourceManager};

#[derive(Debug, Clone)]
pub(crate) struct ResolvedResource {
    pub(crate) requested: String,
    pub(crate) resolved: String,
    pub(crate) entry: ResourceEntry,
}

pub(crate) fn find_scenario_image(
    manager: &ResourceManager,
    requested: &str,
) -> Option<ResolvedResource> {
    if let Some(entry) = manager.find(requested) {
        return Some(ResolvedResource {
            requested: requested.to_string(),
            resolved: entry.entry_name.clone(),
            entry,
        });
    }

    for alias in scenario_image_aliases(requested) {
        if let Some(entry) = manager.find(&alias) {
            return Some(ResolvedResource {
                requested: requested.to_string(),
                resolved: entry.entry_name.clone(),
                entry,
            });
        }
    }

    None
}

fn scenario_image_aliases(requested: &str) -> Vec<String> {
    let lower = requested.to_ascii_lowercase();
    let mut aliases = Vec::new();
    if let Some(suffix) = lower.strip_prefix("ef_soft") {
        if let Ok(number) = suffix.parse::<u32>() {
            for candidate in (2..number).rev() {
                aliases.push(format!("ef_soft{candidate}"));
            }
            aliases.push("ef_soft".to_string());
        }
    }
    aliases.extend(character_sprite_aliases(&lower));
    aliases
}

fn character_sprite_aliases(lower: &str) -> Vec<String> {
    let Some((head, expression)) = lower.rsplit_once("_d_") else {
        return Vec::new();
    };
    let Some((body, face)) = split_body_expression(expression) else {
        return Vec::new();
    };
    let mut aliases = Vec::new();
    for candidate_body in [body, 1, 2, 3] {
        for candidate_face in [face, "a01", "a03", "c01", "e01", "1base", "2base", "3base"] {
            let candidate = format!("{head}_d_{candidate_body}{candidate_face}");
            if candidate != lower && !aliases.iter().any(|alias| alias == &candidate) {
                aliases.push(candidate);
            }
        }
    }
    aliases
}

fn split_body_expression(expression: &str) -> Option<(i32, &str)> {
    let mut chars = expression.char_indices();
    let (_, first) = chars.next()?;
    if !first.is_ascii_digit() {
        return None;
    }
    let body = first.to_digit(10)? as i32;
    let face_start = first.len_utf8();
    let face = expression.get(face_start..)?;
    if face.is_empty() {
        return None;
    }
    Some((body, face))
}
