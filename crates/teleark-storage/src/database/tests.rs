use std::collections::BTreeSet;
use std::error::Error;

use rusqlite::Connection;
use teleark_core::{
    AccountId, ChatId, CollectionId, EncryptionState, FileKind, IndexJobId, LogicalFileId,
    MessageId, PartIndex, RemoteState, TransferId, VerificationState,
};
use tempfile::tempdir;

use super::*;
use crate::error::{CursorError, InvariantViolation};
use crate::migration::{APPLICATION_ID, LATEST_SCHEMA_VERSION, MIGRATIONS};
use crate::model::{
    AccountRecord, ChatRecord, CollectionKind, CollectionRecord, FileSearchFacets, IndexBatch,
    IndexJobRecord, IndexRangeRecord, NewLogicalFileRecord, NewNativeDownloadBatchRecord,
    NewNativeDownloadTaskRecord, RemoteFileUpsert, SearchQuery, SettingRecord, StoredIndexCoverage,
    StoredIndexJobState, StoredNativeDownloadState, StoredNativeDownloadVerification,
    StoredPartState, StoredTransferDirection, StoredTransferState, TelegramIndexStateRecord,
    TransferPartCheckpoint, TransferTaskRecord, VaultMetadataRecord,
};

fn account(id: i64) -> AccountRecord {
    AccountRecord {
        id: AccountId::new(id),
        display_name: format!("account-{id}"),
        created_at_unix_ms: 1,
        updated_at_unix_ms: 1,
    }
}

fn chat(account_id: i64, id: i64) -> ChatRecord {
    ChatRecord {
        account_id: AccountId::new(account_id),
        id: ChatId::new(id),
        title: format!("channel-{id}"),
        username: None,
        updated_at_unix_ms: 1,
    }
}

fn local_file(id: u64, name: impl Into<String>, modified_at: i64) -> LogicalFileRecord {
    let name = name.into();
    LogicalFileRecord {
        id: LogicalFileId::new(id),
        extension: name
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_owned()),
        name,
        relative_path: None,
        size_bytes: 1_000 + id,
        kind: FileKind::Document,
        mime_type: None,
        caption: None,
        source_account_id: None,
        source_chat_id: None,
        created_at_unix_ms: Some(modified_at - 1),
        modified_at_unix_ms: Some(modified_at),
        remote_state: RemoteState::LocalOnly,
        encryption_state: EncryptionState::Unencrypted,
        verification_state: VerificationState::Unverified,
        package_id: None,
        locally_available: true,
        local_source_path: None,
    }
}

#[test]
fn empty_database_migrates_and_configures_connection() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("teleark.sqlite3");
    let database = Database::open(&path)?;

    assert_eq!(database.schema_version()?, LATEST_SCHEMA_VERSION);
    assert_eq!(database.journal_mode()?.to_lowercase(), "wal");
    assert!(database.foreign_keys_enabled()?);
    database.quick_check()?;
    Ok(())
}

#[test]
fn vault_metadata_round_trips_and_replaces_atomically() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    assert_eq!(database.vault_metadata()?, None);
    let mut record = VaultMetadataRecord {
        vault_id: [7; 16],
        password_wrap: vec![1; 124],
        recovery_wrap: vec![2; 88],
        password_generation: 1,
        recovery_generation: 1,
        created_at_unix_ms: 10,
        updated_at_unix_ms: 10,
    };
    database.save_vault_metadata(&record)?;
    assert_eq!(database.vault_metadata()?, Some(record.clone()));

    record.password_wrap = vec![3; 132];
    record.password_generation = 2;
    record.updated_at_unix_ms = 20;
    database.save_vault_metadata(&record)?;
    assert_eq!(database.vault_metadata()?, Some(record));
    Ok(())
}

#[test]
fn every_pre_latest_upgrade_preserves_rows_and_builds_fts() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    for starting_version in 1..LATEST_SCHEMA_VERSION {
        let path = directory
            .path()
            .join(format!("upgrade-from-{starting_version}.sqlite3"));
        {
            let connection = Connection::open(&path)?;
            connection.pragma_update(None, "foreign_keys", true)?;
            for migration in MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= starting_version)
            {
                connection.execute_batch(migration.sql)?;
                connection.pragma_update(None, "user_version", migration.version)?;
            }
            connection.pragma_update(None, "application_id", APPLICATION_ID)?;
            connection.execute(
                r#"
INSERT INTO logical_files (
    id, name, size_bytes, kind, remote_state, encryption_state,
    verification_state, locally_available
) VALUES (7, 'preserved report.pdf', 42, 'document', 'local_only',
          'unencrypted', 'unverified', 1)
"#,
                [],
            )?;
        }

        let database = Database::open(&path)?;
        assert_eq!(database.schema_version()?, LATEST_SCHEMA_VERSION);
        let preserved = database.logical_file(LogicalFileId::new(7))?;
        assert_eq!(
            preserved.as_ref().map(|file| file.name.as_str()),
            Some("preserved report.pdf")
        );
        let result = database.search_files(&SearchQuery {
            text: Some("preserved report".to_owned()),
            ..SearchQuery::default()
        })?;
        assert_eq!(result.files.len(), 1);
    }
    Ok(())
}

