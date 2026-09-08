use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultKeyPhase {
    Queued,
    Generating,
    WrappingPassword,
    WrappingRecovery,
    Saving,
    Completed,
}
#[derive(Clone)]
pub struct VaultKeyProgress {
    state: Arc<Mutex<VaultKeySnapshot>>,
    cancelled: Arc<AtomicBool>,
}
#[derive(Clone, Debug)]
pub struct VaultKeySnapshot {
    pub phase: VaultKeyPhase,
    pub phase_since: Instant,
    pub last_activity: Instant,
    pub timeline: Vec<(VaultKeyPhase, u64)>,
    pub finished: bool,
    pub error: Option<ApplicationErrorKind>,
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
            })),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn snapshot(&self) -> VaultKeySnapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub(super) fn phase(&self, phase: VaultKeyPhase) -> Result<(), ApplicationError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.finished && state.phase != phase {
            state.phase = phase;
            state.phase_since = Instant::now();
            state.last_activity = state.phase_since;
            let millis = state
                .started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            // Six monotonic phases per operation; no progress samples or secret data.
            if state.timeline.len() < 6 {
                state.timeline.push((phase, millis));
            }
        }
        Ok(())
    }
    pub(super) fn finish(&self, error: Option<ApplicationErrorKind>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.error = error;
        state.finished = true;
        state.last_activity = Instant::now();
        if error.is_none() {
            state.phase = VaultKeyPhase::Completed;
            state.phase_since = state.last_activity;
            let millis = state
                .started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            if state.timeline.len() < 6 {
                state.timeline.push((VaultKeyPhase::Completed, millis));
            }
        }
    }
}
