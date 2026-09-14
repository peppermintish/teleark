//! Event-driven session loss. Connection events and RPC failures request one
//! home-DC confirmation; silence never causes polling or implies revocation.
use grammers_client::client::{RetryContext, RetryPolicy};
use grammers_mtsender::InvocationError;
use std::{
    ops::ControlFlow,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthorizationPhase {
    #[default]
    SignedOut,
    Authorized,
    Checking,
    Revoked,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuthorizationSnapshot {
    pub generation: u64,
    pub network_generation: u64,
    pub account_id: Option<i64>,
    pub phase: AuthorizationPhase,
}

#[derive(Clone)]
pub struct AuthorizationMonitor {
    state: Arc<Mutex<AuthorizationSnapshot>>,
    changes: watch::Sender<AuthorizationSnapshot>,
}

impl Default for AuthorizationMonitor {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            changes: watch::channel(AuthorizationSnapshot::default()).0,
        }
    }
}

impl AuthorizationMonitor {
    pub fn snapshot(&self) -> AuthorizationSnapshot {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn subscribe(&self) -> AuthorizationUpdates {
        AuthorizationUpdates {
            receiver: self.changes.subscribe(),
        }
    }

    fn update(&self, change: impl FnOnce(&mut AuthorizationSnapshot)) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let before = *state;
        change(&mut state);
        if *state != before {
            self.changes.send_replace(*state);
        }
    }

    /// Invalidate old transport callbacks. Retain a confirmed loss until login
    /// replaces it, including when local cleanup fails and needs retry.
    pub fn replace_network(&self, network_generation: u64) {
        self.update(|state| {
            if network_generation <= state.network_generation {
                return;
            }
            state.network_generation = network_generation;
            if state.phase != AuthorizationPhase::Revoked {
                state.generation = state.generation.wrapping_add(1);
                state.account_id = None;
                state.phase = AuthorizationPhase::SignedOut;
            }
        });
    }

    pub fn set_account(&self, network_generation: u64, account_id: Option<i64>) {
        self.update(|state| {
            if network_generation != state.network_generation
                || state.phase == AuthorizationPhase::Revoked
            {
                return;
            }
            if state.account_id == account_id
                && matches!(
                    (account_id, state.phase),
                    (Some(_), AuthorizationPhase::Authorized)
                        | (None, AuthorizationPhase::SignedOut)
                )
            {
                return;
            }
            state.generation = state.generation.wrapping_add(1);
            state.account_id = account_id;
            state.phase = if account_id.is_some() {
                AuthorizationPhase::Authorized
            } else {
                AuthorizationPhase::SignedOut
            };
        });
    }

    /// Called only after the retired transport has stopped and its invalid
    /// session has been cleared. Until then even late auth replies stay fenced.
    pub fn complete_retirement(&self, expected: AuthorizationSnapshot, network_generation: u64) {
        self.update(|state| {
            if state.phase == AuthorizationPhase::Revoked
                && state.generation == expected.generation
                && state.account_id == expected.account_id
                && state.network_generation == network_generation
                && network_generation > expected.network_generation
            {
                state.generation = state.generation.wrapping_add(1);
                state.account_id = None;
                state.phase = AuthorizationPhase::SignedOut;
            }
        });
    }

    pub(crate) fn request_check(&self, network_generation: u64) {
        self.update(|state| {
            if state.network_generation == network_generation
                && state.phase == AuthorizationPhase::Authorized
            {
                state.phase = AuthorizationPhase::Checking;
            }
        });
    }

    pub(crate) fn finish_check(&self, expected: AuthorizationSnapshot, lost: bool) {
        self.update(|state| {
            if *state == expected && state.phase == AuthorizationPhase::Checking {
                state.phase = if lost {
                    AuthorizationPhase::Revoked
                } else {
                    AuthorizationPhase::Authorized
                };
            }
        });
    }

    pub(crate) fn observe_error(&self, network_generation: u64, error: &InvocationError) {
        if confirms_session_loss(error) {
            // A file DC's missing authorization is not proof that the home
            // session was revoked. Confirm via the normal home connection.
            self.request_check(network_generation);
        }
    }
}

pub struct AuthorizationUpdates {
    receiver: watch::Receiver<AuthorizationSnapshot>,
}
impl AuthorizationUpdates {
    pub async fn changed(&mut self) -> Option<AuthorizationSnapshot> {
        self.receiver.changed().await.ok()?;
        Some(*self.receiver.borrow_and_update())
    }
}

pub(crate) struct ObservedRetryPolicy {
    pub monitor: AuthorizationMonitor,
    pub network_generation: u64,
    pub inner: Box<dyn RetryPolicy>,
}
impl RetryPolicy for ObservedRetryPolicy {
    fn should_retry(&self, context: &RetryContext) -> ControlFlow<(), Duration> {
        self.monitor
            .observe_error(self.network_generation, &context.error);
        self.inner.should_retry(context)
    }
}

pub(crate) fn confirms_session_loss(error: &InvocationError) -> bool {
    matches!(error, InvocationError::Rpc(rpc) if rpc.code == 401 || rpc.name == "AUTH_KEY_DUPLICATED")
        || matches!(
            error,
            InvocationError::Transport(grammers_mtproto::transport::Error::BadStatus {
                status: 404
            })
        )
}

