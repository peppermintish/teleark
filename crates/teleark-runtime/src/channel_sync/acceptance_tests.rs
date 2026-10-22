use super::*;
use std::sync::{Condvar, atomic::AtomicUsize};

#[derive(Clone)]
struct RestartRemote {
    lifecycle: crate::telegram::lifecycle::Lifecycle,
    discoveries: Arc<AtomicUsize>,
    discovered: mpsc::SyncSender<()>,
    reads: Arc<Mutex<Vec<(i64, ChannelRead)>>>,
    changed: Arc<AtomicBool>,
    interval: Duration,
}
impl AccountSource for RestartRemote {
    fn request_interval(&self) -> Duration {
        self.interval
    }
    fn lifecycle(&self) -> crate::telegram::lifecycle::Lifecycle {
        self.lifecycle.clone()
    }
    fn sync_sources(
        &self,
        _: i64,
        _: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ChannelSyncFailure> {
        self.discoveries.fetch_add(1, Ordering::AcqRel);
        let _ = self.discovered.try_send(());
        Ok((2..34)
            .map(|id| TelegramChatSummary {
                id,
                name: format!("Fixture {id}"),
                username: None,
                kind: TelegramChatKind::Channel,
                sync_pts: Some(if id == 2 && self.changed.load(Ordering::Acquire) {
                    51
                } else {
                    50
                }),
            })
            .collect())
    }
}
impl ChannelSource for RestartRemote {
    fn read(
        &self,
        _: i64,
        chat: i64,
        request: ChannelRead,
        _: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        self.reads.lock().expect("reads").push((chat, request));
        Ok(ChannelReadPage {
            files: vec![],
            removed: vec![],
            edited: vec![],
            pts: Some(51),
            complete: true,
            timeout_seconds: Some(30),
            history_gap: false,
            before: None,
        })
    }
}

#[test]
fn restart_and_connection_replacement_skip_unchanged_channels_but_recover_changed_heads() {
    let temp = tempfile::tempdir().expect("temp");
    let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("library");
    let (discovered, directory_events) = mpsc::sync_channel(32);
    let remote = RestartRemote {
        discovered,
        lifecycle: Default::default(),
        discoveries: Default::default(),
        reads: Default::default(),
        changed: Default::default(),
        interval: Duration::ZERO,
    };
    let account = TelegramAccount {
        id: 1,
        display_name: "Fixture".into(),
        username: None,
    };
    // Old observations model reopening after days away, not just a quick restart.
    let sources = remote.sync_sources(1, Default::default()).expect("sources");
    library
        .save_telegram_sources(&account, &sources)
        .expect("account");
    for chat in &sources {
        library
            .worker
            .request("fixture", |reply| StorageRequest::CommitChannelSync {
                batch: ChannelSyncCommit {
                    account_id: AccountId::new(1),
                    chat_id: ChatId::new(chat.id),
                    expected_revision: 0,
                    state: ChannelSyncState {
                        pts: 50,
                        revision: 1,
                        last_synced_at_unix_ms: Some(1),
                        ..Default::default()
                    },
                    files: vec![],
                    removed: vec![],
                    edited: vec![],
                    invalidate: vec![],
                    authoritative: true,
                    managed_catalog_seed: false,
                    gap_detected: false,
                    observed_at_unix_ms: 1,
                },
                reply,
            })
            .expect("durable cursor");
    }
    remote.discoveries.store(0, Ordering::Release);
    for launch in 0..2 {
        remote
            .lifecycle
            .publish(launch, Some(1), Some(Default::default()));
        let sync =
            ChannelSync::start_with(remote.clone(), library.clone(), account.clone(), vec![])
                .expect("start");
        until(&sync, || {
            let s = sync.snapshot().expect("snapshot");
            remote.discoveries.load(Ordering::Acquire) > launch as usize
                && s.phase == ChannelSyncPhase::Idle
                && s.queued == 0
        });
        assert!(
            remote.reads.lock().expect("reads").is_empty(),
            "32 unchanged channels issue no difference requests"
        );
        sync.stop();
        if let Some(join) = sync.inner.join.lock().expect("owner").take() {
            join.join().expect("shutdown");
        }
    }
    remote.changed.store(true, Ordering::Release);
    remote
        .lifecycle
        .publish(3, Some(1), Some(Default::default()));
    let epoch = Instant::now();
    let seconds = Arc::new(AtomicU64::new(0));
    let clock_seconds = seconds.clone();
    let sync = ChannelSync::start_with_clock(
        remote.clone(),
        library,
        account,
        vec![],
        Arc::new(move || epoch + Duration::from_secs(clock_seconds.load(Ordering::Acquire))),
    )
    .expect("changed start");
    until(&sync, || {
        !remote.reads.lock().expect("reads").is_empty()
            && sync.snapshot().expect("snapshot").phase == ChannelSyncPhase::Idle
    });
    let reads = remote.reads.lock().expect("reads");
    assert_eq!(reads.len(), 1);
    assert!(matches!(reads[0], (2, ChannelRead::Difference(50))));
    drop(reads);
    // A replacement connection also discovers before deciding which cursors lag.
    remote
        .lifecycle
        .publish(4, Some(1), Some(Default::default()));
    until(&sync, || {
        remote.discoveries.load(Ordering::Acquire) >= 4
            && sync.snapshot().expect("snapshot").phase == ChannelSyncPhase::Idle
    });
    assert_eq!(remote.reads.lock().expect("reads").len(), 1);
    sync.view_channel(Some(2)).expect("view channel");
    until(&sync, || {
        remote.reads.lock().expect("reads").len() == 2
            && sync.snapshot().expect("snapshot").phase == ChannelSyncPhase::Idle
    });
    seconds.store(30, Ordering::Release);
    sync.inner.worker.unpark();
    until(&sync, || {
        remote.reads.lock().expect("reads").len() == 3
            && sync.snapshot().expect("snapshot").phase == ChannelSyncPhase::Idle
    });
    sync.view_channel(None).expect("close channel");
    // An acknowledged directory result fences view retirement, then a much
    // later account reconciliation must not fan out differences to quiet channels.
    let before = remote.discoveries.load(Ordering::Acquire);
    seconds.store(31, Ordering::Release);
    sync.refresh_directory().expect("directory reconciliation");
    until(&sync, || {
        remote.discoveries.load(Ordering::Acquire) > before
            && sync.snapshot().expect("snapshot").phase == ChannelSyncPhase::Idle
    });
    while directory_events.try_recv().is_ok() {}
    seconds.store(1000, Ordering::Release);
    sync.inner.worker.unpark();
    directory_events
        .recv_timeout(Duration::from_secs(5))
        .expect("quiet directory probe");
    let reads = remote.reads.lock().expect("targeted reads");
    assert_eq!(
        reads.len(),
        3,
        "closed/quiet channels do not repeatedly fetch differences"
    );
    assert!(
        reads
            .iter()
            .all(|(id, read)| *id == 2 && matches!(read, ChannelRead::Difference(_)))
    );
    drop(reads);
    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("shutdown");
    }
}

