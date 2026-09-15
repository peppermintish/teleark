use super::*;
use crate::migration::{APPLICATION_ID, MIGRATIONS};

fn job(id: u64, direction: VaultJobDirection) -> VaultJobRecord {
    VaultJobRecord {
        account_id: 7,
        id,
        chat_id: 11,
        direction,
        package_id: [id as u8; 16],
        context_version: 1,
        context: vec![1, 2, 3],
        state: VaultJobState::Queued,
        generation: 0,
        created_at_unix_ms: 100,
        updated_at_unix_ms: 100,
        failure_code: None,
    }
}
fn lease(id: u64, generation: u64) -> VaultJobLease {
    VaultJobLease {
        account_id: 7,
        id,
        generation,
    }
}
fn step(
    db: &mut Database,
    id: u64,
    generation: u64,
    from: VaultJobState,
    action: VaultJobTransition,
) -> StorageResult<bool> {
    db.transition_vault_job(lease(id, generation), from, action, 101, None)
}

#[test]
fn remote_import_is_atomic_restartable_and_cannot_promote_after_pause() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    let job = job(9, VaultJobDirection::Upload);
    let pending = PendingVaultUpload {
        account_id: 7,
        id: 9,
        chat_id: 11,
        batch_id: 9,
        created_at_unix_ms: 100,
        codec_version: 1,
        context: vec![1, 2, 3],
    };
    db.admit_pending_vault_uploads(std::slice::from_ref(&pending))?;
    let parts = [VaultPartRecord {
        part_index: 0,
        identity: vec![4],
        receipt: Some(vec![5]),
    }];
    db.connection.execute_batch("CREATE TEMP TRIGGER interrupt_import BEFORE INSERT ON vault_transfer_parts BEGIN SELECT RAISE(ABORT, 'synthetic import interruption'); END;")?;
    assert!(db.import_vault_upload(&pending, 0, &job, &parts).is_err());
    assert!(db.vault_job(7, 9)?.is_none());
    assert_eq!(
        db.pending_vault_upload(7, 9)?
            .expect("pending preserved")
            .state,
        PendingVaultUploadState::Queued
    );
    db.connection
        .execute_batch("DROP TRIGGER interrupt_import;")?;
    assert!(db.transition_pending_vault_upload(
        lease(9, 0),
        PendingVaultUploadState::Queued,
        VaultJobTransition::RequestPause,
        None
    )?);
    assert!(!db.import_vault_upload(&pending, 0, &job, &parts)?);
    assert!(db.vault_job(7, 9)?.is_none());
    assert!(db.transition_pending_vault_upload(
        lease(9, 1),
        PendingVaultUploadState::Paused,
        VaultJobTransition::Resume,
        None
    )?);
    assert!(db.import_vault_upload(&pending, 2, &job, &parts)?);
    assert!(!db.import_vault_upload(&pending, 2, &job, &parts)?);
    assert_eq!(db.vault_parts(7, 9, None, 1)?, parts);
    assert_eq!(db.last_confirmed_vault_part(7, 9)?, Some(0));
    let plan:String = db.connection.query_row("EXPLAIN QUERY PLAN SELECT part_index FROM vault_transfer_parts WHERE account_id=7 AND task_id=9 AND receipt IS NOT NULL ORDER BY part_index DESC LIMIT 1",[],|row| row.get(3))?;
    assert!(
        plan.contains("SEARCH") && !plan.contains("TEMP B-TREE"),
        "{plan}"
    );
    Ok(())
}

#[test]
fn admission_is_idempotent_without_rewinding_work_or_replacing_identity() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    let original = job(1, VaultJobDirection::Upload);
    assert!(db.admit_vault_job(&original)?);
    assert!(step(
        &mut db,
        1,
        0,
        VaultJobState::Queued,
        VaultJobTransition::Start
    )?);
    assert!(!db.admit_vault_job(&original)?);
    let mut different = original.clone();
    different.context.push(4);
    assert!(db.admit_vault_job(&different).is_err());
    different = original;
    different.package_id = [9; 16];
    assert!(db.admit_vault_job(&different).is_err());
    let stored = db.vault_job(7, 1)?.expect("admitted job");
    assert_eq!(stored.context, vec![1, 2, 3]);
    assert_eq!(stored.state, VaultJobState::Running);
    assert_eq!(stored.generation, 1);
    assert!(db.vault_job(8, 1)?.is_none());
    Ok(())
}