#[test]
fn future_schema_and_wrong_application_are_rejected() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let future_path = directory.path().join("future.sqlite3");
    {
        let connection = Connection::open(&future_path)?;
        connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        connection.pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 1)?;
    }
    assert!(matches!(
        Database::open(&future_path),
        Err(StorageError::UnsupportedSchema { .. })
    ));

    let wrong_path = directory.path().join("wrong.sqlite3");
    {
        let connection = Connection::open(&wrong_path)?;
        connection.pragma_update(None, "application_id", 123_u32)?;
    }
    assert!(matches!(
        Database::open(&wrong_path),
        Err(StorageError::WrongApplication { found: 123 })
    ));
    Ok(())
}

#[test]
fn assigned_ids_are_not_reused_and_paths_round_trip() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let first_path = std::env::temp_dir().join("TeleArk").join("one.pdf");
    let second_path = std::env::temp_dir().join("TeleArk").join("two.pdf");
    let first = database.insert_logical_file(&NewLogicalFileRecord::local_import(
        first_path,
        "one.pdf".to_owned(),
        10,
        FileKind::Document,
        Some(5),
    ))?;
    assert!(database.delete_logical_file(first.id)?);
    let second = database.insert_logical_file(&NewLogicalFileRecord::local_import(
        second_path.clone(),
        "two.pdf".to_owned(),
        20,
        FileKind::Document,
        Some(6),
    ))?;
    assert!(second.id > first.id);
    assert_eq!(
        database
            .logical_file(second.id)?
            .and_then(|file| file.local_source_path),
        Some(second_path)
    );
    Ok(())
}

#[test]
fn native_download_history_progress_and_terminal_state_round_trip() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let destination = std::env::temp_dir()
        .join("TeleArk")
        .join("restored-video.mp4");
    let mut task = database.insert_native_download(&NewNativeDownloadTaskRecord {
        account_id: 1,
        chat_id: 101,
        message_id: 202,
        message_sent_at_unix_ms: Some(9),
        file_name: "restored-video.mp4".to_owned(),
        caption: Some("A complete Telegram caption".to_owned()),
        mime_type: Some("video/mp4".to_owned()),
        size_bytes: 1_048_576,
        destination: destination.clone(),
        created_at_unix_ms: 10,
    })?;
    assert!(task.id > 0);
    assert_eq!(task.state, StoredNativeDownloadState::Queued);
    task.state = StoredNativeDownloadState::Running;
    task.transferred_bytes = 524_288;
    task.started_at_unix_ms = Some(11);
    task.queue_wait_ms = Some(1);
    task.attempts = 1;
    task.updated_at_unix_ms = 12;
    database.save_native_download(&task)?;
    task.state = StoredNativeDownloadState::Completed;
    task.verification = StoredNativeDownloadVerification::SizeChecked;
    task.transferred_bytes = task.size_bytes;
    task.finished_at_unix_ms = Some(20);
    task.duration_ms = Some(9);
    task.average_bytes_per_second = Some(116_508_444);
    task.updated_at_unix_ms = 20;
    database.save_native_download(&task)?;

    let restored = database.native_downloads()?;
    assert_eq!(restored, vec![task]);
    assert_eq!(restored[0].destination, destination);
    Ok(())
}

#[test]
fn native_download_batch_is_atomic_and_restores_message_metadata() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let task = |chat_id, message_id, name: &str| NewNativeDownloadTaskRecord {
        account_id: 1,
        chat_id,
        message_id,
        message_sent_at_unix_ms: Some(1_700_000_000_000 + message_id),
        file_name: name.to_owned(),
        caption: Some(format!("caption-{message_id}")),
        mime_type: Some("video/mp4".to_owned()),
        size_bytes: 100,
        destination: std::env::temp_dir().join("TeleArk").join(name),
        created_at_unix_ms: 10,
    };
    let tasks = vec![task(101, 201, "first.mp4"), task(101, 202, "second.mp4")];
    let (batch, inserted) = database.insert_native_download_batch(
        &NewNativeDownloadBatchRecord {
            chat_id: 101,
            created_at_unix_ms: 10,
        },
        &tasks,
    )?;
    assert_eq!(inserted.len(), 2);
    assert!(
        inserted
            .iter()
            .all(|record| record.batch_id == Some(batch.id))
    );
    assert_eq!(inserted[0].caption.as_deref(), Some("caption-201"));
    assert_eq!(database.native_downloads()?, inserted);

    let invalid = vec![task(101, 203, "third.mp4"), task(999, 204, "wrong.mp4")];
    assert!(matches!(
        database.insert_native_download_batch(
            &NewNativeDownloadBatchRecord {
                chat_id: 101,
                created_at_unix_ms: 11,
            },
            &invalid,
        ),
        Err(StorageError::InvalidInput { .. })
    ));
    assert_eq!(database.native_downloads()?, inserted);
    Ok(())
}

