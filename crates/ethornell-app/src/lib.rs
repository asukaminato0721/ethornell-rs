use ethornell_archive::{detect_magic, scan_game_root, MagicKind, ResourceManager};
use ethornell_audio::AudioSystem;
use ethornell_core::{EthornellError, GameRoot, Result};
use ethornell_image::{decode_image, DecodedImage};
use ethornell_render::{RenderCommand, Renderer, TextureHandle};
use ethornell_script::{calls::known_call_arg_count, known_call_name};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
mod animation;
mod audio_runtime;
mod auto_input;
mod character_image;
mod debug_trace;
mod effects;
mod graph;
mod graph_defaults;
mod graph_effect;
mod graph_input;
mod graph_scroll;
mod headless;
mod native_effect;
mod native_graph;
mod native_graph_ext;
mod native_graph_resource;
mod native_sound;
mod native_system;
mod native_system_ext;
mod native_user;
mod platform;
mod resource_lookup;
mod scenario;
mod scene;
mod snapshot;
mod surface_controls;
mod text;
mod text_anim;
mod timeline;
mod title;
mod user_files;
use animation::{GraphAnimationRegistry, LayerAnimationSystem};
use audio_runtime::{
    execute_audio_command, AudioAsset, AudioCommand, NativeAudioClock, SoundSlot,
};
use character_image::decode_scenario_resource_image;
use graph::{
    blend_decoded_image_parameter, blit_decoded_image, blit_decoded_image_parameter,
    crop_decoded_image, crossfade_decoded_images, fixed_16_to_f32, native_draw_order,
    scale_decoded_image_fixed, NativeImageNodeArgs, RuntimeClipRect, RuntimeGraphDrawItem,
    RuntimeGraphLayer, RuntimeGraphObjectProperties, RuntimeGraphResource,
    RuntimeGraphTransitionNode, RuntimeSurface, RuntimeUserControl, NATIVE_DISPLAY_Z,
    NATIVE_SCREEN_BITMAP,
};
use graph_defaults::{GraphRuntimeDefaults, SurfaceTextState};
use graph_input::{pack_words, RuntimeGraphInputObject};
use graph_scroll::GraphScrollState;
use headless::{HeadlessInputEvent, HeadlessInputScript};
use resource_lookup::find_scenario_image;
use scenario::{ScenarioAction, ScenarioPlayback};
use scene::{
    place_scenario_sprite_with_hints, scenario_layer_ids_for_slot, sprite_fade_frames,
    sprite_target_opacity, SCENARIO_OVERLAY_LAYER_ID,
};
use surface_controls::SurfaceControlRegistry;
use text::{RuntimeTextNode, TextState, MESSAGE_NAME_TEXT_Z, MESSAGE_TEXT_Z};
use text_anim::TextRuntime;
use timeline::{TimelineEvent, TimelineSystem};
use user_files::{
    find_runtime_file, game_root_path, is_empty_archive_arg, read_runtime_bytes,
    runtime_file_cache_key, runtime_file_path,
};
use winit::event::{ElementState, Event, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, WindowBuilder};

macro_rules! trace_graph {
    ($runtime:expr, $($arg:tt)*) => {
        if $runtime.debug_graph {
            let message = format!($($arg)*);
            $runtime.trace_graph(message);
        }
    };
}

const INPUT_DESCRIPTOR_LEFT: i32 = 37;
const INPUT_DESCRIPTOR_UP: i32 = 38;
const INPUT_DESCRIPTOR_RIGHT: i32 = 39;
const INPUT_DESCRIPTOR_DOWN: i32 = 40;
const INPUT_DESCRIPTOR_MOUSE_LEFT: i32 = 1;
const INPUT_DESCRIPTOR_ENTER: i32 = 13;