#[test]
fn saved_cooldown_blocks_startup_discovery_until_deadline_and_allows_cancellation() {
    let temp = tempfile::tempdir().expect("temp");
    let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("library");
    let wall = crate::system_time_unix_ms(std::time::SystemTime::now()).expect("clock");
    library
        .save_sync_cooldown(1, wall + 120_000, 8_000)
        .expect("saved wait");
    let remote = RestartRemote {
        discovered: mpsc::sync_channel(1).0,
        lifecycle: Default::default(),
        discoveries: Default::default(),
        reads: Default::default(),
        changed: Default::default(),
        interval: Duration::from_secs(1),
    };
    remote
        .lifecycle
        .publish(0, Some(1), Some(Default::default()));
    let sync = ChannelSync::start_with(
        remote.clone(),
        library.clone(),
        TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        },
        vec![],
    )
    .expect("restart");
    until(&sync, || {
        sync.snapshot().expect("snapshot").phase == ChannelSyncPhase::RateLimited
    });
    assert_eq!(remote.discoveries.load(Ordering::Acquire), 0);
    assert_eq!(
        sync.inner
            .shared
            .pacing
            .lock()
            .expect("restored budget")
            .interval_ms(),
        8_000,
        "the owner must restore learned spacing before admitting startup discovery"
    );
    assert!(sync.snapshot().expect("deadline").retry_at.is_some());
    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("cancel waiting owner");
    }
    assert_eq!(remote.discoveries.load(Ordering::Acquire), 0);

    // A subsequent launch automatically proceeds after the saved deadline;
    // no refresh command or GUI action is involved.
    let epoch = Instant::now();
    let offset = Arc::new(AtomicU64::new(0));
    let clock_offset = offset.clone();
    let sync = ChannelSync::start_with_clock(
        remote.clone(),
        library,
        TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        },
        vec![],
        Arc::new(move || epoch + Duration::from_millis(clock_offset.load(Ordering::Acquire))),
    )
    .expect("another restart");
    until(&sync, || {
        sync.snapshot().expect("snapshot").phase == ChannelSyncPhase::RateLimited
    });
    assert_eq!(remote.discoveries.load(Ordering::Acquire), 0);
    offset.store(121_000, Ordering::Release);
    sync.inner.worker.unpark();
    until(&sync, || remote.discoveries.load(Ordering::Acquire) > 0);
    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("automatic recovery");
    }
}

#[derive(Default)]
struct Gate {
    released: Mutex<bool>,
    changed: Condvar,
}
impl Gate {
    fn wait(&self) {
        let mut released = self.released.lock().expect("gate");
        while !*released {
            released = self.changed.wait(released).expect("wait");
        }
    }
    fn release(&self) {
        *self.released.lock().expect("gate") = true;
        self.changed.notify_all();
    }
}
struct ReleaseOnDrop(Arc<Gate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[derive(Clone)]
struct Remote {
    started: mpsc::SyncSender<()>,
    lifecycle: crate::telegram::lifecycle::Lifecycle,
    blocked_channel: Arc<Gate>,
    blocked_metadata: Arc<Gate>,
    metadata_calls: Arc<AtomicUsize>,
    channel_calls: Arc<AtomicI64>,
    first_blocked_file: Arc<AtomicI64>,
}
impl AccountSource for Remote {
    fn lifecycle(&self) -> crate::telegram::lifecycle::Lifecycle {
        self.lifecycle.clone()
    }
    fn sync_sources(
        &self,
        _: i64,
        _: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ChannelSyncFailure> {
        let call = self.metadata_calls.fetch_add(1, Ordering::AcqRel);
        let _ = self.started.send(());
        if call == 1 {
            self.blocked_metadata.wait();
        }
        Ok([2, 3]
            .map(|id| TelegramChatSummary {
                id,
                name: format!("Source {id}"),
                username: None,
                kind: TelegramChatKind::Channel,
                sync_pts: Some(50),
            })
            .to_vec())
    }
}
impl ChannelSource for Remote {
    fn read(
        &self,
        _: i64,
        chat: i64,
        _: ChannelRead,
        _: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        let message = self.channel_calls.fetch_add(1, Ordering::AcqRel) + 100;
        if chat == 2
            && self
                .first_blocked_file
                .compare_exchange(0, message, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            let _ = self.started.send(());
            // Intentionally uncooperative transport: the post-read fence must reject its late page.
            self.blocked_channel.wait();
        }
        Ok(ChannelReadPage {
            files: vec![TelegramFileSummary {
                message_id: message,
                file_name: format!("{message}.bin"),
                caption: String::new(),
                mime_type: None,
                size_bytes: 42,
                duration_ms: None,
                sent_at_unix_ms: message,
                modified_at_unix_ms: message,
            }],
            removed: vec![],
            edited: vec![],
            pts: Some(50),
            complete: true,
            timeout_seconds: None,
            history_gap: false,
            before: None,
        })
    }
}

fn until(sync: &ChannelSync, condition: impl Fn() -> bool) {
    let mut subscription = sync.subscribe();
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime")
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !condition() {
                    assert!(subscription.changed().await);
                }
            })
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "observable state must advance without releasing blocked dependencies: {:?}",
                    sync.snapshot()
                )
            });
        });
}

