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

fn sync_file(id: i64) -> RemoteFileUpsert {
    RemoteFileUpsert {
        account_id: AccountId::new(1),
        chat_id: ChatId::new(2),
        message_id: MessageId::new(id),
        revision: 1_000,
        remote_key: id.to_be_bytes().to_vec(),
        name: format!("{id}.pdf"),
        size_bytes: 42,
        kind: FileKind::Document,
        mime_type: None,
        caption: None,
        sent_at_unix_ms: 1_000,
        modified_at_unix_ms: 1_000,
    }
}

#[test]
fn channel_changes_and_cursors_commit_together_and_preserve_local_evidence()
-> Result<(), Box<dyn Error>> {
    use crate::{ChannelSyncCommit, ChannelSyncState};
    let directory = tempdir()?;
    let path = directory.path().join("sync.sqlite3");
    let mut database = Database::open(&path)?;
    seed_account_chat(&mut database)?;
    let mut batch = ChannelSyncCommit {
        edited: vec![],
        invalidate: vec![],
        managed_catalog_seed: false,
        gap_detected: false,
        observed_at_unix_ms: 0,
        account_id: AccountId::new(1),
        chat_id: ChatId::new(2),
        expected_revision: 0,
        state: ChannelSyncState {
            pts: 50,
            revision: 1,
            history_before: Some(20),
            ..Default::default()
        },
        files: vec![sync_file(77)],
        removed: vec![],
        authoritative: true,
    };
    database.commit_channel_sync(&batch)?;
    let remote = database
        .remote_object_by_source(batch.account_id, batch.chat_id, MessageId::new(77))?
        .expect("remote");
    let mut local = database
        .logical_file(remote.logical_file_id)?
        .expect("logical file");
    local.locally_available = true;
    local.local_source_path = Some(directory.path().join("retained.pdf"));
    local.verification_state = VerificationState::Verified;
    database.upsert_logical_file(&local)?;
    // Telegram can edit twice within one timestamp second. PTS orders the
    // authoritative updates even though metadata timestamps remain equal.
    batch.expected_revision = 1;
    batch.state.revision = 2;
    batch.state.pts = 51;
    batch.files[0].name = "edited.pdf".into();
    batch.files[0].size_bytes = 43;
    database.commit_channel_sync(&batch)?;
    let edited = database.logical_file(local.id)?.expect("retained identity");
    assert_eq!(edited.name, "edited.pdf");
    assert_eq!(edited.local_source_path, local.local_source_path);
    assert_eq!(edited.verification_state, VerificationState::Unverified);
    assert!(edited.locally_available);
    // Failure after the first upsert must roll back that upsert and the cursor.
    batch.expected_revision = 2;
    batch.state.revision = 3;
    batch.state.pts = 52;
    batch.files = vec![sync_file(88), sync_file(89)];
    batch.files[1].name.clear();
    assert!(database.commit_channel_sync(&batch).is_err());
    assert!(
        database
            .remote_object_by_source(batch.account_id, batch.chat_id, MessageId::new(88))?
            .is_none()
    );
    assert_eq!(
        database
            .channel_sync_state(batch.account_id, batch.chat_id)?
            .pts,
        51
    );
    batch.files.clear();
    batch.removed = vec![77];
    database.commit_channel_sync(&batch)?;
    assert!(
        database
            .cached_telegram_files(batch.account_id, batch.chat_id, 100)?
            .is_empty()
    );
    // A later stale history response cannot resurrect a tombstoned file.
    batch.expected_revision = 3;
    batch.state.revision = 4;
    batch.authoritative = false;
    batch.files = vec![sync_file(77)];
    batch.removed.clear();
    database.commit_channel_sync(&batch)?;
    assert!(
        database
            .cached_telegram_files(batch.account_id, batch.chat_id, 100)?
            .is_empty()
    );
    assert!(database.commit_channel_sync(&batch).is_err()); // stale CAS
    drop(database);
    let database = Database::open(path)?;
    assert_eq!(
        database.channel_sync_state(batch.account_id, batch.chat_id)?,
        batch.state
    );
    assert!(
        database
            .cached_telegram_files(batch.account_id, batch.chat_id, 100)?
            .is_empty()
    );
    assert!(
        database
            .logical_file(local.id)?
            .expect("local evidence survives")
            .locally_available
    );
    assert_eq!(
        database.channel_sync_state(AccountId::new(99), batch.chat_id)?,
        ChannelSyncState::default()
    );
    Ok(())
}

#[test]
fn schema_eleven_disk_full_rolls_back_and_progress_precedes_conversion()
-> Result<(), Box<dyn Error>> {
    use crate::MigrationProgress;
    use std::cell::RefCell;
    let mut connection = Connection::open_in_memory()?;
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version <= 10)
    {
        connection.execute_batch(migration.sql)?;
        connection.pragma_update(None, "user_version", migration.version)?;
    }
    connection.execute("INSERT INTO settings(key,value,updated_at_unix_ms) VALUES('fixture-wrap.v1','retained-unlock-fixture',1)", [])?;
    let pages: i64 = connection.pragma_query_value(None, "page_count", |row| row.get(0))?;
    connection.pragma_update(None, "max_page_count", pages)?;
    let phases = RefCell::new(Vec::new());
    assert!(
        crate::migration::migrate(&mut connection, &|phase| phases.borrow_mut().push(phase))
            .is_err()
    );
    assert_eq!(
        *phases.borrow(),
        vec![
            MigrationProgress::Detecting,
            MigrationProgress::Preparing { from: 10, to: 11 },
            MigrationProgress::Converting { version: 11 }
        ]
    );
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 10);
    let value: String = connection.query_row(
        "SELECT value FROM settings WHERE key='fixture-wrap.v1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(value, "retained-unlock-fixture");
    connection.pragma_update(None, "max_page_count", pages + 100)?;
    phases.borrow_mut().clear();
    crate::migration::migrate(&mut connection, &|phase| phases.borrow_mut().push(phase))?;
    assert_eq!(phases.borrow().last(), Some(&MigrationProgress::Completed));
    assert!(
        phases
            .borrow()
            .contains(&MigrationProgress::Verifying { version: 11 })
    );
    Ok(())
}

