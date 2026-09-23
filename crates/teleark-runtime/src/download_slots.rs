//! One shared file limit for native and Vault downloads. Blocking owners only.
use std::sync::{Condvar, Mutex};
#[derive(Default)]
struct State {
    active: u16,
    limit: u16,
}
pub(crate) struct DownloadSlots {
    state: Mutex<State>,
    changed: Condvar,
    revisions: tokio::sync::watch::Sender<u64>,
}
pub(crate) struct DownloadSlot<'a>(&'a DownloadSlots);
impl DownloadSlots {
    pub(crate) fn new(limit: u16) -> Self {
        Self {
            state: Mutex::new(State {
                active: 0,
                limit: limit.clamp(1, 8),
            }),
            changed: Condvar::new(),
            revisions: tokio::sync::watch::channel(0).0,
        }
    }
    pub(crate) fn set_limit(&self, limit: u16) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).limit = limit.clamp(1, 8);
        self.wake();
    }
    /// Wake queued admissions when cancellation or shutdown changes eligibility.
    pub(crate) fn wake(&self) {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.changed.notify_all();
        self.revisions
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        drop(guard);
    }
    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.revisions.subscribe()
    }
    pub(crate) fn acquire(&self) -> DownloadSlot<'_> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.active >= state.limit {
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        state.active += 1;
        DownloadSlot(self)
    }
    pub(crate) fn acquire_while(&self, eligible: impl Fn() -> bool) -> Option<DownloadSlot<'_>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.active >= state.limit {
            if !eligible() {
                return None;
            }
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        if !eligible() {
            return None;
        }
        state.active += 1;
        Some(DownloadSlot(self))
    }
}
impl Drop for DownloadSlot<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active -= 1;
        drop(state);
        self.0.wake();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[test]
    fn raising_manual_limit_wakes_waiters_and_lowering_preserves_active_work() {
        let slots = std::sync::Arc::new(DownloadSlots::new(1));
        let first = slots.acquire();
        let (entered, receiver) = std::sync::mpsc::channel();
        let other = slots.clone();
        let handle = std::thread::spawn(move || {
            let _slot = other.acquire();
            entered.send(()).expect("observer");
        });
        assert!(receiver.try_recv().is_err());
        slots.set_limit(2);
        receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("new capacity");
        handle.join().expect("owner");
        slots.set_limit(1);
        assert_eq!(slots.state.lock().expect("state").active, 1);
        drop(first);
        let _next = slots.acquire();
    }

    #[test]
    fn cancellation_wakes_an_admission_waiting_for_a_slot() {
        let slots = std::sync::Arc::new(DownloadSlots::new(1));
        let held = slots.acquire();
        let eligible = std::sync::Arc::new(AtomicBool::new(true));
        let (done, result) = std::sync::mpsc::channel();
        let waiting_slots = slots.clone();
        let waiting_eligible = eligible.clone();
        let waiter = std::thread::spawn(move || {
            let acquired = waiting_slots
                .acquire_while(|| waiting_eligible.load(Ordering::Acquire))
                .is_some();
            done.send(acquired).expect("observer");
        });
        eligible.store(false, Ordering::Release);
        slots.wake();
        assert!(
            !result
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("cancellation wake")
        );
        drop(held);
        waiter.join().expect("waiter");
    }
}