#[test]
fn pause_and_cancel_intent_win_against_completion_and_stale_workers() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    db.admit_vault_job(&job(1, VaultJobDirection::Download))?;
    assert!(step(
        &mut db,
        1,
        0,
        VaultJobState::Queued,
        VaultJobTransition::Start
    )?);
    let part = VaultPartRecord {
        part_index: 0,
        identity: vec![1],
        receipt: None,
    };
    assert!(db.reserve_vault_part(lease(1, 1), &part)?);
    assert!(step(
        &mut db,
        1,
        1,
        VaultJobState::Running,
        VaultJobTransition::RequestPause
    )?);
    assert!(!step(
        &mut db,
        1,
        1,
        VaultJobState::Running,
        VaultJobTransition::Complete
    )?);
    // Network success racing pause is retained, but does not erase pause intent.
    assert!(db.confirm_vault_part(lease(1, 1), 0, b"verified")?);
    assert!(step(
        &mut db,
        1,
        1,
        VaultJobState::Pausing,
        VaultJobTransition::AcknowledgePause
    )?);
    assert!(!db.reserve_vault_part(lease(1, 1), &part)?);
    assert!(step(
        &mut db,
        1,
        1,
        VaultJobState::Paused,
        VaultJobTransition::Resume
    )?);
    assert!(step(
        &mut db,
        1,
        1,
        VaultJobState::Queued,
        VaultJobTransition::Start
    )?);
    assert!(!db.confirm_vault_part(lease(1, 1), 0, b"stale")?);
    assert!(!step(
        &mut db,
        1,
        1,
        VaultJobState::Running,
        VaultJobTransition::RequestCancel
    )?);
    assert!(step(
        &mut db,
        1,
        2,
        VaultJobState::Running,
        VaultJobTransition::RequestCancel
    )?);
    assert!(step(
        &mut db,
        1,
        2,
        VaultJobState::Cancelling,
        VaultJobTransition::AcknowledgeCancel
    )?);
    assert!(!db.confirm_vault_part(lease(1, 2), 0, b"late")?);
    assert!(
        step(
            &mut db,
            1,
            2,
            VaultJobState::Cancelled,
            VaultJobTransition::Resume
        )
        .is_err()
    );
    assert_eq!(
        db.vault_parts(7, 1, None, 256)?[0].receipt.as_deref(),
        Some(b"verified".as_slice())
    );
    Ok(())
}

#[test]
fn reserved_part_identity_and_confirmed_receipt_never_change() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    db.admit_vault_job(&job(1, VaultJobDirection::Upload))?;
    step(
        &mut db,
        1,
        0,
        VaultJobState::Queued,
        VaultJobTransition::Start,
    )?;
    let mut part = VaultPartRecord {
        part_index: 2,
        identity: vec![9, 8],
        receipt: None,
    };
    assert!(db.reserve_vault_part(lease(1, 1), &part)?);
    assert!(db.reserve_vault_part(lease(1, 1), &part)?);
    part.identity = vec![9, 7];
    assert!(db.reserve_vault_part(lease(1, 1), &part).is_err());
    assert!(db.confirm_vault_part(lease(1, 1), 2, b"receipt")?);
    assert!(db.confirm_vault_part(lease(1, 1), 2, b"receipt")?);
    assert!(!db.confirm_vault_part(lease(1, 1), 2, b"different")?);
    assert!(!db.confirm_vault_part(lease(1, 1), 3, b"unreserved")?);
    let mut foreign = lease(1, 1);
    foreign.account_id = 8;
    assert!(!db.reserve_vault_part(foreign, &part)?);
    assert!(db.vault_parts(8, 1, None, 256)?.is_empty());
    assert_eq!(db.vault_parts(7, 1, None, 256)?[0].identity, vec![9, 8]);
    Ok(())
}