#[test]
fn native_download_deletion_removes_the_last_empty_batch() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let task = |message_id, name: &str| NewNativeDownloadTaskRecord {
        account_id: 1,
        chat_id: 101,
        message_id,
        message_sent_at_unix_ms: None,
        file_name: name.to_owned(),
        caption: None,
        mime_type: None,
        size_bytes: 100,
        destination: std::env::temp_dir().join("TeleArk").join(name),
        created_at_unix_ms: 10,
    };
    let (batch, inserted) = database.insert_native_download_batch(
        &NewNativeDownloadBatchRecord {
            chat_id: 101,
            created_at_unix_ms: 10,
        },
        &[task(201, "first.bin"), task(202, "second.bin")],
    )?;

    database.delete_native_download(inserted[0].id)?;
    assert_eq!(database.native_downloads()?, vec![inserted[1].clone()]);
    database.delete_native_download(inserted[1].id)?;
    assert!(database.native_downloads()?.is_empty());
    assert!(matches!(
        database.delete_native_download(inserted[1].id),
        Err(StorageError::NotFound { .. })
    ));

    let remaining_batches: i64 = database.connection.query_row(
        "SELECT COUNT(*) FROM native_download_batches WHERE id = ?1",
        [i64::try_from(batch.id)?],
        |row| row.get(0),
    )?;
    assert_eq!(remaining_batches, 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_utf8_local_path_round_trips_without_loss() -> Result<(), Box<dyn Error>> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let mut bytes = b"/tmp/TeleArk/non-utf8-".to_vec();
    bytes.push(0xff);
    let source_path = std::path::PathBuf::from(OsString::from_vec(bytes));
    let mut database = Database::open_in_memory()?;
    let inserted = database.insert_logical_file(&NewLogicalFileRecord::local_import(
        source_path.clone(),
        "opaque.bin".to_owned(),
        1,
        FileKind::Other,
        None,
    ))?;
    assert_eq!(
        database
            .logical_file(inserted.id)?
            .and_then(|file| file.local_source_path),
        Some(source_path)
    );
    Ok(())
}

#[test]
fn fts_facets_and_adversarial_input_are_safe() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let mut chinese = local_file(1, "年度報告書 旅行写真.jpg", 10);
    chinese.kind = FileKind::Image;
    chinese.caption = Some("夏の旅行 ✨".to_owned());
    chinese.extension = Some("JPG".to_owned());
    database.upsert_logical_file(&chinese)?;
    database.upsert_logical_file(&local_file(2, "ordinary notes.txt", 9))?;

    let page = database.search_files(&SearchQuery {
        text: Some("年度報告書".to_owned()),
        facets: FileSearchFacets {
            kind: Some(FileKind::Image),
            extension: Some(".jpg".to_owned()),
            ..FileSearchFacets::default()
        },
        ..SearchQuery::default()
    })?;
    assert_eq!(
        page.files.iter().map(|file| file.id).collect::<Vec<_>>(),
        vec![chinese.id]
    );

    let adversarial = database.search_files(&SearchQuery {
        text: Some("x\" OR *; DROP TABLE logical_files; --".to_owned()),
        ..SearchQuery::default()
    })?;
    assert!(adversarial.files.is_empty());
    let japanese = database.search_files(&SearchQuery {
        text: Some("夏の旅行".to_owned()),
        ..SearchQuery::default()
    })?;
    assert_eq!(japanese.files.len(), 1);
    assert_eq!(database.logical_file_count()?, 2);
    Ok(())
}

#[test]
fn fts_triggers_track_updates_and_deletes() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let mut file = local_file(1, "before.pdf", 10);
    database.upsert_logical_file(&file)?;
    file.name = "after.pdf".to_owned();
    database.upsert_logical_file(&file)?;

    let before = database.search_files(&SearchQuery {
        text: Some("before".to_owned()),
        ..SearchQuery::default()
    })?;
    let after = database.search_files(&SearchQuery {
        text: Some("after".to_owned()),
        ..SearchQuery::default()
    })?;
    assert!(before.files.is_empty());
    assert_eq!(after.files.len(), 1);
    assert!(database.delete_logical_file(file.id)?);
    assert!(
        database
            .search_files(&SearchQuery {
                text: Some("after".to_owned()),
                ..SearchQuery::default()
            })?
            .files
            .is_empty()
    );
    Ok(())
}

