use super::{value_to_i32, RuntimeTraceApi, RuntimeUserControl, INPUT_DESCRIPTOR_ENTER};
use ethornell_vm::Value;

impl RuntimeTraceApi {
    pub(crate) fn hit_test_user_controls(&self, point: (f32, f32)) -> Option<i32> {
        self.hit_test_user_control(point).map(|control| control.id)
    }

    pub(crate) fn hit_test_user_control(&self, point: (f32, f32)) -> Option<&RuntimeUserControl> {
        if self.title_ui_active && !self.has_title_child_render_context() {
            return self
                .user_controls
                .iter()
                .rev()
                .filter_map(|(_, control)| is_primary_title_control(control).then_some(control))
                .find(|control| control.contains(point, true));
        }
        self.user_controls
            .iter()
            .rev()
            .filter_map(|(_, control)| (!control.title_only).then_some(control))
            .find(|control| control.contains(point, self.title_ui_active))
    }

    pub(crate) fn hit_test_user_control_for_object(
        &self,
        point: (f32, f32),
        object: i32,
    ) -> Option<&RuntimeUserControl> {
        if self.title_ui_active && !self.has_title_child_render_context() {
            return self
                .user_controls
                .iter()
                .rev()
                .filter_map(|(_, control)| is_primary_title_control(control).then_some(control))
                .find(|control| {
                    control.contains(point, true)
                        && (object == 0
                            || object == control.id
                            || object == control.owner_id
                            || object == control.payload
                            || control.title_only)
                });
        }
        self.user_controls
            .iter()
            .rev()
            .filter_map(|(_, control)| (!control.title_only).then_some(control))
            .find(|control| {
                control.contains(point, self.title_ui_active)
                    && (object == 0
                        || object == control.id
                        || object == control.owner_id
                        || object == control.payload)
            })
    }

    pub(crate) fn observe_loaded_program_for_title(&mut self, file: &str) {
        if !self.title_ui_active {
            return;
        }
        if file.eq_ignore_ascii_case("scrmain._bp") {
            if !self.title_child_program_active
                && self.title_scenario_requested
                && self.last_hit_control != 0
                && is_title_scenario_callback(self.last_hit_payload)
            {
                self.depart_title_ui_after_script(file);
            } else {
                self.trace_graph("scrmain preload observed while title ui remains active");
            }
        } else if file.eq_ignore_ascii_case("omake._bp")
            || file.eq_ignore_ascii_case("syswnd._bp")
            || file.eq_ignore_ascii_case("cnfgwnd._bp")
            || file.eq_ignore_ascii_case("usdtwnd._bp")
        {
            let key = file.to_ascii_lowercase();
            self.title_child_program_active = true;
            if self.title_child_program.as_deref() != Some(key.as_str()) {
                let cleared = self.user_controls.len();
                self.user_controls.clear();
                self.title_child_program = Some(key);
                self.trace_graph(format!(
                    "title child controls reset for {file} cleared={cleared}"
                ));
            }
        }
    }

    fn depart_title_ui_after_script(&mut self, file: &str) {
        self.depart_title_ui_for_scenario(file);
    }

    pub(crate) fn depart_title_ui_for_scenario(&mut self, file: &str) {
        if self.title_ui_departed {
            self.start_pending_scenario_bootstrap();
            return;
        }
        self.title_ui_active = false;
        self.title_ui_departed = true;
        self.title_child_program_active = false;
        self.title_child_program = None;
        self.title_scenario_requested = false;
        self.user_controls.retain(|_, control| !control.title_only);
        self.text_nodes.retain(|id, _| *id < 50_000);
        self.clear_title_graph_layers();
        tracing::info!(control = self.last_hit_control, file, "title ui departed");
        self.trace_graph(format!(
            "title ui departed after control #{} loaded {file}",
            self.last_hit_control
        ));
        self.start_pending_scenario_bootstrap();
    }

    pub(crate) fn maybe_consume_stale_title_click(&mut self) {
        // Title input is script-driven. Let title._bp consume clicks through
        // GraphPollObjectEvent so settings/omake follow the original callback table.
    }

