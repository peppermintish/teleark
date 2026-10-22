//! Account-wide admission for remote sync pages. Local reads and contiguous
//! pushes bypass it; waiting never owns the coordinator or a network reactor.
use super::*;

pub(super) struct RequestBudget {
    interval: Duration,
    recovery_at: Option<Instant>,
    clock: SyncClock,
    last_start: Option<Instant>,
    flood: Option<Instant>,
}

impl Default for RequestBudget {
    fn default() -> Self {
        Self::new(Duration::ZERO)
    }
}

impl RequestBudget {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            recovery_at: None,
            clock: Arc::new(Instant::now),
            last_start: None,
            flood: None,
        }
    }

    pub fn with_clock(mut self, clock: SyncClock) -> Self {
        self.clock = clock;
        self
    }

    fn admit(&mut self, now: Instant) -> Result<(), Instant> {
        self.recover(now);
        let next = self.last_start.and_then(|at| at.checked_add(self.interval));
        if let Some(deadline) = next.into_iter().chain(self.flood).max()
            && deadline > now
        {
            return Err(deadline);
        }
        self.last_start = Some(now);
        Ok(())
    }

    pub fn defer(&mut self, deadline: Instant) {
        self.flood = Some(self.flood.map_or(deadline, |old| old.max(deadline)));
        self.recovery_at = Some(self.recovery_at.map_or(deadline, |old| old.max(deadline)));
    }

    pub fn flood(&self) -> Option<Instant> {
        self.flood
    }

    pub fn interval_ms(&self) -> u64 {
        u64::try_from(self.interval.as_millis())
            .unwrap_or(30_000)
            .max(1_000)
    }

    pub fn restore_interval(&mut self, milliseconds: u64, elapsed: Duration) {
        // Zero disables admission spacing for controlled fake remotes only.
        if !self.interval.is_zero() {
            self.interval = Duration::from_millis(milliseconds.clamp(1_000, 30_000));
            let now = (self.clock)();
            // Apply complete wall-time windows directly: a newly booted system's
            // monotonic clock need not support subtracting the whole offline age.
            for _ in 0..(elapsed.as_secs() / 60).min(5) {
                self.interval = (self.interval / 2).max(Duration::from_secs(1));
            }
            self.recovery_at = Some(
                now.checked_sub(Duration::from_secs(elapsed.as_secs() % 60))
                    .unwrap_or(now),
            );
        }
    }

    fn rate_limited(&mut self, now: Instant, deadline: Instant) {
        if !self.interval.is_zero() && self.flood.is_none_or(|at| at <= now) {
            self.interval = self.interval.saturating_mul(2).min(Duration::from_secs(30));
        }
        self.defer(deadline);
    }

    fn recover(&mut self, now: Instant) {
        const WINDOW: Duration = Duration::from_secs(60);
        if self.interval <= Duration::from_secs(1) || self.flood.is_some_and(|at| at > now) {
            return;
        }
        let anchor = self.recovery_at.get_or_insert(now);
        let windows = (now.saturating_duration_since(*anchor).as_secs() / WINDOW.as_secs()).min(5);
        for _ in 0..windows {
            self.interval = (self.interval / 2).max(Duration::from_secs(1));
        }
        *anchor += WINDOW * windows as u32;
    }

    fn succeeded(&mut self, now: Instant) {
        self.recover(now);
    }
}

pub(super) fn now(shared: &Shared) -> Instant {
    shared
        .pacing
        .lock()
        .map_or_else(|_| Instant::now(), |budget| (budget.clock)())
}

pub(super) fn wait(
    shared: &Shared,
    chat: Option<i64>,
    cancellation: &TelegramScanCancellation,
) -> Result<(), ChannelSyncFailure> {
    let mut announced = None;
    loop {
        if cancellation.is_cancelled() || shared.stop.load(Ordering::Acquire) {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled).into());
        }
        let (admission, flood, now) = {
            let mut budget = shared
                .pacing
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Conflict))?;
            {
                let now = (budget.clock)();
                (
                    budget.admit(now),
                    budget.flood().is_some_and(|at| at > now),
                    now,
                )
            }
        };
        match admission {
            Ok(()) => return Ok(()),
            Err(at) => {
                let phase = if flood {
                    ChannelSyncPhase::RateLimited
                } else {
                    ChannelSyncPhase::Pacing
                };
                if announced != Some((phase, at)) {
                    publish(shared, phase, chat, None, Some(at));
                    announced = Some((phase, at));
                }
                // Cancellation latency is bounded without holding any lock. The
                // worker is retained and there are at most six waiting owners.
                thread::park_timeout(
                    at.saturating_duration_since(now)
                        .min(Duration::from_millis(50)),
                );
            }
        }
    }
}