#[test]
fn keyset_cursor_visits_tied_rows_once_and_rejects_query_changes() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    for id in 1..=17_u64 {
        database.upsert_logical_file(&local_file(id, format!("file-{id}.txt"), 10))?;
    }

    let mut query = SearchQuery {
        limit: 4,
        ..SearchQuery::default()
    };
    let mut visited = Vec::new();
    loop {
        let page = database.search_files(&query)?;
        assert_eq!(page.total_matching, 17);
        visited.extend(page.files.iter().map(|file| file.id));
        match page.next_cursor {
            Some(cursor) => query.cursor = Some(crate::PageCursor::parse(cursor.0)?),
            None => break,
        }
    }
    assert_eq!(visited.len(), 17);
    assert_eq!(visited.iter().copied().collect::<BTreeSet<_>>().len(), 17);
    assert_eq!(visited.first(), Some(&LogicalFileId::new(17)));
    assert_eq!(visited.last(), Some(&LogicalFileId::new(1)));

    let first = database.search_files(&SearchQuery {
        limit: 1,
        ..SearchQuery::default()
    })?;
    let mismatched = database.search_files(&SearchQuery {
        facets: FileSearchFacets {
            kind: Some(FileKind::Image),
            ..FileSearchFacets::default()
        },
        cursor: first.next_cursor,
        limit: 1,
        text: None,
    });
    assert!(matches!(
        mismatched,
        Err(StorageError::InvalidCursor(CursorError::QueryMismatch))
    ));
    Ok(())
}

#[test]
fn settings_and_manual_collection_membership_persist() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    database.set_setting(&SettingRecord {
        key: "locale.override".to_owned(),
        value: "ja-JP".to_owned(),
        updated_at_unix_ms: 10,
    })?;
    assert_eq!(
        database
            .setting("locale.override")?
            .map(|value| value.value),
        Some("ja-JP".to_owned())
    );
    database.set_settings(&[
        SettingRecord {
            key: "telegram.api_id".to_owned(),
            value: "12345".to_owned(),
            updated_at_unix_ms: 11,
        },
        SettingRecord {
            key: "telegram.api_hash".to_owned(),
            value: "0123456789abcdef0123456789abcdef".to_owned(),
            updated_at_unix_ms: 11,
        },
    ])?;
    assert_eq!(
        database
            .setting("telegram.api_hash")?
            .map(|value| value.value),
        Some("0123456789abcdef0123456789abcdef".to_owned())
    );
    assert_eq!(
        database.delete_settings(&["telegram.api_id", "telegram.api_hash"])?,
        2
    );
    assert!(database.setting("telegram.api_id")?.is_none());
    assert!(database.setting("telegram.api_hash")?.is_none());

    let file = local_file(1, "manual.pdf", 10);
    database.upsert_logical_file(&file)?;
    let collection = CollectionRecord {
        id: CollectionId::new(1),
        name: "Manual".to_owned(),
        kind: CollectionKind::Manual,
        rule_version: None,
        rule_payload: None,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 1,
    };
    database.upsert_collection(&collection)?;
    assert!(database.add_collection_file(collection.id, file.id, 11)?);
    assert!(!database.add_collection_file(collection.id, file.id, 12)?);
    assert_eq!(database.collection_file_ids(collection.id)?, vec![file.id]);

    let smart = CollectionRecord {
        id: CollectionId::new(2),
        name: "Smart".to_owned(),
        kind: CollectionKind::Smart,
        rule_version: Some(1),
        rule_payload: Some("kind=document".to_owned()),
        created_at_unix_ms: 1,
        updated_at_unix_ms: 1,
    };
    database.upsert_collection(&smart)?;
    assert!(matches!(
        database.add_collection_file(smart.id, file.id, 1),
        Err(StorageError::Invariant(
            InvariantViolation::SmartCollectionMembership
        ))
    ));
    Ok(())
}

#[test]
fn settings_batch_validates_before_writing_any_row() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let oversized = "x".repeat(1_048_577);
    let result = database.set_settings(&[
        SettingRecord {
            key: "telegram.api_id".to_owned(),
            value: "12345".to_owned(),
            updated_at_unix_ms: 1,
        },
        SettingRecord {
            key: "telegram.api_hash".to_owned(),
            value: oversized,
            updated_at_unix_ms: 1,
        },
    ]);
    assert!(matches!(result, Err(StorageError::InvalidInput { .. })));
    assert!(database.setting("telegram.api_id")?.is_none());
    assert!(database.setting("telegram.api_hash")?.is_none());
    Ok(())
}