    pub(crate) fn consume_title_control_event(&mut self, hit: i32, payload: i32, source: &str) {
        self.last_hit_control = hit;
        self.last_hit_payload = payload;
        self.title_scenario_requested = is_title_scenario_callback(payload);
        self.pending_click = None;
        self.pending_click_age_frames = 0;
        self.pending_object_state = None;
        self.pending_input_state = None;
        self.pending_input_descriptor = None;
        self.pending_title_payload_override = None;
        self.mouse_pressed = false;
        self.auto_title_release_after_state = false;
        self.trace_graph(format!(
            "title control event delivered to script source={source} hit=#{hit} payload={payload}"
        ));
    }

    pub(crate) fn seed_title_controls(&mut self) {
        // title._bp callback table at 0x1357. The visual order in
        // SGTitle000000 is not the numeric callback order: index 1 opens the
        // load/user-data window, while index 5 starts from the first scenario.
        const TITLE_BUTTONS: [(i32, i32, i32, f32, f32, f32, f32); 7] = [
            (0, 6164, 0, 141.0, 526.0, 224.0, 54.0),
            (1, 6154, 5, 399.0, 526.0, 224.0, 54.0),
            (2, 6159, 1, 657.0, 526.0, 224.0, 54.0),
            (3, 6184, 2, 915.0, 526.0, 224.0, 54.0),
            (4, 6169, 7, 271.0, 604.0, 224.0, 54.0),
            (5, 6174, 3, 529.0, 604.0, 224.0, 54.0),
            (6, 6179, 6, 787.0, 604.0, 224.0, 54.0),
        ];
        for (slot, object_id, logical, x, y, width, height) in TITLE_BUTTONS {
            self.user_controls.insert(
                slot,
                RuntimeUserControl {
                    id: object_id,
                    owner_id: object_id,
                    payload: logical & 0xffff,
                    x,
                    y,
                    width,
                    height,
                    enabled: true,
                    title_only: true,
                },
            );
            self.trace_graph(format!(
                "title control slot={slot} object=#{object_id} payload={logical} x={x:.0} y={y:.0} w={width:.0} h={height:.0}"
            ));
        }
    }

    pub(crate) fn observe_user_control(
        &mut self,
        group: u8,
        id: u16,
        stack: &[ethornell_vm::Value],
    ) {
        let args: Vec<i32> = stack.iter().rev().take(20).map(value_to_i32).collect();
        if self.debug_graph {
            self.trace_graph(format!("user 0x{group:02X}:0x{id:02X} args(top)={args:?}"));
        }
    }

