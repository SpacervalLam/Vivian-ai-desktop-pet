//! Scheduling policy for work-context compaction.

pub const KEEP_RECENT: usize = 24;
pub const MAX_ARCHIVE: usize = 24;
pub const MIN_ARCHIVE: usize = 8;

/// Token pressure is independent of message count.
pub fn token_pressure(used: u64, window: u64) -> bool {
    window > 0 && used > 0 && u128::from(used) * 100 >= u128::from(window) * 75
}

/// Under pressure retain the latest message group rather than a fixed 24 messages.
/// The caller adjusts this boundary to keep tool calls/results intact.
pub fn archive_target(count: usize, pressure: bool) -> usize {
    let keep = if pressure { 1 } else { KEEP_RECENT };
    count.saturating_sub(keep).min(MAX_ARCHIVE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn few_large_messages_can_be_archived() {
        assert!(token_pressure(8_000, 10_000));
        assert_eq!(archive_target(4, true), 3);
        assert_eq!(archive_target(4, false), 0);
        assert_eq!(archive_target(1, true), 0);
    }

    #[test]
    fn ordinary_history_keeps_recent_messages() {
        assert_eq!(archive_target(40, false), 16);
        assert_eq!(archive_target(100, false), MAX_ARCHIVE);
        assert!(!token_pressure(7_499, 10_000));
        assert!(token_pressure(7_500, 10_000));
        assert!(!token_pressure(1, 0));
        assert!(token_pressure(u64::MAX, u64::MAX));
    }
}