#[test]
fn interrupted_schema_eleven_rolls_back_and_restarts_without_losing_old_data()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("v10.sqlite3");
    let mut connection = Connection::open(&path)?;
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version <= 10)
    {
        connection.execute_batch(migration.sql)?;
    }
    connection.pragma_update(None, "user_version", 10)?;
    connection.execute("INSERT INTO settings(key,value,updated_at_unix_ms) VALUES('fixture.unlock','preserve-fixture-bytes',1)", [])?;
    // Inject a deterministic DDL failure after the first statement of v11.
    connection.execute_batch("CREATE TABLE channel_sync_tombstones (fixture INTEGER)")?;
    assert!(crate::migration::migrate(&mut connection, &|_| {}).is_err());
    assert_eq!(
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))?,
        10
    );
    assert_eq!(
        connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='channel_sync_state'",
            [],
            |row| row.get::<_, i32>(0)
        )?,
        0
    );
    assert_eq!(
        connection.query_row(
            "SELECT value FROM settings WHERE key='fixture.unlock'",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "preserve-fixture-bytes"
    );
    connection.execute_batch("DROP TABLE channel_sync_tombstones")?;
    drop(connection);
    let database = Database::open(path)?;
    assert_eq!(database.schema_version()?, LATEST_SCHEMA_VERSION);
    database.quick_check()?;
    Ok(())
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
    assert!(!database.native_download_destination_in_use(&destination)?);
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
    assert!(database.native_download_destination_in_use(&destination)?);
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
    assert!(database.native_download_destination_in_use(&destination)?);
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
fn setting_compare_and_swap_rejects_stale_connections_and_promotes_legacy()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("catalog.sqlite3");
    let mut first = Database::open(&path)?;
    let mut second = Database::open(&path)?;
    first.set_setting(&SettingRecord {
        key: "binding.v1".to_owned(),
        value: "7".to_owned(),
        updated_at_unix_ms: 1,
    })?;
    assert!(first.compare_and_swap_setting(
        &SettingRecord {
            key: "binding.v2".to_owned(),
            value: "9".to_owned(),
            updated_at_unix_ms: 2,
        },
        None,
        Some(("binding.v1", "7")),
    )?);
    assert!(!second.compare_and_swap_setting(
        &SettingRecord {
            key: "binding.v2".to_owned(),
            value: "10".to_owned(),
            updated_at_unix_ms: 3,
        },
        None,
        Some(("binding.v1", "7")),
    )?);
    assert_eq!(
        second.setting("binding.v2")?.map(|record| record.value),
        Some("9".to_owned())
    );
    assert!(second.compare_and_swap_setting(
        &SettingRecord {
            key: "binding.v2".to_owned(),
            value: "11".to_owned(),
            updated_at_unix_ms: 4,
        },
        Some("9"),
        None,
    )?);
    assert!(!first.compare_and_swap_setting(
        &SettingRecord {
            key: "binding.v2".to_owned(),
            value: "12".to_owned(),
            updated_at_unix_ms: 5,
        },
        Some("9"),
        None,
    )?);
    assert_eq!(
        first.setting("binding.v2")?.map(|record| record.value),
        Some("11".to_owned())
    );
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

#[test]
fn managed_watch_tracks_same_second_edits_deletes_ack_races_and_restart()
-> Result<(), Box<dyn Error>> {
    use crate::{ChannelSyncCommit, ChannelSyncState};
    let temp = tempdir()?;
    let path = temp.path().join("watch.sqlite3");
    let mut database = Database::open(&path)?;
    seed_account_chat(&mut database)?;
    let a = AccountId::new(1);
    let c = ChatId::new(2);
    database.watch_managed_channel(a, c)?;
    let mut manifest = sync_file(77);
    manifest.caption = Some("fixture-manifest".into());
    let mut batch = ChannelSyncCommit {
        account_id: a,
        chat_id: c,
        expected_revision: 0,
        state: ChannelSyncState {
            revision: 1,
            pts: 50,
            ..Default::default()
        },
        files: vec![manifest],
        removed: vec![],
        authoritative: true,
        edited: vec![],
        invalidate: vec![],
        managed_catalog_seed: true,
        gap_detected: false,
        observed_at_unix_ms: 1_000,
    };
    let initial = database.commit_channel_sync(&batch)?;
    assert_eq!(initial.upserted.len(), 1);
    let watch = initial.managed_watch.expect("registered observation");
    assert!(watch.catalog_ready);
    assert_eq!(watch.change_count, 0, "normal upload is not an edit alert");
    let candidate = database
        .cached_manifest_candidates(a, c, "fixture-manifest", 1_001)?
        .remove(0);
    assert_eq!(candidate.revision, 1);
    // Identical verification pages preserve the derived-manifest cache version.
    batch.expected_revision += 1;
    batch.state.revision += 1;
    assert!(database.commit_channel_sync(&batch)?.upserted.is_empty());
    assert_eq!(
        database.cached_manifest_candidates(a, c, "fixture-manifest", 1_001)?[0].revision,
        1
    );
    // Explicit edits invalidate even when name/size/timestamps all compare equal.
    batch.expected_revision += 1;
    batch.state.revision += 1;
    batch.state.pts += 1;
    batch.edited = vec![77];
    let edited = database.commit_channel_sync(&batch)?;
    assert_eq!(edited.upserted.len(), 1);
    assert_eq!(edited.managed_watch.expect("watch").change_count, 1);
    assert_eq!(
        database.cached_manifest_candidates(a, c, "fixture-manifest", 1_001)?[0].revision,
        3
    );
    database.acknowledge_managed_changes(a, c, 1)?;
    batch.expected_revision += 1;
    batch.state.revision += 1;
    batch.state.pts += 1;
    batch.edited.clear();
    batch.files.clear();
    batch.removed = vec![77];
    let deleted = database.commit_channel_sync(&batch)?;
    assert_eq!(deleted.removed, vec![77]);
    assert!(
        database
            .cached_manifest_candidates(a, c, "fixture-manifest", 1_001)?
            .is_empty()
    );
    assert!(database.cached_channel_view(a, c, 100)?.1.is_empty());
    assert_eq!(
        database
            .acknowledge_managed_changes(a, c, 1)?
            .unacknowledged(),
        1,
        "a late acknowledgement cannot hide a newer change"
    );
    assert_eq!(
        database
            .managed_channel_watch(AccountId::new(9), c)?
            .change_count,
        0
    );
    drop(database);
    let database = Database::open(&path)?;
    let watch = database.managed_channel_watch(a, c)?;
    assert_eq!((watch.change_count, watch.acknowledged_count), (2, 1));
    assert_eq!(
        watch.changes[0].kind,
        crate::ManagedChannelChangeKind::Deleted
    );
    assert_eq!(database.channel_sync_state(a, c)?.pts, 52);
    Ok(())
}

#[test]
fn managed_watch_journal_is_bounded_and_failed_commit_leaves_observations_intact()
-> Result<(), Box<dyn Error>> {
    use crate::{ChannelSyncCommit, ChannelSyncState};
    let temp = tempdir()?;
    let mut database = Database::open(temp.path().join("watch.sqlite3"))?;
    seed_account_chat(&mut database)?;
    let a = AccountId::new(1);
    let c = ChatId::new(2);
    database.watch_managed_channel(a, c)?;
    let mut batch = ChannelSyncCommit {
        account_id: a,
        chat_id: c,
        expected_revision: 0,
        state: ChannelSyncState {
            revision: 1,
            pts: 1,
            ..Default::default()
        },
        files: vec![],
        removed: vec![1],
        authoritative: true,
        edited: vec![],
        invalidate: vec![],
        managed_catalog_seed: false,
        gap_detected: false,
        observed_at_unix_ms: 1_000,
    };
    for id in 1..=150 {
        batch.removed = vec![id];
        database.commit_channel_sync(&batch)?;
        batch.expected_revision += 1;
        batch.state.revision += 1;
        batch.state.pts += 1;
    }
    let watch = database.managed_channel_watch(a, c)?;
    assert_eq!(
        (
            watch.change_count,
            watch.changes.len(),
            watch.omitted_changes()
        ),
        (150, 128, 22)
    );
    batch.removed.clear();
    batch.files = vec![sync_file(500), sync_file(501)];
    batch.files[1].name.clear();
    batch.edited = vec![500];
    batch.gap_detected = true;
    assert!(database.commit_channel_sync(&batch).is_err());
    assert_eq!(database.managed_channel_watch(a, c)?, watch);
    assert_eq!(database.channel_sync_state(a, c)?.revision, 150);
    assert!(database.cached_channel_view(a, c, 100)?.1.is_empty());
    Ok(())
}

#[test]
fn interrupted_schema_twelve_preserves_eleven_and_restarts_atomically() -> Result<(), Box<dyn Error>>
{
    let temp = tempdir()?;
    let path = temp.path().join("v11.sqlite3");
    {
        let connection = Connection::open(&path)?;
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 11)
        {
            connection.execute_batch(migration.sql)?;
        }
        connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        connection.pragma_update(None, "user_version", 11)?;
        connection.execute(
            "INSERT INTO settings(key,value,updated_at_unix_ms) VALUES('fixture','preserve',1)",
            [],
        )?;
        // A deterministic failure after the first v12 DDL must roll back all v12 work.
        connection.execute("CREATE TABLE managed_channel_watches(fixture TEXT)", [])?;
    }
    assert!(Database::open(&path).is_err());
    {
        let connection = Connection::open(&path)?;
        assert_eq!(
            connection.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?,
            11
        );
        assert_eq!(
            connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='channel_file_versions'",
                [],
                |row| row.get::<_, u32>(0)
            )?,
            0
        );
        assert_eq!(
            connection.query_row(
                "SELECT value FROM settings WHERE key='fixture'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            "preserve"
        );
        connection.execute("DROP TABLE managed_channel_watches", [])?;
    }
    let database = Database::open(&path)?;
    assert_eq!(
        database
            .setting("fixture")?
            .expect("preserved setting")
            .value,
        "preserve"
    );
    Ok(())
}

