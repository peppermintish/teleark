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
