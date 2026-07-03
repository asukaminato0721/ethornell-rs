use ethornell_archive::{detect_magic, scan_game_root, MagicKind, ResourceManager};
use ethornell_audio::AudioSystem;
use ethornell_core::{EthornellError, GameRoot, Result};
use ethornell_image::{decode_image, DecodedImage};
use ethornell_render::{RenderCommand, Renderer, TextureHandle};
use ethornell_script::{calls::known_call_arg_count, known_call_name};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
mod animation;
mod auto_input;
mod character_image;
mod graph;
mod headless;
mod resource_lookup;
mod scenario;
mod scene;
mod snapshot;
mod text;
mod text_anim;
mod timeline;
mod title;
use animation::LayerAnimationSystem;
use character_image::decode_scenario_resource_image;
use graph::{
    fixed_16_to_f32, RuntimeGraphDrawItem, RuntimeGraphLayer, RuntimeGraphResource, RuntimeSurface,
    RuntimeUserControl,
};
use headless::{HeadlessInputEvent, HeadlessInputScript};
use resource_lookup::find_scenario_image;
use scenario::{ScenarioAction, ScenarioPlayback};
use scene::{
    place_scenario_sprite_with_hints, scenario_layer_ids_for_slot, sprite_fade_frames,
    sprite_target_opacity, SCENARIO_OVERLAY_LAYER_ID,
};
use text::{wrap_text_to_state, RuntimeTextNode, TextState, MESSAGE_NAME_TEXT_Z, MESSAGE_TEXT_Z};
use text_anim::TextRuntime;
use timeline::{TimelineEvent, TimelineSystem};
use title::{title_object_for_control_text, title_payload_for_control_text};
use winit::event::{ElementState, Event, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowBuilder;

const INPUT_DESCRIPTOR_LEFT: i32 = 37;
const INPUT_DESCRIPTOR_UP: i32 = 38;
const INPUT_DESCRIPTOR_RIGHT: i32 = 39;
const INPUT_DESCRIPTOR_DOWN: i32 = 40;
const INPUT_DESCRIPTOR_ENTER: i32 = 13;

pub struct AppConfig {
    pub game_root: GameRoot,
    pub trace: bool,
    pub fail_on_stub: bool,
    pub force_text_test: bool,
    pub script: Option<String>,
}

pub fn run(config: AppConfig) -> Result<()> {
    let scan = scan_game_root(&config.game_root)?;
    tracing::info!(
        files = scan.file_count,
        arcs = scan.arc_candidates,
        bp = scan.bp_scripts,
        cbg = scan.compressed_bg_candidates,
        ogg = scan.ogg_candidates,
        "game resource scan complete"
    );
    let manager = ResourceManager::open_game(config.game_root.path())?;
    tracing::info!(
        archives = manager.archives().archives.len(),
        resources = manager.list().len(),
        "archive-backed resource manager ready"
    );

    let bgm = load_runtime_bgm(&manager);
    let startup_script = config.script.as_deref().unwrap_or("title._bp");
    let runtime = match Some(startup_script) {
        Some(script) => match manager.read_decoded(script) {
            Ok(bytes) => Some(RuntimeEngine::new(
                script,
                &bytes,
                manager.clone(),
                config.trace,
                config.fail_on_stub,
            )),
            Err(err) => {
                tracing::warn!(script, %err, "startup script unavailable");
                None
            }
        },
        None => None,
    };
    let image = if runtime.is_some() {
        None
    } else {
        Some(load_runtime_image(&manager)?)
    };
    if config.trace {
        if let Some(image) = image.as_ref() {
            tracing::info!(
                width = image.width,
                height = image.height,
                "runtime reached resource image decode stage"
            );
        }
    }
    let text = config
        .force_text_test
        .then(|| "こんにちは\nHello Ethornell".to_string());
    if std::env::var_os("ETHORNELL_GUI_HEADLESS").is_some() {
        run_headless("ethornell-rs runtime", image, text, bgm.ok(), runtime)
    } else {
        run_window("ethornell-rs runtime", image, text, bgm.ok(), runtime)
    }
}

pub fn view_image_file(path: &Path) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let image = decode_image(&bytes)?;
    run_window(
        &format!("ethornell-rs: {}", path.display()),
        Some(image),
        None,
        None,
        None,
    )
}

struct RuntimeTraceApi {
    manager: ResourceManager,
    text_state: TextState,
    text_runtime: TextRuntime,
    window_mode: i32,
    screen_width: i32,
    screen_height: i32,
    debug_graph: bool,
    graph_trace: VecDeque<String>,
    graph_images: BTreeMap<String, DecodedImage>,
    graph_resources: BTreeMap<i32, RuntimeGraphResource>,
    graph_bindings: BTreeMap<i32, i32>,
    graph_layers: BTreeMap<i32, RuntimeGraphLayer>,
    graph_object_layers: BTreeMap<i32, BTreeSet<i32>>,
    graph_object_enabled: BTreeMap<i32, bool>,
    current_graph_object: Option<i32>,
    graph_surfaces: BTreeMap<i32, RuntimeSurface>,
    layer_animations: LayerAnimationSystem,
    timelines: TimelineSystem,
    user_controls: BTreeMap<i32, RuntimeUserControl>,
    text_nodes: BTreeMap<i32, RuntimeTextNode>,
    sound_slots: BTreeMap<i32, AudioRequest>,
    audio_requests: VecDeque<AudioRequest>,
    current_bgm: Option<(String, String)>,
    program_cache: HashMap<String, ethornell_script::BpProgram>,
    file_exists_cache: HashMap<String, bool>,
    file_size_cache: HashMap<String, i32>,
    title_ui_resources_loaded: bool,
    title_ui_active: bool,
    title_ui_departed: bool,
    title_child_program_active: bool,
    title_child_program: Option<String>,
    title_scenario_requested: bool,
    scenario_bootstrapped: bool,
    pending_scenario_bootstrap: Option<(String, String, Vec<u8>)>,
    scenario_playback: Option<ScenarioPlayback>,
    scenario_input_latched: bool,
    scenario_auto_mode: bool,
    scenario_skip_mode: bool,
    scenario_auto_wait_frames: u32,
    scenario_loaded_scripts: BTreeSet<String>,
    scenario_sprite_sequence: u32,
    scenario_scene_layers: BTreeSet<i32>,
    last_scenario_sound: Option<String>,
    next_node_handle: i32,
    next_object_handle: i32,
    next_surface_handle: i32,
    next_timeline_handle: i32,
    mouse_pos: Option<(f32, f32)>,
    mouse_pressed: bool,
    pending_click: Option<(f32, f32)>,
    pending_click_age_frames: u8,
    pending_object_state: Option<(f32, f32)>,
    pending_input_state: Option<i32>,
    pending_input_descriptor: Option<i32>,
    last_hit_control: i32,
    last_hit_payload: i32,
    auto_title_click: bool,
    auto_title_click_id: i32,
    auto_title_click_done: bool,
    auto_title_payload_override: Option<i32>,
    pending_title_payload_override: Option<i32>,
    auto_title_hover_frames: u32,
    auto_title_hovered_frames: u32,
    auto_title_release_after_state: bool,
    auto_user_click: bool,
    auto_user_click_id: Option<i32>,
    auto_user_click_done: bool,
    auto_user_click_repeat: bool,
    auto_user_click_point: Option<(f32, f32)>,
    auto_user_click_delay_frames: u32,
    auto_user_click_elapsed_frames: u32,
    auto_user_click_hover_frames: u32,
    auto_user_click_hovered_frames: u32,
    mouse_event_code: i32,
    frame_yield_requested: bool,
    animation_queue_remaining: u32,
    graph94_yield_count: u32,
    graph94_yield_every: u32,
    call_coverage: BTreeMap<(u8, u16), usize>,
}

