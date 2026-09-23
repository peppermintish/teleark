//! A coalesced wake for Telegram's QR login update, independent of RPC work.
use tokio::sync::watch;

#[derive(Clone)]
pub struct QrLoginSignal(watch::Sender<bool>);

impl Default for QrLoginSignal {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}

impl QrLoginSignal {
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }

    pub(crate) fn announce(&self) {
        self.0.send_replace(true);
    }

    pub(crate) fn take(&self) -> bool {
        self.0.send_replace(false)
    }

    pub(crate) fn clear(&self) {
        self.0.send_if_modified(|pending| {
            if !*pending {
                return false;
            }
            *pending = false;
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn update_wakes_a_waiting_subscriber_and_is_consumed_once() {
        let signal = QrLoginSignal::default();
        let mut updates = signal.subscribe();
        assert!(!*updates.borrow());
        signal.announce();
        updates.changed().await.expect("QR update");
        assert!(*updates.borrow_and_update());
        assert!(signal.take());
        assert!(!signal.take());
        assert!(!*updates.borrow());
    }
}