pub(crate) async fn confirm_events<F, Fut>(
    monitor: AuthorizationMonitor,
    network_generation: u64,
    mut check: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), InvocationError>>,
{
    let mut updates = monitor.subscribe();
    // Include a baseline so an event arriving before owner startup is retained.
    let mut snapshot = monitor.snapshot();
    loop {
        if snapshot.network_generation != network_generation {
            return;
        }
        if snapshot.phase == AuthorizationPhase::Checking {
            let result = tokio::time::timeout(Duration::from_secs(10), check()).await;
            let lost = matches!(result, Ok(Err(ref error)) if confirms_session_loss(error));
            monitor.finish_check(snapshot, lost);
        }
        let Some(next) = updates.changed().await else {
            return;
        };
        snapshot = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_mtsender::RpcError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn rpc(code: i32, name: &str) -> InvocationError {
        InvocationError::Rpc(RpcError {
            code,
            name: name.into(),
            value: None,
            caused_by: None,
        })
    }

    #[test]
    fn only_confirmed_home_authorization_errors_mean_session_loss() {
        for error in [
            rpc(401, "SESSION_REVOKED"),
            rpc(401, "AUTH_KEY_UNREGISTERED"),
            rpc(406, "AUTH_KEY_DUPLICATED"),
            InvocationError::Transport(grammers_mtproto::transport::Error::BadStatus {
                status: 404,
            }),
        ] {
            assert!(confirms_session_loss(&error));
        }
        for error in [
            rpc(403, "CHANNEL_PRIVATE"),
            rpc(420, "FLOOD_WAIT"),
            rpc(500, "INTERNAL"),
            InvocationError::Dropped,
            InvocationError::Io(std::io::ErrorKind::ConnectionReset.into()),
            InvocationError::Transport(grammers_mtproto::transport::Error::BadStatus {
                status: 429,
            }),
        ] {
            assert!(!confirms_session_loss(&error));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn silence_never_polls_and_duplicate_events_coalesce_while_confirmation_is_blocked() {
        let monitor = AuthorizationMonitor::default();
        monitor.set_account(0, Some(17));
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = calls.clone();
        let release = Arc::new(tokio::sync::Notify::new());
        let gate = release.clone();
        let task = tokio::spawn(confirm_events(monitor.clone(), 0, move || {
            entered.fetch_add(1, Ordering::SeqCst);
            let gate = gate.clone();
            async move {
                gate.notified().await;
                Err(rpc(401, "SESSION_REVOKED"))
            }
        }));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3600)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        monitor.request_check(0);
        tokio::task::yield_now().await;
        for _ in 0..1000 {
            monitor.request_check(0);
        }
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Checking);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let unrelated = tokio::spawn(async { 42 });
        assert_eq!(unrelated.await.expect("unrelated task completes"), 42);
        release.notify_one();
        tokio::task::yield_now().await;
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Revoked);
        tokio::time::advance(Duration::from_secs(3600)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        monitor.request_check(0);
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Revoked);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn failed_or_timed_out_confirmation_never_logs_out_or_schedules_another_check() {
        for blocked in [false, true] {
            let monitor = AuthorizationMonitor::default();
            monitor.set_account(0, Some(17));
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let task = tokio::spawn(confirm_events(monitor.clone(), 0, move || {
                count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if blocked {
                        std::future::pending::<()>().await;
                    }
                    Err(rpc(500, "INTERNAL"))
                }
            }));
            monitor.request_check(0);
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_secs(11)).await;
            tokio::task::yield_now().await;
            assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Authorized);
            tokio::time::advance(Duration::from_secs(3600)).await;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            task.abort();
        }
    }

    #[test]
    fn stale_connection_or_account_confirmation_cannot_revoke_the_replacement() {
        let monitor = AuthorizationMonitor::default();
        monitor.request_check(0);
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::SignedOut);
        monitor.set_account(0, Some(17));
        monitor.request_check(0);
        let old = monitor.snapshot();
        monitor.replace_network(1);
        monitor.set_account(1, Some(18));
        monitor.finish_check(old, true);
        monitor.observe_error(0, &rpc(401, "SESSION_REVOKED"));
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Authorized);
        assert_eq!(monitor.snapshot().account_id, Some(18));
        monitor.request_check(1);
        let check = monitor.snapshot();
        monitor.finish_check(check, true);
        let revoked = monitor.snapshot();
        monitor.replace_network(2);
        assert_eq!(monitor.snapshot().generation, revoked.generation);
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Revoked);
        monitor.set_account(1, Some(17));
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Revoked);
        monitor.set_account(2, Some(17));
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Revoked);
        monitor.complete_retirement(old, 2);
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Revoked);
        monitor.complete_retirement(revoked, 2);
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::SignedOut);
        monitor.set_account(2, Some(18));
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Authorized);
    }

    #[tokio::test(start_paused = true)]
    async fn file_dc_error_and_a_healthy_home_confirmation_keep_the_account_authorized() {
        let monitor = AuthorizationMonitor::default();
        monitor.set_account(0, Some(17));
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let owner = tokio::spawn(confirm_events(monitor.clone(), 0, move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        }));
        monitor.observe_error(0, &rpc(401, "AUTH_KEY_UNREGISTERED"));
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Checking);
        tokio::task::yield_now().await;
        assert_eq!(monitor.snapshot().phase, AuthorizationPhase::Authorized);
        assert_eq!(monitor.snapshot().account_id, Some(17));
        tokio::time::advance(Duration::from_secs(3600)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        owner.abort();
    }
}
