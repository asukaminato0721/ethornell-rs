use ethornell_script::bcs::{parse_bcs, BcsCommand, BcsValue};
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone)]
pub(crate) enum ScenarioAction {
    Sound {
        file: String,
    },
    Bgm {
        file: String,
    },
    Sprite {
        file: String,
        wait_frames: u32,
        slot: Option<i32>,
        x: Option<i32>,
        z: Option<i32>,
        opacity: Option<f32>,
        body_layer: Option<&'static str>,
    },
    TransformSprite {
        slot: i32,
        wait_frames: u32,
        x: Option<i32>,
        y: Option<i32>,
        opacity: Option<f32>,
    },
    HideSprite {
        slot: Option<i32>,
        wait_frames: u32,
    },
    Message {
        speaker: Option<String>,
        text: String,
    },
    Wait {
        frames: u32,
    },
    WaitForInput,
    ClearSprite,
    LoadScript {
        file: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ScenarioPlayback {
    actions: VecDeque<ScenarioAction>,
    wait_frames: u32,
    waiting_for_input: bool,
}

impl ScenarioPlayback {
    pub(crate) fn from_bcs(bytes: &[u8]) -> Option<Self> {
        let program = parse_bcs(bytes)?;
        let mut builder = ScenarioActionBuilder::default();
        for command in &program.commands {
            builder.observe(command);
        }
        let actions = builder.actions.into();
        Some(Self {
            actions,
            wait_frames: 0,
            waiting_for_input: false,
        })
    }

    pub(crate) fn action_count(&self) -> usize {
        self.actions.len()
    }

    pub(crate) fn is_waiting_for_input(&self) -> bool {
        self.waiting_for_input
    }

    pub(crate) fn append_bcs(&mut self, bytes: &[u8]) -> Option<usize> {
        let program = parse_bcs(bytes)?;
        let mut builder = ScenarioActionBuilder::default();
        for command in &program.commands {
            builder.observe(command);
        }
        let added = builder.actions.len();
        self.actions.extend(builder.actions);
        Some(added)
    }

    pub(crate) fn tick(&mut self, input_advance: bool) -> (Option<ScenarioAction>, bool) {
        let mut input_consumed = false;

        loop {
            if self.waiting_for_input {
                if !input_advance {
                    return (None, input_consumed);
                }
                self.waiting_for_input = false;
                input_consumed = true;
            }

            if self.wait_frames > 0 {
                self.wait_frames -= 1;
                return (None, input_consumed);
            }

            let Some(action) = self.actions.pop_front() else {
                return (None, input_consumed);
            };
            match action {
                ScenarioAction::Wait { frames } => {
                    self.wait_frames = frames;
                    return (None, input_consumed);
                }
                ScenarioAction::WaitForInput => {
                    if input_advance {
                        input_consumed = true;
                        continue;
                    }
                    self.waiting_for_input = true;
                    return (None, input_consumed);
                }
                action => return (Some(action), input_consumed),
            }
        }
    }
}

#[derive(Debug, Default)]
struct ScenarioActionBuilder {
    actions: Vec<ScenarioAction>,
    scheduled_actions: Vec<ScenarioAction>,
    characters: BTreeMap<i32, CharacterState>,
    stack: Vec<BcsValue>,
    pending_arg_count: Option<usize>,
}

impl ScenarioActionBuilder {
    fn observe(&mut self, command: &BcsCommand) {
        if is_push_command(command) {
            self.stack.extend(command.args.iter().cloned());
            return;
        }
        if matches!(command.name, Some("nargs")) {
            self.pending_arg_count = command
                .args
                .iter()
                .filter_map(value_i32)
                .last()
                .and_then(|value| usize::try_from(value).ok());
            return;
        }

        if self.observe_stack_operator(command) {
            return;
        }

        if command.opcode == 0x1c {
            let context = self.take_script_call_context();
            self.observe_script_call(&context);
        } else if matches!(
            command.name,
            Some("sprite" | "bg" | "bg_transition" | "bg240")
        ) {
            let args = self.take_pending_args();
            self.observe_visual_command(&args);
        } else if matches!(command.name, Some("sprite_hide")) {
            let args = self.take_pending_args();
            self.observe_sprite_hide(&args);
        } else if matches!(command.name, Some("wait")) {
            let args = self.take_pending_args();
            self.observe_wait(&args);
        } else if matches!(command.name, Some("say" | "msg")) {
            let args = self.take_message_args(command);
            self.observe_message(&args);
        } else if matches!(command.name, Some("sound" | "sound_1a0" | "snd")) {
            let args = self.take_pending_args();
            self.observe_sound_command(command, &args);
        } else if matches!(command.name, Some("exec_script")) {
            let args = self.take_pending_args();
            self.observe_script_jump(&args);
        } else if command.opcode == 0xf3 {
            let args = self.take_script_jump_context();
            self.observe_script_jump(&args);
        } else if should_consume_pending_args(command) {
            let _ = self.take_pending_args();
        }
    }

    fn observe_script_call(&mut self, args: &[BcsValue]) {
        let Some(function) = last_string(args).filter(|text| text.starts_with('_')) else {
            return;
        };
        match function {
            "_PlaySE" => {
                if let Some(file) = strings(args)
                    .into_iter()
                    .rev()
                    .find(|text| text.starts_with("se_"))
                {
                    self.actions.push(ScenarioAction::Sound {
                        file: file.to_string(),
                    });
                }
            }
            "_PlayVoice" => {
                if let Some(file) = strings(args)
                    .into_iter()
                    .rev()
                    .find(|text| looks_like_resource_name(text))
                {
                    self.actions.push(ScenarioAction::Sound {
                        file: file.to_string(),
                    });
                }
            }
            "_PlayBGM" => {
                if let Some(file) = strings(args)
                    .into_iter()
                    .rev()
                    .find(|text| looks_like_resource_name(text))
                {
                    self.actions.push(ScenarioAction::Bgm {
                        file: file.to_string(),
                    });
                }
            }
            "_DrawScene" => {
                if let Some(file) = visual_resource_name(args) {
                    self.actions.push(ScenarioAction::ClearSprite);
                    self.push_sprite_with_hints(
                        file.to_string(),
                        duration_frames(args).unwrap_or(1),
                        draw_scene_hints(args),
                    );
                }
            }
            "_SetBSsize" => {
                let ints = ints(args);
                if let Some((slot, size)) = last_two(&ints) {
                    self.characters.entry(slot).or_default().size = Some(size);
                }
            }
            "_SetClothes" => {
                let ints = ints(args);
                if let Some((slot, clothes)) = last_two(&ints) {
                    self.characters.entry(slot).or_default().clothes = Some(clothes);
                }
            }
            "_BS_P" | "_BS" => {
                if let Some(sprite) = self.character_sprite_from_bs(args) {
                    self.schedule_sprite_with_hints(
                        sprite.file,
                        sprite.wait_frames,
                        VisualHints {
                            slot: Some(sprite.slot),
                            x: sprite.x,
                            z: Some(70 + sprite.slot),
                            opacity: None,
                            body_layer: sprite.body_layer,
                        },
                    );
                }
            }
            "_MBS_P" | "_MBS" | "_FBS_P" | "_FBS" => {
                if let Some(transform) = transform_hints(args) {
                    self.scheduled_actions
                        .push(ScenarioAction::TransformSprite {
                            slot: transform.slot,
                            wait_frames: transform.wait_frames,
                            x: transform.x,
                            y: transform.y,
                            opacity: transform.opacity,
                        });
                } else if let Some(frames) = duration_frames(args) {
                    self.actions.push(ScenarioAction::Wait { frames });
                }
            }
            "_SPR_P" => {
                if let Some(file) = visual_resource_name(args) {
                    let hints = visual_hints(args);
                    self.schedule_sprite_with_hints(
                        file.to_string(),
                        duration_frames(args).unwrap_or(1),
                        hints,
                    );
                }
            }
            "_SPR" => {
                if let Some(file) = visual_resource_name(args) {
                    let hints = visual_hints(args);
                    self.push_sprite_with_hints(
                        file.to_string(),
                        duration_frames(args).unwrap_or(1),
                        hints,
                    );
                }
            }
            "_FSPR_P" | "_FSPR" => {
                if let Some(transform) = transform_hints(args) {
                    self.scheduled_actions
                        .push(ScenarioAction::TransformSprite {
                            slot: transform.slot,
                            wait_frames: transform.wait_frames,
                            x: transform.x,
                            y: transform.y,
                            opacity: transform.opacity,
                        });
                }
            }
            "_MSPR_P" | "_MSPR" => {
                if let Some(transform) = transform_hints(args) {
                    self.scheduled_actions
                        .push(ScenarioAction::TransformSprite {
                            slot: transform.slot,
                            wait_frames: transform.wait_frames,
                            x: transform.x,
                            y: transform.y,
                            opacity: transform.opacity,
                        });
                } else if let Some(frames) = duration_frames(args) {
                    self.actions.push(ScenarioAction::Wait { frames });
                }
            }
            "_Exec_P" | "_Exec" | "_ExecuteScheduledControl" => {
                let scheduled_wait = self.flush_scheduled_actions();
                if let Some(frames) = duration_frames(args).or(scheduled_wait) {
                    self.actions.push(ScenarioAction::Wait { frames });
                }
            }
            "_FadeScene" => {
                let frames = duration_frames(args).unwrap_or(1);
                self.actions.push(ScenarioAction::HideSprite {
                    slot: None,
                    wait_frames: frames,
                });
                self.actions.push(ScenarioAction::Wait { frames });
            }
            "_Wait" | "_WaitReturnValued" => {
                if function == "_Wait" {
                    if let Some(frames) = duration_frames(args) {
                        self.actions.push(ScenarioAction::Wait { frames });
                    }
                } else {
                    self.actions.push(ScenarioAction::WaitForInput);
                }
            }
            _ => {}
        }
    }

    fn observe_visual_command(&mut self, args: &[BcsValue]) {
        if let Some(file) = visual_resource_name(args) {
            self.push_sprite_with_hints(
                file.to_string(),
                duration_frames(args).unwrap_or(1),
                visual_hints(args),
            );
        }
    }

    fn observe_sprite_hide(&mut self, args: &[BcsValue]) {
        let hints = visual_hints(args);
        let frames = duration_frames(args).unwrap_or(1);
        self.actions.push(ScenarioAction::HideSprite {
            slot: hints.slot,
            wait_frames: frames,
        });
        self.actions.push(ScenarioAction::Wait { frames });
    }

    fn observe_wait(&mut self, args: &[BcsValue]) {
        if let Some(frames) = duration_frames(args) {
            self.actions.push(ScenarioAction::Wait { frames });
        }
    }

    fn observe_message(&mut self, args: &[BcsValue]) {
        let mut speaker = None;
        for text in strings(args) {
            if is_message_text(text) {
                if is_message_marker(text) {
                    continue;
                }
                if is_speaker_label(text) {
                    speaker = Some(text.to_string());
                    continue;
                }
                self.actions.push(ScenarioAction::Message {
                    speaker: speaker.take(),
                    text: text.to_string(),
                });
                if should_wait_after_message(text) {
                    self.actions.push(ScenarioAction::WaitForInput);
                }
            }
        }
    }

    fn observe_sound_command(&mut self, command: &BcsCommand, args: &[BcsValue]) {
        let Some(file) = strings(args)
            .into_iter()
            .rev()
            .find(|text| looks_like_resource_name(text))
        else {
            return;
        };
        if matches!(command.name, Some("sound" | "sound_1a0"))
            && file.to_ascii_lowercase().starts_with("bgm")
        {
            self.actions.push(ScenarioAction::Bgm {
                file: file.to_string(),
            });
        } else {
            self.actions.push(ScenarioAction::Sound {
                file: file.to_string(),
            });
        }
    }

    fn push_sprite(&mut self, file: String, frames: u32) {
        self.push_sprite_with_hints(file, frames, VisualHints::default());
    }

    fn push_sprite_with_hints(&mut self, file: String, frames: u32, hints: VisualHints) {
        self.actions.push(ScenarioAction::Sprite {
            file,
            wait_frames: frames,
            slot: hints.slot,
            x: hints.x,
            z: hints.z,
            opacity: hints.opacity,
            body_layer: hints.body_layer,
        });
        self.actions.push(ScenarioAction::Wait { frames });
    }

    fn schedule_sprite_with_hints(&mut self, file: String, frames: u32, hints: VisualHints) {
        self.scheduled_actions.push(ScenarioAction::Sprite {
            file,
            wait_frames: frames,
            slot: hints.slot,
            x: hints.x,
            z: hints.z,
            opacity: hints.opacity,
            body_layer: hints.body_layer,
        });
    }

    fn flush_scheduled_actions(&mut self) -> Option<u32> {
        if self.scheduled_actions.is_empty() {
            return None;
        }
        let wait_frames = self
            .scheduled_actions
            .iter()
            .filter_map(action_wait_frames)
            .max();
        self.actions.append(&mut self.scheduled_actions);
        wait_frames
    }

    fn observe_script_jump(&mut self, args: &[BcsValue]) {
        let Some(file) = strings(args)
            .into_iter()
            .rev()
            .find(|text| looks_like_script_name(text))
        else {
            return;
        };
        self.actions.push(ScenarioAction::LoadScript {
            file: file.to_string(),
        });
    }

    fn character_sprite_from_bs(&self, args: &[BcsValue]) -> Option<CharacterSpriteCall> {
        let slot = value_i32(args.first()?)?;
        let body = args.get(1).and_then(value_i32).unwrap_or(1);
        let names = args
            .iter()
            .filter_map(|value| match value {
                BcsValue::Str(text)
                    if !text.starts_with('_')
                        && !text.ends_with(".txt")
                        && !text.eq_ignore_ascii_case("macro_story") =>
                {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let pose = names.first().copied().unwrap_or("a");
        let face = names.get(1).copied()?;
        let state = self.characters.get(&slot).copied().unwrap_or_default();
        let character = character_name(slot)?;
        let size = size_prefix(state.size.unwrap_or(2));
        let clothes = state.clothes.unwrap_or(1).clamp(1, 9);
        let wait_frames = duration_frames(args).unwrap_or(1);
        Some(CharacterSpriteCall {
            file: format!(
                "{size}_{character}_{clothes}_d_{}{}",
                body.clamp(1, 9),
                face
            ),
            wait_frames,
            slot,
            x: None,
            body_layer: character_body_layer_for_pose(pose),
        })
    }

    fn take_pending_args(&mut self) -> Vec<BcsValue> {
        let Some(count) = self.pending_arg_count.take() else {
            return Vec::new();
        };
        let split = self.stack.len().saturating_sub(count);
        self.stack.split_off(split)
    }

    fn take_message_args(&mut self, command: &BcsCommand) -> Vec<BcsValue> {
        if self.pending_arg_count.is_some() {
            return self.take_pending_args();
        }
        let fallback_count = match command.name {
            Some("say") => 5,
            Some("msg") => 0,
            _ => 0,
        };
        if fallback_count == 0 || self.stack.len() < fallback_count {
            return Vec::new();
        }
        let split = self.stack.len() - fallback_count;
        self.stack.split_off(split)
    }

    fn take_script_call_context(&mut self) -> Vec<BcsValue> {
        let Some(function_index) = self
            .stack
            .iter()
            .rposition(|value| matches!(value, BcsValue::Str(text) if text.starts_with('_')))
        else {
            return Vec::new();
        };
        let start = self
            .stack
            .iter()
            .take(function_index)
            .rposition(|value| matches!(value, BcsValue::Str(text) if text.ends_with(".txt")))
            .map(|index| {
                let after_source_line = index.saturating_add(2);
                after_source_line.min(function_index)
            })
            .unwrap_or_else(|| function_index.saturating_sub(40));
        self.stack.split_off(start)
    }

    fn take_script_jump_context(&mut self) -> Vec<BcsValue> {
        if self.pending_arg_count.is_some() {
            return self.take_pending_args();
        }
        let Some(script_index) = self.stack.iter().rposition(
            |value| matches!(value, BcsValue::Str(text) if looks_like_script_name(text)),
        ) else {
            return Vec::new();
        };
        if self.stack.len().saturating_sub(script_index) > 4 {
            return Vec::new();
        }
        self.stack.split_off(script_index)
    }

    fn observe_stack_operator(&mut self, command: &BcsCommand) -> bool {
        match command.name {
            Some("add") | Some("sub") | Some("mul") | Some("div") | Some("mod") => {
                let right = self.stack.pop();
                let left = self.stack.pop();
                if let (Some(left), Some(right)) = (left, right) {
                    if let (Some(left), Some(right)) = (value_i32(&left), value_i32(&right)) {
                        let value = match command.name {
                            Some("add") => left.saturating_add(right),
                            Some("sub") => left.saturating_sub(right),
                            Some("mul") => left.saturating_mul(right),
                            Some("div") if right != 0 => left / right,
                            Some("mod") if right != 0 => left % right,
                            _ => 0,
                        };
                        self.stack.push(BcsValue::Int(value));
                    } else {
                        self.stack.push(left);
                        self.stack.push(right);
                    }
                }
                true
            }
            Some("bool_zero") => {
                if let Some(value) = self.stack.pop() {
                    let value = value_i32(&value).map(|value| value == 0).unwrap_or(false);
                    self.stack.push(BcsValue::Int(i32::from(value)));
                }
                true
            }
            Some("jc") | Some("jmp") => {
                let _ = self.stack.pop();
                true
            }
            Some("check_translator_note") => true,
            _ => false,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct CharacterState {
    size: Option<i32>,
    clothes: Option<i32>,
}

#[derive(Debug, Clone)]
struct CharacterSpriteCall {
    file: String,
    wait_frames: u32,
    slot: i32,
    x: Option<i32>,
    body_layer: Option<&'static str>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct VisualHints {
    pub(crate) slot: Option<i32>,
    pub(crate) x: Option<i32>,
    pub(crate) z: Option<i32>,
    pub(crate) opacity: Option<f32>,
    pub(crate) body_layer: Option<&'static str>,
}

fn is_push_command(command: &BcsCommand) -> bool {
    matches!(
        command.name,
        Some("push_dword" | "push_offset" | "push_base_offset" | "push_string" | "line")
    )
}

fn should_consume_pending_args(command: &BcsCommand) -> bool {
    matches!(
        command.name,
        Some(
            "sys"
                | "snd"
                | "grp"
                | "slct"
                | "cmd0x120"
                | "cmd0x121"
                | "cmd0x126"
                | "set_font"
                | "cmd0x151"
                | "sound"
                | "sound_1a0"
                | "img_hide"
                | "fx_smth1"
                | "cmd0x1b1"
                | "char_act"
                | "set_script_file"
                | "set_voice_seq"
                | "play_movie"
                | "cmd0x230"
                | "fade_to_black"
                | "transition"
                | "sprite_hide_all"
                | "cmd0x340"
        )
    ) || matches!(command.opcode, 0xe0 | 0xe2 | 0xe3 | 0xe5 | 0xe6 | 0xfe)
}

fn strings(values: &[BcsValue]) -> Vec<&str> {
    let mut out = Vec::new();
    for value in values {
        collect_strings(value, &mut out);
    }
    out
}

fn ints(values: &[BcsValue]) -> Vec<i32> {
    values.iter().filter_map(value_i32).collect()
}

fn last_two(values: &[i32]) -> Option<(i32, i32)> {
    Some((*values.get(values.len().checked_sub(2)?)?, *values.last()?))
}

fn collect_strings<'a>(value: &'a BcsValue, out: &mut Vec<&'a str>) {
    match value {
        BcsValue::Str(text) => out.push(text.as_str()),
        BcsValue::Mul(left, right) => {
            collect_strings(left, out);
            collect_strings(right, out);
        }
        BcsValue::CheckNote(value) => collect_strings(value, out),
        _ => {}
    }
}

fn last_string(values: &[BcsValue]) -> Option<&str> {
    strings(values).into_iter().last()
}

fn visual_resource_name(values: &[BcsValue]) -> Option<&str> {
    strings(values).into_iter().rev().find(|text| {
        looks_like_resource_name(text)
            && !text.starts_with("se_")
            && !text.starts_with("bgm")
            && !text.starts_with("BGM")
    })
}

fn character_name(slot: i32) -> Option<&'static str> {
    match slot {
        1 => Some("koh"),
        3 => Some("hih"),
        15 => Some("tay"),
        _ => None,
    }
}

fn size_prefix(size: i32) -> &'static str {
    match size {
        0 | 1 => "L",
        3 => "M",
        4 => "S",
        _ => "LL",
    }
}

fn character_body_layer_for_pose(pose: &str) -> Option<&'static str> {
    match pose {
        "a" => Some("ax11"),
        "d" => Some("ax12"),
        "c" => Some("ax14"),
        _ => None,
    }
}

fn first_position_hint(values: &[BcsValue]) -> Option<i32> {
    values
        .iter()
        .filter_map(value_i32)
        .find(|value| (-640..=640).contains(value) && *value != 0 && *value != 1 && *value != 256)
}

fn visual_hints(values: &[BcsValue]) -> VisualHints {
    let file_index = values
        .iter()
        .rposition(|value| matches!(value, BcsValue::Str(text) if looks_like_resource_name(text)));
    let ints = values.iter().filter_map(value_i32).collect::<Vec<_>>();
    if let Some(file_index) = file_index {
        let before_file = values[..file_index]
            .iter()
            .filter_map(value_i32)
            .collect::<Vec<_>>();
        let after_file = values[file_index + 1..]
            .iter()
            .filter_map(value_i32)
            .collect::<Vec<_>>();
        let duration = duration_frames(values).and_then(|frames| i32::try_from(frames * 16).ok());
        let slot = before_file
            .last()
            .copied()
            .filter(|slot| (0..=99).contains(slot))
            .or_else(|| {
                after_file
                    .last()
                    .copied()
                    .filter(|slot| (0..=99).contains(slot))
            });
        let z = after_file.iter().rev().copied().find(|value| {
            Some(*value) != slot
                && Some(*value) != duration
                && *value != 256
                && (2..=899).contains(value)
        });
        return VisualHints {
            slot,
            x: after_file.iter().copied().find(|x| {
                Some(*x) != duration
                    && Some(*x) != slot
                    && *x != 0
                    && *x != 1
                    && *x != 256
                    && (-1280..=1280).contains(x)
            }),
            z,
            opacity: None,
            body_layer: None,
        };
    }

    let slot = ints
        .iter()
        .rev()
        .copied()
        .find(|value| (0..=99).contains(value));
    let x = file_index.and_then(|index| {
        values
            .iter()
            .skip(index + 1)
            .filter_map(value_i32)
            .find(|value| (-640..=640).contains(value))
    });
    let z = ints
        .iter()
        .rev()
        .copied()
        .find(|value| (1..=160).contains(value));
    VisualHints {
        slot,
        x,
        z,
        opacity: None,
        body_layer: None,
    }
}

fn draw_scene_hints(values: &[BcsValue]) -> VisualHints {
    let mut hints = visual_hints(values);
    hints.slot = None;
    hints.x = None;
    hints.z = Some(10);
    hints.opacity = None;
    hints
}

#[derive(Debug, Clone, Copy)]
struct TransformHints {
    slot: i32,
    wait_frames: u32,
    x: Option<i32>,
    y: Option<i32>,
    opacity: Option<f32>,
}

fn transform_hints(values: &[BcsValue]) -> Option<TransformHints> {
    let ints = values.iter().filter_map(value_i32).collect::<Vec<_>>();
    let slot = ints
        .first()
        .copied()
        .filter(|slot| (0..=99).contains(slot))?;
    let wait_frames = duration_frames(values).unwrap_or(1);
    let duration_ms = duration_millis(values);
    let positive_opacity =
        ints.iter().rev().copied().find(|value| {
            (2..=256).contains(value) && *value != slot && Some(*value) != duration_ms
        });
    let opacity = positive_opacity.map(script_opacity);
    let coord_values = ints
        .iter()
        .skip(1)
        .copied()
        .filter(|value| {
            Some(*value) != duration_ms
                && Some(*value) != positive_opacity
                && *value != i32::MIN + 1
                && (-1280..=1280).contains(value)
        })
        .collect::<Vec<_>>();
    let x = coord_values
        .iter()
        .copied()
        .find(|value| *value != -1 && *value != 0 && *value != 1 && *value != 256);
    Some(TransformHints {
        slot,
        wait_frames,
        x,
        y: None,
        opacity,
    })
}

fn script_opacity(value: i32) -> f32 {
    (value as f32 / 256.0).clamp(0.0, 1.0)
}

fn action_wait_frames(action: &ScenarioAction) -> Option<u32> {
    match action {
        ScenarioAction::Sprite { wait_frames, .. }
        | ScenarioAction::HideSprite { wait_frames, .. }
        | ScenarioAction::TransformSprite { wait_frames, .. } => Some(*wait_frames),
        ScenarioAction::Wait { frames } => Some(*frames),
        _ => None,
    }
}

fn looks_like_resource_name(text: &str) -> bool {
    !text.is_empty()
        && !text.ends_with(".txt")
        && !text.starts_with('_')
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn looks_like_script_name(text: &str) -> bool {
    matches!(text, "_GM" | "_00")
        || (!text.is_empty()
            && !text.ends_with(".txt")
            && !text.starts_with('_')
            && text
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
}

fn duration_frames(values: &[BcsValue]) -> Option<u32> {
    duration_millis(values).map(|duration| (duration as u32).div_ceil(16).max(1))
}

fn duration_millis(values: &[BcsValue]) -> Option<i32> {
    values
        .iter()
        .filter_map(value_i32)
        .filter(|value| (16..=30_000).contains(value))
        .last()
}

fn value_i32(value: &BcsValue) -> Option<i32> {
    match value {
        BcsValue::Int(value) | BcsValue::Addr(value) | BcsValue::BaseOffset(value) => Some(*value),
        BcsValue::Mul(left, right) => Some(value_i32(left)? * value_i32(right)?),
        BcsValue::CheckNote(value) => value_i32(value),
        BcsValue::Line { .. } | BcsValue::Arg2 | BcsValue::Str(_) => None,
    }
}

fn is_message_text(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('_')
        && !text.ends_with(".txt")
        && !text.starts_with("se_")
        && !text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_message_marker(text: &str) -> bool {
    matches!(text, "空")
}

fn is_speaker_label(text: &str) -> bool {
    let char_count = text.chars().count();
    char_count <= 12
        && !text.contains('\n')
        && !text.chars().any(|ch| {
            matches!(
                ch,
                '「' | '」'
                    | '『'
                    | '』'
                    | '（'
                    | '）'
                    | '。'
                    | '、'
                    | '，'
                    | '．'
                    | '！'
                    | '？'
                    | '!'
                    | '?'
                    | '…'
                    | 'ー'
            )
        })
}

fn should_wait_after_message(text: &str) -> bool {
    !is_message_marker(text) && !is_speaker_label(text)
}
