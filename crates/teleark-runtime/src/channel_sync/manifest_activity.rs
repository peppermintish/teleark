//! Manifest projection reports through the same account event feed as messages.
use super::*;

#[derive(Clone, Debug)]
pub struct ManagedScanStatus {
    pub chat_id: i64,
    pub phase: ChannelSyncPhase,
    pub phase_started: Instant,
    pub last_activity: Instant,
    pub completed: usize,
    pub total: Option<usize>,
    pub cached: usize,
    pub rejected: usize,
    pub failure: Option<ApplicationErrorKind>,
    pub retry_after: Option<Duration>,
}
impl ManagedScanStatus {
    pub fn active(&self) -> bool {
        !matches!(
            self.phase,
            ChannelSyncPhase::ManifestCompleted
                | ChannelSyncPhase::ManifestFailed
                | ChannelSyncPhase::ManifestCancelled
        )
    }
}

type Listener = Arc<dyn Fn(ManagedScanStatus, bool) + Send + Sync>;
#[derive(Clone)]
pub struct ManagedScanObserver {
    state: Arc<Mutex<ManagedScanStatus>>,
    listener: Listener,
}
impl ManagedScanObserver {
    pub(crate) fn new(chat: i64, listener: Listener) -> Self {
        let now = Instant::now();
        let state = ManagedScanStatus {
            chat_id: chat,
            phase: ChannelSyncPhase::ManifestQueued,
            phase_started: now,
            last_activity: now,
            completed: 0,
            total: None,
            cached: 0,
            rejected: 0,
            failure: None,
            retry_after: None,
        };
        listener(state.clone(), true);
        Self {
            state: Arc::new(Mutex::new(state)),
            listener,
        }
    }
    pub(crate) fn silent(chat: i64) -> Self {
        Self::new(chat, Arc::new(|_, _| {}))
    }
    fn update(&self, change: impl FnOnce(&mut ManagedScanStatus)) {
        let next = self.state.lock().ok().map(|mut state| {
            let previous = state.phase;
            change(&mut state);
            let transitioned = state.phase != previous;
            state.last_activity = Instant::now();
            if transitioned {
                state.phase_started = state.last_activity;
            }
            (state.clone(), transitioned)
        });
        if let Some((state, transitioned)) = next {
            (self.listener)(state, transitioned);
        }
    }
    pub(crate) fn phase(&self, phase: ChannelSyncPhase) {
        self.update(|state| state.phase = phase);
    }
    pub(crate) fn retry(&self, error: &ApplicationError, delay: Duration) {
        self.update(|state| {
            state.phase = if error.retry_after().is_some() {
                ChannelSyncPhase::RateLimited
            } else {
                ChannelSyncPhase::Waiting
            };
            state.failure = Some(error.kind());
            state.retry_after = Some(delay);
        });
    }
    pub(crate) fn restart(&self) {
        self.update(|state| {
            state.phase = ChannelSyncPhase::ManifestQueued;
            state.failure = None;
            state.retry_after = None;
            state.completed = 0;
            state.total = None;
            state.cached = 0;
            state.rejected = 0;
        });
    }
    pub(crate) fn total(&self, total: usize) {
        self.update(|state| state.total = Some(total));
    }
    pub(crate) fn advance(&self, cached: bool, rejected: bool) {
        self.update(|state| {
            state.completed += 1;
            state.cached += usize::from(cached);
            state.rejected += usize::from(rejected);
        });
    }
    pub(crate) fn finish(&self, failure: Option<ApplicationErrorKind>) {
        self.update(|state| {
            state.failure = failure;
            state.retry_after = None;
            state.phase = match failure {
                None => ChannelSyncPhase::ManifestCompleted,
                Some(ApplicationErrorKind::Cancelled) => ChannelSyncPhase::ManifestCancelled,
                Some(_) => ChannelSyncPhase::ManifestFailed,
            };
        });
    }
}

