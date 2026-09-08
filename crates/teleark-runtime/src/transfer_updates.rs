//! Frontend-neutral, coalesced transfer updates and reusable immutable views.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use teleark_core::{ApplicationError, ApplicationErrorKind};
use tokio::sync::watch;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(100);

pub struct TransferSubscription {
    receiver: watch::Receiver<u64>,
}

impl TransferSubscription {
    pub async fn changed(&mut self) -> bool {
        self.receiver.changed().await.is_ok()
    }
}

#[derive(Clone, Debug)]
pub struct TransferSnapshotView<T> {
    pub revision: u64,
    pub items: Arc<[Arc<T>]>,
    /// Older terminal records retained outside this bounded presentation view.
    pub omitted_items: u64,
}

impl<T> Default for TransferSnapshotView<T> {
    fn default() -> Self {
        Self {
            revision: 0,
            items: Arc::from([]),
            omitted_items: 0,
        }
    }
}

pub(crate) trait TransferRecord: Clone {
    type Phase: PartialEq;
    fn id(&self) -> u64;
    fn phase(&self) -> Self::Phase;
}

struct PublishState {
    revision: u64,
    published: u64,
    urgent: bool,
    last_publish: Instant,
    closed: bool,
}

struct UpdateSignal {
    state: Arc<Mutex<PublishState>>,
    sender: watch::Sender<u64>,
    worker: JoinHandle<()>,
}

impl UpdateSignal {
    fn new() -> Result<Self, ApplicationError> {
        let (sender, _) = watch::channel(0);
        let state = Arc::new(Mutex::new(PublishState {
            revision: 0,
            published: 0,
            urgent: false,
            last_publish: Instant::now(),
            closed: false,
        }));
        let worker_state = state.clone();
        let worker_sender = sender.clone();
        // No runtime timer context is required by subscribers (including GPUI).
        let worker = thread::Builder::new()
            .name("teleark-transfer-updates".into())
            .spawn(move || {
                loop {
                    let (revision, deadline) = {
                        let Ok(mut state) = worker_state.lock() else {
                            return;
                        };
                        if state.closed {
                            return;
                        }
                        let deadline = state.last_publish + SAMPLE_INTERVAL;
                        if state.revision != state.published
                            && (state.urgent || Instant::now() >= deadline)
                        {
                            state.published = state.revision;
                            state.urgent = false;
                            state.last_publish = Instant::now();
                            (Some(state.published), None)
                        } else {
                            (
                                None,
                                (state.revision != state.published).then_some(deadline),
                            )
                        }
                    };
                    if let Some(revision) = revision {
                        worker_sender.send_replace(revision);
                    } else if let Some(deadline) = deadline {
                        thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
                    } else {
                        thread::park();
                    }
                }
            })
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        Ok(Self {
            state,
            sender,
            worker,
        })
    }

    fn changed(&self, urgent: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.revision = state.revision.wrapping_add(1);
            state.urgent |= urgent;
        }
        self.worker.thread().unpark();
    }

    fn subscribe(&self) -> TransferSubscription {
        TransferSubscription {
            receiver: self.sender.subscribe(),
        }
    }
}

impl Drop for UpdateSignal {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
        self.worker.thread().unpark();
        // The worker owns only the bounded signal state; no disk/network work or
        // synchronous join is performed on the thread dropping the last owner.
    }
}

struct Entry<T> {
    value: T,
    cached: Option<Arc<T>>,
}

struct Records<T> {
    entries: BTreeMap<u64, Entry<T>>,
    revision: u64,
    view: TransferSnapshotView<T>,
}

pub(crate) struct TransferSnapshots<T: TransferRecord> {
    records: Mutex<Records<T>>,
    signal: UpdateSignal,
}

impl<T: TransferRecord> TransferSnapshots<T> {
    pub(crate) fn new(values: Vec<T>) -> Result<Self, ApplicationError> {
        Ok(Self {
            records: Mutex::new(Records {
                entries: values
                    .into_iter()
                    .map(|value| {
                        (
                            value.id(),
                            Entry {
                                value,
                                cached: None,
                            },
                        )
                    })
                    .collect(),
                revision: 1,
                view: TransferSnapshotView::default(),
            }),
            signal: UpdateSignal::new()?,
        })
    }

