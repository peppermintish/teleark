//! Transient, bounded feedback for remote private-channel setup and replacement.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};
use teleark_core::ApplicationErrorKind;
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageSetupPhase {
    CheckingBinding,
    ReadingDialogs,
    VerifyingChannel,
    DiscoveringReplacement,
    CreatingChannel,
    PreparingChannel,
    SavingBinding,
    CreatingKey,
    Completed,
}

#[derive(Clone, Debug)]
pub struct StorageSetupSnapshot {
    pub phase: StorageSetupPhase,
    pub started: Instant,
    pub phase_since: Instant,
    pub last_activity: Instant,
    pub timeline: VecDeque<(StorageSetupPhase, u64)>,
    pub omitted: u64,
    pub finished: bool,
    pub error: Option<ApplicationErrorKind>,
}

#[derive(Clone)]
pub struct StorageSetupProgress {
    state: Arc<Mutex<StorageSetupSnapshot>>,
    changed: Arc<watch::Sender<u64>>,
}

impl Default for StorageSetupProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageSetupProgress {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            state: Arc::new(Mutex::new(StorageSetupSnapshot {
                phase: StorageSetupPhase::CheckingBinding,
                started: now,
                phase_since: now,
                last_activity: now,
                timeline: VecDeque::from([(StorageSetupPhase::CheckingBinding, 0)]),
                omitted: 0,
                finished: false,
                error: None,
            })),
            changed: Arc::new(watch::channel(0).0),
        }
    }

    pub fn snapshot(&self) -> StorageSetupSnapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub fn phase(&self, phase: StorageSetupPhase) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.finished {
            return;
        }
        let now = Instant::now();
        state.last_activity = now;
        if state.phase != phase {
            state.phase = phase;
            state.phase_since = now;
            if state.timeline.len() == 16 {
                state.timeline.pop_front();
                state.omitted += 1;
            }
            let millis = state
                .started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            state.timeline.push_back((phase, millis));
        }
        drop(state);
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub fn finish(&self, error: Option<ApplicationErrorKind>) {
        if error.is_none() {
            self.phase(StorageSetupPhase::Completed);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.error = error;
        state.finished = true;
        state.last_activity = Instant::now();
        drop(state);
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_history_bounds_transitions_and_keeps_terminal_result() {
        let progress = StorageSetupProgress::new();
        let mut changes = progress.subscribe();
        assert!(!changes.has_changed().expect("open signal"));
        for _ in 0..20 {
            progress.phase(StorageSetupPhase::ReadingDialogs);
            progress.phase(StorageSetupPhase::VerifyingChannel);
        }
        progress.finish(Some(ApplicationErrorKind::StorageAccessDenied));
        assert!(changes.has_changed().expect("phase and terminal event"));
        changes.borrow_and_update();
        assert!(!changes.has_changed().expect("coalesced signal"));
        let snapshot = progress.snapshot();
        assert!(snapshot.finished);
        assert_eq!(
            snapshot.error,
            Some(ApplicationErrorKind::StorageAccessDenied)
        );
        assert!(snapshot.timeline.len() <= 16);
        assert!(snapshot.omitted > 0);
        progress.phase(StorageSetupPhase::Completed);
        assert_eq!(progress.snapshot().error, snapshot.error);
    }
}