#[test]
fn account_owner_starts_without_gui_discovery_survives_blocked_calls_and_fences_replaced_connection()
 {
    let temp = tempfile::tempdir().expect("temp");
    let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("library");
    let (started, calls) = mpsc::sync_channel(32);
    let remote = Remote {
        started,
        lifecycle: Default::default(),
        blocked_channel: Default::default(),
        blocked_metadata: Default::default(),
        metadata_calls: Default::default(),
        channel_calls: Default::default(),
        first_blocked_file: Default::default(),
    };
    let _release_channel = ReleaseOnDrop(remote.blocked_channel.clone());
    let _release_metadata = ReleaseOnDrop(remote.blocked_metadata.clone());
    remote
        .lifecycle
        .publish(0, Some(1), Some(Default::default()));
    let sync = ChannelSync::start_with(
        remote.clone(),
        library.clone(),
        TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        },
        vec![],
    )
    .expect("start without a GUI catalog");
    until(&sync, || {
        !library
            .cached_telegram_files(1, 3, 100)
            .expect("cache")
            .is_empty()
    });
    assert!(sync.sources_since(0).is_some());
    assert!(
        library
            .cached_telegram_files(1, 2, 100)
            .expect("blocked cache")
            .is_empty()
    );
    sync.refresh(0).expect("reconcile directory");
    while remote.metadata_calls.load(Ordering::Acquire) < 2 {
        calls
            .recv_timeout(Duration::from_secs(5))
            .expect("metadata started");
    }
    let before = library
        .cached_telegram_files(1, 3, 100)
        .expect("before")
        .len();
    sync.refresh(3).expect("independent channel");
    until(&sync, || {
        library
            .cached_telegram_files(1, 3, 100)
            .expect("after")
            .len()
            > before
    });
    while remote.first_blocked_file.load(Ordering::Acquire) == 0 {
        calls
            .recv_timeout(Duration::from_secs(5))
            .expect("channel started");
    }
    let snapshot = sync.snapshot().expect("visible state");
    assert!(
        snapshot
            .active
            .iter()
            .any(|activity| activity.chat_id == Some(2))
    );
    assert_ne!(snapshot.phase, ChannelSyncPhase::Idle);
    let stale_message = remote.first_blocked_file.load(Ordering::Acquire);
    assert!(stale_message > 0);
    remote.lifecycle.publish(1, None, None);
    remote
        .lifecycle
        .publish(1, Some(1), Some(Default::default()));
    remote.blocked_channel.release();
    remote.blocked_metadata.release();
    until(&sync, || {
        !library
            .cached_telegram_files(1, 2, 100)
            .expect("recovered cache")
            .is_empty()
    });
    assert!(
        library
            .cached_telegram_files(1, 2, 100)
            .expect("recovered cache")
            .iter()
            .all(|file| file.message_id != stale_message)
    );
    assert!(
        remote.metadata_calls.load(Ordering::Acquire) >= 3,
        "new reactor must repopulate its peer directory"
    );
    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("shutdown");
    }
}

#[test]
fn source_notifications_coalesce_without_losing_latest_membership_and_unchanged_lists_are_quiet() {
    let shared = super::event_tests::shared();
    let mut subscription = shared.changes.subscribe();
    let source = |id, name: &str| TelegramChatSummary {
        id,
        name: name.into(),
        username: None,
        kind: TelegramChatKind::Channel,
        sync_pts: None,
    };
    publish_sources(&shared, &[source(2, "Before"), source(3, "Departing")]);
    subscription.borrow_and_update();
    let initial = shared.sources.lock().expect("sources").clone();
    publish_sources(&shared, initial.1.as_ref());
    assert!(!subscription.has_changed().expect("feed"));
    for revision in 0..300 {
        publish_sources(&shared, &[source(2, &format!("Name {revision}"))]);
    }
    let latest = shared.sources.lock().expect("latest");
    assert_eq!(latest.1.len(), 1);
    assert_eq!(latest.1[0].name, "Name 299");
    assert!(latest.0 > initial.0);
    assert_eq!(
        initial.1.len(),
        2,
        "previous coherent snapshots remain immutable"
    );
}