fn transfer_fixture(file_id: LogicalFileId) -> (TransferTaskRecord, Vec<TransferPartCheckpoint>) {
    let task = TransferTaskRecord {
        id: TransferId::new(8),
        logical_file_id: file_id,
        account_id: None,
        direction: StoredTransferDirection::Upload,
        priority: 5,
        state: StoredTransferState::Running,
        total_bytes: 30,
        transferred_bytes: 10,
        source_path: Some("/tmp/source.bin".to_owned()),
        destination_path: None,
        retry_count: 0,
        next_retry_at_unix_ms: None,
        last_error_code: None,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 2,
    };
    let parts = vec![
        TransferPartCheckpoint {
            transfer_id: task.id,
            index: PartIndex::new(0),
            offset_bytes: 0,
            size_bytes: 10,
            transferred_bytes: 10,
            state: StoredPartState::Transferred,
            attempts: 1,
            remote_object_id: Some(10),
            checkpoint_version: Some(1),
            checkpoint_data: Some(vec![1, 2, 3]),
            updated_at_unix_ms: 2,
        },
        TransferPartCheckpoint {
            transfer_id: task.id,
            index: PartIndex::new(1),
            offset_bytes: 10,
            size_bytes: 20,
            transferred_bytes: 0,
            state: StoredPartState::Running,
            attempts: 1,
            remote_object_id: None,
            checkpoint_version: None,
            checkpoint_data: None,
            updated_at_unix_ms: 2,
        },
    ];
    (task, parts)
}

#[test]
fn transfer_checkpoints_are_atomic_validated_and_recoverable() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let file = local_file(1, "transfer.bin", 1);
    database.upsert_logical_file(&file)?;
    let (task, parts) = transfer_fixture(file.id);
    database.save_transfer(&task, &parts)?;
    assert_eq!(
        database.transfer(task.id)?,
        Some((task.clone(), parts.clone()))
    );

    let mut broken = parts.clone();
    broken[1].offset_bytes = 11;
    assert!(matches!(
        database.save_transfer(&task, &broken),
        Err(StorageError::Invariant(
            InvariantViolation::TransferPartOffsets
        ))
    ));
    assert_eq!(database.transfer(task.id)?, Some((task.clone(), parts)));

    assert_eq!(database.recover_interrupted_work(100)?, (1, 0));
    let recovered = database
        .transfer(task.id)?
        .ok_or("missing recovered transfer")?;
    assert_eq!(recovered.0.state, StoredTransferState::Queued);
    assert_eq!(recovered.1[0].state, StoredPartState::Transferred);
    assert_eq!(recovered.1[1].state, StoredPartState::Queued);
    Ok(())
}

fn seed_account_chat(database: &mut Database) -> Result<(), StorageError> {
    database.upsert_account(&account(1))?;
    database.upsert_chat(&chat(1, 2))
}

fn index_job() -> IndexJobRecord {
    IndexJobRecord {
        id: IndexJobId::new(3),
        account_id: AccountId::new(1),
        chat_id: ChatId::new(2),
        state: StoredIndexJobState::Running,
        policy_version: 1,
        policy_fingerprint: "files-v1".to_owned(),
        requested_start_message_id: Some(MessageId::new(1)),
        requested_end_message_id: Some(MessageId::new(500)),
        checkpoint_message_id: Some(MessageId::new(100)),
        messages_scanned: 100,
        files_indexed: 1,
        last_error_code: None,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 2,
    }
}

fn index_range(start: i64, end: i64) -> IndexRangeRecord {
    IndexRangeRecord {
        account_id: AccountId::new(1),
        chat_id: ChatId::new(2),
        start_message_id: MessageId::new(start),
        end_message_id: MessageId::new(end),
        coverage: StoredIndexCoverage::Partial,
        checkpoint_message_id: Some(MessageId::new(start)),
        messages_scanned: 10,
        files_indexed: 1,
        policy_version: 1,
        policy_fingerprint: "files-v1".to_owned(),
        scan_generation: 1,
        updated_at_unix_ms: 2,
    }
}