#[test]
fn real_database_reopen_preserves_intent_and_fences_previous_generations() -> StorageResult<()> {
    let temp = tempfile::tempdir().expect("temporary database directory");
    let path = temp.path().join("jobs.sqlite");
    {
        let mut db = Database::open(&path)?;
        for id in 1..=3 {
            db.admit_vault_job(&job(id, VaultJobDirection::Upload))?;
            step(
                &mut db,
                id,
                0,
                VaultJobState::Queued,
                VaultJobTransition::Start,
            )?;
        }
        step(
            &mut db,
            2,
            1,
            VaultJobState::Running,
            VaultJobTransition::RequestPause,
        )?;
        step(
            &mut db,
            3,
            1,
            VaultJobState::Running,
            VaultJobTransition::RequestCancel,
        )?;
        db.reserve_vault_part(
            lease(1, 1),
            &VaultPartRecord {
                part_index: 0,
                identity: vec![42],
                receipt: None,
            },
        )?;
    }
    let mut db = Database::open(&path)?;
    assert_eq!(db.recover_vault_jobs(8, 110)?, 0);
    assert_eq!(db.recover_vault_jobs(7, 110)?, 3);
    assert_eq!(db.recover_vault_jobs(7, 120)?, 0);
    for (id, state) in [
        (1, VaultJobState::Queued),
        (2, VaultJobState::Paused),
        (3, VaultJobState::Cancelled),
    ] {
        let restored = db.vault_job(7, id)?.expect("admitted job");
        assert_eq!(restored.state, state);
        assert_eq!(restored.generation, 2);
        assert_eq!(restored.context, vec![1, 2, 3]);
    }
    assert!(!db.confirm_vault_part(lease(1, 1), 0, b"old")?);
    assert_eq!(db.vault_parts(7, 1, None, 16)?[0].identity, vec![42]);
    Ok(())
}

#[test]
fn retryable_failure_is_explicit_and_blocked_or_terminal_tasks_do_not_restart() -> StorageResult<()>
{
    let mut db = Database::open_in_memory()?;
    db.admit_vault_job(&job(1, VaultJobDirection::Download))?;
    step(
        &mut db,
        1,
        0,
        VaultJobState::Queued,
        VaultJobTransition::Start,
    )?;
    assert!(
        db.transition_vault_job(
            lease(1, 1),
            VaultJobState::Running,
            VaultJobTransition::FailRetryable,
            101,
            None
        )
        .is_err()
    );
    assert!(db.transition_vault_job(
        lease(1, 1),
        VaultJobState::Running,
        VaultJobTransition::FailRetryable,
        101,
        Some("network")
    )?);
    assert_eq!(db.recover_vault_jobs(7, 102)?, 0);
    assert!(step(
        &mut db,
        1,
        1,
        VaultJobState::Retryable,
        VaultJobTransition::Retry
    )?);
    assert!(
        db.vault_job(7, 1)?
            .expect("admitted job")
            .failure_code
            .is_none()
    );
    step(
        &mut db,
        1,
        1,
        VaultJobState::Queued,
        VaultJobTransition::Start,
    )?;
    db.transition_vault_job(
        lease(1, 2),
        VaultJobState::Running,
        VaultJobTransition::FailBlocked,
        103,
        Some("source_changed"),
    )?;
    assert!(
        step(
            &mut db,
            1,
            2,
            VaultJobState::Blocked,
            VaultJobTransition::Retry
        )
        .is_err()
    );
    assert_eq!(db.recover_vault_jobs(7, 104)?, 0);
    Ok(())
}

#[test]
fn failed_write_rolls_back_intent_and_restores_connection_policy() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    db.admit_vault_job(&job(1, VaultJobDirection::Upload))?;
    let before: i64 = db
        .connection
        .pragma_query_value(None, "synchronous", |r| r.get(0))?;
    db.connection.execute_batch("CREATE TRIGGER reject_job_update BEFORE UPDATE ON vault_transfer_jobs BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
    assert!(
        step(
            &mut db,
            1,
            0,
            VaultJobState::Queued,
            VaultJobTransition::Start
        )
        .is_err()
    );
    assert_eq!(
        db.vault_job(7, 1)?.expect("admitted job").state,
        VaultJobState::Queued
    );
    assert_eq!(
        db.connection
            .pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))?,
        before
    );
    Ok(())
}