#[derive(Clone)]
struct RetryDirectoryRemote {
    lifecycle: crate::telegram::lifecycle::Lifecycle,
    attempts: Arc<AtomicUsize>,
    started: mpsc::SyncSender<usize>,
}
impl AccountSource for RetryDirectoryRemote {
    fn lifecycle(&self) -> crate::telegram::lifecycle::Lifecycle {
        self.lifecycle.clone()
    }
    fn sync_sources(
        &self,
        _: i64,
        _: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ChannelSyncFailure> {
        let attempt = self.attempts.fetch_add(1, Ordering::AcqRel);
        let _ = self.started.send(attempt);
        match attempt {
            0 => Err(ChannelSyncFailure {
                kind: ApplicationErrorKind::Network,
                retry_after: None,
            }),
            1 => Err(ChannelSyncFailure {
                kind: ApplicationErrorKind::Network,
                retry_after: None,
            }),
            2 => Err(ChannelSyncFailure {
                kind: ApplicationErrorKind::Server,
                retry_after: Some(Duration::from_secs(120)),
            }),
            _ => Ok(vec![]),
        }
    }
}
impl ChannelSource for RetryDirectoryRemote {
    fn read(
        &self,
        _: i64,
        _: i64,
        _: ChannelRead,
        _: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        panic!("an empty directory must not start channel work")
    }
}

#[derive(Clone)]
struct OverlappingFloodRemote {
    lifecycle: crate::telegram::lifecycle::Lifecycle,
    metadata_calls: Arc<AtomicUsize>,
    channel_calls: Arc<AtomicUsize>,
    started: mpsc::SyncSender<(&'static str, usize)>,
    channel_gate: Arc<Gate>,
}
impl AccountSource for OverlappingFloodRemote {
    fn lifecycle(&self) -> crate::telegram::lifecycle::Lifecycle {
        self.lifecycle.clone()
    }
    fn sync_sources(
        &self,
        _: i64,
        _: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ChannelSyncFailure> {
        let attempt = self.metadata_calls.fetch_add(1, Ordering::AcqRel);
        let _ = self.started.send(("directory", attempt));
        if attempt == 1 {
            return Err(ChannelSyncFailure {
                kind: ApplicationErrorKind::Server,
                retry_after: Some(Duration::from_secs(120)),
            });
        }
        Ok(vec![TelegramChatSummary {
            id: 2,
            name: "Source 2".into(),
            username: None,
            kind: TelegramChatKind::Channel,
            sync_pts: Some(50),
        }])
    }
}
impl ChannelSource for OverlappingFloodRemote {
    fn read(
        &self,
        _: i64,
        _: i64,
        _: ChannelRead,
        _: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        let attempt = self.channel_calls.fetch_add(1, Ordering::AcqRel);
        let _ = self.started.send(("channel", attempt));
        if attempt == 0 {
            self.channel_gate.wait();
            return Err(ChannelSyncFailure {
                kind: ApplicationErrorKind::Server,
                retry_after: Some(Duration::from_secs(30)),
            });
        }
        Ok(ChannelReadPage {
            files: vec![],
            removed: vec![],
            edited: vec![],
            pts: Some(50),
            complete: true,
            timeout_seconds: None,
            history_gap: false,
            before: None,
        })
    }
}

#[derive(Clone)]
struct RetiredSourceRemote {
    lifecycle: crate::telegram::lifecycle::Lifecycle,
    channel_calls: Arc<AtomicUsize>,
    channel_started: mpsc::SyncSender<usize>,
    channel_gate: Arc<Gate>,
    cancellation_observed: Arc<AtomicBool>,
    late_result_returned: Arc<AtomicBool>,
}
impl AccountSource for RetiredSourceRemote {
    fn lifecycle(&self) -> crate::telegram::lifecycle::Lifecycle {
        self.lifecycle.clone()
    }
    fn sync_sources(
        &self,
        _: i64,
        _: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ChannelSyncFailure> {
        Ok(vec![TelegramChatSummary {
            id: 2,
            name: "Source 2".into(),
            username: None,
            kind: TelegramChatKind::Channel,
            sync_pts: Some(50),
        }])
    }
}
impl ChannelSource for RetiredSourceRemote {
    fn read(
        &self,
        _: i64,
        _: i64,
        _: ChannelRead,
        cancellation: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        let attempt = self.channel_calls.fetch_add(1, Ordering::AcqRel);
        let _ = self.channel_started.send(attempt);
        if attempt == 0 {
            return Err(ChannelSyncFailure {
                kind: ApplicationErrorKind::Network,
                retry_after: None,
            });
        }

        self.channel_gate.wait();
        self.cancellation_observed
            .store(cancellation.is_cancelled(), Ordering::Release);
        self.late_result_returned.store(true, Ordering::Release);
        Ok(ChannelReadPage {
            files: vec![TelegramFileSummary {
                message_id: 404,
                sent_at_unix_ms: 1_000,
                modified_at_unix_ms: 1_000,
                file_name: "late.bin".into(),
                caption: String::new(),
                mime_type: None,
                size_bytes: 42,
                duration_ms: None,
            }],
            removed: vec![],
            edited: vec![],
            pts: Some(51),
            complete: true,
            timeout_seconds: None,
            history_gap: false,
            before: None,
        })
    }
}

#[test]
fn owner_retries_directory_failures_automatically_and_keeps_flood_wait_after_refresh() {
    let temp = tempfile::tempdir().expect("temp");
    let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("database");
    let (started, attempts) = mpsc::sync_channel(8);
    let remote = RetryDirectoryRemote {
        lifecycle: Default::default(),
        attempts: Default::default(),
        started,
    };
    remote
        .lifecycle
        .publish(0, Some(1), Some(Default::default()));

    let epoch = Instant::now();
    let offset = Arc::new(AtomicU64::new(0));
    let clock_offset = Arc::clone(&offset);
    let clock: SyncClock =
        Arc::new(move || epoch + Duration::from_millis(clock_offset.load(Ordering::Acquire)));
    let sync = ChannelSync::start_with_clock_and_entropy(
        remote.clone(),
        library,
        TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        },
        vec![],
        clock,
        Arc::new(|_, _| 0),
    )
    .expect("owner");

