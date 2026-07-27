use super::{value_to_i32, RuntimeTraceApi};
use crate::graph::{RuntimeGraphLayer, RuntimeUserControl};
use crate::text_anim::{normalize_message_text, parse_message_markup, RuntimeRubySpan};

const MESSAGE_TEXT_NODE_ID: i32 = -20_000;
const MESSAGE_NAME_TEXT_NODE_ID: i32 = -20_001;
const MESSAGE_OVERLAY_TEXT_NODE_ID: i32 = -20_050;
pub(crate) const MESSAGE_WINDOW_LAYER_ID: i32 = -20_010;
pub(crate) const MESSAGE_NAME_WINDOW_LAYER_ID: i32 = -20_011;
const MESSAGE_WINDOW_KEY: &str = "sysgrp.arc:SGMsgWnd800000";
const MESSAGE_WINDOW_RESOURCE: &str = "SGMsgWnd800000";
const MESSAGE_NAME_WINDOW_KEY: &str = "sysgrp.arc:SGMsgWnd700000";
const MESSAGE_NAME_WINDOW_RESOURCE: &str = "SGMsgWnd700000";
const MESSAGE_WINDOW_Z: i32 = 900;
const MESSAGE_NAME_WINDOW_Z: i32 = 901;
pub(crate) const MESSAGE_TEXT_Z: i32 = 3_000;
pub(crate) const MESSAGE_NAME_TEXT_Z: i32 = 3_001;
const MESSAGE_INPUT_OBJECT_ID: i32 = 50_000;
const MESSAGE_NAME_WINDOW_X: f32 = 0.0;
const MESSAGE_NAME_WINDOW_Y: f32 = 466.0;
const MESSAGE_NAME_TEXT_X: f32 = 72.0;
const MESSAGE_NAME_TEXT_Y: f32 = 525.0;
pub(crate) const MESSAGE_CONTROL_OWNER_ID: i32 = -20_020;
const MESSAGE_CONTROL_NORMAL_RESOURCE: &str = "SGMsgWnd000000";
const MESSAGE_CONTROL_HOVER_RESOURCE: &str = "SGMsgWnd000001";
const MESSAGE_CONTROL_ACTIVE_RESOURCE: &str = "SGMsgWnd000002";
const MESSAGE_CONTROL_DISABLED_RESOURCE: &str = "SGMsgWnd000003";

const MESSAGE_CONTROL_TEMPLATES: [MessageControlTemplate; 11] = [
    MessageControlTemplate::new(1036, 8, 6.0, 553.0, 380.0, 60.0),
    MessageControlTemplate::new(1052, 9, 413.0, 549.0, 65.0, 55.0),
    MessageControlTemplate::new(1048, 10, 488.0, 549.0, 55.0, 55.0),
    MessageControlTemplate::new(1056, 11, 554.0, 549.0, 55.0, 55.0),
    MessageControlTemplate::new(1060, 12, 620.0, 549.0, 81.0, 55.0),
    MessageControlTemplate::new(1024, 13, 731.0, 549.0, 65.0, 55.0),
    MessageControlTemplate::new(1040, 14, 803.0, 549.0, 76.0, 55.0),
    MessageControlTemplate::new(1028, 15, 890.0, 549.0, 67.0, 55.0),
    MessageControlTemplate::new(1044, 16, 963.0, 549.0, 79.0, 55.0),
    MessageControlTemplate::new(1032, 17, 1075.0, 549.0, 75.0, 55.0),
    MessageControlTemplate::new(1072, 18, 1168.0, 549.0, 67.0, 55.0),
];