pub(super) fn observe<T>(shared: &Shared, result: &Result<T, ChannelSyncFailure>) {
    let changed = if let Ok(mut budget) = shared.pacing.lock() {
        let before = budget.interval_ms();
        let now = (budget.clock)();
        match result {
            Ok(_) => budget.succeeded(now),
            Err(error) => {
                if let Some(deadline) = error.retry_after.and_then(|delay| now.checked_add(delay)) {
                    // Fence already queued workers before the coordinator sees
                    // this completion, and adapt only once per cooldown window.
                    budget.rate_limited(now, deadline);
                }
            }
        }
        (before != budget.interval_ms()).then_some(budget.interval_ms())
    } else {
        None
    };
    if let Some(interval_ms) = changed {
        tracing::info!(
            event = "sync.admission.interval_changed",
            interval_ms,
            reason = if result.is_ok() {
                "sustained_success"
            } else {
                "server_wait"
            },
            "Automatic synchronization pacing changed"
        );
    }
}

pub(super) fn log_page<T>(
    kind: &'static str,
    started: Instant,
    result: &Result<T, ChannelSyncFailure>,
) {
    let error = result.as_ref().err();
    tracing::debug!(event = "sync.remote_page", kind, success = result.is_ok(),
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        error_kind = ?error.map(|e| e.kind),
        retry_after_ms = error.and_then(|e| e.retry_after).map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        "Synchronization remote page finished");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_recovery_preserves_server_deadlines_and_never_bursts() {
        let now = Instant::now();
        let mut budget = RequestBudget::new(Duration::from_secs(1));
        budget.rate_limited(now, now + Duration::from_secs(30));
        assert_eq!(budget.interval_ms(), 2_000);
        budget.rate_limited(now + Duration::from_secs(1), now + Duration::from_secs(40));
        assert_eq!(
            budget.interval_ms(),
            2_000,
            "one increase per server window"
        );
        budget.succeeded(now + Duration::from_secs(39));
        assert_eq!(
            budget.admit(now + Duration::from_secs(39)),
            Err(now + Duration::from_secs(40))
        );
        assert_eq!(budget.interval_ms(), 2_000);
        budget.succeeded(now + Duration::from_secs(99));
        assert_eq!(budget.interval_ms(), 2_000);
        budget.succeeded(now + Duration::from_secs(100));
        assert_eq!(budget.interval_ms(), 1_000);
        assert_eq!(budget.admit(now + Duration::from_secs(100)), Ok(()));
        assert_eq!(
            budget.admit(now + Duration::from_secs(100)),
            Err(now + Duration::from_secs(101))
        );
    }

    #[test]
    fn high_spacing_recovers_without_page_counts_and_restores_elapsed_time() {
        let now = Instant::now();
        let mut budget =
            RequestBudget::new(Duration::from_secs(1)).with_clock(Arc::new(move || now));
        budget.restore_interval(30_000, Duration::ZERO);
        assert_eq!(budget.admit(now), Ok(()));
        for (seconds, expected) in [
            (60, 15_000),
            (120, 7_500),
            (180, 3_750),
            (240, 1_875),
            (300, 1_000),
        ] {
            budget.succeeded(now + Duration::from_secs(seconds));
            assert_eq!(budget.interval_ms(), expected);
        }
        budget.restore_interval(30_000, Duration::from_secs(180));
        assert_eq!(budget.interval_ms(), 3_750);
        budget.restore_interval(30_000, Duration::from_secs(86_400));
        assert_eq!(
            budget.interval_ms(),
            1_000,
            "expired historical pacing must not penalize restart"
        );
    }

    #[test]
    fn queued_deadline_recomputes_after_recovery_and_overlapping_server_wait_extends_anchor() {
        let now = Instant::now();
        let mut budget =
            RequestBudget::new(Duration::from_secs(1)).with_clock(Arc::new(move || now));
        budget.restore_interval(30_000, Duration::ZERO);
        assert_eq!(budget.admit(now + Duration::from_secs(50)), Ok(()));
        assert_eq!(
            budget.admit(now + Duration::from_secs(60)),
            Err(now + Duration::from_secs(65))
        );
        assert_eq!(budget.admit(now + Duration::from_secs(65)), Ok(()));
        budget.rate_limited(
            now + Duration::from_secs(66),
            now + Duration::from_secs(100),
        );
        budget.rate_limited(
            now + Duration::from_secs(67),
            now + Duration::from_secs(150),
        );
        assert_eq!(budget.interval_ms(), 30_000);
        budget.succeeded(now + Duration::from_secs(209));
        assert_eq!(budget.interval_ms(), 30_000);
        budget.succeeded(now + Duration::from_secs(210));
        assert_eq!(budget.interval_ms(), 15_000);
    }

    #[test]
    fn admission_wait_is_visible_cancellable_and_does_not_hold_the_budget_lock() {
        let shared = super::super::event_tests::shared();
        let mut changes = shared.changes.subscribe();
        let mut budget = RequestBudget::new(Duration::from_secs(3600));
        assert_eq!(budget.admit(Instant::now()), Ok(()));
        *shared.pacing.lock().expect("budget") = budget;
        let cancellation = TelegramScanCancellation::new();
        let worker_cancel = cancellation.clone();
        let worker_shared = shared.clone();
        let (done, result) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let result = wait(&worker_shared, Some(2), &worker_cancel);
            done.send(result).expect("result");
        });
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime")
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while shared.snapshot.lock().expect("snapshot").phase
                        != ChannelSyncPhase::Pacing
                    {
                        changes.changed().await.expect("visible acknowledgment");
                    }
                })
                .await
                .expect("feedback before request");
            });
        assert!(
            shared.pacing.try_lock().is_ok(),
            "waiting releases the shared lock"
        );
        assert!(
            shared
                .snapshot
                .lock()
                .expect("waiting")
                .waiting
                .iter()
                .any(|e| e.phase == ChannelSyncPhase::Pacing)
        );
        cancellation.cancel();
        assert_eq!(
            result
                .recv_timeout(Duration::from_secs(5))
                .expect("cancel promptly")
                .expect_err("cancelled")
                .kind,
            ApplicationErrorKind::Cancelled
        );
        worker.join().expect("retained worker");
    }

    #[test]
    fn pages_are_spaced_and_overlapping_floods_never_shorten_the_wait() {
        let now = Instant::now();
        let mut budget = RequestBudget::new(Duration::from_secs(1));
        assert_eq!(budget.admit(now), Ok(()));
        assert_eq!(budget.admit(now), Err(now + Duration::from_secs(1)));
        assert_eq!(budget.admit(now + Duration::from_secs(1)), Ok(()));
        budget.defer(now + Duration::from_secs(120));
        budget.defer(now + Duration::from_secs(30));
        for second in [1, 2, 30, 119] {
            assert_eq!(
                budget.admit(now + Duration::from_secs(second)),
                Err(now + Duration::from_secs(120))
            );
        }
        assert_eq!(budget.admit(now + Duration::from_secs(120)), Ok(()));
        assert_eq!(
            budget.admit(now + Duration::from_secs(120)),
            Err(now + Duration::from_secs(121))
        );
    }
}

#[cfg(test)]
mod recovery_regressions {
    use super::*;
    #[test]
    fn ordinary_network_errors_do_not_restart_spacing_recovery() {
        let epoch = Instant::now();
        let elapsed = Arc::new(AtomicU64::new(0));
        let clock = elapsed.clone();
        let shared = super::super::event_tests::shared();
        *shared.pacing.lock().expect("budget") = RequestBudget::new(Duration::from_secs(1))
            .with_clock(Arc::new(move || {
                epoch + Duration::from_secs(clock.load(Ordering::Acquire))
            }));
        shared
            .pacing
            .lock()
            .expect("budget")
            .restore_interval(30_000, Duration::ZERO);
        elapsed.store(59, Ordering::Release);
        observe::<()>(
            &shared,
            &Err(ChannelSyncFailure {
                kind: ApplicationErrorKind::Network,
                retry_after: None,
            }),
        );
        elapsed.store(60, Ordering::Release);
        let mut budget = shared.pacing.lock().expect("budget");
        assert_eq!(budget.admit(epoch + Duration::from_secs(60)), Ok(()));
        assert_eq!(budget.interval_ms(), 15_000);
    }
}