#[test]
fn index_batch_is_atomic_and_ranges_reject_overlap() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    seed_account_chat(&mut database)?;
    let mut indexed_file = local_file(1, "indexed.pdf", 5);
    indexed_file.source_account_id = Some(AccountId::new(1));
    indexed_file.source_chat_id = Some(ChatId::new(2));
    database.apply_index_batch(&IndexBatch {
        files: vec![indexed_file.clone()],
        job: index_job(),
        range: index_range(1, 100),
    })?;
    assert_eq!(database.logical_file(indexed_file.id)?, Some(indexed_file));
    assert_eq!(
        database
            .index_ranges(AccountId::new(1), ChatId::new(2), 1, "files-v1")?
            .len(),
        1
    );

    let rolled_back = local_file(2, "must-not-commit.pdf", 6);
    let result = database.apply_index_batch(&IndexBatch {
        files: vec![rolled_back.clone()],
        job: index_job(),
        range: index_range(50, 150),
    });
    assert!(matches!(
        result,
        Err(StorageError::Invariant(
            InvariantViolation::OverlappingIndexRange
        ))
    ));
    assert_eq!(database.logical_file(rolled_back.id)?, None);
    Ok(())
}

#[test]
fn foreign_keys_reject_unscoped_source_rows() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let mut file = local_file(1, "remote.pdf", 1);
    file.source_account_id = Some(AccountId::new(99));
    file.source_chat_id = Some(ChatId::new(100));
    assert!(matches!(
        database.upsert_logical_file(&file),
        Err(StorageError::Sqlite(_))
    ));
    Ok(())
}

#[test]
fn remote_file_upsert_is_atomic_idempotent_and_revision_safe() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    seed_account_chat(&mut database)?;
    let mut incoming = RemoteFileUpsert {
        account_id: AccountId::new(1),
        chat_id: ChatId::new(2),
        message_id: MessageId::new(77),
        revision: 1,
        remote_key: 77_i64.to_be_bytes().to_vec(),
        name: "remote.pdf".to_owned(),
        size_bytes: 42,
        kind: FileKind::Document,
        mime_type: Some("application/pdf".to_owned()),
        caption: Some("source caption".to_owned()),
        sent_at_unix_ms: 900,
        modified_at_unix_ms: 1_000,
    };

    let (first_file, first_remote) = database.upsert_remote_file(&incoming)?;
    let (same_file, same_remote) = database.upsert_remote_file(&incoming)?;
    assert_eq!(same_file, first_file);
    assert_eq!(same_remote, first_remote);
    let cached = database.cached_telegram_files(incoming.account_id, incoming.chat_id, 10)?;
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].message_id, incoming.message_id);
    assert_eq!(cached[0].file_name, incoming.name);
    assert_eq!(cached[0].caption, incoming.caption);
    assert_eq!(cached[0].mime_type, incoming.mime_type);
    assert_eq!(cached[0].size_bytes, incoming.size_bytes);
    assert_eq!(cached[0].sent_at_unix_ms, incoming.sent_at_unix_ms);
    assert!(
        database
            .cached_telegram_files(incoming.account_id, incoming.chat_id, 0)
            .is_err()
    );
    assert_eq!(
        database.remote_object_by_source(
            incoming.account_id,
            incoming.chat_id,
            incoming.message_id
        )?,
        Some(first_remote.clone())
    );

    incoming.revision = 2;
    incoming.name = "renamed.pdf".to_owned();
    incoming.modified_at_unix_ms = 2_000;
    let (updated_file, updated_remote) = database.upsert_remote_file(&incoming)?;
    assert_eq!(updated_file.id, first_file.id);
    assert_eq!(updated_remote.id, first_remote.id);
    assert_eq!(updated_file.name, "renamed.pdf");

    let mut conflicting = incoming.clone();
    conflicting.remote_key = b"different".to_vec();
    assert!(matches!(
        database.upsert_remote_file(&conflicting),
        Err(StorageError::Invariant(
            InvariantViolation::RemoteRevisionConflict
        ))
    ));
    assert_eq!(database.logical_file(first_file.id)?, Some(updated_file));
    Ok(())
}

