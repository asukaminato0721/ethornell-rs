#[derive(Debug, Clone)]
pub(crate) struct TextRuntime {
    pub(crate) full_text: String,
    pub(crate) visible_chars: usize,
    pub(crate) frames_until_next_char: u8,
    pub(crate) frames_per_char: u8,
    pub(crate) target_node: Option<i32>,
    pub(crate) history: Vec<String>,
}

impl Default for TextRuntime {
    fn default() -> Self {
        Self {
            full_text: String::new(),
            visible_chars: 0,
            frames_until_next_char: 0,
            frames_per_char: 2,
            target_node: None,
            history: Vec::new(),
        }
    }
}

impl TextRuntime {
    pub(crate) fn start_message(&mut self, text: String, target_node: i32) {
        self.full_text = normalize_message_text(&text);
        self.visible_chars = 0;
        self.frames_until_next_char = 0;
        self.target_node = Some(target_node);
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
        let ch = self
            .full_text
            .chars()
            .nth(self.visible_chars)
            .unwrap_or_default();
        self.visible_chars = (self.visible_chars + 1).min(total);
        self.frames_until_next_char = char_wait_frames(ch, self.frames_per_char);
        let text = self
            .full_text
            .chars()
            .take(self.visible_chars)
            .collect::<String>();
        Some((target, text))
    }
}

fn char_wait_frames(ch: char, base_frames_per_char: u8) -> u8 {
    match ch {
        '。' | '！' | '？' | '!' | '?' => 8,
        '、' | ',' => 4,
        '…' | '」' | '』' => 6,
        _ => base_frames_per_char.saturating_sub(1),
    }
}

pub(crate) fn normalize_message_text(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    strip_ruby_tags(&text)
}

fn strip_ruby_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<R") {
        out.push_str(&rest[..start]);
        let ruby_start = &rest[start + 2..];
        let Some(close) = ruby_start.find('>') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let body_start = start + 2 + close + 1;
        let body_and_tail = &rest[body_start..];
        let Some(end) = body_and_tail.find("</R>") else {
            out.push_str(&rest[start..]);
            return out;
        };
        out.push_str(&body_and_tail[..end]);
        rest = &body_and_tail[end + 4..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::normalize_message_text;

    #[test]
    fn strips_bgi_ruby_tags() {
        assert_eq!(
            normalize_message_text("<Rやこたみちょう>矢古民町</R>へ行く"),
            "矢古民町へ行く"
        );
    }

    #[test]
    fn keeps_malformed_ruby_literal() {
        assert_eq!(normalize_message_text("<Rfoo>bar"), "<Rfoo>bar");
    }
}
