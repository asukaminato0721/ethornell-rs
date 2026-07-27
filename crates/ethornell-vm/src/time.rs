#[derive(Debug, Clone)]
pub(crate) struct VmTime {
    advanced_ms: u64,
    wait_deadline_ms: Option<i64>,
}

impl Default for VmTime {
    fn default() -> Self {
        Self {
            advanced_ms: 0,
            wait_deadline_ms: None,
        }
    }
}

impl VmTime {
    pub(crate) fn reset(&mut self) {
        self.advanced_ms = 0;
        self.wait_deadline_ms = None;
    }

    pub(crate) fn tick_count(&self) -> i32 {
        self.advanced_ms.min(i32::MAX as u64) as i32
    }

    pub(crate) fn performance_counter(&self) -> i64 {
        (self.advanced_ms as u128 * 1_000_000).min(i64::MAX as u128) as i64
    }

    pub(crate) fn advance(&mut self, milliseconds: u64) {
        self.advanced_ms = self.advanced_ms.saturating_add(milliseconds);
    }

    pub(crate) fn begin_wait(&mut self, milliseconds: i32) {
        let duration = i64::from(milliseconds.max(0));
        self.wait_deadline_ms = Some(i64::from(self.tick_count()).saturating_add(duration));
        tracing::debug!(milliseconds = duration, "WaitTimingEx deadline started");
    }

    pub(crate) fn remaining_wait_ms(&self) -> i32 {
        let Some(deadline) = self.wait_deadline_ms else {
            return 0;
        };
        deadline
            .saturating_sub(i64::from(self.tick_count()))
            .clamp(0, i64::from(i32::MAX)) as i32
    }
}