#[test]
fn search_source_identity_is_scoped_paged_and_unambiguous() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    for account_id in [1, 2] {
        database.upsert_account(&account(account_id))?;
        let mut source = chat(account_id, 7);
        source.title = format!("京都・设计稿 {account_id}");
        database.upsert_chat(&source)?;
        database.upsert_remote_file(&RemoteFileUpsert {
            account_id: AccountId::new(account_id),
            chat_id: ChatId::new(7),
            message_id: MessageId::new(77),
            revision: 1,
            remote_key: vec![1],
            name: "source.pdf".into(),
            size_bytes: 42,
            kind: FileKind::Document,
            mime_type: None,
            caption: None,
            sent_at_unix_ms: 100,
            modified_at_unix_ms: 100,
        })?;
    }
    let first = database.search_files(&SearchQuery {
        limit: 1,
        ..Default::default()
    })?;
    assert_eq!(first.files.len(), 1);
    assert_eq!(first.sources.len(), 1); // Excludes the pagination look-ahead row.
    let first_id = first.files[0].id;
    assert_eq!(first.sources[&first_id].name, "京都・设计稿 2");
    assert_eq!(
        first.sources[&first_id].message_id,
        Some(MessageId::new(77))
    );
    let second = database.search_files(&SearchQuery {
        cursor: first.next_cursor,
        limit: 1,
        ..Default::default()
    })?;
    assert_eq!(second.sources[&second.files[0].id].name, "京都・设计稿 1");
    assert_ne!(first_id, second.files[0].id);

    // A future multi-object projection must not invent one authoritative source message.
    database.connection.execute(
        "INSERT INTO remote_objects (id, logical_file_id, account_id, chat_id, message_id,
          revision, remote_key, encoded_size_bytes, modified_at_unix_ms)
         VALUES (100, ?1, 2, 7, 78, 1, X'01', 42, 100)",
        [i64::try_from(first_id.get())?],
    )?;
    let page = database.search_files(&SearchQuery::default())?;
    assert_eq!(page.sources[&first_id].message_id, None);
    assert_eq!(page.sources[&first_id].name, "京都・设计稿 2");
    Ok(())
}

#[test]
fn telegram_index_cursor_round_trips_for_restart_resume() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    seed_account_chat(&mut database)?;
    let state = TelegramIndexStateRecord {
        account_id: AccountId::new(1),
        chat_id: ChatId::new(2),
        before_message_id: Some(MessageId::new(900)),
        exhausted: false,
        messages_scanned: 1_000,
        files_indexed: 73,
        updated_at_unix_ms: 44,
    };
    database.save_telegram_index_state(&state)?;
    assert_eq!(
        database.telegram_index_state(state.account_id, state.chat_id)?,
        Some(state)
    );
    Ok(())
}

#[test]
fn legacy_download_account_resolution_is_durable_and_never_reassigns_history()
-> Result<(), Box<dyn Error>> {
    for restored in [None, Some(41)] {
        let directory = tempdir()?;
        let path = directory.path().join("v8.sqlite3");
        {
            let connection = Connection::open(&path)?;
            for migration in MIGRATIONS.iter().filter(|m| m.version <= 8) {
                connection.execute_batch(migration.sql)?;
                connection.pragma_update(None, "user_version", migration.version)?;
            }
            connection.pragma_update(None, "application_id", APPLICATION_ID)?;
            connection.execute("INSERT INTO native_download_tasks (chat_id, message_id, file_name, size_bytes, destination_path, state, verification, transferred_bytes, attempts, created_at_unix_ms, updated_at_unix_ms) VALUES (100, 200, 'legacy.zip', 14, '/tmp/legacy.zip', 'paused', 'pending', 7, 1, 10, 12)", [])?;
        }
        {
            let mut database = Database::open(&path)?;
            assert_eq!(database.native_downloads()?[0].account_id, None);
            database.resolve_legacy_native_download_accounts(restored)?;
        }
        let mut database = Database::open(&path)?;
        database.resolve_legacy_native_download_accounts(Some(99))?;
        let mut task = database.native_downloads()?.remove(0);
        assert_eq!(task.account_id, restored);
        assert_eq!(task.transferred_bytes, 7);
        if restored.is_some() {
            task.account_id = Some(99);
            assert!(database.save_native_download(&task).is_err());
            assert_eq!(database.native_downloads()?[0].account_id, restored);
        }
    }
    Ok(())
}

#[test]
fn downloaded_inventory_pages_preserve_account_scope_and_platform_paths()
-> Result<(), Box<dyn Error>> {
    use crate::{DownloadedFilesCursor, VaultDownloadRecord};
    let directory = tempdir()?;
    let path = directory.path().join("inventory.sqlite3");
    let mut db = Database::open(&path)?;
    for index in 0..140 {
        db.record_vault_download(&VaultDownloadRecord {
            account_id: 7,
            chat_id: 90,
            package_id: format!("{index:032x}"),
            destination: directory.path().join(format!("写真 {index}.bin")),
            size_bytes: 14,
            completed_at_unix_ms: index,
        })?;
    }
    db.record_vault_download(&VaultDownloadRecord {
        account_id: 8,
        chat_id: 90,
        package_id: "a".repeat(32),
        destination: directory.path().join("private.bin"),
        size_bytes: 2,
        completed_at_unix_ms: 1,
    })?;
    drop(db);
    let db = Database::open(&path)?;
    let first = db.downloaded_files_page(7, None)?;
    assert_eq!(first.len(), 128);
    assert!(
        first
            .iter()
            .all(|file| file.account_id == 7 && file.message_id.is_none())
    );
    assert!(first[0].destination.ends_with("写真 139.bin"));
    let second = db.downloaded_files_page(7, first.last().map(|file| file.cursor))?;
    assert_eq!(second.len(), 12);
    assert!(
        db.downloaded_files_page(7, second.last().map(|file| file.cursor))?
            .is_empty()
    );
    assert_eq!(db.downloaded_files_page(8, None)?.len(), 1);
    assert!(db.downloaded_files_page(0, None).is_err());
    assert!(
        db.downloaded_files_page(7, Some(DownloadedFilesCursor { kind: 2, id: 0 }))
            .is_err()
    );
    Ok(())
}

