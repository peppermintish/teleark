use super::*;
use std::cell::RefCell;

fn chat(id: i64) -> TelegramChatSummary {
    TelegramChatSummary {
        id,
        sync_pts: Some(50),
        name: format!("Fixture {id}"),
        username: None,
        kind: TelegramChatKind::Channel,
    }
}
fn file(id: i64) -> TelegramFileSummary {
    TelegramFileSummary {
        message_id: id,
        sent_at_unix_ms: 1_000,
        modified_at_unix_ms: 1_000,
        file_name: format!("{id}.bin"),
        caption: crate::transfer::MANIFEST_CAPTION.into(),
        mime_type: None,
        size_bytes: 42,
    }
}
pub(super) fn shared() -> Arc<Shared> {
    Arc::new(Shared {
        snapshot: Mutex::new(ChannelSyncSnapshot::new(1, 0)),
        pending_channel_retries: AtomicU64::new(0),
        changes: tokio::sync::watch::channel(()).0,
        deltas: Mutex::new(feed::DeltaJournal::default()),
        sources: Mutex::new((0, Arc::new(Vec::new()))),
        history: Mutex::new(history::Requests::default()),
        managed_id: AtomicI64::new(0),
        observation: AtomicU64::new(0),
        manifest_generation: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        active: Mutex::new(BTreeMap::new()),
    })
}
fn setup() -> (tempfile::TempDir, DesktopLibrary, Scheduler) {
    let temp = tempfile::tempdir().expect("temp");
    let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("database");
    let chats = vec![chat(2), chat(3)];
    library
        .save_telegram_sources(
            &TelegramAccount {
                id: 1,
                display_name: "Fixture".into(),
                username: None,
            },
            &chats,
        )
        .expect("sources");
    let mut scheduler = Scheduler::new(chats);
    let batch = ChannelSyncCommit {
        account_id: AccountId::new(1),
        chat_id: ChatId::new(2),
        expected_revision: 0,
        state: ChannelSyncState {
            pts: 50,
            revision: 1,
            ..Default::default()
        },
        files: crate::telegram_file_upserts(AccountId::new(1), ChatId::new(2), &[file(10)])
            .expect("file"),
        removed: vec![],
        authoritative: true,
        edited: vec![],
        invalidate: vec![],
        managed_catalog_seed: false,
        gap_detected: false,
        observed_at_unix_ms: 1_000,
    };
    library
        .worker
        .request("fixture", |reply| StorageRequest::CommitChannelSync {
            batch,
            reply,
        })
        .expect("commit");
    for job in scheduler.jobs.values_mut() {
        job.state = Some(ChannelSyncState {
            pts: 50,
            revision: 1,
            ..Default::default()
        });
    }
    scheduler.queue.clear();
    (temp, library, scheduler)
}
fn push(pts: i32, edited: bool) -> teleark_telegram::ChannelPush {
    let f = file(10);
    teleark_telegram::ChannelPush {
        channel_id: 2,
        pts,
        pts_count: 1,
        files: vec![teleark_telegram::ChannelFileUpdate {
            message_id: f.message_id,
            sent_at_unix_ms: f.sent_at_unix_ms,
            modified_at_unix_ms: f.modified_at_unix_ms,
            file_name: f.file_name,
            caption: f.caption,
            mime_type: f.mime_type,
            size_bytes: f.size_bytes,
        }],
        removed: vec![],
        edited: if edited { vec![10] } else { vec![] },
    }
}
fn page(pts: i32) -> ChannelReadPage {
    ChannelReadPage {
        pts: Some(pts),
        complete: true,
        history_gap: false,
        files: vec![],
        removed: vec![],
        edited: vec![],
        before: None,
    }
}
struct Source {
    calls: RefCell<Vec<ChannelRead>>,
    page: RefCell<Option<ChannelReadPage>>,
}
impl ChannelSource for Source {
    fn read(
        &self,
        _: i64,
        _: i64,
        request: ChannelRead,
        _: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        self.calls.borrow_mut().push(request);
        Ok(self.page.borrow_mut().take().expect("no unplanned RPC"))
    }
}

#[test]
fn contiguous_push_commits_without_rpc_and_duplicate_pushes_do_nothing() {
    let (_temp, library, mut scheduler) = setup();
    let shared = shared();
    library.managed_channel_watch(1, 2, true).expect("watch");
    shared.managed_id.store(2, Ordering::Release);
    scheduler.jobs.get_mut(&2).expect("job").catalog_ready = Some(true);
    scheduler.jobs.get_mut(&2).expect("job").watch_loaded = true;
    let source = Source {
        calls: RefCell::new(vec![]),
        page: RefCell::new(None),
    };
    let mut changed = push(51, true);
    changed.files[0].file_name = "edited.bin".into();
    scheduler.receive_push(changed.clone());
    scheduler.hint(2, 51);
    step(
        &source,
        &library,
        1,
        2,
        scheduler.jobs.get_mut(&2).expect("job"),
        &shared,
        TelegramScanCancellation::new(),
    )
    .expect("direct push");
    assert!(source.calls.borrow().is_empty());
    let view = library
        .cached_channel_view(1, 2, 100)
        .expect("coherent view");
    assert_eq!(
        (view.revision, view.files[0].file_name.as_str()),
        (2, "edited.bin")
    );
    let journal = shared.deltas.lock().expect("journal");
    assert_eq!(journal.changes(2, 1).deltas[0].upserted.len(), 1);
    assert!(journal.changes(2, 1).managed_catalog_changed);
    assert!(!journal.changes(2, 2).managed_catalog_changed);
    assert!(journal.changes(3, 1).deltas.is_empty());
    assert!(!journal.changes(3, 1).reset_required);
    drop(journal);
    for _ in 0..50 {
        scheduler.receive_push(changed.clone());
        scheduler.hint(2, 51);
    }
    assert!(!needed(scheduler.jobs.get(&2).expect("job")));
    assert_eq!(
        library
            .managed_channel_watch(1, 2, false)
            .expect("durable alert")
            .change_count,
        1
    );
}