#[derive(Debug, Clone, Copy)]
struct MessageControlTemplate {
    layer_id: i32,
    payload: i32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl MessageControlTemplate {
    const fn new(layer_id: i32, payload: i32, x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            layer_id,
            payload,
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeTextNode {
    pub(crate) text: String,
    pub(crate) enabled: bool,
    pub(crate) screen_attached: bool,
    pub(crate) target_surface: Option<i32>,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) size: f32,
    pub(crate) color: [f32; 4],
    pub(crate) z: i32,
    pub(crate) ruby_spans: Vec<RuntimeRubySpan>,
}

#[derive(Debug, Clone)]
pub(crate) struct TextState {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) font_size: f32,
    pub(crate) color: [f32; 4],
    pub(crate) line_height: f32,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            x: 54.0,
            y: 616.0,
            width: 1160.0,
            height: 82.0,
            font_size: 26.0,
            color: [1.0, 1.0, 1.0, 1.0],
            line_height: 32.0,
        }
    }
}

impl TextState {
    pub(crate) fn message_window() -> Self {
        Self::default()
    }
}

impl RuntimeTraceApi {
    pub(crate) fn observe_user_message(
        &mut self,
        group: u8,
        id: u16,
        stack: &[ethornell_vm::Value],
    ) {
        if (group, id) != (0xb0, 0x80) {
            return;
        }
        let text = stack.iter().rev().find_map(value_to_text_string);
        self.trace_graph(format!("native message box suppressed text={text:?}"));
    }

    pub(crate) fn render_graph_text(&mut self, args: &[ethornell_vm::Value]) {
        let Some(text) = args.iter().find_map(value_to_text_string) else {
            self.trace_graph(format!(
                "RenderText without string args={:?}",
                args.iter().map(value_to_i32).collect::<Vec<_>>()
            ));
            return;
        };
        if text.is_empty() {
            return;
        }
        if args.len() == 21 {
            let target = args.get(20).map(value_to_i32).unwrap_or_default();
            if target > 0 && self.graph_surfaces.contains_key(&target) {
                let x = args.get(19).map(value_to_i32).unwrap_or_default() as f32;
                let y = args.get(18).map(value_to_i32).unwrap_or_default() as f32;
                let size = args
                    .get(11)
                    .map(value_to_i32)
                    .filter(|size| (8..=96).contains(size))
                    .unwrap_or(self.text_state.font_size as i32) as f32;
                let color = args
                    .get(16)
                    .map(value_to_i32)
                    .filter(|color| (0..=0xFF_FFFF).contains(color))
                    .map(|color| {
                        [
                            ((color >> 16) & 0xff) as f32 / 255.0,
                            ((color >> 8) & 0xff) as f32 / 255.0,
                            (color & 0xff) as f32 / 255.0,
                            1.0,
                        ]
                    })
                    .unwrap_or(self.text_state.color);
                let normalized = normalize_message_text(&text);
                self.text_nodes.insert(
                    target,
                    RuntimeTextNode {
                        text: normalized.clone(),
                        enabled: true,
                        screen_attached: false,
                        target_surface: None,
                        x,
                        y,
                        size,
                        color,
                        z: target,
                        ruby_spans: Vec::new(),
                    },
                );
                tracing::info!(target, x, y, size, text = %normalized, "RenderBitmapText");
                self.trace_graph(format!(
                    "render text into bitmap #{target} at ({x:.0},{y:.0}) {normalized:?}"
                ));
                return;
            }
        }
        let target = self.ensure_message_text_node();
        self.apply_text_to_node(target, normalize_message_text(&text));
        tracing::info!(
            target,
            text = %normalize_message_text(&text),
            state = ?self.text_state,
            "RenderTextNode"
        );
        self.trace_graph(format!(
            "render text node #{target} {:?}",
            normalize_message_text(&text)
        ));
    }

