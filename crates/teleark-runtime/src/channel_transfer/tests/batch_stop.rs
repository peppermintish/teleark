use super::*;

struct BatchBackend {
    started: mpsc::Sender<(i64, Arc<dyn DownloadObserver>)>,
    stopped: mpsc::Sender<i64>,
    release: (StdMutex<bool>, Condvar),
    complete_new: AtomicBool,
    calls: AtomicUsize,
}

impl ChannelDownloadBackend for BatchBackend {
    fn download(
        &self,
        _account: Option<i64>,
        _chat: i64,
        message: i64,
        destination: &Path,
        observer: Arc<dyn DownloadObserver>,
    ) -> Result<(), ApplicationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.complete_new.load(Ordering::Acquire) {
            fs::write(destination, b"telegram bytes").expect("new final file");
            return Ok(());
        }
        let name = destination.file_name().expect("name").to_string_lossy();
        fs::write(
            destination.with_file_name(format!(".{name}.teleark-partial")),
            b"partial",
        )
        .expect("partial");
        fs::write(
            destination.with_file_name(format!(".{name}.teleark-partial.map")),
            b"map",
        )
        .expect("map");
        self.started
            .send((message, observer.clone()))
            .expect("started");
        let deadline = Instant::now() + Duration::from_secs(5);
        while observer.control() == DownloadControl::Continue {
            assert!(Instant::now() < deadline, "writer never received stop");
            thread::yield_now();
        }
        self.stopped.send(message).expect("stop observed");
        let (released, _) = self
            .release
            .1
            .wait_timeout_while(
                self.release.0.lock().expect("gate"),
                Duration::from_secs(5),
                |released| !*released,
            )
            .expect("wait");
        assert!(*released, "writer release timed out");
        Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
    }

    fn discard_partial(&self, destination: &Path) -> Result<(), ApplicationError> {
        teleark_telegram::discard_partial_download(destination)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))
    }
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "condition timed out");
        thread::yield_now();
    }
}

