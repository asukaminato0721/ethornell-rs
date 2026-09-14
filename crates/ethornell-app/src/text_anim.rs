#[derive(Debug, Clone)]
pub(crate) struct TextRuntime {
    pub(crate) full_text: String,
    pub(crate) visible_chars: usize,
    /// Character-count boundary reached after each timed glyph. Newline
    /// control records are folded into the preceding boundary because target
    /// sub_433960 processes them without consuming a glyph-delay interval.
    reveal_boundaries: Vec<usize>,
    /// Number of timed glyph boundaries already revealed.
    revealed_glyphs: usize,
    /// Remaining real milliseconds until the next glyph reveal.
    pub(crate) next_glyph_remaining_ms: u32,
    /// Script-configured glyph cadence from Graph90:94.
    pub(crate) glyph_delay_ms: u32,
    pub(crate) target_node: Option<i32>,
    pub(crate) history: Vec<String>,
    pub(crate) ruby_spans: Vec<RuntimeRubySpan>,
    pub(crate) style_spans: Vec<RuntimeTextStyleSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeRubySpan {
    pub(crate) start_char: usize,
    pub(crate) end_char: usize,
    pub(crate) reading: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RuntimeTextStyle {
    pub(crate) packed_rgb: Option<u32>,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeTextStyleSpan {
    pub(crate) start_char: usize,
    pub(crate) end_char: usize,
    pub(crate) style: RuntimeTextStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedMessageMarkup {
    pub(crate) text: String,
    pub(crate) ruby_spans: Vec<RuntimeRubySpan>,
    pub(crate) style_spans: Vec<RuntimeTextStyleSpan>,
}

impl Default for TextRuntime {
    fn default() -> Self {
        Self {
            full_text: String::new(),
            visible_chars: 0,
            reveal_boundaries: Vec::new(),
            revealed_glyphs: 0,
            next_glyph_remaining_ms: 0,
            glyph_delay_ms: 16,
            target_node: None,
            history: Vec::new(),
            ruby_spans: Vec::new(),
            style_spans: Vec::new(),
        }
    }
}

impl TextRuntime {
    pub(crate) fn start_message(&mut self, text: String, target_node: i32) {
        let parsed = parse_message_markup_styled(&text);
        self.start_styled_message(
            parsed.text,
            parsed.ruby_spans,
            parsed.style_spans,
            target_node,
        );
    }

    pub(crate) fn set_glyph_delay_ms(&mut self, delay_ms: i32) {
        self.glyph_delay_ms = delay_ms.max(0) as u32;
        if self.revealed_glyphs < self.reveal_boundaries.len() {
            self.next_glyph_remaining_ms = self.glyph_delay_ms;
        }
    }

    pub(crate) fn start_styled_message(
        &mut self,
        text: String,
        ruby_spans: Vec<RuntimeRubySpan>,
        style_spans: Vec<RuntimeTextStyleSpan>,
        target_node: i32,
    ) {
        self.full_text = text;
        let (initial_visible_chars, reveal_boundaries) =
            build_message_reveal_boundaries(&self.full_text);
        self.visible_chars = initial_visible_chars;
        self.reveal_boundaries = reveal_boundaries;
        self.revealed_glyphs = 0;
        self.next_glyph_remaining_ms = if self.reveal_boundaries.is_empty() {
            0
        } else {
            self.glyph_delay_ms
        };
        self.target_node = Some(target_node);
        self.ruby_spans = ruby_spans;
        self.style_spans = style_spans;
        if !self.full_text.is_empty() {
            self.history.push(self.full_text.clone());
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
    }

    pub(crate) fn reveal_all(&mut self) -> Option<(i32, String)> {
        let target = self.target_node?;
        if self.revealed_glyphs < self.reveal_boundaries.len() {
            self.revealed_glyphs = self.reveal_boundaries.len();
            self.visible_chars = self.full_text.chars().count();
            self.next_glyph_remaining_ms = 0;
            return Some((target, self.full_text.clone()));
        }
        None
    }

    pub(crate) fn current_visible_text(&self) -> String {
        self.full_text
            .chars()
            .take(self.visible_chars)
            .collect::<String>()
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.revealed_glyphs < self.reveal_boundaries.len()
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

    pub(crate) fn visible_style_spans(&self) -> Vec<RuntimeTextStyleSpan> {
        self.style_spans
            .iter()
            .filter_map(|span| {
                let end_char = span.end_char.min(self.visible_chars);
                (span.start_char < end_char).then(|| RuntimeTextStyleSpan {
                    start_char: span.start_char,
                    end_char,
                    style: span.style,
                })
            })
            .collect()
    }

    pub(crate) fn duration_ms(&self) -> i32 {
        if self.reveal_boundaries.is_empty() {
            return 0;
        }
        let delay = self.glyph_delay_ms.max(1);
        (self.reveal_boundaries.len() as u32)
            .saturating_mul(delay)
            .min(i32::MAX as u32) as i32
    }

    /// Advance typewriter reveal using the same pause-adjusted real
    /// millisecond clock as the native message procedure. Presentation
    /// cadence must not change glyph speed.
    pub(crate) fn tick(&mut self, elapsed_ms: u64) -> Option<(i32, String)> {
        let target = self.target_node?;
        if self.revealed_glyphs >= self.reveal_boundaries.len() {
            return None;
        }

        let mut budget = elapsed_ms.min(u64::from(u32::MAX)) as u32;
        let before = self.revealed_glyphs;
        if self.glyph_delay_ms == 0 {
            self.revealed_glyphs = self.reveal_boundaries.len();
            self.visible_chars = self.full_text.chars().count();
            self.next_glyph_remaining_ms = 0;
        } else {
            if self.next_glyph_remaining_ms == 0 {
                self.next_glyph_remaining_ms = self.glyph_delay_ms;
            }
            while self.revealed_glyphs < self.reveal_boundaries.len()
                && budget >= self.next_glyph_remaining_ms
            {
                budget -= self.next_glyph_remaining_ms;
                self.visible_chars = self.reveal_boundaries[self.revealed_glyphs];
                self.revealed_glyphs += 1;
                self.next_glyph_remaining_ms = self.glyph_delay_ms;
            }
            if self.revealed_glyphs < self.reveal_boundaries.len() {
                self.next_glyph_remaining_ms = self.next_glyph_remaining_ms.saturating_sub(budget);
            } else {
                self.next_glyph_remaining_ms = 0;
            }
        }

        if self.revealed_glyphs == before {
            return None;
        }
        let text = self
            .full_text
            .chars()
            .take(self.visible_chars)
            .collect::<String>();
        Some((target, text))
    }
}

/// Build the target-shaped timed reveal plan for plain message characters.
/// `CProcDspMsg::Parse` dispatches byte 0x0A to sub_433960, which performs
/// the line-break/layout mutation and returns success immediately.  Therefore
/// a newline must never consume a separate Graph90:94 glyph interval.
fn build_message_reveal_boundaries(text: &str) -> (usize, Vec<usize>) {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = 0usize;
    while index < chars.len() && chars[index] == '\n' {
        index += 1;
    }
    let initial_visible_chars = index;
    let mut boundaries = Vec::new();
    while index < chars.len() {
        if chars[index] == '\n' {
            // Defensive fallback for a control reached without a preceding
            // timed glyph.  It is still consumed immediately.
            index += 1;
            if let Some(last) = boundaries.last_mut() {
                *last = index;
            }
            continue;
        }
        index += 1;
        while index < chars.len() && chars[index] == '\n' {
            index += 1;
        }
        boundaries.push(index);
    }
    (initial_visible_chars, boundaries)
}

pub(crate) fn normalize_message_text(text: &str) -> String {
    parse_message_markup(text).0
}

pub(crate) fn parse_message_markup(text: &str) -> (String, Vec<RuntimeRubySpan>) {
    let parsed = parse_message_markup_styled(text);
    (parsed.text, parsed.ruby_spans)
}

pub(crate) fn parse_message_markup_styled(text: &str) -> ParsedMessageMarkup {
    // Form-feed is a native message wait marker, not a printable glyph.
    let text = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\x0c', "");
    let mut out = String::with_capacity(text.len());
    let mut ruby_spans = Vec::new();
    let mut style_spans = Vec::new();
    let mut style = RuntimeTextStyle::default();
    let mut color_stack = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find('<') {
        append_styled_text(&mut out, &mut style_spans, &rest[..start], style);
        let tag_start = &rest[start + 1..];
        let Some(close) = tag_start.find('>') else {
            append_styled_text(&mut out, &mut style_spans, &rest[start..], style);
            return ParsedMessageMarkup {
                text: out,
                ruby_spans,
                style_spans,
            };
        };
        let tag = &tag_start[..close];
        let lower = tag.to_ascii_lowercase();
        let command = target_markup_command(&lower);
        let tail = &tag_start[close + 1..];

        match command {
            Some(("r", argument)) => {
                let Some(end) = find_ascii_case_insensitive(tail, "</r>") else {
                    append_styled_text(&mut out, &mut style_spans, &rest[start..], style);
                    return ParsedMessageMarkup {
                        text: out,
                        ruby_spans,
                        style_spans,
                    };
                };
                let body = &tail[..end];
                let start_char = out.chars().count();
                append_styled_text(&mut out, &mut style_spans, body, style);
                let end_char = out.chars().count();
                let reading = argument.trim_start_matches(' ');
                if !reading.is_empty() && end_char > start_char {
                    ruby_spans.push(RuntimeRubySpan {
                        start_char,
                        end_char,
                        reading: reading.to_string(),
                    });
                }
                rest = &tail[end + 4..];
            }
            Some(("cr", _)) => {
                append_styled_text(&mut out, &mut style_spans, "\n", style);
                rest = tail;
            }
            Some(("b", _)) => {
                style.bold = true;
                rest = tail;
            }
            Some(("/b", _)) => {
                style.bold = false;
                rest = tail;
            }
            Some(("i", _)) => {
                style.italic = true;
                rest = tail;
            }
            Some(("/i", _)) => {
                style.italic = false;
                rest = tail;
            }
            Some(("c", argument)) => {
                if let Some(packed_rgb) = parse_target_color(argument) {
                    color_stack.push(style.packed_rgb);
                    style.packed_rgb = Some(packed_rgb);
                }
                rest = tail;
            }
            Some(("/c", _)) => {
                if let Some(previous) = color_stack.pop() {
                    style.packed_rgb = previous;
                }
                rest = tail;
            }
            // These commands alter target renderer state or emit side-band
            // records, not visible glyphs. Their state spans are recovered
            // separately; never expose the control syntax as message text.
            Some(_) | None => {
                rest = tail;
            }
        }
    }
    append_styled_text(&mut out, &mut style_spans, rest, style);
    ParsedMessageMarkup {
        text: out,
        ruby_spans,
        style_spans,
    }
}

fn append_styled_text(
    out: &mut String,
    spans: &mut Vec<RuntimeTextStyleSpan>,
    text: &str,
    style: RuntimeTextStyle,
) {
    if text.is_empty() {
        return;
    }
    let start_char = out.chars().count();
    out.push_str(text);
    let end_char = out.chars().count();
    if style == RuntimeTextStyle::default() {
        return;
    }
    if let Some(previous) = spans
        .last_mut()
        .filter(|previous| previous.end_char == start_char && previous.style == style)
    {
        previous.end_char = end_char;
    } else {
        spans.push(RuntimeTextStyleSpan {
            start_char,
            end_char,
            style,
        });
    }
}

fn parse_target_color(argument: &str) -> Option<u32> {
    let digits = argument.trim_start_matches(' ').as_bytes().get(..6)?;
    digits.iter().try_fold(0u32, |color, digit| {
        let nibble = match digit {
            b'0'..=b'9' => u32::from(digit - b'0'),
            b'a'..=b'f' => u32::from(digit - b'a' + 10),
            _ => return None,
        };
        Some((color << 4) | nibble)
    })
}

fn target_markup_command(tag: &str) -> Option<(&'static str, &str)> {
    // sub_435290 compares the first "/" entry exactly, then accepts prefixes
    // for the remaining entries. `ruby` must precede `r`, and `cr` must
    // precede `c`.
    if tag == "/" {
        return Some(("/", ""));
    }
    const COMMANDS: [&str; 14] = [
        "b", "/b", "i", "/i", "ruby", "r", "/r", "cr", "c", "/c", "l", "/l", "t", "ev",
    ];
    COMMANDS.into_iter().find_map(|command| {
        tag.strip_prefix(command)
            .map(|argument| (command, argument))
    })
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_message_text, parse_message_markup, parse_message_markup_styled, RuntimeRubySpan,
        RuntimeTextStyle, RuntimeTextStyleSpan, TextRuntime,
    };

    #[test]
    fn strips_bgi_ruby_tags() {
        assert_eq!(
            normalize_message_text("<Rやこたみちょう>矢古民町</R>へ行く"),
            "矢古民町へ行く"
        );
    }

    #[test]
    fn target_markup_dispatch_is_case_insensitive_and_prefix_based() {
        assert_eq!(
            parse_message_markup("<r しんき>神気</R><CR>次").0,
            "神気\n次"
        );
        assert_eq!(
            parse_message_markup("<b>太字</b><i>斜体</i><c ff0000>赤</c>").0,
            "太字斜体赤"
        );
    }

    #[test]
    fn target_non_glyph_commands_are_not_exposed_as_text() {
        assert_eq!(
            parse_message_markup("<ruby base,reading><l>label</l><t 4><ev 3>").0,
            "label"
        );
        assert_eq!(
            parse_message_markup("before<unknown>after").0,
            "beforeafter"
        );
    }

    #[test]
    fn target_font_and_color_commands_produce_character_style_spans() {
        let parsed =
            parse_message_markup_styled("前<b>太<i>斜</i></b><c ff0000>赤<c 00ff00>緑</c>赤</c>後");
        assert_eq!(parsed.text, "前太斜赤緑赤後");
        assert_eq!(
            parsed.style_spans,
            vec![
                RuntimeTextStyleSpan {
                    start_char: 1,
                    end_char: 2,
                    style: RuntimeTextStyle {
                        bold: true,
                        ..RuntimeTextStyle::default()
                    },
                },
                RuntimeTextStyleSpan {
                    start_char: 2,
                    end_char: 3,
                    style: RuntimeTextStyle {
                        bold: true,
                        italic: true,
                        ..RuntimeTextStyle::default()
                    },
                },
                RuntimeTextStyleSpan {
                    start_char: 3,
                    end_char: 4,
                    style: RuntimeTextStyle {
                        packed_rgb: Some(0xff0000),
                        ..RuntimeTextStyle::default()
                    },
                },
                RuntimeTextStyleSpan {
                    start_char: 4,
                    end_char: 5,
                    style: RuntimeTextStyle {
                        packed_rgb: Some(0x00ff00),
                        ..RuntimeTextStyle::default()
                    },
                },
                RuntimeTextStyleSpan {
                    start_char: 5,
                    end_char: 6,
                    style: RuntimeTextStyle {
                        packed_rgb: Some(0xff0000),
                        ..RuntimeTextStyle::default()
                    },
                },
            ]
        );
    }

    #[test]
    fn target_color_parser_lowercases_hex_before_color_stack_updates() {
        let parsed = parse_message_markup_styled("<c FF0000>白</c><c ff0000>赤</c>");
        assert_eq!(parsed.text, "白赤");
        assert_eq!(
            parsed.style_spans,
            vec![RuntimeTextStyleSpan {
                start_char: 0,
                end_char: 2,
                style: RuntimeTextStyle {
                    packed_rgb: Some(0xff0000),
                    ..RuntimeTextStyle::default()
                },
            }]
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
    fn message_start_delay_blocks_glyph_reveal() {
        let mut runtime = TextRuntime::default();
        runtime.start_message("ab".to_string(), 1);
        assert_eq!(runtime.tick(16), None);
        assert_eq!(runtime.tick(16), None);
        assert_eq!(runtime.visible_chars, 0);
        assert_eq!(runtime.tick(16), Some((1, "a".to_string())));
    }

    #[test]
    fn message_duration_tracks_typewriter_frames() {
        let mut runtime = TextRuntime::default();
        runtime.start_message("abc".to_string(), 1);
        assert_eq!(runtime.duration_ms(), 48);
        runtime.set_glyph_delay_ms(33);
        assert_eq!(runtime.duration_ms(), 99);
    }

    #[test]
    fn glyph_delay_uses_real_milliseconds_not_render_frames() {
        let mut runtime = TextRuntime::default();
        runtime.set_glyph_delay_ms(39);
        runtime.start_message("ab".to_string(), 1);
        assert_eq!(runtime.tick(16), None);
        assert_eq!(runtime.tick(16), None);
        assert_eq!(runtime.tick(7), Some((1, "a".to_string())));
        assert_eq!(runtime.tick(38), None);
        assert_eq!(runtime.tick(1), Some((1, "ab".to_string())));
    }

    #[test]
    fn typewriter_visibility_clips_style_spans_to_revealed_characters() {
        let mut runtime = TextRuntime::default();
        runtime.start_message("<c ff0000>赤字</c>".to_string(), 7);
        assert_eq!(runtime.tick(16), Some((7, "赤".to_string())));
        assert_eq!(
            runtime.visible_style_spans(),
            vec![RuntimeTextStyleSpan {
                start_char: 0,
                end_char: 1,
                style: RuntimeTextStyle {
                    packed_rgb: Some(0xff0000),
                    ..RuntimeTextStyle::default()
                },
            }]
        );
        assert_eq!(runtime.tick(16), Some((7, "赤字".to_string())));
        assert_eq!(runtime.visible_style_spans()[0].end_char, 2);
    }
}
