//! One generation-scoped viewport demand per account. A cancelled/old page cannot
//! complete a newer demand. Live pushes may preempt history without pausing sync.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryStatus {
    Pending,
    Complete,
    Cancelled,
    Failed(ApplicationErrorKind),
}

struct Request {
    chat: i64,
    id: u64,
    status: HistoryStatus,
    token: Option<TelegramScanCancellation>,
}

#[derive(Default)]
pub(super) struct Requests {
    next: u64,
    current: Option<Request>,
}

impl Requests {
    pub(super) fn begin(&mut self, chat: i64) -> u64 {
        if let Some(old) = &self.current
            && let Some(token) = &old.token
        {
            token.cancel();
        }
        self.next = self.next.wrapping_add(1);
        self.current = Some(Request {
            chat,
            id: self.next,
            status: HistoryStatus::Pending,
            token: None,
        });
        self.next
    }
    pub fn pending(&self, chat: i64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|r| r.chat == chat && r.status == HistoryStatus::Pending)
    }
    pub fn attach(&mut self, chat: i64, token: TelegramScanCancellation) -> Option<u64> {
        let request = self
            .current
            .as_mut()
            .filter(|r| r.chat == chat && r.status == HistoryStatus::Pending)?;
        request.token = Some(token);
        Some(request.id)
    }
    pub fn finish(&mut self, chat: i64, id: u64, status: HistoryStatus) -> bool {
        if let Some(request) = self
            .current
            .as_mut()
            .filter(|r| r.chat == chat && r.id == id && r.status == HistoryStatus::Pending)
        {
            request.status = status;
            if let Some(token) = request.token.take()
                && status == HistoryStatus::Cancelled
            {
                token.cancel();
            }
            return true;
        }
        false
    }
    pub fn fail(&mut self, chat: i64, error: ApplicationErrorKind) {
        if let Some(request) = self
            .current
            .as_mut()
            .filter(|r| r.chat == chat && r.status == HistoryStatus::Pending)
        {
            request.status = HistoryStatus::Failed(error);
            request.token = None;
        }
    }
    pub fn yield_to_live(&self, chat: i64) {
        if let Some(request) = self
            .current
            .as_ref()
            .filter(|r| r.chat == chat && r.status == HistoryStatus::Pending)
            && let Some(token) = &request.token
        {
            token.cancel();
        }
    }
}

impl ChannelSync {
    pub fn request_history(&self, chat: i64) -> Result<u64, ApplicationError> {
        let id = self
            .inner
            .shared
            .history
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Conflict))?
            .begin(chat);
        if let Err(error) = self.command(Command::History(chat)) {
            if let Ok(mut history) = self.inner.shared.history.lock() {
                history.finish(chat, id, HistoryStatus::Failed(error.kind()));
            }
            return Err(error);
        }
        Ok(id)
    }
    pub fn cancel_history(&self, chat: i64, id: u64) {
        let cancelled = self
            .inner
            .shared
            .history
            .lock()
            .is_ok_and(|mut history| history.finish(chat, id, HistoryStatus::Cancelled));
        if !cancelled {
            return;
        }
        publish(
            &self.inner.shared,
            ChannelSyncPhase::Cancelled,
            Some(chat),
            None,
            None,
        );
        self.inner.worker.unpark();
        self.inner.shared.changes.send_replace(());
    }
    pub fn history_status(&self, chat: i64, id: u64) -> Option<HistoryStatus> {
        self.inner
            .shared
            .history
            .lock()
            .ok()?
            .current
            .as_ref()
            .filter(|r| r.chat == chat && r.id == id)
            .map(|r| r.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newer_view_cancels_old_page_and_rejects_stale_completion() {
        let mut requests = Requests::default();
        let old = requests.begin(2);
        let cancellation = TelegramScanCancellation::new();
        assert_eq!(requests.attach(2, cancellation.clone()), Some(old));
        let new = requests.begin(3);
        assert!(cancellation.is_cancelled());
        requests.finish(2, old, HistoryStatus::Complete);
        assert!(requests.pending(3));
        let token = TelegramScanCancellation::new();
        requests.attach(3, token.clone());
        requests.yield_to_live(3);
        assert!(token.is_cancelled());
        assert!(
            requests.pending(3),
            "a live push preserves the history demand"
        );
        requests.finish(3, new, HistoryStatus::Complete);
        assert!(!requests.pending(3));
        assert!(
            !requests.finish(3, new, HistoryStatus::Cancelled),
            "a completed demand keeps its terminal result"
        );
    }
}