    pub(crate) fn subscribe(&self) -> TransferSubscription {
        self.signal.subscribe()
    }

    pub(crate) fn view(&self) -> Option<TransferSnapshotView<T>> {
        let mut records = self.records.lock().ok()?;
        if records.view.revision != records.revision {
            let items = records
                .entries
                .values_mut()
                .map(|entry| {
                    entry
                        .cached
                        .get_or_insert_with(|| Arc::new(entry.value.clone()))
                        .clone()
                })
                .collect();
            records.view = TransferSnapshotView {
                revision: records.revision,
                items,
                omitted_items: 0,
            };
        }
        Some(records.view.clone())
    }

    pub(crate) fn all(&self) -> Option<Vec<T>> {
        self.records.lock().ok().map(|records| {
            records
                .entries
                .values()
                .map(|entry| entry.value.clone())
                .collect()
        })
    }

    pub(crate) fn get(&self, id: u64) -> Option<T> {
        self.read(id, Clone::clone)
    }

    pub(crate) fn read<R>(&self, id: u64, read: impl FnOnce(&T) -> R) -> Option<R> {
        self.records
            .lock()
            .ok()?
            .entries
            .get(&id)
            .map(|entry| read(&entry.value))
    }

    pub(crate) fn select(
        &self,
        mut predicate: impl FnMut(&T) -> bool,
        limit: usize,
    ) -> Option<Vec<T>> {
        self.records.lock().ok().map(|records| {
            records
                .entries
                .values()
                .filter(|entry| predicate(&entry.value))
                .take(limit)
                .map(|entry| entry.value.clone())
                .collect()
        })
    }

    pub(crate) fn fold<R>(&self, initial: R, mut read: impl FnMut(R, &T) -> R) -> Option<R> {
        let records = self.records.lock().ok()?;
        Some(
            records
                .entries
                .values()
                .fold(initial, |value, entry| read(value, &entry.value)),
        )
    }

    pub(crate) fn insert(&self, value: T) -> bool {
        self.extend([value])
    }

    pub(crate) fn insert_pruning(
        &self,
        value: T,
        prune: impl FnOnce(&[&T]) -> Option<Vec<u64>>,
    ) -> bool {
        let Ok(mut records) = self.records.lock() else {
            return false;
        };
        let removed = if records.entries.contains_key(&value.id()) {
            Vec::new()
        } else {
            let Some(removed) = prune(
                &records
                    .entries
                    .values()
                    .map(|entry| &entry.value)
                    .collect::<Vec<_>>(),
            ) else {
                return false;
            };
            removed
        };
        for id in removed {
            records.entries.remove(&id);
        }
        records.entries.insert(
            value.id(),
            Entry {
                value,
                cached: None,
            },
        );
        records.revision = records.revision.wrapping_add(1);
        drop(records);
        self.signal.changed(true);
        true
    }

    pub(crate) fn extend(&self, values: impl IntoIterator<Item = T>) -> bool {
        let Ok(mut records) = self.records.lock() else {
            return false;
        };
        for value in values {
            records.entries.insert(
                value.id(),
                Entry {
                    value,
                    cached: None,
                },
            );
        }
        records.revision = records.revision.wrapping_add(1);
        drop(records);
        self.signal.changed(true);
        true
    }

    pub(crate) fn remove(&self, id: u64) -> bool {
        self.remove_if(id, |_| true)
    }

    pub(crate) fn prune(&self, choose: impl FnOnce(&[&T]) -> Vec<u64>) -> Option<Vec<u64>> {
        let mut records = self.records.lock().ok()?;
        let chosen = choose(
            &records
                .entries
                .values()
                .map(|entry| &entry.value)
                .collect::<Vec<_>>(),
        );
        let removed: Vec<_> = chosen
            .into_iter()
            .filter(|id| records.entries.remove(id).is_some())
            .collect();
        if !removed.is_empty() {
            records.revision = records.revision.wrapping_add(1);
        }
        drop(records);
        if !removed.is_empty() {
            self.signal.changed(true);
        }
        Some(removed)
    }