impl RuntimeTraceApi {
    fn new(manager: ResourceManager) -> Self {
        Self {
            manager,
            text_state: TextState::default(),
            text_runtime: TextRuntime::default(),
            window_mode: 0,
            screen_width: 0,
            screen_height: 0,
            debug_graph: std::env::var("DEBUG").ok().as_deref() == Some("1"),
            graph_trace: VecDeque::new(),
            graph_images: BTreeMap::new(),
            graph_resources: BTreeMap::new(),
            graph_bindings: BTreeMap::new(),
            graph_layers: BTreeMap::new(),
            graph_object_layers: BTreeMap::new(),
            graph_object_enabled: BTreeMap::new(),
            current_graph_object: None,
            graph_surfaces: BTreeMap::new(),
            layer_animations: LayerAnimationSystem::default(),
            timelines: TimelineSystem::default(),
            user_controls: BTreeMap::new(),
            text_nodes: BTreeMap::new(),
            sound_slots: BTreeMap::new(),
            audio_requests: VecDeque::new(),
            current_bgm: None,
            program_cache: HashMap::new(),
            file_exists_cache: HashMap::new(),
            file_size_cache: HashMap::new(),
            title_ui_resources_loaded: false,
            title_ui_active: false,
            title_ui_departed: false,
            title_child_program_active: false,
            title_child_program: None,
            title_scenario_requested: false,
            scenario_bootstrapped: false,
            pending_scenario_bootstrap: None,
            scenario_playback: None,
            scenario_input_latched: false,
            scenario_auto_mode: false,
            scenario_skip_mode: false,
            scenario_auto_wait_frames: 0,
            scenario_loaded_scripts: BTreeSet::new(),
            scenario_sprite_sequence: 0,
            scenario_scene_layers: BTreeSet::new(),
            last_scenario_sound: None,
            next_node_handle: 1,
            next_object_handle: 10_000,
            next_surface_handle: 20_000,
            next_timeline_handle: 30_000,
            mouse_pos: None,
            mouse_pressed: false,
            pending_click: None,
            pending_click_age_frames: 0,
            pending_object_state: None,
            pending_input_state: None,
            pending_input_descriptor: None,
            last_hit_control: 0,
            last_hit_payload: 0,
            auto_title_click: std::env::var("ETHORNELL_AUTO_TITLE_CLICK").ok().as_deref()
                == Some("1"),
            auto_title_click_id: std::env::var("ETHORNELL_AUTO_TITLE_CLICK_ID")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1),
            auto_title_click_done: false,
            auto_title_payload_override: std::env::var("ETHORNELL_AUTO_TITLE_PAYLOAD")
                .ok()
                .and_then(|value| value.parse().ok()),
            pending_title_payload_override: None,
            auto_title_hover_frames: std::env::var("ETHORNELL_AUTO_TITLE_HOVER_FRAMES")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(3),
            auto_title_hovered_frames: 0,
            auto_title_release_after_state: false,
            auto_user_click: std::env::var("ETHORNELL_AUTO_USER_CLICK").ok().as_deref()
                == Some("1"),
            auto_user_click_id: std::env::var("ETHORNELL_AUTO_USER_CLICK_ID")
                .ok()
                .and_then(|value| parse_i32_env(&value)),
            auto_user_click_done: false,
            auto_user_click_repeat: std::env::var("ETHORNELL_AUTO_USER_CLICK_REPEAT")
                .ok()
                .as_deref()
                == Some("1"),
            auto_user_click_point: parse_env_point(
                "ETHORNELL_AUTO_USER_CLICK_X",
                "ETHORNELL_AUTO_USER_CLICK_Y",
            ),
            auto_user_click_delay_frames: std::env::var("ETHORNELL_AUTO_USER_CLICK_DELAY_FRAMES")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(8),
            auto_user_click_elapsed_frames: 0,
            auto_user_click_hover_frames: std::env::var("ETHORNELL_AUTO_USER_CLICK_HOVER_FRAMES")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(2),
            auto_user_click_hovered_frames: 0,
            mouse_event_code: std::env::var("ETHORNELL_MOUSE_EVENT_CODE")
                .ok()
                .and_then(|value| parse_i32_env(&value))
                .unwrap_or(0x1000_0002),
            frame_yield_requested: false,
            animation_queue_remaining: 0,
            graph94_yield_count: 0,
            graph94_yield_every: std::env::var("ETHORNELL_GRAPH94_YIELD_EVERY")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(8)
                .max(1),
            call_coverage: BTreeMap::new(),
        }
    }

    fn record_call(&mut self, group: u8, id: u16) {
        *self.call_coverage.entry((group, id)).or_default() += 1;
    }

    fn write_call_coverage_if_requested(&self) {
        let Some(path) = std::env::var_os("ETHORNELL_CALL_COVERAGE") else {
            return;
        };
        let mut out = String::from("group,id,name,count\n");
        for ((group, id), count) in &self.call_coverage {
            let name = known_call_name(*group, *id).unwrap_or("unknown");
            out.push_str(&format!("0x{group:02X},0x{id:02X},{name},{count}\n"));
        }
        if let Err(err) = std::fs::write(&path, out) {
            tracing::warn!(path = ?path, %err, "write call coverage failed");
        } else {
            tracing::info!(path = ?path, calls = self.call_coverage.len(), "call coverage written");
        }
    }

    fn should_yield_frame(&self) -> bool {
        self.title_ui_resources_loaded
            || !self.graph_images.is_empty()
            || !self.graph_layers.is_empty()
            || self.scenario_playback.is_some()
            || !self.text_nodes.is_empty()
    }

    fn should_continue_vm_after_yield(&self) -> bool {
        self.vm_after_yield_blockers().is_empty()
    }

    fn vm_after_yield_blockers(&self) -> Vec<&'static str> {
        let mut blockers = Vec::new();
        if self.title_ui_active {
            blockers.push("title_ui_active");
        }
        if self.has_active_message_window() {
            blockers.push("active_message_window");
        }
        if self.has_blocking_user_controls() {
            blockers.push("user_controls");
        }
        blockers
    }

    fn has_blocking_user_controls(&self) -> bool {
        self.user_controls.values().any(|control| {
            control.enabled
                && (control.owner_id != text::MESSAGE_CONTROL_OWNER_ID
                    || self.has_active_message_window())
        })
    }

    fn should_continue_input_yield(&self) -> bool {
        self.title_ui_active
            && (self.pending_click.is_some()
                || self.pending_object_state.is_some()
                || self.pending_input_state.is_some())
    }

    fn trace_graph(&mut self, message: impl Into<String>) {
        if !self.debug_graph {
            return;
        }
        let message = message.into();
        tracing::info!(target: "graph_tree", "{message}");
        self.graph_trace.push_back(message);
        while self.graph_trace.len() > 512 {
            self.graph_trace.pop_front();
        }
    }

    fn load_bp_program_cached(
        &mut self,
        archive: &str,
        file: &str,
        syscall: &'static str,
    ) -> ethornell_script::BpProgram {
        let key = format!("{archive}:{file}");
        if let Some(program) = self.program_cache.get(&key) {
            tracing::debug!(archive, file, syscall, "LoadProgram cache hit");
            return program.clone();
        }
        let program = match self.manager.read_decoded_from_archive(archive, file) {
            Ok(bytes) => ethornell_script::parse_bp_program(Some(key.clone()), &bytes),
            Err(err) => {
                tracing::warn!(
                    archive,
                    file,
                    syscall,
                    %err,
                    "LoadProgram fell back to empty program"
                );
                empty_bp_program(key.clone())
            }
        };
        self.program_cache.insert(key, program.clone());
        program
    }

    fn has_cached_bp_program(&self, archive: &str, file: &str) -> bool {
        self.program_cache
            .contains_key(&format!("{archive}:{file}"))
    }

    fn cached_file_exists(&mut self, archive: &str, file: &str) -> bool {
        let key = runtime_file_cache_key(archive, file);
        if let Some(exists) = self.file_exists_cache.get(&key).copied() {
            return exists;
        }
        let exists = synthetic_user_data_bytes(archive, file).is_some()
            || find_runtime_file(&self.manager, archive, file)
                .map(|path| path.exists())
                .unwrap_or_else(|| find_runtime_resource(&self.manager, archive, file).is_some());
        self.file_exists_cache.insert(key, exists);
        exists
    }

    fn cached_file_size(&mut self, archive: &str, file: &str) -> i32 {
        let key = runtime_file_cache_key(archive, file);
        if let Some(size) = self.file_size_cache.get(&key).copied() {
            return size;
        }
        let size = find_runtime_file(&self.manager, archive, file)
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|meta| meta.len() as i32)
            .or_else(|| synthetic_user_data_bytes(archive, file).map(|bytes| bytes.len() as i32))
            .or_else(|| {
                find_runtime_resource(&self.manager, archive, file)
                    .map(|entry| entry.unpacked_size.unwrap_or(entry.packed_size) as i32)
            })
            .unwrap_or(-1);
        self.file_size_cache.insert(key, size);
        size
    }

    fn load_graph_image_resource(
        &mut self,
        target_id: i32,
        archive_name: &str,
        resource_name: &str,
    ) -> bool {
        let key = format!("{archive_name}:{resource_name}");
        if self.graph_images.contains_key(&key) {
            if target_id != 0 {
                self.graph_resources
                    .insert(target_id, RuntimeGraphResource { key: key.clone() });
            }
            self.trace_graph(format!("load image #{target_id} {key} (cached)"));
            return true;
        }
        let Some(entry) = find_runtime_resource(&self.manager, archive_name, resource_name) else {
            return false;
        };
        let Some(image) = self
            .manager
            .read_by_entry_decoded(&entry)
            .ok()
            .and_then(|bytes| decode_image(&bytes).ok())
        else {
            self.trace_graph(format!(
                "load resource {archive_name}:{resource_name} (not image)"
            ));
            return false;
        };
        self.trace_graph(format!(
            "load image #{target_id} {key} -> {}x{}",
            image.width, image.height
        ));
        if target_id != 0 {
            self.graph_resources
                .insert(target_id, RuntimeGraphResource { key: key.clone() });
        }
        self.graph_images.insert(key, image);
        true
    }

    fn ensure_title_atlas_images(&mut self) {
        for resource_name in ["SGTitle000000", "SGTitle000001", "SGTitle000003"] {
            let key = format!("sysgrp.arc:{resource_name}");
            if !self.graph_images.contains_key(&key) {
                self.load_graph_image_resource(0, "sysgrp.arc", resource_name);
            }
        }
    }

    fn start_scenario_playback(&mut self, archive: &str, file: &str, bytes: &[u8]) {
        if self.scenario_bootstrapped {
            return;
        }
        let Some(playback) = ScenarioPlayback::from_bcs(bytes) else {
            return;
        };
        let action_count = playback.action_count();
        self.scenario_playback = Some(playback);
        self.scenario_bootstrapped = true;
        self.scenario_loaded_scripts.insert(file.to_string());
        self.scenario_input_latched = false;
        self.pending_click = None;
        self.pending_click_age_frames = 0;
        self.pending_object_state = None;
        self.pending_input_state = None;
        self.pending_input_descriptor = None;
        self.mouse_pressed = false;
        self.clear_title_graph_layers();
        tracing::info!(archive, file, action_count, "BCS scenario playback started");
        self.trace_graph(format!(
            "BCS playback started {archive}:{file} actions={action_count}"
        ));
    }

    fn start_pending_scenario_bootstrap(&mut self) {
        if self.scenario_bootstrapped {
            self.pending_scenario_bootstrap = None;
            return;
        }
        let Some((archive, file, bytes)) = self.pending_scenario_bootstrap.take() else {
            return;
        };
        self.start_scenario_playback(&archive, &file, &bytes);
    }

    fn queue_scenario_sound(&mut self, sound_name: &str) {
        let Some(entry) = self.manager.find(sound_name) else {
            tracing::warn!(file = sound_name, "BCS scenario audio missing");
            return;
        };
        self.last_scenario_sound = Some(sound_name.to_string());
        let archive = entry
            .archive_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        match self.manager.read_by_entry_decoded(&entry) {
            Ok(bytes) => {
                self.audio_requests.push_back(AudioRequest {
                    archive: archive.clone(),
                    file: sound_name.to_string(),
                    bytes,
                });
                tracing::info!(archive, file = sound_name, "BCS scenario audio queued");
            }
            Err(err) => {
                tracing::warn!(archive, file = sound_name, %err, "BCS scenario audio read failed")
            }
        }
    }

    fn load_scenario_sprite(
        &mut self,
        sprite_name: &str,
        wait_frames: u32,
        slot: Option<i32>,
        x: Option<i32>,
        z: Option<i32>,
        opacity: Option<f32>,
        body_layer: Option<&str>,
    ) {
        let Some(resource) = find_scenario_image(&self.manager, sprite_name) else {
            tracing::warn!(file = sprite_name, "BCS scenario sprite missing");
            return;
        };
        let entry = &resource.entry;
        let archive = entry
            .archive_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        let placement = place_scenario_sprite_with_hints(
            sprite_name,
            self.scenario_sprite_sequence,
            slot,
            x,
            z,
        );
        if matches!(
            placement.class,
            scene::ScenarioSpriteClass::Background | scene::ScenarioSpriteClass::Event
        ) {
            self.clear_transition_cover_layers();
        }
        let key = if let Some(body_layer) = body_layer {
            format!("{archive}:{}+{body_layer}", resource.resolved)
        } else {
            format!("{archive}:{}", resource.resolved)
        };
        let (width, height) = if let Some(image) = self.graph_images.get(&key) {
            (image.width as f32, image.height as f32)
        } else {
            let Ok(image) = decode_scenario_resource_image(
                &self.manager,
                &resource,
                placement.class == scene::ScenarioSpriteClass::Character,
                body_layer,
            ) else {
                tracing::warn!(
                    archive,
                    file = sprite_name,
                    "BCS scenario sprite decode failed"
                );
                return;
            };
            let width = image.width as f32;
            let height = image.height as f32;
            self.graph_images.insert(key.clone(), image);
            (width, height)
        };
        self.scenario_sprite_sequence = self.scenario_sprite_sequence.saturating_add(1);
        if placement.track_scene_layer {
            self.scenario_scene_layers.insert(placement.layer_id);
        }
        let fade_frames = sprite_fade_frames(placement.class, wait_frames);
        let target_opacity =
            opacity.unwrap_or_else(|| sprite_target_opacity(sprite_name, placement.class));
        let initial_opacity = if fade_frames > 1 { 0.0 } else { target_opacity };
        let viewport_sprite = scene::is_viewport_sprite_class(placement.class);
        let layer_width = if viewport_sprite {
            1280.0_f32.min(width)
        } else {
            width
        };
        let layer_height = if viewport_sprite {
            720.0_f32.min(height)
        } else {
            height
        };
        let layer_x = if viewport_sprite {
            0.0
        } else {
            placement
                .x
                .map(|offset| (1280.0 - width).max(0.0) * 0.5 + offset)
                .unwrap_or_else(|| (1280.0 - width).max(0.0) * 0.5)
        };
        let layer_y = if viewport_sprite {
            0.0
        } else {
            placement.y.unwrap_or_else(|| match placement.class {
                scene::ScenarioSpriteClass::Character => 0.0,
                _ => (720.0 - height).max(0.0) * 0.5,
            })
        };
        self.graph_layers.insert(
            placement.layer_id,
            RuntimeGraphLayer {
                hit_id: placement.hit_id,
                owner_object: None,
                key: key.clone(),
                target_surface: None,
                x: layer_x,
                y: layer_y,
                width: layer_width,
                height: layer_height,
                src_x: 0.0,
                src_y: 0.0,
                opacity: initial_opacity,
                z: placement.z,
                enabled: true,
                transform_x: 0.0,
                transform_y: 0.0,
                scale_x: 1.0,
                scale_y: 1.0,
                rotation_degrees: 0.0,
                clip: None,
            },
        );
        if fade_frames > 1 {
            self.layer_animations
                .fade_to(placement.layer_id, 0.0, target_opacity, fade_frames);
        }
        self.trace_graph(format!(
            "BCS scenario sprite {key} requested={} class={:?} layer={} slot={slot:?} x={layer_x:.1} y={layer_y:.1} z={} blocks={} opacity={:.2} fade={} layer={}x{} image={}x{}",
            resource.requested,
            placement.class,
            placement.layer_id,
            placement.z,
            placement.blocks_input,
            target_opacity,
            fade_frames,
            layer_width as i32,
            layer_height as i32,
            width as i32,
            height as i32
        ));
        tracing::info!(
            archive,
            file = resource.requested,
            resolved = resource.resolved,
            width,
            height,
            class = ?placement.class,
            layer = placement.layer_id,
            slot,
            x = layer_x,
            y = layer_y,
            z = placement.z,
            "BCS scenario sprite loaded"
        );
    }

    fn clear_transition_cover_layers(&mut self) {
        for layer_id in [SCENARIO_OVERLAY_LAYER_ID, scene::FADE_PLATE_LAYER_ID] {
            if self.graph_layers.remove(&layer_id).is_some() {
                self.layer_animations.clear_layer(layer_id);
                self.scenario_scene_layers.remove(&layer_id);
                self.trace_graph(format!(
                    "BCS playback clear transition cover layer={layer_id}"
                ));
            }
        }
    }

    fn append_scenario_script(&mut self, file: &str) {
        if self.scenario_loaded_scripts.contains(file) {
            self.trace_graph(format!("BCS playback skip already-loaded script {file}"));
            return;
        }
        let Some(entry) = find_runtime_resource(&self.manager, "data01xxx.arc", file) else {
            tracing::warn!(file, "BCS scenario script missing");
            return;
        };
        let archive = entry
            .archive_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        let Ok(bytes) = self.manager.read_by_entry_decoded(&entry) else {
            tracing::warn!(archive, file, "BCS scenario script read failed");
            return;
        };
        if ethornell_script::bcs::parse_bcs(&bytes).is_none() {
            tracing::warn!(archive, file, "BCS scenario script parse failed");
            return;
        }
        self.scenario_loaded_scripts.insert(file.to_string());
        let Some(added) = self
            .scenario_playback
            .as_mut()
            .and_then(|playback| playback.append_bcs(&bytes))
        else {
            tracing::warn!(archive, file, "BCS scenario script parse failed");
            return;
        };
        tracing::info!(archive, file, added, "BCS scenario script appended");
        self.trace_graph(format!(
            "BCS playback append script {archive}:{file} actions={added}"
        ));
    }

    fn trace_timeline_event(&mut self, event: TimelineEvent) {
        match event {
            TimelineEvent::Configured { handle, duration } => {
                if handle <= 0 {
                    return;
                }
                self.trace_graph(format!("timeline #{handle} configured duration={duration}"));
            }
            TimelineEvent::Attached { handle, target } => {
                if handle <= 0 {
                    return;
                }
                self.trace_graph(format!("timeline #{handle} attached target #{target}"));
            }
            TimelineEvent::Enabled {
                handle,
                enabled,
                remaining,
            } => {
                if handle <= 0 {
                    return;
                }
                self.trace_graph(format!(
                    "timeline #{handle} enabled={enabled} remaining={remaining}"
                ));
            }
        }
    }

    fn alloc_node(&mut self) -> i32 {
        let handle = self.next_node_handle;
        self.next_node_handle += 1;
        handle
    }

    fn alloc_object(&mut self) -> i32 {
        let handle = self.next_object_handle;
        self.next_object_handle += 1;
        handle
    }

    fn alloc_surface(&mut self) -> i32 {
        let handle = self.next_surface_handle;
        self.next_surface_handle += 1;
        handle
    }

    fn alloc_timeline(&mut self) -> i32 {
        let handle = self.next_timeline_handle;
        self.next_timeline_handle += 1;
        handle
    }

    fn set_current_graph_object(&mut self, object: i32) {
        if object > 0 {
            self.current_graph_object = Some(object);
            self.graph_object_enabled.entry(object).or_insert(true);
            self.graph_object_layers.entry(object).or_default();
        }
    }

    fn adopt_previous_graph_object_layers(&mut self, object: i32) -> Option<usize> {
        let is_empty = self
            .graph_object_layers
            .get(&object)
            .is_none_or(BTreeSet::is_empty);
        if !is_empty {
            return None;
        }
        let source = self
            .graph_object_layers
            .iter()
            .rev()
            .find_map(|(candidate, layers)| {
                (*candidate != object && !layers.is_empty()).then_some(*candidate)
            })?;
        let layers = self.graph_object_layers.remove(&source)?;
        let count = layers.len();
        for layer_id in &layers {
            if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                layer.owner_object = Some(object);
            }
        }
        self.graph_object_layers.insert(object, layers);
        self.graph_object_enabled.remove(&source);
        self.trace_graph(format!(
            "object #{object} adopted {count} layers from object #{source}"
        ));
        Some(count)
    }

    fn register_layer_for_current_object(&mut self, layer_id: i32) -> Option<i32> {
        let object = self.current_graph_object?;
        self.graph_object_layers
            .entry(object)
            .or_default()
            .insert(layer_id);
        Some(object)
    }

    fn object_enabled_for_layer(&self, owner_object: Option<i32>) -> bool {
        owner_object
            .and_then(|object| self.graph_object_enabled.get(&object).copied())
            .unwrap_or(true)
    }

    fn set_graph_object_enabled(&mut self, object: i32, enabled: bool) {
        self.graph_object_enabled.insert(object, enabled);
        let layer_ids = self
            .graph_object_layers
            .get(&object)
            .cloned()
            .unwrap_or_default();
        for layer_id in layer_ids {
            if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                layer.enabled = enabled && !is_default_hidden_object_state(&layer.key);
            }
        }
        self.trace_graph(format!(
            "object #{object} enabled={enabled} layers={}",
            self.graph_object_layers
                .get(&object)
                .map(BTreeSet::len)
                .unwrap_or_default()
        ));
    }

    fn remove_graph_object(&mut self, object: i32) {
        self.graph_object_enabled.remove(&object);
        self.user_controls
            .retain(|_, control| control.title_only || control.owner_id != object);
        if self.current_graph_object == Some(object) {
            self.current_graph_object = None;
        }
        if let Some(layer_ids) = self.graph_object_layers.remove(&object) {
            for layer_id in layer_ids {
                if self
                    .graph_layers
                    .get(&layer_id)
                    .is_some_and(|layer| layer.owner_object == Some(object))
                {
                    self.graph_layers.remove(&layer_id);
                }
            }
            self.trace_graph(format!("finalize object #{object}"));
        }
    }

    fn apply_graph_node_transition(&mut self, args: &[ethornell_vm::Value]) {
        let ints = args.iter().map(value_to_i32).collect::<Vec<_>>();
        let Some(timeline) = ints
            .get(6)
            .copied()
            .filter(|value| *value >= 30_000)
            .or_else(|| ints.iter().rev().copied().find(|value| *value >= 30_000))
        else {
            return;
        };
        let duration_ms = ints
            .get(3)
            .copied()
            .filter(|value| (1..=10_000).contains(value))
            .unwrap_or_else(|| {
                ints.iter()
                    .copied()
                    .filter(|value| (1..=10_000).contains(value) && *value != 256)
                    .next_back()
                    .unwrap_or(0)
            });
        let alpha = ints
            .get(5)
            .copied()
            .filter(|value| (0..=256).contains(value))
            .unwrap_or_else(|| {
                ints.iter()
                    .copied()
                    .filter(|value| (0..=256).contains(value))
                    .next_back()
                    .unwrap_or(256)
            });
        let target_opacity = (alpha as f32 / 256.0).clamp(0.0, 1.0);
        let duration_frames = duration_ms.unsigned_abs().div_ceil(16).max(1);
        let event = self.timelines.set_duration_ms(timeline, duration_ms);
        self.trace_timeline_event(event);

        let mut targets = self.timelines.attachments(timeline);
        targets.extend(ints.iter().copied().filter(|value| {
            self.graph_layers.contains_key(value) || self.text_nodes.contains_key(value)
        }));
        targets.sort_unstable();
        targets.dedup();

        let mut animated_layers = 0usize;
        let mut animated_text = 0usize;
        for target in targets {
            if let Some(layer_ids) = self.graph_object_layers.get(&target).cloned() {
                for layer_id in layer_ids {
                    if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                        let from = layer.opacity;
                        if duration_frames > 1 && (from - target_opacity).abs() > f32::EPSILON {
                            self.layer_animations.fade_to(
                                layer_id,
                                from,
                                target_opacity,
                                duration_frames,
                            );
                        } else {
                            layer.opacity = target_opacity;
                        }
                        animated_layers += 1;
                    }
                }
                continue;
            }
            if let Some(layer) = self.graph_layers.get_mut(&target) {
                let from = layer.opacity;
                if duration_frames > 1 && (from - target_opacity).abs() > f32::EPSILON {
                    self.layer_animations
                        .fade_to(target, from, target_opacity, duration_frames);
                } else {
                    layer.opacity = target_opacity;
                }
                animated_layers += 1;
            }
            if let Some(node) = self.text_nodes.get_mut(&target) {
                node.color[3] = target_opacity;
                animated_text += 1;
            }
        }
        self.animation_queue_remaining = self.animation_queue_remaining.max(duration_frames);
        self.trace_graph(format!(
            "node transition timeline=#{timeline} alpha={target_opacity:.2} duration_ms={duration_ms} frames={duration_frames} layers={animated_layers} text={animated_text}"
        ));
    }

    fn apply_graph_rect_transition(&mut self, args: &[ethornell_vm::Value]) {
        let popped = args.iter().map(value_to_i32).collect::<Vec<_>>();
        let mut source = popped.clone();
        source.reverse();
        let duration_ms = source
            .iter()
            .copied()
            .filter(|value| (1..=10_000).contains(value))
            .find(|value| {
                !self.graph_layers.contains_key(value) && !self.text_nodes.contains_key(value)
            })
            .unwrap_or(0);
        let alpha = source
            .iter()
            .rev()
            .copied()
            .find(|value| (1..=256).contains(value))
            .unwrap_or(256);
        let target_opacity = (alpha as f32 / 256.0).clamp(0.0, 1.0);
        let duration_frames = duration_ms.unsigned_abs().div_ceil(16).max(1);
        let mut targets = source
            .iter()
            .copied()
            .filter(|value| {
                self.graph_layers.contains_key(value)
                    || self.text_nodes.contains_key(value)
                    || self.graph_object_layers.contains_key(value)
            })
            .collect::<Vec<_>>();
        targets.sort_unstable();
        targets.dedup();

        let mut animated_layers = 0usize;
        let mut animated_text = 0usize;
        for target in &targets {
            if let Some(layer_ids) = self.graph_object_layers.get(target).cloned() {
                for layer_id in layer_ids {
                    if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                        let from = layer.opacity;
                        if duration_frames > 1 && (from - target_opacity).abs() > f32::EPSILON {
                            self.layer_animations.fade_to(
                                layer_id,
                                from,
                                target_opacity,
                                duration_frames,
                            );
                        } else {
                            layer.opacity = target_opacity;
                        }
                        animated_layers += 1;
                    }
                }
                continue;
            }
            if let Some(layer) = self.graph_layers.get_mut(target) {
                let from = layer.opacity;
                if duration_frames > 1 && (from - target_opacity).abs() > f32::EPSILON {
                    self.layer_animations
                        .fade_to(*target, from, target_opacity, duration_frames);
                } else {
                    layer.opacity = target_opacity;
                }
                animated_layers += 1;
            }
            if let Some(node) = self.text_nodes.get_mut(target) {
                node.color[3] = target_opacity;
                animated_text += 1;
            }
        }
        if duration_frames > 1 && (!targets.is_empty() || target_opacity < 1.0) {
            self.animation_queue_remaining = self.animation_queue_remaining.max(duration_frames);
        }
        self.trace_graph(format!(
            "rect transition source={source:?} alpha={target_opacity:.2} duration_ms={duration_ms} frames={duration_frames} targets={targets:?} layers={animated_layers} text={animated_text}"
        ));
    }

    fn tick_timelines(&mut self) {
        self.timelines.tick();
        for event in self.layer_animations.tick(&mut self.graph_layers) {
            if self.debug_graph {
                match event {
                    animation::LayerAnimationEvent::Finished { layer_id, property } => {
                        self.trace_graph(format!(
                            "layer animation finished layer={layer_id} property={property:?}"
                        ));
                    }
                }
            }
        }
        if self.animation_queue_remaining > 0 {
            self.animation_queue_remaining -= 1;
            self.trace_graph(format!(
                "animation queue frame tick remaining={}",
                self.animation_queue_remaining
            ));
        }
    }

    fn scenario_overlay_active(&self) -> bool {
        self.graph_layers
            .get(&SCENARIO_OVERLAY_LAYER_ID)
            .is_some_and(|layer| layer.enabled && layer.opacity > 0.001)
    }

    fn clear_scenario_sprites(&mut self) {
        if self
            .graph_layers
            .remove(&SCENARIO_OVERLAY_LAYER_ID)
            .is_some()
        {
            self.layer_animations.clear_layer(SCENARIO_OVERLAY_LAYER_ID);
            self.trace_graph("BCS playback clear modal sprite");
            return;
        }
        let layers = self.scenario_scene_layers.clone();
        self.layer_animations.clear_layers(&layers);
        for layer_id in &layers {
            self.graph_layers.remove(layer_id);
        }
        self.scenario_scene_layers.clear();
        self.trace_graph(format!(
            "BCS playback clear scene sprites count={}",
            layers.len()
        ));
    }

    fn hide_scenario_sprite(&mut self, slot: Option<i32>, wait_frames: u32) {
        let mut layer_ids = Vec::new();
        if let Some(slot) = slot.filter(|slot| (0..=99).contains(slot)) {
            for layer_id in scenario_layer_ids_for_slot(slot) {
                if self.graph_layers.contains_key(&layer_id) {
                    layer_ids.push(layer_id);
                }
            }
        }
        if layer_ids.is_empty() {
            if self.graph_layers.contains_key(&SCENARIO_OVERLAY_LAYER_ID) {
                layer_ids.push(SCENARIO_OVERLAY_LAYER_ID);
            } else {
                layer_ids.extend(self.scenario_scene_layers.iter().copied());
            }
        }
        let fade_frames = wait_frames.max(1);
        for layer_id in layer_ids {
            if let Some(layer) = self.graph_layers.get(&layer_id) {
                self.layer_animations
                    .fade_to(layer_id, layer.opacity, 0.0, fade_frames);
            }
        }
        self.trace_graph(format!(
            "BCS playback hide sprite slot={slot:?} fade={fade_frames}"
        ));
    }

    fn transform_scenario_sprite(
        &mut self,
        slot: i32,
        wait_frames: u32,
        x: Option<i32>,
        y: Option<i32>,
        opacity: Option<f32>,
    ) {
        let layer_ids = scenario_layer_ids_for_slot(slot)
            .into_iter()
            .filter(|layer_id| self.graph_layers.contains_key(layer_id))
            .collect::<Vec<_>>();
        if layer_ids.is_empty() {
            self.trace_graph(format!(
                "BCS playback transform skipped missing slot={slot} x={x:?} y={y:?} opacity={opacity:?}"
            ));
            return;
        }
        let frames = wait_frames.max(1);
        for layer_id in layer_ids {
            if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                if let Some(x) = x {
                    if scene::is_background_layer_id(layer_id) {
                        let max_src_x = self
                            .graph_images
                            .get(&layer.key)
                            .map(|image| (image.width as f32 - layer.width).max(0.0))
                            .unwrap_or(0.0);
                        layer.src_x = (-(x as f32)).clamp(0.0, max_src_x);
                    } else {
                        let from = layer.transform_x;
                        let to = x as f32;
                        if frames > 1 {
                            self.layer_animations.move_x_to(layer_id, from, to, frames);
                        } else {
                            layer.transform_x = to;
                        }
                    }
                }
                if let Some(y) = y {
                    if scene::is_background_layer_id(layer_id) {
                        let max_src_y = self
                            .graph_images
                            .get(&layer.key)
                            .map(|image| (image.height as f32 - layer.height).max(0.0))
                            .unwrap_or(0.0);
                        layer.src_y = (-(y as f32)).clamp(0.0, max_src_y);
                    } else {
                        let from = layer.transform_y;
                        let to = y as f32;
                        if frames > 1 {
                            self.layer_animations.move_y_to(layer_id, from, to, frames);
                        } else {
                            layer.transform_y = to;
                        }
                    }
                }
                if let Some(opacity) = opacity {
                    let from = layer.opacity;
                    let to = opacity.clamp(0.0, 1.0);
                    if frames > 1 {
                        self.layer_animations.fade_to(layer_id, from, to, frames);
                    } else {
                        layer.opacity = to;
                    }
                }
            }
            self.trace_graph(format!(
                "BCS playback transform layer={layer_id} slot={slot} x={x:?} y={y:?} opacity={opacity:?} frames={frames}"
            ));
        }
    }

    fn tick_scenario(&mut self) {
        self.maybe_inject_scenario_mode_input();
        if self.handle_message_control_input() {
            return;
        }
        let input_observed = self.pending_click.is_some()
            || self.pending_object_state.is_some()
            || self.pending_input_state.is_some_and(|state| state != 0);
        if input_observed {
            if self.text_runtime.is_animating() {
                self.reveal_text_on_input();
                self.scenario_input_latched = false;
                self.pending_click = None;
                self.pending_object_state = None;
                self.pending_input_state = None;
                self.pending_input_descriptor = None;
                self.mouse_pressed = false;
                return;
            } else {
                self.scenario_input_latched = true;
            }
        }

        let Some(playback) = self.scenario_playback.as_mut() else {
            return;
        };
        let (action, input_consumed) = playback.tick(self.scenario_input_latched);
        if input_consumed {
            self.scenario_input_latched = false;
            self.pending_click = None;
            self.pending_object_state = None;
            self.pending_input_state = None;
            self.pending_input_descriptor = None;
            self.mouse_pressed = false;
            self.trace_graph("BCS playback consumed latched input");
        }
        let Some(action) = action else {
            return;
        };

        match action {
            ScenarioAction::Sound { file } => {
                self.queue_scenario_sound(&file);
                self.trace_graph(format!("BCS playback sound {file}"));
            }
            ScenarioAction::Bgm { file } => {
                self.queue_scenario_sound(&file);
                self.trace_graph(format!("BCS playback bgm {file}"));
            }
            ScenarioAction::Sprite {
                file,
                wait_frames,
                slot,
                x,
                z,
                opacity,
                body_layer,
            } => {
                self.load_scenario_sprite(&file, wait_frames, slot, x, z, opacity, body_layer);
                self.trace_graph(format!(
                    "BCS playback sprite {file} slot={slot:?} x={x:?} z={z:?} opacity={opacity:?} body_layer={body_layer:?}"
                ));
            }
            ScenarioAction::HideSprite { slot, wait_frames } => {
                self.hide_scenario_sprite(slot, wait_frames);
            }
            ScenarioAction::TransformSprite {
                slot,
                wait_frames,
                x,
                y,
                opacity,
            } => {
                self.transform_scenario_sprite(slot, wait_frames, x, y, opacity);
            }
            ScenarioAction::Message { speaker, text } => {
                self.start_scenario_message(speaker, text);
            }
            ScenarioAction::Wait { .. } => {}
            ScenarioAction::WaitForInput => {}
            ScenarioAction::ClearSprite => {
                self.clear_scenario_sprites();
            }
            ScenarioAction::LoadScript { file } => {
                self.append_scenario_script(&file);
            }
        }
    }

    fn maybe_inject_scenario_mode_input(&mut self) {
        if !self.scenario_bootstrapped
            || self.pending_click.is_some()
            || self.pending_object_state.is_some()
            || self.pending_input_state.is_some()
        {
            return;
        }
        if self.scenario_skip_mode {
            self.pending_click = Some((640.0, 650.0));
            self.pending_click_age_frames = 0;
            self.pending_input_state = Some(0x1000_0006);
            self.pending_input_descriptor = Some(INPUT_DESCRIPTOR_ENTER);
            self.trace_graph("scenario skip mode injected input");
            return;
        }
        if self.scenario_auto_mode {
            if self.text_runtime.is_animating() {
                self.scenario_auto_wait_frames = 0;
                return;
            }
            if self.scenario_auto_wait_frames < 45 {
                self.scenario_auto_wait_frames += 1;
                return;
            }
            self.scenario_auto_wait_frames = 0;
            self.pending_click = Some((640.0, 650.0));
            self.pending_click_age_frames = 0;
            self.pending_input_state = Some(0x1000_0006);
            self.pending_input_descriptor = Some(INPUT_DESCRIPTOR_ENTER);
            self.trace_graph("scenario auto mode injected input");
        }
    }

    fn finish_frame_input(&mut self) {
        let had_input_state = self.pending_input_state.is_some();
        if self.pending_input_state.is_some() && self.auto_title_release_after_state {
            self.mouse_pressed = false;
            self.auto_title_release_after_state = false;
            self.pending_input_state = Some(0x1000_0006);
            self.pending_input_descriptor = Some(INPUT_DESCRIPTOR_ENTER);
            self.trace_graph("auto title click queued release input");
        } else {
            self.pending_input_state = None;
            self.pending_input_descriptor = None;
        }
        if self.pending_click.is_some() {
            self.pending_click_age_frames = self.pending_click_age_frames.saturating_add(1);
        } else {
            self.pending_click_age_frames = 0;
        }
        self.maybe_consume_stale_title_click();
        if !self.mouse_pressed
            && self.pending_input_state.is_none()
            && !self.scenario_overlay_active()
            && !self.title_ui_active
            && self.pending_object_state.is_some()
        {
            self.pending_object_state = None;
            self.trace_graph("stale object state cleared");
        }
        if !self.mouse_pressed
            && !had_input_state
            && self.pending_input_state.is_none()
            && !self.scenario_overlay_active()
            && !self.title_ui_active
            && self.pending_click.is_some()
            && self.pending_click_age_frames > 240
        {
            self.pending_click = None;
            self.pending_click_age_frames = 0;
            self.trace_graph("stale click event expired");
        }
    }

    fn resolve_resource_key(&self, id: i32) -> Option<&str> {
        let resource = self.graph_resources.get(&id).or_else(|| {
            self.graph_bindings
                .get(&id)
                .and_then(|target| self.graph_resources.get(target))
        })?;
        Some(resource.key.as_str())
    }

    fn ensure_transform_layer(&mut self, layer_id: i32, resource_id: i32) -> bool {
        if self.graph_layers.contains_key(&layer_id) {
            return true;
        }
        let Some(key) = self.resolve_resource_key(resource_id).map(str::to_string) else {
            return false;
        };
        let Some(image) = self.graph_images.get(&key) else {
            return false;
        };
        let z = if key.starts_with("sysgrp.arc:SGTitle") {
            5
        } else if key.contains(":bg") || key.contains(":BG") {
            10
        } else {
            80
        };
        self.graph_layers.insert(
            layer_id,
            RuntimeGraphLayer {
                hit_id: 0,
                owner_object: None,
                key: key.clone(),
                target_surface: None,
                x: 0.0,
                y: 0.0,
                width: image.width as f32,
                height: image.height as f32,
                src_x: 0.0,
                src_y: 0.0,
                opacity: 1.0,
                z,
                enabled: true,
                transform_x: 0.0,
                transform_y: 0.0,
                scale_x: 1.0,
                scale_y: 1.0,
                rotation_degrees: 0.0,
                clip: None,
            },
        );
        self.trace_graph(format!(
            "create transform layer #{layer_id} res=#{resource_id} {key} z={z}"
        ));
        true
    }

    fn clear_title_graph_layers(&mut self) {
        let title_layers = self
            .graph_layers
            .iter()
            .filter_map(|(id, layer)| layer.key.starts_with("sysgrp.arc:SGTitle").then_some(*id))
            .collect::<Vec<_>>();
        if title_layers.is_empty() {
            return;
        }
        for layer_id in &title_layers {
            self.graph_layers.remove(layer_id);
        }
        self.layer_animations.clear_layers(title_layers.iter());
        self.trace_graph(format!("cleared {} title graph layers", title_layers.len()));
    }

    fn has_title_child_render_context(&self) -> bool {
        self.title_child_program_active
            || self
                .graph_layers
                .values()
                .any(|layer| is_title_child_layer_key(&layer.key))
            || self
                .graph_resources
                .values()
                .any(|resource| is_title_child_layer_key(&resource.key))
    }

    fn ensure_title_child_resource_layer(&mut self, resource_id: i32) {
        let Some(key) = self.resolve_resource_key(resource_id).map(str::to_string) else {
            return;
        };
        if !self.title_child_program_active && !is_title_child_layer_key(&key) {
            return;
        }
        let Some(image) = self.graph_images.get(&key) else {
            return;
        };
        let layer_id = -30_000 - resource_id.rem_euclid(10_000);
        self.graph_layers.insert(
            layer_id,
            RuntimeGraphLayer {
                hit_id: 0,
                owner_object: None,
                key: key.clone(),
                target_surface: None,
                x: 0.0,
                y: 0.0,
                width: image.width as f32,
                height: image.height as f32,
                src_x: 0.0,
                src_y: 0.0,
                opacity: 1.0,
                z: 40,
                enabled: true,
                transform_x: 0.0,
                transform_y: 0.0,
                scale_x: 1.0,
                scale_y: 1.0,
                rotation_degrees: 0.0,
                clip: None,
            },
        );
        self.trace_graph(format!(
            "title child resource layer #{layer_id} res=#{resource_id} {key}"
        ));
    }

    fn graph_draw_items(&self) -> Vec<RuntimeGraphDrawItem> {
        let mut items = Vec::new();
        self.push_title_surface_composite(&mut items);
        self.push_surface_draw_items(&mut items);
        for layer in self.graph_layers.values() {
            if !self.should_draw_graph_layer(layer) {
                continue;
            }
            if !layer.enabled || layer.opacity <= 0.001 {
                continue;
            }
            if layer.target_surface.is_some_and(|surface| {
                self.graph_surfaces
                    .get(&surface)
                    .is_some_and(|surface| !surface.enabled)
            }) {
                continue;
            }
            let draw_key = self
                .message_control_layer_draw_key(layer)
                .unwrap_or(layer.key.as_str());
            let Some(image) = self.graph_images.get(draw_key) else {
                continue;
            };
            if layer.width <= 0.0 || layer.height <= 0.0 {
                continue;
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
            if src_width <= 0.0 || src_height <= 0.0 {
                continue;
            }
            items.push(RuntimeGraphDrawItem {
                key: draw_key.to_string(),
                x: layer.screen_x(&self.graph_surfaces) + layer.transform_x,
                y: layer.screen_y(&self.graph_surfaces) + layer.transform_y,
                width,
                height,
                src_x,
                src_y,
                src_width,
                src_height,
                opacity: layer.opacity,
                rotation_degrees: layer.rotation_degrees,
                clip: layer.clip,
                z: layer.z,
                hit_id: layer.hit_id,
            });
        }
        items.sort_by_key(|item| (item.z, item.hit_id));
        items
    }

    fn push_surface_draw_items(&self, items: &mut Vec<RuntimeGraphDrawItem>) {
        if !self.has_title_child_render_context() {
            return;
        }
        for surface in self.graph_surfaces.values() {
            if !surface.enabled {
                continue;
            }
            let Some(resource_id) = surface.resource_id else {
                continue;
            };
            let Some(key) = self.resolve_resource_key(resource_id) else {
                continue;
            };
            if is_title_layer(key) {
                continue;
            }
            if key.starts_with("sysgrp.arc:SGMsgWnd") {
                continue;
            }
            let Some(image) = self.graph_images.get(key) else {
                continue;
            };
            let width = surface.viewport_width.min(image.width as f32).max(0.0);
            let height = surface.viewport_height.min(image.height as f32).max(0.0);
            if width <= 0.0 || height <= 0.0 {
                continue;
            }
            items.push(RuntimeGraphDrawItem {
                key: key.to_string(),
                x: surface.x,
                y: surface.y,
                width,
                height,
                src_x: 0.0,
                src_y: 0.0,
                src_width: width,
                src_height: height,
                opacity: 1.0,
                rotation_degrees: 0.0,
                clip: None,
                z: surface.id,
                hit_id: 0,
            });
        }
    }

    fn should_draw_graph_layer(&self, layer: &RuntimeGraphLayer) -> bool {
        let key = layer.key.as_str();
        if self.title_ui_departed && is_title_layer(key) {
            return false;
        }
        if is_title_layer(key) && !is_title_base_layer(key) {
            return false;
        }
        if self.title_ui_active
            && !self.title_child_program_active
            && !self.has_title_child_render_context()
        {
            return false;
        }
        if is_message_control_layer(key) {
            return self.should_draw_message_control_layer(layer);
        }
        if !self.title_ui_active && !self.scenario_bootstrapped && is_boot_suppressed_layer(key) {
            return false;
        }
        true
    }

    fn should_draw_text_node(&self, node: &RuntimeTextNode) -> bool {
        if !node.enabled || node.text.is_empty() {
            return false;
        }
        if node.z == MESSAGE_TEXT_Z || node.z == MESSAGE_NAME_TEXT_Z {
            return self.has_active_message_window();
        }
        if self.title_ui_active
            && !self.title_child_program_active
            && !self.has_title_child_render_context()
        {
            return false;
        }
        if !self.title_ui_active && !self.scenario_bootstrapped {
            return false;
        }
        true
    }

    fn push_title_surface_composite(&self, items: &mut Vec<RuntimeGraphDrawItem>) {
        if !self.title_ui_active || self.title_ui_departed {
            return;
        }
        if !self.title_ui_resources_loaded {
            return;
        }
        let child_context = self.has_title_child_render_context();
        for (key, z) in [
            ("sysgrp.arc:SGTitle990000", 0),
            ("sysgrp.arc:SGTitle990200", 1),
            ("sysgrp.arc:SGTitle990300", 3),
        ] {
            let Some(image) = self.graph_images.get(key) else {
                continue;
            };
            items.push(RuntimeGraphDrawItem {
                key: key.to_string(),
                x: 0.0,
                y: 0.0,
                width: image.width as f32,
                height: image.height as f32,
                src_x: 0.0,
                src_y: 0.0,
                src_width: image.width as f32,
                src_height: image.height as f32,
                opacity: 1.0,
                rotation_degrees: 0.0,
                clip: None,
                z,
                hit_id: 0,
            });
        }
        if child_context {
            return;
        }
        let hovered_title_control = self.mouse_pos.and_then(|point| {
            self.user_controls
                .values()
                .find(|control| is_primary_title_control(control) && control.contains(point, true))
                .map(|control| control.id)
        });
        for control in self
            .user_controls
            .values()
            .filter(|control| is_primary_title_control(control))
        {
            let key = if hovered_title_control == Some(control.id) && self.mouse_pressed {
                "sysgrp.arc:SGTitle000003"
            } else if hovered_title_control == Some(control.id) {
                "sysgrp.arc:SGTitle000001"
            } else {
                "sysgrp.arc:SGTitle000000"
            };
            let Some(image) = self.graph_images.get(key) else {
                continue;
            };
            let src_x = control.x.clamp(0.0, image.width.saturating_sub(1) as f32);
            let src_y = control.y.clamp(0.0, image.height.saturating_sub(1) as f32);
            let src_width = control.width.min(image.width as f32 - src_x).max(0.0);
            let src_height = control.height.min(image.height as f32 - src_y).max(0.0);
            if src_width <= 0.0 || src_height <= 0.0 {
                continue;
            }
            items.push(RuntimeGraphDrawItem {
                key: key.to_string(),
                x: control.x,
                y: control.y,
                width: control.width,
                height: control.height,
                src_x,
                src_y,
                src_width,
                src_height,
                opacity: 1.0,
                rotation_degrees: 0.0,
                clip: None,
                z: 4,
                hit_id: control.id,
            });
        }
    }

    fn trace_render_snapshot(&self, frame: usize, report: Option<&ethornell_vm::VmRunReport>) {
        if !self.debug_graph {
            return;
        }
        let draw_items = self.graph_draw_items();
        let draw_preview = draw_items
            .iter()
            .take(10)
            .map(|item| {
                format!(
                    "#{} z={} {} ({:.1},{:.1}) {:.1}x{:.1} a={:.2}",
                    item.hit_id,
                    item.z,
                    item.key,
                    item.x,
                    item.y,
                    item.width,
                    item.height,
                    item.opacity
                )
            })
            .collect::<Vec<_>>();
        let scene_draw_preview = draw_items
            .iter()
            .filter(|item| item.z < 900)
            .take(12)
            .map(|item| {
                format!(
                    "#{} z={} {} ({:.1},{:.1}) {:.1}x{:.1} src={:.1},{:.1}+{:.1}x{:.1} a={:.2}",
                    item.hit_id,
                    item.z,
                    item.key,
                    item.x,
                    item.y,
                    item.width,
                    item.height,
                    item.src_x,
                    item.src_y,
                    item.src_width,
                    item.src_height,
                    item.opacity
                )
            })
            .collect::<Vec<_>>();
        let text_preview = self
            .text_nodes
            .iter()
            .filter(|(_, node)| self.should_draw_text_node(node))
            .take(8)
            .map(|(id, node)| {
                let text = node.text.chars().take(48).collect::<String>();
                format!(
                    "#{id} ({:.1},{:.1}) size={:.1} {:?}",
                    node.x, node.y, node.size, text
                )
            })
            .collect::<Vec<_>>();
        let trace_tail = self
            .graph_trace
            .iter()
            .rev()
            .take(8)
            .cloned()
            .collect::<Vec<_>>();
        tracing::info!(
            target: "graph_tree",
            frame,
            pc = report.map(|report| report.pc),
            program = report.map(|report| report.program.as_str()),
            offset = ?report.and_then(|report| report.offset),
            reason = ?report.map(|report| &report.stop_reason),
            draw_count = draw_items.len(),
            text_count = self.text_nodes.len(),
            resource_count = self.graph_resources.len(),
            layer_count = self.graph_layers.len(),
            image_count = self.graph_images.len(),
            surface_count = self.graph_surfaces.len(),
            user_control_count = self.user_controls.len(),
            mouse_pos = ?self.mouse_pos,
            mouse_pressed = self.mouse_pressed,
            pending_click = ?self.pending_click,
            pending_object_state = ?self.pending_object_state,
            pending_input_state = ?self.pending_input_state.map(|state| format!("0x{state:08X}")),
            pending_input_descriptor = ?self.pending_input_descriptor,
            continue_vm_after_yield = self.should_continue_vm_after_yield(),
            yield_blockers = ?self.vm_after_yield_blockers(),
            vm_trace_tail = ?report.map(|report| report.recent_trace.iter().rev().take(8).cloned().collect::<Vec<_>>()),
            ?draw_preview,
            ?scene_draw_preview,
            ?text_preview,
            ?trace_tail,
            "render snapshot"
        );
    }

    fn hit_test_graph(&self, point: (f32, f32)) -> Option<i32> {
        let user_hit = if self.scenario_overlay_active() && !self.has_active_message_window() {
            None
        } else {
            self.hit_test_user_controls(point)
        };
        user_hit.or_else(|| {
            self.graph_draw_items().into_iter().rev().find_map(|item| {
                if item.hit_id == 0 {
                    return None;
                }
                let inside = point.0 >= item.x
                    && point.0 < item.x + item.width
                    && point.1 >= item.y
                    && point.1 < item.y + item.height;
                inside.then_some(item.hit_id)
            })
        })
    }
}