    pub(crate) fn start_scenario_message(&mut self, speaker: Option<String>, text: String) {
        if text.is_empty() {
            return;
        }
        self.text_state = TextState::message_window();
        self.ensure_message_window_layer();
        self.ensure_message_control_layers();
        if let Some(speaker) = speaker.filter(|speaker| !speaker.is_empty()) {
            self.ensure_message_name_layer();
            let target = self.ensure_message_name_text_node();
            self.apply_name_to_node(target, speaker);
        } else {
            self.hide_message_name();
        }
        let target = self.ensure_message_text_node();
        let (display_text, ruby_spans) = parse_and_wrap_message(&text, &self.text_state);
        self.text_runtime
            .start_styled_message(display_text, ruby_spans, target);
        self.apply_text_to_node(target, String::new());
        if self.graph_defaults.instant_reveal {
            self.reveal_text_on_input();
        }
        tracing::info!(target, text = %normalize_message_text(&text), "ScenarioMessage");
        self.trace_graph(format!(
            "scenario message node #{target} start {:?}",
            normalize_message_text(&text)
        ));
    }

    pub(crate) fn start_native_message(&mut self, text: String) -> i32 {
        let (text, ruby_spans) = parse_and_wrap_message(&text, &self.text_state);
        if text.is_empty() {
            return 1;
        }
        self.ensure_message_window_layer();
        self.user_controls.remove(&MESSAGE_INPUT_OBJECT_ID);
        self.ensure_message_control_layers();
        let target = self.ensure_message_text_node();
        self.text_runtime
            .start_styled_message(text.clone(), ruby_spans, target);
        self.apply_text_to_node(target, String::new());
        if self.graph_defaults.instant_reveal {
            self.reveal_text_on_input();
        }
        self.native_message_active = true;
        let duration_ms = if self.graph_defaults.instant_reveal {
            1
        } else {
            self.text_runtime.duration_ms()
        };
        tracing::info!(target, duration_ms, text = %text, "NativeMessage");
        self.trace_graph(format!(
            "native message node #{target} duration={duration_ms}ms {text:?}"
        ));
        duration_ms
    }

