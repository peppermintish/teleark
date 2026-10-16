//! Bounded restore selections owned by the download worker, never by the view.
use super::*;
use std::collections::{BTreeSet, VecDeque};

pub const VAULT_DOWNLOAD_BATCH_LIMIT: usize = 65_536;
const EVENT_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultDownloadBatchPhase {
    Queued,
    Restoring,
    Stopping,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct VaultDownloadBatchEvent {
    pub phase: VaultDownloadBatchPhase,
    pub at: Instant,
    pub package_id: Option<u64>,
    pub failure: Option<ApplicationErrorKind>,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct VaultDownloadBatchSnapshot {
    pub phase: VaultDownloadBatchPhase,
    pub phase_started: Instant,
    pub last_activity: Instant,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub current: Option<u64>,
    pub failure: Option<ApplicationErrorKind>,
    pub events: VecDeque<VaultDownloadBatchEvent>,
    pub dropped_events: u64,
}

impl VaultDownloadBatchSnapshot {
    pub fn active(&self) -> bool {
        matches!(
            self.phase,
            VaultDownloadBatchPhase::Queued
                | VaultDownloadBatchPhase::Restoring
                | VaultDownloadBatchPhase::Stopping
        )
    }
}

#[derive(Clone)]
pub struct VaultDownloadBatchProgress {
    state: Arc<Mutex<VaultDownloadBatchSnapshot>>,
    cancellation: Arc<AtomicBool>,
    changes: tokio::sync::watch::Sender<()>,
    failed_packages: Arc<Mutex<Vec<u64>>>,
}

impl VaultDownloadBatchProgress {
    pub fn new(total: usize) -> Self {
        let now = Instant::now();
        Self {
            state: Arc::new(Mutex::new(VaultDownloadBatchSnapshot {
                phase: VaultDownloadBatchPhase::Queued,
                phase_started: now,
                last_activity: now,
                total,
                completed: 0,
                failed: 0,
                skipped: 0,
                current: None,
                failure: None,
                events: VecDeque::from([VaultDownloadBatchEvent {
                    phase: VaultDownloadBatchPhase::Queued,
                    at: now,
                    package_id: None,
                    failure: None,
                }]),
                dropped_events: 0,
            })),
            cancellation: Arc::new(AtomicBool::new(false)),
            changes: tokio::sync::watch::channel(()).0,
            failed_packages: Default::default(),
        }
    }

    pub fn snapshot(&self) -> VaultDownloadBatchSnapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<()> {
        self.changes.subscribe()
    }

    /// Read only on an explicit retry. Live snapshots retain counters rather
    /// than repeatedly copying an ever-growing failure collection.
    pub fn failed_packages(&self) -> Vec<u64> {
        self.failed_packages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Stop before the next file; the current authenticated restore retains its
    /// ordinary pause/cancel controls in Transfers and finishes safely.
    pub fn stop_remaining(&self) {
        self.cancellation.store(true, Ordering::Release);
        self.update(|state| {
            if state.active() {
                state.phase = VaultDownloadBatchPhase::Stopping;
            }
        });
    }

    fn update(&self, change: impl FnOnce(&mut VaultDownloadBatchSnapshot)) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.active() {
            return;
        }
        let previous = state.phase;
        change(&mut state);
        let now = Instant::now();
        if state.phase != previous {
            state.phase_started = now;
        }
        state.last_activity = now;
        let event = VaultDownloadBatchEvent {
            phase: state.phase,
            at: now,
            package_id: state.current,
            failure: state.failure,
        };
        if state.events.len() == EVENT_LIMIT {
            state.events.pop_front();
            state.dropped_events = state.dropped_events.saturating_add(1);
        }
        state.events.push_back(event);
        drop(state);
        self.changes.send_replace(());
    }

    pub(crate) fn finish(&self, failure: Option<ApplicationErrorKind>) {
        self.update(|state| {
            state.current = None;
            state.skipped = state.total.saturating_sub(state.completed + state.failed);
            state.failure = failure.or(state.failure);
            state.phase = if failure == Some(ApplicationErrorKind::Cancelled) {
                VaultDownloadBatchPhase::Cancelled
            } else if state.failure.is_some() || state.failed > 0 {
                VaultDownloadBatchPhase::Failed
            } else if state.skipped > 0 {
                VaultDownloadBatchPhase::Cancelled
            } else {
                VaultDownloadBatchPhase::Completed
            };
        });
    }

    fn run(
        &self,
        packages: &[u64],
        mut current: impl FnMut() -> bool,
        mut restore: impl FnMut(u64) -> Result<(), ApplicationError>,
    ) -> Result<(), ApplicationError> {
        for package in packages {
            if self.cancellation.load(Ordering::Acquire) || !current() {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            self.update(|state| {
                state.phase = if self.cancellation.load(Ordering::Acquire) {
                    VaultDownloadBatchPhase::Stopping
                } else {
                    VaultDownloadBatchPhase::Restoring
                };
                state.current = Some(*package);
            });
            if self.cancellation.load(Ordering::Acquire) || !current() {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            let result = restore(*package);
            if result
                .as_ref()
                .is_err_and(|error| error.kind() != ApplicationErrorKind::Cancelled)
            {
                self.failed_packages
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(*package);
            }
            self.update(|state| match &result {
                Ok(()) => state.completed += 1,
                Err(error) => {
                    state.failed += usize::from(error.kind() != ApplicationErrorKind::Cancelled);
                    state.failure = Some(error.kind());
                }
            });
            if let Err(error) = result
                && matches!(
                    error.kind(),
                    ApplicationErrorKind::Authorization
                        | ApplicationErrorKind::VaultKeyUnavailable
                        | ApplicationErrorKind::Cancelled
                )
            {
                return Err(error);
            }
        }
        Ok(())
    }
}

impl DesktopVault {
    /// Admission only copies bounded IDs. Payloads use the same authenticated,
    /// streaming and durable restore path as individual downloads.
    pub fn submit_download_batch(
        &self,
        account_id: i64,
        chat_id: i64,
        packages: Vec<u64>,
        progress: VaultDownloadBatchProgress,
    ) -> Result<VaultJob<()>, ApplicationError> {
        let result = (|| {
            if packages.len() > VAULT_DOWNLOAD_BATCH_LIMIT {
                return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
            }
            if account_id <= 0
                || chat_id <= 0
                || packages.is_empty()
                || packages.contains(&0)
                || packages.iter().copied().collect::<BTreeSet<_>>().len() != packages.len()
                || progress.snapshot().total != packages.len()
            {
                return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
            }
            self.submit(|reply| VaultCommand::DownloadBatch {
                account_id,
                chat_id,
                packages,
                progress: progress.clone(),
                reply,
            })
        })();
        if let Err(error) = &result {
            progress.finish(Some(error.kind()));
        }
        result
    }
}

impl VaultOwner {
    pub(super) fn download_batch(
        &mut self,
        account: i64,
        chat: i64,
        packages: &[u64],
        progress: &VaultDownloadBatchProgress,
    ) -> Result<(), ApplicationError> {
        let session = self.session.clone();
        let lifecycle = self.telegram.lifecycle();
        let generation = self.session_generation;
        let key_revision = self.catalog_key_revision;
        let batch = random_transfer_id()?;
        progress.run(
            packages,
            || {
                lifecycle.snapshot().1 == Some(account)
                    && session.lock().is_ok_and(|session| {
                        !session.closing && session.scan_revision() == (generation, key_revision)
                    })
            },
            |package| {
                self.download(account, chat, PackageId::new(package), None, Some(batch))
                    .map(|_| ())
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_restore_reports_execution_and_stop_preserves_current_success() {
        let progress = VaultDownloadBatchProgress::new(3);
        let mut changes = progress.subscribe();
        let (entered, blocked) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let owner = progress.clone();
        let worker = thread::spawn(move || {
            let result = owner.run(
                &[1, 2, 3],
                || true,
                |package| {
                    entered.send(package).expect("entered");
                    wait.recv().expect("release");
                    Ok(())
                },
            );
            owner.finish(result.as_ref().err().map(ApplicationError::kind));
        });
        assert_eq!(
            blocked
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("first file"),
            1
        );
        assert!(changes.has_changed().expect("phase published"));
        changes.borrow_and_update();
        assert_eq!(progress.snapshot().current, Some(1));
        progress.stop_remaining();
        assert_eq!(progress.snapshot().phase, VaultDownloadBatchPhase::Stopping);
        release.send(()).expect("finish current");
        worker.join().expect("worker");
        let final_state = progress.snapshot();
        assert_eq!((final_state.completed, final_state.skipped), (1, 2));
        assert_eq!(final_state.phase, VaultDownloadBatchPhase::Cancelled);
        assert!(blocked.try_recv().is_err());
        progress.stop_remaining();
        assert_eq!(progress.snapshot().events.len(), final_state.events.len());
    }

    #[test]
    fn file_failures_continue_but_stale_sessions_stop_and_timeline_is_bounded() {
        let progress = VaultDownloadBatchProgress::new(100);
        let packages = (1..=100).collect::<Vec<_>>();
        progress
            .run(
                &packages,
                || true,
                |id| {
                    if id == 2 {
                        Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest))
                    } else {
                        Ok(())
                    }
                },
            )
            .expect("remaining files restored");
        progress.finish(None);
        let snapshot = progress.snapshot();
        assert_eq!((snapshot.completed, snapshot.failed), (99, 1));
        assert_eq!(snapshot.phase, VaultDownloadBatchPhase::Failed);
        assert_eq!(snapshot.events.len(), EVENT_LIMIT);
        assert!(snapshot.dropped_events > 0);
        assert_eq!(
            snapshot.events.back().expect("terminal retained").phase,
            VaultDownloadBatchPhase::Failed
        );
        let stale = VaultDownloadBatchProgress::new(1);
        assert_eq!(
            stale
                .run(&[1], || false, |_| panic!("stale account must not restore"))
                .expect_err("stale")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        stale.finish(Some(ApplicationErrorKind::Cancelled));
        assert_eq!(stale.snapshot().skipped, 1);
    }
}
