use super::*;
use teleark_storage::{
    NewNativeDownloadTaskRecord, StoredNativeDownloadState, StoredNativeDownloadVerification,
    VaultDownloadRecord,
};

fn record(root: &std::path::Path, id: u64) -> DownloadedFileRecord {
    DownloadedFileRecord {
        cursor: DownloadedFilesCursor { kind: 0, id },
        account_id: 1,
        chat_id: 20,
        message_id: Some(id as i64),
        package_id: None,
        destination: root.join(format!("download-{id}.bin")),
        size_bytes: 3,
        completed_at_unix_ms: id as i64,
    }
}

fn next(model: &mut Observations, at: Instant) -> Probe {
    let (tx, rx) = mpsc::sync_channel(1);
    model.dispatch(1, at, &tx);
    rx.try_recv().expect("scheduled probe")
}

#[test]
fn removed_history_does_not_stop_rechecking_a_previously_present_path() {
    let dir = tempfile::tempdir().expect("fixture");
    let file = record(dir.path(), 1);
    std::fs::write(&file.destination, b"abc").expect("download");
    let now = Instant::now();
    let mut model = Observations::default();
    model.insert(file.clone(), now);
    assert_eq!(
        model.entries[&file.destination].observation.presence,
        LocalFilePresence::Checking
    );
    let job = next(&mut model, now);
    model.complete(
        job,
        now,
        local_file_presence(&file.destination, 3),
        Some(20),
        now,
    );
    assert_eq!(
        model.entries[&file.destination].observation.presence,
        LocalFilePresence::Present
    );
    // Subsequent inventory reads contain no record: transfer history was deleted.
    // The retained path must still be rechecked without another inventory insert.
    std::fs::remove_file(&file.destination).expect("external deletion");
    let later = now + VISIBLE_INTERVAL;
    let job = next(&mut model, later);
    model.complete(
        job,
        later,
        local_file_presence(&file.destination, 3),
        Some(20),
        later,
    );
    assert_eq!(
        model.entries[&file.destination].observation.presence,
        LocalFilePresence::Missing
    );
    std::fs::write(&file.destination, b"different").expect("changed output");
    let later = later + VISIBLE_INTERVAL;
    let job = next(&mut model, later);
    model.complete(
        job,
        later,
        local_file_presence(&file.destination, 3),
        Some(20),
        later,
    );
    assert_eq!(
        model.entries[&file.destination].observation.presence,
        LocalFilePresence::SizeChanged
    );
}

#[test]
fn stale_presence_expires_even_when_a_probe_never_finishes() {
    let dir = tempfile::tempdir().expect("fixture");
    let file = record(dir.path(), 1);
    let now = Instant::now();
    let mut model = Observations::default();
    model.insert(file.clone(), now);
    let job = next(&mut model, now);
    model.complete(job, now, LocalFilePresence::Present, Some(20), now);
    let blocked = next(&mut model, now + VISIBLE_INTERVAL);
    model.expire(now + MAX_AGE);
    assert_eq!(
        model.entries[&file.destination].observation.presence,
        LocalFilePresence::Checking
    );
    // An excessively late response cannot restore an old positive observation.
    model.complete(
        blocked,
        now + VISIBLE_INTERVAL,
        LocalFilePresence::Present,
        Some(20),
        now + MAX_AGE * 2,
    );
    assert_eq!(
        model.entries[&file.destination].observation.presence,
        LocalFilePresence::Unavailable
    );
}

#[test]
fn replacement_metadata_rejects_old_probe_and_older_history_pages() {
    let dir = tempfile::tempdir().expect("fixture");
    let file = record(dir.path(), 1);
    let now = Instant::now();
    let mut model = Observations::default();
    model.insert(file.clone(), now);
    let old = next(&mut model, now);
    let mut replacement = file.clone();
    replacement.message_id = Some(2);
    replacement.completed_at_unix_ms += 1;
    model.insert(replacement.clone(), now);
    model.complete(old, now, LocalFilePresence::Present, Some(20), now);
    model.insert(file, now);
    let row = &model.entries[&replacement.destination].observation;
    assert_eq!(row.file, replacement);
    assert_eq!(row.presence, LocalFilePresence::Checking);
}

