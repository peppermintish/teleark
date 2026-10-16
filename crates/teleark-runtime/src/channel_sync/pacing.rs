//! Account-wide admission for remote sync pages. Local reads and contiguous
//! pushes bypass it; waiting never owns the coordinator or a network reactor.
use super::*;

pub(super) struct RequestBudget {
    interval: Duration,
    clock: SyncClock,
    next: Option<Instant>,
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
            clock: Arc::new(Instant::now),
            next: None,
            flood: None,
        }
    }

    pub fn with_clock(mut self, clock: SyncClock) -> Self {
        self.clock = clock;
        self
    }

    fn admit(&mut self, now: Instant) -> Result<(), Instant> {
        if let Some(deadline) = self.next.into_iter().chain(self.flood).max()
            && deadline > now
        {
            return Err(deadline);
        }
        self.next = now.checked_add(self.interval);
        Ok(())
    }

    pub fn defer(&mut self, deadline: Instant) {
        self.flood = Some(self.flood.map_or(deadline, |old| old.max(deadline)));
    }

    pub fn flood(&self) -> Option<Instant> {
        self.flood
    }
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
                if announced != Some(phase) {
                    publish(shared, phase, chat, None, Some(at));
                    announced = Some(phase);
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
    if let Err(error) = result
        && let Some(delay) = error.retry_after
        && let Ok(mut budget) = shared.pacing.lock()
        && let Some(deadline) = (budget.clock)().checked_add(delay)
    {
        // Fence workers already queued for admission before the coordinator
        // receives the completion. No queued page can race past a known wait.
        budget.defer(deadline);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                .expect("active")
                .active
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
