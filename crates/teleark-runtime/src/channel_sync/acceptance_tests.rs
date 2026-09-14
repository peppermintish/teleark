use super::*;
use std::sync::{Condvar, atomic::AtomicUsize};

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
                sent_at_unix_ms: message,
                modified_at_unix_ms: message,
            }],
            removed: vec![],
            edited: vec![],
            pts: Some(50),
            complete: true,
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
            .expect("observable state must advance without releasing blocked dependencies");
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
