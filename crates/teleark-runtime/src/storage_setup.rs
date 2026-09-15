//! Transient, bounded feedback for remote private-channel setup and replacement.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};
use teleark_core::ApplicationErrorKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageSetupPhase {
    CheckingBinding,
    ReadingDialogs,
    VerifyingChannel,
    DiscoveringReplacement,
    CreatingChannel,
    PreparingChannel,
    SavingBinding,
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
pub struct StorageSetupProgress(Arc<Mutex<StorageSetupSnapshot>>);

impl Default for StorageSetupProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageSetupProgress {
    pub fn new() -> Self {
        let now = Instant::now();
        Self(Arc::new(Mutex::new(StorageSetupSnapshot {
            phase: StorageSetupPhase::CheckingBinding,
            started: now,
            phase_since: now,
            last_activity: now,
            timeline: VecDeque::from([(StorageSetupPhase::CheckingBinding, 0)]),
            omitted: 0,
            finished: false,
            error: None,
        })))
    }

    pub fn snapshot(&self) -> StorageSetupSnapshot {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn phase(&self, phase: StorageSetupPhase) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
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
    }

    pub fn finish(&self, error: Option<ApplicationErrorKind>) {
        if error.is_none() {
            self.phase(StorageSetupPhase::Completed);
        }
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.error = error;
        state.finished = true;
        state.last_activity = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_history_bounds_transitions_and_keeps_terminal_result() {
        let progress = StorageSetupProgress::new();
        for _ in 0..20 {
            progress.phase(StorageSetupPhase::ReadingDialogs);
            progress.phase(StorageSetupPhase::VerifyingChannel);
        }
        progress.finish(Some(ApplicationErrorKind::StorageAccessDenied));
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
