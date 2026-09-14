//! Account/connection readiness outlives any one network reactor. Subscribers receive
//! a coherent baseline and wake on replacement; no RPC or polling is required.
use super::*;
use teleark_telegram::ChannelUpdateSignals;

type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Default)]
pub(crate) struct Lifecycle(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    generation: u64,
    revision: u64,
    account: Option<i64>,
    signals: Option<ChannelUpdateSignals>,
    next_subscriber: u64,
    subscribers: BTreeMap<u64, Wake>,
}

pub(crate) struct Subscription {
    lifecycle: Lifecycle,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Ok(mut state) = self.lifecycle.0.lock() {
            state.subscribers.remove(&self.id);
        }
    }
}

impl Lifecycle {
    pub(crate) fn snapshot(&self) -> (u64, Option<i64>, Option<ChannelUpdateSignals>) {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (state.revision, state.account, state.signals.clone())
    }

    pub(crate) fn subscribe(&self, wake: Wake) -> Result<Subscription, ApplicationError> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.subscribers.len() >= 8 {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        state.next_subscriber = state.next_subscriber.wrapping_add(1);
        let id = state.next_subscriber;
        state.subscribers.insert(id, wake);
        Ok(Subscription {
            lifecycle: self.clone(),
            id,
        })
    }

    pub(crate) fn publish(
        &self,
        generation: u64,
        account: Option<i64>,
        signals: Option<ChannelUpdateSignals>,
    ) {
        let wakes = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if generation < state.generation {
                return;
            }
            state.generation = generation;
            state.revision = state.revision.wrapping_add(1);
            state.account = account;
            state.signals = signals;
            state.subscribers.values().cloned().collect::<Vec<_>>()
        };
        for wake in wakes {
            wake();
        }
    }
}

impl DesktopTelegram {
    pub(crate) fn lifecycle(&self) -> Lifecycle {
        self.inner.lifecycle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn late_reactor_cannot_restore_old_account_and_subscription_is_bounded_and_released() {
        let lifecycle = Lifecycle::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let readable = lifecycle.clone();
        let subscription = lifecycle
            .subscribe(Arc::new(move || {
                // Publication releases its lock before callbacks run.
                let _ = readable.snapshot();
                count.fetch_add(1, Ordering::Relaxed);
            }))
            .expect("subscribe");
        lifecycle.publish(1, Some(10), Some(ChannelUpdateSignals::default()));
        assert_eq!(lifecycle.snapshot().1, Some(10));
        lifecycle.publish(2, None, None);
        lifecycle.publish(1, Some(10), Some(ChannelUpdateSignals::default()));
        assert_eq!(lifecycle.snapshot().1, None);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        lifecycle.publish(2, Some(20), Some(ChannelUpdateSignals::default()));
        assert_eq!(lifecycle.snapshot().1, Some(20));
        drop(subscription);
        lifecycle.publish(3, None, None);
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        let subscriptions = (0..8)
            .map(|_| lifecycle.subscribe(Arc::new(|| {})).expect("bounded"))
            .collect::<Vec<_>>();
        assert!(lifecycle.subscribe(Arc::new(|| {})).is_err());
        drop(subscriptions);
        assert!(lifecycle.subscribe(Arc::new(|| {})).is_ok());
    }
}