struct RuntimeEngine {
    vm: ethornell_vm::Vm,
    api: RuntimeTraceApi,
    options: ethornell_vm::VmRunOptions,
}

impl RuntimeEngine {
    fn new(
        script: &str,
        bytes: &[u8],
        manager: ResourceManager,
        trace: bool,
        fail_on_stub: bool,
    ) -> Self {
        let program = ethornell_script::parse_bp_program(Some(script.to_string()), bytes);
        let mut vm = ethornell_vm::Vm::new();
        vm.start(&program);
        Self {
            vm,
            api: RuntimeTraceApi::new(manager),
            options: ethornell_vm::VmRunOptions {
                max_steps: parse_usize_env("ETHORNELL_VM_MAX_STEPS").unwrap_or(8_000),
                trace,
                fail_on_stub,
            },
        }
    }

    fn tick(&mut self) -> ethornell_vm::VmRunReport {
        self.api.tick_timelines();
        self.api.tick_text();
        self.api.tick_scenario();
        self.api.maybe_auto_click_title();
        self.api.maybe_auto_click_user();
        self.vm.run_loaded(&mut self.api, &self.options)
    }
}

fn parse_i32_env(value: &str) -> Option<i32> {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .and_then(|hex| i32::from_str_radix(hex, 16).ok())
        .or_else(|| value.parse().ok())
}

fn parse_usize_env(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
}

fn parse_env_point(x_key: &str, y_key: &str) -> Option<(f32, f32)> {
    let x = std::env::var(x_key).ok()?.parse().ok()?;
    let y = std::env::var(y_key).ok()?.parse().ok()?;
    Some((x, y))
}

fn fixed_node_coord(raw: i32, extent: f32, screen_extent: f32) -> f32 {
    if raw.unsigned_abs() < 32_768 {
        return raw as f32;
    }
    let value = fixed_16_to_f32(raw);
    if value <= -(extent * 0.4) {
        screen_extent.max(extent) * 0.5 + value
    } else {
        value
    }
}

