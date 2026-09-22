use super::*;
use crate::{TelegramAccount, TelegramChatKind, TelegramChatSummary, TelegramFileSummary};
use teleark_core::FileKind;

fn seed(library: &DesktopLibrary, count: usize) {
    library
        .save_telegram_sources(
            &TelegramAccount {
                id: 1,
                display_name: "Synthetic".into(),
                username: None,
            },
            &[TelegramChatSummary {
                id: 100,
                name: "Fixture channel".into(),
                username: None,
                kind: TelegramChatKind::Channel,
                sync_pts: None,
            }],
        )
        .expect("source");
    let files = (1..=count)
        .map(|id| TelegramFileSummary {
            message_id: id as i64,
            sent_at_unix_ms: 1_000,
            modified_at_unix_ms: 1_000,
            // Only the oldest two match; neither appears in the 5,000-row browser.
            file_name: if id <= 2 {
                "../same.zip".into()
            } else {
                "document.pdf".into()
            },
            caption: "Synthetic caption".into(),
            mime_type: None,
            size_bytes: 14,
        })
        .collect::<Vec<_>>();
    library.cache_telegram_files(1, 100, &files).expect("cache");
}

fn filter() -> ChannelBatchFilter {
    ChannelBatchFilter {
        account_id: 1,
        chat_id: 100,
        earliest_unix_ms: Some(500),
        latest_unix_ms: 1_500,
        kinds: vec![FileKind::Archive],
    }
}

#[test]
fn filtered_batch_discovers_hidden_rows_uses_private_unique_folder_and_survives_restart() {
    let directory = tempfile::tempdir().expect("fixture");
    let library = library(&directory);
    seed(&library, 5_002);
    let visible = library
        .cached_telegram_files(1, 100, 5_000)
        .expect("browser page");
    assert!(visible.iter().all(|file| file.message_id > 2));
    let backend = Arc::new(FakeBackend {
        outcome: Ok(()),
        calls: StdMutex::new(Vec::new()),
    });
    let transfers = test_transfers(backend.clone(), library.clone()).expect("transfers");
    let progress = ChannelBatchPreparation::default();
    let batch = transfers
        .enqueue_filtered_channel_batch(filter(), &progress)
        .expect("batch")
        .expect("matches");
    assert_eq!(batch.count, 2);
    assert_eq!(
        batch.directory.parent(),
        Some(
            library
                .managed_directories()
                .expect("managed")
                .downloads
                .as_path()
        )
    );
    let rows = transfers.snapshots().expect("snapshots");
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .map(|row| row.message_id)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2])
    );
    assert_ne!(
        rows[0].destination, rows[1].destination,
        "duplicate names have distinct paths"
    );
    for row in &rows {
        assert_eq!(row.destination.parent(), Some(batch.directory.as_path()));
        assert_eq!(
            wait_for_terminal(&transfers, row.id).state,
            ChannelDownloadState::Completed
        );
    }
    let state = progress.snapshot().expect("feedback");
    assert_eq!(state.examined, 5_002);
    assert_eq!(
        state
            .events
            .iter()
            .map(|(phase, _)| *phase)
            .collect::<Vec<_>>(),
        vec![
            ChannelBatchPreparationPhase::Discovering,
            ChannelBatchPreparationPhase::PreparingFolder,
            ChannelBatchPreparationPhase::Queuing,
            ChannelBatchPreparationPhase::Completed,
        ]
    );
    let repeated = transfers
        .enqueue_filtered_channel_batch(filter(), &ChannelBatchPreparation::default())
        .expect("repeat")
        .expect("matches");
    assert_ne!(batch.directory, repeated.directory);
    for row in transfers.snapshots().expect("second batch") {
        wait_for_terminal(&transfers, row.id);
    }
    drop(transfers);
    let transfers = test_transfers(backend, library).expect("restart");
    let restored = transfers.snapshots().expect("durable history");
    assert!(
        restored
            .iter()
            .any(|row| row.batch_id == Some(batch.batch_id)
                && row.destination.parent() == Some(batch.directory.as_path()))
    );
}

#[test]
fn oversized_empty_cancelled_and_wrong_account_filters_admit_no_tasks() {
    let directory = tempfile::tempdir().expect("fixture");
    let library = library(&directory);
    seed(&library, 5_002);
    let backend = Arc::new(FakeBackend {
        outcome: Ok(()),
        calls: StdMutex::new(Vec::new()),
    });
    let transfers = test_transfers(backend.clone(), library.clone()).expect("transfers");
    let directories = library.managed_directories().expect("managed");
    let mut all = filter();
    all.kinds.clear();
    let progress = ChannelBatchPreparation::default();
    assert_eq!(
        transfers
            .enqueue_filtered_channel_batch(all, &progress)
            .expect_err("limit")
            .kind(),
        ApplicationErrorKind::Capacity
    );
    assert_eq!(
        progress.snapshot().expect("feedback").phase,
        ChannelBatchPreparationPhase::Failed(ApplicationErrorKind::Capacity)
    );
    let cancelled = ChannelBatchPreparation::default();
    cancelled.cancel();
    assert_eq!(
        transfers
            .enqueue_filtered_channel_batch(filter(), &cancelled)
            .expect_err("cancel")
            .kind(),
        ApplicationErrorKind::Cancelled
    );
    let mut wrong = filter();
    wrong.account_id = 2;
    assert_eq!(
        transfers
            .enqueue_filtered_channel_batch(wrong, &ChannelBatchPreparation::default())
            .expect_err("isolation")
            .kind(),
        ApplicationErrorKind::Authorization
    );
    let mut empty = filter();
    empty.earliest_unix_ms = Some(1_001);
    assert!(
        transfers
            .enqueue_filtered_channel_batch(empty, &ChannelBatchPreparation::default())
            .expect("empty")
            .is_none()
    );
    assert!(
        fs::read_dir(directories.downloads)
            .expect("directory")
            .next()
            .is_none()
    );
    assert!(transfers.snapshots().expect("history").is_empty());
    assert!(backend.calls.lock().expect("backend").is_empty());
}
