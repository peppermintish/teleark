use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

const RUNNING: u8 = 0;
const PAUSE_REQUESTED: u8 = 1;
const CANCEL_REQUESTED: u8 = 2;

/// Cooperative control observed at durable batch boundaries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ControlState {
    #[default]
    Running,
    PauseRequested,
    CancelRequested,
}

/// Cloneable pause/cancel handle for one coordinator run.
///
/// Cancellation is terminal for this token. [`ControlToken::resume`] only
/// clears a pause request and cannot undo cancellation.
#[derive(Clone, Debug, Default)]
pub struct ControlToken {
    state: Arc<AtomicU8>,
}

impl ControlToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> ControlState {
        match self.state.load(Ordering::Acquire) {
            PAUSE_REQUESTED => ControlState::PauseRequested,
            CANCEL_REQUESTED => ControlState::CancelRequested,
            _ => ControlState::Running,
        }
    }

    pub fn request_pause(&self) {
        let _ = self.state.compare_exchange(
            RUNNING,
            PAUSE_REQUESTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub fn request_cancel(&self) {
        self.state.store(CANCEL_REQUESTED, Ordering::Release);
    }

    /// Returns `true` when a pause was cleared.
    pub fn resume(&self) -> bool {
        self.state
            .compare_exchange(
                PAUSE_REQUESTED,
                RUNNING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}