#[test]
fn keyset_pages_use_the_filtered_index_and_future_context_stays_intact() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    for id in 1..=8 {
        db.admit_vault_job(&job(id, VaultJobDirection::Download))?;
    }
    assert_eq!(
        db.vault_job_ids(7, VaultJobState::Queued, 4, 2)?,
        vec![5, 6]
    );
    assert!(db.vault_job_ids(8, VaultJobState::Queued, 0, 2)?.is_empty());
    let mut stmt=db.connection.prepare("EXPLAIN QUERY PLAN SELECT id FROM vault_transfer_jobs WHERE account_id=?1 AND state=?2 AND id>?3 ORDER BY id LIMIT ?4")?;
    let plans = stmt
        .query_map(params![7, "queued", 1000000, 256], |row| {
            row.get::<_, String>(3)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        plans
            .iter()
            .any(|p| p.contains("vault_transfer_jobs_schedule") && p.contains("id>?")),
        "{plans:?}"
    );
    drop(stmt);
    db.connection.execute(
        "UPDATE vault_transfer_jobs SET context_version=99 WHERE id=1",
        [],
    )?;
    assert!(db.vault_job(7, 1).is_err());
    assert!(!step(
        &mut db,
        1,
        0,
        VaultJobState::Queued,
        VaultJobTransition::Start
    )?);
    let context: Vec<u8> = db.connection.query_row(
        "SELECT context FROM vault_transfer_jobs WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(context, vec![1, 2, 3]);
    Ok(())
}

#[test]
fn schema_16_upgrade_preserves_legacy_history_without_fabricating_recovery_context()
-> StorageResult<()> {
    let temp = tempfile::tempdir().expect("temporary database directory");
    let path = temp.path().join("legacy.sqlite");
    let connection = rusqlite::Connection::open(&path)?;
    for migration in MIGRATIONS.iter().filter(|m| m.version <= 16) {
        connection.execute_batch(migration.sql)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 16)?;
    connection.execute_batch("INSERT INTO vault_upload_history(account_id,id,chat_id,queued_at,file_name,size_bytes,transferred_bytes,completed_parts,part_count,started_at,state) VALUES(7,1,11,100,'legacy',123,0,0,1,100,'interrupted');")?;
    drop(connection);
    let db = Database::open(&path)?;
    assert_eq!(
        db.schema_version()?,
        crate::migration::LATEST_SCHEMA_VERSION
    );
    assert!(
        db.vault_job_ids(7, VaultJobState::Queued, 0, 256)?
            .is_empty()
    );
    assert_eq!(db.vault_upload_history(7)?.records[0].file_name, "legacy");
    Ok(())
}

#[test]
fn migration_17_failure_preserves_version_16_and_can_restart() -> StorageResult<()> {
    let temp = tempfile::tempdir().expect("temporary database directory");
    let path = temp.path().join("rollback.sqlite");
    let connection = rusqlite::Connection::open(&path)?;
    for migration in MIGRATIONS.iter().filter(|m| m.version <= 16) {
        connection.execute_batch(migration.sql)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 16)?;
    // Force failure after the new jobs table/index have been created.
    connection.execute_batch("CREATE TABLE vault_transfer_parts(sentinel TEXT); INSERT INTO vault_transfer_parts VALUES('original');")?;
    drop(connection);
    assert!(Database::open(&path).is_err());
    let connection = rusqlite::Connection::open(&path)?;
    assert_eq!(
        connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
        16
    );
    assert_eq!(
        connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='vault_transfer_jobs'",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        0
    );
    assert_eq!(
        connection.query_row("SELECT sentinel FROM vault_transfer_parts", [], |r| r
            .get::<_, String>(0))?,
        "original"
    );
    connection.execute_batch("DROP TABLE vault_transfer_parts;")?;
    drop(connection);
    assert_eq!(
        Database::open(&path)?.schema_version()?,
        crate::migration::LATEST_SCHEMA_VERSION
    );
    Ok(())
}

#[test]
fn ledger_writes_enable_full_sync_only_for_the_durable_transaction() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    let original: i64 = db
        .connection
        .pragma_query_value(None, "synchronous", |r| r.get(0))?;
    let original_full: bool = db
        .connection
        .pragma_query_value(None, "fullfsync", |r| r.get(0))?;
    db.durable_vault_write(|tx| {
        // In-memory SQLite intentionally reports synchronous=OFF; the flag is
        // meaningful on disk, and fullfsync must still be scoped on this owner.
        assert!(tx.pragma_query_value(None, "fullfsync", |r| r.get::<_, bool>(0))?);
        Ok(())
    })?;
    assert_eq!(
        db.connection
            .pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))?,
        original
    );
    assert_eq!(
        db.connection
            .pragma_query_value(None, "fullfsync", |r| r.get::<_, bool>(0))?,
        original_full
    );
    let temp = tempfile::tempdir().expect("temporary database directory");
    let mut disk = Database::open(temp.path().join("durable.sqlite"))?;
    disk.durable_vault_write(|tx| {
        assert_eq!(
            tx.pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))?,
            2
        );
        assert!(tx.pragma_query_value(None, "fullfsync", |r| r.get::<_, bool>(0))?);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn cold_start_recovers_all_accounts_without_rewriting_future_contexts() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    for account in 7..=10 {
        let mut record = job(1, VaultJobDirection::Upload);
        record.account_id = account;
        db.admit_vault_job(&record)?;
        let mut owner = VaultJobLease {
            account_id: account,
            id: 1,
            generation: 0,
        };
        assert!(db.transition_vault_job(
            owner,
            VaultJobState::Queued,
            VaultJobTransition::Start,
            101,
            None
        )?);
        owner.generation = 1;
        if account == 8 {
            assert!(db.transition_vault_job(
                owner,
                VaultJobState::Running,
                VaultJobTransition::RequestPause,
                102,
                None
            )?);
        } else if account == 9 {
            assert!(db.transition_vault_job(
                owner,
                VaultJobState::Running,
                VaultJobTransition::RequestCancel,
                102,
                None
            )?);
        }
    }
    db.connection.execute(
        "UPDATE vault_transfer_jobs SET context_version=99 WHERE account_id=10",
        [],
    )?;
    assert_eq!(db.recover_all_vault_jobs(103)?, 3);
    for (account, expected) in [
        (7, VaultJobState::Queued),
        (8, VaultJobState::Paused),
        (9, VaultJobState::Cancelled),
    ] {
        let row = db.vault_job(account, 1)?.expect("retained job");
        assert_eq!(row.state, expected);
        assert_eq!(row.generation, 2);
        assert_eq!(row.context, vec![1, 2, 3]);
    }
    let future: (i64, String) = db.connection.query_row(
        "SELECT generation,state FROM vault_transfer_jobs WHERE account_id=10",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(future, (1, "running".into()));
    assert_eq!(db.recover_all_vault_jobs(104)?, 0);
    Ok(())
}

