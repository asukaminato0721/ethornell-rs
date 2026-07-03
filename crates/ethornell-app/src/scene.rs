#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScenarioSpriteClass {
    Modal,
    Background,
    Event,
    Character,
    Effect,
    FadePlate,
    Logo,
    Ui,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScenarioSpritePlacement {
    pub(crate) class: ScenarioSpriteClass,
    pub(crate) layer_id: i32,
    pub(crate) z: i32,
    pub(crate) x: Option<f32>,
    pub(crate) y: Option<f32>,
    pub(crate) hit_id: i32,
    pub(crate) blocks_input: bool,
    pub(crate) track_scene_layer: bool,
}

pub(crate) const SCENARIO_OVERLAY_LAYER_ID: i32 = -50_000;
pub(crate) const SCENARIO_OVERLAY_HIT_ID: i32 = 50_000;

const BACKGROUND_LAYER_ID: i32 = -60_010;
const EVENT_LAYER_ID: i32 = -60_020;
const LOGO_LAYER_ID: i32 = -60_030;
const UI_LAYER_ID: i32 = -60_040;
const EFFECT_LAYER_BASE: i32 = -60_200;
pub(crate) const FADE_PLATE_LAYER_ID: i32 = EFFECT_LAYER_BASE - 90;
const CHARACTER_LAYER_BASE: i32 = -60_400;
const OTHER_LAYER_BASE: i32 = -60_700;

pub(crate) fn classify_scenario_sprite(name: &str) -> ScenarioSpriteClass {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("warn") {
        ScenarioSpriteClass::Modal
    } else if matches!(lower.as_str(), "bg_black" | "bg_white") {
        ScenarioSpriteClass::FadePlate
    } else if lower.starts_with("bg") {
        ScenarioSpriteClass::Background
    } else if lower.starts_with("ev_") || lower.starts_with("ev") {
        ScenarioSpriteClass::Event
    } else if lower.starts_with("ef_") || lower.starts_with("ef") {
        ScenarioSpriteClass::Effect
    } else if lower.starts_with("brandlogo") || lower.contains("logo") {
        ScenarioSpriteClass::Logo
    } else if lower.starts_with("msg") || lower.starts_with("sys") {
        ScenarioSpriteClass::Ui
    } else if looks_like_character_resource(&lower) {
        ScenarioSpriteClass::Character
    } else {
        ScenarioSpriteClass::Other
    }
}

pub(crate) fn place_scenario_sprite(name: &str, sequence: u32) -> ScenarioSpritePlacement {
    place_scenario_sprite_with_hints(name, sequence, None, None, None)
}