#[test]
fn completed_native_outputs_are_observed_without_rewriting_transfer_history()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let mut db = Database::open(directory.path().join("inventory.sqlite3"))?;
    let mut task = db.insert_native_download(&NewNativeDownloadTaskRecord {
        account_id: 7,
        chat_id: 90,
        message_id: 100,
        message_sent_at_unix_ms: None,
        file_name: "native.bin".into(),
        caption: None,
        mime_type: None,
        size_bytes: 14,
        destination: directory.path().join("native.bin"),
        created_at_unix_ms: 1,
    })?;
    assert!(db.downloaded_files_page(7, None)?.is_empty());
    task.state = StoredNativeDownloadState::Completed;
    task.transferred_bytes = 14;
    task.finished_at_unix_ms = Some(20);
    task.updated_at_unix_ms = 20;
    db.save_native_download(&task)?;
    let outputs = db.downloaded_files_page(7, None)?;
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].cursor.kind, 0);
    assert_eq!(outputs[0].message_id, Some(100));
    assert!(db.downloaded_files_page(8, None)?.is_empty());
    // A missing local path cannot turn completed history back into a failed transfer.
    assert_eq!(
        db.native_downloads()?[0].state,
        StoredNativeDownloadState::Completed
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn downloaded_file_v10_fixture_retains_exact_path_bytes() -> Result<(), Box<dyn Error>> {
    let mut db = Database::open_in_memory()?;
    db.connection
        .execute_batch(include_str!("../fixtures/v10-vault-download.sql"))?;
    let record = db.downloaded_files_page(7, None)?.remove(0);
    assert_eq!(record.destination, PathBuf::from("/tmp/TeleArk/京都.pdf"));
    assert_eq!(
        record.package_id.as_deref(),
        Some("5441524b504b4731000000000000002a")
    );
    let original = crate::VaultDownloadRecord {
        account_id: 7,
        chat_id: 90,
        package_id: record.package_id.clone().expect("fixture identity"),
        destination: record.destination.clone(),
        size_bytes: record.size_bytes,
        completed_at_unix_ms: record.completed_at_unix_ms,
    };
    db.record_vault_download(&original)?;
    assert_eq!(
        db.downloaded_files_page(7, None)?.len(),
        1,
        "repeated registration is idempotent"
    );
    Ok(())
}

#[test]
fn mixed_output_cursor_visits_both_kinds_once_without_deep_offsets() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let mut db = Database::open_in_memory()?;
    for index in 1..=130 {
        let mut task = db.insert_native_download(&NewNativeDownloadTaskRecord {
            account_id: 7,
            chat_id: 90,
            message_id: index,
            message_sent_at_unix_ms: None,
            file_name: format!("native {index}.bin"),
            caption: None,
            mime_type: None,
            size_bytes: 14,
            destination: directory.path().join(format!("native {index}.bin")),
            created_at_unix_ms: index,
        })?;
        task.state = StoredNativeDownloadState::Completed;
        task.transferred_bytes = 14;
        task.finished_at_unix_ms = Some(200);
        task.updated_at_unix_ms = 200;
        db.save_native_download(&task)?;
        db.record_vault_download(&crate::VaultDownloadRecord {
            account_id: 7,
            chat_id: 90,
            package_id: format!("{index:032x}"),
            destination: directory.path().join(format!("vault {index}.bin")),
            size_bytes: 14,
            completed_at_unix_ms: 200,
        })?;
    }
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut page_sizes = Vec::new();
    loop {
        let page = db.downloaded_files_page(7, cursor)?;
        if page.is_empty() {
            break;
        }
        page_sizes.push(page.len());
        cursor = page.last().map(|file| file.cursor);
        for file in page {
            assert!(seen.insert((file.cursor.kind, file.cursor.id)));
        }
    }
    assert_eq!(page_sizes, vec![128, 128, 4]);
    assert_eq!(seen.iter().filter(|(kind, _)| *kind == 0).count(), 130);
    assert_eq!(seen.iter().filter(|(kind, _)| *kind == 1).count(), 130);
    Ok(())
}