#[test]
fn manifest_outbox_is_immutable_and_receipts_obey_stop_generation() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    db.admit_vault_job(&job(1, VaultJobDirection::Upload))?;
    let reservation = VaultManifestOutbox {
        codec_version: 1,
        commitment: [7; 32],
        random_id: -19,
        envelope: None,
        message_id: None,
    };
    assert!(!db.reserve_vault_manifest(lease(1, 0), &reservation)?);
    assert!(step(
        &mut db,
        1,
        0,
        VaultJobState::Queued,
        VaultJobTransition::Start
    )?);
    assert!(db.reserve_vault_manifest(lease(1, 1), &reservation)?);
    assert!(db.reserve_vault_manifest(lease(1, 1), &reservation)?);
    let mut changed = reservation.clone();
    changed.commitment[0] ^= 1;
    assert!(db.reserve_vault_manifest(lease(1, 1), &changed).is_err());
    changed = reservation.clone();
    changed.random_id = 20;
    assert!(db.reserve_vault_manifest(lease(1, 1), &changed).is_err());
    assert!(!db.confirm_vault_manifest(lease(1, 1), 99)?);
    assert!(db.save_vault_manifest_envelope(lease(1, 1), b"sealed")?);
    assert!(!db.save_vault_manifest_envelope(lease(1, 1), b"changed")?);
    assert!(step(
        &mut db,
        1,
        1,
        VaultJobState::Running,
        VaultJobTransition::RequestPause
    )?);
    assert!(!db.save_vault_manifest_envelope(lease(1, 1), b"sealed")?);
    assert!(db.confirm_vault_manifest(lease(1, 1), 99)?);
    assert!(!db.confirm_vault_manifest(lease(1, 1), 100)?);
    assert!(step(
        &mut db,
        1,
        1,
        VaultJobState::Pausing,
        VaultJobTransition::AcknowledgePause
    )?);
    assert!(!db.confirm_vault_manifest(lease(1, 1), 99)?);
    assert!(step(
        &mut db,
        1,
        1,
        VaultJobState::Paused,
        VaultJobTransition::Resume
    )?);
    assert!(step(
        &mut db,
        1,
        1,
        VaultJobState::Queued,
        VaultJobTransition::Start
    )?);
    assert!(!db.reserve_vault_manifest(lease(1, 1), &reservation)?);
    assert!(db.reserve_vault_manifest(lease(1, 2), &reservation)?);
    let saved = db.vault_manifest_outbox(7, 1)?.expect("saved outbox");
    assert_eq!(saved.envelope.as_deref(), Some(b"sealed".as_slice()));
    assert_eq!(saved.message_id, Some(99));
    assert!(db.vault_manifest_outbox(8, 1)?.is_none());
    Ok(())
}

