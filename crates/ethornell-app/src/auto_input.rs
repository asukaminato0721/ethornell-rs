use super::{INPUT_DESCRIPTOR_MOUSE_LEFT, RuntimeTraceApi, RuntimeUserControl};

impl RuntimeTraceApi {
    pub(crate) fn maybe_auto_click_user(&mut self) {
        if !self.auto_user_click
            || (self.title_ui_active && !self.title_child_program_active)
            || self.pending_click.is_some()
        {
            return;
        }
        if self.auto_user_click_done && !self.auto_user_click_repeat {
            return;
        }
        if !self.auto_user_input_ready() {
            return;
        }
        if self.auto_user_click_elapsed_frames < self.auto_user_click_delay_frames {
            self.auto_user_click_elapsed_frames += 1;
            return;
        }

        let point_override = self.auto_user_click_point;
        let control = point_override
            .and_then(|point| self.hit_test_user_control(point).copied())
            .or_else(|| self.pick_auto_user_control().copied());
        let point = point_override
            .or_else(|| {
                control.map(|control| {
                    (
                        control.x + control.width * 0.5,
                        control.y + control.height * 0.5,
                    )
                })
            })
            .or({
                if self.scenario_bootstrapped {
                    Some((640.0, 650.0))
                } else {
                    self.mouse_pos
                }
            })
            .unwrap_or((640.0, 360.0));
        self.mouse_pos = Some(point);

        if self.auto_user_click_hovered_frames < self.auto_user_click_hover_frames {
            self.auto_user_click_hovered_frames += 1;
            if let Some(control) = control {
                self.trace_graph(format!(
                    "auto user hover frame {}/{} control=#{} at x={:.0} y={:.0}",
                    self.auto_user_click_hovered_frames,
                    self.auto_user_click_hover_frames,
                    control.id,
                    point.0,
                    point.1
                ));
            }
            return;
        }

        self.mouse_pressed = true;
        self.pending_click = Some(point);
        self.pending_click_age_frames = 0;
        self.pending_object_state = Some(point);
        self.pending_input_state = Some(0x1000_0002);
        self.pending_input_descriptor = Some(INPUT_DESCRIPTOR_MOUSE_LEFT);
        self.pending_input_consumed = false;
        self.auto_user_click_done = true;
        let stop_after_control = control
            .is_some_and(|control| self.auto_user_click_stop_payload == Some(control.payload));
        if self.auto_user_click_repeat && !stop_after_control {
            self.auto_user_click_elapsed_frames = 0;
            self.auto_user_click_hovered_frames = 0;
        } else if stop_after_control {
            self.auto_user_click_repeat = false;
        }
        self.auto_title_release_after_state = true;
        if let Some(control) = control {
            tracing::info!(
                id = control.id,
                payload = control.payload,
                x = point.0,
                y = point.1,
                "auto user click injected"
            );
            self.trace_graph(format!(
                "auto user click control=#{} payload=0x{:04X} at x={:.0} y={:.0}",
                control.id, control.payload, point.0, point.1
            ));
        } else {
            tracing::info!(x = point.0, y = point.1, "auto user input injected");
            self.trace_graph(format!(
                "auto user input at x={:.0} y={:.0}",
                point.0, point.1
            ));
        }
    }

    fn pick_auto_user_control(&self) -> Option<&RuntimeUserControl> {
        if let Some(id) = self.auto_user_click_id {
            return self
                .user_controls
                .values()
                .find(|control| !control.title_only && control.enabled && control.id == id);
        }
        None
    }

    fn auto_user_input_ready(&self) -> bool {
        if self.title_child_program_active
            && self
                .user_controls
                .values()
                .any(|control| !control.title_only && control.enabled)
        {
            return true;
        }
        if !self.scenario_bootstrapped {
            return self.scenario_overlay_active()
                || self.native_message_active
                || self.text_runtime.is_animating();
        }
        self.scenario_overlay_active()
            || self.text_runtime.is_animating()
            || self
                .user_controls
                .values()
                .any(|control| !control.title_only && control.enabled)
    }
}