#[test]
fn missing_push_sequence_fetches_one_difference_and_does_not_apply_unverified_payload() {
    let (_temp, library, mut scheduler) = setup();
    let shared = shared();
    let mut future = push(52, true);
    future.files[0].file_name = "unverified.bin".into();
    scheduler.receive_push(future);
    scheduler.hint(2, 52);
    let source = Source {
        calls: RefCell::new(vec![]),
        page: RefCell::new(Some(page(52))),
    };
    step(
        &source,
        &library,
        1,
        2,
        scheduler.jobs.get_mut(&2).expect("job"),
        &shared,
        TelegramScanCancellation::new(),
    )
    .expect("difference");
    assert!(matches!(
        source.calls.borrow().as_slice(),
        [ChannelRead::Difference(50)]
    ));
    assert_eq!(
        library.cached_channel_view(1, 2, 100).expect("view").files,
        vec![file(10)]
    );
}

#[test]
fn completed_differences_do_not_create_polling_or_write_unchanged_rows() {
    let (_temp, library, mut scheduler) = setup();
    let shared = shared();
    let source = Source {
        calls: RefCell::new(vec![]),
        page: RefCell::new(Some(page(50))),
    };
    scheduler.hint(2, 0); // An actual gap signal, never viewport selection.
    step(
        &source,
        &library,
        1,
        2,
        scheduler.jobs.get_mut(&2).expect("job"),
        &shared,
        TelegramScanCancellation::new(),
    )
    .expect("gap recovery");
    assert!(
        library
            .worker
            .request("channel_sync_state", |reply| {
                StorageRequest::ChannelSyncState {
                    account: AccountId::new(1),
                    chat: ChatId::new(2),
                    reply,
                }
            })
            .expect("durable empty difference")
            .last_synced_at_unix_ms
            .is_some()
    );
    assert_eq!(source.calls.borrow().len(), 1);
    assert_eq!(
        library
            .cached_channel_view(1, 2, 100)
            .expect("view")
            .revision,
        1
    );
    assert_eq!(shared.snapshot.lock().expect("snapshot").committed_pages, 0);
    scheduler.queue.clear();
    for seconds in [1, 20, 60, 900, 86400] {
        assert_eq!(
            scheduler.pop(Instant::now() + Duration::from_secs(seconds)),
            None
        );
        assert!(!needed(&scheduler.jobs[&2]));
    }
}

#[test]
fn private_mutation_wakes_ui_while_remote_read_is_blocked_and_rejects_stale_owner() {
    struct Blocked {
        ready: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
    }
    impl ChannelSource for Blocked {
        fn read(
            &self,
            _: i64,
            _: i64,
            _: ChannelRead,
            _: TelegramScanCancellation,
        ) -> Result<ChannelReadPage, ChannelSyncFailure> {
            self.ready.send(()).expect("ready");
            self.release.recv().expect("release");
            Ok(page(50))
        }
    }
    let (_temp, library, mut scheduler) = setup();
    let shared = shared();
    shared.managed_id.store(2, Ordering::Release);
    let wake = transport_waker(&shared, thread::current());
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let worker_shared = shared.clone();
    let worker_library = library.clone();
    let worker = thread::spawn(move || {
        let job = scheduler.jobs.get_mut(&2).expect("job");
        job.force = true;
        step(
            &Blocked {
                ready: ready_tx,
                release: release_rx,
            },
            &worker_library,
            1,
            2,
            job,
            &worker_shared,
            TelegramScanCancellation::new(),
        )
        .expect("released read");
    });
    ready_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("remote started");
    assert_eq!(
        shared.snapshot.lock().expect("phase before wait").phase,
        ChannelSyncPhase::Receiving
    );
    let mut subscription = shared.changes.subscribe();
    wake(Some(3), true);
    assert!(!subscription.has_changed().expect("subscription"));
    wake(Some(2), false);
    assert!(!subscription.has_changed().expect("ordinary publication"));
    wake(Some(2), true);
    assert!(subscription.has_changed().expect("immediate wake"));
    subscription.borrow_and_update();
    assert!(
        shared
            .snapshot
            .lock()
            .expect("warning")
            .managed_review_pending
    );
    assert_eq!(
        library
            .cached_channel_view(1, 2, 100)
            .expect("responsive database")
            .revision,
        1
    );
    release_tx.send(()).expect("release");
    worker.join().expect("worker");
    assert!(
        shared
            .snapshot
            .lock()
            .expect("newer update still pending")
            .managed_review_pending
    );
    shared.stop.store(true, Ordering::Release);
    let generation = shared.observation.load(Ordering::Acquire);
    wake(Some(2), true);
    assert_eq!(shared.observation.load(Ordering::Acquire), generation);
}

#[test]
fn bounded_push_queue_recovers_overflow_from_durable_pts() {
    let (_temp, _library, mut scheduler) = setup();
    for pts in 51..800 {
        scheduler.receive_push(push(pts, false));
        scheduler.hint(2, pts);
    }
    assert!(scheduler.jobs[&2].force);
    assert!(
        scheduler
            .jobs
            .values()
            .map(|job| job.pushes.len())
            .sum::<usize>()
            <= 512
    );
    assert_eq!(
        scheduler.jobs[&2]
            .state
            .as_ref()
            .expect("committed state")
            .pts,
        50
    );
}

#[test]
fn protected_channel_has_priority_without_starving_other_sources() {
    let mut scheduler = Scheduler::new(vec![chat(2), chat(3), chat(4)]);
    scheduler.jobs.get_mut(&2).expect("protected").catalog_ready = Some(true);
    scheduler.command(Command::Prioritize(3));
    let mut order = Vec::new();
    for _ in 0..4 {
        let id = scheduler.pop(Instant::now()).expect("work");
        order.push(id);
        scheduler.enqueue(id);
    }
    assert_eq!(order.last(), Some(&4));
}

