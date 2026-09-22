use super::*;

fn completed_batch(
    transfers: &DesktopTransfers,
    directory: &Path,
    count: usize,
) -> Vec<ChannelDownloadSnapshot> {
    let requests = (0..count)
        .map(|index| {
            let mut item = request(directory.join(format!("original-{index}.zip")));
            item.message_id += index as i64;
            item
        })
        .collect();
    let batch = transfers
        .enqueue_channel_download_batch(requests)
        .expect("batch");
    let ids = transfers
        .snapshots()
        .expect("view")
        .into_iter()
        .filter(|row| row.batch_id == Some(batch))
        .map(|row| row.id)
        .collect::<Vec<_>>();
    ids.into_iter()
        .map(|id| {
            let row = wait_for_terminal(transfers, id);
            assert_eq!(row.state, ChannelDownloadState::Completed);
            row
        })
        .collect()
}

#[test]
fn completed_batch_retry_groups_only_missing_files_and_keeps_original_history() {
    let directory = tempfile::tempdir().expect("fixture");
    let library = library(&directory);
    let backend = Arc::new(FakeBackend {
        outcome: Ok(()),
        calls: StdMutex::new(Vec::new()),
    });
    let transfers = test_transfers(backend.clone(), library.clone()).expect("transfers");
    let original = completed_batch(&transfers, directory.path(), 3);
    for row in [&original[0], &original[2]] {
        fs::remove_file(&row.destination).expect("external deletion");
    }
    drop(transfers);
    let transfers = test_transfers(backend.clone(), library.clone()).expect("restored history");
    let mut ids = original.iter().map(|row| row.id).collect::<Vec<_>>();
    ids.extend([original[0].id, original[2].id]); // selected header plus children
    let batches = transfers
        .redownload_missing_completed(&ids, &AtomicBool::new(false))
        .expect("retry missing outputs");
    assert_eq!(batches.len(), 1);
    assert_ne!(Some(batches[0]), original[0].batch_id);
    let new = transfers
        .snapshots()
        .expect("view")
        .into_iter()
        .filter(|row| row.batch_id == Some(batches[0]))
        .collect::<Vec<_>>();
    assert_eq!(new.len(), 2);
    assert_eq!(
        new.iter()
            .map(|row| row.message_id)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([original[0].message_id, original[2].message_id])
    );
    assert_ne!(
        new[0].destination, new[1].destination,
        "same names get distinct destinations"
    );
    for row in new {
        assert_eq!(
            wait_for_terminal(&transfers, row.id).state,
            ChannelDownloadState::Completed
        );
        assert_eq!(
            fs::read(row.destination).expect("replacement"),
            b"telegram bytes"
        );
    }
    assert_eq!(
        fs::read(&original[1].destination).expect("existing copy"),
        b"telegram bytes"
    );
    assert_eq!(backend.calls.lock().expect("calls").len(), 5);
    for row in original {
        let stored = library
            .native_download(row.id)
            .expect("history")
            .expect("preserved");
        assert_eq!(stored.state, StoredNativeDownloadState::Completed);
        assert_eq!(stored.batch_id, row.batch_id);
        assert_eq!(stored.destination, row.destination);
    }
}

#[test]
fn completed_batch_retry_rechecks_recreated_changed_and_unavailable_paths() {
    let directory = tempfile::tempdir().expect("fixture");
    let library = library(&directory);
    let backend = Arc::new(FakeBackend {
        outcome: Ok(()),
        calls: StdMutex::new(Vec::new()),
    });
    let transfers = test_transfers(backend.clone(), library.clone()).expect("transfers");
    let original = completed_batch(&transfers, directory.path(), 3);
    for row in &original {
        fs::remove_file(&row.destination).expect("delete");
    }
    fs::write(&original[0].destination, b"telegram bytes").expect("restored after cached deletion");
    fs::write(&original[1].destination, b"user replacement").expect("changed file");
    let ids = original.iter().map(|row| row.id).collect::<Vec<_>>();
    let batches = transfers
        .redownload_missing_with_probe(&ids, &AtomicBool::new(false), &|path, size| {
            if path == original[2].destination {
                crate::LocalFilePresence::Unavailable
            } else {
                crate::local_file_presence(path, size)
            }
        })
        .expect("recheck");
    assert!(batches.is_empty());
    assert_eq!(backend.calls.lock().expect("calls").len(), 3);
    assert_eq!(library.native_downloads().expect("history").len(), 3);
    assert_eq!(
        fs::read(&original[1].destination).expect("preserved"),
        b"user replacement"
    );
    transfers.activate_account(2).expect("other account");
    assert_eq!(
        transfers
            .redownload_missing_completed(&ids, &AtomicBool::new(false))
            .expect_err("account isolation")
            .kind(),
        ApplicationErrorKind::Authorization
    );
}