pub struct AppConfig {
    pub game_root: GameRoot,
    pub trace: bool,
    pub fail_on_stub: bool,
    pub force_text_test: bool,
    pub headless: bool,
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
    // sub_4650F0/sub_465090 in the native executable select this pair for the
    // ordinary bootstrap. autoload.arc is a separate shortcut entry point
    // which deliberately sets the auto-load mode flag before chaining here.
    let startup_script = config.script.as_deref().unwrap_or("system.arc:ipl._bp");
    let startup_bytes = if let Some(script) = config.script.as_deref() {
        manager.read_decoded(script)
    } else {
        manager.read_decoded_from_archive("system.arc", "ipl._bp")
    };
    tracing::info!(script = startup_script, "selected runtime bootstrap script");
    let runtime = match Some(startup_script) {
        Some(script) => match startup_bytes {
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
    if config.headless || std::env::var_os("ETHORNELL_GUI_HEADLESS").is_some() {
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
    graph_defaults: GraphRuntimeDefaults,
    window_mode: i32,
    screen_width: i32,
    screen_height: i32,
    debug_graph: bool,
    trace_render_tree: bool,
    graph_trace: VecDeque<String>,
    graph_images: BTreeMap<String, DecodedImage>,
    graph_image_revisions: BTreeMap<String, u64>,
    next_graph_image_revision: u64,
    graphic_resource_keys: BTreeMap<(i32, i32), String>,
    graph_resources: BTreeMap<i32, RuntimeGraphResource>,
    graph_bindings: BTreeMap<i32, i32>,
    graph_layers: BTreeMap<i32, RuntimeGraphLayer>,
    graph_transition_nodes: BTreeMap<i32, RuntimeGraphTransitionNode>,
    graph_object_layers: BTreeMap<i32, BTreeSet<i32>>,
    graph_object_enabled: BTreeMap<i32, bool>,
    graph_object_properties: BTreeMap<i32, RuntimeGraphObjectProperties>,
    graph_object_input_tables: BTreeMap<i32, i32>,
    graph_input_objects: BTreeMap<i32, RuntimeGraphInputObject>,
    current_graph_object: Option<i32>,
    primary_bitmap: Option<i32>,
    bitmap_dimensions: BTreeMap<i32, (u32, u32)>,
    graph_surfaces: BTreeMap<i32, RuntimeSurface>,
    surface_text_states: BTreeMap<i32, SurfaceTextState>,
    surface_text_buffers: BTreeMap<i32, String>,
    graph_scroll_states: BTreeMap<i32, GraphScrollState>,
    surface_controls: SurfaceControlRegistry,
    graph_default_priority: i32,
    graph_driver_mode: i32,
    graph_global_offset: (f32, f32),
    graph_special_watches: BTreeSet<i32>,
    graph_special_events: VecDeque<i32>,
    graph_process_handles: BTreeSet<i32>,
    graph_blob_cache: BTreeMap<(String, String), Vec<u8>>,
    queued_system_events: VecDeque<[i32; 3]>,
    dropped_files: VecDeque<String>,
    quit_requested: bool,
    graph_animations: GraphAnimationRegistry,
    graph_effects: graph_effect::GraphEffectRegistry,
    effects: effects::RuntimeEffects,
    layer_animations: LayerAnimationSystem,
    timelines: TimelineSystem,
    user_controls: BTreeMap<i32, RuntimeUserControl>,
    text_nodes: BTreeMap<i32, RuntimeTextNode>,
    sound_slots: BTreeMap<i32, SoundSlot>,
    bgm_slots: BTreeMap<i32, SoundSlot>,
    sound_group_volumes: [u8; 16],
    bgm_channel_volumes: [u8; 16],
    sound_channel_volumes: [u8; 64],
    bgm_channel_active: [bool; 16],
    sound_channel_active: [bool; 64],
    bgm_playback_clocks: [NativeAudioClock; 16],
    sound_playback_clocks: [NativeAudioClock; 64],
    audio_requests: VecDeque<AudioCommand>,
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
    native_message_active: bool,
    native_message_surface_target: Option<i32>,
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
    input_master_gate: i32,
    input_latched_state: i32,
    pending_input_consumed: bool,
    suppress_message_mouse_release: bool,
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
    frame_yield_requested: bool,
    pending_graph_procedure_schedule: Option<ethornell_vm::GraphProcedureSchedule>,
    animation_queue_remaining: u32,
    pending_transition_destination: Option<i32>,
    pending_window_title: Option<String>,
    pending_fullscreen: Option<bool>,
    pending_window_visible: Option<bool>,
    pending_window_minimize: bool,
    window_focused: bool,
    input_requires_focus: bool,
    system_mode_flag: i32,
    system_extension_state: i32,
    system_config_input_mode: i32,
    native_system: native_system::NativeSystemState,
    native_user: native_user::NativeUserState,
    native_effect: native_effect::NativeEffectState,
    call_coverage_enabled: bool,
    call_coverage: BTreeMap<(u8, u16), usize>,
    runtime_stubbed: bool,
}

impl RuntimeTraceApi {
    fn new(manager: ResourceManager) -> Self {
        Self {
            manager,
            text_state: TextState::default(),
            text_runtime: TextRuntime::default(),
            graph_defaults: GraphRuntimeDefaults::default(),
            window_mode: 0,
            screen_width: 0,
            screen_height: 0,
            debug_graph: std::env::var("DEBUG").ok().as_deref() == Some("1"),
            trace_render_tree: std::env::var_os("TRACE_RENDER_TREE").is_some(),
            graph_trace: VecDeque::new(),
            graph_images: BTreeMap::new(),
            graph_image_revisions: BTreeMap::new(),
            next_graph_image_revision: 1,
            graphic_resource_keys: BTreeMap::new(),
            graph_resources: BTreeMap::new(),
            graph_bindings: BTreeMap::new(),
            graph_layers: BTreeMap::new(),
            graph_transition_nodes: BTreeMap::new(),
            graph_object_layers: BTreeMap::new(),
            graph_object_enabled: BTreeMap::new(),
            graph_object_properties: BTreeMap::new(),
            graph_object_input_tables: BTreeMap::new(),
            graph_input_objects: BTreeMap::new(),
            current_graph_object: None,
            primary_bitmap: None,
            bitmap_dimensions: BTreeMap::new(),
            graph_surfaces: BTreeMap::new(),
            surface_text_states: BTreeMap::new(),
            surface_text_buffers: BTreeMap::new(),
            graph_scroll_states: BTreeMap::new(),
            surface_controls: SurfaceControlRegistry::default(),
            graph_default_priority: 0,
            graph_driver_mode: 0,
            graph_global_offset: (0.0, 0.0),
            graph_special_watches: BTreeSet::new(),
            graph_special_events: VecDeque::new(),
            graph_process_handles: BTreeSet::new(),
            graph_blob_cache: BTreeMap::new(),
            queued_system_events: VecDeque::new(),
            dropped_files: VecDeque::new(),
            quit_requested: false,
            graph_animations: GraphAnimationRegistry::default(),
            graph_effects: graph_effect::GraphEffectRegistry::default(),
            effects: effects::RuntimeEffects::default(),
            layer_animations: LayerAnimationSystem::default(),
            timelines: TimelineSystem::default(),
            user_controls: BTreeMap::new(),
            text_nodes: BTreeMap::new(),
            sound_slots: BTreeMap::new(),
            bgm_slots: BTreeMap::new(),
            sound_group_volumes: [128; 16],
            bgm_channel_volumes: [128; 16],
            sound_channel_volumes: [128; 64],
            bgm_channel_active: [false; 16],
            sound_channel_active: [false; 64],
            bgm_playback_clocks: [NativeAudioClock::default(); 16],
            sound_playback_clocks: [NativeAudioClock::default(); 64],
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
            native_message_active: false,
            native_message_surface_target: None,
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
            input_master_gate: 0,
            input_latched_state: 0,
            pending_input_consumed: false,
            suppress_message_mouse_release: false,
            last_hit_control: 0,
            last_hit_payload: 0,
            auto_title_click: std::env::var("ETHORNELL_AUTO_TITLE_CLICK").ok().as_deref()
                == Some("1"),
            auto_title_click_id: std::env::var("ETHORNELL_AUTO_TITLE_CLICK_ID")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(6154),
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
            frame_yield_requested: false,
            pending_graph_procedure_schedule: None,
            animation_queue_remaining: 0,
            pending_transition_destination: None,
            pending_window_title: None,
            pending_fullscreen: None,
            pending_window_visible: None,
            pending_window_minimize: false,
            window_focused: true,
            input_requires_focus: false,
            system_mode_flag: 0,
            system_extension_state: 0,
            system_config_input_mode: 0,
            native_system: native_system::NativeSystemState::default(),
            native_user: native_user::NativeUserState::default(),
            native_effect: native_effect::NativeEffectState::default(),
            call_coverage_enabled: std::env::var_os("ETHORNELL_CALL_COVERAGE").is_some(),
            call_coverage: BTreeMap::new(),
            runtime_stubbed: false,
        }
    }

    fn store_graph_image(&mut self, key: String, image: DecodedImage) {
        let revision = self.next_graph_image_revision;
        self.next_graph_image_revision = self.next_graph_image_revision.wrapping_add(1).max(1);
        self.graph_images.insert(key.clone(), image);
        self.graph_image_revisions.insert(key, revision);
    }

    fn graph_image_revision(&self, key: &str) -> u64 {
        self.graph_image_revisions
            .get(key)
            .copied()
            .unwrap_or_default()
    }

    fn is_primary_framebuffer(&self, bitmap: i32) -> bool {
        if bitmap == NATIVE_SCREEN_BITMAP || self.primary_bitmap == Some(bitmap) {
            return true;
        }
        if self.primary_bitmap.is_some() {
            return false;
        }
        let screen_dimensions = snapshot::runtime_frame_size(self);
        self.bitmap_dimensions
            .get(&bitmap)
            .is_some_and(|dimensions| *dimensions == screen_dimensions)
    }

    fn graph_bitmap_image(&self, bitmap: i32) -> Option<DecodedImage> {
        self.resource_image_region(bitmap)
            .and_then(|(key, region)| {
                self.graph_images
                    .get(key)
                    .map(|image| crop_decoded_image(image, region))
            })
            .or_else(|| {
                self.is_primary_framebuffer(bitmap)
                    .then(|| snapshot::compose_runtime_frame(self, true))
            })
    }

    fn record_call(&mut self, group: u8, id: u16) {
        if !self.call_coverage_enabled {
            return;
        }
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
        !self.title_ui_departed
            && !self.scenario_bootstrapped
            && self.vm_after_yield_blockers().is_empty()
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
        if self.animation_queue_remaining > 0 {
            blockers.push("animation");
        }
        blockers
    }

    fn has_blocking_user_controls(&self) -> bool {
        if self.scenario_bootstrapped && !self.has_active_message_window() {
            return false;
        }
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
        let exists = find_runtime_file(&self.manager, archive, file)
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
                    .insert(target_id, RuntimeGraphResource::whole(key.clone()));
            }
            trace_graph!(self, "load image #{target_id} {key} (cached)");
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
            trace_graph!(
                self,
                "load resource {archive_name}:{resource_name} (not image)"
            );
            return false;
        };
        trace_graph!(
            self,
            "load image #{target_id} {key} -> {}x{}",
            image.width,
            image.height
        );
        if target_id != 0 {
            self.graph_resources
                .insert(target_id, RuntimeGraphResource::whole(key.clone()));
        }
        self.store_graph_image(key, image);
        true
    }

    fn load_graph_image_key(&mut self, key: &str) -> bool {
        if self.graph_images.contains_key(key) {
            return true;
        }
        let Some((archive, resource)) = key.split_once(':') else {
            return false;
        };
        self.load_graph_image_resource(0, archive, resource)
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
        let instruction_count = playback.instruction_count();
        self.scenario_playback = Some(playback);
        self.scenario_bootstrapped = true;
        self.scenario_loaded_scripts.insert(file.to_string());
        self.scenario_input_latched = false;
        self.pending_click = None;
        self.pending_click_age_frames = 0;
        self.pending_object_state = None;
        self.pending_input_state = None;
        self.pending_input_descriptor = None;
        self.pending_input_consumed = false;
        self.mouse_pressed = false;
        self.clear_title_graph_layers();
        tracing::info!(
            archive,
            file,
            instruction_count,
            "BCS scenario playback started"
        );
        trace_graph!(
            self,
            "BCS playback started {archive}:{file} instructions={instruction_count}"
        );
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
                let channel = scenario_audio_channel(sound_name);
                let looped = sound_name.to_ascii_lowercase().starts_with("bgm");
                let asset = AudioAsset {
                    archive: archive.clone(),
                    file: sound_name.to_string(),
                    bytes,
                };
                self.audio_requests.push_back(AudioCommand::Play {
                    asset,
                    channel,
                    looped,
                    volume: 1.0,
                    decode_gain: 1.0,
                    playback_rate: 1.0,
                    panning: 0.5,
                    fade_ms: 0,
                    restart: true,
                });
                set_sound_channel_active(&mut self.sound_channel_active, channel, true);
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
            self.store_graph_image(key.clone(), image);
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
                transform_z: 0,
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
        trace_graph!(self,
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
        );
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
                trace_graph!(self, "BCS playback clear transition cover layer={layer_id}");
            }
        }
    }

    fn append_scenario_script(&mut self, file: &str) {
        let previously_loaded = self.scenario_loaded_scripts.contains(file);
        let Some(entry) = find_runtime_resource(&self.manager, "data01xxx.arc", file) else {
            tracing::warn!(file, "BCS scenario script missing");
            if let Some(playback) = self.scenario_playback.as_mut() {
                playback.cancel_external_script();
            }
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
            if let Some(playback) = self.scenario_playback.as_mut() {
                playback.cancel_external_script();
            }
            return;
        };
        self.scenario_loaded_scripts.insert(file.to_string());
        let Some(added) = self
            .scenario_playback
            .as_mut()
            .and_then(|playback| playback.append_bcs(&bytes))
        else {
            tracing::warn!(archive, file, "BCS scenario script parse failed");
            return;
        };
        tracing::info!(
            archive,
            file,
            added,
            previously_loaded,
            "BCS scenario script appended"
        );
        trace_graph!(
            self,
            "BCS playback append script {archive}:{file} instructions={added}"
        );
    }

    fn trace_timeline_event(&mut self, event: TimelineEvent) {
        match event {
            TimelineEvent::Configured { handle, duration } => {
                if handle <= 0 {
                    return;
                }
                trace_graph!(self, "timeline #{handle} configured duration={duration}");
            }
            TimelineEvent::Attached { handle, target } => {
                if handle <= 0 {
                    return;
                }
                trace_graph!(self, "timeline #{handle} attached target #{target}");
            }
            TimelineEvent::Enabled {
                handle,
                enabled,
                remaining,
            } => {
                if handle <= 0 {
                    return;
                }
                trace_graph!(
                    self,
                    "timeline #{handle} enabled={enabled} remaining={remaining}"
                );
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
            self.graph_object_properties.entry(object).or_default();
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
        if let Some(properties) = self.graph_object_properties.remove(&source) {
            self.graph_object_properties.insert(object, properties);
        }
        trace_graph!(
            self,
            "object #{object} adopted {count} layers from object #{source}"
        );
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
        trace_graph!(
            self,
            "object #{object} enabled={enabled} layers={}",
            self.graph_object_layers
                .get(&object)
                .map(BTreeSet::len)
                .unwrap_or_default()
        );
    }

    fn remove_graph_object(&mut self, object: i32) -> bool {
        let mut removed = self.graph_object_enabled.remove(&object).is_some();
        removed |= self.graph_input_objects.remove(&object).is_some();
        removed |= self.graph_object_properties.remove(&object).is_some();
        self.user_controls
            .retain(|_, control| control.title_only || control.owner_id != object);
        if self.current_graph_object == Some(object) {
            self.current_graph_object = None;
        }
        if let Some(layer_ids) = self.graph_object_layers.remove(&object) {
            removed = true;
            for layer_id in layer_ids {
                if self
                    .graph_layers
                    .get(&layer_id)
                    .is_some_and(|layer| layer.owner_object == Some(object))
                {
                    self.graph_layers.remove(&layer_id);
                }
            }
            trace_graph!(self, "finalize object #{object}");
        }
        removed
    }

    fn apply_graph_blit_rect(&mut self, args: &[ethornell_vm::Value]) {
        let popped = args.iter().map(value_to_i32).collect::<Vec<_>>();
        let mut source_order = popped.clone();
        source_order.reverse();
        let height = popped.first().copied().unwrap_or_default().max(0) as f32;
        let width = popped.get(1).copied().unwrap_or_default().max(0) as f32;
        let y = popped.get(2).copied().unwrap_or_default() as f32;
        let x = popped.get(3).copied().unwrap_or_default() as f32;
        let resource_id = source_order
            .iter()
            .copied()
            .find(|value| self.resolve_resource_key(*value).is_some());
        let target = source_order
            .iter()
            .copied()
            .find(|value| {
                *value > 0
                    && Some(*value) != resource_id
                    && (self.graph_layers.contains_key(value)
                        || self.graph_surfaces.contains_key(value)
                        || *value >= 128)
            })
            .unwrap_or_default();
        let Some(resource_id) = resource_id else {
            trace_graph!(self, "blit rect skipped no resource raw={source_order:?}");
            return;
        };
        let Some(key) = self.resolve_resource_key(resource_id).map(str::to_string) else {
            return;
        };
        let Some(image) = self.graph_images.get(&key) else {
            return;
        };
        let width = if width > 0.0 {
            width.min(image.width as f32)
        } else {
            image.width as f32
        };
        let height = if height > 0.0 {
            height.min(image.height as f32)
        } else {
            image.height as f32
        };
        if let Some(surface) = self.graph_surfaces.get_mut(&target) {
            surface.resource_id = Some(resource_id);
            surface.viewport_width = width;
            surface.viewport_height = height;
            trace_graph!(self,
                "blit rect surface #{target} res=#{resource_id} {key} x={x:.0} y={y:.0} w={width:.0} h={height:.0}"
            );
            return;
        }
        let layer_id = if target > 0 { target } else { resource_id };
        let owner_object = self.current_graph_object;
        if let Some(object) = owner_object {
            self.graph_object_layers
                .entry(object)
                .or_default()
                .insert(layer_id);
        }
        let enabled = self.object_enabled_for_layer(owner_object);
        self.graph_layers.insert(
            layer_id,
            RuntimeGraphLayer {
                hit_id: layer_id,
                owner_object,
                key: key.clone(),
                target_surface: None,
                x,
                y,
                width,
                height,
                src_x: x.max(0.0),
                src_y: y.max(0.0),
                opacity: 1.0,
                z: layer_id,
                enabled,
                transform_x: 0.0,
                transform_y: 0.0,
                transform_z: 0,
                scale_x: 1.0,
                scale_y: 1.0,
                rotation_degrees: 0.0,
                clip: None,
            },
        );
        trace_graph!(self,
            "blit rect layer #{layer_id} res=#{resource_id} {key} x={x:.0} y={y:.0} w={width:.0} h={height:.0} owner={owner_object:?}"
        );
    }

    fn refresh_graph_object_rect(&mut self, args: &[ethornell_vm::Value]) {
        let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
        if values.len() != 5 {
            return;
        }
        // sub_47C170 pops height, width, y, x, then the display-object id and
        // forwards the object to its native refresh virtual. The rectangle is
        // a dirty-region hint, not a bitmap copy operation.
        let object = values[4];
        let x = values[3];
        let y = values[2];
        let width = values[1].max(0);
        let height = values[0].max(0);
        let exists = self.graph_handle_exists(object);
        trace_graph!(
            self,
            "refresh object #{object} rect=({x},{y} {width}x{height}) exists={exists}"
        );
    }

    fn apply_graph_scroll_position(&mut self, handle: i32) {
        let Some(state) = self.graph_scroll_states.get(&handle).copied() else {
            return;
        };
        let dimensions = self
            .graph_layers
            .get(&state.target)
            .map(|layer| (layer.width, layer.height))
            .or_else(|| {
                self.graph_object_layers
                    .get(&state.target)
                    .and_then(|layers| layers.iter().next())
                    .and_then(|layer| self.graph_layers.get(layer))
                    .map(|layer| (layer.width, layer.height))
            })
            .unwrap_or_default();
        let (x, y) = state.display_position(dimensions.0, dimensions.1);
        let mut moved = 0usize;
        if let Some(layer) = self.graph_layers.get_mut(&state.target) {
            layer.x = x;
            layer.y = y;
            moved = 1;
        } else if let Some(layers) = self.graph_object_layers.get(&state.target).cloned() {
            for layer_id in layers {
                if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                    layer.x = x;
                    layer.y = y;
                    moved += 1;
                }
            }
        }
        trace_graph!(
            self,
            "scroll #{handle} target=#{} logical=({}, {}) display=({x:.0}, {y:.0}) bounds={}x{} extent={}x{} moved={moved}",
            state.target,
            state.x,
            state.y,
            state.bounds_width,
            state.bounds_height,
            state.extent_x,
            state.extent_y
        );
    }

    fn apply_graph_layer_property(&mut self, args: &[ethornell_vm::Value]) {
        let popped = args.iter().map(value_to_i32).collect::<Vec<_>>();
        if popped.len() < 2 {
            return;
        }
        let value = popped[0];
        let target = popped[1];
        let alpha = (0..=256)
            .contains(&value)
            .then_some((value as f32 / 256.0).clamp(0.0, 1.0));
        let mut changed = 0usize;
        if let Some(layer_ids) = self.graph_object_layers.get(&target).cloned() {
            for layer_id in layer_ids {
                if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                    if let Some(alpha) = alpha {
                        layer.opacity = alpha;
                    }
                    changed += 1;
                }
            }
        }
        if let Some(layer) = self.graph_layers.get_mut(&target) {
            if let Some(alpha) = alpha {
                layer.opacity = alpha;
            }
            changed += 1;
        }
        if let Some(surface) = self.graph_surfaces.get_mut(&target) {
            surface.enabled = value != 0;
            changed += 1;
        }
        trace_graph!(
            self,
            "layer property target=#{target} value={value} alpha={alpha:?} changed={changed}"
        );
    }

    fn apply_graph_object_position(&mut self, args: &[ethornell_vm::Value]) {
        let popped = args.iter().map(value_to_i32).collect::<Vec<_>>();
        if popped.len() < 3 {
            return;
        }
        let y = popped[0] as f32;
        let x = popped[1] as f32;
        let target = popped[2];
        let mut changed = 0usize;
        if let Some(layer_ids) = self.graph_object_layers.get(&target).cloned() {
            for layer_id in layer_ids {
                if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                    layer.transform_x = x;
                    layer.transform_y = y;
                    changed += 1;
                }
            }
        }
        if let Some(layer) = self.graph_layers.get_mut(&target) {
            layer.transform_x = x;
            layer.transform_y = y;
            changed += 1;
        }
        if self.set_graph_surface_position(target, x, y) {
            changed += 1;
        }
        trace_graph!(
            self,
            "object position target=#{target} x={x:.0} y={y:.0} changed={changed}"
        );
    }

    fn set_graph_surface_position(&mut self, target: i32, x: f32, y: f32) -> bool {
        let parent_origin = self
            .graph_surfaces
            .get(&target)
            .and_then(|surface| surface.parent_surface)
            .and_then(|parent| self.graph_surfaces.get(&parent))
            .map(|parent| (parent.x, parent.y));
        let Some(surface) = self.graph_surfaces.get_mut(&target) else {
            return false;
        };
        surface.x = x;
        surface.y = y;
        if let Some((parent_x, parent_y)) = parent_origin {
            surface.local_x = x - parent_x;
            surface.local_y = y - parent_y;
        }
        self.propagate_graph_surface_children(target);
        true
    }

    fn attach_graph_surface(&mut self, parent: i32, child: i32, x: f32, y: f32) -> bool {
        let Some((parent_x, parent_y)) = self
            .graph_surfaces
            .get(&parent)
            .map(|surface| (surface.x, surface.y))
        else {
            return false;
        };
        if let Some(surface) = self
            .graph_surfaces
            .get_mut(&child)
            .filter(|surface| surface.display_attached)
        {
            surface.parent_surface = Some(parent);
            surface.local_x = x;
            surface.local_y = y;
            surface.x = parent_x + x;
            surface.y = parent_y + y;
            self.propagate_graph_surface_children(child);
            return true;
        }
        if let Some(layer) = self.graph_layers.get_mut(&child) {
            layer.target_surface = Some(parent);
            layer.x = x;
            layer.y = y;
            return true;
        }
        if let Some(node) = self.text_nodes.get_mut(&child) {
            node.target_surface = Some(parent);
            node.x = x;
            node.y = y;
            return true;
        }
        if let Some(surface) = self.graph_surfaces.get_mut(&child) {
            surface.parent_surface = Some(parent);
            surface.local_x = x;
            surface.local_y = y;
            surface.x = parent_x + x;
            surface.y = parent_y + y;
            self.propagate_graph_surface_children(child);
            return true;
        }
        false
    }

    fn detach_graph_surface(&mut self, parent: i32, child: i32) -> bool {
        let parent_origin = self
            .graph_surfaces
            .get(&parent)
            .map(|surface| (surface.x, surface.y))
            .unwrap_or_default();
        if let Some(surface) = self
            .graph_surfaces
            .get_mut(&child)
            .filter(|surface| surface.parent_surface == Some(parent))
        {
            surface.parent_surface = None;
            surface.local_x = surface.x;
            surface.local_y = surface.y;
            return true;
        }
        if let Some(layer) = self
            .graph_layers
            .get_mut(&child)
            .filter(|layer| layer.target_surface == Some(parent))
        {
            layer.target_surface = None;
            layer.x += parent_origin.0;
            layer.y += parent_origin.1;
            return true;
        }
        if let Some(node) = self
            .text_nodes
            .get_mut(&child)
            .filter(|node| node.target_surface == Some(parent))
        {
            node.target_surface = None;
            node.x += parent_origin.0;
            node.y += parent_origin.1;
            return true;
        }
        false
    }

    fn detach_graph_surface_relations(&mut self, surface: i32) {
        let parent_origin = self
            .graph_surfaces
            .get(&surface)
            .map(|parent| (parent.x, parent.y))
            .unwrap_or_default();
        for child in self.graph_surfaces.values_mut() {
            if child.parent_surface == Some(surface) {
                child.parent_surface = None;
                child.local_x = child.x;
                child.local_y = child.y;
            }
        }
        for layer in self.graph_layers.values_mut() {
            if layer.target_surface == Some(surface) {
                layer.target_surface = None;
                layer.x += parent_origin.0;
                layer.y += parent_origin.1;
            }
        }
        for node in self.text_nodes.values_mut() {
            if node.target_surface == Some(surface) {
                node.target_surface = None;
                node.x += parent_origin.0;
                node.y += parent_origin.1;
            }
        }
    }

    fn propagate_graph_surface_children(&mut self, parent: i32) {
        let Some((parent_x, parent_y)) = self
            .graph_surfaces
            .get(&parent)
            .map(|surface| (surface.x, surface.y))
        else {
            return;
        };
        let children = self
            .graph_surfaces
            .iter()
            .filter_map(|(&id, surface)| (surface.parent_surface == Some(parent)).then_some(id))
            .collect::<Vec<_>>();
        for child in children {
            if let Some(surface) = self.graph_surfaces.get_mut(&child) {
                surface.x = parent_x + surface.local_x;
                surface.y = parent_y + surface.local_y;
            }
            self.propagate_graph_surface_children(child);
        }
    }

    fn hit_test_graph_input_object(
        &self,
        object: i32,
        screen_point: (f32, f32),
    ) -> Option<(ethornell_vm::GraphInputRegion, i32, i32)> {
        let input = self.graph_input_objects.get(&object)?;
        let origin = self
            .graph_surfaces
            .get(&input.layer)
            .map(|surface| (surface.x, surface.y))
            .unwrap_or_default();
        input.hit_test((screen_point.0 - origin.0, screen_point.1 - origin.1))
    }

    fn apply_graph_object_property(&mut self, args: &[ethornell_vm::Value]) {
        let popped = args.iter().map(value_to_i32).collect::<Vec<_>>();
        if popped.len() < 4 {
            return;
        }
        let extra = popped[0];
        let value = popped[1];
        let property = popped[2] as u32;
        let target = popped[3];

        self.graph_object_properties
            .entry(target)
            .or_default()
            .set_property(property, value, extra);

        // Property 0 calls CDspObj's virtual position setter (+44).
        if property == 0 {
            self.apply_graph_object_position(&[
                ethornell_vm::Value::Int(extra),
                ethornell_vm::Value::Int(value),
                ethornell_vm::Value::Int(target),
            ]);
        }
        trace_graph!(
            self,
            "object property target=#{target} property=0x{property:08X} value={value} extra={extra}"
        );
    }

    fn apply_graph_layer_color_blend(&mut self, args: &[ethornell_vm::Value]) {
        let popped = args.iter().map(value_to_i32).collect::<Vec<_>>();
        let alpha = popped
            .iter()
            .copied()
            .find(|value| (0..=256).contains(value))
            .map(|value| (value as f32 / 256.0).clamp(0.0, 1.0));
        let color = popped
            .iter()
            .copied()
            .find(|value| (0x010000..=0x00ff_ffff).contains(value));
        let mut targets = popped
            .iter()
            .copied()
            .filter(|value| self.graph_layers.contains_key(value))
            .collect::<Vec<_>>();
        targets.extend(
            popped
                .iter()
                .copied()
                .filter(|value| self.graph_object_layers.contains_key(value)),
        );
        targets.sort_unstable();
        targets.dedup();
        let mut changed = 0usize;
        for target in &targets {
            if let Some(layer_ids) = self.graph_object_layers.get(target).cloned() {
                for layer_id in layer_ids {
                    if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                        if let Some(alpha) = alpha {
                            layer.opacity = alpha;
                        }
                        changed += 1;
                    }
                }
            }
            if let Some(layer) = self.graph_layers.get_mut(target) {
                if let Some(alpha) = alpha {
                    layer.opacity = alpha;
                }
                changed += 1;
            }
        }
        trace_graph!(self,
            "layer color blend targets={targets:?} alpha={alpha:?} color={color:?} changed={changed} raw={popped:?}"
        );
    }

    fn apply_graph_node_transition(&mut self, args: &[ethornell_vm::Value]) {
        let ints = args.iter().map(value_to_i32).collect::<Vec<_>>();
        if ints.len() != 7 {
            return;
        }
        // funcs_48065E[0x22] -> sub_47A790 pops seven values. BP source
        // order is target, mask, duration, transition, and three parameters.
        let target = ints[6];
        let duration_ms = ints
            .get(4)
            .copied()
            .filter(|value| (1..=10_000).contains(value))
            .unwrap_or(0);
        let mask = ints
            .get(5)
            .copied()
            .filter(|value| (0..=256).contains(value))
            .unwrap_or(0);
        let target_opacity = ((256 - mask) as f32 / 256.0).clamp(0.0, 1.0);
        let duration_frames = duration_ms.unsigned_abs().div_ceil(16).max(1);

        let mut animated_layers = 0usize;
        let mut animated_text = 0usize;
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
        } else if let Some(layer) = self.graph_layers.get_mut(&target) {
            let from = layer.opacity;
            if duration_frames > 1 && (from - target_opacity).abs() > f32::EPSILON {
                self.layer_animations
                    .fade_to(target, from, target_opacity, duration_frames);
            } else {
                layer.opacity = target_opacity;
            }
            animated_layers += 1;
        }
        if let Some(surface) = self.graph_surfaces.get_mut(&target) {
            let from = surface.opacity;
            if duration_frames > 1 && (from - target_opacity).abs() > f32::EPSILON {
                self.layer_animations
                    .fade_to(target, from, target_opacity, duration_frames);
            } else {
                surface.opacity = target_opacity;
            }
        }
        if let Some(node) = self.text_nodes.get_mut(&target) {
            node.color[3] = target_opacity;
            animated_text += 1;
        }
        self.animation_queue_remaining = self.animation_queue_remaining.max(duration_frames);
        trace_graph!(self,
            "node transition target=#{target} mask={mask} opacity={target_opacity:.2} duration_ms={duration_ms} frames={duration_frames} layers={animated_layers} text={animated_text}"
        );
    }

    fn graph_target_layers(&self, target: i32) -> Vec<i32> {
        let mut layer_ids = self
            .graph_object_layers
            .get(&target)
            .cloned()
            .map(|layers| layers.into_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        if self.graph_layers.contains_key(&target) {
            layer_ids.push(target);
        }
        layer_ids.sort_unstable();
        layer_ids.dedup();
        layer_ids
    }

    fn infer_graph_effect_target(&self, source: &[i32]) -> i32 {
        source
            .iter()
            .copied()
            .find(|value| {
                self.graph_object_layers.contains_key(value)
                    || self.graph_layers.contains_key(value)
                    || self.graph_surfaces.contains_key(value)
            })
            .or(self.current_graph_object)
            .unwrap_or_default()
    }

    fn apply_graph_effect_config(&mut self, effect_id: u16, args: &[ethornell_vm::Value]) {
        let mut source = args.iter().map(value_to_i32).collect::<Vec<_>>();
        source.reverse();
        let target = self.infer_graph_effect_target(&source);
        let layer_ids = self.graph_target_layers(target);
        let duration_frames = self.animation_queue_remaining.max(1);
        let mut changed = 0usize;

        match effect_id {
            0x10 | 0x16 if source.len() >= 5 => {
                let x = graph_motion_coord(source[1]);
                let y = graph_motion_coord(source[2]);
                let width = graph_motion_coord(source[3]).abs().max(1.0);
                let height = graph_motion_coord(source[4]).abs().max(1.0);
                for layer_id in &layer_ids {
                    if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                        if effect_id == 0x10 {
                            if duration_frames > 1 {
                                self.layer_animations.move_x_to(
                                    *layer_id,
                                    layer.transform_x,
                                    x,
                                    duration_frames,
                                );
                                self.layer_animations.move_y_to(
                                    *layer_id,
                                    layer.transform_y,
                                    y,
                                    duration_frames,
                                );
                            } else {
                                layer.transform_x = x;
                                layer.transform_y = y;
                            }
                        } else {
                            layer.clip = Some(RuntimeClipRect {
                                x,
                                y,
                                width,
                                height,
                            });
                        }
                        changed += 1;
                    }
                }
            }
            0x11 if source.len() >= 2 => {
                let alpha = (source[1] as f32 / 256.0).clamp(0.0, 1.0);
                for layer_id in &layer_ids {
                    if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                        if duration_frames > 1 {
                            self.layer_animations.fade_to(
                                *layer_id,
                                layer.opacity,
                                alpha,
                                duration_frames,
                            );
                        } else {
                            layer.opacity = alpha;
                        }
                        changed += 1;
                    }
                }
            }
            0x13 if source.len() >= 4 => {
                let rotation = fixed_16_to_f32(source[1]);
                let x = graph_motion_coord(source[2]);
                let y = graph_motion_coord(source[3]);
                for layer_id in &layer_ids {
                    if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                        layer.rotation_degrees = rotation;
                        layer.transform_x = x;
                        layer.transform_y = y;
                        changed += 1;
                    }
                }
            }
            0x12 | 0x15 => {
                let alpha = source
                    .iter()
                    .copied()
                    .skip(1)
                    .find(|value| (0..=256).contains(value))
                    .map(|value| (value as f32 / 256.0).clamp(0.0, 1.0));
                if let Some(alpha) = alpha {
                    for layer_id in &layer_ids {
                        if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                            layer.opacity = alpha;
                            changed += 1;
                        }
                    }
                }
            }
            _ => {}
        }

        trace_graph!(self,
            "effect 0x{effect_id:02X} target=#{target} layers={layer_ids:?} changed={changed} raw={source:?}"
        );
    }

    fn apply_graph_object_effect(&mut self, args: &[ethornell_vm::Value]) {
        let mut source = args.iter().map(value_to_i32).collect::<Vec<_>>();
        source.reverse();
        let schedule = animation::ScheduledObjectControl::from_popped_args(args);
        if let Some(schedule) = schedule {
            self.pending_graph_procedure_schedule = Some(schedule.procedure_schedule());
            let duration_frames = schedule.duration_ms.unsigned_abs().div_ceil(16).max(1);
            let target_x = fixed_16_to_f32(schedule.target_x);
            let target_y = fixed_16_to_f32(schedule.target_y);
            let target_alpha = (schedule.target_alpha as f32 / 256.0).clamp(0.0, 1.0);
            let layer_ids = self.graph_target_layers(schedule.target_object);
            for layer_id in &layer_ids {
                if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                    self.layer_animations.base_vector_to(
                        *layer_id,
                        (layer.x, layer.y, layer.z),
                        (target_x, target_y, schedule.target_z),
                        duration_frames,
                        schedule.position_curve,
                        schedule.z_curve,
                    );
                    self.layer_animations.fade_to_eased(
                        *layer_id,
                        layer.opacity,
                        target_alpha,
                        duration_frames,
                        0,
                    );
                }
            }
            self.animation_queue_remaining = self.animation_queue_remaining.max(duration_frames);
            trace_graph!(
                self,
                "object control target=#{} layers={layer_ids:?} position=({target_x:.2},{target_y:.2},{}) curves=({}, {}) alpha={target_alpha:.3} duration_ms={} update={}/{}",
                schedule.target_object,
                schedule.target_z,
                schedule.position_curve,
                schedule.z_curve,
                schedule.duration_ms,
                schedule.update_numerator,
                schedule.update_denominator
            );
        }
        let destination = self.pending_transition_destination;
        trace_graph!(
            self,
            "transition start destination={destination:?} schedule={schedule:?} raw={source:?}"
        );
    }

    fn apply_graph_affine_transform(&mut self, args: &[ethornell_vm::Value]) {
        let popped = args.iter().map(value_to_i32).collect::<Vec<_>>();
        let mut source = popped.clone();
        source.reverse();
        let resource_id = source
            .iter()
            .copied()
            .find(|value| self.resolve_resource_key(*value).is_some());
        let target = source
            .iter()
            .copied()
            .find(|value| {
                self.graph_layers.contains_key(value)
                    || self.graph_object_layers.contains_key(value)
                    || self.graph_surfaces.contains_key(value)
            })
            .or(resource_id)
            .unwrap_or_default();
        if target <= 0 {
            trace_graph!(self, "affine transform skipped raw={source:?}");
            return;
        }
        if let Some(resource_id) = resource_id {
            self.ensure_transform_layer(target, resource_id);
        }

        let fixed_values = source
            .iter()
            .copied()
            .filter(|value| value.unsigned_abs() >= 32_768)
            .map(fixed_16_to_f32)
            .collect::<Vec<_>>();
        let scale_values = fixed_values
            .iter()
            .copied()
            .filter(|value| (0.001..=8.0).contains(value))
            .collect::<Vec<_>>();
        let coord_values = fixed_values
            .iter()
            .copied()
            .filter(|value| value.abs() > 8.0)
            .collect::<Vec<_>>();
        let x = coord_values.first().copied();
        let y = coord_values.get(1).copied();
        let scale_x = scale_values.first().copied().map(normalize_transform_scale);
        let scale_y = scale_values.get(1).copied().map(normalize_transform_scale);
        let opacity = None::<f32>;

        let mut layer_ids = self
            .graph_object_layers
            .get(&target)
            .cloned()
            .map(|layers| layers.into_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        if self.graph_layers.contains_key(&target) {
            layer_ids.push(target);
        }
        layer_ids.sort_unstable();
        layer_ids.dedup();

        let duration_frames = self.animation_queue_remaining.max(1);
        let mut changed = 0usize;
        for layer_id in &layer_ids {
            if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                if let Some(x) = x {
                    if duration_frames > 1 {
                        self.layer_animations.move_x_to(
                            *layer_id,
                            layer.transform_x,
                            x,
                            duration_frames,
                        );
                    } else {
                        layer.transform_x = x;
                    }
                }
                if let Some(y) = y {
                    if duration_frames > 1 {
                        self.layer_animations.move_y_to(
                            *layer_id,
                            layer.transform_y,
                            y,
                            duration_frames,
                        );
                    } else {
                        layer.transform_y = y;
                    }
                }
                if let Some(scale_x) = scale_x {
                    layer.scale_x = scale_x;
                }
                if let Some(scale_y) = scale_y {
                    layer.scale_y = scale_y;
                }
                if let Some(opacity) = opacity {
                    layer.opacity = opacity;
                }
                changed += 1;
            }
        }
        trace_graph!(self,
            "affine transform target=#{target} resource={resource_id:?} layers={layer_ids:?} changed={changed} x={x:?} y={y:?} scale={scale_x:?}x{scale_y:?} opacity={opacity:?} raw={source:?}"
        );
    }

    fn apply_graph_rect_transition(&mut self, args: &[ethornell_vm::Value]) {
        let popped = args.iter().map(value_to_i32).collect::<Vec<_>>();
        let mut source = popped.clone();
        source.reverse();
        if let Some(destination) = source.get(2).copied().filter(|id| {
            self.resolve_resource_key(*id)
                .is_some_and(is_scene_transition_resource)
        }) {
            self.pending_transition_destination = Some(destination);
        }
        trace_graph!(
            self,
            "bitmap rect operation source={source:?} destination={:?}",
            self.pending_transition_destination
        );
    }

    fn tick_timelines(&mut self) {
        let mut finished = self.timelines.tick();
        finished.extend(self.graph_animations.tick());
        for handle in finished {
            if self.graph_special_watches.contains(&handle)
                && !self.graph_special_events.contains(&handle)
            {
                self.graph_special_events.push_back(handle);
                trace_graph!(self, "special graph event queued handle=#{handle}");
            }
        }
        for event in self
            .layer_animations
            .tick(&mut self.graph_layers, &mut self.graph_surfaces)
        {
            if self.debug_graph {
                match event {
                    animation::LayerAnimationEvent::Finished { layer_id, property } => {
                        trace_graph!(
                            self,
                            "layer animation finished layer={layer_id} property={property:?}"
                        );
                    }
                }
            }
        }
        if self.animation_queue_remaining > 0 {
            self.animation_queue_remaining -= 1;
            trace_graph!(
                self,
                "animation queue frame tick remaining={}",
                self.animation_queue_remaining
            );
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
        trace_graph!(
            self,
            "BCS playback clear scene sprites count={}",
            layers.len()
        );
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
        trace_graph!(
            self,
            "BCS playback hide sprite slot={slot:?} fade={fade_frames}"
        );
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
            trace_graph!(self,
                "BCS playback transform skipped missing slot={slot} x={x:?} y={y:?} opacity={opacity:?}"
            );
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
            trace_graph!(self,
                "BCS playback transform layer={layer_id} slot={slot} x={x:?} y={y:?} opacity={opacity:?} frames={frames}"
            );
        }
    }

    fn tick_scenario(&mut self) {
        self.maybe_inject_scenario_mode_input();
        if self.handle_message_control_input() {
            return;
        }
        // Native BCS execution owns its input waits in the VM. The latch below
        // belongs only to the optional ScenarioPlayback shadow interpreter;
        // consuming native clicks here prevents the BP message scripts from
        // ever observing them.
        if self.scenario_playback.is_none() {
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
                self.pending_input_consumed = false;
                self.mouse_pressed = false;
                return;
            } else {
                self.scenario_input_latched = true;
            }
        }

        let Some(playback) = self.scenario_playback.as_mut() else {
            unreachable!("scenario playback was checked above");
        };
        let (action, input_consumed) = playback.tick(self.scenario_input_latched);
        if input_consumed {
            self.scenario_input_latched = false;
            self.pending_click = None;
            self.pending_object_state = None;
            self.pending_input_state = None;
            self.pending_input_descriptor = None;
            self.pending_input_consumed = false;
            self.mouse_pressed = false;
            self.trace_graph("BCS playback consumed latched input");
        }
        let Some(action) = action else {
            return;
        };

        match action {
            ScenarioAction::Sound { file } => {
                self.queue_scenario_sound(&file);
                trace_graph!(self, "BCS playback sound {file}");
            }
            ScenarioAction::Bgm { file } => {
                self.queue_scenario_sound(&file);
                trace_graph!(self, "BCS playback bgm {file}");
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
                trace_graph!(self,
                    "BCS playback sprite {file} slot={slot:?} x={x:?} z={z:?} opacity={opacity:?} body_layer={body_layer:?}"
                );
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
            ScenarioAction::LoadScript {
                file,
                symbol,
                transfer,
            } => {
                tracing::info!(file, ?symbol, ?transfer, "BCS scenario transfer requested");
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
            self.pending_input_descriptor = Some(INPUT_DESCRIPTOR_MOUSE_LEFT);
            self.pending_input_consumed = false;
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
            self.pending_input_descriptor = Some(INPUT_DESCRIPTOR_MOUSE_LEFT);
            self.pending_input_consumed = false;
            self.trace_graph("scenario auto mode injected input");
        }
    }

    fn finish_frame_input(&mut self) {
        let had_input_state = self.pending_input_state.is_some();
        if self.pending_input_state.is_some() && self.auto_title_release_after_state {
            self.mouse_pressed = false;
            self.pending_input_state = Some(0x1000_0006);
            self.pending_input_descriptor = Some(INPUT_DESCRIPTOR_MOUSE_LEFT);
            self.pending_input_consumed = false;
            self.trace_graph("auto title click queued release input");
        } else if self.pending_input_consumed {
            self.pending_input_state = None;
            self.pending_input_descriptor = None;
            self.pending_input_consumed = false;
        } else if had_input_state {
            // Key/button state belongs to one engine tick. Completed clicks use
            // `pending_click`, the separate object-event queue, when a script
            // needs to consume the release later.
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

    fn resolve_graph_resource(&self, id: i32) -> Option<&RuntimeGraphResource> {
        self.graph_resources.get(&id).or_else(|| {
            self.graph_bindings
                .get(&id)
                .and_then(|target| self.graph_resources.get(target))
        })
    }

    fn copy_graph_backing(&mut self, source: i32, destination: i32) {
        if source == 0 || destination == 0 || source == destination {
            return;
        }
        if let Some(resource) = self.resolve_graph_resource(source).cloned() {
            self.graph_resources.insert(destination, resource);
        }
        if let Some(source_surface) = self.graph_surfaces.get(&source).cloned() {
            let destination_surface = self.graph_surfaces.entry(destination).or_insert_with(|| {
                RuntimeSurface::bitmap(destination, source_surface.width, source_surface.height)
            });
            destination_surface.width = source_surface.width;
            destination_surface.height = source_surface.height;
            destination_surface.viewport_width = source_surface.viewport_width;
            destination_surface.viewport_height = source_surface.viewport_height;
            destination_surface.resource_id = source_surface.resource_id;
        }
        self.graph_bindings.insert(destination, source);
        trace_graph!(self, "copy graph backing #{source} -> #{destination}");
    }

    fn resolve_resource_key(&self, id: i32) -> Option<&str> {
        Some(self.resolve_graph_resource(id)?.key.as_str())
    }

    fn resource_image_region(&self, id: i32) -> Option<(&str, RuntimeClipRect)> {
        let resource = self.resolve_graph_resource(id)?;
        let image = self.graph_images.get(&resource.key)?;
        let full = RuntimeClipRect {
            x: 0.0,
            y: 0.0,
            width: image.width as f32,
            height: image.height as f32,
        };
        let requested = resource.source_rect.unwrap_or(full);
        let x = requested.x.clamp(0.0, full.width);
        let y = requested.y.clamp(0.0, full.height);
        let width = requested.width.max(0.0).min(full.width - x);
        let height = requested.height.max(0.0).min(full.height - y);
        Some((
            resource.key.as_str(),
            RuntimeClipRect {
                x,
                y,
                width,
                height,
            },
        ))
    }

    fn replace_surface_control_layers(
        &mut self,
        surface: i32,
        descriptor: &ethornell_vm::GraphInputDescriptor,
    ) -> usize {
        if self.surface_controls.is_unchanged(surface, descriptor) {
            return self.surface_controls.layer_count(surface);
        }
        for layer in self.surface_controls.remove(surface) {
            self.graph_layers.remove(&layer);
        }

        let base_z = surface.saturating_add(1);
        let controls = descriptor
            .regions
            .iter()
            .enumerate()
            .filter_map(|(ordinal, region)| {
                if region.enabled_depth == 0 {
                    return None;
                }
                let selected = region.selected && region.selected_resource >= 0;
                let preferred = if selected {
                    region.selected_resource
                } else {
                    region.normal_resource
                };
                let resource_id = self
                    .resource_image_region(preferred)
                    .map(|_| preferred)
                    .or_else(|| {
                        self.resource_image_region(region.normal_resource)
                            .map(|_| region.normal_resource)
                    })?;
                let (key, source) = self.resource_image_region(resource_id)?;
                let width = if region.width > 0 {
                    region.width as f32
                } else {
                    source.width
                };
                let height = if region.height > 0 {
                    region.height as f32
                } else {
                    source.height
                };
                Some((
                    ordinal,
                    resource_id,
                    key.to_string(),
                    source,
                    width,
                    height,
                    region,
                ))
            })
            .collect::<Vec<_>>();

        let mut layer_ids = BTreeSet::new();
        for (ordinal, resource_id, key, source, width, height, region) in controls {
            let layer_id = self.alloc_node();
            self.graph_layers.insert(
                layer_id,
                RuntimeGraphLayer {
                    hit_id: ordinal as i32,
                    owner_object: None,
                    key: key.clone(),
                    target_surface: Some(surface),
                    x: region.x as f32,
                    y: region.y as f32,
                    width,
                    height,
                    src_x: source.x,
                    src_y: source.y,
                    opacity: 1.0,
                    z: base_z.saturating_add(native_surface_control_depth(region)),
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
            layer_ids.insert(layer_id);
            trace_graph!(
                self,
                "surface #{surface} control layer #{layer_id} group={} index={} selected={} resource=#{resource_id} {key} rect=({},{} {}x{}) z={}",
                region.group,
                region.index,
                region.selected,
                region.x,
                region.y,
                width,
                height,
                base_z.saturating_add(native_surface_control_depth(region))
            );
        }
        let count = layer_ids.len();
        for stale in self
            .surface_controls
            .replace(surface, descriptor.clone(), layer_ids)
        {
            self.graph_layers.remove(&stale);
        }
        count
    }

    fn remove_surface_control_layers(&mut self, surface: i32) {
        for layer in self.surface_controls.remove(surface) {
            self.graph_layers.remove(&layer);
        }
    }

    fn composite_graph_bitmap(
        &mut self,
        destination: i32,
        source: i32,
        x: i32,
        y: i32,
        mode: i32,
        alpha_parameter: i32,
    ) -> Option<String> {
        let (source_key, source_region) = self
            .resource_image_region(source)
            .map(|(key, region)| (key.to_string(), region))?;
        let source_image = crop_decoded_image(self.graph_images.get(&source_key)?, source_region);
        let composite_key = format!("runtime:bitmap:{destination}");

        let mut destination_image = if mode == 128 && alpha_parameter >= 256 && x == 0 && y == 0 {
            DecodedImage {
                width: source_image.width,
                height: source_image.height,
                rgba: vec![0; source_image.rgba.len()],
            }
        } else if let Some(image) = self.graph_images.get(&composite_key) {
            image.clone()
        } else if let Some((key, region)) = self
            .resource_image_region(destination)
            .map(|(key, region)| (key.to_string(), region))
        {
            crop_decoded_image(self.graph_images.get(&key)?, region)
        } else if let Some(surface) = self.graph_surfaces.get(&destination) {
            let width = surface.width.max(1.0) as u32;
            let height = surface.height.max(1.0) as u32;
            DecodedImage {
                width,
                height,
                rgba: vec![0; width as usize * height as usize * 4],
            }
        } else {
            let width = (x.max(0) as u32).saturating_add(source_image.width).max(1);
            let height = (y.max(0) as u32).saturating_add(source_image.height).max(1);
            DecodedImage {
                width,
                height,
                rgba: vec![0; width as usize * height as usize * 4],
            }
        };

        blit_decoded_image_parameter(
            &mut destination_image,
            &source_image,
            x,
            y,
            mode,
            alpha_parameter,
        );
        self.graph_images
            .insert(composite_key.clone(), destination_image);
        self.graph_resources.insert(
            destination,
            RuntimeGraphResource::whole(composite_key.clone()),
        );
        if let Some(surface) = self.graph_surfaces.get_mut(&destination) {
            surface.resource_id = Some(destination);
        }
        Some(composite_key)
    }

    fn render_graph_object_to_bitmap(&mut self, object: i32, destination: i32) -> bool {
        let layers = self
            .graph_target_layers(object)
            .into_iter()
            .filter_map(|layer_id| self.graph_layers.get(&layer_id).cloned())
            .collect::<Vec<_>>();
        if layers.is_empty() {
            return false;
        }

        let destination_size = self
            .graph_surfaces
            .get(&destination)
            .map(|surface| (surface.width as u32, surface.height as u32))
            .or_else(|| {
                self.resource_image_region(destination)
                    .map(|(key, region)| {
                        self.graph_images
                            .get(key)
                            .map(|image| {
                                (
                                    region.width.min(image.width as f32) as u32,
                                    region.height.min(image.height as f32) as u32,
                                )
                            })
                            .unwrap_or_default()
                    })
            })
            .filter(|(width, height)| *width > 0 && *height > 0);
        let Some((width, height)) = destination_size else {
            return false;
        };

        let min_x = layers
            .iter()
            .map(|layer| layer.x)
            .fold(f32::INFINITY, f32::min);
        let min_y = layers
            .iter()
            .map(|layer| layer.y)
            .fold(f32::INFINITY, f32::min);
        let mut output = DecodedImage {
            width,
            height,
            rgba: vec![0; width as usize * height as usize * 4],
        };
        let mut rendered = 0usize;
        for layer in layers {
            let Some(image) = self.graph_images.get(&layer.key) else {
                continue;
            };
            let source = crop_decoded_image(
                image,
                RuntimeClipRect {
                    x: layer.src_x,
                    y: layer.src_y,
                    width: layer.width,
                    height: layer.height,
                },
            );
            let x = (layer.x - min_x).round() as i32;
            let y = (layer.y - min_y).round() as i32;
            blit_decoded_image(&mut output, &source, x, y, 128);
            rendered += 1;
        }
        if rendered == 0 {
            return false;
        }

        let key = format!("runtime:render-target:{destination}");
        self.store_graph_image(key.clone(), output);
        self.graph_resources
            .insert(destination, RuntimeGraphResource::whole(key));
        if let Some(surface) = self.graph_surfaces.get_mut(&destination) {
            surface.resource_id = Some(destination);
        }
        trace_graph!(
            self,
            "render object #{object} into bitmap #{destination} layers={rendered}"
        );
        true
    }

    fn update_transition_node_image(&mut self, node: i32) -> Option<String> {
        let transition = *self.graph_transition_nodes.get(&node)?;
        let (primary_key, primary_region) = self
            .resource_image_region(transition.primary_resource)
            .map(|(key, region)| (key.to_string(), region))?;
        let (secondary_key, secondary_region) = self
            .resource_image_region(transition.secondary_resource)
            .map(|(key, region)| (key.to_string(), region))?;
        let primary = crop_decoded_image(self.graph_images.get(&primary_key)?, primary_region);
        let secondary =
            crop_decoded_image(self.graph_images.get(&secondary_key)?, secondary_region);
        let image = crossfade_decoded_images(&primary, &secondary, transition.alpha_parameter);
        let width = image.width as f32;
        let height = image.height as f32;
        let key = format!("runtime:transition:{node}");
        self.store_graph_image(key.clone(), image);
        if let Some(layer) = self.graph_layers.get_mut(&node) {
            layer.key = key.clone();
            layer.width = width;
            layer.height = height;
            layer.src_x = 0.0;
            layer.src_y = 0.0;
        }
        Some(key)
    }

    fn graph_handle_exists(&self, id: i32) -> bool {
        id != 0
            && (self.graph_resources.contains_key(&id)
                || self.graph_bindings.contains_key(&id)
                || self.graph_layers.contains_key(&id)
                || self.graph_object_layers.contains_key(&id)
                || self.graph_object_enabled.contains_key(&id)
                || self.graph_surfaces.contains_key(&id)
                || self.text_nodes.contains_key(&id)
                || self.timelines.contains(id)
                || self.user_controls.contains_key(&id))
    }

    fn ensure_transform_layer(&mut self, layer_id: i32, resource_id: i32) -> bool {
        if self.graph_layers.contains_key(&layer_id) {
            return true;
        }
        let Some((key, region)) = self
            .resource_image_region(resource_id)
            .map(|(key, region)| (key.to_string(), region))
        else {
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
                width: region.width,
                height: region.height,
                src_x: region.x,
                src_y: region.y,
                opacity: 1.0,
                z,
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
        trace_graph!(
            self,
            "create transform layer #{layer_id} res=#{resource_id} {key} z={z}"
        );
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
        trace_graph!(self, "cleared {} title graph layers", title_layers.len());
    }

    fn has_title_child_render_context(&self) -> bool {
        self.title_child_program_active
    }

    fn has_native_fullscreen_modal(&self) -> bool {
        let screen_width = self.screen_width.max(1) as f32;
        let screen_height = self.screen_height.max(1) as f32;
        self.graph_surfaces.iter().any(|(&id, surface)| {
            let has_active_input = self.graph_input_objects.iter().any(|(&object, input)| {
                input.layer == id
                    && !input.descriptor.regions.is_empty()
                    && self
                        .graph_object_enabled
                        .get(&object)
                        .copied()
                        .unwrap_or(true)
            });
            self.surface_display_chain_visible(id, surface)
                && Some(id) != self.native_message_surface_target
                && surface.viewport_width >= screen_width
                && surface.viewport_height >= screen_height
                && self.surface_controls.layer_count(id) != 0
                && has_active_input
        })
    }

    fn graph_draw_items(&self) -> Vec<RuntimeGraphDrawItem> {
        let mut items = Vec::new();
        self.push_title_surface_composite(&mut items);
        self.push_surface_draw_items(&mut items);
        for (layer_id, layer) in &self.graph_layers {
            if !self.should_draw_graph_layer(*layer_id, layer) {
                continue;
            }
            let object_opacity = layer
                .owner_object
                .filter(|object| *object != *layer_id)
                .and_then(|object| self.graph_object_properties.get(&object))
                .map(RuntimeGraphObjectProperties::opacity)
                .unwrap_or(1.0);
            let node_opacity = if self.graph_transition_nodes.contains_key(layer_id) {
                1.0
            } else {
                self.graph_object_properties
                    .get(layer_id)
                    .map(RuntimeGraphObjectProperties::opacity)
                    .unwrap_or(1.0)
            };
            let opacity = layer.opacity * node_opacity * object_opacity;
            let opacity = layer
                .target_surface
                .map(|surface| self.surface_display_opacity(surface))
                .unwrap_or(1.0)
                * opacity;
            if !layer.enabled || opacity <= 0.001 {
                continue;
            }
            if layer.target_surface.is_some_and(|surface| {
                self.graph_surfaces
                    .get(&surface)
                    .is_some_and(|record| !self.surface_display_chain_visible(surface, record))
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
                x: layer.screen_x(&self.graph_surfaces)
                    + layer.transform_x
                    + self.graph_global_offset.0,
                y: layer.screen_y(&self.graph_surfaces)
                    + layer.transform_y
                    + self.graph_global_offset.1,
                width,
                height,
                src_x,
                src_y,
                src_width,
                src_height,
                opacity,
                rotation_degrees: layer.rotation_degrees,
                clip: layer.clip,
                z: layer.screen_z(),
                hit_id: layer.hit_id,
            });
        }
        // Native CObjectManager inserts equal-depth objects after existing
        // entries, then consumes the display chain back-to-front.
        items.sort_by_key(native_draw_order);
        items
    }

    fn push_surface_draw_items(&self, items: &mut Vec<RuntimeGraphDrawItem>) {
        for surface in self.graph_surfaces.values() {
            let flattened_input_surface = self.surface_has_active_input(surface.id);
            if !surface.display_attached
                || (!self.has_title_child_render_context() && !flattened_input_surface)
                || !self.surface_display_chain_visible(surface.id, surface)
            {
                continue;
            }
            let Some(resource_id) = surface.resource_id else {
                continue;
            };
            let Some((key, region)) = self.resource_image_region(resource_id) else {
                continue;
            };
            if is_title_layer(key) {
                continue;
            }
            if key.starts_with("sysgrp.arc:SGMsgWnd") {
                continue;
            }
            let viewport_x = surface.viewport_x.max(0.0);
            let viewport_y = surface.viewport_y.max(0.0);
            let width = surface
                .viewport_width
                .min(region.width - viewport_x)
                .max(0.0);
            let height = surface
                .viewport_height
                .min(region.height - viewport_y)
                .max(0.0);
            if width <= 0.0 || height <= 0.0 {
                continue;
            }
            items.push(RuntimeGraphDrawItem {
                key: key.to_string(),
                x: surface.x,
                y: surface.y,
                width,
                height,
                src_x: region.x + viewport_x,
                src_y: region.y + viewport_y,
                src_width: width,
                src_height: height,
                opacity: surface.opacity,
                rotation_degrees: 0.0,
                clip: None,
                z: surface.z,
                hit_id: 0,
            });
        }
    }

    fn surface_has_active_input(&self, surface: i32) -> bool {
        self.surface_controls.layer_count(surface) != 0
            && self.graph_input_objects.iter().any(|(&object, input)| {
                input.layer == surface
                    && !input.descriptor.regions.is_empty()
                    && self
                        .graph_object_enabled
                        .get(&object)
                        .copied()
                        .unwrap_or(true)
            })
    }

    fn surface_display_chain_visible(&self, surface: i32, record: &RuntimeSurface) -> bool {
        (record.enabled || self.surface_has_active_input(surface))
            && self.surface_display_opacity(surface) > 0.001
    }

    fn surface_display_opacity(&self, surface: i32) -> f32 {
        let mut opacity = 1.0;
        let mut current = Some(surface);
        let mut depth = 0;
        while let Some(id) = current {
            let Some(record) = self.graph_surfaces.get(&id) else {
                break;
            };
            opacity *= record.opacity;
            current = record.parent_surface;
            depth += 1;
            if depth >= 32 {
                break;
            }
        }
        opacity.clamp(0.0, 1.0)
    }

    fn should_draw_graph_layer(&self, layer_id: i32, layer: &RuntimeGraphLayer) -> bool {
        let key = layer.key.as_str();
        if self.title_ui_departed && is_title_layer(key) {
            return false;
        }
        if self.has_title_child_render_context() && is_title_layer(key) {
            return false;
        }
        if is_title_layer(key) && !is_title_base_layer(key) && layer.target_surface.is_none() {
            return false;
        }
        if self.title_ui_active
            && !self.title_child_program_active
            && !self.has_title_child_render_context()
            && !is_title_layer(key)
        {
            return false;
        }
        if is_message_control_layer(key) {
            return !self.has_native_fullscreen_modal()
                && self.should_draw_message_control_layer(layer);
        }
        if key == "sysgrp.arc:SGMsgWnd800000" {
            return !self.has_native_fullscreen_modal()
                && layer_id == text::MESSAGE_WINDOW_LAYER_ID
                && self.has_active_message_window();
        }
        if key == "sysgrp.arc:SGMsgWnd700000" {
            return !self.has_native_fullscreen_modal()
                && layer_id == text::MESSAGE_NAME_WINDOW_LAYER_ID
                && self.has_active_message_window();
        }
        if !self.title_ui_active && !self.scenario_bootstrapped && is_boot_suppressed_layer(key) {
            return false;
        }
        true
    }

    fn should_draw_text_node(&self, node: &RuntimeTextNode) -> bool {
        if !node.enabled || !node.screen_attached || node.text.is_empty() {
            return false;
        }
        if node.z == MESSAGE_TEXT_Z || node.z == MESSAGE_NAME_TEXT_Z {
            return !self.has_native_fullscreen_modal() && self.has_active_message_window();
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
        let title_surface_z = self
            .graph_layers
            .values()
            .filter(|layer| {
                layer.target_surface.is_some()
                    && is_title_layer(&layer.key)
                    && !is_title_base_layer(&layer.key)
            })
            .filter_map(|layer| layer.target_surface)
            .filter_map(|surface| self.graph_surfaces.get(&surface).map(|record| record.z))
            .max()
            .unwrap_or(2);
        for (key, z) in [
            ("sysgrp.arc:SGTitle990000", 0),
            ("sysgrp.arc:SGTitle990200", 1),
            ("sysgrp.arc:SGTitle990300", title_surface_z + 1),
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
        let has_native_control_layers = self.graph_layers.values().any(|layer| {
            layer.target_surface.is_some()
                && layer.enabled
                && layer.opacity > 0.001
                && is_title_layer(&layer.key)
                && !is_title_base_layer(&layer.key)
        });
        if has_native_control_layers {
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
            let hovered = hovered_title_control == Some(control.id);
            let resource_id = if hovered && control.selected_resource >= 0 {
                control.selected_resource
            } else {
                control.normal_resource
            };
            let resolved = self
                .resource_image_region(resource_id)
                .or_else(|| self.resource_image_region(control.normal_resource))
                .filter(|(_, source)| source.width > 0.0 && source.height > 0.0)
                .map(|(key, source)| (key.to_string(), source, control.x, control.y));
            let (key, source, draw_x, draw_y) = resolved.unwrap_or_else(|| {
                let key = if hovered && self.mouse_pressed {
                    "sysgrp.arc:SGTitle000003"
                } else if hovered {
                    "sysgrp.arc:SGTitle000001"
                } else {
                    "sysgrp.arc:SGTitle000000"
                };
                (
                    key.to_string(),
                    RuntimeClipRect {
                        x: control.x + 8.0,
                        y: control.y + 6.0,
                        width: (control.width - 16.0).max(0.0),
                        height: (control.height - 10.0).max(0.0),
                    },
                    control.x + 8.0,
                    control.y + 6.0,
                )
            });
            if source.width <= 0.0 || source.height <= 0.0 {
                continue;
            };
            items.push(RuntimeGraphDrawItem {
                key,
                x: draw_x,
                y: draw_y,
                width: source.width,
                height: source.height,
                src_x: source.x,
                src_y: source.y,
                src_width: source.width,
                src_height: source.height,
                opacity: 1.0,
                rotation_degrees: 0.0,
                clip: None,
                z: 4,
                hit_id: control.id,
            });
        }
    }

    fn trace_render_snapshot(&self, frame: usize, report: Option<&ethornell_vm::VmRunReport>) {
        if !self.debug_graph && !self.trace_render_tree {
            return;
        }
        let frame_selector = std::env::var("TRACE_RENDER_TREE_FRAME").ok();
        if !debug_trace::frame_selected(frame, frame_selector.as_deref()) {
            return;
        }
        let draw_items = self.graph_draw_items();
        let full_trace = std::env::var("TRACE_RENDER_TREE")
            .is_ok_and(|value| value.eq_ignore_ascii_case("full"));
        if full_trace {
            for (id, layer) in &self.graph_layers {
                let draw = self.should_draw_graph_layer(*id, layer)
                    && layer.enabled
                    && layer.opacity > 0.001;
                tracing::info!(
                    target: "graph_tree",
                    frame,
                    layer = *id,
                    hit_id = layer.hit_id,
                    owner = ?layer.owner_object,
                    parent = ?layer.target_surface,
                    key = layer.key,
                    x = layer.x,
                    y = layer.y,
                    width = layer.width,
                    height = layer.height,
                    src_x = layer.src_x,
                    src_y = layer.src_y,
                    opacity = layer.opacity,
                    z = layer.z,
                    enabled = layer.enabled,
                    draw,
                    "render layer"
                );
            }
            for (ordinal, item) in draw_items.iter().enumerate() {
                tracing::info!(
                    target: "graph_tree",
                    frame,
                    ordinal,
                    hit_id = item.hit_id,
                    key = item.key,
                    x = item.x,
                    y = item.y,
                    width = item.width,
                    height = item.height,
                    src_x = item.src_x,
                    src_y = item.src_y,
                    src_width = item.src_width,
                    src_height = item.src_height,
                    opacity = item.opacity,
                    z = item.z,
                    "render draw item"
                );
            }
        }
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
        let layer_relation_preview = self
            .graph_layers
            .iter()
            .filter(|(_, layer)| layer.target_surface.is_some())
            .take(16)
            .map(|(id, layer)| {
                format!(
                    "#{id} parent={:?} local=({:.1},{:.1}) key={}",
                    layer.target_surface, layer.x, layer.y, layer.key
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
        let surface_preview = self
            .graph_surfaces
            .values()
            .filter(|surface| {
                surface.display_attached
                    || self
                        .graph_layers
                        .values()
                        .any(|layer| layer.target_surface == Some(surface.id))
                    || self
                        .text_nodes
                        .values()
                        .any(|node| node.target_surface == Some(surface.id))
            })
            .take(16)
            .map(|surface| {
                format!(
                    "#{} pos=({:.1},{:.1}) parent={:?} local=({:.1},{:.1}) viewport=({:.1},{:.1} {:.1}x{:.1})",
                    surface.id,
                    surface.x,
                    surface.y,
                    surface.parent_surface,
                    surface.local_x,
                    surface.local_y,
                    surface.viewport_x,
                    surface.viewport_y,
                    surface.viewport_width,
                    surface.viewport_height
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
            ?layer_relation_preview,
            ?text_preview,
            ?surface_preview,
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
    last_report: Option<ethornell_vm::VmRunReport>,
    tick_remainder_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct RuntimeFramePacing {
    vm_slices_per_frame: usize,
    bootstrap_vm_slices_per_frame: usize,
    continue_after_yield: bool,
}

impl RuntimeFramePacing {
    fn from_env() -> Self {
        Self {
            vm_slices_per_frame: std::env::var("ETHORNELL_VM_SLICES_PER_FRAME")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(1),
            bootstrap_vm_slices_per_frame: std::env::var("ETHORNELL_VM_BOOTSTRAP_SLICES_PER_FRAME")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(1),
            continue_after_yield: std::env::var("ETHORNELL_VM_CONTINUE_AFTER_YIELD")
                .ok()
                .map(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
                .unwrap_or(false),
        }
    }
}

struct RuntimeFrameReport {
    last_report: Option<ethornell_vm::VmRunReport>,
    total_steps: usize,
    slices_this_frame: usize,
    continue_setup_yield: bool,
    continue_input_yield: bool,
}

#[derive(Clone)]
struct CachedGraphTexture {
    handle: TextureHandle,
    revision: u64,
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
                max_steps: parse_usize_env("ETHORNELL_VM_MAX_STEPS").unwrap_or(250_000),
                trace,
                fail_on_stub,
                collect_diagnostics: trace || fail_on_stub,
            },
            last_report: None,
            tick_remainder_ms: 0,
        }
    }

    fn begin_frame(&mut self, elapsed_ms: u64) {
        let elapsed_ms = elapsed_ms.min(250);
        self.vm.advance_time_ms(elapsed_ms);
        self.api.advance_audio_clocks(elapsed_ms);
        self.tick_remainder_ms = self.tick_remainder_ms.saturating_add(elapsed_ms);
        let native_ticks = self.tick_remainder_ms / 16;
        self.tick_remainder_ms %= 16;
        for _ in 0..native_ticks {
            self.api.tick_timelines();
            self.api.tick_text();
            self.api.tick_scenario();
            self.api.maybe_auto_click_title();
            self.api.maybe_auto_click_user();
        }
    }

    fn tick_vm(&mut self) -> ethornell_vm::VmRunReport {
        if self.api.animation_queue_remaining > 0 {
            if let Some(mut report) = self.last_report.clone() {
                report.steps = 0;
                report.stop_reason = ethornell_vm::VmStopReason::WaitingForAnimation;
                return report;
            }
        }
        let report = self.vm.run_loaded(&mut self.api, &self.options);
        self.last_report = Some(report.clone());
        report
    }

    fn run_frame(&mut self, pacing: RuntimeFramePacing, elapsed_ms: u64) -> RuntimeFrameReport {
        self.begin_frame(elapsed_ms);
        let mut last_report = None;
        let mut total_steps = 0usize;
        let mut last_continue_setup_yield = false;
        let mut last_continue_input_yield = false;
        let slices_this_frame = if self.api.should_continue_vm_after_yield() {
            pacing.bootstrap_vm_slices_per_frame
        } else {
            pacing.vm_slices_per_frame
        };
        for _ in 0..slices_this_frame {
            let report = self.tick_vm();
            total_steps += report.steps;
            let yielded = report.stop_reason == ethornell_vm::VmStopReason::WaitingForAnimation;
            let continue_setup_yield = self.api.should_continue_vm_after_yield();
            let continue_input_yield = self.api.should_continue_input_yield();
            last_continue_setup_yield = continue_setup_yield;
            last_continue_input_yield = continue_input_yield;
            last_report = Some(report);
            if yielded
                && !pacing.continue_after_yield
                && !continue_setup_yield
                && !continue_input_yield
            {
                break;
            }
            if !yielded {
                break;
            }
        }
        self.api.finish_frame_input();
        RuntimeFrameReport {
            last_report,
            total_steps,
            slices_this_frame,
            continue_setup_yield: last_continue_setup_yield,
            continue_input_yield: last_continue_input_yield,
        }
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

fn read_vm_u32(memory: &[u8], address: usize) -> Option<u32> {
    let bytes: [u8; 4] = memory
        .get(address..address.checked_add(4)?)?
        .try_into()
        .ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn parse_env_point(x_key: &str, y_key: &str) -> Option<(f32, f32)> {
    let x = std::env::var(x_key).ok()?.parse().ok()?;
    let y = std::env::var(y_key).ok()?.parse().ok()?;
    Some((x, y))
}

fn graph_motion_coord(raw: i32) -> f32 {
    if raw.unsigned_abs() >= 32_768 {
        fixed_16_to_f32(raw)
    } else {
        raw as f32
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

fn input_state_for_descriptor(
    pending_state: Option<i32>,
    active_descriptor: Option<i32>,
    requested_descriptor: i32,
) -> i32 {
    match (pending_state, active_descriptor) {
        (Some(state), Some(active))
            if active == requested_descriptor || requested_descriptor == 0 =>
        {
            state
        }
        (Some(state), None) if requested_descriptor == INPUT_DESCRIPTOR_ENTER => state,
        _ => 0,
    }
}

fn input_class_state_for_descriptor(
    pending_state: Option<i32>,
    active_descriptor: Option<i32>,
) -> i32 {
    if pending_state.is_none() {
        return 0;
    }
    match active_descriptor {
        Some(INPUT_DESCRIPTOR_MOUSE_LEFT) => 0x0000_0001,
        Some(INPUT_DESCRIPTOR_ENTER) => 0x0000_0100,
        Some(INPUT_DESCRIPTOR_UP) => 0x0000_1000,
        Some(INPUT_DESCRIPTOR_DOWN) => 0x0000_2000,
        Some(INPUT_DESCRIPTOR_LEFT) => 0x0000_4000,
        Some(INPUT_DESCRIPTOR_RIGHT) => 0x0000_8000,
        _ => 0,
    }
}

#[cfg(test)]
mod input_tests {
    use super::{
        input_class_state_for_descriptor, input_state_for_descriptor, INPUT_DESCRIPTOR_DOWN,
        INPUT_DESCRIPTOR_ENTER, INPUT_DESCRIPTOR_LEFT, INPUT_DESCRIPTOR_MOUSE_LEFT,
        INPUT_DESCRIPTOR_RIGHT, INPUT_DESCRIPTOR_UP,
    };
    use ethornell_vm::{GraphApi, SoundApi, SysApi, Value};

    #[test]
    fn mouse_input_does_not_activate_keyboard_or_system_classes() {
        let pressed = Some(0x1000_0002);
        let mouse = Some(INPUT_DESCRIPTOR_MOUSE_LEFT);

        assert_eq!(input_state_for_descriptor(pressed, mouse, 0), 0x1000_0002);
        assert_eq!(
            input_state_for_descriptor(pressed, mouse, INPUT_DESCRIPTOR_MOUSE_LEFT),
            0x1000_0002
        );
        assert_eq!(
            input_state_for_descriptor(pressed, mouse, INPUT_DESCRIPTOR_ENTER),
            0
        );
        for descriptor in [2, 4, 512, 4160] {
            assert_eq!(input_state_for_descriptor(pressed, mouse, descriptor), 0);
        }
    }

    #[test]
    fn native_input_class_bits_match_keyboard_and_mouse_groups() {
        let event = Some(0x1000_0002);
        assert_eq!(
            input_class_state_for_descriptor(event, Some(INPUT_DESCRIPTOR_MOUSE_LEFT)),
            1
        );
        assert_eq!(
            input_class_state_for_descriptor(event, Some(INPUT_DESCRIPTOR_ENTER)),
            0x100
        );
        assert_eq!(
            input_class_state_for_descriptor(event, Some(INPUT_DESCRIPTOR_UP)),
            0x1000
        );
        assert_eq!(
            input_class_state_for_descriptor(event, Some(INPUT_DESCRIPTOR_DOWN)),
            0x2000
        );
        assert_eq!(
            input_class_state_for_descriptor(event, Some(INPUT_DESCRIPTOR_LEFT)),
            0x4000
        );
        assert_eq!(
            input_class_state_for_descriptor(event, Some(INPUT_DESCRIPTOR_RIGHT)),
            0x8000
        );
        assert_eq!(
            input_class_state_for_descriptor(None, Some(INPUT_DESCRIPTOR_MOUSE_LEFT)),
            0
        );
    }

    #[test]
    fn windows_file_patterns_are_case_insensitive() {
        assert!(super::windows_wildcard_matches("save*.dat", "SAVE012.DAT"));
        assert!(super::windows_wildcard_matches("??.sav", "01.SAV"));
        assert!(!super::windows_wildcard_matches("??.sav", "001.SAV"));
    }

    #[test]
    fn report_registered_native_calls_not_owned_by_the_app_layer() {
        let manager =
            ethornell_archive::ResourceManager::open_game(env!("CARGO_MANIFEST_DIR")).unwrap();
        let mut unowned = Vec::new();
        for group in [0x80, 0x81, 0x90, 0x91, 0x92, 0xA0, 0xB0, 0xC0] {
            for id in 0..=u8::MAX {
                let Some(abi) = ethornell_script::native_abi::lookup(group, u16::from(id)) else {
                    continue;
                };
                let mut api = super::RuntimeTraceApi::new(manager.clone());
                let mut stack = vec![Value::Int(0); abi.argc];
                let owned = match group {
                    0x80 | 0x81 => {
                        let _ = SysApi::call_sys(&mut api, group, u16::from(id), &mut stack);
                        !SysApi::take_runtime_stub(&mut api)
                    }
                    0x90 | 0x91 | 0x92 => {
                        let _ = GraphApi::call_graph(&mut api, group, u16::from(id), &mut stack);
                        !SysApi::take_runtime_stub(&mut api)
                    }
                    0xA0 => {
                        let _ = SoundApi::call_sound(&mut api, group, u16::from(id), &mut stack);
                        !SysApi::take_runtime_stub(&mut api)
                    }
                    0xB0 | 0xC0 => SoundApi::call_user(
                        &mut api,
                        group,
                        u16::from(id),
                        &mut stack,
                    )
                    .is_ok_and(|result| result.is_some()),
                    _ => unreachable!(),
                };
                if !owned {
                    unowned.push((group, id));
                }
            }
        }
        assert!(
            unowned.iter().all(|&(group, id)| {
                ethornell_vm::native_ownership::owns_before_host_dispatch(group, u16::from(id))
            }),
            "registered calls have neither a VM nor app owner: {unowned:02X?}"
        );
        let recovered = [0x80, 0x81, 0x90, 0x91, 0x92, 0xA0, 0xB0, 0xC0]
            .into_iter()
            .flat_map(|group| (0..=u8::MAX).map(move |id| (group, id)))
            .filter(|&(group, id)| {
                ethornell_script::native_abi::lookup(group, u16::from(id)).is_some()
            })
            .count();
        assert_eq!(recovered, 695);
    }
}

#[derive(Debug)]
enum RuntimeInputEvent {
    MouseMove { x: f32, y: f32 },
    MousePress { x: f32, y: f32 },
    MouseRelease { x: f32, y: f32 },
    KeyPress { descriptor: i32 },
}

fn apply_runtime_input_event(api: &mut RuntimeTraceApi, event: RuntimeInputEvent) {
    if api.input_requires_focus && !api.window_focused {
        tracing::debug!(?event, "input ignored while the game window is not focused");
        return;
    }
    match event {
        RuntimeInputEvent::MouseMove { x, y } => {
            api.mouse_pos = Some((x, y));
        }
        RuntimeInputEvent::MousePress { x, y } => {
            let point = Some((x, y));
            api.mouse_pos = point;
            api.mouse_pressed = true;
            api.pending_object_state = point;
            api.pending_input_state = Some(0x1000_0002);
            api.pending_input_descriptor = Some(INPUT_DESCRIPTOR_MOUSE_LEFT);
            api.pending_input_consumed = false;
        }
        RuntimeInputEvent::MouseRelease { x, y } => {
            let point = Some((x, y));
            api.mouse_pos = point;
            api.mouse_pressed = false;
            api.pending_click = point;
            api.pending_click_age_frames = 0;
            api.pending_input_state = Some(0x1000_0006);
            api.pending_input_descriptor = Some(INPUT_DESCRIPTOR_MOUSE_LEFT);
            api.pending_input_consumed = false;
        }
        RuntimeInputEvent::KeyPress { descriptor } => {
            api.pending_input_state = Some(0x1000_0002);
            api.pending_input_descriptor = Some(descriptor);
            api.pending_input_consumed = false;
        }
    }
}

fn apply_headless_input_event(api: &mut RuntimeTraceApi, event: HeadlessInputEvent) {
    let runtime_event = match event {
        HeadlessInputEvent::MouseMove { x, y } => RuntimeInputEvent::MouseMove { x, y },
        HeadlessInputEvent::MousePress { x, y } => RuntimeInputEvent::MousePress { x, y },
        HeadlessInputEvent::MouseRelease { x, y } => RuntimeInputEvent::MouseRelease { x, y },
        HeadlessInputEvent::KeyPress { key } => {
            let Some(descriptor) = input_descriptor_for_headless_key(&key) else {
                tracing::warn!(key, "unsupported headless input key");
                return;
            };
            RuntimeInputEvent::KeyPress { descriptor }
        }
    };
    tracing::info!(event = ?runtime_event, "scripted runtime input");
    apply_runtime_input_event(api, runtime_event);
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
    fn take_runtime_stub(&mut self) -> bool {
        std::mem::take(&mut self.runtime_stubbed)
    }

    fn observe_dispatch(&mut self, group: u8, id: u16) {
        self.record_call(group, id);
    }

    fn register_graphic_resource(&mut self, namespace: i32, resource_id: i32, path: &str) -> bool {
        let resource = self.manager.find(path);
        let found = resource.is_some()
            || runtime_file_path(&self.manager, path)
                .map(|path| path.is_file())
                .unwrap_or(false);
        if let Some(entry) = resource {
            let archive = entry
                .archive_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<unknown>");
            self.graphic_resource_keys
                .insert((namespace, resource_id), format!("{archive}:{path}"));
        }
        tracing::info!(
            path,
            namespace,
            resource_id,
            found,
            "RegisterGraphicResource"
        );
        found
    }

    fn post_queued_event(&mut self, code: i32, parameter: i32) {
        self.queued_system_events.push_back([0, code, parameter]);
        if code == 0x7fff {
            self.quit_requested = true;
        }
        tracing::info!(code, parameter, "PostQueuedEvent");
    }

    fn poll_queued_event(&mut self) -> Option<[i32; 3]> {
        let event = self.queued_system_events.pop_front();
        tracing::debug!(?event, "PollQueuedEvent");
        event
    }

    fn load_file_bytes(&mut self, archive: &str, file: &str) -> Option<Vec<u8>> {
        let bytes = read_runtime_bytes(&self.manager, archive, file);
        let resolved_archive = find_runtime_resource(&self.manager, archive, file)
            .and_then(|entry| {
                entry
                    .archive_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| archive.to_string());
        tracing::info!(
            archive,
            resolved_archive,
            file,
            size = bytes.as_ref().map(Vec::len).unwrap_or_default(),
            found = bytes.is_some(),
            "LoadFileBytes"
        );
        if file.eq_ignore_ascii_case("main") {
            if let Some(bytes) = bytes.as_deref() {
                if self.title_ui_active && self.title_scenario_requested {
                    self.depart_title_ui_for_scenario(file);
                }
                self.scenario_bootstrapped = true;
                if std::env::var("ETHORNELL_SCENARIO_SHADOW").ok().as_deref() == Some("1") {
                    if self.title_ui_departed {
                        self.start_scenario_playback(&resolved_archive, file, bytes);
                    } else {
                        self.pending_scenario_bootstrap =
                            Some((resolved_archive.clone(), file.to_string(), bytes.to_vec()));
                        trace_graph!(self,
                        "BCS playback bootstrap deferred {resolved_archive}:{file} requested={archive}"
                    );
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

    fn read_user_file_bytes(&mut self, path: &str) -> Option<Vec<u8>> {
        let path = runtime_file_path(&self.manager, path)?;
        match std::fs::read(&path) {
            Ok(bytes) => {
                tracing::info!(path = %path.display(), size = bytes.len(), "ReadUserFileBytes");
                Some(bytes)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(path = %path.display(), "ReadUserFileBytes not found");
                None
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "ReadUserFileBytes failed");
                None
            }
        }
    }

    fn enumerate_user_files(
        &mut self,
        pattern: &str,
        recursive: bool,
        max_count: usize,
    ) -> Vec<String> {
        let normalized = pattern.replace('\\', "/");
        let pattern_path = Path::new(&normalized);
        let file_pattern = pattern_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("*");
        let parent = pattern_path.parent().unwrap_or_else(|| Path::new(""));
        let Some(root) = runtime_file_path(&self.manager, &parent.to_string_lossy()) else {
            return Vec::new();
        };
        let mut files = Vec::new();
        enumerate_matching_files(&root, &root, file_pattern, recursive, max_count, &mut files);
        files.sort_by_key(|name| name.to_ascii_lowercase());
        if max_count != 0 {
            files.truncate(max_count);
        }
        tracing::debug!(
            pattern,
            recursive,
            max_count,
            count = files.len(),
            "EnumerateFiles"
        );
        files
    }

    fn enumerate_user_directories(&mut self, pattern: &str, max_count: usize) -> Vec<String> {
        let directories = self.enumerate_native_paths(pattern, false, max_count, true);
        tracing::debug!(
            pattern,
            max_count,
            count = directories.len(),
            "EnumerateDirectories"
        );
        directories
    }

    fn take_dropped_file(&mut self) -> Option<String> {
        if !self.native_system.drag_drop_enabled {
            return None;
        }
        self.dropped_files.pop_front()
    }

    fn native_save_header(&mut self, slot: i32) -> Option<[u8; 64]> {
        self.native_system.save_headers.get(&slot).copied()
    }

    fn registered_object_value(&mut self, object: i32) -> Option<i32> {
        self.graph_input_objects
            .get(&object)
            .map(|input| input.registered_state)
    }

    fn host_user_name(&mut self) -> String {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default()
    }

    fn host_computer_name(&mut self) -> String {
        std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "localhost".into())
    }

    fn keyboard_state(&mut self) -> [u8; 256] {
        let mut state = [0u8; 256];
        if self.pending_input_state.unwrap_or_default() != 0 {
            if let Some(descriptor) = self.pending_input_descriptor {
                if let Ok(index) = usize::try_from(descriptor) {
                    if let Some(value) = state.get_mut(index) {
                        *value = 0x80;
                    }
                }
            }
        }
        state
    }

    fn pointer_position(&mut self, index: i32) -> Option<(i32, i32)> {
        (index == 0).then(|| {
            self.mouse_pos
                .map(|(x, y)| (x.round() as i32, y.round() as i32))
                .unwrap_or_default()
        })
    }

    fn runtime_command_line(&mut self) -> String {
        std::env::args().collect::<Vec<_>>().join(" ")
    }

    fn delete_file(&mut self, root: &str, file: &str) -> bool {
        let file_path = Path::new(file);
        let combined = if file_path.is_absolute() || is_empty_archive_arg(root.trim()) {
            file_path.to_path_buf()
        } else {
            Path::new(root).join(file_path)
        };
        let Some(path) = runtime_file_path(&self.manager, &combined.to_string_lossy()) else {
            return false;
        };
        let ok = std::fs::remove_file(&path).is_ok();
        if ok {
            if let Ok(relative) = path.strip_prefix(self.manager.archives().root.as_path()) {
                let key = runtime_file_cache_key("", &relative.to_string_lossy());
                self.file_exists_cache.remove(&key);
                self.file_size_cache.remove(&key);
            }
        }
        tracing::debug!(root, file, path = %path.display(), ok, "DeleteFile");
        ok
    }

    fn user_data_root(&mut self, kind: i32) -> Option<String> {
        let root = game_root_path(&self.manager);
        tracing::debug!(kind, root = %root.display(), "GetUserDataRoot");
        let mut root = root.to_string_lossy().into_owned();
        if !root.ends_with(['/', '\\']) {
            root.push(std::path::MAIN_SEPARATOR);
        }
        Some(root)
    }

    fn save_global_user_data(&mut self) -> bool {
        const GLOBAL_DATA_SIZE: usize = 0x10_0424;
        let mut bytes = vec![0u8; GLOBAL_DATA_SIZE];
        bytes[..16].copy_from_slice(b"BURIKO GDB 3.00\0");
        bytes[16..20].copy_from_slice(&(GLOBAL_DATA_SIZE as u32).to_le_bytes());
        bytes[28..32].copy_from_slice(&1024u32.to_le_bytes());
        self.write_file_bytes("BGI.gdb", &bytes)
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
        let packed_payload = descriptor.get(1).map(value_to_i32);
        let payload = if matches!(event, 0x1000_0006 | 0x1000_0007) {
            packed_payload
                .map(|value| value & 0xffff)
                .unwrap_or(self.last_hit_payload)
        } else {
            descriptor
                .get(2)
                .map(value_to_i32)
                .filter(|value| *value != 0)
                .or_else(|| packed_payload.map(|value| value & 0xffff))
                .unwrap_or(self.last_hit_payload)
        };
        if event != 0 {
            if object != 0 {
                self.last_hit_control = object;
            }
            self.last_hit_payload = payload;
            if self.title_ui_active {
                self.title_scenario_requested = crate::title::title_payload_is_scenario(payload);
            }
            trace_graph!(self,
                "dispatch object event object=#{object} count={count} event=0x{event:08X} payload=0x{:04X}",
                self.last_hit_payload
            );
        }
        Ok(())
    }

    fn read_input_state(&mut self, descriptor: i32) -> i32 {
        let state = input_state_for_descriptor(
            self.pending_input_state,
            self.pending_input_descriptor,
            descriptor,
        );
        let is_message_mouse_release = state != 0
            && self.pending_input_descriptor == Some(INPUT_DESCRIPTOR_MOUSE_LEFT)
            && (state & 0xf) == 0x6;
        if self.suppress_message_mouse_release && is_message_mouse_release {
            self.suppress_message_mouse_release = false;
            self.pending_input_consumed = true;
            self.trace_graph("message reveal consumed matching mouse release");
            return 0;
        }
        if self.pending_input_state.is_some() {
            tracing::info!(
                descriptor,
                active_descriptor = ?self.pending_input_descriptor,
                pending_state = ?self.pending_input_state.map(|state| format!("0x{state:08X}")),
                result = format_args!("0x{state:08X}"),
                "runtime input state queried"
            );
            trace_graph!(self,
                "read input state descriptor={descriptor} active={:?} pending={:?} -> 0x{state:08X}",
                self.pending_input_descriptor, self.pending_input_state
            );
        }
        if state != 0 {
            let was_animating = self.text_runtime.is_animating();
            self.pending_input_consumed = true;
            self.reveal_text_on_input();
            if was_animating
                && self.pending_input_descriptor == Some(INPUT_DESCRIPTOR_MOUSE_LEFT)
                && (state & 0xf) == 0x2
            {
                self.suppress_message_mouse_release = true;
            }
        }
        state
    }

    fn query_input_class_state(&mut self, scope: i32) -> i32 {
        let mut state = input_class_state_for_descriptor(
            self.pending_input_state,
            self.pending_input_descriptor,
        );
        if self.input_master_gate != 0 && self.input_latched_state != 0 {
            state |= i32::MIN;
        }
        if state != 0 {
            let was_animating = self.text_runtime.is_animating();
            self.pending_input_consumed = true;
            self.reveal_text_on_input();
            if was_animating && self.pending_input_descriptor == Some(INPUT_DESCRIPTOR_MOUSE_LEFT) {
                self.suppress_message_mouse_release = true;
            }
        }
        if self.pending_input_state.is_some() {
            tracing::info!(
                scope,
                active_descriptor = ?self.pending_input_descriptor,
                pending_state = ?self.pending_input_state.map(|value| format!("0x{value:08X}")),
                result = format_args!("0x{state:08X}"),
                "runtime input class state queried"
            );
        }
        state
    }

    fn set_input_master_gate(&mut self, value: i32) {
        self.input_master_gate = value;
        tracing::debug!(value, "native input master gate changed");
    }

    fn set_input_latched_state(&mut self, value: i32) {
        self.input_latched_state = value;
        tracing::debug!(value, "native input latched state changed");
    }

    fn query_input_descriptor_state(&mut self, class_mask: i32) -> i32 {
        let state = input_class_state_for_descriptor(
            self.pending_input_state,
            self.pending_input_descriptor,
        );
        let selected = state & class_mask;
        if selected != 0 {
            self.pending_input_consumed = true;
        }
        selected
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
        if let Some(result) = self.dispatch_native_system(group, id, stack) {
            return result;
        }
        if let Some(result) = self.dispatch_native_system_ext(group, id, stack) {
            return result;
        }
        match (group, id) {
            (0x80, 0x40) => {
                let file = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let archive = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                tracing::info!(archive, file, "LoadProgram");
                self.observe_loaded_program_for_title(&file);
                let program = self.load_bp_program_cached(&archive, &file, "LoadProgram");
                return Ok(ethornell_vm::Value::Program(std::sync::Arc::new(program)));
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
                return Ok(ethornell_vm::Value::Program(std::sync::Arc::new(program)));
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
            (0x80, 0x08) => {
                let (x, y) = self
                    .mouse_pos
                    .map(|(x, y)| (x.round() as i32, y.round() as i32))
                    .unwrap_or_default();
                stack.push(ethornell_vm::Value::Int(x));
                stack.push(ethornell_vm::Value::Int(y));
                tracing::debug!(x, y, "ReadCursorPoint");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x1f) => {
                let args = pop_args(stack, 6);
                let ints = args.iter().map(value_to_i32).collect::<Vec<_>>();
                let duration_ms = ints.get(1).copied().unwrap_or_default().max(0) as u32;
                if duration_ms > 0 {
                    self.animation_queue_remaining = self
                        .animation_queue_remaining
                        .max(duration_ms.div_ceil(16).max(1));
                }
                tracing::debug!(?ints, "ConfigureUiMotion");
                trace_graph!(self, "ui motion args={ints:?}");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x28) => {
                let path = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let ok = runtime_file_path(&self.manager, &path)
                    .map(|path| std::fs::create_dir_all(path).is_ok())
                    .unwrap_or(false);
                tracing::info!(path, ok, "CreateDirectory");
                return Ok(ethornell_vm::Value::Int(i32::from(ok)));
            }
            (0x80, 0x2a) => {
                let path = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let exists = runtime_file_path(&self.manager, &path)
                    .map(|path| path.is_dir())
                    .unwrap_or(false);
                tracing::info!(path, exists, "DirectoryExists");
                return Ok(ethornell_vm::Value::Int(i32::from(exists)));
            }
            (0x80, 0x2b) => {
                // funcs_48B30E[0x2B] -> sub_488630 pops five values and
                // returns one exactly when its final source pointer is valid.
                let args = pop_args(stack, 5);
                let source_present = args.first().is_some_and(|value| value_to_i32(value) != 0);
                tracing::debug!(args = ?summarize_values(&args), source_present, "ConvertDebugRecord");
                return Ok(ethornell_vm::Value::Int(i32::from(source_present)));
            }
            (0x80, 0x29) => {
                let path = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let ok = runtime_file_path(&self.manager, &path)
                    .map(|path| std::fs::remove_dir(&path).is_ok())
                    .unwrap_or(false);
                tracing::info!(path, ok, "RemoveDirectory");
                return Ok(ethornell_vm::Value::Int(i32::from(ok)));
            }
            (0x80, 0x2c) => {
                let path = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let attrs = runtime_file_attributes(&self.manager, &path);
                tracing::info!(path, attrs, "GetFileAttributes");
                return Ok(ethornell_vm::Value::Int(attrs));
            }
            (0x80, 0x2d) => {
                let attrs = pop_int_value(stack).unwrap_or_default();
                let path = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let ok = runtime_file_path(&self.manager, &path)
                    .map(|path| {
                        if let Ok(mut perms) =
                            std::fs::metadata(&path).map(|meta| meta.permissions())
                        {
                            perms.set_readonly(attrs & 0x01 != 0);
                            std::fs::set_permissions(&path, perms).is_ok()
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                tracing::info!(path, attrs, ok, "SetFileAttributes");
                return Ok(ethornell_vm::Value::Int(i32::from(ok)));
            }
            (0x80, 0x2f) => {
                let from = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let to = pop_string_value(stack).unwrap_or_else(|| "<unknown>".into());
                let ok = match (
                    runtime_file_path(&self.manager, &from),
                    runtime_file_path(&self.manager, &to),
                ) {
                    (Some(from), Some(to)) => {
                        if let Some(parent) = to.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        std::fs::copy(&from, &to).is_ok()
                    }
                    _ => false,
                };
                tracing::info!(from, to, ok, "CopyFile");
                return Ok(ethornell_vm::Value::Int(i32::from(ok)));
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
                let state = pop_int_value(stack).unwrap_or_default();
                let object = pop_int_value(stack).unwrap_or_default();
                if let Some(input) = self.graph_input_objects.get_mut(&object) {
                    input.registered_state = state;
                }
                tracing::debug!(object, state, "SetRegisteredObjectState");
                return Ok(ethornell_vm::Value::Int(i32::from(
                    self.graph_input_objects.contains_key(&object),
                )));
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
            (0x80, 0x15) => {
                let visible = pop_int_value(stack).unwrap_or_default() != 0;
                tracing::debug!(visible, "ShowCursor");
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
            (0x80, 0x13) => {
                let value = self
                    .call_coverage
                    .get(&(group, id))
                    .copied()
                    .unwrap_or_default()
                    .min(i32::MAX as usize) as i32;
                if self.should_yield_frame() {
                    self.frame_yield_requested = true;
                }
                tracing::debug!(value, "PumpWaitState");
                return Ok(ethornell_vm::Value::Int(value));
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
                let fullscreen = pop_int_value(stack).unwrap_or_default() != 0;
                let adapter = pop_int_value(stack).unwrap_or_default();
                let mode = pop_int_value(stack).unwrap_or_default();
                self.window_mode = i32::from(fullscreen);
                self.pending_fullscreen = Some(fullscreen);
                tracing::info!(mode, adapter, fullscreen, "ConfigureDisplayMode");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x61) => {
                tracing::debug!(fullscreen = self.window_mode, "QueryDisplayMode");
                return Ok(ethornell_vm::Value::Int(self.window_mode));
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
                let visible = pop_int_value(stack).unwrap_or_default() != 0;
                self.pending_window_visible = Some(visible);
                if !visible {
                    self.pending_input_state = None;
                    self.pending_input_descriptor = None;
                    self.pending_click = None;
                    self.pending_object_state = None;
                }
                tracing::info!(visible, "SetWindowVisible");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x65) => {
                self.pending_window_minimize = true;
                self.pending_input_state = None;
                self.pending_input_descriptor = None;
                self.pending_click = None;
                self.pending_object_state = None;
                tracing::info!("MinimizeMainWindow");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x66) => {
                let title = pop_string_value(stack).unwrap_or_default();
                self.pending_window_title = Some(title.clone());
                tracing::info!(title, "SetWindowTitle");
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
            (0x80, 0x69) => {
                // funcs_48B30E[0x69] -> sub_4894B0 posts WM_CLOSE. Both
                // frontends observe this flag at the same frame boundary.
                self.quit_requested = true;
                tracing::info!("RequestWindowClose");
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
                let accepted = (0..=1).contains(&value);
                if accepted {
                    self.system_mode_flag = value;
                }
                tracing::info!(value, accepted, "SetSystemModeFlag");
                return Ok(ethornell_vm::Value::Int(i32::from(accepted)));
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
            (0x80, 0x07) => {
                let args = pop_args(stack, 2);
                tracing::debug!(args = ?summarize_values(&args), "ReadPerformanceMetric");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0x25) => {
                let args = pop_args(stack, 5);
                tracing::info!(args = ?summarize_values(&args), "EnumerateFiles");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0x3f) => {
                let args = pop_args(stack, 4);
                tracing::info!(args = ?summarize_values(&args), "ConfigureArchiveRoot");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0x59) => {
                let delta = pop_int_value(stack).unwrap_or_default();
                self.frame_yield_requested = true;
                tracing::debug!(delta, "AdvanceDeadlineAndPoll");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0x9e) => {
                let args = pop_args(stack, 3);
                tracing::debug!(args = ?summarize_values(&args), "ReleaseQueuedRecord");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0xd8) => {
                tracing::debug!("DispatchPendingCallbacks");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xe0) => {
                let args = pop_args(stack, 4);
                tracing::info!(args = ?summarize_values(&args), "LaunchProcess suppressed by portable runtime");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0xe1) => {
                let args = pop_args(stack, 3);
                tracing::info!(args = ?summarize_values(&args), "RestartWithCommand suppressed by portable runtime");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xe2) => {
                let args = pop_args(stack, 3);
                tracing::info!(args = ?summarize_values(&args), "LaunchProcessWithArgs suppressed by portable runtime");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0xe3) => {
                let target = pop_string_value(stack).unwrap_or_default();
                let opened = platform::shell_open(&target);
                tracing::info!(target, opened, "ShellOpen");
                return Ok(ethornell_vm::Value::Int(i32::from(opened)));
            }
            (0x80, 0xed) => {
                let selector = pop_int_value(stack).unwrap_or_default();
                tracing::debug!(
                    selector,
                    value = self.system_extension_state,
                    "SystemExtensionQuery"
                );
                return Ok(ethornell_vm::Value::Int(self.system_extension_state));
            }
            (0x80, 0xee) => {
                self.system_extension_state = 0;
                tracing::debug!("SystemExtensionReset");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xef) => {
                self.system_extension_state = pop_int_value(stack).unwrap_or_default();
                tracing::debug!(value = self.system_extension_state, "SystemExtensionSet");
                return Ok(ethornell_vm::Value::None);
            }
            (0x80, 0xf0) => {
                let args = pop_args(stack, 8);
                tracing::info!(args = ?summarize_values(&args), "ShowInputDialog unavailable on this platform");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0xf2) => {
                let args = pop_args(stack, 16);
                tracing::info!(args = ?summarize_values(&args), "ShowInstallerDialog unavailable on this platform");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0xf7) => {
                let args = pop_args(stack, 3);
                tracing::info!(args = ?summarize_values(&args), "ValidateOrCreateUserPath");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x80, 0xf8) => {
                let args = pop_args(stack, 3);
                tracing::info!(args = ?summarize_values(&args), "ReadInstalledFolder unavailable outside native Windows registry");
                return Ok(ethornell_vm::Value::Int(0));
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
                tracing::debug!(stack = ?summarize_values(stack), "FontAcquireFromStackFrame");
                return Ok(ethornell_vm::Value::None);
            }
            (0x81, 0x18) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::info!(value, "Sys2_18");
                return Ok(ethornell_vm::Value::None);
            }
            (0x81, 0x19) => {
                let destination = stack.pop();
                tracing::debug!(?destination, "CopyTouchRecords");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x81, 0x2f) => {
                let path = pop_string_value(stack).unwrap_or_default();
                let writable = runtime_file_path(&self.manager, &path)
                    .as_deref()
                    .map(platform::test_path_writable)
                    .unwrap_or(false);
                tracing::info!(path, writable, "TestPathWritable");
                return Ok(ethornell_vm::Value::Int(i32::from(writable)));
            }
            (0x81, 0x31) => {
                let args = pop_args(stack, 4);
                tracing::info!(args = ?summarize_values(&args), "InternetRead unavailable in portable runtime");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x81, 0x36) => {
                let destination = stack.pop();
                tracing::info!(
                    ?destination,
                    "EnumerateDriveTypes unavailable outside Windows"
                );
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x81, 0x37) => {
                let path = pop_string_value(stack).unwrap_or_default();
                let destination = stack.pop();
                tracing::info!(
                    path,
                    ?destination,
                    "GetDiskFreeMegabytes unavailable outside native Windows ABI"
                );
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x81, 0xf7) => {
                let args = pop_args(stack, 4);
                tracing::info!(args = ?summarize_values(&args), "ValidateOrCreateUserPathEx");
                return Ok(ethornell_vm::Value::Int(0));
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
                self.system_config_input_mode = i32::from(value != 0);
                tracing::info!(value = self.system_config_input_mode, "SetConfigInputMode");
                return Ok(ethornell_vm::Value::Int(self.system_config_input_mode));
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
        self.runtime_stubbed = true;
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
    fn query_graph_effect_result(&self, process: i32) -> Option<i32> {
        self.graph_effects.result(process)
    }

    fn invoke_graph_effect_process(&mut self, process: i32) -> ethornell_vm::GraphEffectInvocation {
        let invocation = self.graph_effects.invoke(process);
        let source = self
            .graph_effects
            .source(process)
            .map(|(archive, resource)| format!("{archive}:{resource}"))
            .unwrap_or_default();
        trace_graph!(
            self,
            "graph effect invoke handle=#{process} status={} duration_ms={} source={source}",
            invocation.status,
            invocation.duration_ms
        );
        invocation
    }

    fn cache_graph_blob(&mut self, namespace: &str, name: &str, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        self.graph_blob_cache
            .insert((namespace.to_string(), name.to_string()), bytes.to_vec());
        tracing::debug!(namespace, name, size = bytes.len(), "GraphCacheBlob");
        true
    }

    fn create_bitmap_from_rgb(
        &mut self,
        bitmap: i32,
        width: i32,
        height: i32,
        format: i32,
        pixels: &[u8],
    ) -> bool {
        if !(0..0x4000).contains(&bitmap) || width <= 0 || height <= 0 || format != 1 {
            return false;
        }
        let pixel_count = width as usize * height as usize;
        if pixels.len() < pixel_count * 3 {
            return false;
        }
        let mut rgba = Vec::with_capacity(pixel_count * 4);
        for rgb in pixels[..pixel_count * 3].chunks_exact(3) {
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        let image = DecodedImage {
            width: width as u32,
            height: height as u32,
            rgba,
        };
        let key = format!("runtime:rgb:{bitmap}");
        self.store_graph_image(key.clone(), image);
        self.graph_resources
            .insert(bitmap, RuntimeGraphResource::whole(key));
        self.bitmap_dimensions
            .insert(bitmap, (width as u32, height as u32));
        self.graph_surfaces.insert(
            bitmap,
            RuntimeSurface::bitmap(bitmap, width as f32, height as f32),
        );
        true
    }

    fn read_bitmap_pixels(&mut self, bitmap: i32, capacity: usize) -> Option<Vec<u8>> {
        let image = self.graph_bitmap_image(bitmap)?;
        let required = image.width as usize * image.height as usize * 3;
        if capacity < required {
            return None;
        }
        let mut rgb = Vec::with_capacity(required);
        for rgba in image.rgba.chunks_exact(4) {
            rgb.extend_from_slice(&rgba[..3]);
        }
        Some(rgb)
    }

    fn collect_ruby_substitutions(&mut self, source: &str) -> (String, i32) {
        self.graph_defaults.collect_ruby_records(source)
    }

    fn configure_graph_surface_controls(
        &mut self,
        surface: i32,
        descriptor: ethornell_vm::GraphInputDescriptor,
    ) {
        let region_count = descriptor.regions.len();
        let layer_count = self.replace_surface_control_layers(surface, &descriptor);
        tracing::debug!(
            surface,
            region_count,
            layer_count,
            "GraphConfigureSurfaceControls"
        );
        trace_graph!(
            self,
            "surface #{surface} configure controls regions={region_count} drawable={layer_count}"
        );
    }

    fn configure_graph_input_object(
        &mut self,
        object: i32,
        mut descriptor: ethornell_vm::GraphInputDescriptor,
    ) {
        for region in &mut descriptor.regions {
            if region.width > 0 && region.height > 0 {
                continue;
            }
            let Some((_, image_region)) = self.resource_image_region(region.normal_resource) else {
                continue;
            };
            if region.width <= 0 {
                region.width = image_region.width.round() as i32;
            }
            if region.height <= 0 {
                region.height = image_region.height.round() as i32;
            }
        }
        descriptor
            .regions
            .retain(|region| region.enabled_depth != 0 && region.width > 0 && region.height > 0);
        self.sync_title_controls_from_input(object, &descriptor);
        let region_count = descriptor.regions.len();
        let descriptor_changed = self
            .graph_input_objects
            .get(&object)
            .is_none_or(|input| input.descriptor != descriptor);
        let region_trace = (self.debug_graph && descriptor_changed).then(|| {
            descriptor
                .regions
                .iter()
                .map(|region| {
                    format!(
                        "group={} index={} ordinal={} enabled_depth={} selected={} rect=({},{} {}x{}) resources=({},{},{}) flags=0x{:08X}",
                        region.group,
                        region.index,
                        region.ordinal,
                        region.enabled_depth,
                        region.selected,
                        region.x,
                        region.y,
                        region.width,
                        region.height,
                        region.normal_resource,
                        region.selected_resource,
                        region.mask_resource,
                        region.flags
                    )
                })
                .collect::<Vec<_>>()
        });
        let input = self
            .graph_input_objects
            .entry(object)
            .or_insert_with(|| RuntimeGraphInputObject::new(0));
        input.configure(descriptor);
        let input_layer = input.layer;
        trace_graph!(
            self,
            "configure input object #{object} layer=#{} regions={region_count}",
            input_layer
        );
        if let Some(region_trace) = region_trace {
            for (ordinal, region) in region_trace.into_iter().enumerate() {
                tracing::debug!(
                    target: "graph_input",
                    object,
                    layer = input_layer,
                    ordinal,
                    region,
                    "configure input region"
                );
                trace_graph!(self, "input object #{object} region[{ordinal}] {region}");
            }
        }
    }

    fn query_bitmap_info(&mut self, bitmap: i32) -> Option<ethornell_vm::BitmapInfo> {
        let dimensions = self
            .bitmap_dimensions
            .get(&bitmap)
            .map(|&(width, height)| (width as f32, height as f32))
            .or_else(|| {
                self.graph_surfaces
                    .get(&bitmap)
                    .map(|surface| (surface.width, surface.height))
            })
            .or_else(|| {
                self.resource_image_region(bitmap)
                    .map(|(_, region)| (region.width, region.height))
            })
            .or_else(|| {
                self.is_primary_framebuffer(bitmap).then(|| {
                    let (width, height) = snapshot::runtime_frame_size(self);
                    (width as f32, height as f32)
                })
            });
        let info = dimensions.map(|(width, height)| ethornell_vm::BitmapInfo {
            width: width.max(0.0).round() as u32,
            height: height.max(0.0).round() as u32,
            format: 2,
        });
        tracing::debug!(bitmap, ?info, "BitmapQueryInfo");
        info
    }

    fn set_bitmap_dimensions(&mut self, bitmap: i32, width: i32, height: i32) -> bool {
        if !(0..0x4000).contains(&bitmap) {
            return false;
        }
        self.bitmap_dimensions
            .insert(bitmap, (width as u32, height as u32));
        tracing::debug!(bitmap, width, height, "GraphSetBitmapDimensions");
        true
    }

    fn call_graph_spline_control(
        &mut self,
        args: &[ethornell_vm::Value],
        points: &[[i32; 4]],
    ) -> ethornell_vm::VmResult<ethornell_vm::Value> {
        let target = args.first().map(value_to_i32).unwrap_or_default();
        let mode = args.get(3).map(value_to_i32).unwrap_or_default();
        let start_alpha = args.get(4).map(value_to_i32).unwrap_or(256);
        let target_alpha = args.get(6).map(value_to_i32).unwrap_or(start_alpha);
        let duration_ms = args.get(7).map(value_to_i32).unwrap_or_default().max(1);
        let input_enabled = args.get(10).map(value_to_i32).unwrap_or_default() != 0;
        let input_descriptor = args.get(11).map(value_to_i32).unwrap_or_default();
        let duration_frames = duration_ms.unsigned_abs().div_ceil(16).max(1);

        let layer_ids = self.graph_target_layers(target);
        for layer_id in &layer_ids {
            if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                let from = (start_alpha as f32 / 256.0).clamp(0.0, 1.0);
                let to = (target_alpha as f32 / 256.0).clamp(0.0, 1.0);
                layer.opacity = from;
                self.layer_animations
                    .fade_to(*layer_id, from, to, duration_frames);

                if let Some(last) = points.last() {
                    let x = graph_motion_coord(last[0]);
                    let y = graph_motion_coord(last[1]);
                    self.layer_animations.move_x_to(
                        *layer_id,
                        layer.transform_x,
                        x,
                        duration_frames,
                    );
                    self.layer_animations.move_y_to(
                        *layer_id,
                        layer.transform_y,
                        y,
                        duration_frames,
                    );
                }
            }
        }
        self.animation_queue_remaining = self.animation_queue_remaining.max(duration_frames);
        self.pending_graph_procedure_schedule = Some(ethornell_vm::GraphProcedureSchedule {
            duration_ms,
            input_enabled,
            input_descriptor,
            wait_for_input: false,
            completion: ethornell_vm::GraphProcedureCompletion::ControlProgress,
        });
        trace_graph!(self,
            "spline control target=#{target} mode={mode} duration_ms={duration_ms} alpha={start_alpha}->{target_alpha} points={points:?} layers={layer_ids:?}"
        );
        tracing::debug!(
            target,
            mode,
            duration_ms,
            start_alpha,
            target_alpha,
            ?points,
            "GraphScheduleSplineControl"
        );
        Ok(ethornell_vm::Value::None)
    }

    fn take_graph_procedure_schedule(&mut self) -> Option<ethornell_vm::GraphProcedureSchedule> {
        self.pending_graph_procedure_schedule.take()
    }

    fn call_graph(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> ethornell_vm::VmResult<ethornell_vm::Value> {
        if let Some(result) = self.dispatch_native_graph(group, id, stack) {
            return result;
        }
        if let Some(result) = self.dispatch_native_graph_ext(group, id, stack) {
            return result;
        }
        if let Some(result) = self.dispatch_native_graph_resource(group, id, stack) {
            return result;
        }
        match (group, id) {
            (0x90, 0x40) => {
                let bitmap = pop_int_value(stack).unwrap_or_default();
                self.primary_bitmap = (bitmap > 0).then_some(bitmap);
                tracing::debug!(bitmap, "GraphSetPrimaryBitmap");
                return Ok(ethornell_vm::Value::None);
            }
            (0x90, 0x47) => {
                let args = pop_args(stack, 5);
                tracing::info!(args = ?summarize_values(&args), "GraphConfigureEffectResource");
                trace_graph!(
                    self,
                    "configure effect resource raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
                return Ok(ethornell_vm::Value::None);
            }
            (0x90, 0x4a) => {
                let args = pop_args(stack, 5);
                tracing::debug!(args = ?summarize_values(&args), "GraphConfigureBlitSources");
                return Ok(ethornell_vm::Value::None);
            }
            (0x90, 0x4d) => {
                tracing::debug!("GraphGetRenderTarget");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x90, 0xb4) => {
                let args = pop_args(stack, 3);
                tracing::debug!(args = ?summarize_values(&args), "GraphCompositeSpriteBatch");
                return Ok(ethornell_vm::Value::None);
            }
            (0x90, 0xda) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                let state = if self.timelines.contains(handle) {
                    i32::from(self.timelines.poll(handle).finished)
                } else {
                    i32::from(self.graph_handle_exists(handle))
                };
                tracing::debug!(handle, state, "GraphQuerySpecialHandle");
                return Ok(ethornell_vm::Value::Int(state));
            }
            (0x90, 0xdb) => {
                let handle = self.graph_special_events.pop_front().unwrap_or_default();
                tracing::debug!(handle, "GraphTakeSpecialEvent");
                return Ok(ethornell_vm::Value::Int(handle));
            }
            (0x90, 0xde) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                if handle > 0 {
                    self.graph_special_watches.insert(handle);
                }
                tracing::debug!(handle, "GraphWatchSpecialHandle");
                return Ok(ethornell_vm::Value::None);
            }
            (0x90, 0xdf) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                self.graph_special_watches.remove(&handle);
                self.graph_special_events.retain(|event| *event != handle);
                tracing::debug!(handle, "GraphUnwatchSpecialHandle");
                return Ok(ethornell_vm::Value::None);
            }
            (0x90, 0xf0) => {
                let args = pop_args(stack, 5);
                tracing::info!(args = ?summarize_values(&args), "MovieOpenBlocking unavailable in portable renderer");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x90, 0xf1) => {
                // funcs_48065E[0xF1] -> sub_480260 tears down the native
                // graphics/media driver and releases its display state.
                self.graph_driver_mode = 0;
                self.graph_process_handles.clear();
                tracing::info!("GraphShutdownDriver");
            }
            (0x90, 0xf2) => {
                // sub_480280 returns sub_48F690's driver ordering predicate.
                // The portable renderer has no asynchronous device reset.
                let status = 0;
                tracing::debug!(status, "GraphQueryDriverStatus");
                return Ok(ethornell_vm::Value::Int(status));
            }
            (0x90, 0xf3) => {
                let volume = pop_int_value(stack).unwrap_or_default().clamp(0, 128);
                tracing::debug!(volume, "MovieSetVolume");
                return Ok(ethornell_vm::Value::None);
            }
            (0x90, 0xf4) => {
                let args = pop_args(stack, 4);
                let handle = self.next_object_handle;
                self.next_object_handle = self.next_object_handle.saturating_add(1);
                self.graph_process_handles.insert(handle);
                tracing::info!(args = ?summarize_values(&args), handle, "MovieCreateLoader unavailable in portable renderer");
                return Ok(ethornell_vm::Value::Int(handle));
            }
            (0x90, 0xf5) => {
                let handle = stack.pop().unwrap_or(ethornell_vm::Value::None);
                let raw_handle = value_to_i32(&handle);
                // sub_480370 -> sub_405CD0 removes a graph process from the
                // native intrusive list. Function-valued BP handles are the
                // port's tagged representation of an extant process pointer.
                let released = self.graph_process_handles.remove(&raw_handle)
                    || matches!(handle, ethornell_vm::Value::Func { .. });
                let status = if released { 0 } else { 3 };
                tracing::debug!(raw_handle, status, "GraphReleaseProcess");
                return Ok(ethornell_vm::Value::Int(status));
            }
            (0x90, 0xf7) => {
                // sub_4804D0 resolves the first BP handle, attaches the second
                // value as a graph process, and maps native failures to 3/7.
                let process = pop_int_value(stack).unwrap_or_default();
                let object = pop_int_value(stack).unwrap_or_default();
                let status = if !self.graph_handle_exists(object) {
                    3
                } else if self.graph_process_handles.contains(&object) {
                    7
                } else {
                    self.graph_process_handles.insert(object);
                    if process != 0 {
                        self.graph_bindings.insert(object, process);
                    }
                    0
                };
                tracing::debug!(object, process, status, "GraphAttachProcess");
                return Ok(ethornell_vm::Value::Int(status));
            }
            (0x91, 0x37) => {
                let z = fixed_16_to_f32(pop_int_value(stack).unwrap_or_default());
                let y = fixed_16_to_f32(pop_int_value(stack).unwrap_or_default());
                let x = fixed_16_to_f32(pop_int_value(stack).unwrap_or_default());
                let object = pop_int_value(stack).unwrap_or_default();
                let layer_ids = self.graph_target_layers(object);
                for layer_id in &layer_ids {
                    if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                        layer.x = x;
                        layer.y = y;
                        layer.z = z.round() as i32;
                    }
                }
                tracing::debug!(object, x, y, z, ?layer_ids, "GraphObjectSetBaseVector3");
                trace_graph!(
                    self,
                    "object #{object} base vector x={x:.2} y={y:.2} z={z:.2} layers={layer_ids:?}"
                );
                return Ok(ethornell_vm::Value::None);
            }
            (0x91, 0x36) => {
                let z = fixed_16_to_f32(pop_int_value(stack).unwrap_or_default()).round() as i32;
                let y = fixed_16_to_f32(pop_int_value(stack).unwrap_or_default());
                let x = fixed_16_to_f32(pop_int_value(stack).unwrap_or_default());
                let object = pop_int_value(stack).unwrap_or_default();
                let layer_ids = self.graph_target_layers(object);
                for layer_id in &layer_ids {
                    if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                        layer.transform_x = x;
                        layer.transform_y = y;
                        layer.transform_z = z;
                    }
                }
                trace_graph!(
                    self,
                    "object #{object} transform vector x={x:.2} y={y:.2} z={z} layers={layer_ids:?}"
                );
                return Ok(ethornell_vm::Value::None);
            }
            (0x91, 0x1b) => {
                let alpha = pop_int_value(stack).unwrap_or_default();
                let scale_y = pop_int_value(stack).unwrap_or_default();
                let scale_x = pop_int_value(stack).unwrap_or_default();
                let source = pop_int_value(stack).unwrap_or_default();
                let destination = pop_int_value(stack).unwrap_or_default();
                let source_image = self
                    .resource_image_region(source)
                    .and_then(|(key, region)| {
                        self.graph_images
                            .get(key)
                            .map(|image| crop_decoded_image(image, region))
                    });
                let destination_image =
                    self.resource_image_region(destination)
                        .and_then(|(key, region)| {
                            self.graph_images
                                .get(key)
                                .map(|image| crop_decoded_image(image, region))
                        });
                if let (Some(source_image), Some(mut destination_image)) =
                    (source_image, destination_image)
                {
                    if let Some(scaled) = scale_decoded_image_fixed(&source_image, scale_x, scale_y)
                    {
                        blend_decoded_image_parameter(&mut destination_image, &scaled, alpha);
                        let width = destination_image.width;
                        let height = destination_image.height;
                        let key = format!("runtime:scaled-blend:{destination}");
                        self.store_graph_image(key.clone(), destination_image);
                        self.graph_resources
                            .insert(destination, RuntimeGraphResource::whole(key));
                        self.bitmap_dimensions.insert(destination, (width, height));
                    }
                }
                tracing::debug!(
                    destination,
                    source,
                    scale_x,
                    scale_y,
                    alpha,
                    "GraphBlendBitmapsScaled"
                );
                return Ok(ethornell_vm::Value::None);
            }
            (0x91, 0x1c) => {
                let pixel_format = pop_int_value(stack).unwrap_or_default();
                let scale_y = pop_int_value(stack).unwrap_or_default();
                let scale_x = pop_int_value(stack).unwrap_or_default();
                let source = pop_int_value(stack).unwrap_or_default();
                let destination = pop_int_value(stack).unwrap_or_default();
                let scaled = self
                    .graph_bitmap_image(source)
                    .and_then(|image| scale_decoded_image_fixed(&image, scale_x, scale_y));
                if let Some(image) = scaled {
                    let width = image.width;
                    let height = image.height;
                    let key = format!("runtime:scaled-bitmap:{destination}");
                    self.store_graph_image(key.clone(), image);
                    self.graph_resources
                        .insert(destination, RuntimeGraphResource::whole(key));
                    self.bitmap_dimensions.insert(destination, (width, height));
                    self.graph_surfaces.insert(
                        destination,
                        RuntimeSurface::bitmap(destination, width as f32, height as f32),
                    );
                }
                tracing::debug!(
                    destination,
                    source,
                    scale_x,
                    scale_y,
                    pixel_format,
                    "GraphCreateScaledBitmap"
                );
                return Ok(ethornell_vm::Value::None);
            }
            (0x91, 0xdb) => {
                tracing::debug!("GraphCurrentSpecialHandle");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x92, 0x01) => {
                let args = pop_args(stack, 7);
                let configured = self.effects.configure_wave_table(&args);
                tracing::debug!(args = ?summarize_values(&args), configured, "GraphConfigureWaveTable");
                return Ok(ethornell_vm::Value::None);
            }
            (0x92, 0x10) => {
                let args = pop_args(stack, 5);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                let generated =
                    if let [period, center_y, center_x, direction, bitmap] = values.as_slice() {
                        self.effects
                            .generate_ripple_map(*bitmap, *direction, *center_x, *center_y, *period)
                    } else {
                        false
                    };
                tracing::debug!(args = ?summarize_values(&args), generated, "GraphGenerateRippleMap");
                return Ok(ethornell_vm::Value::None);
            }
            (0x92, 0x8d) => {
                let args = pop_args(stack, 6);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                let mut applied = false;
                if let [alpha, mode, source, y, x, destination] = values.as_slice() {
                    applied = self
                        .composite_graph_bitmap(*destination, *source, *x, *y, *mode, *alpha)
                        .is_some();
                }
                tracing::debug!(args = ?summarize_values(&args), applied, "GraphApplyEffectResource");
                return Ok(ethornell_vm::Value::None);
            }
            (0x92, 0xf0) => {
                let args = pop_args(stack, 6);
                tracing::info!(args = ?summarize_values(&args), "MovieOpen unavailable in portable renderer");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x92, 0xf5) => {
                let args = pop_args(stack, 2);
                tracing::debug!(args = ?summarize_values(&args), "MovieGetState");
                return Ok(ethornell_vm::Value::Int(0));
            }
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
            (0x90, 0x01) => {
                let value = pop_int_value(stack).unwrap_or_default();
                tracing::debug!(value, "GraphShutdownOrReset");
                trace_graph!(self, "graph reset/shutdown mode={value}");
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
            (0x90, 0x05) => {
                let args = pop_args(stack, 2);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                if let Some(bitmap_id) = values.last().copied().filter(|id| *id > 0) {
                    self.graph_resources.remove(&bitmap_id);
                    if let Some(surface) = self.graph_surfaces.get_mut(&bitmap_id) {
                        surface.resource_id = None;
                    }
                    let resource_id = values.first().copied().unwrap_or_default();
                    let key = self
                        .graphic_resource_keys
                        .get(&(0, resource_id))
                        .cloned()
                        .or_else(|| {
                            self.graphic_resource_keys
                                .iter()
                                .find_map(|((_, id), key)| {
                                    (*id == resource_id).then(|| key.clone())
                                })
                        });
                    if let Some(key) = key {
                        self.load_graph_image_key(&key);
                        self.graph_resources
                            .insert(bitmap_id, RuntimeGraphResource::whole(key.clone()));
                        trace_graph!(
                            self,
                            "load resource #{resource_id} into bitmap #{bitmap_id} source={key} values={values:?}"
                        );
                    }
                }
                tracing::debug!(args = ?summarize_values(&args), "GraphSelectWorkBuffer");
            }
            (0x90, 0x08) => {
                let enabled = pop_int_value(stack).unwrap_or_default();
                tracing::info!(enabled, "GraphSetEnabled");
            }
            (0x90, 0x09) => {
                let priority = pop_int_value(stack).unwrap_or_default();
                if (0..0x1_0000).contains(&priority) {
                    self.graph_default_priority = priority;
                } else {
                    tracing::warn!(priority, "invalid graph default priority");
                }
                tracing::debug!(priority, "GraphSetDefaultPriority");
                trace_graph!(self, "default priority={priority}");
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
                tracing::info!(handle, "GraphCreateNode");
                trace_graph!(self, "create node #{handle}");
                return Ok(ethornell_vm::Value::Int(handle));
            }
            (0x90, 0x51) => {
                let target = stack.pop();
                let node = target.as_ref().map(value_to_i32).unwrap_or_default();
                let removed_text = self.text_nodes.remove(&node).is_some();
                let removed_layer = self.graph_layers.remove(&node).is_some();
                self.graph_transition_nodes.remove(&node);
                self.graph_object_properties.remove(&node);
                tracing::debug!(?target, node, removed_text, "GraphNodeRelease");
                if removed_text || removed_layer {
                    trace_graph!(self, "release node #{node}");
                }
            }
            (0x90, 0x53) => {
                let args = pop_args(stack, 5);
                self.refresh_graph_object_rect(&args);
                tracing::debug!(args = ?summarize_values(&args), "GraphRefreshObjectRect");
            }
            (0x90, 0x54) => {
                let enabled = stack.pop();
                let node = stack.pop();
                tracing::info!(?node, ?enabled, "GraphNodeSetEnabled");
                if let Some(node) = node.as_ref().map(value_to_i32) {
                    let enabled = enabled.as_ref().map(value_to_i32).unwrap_or(0) != 0;
                    if let Some(record) = self.text_nodes.get_mut(&node) {
                        record.enabled = enabled;
                    }
                    if let Some(layer) = self.graph_layers.get_mut(&node) {
                        layer.enabled = enabled;
                    }
                }
            }
            (0x90, 0x55) => {
                let args = pop_args(stack, 2);
                self.apply_graph_layer_property(&args);
                tracing::debug!(args = ?summarize_values(&args), "GraphLayerSetProperty");
            }
            (0x90, 0x56) => {
                let args = pop_args(stack, 7);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                let node = values.last().copied().unwrap_or_default();
                if node > 0 {
                    self.graph_transition_nodes.remove(&node);
                    let bitmap_id = values.get(3).copied().unwrap_or_default();
                    if let Some(mut text_node) = self.text_nodes.get(&bitmap_id).cloned() {
                        let flags = values.first().copied().unwrap_or_default();
                        let (bitmap_width, bitmap_height) = self
                            .graph_surfaces
                            .get(&bitmap_id)
                            .map(|surface| (surface.width, surface.height))
                            .unwrap_or((0.0, 0.0));
                        text_node.screen_attached = true;
                        text_node.x += values.get(5).copied().unwrap_or_default() as f32;
                        text_node.y += values.get(4).copied().unwrap_or_default() as f32;
                        if flags & 0x4 == 0 {
                            text_node.x -= bitmap_width;
                        }
                        if flags & 0x2 != 0 {
                            text_node.y -= bitmap_height;
                        }
                        text_node.z = values.first().copied().unwrap_or(NATIVE_DISPLAY_Z);
                        self.text_nodes.insert(node, text_node);
                        self.graph_layers.remove(&node);
                        trace_graph!(self,
                            "configure text node #{node} bitmap=#{bitmap_id} x={:.0} y={:.0} flags=0x{flags:X} values={values:?}",
                            self.text_nodes.get(&node).map(|node| node.x).unwrap_or_default(),
                            self.text_nodes.get(&node).map(|node| node.y).unwrap_or_default()
                        );
                    }
                    if let Some((key, region)) = self
                        .resource_image_region(bitmap_id)
                        .map(|(key, region)| (key.to_string(), region))
                    {
                        let x = values.get(5).copied().unwrap_or_default() as f32;
                        let y = values.get(4).copied().unwrap_or_default() as f32;
                        self.text_nodes.remove(&node);
                        self.graph_layers.insert(
                            node,
                            RuntimeGraphLayer {
                                hit_id: node,
                                owner_object: self.current_graph_object,
                                key: key.clone(),
                                target_surface: None,
                                x,
                                y,
                                width: region.width,
                                height: region.height,
                                src_x: region.x,
                                src_y: region.y,
                                opacity: 1.0,
                                // sub_47C1E0 pops the native display priority
                                // last, so it is the first reverse-pop value.
                                z: values.first().copied().unwrap_or(NATIVE_DISPLAY_Z),
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
                        if let Some(owner) = self.current_graph_object {
                            self.graph_object_layers
                                .entry(owner)
                                .or_default()
                                .insert(node);
                        }
                        trace_graph!(self,
                            "configure image node #{node} bitmap=#{bitmap_id} key={key} pos=({x:.0},{y:.0}) src=({:.0},{:.0} {}x{}) values={values:?}",
                            region.x,
                            region.y,
                            region.width,
                            region.height
                        );
                    }
                }
                tracing::debug!(args = ?summarize_values(&args), "GraphNodeConfigure");
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
                if let Some(native) =
                    NativeImageNodeArgs::from_popped(&ints).filter(|native| native.node_id > 0)
                {
                    let node_id = native.node_id;
                    let resource_id = native.resource_id;
                    if self.resolve_resource_key(resource_id).is_some() {
                        if let Some((key, region)) = self
                            .resource_image_region(resource_id)
                            .map(|(key, region)| (key.to_string(), region))
                        {
                            let width = region.width;
                            let height = region.height;
                            let viewport_width = self.screen_width.max(1) as f32;
                            let viewport_height = self.screen_height.max(1) as f32;
                            let (x, y) = native.screen_position(
                                width,
                                height,
                                viewport_width,
                                viewport_height,
                            );
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
                                    src_x: region.x,
                                    src_y: region.y,
                                    opacity: 1.0,
                                    // Both 0x56 and 0x5C create nodes in the
                                    // same native display chain. Opcode-local
                                    // z bands let an older 0x5C background
                                    // cover every later 0x56 child window.
                                    z: NATIVE_DISPLAY_Z,
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
                            if let Some(object) = self.current_graph_object {
                                self.graph_object_layers
                                    .entry(object)
                                    .or_default()
                                    .insert(node_id);
                            }
                            trace_graph!(self,
                                    "node image #{node_id} res=#{resource_id} {key} x={x:.0} y={y:.0} origin=({:.1},{:.1},{:.1}) w={width:.0} h={height:.0}",
                                    fixed_16_to_f32(native.origin_x),
                                    fixed_16_to_f32(native.origin_y),
                                    fixed_16_to_f32(native.origin_z)
                                );
                        }
                    }
                }
                tracing::info!(?args, "GraphNodeConfigureImage");
            }
            (0x90, 0x5d) => {
                let args = pop_args(stack, 12);
                let ints = args.iter().map(value_to_i32).collect::<Vec<_>>();
                let duration = ints
                    .iter()
                    .copied()
                    .filter(|value| (1..=10_000).contains(value))
                    .max()
                    .unwrap_or_default();
                if duration > 0 {
                    self.animation_queue_remaining = self
                        .animation_queue_remaining
                        .max(duration.unsigned_abs().div_ceil(16).max(1));
                }
                trace_graph!(
                    self,
                    "node transition pattern duration={duration} raw={ints:?}"
                );
                tracing::debug!(
                    args = ?summarize_values(&args),
                    duration,
                    "GraphNodeTransitionPattern"
                );
            }
            (0x90, 0x60) => {
                let handle = self.alloc_object();
                self.set_current_graph_object(handle);
                tracing::info!(handle, "GraphCreateObject");
                trace_graph!(self, "create object #{handle}");
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
                        trace_graph!(self, "surface/object #{object_id} enabled={enabled_value}");
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
                    trace_graph!(
                        self,
                        "object #{object} configure args={:?}",
                        args.iter().map(value_to_i32).collect::<Vec<_>>()
                    );
                }
                tracing::info!(args = ?summarize_values(&args), "GraphObjectConfigure");
            }
            (0x90, 0x66) => {
                let args = pop_args(stack, 7);
                self.apply_graph_object_effect(&args);
                tracing::debug!(
                    args = ?summarize_values(&args),
                    "GraphObjectApplyEffect"
                );
            }
            (0x90, 0x80) => {
                let height = stack.pop();
                let width = stack.pop();
                let handle = self.alloc_surface();
                let width_value = width.as_ref().map(value_to_i32).unwrap_or_default().max(1);
                let height_value = height.as_ref().map(value_to_i32).unwrap_or_default().max(1);
                self.graph_surfaces.insert(
                    handle,
                    RuntimeSurface::display(handle, width_value as f32, height_value as f32),
                );
                tracing::info!(handle, ?width, ?height, "GraphCreateSurface");
                trace_graph!(
                    self,
                    "create surface #{handle} width={width_value} height={height_value}"
                );
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
                    trace_graph!(self, "surface #{surface_id} enabled={enabled_value}");
                }
                tracing::info!(?surface, ?enabled, "GraphSurfaceSetEnabled");
            }
            (0x90, 0x85) => {
                let args = pop_args(stack, 7);
                if args.len() >= 7 {
                    let surface_id = value_to_i32(&args[6]);
                    let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                    let trace = self.graph_surfaces.get_mut(&surface_id).map(|surface| {
                        surface.z = values[0];
                        format!("surface #{} region args={:?}", surface.id, values)
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
                        surface.viewport_x = x;
                        surface.viewport_y = y;
                        surface.viewport_width = width;
                        surface.viewport_height = height;
                        Some(format!(
                            "surface #{} viewport src=({}, {}) size={}x{} display=({}, {}) backing={}x{}",
                            surface.id,
                            surface.viewport_x,
                            surface.viewport_y,
                            surface.viewport_width,
                            surface.viewport_height,
                            surface.x,
                            surface.y,
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
            (0x90, 0x89) => {
                let args = pop_args(stack, 2);
                let target = args.first().map(value_to_i32).unwrap_or_default();
                let missing = !self.graph_handle_exists(target);
                tracing::debug!(args = ?summarize_values(&args), missing, "GraphObjectTextLookup");
                return Ok(ethornell_vm::Value::Int(i32::from(missing)));
            }
            (0x90, 0x90) => {
                let args = pop_args(stack, 6);
                let text = args.iter().find_map(|value| match value {
                    ethornell_vm::Value::Str(text) => Some(text.clone()),
                    _ => None,
                });
                match text.as_deref() {
                    Some("\u{c}") | Some("\\c") => {
                        self.frame_yield_requested = true;
                        self.trace_graph("text draw control \\c");
                    }
                    Some(text) if !text.is_empty() => {
                        self.render_graph_text(&args);
                        trace_graph!(self, "text draw control text={text:?}");
                    }
                    _ => {
                        trace_graph!(
                            self,
                            "text draw control raw={:?}",
                            args.iter().map(value_to_i32).collect::<Vec<_>>()
                        );
                    }
                }
                self.pending_graph_procedure_schedule =
                    Some(ethornell_vm::GraphProcedureSchedule {
                        duration_ms: 1,
                        input_enabled: false,
                        input_descriptor: 0,
                        wait_for_input: false,
                        completion: ethornell_vm::GraphProcedureCompletion::None,
                    });
                tracing::debug!(
                    args = ?summarize_values(&args),
                    ?text,
                    "GraphTextDrawControl"
                );
                if text.as_deref().is_some_and(|text| !text.is_empty()) {
                    tracing::info!(?text, args = ?summarize_values(&args), "GraphTextDrawControlText");
                }
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
                        self.title_ui_resources_loaded = true;
                    }
                    if !self.title_ui_departed {
                        self.title_ui_active = true;
                    }
                }
            }
            (0x90, 0x11) => {
                let format = stack.pop();
                let height_arg = stack.pop();
                let width_arg = stack.pop();
                let bitmap = stack.pop();
                let bitmap_id = bitmap.as_ref().map(value_to_i32).unwrap_or_default();
                let width = width_arg.as_ref().map(value_to_i32).unwrap_or_default();
                let height = height_arg.as_ref().map(value_to_i32).unwrap_or_default();
                let format_id = format.as_ref().map(value_to_i32).unwrap_or_default();
                if bitmap_id > 0 && width > 0 && height > 0 {
                    let previous_resource = self
                        .graph_surfaces
                        .get(&bitmap_id)
                        .and_then(|surface| surface.resource_id);
                    let mut surface =
                        RuntimeSurface::bitmap(bitmap_id, width as f32, height as f32);
                    surface.resource_id = previous_resource;
                    self.graph_surfaces.insert(bitmap_id, surface);
                    self.bitmap_dimensions
                        .insert(bitmap_id, (width as u32, height as u32));
                    if format_id == 6 {
                        self.effects
                            .create_vector_map(bitmap_id, width as u32, height as u32);
                    } else {
                        self.effects.remove_bitmap(bitmap_id);
                    }
                    trace_graph!(
                        self,
                        "create bitmap #{bitmap_id} {width}x{height} format={format:?}"
                    );
                }
                if bitmap_id < 128 {
                    tracing::info!(bitmap_id, width, height, ?format, "GraphCreateBitmap");
                } else {
                    tracing::debug!(bitmap_id, width, height, ?format, "GraphCreateBitmap");
                }
            }
            (0x90, 0x12) => {
                let bitmap_id = pop_int_value(stack).unwrap_or_default();
                self.remove_surface_control_layers(bitmap_id);
                self.surface_text_states.remove(&bitmap_id);
                self.surface_text_buffers.remove(&bitmap_id);
                self.detach_graph_surface_relations(bitmap_id);
                self.effects.remove_bitmap(bitmap_id);
                self.bitmap_dimensions.remove(&bitmap_id);
                let existed = self.graph_surfaces.remove(&bitmap_id).is_some()
                    | self.graph_resources.remove(&bitmap_id).is_some();
                tracing::debug!(bitmap_id, existed, "GraphReleaseBitmap");
                return Ok(ethornell_vm::Value::Int(i32::from(existed)));
            }
            (0x90, 0x13) => {
                let color = pop_int_value(stack).unwrap_or_default();
                let bitmap_id = pop_int_value(stack).unwrap_or_default();
                if color == 0 {
                    self.effects.clear_vector_map(bitmap_id);
                }
                tracing::debug!(bitmap_id, color, "GraphClearBitmap");
            }
            (0x90, 0x16) => {
                let bitmap = stack.pop();
                let output = stack.pop();
                let bitmap_id = bitmap.as_ref().map(value_to_i32).unwrap_or_default();
                let found = self.resolve_resource_key(bitmap_id).is_some();
                tracing::debug!(?output, ?bitmap, found, "BitmapQueryInfo");
                trace_graph!(
                    self,
                    "bitmap info query bitmap=#{bitmap_id} output={output:?} found={found}"
                );
                return Ok(ethornell_vm::Value::Int(i32::from(found)));
            }
            (0x90, 0x17) => {
                let args = pop_args(stack, 2);
                let valid = args.iter().any(|value| match value {
                    ethornell_vm::Value::Int(value) => *value > 0,
                    ethornell_vm::Value::Ptr(value) => *value != 0,
                    ethornell_vm::Value::Str(value) => !value.is_empty(),
                    ethornell_vm::Value::Func { .. }
                    | ethornell_vm::Value::Program(_)
                    | ethornell_vm::Value::None => false,
                });
                tracing::debug!(args = ?summarize_values(&args), valid, "GraphValidateBitmap");
                return Ok(ethornell_vm::Value::Int(i32::from(valid)));
            }
            (0x90, 0x18) => {
                let args = pop_args(stack, 6);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                if values.len() == 6 {
                    let destination = values[5];
                    let source = values[2];
                    let x = values[4];
                    let y = values[3];
                    let alpha = values[1];
                    let mode = values[0];
                    let copied_vector = self.effects.copy_vector_map(destination, source, x, y);
                    if let Some(key) =
                        self.composite_graph_bitmap(destination, source, x, y, mode, alpha)
                    {
                        trace_graph!(self,
                            "bitmap apply source=#{source} destination=#{destination} key={key} x={x} y={y} alpha={alpha} mode={mode} values={values:?}"
                        );
                    } else if copied_vector {
                        trace_graph!(self,
                            "vector-map apply source=#{source} destination=#{destination} x={x} y={y} alpha={alpha} mode={mode}"
                        );
                    }
                }
                tracing::debug!(args = ?summarize_values(&args), "GraphObjectApply");
            }
            (0x90, 0x20) => {
                let args = pop_args(stack, 6);
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
                            trace_graph!(self,
                                "object #{object} transition alpha={alpha:.2} duration_ms={duration_ms} frames={duration_frames} layers={layer_count}"
                            );
                        }
                    }
                }
                tracing::info!(?args, "GraphObjectTransition");
            }
            (0x90, 0x1f) => {
                let args = pop_args(stack, 6);
                tracing::debug!(args = ?summarize_values(&args), "GraphConfigureBitmapRegion");
                let ints: Vec<i32> = args.iter().map(value_to_i32).collect();
                if let [height, width, y, x, source, destination] = ints.as_slice() {
                    if *width <= 0 || *height <= 0 {
                        tracing::warn!(
                            source,
                            destination,
                            width,
                            height,
                            values = ?ints,
                            "native bitmap region has invalid dimensions"
                        );
                    } else if let Some(resource) = self.resolve_graph_resource(*source).cloned() {
                        let region = resource.subregion(*x, *y, *width, *height);
                        let key = region.key.clone();
                        self.graph_resources.insert(*destination, region);
                        trace_graph!(self,
                            "bitmap region destination=#{destination} source=#{source} {key} src=({x},{y} {width}x{height})"
                        );
                    } else if let Some(source_image) = self.graph_bitmap_image(*source) {
                        let image = crop_decoded_image(
                            &source_image,
                            RuntimeClipRect {
                                x: *x as f32,
                                y: *y as f32,
                                width: *width as f32,
                                height: *height as f32,
                            },
                        );
                        let key = format!("runtime:bitmap-region:{destination}");
                        self.store_graph_image(key.clone(), image);
                        self.graph_resources
                            .insert(*destination, RuntimeGraphResource::whole(key));
                        self.bitmap_dimensions
                            .insert(*destination, (*width as u32, *height as u32));
                    } else {
                        trace_graph!(self,
                            "bitmap region skipped missing source=#{source} destination=#{destination} src=({x},{y} {width}x{height})"
                        );
                    }
                }
            }
            (0x90, 0x32) => {
                let value = stack.pop();
                let object = stack.pop();
                let object_id = object.as_ref().map(value_to_i32).unwrap_or_default();
                let alpha_parameter = value.as_ref().map(value_to_i32).unwrap_or_default();
                if object_id > 0 {
                    if let Some(transition) = self.graph_transition_nodes.get_mut(&object_id) {
                        transition.alpha_parameter = alpha_parameter.clamp(0, 256);
                        let key = self.update_transition_node_image(object_id);
                        trace_graph!(
                            self,
                            "transition node #{object_id} alpha={} key={key:?}",
                            alpha_parameter.clamp(0, 256)
                        );
                    } else {
                        self.graph_object_properties
                            .entry(object_id)
                            .or_default()
                            .set_property(2, alpha_parameter.clamp(0, 256), 0);
                    }
                }
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
                let delay_ms = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults.text_animation.set_glyph_delay(delay_ms);
                self.text_runtime.set_glyph_delay_ms(delay_ms);
                tracing::info!(delay_ms, "GraphSetGlyphRevealDelay");
            }
            (0x90, 0x96) => {
                let step_delay_ms = pop_int_value(stack).unwrap_or_default();
                let steps = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults
                    .text_animation
                    .set_settle(steps, step_delay_ms);
                tracing::info!(steps, step_delay_ms, "GraphSetTextSettleAnimation");
            }
            (0x90, 0x97) => {
                let delay_ms = pop_int_value(stack).unwrap_or_default();
                let enabled = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults
                    .text_animation
                    .set_auto_advance(enabled, delay_ms);
                tracing::info!(enabled, delay_ms, "GraphSetTextAutoAdvance");
            }
            (0x90, 0x9c) => {
                let value = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults.shadow_enabled = value != 0;
                tracing::info!(enabled = value != 0, "GraphSetTextShadowEnabled");
            }
            (0x90, 0x9b) => {
                let delay_ms = pop_int_value(stack).unwrap_or_default();
                let enabled = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults
                    .configure_message_delay(enabled, delay_ms);
                tracing::info!(enabled, delay_ms, "GraphSetMessageStartDelay");
            }
            (0x90, 0x95) => {
                let step_delay_ms = pop_int_value(stack).unwrap_or_default();
                let steps = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults
                    .text_animation
                    .set_reveal(steps, step_delay_ms);
                tracing::info!(steps, step_delay_ms, "GraphSetTextRevealAnimation");
            }
            (0x90, 0x9f) => {
                let value = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults.instant_reveal = value != 0;
                tracing::info!(enabled = value != 0, "GraphSetInstantTextReveal");
            }
            (0x90, 0xaf) => {
                let value = pop_int_value(stack).unwrap_or_default();
                self.input_requires_focus = value != 0;
                tracing::info!(
                    requires_focus = self.input_requires_focus,
                    "GraphSetForegroundInputGuard"
                );
            }
            (0x90, 0xb6) => {
                let args = pop_args(stack, 2);
                let descriptor = args.first().map(value_to_i32).unwrap_or_default();
                let object = args.get(1).map(value_to_i32).unwrap_or_default();
                if object != 0 {
                    self.graph_object_input_tables.insert(object, descriptor);
                }
                tracing::debug!(object, descriptor, "GraphObjectBindInputTable");
                trace_graph!(self, "object #{object} bind input table 0x{descriptor:08X}");
            }
            (0x90, 0xb8) => {
                let layer = pop_int_value(stack).unwrap_or_default();
                let object = self.alloc_object();
                self.graph_input_objects
                    .insert(object, RuntimeGraphInputObject::new(layer));
                tracing::debug!(layer, object, "GraphObjectBeginInput");
                trace_graph!(
                    self,
                    "create input object #{object} for graph object #{layer}"
                );
                return Ok(ethornell_vm::Value::Int(object));
            }
            (0x90, 0xb7) => {
                let state = stack.pop();
                let surface = stack.pop();
                tracing::info!(?surface, ?state, "GraphConfigureSurfaceControls");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x90, 0xb9) => {
                let object = stack.pop();
                let object_id = object.as_ref().map(value_to_i32).unwrap_or_default();
                let removed = self.graph_input_objects.remove(&object_id).is_some();
                tracing::info!(?object, "GraphObjectFinalize");
                return Ok(ethornell_vm::Value::Int(i32::from(removed)));
            }
            (0x90, 0xba) => {
                let args = pop_args(stack, 2);
                let descriptor = args.first().map(value_to_i32).unwrap_or_default();
                let object = args.get(1).map(value_to_i32).unwrap_or_default();
                if object > 0 {
                    self.graph_object_input_tables.insert(object, descriptor);
                }
                tracing::debug!(object, descriptor, "GraphObjectAttachInput");
                trace_graph!(
                    self,
                    "object #{object} attach input descriptor=0x{descriptor:08X}"
                );
            }
            (0x90, 0xd0) => {
                let target = stack.pop();
                let target_id = target.as_ref().map(value_to_i32).unwrap_or_default();
                let handle = self.alloc_object();
                let (base_x, base_y) = self
                    .graph_layers
                    .get(&target_id)
                    .map(|layer| (layer.x, layer.y))
                    .unwrap_or_default();
                self.graph_scroll_states
                    .insert(handle, GraphScrollState::new(target_id, base_x, base_y));
                tracing::debug!(?target, target_id, handle, "GraphCreateScrollState");
                return Ok(ethornell_vm::Value::Int(handle));
            }
            (0x90, 0xd1) => {
                let target = stack.pop();
                let target_id = target.as_ref().map(value_to_i32).unwrap_or_default();
                self.graph_scroll_states.remove(&target_id);
                tracing::debug!(?target, target_id, "GraphReleaseScrollState");
            }
            (0x90, 0xd4) => {
                let args = pop_args(stack, 2);
                let mode = args.first().map(value_to_i32).unwrap_or_default();
                let handle = args.get(1).map(value_to_i32).unwrap_or_default();
                if let Some(state) = self.graph_scroll_states.get_mut(&handle) {
                    state.mode = mode;
                }
                tracing::debug!(handle, mode, "GraphScrollSetMode");
            }
            (0x90, 0xd5) => {
                let args = pop_args(stack, 3);
                let handle = args.get(2).map(value_to_i32).unwrap_or_default();
                tracing::debug!(?args, handle, "GraphScrollCommit");
            }
            (0x90, 0xd6) => {
                let args = pop_args(stack, 3);
                let handle = args.get(2).map(value_to_i32).unwrap_or_default();
                let x = args.get(1).map(value_to_i32).unwrap_or_default();
                let y = args.first().map(value_to_i32).unwrap_or_default();
                if let Some(state) = self.graph_scroll_states.get_mut(&handle) {
                    state.x = x;
                    state.y = y;
                }
                self.apply_graph_scroll_position(handle);
                tracing::debug!(?args, handle, x, y, "GraphScrollSetPosition");
            }
            (0x90, 0xd7) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                let state = self
                    .graph_scroll_states
                    .get(&handle)
                    .copied()
                    .unwrap_or_else(|| GraphScrollState::new(0, 0.0, 0.0));
                stack.push(ethornell_vm::Value::Int(state.x));
                stack.push(ethornell_vm::Value::Int(state.y));
                tracing::debug!(
                    handle,
                    x = state.x,
                    y = state.y,
                    extent_x = state.extent_x,
                    extent_y = state.extent_y,
                    bounds_width = state.bounds_width,
                    bounds_height = state.bounds_height,
                    "GraphScrollGetPosition"
                );
            }
            (0x90, 0xd8) => {
                let args = pop_args(stack, 3);
                let handle = args.get(2).map(value_to_i32).unwrap_or_default();
                let width = args.get(1).map(value_to_i32).unwrap_or_default();
                let height = args.first().map(value_to_i32).unwrap_or_default();
                if let Some(state) = self.graph_scroll_states.get_mut(&handle) {
                    state.extent_x = width;
                    state.extent_y = height;
                }
                self.apply_graph_scroll_position(handle);
                tracing::debug!(?args, handle, width, height, "GraphScrollSetExtent");
            }
            (0x90, 0xd9) => {
                let args = pop_args(stack, 3);
                let handle = args.get(2).map(value_to_i32).unwrap_or_default();
                let width = args.get(1).map(value_to_i32).unwrap_or_default();
                let height = args.first().map(value_to_i32).unwrap_or_default();
                if let Some(state) = self.graph_scroll_states.get_mut(&handle) {
                    state.bounds_width = width;
                    state.bounds_height = height;
                }
                self.apply_graph_scroll_position(handle);
                tracing::debug!(?args, handle, width, height, "GraphScrollSetBounds");
            }
            (0x90, 0xdd) => {
                let value = pop_int_value(stack).unwrap_or_default();
                let previous = std::mem::replace(&mut self.graph_driver_mode, value);
                tracing::info!(value, previous, "GraphSetDriverMode");
                return Ok(ethornell_vm::Value::Int(previous));
            }
            (0x90, 0xbc) => {
                let object = stack.pop();
                let state_buffer = stack.pop();
                tracing::info!(?state_buffer, ?object, "GraphPollObjectState");
            }
            (0x90, 0xbe) => {
                let args = pop_args(stack, 2);
                let out = args.first().map(value_to_i32).unwrap_or_default();
                let object = args.get(1).map(value_to_i32).unwrap_or_default();
                let value = if self.last_hit_control != 0 {
                    self.last_hit_payload
                } else {
                    -1
                };
                tracing::debug!(object, out, value, "GraphObjectResolveInput");
                trace_graph!(
                    self,
                    "object #{object} resolve input out=0x{out:08X} value={value}"
                );
                return Ok(ethornell_vm::Value::Int(value));
            }
            (0x90, 0xbf) => {
                let object = stack.pop();
                let event_buffer = stack.pop();
                tracing::info!(?event_buffer, ?object, "GraphPollObjectEvent");
            }
            (0x90, 0xcc) => {
                let args = pop_args(stack, 2);
                tracing::debug!(args = ?summarize_values(&args), "GraphColorAdjustPrepare");
                trace_graph!(
                    self,
                    "color adjust prepare raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
            }
            (0x90, 0xcd) => {
                let args = pop_args(stack, 8);
                self.apply_graph_blit_rect(&args);
                tracing::debug!(args = ?summarize_values(&args), "GraphColorAdjustedBlit");
            }
            (0x90, 0xe0) => {
                let handle = self.alloc_timeline();
                self.timelines.create(handle);
                tracing::info!(handle, "GraphCreateTimeline");
                trace_graph!(self, "create timeline #{handle}");
                return Ok(ethornell_vm::Value::Int(handle));
            }
            (0x90, 0xe1) => {
                let timeline = stack.pop();
                let handle = timeline.as_ref().map(value_to_i32).unwrap_or_default();
                let poll = self.timelines.poll(handle);
                tracing::info!(?timeline, ?poll, "GraphTimelinePoll");
                if self.debug_graph {
                    trace_graph!(
                        self,
                        "timeline #{handle} poll active={} finished={} remaining={}",
                        poll.active,
                        poll.finished,
                        poll.remaining
                    );
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
            (0x91, 0x10) => {
                let args = pop_args(stack, 5);
                self.apply_graph_effect_config(0x10, &args);
                tracing::debug!(args = ?summarize_values(&args), "GraphEffectSetZoomRect");
            }
            (0x91, 0x11) => {
                let args = pop_args(stack, 2);
                self.apply_graph_effect_config(0x11, &args);
                tracing::debug!(args = ?summarize_values(&args), "GraphEffectSetDiffuse");
            }
            (0x91, 0x12) => {
                let args = pop_args(stack, 6);
                self.apply_graph_effect_config(0x12, &args);
                tracing::debug!(args = ?summarize_values(&args), "GraphEffectSetColorTone");
            }
            (0x91, 0x13) => {
                let args = pop_args(stack, 5);
                self.apply_graph_effect_config(0x13, &args);
                tracing::debug!(args = ?summarize_values(&args), "GraphEffectSetRotation");
            }
            (0x91, 0x15) => {
                let args = pop_args(stack, 5);
                self.apply_graph_effect_config(0x15, &args);
                tracing::debug!(args = ?summarize_values(&args), "GraphEffectSetWave");
            }
            (0x91, 0x16) => {
                let args = pop_args(stack, 7);
                self.apply_graph_effect_config(0x16, &args);
                tracing::debug!(args = ?summarize_values(&args), "GraphEffectSetClipRect");
            }
            (0x91, 0x06) => {
                // sub_4806D0 -> sub_461E40 -> sub_442EC0 stores the two
                // global CDspObj offsets. sub_41B260 adds them during final
                // screen-coordinate resolution for objects using the native
                // (default-enabled) global-offset flag.
                let y = pop_int_value(stack).unwrap_or_default() as f32;
                let x = pop_int_value(stack).unwrap_or_default() as f32;
                self.graph_global_offset = (x, y);
                tracing::debug!(x, y, "GraphSetGlobalDisplayOffset");
                trace_graph!(self, "global display offset x={x:.1} y={y:.1}");
            }
            (0x91, 0x1e) => {
                // funcs_504F00[0x1E] -> sub_4817F0 validates the first two
                // script handles and copies their backing image state.
                let args = pop_args(stack, 4);
                let destination = args.get(2).map(value_to_i32).unwrap_or_default();
                let source = args.get(3).map(value_to_i32).unwrap_or_default();
                self.copy_graph_backing(source, destination);
                tracing::debug!(source, destination, args = ?summarize_values(&args), "GraphCopyBackingState");
            }
            (0x91, 0x1f) => {
                let source = pop_int_value(stack).unwrap_or_default();
                let destination = pop_int_value(stack).unwrap_or_default();
                if let Some(mut surface) = self.graph_surfaces.get(&source).cloned() {
                    surface.id = destination;
                    self.graph_surfaces.insert(destination, surface);
                }
                if let Some(key) = self.graph_resources.get(&source).cloned() {
                    self.graph_resources.insert(destination, key);
                }
                tracing::debug!(source, destination, "GraphCloneBitmap");
            }
            (0x91, 0x55) => {
                let args = pop_args(stack, 2);
                let target = args.first().map(value_to_i32).unwrap_or_default();
                let source = args.get(1).map(value_to_i32).unwrap_or_default();
                tracing::debug!(source, target, "GraphLayerLink");
                trace_graph!(self, "layer link source=#{source} target=#{target}");
                // sub_4824D0 pushes sub_462530's status. Zero is the native
                // success result; 11 and 13..15 describe invalid link graphs.
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x91, 0x60) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                if handle > 0 {
                    self.graph_object_enabled.insert(handle, true);
                }
                tracing::debug!(handle, "GraphTempLayerCreate");
                trace_graph!(self, "temp layer create #{handle}");
            }
            (0x91, 0x61) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                if handle > 0 {
                    self.remove_surface_control_layers(handle);
                    self.graph_layers.remove(&handle);
                    self.detach_graph_surface_relations(handle);
                    self.graph_surfaces.remove(&handle);
                    self.surface_text_states.remove(&handle);
                    self.surface_text_buffers.remove(&handle);
                    self.graph_object_layers.remove(&handle);
                    self.graph_object_enabled.remove(&handle);
                    self.layer_animations.clear_layers(std::iter::once(&handle));
                }
                tracing::debug!(handle, "GraphTempLayerRelease");
                trace_graph!(self, "temp layer release #{handle}");
            }
            (0x91, 0x64) => {
                let args = pop_args(stack, 2);
                let enabled = args.first().map(value_to_i32).unwrap_or_default() != 0;
                let target = args.get(1).map(value_to_i32).unwrap_or_default();
                if target > 0 {
                    self.set_graph_object_enabled(target, enabled);
                    if let Some(layer) = self.graph_layers.get_mut(&target) {
                        layer.enabled = enabled;
                    }
                    if let Some(surface) = self.graph_surfaces.get_mut(&target) {
                        surface.enabled = enabled;
                    }
                }
                tracing::debug!(target, enabled, "GraphTempLayerSetEnabled");
                trace_graph!(self, "temp layer #{target} enabled={enabled}");
            }
            (0x91, 0x65) => {
                let args = pop_args(stack, 6);
                tracing::debug!(args = ?summarize_values(&args), "GraphTempLayerBlit");
                trace_graph!(
                    self,
                    "temp layer blit raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
            }
            (0x91, 0x66) => {
                let args = pop_args(stack, 4);
                tracing::debug!(args = ?summarize_values(&args), "GraphTempLayerCopy");
                trace_graph!(
                    self,
                    "temp layer copy raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
            }
            (0x91, 0x1d) => {
                let args = pop_args(stack, 5);
                self.apply_graph_layer_color_blend(&args);
                tracing::debug!(args = ?summarize_values(&args), "GraphLayerColorBlend");
            }
            (0x91, 0x33) => {
                let args = pop_args(stack, 4);
                let ints = args.iter().map(value_to_i32).collect::<Vec<_>>();
                if ints.len() >= 4 {
                    let object = ints[3];
                    let x = fixed_16_to_f32(ints[2]);
                    let y = fixed_16_to_f32(ints[1]);
                    let layer_ids = self.graph_target_layers(object);
                    for layer_id in &layer_ids {
                        if let Some(layer) = self.graph_layers.get_mut(layer_id) {
                            layer.x = x;
                            layer.y = y;
                        }
                    }
                    trace_graph!(self, "object #{object} fixed position x={x:.1} y={y:.1} snap={} layers={layer_ids:?}", ints[0]);
                }
                tracing::debug!(args = ?summarize_values(&args), "GraphObjectSetMotionPoint");
            }
            (0x91, 0x3e) => {
                let args = pop_args(stack, 4);
                let y = args.first().map(value_to_i32).unwrap_or_default() as f32;
                let x = args.get(1).map(value_to_i32).unwrap_or_default() as f32;
                let child = args.get(2).map(value_to_i32).unwrap_or_default();
                let parent = args.get(3).map(value_to_i32).unwrap_or_default();
                let attached = self.attach_graph_surface(parent, child, x, y);
                let attached_kind = self
                    .graph_surfaces
                    .get(&child)
                    .filter(|surface| surface.parent_surface == Some(parent))
                    .map(|_| "surface")
                    .or_else(|| {
                        self.graph_layers
                            .get(&child)
                            .filter(|layer| layer.target_surface == Some(parent))
                            .map(|_| "layer")
                    })
                    .or_else(|| {
                        self.text_nodes
                            .get(&child)
                            .filter(|node| node.target_surface == Some(parent))
                            .map(|_| "text")
                    });
                tracing::debug!(parent, child, x, y, attached, "GraphAttachSurface");
                trace_graph!(
                    self,
                    "attach surface #{child} to #{parent} local=({x:.0},{y:.0}) attached={attached} kind={attached_kind:?}"
                );
                return Ok(ethornell_vm::Value::None);
            }
            (0x91, 0x3f) => {
                let child = pop_int_value(stack).unwrap_or_default();
                let parent = pop_int_value(stack).unwrap_or_default();
                let detached = self.detach_graph_surface(parent, child);
                if parent != 0 && child != 0 {
                    self.graph_bindings.remove(&parent);
                }
                tracing::debug!(parent, child, detached, "GraphDetachSurface");
                trace_graph!(
                    self,
                    "detach surface #{child} from #{parent} detached={detached}"
                );
            }
            (0x91, 0x40) => {
                let args = pop_args(stack, 9);
                self.apply_graph_affine_transform(&args);
                tracing::debug!(
                    args = ?summarize_values(&args),
                    "GraphLayerAffineTransform"
                );
            }
            (0x91, 0x48) | (0x91, 0x49) | (0x91, 0x4a) => {
                // sub_482310/sub_4823A0/sub_482430 configure one of eight
                // native draw-context vector slots. They do not directly
                // mutate a display object.
                let argc = known_call_arg_count(group, id).unwrap_or_default();
                let args = pop_args(stack, argc);
                tracing::debug!(id, args = ?summarize_values(&args), "GraphConfigureDrawVector");
                trace_graph!(
                    self,
                    "draw vector 0x{id:02X} raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
            }
            (0x91, 0x38) => {
                let property = pop_int_value(stack).unwrap_or_default();
                let object = pop_int_value(stack).unwrap_or_default();
                let value = pop_int_value(stack).unwrap_or_default();
                self.graph_object_properties
                    .entry(object)
                    .or_default()
                    .set_property(property as u32, value, 0);
                tracing::debug!(object, property, value, "GraphConfigureResourceProperty");
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
                let mode = pop_int_value(stack).unwrap_or_default();
                let line_height = pop_int_value(stack).unwrap_or_default();
                let font_size = pop_int_value(stack).unwrap_or_default();
                let reserved = pop_int_value(stack).unwrap_or_default();
                let extent_y = pop_int_value(stack).unwrap_or_default();
                let extent_x = pop_int_value(stack).unwrap_or_default();
                let args = [extent_x, extent_y, reserved, font_size, line_height, mode];
                let result = self.graph_defaults.text_layout.configure(args);
                if result.is_ok() {
                    self.text_state.font_size = font_size as f32;
                    if line_height > 0 {
                        self.text_state.line_height = line_height as f32;
                    }
                }
                tracing::info!(?args, ?result, "GraphConfigureTextLayoutDefaults");
            }
            (0x92, 0x97) => {
                let value_6 = pop_int_value(stack).unwrap_or_default();
                let value_5 = pop_int_value(stack).unwrap_or_default();
                let value_4 = pop_int_value(stack).unwrap_or_default();
                let value_3 = pop_int_value(stack).unwrap_or_default();
                let value_2 = pop_int_value(stack).unwrap_or_default();
                let value_1 = pop_int_value(stack).unwrap_or_default();
                let reserved = pop_int_value(stack).unwrap_or_default();
                let values = [value_1, value_2, value_3, value_4, value_5, value_6];
                self.graph_defaults.configure_text_style(None, values);
                tracing::info!(reserved, ?values, "GraphConfigureTextStyleDefaults");
            }
            (0x92, 0x88) => {
                let value = pop_int_value(stack).unwrap_or_default();
                let object = pop_int_value(stack).unwrap_or_default();
                self.graph_object_properties
                    .entry(object)
                    .or_default()
                    .properties
                    .insert(0x88, (value, 0));
                tracing::debug!(object, value, "GraphSetTextObjectValue");
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
                let layer = pop_int_value(stack).unwrap_or_default();
                let object = self.alloc_object();
                self.graph_input_objects
                    .insert(object, RuntimeGraphInputObject::new(layer));
                tracing::debug!(layer, object, "GraphCreateSurfaceInputObject");
                trace_graph!(self, "create input object #{object} for surface #{layer}");
                return Ok(ethornell_vm::Value::Int(object));
            }
            (0x91, 0xba) => {
                let _descriptor = stack.pop();
                let _object = stack.pop();
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x91, 0x88) => {
                let args = pop_args(stack, 7);
                self.text_state = infer_text_state(&args);
                tracing::debug!(?args, inferred = ?self.text_state, "ConfigureFormatInfo");
            }
            (0x91, 0x89) => {
                let value = stack.pop();
                let target = stack.pop();
                tracing::info!(?target, ?value, "GraphLayerFormatOption");
            }
            (0x91, 0x94) => {
                let replacement = stack.pop().as_ref().and_then(value_to_optional_string);
                let source = stack.pop().as_ref().and_then(value_to_optional_string);
                self.graph_defaults
                    .update_text_substitution(source.clone(), replacement.clone());
                tracing::debug!(?source, ?replacement, "GraphUpdateRubySubstitution");
            }
            (0x91, 0x8b) => {
                let writing_mode = pop_int_value(stack).unwrap_or_default();
                let surface = pop_int_value(stack).unwrap_or_default();
                if writing_mode <= 2 && self.graph_surfaces.contains_key(&surface) {
                    self.surface_text_states
                        .entry(surface)
                        .or_default()
                        .writing_mode = writing_mode;
                }
                tracing::info!(surface, writing_mode, "GraphTextSetWritingMode");
            }
            (0x91, 0x8c) => {
                let y = pop_int_value(stack).unwrap_or_default();
                let x = pop_int_value(stack).unwrap_or_default();
                let surface = pop_int_value(stack).unwrap_or_default();
                if self.graph_surfaces.contains_key(&surface) {
                    let state = self.surface_text_states.entry(surface).or_default();
                    state.cursor_x = x;
                    state.cursor_y = y;
                }
                tracing::debug!(surface, x, y, "GraphTextSetCursorPosition");
            }
            (0x91, 0x8d) => {
                let surface = pop_int_value(stack).unwrap_or_default();
                let exists = self.graph_surfaces.contains_key(&surface);
                let state = self
                    .surface_text_states
                    .get(&surface)
                    .copied()
                    .unwrap_or_default();
                stack.push(ethornell_vm::Value::Int(i32::from(exists)));
                stack.push(ethornell_vm::Value::Int(state.cursor_x));
                tracing::debug!(surface, exists, ?state, "GraphTextGetCursorPosition");
                return Ok(ethornell_vm::Value::Int(state.cursor_y));
            }
            (0x91, 0x8e) => {
                let surface = pop_int_value(stack).unwrap_or_default();
                let state = self
                    .surface_text_states
                    .get(&surface)
                    .copied()
                    .unwrap_or_default();
                let reached = self.graph_surfaces.get(&surface).is_some_and(|target| {
                    if state.writing_mode == 1 {
                        state.cursor_y as f32 >= target.viewport_height
                    } else {
                        state.cursor_x as f32 >= target.viewport_width
                    }
                });
                tracing::debug!(surface, ?state, reached, "GraphTextCursorReachedBoundary");
                return Ok(ethornell_vm::Value::Int(i32::from(reached)));
            }
            (0x91, 0x96) => {
                let records = stack
                    .pop()
                    .as_ref()
                    .and_then(value_to_optional_string)
                    .unwrap_or_default();
                let parsed = self.graph_defaults.register_ruby_records(&records);
                tracing::debug!(records, parsed, "GraphRegisterRubySubstitutions");
                return Ok(ethornell_vm::Value::Int(i32::from(parsed)));
            }
            (0x92, 0x91) => {
                let args = pop_args(stack, 11);
                let text = args.get(9).and_then(value_to_string);
                let target = args.get(10).map(value_to_i32).unwrap_or_default();
                let inferred = infer_text_state(&args);
                if inferred.width > 0.0 && inferred.height > 0.0 {
                    self.text_state = inferred;
                }
                if self.title_ui_departed && self.native_message_surface_target == Some(target) {
                    if let Some(text) = text.as_deref().filter(|text| !text.is_empty()) {
                        let display_text = {
                            let buffer = self.surface_text_buffers.entry(target).or_default();
                            buffer.push_str(text);
                            buffer.clone()
                        };
                        self.render_native_message_text(&display_text);
                        let line_height = self.text_state.line_height.max(1.0) as i32;
                        let glyph_advance = (self.text_state.font_size * 0.5).max(1.0) as i32;
                        let state = self.surface_text_states.entry(target).or_default();
                        for character in text.chars() {
                            if character == '\n' {
                                state.cursor_x = 0;
                                state.cursor_y = state.cursor_y.saturating_add(line_height);
                            } else {
                                state.cursor_x = state.cursor_x.saturating_add(glyph_advance);
                            }
                        }
                    }
                }
                tracing::debug!(
                    target,
                    text,
                    args = ?summarize_values(&args),
                    "GraphDrawFormattedText"
                );
            }
            (0x92, 0x89) => {
                let blend_mode = pop_int_value(stack).unwrap_or_default();
                let opacity = pop_int_value(stack).unwrap_or_default();
                let resource = pop_int_value(stack).unwrap_or_default();
                let y = pop_int_value(stack).unwrap_or_default();
                let x = pop_int_value(stack).unwrap_or_default();
                let work_surface = pop_int_value(stack).unwrap_or_default();
                if self.resolve_resource_key(resource).is_some() {
                    if let Some(surface) = self.graph_surfaces.get_mut(&work_surface) {
                        surface.resource_id = Some(resource);
                    }
                }
                let message_surface = work_surface.saturating_sub(1);
                if self.graph_surfaces.contains_key(&message_surface) {
                    self.native_message_surface_target = Some(message_surface);
                }
                tracing::debug!(
                    work_surface,
                    x,
                    y,
                    resource,
                    opacity,
                    blend_mode,
                    "GraphDrawResourceToSurface"
                );
            }
            (0x92, 0x90) => {
                let args = pop_args(stack, 15);
                let text = args.get(13).and_then(value_to_string);
                let target = args.get(14).map(value_to_i32).unwrap_or_default();
                let inferred = infer_text_state(&args);
                if inferred.width > 0.0 && inferred.height > 0.0 {
                    self.text_state.x = inferred.x;
                    self.text_state.y = inferred.y;
                    self.text_state.width = inferred.width;
                    self.text_state.height = inferred.height;
                    self.text_state.font_size = inferred.font_size;
                    self.text_state.line_height = inferred.line_height;
                }
                let is_message_surface = self.native_message_surface_target == Some(target);
                if let Some(text) = text.as_ref().filter(|text| !text.is_empty()) {
                    let duration_ms = if self.title_ui_departed && is_message_surface {
                        self.start_native_message(text.clone())
                    } else {
                        1
                    };
                    self.pending_graph_procedure_schedule =
                        Some(ethornell_vm::GraphProcedureSchedule {
                            duration_ms,
                            input_enabled: self.title_ui_departed && is_message_surface,
                            input_descriptor: 0,
                            wait_for_input: false,
                            completion: ethornell_vm::GraphProcedureCompletion::MessageInterrupted,
                        });
                } else if self.title_ui_departed
                    && is_message_surface
                    && args.get(2).is_some_and(|value| value_to_i32(value) != 0)
                {
                    self.pending_graph_procedure_schedule =
                        Some(ethornell_vm::GraphProcedureSchedule {
                            duration_ms: 1,
                            input_enabled: true,
                            input_descriptor: 0,
                            wait_for_input: true,
                            completion: ethornell_vm::GraphProcedureCompletion::MessageInterrupted,
                        });
                }
                trace_graph!(
                    self,
                    "styled text draw state={:?} raw={:?}",
                    self.text_state,
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
                tracing::debug!(
                    args = ?summarize_values(&args),
                    state = ?self.text_state,
                    "GraphTextDrawStyled"
                );
                if text.as_deref().is_some_and(|text| !text.is_empty()) {
                    tracing::info!(?text, args = ?summarize_values(&args), "GraphTextDrawStyledText");
                }
            }
            (0x92, 0x8e) => {
                let target = pop_int_value(stack).unwrap_or_default();
                self.surface_text_states
                    .insert(target, SurfaceTextState::default());
                self.surface_text_buffers.insert(target, String::new());
                if self.native_message_surface_target == Some(target) {
                    self.reset_native_message_text();
                }
                tracing::debug!(target, "GraphResetTextSurface");
            }
            (0x92, 0x8c) => {
                // funcs_486FEE[0x8C] -> sub_486260 -> sub_440DC0 stores this
                // update flag in field 380 of the resolved graph object.
                let enabled = pop_int_value(stack).unwrap_or_default();
                let object = pop_int_value(stack).unwrap_or_default();
                self.graph_object_properties
                    .entry(object)
                    .or_default()
                    .properties
                    .insert(0x8c, (enabled, 0));
                tracing::debug!(object, enabled, "GraphSetObjectUpdateFlag");
            }
            (0x92, 0x17) => {
                // Native sub_485950 pops four values and calls sub_4025E0 to
                // read a pixel into the converted destination buffer.
                let args = pop_args(stack, 4);
                tracing::debug!(args = ?summarize_values(&args), "GraphReadBitmapPixel");
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x92, 0x18) => {
                // funcs_486FEE[0x18] -> sub_4859A0 -> sub_408320 copies a
                // type-1/2 image descriptor into a type-3 destination.
                let destination = pop_int_value(stack).unwrap_or_default();
                let source = pop_int_value(stack).unwrap_or_default();
                self.copy_graph_backing(source, destination);
                tracing::debug!(source, destination, "GraphCopyImageDescriptor");
            }
            (0x92, 0x14) => {
                let args = pop_args(stack, 2);
                let file = args.first().and_then(value_to_string).unwrap_or_default();
                let archive = args.get(1).and_then(value_to_string).unwrap_or_default();
                let loaded = !archive.is_empty()
                    && !file.is_empty()
                    && self.load_graph_image_resource(0, &archive, &file);
                trace_graph!(self, "preload resource {archive}:{file} loaded={loaded}");
                tracing::debug!(
                    args = ?summarize_values(&args),
                    archive,
                    file,
                    loaded,
                    "GraphPreloadResource"
                );
            }
            (0x92, 0x15) => {
                tracing::debug!("GraphResourceFlushQueue");
                self.trace_graph("resource flush queue");
            }
            (0x92, 0x16) => {
                let resource = stack.pop();
                let target = stack.pop();
                let resource_id = resource.as_ref().map(value_to_i32).unwrap_or_default();
                let found = self.resolve_resource_key(resource_id).is_some();
                if let Some(key) = self.resolve_resource_key(resource_id) {
                    if let Some(image) = self.graph_images.get(key) {
                        trace_graph!(
                            self,
                            "resource info target={:?} resource=#{resource_id} {key} {}x{}",
                            target,
                            image.width,
                            image.height
                        );
                    } else {
                        trace_graph!(
                            self,
                            "resource info target={:?} resource=#{resource_id} {key}",
                            target
                        );
                    }
                } else {
                    trace_graph!(
                        self,
                        "resource info target={:?} missing resource=#{resource_id}",
                        target
                    );
                }
                tracing::debug!(?target, ?resource, found, "GraphResourceQueryInfo");
                return Ok(ethornell_vm::Value::Int(i32::from(
                    found || resource_id != 0,
                )));
            }
            (0x92, 0x19) => {
                let target = stack.pop();
                let target_id = target.as_ref().map(value_to_i32).unwrap_or_default();
                if self.graph_handle_exists(target_id) {
                    trace_graph!(self, "resource commit #{target_id}");
                }
                tracing::debug!(?target, target_id, "GraphResourceCommit");
            }
            (0x92, 0x1e) => {
                let args = pop_args(stack, 9);
                let ints = args.iter().map(value_to_i32).collect::<Vec<_>>();
                tracing::debug!(?ints, "GraphTextFillRect");
            }
            (0x92, 0x9c) => {
                // funcs_486FEE[0x9c] -> sub_4867D0 pops 21 values. pop_args
                // keeps that native top-to-bottom order, so the text pointer
                // converted by sub_48DF50 is index 17.
                let args = pop_args(stack, 21);
                let text = args.get(17).and_then(|value| match value {
                    ethornell_vm::Value::Str(text) => Some(text.as_str()),
                    _ => None,
                });
                if let Some(size) = args
                    .get(11)
                    .map(value_to_i32)
                    .filter(|size| (8..=96).contains(size))
                {
                    self.text_state.font_size = size as f32;
                    self.text_state.line_height = size as f32 + 6.0;
                }
                if let Some(color) = args
                    .get(16)
                    .and_then(|value| infer_text_color(std::slice::from_ref(value)))
                {
                    self.text_state.color = color;
                }
                let color = self.text_state.color;
                self.render_graph_text(&args);
                if let Some(text) = text {
                    tracing::info!(%text, state = ?self.text_state, ?color, ?args, "RenderText");
                } else {
                    tracing::debug!(state = ?self.text_state, ?color, ?args, "RenderText without inline string");
                }
                let x = args.get(19).map(value_to_i32).unwrap_or_default();
                let advance = text
                    .map(|text| text.chars().count() as i32)
                    .unwrap_or_default()
                    .saturating_mul(self.text_state.font_size as i32);
                return Ok(ethornell_vm::Value::Int(x.saturating_add(advance)));
            }
            (0x91, 0x9c) => {
                let args = pop_args(stack, 14);
                let color = self.text_state.color;
                let text = args.iter().rev().find_map(|value| match value {
                    ethornell_vm::Value::Str(text) => Some(text.as_str()),
                    _ => None,
                });
                self.render_graph_text(&args);
                if let Some(text) = text {
                    tracing::info!(%text, state = ?self.text_state, ?color, ?args, "GraphDrawTextEx");
                } else {
                    tracing::debug!(state = ?self.text_state, ?color, ?args, "GraphDrawTextEx without inline string");
                }
            }
            (0x90, 0x31) => {
                let args = pop_args(stack, 2);
                let enabled = args.first().map(value_to_i32).unwrap_or_default() != 0;
                let target = args.get(1).map(value_to_i32).unwrap_or_default();
                self.graph_object_enabled.insert(target, enabled);
                for layer_id in self.graph_target_layers(target) {
                    if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                        layer.enabled = enabled;
                    }
                }
                tracing::debug!(target, enabled, "GraphSetObjectEnabled");
            }
            (0x90, 0x33) => {
                let args = pop_args(stack, 3);
                if args.len() >= 3 {
                    let target = value_to_i32(&args[2]);
                    let y = value_to_i32(&args[0]) as f32;
                    let x = value_to_i32(&args[1]) as f32;
                    let mut changed = 0usize;
                    let scroll_changed =
                        if let Some(scroll) = self.graph_scroll_states.get_mut(&target) {
                            scroll.base_x = x;
                            scroll.base_y = y;
                            true
                        } else {
                            false
                        };
                    if scroll_changed {
                        self.apply_graph_scroll_position(target);
                        changed += 1;
                    } else if self.set_graph_surface_position(target, x, y) {
                        changed += 1;
                    } else {
                        if let Some(layer) = self.graph_layers.get_mut(&target) {
                            layer.x = x;
                            layer.y = y;
                            changed += 1;
                        }
                        if let Some(layers) = self.graph_object_layers.get(&target).cloned() {
                            for layer_id in layers {
                                if layer_id == target {
                                    continue;
                                }
                                if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                                    layer.x = x;
                                    layer.y = y;
                                    changed += 1;
                                }
                            }
                        }
                    }
                    trace_graph!(
                        self,
                        "display object #{target} position x={x} y={y} scroll={scroll_changed} changed={changed}"
                    );
                }
                tracing::debug!(?args, "GraphSetObjectPosition");
            }
            (0x90, 0x34) => {
                let args = pop_args(stack, 2);
                let mask_alpha = args.first().map(value_to_i32).unwrap_or_default();
                let target = args.get(1).map(value_to_i32).unwrap_or_default();
                self.graph_object_properties
                    .entry(target)
                    .or_default()
                    .mask_alpha = mask_alpha;
                let opacity = self
                    .graph_object_properties
                    .get(&target)
                    .map(RuntimeGraphObjectProperties::opacity)
                    .unwrap_or(1.0);
                tracing::debug!(target, mask_alpha, opacity, "GraphSetObjectMaskAlpha");
            }
            (0x90, 0x35) => {
                let args = pop_args(stack, 2);
                let raw_scale = args.first().map(value_to_i32).unwrap_or(65_536);
                let target = args.get(1).map(value_to_i32).unwrap_or_default();
                let scale = normalize_transform_scale(fixed_16_to_f32(raw_scale));
                for layer_id in self.graph_target_layers(target) {
                    if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                        layer.scale_x = scale;
                    }
                }
                tracing::debug!(target, raw_scale, scale, "GraphSetObjectScaleX");
            }
            (0x90, 0x36) => {
                let args = pop_args(stack, 3);
                self.apply_graph_object_position(&args);
                tracing::debug!(args = ?summarize_values(&args), "GraphObjectSetPosition");
            }
            (0x90, 0x37) => {
                let args = pop_args(stack, 3);
                let y = args.first().map(value_to_i32).unwrap_or_default() as f32;
                let x = args.get(1).map(value_to_i32).unwrap_or_default() as f32;
                let target = args.get(2).map(value_to_i32).unwrap_or_default();
                for layer_id in self.graph_target_layers(target) {
                    if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                        layer.x = x;
                        layer.y = y;
                    }
                }
                tracing::debug!(target, x, y, "GraphSetObjectOrigin");
            }
            (0x90, 0x38) => {
                let args = pop_args(stack, 4);
                self.apply_graph_object_property(&args);
                tracing::debug!(?args, "GraphSetObjectProperty");
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
                let format_resource = args.first().map(value_to_i32).unwrap_or(-1);
                let target = args.get(1).map(value_to_i32).unwrap_or_default();
                let properties = self.graph_object_properties.entry(target).or_default();
                properties.format_resource = match format_resource {
                    -1 => None,
                    value => Some(value),
                };
                tracing::debug!(target, format_resource, "GraphSetObjectFormat");
                trace_graph!(self, "object #{target} format resource={format_resource}");
            }
            (0x90, 0x3d) => {
                let target = stack.pop();
                let target_id = target.as_ref().map(value_to_i32).unwrap_or_default();
                let exists = self.graph_handle_exists(target_id);
                tracing::debug!(?target, target_id, exists, "GraphHandleExists");
                if exists || self.debug_graph {
                    trace_graph!(self, "handle exists #{target_id} -> {exists}");
                }
                return Ok(ethornell_vm::Value::Int(i32::from(exists)));
            }
            (0x90, 0x43) => {
                let args = pop_args(stack, 9);
                self.apply_graph_rect_transition(&args);
                tracing::info!(
                    args = ?summarize_values(&args),
                    "GraphRectTransition"
                );
            }
            (0x90, 0x57) => {
                let args = pop_args(stack, 2);
                let destination = args.first().map(value_to_i32).unwrap_or_default();
                let object = args.get(1).map(value_to_i32).unwrap_or_default();
                let rendered = self.render_graph_object_to_bitmap(object, destination);
                tracing::debug!(object, destination, rendered, "GraphRenderObjectToTarget");
            }
            (0x90, 0x58) => {
                let args = pop_args(stack, 9);
                let values = args.iter().map(value_to_i32).collect::<Vec<_>>();
                if values.len() == 9 {
                    // funcs_48065E[0x58] -> sub_47C470 -> sub_462390.
                    // In reverse-pop order this is
                    // (blend, flags, alpha, multiplier, secondary, primary,
                    //  y, x, node).
                    let node = values[8];
                    let primary_resource = values[5];
                    let secondary_resource = values[4];
                    if node > 0
                        && self.resolve_graph_resource(primary_resource).is_some()
                        && self.resolve_graph_resource(secondary_resource).is_some()
                    {
                        let x = values[7] as f32;
                        let y = values[6] as f32;
                        self.graph_transition_nodes.insert(
                            node,
                            RuntimeGraphTransitionNode {
                                primary_resource,
                                secondary_resource,
                                alpha_parameter: values[2].clamp(0, 256),
                            },
                        );
                        let key = self.update_transition_node_image(node);
                        if let Some(key) = key {
                            let (width, height) = self
                                .graph_images
                                .get(&key)
                                .map(|image| (image.width as f32, image.height as f32))
                                .unwrap_or_default();
                            self.text_nodes.remove(&node);
                            self.graph_layers.insert(
                                node,
                                RuntimeGraphLayer {
                                    hit_id: node,
                                    owner_object: self.current_graph_object,
                                    key,
                                    target_surface: None,
                                    x,
                                    y,
                                    width,
                                    height,
                                    src_x: 0.0,
                                    src_y: 0.0,
                                    opacity: 1.0,
                                    // sub_47C470 forwards the second reverse-pop
                                    // value as the display object's priority.
                                    z: values.get(1).copied().unwrap_or(NATIVE_DISPLAY_Z),
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
                            if let Some(owner) = self.current_graph_object {
                                self.graph_object_layers
                                    .entry(owner)
                                    .or_default()
                                    .insert(node);
                            }
                            trace_graph!(
                                self,
                                "configure transition node #{node} primary=#{primary_resource} secondary=#{secondary_resource} alpha={} pos=({x:.0},{y:.0})",
                                values[2].clamp(0, 256)
                            );
                        }
                    }
                }
                tracing::info!(
                    args = ?summarize_values(&args),
                    "GraphNodeConfigureTransform"
                );
            }
            (0x90, 0x5a) => {
                let args = pop_args(stack, 10);
                let ints = args.iter().map(value_to_i32).collect::<Vec<_>>();
                let alpha = ints
                    .iter()
                    .copied()
                    .find(|value| (0..=256).contains(value))
                    .map(|value| (value as f32 / 256.0).clamp(0.0, 1.0));
                let targets = ints
                    .iter()
                    .copied()
                    .filter(|value| {
                        self.graph_layers.contains_key(value)
                            || self.graph_object_layers.contains_key(value)
                    })
                    .collect::<Vec<_>>();
                let mut changed = 0usize;
                for target in targets {
                    if let Some(layer_ids) = self.graph_object_layers.get(&target).cloned() {
                        for layer_id in layer_ids {
                            if let Some(layer) = self.graph_layers.get_mut(&layer_id) {
                                if let Some(alpha) = alpha {
                                    layer.opacity = alpha;
                                }
                                changed += 1;
                            }
                        }
                    }
                    if let Some(layer) = self.graph_layers.get_mut(&target) {
                        if let Some(alpha) = alpha {
                            layer.opacity = alpha;
                        }
                        changed += 1;
                    }
                }
                tracing::debug!(args = ?summarize_values(&args), alpha, changed, "GraphNodeDrawStateEx");
                trace_graph!(
                    self,
                    "node draw state ex alpha={alpha:?} changed={changed} raw={ints:?}"
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
            (0x90, 0x1e) => {
                let args = pop_args(stack, 8);
                self.apply_graph_blit_rect(&args);
                tracing::debug!(
                    args = ?summarize_values(&args),
                    "GraphBlitRectFast"
                );
            }
            (0x90, 0x22) => {
                let args = pop_args(stack, 7);
                self.apply_graph_node_transition(&args);
                tracing::info!(
                    args = ?summarize_values(&args),
                    "GraphNodeApplyTransition"
                );
            }
            (0x90, 0x23) => {
                let args = pop_args(stack, 10);
                let duration = args
                    .iter()
                    .map(value_to_i32)
                    .filter(|value| (1..=10_000).contains(value))
                    .max()
                    .unwrap_or_default();
                if duration > 0 {
                    self.animation_queue_remaining = self
                        .animation_queue_remaining
                        .max(duration.unsigned_abs().div_ceil(16).max(1));
                }
                tracing::debug!(args = ?summarize_values(&args), duration, "GraphNodeTransitionEx");
                trace_graph!(
                    self,
                    "node transition ex duration={duration} raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
            }
            (0x90, 0x28) => {
                let args = pop_args(stack, 12);
                self.apply_graph_object_effect(&args);
                self.frame_yield_requested = true;
                tracing::debug!(
                    args = ?summarize_values(&args),
                    "GraphScheduleObjectControl"
                );
            }
            (0x90, 0x98) => {
                let resource_table = stack.pop();
                let frame_count = pop_int_value(stack).unwrap_or_default();
                tracing::info!(frame_count, ?resource_table, "GraphConfigureCaretFrames");
            }
            (0x90, 0x99) => {
                let delay_ms = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults.caret_frame_delay_ms = delay_ms;
                tracing::info!(delay_ms, "GraphSetCaretFrameDelay");
            }
            (0x90, 0x9a) => {
                let y = pop_int_value(stack).unwrap_or_default();
                let x = pop_int_value(stack).unwrap_or_default();
                let mode = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults.configure_caret_position(mode, x, y);
                tracing::info!(mode, x, y, "GraphSetCaretPosition");
            }
            (0x90, 0x9d) => {
                let alpha = pop_int_value(stack).unwrap_or_default();
                let y = pop_int_value(stack).unwrap_or_default();
                let x = pop_int_value(stack).unwrap_or_default();
                self.graph_defaults.configure_shadow(x, y, alpha);
                tracing::info!(x, y, alpha, "GraphSetTextShadowParameters");
            }
            (0x90, 0xf6) => {
                let args = pop_args(stack, 5);
                let snapshot = self.graph_animations.evaluate(&args);
                if let Some(snapshot) = &snapshot {
                    self.animation_queue_remaining = self
                        .animation_queue_remaining
                        .max(snapshot.remaining_frames.max(u32::from(snapshot.active)));
                    trace_graph!(self,
                        "graph transition evaluate handle=#{} active={} remaining={} released={} raw={:?} record={:?}",
                        snapshot.handle,
                        snapshot.active,
                        snapshot.remaining_frames,
                        snapshot.released,
                        args.iter().map(value_to_i32).collect::<Vec<_>>(),
                        snapshot.args
                    );
                } else {
                    trace_graph!(
                        self,
                        "graph transition evaluate raw={:?}",
                        args.iter().map(value_to_i32).collect::<Vec<_>>()
                    );
                }
                tracing::debug!(args = ?summarize_values(&args), ?snapshot, "GraphTransitionEvaluate");
            }
            (0x91, 0xf1) => {
                let args = pop_args(stack, 2);
                let handle = args.first().map(value_to_i32).unwrap_or_default();
                let duration = args.get(1).map(value_to_i32).unwrap_or_default();
                self.graph_animations.start(handle, duration, &args);
                let frames = duration.unsigned_abs().div_ceil(16).max(1);
                self.animation_queue_remaining = self.animation_queue_remaining.max(frames);
                trace_graph!(self,
                    "graph animation start handle=#{handle} duration={duration} frames={frames} raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
                tracing::debug!(args = ?summarize_values(&args), handle, duration, "GraphAnimationStart");
            }
            (0x91, 0xf2) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                self.graph_animations.cancel(handle);
                trace_graph!(self, "graph animation cancel handle=#{handle}");
                tracing::debug!(handle, "GraphAnimationCancel");
            }
            (0x91, 0xf6) => {
                let handle = pop_int_value(stack).unwrap_or_default();
                self.graph_animations.release(handle);
                trace_graph!(self, "graph animation release handle=#{handle}");
                tracing::debug!(handle, "GraphAnimationRelease");
            }
            (0x92, 0xf1) => {
                let args = pop_args(stack, 5);
                tracing::debug!(args = ?summarize_values(&args), "GraphTransitionLoad");
                trace_graph!(
                    self,
                    "graph transition load raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
                return Ok(ethornell_vm::Value::Int(0));
            }
            (0x92, 0xf2) => {
                // sub_486D30 pops exactly five arguments. In reverse-pop order
                // these are (option2, option1, resource, archive, handle).
                let args = pop_args(stack, 5);
                let handle = args.get(4).map(value_to_i32).unwrap_or_default();
                let resource = args.get(2).and_then(value_to_string).unwrap_or_default();
                let archive = args.get(3).and_then(value_to_string).unwrap_or_default();
                let bytes = read_runtime_bytes(&self.manager, &archive, &resource);
                let status = bytes.as_deref().map_or(4, |bytes| {
                    self.graph_effects.configure_media(
                        handle,
                        archive.clone(),
                        resource.clone(),
                        bytes,
                    )
                });
                let source = format!("{archive}:{resource}");
                let duration = self.graph_effects.duration_ms(handle);
                trace_graph!(
                    self,
                    "graph effect configure handle=#{handle} status={status} duration_ms={duration} source={source} raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
                tracing::debug!(
                    args = ?summarize_values(&args),
                    handle,
                    source,
                    duration,
                    status,
                    "GraphEffectConfigureMedia"
                );
                return Ok(ethornell_vm::Value::Int(status));
            }
            (0x92, 0xf4) => {
                let args = pop_args(stack, 2);
                let position_ms = args.first().map(value_to_i32).unwrap_or_default();
                let handle = args.get(1).map(value_to_i32).unwrap_or_default();
                let status = self.graph_effects.seek(handle, position_ms);
                tracing::debug!(
                    args = ?summarize_values(&args),
                    handle,
                    position_ms,
                    status,
                    "GraphEffectSeek"
                );
                trace_graph!(
                    self,
                    "graph effect seek handle=#{handle} position_ms={position_ms} status={status} raw={:?}",
                    args.iter().map(value_to_i32).collect::<Vec<_>>()
                );
                return Ok(ethornell_vm::Value::Int(status));
            }
            _ => {
                let consumed_args = consume_fallback_args(group, id, stack);
                self.runtime_stubbed = true;
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
            trace_graph!(self,
                "poll object state #{object} point={point} pressed={} pending_hit={} hit={} payload=0x{:04X} -> 0x{state:08X}",
                self.mouse_pressed,
                pending_hit.unwrap_or_default(),
                hit.unwrap_or_default(),
                self.last_hit_payload,
            );
        }
        if state != 0 && pending_hit.is_some() {
            self.pending_object_state = None;
            self.mouse_pressed = false;
            if self.auto_title_release_after_state {
                self.pending_input_state = Some(0x1000_0006);
                self.pending_input_descriptor = Some(INPUT_DESCRIPTOR_MOUSE_LEFT);
                self.pending_input_consumed = false;
                self.trace_graph("queued object state consumed; release event remains pending");
            } else {
                self.trace_graph("queued object state consumed");
            }
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
        if hit.is_none() {
            if let Some(point) = self.pending_click {
                let input = self.graph_input_objects.get(&object);
                let layer = input.map(|input| input.layer);
                let surface_origin = layer
                    .and_then(|layer| self.graph_surfaces.get(&layer))
                    .map(|surface| (surface.x, surface.y));
                let layer_origin =
                    layer
                        .and_then(|layer| self.graph_layers.get(&layer))
                        .map(|layer| {
                            (
                                layer.screen_x(&self.graph_surfaces),
                                layer.screen_y(&self.graph_surfaces),
                            )
                        });
                tracing::debug!(
                    target: "graph_input",
                    object,
                    ?point,
                    ?layer,
                    ?surface_origin,
                    ?layer_origin,
                    regions = input.map(|input| input.descriptor.regions.len()).unwrap_or_default(),
                    "input click missed object"
                );
            }
        }
        let (event, payload) = if let Some((hit, payload, title_only)) = hit {
            let payload = if title_only {
                self.pending_title_payload_override
                    .take()
                    .unwrap_or(payload)
            } else {
                payload
            };
            // GraphPollObjectEvent is raised for the completed click. The BGI
            // title dispatcher branches on the mouse-release code, not press.
            let event_code = 0x1000_0006;
            if title_only {
                self.consume_title_control_event(hit, payload, "poll");
            } else {
                self.last_hit_control = hit;
                self.last_hit_payload = payload;
                self.pending_click = None;
                self.pending_object_state = None;
                self.pending_input_state = None;
                self.pending_input_descriptor = None;
                self.pending_input_consumed = false;
                self.mouse_pressed = false;
                self.auto_title_release_after_state = false;
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
            tracing::debug!(
                target: "graph_input",
                object,
                hit = self.last_hit_control,
                event = format_args!("0x{event:08X}"),
                payload = format_args!("0x{payload:08X}"),
                "input click hit object"
            );
            tracing::info!(
                object,
                hit = self.last_hit_control,
                event = format_args!("0x{event:08X}"),
                payload = format_args!("0x{payload:08X}"),
                "poll object event"
            );
            trace_graph!(
                self,
                "poll object event #{object} hit=#{} -> 0x{event:08X} payload=0x{payload:08X}",
                self.last_hit_control,
            );
        }
        (event, payload)
    }

    fn poll_object_state_record(&mut self, object: i32) -> [i32; 6] {
        if let Some(input) = self.graph_input_objects.get(&object) {
            let hit = self
                .mouse_pos
                .and_then(|point| self.hit_test_graph_input_object(object, point))
                .or_else(|| {
                    self.pending_object_state
                        .and_then(|point| self.hit_test_graph_input_object(object, point))
                });
            return input.state_record(hit);
        }
        [self.poll_object_state(object), 0, 0, 0, 0, 0]
    }

    fn poll_object_event_record(&mut self, object: i32) -> [i32; 3] {
        if let Some(event) = self
            .graph_input_objects
            .get_mut(&object)
            .and_then(RuntimeGraphInputObject::pop_event)
        {
            self.observe_title_graph_event(event);
            tracing::debug!(
                target: "graph_input",
                object,
                code = format_args!("0x{:08X}", event[0]),
                payload = format_args!("0x{:08X}", event[1]),
                parameter = format_args!("0x{:08X}", event[2]),
                "return queued graph input event"
            );
            tracing::info!(
                object,
                code = format_args!("0x{:08X}", event[0]),
                payload = format_args!("0x{:08X}", event[1]),
                parameter = format_args!("0x{:08X}", event[2]),
                "poll queued graph input event"
            );
            return event;
        }
        if self
            .graph_input_objects
            .get(&object)
            .is_some_and(|input| !input.descriptor.regions.is_empty())
        {
            let state_point = self.pending_object_state.or(self.mouse_pos);
            let state_hit =
                state_point.and_then(|point| self.hit_test_graph_input_object(object, point));
            if self.pending_object_state.is_some() || self.pending_click.is_some() {
                let input_layer = self
                    .graph_input_objects
                    .get(&object)
                    .map(|input| input.layer);
                let origin = input_layer
                    .and_then(|layer| self.graph_surfaces.get(&layer))
                    .map(|surface| (surface.x, surface.y));
                tracing::debug!(
                    target: "graph_input",
                    object,
                    ?input_layer,
                    ?origin,
                    ?state_point,
                    hit = ?state_hit.as_ref().map(|(region, x, y)| (region.group, region.index, *x, *y)),
                    pressed = self.mouse_pressed,
                    pending_release = self.pending_click.is_some(),
                    "poll graph input point"
                );
            }
            let previous_hover = self
                .graph_input_objects
                .get(&object)
                .and_then(|input| input.hovered);

            if let Some((region, local_x, local_y)) = state_hit {
                let key = (region.group, region.index);
                if previous_hover != Some(key) || self.pending_object_state.is_some() {
                    if let Some(input) = self.graph_input_objects.get_mut(&object) {
                        input.hovered = Some(key);
                    }
                    self.pending_object_state = None;
                    self.last_hit_control = pack_words(region.group, region.index);
                    self.last_hit_payload = region.index;
                    let event = [
                        0x1000_0007,
                        pack_words(region.group, region.index),
                        pack_words(local_y, local_x),
                    ];
                    if self.pending_click.is_some() {
                        let release_payload = self.apply_title_graph_payload_override(event[1]);
                        let release = [0x1000_0006, release_payload, 0];
                        if let Some(input) = self.graph_input_objects.get_mut(&object) {
                            input.queue_event(release);
                            input.complete(region, local_x, local_y);
                        }
                        self.pending_click = None;
                        self.pending_click_age_frames = 0;
                        self.pending_input_state = None;
                        self.pending_input_descriptor = None;
                        self.pending_input_consumed = false;
                        self.mouse_pressed = false;
                        self.auto_title_release_after_state = false;
                    }
                    tracing::info!(
                        object,
                        code = format_args!("0x{:08X}", event[0]),
                        payload = format_args!("0x{:08X}", event[1]),
                        parameter = format_args!("0x{:08X}", event[2]),
                        "poll graph input event"
                    );
                    tracing::debug!(
                        target: "graph_input",
                        object,
                        code = format_args!("0x{:08X}", event[0]),
                        payload = format_args!("0x{:08X}", event[1]),
                        parameter = format_args!("0x{:08X}", event[2]),
                        "return graph input hover event"
                    );
                    return event;
                }
            } else if previous_hover.is_some() {
                if let Some(input) = self.graph_input_objects.get_mut(&object) {
                    input.hovered = None;
                }
                let event = [0x1000_0006, -1, 0];
                tracing::debug!(
                    target: "graph_input",
                    object,
                    code = format_args!("0x{:08X}", event[0]),
                    payload = event[1],
                    "return graph input leave event"
                );
                return event;
            }

            if let Some(point) = self.pending_click {
                let hit = self.hit_test_graph_input_object(object, point);
                if let Some((region, local_x, local_y)) = hit {
                    // Graph input objects are polled independently. A miss on
                    // one object must not consume the host click before the
                    // object under the pointer gets its turn.
                    self.pending_click = None;
                    self.pending_click_age_frames = 0;
                    self.pending_object_state = None;
                    self.pending_input_state = None;
                    self.pending_input_descriptor = None;
                    self.pending_input_consumed = false;
                    self.mouse_pressed = false;
                    self.auto_title_release_after_state = false;
                    let packed = pack_words(region.group, region.index);
                    self.last_hit_control = packed;
                    self.last_hit_payload = region.index;
                    if let Some(input) = self.graph_input_objects.get_mut(&object) {
                        input.complete(region, local_x, local_y);
                    }
                    let packed = self.apply_title_graph_payload_override(packed);
                    let event = [0x1000_0006, packed, 0];
                    self.observe_title_graph_event(event);
                    tracing::debug!(
                        target: "graph_input",
                        object,
                        code = format_args!("0x{:08X}", event[0]),
                        payload = format_args!("0x{:08X}", event[1]),
                        parameter = format_args!("0x{:08X}", event[2]),
                        "return graph input click event"
                    );
                    tracing::info!(
                        object,
                        code = format_args!("0x{:08X}", event[0]),
                        payload = format_args!("0x{:08X}", event[1]),
                        parameter = format_args!("0x{:08X}", event[2]),
                        "poll graph input event"
                    );
                    return event;
                }
            }
            return [0; 3];
        }
        let (event, payload) = self.poll_object_event_payload(object);
        [event, payload, 0]
    }
}

impl ethornell_vm::SoundApi for RuntimeTraceApi {
    fn current_user_text(&self) -> String {
        self.native_user.text.clone()
    }

    fn configure_user_polygon(&mut self, handle: i32, mode: i32, points: &[[i32; 3]]) -> i32 {
        self.native_effect.polygons.insert(
            handle,
            native_effect::NativePolygon {
                mode,
                points: points.to_vec(),
            },
        );
        0
    }

    fn query_user_polygon(&self, handle: i32, index: i32) -> Option<[i32; 3]> {
        let polygon = self.native_effect.polygons.get(&handle)?;
        let _mode = polygon.mode;
        polygon.points.get(usize::try_from(index).ok()?).copied()
    }

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
        if let Some(result) = self.dispatch_native_user(group, id, stack) {
            return result.map(Some);
        }
        self.call_user_control(group, id, stack)
    }

    fn call_sound(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> ethornell_vm::VmResult<ethornell_vm::Value> {
        if let Some(result) = self.dispatch_native_sound(group, id, stack) {
            return result;
        }
        let consumed_args = consume_fallback_args(group, id, stack);
        self.runtime_stubbed = true;
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

fn native_audio_volume(value: i32) -> f64 {
    value.clamp(0, 128) as f64 / 128.0
}

fn native_audio_duration(value: i32) -> u64 {
    value.max(0) as u64
}

fn native_audio_panning(value: i32) -> f64 {
    f64::from(value.clamp(0, 128)) / 128.0
}

fn set_sound_channel_active(active: &mut [bool; 64], channel: i32, value: bool) {
    if let Some(slot) = usize::try_from(channel)
        .ok()
        .and_then(|index| active.get_mut(index))
    {
        *slot = value;
    }
}

fn sound_channel_active(active: &[bool; 64], channel: i32) -> bool {
    usize::try_from(channel)
        .ok()
        .and_then(|index| active.get(index))
        .copied()
        .unwrap_or(false)
}

fn scenario_audio_channel(name: &str) -> i32 {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("bgm") {
        0
    } else if lower.starts_with("se_") || lower.starts_with("se-") {
        0x100 + 32
    } else {
        0x100 + 16
    }
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

fn native_surface_control_depth(region: &ethornell_vm::GraphInputRegion) -> i32 {
    if region.flags & 0x10 != 0 {
        region.enabled_depth
    } else if region.flags & 0x02 != 0 {
        region.y
    } else {
        region.ordinal
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

fn value_to_optional_string(value: &ethornell_vm::Value) -> Option<String> {
    match value {
        ethornell_vm::Value::Str(text) if !text.is_empty() => Some(text.clone()),
        ethornell_vm::Value::Int(0) | ethornell_vm::Value::Ptr(0) | ethornell_vm::Value::None => {
            None
        }
        _ => value_to_string(value),
    }
}

fn is_title_layer(key: &str) -> bool {
    key.starts_with("sysgrp.arc:SGTitle")
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

fn is_scene_transition_resource(key: &str) -> bool {
    !key.starts_with("sysgrp.arc:")
}

fn is_message_control_layer(key: &str) -> bool {
    key.starts_with("sysgrp.arc:SGMsgWnd000")
}

fn is_primary_title_control(control: &RuntimeUserControl) -> bool {
    control.title_only
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

fn enumerate_matching_files(
    root: &Path,
    directory: &Path,
    pattern: &str,
    recursive: bool,
    max_count: usize,
    output: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries.filter_map(|entry| entry.ok()).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        if max_count != 0 && output.len() >= max_count {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if recursive {
                enumerate_matching_files(root, &path, pattern, true, max_count, output);
            }
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !windows_wildcard_matches(pattern, &name) {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        output.push(
            relative
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "\\"),
        );
    }
}

fn windows_wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase().into_bytes();
    let value = value.to_ascii_lowercase().into_bytes();
    let (mut pattern_index, mut value_index) = (0usize, 0usize);
    let (mut star, mut retry_value) = (None, 0usize);
    while value_index < value.len() {
        if pattern.get(pattern_index) == Some(&b'?')
            || pattern.get(pattern_index) == value.get(value_index)
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern.get(pattern_index) == Some(&b'*') {
            star = Some(pattern_index);
            pattern_index += 1;
            retry_value = value_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            retry_value += 1;
            value_index = retry_value;
        } else {
            return false;
        }
    }
    while pattern.get(pattern_index) == Some(&b'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn runtime_file_attributes(manager: &ResourceManager, file: &str) -> i32 {
    let Some(path) = runtime_file_path(manager, file) else {
        return -1;
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return -1;
    };
    let mut attrs = 0;
    if metadata.permissions().readonly() {
        attrs |= 0x01;
    }
    if metadata.is_dir() {
        attrs |= 0x10;
    } else {
        attrs |= 0x80;
    }
    attrs
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

    let frame_pacing = RuntimeFramePacing::from_env();
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
        vm_slices_per_frame = frame_pacing.vm_slices_per_frame,
        bootstrap_vm_slices_per_frame = frame_pacing.bootstrap_vm_slices_per_frame,
        continue_after_yield = frame_pacing.continue_after_yield,
        realtime,
        max_frames,
        "headless frame pacing configured"
    );
    let mut input_script = HeadlessInputScript::from_env();
    if input_script.is_some() {
        tracing::info!("headless input script loaded");
    }

    let frame_interval = Duration::from_millis(16);
    let mut next_frame = Instant::now();
    let mut runtime_failure = None;
    for frame in 0..max_frames {
        if let Some(runtime) = runtime.as_mut() {
            if let Some(script) = input_script.as_mut() {
                if let Some(event) = script.tick() {
                    apply_headless_input_event(&mut runtime.api, event);
                }
            }
            let frame_report = runtime.run_frame(frame_pacing, 16);
            let last_report = frame_report.last_report;
            if runtime.api.debug_graph || frame % 60 == 0 {
                let report = last_report.as_ref();
                tracing::info!(
                    frame,
                    steps = frame_report.total_steps,
                    program = report.map(|report| report.program.as_str()),
                    pc = report.map(|report| report.pc),
                    offset = ?report.and_then(|report| report.offset),
                    reason = ?report.map(|report| &report.stop_reason),
                    slices_this_frame = frame_report.slices_this_frame,
                    continue_setup_yield = frame_report.continue_setup_yield,
                    continue_input_yield = frame_report.continue_input_yield,
                    yield_blockers = ?runtime.api.vm_after_yield_blockers(),
                    stack = runtime.vm.stack.len(),
                    draw_items = runtime.api.graph_draw_items().len(),
                    text_nodes = runtime.api.text_nodes.len(),
                    graph_images = runtime.api.graph_images.len(),
                    animations = runtime.api.layer_animations.active_count(),
                    animation_queue_remaining = runtime.api.animation_queue_remaining,
                    audio_pending = runtime.api.audio_requests.len(),
                    scenario_code_base = format_args!(
                        "0x{:08X}",
                        read_vm_u32(&runtime.vm.memory, 0x0004_ca78).unwrap_or_default()
                    ),
                    scenario_offset = format_args!(
                        "0x{:08X}",
                        read_vm_u32(&runtime.vm.memory, 0x0004_cc70).unwrap_or_default()
                    ),
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
            if runtime.options.fail_on_stub
                && last_report
                    .as_ref()
                    .is_some_and(|report| report.stop_reason == ethornell_vm::VmStopReason::Error)
            {
                let report = last_report.as_ref().expect("checked above");
                runtime_failure = Some(format!(
                    "VM halted under --fail-on-stub at {} pc={} offset={:?}; stubs={:?}",
                    report.program, report.pc, report.offset, report.stubs
                ));
            }
            if runtime.api.quit_requested {
                tracing::info!(frame, "headless quit event received");
                break;
            }
            while let Some(request) = runtime.api.audio_requests.pop_front() {
                execute_audio_command(audio.as_mut(), request, "headless");
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
            next_frame += frame_interval;
            let now = Instant::now();
            if next_frame > now {
                std::thread::sleep(next_frame - now);
            } else {
                next_frame = now;
            }
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
    if let Some(failure) = runtime_failure {
        return Err(EthornellError::Other(failure));
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
    let mut graph_textures: BTreeMap<String, CachedGraphTexture> = BTreeMap::new();
    let image_size = image.as_ref().map(|image| (image.width, image.height));
    let mut cursor_surface_pos: Option<(f32, f32)> = None;
    if let Some(text) = &text {
        tracing::info!(%text, "text overlay requested");
    }

    let frame_interval = Duration::from_millis(16);
    let frame_pacing = RuntimeFramePacing::from_env();
    tracing::info!(
        vm_slices_per_frame = frame_pacing.vm_slices_per_frame,
        bootstrap_vm_slices_per_frame = frame_pacing.bootstrap_vm_slices_per_frame,
        continue_after_yield = frame_pacing.continue_after_yield,
        "runtime frame pacing configured"
    );
    let mut next_frame = Instant::now();
    let mut last_frame_at = next_frame
        .checked_sub(frame_interval)
        .unwrap_or(next_frame);
    let mut frame_index = 0usize;
    let mut input_script = HeadlessInputScript::from_env();
    if input_script.is_some() {
        tracing::info!("GUI runtime input script loaded");
    }
    let runtime_failure = Arc::new(Mutex::new(None::<String>));
    let failure_sink = runtime_failure.clone();
    event_loop.set_control_flow(ControlFlow::WaitUntil(next_frame));
    let event_result = event_loop.run(move |event, target| match event {
        Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
            WindowEvent::CloseRequested => target.exit(),
            WindowEvent::Resized(size) => renderer.resize(size.width, size.height),
            WindowEvent::Focused(focused) => {
                if let Some(runtime) = runtime.as_mut() {
                    runtime.api.window_focused = focused;
                    if !focused && runtime.api.input_requires_focus {
                        runtime.api.pending_input_state = None;
                        runtime.api.pending_input_descriptor = None;
                        runtime.api.pending_click = None;
                        runtime.api.pending_object_state = None;
                    }
                }
            }
            WindowEvent::DroppedFile(path) => {
                if let Some(runtime) = runtime.as_mut() {
                    runtime
                        .api
                        .dropped_files
                        .push_back(path.to_string_lossy().into_owned());
                }
            }
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
                cursor_surface_pos = Some((position.x as f32, position.y as f32));
                let cursor_game_pos =
                    renderer.surface_to_game_point(position.x as f32, position.y as f32);
                if let Some(runtime) = runtime.as_mut() {
                    if let Some((x, y)) = cursor_game_pos {
                        apply_runtime_input_event(
                            &mut runtime.api,
                            RuntimeInputEvent::MouseMove { x, y },
                        );
                    }
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
                        apply_runtime_input_event(
                            &mut runtime.api,
                            RuntimeInputEvent::KeyPress { descriptor },
                        );
                    }
                    tracing::info!(descriptor, "keyboard advance");
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let cursor_game_pos = cursor_surface_pos
                    .and_then(|(x, y)| renderer.surface_to_game_point(x, y));
                if let Some(runtime) = runtime.as_mut() {
                    if let Some((x, y)) = cursor_game_pos.or(runtime.api.mouse_pos) {
                        apply_runtime_input_event(
                            &mut runtime.api,
                            RuntimeInputEvent::MousePress { x, y },
                        );
                    } else {
                        tracing::warn!("mouse press ignored before a cursor position was observed");
                    }
                }
                tracing::info!(?cursor_game_pos, "mouse advance");
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                let cursor_game_pos = cursor_surface_pos
                    .and_then(|(x, y)| renderer.surface_to_game_point(x, y));
                if let Some(runtime) = runtime.as_mut() {
                    if let Some((x, y)) = cursor_game_pos.or(runtime.api.mouse_pos) {
                        apply_runtime_input_event(
                            &mut runtime.api,
                            RuntimeInputEvent::MouseRelease { x, y },
                        );
                    } else {
                        tracing::warn!(
                            "mouse release ignored before a cursor position was observed"
                        );
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                frame_index = frame_index.saturating_add(1);
                let frame_now = Instant::now();
                let elapsed_ms = frame_now
                    .saturating_duration_since(last_frame_at)
                    .as_millis()
                    .clamp(1, 250) as u64;
                last_frame_at = frame_now;
                let mut frame_draw_items = Vec::new();
                if let Some(runtime) = runtime.as_mut() {
                    if let Some(script) = input_script.as_mut() {
                        if let Some(event) = script.tick() {
                            apply_headless_input_event(&mut runtime.api, event);
                        }
                    }
                    let frame_report = runtime.run_frame(frame_pacing, elapsed_ms);
                    if runtime.api.screen_width > 0 && runtime.api.screen_height > 0 {
                        renderer.set_virtual_size(
                            runtime.api.screen_width as f32,
                            runtime.api.screen_height as f32,
                        );
                    }
                    let last_report = frame_report.last_report;
                    if let Some(title) = runtime.api.pending_window_title.take() {
                        window.set_title(&title);
                    }
                    if let Some(fullscreen) = runtime.api.pending_fullscreen.take() {
                        window.set_fullscreen(
                            fullscreen.then(|| Fullscreen::Borderless(window.current_monitor())),
                        );
                    }
                    if let Some(visible) = runtime.api.pending_window_visible.take() {
                        window.set_visible(visible);
                    }
                    if std::mem::take(&mut runtime.api.pending_window_minimize) {
                        window.set_minimized(true);
                    }
                    if runtime.api.debug_graph {
                        let report = last_report.as_ref();
                        tracing::info!(
                            steps = frame_report.total_steps,
                            slices = frame_pacing.vm_slices_per_frame,
                            program = report.map(|report| report.program.as_str()),
                            pc = report.map(|report| report.pc),
                            offset = ?report.and_then(|report| report.offset),
                            reason = ?report.map(|report| &report.stop_reason),
                            slices_this_frame = frame_report.slices_this_frame,
                            continue_setup_yield = frame_report.continue_setup_yield,
                            continue_input_yield = frame_report.continue_input_yield,
                            yield_blockers = ?runtime.api.vm_after_yield_blockers(),
                            stack = runtime.vm.stack.len(),
                            "runtime frame VM tick"
                        );
                        if let Some(report) = report {
                            runtime.api.trace_render_snapshot(frame_index, Some(report));
                            if report.stop_reason == ethornell_vm::VmStopReason::Error {
                                tracing::warn!(
                                    recent_trace = ?report.recent_trace,
                                    "runtime VM halted with error"
                                );
                            }
                        }
                    }
                    if runtime.options.fail_on_stub
                        && last_report.as_ref().is_some_and(|report| {
                            report.stop_reason == ethornell_vm::VmStopReason::Error
                        })
                    {
                        let report = last_report.as_ref().expect("checked above");
                        let failure = format!(
                            "VM halted under --fail-on-stub at {} pc={} offset={:?}; stubs={:?}",
                            report.program, report.pc, report.offset, report.stubs
                        );
                        tracing::error!(%failure, "GUI runtime stopped on stub");
                        if let Ok(mut slot) = failure_sink.lock() {
                            *slot = Some(failure);
                        }
                        target.exit();
                        return;
                    }
                    if runtime.api.quit_requested {
                        tracing::info!("runtime quit event received");
                        target.exit();
                        return;
                    }
                    while let Some(request) = runtime.api.audio_requests.pop_front() {
                        execute_audio_command(Some(&mut _audio), request, "window");
                    }
                    frame_draw_items = runtime.api.graph_draw_items();
                    for key in frame_draw_items
                        .iter()
                        .map(|item| item.key.as_str())
                        .collect::<BTreeSet<_>>()
                    {
                        let Some(image) = runtime.api.graph_images.get(key) else {
                            continue;
                        };
                        let revision = runtime.api.graph_image_revision(key);
                        match graph_textures.get_mut(key) {
                            Some(cached) if cached.revision != revision => {
                                match renderer.update_rgba(&cached.handle, image) {
                                    Ok(handle) => {
                                        cached.handle = handle;
                                        cached.revision = revision;
                                        tracing::debug!(key, revision, "updated graph texture");
                                    }
                                    Err(err) => tracing::warn!(
                                        key,
                                        revision,
                                        %err,
                                        "graph texture update failed"
                                    ),
                                }
                            }
                            Some(_) => {}
                            None => match renderer.insert_rgba(image) {
                                Ok(handle) => {
                                    tracing::info!(
                                        key,
                                        revision,
                                        width = handle.width,
                                        height = handle.height,
                                        "uploaded graph texture"
                                    );
                                    graph_textures.insert(
                                        key.to_string(),
                                        CachedGraphTexture { handle, revision },
                                    );
                                }
                                Err(err) => {
                                    tracing::warn!(key, %err, "graph texture upload failed")
                                }
                            },
                        }
                    }
                }
                let mut commands = vec![RenderCommand::Clear {
                    color: [0.0, 0.0, 0.0, 1.0],
                }];
                if let (Some(texture), Some((image_width, image_height))) = (&texture, image_size) {
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
                for item in frame_draw_items {
                    let Some(cached) = graph_textures.get(&item.key) else {
                        continue;
                    };
                    let texture = &cached.handle;
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
                        rotation_degrees: item.rotation_degrees,
                        clip: item
                            .clip
                            .map(|clip| [clip.x, clip.y, clip.width, clip.height]),
                        z: 2 + item.z,
                    });
                }
                if let Some(runtime) = runtime.as_ref() {
                    for (index, node) in runtime
                        .api
                        .text_nodes
                        .values()
                        .filter(|node| runtime.api.should_draw_text_node(node))
                        .enumerate()
                    {
                        let parent_origin = node
                            .target_surface
                            .and_then(|surface| runtime.api.graph_surfaces.get(&surface))
                            .map(|surface| (surface.x, surface.y))
                            .unwrap_or_default();
                        let screen_x = parent_origin.0 + node.x;
                        let screen_y = parent_origin.1 + node.y;
                        let x = if screen_x == 0.0 { 760.0 } else { screen_x };
                        let y = if screen_y == 0.0 {
                            330.0 + index as f32 * 40.0
                        } else {
                            screen_y
                        };
                        let size = node.size.max(20.0);
                        if let Some((shadow_x, shadow_y, shadow_alpha)) =
                            runtime.api.graph_defaults.shadow()
                        {
                            commands.push(RenderCommand::DrawText {
                                text: node.text.clone(),
                                x: x + shadow_x as f32,
                                y: y + shadow_y as f32,
                                color: [0.0, 0.0, 0.0, node.color[3] * shadow_alpha],
                                size,
                                z: node.z + index as i32 - 1,
                            });
                        }
                        commands.push(RenderCommand::DrawText {
                            text: node.text.clone(),
                            x,
                            y,
                            color: node.color,
                            size,
                            z: node.z + index as i32,
                        });
                        let mut positioned_node = node.clone();
                        positioned_node.x = x;
                        positioned_node.y = y;
                        for (ruby, ruby_x, ruby_y, ruby_size) in
                            text::ruby_draw_runs(&positioned_node)
                        {
                            if let Some((shadow_x, shadow_y, shadow_alpha)) =
                                runtime.api.graph_defaults.shadow()
                            {
                                commands.push(RenderCommand::DrawText {
                                    text: ruby.clone(),
                                    x: ruby_x + shadow_x as f32,
                                    y: ruby_y + shadow_y as f32,
                                    color: [0.0, 0.0, 0.0, node.color[3] * shadow_alpha],
                                    size: ruby_size,
                                    z: node.z + index as i32,
                                });
                            }
                            commands.push(RenderCommand::DrawText {
                                text: ruby,
                                x: ruby_x,
                                y: ruby_y,
                                color: node.color,
                                size: ruby_size,
                                z: node.z + index as i32 + 1,
                            });
                        }
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
        Event::LoopExiting => {
            if let Some(runtime) = runtime.as_ref() {
                runtime.api.write_call_coverage_if_requested();
                if let Some(path) = std::env::var_os("ETHORNELL_GUI_SNAPSHOT").map(PathBuf::from) {
                    match snapshot::write_runtime_snapshot(&runtime.api, &path) {
                        Ok(()) => tracing::info!(
                            path = %path.display(),
                            "GUI runtime snapshot written"
                        ),
                        Err(err) => tracing::warn!(
                            path = %path.display(),
                            %err,
                            "GUI runtime snapshot failed"
                        ),
                    }
                }
            }
        }
        Event::AboutToWait => {
            let now = Instant::now();
            if now >= next_frame {
                window.request_redraw();
                next_frame = now + frame_interval;
            }
            target.set_control_flow(ControlFlow::WaitUntil(next_frame));
        }
        _ => {}
    });
    event_result.map_err(|err| EthornellError::Other(format!("event loop failed: {err}")))?;
    if let Some(failure) = runtime_failure
        .lock()
        .ok()
        .and_then(|mut value| value.take())
    {
        return Err(EthornellError::Other(failure));
    }
    Ok(())
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
        rotation_degrees: 0.0,
        clip: None,
        z: 0,
    });
}