fn input_descriptor_for_keycode(code: KeyCode) -> Option<i32> {
    match code {
        KeyCode::ArrowLeft => Some(INPUT_DESCRIPTOR_LEFT),
        KeyCode::ArrowUp => Some(INPUT_DESCRIPTOR_UP),
        KeyCode::ArrowRight => Some(INPUT_DESCRIPTOR_RIGHT),
        KeyCode::ArrowDown => Some(INPUT_DESCRIPTOR_DOWN),
        KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => Some(INPUT_DESCRIPTOR_ENTER),
        _ => None,
    }
}

fn input_descriptor_for_headless_key(key: &str) -> Option<i32> {
    match key {
        "left" | "arrowleft" => Some(INPUT_DESCRIPTOR_LEFT),
        "up" | "arrowup" => Some(INPUT_DESCRIPTOR_UP),
        "right" | "arrowright" => Some(INPUT_DESCRIPTOR_RIGHT),
        "down" | "arrowdown" => Some(INPUT_DESCRIPTOR_DOWN),
        "enter" | "return" | "space" => Some(INPUT_DESCRIPTOR_ENTER),
        _ => None,
    }
}

fn apply_headless_input_event(api: &mut RuntimeTraceApi, event: HeadlessInputEvent) {
    match event {
        HeadlessInputEvent::MouseMove { x, y } => {
            api.mouse_pos = Some((x, y));
            tracing::info!(x, y, "headless input mouse move");
        }
        HeadlessInputEvent::MousePress { x, y } => {
            let point = Some((x, y));
            api.mouse_pos = point;
            api.mouse_pressed = true;
            api.pending_object_state = point;
            api.pending_input_state = Some(0x1000_0002);
            api.pending_input_descriptor = Some(INPUT_DESCRIPTOR_ENTER);
            tracing::info!(x, y, "headless input mouse press");
        }
        HeadlessInputEvent::MouseRelease { x, y } => {
            let point = Some((x, y));
            api.mouse_pos = point;
            api.mouse_pressed = false;
            api.pending_click = point;
            api.pending_click_age_frames = 0;
            api.pending_input_state = Some(0x1000_0006);
            api.pending_input_descriptor = Some(INPUT_DESCRIPTOR_ENTER);
            tracing::info!(x, y, "headless input mouse release");
        }
        HeadlessInputEvent::KeyPress { key } => {
            if let Some(descriptor) = input_descriptor_for_headless_key(&key) {
                api.pending_input_state = Some(0x1000_0002);
                api.pending_input_descriptor = Some(descriptor);
                tracing::info!(key, descriptor, "headless input key press");
            } else {
                tracing::warn!(key, "unsupported headless input key");
            }
        }
    }
}

#[derive(Debug, Clone)]
struct AudioRequest {
    archive: String,
    file: String,
    bytes: Vec<u8>,
}

fn empty_bp_program(name: String) -> ethornell_script::BpProgram {
    let mut labels = HashMap::new();
    labels.insert(0x10, 0);
    ethornell_script::BpProgram {
        script_name: Some(name),
        functions: Vec::new(),
        strings: Vec::new(),
        instructions: vec![ethornell_script::BpInstruction {
            offset: 0x10,
            opcode: ethornell_script::BpOpcode::Known {
                code: 0x17,
                name: "ret",
            },
            opcode_hex: "0x17".into(),
            opcode_name: "ret".into(),
            operands: Vec::new(),
            known_call: None,
            raw: vec![0x17],
            warning: Some("generated empty program after runtime load failure".into()),
        }],
        labels,
        warnings: vec!["generated empty program after runtime load failure".into()],
    }
}

impl ethornell_vm::SysApi for RuntimeTraceApi {
    fn load_file_bytes(&mut self, archive: &str, file: &str) -> Option<Vec<u8>> {
        let bytes = read_runtime_bytes(&self.manager, archive, file);
        tracing::info!(
            archive,
            file,
            size = bytes.as_ref().map(Vec::len).unwrap_or_default(),
            found = bytes.is_some(),
            "LoadFileBytes"
        );
        if file.eq_ignore_ascii_case("main") {
            if let Some(bytes) = bytes.as_deref() {
                if self.title_ui_departed {
                    self.start_scenario_playback(archive, file, bytes);
                } else {
                    self.pending_scenario_bootstrap =
                        Some((archive.to_string(), file.to_string(), bytes.to_vec()));
                    self.trace_graph(format!("BCS playback bootstrap deferred {archive}:{file}"));
                    if self.title_ui_active && self.title_scenario_requested {
                        self.depart_title_ui_for_scenario(file);
                    }
                }
            }
        }
        bytes
    }

    fn file_exists(&mut self, archive: &str, file: &str) -> bool {
        self.cached_file_exists(archive, file)
    }

    fn file_size(&mut self, archive: &str, file: &str) -> i32 {
        self.cached_file_size(archive, file)
    }

    fn write_file_bytes(&mut self, path: &str, bytes: &[u8]) -> bool {
        let Some(path) = runtime_file_path(&self.manager, path) else {
            tracing::warn!(path, "WriteFileBytes rejected path");
            return false;
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, bytes)
        })();
        let ok = result.is_ok();
        let file_key = path
            .strip_prefix(self.manager.archives().root.as_path())
            .ok()
            .map(|relative| {
                relative
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            });
        if let Some(file) = file_key.as_deref() {
            let key = runtime_file_cache_key("", file);
            self.file_exists_cache.remove(&key);
            self.file_size_cache.remove(&key);
        }
        match result {
            Ok(()) => tracing::info!(
                path = %path.display(),
                size = bytes.len(),
                "WriteFileBytes"
            ),
            Err(err) => tracing::warn!(
                path = %path.display(),
                size = bytes.len(),
                %err,
                "WriteFileBytes failed"
            ),
        }
        ok
    }

    fn load_program(&mut self, archive: &str, file: &str) -> Option<ethornell_script::BpProgram> {
        if self.has_cached_bp_program(archive, file) {
            tracing::debug!(archive, file, "LoadProgram");
        } else {
            tracing::info!(archive, file, "LoadProgram");
        }
        self.observe_loaded_program_for_title(file);
        Some(self.load_bp_program_cached(archive, file, "LoadProgram"))
    }

    fn load_program_ex(
        &mut self,
        archive: &str,
        file: &str,
        params: &[ethornell_vm::Value],
    ) -> Option<ethornell_script::BpProgram> {
        if self.has_cached_bp_program(archive, file) {
            tracing::debug!(archive, file, ?params, "LoadProgramEx");
        } else {
            tracing::info!(archive, file, ?params, "LoadProgramEx");
        }
        self.observe_loaded_program_for_title(file);
        Some(self.load_bp_program_cached(archive, file, "LoadProgramEx"))
    }

    fn free_program(&mut self, program: ethornell_vm::Value) {
        tracing::info!(program = ?value_summary(&program), "FreeProgram");
    }

    fn dispatch_object_event(
        &mut self,
        object: i32,
        count: i32,
        descriptor: &[ethornell_vm::Value],
    ) -> ethornell_vm::VmResult<()> {
        let event = descriptor.first().map(value_to_i32).unwrap_or_default();
        let payload = descriptor
            .get(1)
            .map(value_to_i32)
            .unwrap_or(self.last_hit_payload);
        if event != 0 {
            if object != 0 {
                self.last_hit_control = object;
            }
            if payload != 0 {
                self.last_hit_payload = payload;
            }
            self.trace_graph(format!(
                "dispatch object event object=#{object} count={count} event=0x{event:08X} payload=0x{:04X}",
                self.last_hit_payload
            ));
        }
        Ok(())
    }

    fn read_input_state(&mut self, descriptor: i32) -> i32 {
        let state = match (self.pending_input_state, self.pending_input_descriptor) {
            (Some(state), Some(active_descriptor)) if active_descriptor == descriptor => state,
            (Some(state), Some(_)) if descriptor == 0 || descriptor == 1 => state,
            (Some(state), None) if descriptor == INPUT_DESCRIPTOR_ENTER => state,
            _ => 0,
        };
        if self.pending_input_state.is_some() {
            self.trace_graph(format!(
                "read input state descriptor={descriptor} active={:?} pending={:?} -> 0x{state:08X}",
                self.pending_input_descriptor, self.pending_input_state
            ));
        }
        if state != 0 {
            self.reveal_text_on_input();
        }
        state
    }

    fn take_frame_yield(&mut self) -> bool {
        std::mem::take(&mut self.frame_yield_requested)
    }

    fn call_sys(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> ethornell_vm::VmResult<ethornell_vm::Value> {
        self.record_call(group, id);
        match (group, id) {
            (0x80, 0x40) => {
                let file = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let archive = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                tracing::info!(archive, file, "LoadProgram");
                self.observe_loaded_program_for_title(&file);
                let program = self.load_bp_program_cached(&archive, &file, "LoadProgram");
                return Ok(ethornell_vm::Value::Program(Box::new(program)));
            }
            (0x80, 0x44) => {
                for _ in 0..3 {
                    let _ = stack.pop();
                }
                let file = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let archive = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                tracing::info!(archive, file, "LoadProgramEx");
                self.observe_loaded_program_for_title(&file);
                let program = self.load_bp_program_cached(&archive, &file, "LoadProgramEx");
                return Ok(ethornell_vm::Value::Program(Box::new(program)));
            }
            (0x80, 0x41) => {
                let program = stack.pop();
                tracing::info!(program = ?program.as_ref().map(value_summary), "FreeProgram");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x34) => {
                let file = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let archive = pop_string_value(stack).unwrap_or_default();
                let exists = self.cached_file_exists(&archive, &file);
                if exists {
                    tracing::info!(archive, file, exists, "FileExists");
                } else {
                    tracing::debug!(archive, file, exists, "FileExists");
                }
                return Ok(ethornell_vm::Value::Int(i32::from(exists)));
            }
            (0x80, 0x35) => {
                let file = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let archive = pop_string_value(stack).unwrap_or_default();
                let size = self.cached_file_size(&archive, &file);
                tracing::info!(archive, file, size, "GetFileSize");
                return Ok(ethornell_vm::Value::Int(size));
            }
            (0x80, 0x1b) => {
                let descriptor = stack.pop();
                let size = stack.pop();
                tracing::info!(?descriptor, ?size, "RegisterMemoryClass");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x1c) => {
                let descriptor = stack.pop();
                tracing::debug!(?descriptor, "SystemSetMemoryClass");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x28) => {
                let path = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                tracing::info!(path, "CreateDirectory");
                return Ok(ethornell_vm::Value::Int(1));
            }
            (0x80, 0x2a) => {
                let path = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                tracing::info!(path, "DirectoryExists");
                return Ok(ethornell_vm::Value::Int(1));
            }
            (0x80, 0x31) => {
                let args = pop_args(stack, 5);
                tracing::info!(args = ?summarize_values(&args), "ReadProfileString");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x33) => {
                let mode = stack.pop();
                let key = stack.pop();
                tracing::info!(?key, ?mode, "FreeProfileString");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x36) => {
                let value = stack.pop();
                tracing::info!(?value, "CommitResourceSearchPaths");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x80) => {
                let value = stack.pop();
                tracing::info!(?value, "Sys80Triple");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x81) => {
                if self.should_yield_frame() {
                    self.frame_yield_requested = true;
                }
                tracing::debug!("Sys81FrameBoundary");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x8a) => {
                let arg4 = stack.pop();
                let arg3 = stack.pop();
                let arg2 = stack.pop();
                let name = stack.pop();
                let duration = arg4
                    .as_ref()
                    .map(value_to_i32)
                    .unwrap_or_default()
                    .unsigned_abs();
                self.animation_queue_remaining = duration.div_ceil(16).max(1);
                tracing::debug!(
                    ?name,
                    ?arg2,
                    ?arg3,
                    ?arg4,
                    remaining = self.animation_queue_remaining,
                    "AnimationQueueBegin"
                );
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x8b) => {
                let arg1 = stack.pop();
                let finished = self.animation_queue_remaining == 0;
                tracing::debug!(
                    ?arg1,
                    remaining = self.animation_queue_remaining,
                    finished,
                    "AnimationQueueStep"
                );
                return Ok(ethornell_vm::Value::Int(i32::from(finished)));
            }
            (0x80, 0x82) | (0x80, 0x83) => {
                let arg3 = stack.pop();
                let arg2 = stack.pop();
                let arg1 = stack.pop();
                tracing::info!(?arg1, ?arg2, ?arg3, "SysBlock");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x98) => {
                let arg3 = stack.pop();
                let arg2 = stack.pop();
                let arg1 = stack.pop();
                tracing::info!(?arg1, ?arg2, ?arg3, "Sys98Triple");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x9d) => {
                let arg3 = stack.pop();
                let arg2 = stack.pop();
                let arg1 = stack.pop();
                tracing::debug!(?arg1, ?arg2, ?arg3, "Sys9DTriple");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xa8) => {
                let arg2 = stack.pop();
                let arg1 = stack.pop();
                tracing::debug!(?arg1, ?arg2, "SysA8Pair");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x99) => {
                let handle = stack.pop();
                tracing::info!(?handle, "Sys99Release");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xd0) => {
                let arg2 = stack.pop();
                let arg1 = stack.pop();
                tracing::info!(?arg1, ?arg2, "SysD0Pair");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x37) => {
                let path = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                tracing::info!(path, "AddResourceSearchPath");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x52) => {
                self.window_mode = pop_int_value(stack).unwrap_or_default();
                tracing::info!(mode = self.window_mode, "SetHeadlessWindowMode");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x50) => {
                let enabled = stack.pop();
                tracing::debug!(?enabled, "SetSystemUiState");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x5f) => {
                self.frame_yield_requested = true;
                tracing::debug!("Yield");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x58) => {
                let time = pop_int_value(stack).unwrap_or_default();
                tracing::info!(time, "SetTimer");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x5a) => {
                tracing::info!("PumpMessages");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x60) => {
                let arg3 = stack.pop();
                let arg2 = stack.pop();
                let arg1 = stack.pop();
                tracing::info!(?arg1, ?arg2, ?arg3, "Sys60Triple");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x5c) => {
                let arg3 = stack.pop();
                let arg2 = stack.pop();
                let arg1 = stack.pop();
                tracing::info!(?arg1, ?arg2, ?arg3, "Sys5CTriple");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x62) => {
                let descriptor = stack.pop();
                let mode = stack.pop();
                tracing::info!(?descriptor, ?mode, "CommitMemoryClasses");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x64) => {
                let enabled = stack.pop();
                tracing::info!(?enabled, "SystemInit64");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x66) => {
                let arg = stack.pop();
                tracing::info!(?arg, "Sys66");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x46) => {
                tracing::info!("SystemInit46");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x67) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "Sys67");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x68) => {
                let enabled = pop_int_value(stack).unwrap_or_default();
                tracing::info!(enabled, "CaptureCloseEvent");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x6a) => {
                tracing::info!("Sys6A");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x70) => {
                let size = pop_int_value(stack).unwrap_or_default();
                tracing::info!(size, "AllocGlobalMem");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x74) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "Sys74");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xaf) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "SysAF");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xc1) => {
                let ptr = stack.pop();
                tracing::info!(?ptr, "SysC1Buffer");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xc5) => {
                let dst = stack.pop();
                let src = stack.pop();
                tracing::info!(?src, ?dst, "SysC5Buffers");
                return Ok(ethornell_vm::Value::Int(1));
            }
            (0x80, 0xd1) => {
                let handle = stack.pop();
                tracing::info!(?handle, "SysD1RecordClose");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xd2) => {
                let src = stack.pop();
                let dst = stack.pop();
                let handle = stack.pop();
                tracing::info!(?handle, ?dst, ?src, "SysD2Record");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xd4) => {
                let mode = stack.pop();
                let selector = stack.pop();
                let handle = stack.pop();
                let dst = stack.pop();
                tracing::info!(?dst, ?handle, ?selector, ?mode, "SysD4RecordFetch");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0xda) => {
                let arg3 = stack.pop();
                let arg2 = stack.pop();
                let arg1 = stack.pop();
                tracing::info!(?arg1, ?arg2, ?arg3, "SysDATriple");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xe8) => {
                let ptr = stack.pop();
                tracing::info!(?ptr, value = "Tayutama2TV", "GetGameId");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xfd) => {
                tracing::info!("IsLauncher");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x81, 0x0e) => {
                let value = stack.pop();
                tracing::info!(?value, "Sys2_0E");
                return Ok(ethornell_vm::Value::None);
            }
            (0x81, 0x0f) => {
                let active = self.animation_queue_remaining > 0;
                tracing::debug!(
                    remaining = self.animation_queue_remaining,
                    active,
                    "AnimationQueuePoll"
                );
                return Ok(ethornell_vm::Value::Int(i32::from(active)));
            }
            (0x81, 0x18) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "Sys2_18");
                return Ok(ethornell_vm::Value::None);
            }
            (0x81, 0x35) => {
                let file = pop_string_value(stack).unwrap_or_default();
                let archive = pop_string_value(stack).unwrap_or_default();
                tracing::info!(archive, file, "OpenResourceHandle");
                return Ok(ethornell_vm::Value::Int(1));
            }
            (0x81, 0x30) => {
                let args = pop_args(stack, 5);
                tracing::info!(args = ?summarize_values(&args), "ReadResourceToBuffer");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x81, 0x60) => {
                self.screen_height = pop_int_value(stack).unwrap_or_default();
                self.screen_width = pop_int_value(stack).unwrap_or_default();
                let flags = pop_int_value(stack).unwrap_or_default();
                tracing::info!(
                    flags,
                    width = self.screen_width,
                    height = self.screen_height,
                    "ConfigureScreen"
                );
                return Ok(ethornell_vm::Value::None);
            }
            (0x81, 0x62) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "Sys2_62");
                return Ok(ethornell_vm::Value::None);
            }
            (0x81, 0x63) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "Sys2_63");
                return Ok(ethornell_vm::Value::None);
            }
            (0x81, 0x64) => {
                self.screen_height = pop_int_value(stack).unwrap_or_default();
                self.screen_width = pop_int_value(stack).unwrap_or_default();
                tracing::info!(
                    width = self.screen_width,
                    height = self.screen_height,
                    "ConfigureScreenSize"
                );
                return Ok(ethornell_vm::Value::None);
            }
            (0x81, 0x6f) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "Sys2_6F");
                return Ok(ethornell_vm::Value::None);
            }
            _ => {}
        }
        let consumed_args = consume_fallback_args(group, id, stack);
        tracing::warn!(
            group = format_args!("0x{group:02X}"),
            id = format_args!("0x{id:02X}"),
            args = ?summarize_values(&consumed_args),
            stack_len = stack.len(),
            stack_top = ?stack_top_summary(stack, 8),
            "runtime syscall stub"
        );
        Ok(ethornell_vm::Value::None)
    }
}

