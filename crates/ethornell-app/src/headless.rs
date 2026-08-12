use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub enum HeadlessInputEvent {
    MouseMove { x: f32, y: f32 },
    MousePress { x: f32, y: f32 },
    MouseRelease { x: f32, y: f32 },
    KeyPress { key: String },
    KeyRelease { key: String },
}

#[derive(Clone, Debug)]
enum HeadlessInputAction {
    Wait(u32),
    MouseMove { x: f32, y: f32 },
    MouseDown { x: f32, y: f32 },
    MouseUp { x: f32, y: f32 },
    Click { x: f32, y: f32 },
    KeyPress { key: String },
}

#[derive(Debug, Default)]
pub struct HeadlessInputScript {
    actions: VecDeque<HeadlessInputAction>,
    wait_remaining: u32,
    queued_release: Option<HeadlessInputEvent>,
}

impl HeadlessInputScript {
    pub fn from_env() -> Option<Self> {
        let source = std::env::var("ETHORNELL_INPUT_SCRIPT")
            .or_else(|_| std::env::var("ETHORNELL_HEADLESS_SCRIPT"))
            .ok()?;
        match Self::parse(&source) {
            Ok(script) => Some(script),
            Err(err) => {
                tracing::warn!(script = source, %err, "invalid headless input script ignored");
                None
            }
        }
    }

    fn parse(source: &str) -> Result<Self, String> {
        let mut actions = VecDeque::new();
        for raw_token in source.split(',') {
            let token = raw_token.trim();
            if token.is_empty() {
                continue;
            }
            let parts = token.split(':').collect::<Vec<_>>();
            let action = match parts.as_slice() {
                ["wait", frames] => HeadlessInputAction::Wait(parse_u32(frames, token)?),
                ["move", x, y] => HeadlessInputAction::MouseMove {
                    x: parse_f32(x, token)?,
                    y: parse_f32(y, token)?,
                },
                ["down", x, y] => HeadlessInputAction::MouseDown {
                    x: parse_f32(x, token)?,
                    y: parse_f32(y, token)?,
                },
                ["up", x, y] => HeadlessInputAction::MouseUp {
                    x: parse_f32(x, token)?,
                    y: parse_f32(y, token)?,
                },
                ["click", x, y] => HeadlessInputAction::Click {
                    x: parse_f32(x, token)?,
                    y: parse_f32(y, token)?,
                },
                ["key", key] if !key.trim().is_empty() => HeadlessInputAction::KeyPress {
                    key: key.trim().to_ascii_lowercase(),
                },
                _ => return Err(format!("unsupported token `{token}`")),
            };
            actions.push_back(action);
        }
        Ok(Self {
            actions,
            wait_remaining: 0,
            queued_release: None,
        })
    }

    pub fn tick(&mut self) -> Option<HeadlessInputEvent> {
        if let Some(event) = self.queued_release.take() {
            return Some(event);
        }
        loop {
            if self.wait_remaining > 0 {
                self.wait_remaining -= 1;
                return None;
            }
            let action = self.actions.pop_front()?;
            match action {
                HeadlessInputAction::Wait(frames) => {
                    self.wait_remaining = frames;
                }
                HeadlessInputAction::MouseMove { x, y } => {
                    return Some(HeadlessInputEvent::MouseMove { x, y });
                }
                HeadlessInputAction::MouseDown { x, y } => {
                    return Some(HeadlessInputEvent::MousePress { x, y });
                }
                HeadlessInputAction::MouseUp { x, y } => {
                    return Some(HeadlessInputEvent::MouseRelease { x, y });
                }
                HeadlessInputAction::Click { x, y } => {
                    self.queued_release = Some(HeadlessInputEvent::MouseRelease { x, y });
                    return Some(HeadlessInputEvent::MousePress { x, y });
                }
                HeadlessInputAction::KeyPress { key } => {
                    self.queued_release = Some(HeadlessInputEvent::KeyRelease { key: key.clone() });
                    return Some(HeadlessInputEvent::KeyPress { key });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HeadlessInputEvent, HeadlessInputScript};

    #[test]
    fn key_action_replays_the_platform_press_and_release_pair() {
        let mut script = HeadlessInputScript::parse("key:enter").expect("parse input script");

        assert!(matches!(
            script.tick(),
            Some(HeadlessInputEvent::KeyPress { key }) if key == "enter"
        ));
        assert!(matches!(
            script.tick(),
            Some(HeadlessInputEvent::KeyRelease { key }) if key == "enter"
        ));
        assert!(script.tick().is_none());
    }
}

fn parse_u32(value: &str, token: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("invalid integer in `{token}`"))
}

fn parse_f32(value: &str, token: &str) -> Result<f32, String> {
    value
        .parse()
        .map_err(|_| format!("invalid coordinate in `{token}`"))
}
