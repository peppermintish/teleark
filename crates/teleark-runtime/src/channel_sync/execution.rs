//! Retained bounded background calls. The coordinator never waits for a remote
//! page; metadata owns a separate slot so discovery cannot starve channel work.
use super::*;

pub(super) enum Outcome {
    Failed(ApplicationErrorKind),
    Sources(Result<Vec<TelegramChatSummary>, ChannelSyncFailure>),
    Channel(Box<Job>, Result<(), ChannelSyncFailure>),
}

pub(super) struct Completion {
    pub id: i64,
    pub revision: u64,
    pub outcome: Outcome,
}

pub(super) struct Executions {
    managed_id: i64,
    joins: BTreeMap<i64, JoinHandle<()>>,
    sender: mpsc::SyncSender<Completion>,
    receiver: mpsc::Receiver<Completion>,
}

impl Executions {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(6);
        Self {
            managed_id: 0,
            joins: BTreeMap::new(),
            sender,
            receiver,
        }
    }

    pub fn channel_slot_available(&self) -> bool {
        self.joins
            .keys()
            .filter(|id| **id != 0 && **id != self.managed_id)
            .count()
            < 4
    }

    pub fn reserve_managed(&mut self, id: i64) {
        self.managed_id = id;
    }

    pub fn available(&self, id: i64) -> bool {
        !self.joins.contains_key(&id)
            && (id == 0
                || (self.joins.keys().filter(|id| **id != 0).count() < 5
                    && (id == self.managed_id || self.channel_slot_available())))
    }

    pub fn any(&self) -> bool {
        !self.joins.is_empty()
    }

    pub fn spawn(
        &mut self,
        id: i64,
        revision: u64,
        work: impl FnOnce() -> Outcome + Send + 'static,
    ) -> Result<(), ApplicationError> {
        if !self.available(id) {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        let sender = self.sender.clone();
        let coordinator = thread::current();
        let join = thread::Builder::new()
            .name("teleark-sync-page".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                    .unwrap_or(Outcome::Failed(ApplicationErrorKind::Conflict));
                let _ = sender.send(Completion {
                    id,
                    revision,
                    outcome,
                });
                coordinator.unpark();
            })
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        self.joins.insert(id, join);
        Ok(())
    }

    pub fn take(&mut self) -> Option<Completion> {
        let completion = self.receiver.try_recv().ok()?;
        if let Some(join) = self.joins.remove(&completion.id) {
            let _ = join.join();
        }
        Some(completion)
    }
}

impl Drop for Executions {
    fn drop(&mut self) {
        // Runs on the retained coordinator/reaper, never on UI/network reactors.
        for (_, join) in std::mem::take(&mut self.joins) {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_channel_keeps_a_reserved_lane_when_all_normal_lanes_block() {
        let mut work = Executions::new();
        work.reserve_managed(99);
        let mut releases = Vec::new();
        for id in [0, 2, 3, 4, 5] {
            let (release, blocked) = mpsc::channel();
            releases.push(release);
            work.spawn(id, 1, move || {
                blocked.recv().expect("release normal lane");
                Outcome::Sources(Ok(vec![]))
            })
            .expect("normal lane");
        }
        assert!(!work.available(6));
        assert!(work.available(99));
        work.spawn(99, 1, || Outcome::Sources(Ok(vec![])))
            .expect("reserved managed lane");
        work.reserve_managed(100);
        assert!(
            !work.available(100),
            "replacing a managed binding cannot exceed five retained channel calls"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let completed = loop {
            if let Some(completed) = work.take() {
                break completed;
            }
            assert!(
                Instant::now() < deadline,
                "managed inventory must not wait for normal lanes"
            );
            thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
        };
        assert_eq!(completed.id, 99);
        assert!(
            work.available(100),
            "replacement uses the retired managed slot"
        );
        for release in releases {
            release.send(()).expect("release");
        }
    }
    #[test]
    fn blocked_channel_and_discovery_do_not_stop_another_channel() {
        let mut work = Executions::new();
        let (release, blocked) = mpsc::channel();
        work.spawn(2, 1, move || {
            blocked.recv().expect("release blocked channel");
            Outcome::Sources(Ok(vec![]))
        })
        .expect("first");
        let (release_metadata, metadata) = mpsc::channel();
        work.spawn(0, 1, move || {
            metadata.recv().expect("release metadata");
            Outcome::Sources(Ok(vec![]))
        })
        .expect("metadata");
        work.spawn(3, 1, || Outcome::Sources(Ok(vec![])))
            .expect("independent");
        let deadline = Instant::now() + Duration::from_secs(5);
        let completed = loop {
            if let Some(completed) = work.take() {
                break completed;
            }
            assert!(
                Instant::now() < deadline,
                "independent work must complete while other calls block"
            );
            thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
        };
        assert_eq!(completed.id, 3);
        assert!(!work.available(2));
        release.send(()).expect("release");
        release_metadata.send(()).expect("release metadata");
    }
}