impl ethornell_vm::GraphApi for RuntimeTraceApi {
    fn call_graph(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> ethornell_vm::VmResult<ethornell_vm::Value> {
        self.record_call(group, id);
        match (group, id) {
            (0x90, 0x06) => {
                let y = pop_int_value(stack).unwrap_or_default();
                let x = pop_int_value(stack).unwrap_or_default();
                tracing::info!(x, y, "GraphSetCenter");
            }
            (0x90, 0x07) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "GraphSetDefaultDuration");
            }
            (0x90, 0x00) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::debug!(value, "GraphInit");
            }
            (0x90, 0x02) => {
                let delay = pop_int_value(stack).unwrap_or_default();
                tracing::info!(delay, "GraphDelay");
            }
            (0x90, 0x03) => {
                let limit = pop_int_value(stack).unwrap_or_default();
                tracing::info!(limit, "GraphSetMemoryLimit");
            }
            (0x90, 0x04) => {
                let context = pop_int_value(stack).unwrap_or_default();
                tracing::debug!(context, "GraphSetDrawContext");
            }
            (0x90, 0x08) => {
                let enabled = pop_int_value(stack).unwrap_or_default();
                tracing::info!(enabled, "GraphSetEnabled");
            }
            (0x90, 0x09) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "GraphSys09");
            }
            (0x90, 0x0c) => {
                let arg2 = pop_int_value(stack).unwrap_or_default();
                let arg1 = pop_int_value(stack).unwrap_or_default();
                tracing::info!(arg1, arg2, "GraphSystemInit");
            }
            (0x90, 0x4c) => {
                let arg2 = pop_int_value(stack).unwrap_or_default();
                let arg1 = pop_int_value(stack).unwrap_or_default();
                tracing::info!(arg1, arg2, "GraphSystemInit2");
            }
            (0x90, 0x50) => {
                let handle = self.alloc_node();
                self.text_nodes.insert(
                    handle,
                    RuntimeTextNode {
                        text: String::new(),
                        enabled: true,
                        x: 0.0,
                        y: 0.0,
                        size: self.text_state.font_size,
                        color: self.text_state.color,
                        z: 950,
                    },
                );
                tracing::info!(handle, "GraphCreateNode");
                self.trace_graph(format!("create node #{handle}"));
                return Ok(ethornell_vm::Value::Int(handle));
            }
            (0x90, 0x51) => {
                let target = stack.pop();
                let node = target.as_ref().map(value_to_i32).unwrap_or_default();
                let removed_text = self.text_nodes.remove(&node).is_some();
                tracing::debug!(?target, node, removed_text, "GraphNodeRelease");
                if removed_text {
                    self.trace_graph(format!("release node #{node}"));
                }
            }
            (0x90, 0x54) => {
                let enabled = stack.pop();
                let node = stack.pop();
                tracing::info!(?node, ?enabled, "GraphNodeSetEnabled");
                if let Some(node) = node.as_ref().map(value_to_i32) {
                    if let Some(record) = self.text_nodes.get_mut(&node) {
                        record.enabled = enabled.as_ref().map(value_to_i32).unwrap_or(0) != 0;
                    }
                }
            }
            (0x90, 0x56) => {
                let mut args = Vec::new();
                for _ in 0..7 {
                    args.push(stack.pop());
                }
                tracing::debug!(?args, "GraphNodeSetText");
                let text = args.iter().flatten().find_map(|value| match value {
                    ethornell_vm::Value::Str(text) => Some(text.clone()),
                    _ => None,
                });
                let node = args.iter().flatten().find_map(|value| match value {
                    ethornell_vm::Value::Int(v) if self.text_nodes.contains_key(v) => Some(*v),
                    _ => None,
                });
                if let Some(node) = node {
                    if let (Some(text), Some(record)) = (text, self.text_nodes.get_mut(&node)) {
                        let state = self.text_state.clone();
                        record.text = wrap_text_to_state(&text, &state);
                        record.x = if record.x > 0.0 { record.x } else { state.x };
                        record.y = if record.y > 0.0 { record.y } else { state.y };
                        record.size = state.font_size.max(18.0);
                        record.color = state.color;
                        record.z = record.z.max(950);
                        self.trace_graph(format!("node #{node} text={text:?}"));
                    }
                }
            }
            (0x90, 0x5c) => {
                let mut args = Vec::new();
                for _ in 0..17 {
                    args.push(stack.pop());
                }
                let ints = args
                    .iter()
                    .map(|value| value.as_ref().map(value_to_i32).unwrap_or_default())
                    .collect::<Vec<_>>();
                if let Some(node_id) = ints.last().copied().filter(|node| *node > 0) {
                    let resource_id = ints
                        .iter()
                        .copied()
                        .find(|resource| self.resolve_resource_key(*resource).is_some());
                    if let Some(resource_id) = resource_id {
                        if let Some(key) =
                            self.resolve_resource_key(resource_id).map(str::to_string)
                        {
                            if let Some(image) = self.graph_images.get(&key) {
                                let width = image.width as f32;
                                let height = image.height as f32;
                                let raw_x = ints.get(15).copied().unwrap_or_default();
                                let raw_y = ints.get(14).copied().unwrap_or_default();
                                let x = fixed_node_coord(raw_x, width, self.screen_width as f32);
                                let y = fixed_node_coord(raw_y, height, self.screen_height as f32);
                                self.graph_layers.insert(
                                    node_id,
                                    RuntimeGraphLayer {
                                        hit_id: node_id,
                                        owner_object: self.current_graph_object,
                                        key: key.clone(),
                                        target_surface: None,
                                        x,
                                        y,
                                        width,
                                        height,
                                        src_x: 0.0,
                                        src_y: 0.0,
                                        opacity: 1.0,
                                        z: 600 + node_id,
                                        enabled: true,
                                        transform_x: 0.0,
                                        transform_y: 0.0,
                                        scale_x: 1.0,
                                        scale_y: 1.0,
                                        rotation_degrees: 0.0,
                                        clip: None,
                                    },
                                );
                                if let Some(object) = self.current_graph_object {
                                    self.graph_object_layers
                                        .entry(object)
                                        .or_default()
                                        .insert(node_id);
                                }
                                self.trace_graph(format!(
                                    "node image #{node_id} res=#{resource_id} {key} x={x:.0} y={y:.0} w={width:.0} h={height:.0}"
                                ));
                            }
                        }
                    }
                }
                tracing::info!(?args, "GraphNodeConfigureImage");
            }
            (0x90, 0x60) => {
                let handle = self.alloc_object();
                self.set_current_graph_object(handle);
                tracing::info!(handle, "GraphCreateObject");
                self.trace_graph(format!("create object #{handle}"));
                return Ok(ethornell_vm::Value::Int(handle));
            }
            (0x90, 0x61) => {
                let object = stack.pop();
                tracing::info!(?object, "GraphObjectUpdate");
            }
            (0x90, 0x64) => {
                let enabled = stack.pop();
                let object = stack.pop();
                let object_id = object.as_ref().map(value_to_i32).unwrap_or_default();
                let enabled_value = enabled.as_ref().map(value_to_i32).unwrap_or_default() != 0;
                if object_id > 0 {
                    self.set_graph_object_enabled(object_id, enabled_value);
                    if self.graph_surfaces.contains_key(&object_id) {
                        if let Some(surface) = self.graph_surfaces.get_mut(&object_id) {
                            surface.enabled = enabled_value;
                        }
                        self.trace_graph(format!(
                            "surface/object #{object_id} enabled={enabled_value}"
                        ));
                    }
                    if let Some(layer) = self.graph_layers.get_mut(&object_id) {
                        layer.enabled =
                            enabled_value && !is_default_hidden_object_state(&layer.key);
                    }
                }
                tracing::info!(?object, ?enabled, "GraphObjectSetEnabled");
            }
            (0x90, 0x65) => {
                let args = pop_args(stack, 4);
                if let Some(object) = args.get(3).map(value_to_i32).filter(|object| *object > 0) {
                    self.set_current_graph_object(object);
                    self.trace_graph(format!(
                        "object #{object} configure args={:?}",
                        args.iter().map(value_to_i32).collect::<Vec<_>>()
                    ));
                }
                tracing::info!(args = ?summarize_values(&args), "GraphObjectConfigure");
            }
            (0x90, 0x80) => {
                let height = stack.pop();
                let width = stack.pop();
                let handle = self.alloc_surface();
                let width_value = width.as_ref().map(value_to_i32).unwrap_or_default().max(1);
                let height_value = height.as_ref().map(value_to_i32).unwrap_or_default().max(1);
                self.graph_surfaces.insert(
                    handle,
                    RuntimeSurface::new(handle, width_value as f32, height_value as f32),
                );
                tracing::info!(handle, ?width, ?height, "GraphCreateSurface");
                self.trace_graph(format!(
                    "create surface #{handle} width={width_value} height={height_value}"
                ));
                return Ok(ethornell_vm::Value::Int(handle));
            }
            (0x90, 0x81) => {
                let target = stack.pop();
                tracing::debug!(?target, "GraphSurfaceFlush");
            }
            (0x90, 0x83) => {
                let surface = stack.pop();
                let buffer = stack.pop();
                let surface_id = surface.as_ref().map(value_to_i32).unwrap_or_default();
                let buffer_id = buffer.as_ref().map(value_to_i32).unwrap_or_default();
                let trace = if let Some(record) = self.graph_surfaces.get_mut(&surface_id) {
                    if self.graph_resources.contains_key(&buffer_id) {
                        record.resource_id = Some(buffer_id);
                    }
                    Some(format!(
                        "surface #{} bind buffer=#{buffer_id} resource={:?}",
                        record.id, record.resource_id
                    ))
                } else {
                    None
                };
                if let Some(trace) = trace {
                    self.trace_graph(trace);
                }
                tracing::info!(?buffer, ?surface, "GraphSurfaceBindBuffer");
            }
            (0x90, 0x84) => {
                let enabled = stack.pop();
                let surface = stack.pop();
                let surface_id = surface.as_ref().map(value_to_i32).unwrap_or_default();
                let enabled_value = enabled.as_ref().map(value_to_i32).unwrap_or_default() != 0;
                if let Some(record) = self.graph_surfaces.get_mut(&surface_id) {
                    record.enabled = enabled_value;
                    self.trace_graph(format!("surface #{surface_id} enabled={enabled_value}"));
                }
                tracing::info!(?surface, ?enabled, "GraphSurfaceSetEnabled");
            }
            (0x90, 0x85) => {
                let args = pop_args(stack, 7);
                if args.len() >= 7 {
                    let surface_id = value_to_i32(&args[6]);
                    let trace = self.graph_surfaces.get(&surface_id).map(|surface| {
                        format!(
                            "surface #{} region args={:?}",
                            surface.id,
                            args.iter().map(value_to_i32).collect::<Vec<_>>()
                        )
                    });
                    if let Some(trace) = trace {
                        self.trace_graph(trace);
                    }
                }
                tracing::info!(?args, "GraphSurfaceSetRegion");
            }
            (0x90, 0x86) => {
                let args = pop_args(stack, 4);
                if args.len() >= 4 {
                    let surface_id = value_to_i32(&args[3]);
                    let resource_id = value_to_i32(&args[0]);
                    let trace = if let Some(surface) = self.graph_surfaces.get_mut(&surface_id) {
                        if resource_id > 0 {
                            surface.resource_id = Some(resource_id);
                        }
                        Some(format!(
                            "surface #{} configure resource={:?} args={:?}",
                            surface.id,
                            surface.resource_id,
                            args.iter().map(value_to_i32).collect::<Vec<_>>()
                        ))
                    } else {
                        None
                    };
                    if let Some(trace) = trace {
                        self.trace_graph(trace);
                    }
                }
                tracing::info!(?args, "GraphSurfaceConfigure");
            }
            (0x90, 0x87) => {
                let value = stack.pop();
                let surface = stack.pop();
                tracing::info!(?surface, ?value, "GraphSurfaceSetOption");
            }
            (0x90, 0x88) => {
                let args = pop_args(stack, 5);
                if args.len() >= 5 {
                    let surface_id = value_to_i32(&args[4]);
                    let height = value_to_i32(&args[0]).max(1) as f32;
                    let width = value_to_i32(&args[1]).max(1) as f32;
                    let x = value_to_i32(&args[2]) as f32;
                    let y = value_to_i32(&args[3]) as f32;
                    let trace = if let Some(surface) = self.graph_surfaces.get_mut(&surface_id) {
                        surface.x = x;
                        surface.y = y;
                        surface.viewport_width = width;
                        surface.viewport_height = height;
                        Some(format!(
                            "surface #{} viewport x={} y={} w={} h={} backing={}x{}",
                            surface.id,
                            surface.x,
                            surface.y,
                            surface.viewport_width,
                            surface.viewport_height,
                            surface.width,
                            surface.height
                        ))
                    } else {
                        None
                    };
                    if let Some(trace) = trace {
                        self.trace_graph(trace);
                    }
                }
                tracing::info!(?args, "GraphSurfaceSetViewport");
            }
            (0x90, 0x0d) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "GraphSetFlag");
            }
            (0x90, 0x10) => {
                let name = stack.pop();
                let archive = stack.pop();
                let target = stack.pop();
                tracing::info!(?target, ?archive, ?name, "GraphLoadResource");
                let target_id = target.as_ref().map(value_to_i32).unwrap_or_default();
                let archive_name = archive
                    .as_ref()
                    .and_then(value_to_string)
                    .unwrap_or_default();
                let resource_name = name.as_ref().and_then(value_to_string).unwrap_or_default();
                if self.load_graph_image_resource(target_id, &archive_name, &resource_name)
                    && archive_name.eq_ignore_ascii_case("sysgrp.arc")
                    && resource_name.starts_with("SGTitle")
                {
                    self.ensure_title_atlas_images();
                    if !self.title_ui_resources_loaded {
                        self.seed_title_controls();
                        self.title_ui_resources_loaded = true;
                    }
                    if !self.title_ui_departed {
                        self.title_ui_active = true;
                    }
                }
                if archive_name.eq_ignore_ascii_case("sysgrp.arc")
                    && is_title_child_resource_name(&resource_name)
                {
                    self.ensure_title_child_resource_layer(target_id);
                }
            }
            (0x90, 0x11) => {
                let d = stack.pop();
                let c = stack.pop();
                let b = stack.pop();
                let a = stack.pop();
                tracing::debug!(?a, ?b, ?c, ?d, "GraphSys11");
            }
            (0x90, 0x12) => {
                let value = stack.pop();
                tracing::debug!(?value, "GraphSys12");
            }
            (0x90, 0x13) => {
                let b = stack.pop();
                let a = stack.pop();
                tracing::debug!(?a, ?b, "GraphSys13");
            }
            (0x90, 0x16) => {
                let source = stack.pop();
                let target = stack.pop();
                tracing::info!(?target, ?source, "GraphBindResource");
                let target_id = target.as_ref().map(value_to_i32).unwrap_or_default();
                let source_id = source.as_ref().map(value_to_i32).unwrap_or_default();
                let source_available = self.graph_resources.contains_key(&source_id)
                    || self.graph_bindings.contains_key(&source_id);
                if target_id != 0 && source_id != 0 && source_available {
                    self.graph_bindings.insert(target_id, source_id);
                    self.trace_graph(format!("bind #{target_id} -> #{source_id}"));
                }
                return Ok(ethornell_vm::Value::Int(source_available as i32));
            }
            (0x90, 0x18) => {
                let mut args = Vec::new();
                for _ in 0..6 {
                    args.push(stack.pop());
                }
                tracing::debug!(?args, "GraphObjectApply");
            }
            (0x90, 0x20) => {
                let args = pop_args(stack, 5);
                let ints: Vec<i32> = args.iter().map(value_to_i32).collect();
                if ints.len() >= 5 {
                    let duration_ms = ints[2].unsigned_abs();
                    let duration_frames = duration_ms.div_ceil(16).max(1);
                    if duration_ms > 0 {
                        self.animation_queue_remaining =
                            self.animation_queue_remaining.max(duration_frames);
                    }
                    let target = ints[4];
                    let alpha = (target as f32 / 256.0).clamp(0.0, 1.0);
                    if let Some(object) = self.current_graph_object {
                        if alpha > 0.0 {
                            self.adopt_previous_graph_object_layers(object);
                        }
                        if let Some(layer_ids) = self.graph_object_layers.get(&object).cloned() {
                            let layer_count = layer_ids.len();
                            for layer_id in layer_ids {
                                if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                                    let from = layer.opacity;
                                    if duration_frames > 1 && (from - alpha).abs() > f32::EPSILON {
                                        self.layer_animations.fade_to(
                                            layer_id,
                                            from,
                                            alpha,
                                            duration_frames,
                                        );
                                    } else {
                                        layer.opacity = alpha;
                                    }
                                }
                            }
                            self.trace_graph(format!(
                                "object #{object} transition alpha={alpha:.2} duration_ms={duration_ms} frames={duration_frames} layers={layer_count}"
                            ));
                        }
                    }
                }
                tracing::info!(?args, "GraphObjectTransition");
            }
            (0x90, 0x1f) => {
                let args = pop_args(stack, 10);
                tracing::info!(args = ?summarize_values(&args), "GraphObjectConfig");
                let ints: Vec<i32> = args.iter().map(value_to_i32).collect();
                if ints.len() >= 6 {
                    let id = ints[5];
                    let resource_id = ints[4];
                    let x = ints[3] as f32;
                    let y = ints[2] as f32;
                    let width = ints[1] as f32;
                    let height = ints[0] as f32;
                    let target_surface = ints
                        .get(6)
                        .copied()
                        .filter(|target| self.graph_surfaces.contains_key(target));
                    if let Some(key) = self.resolve_resource_key(resource_id).map(str::to_string) {
                        let owner_object = self.register_layer_for_current_object(id);
                        let enabled = self.object_enabled_for_layer(owner_object)
                            && !is_default_hidden_object_state(&key);
                        self.graph_layers.insert(
                            id,
                            RuntimeGraphLayer {
                                hit_id: id,
                                owner_object,
                                key: key.clone(),
                                target_surface,
                                x,
                                y,
                                width,
                                height,
                                src_x: x,
                                src_y: y,
                                opacity: 1.0,
                                z: id,
                                enabled,
                                transform_x: 0.0,
                                transform_y: 0.0,
                                scale_x: 1.0,
                                scale_y: 1.0,
                                rotation_degrees: 0.0,
                                clip: None,
                            },
                        );
                        self.trace_graph(format!(
                            "object config #{id} owner={owner_object:?} res=#{resource_id} key={key} surface={target_surface:?} x={x} y={y} w={width} h={height} enabled={enabled}"
                        ));
                    }
                    if let Some(text) = args.iter().rev().find_map(|value| match value {
                        ethornell_vm::Value::Str(text) if !text.trim().is_empty() => {
                            Some(text.clone())
                        }
                        _ => None,
                    }) {
                        let title_payload = title_payload_for_control_text(&text);
                        let state = TextState {
                            x: x + 8.0,
                            y: y + (height * 0.5 - 10.0).max(0.0),
                            width: (width - 16.0).max(1.0),
                            height,
                            font_size: if height <= 58.0 { 18.0 } else { 24.0 },
                            color: [1.0, 1.0, 1.0, 1.0],
                            line_height: if height <= 58.0 { 22.0 } else { 30.0 },
                        };
                        let should_create_text = (!self.title_ui_active
                            || self.title_child_program_active)
                            && title_payload.is_none();
                        if should_create_text {
                            let text_id = id.saturating_add(50_000);
                            self.text_nodes.insert(
                                text_id,
                                RuntimeTextNode {
                                    text: wrap_text_to_state(&text, &state),
                                    x: state.x,
                                    y: state.y,
                                    color: state.color,
                                    size: state.font_size,
                                    z: id.saturating_add(1).max(1000),
                                    enabled: true,
                                },
                            );
                            self.trace_graph(format!(
                                "object config text #{text_id} owner=#{id} text={text:?} x={:.0} y={:.0} w={:.0} h={:.0}",
                                state.x, state.y, state.width, state.height
                            ));
                        } else if let Some(payload) = title_payload {
                            self.trace_graph(format!(
                                "title control text observed owner=#{id} payload={payload} text={text:?}"
                            ));
                        }
                        let should_register_control = !self.title_ui_active
                            || self.title_child_program_active
                            || title_payload.is_some();
                        if should_register_control && width > 16.0 && height > 16.0 {
                            let title_object = title_object_for_control_text(&text);
                            let control_key = title_object.unwrap_or(id);
                            let control = self.user_controls.entry(id).or_default();
                            control.id = control_key;
                            control.owner_id = id;
                            control.payload = title_payload.unwrap_or(id);
                            control.x = x;
                            control.y = y;
                            control.width = width;
                            control.height = height;
                            control.enabled = true;
                            control.title_only = self.title_ui_active
                                && !self.title_child_program_active
                                && title_payload.is_some();
                        }
                    }
                }
            }
            (0x90, 0x32) => {
                let value = stack.pop();
                let object = stack.pop();
                tracing::debug!(?object, ?value, "GraphObjectCommit");
            }
            (0x90, 0x30) => {
                let enabled = stack.pop();
                let timeline = stack.pop();
                let handle = timeline.as_ref().map(value_to_i32).unwrap_or_default();
                let enabled_value = enabled.as_ref().map(value_to_i32).unwrap_or_default() != 0;
                let event = self.timelines.set_enabled(handle, enabled_value);
                tracing::info!(?timeline, ?enabled, ?event, "GraphTimelineSetEnabled");
                self.trace_timeline_event(event);
            }
            (0x90, 0x94) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "GraphSys94");
            }
            (0x90, 0x96) => {
                let y = pop_int_value(stack).unwrap_or_default();
                let x = pop_int_value(stack).unwrap_or_default();
                tracing::info!(x, y, "GraphSys96");
            }
            (0x90, 0x97) => {
                let y = pop_int_value(stack).unwrap_or_default();
                let x = pop_int_value(stack).unwrap_or_default();
                tracing::info!(x, y, "GraphSys97");
            }
            (0x90, 0x9c) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "GraphSys9C");
            }
            (0x90, 0x9b) => {
                let arg2 = pop_int_value(stack).unwrap_or_default();
                let arg1 = pop_int_value(stack).unwrap_or_default();
                tracing::info!(arg1, arg2, "GraphSys9B");
            }
            (0x90, 0x95) => {
                let b = pop_int_value(stack).unwrap_or_default();
                let a = pop_int_value(stack).unwrap_or_default();
                tracing::info!(a, b, "GraphSys95");
            }
            (0x90, 0x9f) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "GraphSys9F");
            }
            (0x90, 0xaf) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "GraphSysAF");
            }
            (0x90, 0xb7) => {
                let state = stack.pop();
                let surface = stack.pop();
                tracing::info!(?surface, ?state, "GraphSurfaceBindState");
            }
            (0x90, 0xb9) => {
                let object = stack.pop();
                let object_id = object.as_ref().map(value_to_i32).unwrap_or_default();
                if object_id > 0 {
                    self.remove_graph_object(object_id);
                }
                tracing::info!(?object, "GraphObjectFinalize");
            }
            (0x90, 0xd0) => {
                let target = stack.pop();
                tracing::debug!(?target, "GraphSysD0");
            }
            (0x90, 0xd1) => {
                let target = stack.pop();
                let target_id = target.as_ref().map(value_to_i32).unwrap_or_default();
                tracing::debug!(?target, target_id, "GraphSysD1");
            }
            (0x90, 0xd4) => {
                let args = pop_args(stack, 2);
                tracing::debug!(?args, "GraphSysD4");
            }
            (0x90, 0xd5) => {
                let args = pop_args(stack, 3);
                tracing::debug!(?args, "GraphSysD5");
            }
            (0x90, 0xd6) => {
                let args = pop_args(stack, 3);
                tracing::debug!(?args, "GraphSysD6");
            }
            (0x90, 0xd8) => {
                let args = pop_args(stack, 3);
                tracing::debug!(?args, "GraphSysD8");
            }
            (0x90, 0xd9) => {
                let args = pop_args(stack, 2);
                tracing::debug!(?args, "GraphSysD9");
            }
            (0x90, 0xdd) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "GraphDriverInit");
            }
            (0x90, 0xbc) => {
                let object = stack.pop();
                let state_buffer = stack.pop();
                tracing::info!(?state_buffer, ?object, "GraphPollObjectState");
            }
            (0x90, 0xbf) => {
                let object = stack.pop();
                let event_buffer = stack.pop();
                tracing::info!(?event_buffer, ?object, "GraphPollObjectEvent");
            }
            (0x90, 0xe0) => {
                let handle = self.alloc_timeline();
                self.timelines.create(handle);
                tracing::info!(handle, "GraphCreateTimeline");
                self.trace_graph(format!("create timeline #{handle}"));
                return Ok(ethornell_vm::Value::Int(handle));
            }
            (0x90, 0xe1) => {
                let timeline = stack.pop();
                let handle = timeline.as_ref().map(value_to_i32).unwrap_or_default();
                let poll = self.timelines.poll(handle);
                tracing::info!(?timeline, ?poll, "GraphTimelinePoll");
                if self.debug_graph {
                    self.trace_graph(format!(
                        "timeline #{handle} poll active={} finished={} remaining={}",
                        poll.active, poll.finished, poll.remaining
                    ));
                }
                return Ok(ethornell_vm::Value::Int(i32::from(poll.finished)));
            }
            (0x90, 0xe4) => {
                let enabled = stack.pop();
                let timeline = stack.pop();
                let handle = timeline.as_ref().map(value_to_i32).unwrap_or_default();
                let enabled_value = enabled.as_ref().map(value_to_i32).unwrap_or_default() != 0;
                let event = self.timelines.set_enabled(handle, enabled_value);
                tracing::info!(?timeline, ?enabled, ?event, "GraphTimelineSetEnabled");
                self.trace_timeline_event(event);
            }
            (0x90, 0xe5) => {
                let args = pop_args(stack, 4);
                let event = self.timelines.configure(&args);
                tracing::info!(
                    args = ?summarize_values(&args),
                    ?event,
                    "GraphTimelineConfigure"
                );
                if let Some(event) = event {
                    self.trace_timeline_event(event);
                }
            }
            (0x90, 0xe8) => {
                let args = pop_args(stack, 4);
                let event = self.timelines.attach(&args);
                tracing::info!(args = ?summarize_values(&args), ?event, "GraphTimelineAttach");
                if let Some(event) = event {
                    self.trace_timeline_event(event);
                }
            }
            (0x90, 0xe9) => {
                let value = stack.pop();
                let timeline = stack.pop();
                let handle = timeline.as_ref().map(value_to_i32).unwrap_or_default();
                let target = value.as_ref().map(value_to_i32).unwrap_or_default();
                let result = self.timelines.query(handle, target);
                tracing::info!(?timeline, ?value, result, "GraphTimelineQuery");
                return Ok(ethornell_vm::Value::Int(result));
            }
            (0x91, 0x0d) => {
                let enabled = pop_int_value(stack).unwrap_or_default();
                tracing::info!(enabled, "GraphSetLayerEnabled");
            }
            (0x91, 0x0e) => {
                let mut args = Vec::new();
                for _ in 0..5 {
                    args.push(stack.pop());
                }
                tracing::info!(?args, "GraphConfigureLayer");
            }
            (0x91, 0x06) => {
                let args = pop_args(stack, 2);
                tracing::debug!(?args, "GraphLayerSys06");
            }
            (0x91, 0x1f) => {
                let b = stack.pop();
                let a = stack.pop();
                tracing::debug!(?a, ?b, "GraphLayerSys1F");
            }
            (0x91, 0x3e) => {
                let mode = stack.pop();
                let x = stack.pop();
                let layer = stack.pop();
                let (hit, payload) = self
                    .mouse_pos
                    .and_then(|point| {
                        self.hit_test_user_control(point)
                            .map(|control| (control.id, control.payload))
                            .or_else(|| self.hit_test_graph(point).map(|hit| (hit, hit & 0xffff)))
                    })
                    .unwrap_or_default();
                self.last_hit_control = hit;
                self.last_hit_payload = payload;
                if hit != 0 {
                    self.trace_graph(format!(
                        "layer hit-test layer={:?} x={:?} mode={:?} -> #{hit} payload={payload}",
                        layer, x, mode
                    ));
                } else {
                    tracing::debug!(?layer, ?x, ?mode, "GraphLayerHitTest");
                }
                return Ok(ethornell_vm::Value::Int(i32::from(hit != 0)));
            }
            (0x91, 0x3f) => {
                let arg2 = stack.pop();
                let arg1 = stack.pop();
                tracing::debug!(?arg1, ?arg2, "GraphLayerFlush");
            }
            (0x91, 0x38) => {
                let args = pop_args(stack, 3);
                tracing::debug!(?args, "GraphLayerSys38");
            }
            (0x91, 0x19) => {
                let args = pop_args(stack, 11);
                tracing::debug!(?args, "GraphLayerTransform");
                let mut ints: Vec<i32> = args.iter().map(value_to_i32).collect();
                if ints.len() >= 11 {
                    ints.reverse();
                    let id = ints[0];
                    let tx = fixed_16_to_f32(ints[1]);
                    let ty = fixed_16_to_f32(ints[2]);
                    let resource_id = ints[3];
                    let sx = fixed_16_to_f32(ints[7]);
                    let sy = fixed_16_to_f32(ints[8]);
                    let rotation = fixed_16_to_f32(ints[9]);
                    let extra_x = fixed_16_to_f32(ints[6]);
                    let duration_frames = self.animation_queue_remaining.max(1);
                    let mut trace = None;
                    let title_after_departure =
                        self.resolve_resource_key(resource_id).is_some_and(|key| {
                            self.title_ui_departed && key.starts_with("sysgrp.arc:SGTitle")
                        });
                    if !title_after_departure {
                        self.ensure_transform_layer(id, resource_id);
                    }
                    if let Some(layer) = self.graph_layers.get_mut(&id) {
                        let target_x = tx + extra_x;
                        let target_y = ty;
                        let scale_x = normalize_transform_scale(sx);
                        let scale_y = normalize_transform_scale(sy);
                        if duration_frames > 1 {
                            self.layer_animations.move_x_to(
                                id,
                                layer.transform_x,
                                target_x,
                                duration_frames,
                            );
                            self.layer_animations.move_y_to(
                                id,
                                layer.transform_y,
                                target_y,
                                duration_frames,
                            );
                            self.layer_animations.scale_x_to(
                                id,
                                layer.scale_x,
                                scale_x,
                                duration_frames,
                            );
                            self.layer_animations.scale_y_to(
                                id,
                                layer.scale_y,
                                scale_y,
                                duration_frames,
                            );
                        } else {
                            layer.transform_x = target_x;
                            layer.transform_y = target_y;
                            layer.scale_x = scale_x;
                            layer.scale_y = scale_y;
                        }
                        layer.rotation_degrees = rotation;
                        trace = Some(format!(
                            "layer transform #{id} tx={target_x:.1} ty={target_y:.1} scale={scale_x:.2}x{scale_y:.2} rot={rotation:.2} raw={ints:?}"
                        ));
                    } else if self.debug_graph && !title_after_departure {
                        trace = Some(format!("layer transform missing #{id} raw={ints:?}"));
                    }
                    if let Some(trace) = trace {
                        self.trace_graph(trace);
                    }
                }
            }
            (0x91, 0x98) => {
                let mut args = Vec::new();
                for _ in 0..7 {
                    args.push(stack.pop());
                }
                tracing::info!(?args, "GraphLayerSys98");
            }
            (0x92, 0x97) => {
                let mut args = Vec::new();
                for _ in 0..7 {
                    args.push(stack.pop());
                }
                tracing::info!(?args, "GraphTextSys97");
            }
            (0x92, 0x88) => {
                let value = stack.pop();
                let surface = stack.pop();
                tracing::info!(?surface, ?value, "GraphTextSys88");
            }
            (0x90, 0x0e) => {
                let mut args = Vec::new();
                for _ in 0..5 {
                    args.push(stack.pop());
                }
                tracing::info!(?args, "GraphConfigureViewport");
            }
            (0x91, 0x9a) => {
                let value = stack.pop();
                let property = stack.pop();
                tracing::info!(?property, ?value, "GraphSetLayerProperty");
            }
            (0x91, 0xb8) => {
                let layer = stack.pop();
                let value = self.last_hit_payload;
                tracing::debug!(?layer, value, "GraphLayerGetValue");
                return Ok(ethornell_vm::Value::Int(value));
            }
            (0x91, 0xba) => {
                let dest = stack.pop();
                let layer = stack.pop();
                let value = layer.as_ref().map(value_to_i32).unwrap_or_default();
                tracing::debug!(?layer, ?dest, value, "GraphLayerStoreValue");
                return Ok(ethornell_vm::Value::Int(value));
            }
            (0x91, 0x88) => {
                let args = pop_args(stack, 6);
                self.text_state = infer_text_state(&args);
                tracing::debug!(?args, inferred = ?self.text_state, "ConfigureFormatInfo");
            }
            (0x91, 0x89) => {
                let value = stack.pop();
                let target = stack.pop();
                tracing::info!(?target, ?value, "GraphLayerFormatOption");
            }
            (0x91, 0x94) => {
                let args = pop_args(stack, 2);
                self.graph94_yield_count = self.graph94_yield_count.saturating_add(1);
                if self.graph94_yield_count >= self.graph94_yield_every {
                    self.graph94_yield_count = 0;
                    self.frame_yield_requested = true;
                }
                tracing::debug!(
                    ?args,
                    every = self.graph94_yield_every,
                    count = self.graph94_yield_count,
                    yielded = self.frame_yield_requested,
                    "GraphLayerSys94"
                );
            }
            (0x91, 0x8b) => {
                let args = pop_args(stack, 2);
                tracing::info!(?args, "GraphLayerSys8B");
            }
            (0x91, 0x8c) => {
                let args = pop_args(stack, 10);
                tracing::debug!(?args, "GraphLayerSys8C");
            }
            (0x91, 0x8d) => {
                let args = pop_args(stack, 1);
                tracing::info!(args = ?summarize_values(&args), "GraphLayerSys8D");
                let value = args.first().map(value_to_i32).unwrap_or_default();
                stack.push(ethornell_vm::Value::Int(value));
                stack.push(ethornell_vm::Value::Int(value));
                return Ok(ethornell_vm::Value::Int(value));
            }
            (0x91, 0x96) => {
                let args = pop_args(stack, 1);
                tracing::debug!(?args, "GraphLayerSys96");
            }
            (0x92, 0x91) => {
                let args = pop_args(stack, 5);
                tracing::debug!(?args, "GraphTextSys91");
            }
            (0x92, 0x89) => {
                let args = pop_args(stack, 6);
                tracing::info!(?args, "GraphTextSys89");
            }
            (0x92, 0x17) => {
                let args = pop_args(stack, 3);
                if let Some(color) = infer_text_color(&args) {
                    self.text_state.color = color;
                }
                tracing::debug!(
                    args = ?summarize_values(&args),
                    color = ?self.text_state.color,
                    "GraphTextSetColorState"
                );
            }
            (0x92, 0x9c) => {
                let args = pop_args(stack, 6);
                let color = self.text_state.color;
                let text = args.iter().rev().find_map(|value| match value {
                    ethornell_vm::Value::Str(text) => Some(text.as_str()),
                    _ => None,
                });
                self.render_graph_text(&args);
                if let Some(text) = text {
                    tracing::info!(%text, state = ?self.text_state, ?color, ?args, "RenderText");
                } else {
                    tracing::debug!(state = ?self.text_state, ?color, ?args, "RenderText without inline string");
                }
            }
            (0x90, 0x31) => {
                let args = pop_args(stack, 5);
                tracing::info!(?args, "GraphSys31");
            }
            (0x90, 0x33) => {
                let args = pop_args(stack, 3);
                if args.len() >= 3 {
                    let surface_id = value_to_i32(&args[2]);
                    let y = value_to_i32(&args[0]) as f32;
                    let x = value_to_i32(&args[1]) as f32;
                    let trace = if let Some(surface) = self.graph_surfaces.get_mut(&surface_id) {
                        surface.x = x;
                        surface.y = y;
                        Some(format!("surface #{} position x={} y={}", surface.id, x, y))
                    } else {
                        None
                    };
                    if let Some(trace) = trace {
                        self.trace_graph(trace);
                    }
                }
                tracing::info!(?args, "GraphSys33");
            }
            (0x90, 0x34) => {
                let args = pop_args(stack, 2);
                tracing::info!(?args, "GraphSys34");
            }
            (0x90, 0x37) => {
                let args = pop_args(stack, 3);
                tracing::info!(?args, "GraphSys37");
            }
            (0x90, 0x38) => {
                let args = pop_args(stack, 4);
                tracing::info!(?args, "GraphSys38");
            }
            (0x90, 0x39) => {
                let args = pop_args(stack, 2);
                if args.len() >= 2 {
                    let surface_id = value_to_i32(&args[1]);
                    let visible = value_to_i32(&args[0]) != 0;
                    let trace = if let Some(surface) = self.graph_surfaces.get_mut(&surface_id) {
                        surface.enabled = visible;
                        Some(format!("surface #{} sys39 enabled={visible}", surface.id))
                    } else {
                        None
                    };
                    if let Some(trace) = trace {
                        self.trace_graph(trace);
                    }
                }
                tracing::info!(?args, "GraphSys39");
            }
            (0x90, 0x3a) => {
                let args = pop_args(stack, 2);
                if args.len() >= 2 {
                    let resource = value_to_i32(&args[0]);
                    let target = value_to_i32(&args[1]);
                    let key = self.resolve_resource_key(resource).map(str::to_string);
                    let mut traces = Vec::new();
                    if let Some(layer) = self.graph_layers.get_mut(&target) {
                        if let Some(key) = key.as_deref() {
                            layer.key = key.to_string();
                        }
                        layer.enabled = true;
                        traces.push(format!(
                            "layer #{target} set node resource={resource} key={:?}",
                            key
                        ));
                    }
                    if let Some(surface) = self.graph_surfaces.get_mut(&target) {
                        if resource > 0 {
                            surface.resource_id = Some(resource);
                        }
                        traces.push(format!(
                            "surface #{target} set node resource={resource} key={:?}",
                            key
                        ));
                    }
                    for trace in traces {
                        self.trace_graph(trace);
                    }
                }
                tracing::info!(args = ?summarize_values(&args), "GraphSetNodeResource");
            }
            (0x90, 0x3c) => {
                let args = pop_args(stack, 2);
                tracing::debug!(?args, "GraphSys3C");
            }
            (0x90, 0x43) => {
                let args = pop_args(stack, 10);
                self.apply_graph_rect_transition(&args);
                tracing::info!(
                    args = ?summarize_values(&args),
                    "GraphRectTransition"
                );
            }
            (0x90, 0x57) => {
                let args = pop_args(stack, 2);
                tracing::info!(?args, "GraphSys57");
            }
            (0x90, 0x58) => {
                let args = pop_args(stack, 9);
                let inferred = infer_text_state(&args);
                if inferred.width > 0.0 && inferred.height > 0.0 {
                    self.text_state = inferred.clone();
                    if let Some(node_id) = args.iter().find_map(|value| match value {
                        ethornell_vm::Value::Int(v) if self.text_nodes.contains_key(v) => Some(*v),
                        _ => None,
                    }) {
                        if let Some(node) = self.text_nodes.get_mut(&node_id) {
                            node.x = inferred.x;
                            node.y = inferred.y;
                            node.size = inferred.font_size.max(18.0);
                            node.color = inferred.color;
                            node.z = node.z.max(950);
                            node.text = wrap_text_to_state(&node.text, &inferred);
                        }
                    }
                }
                tracing::info!(
                    args = ?summarize_values(&args),
                    inferred = ?self.text_state,
                    "GraphNodeConfigureLayout"
                );
            }
            (0x90, 0x82) => {
                let args = pop_args(stack, 2);
                if args.len() >= 2 {
                    let surface_id = value_to_i32(&args[1]);
                    let visible = value_to_i32(&args[0]) != 0;
                    let trace = if let Some(surface) = self.graph_surfaces.get_mut(&surface_id) {
                        surface.enabled = visible;
                        Some(format!("surface #{} sys82 enabled={visible}", surface.id))
                    } else {
                        None
                    };
                    if let Some(trace) = trace {
                        self.trace_graph(trace);
                    }
                }
                tracing::info!(?args, "GraphSys82");
            }
            (0x90, 0x19) => {
                let args = pop_args(stack, 2);
                let ints: Vec<i32> = args.iter().map(value_to_i32).collect();
                if ints.len() >= 2 {
                    let target = ints[1];
                    let value = ints[0];
                    let mut traces = Vec::new();
                    if let Some(surface) = self.graph_surfaces.get_mut(&target) {
                        surface.enabled = value != 0;
                        traces.push(format!(
                            "surface #{} sys19 enabled={}",
                            surface.id, surface.enabled
                        ));
                    }
                    if let Some(layer) = self.graph_layers.get_mut(&target) {
                        layer.enabled = value != 0;
                        traces.push(format!("layer #{} sys19 enabled={}", target, layer.enabled));
                    }
                    for trace in traces {
                        self.trace_graph(trace);
                    }
                }
                tracing::info!(?args, "GraphSys19");
            }
            (0x90, 0x22) => {
                let args = pop_args(stack, 7);
                self.apply_graph_node_transition(&args);
                tracing::info!(
                    args = ?summarize_values(&args),
                    "GraphNodeApplyTransition"
                );
            }
            (0x90, 0x98) => {
                let args = pop_args(stack, 2);
                tracing::info!(?args, "GraphSys98");
            }
            (0x90, 0x99) => {
                let args = pop_args(stack, 1);
                tracing::info!(?args, "GraphSys99");
            }
            (0x90, 0x9a) => {
                let args = pop_args(stack, 3);
                tracing::info!(?args, "GraphSys9A");
            }
            (0x90, 0x9d) => {
                let args = pop_args(stack, 3);
                tracing::info!(?args, "GraphSys9D");
            }
            _ => {
                let consumed_args = consume_fallback_args(group, id, stack);
                tracing::warn!(
                    group = format_args!("0x{group:02X}"),
                    id = format_args!("0x{id:02X}"),
                    args = ?summarize_values(&consumed_args),
                    stack_len = stack.len(),
                    stack_top = ?stack_top_summary(stack, 8),
                    "runtime graphcall stub"
                );
            }
        }
        Ok(ethornell_vm::Value::None)
    }

    fn poll_object_state(&mut self, object: i32) -> i32 {
        let suppress_overlay_input = self.scenario_overlay_active()
            && !self.title_ui_active
            && !self.has_active_message_window();
        if suppress_overlay_input {
            if self.debug_graph {
                self.trace_graph(format!(
                    "poll object state #{object} suppressed during scenario overlay without message window"
                ));
            }
            return 0;
        }
        let hit = self.mouse_pos.and_then(|point| {
            (!suppress_overlay_input)
                .then(|| self.hit_test_user_control_for_object(point, object))
                .flatten()
                .map(|control| control.id)
                .or_else(|| {
                    self.hit_test_graph(point)
                        .filter(|hit| object == 0 || object == *hit)
                })
        });
        let pending_hit = self.pending_object_state.and_then(|point| {
            (!suppress_overlay_input)
                .then(|| self.hit_test_user_control_for_object(point, object))
                .flatten()
                .map(|control| control.id)
                .or_else(|| {
                    self.hit_test_graph(point)
                        .filter(|hit| object == 0 || object == *hit)
                })
        });
        let state_hit = if self.mouse_pressed { hit } else { pending_hit };
        let state = if let Some(hit) = state_hit {
            self.last_hit_control = hit;
            self.last_hit_payload = self
                .mouse_pos
                .and_then(|point| {
                    (!suppress_overlay_input)
                        .then(|| self.hit_test_user_control_for_object(point, object))
                        .flatten()
                        .map(|control| control.payload)
                })
                .unwrap_or(hit & 0xffff);
            0x1000_0002
        } else {
            0
        };
        if state != 0 || self.debug_graph {
            let point = self
                .mouse_pos
                .map(|(x, y)| format!("({x:.0},{y:.0})"))
                .unwrap_or_else(|| "none".to_string());
            self.trace_graph(format!(
                "poll object state #{object} point={point} pressed={} pending_hit={} hit={} payload=0x{:04X} -> 0x{state:08X}",
                self.mouse_pressed,
                pending_hit.unwrap_or_default(),
                hit.unwrap_or_default(),
                self.last_hit_payload,
            ));
        }
        if state != 0 && pending_hit.is_some() {
            self.pending_object_state = None;
            self.mouse_pressed = false;
            self.auto_title_release_after_state = false;
            self.trace_graph("queued object state consumed");
        }
        if state != 0 && self.auto_title_release_after_state && self.pending_click.is_none() {
            self.mouse_pressed = false;
            self.auto_title_release_after_state = false;
            self.trace_graph("auto title click released after state poll");
        }
        state
    }

    fn poll_object_event(&mut self, object: i32) -> i32 {
        self.poll_object_event_payload(object).0
    }

    fn poll_object_event_payload(&mut self, object: i32) -> (i32, i32) {
        let suppress_overlay_input = self.scenario_overlay_active()
            && !self.title_ui_active
            && !self.has_active_message_window();
        if suppress_overlay_input {
            if self.debug_graph {
                self.trace_graph(format!(
                    "poll object event #{object} suppressed during scenario overlay without message window"
                ));
            }
            return (0, 0);
        }
        let hit = self.pending_click.and_then(|point| {
            (!suppress_overlay_input)
                .then(|| self.hit_test_user_control_for_object(point, object))
                .flatten()
                .map(|control| (control.id, control.payload, control.title_only))
                .or_else(|| {
                    self.hit_test_graph(point)
                        .filter(|hit| object == 0 || object == *hit)
                        .map(|hit| (hit, hit & 0xffff, false))
                })
        });
        let (event, payload) = if let Some((hit, payload, title_only)) = hit {
            let payload = if title_only {
                self.pending_title_payload_override
                    .take()
                    .unwrap_or(payload)
            } else {
                payload
            };
            let event_code = if title_only && self.auto_title_release_after_state {
                0x1000_0006
            } else {
                self.mouse_event_code
            };
            if title_only {
                self.consume_title_control_event(hit, payload, "poll");
            } else {
                self.last_hit_control = hit;
                self.last_hit_payload = payload;
                self.pending_click = None;
            }
            if title_only && event_code == 0x1000_0006 && self.auto_title_release_after_state {
                self.mouse_pressed = false;
                self.auto_title_release_after_state = false;
                self.trace_graph("auto title click released after mouse event poll");
            }
            (event_code, pack_object_event_payload(hit, payload))
        } else {
            (0, 0)
        };
        if event != 0 {
            tracing::info!(
                object,
                hit = self.last_hit_control,
                event = format_args!("0x{event:08X}"),
                payload = format_args!("0x{payload:08X}"),
                "poll object event"
            );
            self.trace_graph(format!(
                "poll object event #{object} hit=#{} -> 0x{event:08X} payload=0x{payload:08X}",
                self.last_hit_control,
            ));
        }
        (event, payload)
    }
}