pub(crate) fn place_scenario_sprite_with_hints(
    name: &str,
    sequence: u32,
    slot: Option<i32>,
    x: Option<i32>,
    z: Option<i32>,
) -> ScenarioSpritePlacement {
    let class = classify_scenario_sprite(name);
    let sequence_slot = slot
        .filter(|slot| (0..=99).contains(slot))
        .unwrap_or((sequence % 96) as i32);
    match class {
        ScenarioSpriteClass::Modal => ScenarioSpritePlacement {
            class,
            layer_id: SCENARIO_OVERLAY_LAYER_ID,
            z: 50_000,
            x: None,
            y: None,
            hit_id: SCENARIO_OVERLAY_HIT_ID,
            blocks_input: true,
            track_scene_layer: false,
        },
        ScenarioSpriteClass::Background => ScenarioSpritePlacement {
            class,
            layer_id: slot
                .filter(|slot| (0..=99).contains(slot))
                .map(background_layer_id_for_slot)
                .unwrap_or(BACKGROUND_LAYER_ID),
            z: z.unwrap_or(10).clamp(0, 899),
            x: Some(0.0),
            y: Some(0.0),
            hit_id: 0,
            blocks_input: false,
            track_scene_layer: true,
        },
        ScenarioSpriteClass::Event => ScenarioSpritePlacement {
            class,
            layer_id: EVENT_LAYER_ID,
            z: 50,
            x: Some(0.0),
            y: Some(0.0),
            hit_id: 0,
            blocks_input: false,
            track_scene_layer: true,
        },
        ScenarioSpriteClass::Character => ScenarioSpritePlacement {
            class,
            layer_id: CHARACTER_LAYER_BASE - sequence_slot,
            z: z.unwrap_or(70 + sequence_slot).clamp(40, 899),
            x: x.map(|value| value as f32),
            y: None,
            hit_id: 0,
            blocks_input: false,
            track_scene_layer: true,
        },
        ScenarioSpriteClass::Effect => {
            let lower = name.to_ascii_lowercase();
            let (layer_id, default_z) = if lower.starts_with("ef_soft") {
                (EFFECT_LAYER_BASE, 96)
            } else if lower.starts_with("ef_sepia") {
                (EFFECT_LAYER_BASE - 1, 95)
            } else {
                (EFFECT_LAYER_BASE - sequence_slot, 90 + sequence_slot)
            };
            ScenarioSpritePlacement {
                class,
                layer_id,
                z: z.unwrap_or(default_z).clamp(0, 899),
                x: Some(0.0),
                y: Some(0.0),
                hit_id: 0,
                blocks_input: false,
                track_scene_layer: true,
            }
        }
        ScenarioSpriteClass::FadePlate => ScenarioSpritePlacement {
            class,
            layer_id: FADE_PLATE_LAYER_ID,
            z: z.unwrap_or(900).clamp(0, 999),
            x: Some(0.0),
            y: Some(0.0),
            hit_id: 0,
            blocks_input: false,
            track_scene_layer: true,
        },
        ScenarioSpriteClass::Logo => ScenarioSpritePlacement {
            class,
            layer_id: LOGO_LAYER_ID,
            z: 60,
            x: None,
            y: None,
            hit_id: 0,
            blocks_input: false,
            track_scene_layer: true,
        },
        ScenarioSpriteClass::Ui => ScenarioSpritePlacement {
            class,
            layer_id: UI_LAYER_ID,
            z: 950,
            x: None,
            y: None,
            hit_id: 0,
            blocks_input: false,
            track_scene_layer: true,
        },
        ScenarioSpriteClass::Other => ScenarioSpritePlacement {
            class,
            layer_id: OTHER_LAYER_BASE - sequence_slot,
            z: z.unwrap_or(65 + sequence_slot).clamp(20, 899),
            x: x.map(|value| value as f32),
            y: None,
            hit_id: 0,
            blocks_input: false,
            track_scene_layer: true,
        },
    }
}

pub(crate) fn scenario_layer_ids_for_slot(slot: i32) -> [i32; 3] {
    [
        background_layer_id_for_slot(slot),
        CHARACTER_LAYER_BASE - slot,
        OTHER_LAYER_BASE - slot,
    ]
}

pub(crate) fn is_viewport_sprite_class(class: ScenarioSpriteClass) -> bool {
    matches!(
        class,
        ScenarioSpriteClass::Background
            | ScenarioSpriteClass::Event
            | ScenarioSpriteClass::FadePlate
    )
}

pub(crate) fn is_background_layer_id(layer_id: i32) -> bool {
    ((BACKGROUND_LAYER_ID - 99)..=BACKGROUND_LAYER_ID).contains(&layer_id)
}

fn background_layer_id_for_slot(slot: i32) -> i32 {
    BACKGROUND_LAYER_ID - slot
}

pub(crate) fn sprite_fade_frames(class: ScenarioSpriteClass, wait_frames: u32) -> u32 {
    match class {
        ScenarioSpriteClass::Ui | ScenarioSpriteClass::Other if wait_frames == 0 => 1,
        _ => wait_frames.max(1),
    }
}

pub(crate) fn sprite_target_opacity(name: &str, class: ScenarioSpriteClass) -> f32 {
    let lower = name.to_ascii_lowercase();
    match class {
        ScenarioSpriteClass::FadePlate => 1.0,
        ScenarioSpriteClass::Effect if lower.starts_with("ef_soft") => 0.28,
        ScenarioSpriteClass::Effect if lower.starts_with("ef_sepia") => 0.42,
        ScenarioSpriteClass::Effect => 0.5,
        _ => 1.0,
    }
}

fn looks_like_character_resource(lower: &str) -> bool {
    lower.starts_with("ll_")
        || lower.starts_with("l_")
        || lower.starts_with("m_")
        || lower.starts_with("s_")
        || lower.starts_with("lm_")
        || lower.starts_with("ml_")
        || lower.starts_with("mm_")
        || lower.starts_with("sl_")
        || lower.starts_with("sm_")
        || {
            let mut chars = lower.chars();
            matches!(chars.next(), Some('c' | 'h' | 'm' | 'f'))
                && lower.chars().any(|ch| matches!(ch, '0'..='9' | 'a'..='z'))
        }
}