#[test]
fn stale_history_cannot_replace_pushes_and_gap_validation_invalidates_without_false_edits()
-> Result<(), Box<dyn Error>> {
    use crate::{ChannelSyncCommit, ChannelSyncState};
    let temp = tempdir()?;
    let mut database = Database::open(temp.path().join("projection.sqlite3"))?;
    seed_account_chat(&mut database)?;
    let a = AccountId::new(1);
    let c = ChatId::new(2);
    database.watch_managed_channel(a, c)?;
    let mut file = sync_file(77);
    file.caption = Some("manifest".into());
    file.name = "current-name".into();
    let mut batch = ChannelSyncCommit {
        account_id: a,
        chat_id: c,
        expected_revision: 0,
        state: ChannelSyncState {
            pts: 50,
            revision: 1,
            ..Default::default()
        },
        files: vec![file.clone()],
        removed: vec![],
        edited: vec![],
        invalidate: vec![],
        authoritative: true,
        managed_catalog_seed: true,
        gap_detected: false,
        observed_at_unix_ms: 1_000,
    };
    database.commit_channel_sync(&batch)?;
    batch.expected_revision += 1;
    batch.state.revision += 1;
    batch.authoritative = false;
    batch.files[0].name = "old-search-result".into();
    assert!(database.commit_channel_sync(&batch)?.upserted.is_empty());
    assert_eq!(
        database.cached_channel_view(a, c, 100)?.1[0].file_name,
        "current-name"
    );
    batch.expected_revision += 1;
    batch.state.revision += 1;
    batch.authoritative = true;
    batch.files = vec![file];
    batch.invalidate = vec![77];
    let result = database.commit_channel_sync(&batch)?;
    assert_eq!(result.upserted.len(), 1);
    assert_eq!(result.managed_watch.expect("watch").change_count, 0);
    assert_eq!(
        database.cached_manifest_candidates(a, c, "manifest", 100)?[0].revision,
        3
    );
    Ok(())
}