impl ethornell_vm::SoundApi for RuntimeTraceApi {
    fn observe_user(&mut self, group: u8, id: u16, stack: &[ethornell_vm::Value]) {
        self.observe_user_control(group, id, stack);
        self.observe_user_message(group, id, stack);
    }

    fn call_user(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> ethornell_vm::VmResult<Option<ethornell_vm::Value>> {
        self.call_user_control(group, id, stack)
    }

    fn call_sound(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> ethornell_vm::VmResult<ethornell_vm::Value> {
        self.record_call(group, id);
        if matches!((group, id), (0xa0, 0x11)) {
            let fade = stack.pop();
            let volume = stack.pop();
            let file = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
            let archive = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
            let channel = stack.pop();
            let resource = find_runtime_resource(&self.manager, &archive, &file);
            let key = (archive.clone(), file.clone());
            let already_playing = self.current_bgm.as_ref() == Some(&key);
            if !already_playing {
                self.current_bgm = Some(key);
            }
            if !already_playing {
                if let Some(entry) = &resource {
                    match self.manager.read_by_entry_decoded(entry) {
                        Ok(bytes) => self.audio_requests.push_back(AudioRequest {
                            archive: archive.clone(),
                            file: file.clone(),
                            bytes,
                        }),
                        Err(err) => {
                            tracing::warn!(archive, file, %err, "SoundPlayBgm read failed")
                        }
                    }
                }
            }
            tracing::info!(
                ?channel,
                archive,
                file,
                ?volume,
                ?fade,
                found = resource.is_some(),
                already_playing,
                "SoundPlayBgm"
            );
            return Ok(ethornell_vm::Value::None);
        }
        if matches!((group, id), (0xa0, 0x14)) {
            let arg2 = stack.pop();
            let arg1 = stack.pop();
            tracing::info!(?arg1, ?arg2, "SoundControlBgm");
            return Ok(ethornell_vm::Value::None);
        }
        if matches!((group, id), (0xa0, 0x16)) {
            let duration = stack.pop();
            let volume = stack.pop();
            let channel = stack.pop();
            tracing::debug!(?channel, ?volume, ?duration, "SoundFadeVolume");
            return Ok(ethornell_vm::Value::None);
        }
        if matches!((group, id), (0xa0, 0x20)) {
            let file = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
            let archive = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
            let slot = pop_int_value(stack).unwrap_or_default();
            let resource = find_runtime_resource(&self.manager, &archive, &file);
            if let Some(entry) = &resource {
                match self.manager.read_by_entry_decoded(entry) {
                    Ok(bytes) => {
                        self.sound_slots.insert(
                            slot,
                            AudioRequest {
                                archive: archive.clone(),
                                file: file.clone(),
                                bytes,
                            },
                        );
                    }
                    Err(err) => tracing::warn!(archive, file, %err, "SoundLoadSlot read failed"),
                }
            }
            tracing::info!(
                slot,
                archive,
                file,
                found = resource.is_some(),
                "SoundLoadSlot"
            );
            return Ok(ethornell_vm::Value::None);
        }
        if matches!((group, id), (0xa0, 0x21)) {
            let arg6 = stack.pop();
            let arg5 = stack.pop();
            let arg4 = stack.pop();
            let file = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
            let archive = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
            let slot = pop_int_value(stack).unwrap_or_default();
            let resource = find_runtime_resource(&self.manager, &archive, &file);
            if let Some(entry) = &resource {
                match self.manager.read_by_entry_decoded(entry) {
                    Ok(bytes) => {
                        self.sound_slots.insert(
                            slot,
                            AudioRequest {
                                archive: archive.clone(),
                                file: file.clone(),
                                bytes,
                            },
                        );
                    }
                    Err(err) => tracing::warn!(archive, file, %err, "SoundLoadSlotEx read failed"),
                }
            }
            tracing::info!(
                slot,
                archive,
                file,
                ?arg4,
                ?arg5,
                ?arg6,
                found = resource.is_some(),
                "SoundLoadSlotEx"
            );
            return Ok(ethornell_vm::Value::None);
        }
        if matches!((group, id), (0xa0, 0x22)) {
            let channel = stack.pop();
            tracing::debug!(?channel, "SoundChannelQuery");
            return Ok(ethornell_vm::Value::None);
        }
        if matches!((group, id), (0xa0, 0x24)) {
            let fade = stack.pop();
            let volume = stack.pop();
            let slot = pop_int_value(stack).unwrap_or_default();
            let found = self.sound_slots.get(&slot).cloned();
            let found_slot = found.is_some();
            if let Some(request) = found {
                self.audio_requests.push_back(request);
            }
            tracing::info!(slot, ?volume, ?fade, found = found_slot, "SoundPlaySlot");
            return Ok(ethornell_vm::Value::Int(i32::from(found_slot)));
        }
        if matches!((group, id), (0xa0, 0x25)) {
            let channel = stack.pop();
            tracing::debug!(?channel, "SoundChannelStopOrQuery");
            return Ok(ethornell_vm::Value::None);
        }
        if matches!((group, id), (0xa0, 0x26)) {
            let arg2 = stack.pop();
            let arg1 = stack.pop();
            tracing::debug!(?arg1, ?arg2, "SoundSlotRelease");
            return Ok(ethornell_vm::Value::None);
        }
        if matches!((group, id), (0xa0, 0x08) | (0xa0, 0x09)) {
            let value = stack.pop();
            let channel = stack.pop();
            tracing::info!(
                ?channel,
                ?value,
                id = format_args!("0x{id:02X}"),
                "SoundSys"
            );
            return Ok(ethornell_vm::Value::None);
        }
        if matches!((group, id), (0xa0, 0x19)) {
            let duration = stack.pop();
            let channel = stack.pop();
            tracing::info!(?channel, ?duration, "SoundFadeOrStop");
            return Ok(ethornell_vm::Value::None);
        }
        let consumed_args = consume_fallback_args(group, id, stack);
        tracing::warn!(
            group = format_args!("0x{group:02X}"),
            id = format_args!("0x{id:02X}"),
            args = ?summarize_values(&consumed_args),
            stack_len = stack.len(),
            stack_top = ?stack_top_summary(stack, 8),
            "runtime sndcall stub"
        );
        Ok(ethornell_vm::Value::None)
    }
}

fn pop_int_value(stack: &mut Vec<ethornell_vm::Value>) -> Option<i32> {
    match stack.pop()? {
        ethornell_vm::Value::Int(value) => Some(value),
        ethornell_vm::Value::Ptr(value) => Some(value as i32),
        ethornell_vm::Value::Func { offset, .. } => Some(offset as i32),
        ethornell_vm::Value::Str(_)
        | ethornell_vm::Value::Program(_)
        | ethornell_vm::Value::None => Some(0),
    }
}

fn pop_string_value(stack: &mut Vec<ethornell_vm::Value>) -> Option<String> {
    match stack.pop()? {
        ethornell_vm::Value::Str(text) => Some(text),
        ethornell_vm::Value::Int(value) => Some(format!("0x{value:08X}")),
        ethornell_vm::Value::Ptr(value) => Some(format!("0x{value:08X}")),
        ethornell_vm::Value::Func { offset, .. } => Some(format!("0x{offset:08X}")),
        ethornell_vm::Value::Program(_) | ethornell_vm::Value::None => Some(String::new()),
    }
}

fn pop_args(stack: &mut Vec<ethornell_vm::Value>, count: usize) -> Vec<ethornell_vm::Value> {
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        if let Some(value) = stack.pop() {
            args.push(value);
        }
    }
    args
}

fn consume_fallback_args(
    group: u8,
    id: u16,
    stack: &mut Vec<ethornell_vm::Value>,
) -> Vec<ethornell_vm::Value> {
    known_call_arg_count(group, id)
        .map(|count| pop_args(stack, count))
        .unwrap_or_default()
}

fn pack_object_event_payload(hit: i32, payload: i32) -> i32 {
    ((hit & 0xffff) << 16) | (payload & 0xffff)
}

fn stack_top_summary(stack: &[ethornell_vm::Value], count: usize) -> Vec<String> {
    stack.iter().rev().take(count).map(value_summary).collect()
}

fn summarize_values(values: &[ethornell_vm::Value]) -> Vec<String> {
    values.iter().map(value_summary).collect()
}

fn value_summary(value: &ethornell_vm::Value) -> String {
    match value {
        ethornell_vm::Value::Int(value) => format!("Int({value})"),
        ethornell_vm::Value::Ptr(value) => format!("Ptr(0x{value:08X})"),
        ethornell_vm::Value::Func {
            program_index,
            offset,
        } => {
            format!("Func(program={program_index}, offset=0x{offset:08X})")
        }
        ethornell_vm::Value::Str(text) => {
            let preview: String = text.chars().take(32).collect();
            format!("Str({preview:?})")
        }
        ethornell_vm::Value::Program(program) => {
            let name = program.script_name.as_deref().unwrap_or("<anonymous>");
            format!("Program({name})")
        }
        ethornell_vm::Value::None => "None".to_string(),
    }
}

fn value_to_i32(value: &ethornell_vm::Value) -> i32 {
    match value {
        ethornell_vm::Value::Int(value) => *value,
        ethornell_vm::Value::Ptr(value) => *value as i32,
        ethornell_vm::Value::Func { offset, .. } => *offset as i32,
        ethornell_vm::Value::Str(_)
        | ethornell_vm::Value::Program(_)
        | ethornell_vm::Value::None => 0,
    }
}

fn normalize_transform_scale(value: f32) -> f32 {
    if value <= 0.0 || !value.is_finite() {
        1.0
    } else {
        value.clamp(0.001, 8.0)
    }
}

fn value_to_string(value: &ethornell_vm::Value) -> Option<String> {
    match value {
        ethornell_vm::Value::Str(text) => Some(text.clone()),
        ethornell_vm::Value::Int(value) => Some(format!("0x{value:08X}")),
        ethornell_vm::Value::Ptr(value) => Some(format!("0x{value:08X}")),
        ethornell_vm::Value::Func { offset, .. } => Some(format!("0x{offset:08X}")),
        ethornell_vm::Value::Program(_) | ethornell_vm::Value::None => None,
    }
}

fn is_title_layer(key: &str) -> bool {
    key.starts_with("sysgrp.arc:SGTitle")
}

fn is_title_child_resource_name(name: &str) -> bool {
    name.starts_with("SGCnf") || name.starts_with("SGUsSave") || name.starts_with("SGUsLoad")
}

fn is_title_child_layer_key(key: &str) -> bool {
    key.starts_with("sysgrp.arc:SGCnf")
        || key.starts_with("sysgrp.arc:SGUsSave")
        || key.starts_with("sysgrp.arc:SGUsLoad")
}

fn is_title_base_layer(key: &str) -> bool {
    matches!(
        key,
        "sysgrp.arc:SGTitle990000"
            | "sysgrp.arc:SGTitle990200"
            | "sysgrp.arc:SGTitle990300"
            | "sysgrp.arc:SGTitle000000"
    )
}

fn is_boot_suppressed_layer(key: &str) -> bool {
    key.starts_with("sysgrp.arc:SGMsgWnd") || key == "sysgrp.arc:caret"
}

fn is_message_control_layer(key: &str) -> bool {
    key.starts_with("sysgrp.arc:SGMsgWnd000")
}

fn is_primary_title_control(control: &RuntimeUserControl) -> bool {
    control.title_only && control.owner_id == control.id
}

fn is_default_hidden_object_state(key: &str) -> bool {
    key.starts_with("sysgrp.arc:SGMsgWnd000001")
        || key.starts_with("sysgrp.arc:SGMsgWnd000002")
        || key.starts_with("sysgrp.arc:SGMsgWnd000003")
}

fn find_runtime_resource(
    manager: &ResourceManager,
    archive: &str,
    file: &str,
) -> Option<ethornell_archive::ResourceEntry> {
    let archive = archive.trim();
    if is_empty_archive_arg(archive) {
        manager.find(file)
    } else {
        manager
            .find_in_archive(archive, file)
            .or_else(|| find_wildcard_archive_resource(manager, archive, file))
    }
}

fn find_wildcard_archive_resource(
    manager: &ResourceManager,
    archive: &str,
    file: &str,
) -> Option<ethornell_archive::ResourceEntry> {
    if !archive.contains("xxx") {
        return None;
    }
    let (prefix, suffix) = archive.split_once("xxx")?;
    manager.list().into_iter().find(|entry| {
        entry.entry_name.eq_ignore_ascii_case(file)
            && entry
                .archive_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| {
                    let name = name.to_ascii_lowercase();
                    name.starts_with(&prefix.to_ascii_lowercase())
                        && name.ends_with(&suffix.to_ascii_lowercase())
                })
                .unwrap_or(false)
    })
}