#[test]
fn failed_watch_registration_never_starts_remote_work_and_can_retry() {
    let (_temp, library, mut scheduler) = setup();
    let shared = shared();
    handle_command(Command::Watch(9), &library, 1, &mut scheduler, &shared);
    let source = Source {
        calls: RefCell::new(vec![]),
        page: RefCell::new(Some(page(50))),
    };
    let job = scheduler.jobs.get_mut(&9).expect("pending registration");
    assert!(
        step(
            &source,
            &library,
            1,
            9,
            job,
            &shared,
            TelegramScanCancellation::new()
        )
        .is_err()
    );
    assert!(!job.watch_loaded);
    assert!(source.calls.borrow().is_empty());
    assert!(job.state.is_none());
    library
        .save_telegram_sources(
            &TelegramAccount {
                id: 1,
                display_name: "Fixture".into(),
                username: None,
            },
            &[chat(9)],
        )
        .expect("recover local source");
    step(
        &source,
        &library,
        1,
        9,
        job,
        &shared,
        TelegramScanCancellation::new(),
    )
    .expect("retry registration and seed");
    assert!(job.watch_loaded);
    assert!(
        !job.state.as_ref().expect("fresh source").repair_pending,
        "fresh seeds do not re-download their own rows"
    );
}

#[test]
fn transient_failure_recovers_after_more_than_three_attempts_without_refresh() {
    let (_temp, library, mut scheduler) = setup();
    let shared = shared();
    scheduler.hint(2, 51);
    let mut now = Instant::now();
    for attempt in 0..12 {
        assert_eq!(scheduler.pop(now), Some(2));
        let job = scheduler.jobs.get_mut(&2).expect("job");
        let before = job.state.clone();
        let error = ChannelSyncFailure {
            kind: ApplicationErrorKind::Network,
            retry_after: None,
        };
        assert_eq!(job.fail(&error, now, 0), ChannelSyncPhase::Waiting);
        assert!(!job.paused);
        assert_eq!(job.retries, attempt + 1);
        assert_eq!(job.state, before, "a failed read cannot advance the cursor");
        let deadline = job.retry_at.expect("automatic retry");
        assert!(deadline > now && deadline <= now + Duration::from_secs(60));
        scheduler.enqueue(2);
        // Another channel continues while this channel waits.
        scheduler.hint(3, 51);
        assert_eq!(scheduler.pop(now), Some(3));
        assert_eq!(scheduler.pop(deadline - Duration::from_nanos(1)), None);
        now = deadline;
    }
    assert_eq!(scheduler.pop(now), Some(2));
    let source = Source {
        calls: RefCell::new(vec![]),
        page: RefCell::new(Some(page(51))),
    };
    step(
        &source,
        &library,
        1,
        2,
        scheduler.jobs.get_mut(&2).expect("job"),
        &shared,
        TelegramScanCancellation::new(),
    )
    .expect("recovers automatically");
    assert_eq!(
        library
            .cached_channel_view(1, 2, 200)
            .expect("cache")
            .revision,
        2
    );
}

#[test]
fn ordinary_and_managed_channels_start_and_retry_independently() {
    let (_temp, library, mut scheduler) = setup();
    let initial = Scheduler::new(vec![chat(2), chat(3)]);
    assert_eq!(initial.queue, VecDeque::from([2, 3]));

    let shared = shared();
    handle_command(Command::Watch(2), &library, 1, &mut scheduler, &shared);
    scheduler.hint(3, 51);
    assert_eq!(scheduler.jobs[&2].catalog_ready, Some(false));
    assert_eq!(scheduler.pop(Instant::now()), Some(2));

    let now = Instant::now();
    let managed = scheduler.jobs.get_mut(&2).expect("managed channel");
    assert_eq!(
        managed.fail(
            &ChannelSyncFailure {
                kind: ApplicationErrorKind::Network,
                retry_after: None,
            },
            now,
            0,
        ),
        ChannelSyncPhase::Waiting
    );
    assert_eq!(managed.catalog_ready, Some(false));
    scheduler.enqueue(2);

    // The ordinary channel still gets its turn while the managed source waits.
    assert_eq!(scheduler.pop(now), Some(3));
    let ordinary = scheduler.jobs.get_mut(&3).expect("ordinary channel");
    assert_eq!(
        ordinary.fail(
            &ChannelSyncFailure {
                kind: ApplicationErrorKind::Server,
                retry_after: None,
            },
            now,
            0,
        ),
        ChannelSyncPhase::Waiting
    );
    scheduler.enqueue(3);

    let retry_clock = now + Duration::from_secs(60);
    let mut retried = std::collections::BTreeSet::new();
    for _ in 0..5 {
        let id = scheduler.pop(retry_clock).expect("retry work");
        retried.insert(id);
        scheduler.enqueue(id);
    }
    assert!(retried.contains(&2), "managed catalog work retries");
    assert!(retried.contains(&3), "ordinary channel work retries");
}

#[test]
fn complete_source_snapshot_retires_departed_channels_but_keeps_managed_watch_work() {
    let (_temp, _library, mut scheduler) = setup();
    scheduler
        .jobs
        .get_mut(&2)
        .expect("managed channel")
        .catalog_ready = Some(false);
    scheduler.in_flight.extend([2, 3]);

    let removed = scheduler.update_sources(vec![]);

    assert_eq!(removed, vec![3]);
    assert!(scheduler.jobs.contains_key(&2));
    assert!(!scheduler.jobs.contains_key(&3));
    assert!(scheduler.in_flight.contains(&2));
    assert!(scheduler.in_flight.contains(&3));
}