#[test]
fn search_effective_timestamp_indexes_seek_and_preserve_cursor_order() -> Result<(), Box<dyn Error>>
{
    let database = Database::open_in_memory()?;
    database.connection.execute_batch("WITH RECURSIVE seq(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM seq WHERE x<1003)
    INSERT INTO logical_files(id,name,size_bytes,kind,remote_state,encryption_state,verification_state,created_at_unix_ms,modified_at_unix_ms)
    SELECT x,'fixture-'||x,1,CASE WHEN x%3=0 THEN 'archive' ELSE 'document' END,'local_only','unencrypted','unverified',CASE WHEN x%5=0 THEN NULL ELSE x/10 END,CASE WHEN x%3=0 THEN x/10 ELSE NULL END FROM seq;")?;
    let expression = "COALESCE(modified_at_unix_ms, created_at_unix_ms, -9223372036854775808)";
    for (facet, index) in [
        ("", "logical_files_effective_keyset"),
        (
            "source_account_id=1 AND",
            "logical_files_account_effective_keyset",
        ),
        (
            "source_account_id=1 AND source_chat_id=2 AND",
            "logical_files_source_effective_keyset",
        ),
        ("kind='archive' AND", "logical_files_kind_effective_keyset"),
    ] {
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT id FROM logical_files WHERE {facet} {expression} <= 50 AND ({expression},id)<(50,500) ORDER BY {expression} DESC,id DESC LIMIT 25"
        );
        let plans = database
            .connection
            .prepare(&sql)?
            .query_map([], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            plans
                .iter()
                .any(|p| p.contains("SEARCH") && p.contains(index)),
            "{plans:?}"
        );
        assert!(
            !plans.iter().any(|p| p.contains("TEMP B-TREE")),
            "{plans:?}"
        );
    }
    for kind in [None, Some(FileKind::Archive)] {
        let filter = if kind.is_some() {
            "WHERE kind='archive'"
        } else {
            ""
        };
        let expected = database
            .connection
            .prepare(&format!(
                "SELECT id FROM logical_files {filter} ORDER BY {expression} DESC,id DESC"
            ))?
            .query_map([], |row| {
                row.get::<_, i64>(0)
                    .map(|id| u64::try_from(id).expect("positive fixture ID"))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut query = SearchQuery {
            facets: FileSearchFacets {
                kind,
                ..Default::default()
            },
            limit: 17,
            ..Default::default()
        };
        let mut found = Vec::new();
        loop {
            let page = database.search_files(&query)?;
            assert_eq!(page.total_matching, expected.len() as u64);
            found.extend(page.files.into_iter().map(|file| file.id.get()));
            let Some(cursor) = page.next_cursor else {
                break;
            };
            query.cursor = Some(cursor);
        }
        assert_eq!(found, expected);
    }
    Ok(())
}

#[test]
fn schema_thirteen_index_failure_rolls_back_and_preserves_keys_on_restart()
-> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let path = temp.path().join("v12.sqlite3");
    let mut connection = Connection::open(&path)?;
    for migration in MIGRATIONS.iter().filter(|m| m.version <= 12) {
        connection.execute_batch(migration.sql)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 12)?;
    connection.execute_batch(
        "INSERT INTO settings VALUES('fixture','preserved',1);
        INSERT INTO vault_metadata VALUES(1,zeroblob(16),zeroblob(132),zeroblob(88),7,9,1,2);
        CREATE INDEX logical_files_account_effective_keyset ON logical_files(name);",
    )?;
    assert!(crate::migration::migrate(&mut connection, &|_| {}).is_err());
    assert_eq!(
        connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
        12
    );
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='logical_files_effective_keyset'",
            [],
            |r| r.get::<_, u32>(0)
        )?,
        0
    );
    connection.execute_batch("DROP INDEX logical_files_account_effective_keyset")?;
    // Index rebuilding must also fail atomically when SQLite cannot allocate pages.
    let pages: i64 = connection.pragma_query_value(None, "page_count", |r| r.get(0))?;
    connection.pragma_update(None, "max_page_count", pages)?;
    let phases = std::cell::RefCell::new(Vec::new());
    assert!(
        crate::migration::migrate(&mut connection, &|phase| phases.borrow_mut().push(phase))
            .is_err()
    );
    assert_eq!(
        *phases.borrow(),
        vec![
            crate::MigrationProgress::Detecting,
            crate::MigrationProgress::Preparing { from: 12, to: 13 },
            crate::MigrationProgress::Converting { version: 13 }
        ]
    );
    assert_eq!(
        connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
        12
    );
    connection.pragma_update(None, "max_page_count", pages + 100)?;
    drop(connection);
    let database = Database::open(&path)?;
    assert_eq!(database.schema_version()?, LATEST_SCHEMA_VERSION);
    assert_eq!(
        database.setting("fixture")?.expect("setting").value,
        "preserved"
    );
    let keys = database.vault_metadata()?.expect("wrapped fixture keys");
    assert_eq!(keys.password_wrap, vec![0; 132]);
    assert_eq!(keys.recovery_wrap, vec![0; 88]);
    assert_eq!((keys.password_generation, keys.recovery_generation), (7, 9));
    database.quick_check()?;
    drop(database);
    assert_eq!(
        Database::open(path)?.schema_version()?,
        LATEST_SCHEMA_VERSION
    );
    Ok(())
}