    pub(crate) fn call_user_control(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<Value>,
    ) -> ethornell_vm::VmResult<Option<Value>> {
        if group != 0xc0 {
            return Ok(None);
        }
        let argc = match id {
            0x00 => 2,
            0x01 => 1,
            0x04 => 2,
            0x05 => 6,
            0x09 | 0x0a | 0x0c | 0x0d => 2,
            0x0b => 10,
            0x0f | 0x1f => 1,
            0x18 => 7,
            0x28 => 4,
            0x29 => 16,
            0x2d => 18,
            0x4f => 2,
            _ => return Ok(None),
        };
        let args = pop_user_args(stack, argc)?;
        if self.title_ui_active && !self.title_child_program_active {
            if self.debug_graph {
                self.trace_graph(format!("title preload ignored user2 0x{id:02X}"));
            }
            return Ok(Some(Value::None));
        }
        match id {
            0x01 => {
                if let Some(target) = args.first().map(value_to_i32) {
                    self.trace_graph(format!("user object register #{target}"));
                    let control = self.user_controls.entry(target).or_default();
                    control.id = target;
                    control.owner_id = target;
                    control.payload = target;
                    control.title_only = false;
                }
            }
            0x04 if args.len() >= 2 => {
                let enabled = value_to_i32(&args[0]) != 0;
                let target = value_to_i32(&args[1]);
                let control = self.user_controls.entry(target).or_default();
                control.id = target;
                control.owner_id = target;
                control.payload = target;
                control.enabled = enabled;
                control.title_only = false;
            }
            0x0d if args.len() >= 2 => {
                let target = value_to_i32(&args[1]);
                self.consume_user_control_event_for_object(target, "user2-duration");
            }
            0x28 if args.len() >= 4 => {
                let target = value_to_i32(&args[3]);
                let slot = value_to_i32(&args[2]);
                let (key, payload, x, y, width, height) = match avg_control_bounds(slot) {
                    Some((x, y, width, height)) => {
                        self.user_controls.retain(|_, control| {
                            control.title_only
                                || control.owner_id == target
                                || control.payload != slot
                        });
                        (
                            target.saturating_mul(100).saturating_add(slot),
                            slot,
                            x,
                            y,
                            width,
                            height,
                        )
                    }
                    None => {
                        let (x, y, width, height) = fallback_user_control_bounds(&args);
                        self.trace_graph(format!(
                            "fallback user bounds slot={slot} owner=#{target} x={x:.0} y={y:.0} w={width:.0} h={height:.0}"
                        ));
                        (
                            target.saturating_mul(100).saturating_add(slot),
                            slot,
                            x,
                            y,
                            width,
                            height,
                        )
                    }
                };
                let trace = {
                    let control = self.user_controls.entry(key).or_default();
                    control.id = key;
                    control.owner_id = target;
                    control.payload = payload;
                    control.x = x;
                    control.y = y;
                    control.width = width;
                    control.height = height;
                    control.enabled = true;
                    control.title_only = false;
                    format!(
                        "user bounds #{} owner=#{} payload={} x={} y={} w={} h={}",
                        control.id,
                        control.owner_id,
                        control.payload,
                        control.x,
                        control.y,
                        control.width,
                        control.height
                    )
                };
                self.trace_graph(trace);
            }
            0x4f if args.len() >= 2 => {
                let step = value_to_i32(&args[0]).max(0);
                let target = value_to_i32(&args[1]);
                let control = self.user_controls.entry(target).or_default();
                control.id = target;
                control.owner_id = target;
                control.payload = target;
                control.enabled = true;
                self.trace_graph(format!("user step #{target} step={step}"));
            }
            _ => {}
        }
        Ok(Some(Value::None))
    }

    fn consume_user_control_event_for_object(&mut self, object: i32, source: &str) -> bool {
        let Some(point) = self.pending_click.or(self.pending_object_state) else {
            return false;
        };
        let Some(control) = self
            .hit_test_user_control_for_object(point, object)
            .copied()
        else {
            return false;
        };
        if control.title_only {
            return false;
        }
        self.last_hit_control = control.id;
        self.last_hit_payload = control.payload;
        self.pending_click = None;
        self.pending_click_age_frames = 0;
        self.pending_object_state = None;
        self.pending_input_state = None;
        self.pending_input_descriptor = None;
        self.mouse_pressed = false;
        self.auto_title_release_after_state = false;
        self.trace_graph(format!(
            "user control event consumed source={source} object=#{object} hit=#{} payload={} at x={:.0} y={:.0}",
            control.id, control.payload, point.0, point.1
        ));
        true
    }

    pub(crate) fn maybe_auto_click_title(&mut self) {
        if !self.auto_title_click
            || self.auto_title_click_done
            || !self.title_ui_resources_loaded
            || !self.title_ui_active
        {
            return;
        }
        let Some(control) = self.user_controls.values().find(|control| {
            is_primary_title_control(control) && control.id == self.auto_title_click_id
        }) else {
            return;
        };
        let point = (
            control.x + control.width * 0.5,
            control.y + control.height * 0.5,
        );
        self.mouse_pos = Some(point);
        if self.auto_title_hovered_frames < self.auto_title_hover_frames {
            self.auto_title_hovered_frames += 1;
            self.trace_graph(format!(
                "auto title hover frame {}/{} at x={:.0} y={:.0}",
                self.auto_title_hovered_frames, self.auto_title_hover_frames, point.0, point.1
            ));
            return;
        }
        self.mouse_pressed = true;
        self.pending_click = Some(point);
        self.pending_click_age_frames = 0;
        self.pending_object_state = Some(point);
        self.pending_title_payload_override = self.auto_title_payload_override;
        self.pending_input_state = Some(0x1000_0002);
        self.pending_input_descriptor = Some(INPUT_DESCRIPTOR_ENTER);
        self.auto_title_click_done = true;
        self.auto_title_release_after_state = true;
        tracing::info!(
            id = self.auto_title_click_id,
            payload_override = ?self.auto_title_payload_override,
            x = point.0,
            y = point.1,
            "auto title click injected"
        );
        self.trace_graph(format!(
            "auto title click at x={:.0} y={:.0}",
            point.0, point.1
        ));
    }
}