    pub(crate) fn render_native_message_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.ensure_message_window_layer();
        self.ensure_message_control_layers();
        let target = self.ensure_message_text_node();
        let (display_text, ruby_spans) = parse_and_wrap_message(text, &self.text_state);
        self.apply_text_to_node(target, display_text);
        if let Some(node) = self.text_nodes.get_mut(&target) {
            node.ruby_spans = ruby_spans;
        }
        self.native_message_active = true;
    }

    pub(crate) fn reset_native_message_text(&mut self) {
        self.text_runtime = Default::default();
        if let Some(node) = self.text_nodes.get_mut(&MESSAGE_TEXT_NODE_ID) {
            node.text.clear();
            node.ruby_spans.clear();
        }
        self.native_message_active = false;
    }

    pub(crate) fn tick_text(&mut self) {
        if let Some((target, text)) = self.text_runtime.tick() {
            self.apply_text_to_node(target, text);
            self.apply_visible_ruby_to_node(target);
        }
    }

    pub(crate) fn handle_message_control_input(&mut self) -> bool {
        // Native BCS programs own message-window control dispatch through
        // GraphPollObjectEvent. These fallback actions are only for the
        // optional shadow scenario interpreter.
        if self.scenario_playback.is_none() {
            return false;
        }
        let Some(point) = self.pending_click else {
            return false;
        };
        let Some(control) = self
            .user_controls
            .values()
            .rev()
            .find(|control| {
                control.owner_id == MESSAGE_CONTROL_OWNER_ID
                    && control.contains(point, false)
                    && self.has_active_message_window()
            })
            .copied()
        else {
            return false;
        };

        self.pending_click = None;
        self.pending_object_state = None;
        self.pending_input_state = None;
        self.pending_input_descriptor = None;
        self.pending_input_consumed = false;
        self.mouse_pressed = false;
        self.last_hit_control = control.id;
        self.last_hit_payload = control.payload;

        match control.payload {
            8 => {
                if let Some(sound) = self.last_scenario_sound.clone() {
                    self.queue_scenario_sound(&sound);
                    self.trace_graph(format!("message control voice replay {sound}"));
                } else {
                    self.trace_graph("message control voice replay without cached voice");
                }
            }
            9 => {
                self.scenario_auto_mode = !self.scenario_auto_mode;
                self.scenario_skip_mode = false;
                self.scenario_auto_wait_frames = 0;
                self.trace_graph(format!(
                    "message control auto mode={}",
                    self.scenario_auto_mode
                ));
            }
            10 => {
                self.scenario_skip_mode = !self.scenario_skip_mode;
                self.scenario_auto_mode = false;
                self.scenario_auto_wait_frames = 0;
                self.trace_graph(format!(
                    "message control skip mode={}",
                    self.scenario_skip_mode
                ));
            }
            11 => {
                self.show_message_history_overlay();
            }
            12 => {
                self.hide_message_overlay();
                self.scenario_auto_mode = false;
                self.scenario_skip_mode = false;
                self.trace_graph("message control return");
            }
            13 => {
                self.show_message_overlay(
                    "SAVE",
                    "Save data slots are available from this menu path.\nPersistent slot encoding is still pending.",
                );
                self.trace_graph("message control save overlay");
            }
            14 => {
                self.show_message_overlay(
                    "QUICK SAVE",
                    "Quick save request accepted.\nPersistent slot encoding is still pending.",
                );
                self.trace_graph("message control quick-save overlay");
            }
            15 => {
                self.show_message_overlay(
                    "LOAD",
                    "Load slots are available from this menu path.\nPersistent slot decoding is still pending.",
                );
                self.trace_graph("message control load overlay");
            }
            16 => {
                self.show_message_overlay(
                    "QUICK LOAD",
                    "Quick load request accepted.\nPersistent slot decoding is still pending.",
                );
                self.trace_graph("message control quick-load overlay");
            }
            17 => {
                let auto = if self.scenario_auto_mode { "ON" } else { "OFF" };
                let skip = if self.scenario_skip_mode { "ON" } else { "OFF" };
                self.show_message_overlay(
                    "SYSTEM",
                    &format!("Auto: {auto}\nSkip: {skip}\nUse RETURN to close this panel."),
                );
                self.trace_graph("message control system overlay");
            }
            18 => {
                self.show_message_overlay("LEAF", "Leaf/system menu request accepted.");
                self.trace_graph("message control leaf overlay");
            }
            _ => {
                self.trace_graph(format!(
                    "message control payload={} ignored",
                    control.payload
                ));
            }
        }
        true
    }

    pub(crate) fn reveal_text_on_input(&mut self) {
        if let Some((target, text)) = self.text_runtime.reveal_all() {
            self.apply_text_to_node(target, text);
            self.apply_visible_ruby_to_node(target);
            self.trace_graph(format!("message node #{target} reveal all"));
        }
    }

    fn ensure_message_text_node(&mut self) -> i32 {
        if let Some(target) = self.text_runtime.target_node.filter(|target| {
            *target == MESSAGE_TEXT_NODE_ID && self.text_nodes.contains_key(target)
        }) {
            return target;
        }
        let target = MESSAGE_TEXT_NODE_ID;
        self.text_nodes.insert(
            target,
            RuntimeTextNode {
                text: String::new(),
                enabled: true,
                screen_attached: true,
                target_surface: None,
                x: self.text_state.x,
                y: self.text_state.y,
                size: self.text_state.font_size,
                color: self.text_state.color,
                z: MESSAGE_TEXT_Z,
                ruby_spans: Vec::new(),
            },
        );
        self.text_runtime.target_node = Some(target);
        self.trace_graph(format!("create message text node #{target}"));
        target
    }

    fn apply_text_to_node(&mut self, target: i32, text: String) {
        let state = self.text_state.clone();
        let node = self
            .text_nodes
            .entry(target)
            .or_insert_with(|| RuntimeTextNode {
                text: String::new(),
                enabled: true,
                screen_attached: true,
                target_surface: None,
                x: state.x,
                y: state.y,
                size: state.font_size,
                color: state.color,
                z: MESSAGE_TEXT_Z,
                ruby_spans: Vec::new(),
            });
        node.text = wrap_text_to_state(&text, &state);
        node.enabled = true;
        node.x = if state.x > 0.0 { state.x } else { 36.0 };
        node.y = if state.y > 0.0 { state.y } else { 520.0 };
        node.size = state.font_size.max(18.0);
        node.color = state.color;
        node.z = MESSAGE_TEXT_Z;
    }

    fn apply_visible_ruby_to_node(&mut self, target: i32) {
        let spans = self.text_runtime.visible_ruby_spans();
        if let Some(node) = self.text_nodes.get_mut(&target) {
            node.ruby_spans = spans;
        }
    }

    fn ensure_message_window_layer(&mut self) {
        if !self.graph_images.contains_key(MESSAGE_WINDOW_KEY) {
            self.load_graph_image_resource(0, "sysgrp.arc", MESSAGE_WINDOW_RESOURCE);
        }
        let Some(image) = self.graph_images.get(MESSAGE_WINDOW_KEY) else {
            return;
        };
        let width = image.width as f32;
        let height = image.height as f32;
        self.scenario_scene_layers.insert(MESSAGE_WINDOW_LAYER_ID);
        self.graph_layers.insert(
            MESSAGE_WINDOW_LAYER_ID,
            RuntimeGraphLayer {
                hit_id: 0,
                owner_object: None,
                key: MESSAGE_WINDOW_KEY.to_string(),
                target_surface: None,
                x: 0.0,
                y: 720.0 - height,
                width,
                height,
                src_x: 0.0,
                src_y: 0.0,
                opacity: 1.0,
                z: MESSAGE_WINDOW_Z,
                enabled: true,
                transform_x: 0.0,
                transform_y: 0.0,
                transform_z: 0,
                scale_x: 1.0,
                scale_y: 1.0,
                rotation_degrees: 0.0,
                clip: None,
            },
        );
        if self.scenario_playback.is_some() {
            self.user_controls.insert(
                MESSAGE_INPUT_OBJECT_ID,
                RuntimeUserControl {
                    id: MESSAGE_INPUT_OBJECT_ID,
                    owner_id: MESSAGE_INPUT_OBJECT_ID,
                    payload: MESSAGE_INPUT_OBJECT_ID,
                    x: 0.0,
                    y: 720.0 - height,
                    width,
                    height,
                    normal_resource: -1,
                    selected_resource: -1,
                    enabled: true,
                    title_only: false,
                },
            );
        }
        self.trace_graph(format!(
            "ensure message window layer #{MESSAGE_WINDOW_LAYER_ID} {MESSAGE_WINDOW_KEY} y={:.1}",
            720.0 - height
        ));
    }

    fn ensure_message_name_layer(&mut self) {
        if !self.graph_images.contains_key(MESSAGE_NAME_WINDOW_KEY) {
            self.load_graph_image_resource(0, "sysgrp.arc", MESSAGE_NAME_WINDOW_RESOURCE);
        }
        let Some(image) = self.graph_images.get(MESSAGE_NAME_WINDOW_KEY) else {
            return;
        };
        let width = image.width as f32;
        let height = image.height as f32;
        self.scenario_scene_layers
            .insert(MESSAGE_NAME_WINDOW_LAYER_ID);
        self.graph_layers.insert(
            MESSAGE_NAME_WINDOW_LAYER_ID,
            RuntimeGraphLayer {
                hit_id: 0,
                owner_object: None,
                key: MESSAGE_NAME_WINDOW_KEY.to_string(),
                target_surface: None,
                x: MESSAGE_NAME_WINDOW_X,
                y: MESSAGE_NAME_WINDOW_Y,
                width,
                height,
                src_x: 0.0,
                src_y: 0.0,
                opacity: 1.0,
                z: MESSAGE_NAME_WINDOW_Z,
                enabled: true,
                transform_x: 0.0,
                transform_y: 0.0,
                transform_z: 0,
                scale_x: 1.0,
                scale_y: 1.0,
                rotation_degrees: 0.0,
                clip: None,
            },
        );
        self.trace_graph(format!(
            "ensure message name layer #{MESSAGE_NAME_WINDOW_LAYER_ID} {MESSAGE_NAME_WINDOW_KEY} x={MESSAGE_NAME_WINDOW_X:.1} y={MESSAGE_NAME_WINDOW_Y:.1}"
        ));
    }

    fn ensure_message_name_text_node(&mut self) -> i32 {
        let target = MESSAGE_NAME_TEXT_NODE_ID;
        self.text_nodes.entry(target).or_insert(RuntimeTextNode {
            text: String::new(),
            enabled: true,
            screen_attached: true,
            target_surface: None,
            x: MESSAGE_NAME_TEXT_X,
            y: MESSAGE_NAME_TEXT_Y,
            size: 24.0,
            color: [1.0, 1.0, 1.0, 1.0],
            z: MESSAGE_NAME_TEXT_Z,
            ruby_spans: Vec::new(),
        });
        target
    }

    fn apply_name_to_node(&mut self, target: i32, text: String) {
        let node = self.text_nodes.entry(target).or_insert(RuntimeTextNode {
            text: String::new(),
            enabled: true,
            screen_attached: true,
            target_surface: None,
            x: MESSAGE_NAME_TEXT_X,
            y: MESSAGE_NAME_TEXT_Y,
            size: 24.0,
            color: [1.0, 1.0, 1.0, 1.0],
            z: MESSAGE_NAME_TEXT_Z,
            ruby_spans: Vec::new(),
        });
        node.text = normalize_message_text(&text);
        node.enabled = true;
        node.x = MESSAGE_NAME_TEXT_X;
        node.y = MESSAGE_NAME_TEXT_Y;
        node.size = 24.0;
        node.color = [1.0, 1.0, 1.0, 1.0];
        node.z = MESSAGE_NAME_TEXT_Z;
    }

    fn hide_message_name(&mut self) {
        if let Some(layer) = self.graph_layers.get_mut(&MESSAGE_NAME_WINDOW_LAYER_ID) {
            layer.enabled = false;
        }
        if let Some(node) = self.text_nodes.get_mut(&MESSAGE_NAME_TEXT_NODE_ID) {
            node.enabled = false;
            node.text.clear();
        }
    }

    fn ensure_message_control_layers(&mut self) {
        for resource in [
            MESSAGE_CONTROL_NORMAL_RESOURCE,
            MESSAGE_CONTROL_HOVER_RESOURCE,
            MESSAGE_CONTROL_ACTIVE_RESOURCE,
            MESSAGE_CONTROL_DISABLED_RESOURCE,
        ] {
            let key = format!("sysgrp.arc:{resource}");
            if !self.graph_images.contains_key(&key) {
                self.load_graph_image_resource(0, "sysgrp.arc", resource);
            }
        }
        for template in MESSAGE_CONTROL_TEMPLATES {
            let key = format!("sysgrp.arc:{MESSAGE_CONTROL_NORMAL_RESOURCE}");
            if !self.graph_images.contains_key(&key) {
                continue;
            }
            self.graph_layers.insert(
                template.layer_id,
                RuntimeGraphLayer {
                    hit_id: template.layer_id,
                    owner_object: None,
                    key,
                    target_surface: None,
                    x: template.x,
                    y: template.y,
                    width: template.width,
                    height: template.height,
                    src_x: template.x,
                    src_y: template.y,
                    opacity: 1.0,
                    z: template.layer_id,
                    enabled: true,
                    transform_x: 0.0,
                    transform_y: 0.0,
                    transform_z: 0,
                    scale_x: 1.0,
                    scale_y: 1.0,
                    rotation_degrees: 0.0,
                    clip: None,
                },
            );
            self.user_controls.insert(
                template.layer_id,
                RuntimeUserControl {
                    id: template.layer_id,
                    owner_id: MESSAGE_CONTROL_OWNER_ID,
                    payload: template.payload,
                    x: template.x,
                    y: template.y,
                    width: template.width,
                    height: template.height,
                    normal_resource: -1,
                    selected_resource: -1,
                    enabled: true,
                    title_only: false,
                },
            );
        }
        self.trace_graph(format!(
            "ensure message controls count={}",
            MESSAGE_CONTROL_TEMPLATES.len()
        ));
    }

    pub(crate) fn has_active_message_window(&self) -> bool {
        let layer_enabled = self
            .graph_layers
            .get(&MESSAGE_WINDOW_LAYER_ID)
            .is_some_and(|layer| layer.enabled);
        if !layer_enabled {
            return false;
        }
        if self
            .text_nodes
            .get(&MESSAGE_OVERLAY_TEXT_NODE_ID)
            .is_some_and(|node| node.enabled && !node.text.is_empty())
        {
            return true;
        }
        if !self.text_runtime.has_current_message() {
            return false;
        }
        if self.native_message_active {
            return true;
        }
        if self.text_runtime.is_animating() {
            return true;
        }
        if self.scenario_bootstrapped {
            return self
                .scenario_playback
                .as_ref()
                .is_some_and(|playback| playback.is_waiting_for_input());
        }
        true
    }

    pub(crate) fn should_draw_message_control_layer(&self, layer: &RuntimeGraphLayer) -> bool {
        if !self.has_active_message_window() {
            return false;
        }
        self.user_controls
            .get(&layer.hit_id)
            .is_some_and(|control| control.owner_id == MESSAGE_CONTROL_OWNER_ID && control.enabled)
    }

    pub(crate) fn message_control_layer_draw_key(
        &self,
        layer: &RuntimeGraphLayer,
    ) -> Option<&'static str> {
        let control = self.user_controls.get(&layer.hit_id)?;
        if control.owner_id != MESSAGE_CONTROL_OWNER_ID {
            return None;
        }
        if !control.enabled || !self.has_active_message_window() {
            return Some("sysgrp.arc:SGMsgWnd000003");
        }
        let hovered = self
            .mouse_pos
            .is_some_and(|point| control.contains(point, false));
        if hovered && self.mouse_pressed {
            Some("sysgrp.arc:SGMsgWnd000002")
        } else if (control.payload == 9 && self.scenario_auto_mode)
            || (control.payload == 10 && self.scenario_skip_mode)
        {
            Some("sysgrp.arc:SGMsgWnd000002")
        } else if hovered {
            Some("sysgrp.arc:SGMsgWnd000001")
        } else {
            Some("sysgrp.arc:SGMsgWnd000000")
        }
    }

    fn show_message_history_overlay(&mut self) {
        let history = self
            .text_runtime
            .history
            .iter()
            .rev()
            .take(8)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let text = if history.is_empty() {
            "(no history)".to_string()
        } else {
            history
        };
        self.show_message_overlay("LOG", &text);
        self.trace_graph("message control history overlay");
    }

    fn show_message_overlay(&mut self, title: &str, body: &str) {
        self.text_nodes.insert(
            MESSAGE_OVERLAY_TEXT_NODE_ID,
            RuntimeTextNode {
                text: format!("{title}\n{body}"),
                enabled: true,
                screen_attached: true,
                target_surface: None,
                x: 64.0,
                y: 80.0,
                size: 24.0,
                color: [1.0, 1.0, 1.0, 1.0],
                z: 20_000,
                ruby_spans: Vec::new(),
            },
        );
    }

    fn hide_message_overlay(&mut self) {
        self.text_nodes.remove(&MESSAGE_OVERLAY_TEXT_NODE_ID);
    }
}