fn complete_history(library: &DesktopLibrary, file: &DownloadedFileRecord) {
    let mut task = library
        .insert_native_download(NewNativeDownloadTaskRecord {
            account_id: file.account_id,
            chat_id: file.chat_id,
            message_id: file.message_id.expect("message"),
            file_name: "historical.bin".into(),
            destination: file.destination.clone(),
            size_bytes: file.size_bytes,
            created_at_unix_ms: file.completed_at_unix_ms,
            message_sent_at_unix_ms: None,
            caption: None,
            mime_type: None,
        })
        .expect("history");
    task.state = StoredNativeDownloadState::Completed;
    task.verification = StoredNativeDownloadVerification::SizeChecked;
    task.transferred_bytes = file.size_bytes;
    task.finished_at_unix_ms = Some(file.completed_at_unix_ms);
    library
        .save_native_download(task)
        .expect("completed history");
}

fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "observation timed out");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn startup_checks_old_deleted_downloads_across_pages_and_discovers_new_completions() {
    let dir = tempfile::tempdir().expect("fixture");
    let library = DesktopLibrary::open(dir.path().join("library.sqlite3")).expect("library");
    for id in 1..=260 {
        complete_history(&library, &record(dir.path(), id));
    }
    let existing = record(dir.path(), 130);
    std::fs::write(&existing.destination, b"abc").expect("remaining file");
    let monitor = LocalDownloadMonitor::new(library.clone()).expect("monitor");
    monitor.set_context(Some((1, 7)), Some(20));
    let mut view = BTreeMap::new();
    wait(|| {
        view.extend(monitor.take_updates().changes);
        view.len() == 260
            && view.values().all(|row| {
                row.as_ref()
                    .is_some_and(|row| row.presence != LocalFilePresence::Checking)
            })
    });
    assert_eq!(
        view[&existing.destination]
            .as_ref()
            .expect("existing")
            .presence,
        LocalFilePresence::Present
    );
    assert!(
        view.iter()
            .filter(|(path, _)| **path != existing.destination)
            .all(|(_, row)| row.as_ref().expect("row").presence == LocalFilePresence::Missing)
    );
    let inventory_reads = monitor.shared.inventory_reads.load(Ordering::Acquire);
    // Filesystem checking continues without re-reading an unchanged inventory.
    monitor.set_context(Some((1, 7)), Some(21));
    monitor.set_context(Some((1, 7)), Some(20));
    let new_file = record(dir.path(), 261);
    std::fs::write(&new_file.destination, b"abc").expect("new file");
    complete_history(&library, &new_file);
    wait(|| {
        view.extend(monitor.take_updates().changes);
        view.get(&new_file.destination).is_some_and(|row| {
            row.as_ref()
                .is_some_and(|row| row.presence == LocalFilePresence::Present)
        })
    });
    assert!(monitor.shared.inventory_reads.load(Ordering::Acquire) > inventory_reads);
    assert_eq!(
        library
            .native_download(1)
            .expect("history")
            .expect("retained")
            .state,
        StoredNativeDownloadState::Completed
    );
}

#[test]
fn blocked_path_does_not_block_other_observations_account_change_or_owner_drop() {
    let dir = tempfile::tempdir().expect("fixture");
    let library = DesktopLibrary::open(dir.path().join("library.sqlite3")).expect("library");
    let file = record(dir.path(), 1);
    complete_history(&library, &file);
    let mut other = record(dir.path(), 2);
    other.account_id = 2;
    complete_history(&library, &other);
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Arc::new(Mutex::new(release_rx));
    let monitor = LocalDownloadMonitor::with_probe(
        library,
        Arc::new(move |file| {
            if file.account_id == 1 {
                entered_tx.send(()).expect("entered");
                release_rx
                    .lock()
                    .expect("gate")
                    .recv_timeout(Duration::from_secs(5))
                    .expect("release blocked filesystem");
                LocalFilePresence::Present
            } else {
                LocalFilePresence::Missing
            }
        }),
    )
    .expect("monitor");
    monitor.set_context(Some((1, 1)), Some(20));
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("blocked probe");
    let pending = monitor.take_updates();
    assert_eq!(
        pending.changes[&file.destination]
            .as_ref()
            .expect("acknowledged before filesystem wait")
            .presence,
        LocalFilePresence::Checking
    );
    monitor.set_context(Some((2, 2)), Some(20));
    let mut view = BTreeMap::new();
    wait(|| {
        let update = monitor.take_updates();
        if update.reset {
            view.clear();
        }
        view.extend(update.changes);
        view.get(&other.destination).is_some_and(|row| {
            row.as_ref()
                .is_some_and(|row| row.presence == LocalFilePresence::Missing)
        })
    });
    assert!(!view.contains_key(&file.destination));
    let (dropped_tx, dropped_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        drop(monitor);
        dropped_tx.send(()).expect("drop acknowledged");
    });
    dropped_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("drop must not join blocked I/O");
    release_tx.send(()).expect("release");
}

