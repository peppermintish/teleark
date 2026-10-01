use super::*;
use std::time::Duration;
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultKeyPhase {
    Queued,
    Loading,
    CheckingChannel,
    Waiting,
    Generating,
    WrappingPassword,
    WrappingRecovery,
    Securing,
    Saving,
    Completed,
}
#[derive(Clone)]
pub struct VaultKeyProgress {
    state: Arc<Mutex<VaultKeySnapshot>>,
    cancelled: Arc<AtomicBool>,
    changed: Arc<watch::Sender<u64>>,
}
#[derive(Clone, Debug)]
pub struct VaultKeySnapshot {
    pub phase: VaultKeyPhase,
    pub phase_since: Instant,
    pub last_activity: Instant,
    pub timeline: Vec<(VaultKeyPhase, u64)>,
    pub finished: bool,
    pub error: Option<ApplicationErrorKind>,
    pub retry_at: Option<Instant>,
    pub dropped_events: u64,
    started: Instant,
}
impl Default for VaultKeyProgress {
    fn default() -> Self {
        Self::new()
    }
}
impl VaultKeyProgress {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            state: Arc::new(Mutex::new(VaultKeySnapshot {
                phase: VaultKeyPhase::Queued,
                phase_since: now,
                last_activity: now,
                started: now,
                timeline: vec![(VaultKeyPhase::Queued, 0)],
                finished: false,
                error: None,
                retry_at: None,
                dropped_events: 0,
            })),
            cancelled: Arc::new(AtomicBool::new(false)),
            changed: Arc::new(watch::channel(0).0),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn snapshot(&self) -> VaultKeySnapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }
    pub(crate) fn check_cancelled(&self) -> Result<(), ApplicationError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        Ok(())
    }
    pub(crate) fn activity(&self) -> Result<(), ApplicationError> {
        self.check_cancelled()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let active = !state.finished;
        if active {
            state.last_activity = Instant::now();
        }
        drop(state);
        if active {
            self.changed
                .send_modify(|revision| *revision = revision.wrapping_add(1));
        }
        Ok(())
    }
    pub(crate) fn phase(&self, phase: VaultKeyPhase) -> Result<(), ApplicationError> {
        self.check_cancelled()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let changed = !state.finished && state.phase != phase;
        if changed {
            state.phase = phase;
            state.error = None;
            state.retry_at = None;
            state.phase_since = Instant::now();
            state.last_activity = state.phase_since;
            let millis = state
                .started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            // A command can combine setup and rotation. Retain all transitions
            // in that bounded sequence, including its terminal outcome.
            if state.timeline.len() == 16 {
                state.timeline.remove(0);
                state.dropped_events = state.dropped_events.saturating_add(1);
            }
            state.timeline.push((phase, millis));
        }
        drop(state);
        if changed {
            self.changed
                .send_modify(|revision| *revision = revision.wrapping_add(1));
        }
        Ok(())
    }
    pub(crate) fn finish(&self, error: Option<ApplicationErrorKind>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.error = error;
        state.finished = true;
        state.retry_at = None;
        state.last_activity = Instant::now();
        if error.is_none() {
            state.phase = VaultKeyPhase::Completed;
            state.phase_since = state.last_activity;
            let millis = state
                .started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            if state.timeline.len() == 16 {
                state.timeline.remove(0);
                state.dropped_events = state.dropped_events.saturating_add(1);
            }
            state.timeline.push((VaultKeyPhase::Completed, millis));
        }
        drop(state);
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    pub(super) fn waiting(
        &self,
        error: ApplicationErrorKind,
        delay: Duration,
    ) -> Result<(), ApplicationError> {
        self.phase(VaultKeyPhase::Waiting)?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.error = Some(error);
        state.retry_at = Some(Instant::now() + delay);
        drop(state);
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        Ok(())
    }
}

#[cfg(test)]
mod signal_tests {
    use super::*;

    #[test]
    fn key_phase_and_terminal_state_wake_presentation() {
        let progress = VaultKeyProgress::new();
        let mut changes = progress.subscribe();
        assert!(!changes.has_changed().expect("open signal"));
        progress.phase(VaultKeyPhase::Loading).expect("phase");
        assert!(changes.has_changed().expect("phase event"));
        changes.borrow_and_update();
        progress.finish(None);
        assert!(changes.has_changed().expect("terminal event"));
        assert!(progress.snapshot().finished);
    }
    #[test]
    fn repeated_automatic_retries_bound_history_and_retain_terminal_feedback() {
        let progress = VaultKeyProgress::new();
        for _ in 0..40 {
            progress.phase(VaultKeyPhase::Loading).expect("phase");
            progress
                .waiting(ApplicationErrorKind::Network, Duration::from_secs(2))
                .expect("wait");
            let waiting = progress.snapshot();
            assert!(!waiting.finished);
            assert_eq!(waiting.error, Some(ApplicationErrorKind::Network));
            assert!(waiting.retry_at.is_some());
        }
        progress.finish(None);
        let state = progress.snapshot();
        assert_eq!(state.timeline.len(), 16);
        assert_eq!(
            state.timeline.last().expect("terminal").0,
            VaultKeyPhase::Completed
        );
        assert!(state.dropped_events > 0);
        assert!(state.finished);
        assert!(state.retry_at.is_none());
    }
}