#[test]
#[ignore = "manual SQLite query comparison, no wall-time pass threshold"]
fn perf_library_search_indexes() -> Result<(), Box<dyn Error>> {
    let database = Database::open_in_memory()?;
    database.connection.execute_batch("DROP INDEX logical_files_effective_keyset;
        DROP INDEX logical_files_account_effective_keyset; DROP INDEX logical_files_source_effective_keyset; DROP INDEX logical_files_kind_effective_keyset;
        WITH RECURSIVE seq(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM seq WHERE x<100000)
        INSERT INTO logical_files(id,name,size_bytes,kind,remote_state,encryption_state,verification_state,created_at_unix_ms,modified_at_unix_ms)
        SELECT x,'fixture-'||x,1,'document','local_only','unencrypted','unverified',x,CASE WHEN x%3=0 THEN NULL ELSE x END FROM seq;")?;
    let mut query = SearchQuery {
        limit: 100,
        ..Default::default()
    };
    let page = database.search_files(&query)?;
    let cursor = page.next_cursor.expect("more rows");
    let fingerprint = cursor
        .as_str()
        .rsplit(':')
        .next()
        .expect("cursor fingerprint");
    query.cursor = Some(crate::PageCursor::parse(format!(
        "ta1:{:016x}:{:016x}:{fingerprint}",
        50_000_u64, 50_000_u64
    ))?);
    let before = std::time::Instant::now();
    for _ in 0..100 {
        std::hint::black_box(database.search_files(&query)?);
    }
    let old_us = before.elapsed().as_micros();
    database.connection.execute_batch(
        MIGRATIONS
            .iter()
            .find(|m| m.version == 13)
            .expect("index migration")
            .sql,
    )?;
    let after = std::time::Instant::now();
    for _ in 0..100 {
        std::hint::black_box(database.search_files(&query)?);
    }
    let indexed_us = after.elapsed().as_micros();
    let result = database.search_files(&query)?;
    assert_eq!(result.total_matching, 100_000);
    assert_eq!(result.files.len(), 100);
    assert_eq!(result.files[0].id.get(), 49_999);
    eprintln!(
        "SQLite {} full search: 100 deep page reads in 100000 rows, missing-index_us={old_us}, indexed_us={indexed_us}; exact COUNT still runs per page",
        rusqlite::version()
    );
    Ok(())
}

#[test]
fn search_counts_remain_exact_after_local_and_external_writes() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let path = temp.path().join("counts.sqlite");
    let database = Database::open(&path)?;
    let external = Database::open(&path)?;
    let query = SearchQuery::default();
    assert_eq!(database.search_files(&query)?.total_matching, 0);
    external.connection.execute_batch("INSERT INTO logical_files(id,name,size_bytes,kind,remote_state,encryption_state,verification_state)
        VALUES(1,'document',1,'document','local_only','unencrypted','unverified'),
        (2,'archive',1,'archive','local_only','unencrypted','unverified')")?;
    assert_eq!(database.search_files(&query)?.total_matching, 2);
    let filtered = SearchQuery {
        facets: FileSearchFacets {
            kind: Some(FileKind::Archive),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(database.search_files(&filtered)?.total_matching, 1);
    database
        .connection
        .execute_batch("DELETE FROM logical_files WHERE id=2")?;
    assert_eq!(database.search_files(&query)?.total_matching, 1);
    assert_eq!(database.search_files(&filtered)?.total_matching, 0);
    external
        .connection
        .execute_batch("BEGIN; DELETE FROM logical_files; ROLLBACK;")?;
    assert_eq!(database.search_files(&query)?.total_matching, 1);
    Ok(())
}

#[test]
#[ignore = "manual exact COUNT comparison, no wall-time pass threshold"]
fn perf_library_count() -> Result<(), Box<dyn Error>> {
    let database = Database::open_in_memory()?;
    database.connection.execute_batch("WITH RECURSIVE seq(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM seq WHERE x<100000)
        INSERT INTO logical_files(id,name,size_bytes,kind,remote_state,encryption_state,verification_state)
        SELECT x,'fixture-'||x,1,'document','local_only','unencrypted','unverified' FROM seq;")?;
    for sql in [
        "SELECT COUNT(*) FROM logical_files f WHERE 1 = 1",
        "SELECT COUNT(*) FROM logical_files f",
    ] {
        let started = std::time::Instant::now();
        for _ in 0..100 {
            let count: i64 = database.connection.query_row(sql, [], |r| r.get(0))?;
            assert_eq!(count, 100_000);
            std::hint::black_box(count);
        }
        eprintln!(
            "100 exact counts, 100000 rows: {} us; SQL={sql}",
            started.elapsed().as_micros()
        );
    }
    Ok(())
}

#[test]
fn sampled_download_checkpoints_cannot_revive_retired_attempts() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let mut task = database.insert_native_download(&NewNativeDownloadTaskRecord {
        account_id: 1,
        chat_id: 2,
        message_id: 3,
        message_sent_at_unix_ms: None,
        file_name: "fixture".into(),
        caption: None,
        mime_type: None,
        size_bytes: 100,
        destination: std::env::temp_dir().join("checkpoint-fixture"),
        created_at_unix_ms: 1,
    })?;
    task.state = StoredNativeDownloadState::Running;
    task.attempts = 1;
    database.save_native_download(&task)?;
    let mut sample = task.clone();
    sample.transferred_bytes = 50;
    sample.duration_ms = Some(10);
    sample.updated_at_unix_ms = 2;
    assert!(database.save_native_download_progress(&sample)?);
    assert_eq!(database.native_downloads()?[0].transferred_bytes, 50);
    assert!(!database.save_native_download_progress(&task)?); // earlier bytes/time
    let mut wrong_account = sample.clone();
    wrong_account.account_id = Some(99);
    assert!(!database.save_native_download_progress(&wrong_account)?);
    for state in [
        StoredNativeDownloadState::Paused,
        StoredNativeDownloadState::Cancelled,
        StoredNativeDownloadState::Failed,
        StoredNativeDownloadState::Completed,
        StoredNativeDownloadState::Queued,
    ] {
        task.state = state;
        database.save_native_download(&task)?;
        assert!(!database.save_native_download_progress(&sample)?);
        assert_eq!(database.native_downloads()?[0], task);
    }
    task.state = StoredNativeDownloadState::Running;
    task.attempts = 2;
    database.save_native_download(&task)?;
    assert!(!database.save_native_download_progress(&sample)?);
    assert_eq!(database.native_downloads()?[0], task);
    Ok(())
}

#[test]
fn native_history_restores_old_recoverable_work_and_discloses_retired_omissions()
-> Result<(), Box<dyn Error>> {
    let database = Database::open_in_memory()?;
    database.connection.execute_batch("WITH RECURSIVE seq(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM seq WHERE x<10020)
        INSERT INTO native_download_tasks(id,account_id,chat_id,message_id,file_name,size_bytes,destination_path,state,verification,transferred_bytes,created_at_unix_ms,updated_at_unix_ms)
        SELECT x,1,2,x,'fixture-'||x,42,'/fixture/'||x,'completed','size_checked',7,x,x FROM seq;
        UPDATE native_download_tasks SET state='queued' WHERE id=1;
        UPDATE native_download_tasks SET state='paused' WHERE id=2;
        UPDATE native_download_tasks SET state='failed' WHERE id=3;
        UPDATE native_download_tasks SET state='running' WHERE id=4;
        UPDATE native_download_tasks SET state='cancelled' WHERE id=5;
        INSERT INTO native_download_batches(id,chat_id,created_at_unix_ms) VALUES(1,2,1);
        UPDATE native_download_tasks SET batch_id=1 WHERE id IN(1,6);")?;
    let (tasks, omitted) = database.native_download_history()?;
    let ids: BTreeSet<_> = tasks.iter().map(|task| task.id).collect();
    assert_eq!(tasks.len(), NATIVE_DOWNLOAD_HISTORY_LIMIT);
    assert_eq!(omitted, 20);
    for id in [1, 2, 3, 4, 6, 10020] {
        assert!(ids.contains(&id));
    }
    assert!(!ids.contains(&5));
    assert!(
        tasks
            .iter()
            .all(|task| task.account_id == Some(1) && task.transferred_bytes == 7)
    );
    // Old databases may already exceed the current admission limit. Recovery
    // must preserve those records rather than silently truncating their queue.
    database
        .connection
        .execute_batch("UPDATE native_download_tasks SET state='queued'")?;
    let (tasks, omitted) = database.native_download_history()?;
    assert_eq!(tasks.len(), 10020);
    assert_eq!(omitted, 0);
    assert!(tasks.iter().all(|task| task.transferred_bytes == 7));
    Ok(())
}

#[test]
fn proxy_policy_migration_is_transactional_restartable_and_fences_old_readers()
-> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let path = temp.path().join("v13-proxy.sqlite3");
    let mut connection = Connection::open(&path)?;
    for migration in MIGRATIONS.iter().filter(|m| m.version <= 13) {
        connection.execute_batch(migration.sql)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 13)?;
    connection.execute_batch(
        "INSERT INTO settings VALUES('preserved','value',1);
        CREATE TRIGGER block_proxy_policy BEFORE INSERT ON settings WHEN NEW.key = 'network.proxy'
        BEGIN SELECT RAISE(ABORT, 'controlled persistence failure'); END;",
    )?;
    let phases = std::cell::RefCell::new(Vec::new());
    assert!(
        crate::migration::migrate(&mut connection, &|phase| phases.borrow_mut().push(phase))
            .is_err()
    );
    assert_eq!(
        connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
        13
    );
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM settings WHERE key='network.proxy'",
            [],
            |r| r.get::<_, u32>(0)
        )?,
        0
    );
    assert_eq!(
        *phases.borrow(),
        vec![
            crate::MigrationProgress::Detecting,
            crate::MigrationProgress::Preparing { from: 13, to: 14 },
            crate::MigrationProgress::Converting { version: 14 }
        ]
    );
    connection.execute_batch("DROP TRIGGER block_proxy_policy")?;
    drop(connection);
    let database = Database::open(&path)?;
    assert_eq!(database.schema_version()?, LATEST_SCHEMA_VERSION); // readers supporting <=13 reject this database
    assert_eq!(
        database
            .setting("network.proxy")?
            .expect("migration fixture record")
            .value,
        r#"{"version":1,"mode":"direct"}"#
    );
    assert_eq!(
        database
            .setting("preserved")?
            .expect("migration fixture record")
            .value,
        "value"
    );
    database.quick_check()?;
    drop(database);
    assert_eq!(
        Database::open(&path)?
            .setting("network.proxy")?
            .expect("migration fixture record")
            .value,
        r#"{"version":1,"mode":"direct"}"#
    );
    Ok(())
}

#[test]
fn key_epoch_migration_preserves_wrappers_across_skipped_upgrades_and_restart()
-> Result<(), Box<dyn Error>> {
    for version in [8, 11, 14] {
        let temp = tempdir()?;
        let path = temp.path().join("keys.sqlite3");
        let connection = Connection::open(&path)?;
        for migration in MIGRATIONS.iter().filter(|m| m.version <= version) {
            connection.execute_batch(migration.sql)?;
        }
        connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        connection.pragma_update(None, "user_version", version)?;
        connection.execute_batch(
            "INSERT INTO vault_metadata VALUES(1,zeroblob(16),zeroblob(132),zeroblob(88),7,9,1,2)",
        )?;
        drop(connection);
        let database = Database::open(&path)?;
        let active = database.vault_metadata()?.expect("active");
        assert_eq!(database.vault_key_epoch([0; 16])?, Some(active.clone()));
        assert_eq!(database.schema_version()?, LATEST_SCHEMA_VERSION);
        drop(database);
        let database = Database::open(&path)?;
        assert_eq!(database.vault_metadata()?, Some(active.clone()));
        assert_eq!(database.vault_key_epoch([0; 16])?, Some(active));
    }
    Ok(())
}

#[test]
fn interrupted_key_migration_rolls_back_then_restarts() -> Result<(), Box<dyn Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in MIGRATIONS.iter().filter(|m| m.version <= 14) {
        connection.execute_batch(migration.sql)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 14)?;
    connection.execute_batch("INSERT INTO vault_metadata VALUES(1,zeroblob(16),zeroblob(132),zeroblob(88),7,9,1,2); CREATE TABLE vault_inventory(blocker TEXT)")?;
    let phases = std::cell::RefCell::new(Vec::new());
    assert!(crate::migration::migrate(&mut connection, &|p| phases.borrow_mut().push(p)).is_err());
    assert_eq!(
        connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
        14
    );
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='vault_key_epochs'",
            [],
            |r| r.get::<_, u32>(0)
        )?,
        0
    );
    assert_eq!(
        connection.query_row("SELECT password_generation FROM vault_metadata", [], |r| {
            r.get::<_, u32>(0)
        })?,
        7
    );
    assert!(
        phases
            .borrow()
            .iter()
            .position(|p| matches!(p, crate::MigrationProgress::Preparing { to: 15, .. }))
            .expect("preparing")
            < phases
                .borrow()
                .iter()
                .position(|p| matches!(p, crate::MigrationProgress::Converting { version: 15 }))
                .expect("conversion")
    );
    connection.execute_batch("DROP TABLE vault_inventory")?;
    crate::migration::migrate(&mut connection, &|_| {})?;
    assert_eq!(
        connection.query_row(
            "SELECT recovery_generation FROM vault_key_epochs",
            [],
            |r| r.get::<_, u32>(0)
        )?,
        9
    );
    Ok(())
}

