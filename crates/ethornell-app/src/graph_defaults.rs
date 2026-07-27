use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SurfaceTextState {
    pub(crate) writing_mode: i32,
    pub(crate) cursor_x: i32,
    pub(crate) cursor_y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextAnimationDefaults {
    pub(crate) glyph_delay_ms: i32,
    pub(crate) reveal_steps: i32,
    pub(crate) reveal_step_delay_ms: i32,
    pub(crate) settle_steps: i32,
    pub(crate) settle_step_delay_ms: i32,
    pub(crate) auto_advance_enabled: bool,
    pub(crate) auto_advance_delay_ms: i32,
}

impl Default for TextAnimationDefaults {
    fn default() -> Self {
        Self {
            glyph_delay_ms: 16,
            reveal_steps: 8,
            reveal_step_delay_ms: 50,
            settle_steps: 1,
            settle_step_delay_ms: 0,
            auto_advance_enabled: false,
            auto_advance_delay_ms: 0,
        }
    }
}

impl TextAnimationDefaults {
    pub(crate) fn set_glyph_delay(&mut self, delay_ms: i32) {
        self.glyph_delay_ms = delay_ms.max(0);
    }

    pub(crate) fn set_reveal(&mut self, steps: i32, step_delay_ms: i32) {
        if steps > 0 {
            self.reveal_steps = steps;
            self.reveal_step_delay_ms = step_delay_ms;
        }
    }

    pub(crate) fn set_settle(&mut self, steps: i32, step_delay_ms: i32) {
        if steps > 0 {
            self.settle_steps = steps;
            self.settle_step_delay_ms = step_delay_ms;
        }
    }

    pub(crate) fn set_auto_advance(&mut self, enabled: i32, delay_ms: i32) {
        self.auto_advance_enabled = enabled != 0;
        self.auto_advance_delay_ms = delay_ms;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TextLayoutDefaults {
    pub(crate) extent_x: i32,
    pub(crate) extent_y: i32,
    pub(crate) reserved: i32,
    pub(crate) font_size: i32,
    pub(crate) line_height: i32,
    pub(crate) mode: i32,
}

impl TextLayoutDefaults {
    pub(crate) fn configure(&mut self, args: [i32; 6]) -> Result<(), TextLayoutError> {
        let [extent_x, extent_y, reserved, font_size, line_height, mode] = args;
        if !(25..=100).contains(&font_size) {
            return Err(TextLayoutError::FontSize(font_size));
        }
        if line_height < 0 {
            return Err(TextLayoutError::LineHeight(line_height));
        }
        self.extent_x = extent_x;
        self.extent_y = extent_y.max(1);
        self.reserved = reserved;
        self.font_size = font_size;
        self.line_height = line_height;
        self.mode = mode;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextLayoutError {
    FontSize(i32),
    LineHeight(i32),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TextStyleDefaults {
    pub(crate) font_name: Option<String>,
    pub(crate) values: [i32; 6],
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GraphRuntimeDefaults {
    pub(crate) text_animation: TextAnimationDefaults,
    pub(crate) text_layout: TextLayoutDefaults,
    pub(crate) text_style: TextStyleDefaults,
    pub(crate) caret_frame_delay_ms: i32,
    pub(crate) caret_mode: i32,
    pub(crate) caret_x: i32,
    pub(crate) caret_y: i32,
    pub(crate) message_delay_enabled: bool,
    pub(crate) message_delay_ms: i32,
    pub(crate) shadow_enabled: bool,
    pub(crate) shadow_x: i32,
    pub(crate) shadow_y: i32,
    pub(crate) shadow_alpha: i32,
    pub(crate) instant_reveal: bool,
    text_substitutions: BTreeMap<String, String>,
}

impl GraphRuntimeDefaults {
    pub(crate) fn configure_caret_position(&mut self, mode: i32, x: i32, y: i32) {
        self.caret_mode = mode;
        self.caret_x = x;
        self.caret_y = y;
    }

    pub(crate) fn configure_message_delay(&mut self, enabled: i32, delay_ms: i32) {
        self.message_delay_enabled = enabled != 0;
        self.message_delay_ms = delay_ms;
    }

    pub(crate) fn configure_shadow(&mut self, x: i32, y: i32, alpha: i32) {
        if (0..=100).contains(&x) && (0..=100).contains(&y) && (0..=256).contains(&alpha) {
            self.shadow_x = x;
            self.shadow_y = y;
            self.shadow_alpha = alpha;
        }
    }

    pub(crate) fn shadow(&self) -> Option<(i32, i32, f32)> {
        self.shadow_enabled.then_some((
            self.shadow_x,
            self.shadow_y,
            self.shadow_alpha.clamp(0, 256) as f32 / 256.0,
        ))
    }

    pub(crate) fn configure_text_style(&mut self, font_name: Option<String>, values: [i32; 6]) {
        self.text_style.font_name = font_name.filter(|name| !name.is_empty());
        self.text_style.values = values;
    }

    pub(crate) fn update_text_substitution(
        &mut self,
        source: Option<String>,
        replacement: Option<String>,
    ) {
        match (source.filter(|value| !value.is_empty()), replacement) {
            (None, _) => self.text_substitutions.clear(),
            (Some(source), Some(replacement)) if !replacement.is_empty() => {
                self.text_substitutions.insert(source, replacement);
            }
            (Some(source), _) => {
                self.text_substitutions.remove(&source);
            }
        }
    }

    pub(crate) fn collect_ruby_records(&self, text: &str) -> (String, i32) {
        let mut records = String::new();
        let mut matches = 0_i32;
        let mut offset = 0;
        while offset < text.len() {
            let tail = &text[offset..];
            let hit = self
                .text_substitutions
                .iter()
                .filter(|(base, _)| tail.starts_with(base.as_str()))
                .max_by_key(|(base, _)| base.len());
            if let Some((base, reading)) = hit {
                records.push_str(base);
                records.push('\\');
                records.push_str(reading);
                records.push('\n');
                matches = matches.saturating_add(1);
                offset += base.len();
            } else {
                offset += tail.chars().next().map(char::len_utf8).unwrap_or(1);
            }
        }
        (records, matches)
    }

    pub(crate) fn register_ruby_records(&mut self, records: &str) -> bool {
        if records.is_empty() {
            return true;
        }
        for line in records.lines() {
            let Some((base, reading)) = line.split_once('\\') else {
                return false;
            };
            if base.is_empty() || reading.is_empty() {
                return false;
            }
            self.text_substitutions
                .insert(base.to_string(), reading.to_string());
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{GraphRuntimeDefaults, TextAnimationDefaults, TextLayoutDefaults};

    #[test]
    fn invalid_layout_does_not_replace_native_defaults() {
        let mut defaults = TextLayoutDefaults::default();
        defaults.configure([200, 100, 0, 40, 16, 1]).unwrap();
        assert!(defaults.configure([1, 2, 3, 24, 4, 5]).is_err());
        assert_eq!(defaults.font_size, 40);
    }

    #[test]
    fn non_positive_animation_steps_leave_previous_values() {
        let mut defaults = TextAnimationDefaults::default();
        defaults.set_reveal(6, 25);
        defaults.set_reveal(0, 99);
        assert_eq!(
            (defaults.reveal_steps, defaults.reveal_step_delay_ms),
            (6, 25)
        );
    }

    #[test]
    fn substitutions_support_add_remove_and_clear() {
        let mut defaults = GraphRuntimeDefaults::default();
        defaults.update_text_substitution(Some("A".into()), Some("B".into()));
        assert_eq!(
            defaults.collect_ruby_records("A+A"),
            ("A\\B\nA\\B\n".into(), 2)
        );
        defaults.update_text_substitution(Some("A".into()), None);
        assert_eq!(defaults.collect_ruby_records("A"), (String::new(), 0));
        defaults.update_text_substitution(Some("A".into()), Some("B".into()));
        defaults.update_text_substitution(None, None);
        assert_eq!(defaults.collect_ruby_records("A"), (String::new(), 0));
    }

    #[test]
    fn ruby_record_parser_matches_native_line_format() {
        let mut defaults = GraphRuntimeDefaults::default();
        assert!(defaults.register_ruby_records("矢古民\\やこたみ\n町\\ちょう\n"));
        assert_eq!(
            defaults.collect_ruby_records("矢古民町"),
            ("矢古民\\やこたみ\n町\\ちょう\n".into(), 2)
        );
        assert!(!defaults.register_ruby_records("missing separator"));
    }
}