fn find_runtime_file(manager: &ResourceManager, archive: &str, file: &str) -> Option<PathBuf> {
    if !is_empty_archive_arg(archive.trim()) {
        return None;
    }
    runtime_file_path(manager, file)
}

fn runtime_file_path(manager: &ResourceManager, file: &str) -> Option<PathBuf> {
    let normalized = file.replace('\\', std::path::MAIN_SEPARATOR_STR);
    if normalized.trim().is_empty() {
        return None;
    }
    let path = Path::new(&normalized);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        Some(manager.archives().root.join(path))
    }
}

fn runtime_file_cache_key(archive: &str, file: &str) -> String {
    format!(
        "{}\0{}",
        archive.trim().to_ascii_lowercase(),
        file.replace('\\', "/").to_ascii_lowercase()
    )
}

fn read_runtime_bytes(manager: &ResourceManager, archive: &str, file: &str) -> Option<Vec<u8>> {
    if let Some(path) = find_runtime_file(manager, archive, file) {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(bytes);
        }
    }
    if let Some(bytes) = synthetic_user_data_bytes(archive, file) {
        return Some(bytes);
    }
    let entry = find_runtime_resource(manager, archive, file)?;
    manager.read_by_entry_decoded(&entry).ok()
}

fn synthetic_user_data_bytes(archive: &str, file: &str) -> Option<Vec<u8>> {
    if !is_empty_archive_arg(archive.trim()) {
        return None;
    }
    let normalized = file.replace('\\', "/").to_ascii_lowercase();
    if !normalized.starts_with("userdata/")
        || !normalized.contains("tayutama2")
        || !normalized.ends_with(".sud")
    {
        return None;
    }
    const HEADER_SIZE: usize = 324;
    const PAYLOAD_SIZE: u32 = 4;
    let mut bytes = vec![0; HEADER_SIZE + PAYLOAD_SIZE as usize + 4];
    bytes[236..240].copy_from_slice(&65_536u32.to_le_bytes());
    bytes[240..244].copy_from_slice(&PAYLOAD_SIZE.to_le_bytes());
    bytes[320..324].copy_from_slice(&PAYLOAD_SIZE.to_le_bytes());
    Some(bytes)
}

fn is_empty_archive_arg(archive: &str) -> bool {
    archive.is_empty() || archive == "0" || archive == "0x00000000"
}

fn infer_text_state(args: &[ethornell_vm::Value]) -> TextState {
    let mut state = TextState::default();
    let ints: Vec<i32> = args
        .iter()
        .filter_map(|value| match value {
            ethornell_vm::Value::Int(v) => Some(*v),
            ethornell_vm::Value::Ptr(v) => Some(*v as i32),
            _ => None,
        })
        .collect();
    if let Some(size) = ints.iter().rev().copied().find(|v| (8..=96).contains(v)) {
        state.font_size = size as f32;
        state.line_height = state.font_size + 6.0;
    }
    if let Some((x, y, width, height)) = infer_text_rect(&ints) {
        state.x = x as f32;
        state.y = y as f32;
        state.width = width as f32;
        state.height = height as f32;
    }
    state
}

fn infer_text_color(args: &[ethornell_vm::Value]) -> Option<[f32; 4]> {
    let ints: Vec<i32> = args
        .iter()
        .filter_map(|value| match value {
            ethornell_vm::Value::Int(v) => Some(*v),
            ethornell_vm::Value::Ptr(v) => Some(*v as i32),
            _ => None,
        })
        .collect();
    for value in ints.iter().rev().copied() {
        if (0x010000..=0xFF_FFFF).contains(&value) {
            let r = ((value >> 16) & 0xff) as f32 / 255.0;
            let g = ((value >> 8) & 0xff) as f32 / 255.0;
            let b = (value & 0xff) as f32 / 255.0;
            return Some([r, g, b, 1.0]);
        }
    }
    for window in ints.windows(3).rev() {
        let [r, g, b] = *window else {
            continue;
        };
        if (0..=255).contains(&r) && (0..=255).contains(&g) && (0..=255).contains(&b) {
            return Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]);
        }
    }
    None
}