#[test]
fn new_key_epoch_is_atomic_and_keeps_old_wrapped_keys() -> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let old = VaultMetadataRecord {
        vault_id: [1; 16],
        password_wrap: vec![2; 132],
        recovery_wrap: vec![3; 88],
        password_generation: 2,
        recovery_generation: 3,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 2,
    };
    database.save_vault_metadata(&old)?;
    let mut new = old.clone();
    new.vault_id = [4; 16];
    new.password_wrap = vec![5; 132];
    database.connection.execute_batch("CREATE TRIGGER stop_epoch BEFORE UPDATE ON vault_metadata BEGIN SELECT RAISE(ABORT,'controlled write failure'); END")?;
    assert!(database.save_vault_metadata(&new).is_err());
    assert_eq!(database.vault_metadata()?, Some(old.clone()));
    assert_eq!(database.vault_key_epoch(new.vault_id)?, None);
    database
        .connection
        .execute_batch("DROP TRIGGER stop_epoch")?;
    database.save_vault_metadata(&new)?;
    assert_eq!(database.vault_key_epoch(old.vault_id)?, Some(old));
    assert_eq!(database.vault_metadata()?, Some(new.clone()));
    assert_eq!(database.vault_key_epoch(new.vault_id)?, Some(new));
    Ok(())
}

