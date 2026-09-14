//! Bounded application-payload rates. Receipt counters are not wire measurements.
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(3);
// Fixed 50 ms receipt buckets bound both memory and the maximum window-edge error.
const BUCKET_MS: u128 = 50;
const BUCKETS: usize = 61;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TransferRate {
    pub bytes_per_second: Option<u64>,
    pub logical_bytes_per_second: Option<u64>,
    pub eta_millis: Option<u64>,
    pub last_acknowledgement: Option<Instant>,
    pub awaiting_acknowledgement: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RateInput {
    pub account: i64,
    pub upload: bool,
    pub active: bool,
    pub bytes: u64,
    pub logical_bytes: u64,
    pub total: u64,
    pub in_flight: bool,
}

#[derive(Clone, Copy, Default)]
struct Bucket {
    epoch: u128,
    bytes: u64,
    logical: u64,
}

pub(crate) struct RateWindow {
    input: RateInput,
    started: Instant,
    buckets: [Bucket; BUCKETS],
    last_ack: Option<Instant>,
    pub published: TransferRate,
}

impl RateWindow {
    pub fn new(input: RateInput, now: Instant) -> Self {
        Self {
            input,
            started: now,
            buckets: [Bucket::default(); BUCKETS],
            last_ack: None,
            published: TransferRate::default(),
        }
    }

    /// O(1), with no timeline formatting or UI work. Restored counters are a baseline.
    pub fn observe(&mut self, input: RateInput, now: Instant) -> bool {
        if !input.active
            || !self.input.active
            || input.bytes < self.input.bytes
            || input.account != self.input.account
            || input.upload != self.input.upload
        {
            *self = Self::new(input, now);
            return false;
        }
        if !self.input.in_flight && input.in_flight && self.last_ack.is_none() {
            self.started = now;
        }
        let delta = input.bytes.saturating_sub(self.input.bytes);
        let first = delta > 0 && self.last_ack.is_none();
        if delta > 0 {
            let epoch = now.saturating_duration_since(self.started).as_millis() / BUCKET_MS;
            let bucket = &mut self.buckets[(epoch % BUCKETS as u128) as usize];
            if bucket.epoch != epoch {
                *bucket = Bucket {
                    epoch,
                    ..Default::default()
                };
            }
            bucket.bytes = bucket.bytes.saturating_add(delta);
            bucket.logical = bucket
                .logical
                .saturating_add(input.logical_bytes.saturating_sub(self.input.logical_bytes));
            self.last_ack = Some(now);
        }
        self.input = input;
        first
    }

    pub fn input(&self) -> RateInput {
        self.input
    }

    /// Called once per second by the presentation owner, also without incoming ACKs.
    pub fn publish(&mut self, now: Instant) -> bool {
        let previous = self.published;
        if !self.input.active {
            self.published = TransferRate::default();
            return self.published != previous;
        }
        let age = now.saturating_duration_since(self.started);
        let epoch = age.as_millis() / BUCKET_MS;
        let mut bytes = 0_u64;
        let mut logical_bytes = 0_u64;
        for bucket in &self.buckets {
            if epoch.saturating_sub(bucket.epoch) * BUCKET_MS < WINDOW.as_millis() {
                bytes = bytes.saturating_add(bucket.bytes);
                logical_bytes = logical_bytes.saturating_add(bucket.logical);
            }
        }
        let elapsed = age.min(WINDOW).as_millis();
        let expired = self
            .last_ack
            .is_some_and(|at| now.saturating_duration_since(at) >= WINDOW);
        let rate = |count: u64| {
            (elapsed > 0 && self.last_ack.is_some()).then(|| {
                if expired {
                    0
                } else {
                    (u128::from(count) * 1000 / elapsed).min(u128::from(u64::MAX)) as u64
                }
            })
        };
        let speed = rate(bytes);
        let logical = rate(logical_bytes);
        let remaining = self.input.total.saturating_sub(self.input.logical_bytes);
        self.published = TransferRate {
            bytes_per_second: speed,
            logical_bytes_per_second: logical,
            eta_millis: logical
                .filter(|&n| n > 0)
                .filter(|_| remaining > 0)
                .map(|n| {
                    (u128::from(remaining) * 1000 / u128::from(n)).min(u128::from(u64::MAX)) as u64
                }),
            last_acknowledgement: self.last_ack,
            awaiting_acknowledgement: self.input.in_flight
                && self
                    .last_ack
                    .is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(1)),
        };
        self.published != previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_jitter_and_account_changes_do_not_dilute_or_reuse_receipts() {
        let now = Instant::now();
        let mut input = RateInput {
            active: true,
            total: 100_000,
            ..Default::default()
        };
        let mut window = RateWindow::new(input, now);
        input.in_flight = true;
        window.observe(input, now + Duration::from_secs(60));
        input.bytes = 1000;
        input.logical_bytes = 900;
        assert!(window.observe(input, now + Duration::from_millis(60_500)));
        window.publish(now + Duration::from_millis(60_500));
        assert_eq!(window.published.bytes_per_second, Some(2000));
        for second in 1..=4 {
            input.bytes += 1000;
            input.logical_bytes += 900;
            window.observe(input, now + Duration::from_millis(60_500 + second * 1000));
            window.publish(now + Duration::from_millis(60_501 + second * 1000));
        }
        assert_eq!(
            window.published.bytes_per_second,
            Some(1000),
            "publication jitter does not widen the window to four seconds"
        );
        input.account = 2;
        window.observe(input, now + Duration::from_secs(65));
        window.publish(now + Duration::from_secs(66));
        assert_eq!(window.published.bytes_per_second, None);
    }
    #[test]
    fn acknowledgements_are_incremental_and_silence_expires_the_rate() {
        let start = Instant::now();
        let mut input = RateInput {
            active: true,
            total: 100_000,
            in_flight: true,
            ..Default::default()
        };
        let mut window = RateWindow::new(input, start);
        for second in 1..=3 {
            input.bytes += 1000;
            input.logical_bytes += 900;
            window.observe(input, start + Duration::from_secs(second));
            window.publish(start + Duration::from_secs(second));
            assert_eq!(window.published.bytes_per_second, Some(1000));
        }
        for second in 4..=6 {
            window.publish(start + Duration::from_secs(second));
        }
        assert_eq!(window.published.bytes_per_second, Some(0));
        assert_eq!(window.published.eta_millis, None);
        assert!(window.published.awaiting_acknowledgement);
        assert_eq!(window.buckets.len(), 61);
    }
    #[test]
    fn restored_and_duplicate_receipts_do_not_create_speed_and_pause_invalidates_it() {
        let now = Instant::now();
        let mut input = RateInput {
            active: true,
            bytes: 10_000,
            ..Default::default()
        };
        let mut window = RateWindow::new(input, now);
        window.observe(input, now + Duration::from_secs(1));
        window.publish(now + Duration::from_secs(1));
        assert_eq!(window.published.bytes_per_second, None);
        input.bytes += 1000;
        assert!(window.observe(input, now + Duration::from_secs(2)));
        window.publish(now + Duration::from_secs(2));
        assert_eq!(window.published.bytes_per_second, Some(500));
        input.active = false;
        window.observe(input, now + Duration::from_secs(3));
        assert_eq!(window.published, TransferRate::default());
        input.active = true;
        window.observe(input, now + Duration::from_secs(4));
        window.publish(now + Duration::from_secs(5));
        assert_eq!(window.published.bytes_per_second, None);
    }
}