#[test]
fn queued_hint_during_in_flight_failure_is_counted_and_bulk_cancelled() {
    let (_temp, _library, mut scheduler) = setup();
    let now = Instant::now();
    scheduler.queue.clear();
    scheduler.enqueue(2);
    assert_eq!(scheduler.pop(now), Some(2));
    scheduler.in_flight.insert(2);
    scheduler.hint(2, 51);
    assert_eq!(scheduler.queue, VecDeque::from([2]));
    assert_eq!(scheduler.pending_retry_count, 0);

    let was_waiting_before_completion = scheduler.is_waiting_retry(2);
    assert!(!was_waiting_before_completion);
    scheduler.in_flight.remove(&2);
    let job = scheduler.jobs.get_mut(&2).expect("completed channel");
    job.failure = Some(ApplicationErrorKind::Network);
    job.retry_at = Some(now + Duration::from_secs(5));
    job.paused = false;
    scheduler.enqueue_after_completion(2, was_waiting_before_completion);

    assert_eq!(scheduler.pending_retry_count, 1);
    assert_eq!(scheduler.cancel_pending_retries(), vec![2]);
    assert_eq!(scheduler.pending_retry_count, 0);
    assert!(scheduler.jobs[&2].paused);
    assert_eq!(scheduler.jobs[&2].retry_at, None);
    assert!(scheduler.queue.is_empty());
}

#[test]
fn directory_failures_back_off_and_explicit_refresh_queues_directory_work() {
    let now = Instant::now();
    let mut retry = DirectoryRetry::starting(now);
    let mut pending = false;
    let mut last_check = now;
    let transient = ChannelSyncFailure {
        kind: ApplicationErrorKind::Network,
        retry_after: None,
    };
    let due = retry.failed(&transient, now, 1).expect("network retry");
    assert!(due > now && due <= now + Duration::from_secs(60));

    let refresh = Command::RefreshDirectory;
    assert!(refreshes_directory(&refresh, None));
    let requested_at = due + Duration::from_secs(5);
    request_directory_refresh(
        &mut retry,
        &mut pending,
        &mut last_check,
        requested_at,
        None,
    );
    assert!(pending);
    assert_eq!(retry.retry_at, Some(requested_at));
    assert!(last_check < requested_at);
    assert!(!refreshes_directory(&Command::Refresh(2), None));
    assert!(refreshes_directory(
        &Command::Refresh(2),
        Some(ApplicationErrorKind::Persistence)
    ));
    assert!(!refreshes_directory(
        &Command::Refresh(2),
        Some(ApplicationErrorKind::Cancelled)
    ));

    let flood_wait = ChannelSyncFailure {
        kind: ApplicationErrorKind::Server,
        retry_after: Some(Duration::from_secs(120)),
    };
    assert_eq!(
        retry.failed(&flood_wait, requested_at, 1),
        Some(requested_at + Duration::from_secs(120)),
        "the server deadline overrides local backoff"
    );
    assert_eq!(
        retry.attempt, 1,
        "server deadlines do not consume local attempts"
    );

    let denied = ChannelSyncFailure {
        kind: ApplicationErrorKind::PermissionDenied,
        retry_after: None,
    };
    assert_eq!(retry.failed(&denied, requested_at, 1), None);
    assert_eq!(
        retry.attempt, 1,
        "permanent failures do not consume local attempts"
    );
    assert_eq!(retry.retry_at, None, "permission failure remains visible");
}

#[test]
fn overlapping_flood_waits_keep_the_later_active_deadline() {
    let now = Instant::now();
    let later = now + Duration::from_secs(120);
    let shorter = now + Duration::from_secs(30);

    assert_eq!(merged_flood_deadline(None, Some(later), now), Some(later));
    assert_eq!(
        merged_flood_deadline(Some(later), Some(shorter), now),
        Some(later),
        "a shorter second FloodWait cannot release the shared gate early"
    );
    assert_eq!(
        merged_flood_deadline(Some(shorter), Some(later), now),
        Some(later),
        "a later deadline replaces a shorter active gate"
    );
    assert_eq!(
        merged_flood_deadline(Some(later), Some(shorter), later),
        None,
        "expired waits do not become active deadlines again"
    );
}

#[test]
fn manual_directory_refresh_skips_local_backoff_but_preserves_and_can_cancel_flood_wait() {
    let start = Instant::now();
    let network = ChannelSyncFailure {
        kind: ApplicationErrorKind::Network,
        retry_after: None,
    };
    let mut retry = DirectoryRetry::starting(start);
    assert_eq!(
        retry.failed_with_entropy(&network, start, 0),
        Some(start + Duration::from_millis(500))
    );
    let mut pending = false;
    let mut last_check = start;
    let manual_at = start + Duration::from_millis(10);
    request_directory_refresh(&mut retry, &mut pending, &mut last_check, manual_at, None);
    assert!(pending);
    assert_eq!(retry.retry_at, Some(manual_at));
    assert_eq!(retry.attempt, 1, "manual refresh preserves backoff history");

    let mut rate_limited = DirectoryRetry::starting(start);
    let flood_until = rate_limited
        .failed_with_entropy(
            &ChannelSyncFailure {
                kind: ApplicationErrorKind::Server,
                retry_after: Some(Duration::from_secs(120)),
            },
            start,
            0,
        )
        .expect("FloodWait deadline");
    let refresh_at = start + Duration::from_secs(1);
    request_directory_refresh(
        &mut rate_limited,
        &mut pending,
        &mut last_check,
        refresh_at,
        Some(flood_until),
    );
    assert_eq!(rate_limited.retry_at, Some(flood_until));
    rate_limited.cancelled();
    assert_eq!(
        rate_limited.retry_at, None,
        "pending server-gated work cancels"
    );
}

