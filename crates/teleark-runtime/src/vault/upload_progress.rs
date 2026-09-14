//! Memory-only progress for an arbitrarily sized selection; content work stays bounded.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultUploadSelectionPhase {
    Queued,
    Inspecting,
    SavingQueue,
    CheckingStorage,
    Uploading,
    Finished,
}

#[derive(Clone, Debug)]
pub struct VaultUploadSelectionSnapshot {
    pub phase: VaultUploadSelectionPhase,
    pub total: usize,
    pub inspected: usize,
    pub saved: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub paused: usize,
    pub finished: bool,
    pub cancel_requested: bool,
    pub error: Option<ApplicationErrorKind>,
    pub phase_since: Instant,
    pub last_activity: Instant,
    pub timeline: Vec<(VaultUploadSelectionPhase, u64)>,
    started: Instant,
}

#[derive(Clone)]
pub struct VaultUploadSelectionProgress {
    state: Arc<Mutex<VaultUploadSelectionSnapshot>>,
    pub(super) cancellation: Arc<AtomicBool>,
}

impl VaultUploadSelectionProgress {
    pub fn new(total: usize) -> Self {
        let now = Instant::now();
        Self {
            state: Arc::new(Mutex::new(VaultUploadSelectionSnapshot {
                phase: VaultUploadSelectionPhase::Queued,
                total,
                inspected: 0,
                saved: 0,
                completed: 0,
                failed: 0,
                cancelled: 0,
                paused: 0,
                finished: false,
                cancel_requested: false,
                error: None,
                phase_since: now,
                last_activity: now,
                started: now,
                timeline: vec![(VaultUploadSelectionPhase::Queued, 0)],
            })),
            cancellation: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancellation.store(true, Ordering::Release);
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancel_requested = true;
    }

    pub fn snapshot(&self) -> VaultUploadSelectionSnapshot {
        let mut snapshot = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        snapshot.cancel_requested = self.cancellation.load(Ordering::Acquire);
        snapshot
    }

    pub(super) fn check_cancelled(&self) -> Result<(), ApplicationError> {
        if self.cancellation.load(Ordering::Acquire) {
            Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
        } else {
            Ok(())
        }
    }

    pub(super) fn phase(&self, phase: VaultUploadSelectionPhase) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.finished && state.phase != phase {
            state.phase = phase;
            state.phase_since = Instant::now();
            state.last_activity = state.phase_since;
            let elapsed = state
                .started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            // Six monotonic operation phases, independent of the number of files/chunks.
            if state.timeline.len() < 6 {
                state.timeline.push((phase, elapsed));
            }
        }
    }

    pub(super) fn set_total(&self, total: usize) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).total = total;
    }

    pub(super) fn saved(&self, count: usize) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.saved = state.saved.saturating_add(count);
        state.last_activity = Instant::now();
    }

    pub(super) fn inspected(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.inspected += 1;
        state.last_activity = Instant::now();
    }

    pub(super) fn record(&self, result: Result<(), ApplicationErrorKind>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(()) => state.completed += 1,
            Err(ApplicationErrorKind::Cancelled) => state.cancelled += 1,
            Err(_) => state.failed += 1,
        }
        state.last_activity = Instant::now();
    }

    pub(super) fn record_paused(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.paused += 1;
        state.last_activity = Instant::now();
    }

    pub fn finish(&self, error: Option<ApplicationErrorKind>) {
        self.phase(VaultUploadSelectionPhase::Finished);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.finished = true;
        state.error = error;
        state.last_activity = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_counters_are_incremental_and_history_does_not_grow_with_files() {
        let progress = VaultUploadSelectionProgress::new(10_000);
        progress.phase(VaultUploadSelectionPhase::Inspecting);
        for _ in 0..10_000 {
            progress.inspected();
        }
        progress.phase(VaultUploadSelectionPhase::CheckingStorage);
        progress.phase(VaultUploadSelectionPhase::Uploading);
        for index in 0..10_000 {
            progress.record(if index % 10 == 0 {
                Err(ApplicationErrorKind::SourceChanged)
            } else {
                Ok(())
            });
        }
        progress.finish(Some(ApplicationErrorKind::SourceChanged));
        let state = progress.snapshot();
        assert!(state.finished);
        assert_eq!(state.completed, 9_000);
        assert_eq!(state.failed, 1_000);
        assert_eq!(state.inspected, 10_000);
        assert_eq!(state.timeline.len(), 5);
    }

    #[test]
    fn cancelled_preflight_does_not_touch_an_unavailable_path() {
        let progress = VaultUploadSelectionProgress::new(1);
        progress.cancel();
        let path = std::env::temp_dir().join("teleark-nonexistent-cancelled-upload-source");
        assert_eq!(
            inspect_upload_sources_observed(&[path], Some(&progress))
                .expect_err("cancel first")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        assert_eq!(progress.snapshot().inspected, 0);
    }
    #[test]
    fn saving_queue_reports_only_committed_files_and_preserves_all_phase_transitions() {
        let progress = VaultUploadSelectionProgress::new(513);
        progress.phase(VaultUploadSelectionPhase::Inspecting);
        progress.phase(VaultUploadSelectionPhase::SavingQueue);
        assert_eq!(progress.snapshot().saved, 0);
        for count in [128, 128, 128, 128, 1] {
            progress.saved(count);
        }
        assert_eq!(progress.snapshot().saved, 513);
        assert_eq!(progress.snapshot().completed, 0);
        progress.phase(VaultUploadSelectionPhase::CheckingStorage);
        progress.phase(VaultUploadSelectionPhase::Uploading);
        progress.phase(VaultUploadSelectionPhase::Finished);
        assert_eq!(progress.snapshot().timeline.len(), 6);
        assert_eq!(
            progress
                .snapshot()
                .timeline
                .last()
                .expect("terminal phase")
                .0,
            VaultUploadSelectionPhase::Finished
        );
    }
}
