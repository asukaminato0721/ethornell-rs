/// Ethornell/BGI advances graph procedures on a 16 millisecond engine tick.
/// Durations supplied by graph and timeline calls are milliseconds, including
/// small values such as 1, 16, and 32. They must never be reinterpreted as a
/// number of rendered frames.
pub(crate) const NATIVE_TICK_MS: u64 = 16;

/// Convert a native millisecond duration to engine ticks.
///
/// A non-positive duration is immediate. Positive durations are rounded up so
/// the target value is reached no earlier than the script requested.
pub(crate) fn duration_ms_to_ticks(duration_ms: i32) -> u32 {
    if duration_ms <= 0 {
        return 0;
    }
    (duration_ms as u32).div_ceil(NATIVE_TICK_MS as u32)
}

/// The target engine treats a wall-clock jump larger than 500 ms as a pause.
/// `sub_498720` subtracts that entire gap from its pause-adjusted engine clock
/// instead of trying to catch the interpreter up with several synthetic frames.
pub(crate) const ENGINE_PAUSE_GAP_MS: u64 = 500;

/// Convert a wall-clock delta into target-style engine elapsed time.
///
/// Normal GUI frames advance by their actual integer-millisecond delta. A gap
/// larger than 500 ms is considered a pause and contributes no engine time.
pub(crate) fn normalize_engine_elapsed_ms(elapsed_ms: u64) -> u64 {
    if elapsed_ms > ENGINE_PAUSE_GAP_MS {
        0
    } else {
        elapsed_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_native_durations_remain_milliseconds() {
        assert_eq!(duration_ms_to_ticks(0), 0);
        assert_eq!(duration_ms_to_ticks(1), 1);
        assert_eq!(duration_ms_to_ticks(15), 1);
        assert_eq!(duration_ms_to_ticks(16), 1);
        assert_eq!(duration_ms_to_ticks(17), 2);
        assert_eq!(duration_ms_to_ticks(32), 2);
        assert_eq!(duration_ms_to_ticks(250), 16);
    }

    #[test]
    fn long_wall_clock_gap_is_treated_as_pause() {
        assert_eq!(normalize_engine_elapsed_ms(0), 0);
        assert_eq!(normalize_engine_elapsed_ms(16), 16);
        assert_eq!(normalize_engine_elapsed_ms(17), 17);
        assert_eq!(normalize_engine_elapsed_ms(500), 500);
        assert_eq!(normalize_engine_elapsed_ms(501), 0);
        assert_eq!(normalize_engine_elapsed_ms(5_000), 0);
    }
}
