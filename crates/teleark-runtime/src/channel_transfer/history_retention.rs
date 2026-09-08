//! Admission and replay budgets span the entire native-transfer session.
use super::*;
use std::sync::atomic::{AtomicU64, AtomicUsize};

const DETAILED_IDLE_TASKS: usize = 16;
const COMPACT_PART_EVENTS: usize = 20;
const COMPACT_CONTROLLER_DECISIONS: usize = 32;
const LIFECYCLE_EVENTS: usize = 256;

pub(super) struct HistoryRetention {
    resident: AtomicUsize,
    omitted: AtomicU64,
    limit: usize,
}

pub(super) struct Reservation<'a> {
    owner: &'a HistoryRetention,
    reserved: usize,
}

impl Reservation<'_> {
    pub(super) fn commit(mut self) {
        self.reserved = 0;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.owner
            .resident
            .fetch_sub(self.reserved, Ordering::AcqRel);
    }
}

impl HistoryRetention {
    pub(super) fn new(resident: usize, omitted: u64, limit: usize) -> Self {
        Self {
            resident: AtomicUsize::new(resident),
            omitted: AtomicU64::new(omitted),
            limit,
        }
    }

    pub(super) fn omitted(&self) -> u64 {
        self.omitted.load(Ordering::Acquire)
    }

    pub(super) fn removed(&self) {
        self.resident.fetch_sub(1, Ordering::AcqRel);
    }

    pub(super) fn reserve<'a>(
        &'a self,
        count: usize,
        snapshots: &TransferSnapshots<ChannelDownloadSnapshot>,
        controls: &Mutex<BTreeMap<u64, Arc<AtomicU8>>>,
        scheduled: &Mutex<BTreeSet<u64>>,
    ) -> Result<Reservation<'a>, ApplicationError> {
        if count == 0 || count > self.limit {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        loop {
            let current = self.resident.load(Ordering::Acquire);
            if let Some(next) = current.checked_add(count)
                && next <= self.limit
            {
                if self
                    .resident
                    .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Ok(Reservation {
                        owner: self,
                        reserved: count,
                    });
                }
                continue;
            }
            let needed = current.saturating_add(count).saturating_sub(self.limit);
            let protected = scheduled
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .clone();
            let removed = snapshots
                .prune(|records| {
                    let mut groups =
                        BTreeMap::<(Option<i64>, Option<u64>, u64), (bool, Vec<u64>)>::new();
                    for snapshot in records {
                        let key = (
                            snapshot.account_id,
                            snapshot.batch_id,
                            snapshot.batch_id.map_or(snapshot.id, |_| 0),
                        );
                        let group = groups.entry(key).or_insert((true, Vec::new()));
                        group.0 &= evictable(snapshot) && !protected.contains(&snapshot.id);
                        group.1.push(snapshot.id);
                    }
                    let mut eligible: Vec<_> = groups
                        .into_values()
                        .filter_map(|(eligible, ids)| eligible.then_some(ids))
                        .collect();
                    if eligible.iter().map(Vec::len).sum::<usize>() < needed {
                        return Vec::new();
                    }
                    eligible.sort_unstable_by_key(|ids| ids[0]);
                    let mut chosen = Vec::new();
                    for group in eligible {
                        chosen.extend(group);
                        if chosen.len() >= needed {
                            break;
                        }
                    }
                    self.resident.fetch_sub(chosen.len(), Ordering::AcqRel);
                    self.omitted
                        .fetch_add(chosen.len() as u64, Ordering::AcqRel);
                    chosen
                })
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            for id in &removed {
                if let Ok(mut controls) = controls.lock() {
                    controls.remove(id);
                }
            }
            let removed = removed.len();
            if removed == 0 {
                return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
            }
        }
    }
}

fn evictable(snapshot: &ChannelDownloadSnapshot) -> bool {
    matches!(
        snapshot.state,
        ChannelDownloadState::Completed | ChannelDownloadState::Cancelled
    )
}

pub(super) fn bound_lifecycle(snapshot: &mut ChannelDownloadSnapshot) {
    let excess = snapshot.events.len().saturating_sub(LIFECYCLE_EVENTS);
    if excess != 0 {
        snapshot.events.drain(..excess);
        snapshot.event_history_omitted =
            snapshot.event_history_omitted.saturating_add(excess as u64);
    }
}