#[test]
fn directory_flood_wait_does_not_advance_the_next_local_retry_attempt() {
    let start = Instant::now();
    let mut retry = DirectoryRetry::starting(start);
    let flood_wait = ChannelSyncFailure {
        kind: ApplicationErrorKind::Server,
        retry_after: Some(Duration::from_secs(120)),
    };

    assert_eq!(
        retry.failed(&flood_wait, start, 1),
        Some(start + Duration::from_secs(120))
    );
    assert_eq!(
        retry.attempt, 0,
        "FloodWait keeps the local retry attempt unchanged"
    );

    let after_flood_wait = start + Duration::from_secs(120);
    let network = ChannelSyncFailure {
        kind: ApplicationErrorKind::Network,
        retry_after: None,
    };
    let local_deadline = retry
        .failed(&network, after_flood_wait, 1)
        .expect("network errors schedule local backoff");
    assert!(local_deadline > after_flood_wait);
    assert_eq!(
        retry.attempt, 1,
        "the first local retry advances from attempt zero"
    );
}

#[test]
fn cancelling_directory_retry_resets_its_backoff_attempt() {
    let now = Instant::now();
    let mut retry = DirectoryRetry::starting(now);
    let transient = ChannelSyncFailure {
        kind: ApplicationErrorKind::Network,
        retry_after: None,
    };

    assert!(retry.failed(&transient, now, 1).is_some());
    assert_eq!(retry.attempt, 1);

    retry.cancelled();

    assert_eq!(
        retry.attempt, 0,
        "explicit refresh starts a fresh backoff sequence"
    );
    assert_eq!(
        retry.retry_at, None,
        "cancellation clears the pending deadline"
    );
}

#[test]
fn cancelling_pending_channel_retry_removes_only_its_queued_work() {
    let (_temp, _library, mut scheduler) = setup();
    let now = Instant::now();
    let retry_at = now + Duration::from_secs(5);
    scheduler.queue.clear();
    let retrying = scheduler.jobs.get_mut(&2).expect("retrying channel");
    retrying.force = true;
    retrying.failure = Some(ApplicationErrorKind::Network);
    retrying.retry_at = Some(retry_at);
    scheduler.enqueue(2);
    scheduler.enqueue(3);
    assert_eq!(scheduler.pending_retry_count, 1);

    scheduler.command(Command::Cancel(2));

    assert_eq!(scheduler.queue.iter().copied().collect::<Vec<_>>(), vec![3]);
    let cancelled = scheduler.jobs.get(&2).expect("cancelled channel");
    assert!(cancelled.paused);
    assert_eq!(cancelled.failure, Some(ApplicationErrorKind::Cancelled));
    assert_eq!(cancelled.retry_at, None);
    assert_eq!(scheduler.pending_retry_count, 0);
    assert_eq!(scheduler.pop(retry_at + Duration::from_secs(1)), Some(3));
    assert_eq!(
        scheduler.pop(retry_at + Duration::from_secs(1)),
        None,
        "the cancelled channel is not dispatched when its backoff deadline arrives"
    );
}

#[test]
fn cancel_all_pending_channel_retries_removes_waiters_but_preserves_active_work() {
    let (_temp, _library, mut scheduler) = setup();
    let now = Instant::now();
    let retry_at = now + Duration::from_secs(5);
    assert!(
        scheduler
            .update_sources(vec![chat(2), chat(3), chat(4), chat(5)])
            .is_empty()
    );
    scheduler.queue.clear();
    for id in [2, 4] {
        let waiting = scheduler.jobs.get_mut(&id).expect("waiting channel");
        waiting.force = true;
        waiting.failure = Some(ApplicationErrorKind::Network);
        waiting.retry_at = Some(retry_at);
        scheduler.enqueue(id);
    }
    scheduler.in_flight.insert(3);
    scheduler.jobs.get_mut(&3).expect("active retry").failure = Some(ApplicationErrorKind::Network);
    scheduler.jobs.get_mut(&3).expect("active retry").retry_at = Some(retry_at);
    scheduler.enqueue(5);
    assert_eq!(scheduler.pending_retry_count, 2);

    let cancelled = scheduler.cancel_pending_retries();

    assert_eq!(cancelled, vec![2, 4]);
    assert_eq!(scheduler.pending_retry_count, 0);
    assert_eq!(
        scheduler.queue.iter().copied().collect::<Vec<_>>(),
        vec![5],
        "only a waiting retry is removed; unrelated queued work remains"
    );
    for id in [2, 4] {
        let waiting = scheduler.jobs.get(&id).expect("cancelled retry");
        assert!(waiting.paused);
        assert_eq!(waiting.failure, Some(ApplicationErrorKind::Cancelled));
        assert_eq!(waiting.retry_at, None);
    }
    assert!(scheduler.in_flight.contains(&3));
    let active = scheduler.jobs.get(&3).expect("active channel");
    assert!(!active.paused);
    assert_eq!(active.retry_at, Some(retry_at));
    assert_eq!(scheduler.pop(retry_at + Duration::from_secs(1)), Some(5));
}

#[test]
fn cancelled_retry_batch_notifies_subscribers_and_resolves_each_target() {
    let shared = shared();
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        snapshot.transition(
            ChannelSyncPhase::Waiting,
            Some(2),
            Some(ApplicationErrorKind::Network),
            Some(Instant::now() + Duration::from_secs(5)),
        );
        snapshot.transition(
            ChannelSyncPhase::RateLimited,
            Some(3),
            Some(ApplicationErrorKind::Server),
            Some(Instant::now() + Duration::from_secs(120)),
        );
    }
    let mut changes = shared.changes.subscribe();
    changes.borrow_and_update();

    publish_cancelled_retries(&shared, &[2, 3]);

    assert!(changes.has_changed().expect("change notification"));
    let snapshot = shared.snapshot.lock().expect("snapshot");
    assert!(snapshot.retry_target().is_none());
}