pub(crate) fn wrap_text_to_state(text: &str, state: &TextState) -> String {
    wrap_text_to_state_with_map(text, state).0
}

fn parse_and_wrap_message(text: &str, state: &TextState) -> (String, Vec<RuntimeRubySpan>) {
    let (plain, spans) = parse_message_markup(text);
    let (wrapped, map) = wrap_text_to_state_with_map(&plain, state);
    let spans = spans
        .into_iter()
        .filter_map(|span| {
            Some(RuntimeRubySpan {
                start_char: *map.get(span.start_char)?,
                end_char: *map.get(span.end_char)?,
                reading: span.reading,
            })
        })
        .collect();
    (wrapped, spans)
}

fn wrap_text_to_state_with_map(text: &str, state: &TextState) -> (String, Vec<usize>) {
    let max_units = (state.width.max(120.0) / state.font_size.max(8.0))
        .floor()
        .max(8.0);
    let max_lines = ((state.height.max(state.line_height) / state.line_height.max(1.0)).floor()
        as usize)
        .max(1);
    let mut out = String::new();
    let mut map = Vec::with_capacity(text.chars().count() + 1);
    let mut output_chars = 0usize;
    let mut line_units = 0.0f32;
    let mut lines = 1usize;
    for ch in text.chars() {
        map.push(output_chars);
        if ch == '\n' {
            if lines >= max_lines {
                break;
            }
            out.push(ch);
            output_chars += 1;
            line_units = 0.0;
            lines += 1;
            continue;
        }
        let units = wrap_char_units(ch);
        if line_units > 0.0 && line_units + units > max_units {
            if lines >= max_lines {
                break;
            }
            out.push('\n');
            output_chars += 1;
            line_units = 0.0;
            lines += 1;
        }
        out.push(ch);
        output_chars += 1;
        line_units += units;
    }
    map.push(output_chars);
    (out, map)
}