/// The worker calls this after releasing one backend. Running/scheduled work
/// retains its replay; older idle work keeps a disclosed compact tail.
pub(super) fn compact_idle_replays(
    snapshots: &TransferSnapshots<ChannelDownloadSnapshot>,
    scheduled: &Mutex<BTreeSet<u64>>,
) {
    let Ok(protected) = scheduled.lock().map(|ids| ids.clone()) else {
        return;
    };
    let Some(mut detailed) = snapshots.fold(Vec::new(), |mut candidates, snapshot| {
        if snapshot.state != ChannelDownloadState::Running
            && !protected.contains(&snapshot.id)
            && (snapshot.part_events.len() > COMPACT_PART_EVENTS
                || snapshot.telemetry.decisions.len() > COMPACT_CONTROLLER_DECISIONS)
        {
            let activity = snapshot
                .events
                .last()
                .map_or(snapshot.queued_at_unix_ms, |event| event.timestamp_unix_ms);
            candidates.push((activity, snapshot.id));
        }
        candidates
    }) else {
        return;
    };
    detailed.sort_unstable();
    let compact = detailed.len().saturating_sub(DETAILED_IDLE_TASKS);
    for (_, id) in detailed.into_iter().take(compact) {
        snapshots.update(id, |snapshot| {
            if snapshot.state == ChannelDownloadState::Running {
                return;
            }
            snapshot.part_events.retain_recent(COMPACT_PART_EVENTS);
            let excess = snapshot
                .telemetry
                .decisions
                .len()
                .saturating_sub(COMPACT_CONTROLLER_DECISIONS);
            snapshot.telemetry.decisions.drain(..excess);
            snapshot.telemetry.decisions.shrink_to_fit();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> ChannelDownloadSnapshot {
        let directory = tempfile::tempdir().expect("temporary fixture");
        let library = DesktopLibrary::open(directory.path().join("db")).expect("library");
        snapshot_from_record(
            library
                .insert_native_download(NewNativeDownloadTaskRecord {
                    account_id: 1,
                    chat_id: 2,
                    message_id: 3,
                    message_sent_at_unix_ms: None,
                    file_name: "fixture".into(),
                    caption: None,
                    mime_type: None,
                    size_bytes: 1,
                    destination: directory.path().join("fixture"),
                    created_at_unix_ms: 1,
                })
                .expect("task"),
        )
    }

    #[test]
    fn admission_evicts_whole_retired_batches_and_protects_recoverable_work() {
        let template = template();
        let snapshots = TransferSnapshots::new(
            (1..=6)
                .map(|id| {
                    let mut snapshot = template.clone();
                    snapshot.id = id;
                    snapshot.state = match id {
                        1 | 2 => ChannelDownloadState::Completed,
                        3 => ChannelDownloadState::Failed(ApplicationErrorKind::Network),
                        4 => ChannelDownloadState::Paused,
                        5 => ChannelDownloadState::Queued,
                        _ => ChannelDownloadState::Cancelled,
                    };
                    snapshot.batch_id = (id <= 2).then_some(1);
                    snapshot
                })
                .collect(),
        )
        .expect("snapshots");
        let controls = Mutex::new(
            (1..=6)
                .map(|id| (id, Arc::new(AtomicU8::new(CONTROL_RUNNING))))
                .collect(),
        );
        let scheduled = Mutex::new(BTreeSet::from([6]));
        let retention = HistoryRetention::new(6, 0, 6);
        let reservation = retention
            .reserve(1, &snapshots, &controls, &scheduled)
            .expect("capacity from whole batch");
        assert_eq!(snapshots.all().expect("records").len(), 4);
        assert!(snapshots.get(1).is_none() && snapshots.get(2).is_none());
        assert_eq!(retention.omitted(), 2);
        assert_eq!(retention.resident.load(Ordering::Acquire), 5);
        assert!(!controls.lock().expect("controls").contains_key(&1));
        drop(reservation); // Failed persistence releases only the reserved admission.
        assert_eq!(retention.resident.load(Ordering::Acquire), 4);
        assert!(
            retention
                .reserve(3, &snapshots, &controls, &scheduled)
                .is_err()
        );
        assert_eq!(snapshots.all().expect("protected records").len(), 4);
        scheduled.lock().expect("scheduled").clear();
        snapshots.update(6, |snapshot| snapshot.state = ChannelDownloadState::Queued);
        assert!(
            retention
                .reserve(3, &snapshots, &controls, &scheduled)
                .is_err()
        );
        // A queued member also protects the completed members of its batch.
        snapshots.update(3, |snapshot| {
            snapshot.state = ChannelDownloadState::Completed;
            snapshot.batch_id = Some(8);
        });
        snapshots.update(5, |snapshot| snapshot.batch_id = Some(8));
        assert!(
            retention
                .reserve(3, &snapshots, &controls, &scheduled)
                .is_err()
        );
    }

    #[test]
    fn concurrent_admissions_share_one_budget_and_failed_admissions_release_it() {
        let retention = Arc::new(HistoryRetention::new(0, 0, 5));
        let snapshots = Arc::new(
            TransferSnapshots::new(Vec::<ChannelDownloadSnapshot>::new()).expect("snapshots"),
        );
        let barrier = Arc::new(std::sync::Barrier::new(17));
        let owners: Vec<_> = (0..16)
            .map(|_| {
                let retention = Arc::clone(&retention);
                let snapshots = Arc::clone(&snapshots);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let controls = Mutex::new(BTreeMap::new());
                    let scheduled = Mutex::new(BTreeSet::new());
                    let reservation = retention.reserve(1, &snapshots, &controls, &scheduled);
                    barrier.wait();
                    reservation.is_ok()
                })
            })
            .collect();
        barrier.wait();
        assert_eq!(
            owners
                .into_iter()
                .map(|owner| usize::from(owner.join().expect("admission owner")))
                .sum::<usize>(),
            5
        );
        assert_eq!(retention.resident.load(Ordering::Acquire), 0);
    }

    #[test]
    fn replay_budget_spans_idle_tasks_and_lifecycle_events_keep_the_latest_outcome() {
        let mut template = template();
        for index in 0..8192 {
            template.part_events.push(ChannelDownloadPartEvent {
                part_index: index,
                offset_bytes: index * DOWNLOAD_PART_SIZE_BYTES,
                length_bytes: DOWNLOAD_PART_SIZE_BYTES,
                state: DownloadPartState::Completed,
                attempt: 1,
                elapsed_millis: 1,
            });
        }
        let mut controller = new_download_controller(
            teleark_transfer::SoftLimitPolicy::Respect,
            DownloadThroughputStrategy::MaxThroughput,
        )
        .expect("controller");
        for sequence in 1..=128 {
            controller.observe(PerformanceSample {
                observed_at_millis: sequence * 1000,
                ..PerformanceSample::default()
            });
        }
        template.telemetry = controller.snapshot();
        assert!(template.telemetry.decisions.len() > COMPACT_CONTROLLER_DECISIONS);
        let decisions = template.telemetry.decisions.len();
        let snapshots = TransferSnapshots::new(
            (1..=64)
                .map(|id| {
                    let mut snapshot = template.clone();
                    snapshot.id = id;
                    snapshot.state = if id == 64 {
                        ChannelDownloadState::Running
                    } else {
                        ChannelDownloadState::Paused
                    };
                    snapshot
                })
                .collect(),
        )
        .expect("snapshots");
        let scheduled = Mutex::new(BTreeSet::from([63, 64]));
        compact_idle_replays(&snapshots, &scheduled);
        let total = snapshots
            .fold(0, |total, snapshot| total + snapshot.part_events.len())
            .expect("event count");
        assert_eq!(total, 18 * 8192 + 46 * COMPACT_PART_EVENTS);
        let compact = snapshots.get(1).expect("compact record");
        assert_eq!(
            compact.part_events.omitted(),
            8192 - COMPACT_PART_EVENTS as u64
        );
        assert_eq!(
            compact.telemetry.decisions.len(),
            COMPACT_CONTROLLER_DECISIONS
        );
        assert_eq!(
            compact.telemetry.decisions[0].sequence - 1,
            decisions as u64 - COMPACT_CONTROLLER_DECISIONS as u64
        );
        assert_eq!(snapshots.get(64).expect("running").part_events.len(), 8192);
        for index in 0..1000 {
            update_snapshot(&snapshots, 1, |snapshot| {
                snapshot.events.push(ChannelDownloadEvent {
                    kind: if index == 999 {
                        ChannelDownloadEventKind::Completed
                    } else {
                        ChannelDownloadEventKind::Paused
                    },
                    timestamp_unix_ms: index,
                    elapsed_ms: None,
                    failure_kind: None,
                });
            });
        }
        let snapshot = snapshots.get(1).expect("bounded lifecycle");
        assert_eq!(snapshot.events.len(), LIFECYCLE_EVENTS);
        assert_eq!(
            snapshot.event_history_omitted,
            1001 - LIFECYCLE_EVENTS as u64
        );
        assert_eq!(
            snapshot.events.last().expect("latest outcome").kind,
            ChannelDownloadEventKind::Completed
        );
    }
}