#[test]
fn encrypted_inventory_survives_replacement_and_pages_by_account_and_message()
-> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let path = temp.path().join("inventory.sqlite3");
    let mut database = Database::open(&path)?;
    for account in [1, 2] {
        for id in 1..=80 {
            database.save_vault_inventory(&crate::VaultInventoryRecord {
                account_id: account,
                chat_id: 3,
                manifest_message_id: id,
                remote_name: format!("{id}.tarkm"),
                vault_id: [4; 16],
                sealed_manifest: vec![5; 100],
                observed_at_unix_ms: 1,
                manifest_invalid: false,
            })?;
        }
    }
    database.save_vault_inventory(&crate::VaultInventoryRecord {
        account_id: 1,
        chat_id: 3,
        manifest_message_id: 80,
        remote_name: "replacement".into(),
        vault_id: [9; 16],
        sealed_manifest: vec![6; 100],
        observed_at_unix_ms: 2,
        manifest_invalid: false,
    })?;
    drop(database);
    let database = Database::open(&path)?;
    let first = database.vault_inventory_page(1, 3, i64::MAX, 1)?.remove(0);
    assert_eq!(first.vault_id, [4; 16]);
    assert_eq!(first.sealed_manifest, vec![5; 100]);
    let deep = database.vault_inventory_page(1, 3, 21, 10)?;
    assert_eq!(
        deep.iter()
            .map(|record| record.manifest_message_id)
            .collect::<Vec<_>>(),
        (11..=20).rev().collect::<Vec<_>>()
    );
    assert!(database.vault_inventory_page(1, 4, i64::MAX, 1)?.is_empty());
    assert!(database.vault_inventory_page(9, 3, i64::MAX, 1)?.is_empty());
    let plan: Vec<String> = database.connection.prepare("EXPLAIN QUERY PLAN SELECT manifest_message_id,remote_name,vault_id,sealed_manifest,observed_at FROM vault_inventory WHERE account_id=?1 AND chat_id=?2 AND manifest_message_id<?3 ORDER BY manifest_message_id DESC LIMIT ?4")?.query_map(params![1,3,21,10], |row| row.get(3))?.collect::<Result<_,_>>()?;
    assert!(
        plan.iter().any(
            |line| line.contains("SEARCH vault_inventory USING PRIMARY KEY")
                && line.contains("manifest_message_id<?")
        ),
        "{plan:?}"
    );
    assert!(
        plan.iter().all(|line| !line.contains("TEMP B-TREE")),
        "{plan:?}"
    );
    Ok(())
}

