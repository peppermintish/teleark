//! Fixed event timestamps. No presentation clock or periodic invalidation.
#[cfg(test)]
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Map monotonic event instants to a fixed wall-clock anchor once per app.
/// Subsequent renders and wall-clock adjustments cannot age historical rows.
pub(super) struct TimeAnchor {
    instant: Instant,
    unix_millis: i64,
}
impl TimeAnchor {
    pub fn new() -> Self {
        Self {
            instant: Instant::now(),
            unix_millis: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|d| i64::try_from(d.as_millis()).ok())
                .unwrap_or(0),
        }
    }
    pub fn unix_millis(&self, at: Instant) -> i64 {
        if at >= self.instant {
            self.unix_millis.saturating_add(
                i64::try_from(at.duration_since(self.instant).as_millis()).unwrap_or(i64::MAX),
            )
        } else {
            self.unix_millis.saturating_sub(
                i64::try_from(self.instant.duration_since(at).as_millis()).unwrap_or(i64::MAX),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_anchor_is_fixed_and_handles_instants_on_both_sides() {
        let now = Instant::now();
        let anchor = TimeAnchor {
            instant: now,
            unix_millis: 100_000,
        };
        assert_eq!(anchor.unix_millis(now - Duration::from_secs(3)), 97_000);
        assert_eq!(anchor.unix_millis(now + Duration::from_secs(7)), 107_000);
        assert_eq!(anchor.unix_millis(now - Duration::from_secs(3)), 97_000);
    }
}