pub(crate) fn ruby_draw_runs(node: &RuntimeTextNode) -> Vec<(String, f32, f32, f32)> {
    let chars = node.text.chars().collect::<Vec<_>>();
    let line_height = (node.size + 6.0).max(18.0);
    let ruby_size = (node.size * 0.46).max(10.0);
    node.ruby_spans
        .iter()
        .filter_map(|span| {
            if span.start_char >= span.end_char || span.end_char > chars.len() {
                return None;
            }
            let mut x_units = 0.0f32;
            let mut line = 0usize;
            for ch in &chars[..span.start_char] {
                if *ch == '\n' {
                    x_units = 0.0;
                    line += 1;
                } else {
                    x_units += wrap_char_units(*ch);
                }
            }
            if chars[span.start_char..span.end_char]
                .iter()
                .any(|ch| *ch == '\n')
            {
                return None;
            }
            let body_units = chars[span.start_char..span.end_char]
                .iter()
                .map(|ch| wrap_char_units(*ch))
                .sum::<f32>();
            let reading_units = span.reading.chars().map(wrap_char_units).sum::<f32>();
            let body_width = body_units * node.size;
            let reading_width = reading_units * ruby_size;
            let x = node.x + x_units * node.size + (body_width - reading_width) * 0.5;
            let y = node.y + line as f32 * line_height - ruby_size * 0.78;
            Some((span.reading.clone(), x, y, ruby_size))
        })
        .collect()
}

fn wrap_char_units(ch: char) -> f32 {
    if ch.is_ascii() {
        if ch.is_ascii_whitespace() {
            0.35
        } else {
            0.55
        }
    } else {
        1.0
    }
}

fn value_to_text_string(value: &ethornell_vm::Value) -> Option<String> {
    match value {
        ethornell_vm::Value::Str(text) => Some(text.clone()),
        _ => None,
    }
}
