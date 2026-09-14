//! Frontend-neutral, coalesced transfer updates and reusable immutable views.
use crate::transfer_rate::{RateInput, RateWindow, TransferRate};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use teleark_core::{ApplicationError, ApplicationErrorKind};
use tokio::sync::watch;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

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
    /// Changed records over the stable identity/phase view; bounded to changed rows.
    pub updates: Arc<BTreeMap<u64, Arc<T>>>,
    pub rates: Arc<BTreeMap<u64, TransferRate>>,
    pub account_rates: Arc<BTreeMap<i64, crate::TransferRates>>,
}

impl<T> Default for TransferSnapshotView<T> {
    fn default() -> Self {
        Self {
            revision: 0,
            items: Arc::from([]),
            omitted_items: 0,
            updates: Arc::new(BTreeMap::new()),
            rates: Arc::new(BTreeMap::new()),
            account_rates: Arc::new(BTreeMap::new()),
        }
    }
}

pub(crate) trait TransferRecord: Clone + Send + Sync + 'static {
    type Phase: PartialEq;
    fn id(&self) -> u64;
    fn phase(&self) -> Self::Phase;
    fn rate_input(&self) -> RateInput {
        RateInput::default()
    }
    fn sample_activity(&mut self, _now: Instant, _rate: TransferRate) -> bool {
        false
    }
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
    fn new(
        mut tick: impl FnMut(Instant) -> (bool, bool) + Send + 'static,
    ) -> Result<Self, ApplicationError> {
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
                let mut next_tick = Instant::now() + SAMPLE_INTERVAL;
                let mut active = false;
                loop {
                    if Instant::now() >= next_tick {
                        let (changed, running) = tick(Instant::now());
                        active = running;
                        next_tick = Instant::now() + SAMPLE_INTERVAL;
                        if changed && let Ok(mut state) = worker_state.lock() {
                            state.revision = state.revision.wrapping_add(1);
                        }
                    }
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
                        let (_, running) = tick(Instant::now());
                        active = running;
                        worker_sender.send_replace(revision);
                    } else if let Some(deadline) = deadline {
                        thread::park_timeout(
                            deadline
                                .min(next_tick)
                                .saturating_duration_since(Instant::now()),
                        );
                    } else if active {
                        thread::park_timeout(next_tick.saturating_duration_since(Instant::now()));
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
        let wake = if let Ok(mut state) = self.state.lock() {
            let wake = urgent || state.revision == state.published;
            state.revision = state.revision.wrapping_add(1);
            state.urgent |= urgent;
            wake
        } else {
            false
        };
        if wake {
            self.worker.thread().unpark();
        }
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
    rate: RateWindow,
}
impl<T: TransferRecord> Entry<T> {
    fn new(value: T) -> Self {
        Self {
            rate: RateWindow::new(value.rate_input(), Instant::now()),
            value,
            cached: None,
        }
    }
}

struct Records<T> {
    entries: BTreeMap<u64, Entry<T>>,
    revision: u64,
    view: TransferSnapshotView<T>,
    omitted: u64,
    structure_changed: bool,
    dirty: BTreeSet<u64>,
    updates: BTreeMap<u64, Arc<T>>,
    active: BTreeSet<u64>,
    rates: BTreeMap<u64, TransferRate>,
    account_rates: BTreeMap<i64, crate::TransferRates>,
}

impl<T: TransferRecord> Records<T> {
    fn set_rate(&mut self, id: u64, input: RateInput, previous: TransferRate, next: TransferRate) {
        let was_sampling = self
            .rates
            .get(&id)
            .is_some_and(|rate| rate.bytes_per_second.is_none());
        let total = self.account_rates.entry(input.account).or_default();
        let sampling = if input.upload {
            &mut total.uploads_sampling
        } else {
            &mut total.downloads_sampling
        };
        *sampling = sampling
            .saturating_sub(u32::from(was_sampling))
            .saturating_add(u32::from(input.active && next.bytes_per_second.is_none()));
        let value = if input.upload {
            &mut total.upload_bytes_per_second
        } else {
            &mut total.download_bytes_per_second
        };
        *value = value
            .saturating_sub(previous.bytes_per_second.unwrap_or(0))
            .saturating_add(next.bytes_per_second.unwrap_or(0));
        if input.active {
            self.active.insert(id);
            self.rates.insert(id, next);
        } else {
            self.active.remove(&id);
            self.rates.remove(&id);
        }
    }
    fn rebuild_rates(&mut self) {
        self.structure_changed = true;
        self.active.clear();
        self.rates.clear();
        self.account_rates.clear();
        for (&id, entry) in &self.entries {
            let input = entry.value.rate_input();
            if input.active {
                self.active.insert(id);
                self.rates.insert(id, entry.rate.published);
                let total = self.account_rates.entry(input.account).or_default();
                if entry.rate.published.bytes_per_second.is_none() {
                    if input.upload {
                        total.uploads_sampling += 1;
                    } else {
                        total.downloads_sampling += 1;
                    }
                }
                let value = if input.upload {
                    &mut total.upload_bytes_per_second
                } else {
                    &mut total.download_bytes_per_second
                };
                *value = value.saturating_add(entry.rate.published.bytes_per_second.unwrap_or(0));
            }
        }
    }
}

pub(crate) struct TransferSnapshots<T: TransferRecord> {
    records: Arc<Mutex<Records<T>>>,
    signal: UpdateSignal,
}

impl<T: TransferRecord> TransferSnapshots<T> {
    pub(crate) fn new(values: Vec<T>) -> Result<Self, ApplicationError> {
        let entries: BTreeMap<_, _> = values
            .into_iter()
            .map(|value| (value.id(), Entry::new(value)))
            .collect();
        let records = Arc::new(Mutex::new(Records {
            active: entries
                .iter()
                .filter_map(|(&id, entry)| entry.value.rate_input().active.then_some(id))
                .collect(),
            entries,
            omitted: 0,
            structure_changed: true,
            dirty: BTreeSet::new(),
            updates: BTreeMap::new(),
            revision: 1,
            view: TransferSnapshotView::default(),
            rates: BTreeMap::new(),
            account_rates: BTreeMap::new(),
        }));
        if let Ok(mut state) = records.lock() {
            state.rebuild_rates();
        }
        let weak = Arc::downgrade(&records);
        let signal = UpdateSignal::new(move |now| {
            let Some(records) = weak.upgrade() else {
                return (false, false);
            };
            let Ok(mut records) = records.lock() else {
                return (false, false);
            };
            let active: Vec<_> = records.active.iter().copied().collect();
            let mut changed = false;
            for id in active {
                let Some(entry) = records.entries.get_mut(&id) else {
                    continue;
                };
                let previous = entry.rate.published;
                let rate_changed = entry.rate.publish(now);
                let activity_changed = entry.value.sample_activity(now, entry.rate.published);
                changed |= rate_changed || activity_changed;
                if activity_changed {
                    entry.cached = None;
                }
                let input = entry.rate.input();
                let rate = entry.rate.published;
                records.dirty.insert(id);
                records.set_rate(id, input, previous, rate);
            }
            if changed {
                records.revision = records.revision.wrapping_add(1);
            }
            (changed, !records.active.is_empty())
        })?;
        signal.changed(true);
        Ok(Self { records, signal })
    }

    pub(crate) fn subscribe(&self) -> TransferSubscription {
        self.signal.subscribe()
    }

    pub(crate) fn view(&self) -> Option<TransferSnapshotView<T>> {
        let mut records = self.records.lock().ok()?;
        if records.view.revision != records.revision {
            let items = if records.structure_changed {
                records.updates.clear();
                records.dirty.clear();
                records.structure_changed = false;
                records
                    .entries
                    .values_mut()
                    .map(|entry| {
                        entry
                            .cached
                            .get_or_insert_with(|| Arc::new(entry.value.clone()))
                            .clone()
                    })
                    .collect()
            } else {
                let dirty = std::mem::take(&mut records.dirty);
                for id in dirty {
                    if let Some(entry) = records.entries.get_mut(&id) {
                        let value = entry
                            .cached
                            .get_or_insert_with(|| Arc::new(entry.value.clone()))
                            .clone();
                        records.updates.insert(id, value);
                    }
                }
                records.view.items.clone()
            };
            records.view = TransferSnapshotView {
                revision: records.revision,
                items,
                omitted_items: records.omitted,
                updates: Arc::new(records.updates.clone()),
                rates: Arc::new(records.rates.clone()),
                account_rates: Arc::new(records.account_rates.clone()),
            };
        }
        Some(records.view.clone())
    }

    pub(crate) fn current_rates(&self) -> Option<crate::TransferRates> {
        let records = self.records.lock().ok()?;
        Some(records.account_rates.values().fold(
            crate::TransferRates::default(),
            |mut total, rate| {
                total.upload_bytes_per_second = total
                    .upload_bytes_per_second
                    .saturating_add(rate.upload_bytes_per_second);
                total.download_bytes_per_second = total
                    .download_bytes_per_second
                    .saturating_add(rate.download_bytes_per_second);
                total.uploads_sampling =
                    total.uploads_sampling.saturating_add(rate.uploads_sampling);
                total.downloads_sampling = total
                    .downloads_sampling
                    .saturating_add(rate.downloads_sampling);
                total
            },
        ))
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
        records.omitted = records.omitted.saturating_add(removed.len() as u64);
        for id in removed {
            records.entries.remove(&id);
        }
        records.entries.insert(value.id(), Entry::new(value));
        records.rebuild_rates();
        records.revision = records.revision.wrapping_add(1);
        drop(records);
        self.signal.changed(true);
        true
    }

    pub(crate) fn restore_history(
        &self,
        values: Vec<T>,
        omitted: u64,
        retain: impl Fn(&T) -> bool,
    ) {
        let Ok(mut records) = self.records.lock() else {
            return;
        };
        let mut previous = std::mem::take(&mut records.entries);
        for value in values {
            let entry = previous
                .remove(&value.id())
                .filter(|entry| retain(&entry.value))
                .unwrap_or_else(|| Entry::new(value));
            records.entries.insert(entry.value.id(), entry);
        }
        records.entries.extend(
            previous
                .into_iter()
                .filter(|(_, entry)| retain(&entry.value)),
        );
        records.omitted = omitted;
        records.rebuild_rates();
        records.revision = records.revision.wrapping_add(1);
        drop(records);
        self.signal.changed(true);
    }

    pub(crate) fn extend(&self, values: impl IntoIterator<Item = T>) -> bool {
        let Ok(mut records) = self.records.lock() else {
            return false;
        };
        for value in values {
            records.entries.insert(value.id(), Entry::new(value));
        }
        records.rebuild_rates();
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
            records.rebuild_rates();
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
            records.rebuild_rates();
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
        let previous = entry.rate.published;
        let previous_input = entry.rate.input();
        let result = update(&mut entry.value);
        let input = entry.value.rate_input();
        let first = entry.rate.observe(input, Instant::now());
        let phase_changed = phase != entry.value.phase();
        let urgent = phase_changed || first;
        if urgent {
            entry.rate.publish(Instant::now());
        }
        let rate = entry.rate.published;
        entry.cached = None;
        if input.account != previous_input.account || input.upload != previous_input.upload {
            records.set_rate(
                id,
                RateInput {
                    active: false,
                    ..previous_input
                },
                previous,
                TransferRate::default(),
            );
        }
        records.structure_changed |= phase_changed;
        records.dirty.insert(id);
        records.set_rate(
            id,
            input,
            if input.account != previous_input.account || input.upload != previous_input.upload {
                TransferRate::default()
            } else {
                previous
            },
            rate,
        );
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
        assert_eq!(next.updates.get(&5_000).expect("changed row").bytes, 10);
        assert!(
            Arc::ptr_eq(&first.items, &next.items),
            "byte samples never copy/scan the identity list"
        );
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
        let signal =
            UpdateSignal::new(|_| (false, false)).expect("valid fixture or timely publication");
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