    assert_eq!(
        attempts
            .recv_timeout(Duration::from_secs(5))
            .expect("first metadata call"),
        0
    );
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.failure == Some(ApplicationErrorKind::Network) && snapshot.retry_at.is_some()
        })
    });
    let first_retry = sync
        .snapshot()
        .expect("snapshot")
        .retry_at
        .expect("bounded network retry");
    assert_eq!(
        first_retry,
        epoch + Duration::from_millis(500),
        "the first transient directory failure uses attempt-zero backoff"
    );

    offset.store(
        u64::try_from(first_retry.duration_since(epoch).as_millis()).expect("clock range") + 1,
        Ordering::Release,
    );
    sync.inner.worker.unpark();
    assert_eq!(
        attempts
            .recv_timeout(Duration::from_secs(5))
            .expect("automatic retry"),
        1
    );
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.phase == ChannelSyncPhase::Waiting
                && snapshot.failure == Some(ApplicationErrorKind::Network)
                && snapshot.retry_at.is_some()
        })
    });
    let second_retry = sync
        .snapshot()
        .expect("snapshot")
        .retry_at
        .expect("second local retry deadline");
    assert_eq!(
        second_retry,
        first_retry + Duration::from_millis(1) + Duration::from_secs(1),
        "a second transient failure advances to attempt-one backoff instead of resetting at dispatch"
    );

    offset.store(
        u64::try_from(second_retry.duration_since(epoch).as_millis()).expect("clock range") + 1,
        Ordering::Release,
    );
    sync.inner.worker.unpark();
    assert_eq!(
        attempts
            .recv_timeout(Duration::from_secs(5))
            .expect("FloodWait attempt"),
        2
    );
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.phase == ChannelSyncPhase::RateLimited
                && snapshot.failure == Some(ApplicationErrorKind::Server)
                && snapshot.retry_at.is_some()
        })
    });
    let flood_until = sync
        .snapshot()
        .expect("snapshot")
        .retry_at
        .expect("server deadline");
    let before_refresh = sync.snapshot().expect("snapshot").events.len();
    sync.refresh_directory().expect("directory refresh request");
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.events.len() > before_refresh
                && snapshot.phase == ChannelSyncPhase::RateLimited
                && snapshot.directory_retry_waiting()
                && snapshot.retry_at == Some(flood_until)
        })
    });
    assert_eq!(
        sync.snapshot().expect("snapshot").retry_at,
        Some(flood_until),
        "the directory request cannot shorten Telegram's FloodWait"
    );
    assert!(
        attempts.try_recv().is_err(),
        "no call starts before FloodWait"
    );

    sync.cancel_directory()
        .expect("cancel requested directory refresh");
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.phase == ChannelSyncPhase::Cancelled
                && snapshot.retry_at.is_none()
                && !snapshot.directory_retry_waiting()
        })
    });
    sync.refresh_directory()
        .expect("explicit refresh queues behind the remaining server gate");
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.phase == ChannelSyncPhase::RateLimited
                && snapshot.directory_retry_waiting()
                && snapshot.retry_at == Some(flood_until)
        })
    });
    assert!(
        attempts.try_recv().is_err(),
        "a refresh after cancellation still honors the active FloodWait"
    );

    offset.store(
        u64::try_from(flood_until.duration_since(epoch).as_millis()).expect("clock range") + 1_000,
        Ordering::Release,
    );
    sync.inner.worker.unpark();
    assert_eq!(
        attempts
            .recv_timeout(Duration::from_secs(5))
            .expect("directory recovers"),
        3
    );
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.last_completed_at.is_some() && snapshot.failure.is_none()
        })
    });
    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("shutdown");
    }
}