#[test]
fn schema_seventeen_upgrade_preserves_job_and_adds_manifest_outbox()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("upgrade.sqlite");
    let connection = rusqlite::Connection::open(&path)?;
    for migration in MIGRATIONS.iter().filter(|m| m.version <= 17) {
        connection.execute_batch(migration.sql)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 17)?;
    connection.execute("INSERT INTO vault_transfer_jobs(account_id,id,chat_id,direction,package_id,context_version,context,state,generation,created_at,updated_at) VALUES(7,1,11,'upload',zeroblob(16),1,X'010203','running',5,100,101)", [])?;
    connection.execute_batch("CREATE TABLE vault_manifest_outbox(sentinel TEXT); INSERT INTO vault_manifest_outbox VALUES('preserve');")?;
    drop(connection);
    assert!(
        Database::open(&path).is_err(),
        "failed additive migration retains originals"
    );
    let connection = rusqlite::Connection::open(&path)?;
    assert_eq!(
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?,
        17
    );
    assert_eq!(
        connection.query_row("SELECT sentinel FROM vault_manifest_outbox", [], |row| {
            row.get::<_, String>(0)
        })?,
        "preserve"
    );
    connection.execute_batch("DROP TABLE vault_manifest_outbox")?;
    drop(connection);
    let mut db = Database::open(&path)?;
    let job = db.vault_job(7, 1)?.expect("existing job preserved");
    assert_eq!(job.generation, 5);
    assert_eq!(job.context, vec![1, 2, 3]);
    assert_eq!(job.state, VaultJobState::Running);
    assert!(db.vault_manifest_outbox(7, 1)?.is_none());
    let reservation = VaultManifestOutbox {
        codec_version: 1,
        commitment: [9; 32],
        random_id: 9,
        envelope: None,
        message_id: None,
    };
    assert!(db.reserve_vault_manifest(lease(1, 5), &reservation)?);
    drop(db);
    assert_eq!(
        Database::open(path)?.vault_manifest_outbox(7, 1)?,
        Some(reservation)
    );
    Ok(())
}