#[test]
fn pending_retry_count_projection_is_bounded_and_readable() {
    let shared = shared();
    shared
        .pending_channel_retries
        .store(4_096, Ordering::Release);
    let (sender, _receiver) = mpsc::sync_channel(1);
    let sync = ChannelSync {
        inner: Arc::new(Owner {
            sender,
            shared,
            join: Mutex::new(None),
            worker: thread::current(),
        }),
    };

    assert_eq!(sync.pending_channel_retries(), 4_096);
}

#[test]
fn manual_refresh_skips_local_backoff_but_keeps_telegram_flood_wait() {
    let (_temp, _library, mut scheduler) = setup();
    let now = Instant::now();
    scheduler.queue.clear();
    let local_deadline = now + Duration::from_secs(10);
    let local = scheduler.jobs.get_mut(&2).expect("local backoff");
    local.failure = Some(ApplicationErrorKind::Network);
    local.retry_at = Some(local_deadline);
    local.retries = 3;
    scheduler.enqueue(2);

    let flood_deadline = now + Duration::from_secs(120);
    let flood = scheduler.jobs.get_mut(&3).expect("FloodWait");
    flood.failure = Some(ApplicationErrorKind::Server);
    flood.retry_at = Some(flood_deadline);
    flood.rate_limited = true;
    scheduler.enqueue(3);
    assert_eq!(scheduler.pending_retry_count, 2);

    scheduler.command(Command::Refresh(2));
    scheduler.command(Command::Refresh(3));

    let local = scheduler.jobs.get(&2).expect("refreshed local channel");
    assert_eq!(local.retry_at, None, "manual retry skips local backoff");
    assert_eq!(local.failure, None);
    assert!(!local.rate_limited);
    let flood = scheduler.jobs.get(&3).expect("refreshed FloodWait channel");
    assert_eq!(flood.retry_at, Some(flood_deadline));
    assert_eq!(flood.failure, Some(ApplicationErrorKind::Server));
    assert!(flood.rate_limited);
    assert_eq!(scheduler.pending_retry_count, 1);
}

#[test]
fn watch_then_refresh_preserves_a_pending_flood_wait() {
    let (_temp, library, mut scheduler) = setup();
    let shared = shared();
    let now = Instant::now();
    let flood_deadline = now + Duration::from_secs(120);
    scheduler.queue.clear();
    let local = scheduler.jobs.get_mut(&3).expect("local retry channel");
    local.failure = Some(ApplicationErrorKind::Network);
    local.retry_at = Some(now + Duration::from_secs(10));
    local.retries = 2;
    scheduler.enqueue(3);
    let flood = scheduler.jobs.get_mut(&2).expect("FloodWait channel");
    flood.failure = Some(ApplicationErrorKind::Server);
    flood.retry_at = Some(flood_deadline);
    flood.rate_limited = true;
    scheduler.enqueue(2);
    assert_eq!(scheduler.pending_retry_count, 2);

    handle_command(Command::Watch(2), &library, 1, &mut scheduler, &shared);
    let watched = scheduler.jobs.get(&2).expect("watched channel");
    assert_eq!(watched.failure, Some(ApplicationErrorKind::Server));
    assert_eq!(watched.retry_at, Some(flood_deadline));
    assert!(watched.rate_limited);
    assert_eq!(scheduler.pending_retry_count, 2);

    handle_command(Command::Watch(3), &library, 1, &mut scheduler, &shared);
    let locally_retried = scheduler.jobs.get(&3).expect("local retry reset by watch");
    assert_eq!(locally_retried.failure, None);
    assert_eq!(locally_retried.retry_at, None);
    assert_eq!(locally_retried.retries, 0);
    assert_eq!(scheduler.pending_retry_count, 1);

    scheduler.command(Command::Refresh(2));
    let refreshed = scheduler.jobs.get(&2).expect("refreshed channel");
    assert_eq!(refreshed.failure, Some(ApplicationErrorKind::Server));
    assert_eq!(refreshed.retry_at, Some(flood_deadline));
    assert!(refreshed.rate_limited);
    assert_eq!(scheduler.pending_retry_count, 1);
    assert_eq!(scheduler.pop(now + Duration::from_secs(30)), Some(3));
    assert_eq!(scheduler.pop(flood_deadline), Some(2));
    assert_eq!(scheduler.pending_retry_count, 0);
}

#[test]
fn retry_target_survives_other_channel_progress_until_its_own_success() {
    let mut snapshot = ChannelSyncSnapshot::new(1, 0);
    snapshot.transition(
        ChannelSyncPhase::Failed,
        Some(7),
        Some(ApplicationErrorKind::Network),
        None,
    );
    for index in 0..EVENT_CAPACITY * 2 {
        snapshot.transition(
            if index % 2 == 0 {
                ChannelSyncPhase::Receiving
            } else {
                ChannelSyncPhase::Persisting
            },
            Some(3),
            None,
            None,
        );
    }

    assert_eq!(snapshot.failure, None);
    assert_eq!(snapshot.chat_id, Some(3));
    assert_eq!(snapshot.events.len(), EVENT_CAPACITY);
    assert_eq!(
        snapshot.retry_target().map(|event| event.chat_id),
        Some(Some(7))
    );
    assert!(snapshot.events.iter().any(|event| {
        event.chat_id == Some(7) && event.failure == Some(ApplicationErrorKind::Network)
    }));

    snapshot.transition(ChannelSyncPhase::Idle, Some(7), None, None);
    assert!(snapshot.retry_target().is_none());
    assert!(
        snapshot.events.iter().any(|event| {
            event.chat_id == Some(7) && event.failure == Some(ApplicationErrorKind::Network)
        }),
        "completed failures remain available in the event history"
    );

    for index in 0..EVENT_CAPACITY {
        snapshot.transition(
            if index % 2 == 0 {
                ChannelSyncPhase::Receiving
            } else {
                ChannelSyncPhase::Persisting
            },
            Some(3),
            None,
            None,
        );
    }
    assert!(snapshot.events.iter().all(|event| {
        event.chat_id != Some(7) || event.failure != Some(ApplicationErrorKind::Network)
    }));
}

