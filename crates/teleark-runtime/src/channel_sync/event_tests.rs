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
fn shared() -> Arc<Shared> {
    Arc::new(Shared {
        snapshot: Mutex::new(ChannelSyncSnapshot::new(1, 0)),
        changes: tokio::sync::watch::channel(()).0,
        deltas: Mutex::new(feed::DeltaJournal::default()),
        managed_id: AtomicI64::new(0),
        observation: AtomicU64::new(0),
        manifest_generation: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        active: Mutex::new(None),
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
        timeout_seconds: Some(20),
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
fn subscribed_deadlines_stop_on_navigation_and_blank_differences_do_not_write() {
    let (_temp, library, mut scheduler) = setup();
    let shared = shared();
    scheduler.command(Command::Observe(Some(2)));
    scheduler.due_subscriptions(Instant::now());
    let source = Source {
        calls: RefCell::new(vec![]),
        page: RefCell::new(Some(page(50))),
    };
    let before = Instant::now();
    step(
        &source,
        &library,
        1,
        2,
        scheduler.jobs.get_mut(&2).expect("job"),
        &shared,
        TelegramScanCancellation::new(),
    )
    .expect("deadline check");
    let deadline = scheduler.jobs[&2].next_check.expect("server timeout");
    assert!(deadline >= before + Duration::from_secs(20));
    assert_eq!(
        library
            .cached_channel_view(1, 2, 100)
            .expect("view")
            .revision,
        1
    );
    assert_eq!(shared.snapshot.lock().expect("snapshot").committed_pages, 0);
    scheduler.queue.clear();
    scheduler.due_subscriptions(deadline - Duration::from_millis(1));
    assert!(scheduler.queue.is_empty());
    scheduler.command(Command::Observe(None));
    scheduler.due_subscriptions(deadline);
    assert!(scheduler.queue.is_empty());
    // The private protection interest survives closing the visible channel.
    handle_command(Command::Watch(2), &library, 1, &mut scheduler, &shared);
    scheduler.command(Command::Observe(None));
    assert!(scheduler.jobs[&2].followed);
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