#[test]
fn file_health_keeps_unknown_distinct_and_isolates_deleted_messages() -> Result<(), Box<dyn Error>>
{
    use crate::VaultFileHealth as H;
    let temp = tempdir()?;
    let path = temp.path().join("health.sqlite3");
    let mut database = Database::open(&path)?;
    assert_eq!(
        database.vault_message_health(1, 2, &[10, 11, 12])?,
        vec![H::Unchecked; 3]
    );
    database.save_vault_message_health(1, 2, &[(10, true), (11, false), (12, true)], 1)?;
    assert_eq!(
        database.vault_message_health(1, 2, &[10, 11, 12, 13])?,
        vec![H::Present, H::MissingParts, H::Present, H::Unchecked]
    );
    assert_eq!(
        database.vault_message_health(9, 2, &[11])?,
        vec![H::Unchecked]
    );
    assert_eq!(
        database.vault_message_health(1, 9, &[11])?,
        vec![H::Unchecked]
    );
    assert!(
        database
            .save_vault_message_health(1, 2, &vec![(14, true); 101], 2)
            .is_err()
    );
    assert_eq!(
        database.vault_message_health(1, 2, &[14])?,
        vec![H::Unchecked]
    );
    drop(database);
    let database = Database::open(&path)?;
    assert_eq!(
        database.vault_message_health(1, 2, &[10, 11, 12])?,
        vec![H::Present, H::MissingParts, H::Present]
    );
    Ok(())
}

#[test]
fn stale_key_revision_cannot_overwrite_a_new_epoch_or_password_rotation()
-> Result<(), Box<dyn Error>> {
    let mut database = Database::open_in_memory()?;
    let old = VaultMetadataRecord {
        vault_id: [1; 16],
        password_wrap: vec![2; 132],
        recovery_wrap: vec![3; 88],
        password_generation: 1,
        recovery_generation: 1,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 1,
    };
    assert!(database.save_vault_metadata_checked(&old, None)?);
    let expected = Some((old.vault_id, 1, 1));
    let mut new = old.clone();
    new.vault_id = [4; 16];
    assert!(database.save_vault_metadata_checked(&new, expected)?);
    let mut stale = old.clone();
    stale.password_generation = 2;
    assert!(!database.save_vault_metadata_checked(&stale, expected)?);
    assert_eq!(database.vault_metadata()?, Some(new.clone()));
    assert_eq!(database.vault_key_epoch(old.vault_id)?, Some(old));
    let revision = Some((new.vault_id, 1, 1));
    new.password_generation = 2;
    assert!(database.save_vault_metadata_checked(&new, revision)?);
    assert!(!database.save_vault_metadata_checked(&new, revision)?);
    Ok(())
}