#[test]
fn cancelled_failure_event_resolves_its_retry_target() {
    let mut snapshot = ChannelSyncSnapshot::new(1, 0);
    snapshot.transition(
        ChannelSyncPhase::Failed,
        None,
        Some(ApplicationErrorKind::Network),
        None,
    );
    snapshot.transition(
        ChannelSyncPhase::Cancelled,
        None,
        Some(ApplicationErrorKind::Cancelled),
        None,
    );

    assert!(snapshot.retry_target().is_none());
}

#[test]
fn directory_retry_waiting_survives_timeline_overflow_until_cancelled() {
    let mut snapshot = ChannelSyncSnapshot::new(1, 0);
    snapshot.transition(
        ChannelSyncPhase::Waiting,
        None,
        Some(ApplicationErrorKind::Network),
        Some(Instant::now() + Duration::from_secs(1)),
    );
    for index in 0..EVENT_CAPACITY * 2 {
        snapshot.transition(
            if index % 2 == 0 {
                ChannelSyncPhase::Receiving
            } else {
                ChannelSyncPhase::Persisting
            },
            Some(3),
            None,
            None,
        );
    }
    assert!(snapshot.directory_retry_waiting());
    assert!(snapshot.events.iter().any(|event| {
        event.chat_id.is_none()
            && event.phase == ChannelSyncPhase::Waiting
            && event.failure == Some(ApplicationErrorKind::Network)
    }));

    snapshot.transition(
        ChannelSyncPhase::Cancelled,
        None,
        Some(ApplicationErrorKind::Cancelled),
        None,
    );
    assert!(!snapshot.directory_retry_waiting());
}

#[test]
fn retry_policy_is_bounded_jittered_and_classified_without_error_prose() {
    for (attempt, seconds) in [1, 2, 4, 8, 16, 32, 60, 60].into_iter().enumerate() {
        let lower = recovery_delay(ApplicationErrorKind::Server, attempt as u32, 0).expect("retry");
        let upper = recovery_delay(ApplicationErrorKind::Server, attempt as u32, seconds * 500)
            .expect("retry");
        assert_eq!(lower, Duration::from_millis(seconds * 500));
        assert_eq!(upper, Duration::from_secs(seconds));
    }
    assert!(
        recovery_delay(ApplicationErrorKind::Network, u32::MAX, u64::MAX).expect("saturates")
            <= Duration::from_secs(60)
    );
    for kind in [
        ApplicationErrorKind::Authorization,
        ApplicationErrorKind::PermissionDenied,
        ApplicationErrorKind::StorageAccessDenied,
        ApplicationErrorKind::StorageConfigurationUnsafe,
        ApplicationErrorKind::StorageIdentityDamaged,
        ApplicationErrorKind::StorageIdentityUnsupported,
        ApplicationErrorKind::SourceMissing,
        ApplicationErrorKind::InvalidRequest,
        ApplicationErrorKind::NotFound,
        ApplicationErrorKind::Capacity,
        ApplicationErrorKind::Persistence,
        ApplicationErrorKind::Cancelled,
    ] {
        assert_eq!(recovery_delay(kind, 0, 0), None);
    }
}

#[test]
fn server_deadline_overrides_jitter_and_storage_failure_preserves_committed_state() {
    let (_temp, _library, mut scheduler) = setup();
    let now = Instant::now();
    let job = scheduler.jobs.get_mut(&2).expect("job");
    let original = job.state.clone();
    assert_eq!(
        job.fail(
            &ChannelSyncFailure {
                kind: ApplicationErrorKind::Server,
                retry_after: Some(Duration::from_secs(120))
            },
            now,
            0
        ),
        ChannelSyncPhase::RateLimited
    );
    assert_eq!(job.retry_at, Some(now + Duration::from_secs(120)));
    scheduler.command(Command::History(2));
    scheduler.command(Command::Refresh(2));
    assert_eq!(scheduler.pop(now + Duration::from_secs(119)), None);
    assert_eq!(scheduler.pop(now + Duration::from_secs(120)), Some(2));
    let job = scheduler.jobs.get_mut(&2).expect("job");
    assert_eq!(
        job.fail(
            &ChannelSyncFailure {
                kind: ApplicationErrorKind::Persistence,
                retry_after: None
            },
            now,
            0
        ),
        ChannelSyncPhase::Failed
    );
    assert!(job.paused);
    assert_eq!(job.retry_at, None);
    assert_eq!(job.state, original);
}

#[test]
fn local_backoff_advances_across_server_flood_waits() {
    let (_temp, _library, mut scheduler) = setup();
    let job = scheduler.jobs.get_mut(&2).expect("job");
    let mut now = Instant::now();
    let flood_wait = ChannelSyncFailure {
        kind: ApplicationErrorKind::Server,
        retry_after: Some(Duration::from_secs(120)),
    };

    assert_eq!(job.fail(&flood_wait, now, 0), ChannelSyncPhase::RateLimited);
    assert_eq!(job.retries, 0, "the server supplies this delay");
    now = job.retry_at.expect("first server deadline");

    assert_eq!(
        job.fail(
            &ChannelSyncFailure {
                kind: ApplicationErrorKind::Network,
                retry_after: None,
            },
            now,
            0,
        ),
        ChannelSyncPhase::Waiting
    );
    let first_local_delay = job.retry_at.expect("first local deadline") - now;
    assert_eq!(first_local_delay, Duration::from_millis(500));
    assert_eq!(job.retries, 1);
    now = job.retry_at.expect("first local deadline");

    assert_eq!(job.fail(&flood_wait, now, 0), ChannelSyncPhase::RateLimited);
    assert_eq!(job.retries, 1, "FloodWait preserves prior local attempts");
    now = job.retry_at.expect("second server deadline");

    assert_eq!(
        job.fail(
            &ChannelSyncFailure {
                kind: ApplicationErrorKind::Server,
                retry_after: None,
            },
            now,
            0,
        ),
        ChannelSyncPhase::Waiting
    );
    let second_local_delay = job.retry_at.expect("second local deadline") - now;
    assert_eq!(second_local_delay, Duration::from_secs(1));
    assert_eq!(job.retries, 2);
    assert!(
        second_local_delay > first_local_delay,
        "local backoff grows after FloodWait expires and a transient failure recurs"
    );
}