    pub(crate) fn remove_if(&self, id: u64, predicate: impl FnOnce(&T) -> bool) -> bool {
        let Ok(mut records) = self.records.lock() else {
            return false;
        };
        if !records
            .entries
            .get(&id)
            .is_some_and(|entry| predicate(&entry.value))
        {
            return false;
        }
        let removed = records.entries.remove(&id).is_some();
        if removed {
            records.revision = records.revision.wrapping_add(1);
        }
        drop(records);
        if removed {
            self.signal.changed(true);
        }
        removed
    }

    pub(crate) fn update<R>(&self, id: u64, update: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut records = self.records.lock().ok()?;
        let entry = records.entries.get_mut(&id)?;
        let phase = entry.value.phase();
        let result = update(&mut entry.value);
        let urgent = phase != entry.value.phase();
        entry.cached = None;
        records.revision = records.revision.wrapping_add(1);
        drop(records);
        self.signal.changed(urgent);
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Row {
        id: u64,
        phase: u8,
        bytes: u64,
        clones: Arc<AtomicUsize>,
    }
    impl Clone for Row {
        fn clone(&self) -> Self {
            self.clones.fetch_add(1, Ordering::Relaxed);
            Self {
                id: self.id,
                phase: self.phase,
                bytes: self.bytes,
                clones: self.clones.clone(),
            }
        }
    }
    impl TransferRecord for Row {
        type Phase = u8;
        fn id(&self) -> u64 {
            self.id
        }
        fn phase(&self) -> u8 {
            self.phase
        }
    }

    #[test]
    fn immutable_views_clone_only_changed_records_and_reuse_idle_views() {
        let clones = Arc::new(AtomicUsize::new(0));
        let records = TransferSnapshots::new(
            (0..10_000)
                .map(|id| Row {
                    id,
                    phase: 0,
                    bytes: 0,
                    clones: clones.clone(),
                })
                .collect(),
        )
        .expect("valid fixture or timely publication");
        let first = records.view().expect("valid fixture or timely publication");
        assert_eq!(clones.swap(0, Ordering::Relaxed), 10_000);
        for _ in 0..1_000 {
            assert!(Arc::ptr_eq(
                &first.items,
                &records
                    .view()
                    .expect("valid fixture or timely publication")
                    .items
            ));
        }
        assert_eq!(clones.load(Ordering::Relaxed), 0);
        records.update(5_000, |row| row.bytes = 10);
        let next = records.view().expect("valid fixture or timely publication");
        assert_eq!(clones.load(Ordering::Relaxed), 1);
        assert!(Arc::ptr_eq(&first.items[0], &next.items[0]));
        assert_eq!(first.items[5_000].bytes, 0);
        assert_eq!(next.items[5_000].bytes, 10);
        records.remove(5_000);
        assert_eq!(
            records
                .view()
                .expect("valid fixture or timely publication")
                .items
                .len(),
            9_999
        );
    }

    #[test]
    fn signal_sleeps_when_idle_coalesces_samples_and_delivers_terminal_changes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("valid fixture or timely publication");
        let signal = UpdateSignal::new().expect("valid fixture or timely publication");
        let mut subscription = signal.subscribe();
        runtime.block_on(async {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), subscription.changed())
                    .await
                    .is_err()
            );
            for _ in 0..10_000 {
                signal.changed(false);
            }
            assert!(
                tokio::time::timeout(Duration::from_secs(2), subscription.changed())
                    .await
                    .expect("valid fixture or timely publication")
            );
            assert_eq!(*subscription.receiver.borrow_and_update(), 10_000);
            signal.changed(false);
            signal.changed(true);
            assert!(
                tokio::time::timeout(Duration::from_secs(2), subscription.changed())
                    .await
                    .expect("valid fixture or timely publication")
            );
            assert_eq!(*subscription.receiver.borrow_and_update(), 10_002);
            drop(signal);
            assert!(
                !tokio::time::timeout(Duration::from_secs(2), subscription.changed())
                    .await
                    .expect("valid fixture or timely publication")
            );
        });
    }
}