#[test]
fn cancelling_blocked_batch_retry_releases_prepared_markers_without_queueing_more_work() {
    let directory = tempfile::tempdir().expect("fixture");
    let library = library(&directory);
    let backend = Arc::new(FakeBackend {
        outcome: Ok(()),
        calls: StdMutex::new(Vec::new()),
    });
    let transfers = test_transfers(backend.clone(), library.clone()).expect("transfers");
    let original = completed_batch(&transfers, directory.path(), 2);
    for row in &original {
        fs::remove_file(&row.destination).expect("delete");
    }
    let ids = original.iter().map(|row| row.id).collect::<Vec<_>>();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let owner = transfers.clone();
    let cancellation = cancelled.clone();
    let work = thread::spawn(move || {
        let calls = AtomicUsize::new(0);
        owner.redownload_missing_with_probe(&ids, &cancellation, &|_, _| {
            if calls.fetch_add(1, Ordering::Relaxed) == 1 {
                entered_tx.send(()).expect("blocked");
                release_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("release");
            }
            crate::LocalFilePresence::Missing
        })
    });
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("second check blocks after preparation");
    let unrelated = transfers
        .enqueue_channel_download(request(directory.path().join("unrelated.zip")))
        .expect("unrelated admission remains responsive");
    assert_eq!(
        wait_for_terminal(&transfers, unrelated).state,
        ChannelDownloadState::Completed
    );
    cancelled.store(true, Ordering::Release);
    release_tx.send(()).expect("release");
    assert_eq!(
        work.join().expect("owner").expect_err("cancelled").kind(),
        ApplicationErrorKind::Cancelled
    );
    assert_eq!(library.native_downloads().expect("history").len(), 3);
    assert_eq!(backend.calls.lock().expect("calls").len(), 3);
    let downloads = library.managed_directories().expect("downloads").downloads;
    assert!(
        fs::read_dir(downloads).expect("directory").next().is_none(),
        "no abandoned markers"
    );
}

#[test]
fn batch_retry_admission_failure_releases_only_its_unpublished_reservations() {
    let directory = tempfile::tempdir().expect("fixture");
    let library = library(&directory);
    let backend = Arc::new(FakeBackend {
        outcome: Ok(()),
        calls: StdMutex::new(Vec::new()),
    });
    let mut transfers = test_transfers(backend, library.clone()).expect("transfers");
    let original = completed_batch(&transfers, directory.path(), 2);
    for row in &original {
        fs::remove_file(&row.destination).expect("delete");
    }
    Arc::get_mut(&mut transfers.inner)
        .expect("unique owner")
        .retention = HistoryRetention::new(2, 0, 1);
    let downloads = library.managed_directories().expect("downloads").downloads;
    let unrelated = downloads.join("retained.partial");
    fs::write(&unrelated, b"encrypted recovery").expect("unrelated recovery");
    let ids = original.iter().map(|row| row.id).collect::<Vec<_>>();
    assert_eq!(
        transfers
            .redownload_missing_completed(&ids, &AtomicBool::new(false))
            .expect_err("capacity")
            .kind(),
        ApplicationErrorKind::Capacity
    );
    assert_eq!(library.native_downloads().expect("history").len(), 2);
    assert_eq!(fs::read_dir(downloads).expect("directory").count(), 1);
    assert_eq!(
        fs::read(unrelated).expect("preserved"),
        b"encrypted recovery"
    );
}
