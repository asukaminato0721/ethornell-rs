#[derive(Debug, Clone)]
pub(crate) struct TextRuntime {
    pub(crate) full_text: String,
    pub(crate) visible_chars: usize,
    pub(crate) frames_until_next_char: u16,
    pub(crate) frames_per_char: u16,
    pub(crate) target_node: Option<i32>,
    pub(crate) history: Vec<String>,
    pub(crate) ruby_spans: Vec<RuntimeRubySpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeRubySpan {
    pub(crate) start_char: usize,
    pub(crate) end_char: usize,
    pub(crate) reading: String,
}

impl Default for TextRuntime {
    fn default() -> Self {
        Self {
            full_text: String::new(),
            visible_chars: 0,
            frames_until_next_char: 0,
            frames_per_char: 1,
            target_node: None,
            history: Vec::new(),
            ruby_spans: Vec::new(),
        }
    }
}

impl TextRuntime {
    pub(crate) fn start_message(&mut self, text: String, target_node: i32) {
        self.start_styled_message(normalize_message_text(&text), Vec::new(), target_node);
    }

    pub(crate) fn set_glyph_delay_ms(&mut self, delay_ms: i32) {
        self.frames_per_char = delay_ms.max(0).saturating_add(15).saturating_div(16).max(1) as u16;
    }

    pub(crate) fn start_styled_message(
        &mut self,
        text: String,
        ruby_spans: Vec<RuntimeRubySpan>,
        target_node: i32,
    ) {
        self.full_text = text;
        self.visible_chars = 0;
        self.frames_until_next_char = 0;
        self.target_node = Some(target_node);
        self.ruby_spans = ruby_spans;
        if !self.full_text.is_empty() {
            self.history.push(self.full_text.clone());
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
    }

    pub(crate) fn reveal_all(&mut self) -> Option<(i32, String)> {
        let target = self.target_node?;
        let total = self.full_text.chars().count();
        if self.visible_chars < total {
            self.visible_chars = total;
            return Some((target, self.full_text.clone()));
        }
        None
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.visible_chars < self.full_text.chars().count()
    }

    pub(crate) fn has_current_message(&self) -> bool {
        self.target_node.is_some() && !self.full_text.is_empty()
    }

    pub(crate) fn visible_ruby_spans(&self) -> Vec<RuntimeRubySpan> {
        self.ruby_spans
            .iter()
            .filter(|span| span.end_char <= self.visible_chars)
            .cloned()
            .collect()
    }

    pub(crate) fn duration_ms(&self) -> i32 {
        let frames =
            (self.full_text.chars().count() as u32).saturating_mul(u32::from(self.frames_per_char));
        frames.saturating_mul(16).max(1).min(i32::MAX as u32) as i32
    }

    pub(crate) fn tick(&mut self) -> Option<(i32, String)> {
        let target = self.target_node?;
        let total = self.full_text.chars().count();
        if self.visible_chars >= total {
            return None;
        }
        if self.frames_until_next_char > 0 {
            self.frames_until_next_char -= 1;
            return None;
        }
        self.visible_chars = (self.visible_chars + 1).min(total);
        self.frames_until_next_char = self.frames_per_char.saturating_sub(1);
        let text = self
            .full_text
            .chars()
            .take(self.visible_chars)
            .collect::<String>();
        Some((target, text))
    }
}

pub(crate) fn normalize_message_text(text: &str) -> String {
    parse_message_markup(text).0
}

pub(crate) fn parse_message_markup(text: &str) -> (String, Vec<RuntimeRubySpan>) {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(text.len());
    let mut spans = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<R") {
        out.push_str(&rest[..start]);
        let ruby_start = &rest[start + 2..];
        let Some(close) = ruby_start.find('>') else {
            out.push_str(&rest[start..]);
            return (out, spans);
        };
        let reading = &ruby_start[..close];
        let body_start = start + 2 + close + 1;
        let body_and_tail = &rest[body_start..];
        let Some(end) = body_and_tail.find("</R>") else {
            out.push_str(&rest[start..]);
            return (out, spans);
        };
        let body = &body_and_tail[..end];
        let start_char = out.chars().count();
        out.push_str(body);
        let end_char = out.chars().count();
        if !reading.is_empty() && end_char > start_char {
            spans.push(RuntimeRubySpan {
                start_char,
                end_char,
                reading: reading.to_string(),
            });
        }
        rest = &body_and_tail[end + 4..];
    }
    out.push_str(rest);
    (out, spans)
}

#[cfg(test)]
mod tests {
    use super::{normalize_message_text, parse_message_markup, RuntimeRubySpan, TextRuntime};

    #[test]
    fn strips_bgi_ruby_tags() {
        assert_eq!(
            normalize_message_text("<Rやこたみちょう>矢古民町</R>へ行く"),
            "矢古民町へ行く"
        );
    }

    #[test]
    fn preserves_bgi_ruby_character_range() {
        assert_eq!(
            parse_message_markup("前<Rやこたみ>矢古民</R>後"),
            (
                "前矢古民後".to_string(),
                vec![RuntimeRubySpan {
                    start_char: 1,
                    end_char: 4,
                    reading: "やこたみ".to_string(),
                }]
            )
        );
    }

    #[test]
    fn keeps_malformed_ruby_literal() {
        assert_eq!(normalize_message_text("<Rfoo>bar"), "<Rfoo>bar");
    }

    #[test]
    fn message_duration_tracks_typewriter_frames() {
        let mut runtime = TextRuntime::default();
        runtime.start_message("abc".to_string(), 1);
        assert_eq!(runtime.duration_ms(), 48);
        runtime.set_glyph_delay_ms(33);
        assert_eq!(runtime.duration_ms(), 144);
    }
}