pub(crate) fn title_payload_for_control_text(text: &str) -> Option<i32> {
    let compact = text.trim();
    if compact.contains("オートセーブ") {
        Some(0)
    } else if compact.contains("シナリオを序章") {
        Some(5)
    } else if compact.contains("データを選んで再開") {
        Some(1)
    } else if compact.contains("好きな章") || compact.contains("Ｈシーン") {
        Some(3)
    } else if compact.contains("各種環境設定") {
        Some(7)
    } else if compact.contains("プログラムを終了") {
        Some(6)
    } else {
        None
    }
}

pub(crate) fn title_object_for_control_text(text: &str) -> Option<i32> {
    match title_payload_for_control_text(text)? {
        0 => Some(6164),
        1 => Some(6159),
        2 => Some(6184),
        3 => Some(6184),
        5 => Some(6154),
        6 => Some(6179),
        7 => Some(6169),
        _ => None,
    }
}

fn is_title_scenario_callback(payload: i32) -> bool {
    matches!(payload & 0xffff, 0 | 5 | 8)
}

fn is_primary_title_control(control: &RuntimeUserControl) -> bool {
    control.title_only && control.owner_id == control.id
}

fn avg_control_bounds(slot: i32) -> Option<(f32, f32, f32, f32)> {
    match slot {
        8 => Some((48.0, 528.0, 104.0, 50.0)),
        9 => Some((410.0, 548.0, 70.0, 54.0)),
        10 => Some((486.0, 548.0, 68.0, 54.0)),
        11 => Some((556.0, 548.0, 78.0, 54.0)),
        12 => Some((632.0, 548.0, 98.0, 54.0)),
        13 => Some((742.0, 548.0, 76.0, 54.0)),
        14 => Some((812.0, 548.0, 92.0, 54.0)),
        15 => Some((910.0, 548.0, 82.0, 54.0)),
        16 => Some((988.0, 548.0, 96.0, 54.0)),
        17 => Some((1072.0, 548.0, 108.0, 54.0)),
        18 => Some((1178.0, 548.0, 78.0, 54.0)),
        19 => Some((48.0, 598.0, 104.0, 54.0)),
        20 => Some((1072.0, 598.0, 84.0, 54.0)),
        21 => Some((1154.0, 598.0, 72.0, 54.0)),
        22 => Some((1224.0, 598.0, 52.0, 54.0)),
        _ => None,
    }
}

fn fallback_user_control_bounds(args: &[Value]) -> (f32, f32, f32, f32) {
    let raw_width = value_to_i32(&args[0]).abs().max(1);
    let raw_y = value_to_i32(&args[1]);
    let raw_x = value_to_i32(&args[2]);

    let x = raw_x.clamp(0, 1279) as f32;
    let y = raw_y.clamp(0, 719) as f32;
    let max_width = (1280.0 - x).max(1.0);
    let width = (raw_width as f32).clamp(1.0, max_width);
    (x, y, width, 48.0)
}

fn pop_user_args(stack: &mut Vec<Value>, count: usize) -> ethornell_vm::VmResult<Vec<Value>> {
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        args.push(stack.pop().ok_or(ethornell_vm::VmError::StackUnderflow)?);
    }
    Ok(args)
}