impl ChannelSync {
    /// Queue acknowledgement occurs before submitting work to the Vault owner.
    pub fn observe_managed_scan(&self, chat: i64) -> ManagedScanObserver {
        let generation = self
            .inner
            .shared
            .manifest_generation
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        let weak = Arc::downgrade(&self.inner.shared);
        ManagedScanObserver::new(
            chat,
            Arc::new(move |scan, transitioned| {
                let Some(shared) = weak
                    .upgrade()
                    .filter(|shared| !shared.stop.load(Ordering::Acquire))
                else {
                    return;
                };
                if let Ok(mut snapshot) = shared.snapshot.lock() {
                    if transitioned {
                        if snapshot.events.len() == EVENT_CAPACITY {
                            snapshot.events.pop_front();
                            snapshot.dropped_events = snapshot.dropped_events.saturating_add(1);
                        }
                        snapshot.events.push_back(ChannelSyncEvent {
                            phase: scan.phase,
                            chat_id: Some(chat),
                            at: scan.last_activity,
                            failure: scan.failure,
                        });
                    }
                    if shared.manifest_generation.load(Ordering::Acquire) == generation {
                        snapshot.managed_scan = Some(scan);
                    }
                }
                shared.changes.send_replace(());
            }),
        )
    }
}

impl Drop for ManagedScanObserver {
    fn drop(&mut self) {
        if Arc::strong_count(&self.state) == 1
            && self.state.lock().is_ok_and(|state| state.active())
        {
            self.finish(Some(ApplicationErrorKind::Cancelled));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_feedback_precedes_work_and_stale_terminal_events_do_not_replace_newer_activity() {
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(ChannelSyncSnapshot::new(1, 0)),
            changes: tokio::sync::watch::channel(()).0,
            deltas: Mutex::new(feed::DeltaJournal::default()),
            managed_id: AtomicI64::new(2),
            observation: AtomicU64::new(0),
            manifest_generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            active: Mutex::new(None),
        });
        let (sender, _receiver) = mpsc::sync_channel(1);
        let sync = ChannelSync {
            inner: Arc::new(Owner {
                sender,
                shared,
                join: Mutex::new(None),
                worker: thread::current(),
            }),
        };
        let mut subscription = sync.subscribe();
        let old = sync.observe_managed_scan(2);
        assert!(
            subscription
                .receiver
                .has_changed()
                .expect("queued wake before work")
        );
        subscription.receiver.borrow_and_update();
        assert_eq!(
            sync.snapshot()
                .expect("snapshot")
                .managed_scan
                .expect("queued scan")
                .phase,
            ChannelSyncPhase::ManifestQueued
        );
        let current = sync.observe_managed_scan(2);
        old.finish(Some(ApplicationErrorKind::Cancelled));
        assert_eq!(
            sync.snapshot()
                .expect("snapshot")
                .managed_scan
                .expect("newer scan")
                .phase,
            ChannelSyncPhase::ManifestQueued
        );
        current.phase(ChannelSyncPhase::ManifestReading);
        current.total(2);
        for _ in 0..150 {
            current.phase(ChannelSyncPhase::ManifestReceiving);
            current.phase(ChannelSyncPhase::ManifestVerifying);
        }
        current.advance(false, false);
        current.advance(true, false);
        current.finish(None);
        let snapshot = sync.snapshot().expect("completed snapshot");
        let scan = snapshot.managed_scan.expect("scan");
        assert_eq!((scan.completed, scan.total, scan.cached), (2, Some(2), 1));
        assert!(!scan.active());
        assert_eq!(snapshot.events.len(), EVENT_CAPACITY);
        assert!(snapshot.dropped_events > 0);
        assert_eq!(
            snapshot.events.back().expect("terminal retained").phase,
            ChannelSyncPhase::ManifestCompleted
        );
        let abandoned = sync.observe_managed_scan(2);
        drop(abandoned);
        assert_eq!(
            sync.snapshot()
                .expect("abandoned task")
                .managed_scan
                .expect("terminal")
                .phase,
            ChannelSyncPhase::ManifestCancelled
        );
    }
}