#[test]
fn direction_history_has_indexed_deep_cursor_and_includes_maximum_id() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    db.admit_vault_job(&job(1, VaultJobDirection::Upload))?;
    db.admit_vault_job(&job(2, VaultJobDirection::Download))?;
    db.admit_vault_job(&job(i64::MAX as u64, VaultJobDirection::Download))?;
    assert_eq!(
        db.vault_job_ids_by_direction(7, VaultJobDirection::Download, None, 1)?,
        vec![i64::MAX as u64]
    );
    assert_eq!(
        db.vault_job_ids_by_direction(7, VaultJobDirection::Download, Some(i64::MAX as u64), 1)?,
        vec![2]
    );
    assert_eq!(
        db.vault_job_count_by_direction(7, VaultJobDirection::Download)?,
        2
    );
    assert!(
        db.vault_job_ids_by_direction(8, VaultJobDirection::Download, None, 256)?
            .is_empty()
    );
    let mut stmt = db.connection.prepare("EXPLAIN QUERY PLAN SELECT id FROM vault_transfer_jobs WHERE account_id=?1 AND direction=?2 AND id<?3 ORDER BY id DESC LIMIT ?4")?;
    let plan = stmt
        .query_map(params![7, "download", i64::MAX, 1], |row| {
            row.get::<_, String>(3)
        })?
        .collect::<Result<Vec<_>, _>>()?
        .join(" ");
    assert!(plan.contains("vault_transfer_jobs_direction"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    Ok(())
}

#[test]
fn deleting_terminal_vault_history_is_atomic_and_preserves_downloaded_output() -> StorageResult<()>
{
    let directory = tempfile::tempdir().expect("temporary output directory");
    let output = directory.path().join("downloaded.bin");
    std::fs::write(&output, b"user data").expect("downloaded output");
    let mut db = Database::open_in_memory()?;
    db.admit_vault_job(&job(9, VaultJobDirection::Download))?;
    db.connection.execute(
        "UPDATE vault_transfer_jobs SET state='completed' WHERE account_id=7 AND id=9",
        [],
    )?;
    db.connection.execute(
        "INSERT INTO vault_transfer_parts(account_id,task_id,part_index,identity,receipt) VALUES(7,9,0,X'01',X'02')",
        [],
    )?;
    db.connection.execute(
        "INSERT INTO vault_manifest_outbox(account_id,task_id,codec_version,commitment,random_id) VALUES(7,9,1,zeroblob(32),9)",
        [],
    )?;
    db.record_vault_download(&crate::VaultDownloadRecord {
        account_id: 7,
        chat_id: 11,
        package_id: "09".repeat(16),
        destination: output.clone(),
        size_bytes: 9,
        completed_at_unix_ms: 101,
    })?;

    db.delete_vault_transfer(7, 9)?;

    assert!(db.vault_job(7, 9)?.is_none());
    assert!(db.vault_parts(7, 9, None, 1)?.is_empty());
    assert!(db.vault_manifest_outbox(7, 9)?.is_none());
    assert_eq!(db.downloaded_files_page(7, None)?.len(), 1);
    assert_eq!(
        std::fs::read(&output).expect("output remains"),
        b"user data"
    );
    Ok(())
}

#[test]
fn deleting_failed_and_cancelled_vault_history_preserves_downloaded_output() -> StorageResult<()> {
    let directory = tempfile::tempdir().expect("temporary output directory");
    let mut db = Database::open_in_memory()?;
    for (id, state, package_byte) in [(12_u64, "retryable", 0x0c_u8), (13, "cancelled", 0x0d)] {
        let output = directory.path().join(format!("{id}.bin"));
        std::fs::write(&output, format!("preserved-{id}")).expect("downloaded output");
        db.admit_vault_job(&job(id, VaultJobDirection::Download))?;
        db.connection.execute(
            "UPDATE vault_transfer_jobs SET state=?1,failure_code=?2 WHERE account_id=7 AND id=?3",
            rusqlite::params![
                state,
                (state == "retryable").then_some("network"),
                id as i64
            ],
        )?;
        db.record_vault_download(&crate::VaultDownloadRecord {
            account_id: 7,
            chat_id: 11,
            package_id: format!("{package_byte:02x}").repeat(16),
            destination: output.clone(),
            size_bytes: format!("preserved-{id}").len() as u64,
            completed_at_unix_ms: 101,
        })?;
        db.delete_vault_transfer(7, id)?;
        assert!(db.vault_job(7, id)?.is_none());
        assert_eq!(
            std::fs::read(&output).expect("output remains"),
            format!("preserved-{id}").as_bytes()
        );
    }
    Ok(())
}

#[test]
fn deleting_nonterminal_vault_history_rolls_back_and_legacy_upload_history_is_removed()
-> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    db.admit_vault_job(&job(10, VaultJobDirection::Upload))?;
    db.connection.execute(
        "INSERT INTO vault_transfer_parts(account_id,task_id,part_index,identity) VALUES(7,10,0,X'01')",
        [],
    )?;
    assert!(db.delete_vault_transfer(7, 10).is_err());
    assert!(db.vault_job(7, 10)?.is_some());
    assert_eq!(db.vault_parts(7, 10, None, 1)?.len(), 1);

    db.save_vault_uploads(&[crate::VaultUploadRecord {
        account_id: 7,
        id: 11,
        chat_id: 11,
        batch_id: None,
        queued_at_unix_ms: 100,
        file_name: "legacy.bin".into(),
        package_id: None,
        size_bytes: 8,
        transferred_bytes: 0,
        completed_parts: 0,
        part_count: 1,
        started_at_unix_ms: 100,
        duration_ms: None,
        average_bytes_per_second: None,
        state: crate::StoredVaultUploadState::Interrupted,
        failure_code: None,
    }])?;
    db.delete_vault_transfer(7, 11)?;
    assert!(db.vault_upload_history(7)?.records.is_empty());
    Ok(())
}

