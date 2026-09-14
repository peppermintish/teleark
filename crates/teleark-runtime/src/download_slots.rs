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
        }
    }
    pub(crate) fn set_limit(&self, limit: u16) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).limit = limit.clamp(1, 8);
        self.changed.notify_all();
    }
    pub(crate) fn acquire(&self) -> DownloadSlot<'_> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.active >= state.limit {
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        state.active += 1;
        DownloadSlot(self)
    }
}
impl Drop for DownloadSlot<'_> {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active -= 1;
        self.0.changed.notify_all();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
}
