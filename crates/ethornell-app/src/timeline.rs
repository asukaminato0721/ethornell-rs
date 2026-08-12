use crate::timing::duration_ms_to_ticks;
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
            duration: timeline.duration_ticks,
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
        if enabled && timeline.duration_ticks == 0 {
            timeline.enabled = false;
            timeline.finished = true;
            timeline.remaining_ticks = 0;
        } else {
            timeline.enabled = enabled;
            if enabled {
                timeline.finished = false;
                timeline.remaining_ticks = timeline.duration_ticks;
            }
        }
        TimelineEvent::Enabled {
            handle,
            enabled: timeline.enabled,
            remaining: timeline.remaining_ticks,
        }
    }

    pub(crate) fn set_duration_ms(&mut self, handle: i32, duration_ms: i32) -> TimelineEvent {
        let timeline = self
            .timelines
            .entry(handle)
            .or_insert_with(|| RuntimeTimeline::new(handle));
        timeline.duration_ticks = duration_ms_to_ticks(duration_ms);
        timeline.remaining_ticks = timeline.duration_ticks;
        timeline.finished = timeline.duration_ticks == 0;
        if timeline.finished {
            timeline.enabled = false;
        }
        TimelineEvent::Configured {
            handle,
            duration: timeline.duration_ticks,
        }
    }

    pub(crate) fn attachments(&self, handle: i32) -> Vec<i32> {
        self.timelines
            .get(&handle)
            .map(|timeline| timeline.attachments.clone())
            .unwrap_or_default()
    }

    pub(crate) fn contains(&self, handle: i32) -> bool {
        self.timelines.contains_key(&handle)
    }

    pub(crate) fn tick(&mut self) -> Vec<i32> {
        let mut finished = Vec::new();
        for timeline in self.timelines.values_mut() {
            if !timeline.enabled || timeline.finished {
                continue;
            }
            if timeline.remaining_ticks > 0 {
                timeline.remaining_ticks -= 1;
            }
            if timeline.remaining_ticks == 0 {
                timeline.finished = true;
                timeline.enabled = false;
                finished.push(timeline.handle);
            }
        }
        finished
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
    handle: i32,
    attachments: Vec<i32>,
    duration_ticks: u32,
    remaining_ticks: u32,
    enabled: bool,
    finished: bool,
}

impl RuntimeTimeline {
    fn new(handle: i32) -> Self {
        Self {
            handle,
            attachments: Vec::new(),
            duration_ticks: 1,
            remaining_ticks: 0,
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
        self.duration_ticks = duration_ms_to_ticks(duration);
        if !self.enabled {
            self.remaining_ticks = self.duration_ticks;
        }
        self.finished = self.duration_ticks == 0;
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
            remaining: self.remaining_ticks,
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

fn value_to_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Int(value) => Some(*value),
        Value::Ptr(value) => Some(*value as i32),
        Value::Func { offset, .. } => Some(*offset as i32),
        Value::Str(_) | Value::Program(_) | Value::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_durations_use_milliseconds_for_small_values() {
        let mut timelines = TimelineSystem::default();
        timelines.create(1);
        timelines.set_duration_ms(1, 16);
        match timelines.set_enabled(1, true) {
            TimelineEvent::Enabled { remaining, .. } => assert_eq!(remaining, 1),
            _ => unreachable!(),
        }
        assert_eq!(timelines.tick(), vec![1]);
    }

    #[test]
    fn zero_duration_timeline_is_immediately_finished() {
        let mut timelines = TimelineSystem::default();
        timelines.create(1);
        timelines.set_duration_ms(1, 0);
        match timelines.set_enabled(1, true) {
            TimelineEvent::Enabled {
                enabled, remaining, ..
            } => {
                assert!(!enabled);
                assert_eq!(remaining, 0);
            }
            _ => unreachable!(),
        }
        let poll = timelines.poll(1);
        assert!(poll.finished);
        assert!(!poll.active);
        assert_eq!(poll.remaining, 0);
    }
}