#[test]
fn stop_fences_all_48_members_before_cleanup_and_never_revives_them() {
    let directory = tempfile::tempdir().expect("fixture");
    let library = library(&directory);
    library.download_slots.set_limit(3);
    let (started, started_rx) = mpsc::channel();
    let (stopped, stopped_rx) = mpsc::channel();
    let backend = Arc::new(BatchBackend {
        started,
        stopped,
        release: (StdMutex::new(false), Condvar::new()),
        complete_new: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    let transfers = test_transfers(backend.clone(), library.clone()).expect("owner");
    let requests = (0..48)
        .map(|index| {
            let mut row = request(
                library
                    .next_download_destination(&format!("batch-{index}.bin"))
                    .expect("destination"),
            );
            row.message_id += index;
            row
        })
        .collect();
    transfers
        .enqueue_channel_download_batch(requests)
        .expect("48-file batch");
    let observers = (0..3)
        .map(|_| {
            started_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("active writer")
                .1
        })
        .collect::<Vec<_>>();
    let rows = transfers.snapshots().expect("snapshots");
    let ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    transfers
        .stop(&ids)
        .expect("durable stop acknowledges while all writers remain blocked");
    for _ in 0..3 {
        stopped_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("every writer signalled");
    }
    assert!(ids.iter().all(|id| {
        transfers
            .control(*id)
            .expect("control")
            .load(Ordering::Acquire)
            == CONTROL_CANCELLED
    }));
    assert!(
        transfers
            .snapshots()
            .expect("stopped")
            .iter()
            .all(|row| row.state == ChannelDownloadState::Cancelled)
    );
    assert!(
        library
            .native_downloads()
            .expect("saved")
            .iter()
            .all(|row| row.state == StoredNativeDownloadState::Cancelled)
    );
    assert_eq!(
        backend.calls.load(Ordering::Acquire),
        3,
        "pending members must never start"
    );
    // Stopped slot waiters and queued commands drain even while all file slots
    // remain held. Their marker cleanup must not queue behind a blocked writer.
    wait_until(|| transfers.inner.scheduled.lock().expect("scheduled").len() == 3);
    wait_until(|| {
        transfers
            .snapshots()
            .expect("view")
            .iter()
            .filter(|row| row.attempts == 0)
            .all(|row| row.cleanup.is_none())
    });
    for row in rows
        .iter()
        .filter(|row| transfers.snapshot(row.id).expect("row").attempts == 0)
    {
        assert!(!PathBuf::from(format!("{}.partial", row.destination.display())).exists());
    }
    // Stale receipts cannot change the stop decision or its displayed progress.
    for observer in observers {
        observer.progressed(14);
    }
    *backend.release.0.lock().expect("release") = true;
    backend.release.1.notify_all();
    wait_until(|| {
        transfers
            .snapshots()
            .expect("view")
            .iter()
            .all(|row| row.cleanup.is_none())
            && transfers
                .inner
                .scheduled
                .lock()
                .expect("scheduled")
                .is_empty()
    });
    let downloads = library
        .managed_directories()
        .expect("directories")
        .downloads;
    assert_eq!(
        fs::read_dir(&downloads).expect("downloads").count(),
        0,
        "all partials and reservations removed"
    );

    // A smaller batch reuses remote source identities but gets new task identities.
    backend.complete_new.store(true, Ordering::Release);
    let second = rows
        .iter()
        .take(2)
        .map(|row| {
            let mut next = request(
                library
                    .next_download_destination(&row.file_name)
                    .expect("new destination"),
            );
            next.message_id = row.message_id;
            next
        })
        .collect();
    transfers
        .enqueue_channel_download_batch(second)
        .expect("small replacement batch");
    wait_until(|| {
        transfers
            .snapshots()
            .expect("view")
            .iter()
            .filter(|row| !ids.contains(&row.id))
            .all(|row| row.state == ChannelDownloadState::Completed)
    });
    assert_eq!(backend.calls.load(Ordering::Acquire), 5);
    assert!(ids.iter().all(
        |id| transfers.snapshot(*id).expect("old row").state == ChannelDownloadState::Cancelled
    ));
    for id in &ids {
        transfers.delete(*id).expect("stopped history is deletable");
    }
    assert_eq!(
        fs::read_dir(&downloads).expect("final downloads").count(),
        2,
        "deleting stopped history preserves new final files"
    );
}

#[test]
fn stop_revokes_a_retry_while_cleanup_still_owns_the_partial() {
    let directory = tempfile::tempdir().expect("fixture");
    let library = library(&directory);
    let (started, started_rx) = mpsc::channel();
    let (stopped, stopped_rx) = mpsc::channel();
    let backend = Arc::new(BatchBackend {
        started,
        stopped,
        release: (StdMutex::new(false), Condvar::new()),
        complete_new: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    let transfers = test_transfers(backend.clone(), library.clone()).expect("owner");
    let id = transfers
        .enqueue_channel_download(request(directory.path().join("one.bin")))
        .expect("task");
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("started");
    transfers.stop(&[id]).expect("stop");
    stopped_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("stopped");
    transfers.retry(id).expect("retry while draining");
    assert!(
        transfers
            .snapshot(id)
            .expect("row")
            .cleanup
            .expect("cleanup")
            .retry_requested
    );
    transfers.stop(&[id]).expect("stop revokes pending retry");
    assert!(
        !transfers
            .snapshot(id)
            .expect("row")
            .cleanup
            .expect("cleanup")
            .retry_requested
    );
    *backend.release.0.lock().expect("release") = true;
    backend.release.1.notify_all();
    wait_until(|| transfers.snapshot(id).expect("row").cleanup.is_none());
    assert_eq!(backend.calls.load(Ordering::Acquire), 1);
    assert_eq!(
        transfers.snapshot(id).expect("row").state,
        ChannelDownloadState::Cancelled
    );
    let restored = test_transfers(backend.clone(), library).expect("restored owner");
    assert_eq!(
        restored.snapshot(id).expect("restored row").state,
        ChannelDownloadState::Cancelled
    );
    restored.activate_pending_downloads().expect("refill");
    assert_eq!(backend.calls.load(Ordering::Acquire), 1);
}