#[test]
fn vault_inventory_revision_changes_only_on_success() {
    let dir = tempfile::tempdir().expect("fixture");
    let library = DesktopLibrary::open(dir.path().join("library.sqlite3")).expect("library");
    let before = library.downloaded_files_revision.load(Ordering::Acquire);
    let mut file = VaultDownloadRecord {
        account_id: 1,
        chat_id: 20,
        package_id: "a".repeat(32),
        destination: dir.path().join("vault.bin"),
        size_bytes: 3,
        completed_at_unix_ms: 1,
    };
    library.record_vault_download(file.clone()).expect("record");
    assert_eq!(
        library.downloaded_files_revision.load(Ordering::Acquire),
        before + 1
    );
    file.account_id = 0;
    assert!(library.record_vault_download(file).is_err());
    assert_eq!(
        library.downloaded_files_revision.load(Ordering::Acquire),
        before + 1
    );
}

#[test]
fn dropping_observer_closes_inventory_without_a_subscriber_acknowledgement() {
    let dir = tempfile::tempdir().expect("fixture");
    let library = DesktopLibrary::open(dir.path().join("library.sqlite3")).expect("library");
    let worker = Arc::downgrade(&library.worker.inner);
    let monitor = LocalDownloadMonitor::new(library).expect("monitor");
    let _subscription = monitor.subscribe();
    monitor.set_context(Some((1, 1)), Some(20));
    drop(monitor);
    wait(|| worker.upgrade().is_none());
}

#[test]
fn deleting_native_history_and_then_the_file_updates_the_existing_observation() {
    let dir = tempfile::tempdir().expect("fixture");
    let library = DesktopLibrary::open(dir.path().join("library.sqlite3")).expect("library");
    let file = record(dir.path(), 1);
    std::fs::write(&file.destination, b"abc").expect("download");
    complete_history(&library, &file);
    let monitor = LocalDownloadMonitor::new(library.clone()).expect("monitor");
    monitor.set_context(Some((1, 1)), Some(20));
    wait(|| {
        monitor
            .take_updates()
            .changes
            .get(&file.destination)
            .is_some_and(|row| {
                row.as_ref()
                    .is_some_and(|row| row.presence == LocalFilePresence::Present)
            })
    });
    let reads = monitor.shared.inventory_reads.load(Ordering::Acquire);
    library.delete_native_download(1).expect("delete history");
    assert!(
        file.destination.exists(),
        "history deletion retains the download"
    );
    std::fs::remove_file(&file.destination).expect("external deletion");
    wait(|| {
        monitor
            .take_updates()
            .changes
            .get(&file.destination)
            .is_some_and(|row| {
                row.as_ref()
                    .is_some_and(|row| row.presence == LocalFilePresence::Missing)
            })
    });
    assert_eq!(
        monitor.shared.inventory_reads.load(Ordering::Acquire),
        reads,
        "known paths refresh independently of SQL discovery"
    );
    assert!(
        library
            .downloaded_files_page(1, None)
            .expect("inventory")
            .is_empty()
    );
}

#[test]
fn retention_and_unconsumed_changes_are_bounded_and_report_omission() {
    let (revision, _) = watch::channel(0);
    let shared = Shared {
        context: Mutex::new(Context::default()),
        shutdown: AtomicBool::new(false),
        epoch: AtomicU64::new(1),
        inventory_reads: AtomicU64::new(0),
        updates: Mutex::new(LocalDownloadUpdates::default()),
        revision,
    };
    let mut model = Observations::default();
    let now = Instant::now();
    for id in 0..CAPACITY * 3 {
        model.insert(
            record(
                std::path::Path::new("/synthetic-local-observation"),
                id as u64,
            ),
            now,
        );
        if id % 128 == 0 {
            model.publish(&shared, Some((1, 1)), false);
        }
        assert!(model.entries.len() <= CAPACITY);
        assert!(shared.updates.lock().expect("updates").changes.len() <= CAPACITY * 2);
    }
    model.publish(&shared, Some((1, 1)), false);
    assert_eq!(model.entries.len(), CAPACITY);
    assert_eq!(model.due.len(), CAPACITY);
    assert_eq!(model.ages.len(), CAPACITY);
    let update = take_updates(&shared);
    assert!(update.limited && update.reset);
    let restored: BTreeMap<_, _> = update
        .changes
        .into_iter()
        .filter_map(|(path, row)| row.map(|row| (path, row)))
        .collect();
    assert_eq!(restored.len(), CAPACITY);
    for (path, row) in model.entries {
        assert_eq!(restored[&path], row.observation);
    }
}