#[test]
fn resumed_delivery_wakes_network_retries_but_never_shortens_flood_wait() {
    let (_temp, _library, mut scheduler) = setup();
    let now = Instant::now();
    scheduler.hint(2, 51);
    let job = scheduler.jobs.get_mut(&2).expect("job");
    job.retries = 10;
    job.fail(
        &ChannelSyncFailure {
            kind: ApplicationErrorKind::Network,
            retry_after: None,
        },
        now,
        0,
    );
    scheduler.transport_available(now, Some(now + Duration::from_secs(120)));
    assert_eq!(
        scheduler.pop(now),
        None,
        "delivery cannot bypass server embargo"
    );
    scheduler.transport_available(now, None);
    assert_eq!(
        scheduler.pop(now),
        Some(2),
        "delivery wakes the queued network retry"
    );
    let job = scheduler.jobs.get_mut(&2).expect("job");
    job.fail(
        &ChannelSyncFailure {
            kind: ApplicationErrorKind::Server,
            retry_after: None,
        },
        now,
        0,
    );
    scheduler.enqueue(2);
    scheduler.transport_available(now, None);
    assert_eq!(
        scheduler.pop(now),
        None,
        "server backoff survives network delivery"
    );
}

#[test]
fn completion_time_survives_history_eviction_and_failures_are_not_successes() {
    let shared = shared();
    assert_eq!(
        shared.snapshot.lock().expect("snapshot").last_completed_at,
        None
    );
    publish_completed(&shared, Some(2));
    let completed = shared.snapshot.lock().expect("snapshot").last_completed_at;
    assert!(completed.is_some());
    for _ in 0..EVENT_CAPACITY {
        publish(&shared, ChannelSyncPhase::Receiving, Some(2), None, None);
        publish(
            &shared,
            ChannelSyncPhase::Failed,
            Some(2),
            Some(ApplicationErrorKind::Network),
            None,
        );
    }
    publish(&shared, ChannelSyncPhase::Cancelled, Some(2), None, None);
    publish(&shared, ChannelSyncPhase::Idle, None, None, None);
    let snapshot = shared.snapshot.lock().expect("snapshot");
    assert_eq!(snapshot.last_completed_at, completed);
    assert!(snapshot.dropped_events > 0);
    assert_eq!(ChannelSyncSnapshot::new(99, 0).last_completed_at, None);
}

#[test]
fn silence_recovery_is_postponed_by_delivery_and_metadata_hints_are_coalesced() {
    let now = Instant::now();
    assert_eq!(
        source_reconciliation_deadline(now, now, false),
        now + Duration::from_secs(420)
    );
    let delivered = now + Duration::from_secs(419);
    assert_eq!(
        source_reconciliation_deadline(now, delivered, false),
        delivered + Duration::from_secs(420)
    );
    assert_eq!(
        source_reconciliation_deadline(now, delivered, true),
        now + Duration::from_secs(1)
    );
}

#[test]
fn quiet_channels_recover_independently_and_restart_uses_durable_observations() {
    let (_temp, _library, mut scheduler) = setup();
    let now = Instant::now();
    scheduler
        .jobs
        .get_mut(&2)
        .expect("quiet channel")
        .quiet_deadline = Some(now);
    scheduler
        .jobs
        .get_mut(&3)
        .expect("busy channel")
        .quiet_deadline = Some(now + UPDATE_SILENCE_RECOVERY);
    scheduler.recover_quiet_channels(now);
    assert_eq!(scheduler.pop(now), Some(2));
    assert!(scheduler.jobs[&2].force);
    assert!(!scheduler.jobs[&3].force);
    assert_eq!(scheduler.pop(now), None);
    for (saved, expected) in [
        (None, Duration::ZERO),
        (Some(0), Duration::ZERO),
        (Some(599_000), Duration::from_secs(419)),
        (Some(600_000), UPDATE_SILENCE_RECOVERY),
        (Some(600_001), Duration::ZERO),
    ] {
        assert_eq!(restored_quiet_deadline(saved, 600_000, now), now + expected);
    }
    // A blocked dependency is not polled repeatedly by the fallback timer.
    scheduler.in_flight.insert(2);
    scheduler
        .jobs
        .get_mut(&2)
        .expect("blocked channel")
        .quiet_deadline = Some(now);
    assert_eq!(
        scheduler.quiet_deadline(),
        Some(now + UPDATE_SILENCE_RECOVERY)
    );
}

#[test]
fn message_pts_alone_does_not_rebuild_source_metadata() {
    let shared = shared();
    let mut sources = vec![chat(2)];
    publish_sources(&shared, &sources);
    let revision = shared.sources.lock().expect("sources").0;
    let mut subscription = shared.changes.subscribe();
    sources[0].sync_pts = Some(999);
    publish_sources(&shared, &sources);
    assert_eq!(shared.sources.lock().expect("sources").0, revision);
    assert!(!subscription.has_changed().expect("no metadata change"));
    sources[0].name = "Renamed".into();
    publish_sources(&shared, &sources);
    assert!(subscription.has_changed().expect("rename event"));
    subscription.borrow_and_update();
}