#[test]
fn owner_keeps_later_directory_flood_wait_when_an_in_flight_channel_returns_a_shorter_wait() {
    let temp = tempfile::tempdir().expect("temp");
    let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("database");
    let (started, events) = mpsc::sync_channel(16);
    let remote = OverlappingFloodRemote {
        lifecycle: Default::default(),
        metadata_calls: Default::default(),
        channel_calls: Default::default(),
        started,
        channel_gate: Default::default(),
    };
    let _release_channel = ReleaseOnDrop(remote.channel_gate.clone());
    remote
        .lifecycle
        .publish(0, Some(1), Some(Default::default()));

    let epoch = Instant::now();
    let offset = Arc::new(AtomicU64::new(0));
    let clock_offset = Arc::clone(&offset);
    let clock: SyncClock =
        Arc::new(move || epoch + Duration::from_millis(clock_offset.load(Ordering::Acquire)));
    let sync = ChannelSync::start_with_clock_and_entropy(
        remote.clone(),
        library,
        TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        },
        vec![],
        clock,
        Arc::new(|_, _| 0),
    )
    .expect("owner");

    assert_eq!(
        events
            .recv_timeout(Duration::from_secs(5))
            .expect("initial directory"),
        ("directory", 0)
    );
    assert_eq!(
        events
            .recv_timeout(Duration::from_secs(5))
            .expect("channel starts"),
        ("channel", 0)
    );
    sync.refresh_directory()
        .expect("overlapping directory refresh");
    assert_eq!(
        events
            .recv_timeout(Duration::from_secs(5))
            .expect("second directory"),
        ("directory", 1)
    );
    let directory_deadline = epoch + Duration::from_secs(120);
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.retry_at == Some(directory_deadline)
                && snapshot.events.iter().any(|event| {
                    event.chat_id.is_none()
                        && event.phase == ChannelSyncPhase::RateLimited
                        && event.failure == Some(ApplicationErrorKind::Server)
                })
        })
    });

    remote.channel_gate.release();
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.retry_at == Some(directory_deadline)
                && snapshot.events.iter().any(|event| {
                    event.chat_id == Some(2)
                        && event.phase == ChannelSyncPhase::RateLimited
                        && event.failure == Some(ApplicationErrorKind::Server)
                })
        })
    });
    assert_eq!(sync.pending_channel_retries(), 1);

    // Move beyond the channel's shorter 30 second wait, while staying inside
    // the directory's 120 second wait. Cancel and explicitly refresh the
    // directory to make the owner process a fresh manual channel retry.
    offset.store(31_000, Ordering::Release);
    sync.inner.worker.unpark();
    sync.cancel_directory()
        .expect("cancel pending directory retry");
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.phase == ChannelSyncPhase::RateLimited
                && snapshot.retry_at == Some(directory_deadline)
                && snapshot.chat_id == Some(2)
                && snapshot.failure == Some(ApplicationErrorKind::Server)
                && !snapshot.directory_retry_waiting()
                && snapshot.events.iter().any(|event| {
                    event.chat_id.is_none()
                        && event.phase == ChannelSyncPhase::Cancelled
                        && event.failure == Some(ApplicationErrorKind::Cancelled)
                })
        })
    });
    sync.refresh(2).expect("channel retry request");
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.phase == ChannelSyncPhase::RateLimited
                && snapshot.retry_at == Some(directory_deadline)
                && snapshot.chat_id == Some(2)
                && snapshot.failure == Some(ApplicationErrorKind::Server)
                && !snapshot.directory_retry_waiting()
        })
    });
    assert_eq!(remote.channel_calls.load(Ordering::Acquire), 1);
    assert!(
        events.try_recv().is_err(),
        "no channel starts before the later deadline"
    );

    offset.store(121_000, Ordering::Release);
    sync.inner.worker.unpark();
    until(&sync, || {
        remote.channel_calls.load(Ordering::Acquire) == 2
            && remote.metadata_calls.load(Ordering::Acquire) == 2
            && sync
                .snapshot()
                .is_ok_and(|snapshot| !snapshot.directory_retry_waiting())
    });
    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("shutdown");
    }
}

#[test]
fn owner_cancels_retired_channel_and_discards_its_late_success() {
    let temp = tempfile::tempdir().expect("temp");
    let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("database");
    let (channel_started, channel_calls) = mpsc::sync_channel(4);
    let channel_gate = Arc::new(Gate::default());
    let remote = RetiredSourceRemote {
        lifecycle: Default::default(),
        channel_calls: Default::default(),
        channel_started,
        channel_gate: Arc::clone(&channel_gate),
        cancellation_observed: Default::default(),
        late_result_returned: Default::default(),
    };
    let _release = ReleaseOnDrop(Arc::clone(&channel_gate));
    remote
        .lifecycle
        .publish(0, Some(1), Some(Default::default()));

    let epoch = Instant::now();
    let offset = Arc::new(AtomicU64::new(0));
    let clock_offset = Arc::clone(&offset);
    let clock: SyncClock =
        Arc::new(move || epoch + Duration::from_millis(clock_offset.load(Ordering::Acquire)));
    let sync = ChannelSync::start_with_clock_and_entropy(
        remote.clone(),
        library.clone(),
        TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        },
        vec![],
        clock,
        Arc::new(|_, _| 0),
    )
    .expect("owner");

    assert_eq!(
        channel_calls
            .recv_timeout(Duration::from_secs(5))
            .expect("first channel attempt"),
        0
    );
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.retry_target().is_some_and(|event| {
                event.chat_id == Some(2) && event.failure == Some(ApplicationErrorKind::Network)
            }) && snapshot.retry_at.is_some()
        })
    });
    let retry_at = sync
        .snapshot()
        .expect("waiting snapshot")
        .retry_at
        .expect("local backoff deadline");
    offset.store(
        u64::try_from(retry_at.duration_since(epoch).as_millis()).expect("clock range") + 1,
        Ordering::Release,
    );
    sync.inner.worker.unpark();
    assert_eq!(
        channel_calls
            .recv_timeout(Duration::from_secs(5))
            .expect("second channel attempt"),
        1
    );
    until(&sync, || {
        sync.inner
            .shared
            .active
            .lock()
            .is_ok_and(|active| active.contains_key(&2))
    });
    let completed_before_retirement = sync.snapshot().expect("active snapshot").last_completed_at;

    sync.update_sources(vec![])
        .expect("remove source from directory");
    until(&sync, || {
        let removed_event = sync.snapshot().is_ok_and(|snapshot| {
            snapshot.retry_target().is_none()
                && snapshot.chat_id.is_none()
                && snapshot.events.iter().any(|event| {
                    event.chat_id == Some(2)
                        && event.phase == ChannelSyncPhase::Cancelled
                        && event.failure == Some(ApplicationErrorKind::SourceMissing)
                })
        });
        let cancelled_token = sync.inner.shared.active.lock().is_ok_and(|active| {
            active
                .get(&2)
                .is_some_and(TelegramScanCancellation::is_cancelled)
        });
        removed_event && cancelled_token
    });
    assert!(
        sync.sources_since(0)
            .is_some_and(|(_, sources)| sources.is_empty())
    );

    channel_gate.release();
    until(&sync, || {
        remote.late_result_returned.load(Ordering::Acquire)
            && sync
                .inner
                .shared
                .active
                .lock()
                .is_ok_and(|active| !active.contains_key(&2))
    });
    assert!(remote.cancellation_observed.load(Ordering::Acquire));
    assert!(
        library
            .cached_telegram_files(1, 2, 100)
            .expect("retired channel cache")
            .is_empty()
    );
    let snapshot = sync.snapshot().expect("retired snapshot");
    assert!(snapshot.retry_target().is_none());
    assert_eq!(snapshot.last_completed_at, completed_before_retirement);
    assert!(
        !snapshot
            .events
            .iter()
            .any(|event| { event.chat_id == Some(2) && event.phase == ChannelSyncPhase::Idle })
    );

    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("shutdown");
    }
}

