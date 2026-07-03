use std::time::Instant;

#[derive(Debug, Clone)]
pub(crate) struct VmTime {
    started_at: Instant,
    last_wait_ms: i32,
}

impl Default for VmTime {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            last_wait_ms: 0,
        }
    }
}

impl VmTime {
    pub(crate) fn reset(&mut self) {
        self.started_at = Instant::now();
        self.last_wait_ms = 0;
    }

    pub(crate) fn tick_count(&self) -> i32 {
        self.started_at.elapsed().as_millis().min(i32::MAX as u128) as i32
    }

    pub(crate) fn observe_wait(&mut self, milliseconds: i32) {
        self.last_wait_ms = milliseconds.max(0);
        tracing::debug!(milliseconds = self.last_wait_ms, "WaitMilliseconds");
    }
}