#[test]
fn recovery_history_keeps_old_live_jobs_ahead_of_new_terminal_history() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    db.admit_vault_job(&job(1, VaultJobDirection::Upload))?;
    for id in 2..=300 {
        let mut record = job(id, VaultJobDirection::Upload);
        record.package_id = [1; 16];
        db.admit_vault_job(&record)?;
        db.connection.execute(
            "UPDATE vault_transfer_jobs SET state='completed' WHERE account_id=7 AND id=?1",
            [id as i64],
        )?;
    }
    let ids = db.vault_job_history_ids(7, VaultJobDirection::Upload, 256)?;
    assert_eq!(ids.len(), 256);
    assert_eq!(ids[0], 1);
    assert_eq!(ids[1], 300);
    let mut query = db.connection.prepare("EXPLAIN QUERY PLAN SELECT id FROM vault_transfer_jobs INDEXED BY vault_transfer_jobs_history WHERE account_id=?1 AND direction=?2 ORDER BY CASE WHEN state IN ('queued','running','pausing','cancelling') THEN 0 WHEN state IN ('paused','retryable','blocked') THEN 1 ELSE 2 END,id DESC LIMIT ?3")?;
    let plan = query
        .query_map(params![7, "upload", 256], |row| row.get::<_, String>(3))?
        .collect::<Result<Vec<_>, _>>()?
        .join(" ");
    assert!(plan.contains("vault_transfer_jobs_history"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    Ok(())
}

#[test]
fn ciphertext_rotation_and_expired_restart_are_fenced_and_atomic() -> StorageResult<()> {
    let mut db = Database::open_in_memory()?;
    let original = job(1, VaultJobDirection::Upload);
    db.admit_vault_job(&original)?;
    step(
        &mut db,
        1,
        0,
        VaultJobState::Queued,
        VaultJobTransition::Start,
    )?;
    db.reserve_vault_part(
        lease(1, 1),
        &VaultPartRecord {
            part_index: 0,
            identity: b"old".to_vec(),
            receipt: None,
        },
    )?;
    assert!(!db.replace_unpublished_vault_part(lease(1, 0), 0, b"old", b"new")?);
    assert!(!db.replace_unpublished_vault_part(lease(1, 1), 0, b"foreign", b"new")?);
    assert!(db.replace_unpublished_vault_part(lease(1, 1), 0, b"old", b"new")?);
    assert!(!db.replace_unpublished_vault_part(lease(1, 1), 0, b"old", b"new")?);
    db.confirm_vault_part(lease(1, 1), 0, b"receipt")?;
    assert!(!db.replace_unpublished_vault_part(lease(1, 1), 0, b"new", b"later")?);
    step(
        &mut db,
        1,
        1,
        VaultJobState::Running,
        VaultJobTransition::RequestPause,
    )?;
    step(
        &mut db,
        1,
        1,
        VaultJobState::Pausing,
        VaultJobTransition::AcknowledgePause,
    )?;
    step(
        &mut db,
        1,
        1,
        VaultJobState::Paused,
        VaultJobTransition::Resume,
    )?;
    let mut replacement = original.clone();
    replacement.package_id = [9; 16];
    replacement.context = vec![9, 8, 7];
    replacement.created_at_unix_ms = 200;
    replacement.updated_at_unix_ms = 200;
    assert!(!db.restart_vault_upload(lease(1, 0), &original.context, &replacement)?);
    db.connection.execute_batch("CREATE TEMP TRIGGER fail_restart BEFORE DELETE ON vault_transfer_parts BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;")?;
    assert!(
        db.restart_vault_upload(lease(1, 1), &original.context, &replacement)
            .is_err()
    );
    assert_eq!(
        db.vault_job(7, 1)?.expect("original").context,
        original.context
    );
    assert_eq!(db.vault_parts(7, 1, None, 10)?.len(), 1);
    db.connection.execute_batch("DROP TRIGGER fail_restart;")?;
    assert!(db.restart_vault_upload(lease(1, 1), &original.context, &replacement)?);
    assert!(!db.restart_vault_upload(lease(1, 1), &original.context, &replacement)?);
    assert_eq!(
        db.vault_job(7, 1)?.expect("replacement").context,
        replacement.context
    );
    assert!(db.vault_parts(7, 1, None, 10)?.is_empty());
    assert!(!db.confirm_vault_part(lease(1, 1), 0, b"stale")?);
    Ok(())
}