#[derive(Clone)]
struct CancelDirectoryRemote {
    lifecycle: crate::telegram::lifecycle::Lifecycle,
    attempts: Arc<AtomicUsize>,
    started: mpsc::SyncSender<usize>,
    blocked_first_call: Arc<Gate>,
    cancellation_observed: Arc<AtomicBool>,
}
impl AccountSource for CancelDirectoryRemote {
    fn lifecycle(&self) -> crate::telegram::lifecycle::Lifecycle {
        self.lifecycle.clone()
    }
    fn sync_sources(
        &self,
        _: i64,
        cancellation: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ChannelSyncFailure> {
        let attempt = self.attempts.fetch_add(1, Ordering::AcqRel);
        let _ = self.started.send(attempt);
        if attempt == 0 {
            self.blocked_first_call.wait();
            let cancelled = cancellation.is_cancelled();
            self.cancellation_observed
                .store(cancelled, Ordering::Release);
            if cancelled {
                return Err(ChannelSyncFailure {
                    kind: ApplicationErrorKind::Cancelled,
                    retry_after: None,
                });
            }
        }
        Ok(vec![])
    }
}
impl ChannelSource for CancelDirectoryRemote {
    fn read(
        &self,
        _: i64,
        _: i64,
        _: ChannelRead,
        _: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        panic!("an empty directory must not start channel work")
    }
}

#[test]
fn owner_keeps_directory_cancellation_terminal_and_allows_explicit_refresh() {
    let temp = tempfile::tempdir().expect("temp");
    let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("database");
    let (started, attempts) = mpsc::sync_channel(4);
    let blocked_first_call = Arc::new(Gate::default());
    let remote = CancelDirectoryRemote {
        lifecycle: Default::default(),
        attempts: Default::default(),
        started,
        blocked_first_call: Arc::clone(&blocked_first_call),
        cancellation_observed: Default::default(),
    };
    let _release = ReleaseOnDrop(Arc::clone(&blocked_first_call));
    remote
        .lifecycle
        .publish(0, Some(1), Some(Default::default()));
    let epoch = Instant::now();
    let clock: SyncClock = Arc::new(move || epoch);
    let sync = ChannelSync::start_with_clock(
        remote.clone(),
        library,
        TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        },
        vec![],
        clock,
    )
    .expect("owner");

    assert_eq!(
        attempts
            .recv_timeout(Duration::from_secs(5))
            .expect("initial directory request"),
        0
    );
    sync.cancel_directory().expect("cancel directory request");
    blocked_first_call.release();
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.failure == Some(ApplicationErrorKind::Cancelled) && snapshot.retry_at.is_none()
        })
    });
    let cancelled = sync.snapshot().expect("cancelled snapshot");
    assert_eq!(cancelled.phase, ChannelSyncPhase::Cancelled);
    assert!(remote.cancellation_observed.load(Ordering::Acquire));
    assert_eq!(remote.attempts.load(Ordering::Acquire), 1);

    sync.refresh_directory()
        .expect("explicit refresh after cancel");
    assert_eq!(
        attempts
            .recv_timeout(Duration::from_secs(5))
            .expect("refreshed directory request"),
        1
    );
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.last_completed_at.is_some() && snapshot.failure.is_none()
        })
    });
    assert_eq!(remote.attempts.load(Ordering::Acquire), 2);

    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("shutdown");
    }
}