fn infer_text_rect(ints: &[i32]) -> Option<(i32, i32, i32, i32)> {
    for window in ints.windows(4).rev() {
        let [x, y, width, height] = *window else {
            continue;
        };
        if (0..=1279).contains(&x)
            && (0..=719).contains(&y)
            && (120..=1280).contains(&width)
            && (24..=720).contains(&height)
        {
            return Some((x, y, width, height));
        }
    }
    None
}

pub fn view_resource_image(game_root: GameRoot, name: String) -> Result<()> {
    let manager = ResourceManager::open_game(game_root.path())?;
    let bytes = manager.read_decoded(&name)?;
    let image = decode_image(&bytes)?;
    run_window(
        &format!("ethornell-rs: {name}"),
        Some(image),
        Some(name),
        None,
        None,
    )
}

pub fn run_text_test(text: String) -> Result<()> {
    run_window("ethornell-rs text-test", None, Some(text), None, None)
}

fn load_runtime_image(manager: &ResourceManager) -> Result<DecodedImage> {
    for name in [
        "SGTitle990000",
        "SGTitle990200",
        "SGTitle990300",
        "SGTitle000000",
    ] {
        if let Ok(bytes) = manager.read_decoded_from_archive("sysgrp.arc", name) {
            match decode_image(&bytes) {
                Ok(image) => {
                    tracing::info!(
                        resource = name,
                        width = image.width,
                        height = image.height,
                        "selected title image resource"
                    );
                    return Ok(image);
                }
                Err(err) => tracing::debug!(resource = name, %err, "title image decode failed"),
            }
        }
    }
    if let Ok(bytes) = manager.read_decoded("BG01D_a") {
        if detect_magic(&bytes) == MagicKind::CompressedBg {
            return decode_image(&bytes);
        }
    }
    for entry in manager.list() {
        let Ok(bytes) = manager.read_by_entry_decoded(&entry) else {
            continue;
        };
        if detect_magic(&bytes) != MagicKind::CompressedBg {
            continue;
        }
        match decode_image(&bytes) {
            Ok(image) => {
                tracing::info!(
                    archive = %entry.archive_path.display(),
                    name = %entry.entry_name,
                    "selected first decodable CBG resource"
                );
                return Ok(image);
            }
            Err(err) => tracing::debug!(name = %entry.entry_name, %err, "CBG decode failed"),
        }
    }
    Err(EthornellError::UnsupportedFormat(
        "no decodable CompressedBG resource found".into(),
    ))
}

fn load_runtime_bgm(manager: &ResourceManager) -> Result<Vec<u8>> {
    let entry = find_runtime_resource(manager, "data05xxx.arc", "bgm01")
        .ok_or_else(|| EthornellError::Parse("title BGM bgm01 not found".into()))?;
    let bytes = manager.read_by_entry_decoded(&entry)?;
    tracing::info!(
        archive = %entry.archive_path.display(),
        name = %entry.entry_name,
        bytes = bytes.len(),
        "selected title BGM resource"
    );
    Ok(bytes)
}

fn run_headless(
    title: &str,
    image: Option<DecodedImage>,
    text: Option<String>,
    bgm: Option<Vec<u8>>,
    mut runtime: Option<RuntimeEngine>,
) -> Result<()> {
    tracing::info!(title, "GUI headless runtime started");
    let mut audio = match AudioSystem::new() {
        Ok(audio) => Some(audio),
        Err(err) => {
            tracing::warn!(%err, "headless audio init failed");
            None
        }
    };
    if runtime.is_none() {
        if let (Some(audio), Some(bgm)) = (audio.as_mut(), bgm.as_ref()) {
            match audio.play_from_bytes(bgm) {
                Ok(()) => tracing::info!("headless title BGM playback started"),
                Err(err) => tracing::warn!(%err, "headless title BGM playback failed"),
            }
        }
    }
    if let Some(image) = image.as_ref() {
        tracing::info!(
            width = image.width,
            height = image.height,
            "headless fallback image ready"
        );
    }
    if let Some(text) = text.as_ref() {
        tracing::info!(%text, "headless text overlay requested");
    }

    let vm_slices_per_frame = std::env::var("ETHORNELL_VM_SLICES_PER_FRAME")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(8);
    let bootstrap_vm_slices_per_frame = std::env::var("ETHORNELL_VM_BOOTSTRAP_SLICES_PER_FRAME")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(16);
    let continue_after_yield = std::env::var("ETHORNELL_VM_CONTINUE_AFTER_YIELD")
        .ok()
        .map(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
        .unwrap_or(false);
    let realtime = std::env::var("ETHORNELL_HEADLESS_REALTIME")
        .ok()
        .map(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
        .unwrap_or(true);
    let max_frames = std::env::var("ETHORNELL_HEADLESS_FRAMES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(600);
    tracing::info!(
        vm_slices_per_frame,
        bootstrap_vm_slices_per_frame,
        continue_after_yield,
        realtime,
        max_frames,
        "headless frame pacing configured"
    );
    let mut input_script = HeadlessInputScript::from_env();
    if input_script.is_some() {
        tracing::info!("headless input script loaded");
    }

    for frame in 0..max_frames {
        if let Some(runtime) = runtime.as_mut() {
            if let Some(script) = input_script.as_mut() {
                if let Some(event) = script.tick() {
                    apply_headless_input_event(&mut runtime.api, event);
                }
            }
            let mut last_report = None;
            let mut total_steps = 0usize;
            let mut last_continue_setup_yield = false;
            let mut last_continue_input_yield = false;
            let slices_this_frame = if runtime.api.should_continue_vm_after_yield() {
                bootstrap_vm_slices_per_frame
            } else {
                vm_slices_per_frame
            };
            for _ in 0..slices_this_frame {
                let report = runtime.tick();
                total_steps += report.steps;
                let yielded = report.stop_reason == ethornell_vm::VmStopReason::WaitingForAnimation;
                let continue_setup_yield = runtime.api.should_continue_vm_after_yield();
                let continue_input_yield = runtime.api.should_continue_input_yield();
                last_continue_setup_yield = continue_setup_yield;
                last_continue_input_yield = continue_input_yield;
                last_report = Some(report);
                if yielded
                    && !continue_after_yield
                    && !continue_setup_yield
                    && !continue_input_yield
                {
                    break;
                }
                if !yielded {
                    break;
                }
            }
            if runtime.api.debug_graph || frame % 60 == 0 {
                let report = last_report.as_ref();
                tracing::info!(
                    frame,
                    steps = total_steps,
                    program = report.map(|report| report.program.as_str()),
                    pc = report.map(|report| report.pc),
                    offset = ?report.and_then(|report| report.offset),
                    reason = ?report.map(|report| &report.stop_reason),
                    slices_this_frame,
                    continue_setup_yield = last_continue_setup_yield,
                    continue_input_yield = last_continue_input_yield,
                    yield_blockers = ?runtime.api.vm_after_yield_blockers(),
                    stack = runtime.vm.stack.len(),
                    draw_items = runtime.api.graph_draw_items().len(),
                    text_nodes = runtime.api.text_nodes.len(),
                    graph_images = runtime.api.graph_images.len(),
                    animations = runtime.api.layer_animations.active_count(),
                    animation_queue_remaining = runtime.api.animation_queue_remaining,
                    audio_pending = runtime.api.audio_requests.len(),
                    "headless frame tick"
                );
                runtime.api.trace_render_snapshot(frame, report);
                if let Some(report) = report {
                    if report.stop_reason == ethornell_vm::VmStopReason::Error {
                        tracing::warn!(
                            recent_trace = ?report.recent_trace,
                            "headless runtime VM halted with error"
                        );
                    }
                }
            }
            runtime.api.finish_frame_input();
            while let Some(request) = runtime.api.audio_requests.pop_front() {
                if let Some(audio) = audio.as_mut() {
                    match audio.play_from_bytes(&request.bytes) {
                        Ok(()) => tracing::info!(
                            archive = request.archive,
                            file = request.file,
                            "headless script audio playback started"
                        ),
                        Err(err) => tracing::warn!(
                            archive = request.archive,
                            file = request.file,
                            %err,
                            "headless script audio playback failed"
                        ),
                    }
                }
            }
            if runtime.vm.halted {
                tracing::info!(
                    frame,
                    pc = last_report.as_ref().map(|report| report.pc),
                    offset = ?last_report.as_ref().and_then(|report| report.offset),
                    reason = ?last_report.as_ref().map(|report| &report.stop_reason),
                    recent_trace = ?last_report.as_ref().map(|report| &report.recent_trace),
                    "headless VM halted"
                );
                break;
            }
        }
        if realtime {
            std::thread::sleep(Duration::from_millis(16));
        }
    }
    if let (Some(path), Some(runtime)) = (
        std::env::var_os("ETHORNELL_HEADLESS_SNAPSHOT").map(PathBuf::from),
        runtime.as_ref(),
    ) {
        snapshot::write_runtime_snapshot(&runtime.api, &path)?;
        tracing::info!(path = %path.display(), "headless snapshot written");
    }
    if let Some(runtime) = runtime.as_ref() {
        runtime.api.write_call_coverage_if_requested();
    }
    tracing::info!("GUI headless runtime finished");
    Ok(())
}

fn run_window(
    title: &str,
    image: Option<DecodedImage>,
    text: Option<String>,
    bgm: Option<Vec<u8>>,
    mut runtime: Option<RuntimeEngine>,
) -> Result<()> {
    let event_loop =
        EventLoop::new().map_err(|err| EthornellError::Other(format!("event loop: {err}")))?;
    let window = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0))
        .build(&event_loop)
        .map_err(|err| EthornellError::Other(format!("window init failed: {err}")))?;
    let window: &'static winit::window::Window = Box::leak(Box::new(window));

    let mut renderer = pollster::block_on(Renderer::new(window))?;
    let mut _audio = AudioSystem::new()?;
    if runtime.is_none() {
        if let Some(bgm) = bgm {
            match _audio.play_from_bytes(&bgm) {
                Ok(()) => tracing::info!("title BGM playback started"),
                Err(err) => tracing::warn!(%err, "title BGM playback failed"),
            }
        }
    }
    let texture = match &image {
        Some(image) => Some(renderer.insert_rgba(image)?),
        None => None,
    };
    let mut graph_textures: BTreeMap<String, TextureHandle> = BTreeMap::new();
    let image_size = image.as_ref().map(|image| (image.width, image.height));
    let mut cursor_game_pos: Option<(f32, f32)> = None;
    if let Some(text) = &text {
        tracing::info!(%text, "text overlay requested");
    }

    let frame_interval = Duration::from_millis(16);
    let vm_slices_per_frame = std::env::var("ETHORNELL_VM_SLICES_PER_FRAME")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(8);
    let bootstrap_vm_slices_per_frame = std::env::var("ETHORNELL_VM_BOOTSTRAP_SLICES_PER_FRAME")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(16);
    let continue_after_yield = std::env::var("ETHORNELL_VM_CONTINUE_AFTER_YIELD")
        .ok()
        .map(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
        .unwrap_or(false);
    tracing::info!(
        vm_slices_per_frame,
        bootstrap_vm_slices_per_frame,
        continue_after_yield,
        "runtime frame pacing configured"
    );
    let mut next_frame = Instant::now();
    event_loop.set_control_flow(ControlFlow::WaitUntil(next_frame));
    event_loop
        .run(move |event, target| match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(size) => renderer.resize(size.width, size.height),
                WindowEvent::KeyboardInput {
                    event:
                        KeyEvent {
                            physical_key: PhysicalKey::Code(KeyCode::Escape),
                            state: ElementState::Pressed,
                            ..
                        },
                    ..
                } => target.exit(),
                WindowEvent::CursorMoved { position, .. } => {
                    cursor_game_pos =
                        renderer.surface_to_game_point(position.x as f32, position.y as f32);
                    if let Some(runtime) = runtime.as_mut() {
                        runtime.api.mouse_pos = cursor_game_pos;
                    }
                }
                WindowEvent::KeyboardInput {
                    event:
                        KeyEvent {
                            physical_key: PhysicalKey::Code(code),
                            state: ElementState::Pressed,
                            ..
                        },
                    ..
                } => {
                    if let Some(descriptor) = input_descriptor_for_keycode(code) {
                        if let Some(runtime) = runtime.as_mut() {
                            runtime.api.pending_input_state = Some(0x1000_0002);
                            runtime.api.pending_input_descriptor = Some(descriptor);
                        }
                        tracing::info!(descriptor, "keyboard advance");
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    if let Some(runtime) = runtime.as_mut() {
                        runtime.api.mouse_pressed = true;
                        runtime.api.pending_object_state = cursor_game_pos;
                        runtime.api.pending_input_state = Some(0x1000_0002);
                        runtime.api.pending_input_descriptor = Some(INPUT_DESCRIPTOR_ENTER);
                    }
                    tracing::info!(?cursor_game_pos, "mouse advance");
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } => {
                    if let Some(runtime) = runtime.as_mut() {
                        runtime.api.mouse_pressed = false;
                        runtime.api.pending_click = cursor_game_pos;
                        runtime.api.pending_click_age_frames = 0;
                        runtime.api.pending_input_state = Some(0x1000_0006);
                        runtime.api.pending_input_descriptor = Some(INPUT_DESCRIPTOR_ENTER);
                    }
                }
                WindowEvent::RedrawRequested => {
                    if let Some(runtime) = runtime.as_mut() {
                        let mut last_report = None;
                        let mut total_steps = 0usize;
                        let mut last_continue_setup_yield = false;
                        let mut last_continue_input_yield = false;
                        let slices_this_frame = if runtime.api.should_continue_vm_after_yield() {
                            bootstrap_vm_slices_per_frame
                        } else {
                            vm_slices_per_frame
                        };
                        for _ in 0..slices_this_frame {
                            let report = runtime.tick();
                            total_steps += report.steps;
                            let yielded = report.stop_reason
                                == ethornell_vm::VmStopReason::WaitingForAnimation;
                            let continue_setup_yield = runtime.api.should_continue_vm_after_yield();
                            let continue_input_yield = runtime.api.should_continue_input_yield();
                            last_continue_setup_yield = continue_setup_yield;
                            last_continue_input_yield = continue_input_yield;
                            last_report = Some(report);
                            if yielded
                                && !continue_after_yield
                                && !continue_setup_yield
                                && !continue_input_yield
                            {
                                break;
                            }
                            if !yielded {
                                break;
                            }
                        }
                        if runtime.api.debug_graph {
                            let report = last_report.as_ref();
                            tracing::info!(
                                steps = total_steps,
                                slices = vm_slices_per_frame,
                                program = report.map(|report| report.program.as_str()),
                                pc = report.map(|report| report.pc),
                                offset = ?report.and_then(|report| report.offset),
                                reason = ?report.map(|report| &report.stop_reason),
                                slices_this_frame,
                                continue_setup_yield = last_continue_setup_yield,
                                continue_input_yield = last_continue_input_yield,
                                yield_blockers = ?runtime.api.vm_after_yield_blockers(),
                                stack = runtime.vm.stack.len(),
                                "runtime frame VM tick"
                            );
                            if let Some(report) = report {
                                runtime.api.trace_render_snapshot(0, Some(report));
                                if report.stop_reason == ethornell_vm::VmStopReason::Error {
                                    tracing::warn!(
                                        recent_trace = ?report.recent_trace,
                                        "runtime VM halted with error"
                                    );
                                }
                            }
                        }
                        runtime.api.finish_frame_input();
                        while let Some(request) = runtime.api.audio_requests.pop_front() {
                            match _audio.play_from_bytes(&request.bytes) {
                                Ok(()) => tracing::info!(
                                    archive = request.archive,
                                    file = request.file,
                                    "script audio playback started"
                                ),
                                Err(err) => tracing::warn!(
                                    archive = request.archive,
                                    file = request.file,
                                    %err,
                                    "script audio playback failed"
                                ),
                            }
                        }
                        for (key, image) in &runtime.api.graph_images {
                            if !graph_textures.contains_key(key) {
                                match renderer.insert_rgba(image) {
                                    Ok(handle) => {
                                        tracing::info!(
                                            key,
                                            width = handle.width,
                                            height = handle.height,
                                            "uploaded graph texture"
                                        );
                                        graph_textures.insert(key.clone(), handle);
                                    }
                                    Err(err) => {
                                        tracing::warn!(key, %err, "graph texture upload failed")
                                    }
                                }
                            }
                        }
                    }
                    let mut commands = vec![RenderCommand::Clear {
                        color: [0.0, 0.0, 0.0, 1.0],
                    }];
                    if let (Some(texture), Some((image_width, image_height))) =
                        (&texture, image_size)
                    {
                        push_fit_texture_command(&mut commands, texture, image_width, image_height);
                    }
                    if let Some(text) = &text {
                        commands.push(RenderCommand::DrawText {
                            text: text.clone(),
                            x: 38.0,
                            y: 38.0,
                            color: [0.0, 0.0, 0.0, 0.75],
                            size: 28.0,
                            z: 9,
                        });
                        commands.push(RenderCommand::DrawText {
                            text: text.clone(),
                            x: 36.0,
                            y: 36.0,
                            color: [1.0, 1.0, 1.0, 1.0],
                            size: 28.0,
                            z: 10,
                        });
                    }
                    if let Some(runtime) = runtime.as_ref() {
                        for item in runtime.api.graph_draw_items() {
                            let Some(texture) = graph_textures.get(&item.key) else {
                                continue;
                            };
                            commands.push(RenderCommand::DrawTexture {
                                texture: texture.id,
                                x: item.x,
                                y: item.y,
                                width: item.width,
                                height: item.height,
                                src_x: item.src_x / texture.width as f32,
                                src_y: item.src_y / texture.height as f32,
                                src_width: item.src_width / texture.width as f32,
                                src_height: item.src_height / texture.height as f32,
                                opacity: item.opacity,
                                z: 2 + item.z,
                            });
                        }
                    }
                    if let Some(runtime) = runtime.as_ref() {
                        for (index, node) in runtime
                            .api
                            .text_nodes
                            .values()
                            .filter(|node| runtime.api.should_draw_text_node(node))
                            .enumerate()
                        {
                            let x = if node.x == 0.0 { 760.0 } else { node.x };
                            let y = if node.y == 0.0 {
                                330.0 + index as f32 * 40.0
                            } else {
                                node.y
                            };
                            let size = node.size.max(20.0);
                            commands.push(RenderCommand::DrawText {
                                text: node.text.clone(),
                                x: x + 2.0,
                                y: y + 2.0,
                                color: [0.0, 0.0, 0.0, node.color[3] * 0.75],
                                size,
                                z: node.z + index as i32 - 1,
                            });
                            commands.push(RenderCommand::DrawText {
                                text: node.text.clone(),
                                x,
                                y,
                                color: node.color,
                                size,
                                z: node.z + index as i32,
                            });
                        }
                    }
                    renderer.submit(&commands);
                    if let Err(err) = renderer.render() {
                        tracing::error!(%err, "render failed");
                        target.exit();
                    }
                }
                _ => {}
            },
            Event::AboutToWait => {
                let now = Instant::now();
                if now >= next_frame {
                    window.request_redraw();
                    next_frame = now + frame_interval;
                }
                target.set_control_flow(ControlFlow::WaitUntil(next_frame));
            }
            _ => {}
        })
        .map_err(|err| EthornellError::Other(format!("event loop failed: {err}")))
}

fn push_fit_texture_command(
    commands: &mut Vec<RenderCommand>,
    texture: &TextureHandle,
    image_width: u32,
    image_height: u32,
) {
    let target_w = 1280.0f32;
    let target_h = 720.0f32;
    let scale = (target_w / image_width as f32)
        .min(target_h / image_height as f32)
        .max(0.01);
    let width = image_width as f32 * scale;
    let height = image_height as f32 * scale;
    commands.push(RenderCommand::DrawTexture {
        texture: texture.id,
        x: (target_w - width) * 0.5,
        y: (target_h - height) * 0.5,
        width,
        height,
        src_x: 0.0,
        src_y: 0.0,
        src_width: 1.0,
        src_height: 1.0,
        opacity: 1.0,
        z: 0,
    });
}
