use ethornell_vm::Value;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(crate) struct TimelineSystem {
    timelines: BTreeMap<i32, RuntimeTimeline>,
}

impl TimelineSystem {
    pub(crate) fn create(&mut self, handle: i32) {
        self.timelines.insert(handle, RuntimeTimeline::new(handle));
    }

    pub(crate) fn configure(&mut self, args: &[Value]) -> Option<TimelineEvent> {
        let handle = args.last().and_then(value_to_i32)?;
        let timeline = self
            .timelines
            .entry(handle)
            .or_insert_with(|| RuntimeTimeline::new(handle));
        timeline.configure(args);
        Some(TimelineEvent::Configured {
            handle,
            duration: timeline.duration_frames,
        })
    }

    pub(crate) fn attach(&mut self, args: &[Value]) -> Option<TimelineEvent> {
        let handle = args.last().and_then(value_to_i32)?;
        let target = args.get(args.len().saturating_sub(2))?;
        let target = value_to_i32(target)?;
        let timeline = self
            .timelines
            .entry(handle)
            .or_insert_with(|| RuntimeTimeline::new(handle));
        timeline.attach(target);
        Some(TimelineEvent::Attached { handle, target })
    }

    pub(crate) fn set_enabled(&mut self, handle: i32, enabled: bool) -> TimelineEvent {
        if handle <= 0 {
            return TimelineEvent::Enabled {
                handle,
                enabled: false,
                remaining: 0,
            };
        }
        let timeline = self
            .timelines
            .entry(handle)
            .or_insert_with(|| RuntimeTimeline::new(handle));
        timeline.enabled = enabled;
        if enabled {
            timeline.finished = false;
            timeline.remaining_frames = timeline.duration_frames;
        }
        TimelineEvent::Enabled {
            handle,
            enabled,
            remaining: timeline.remaining_frames,
        }
    }

    pub(crate) fn set_duration_ms(&mut self, handle: i32, duration_ms: i32) -> TimelineEvent {
        let timeline = self
            .timelines
            .entry(handle)
            .or_insert_with(|| RuntimeTimeline::new(handle));
        timeline.duration_frames = duration_to_frames(duration_ms);
        if timeline.enabled && !timeline.finished {
            timeline.remaining_frames = timeline.duration_frames.max(1);
        } else {
            timeline.remaining_frames = timeline.duration_frames;
            timeline.finished = timeline.duration_frames == 0;
        }
        TimelineEvent::Configured {
            handle,
            duration: timeline.duration_frames,
        }
    }

    pub(crate) fn attachments(&self, handle: i32) -> Vec<i32> {
        self.timelines
            .get(&handle)
            .map(|timeline| timeline.attachments.clone())
            .unwrap_or_default()
    }

    pub(crate) fn tick(&mut self) {
        for timeline in self.timelines.values_mut() {
            if !timeline.enabled || timeline.finished {
                continue;
            }
            if timeline.remaining_frames > 0 {
                timeline.remaining_frames -= 1;
            }
            if timeline.remaining_frames == 0 {
                timeline.finished = true;
                timeline.enabled = false;
            }
        }
    }

    pub(crate) fn poll(&self, handle: i32) -> TimelinePoll {
        self.timelines
            .get(&handle)
            .map(RuntimeTimeline::poll)
            .unwrap_or(TimelinePoll {
                active: false,
                finished: true,
                remaining: 0,
            })
    }

    pub(crate) fn query(&self, handle: i32, target: i32) -> i32 {
        self.timelines.get(&handle).map_or(1, |timeline| {
            if target != 0 && !timeline.attachments.contains(&target) {
                return 0;
            }
            i32::from(timeline.finished || !timeline.enabled)
        })
    }
}

#[derive(Debug, Clone)]
struct RuntimeTimeline {
    attachments: Vec<i32>,
    duration_frames: u32,
    remaining_frames: u32,
    enabled: bool,
    finished: bool,
}

impl RuntimeTimeline {
    fn new(_handle: i32) -> Self {
        Self {
            attachments: Vec::new(),
            duration_frames: 1,
            remaining_frames: 0,
            enabled: false,
            finished: true,
        }
    }

    fn configure(&mut self, args: &[Value]) {
        let values: Vec<i32> = args.iter().filter_map(value_to_i32).collect();
        let duration = values
            .iter()
            .rev()
            .skip(1)
            .copied()
            .find(|value| *value > 0)
            .unwrap_or(1);
        self.duration_frames = duration_to_frames(duration);
        if !self.enabled {
            self.remaining_frames = self.duration_frames;
        }
        self.finished = self.duration_frames == 0;
    }

    fn attach(&mut self, target: i32) {
        if !self.attachments.contains(&target) {
            self.attachments.push(target);
        }
    }

    fn poll(&self) -> TimelinePoll {
        TimelinePoll {
            active: self.enabled && !self.finished,
            finished: self.finished || !self.enabled,
            remaining: self.remaining_frames,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TimelinePoll {
    pub(crate) active: bool,
    pub(crate) finished: bool,
    pub(crate) remaining: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TimelineEvent {
    Configured {
        handle: i32,
        duration: u32,
    },
    Attached {
        handle: i32,
        target: i32,
    },
    Enabled {
        handle: i32,
        enabled: bool,
        remaining: u32,
    },
}

fn duration_to_frames(duration: i32) -> u32 {
    if duration <= 0 {
        return 1;
    }
    if duration <= 32 {
        return duration as u32;
    }
    (duration as u32).div_ceil(16).max(1)
}

fn value_to_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Int(value) => Some(*value),
        Value::Ptr(value) => Some(*value as i32),
        Value::Func { offset, .. } => Some(*offset as i32),
        Value::Str(_) | Value::Program(_) | Value::None => None,
    }
}