#[derive(Clone)]
struct DirectoryBackoffCancelRemote {
    lifecycle: crate::telegram::lifecycle::Lifecycle,
    directory_attempts: Arc<AtomicUsize>,
    directory_started: mpsc::SyncSender<usize>,
    channel_reads: Arc<AtomicUsize>,
    channel_started: mpsc::SyncSender<()>,
    channel_gate: Arc<Gate>,
    channel_cancelled: Arc<AtomicBool>,
    channel_finished: Arc<AtomicBool>,
}
impl AccountSource for DirectoryBackoffCancelRemote {
    fn lifecycle(&self) -> crate::telegram::lifecycle::Lifecycle {
        self.lifecycle.clone()
    }
    fn sync_sources(
        &self,
        _: i64,
        _: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ChannelSyncFailure> {
        let attempt = self.directory_attempts.fetch_add(1, Ordering::AcqRel);
        let _ = self.directory_started.send(attempt);
        if attempt == 0 {
            Ok(vec![TelegramChatSummary {
                id: 2,
                name: "Source 2".into(),
                username: None,
                kind: TelegramChatKind::Channel,
                sync_pts: Some(50),
            }])
        } else {
            Err(ChannelSyncFailure {
                kind: ApplicationErrorKind::Network,
                retry_after: None,
            })
        }
    }
}
impl ChannelSource for DirectoryBackoffCancelRemote {
    fn read(
        &self,
        _: i64,
        _: i64,
        _: ChannelRead,
        cancellation: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        let call = self.channel_reads.fetch_add(1, Ordering::AcqRel);
        if call == 0 {
            let _ = self.channel_started.send(());
            self.channel_gate.wait();
            let cancelled = cancellation.is_cancelled();
            self.channel_cancelled.store(cancelled, Ordering::Release);
            self.channel_finished.store(true, Ordering::Release);
            if cancelled {
                return Err(ChannelSyncFailure {
                    kind: ApplicationErrorKind::Cancelled,
                    retry_after: None,
                });
            }
        }
        Ok(ChannelReadPage {
            files: vec![],
            removed: vec![],
            edited: vec![],
            pts: Some(50),
            complete: true,
            timeout_seconds: None,
            history_gap: false,
            before: None,
        })
    }
}

#[test]
fn owner_cancels_pending_directory_backoff_without_cancelling_active_channel() {
    let temp = tempfile::tempdir().expect("temp");
    let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("database");
    let (directory_started, directory_attempts) = mpsc::sync_channel(4);
    let (channel_started, channel_calls) = mpsc::sync_channel(4);
    let channel_gate = Arc::new(Gate::default());
    let remote = DirectoryBackoffCancelRemote {
        lifecycle: Default::default(),
        directory_attempts: Default::default(),
        directory_started,
        channel_reads: Default::default(),
        channel_started,
        channel_gate: Arc::clone(&channel_gate),
        channel_cancelled: Default::default(),
        channel_finished: Default::default(),
    };
    let _release = ReleaseOnDrop(Arc::clone(&channel_gate));
    remote
        .lifecycle
        .publish(0, Some(1), Some(Default::default()));
    let epoch = Instant::now();
    let offset = Arc::new(AtomicU64::new(0));
    let clock_offset = Arc::clone(&offset);
    let clock: SyncClock =
        Arc::new(move || epoch + Duration::from_millis(clock_offset.load(Ordering::Acquire)));
    let sync = ChannelSync::start_with_clock(
        remote.clone(),
        library,
        TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        },
        vec![],
        clock,
    )
    .expect("owner");

    assert_eq!(
        directory_attempts
            .recv_timeout(Duration::from_secs(5))
            .expect("initial directory request"),
        0
    );
    channel_calls
        .recv_timeout(Duration::from_secs(5))
        .expect("channel work starts and remains blocked");
    sync.refresh_directory().expect("start directory probe");
    assert_eq!(
        directory_attempts
            .recv_timeout(Duration::from_secs(5))
            .expect("directory probe"),
        1
    );
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.retry_target().is_some_and(|event| {
                event.chat_id.is_none() && event.failure == Some(ApplicationErrorKind::Network)
            }) && snapshot.retry_at.is_some()
        })
    });
    let retry_at = sync
        .snapshot()
        .expect("waiting snapshot")
        .retry_at
        .expect("automatic retry deadline");

    sync.cancel_directory()
        .expect("cancel pending directory retry");
    until(&sync, || {
        sync.snapshot().is_ok_and(|snapshot| {
            snapshot.failure == Some(ApplicationErrorKind::Cancelled)
                && snapshot.retry_at.is_none()
                && !snapshot.directory_retry_waiting()
                && snapshot
                    .active
                    .iter()
                    .any(|activity| activity.chat_id == Some(2))
        })
    });
    {
        let active = sync.inner.shared.active.lock().expect("active work");
        assert!(
            !active.contains_key(&0),
            "no directory request is in flight"
        );
        assert!(active.get(&2).is_some_and(|token| !token.is_cancelled()));
    }

    offset.store(
        u64::try_from(retry_at.duration_since(epoch).as_millis()).expect("clock range") + 1_000,
        Ordering::Release,
    );
    sync.inner.worker.unpark();
    assert!(matches!(
        directory_attempts.recv_timeout(Duration::from_millis(200)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(remote.directory_attempts.load(Ordering::Acquire), 2);

    channel_gate.release();
    until(&sync, || remote.channel_finished.load(Ordering::Acquire));
    assert!(!remote.channel_cancelled.load(Ordering::Acquire));

    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("shutdown");
    }
}

#[test]
fn expired_saved_spacing_recovers_before_first_startup_request() {
    let temp = tempfile::tempdir().expect("temp");
    let path = temp.path().join("elapsed-restart.sqlite3");
    let wall = crate::system_time_unix_ms(std::time::SystemTime::now()).expect("clock");
    {
        let mut db = teleark_storage::Database::open(&path).expect("database");
        db.set_setting(&teleark_storage::SettingRecord {
            key: "channel-sync-cooldown.1".into(),
            value:
                serde_json::json!({"version":1,"until_unix_ms":wall-600_000,"interval_ms":30_000})
                    .to_string(),
            updated_at_unix_ms: wall - 600_000,
        })
        .expect("old pacing");
    }
    let library = DesktopLibrary::open(&path).expect("library");
    let remote = RestartRemote {
        discovered: mpsc::sync_channel(1).0,
        lifecycle: Default::default(),
        discoveries: Default::default(),
        reads: Default::default(),
        changed: Default::default(),
        interval: Duration::from_secs(1),
    };
    remote
        .lifecycle
        .publish(0, Some(1), Some(Default::default()));
    let sync = ChannelSync::start_with(
        remote.clone(),
        library,
        TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        },
        vec![],
    )
    .expect("start");
    until(&sync, || remote.discoveries.load(Ordering::Acquire) > 0);
    assert_eq!(
        sync.inner
            .shared
            .pacing
            .lock()
            .expect("budget")
            .interval_ms(),
        1_000
    );
    sync.stop();
    if let Some(join) = sync.inner.join.lock().expect("owner").take() {
        join.join().expect("stop");
    }
}
