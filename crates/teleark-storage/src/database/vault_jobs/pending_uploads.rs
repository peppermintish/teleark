//! Metadata-only admission before hashing or allocating any encryption identity.
use super::*;

const STOP_FORMAL_BATCH_SQL: &str = "UPDATE vault_transfer_jobs SET state='cancelled',updated_at=max(updated_at,?4),failure_code=NULL WHERE account_id=?1 AND state='queued' AND direction='upload' AND context_version=1 AND (?3 IS NULL OR id!=?3) AND id IN (SELECT id FROM vault_pending_uploads INDEXED BY vault_pending_uploads_batch WHERE account_id=?1 AND state='promoted' AND batch_id=?2 AND codec_version=1 UNION SELECT h.id FROM vault_upload_history h INDEXED BY vault_upload_history_batch JOIN vault_transfer_jobs j ON j.account_id=h.account_id AND j.id=h.id AND j.chat_id=h.chat_id WHERE h.account_id=?1 AND h.batch_id=?2 AND NOT EXISTS(SELECT 1 FROM vault_pending_uploads p WHERE p.account_id=h.account_id AND p.id=h.id))";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingVaultUpload {
    pub account_id: i64,
    pub id: u64,
    pub chat_id: i64,
    pub batch_id: u64,
    pub created_at_unix_ms: i64,
    pub codec_version: u32,
    /// Versioned native path and source metadata. No file contents or raw keys.
    pub context: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingVaultUploadState {
    Queued,
    Paused,
    Cancelled,
    Retryable,
    Blocked,
    Promoted,
}
impl PendingVaultUploadState {
    fn code(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Paused => "paused",
            Self::Cancelled => "cancelled",
            Self::Retryable => "retryable",
            Self::Blocked => "blocked",
            Self::Promoted => "promoted",
        }
    }
    fn parse(value: &str) -> StorageResult<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "paused" => Ok(Self::Paused),
            "cancelled" => Ok(Self::Cancelled),
            "retryable" => Ok(Self::Retryable),
            "blocked" => Ok(Self::Blocked),
            "promoted" => Ok(Self::Promoted),
            _ => Err(corrupt("vault_pending_uploads", "state", "unknown")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingVaultUploadSnapshot {
    pub record: PendingVaultUpload,
    pub generation: u64,
    pub state: PendingVaultUploadState,
    pub failure_code: Option<String>,
}

impl Database {
    /// Durably stops the unstarted members of one admitted selection. The
    /// currently preparing file may be excluded. Promoted queued uploads are
    /// stopped in the same transaction; running publication owners are retained.
    pub fn cancel_queued_vault_upload_batch(
        &mut self,
        account: i64,
        batch: u64,
        preparing: Option<u64>,
        at: i64,
    ) -> StorageResult<usize> {
        if at < 0 {
            return Err(invalid("vault_job.updated_at"));
        }
        let batch = unsigned_to_sql("pending_upload.batch_id", batch)?;
        let preparing = preparing
            .map(|id| unsigned_to_sql("pending_upload.id", id))
            .transpose()?;
        self.durable_vault_write(|tx| {
            let exhausted: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM vault_pending_uploads WHERE account_id=?1 AND state='queued' AND batch_id=?2 AND codec_version=1 AND (?3 IS NULL OR id!=?3) AND generation=9223372036854775807)",
                params![account, batch, preparing],
                |row| row.get(0),
            )?;
            if exhausted {
                return Err(invalid("pending_upload.generation"));
            }
            let pending = tx.execute(
                "UPDATE vault_pending_uploads SET state='cancelled',generation=generation+1,failure_code=NULL WHERE account_id=?1 AND state='queued' AND batch_id=?2 AND codec_version=1 AND (?3 IS NULL OR id!=?3)",
                params![account, batch, preparing],
            )?;
            let promoted = tx.execute(
                STOP_FORMAL_BATCH_SQL,
                params![account, batch, preparing, at],
            )?;
            Ok(pending + promoted)
        })
    }

    /// Exact union count for the combined encrypted transfer history. One task
    /// appearing in pending, executable and compatibility history counts once.
    pub fn vault_transfer_history_count(&self, account: i64) -> StorageResult<u64> {
        let count:i64=self.connection.query_row("SELECT count(*) FROM (SELECT id FROM vault_upload_history WHERE account_id=?1 UNION SELECT id FROM vault_pending_uploads WHERE account_id=?1 AND state!='promoted' UNION SELECT id FROM vault_transfer_jobs WHERE account_id=?1)",[account],|row|row.get(0))?;
        nonnegative_from_sql("vault_transfer_history", "count", count)
    }

    /// Bounded newest-id history candidates from each live pending state. Query
    /// each state through its index so promoted receipts never cause a deep scan.
    pub fn pending_vault_upload_history_ids(
        &self,
        account: i64,
        limit: usize,
    ) -> StorageResult<(Vec<u64>, u64)> {
        if limit == 0 || limit > 256 {
            return Err(invalid("pending_upload.history_limit"));
        }
        let mut ids = Vec::new();
        let mut count = 0_u64;
        for state in ["queued", "paused", "cancelled", "retryable", "blocked"] {
            let size:i64=self.connection.query_row("SELECT count(*) FROM vault_pending_uploads INDEXED BY vault_pending_uploads_schedule WHERE account_id=?1 AND state=?2",params![account,state],|row|row.get(0))?;
            count = count.saturating_add(nonnegative_from_sql(
                "vault_pending_uploads",
                "count",
                size,
            )?);
            let mut query=self.connection.prepare("SELECT id FROM vault_pending_uploads INDEXED BY vault_pending_uploads_schedule WHERE account_id=?1 AND state=?2 ORDER BY id DESC LIMIT ?3")?;
            for id in query.query_map(params![account, state, limit as i64], |row| {
                row.get::<_, i64>(0)
            })? {
                ids.push((
                    state == "queued",
                    nonnegative_from_sql("vault_pending_uploads", "id", id?)?,
                ));
            }
        }
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids.truncate(limit);
        Ok((ids.into_iter().map(|(_, id)| id).collect(), count))
    }

    /// Read the independently versioned bytes even when no decoder is available.
    /// A promoted receipt points callers to the executable ledger, never a second worker.
    pub fn pending_vault_upload(
        &self,
        account: i64,
        id: u64,
    ) -> StorageResult<Option<PendingVaultUploadSnapshot>> {
        let mut statement = self.connection.prepare("SELECT chat_id,batch_id,created_at,codec_version,context,generation,state,failure_code FROM vault_pending_uploads WHERE account_id=?1 AND id=?2")?;
        let mut rows =
            statement.query(params![account, unsigned_to_sql("pending_upload.id", id)?])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(PendingVaultUploadSnapshot {
            record: PendingVaultUpload {
                account_id: account,
                id,
                chat_id: row.get(0)?,
                batch_id: nonnegative_from_sql("vault_pending_uploads", "batch_id", row.get(1)?)?,
                created_at_unix_ms: row.get(2)?,
                codec_version: row.get(3)?,
                context: row.get(4)?,
            },
            generation: nonnegative_from_sql("vault_pending_uploads", "generation", row.get(5)?)?,
            state: PendingVaultUploadState::parse(&row.get::<_, String>(6)?)?,
            failure_code: row.get(7)?,
        }))
    }

    /// Pending work has no remote encryption owner. Every accepted command
    /// advances its generation so preflight results from older intent cannot
    /// promote after pause/resume or retry. The runtime signals its read owner
    /// separately after persisting the command.
    pub fn transition_pending_vault_upload(
        &mut self,
        lease: VaultJobLease,
        expected: PendingVaultUploadState,
        action: VaultJobTransition,
        failure: Option<&str>,
    ) -> StorageResult<bool> {
        use PendingVaultUploadState as S;
        let target = match (action, expected) {
            (VaultJobTransition::RequestPause, S::Queued) => S::Paused,
            (
                VaultJobTransition::RequestCancel,
                S::Queued | S::Paused | S::Retryable | S::Blocked,
            ) => S::Cancelled,
            (VaultJobTransition::Resume, S::Paused) | (VaultJobTransition::Retry, S::Retryable) => {
                S::Queued
            }
            (VaultJobTransition::FailRetryable, S::Queued) => S::Retryable,
            (VaultJobTransition::FailBlocked, S::Queued) => S::Blocked,
            _ => return Ok(false),
        };
        if matches!(target, S::Retryable | S::Blocked) != failure.is_some()
            || failure.is_some_and(|code| code.is_empty() || code.len() > 64 || !code.is_ascii())
        {
            return Err(invalid("pending_upload.failure_code"));
        }
        let generation = unsigned_to_sql("pending_upload.generation", lease.generation)?;
        if generation == i64::MAX {
            return Err(invalid("pending_upload.generation"));
        }
        self.durable_vault_write(|tx| {
            Ok(tx.execute("UPDATE vault_pending_uploads SET state=?4,generation=generation+1,failure_code=?5 WHERE account_id=?1 AND id=?2 AND generation=?3 AND state=?6 AND codec_version=1",params![lease.account_id,unsigned_to_sql("pending_upload.id",lease.id)?,generation,target.code(),failure,expected.code()])? == 1)
        })
    }

    /// Compatibility entry for an individually admitted bounded group.
    /// Use selection admission when multiple groups represent one user action.
    pub fn admit_pending_vault_uploads(
        &mut self,
        records: &[PendingVaultUpload],
    ) -> StorageResult<()> {
        if records.is_empty() || records.len() > 128 {
            return Err(invalid("pending_upload.window"));
        }
        self.admit_pending_vault_upload_selection(records.iter().cloned().map(Ok))
    }

    /// Atomically admit a complete selection with one encoded record resident
    /// at a time. Iterator errors (including caller cancellation) roll back all
    /// new rows. The iterator must not perform filesystem or network work while
    /// this write transaction is held. Existing identical admissions are kept.
    pub fn admit_pending_vault_upload_selection(
        &mut self,
        records: impl IntoIterator<Item = StorageResult<PendingVaultUpload>>,
    ) -> StorageResult<()> {
        self.durable_vault_write(|tx| {
            let mut count = 0_usize;
            for record in records {
                let record = record?;
                if record.account_id <= 0
                    || record.chat_id <= 0
                    || record.id == 0
                    || record.batch_id == 0
                    || record.created_at_unix_ms < 0
                    || record.codec_version != 1
                    || record.context.is_empty()
                    || record.context.len() > 131_072
                {
                    return Err(invalid("pending_upload.admission"));
                }
                count = count.checked_add(1).ok_or_else(|| invalid("pending_upload.count"))?;
                let id = unsigned_to_sql("pending_upload.id", record.id)?;
                let batch = unsigned_to_sql("pending_upload.batch_id", record.batch_id)?;
                let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM vault_transfer_jobs WHERE account_id=?1 AND id=?2) AND NOT EXISTS(SELECT 1 FROM vault_pending_uploads WHERE account_id=?1 AND id=?2)", params![record.account_id,id], |row| row.get(0))?;
                if occupied { return Err(invalid("pending_upload.existing_job")); }

                tx.execute("INSERT INTO vault_pending_uploads(account_id,id,chat_id,batch_id,created_at,codec_version,context,state) VALUES(?1,?2,?3,?4,?5,1,?6,'queued') ON CONFLICT(account_id,id) DO NOTHING", params![record.account_id,id,record.chat_id,batch,record.created_at_unix_ms,record.context])?;
                let same: bool = tx.query_row("SELECT chat_id=?3 AND batch_id=?4 AND created_at=?5 AND codec_version=1 AND context=?6 FROM vault_pending_uploads WHERE account_id=?1 AND id=?2",params![record.account_id,id,record.chat_id,batch,record.created_at_unix_ms,record.context], |row| row.get(0))?;
                if !same { return Err(invalid("pending_upload.identity")); }
            }
            if count == 0 { return Err(invalid("pending_upload.selection")); }
            Ok(())
        })
    }

    /// Keyset pagination uses the account/state index; no full queue materialization.
    /// Future codecs are returned intact so the owner can explain unsupported work.
    pub fn queued_pending_vault_uploads(
        &self,
        account: i64,
        after: u64,
        limit: usize,
    ) -> StorageResult<Vec<PendingVaultUpload>> {
        if limit == 0 || limit > 128 {
            return Err(invalid("pending_upload.limit"));
        }
        let mut statement = self.connection.prepare("SELECT id,chat_id,batch_id,created_at,codec_version,context FROM vault_pending_uploads INDEXED BY vault_pending_uploads_schedule WHERE account_id=?1 AND state='queued' AND id>?2 ORDER BY id LIMIT ?3")?;
        let mut rows = statement.query(params![
            account,
            unsigned_to_sql("pending_upload.cursor", after)?,
            limit as i64
        ])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(PendingVaultUpload {
                account_id: account,
                id: nonnegative_from_sql("vault_pending_uploads", "id", row.get(0)?)?,
                chat_id: row.get(1)?,
                batch_id: nonnegative_from_sql("vault_pending_uploads", "batch_id", row.get(2)?)?,
                created_at_unix_ms: row.get(3)?,
                codec_version: row.get(4)?,
                context: row.get(5)?,
            });
        }
        Ok(records)
    }

    /// No remote work may start until this transaction commits. Pending metadata
    /// stays immutable; the separately versioned executable context is inserted
    /// once, together with retirement of the queue entry. A stopped/stale owner
    /// returns false without admitting an executable job.
    pub fn promote_pending_vault_upload(
        &mut self,
        pending: &PendingVaultUpload,
        generation: u64,
        job: &VaultJobRecord,
    ) -> StorageResult<bool> {
        validate_admission(job)?;
        if pending.account_id != job.account_id
            || pending.id != job.id
            || pending.chat_id != job.chat_id
            || pending.created_at_unix_ms != job.created_at_unix_ms
            || pending.codec_version != 1
            || job.direction != VaultJobDirection::Upload
        {
            return Err(invalid("pending_upload.promotion_scope"));
        }
        self.durable_vault_write(|tx| {
            let changed = tx.execute("UPDATE vault_pending_uploads SET state='promoted',generation=generation+1 WHERE account_id=?1 AND id=?2 AND chat_id=?3 AND batch_id=?4 AND created_at=?5 AND codec_version=1 AND context=?6 AND state='queued' AND generation=?7", params![pending.account_id,unsigned_to_sql("pending_upload.id",pending.id)?,pending.chat_id,unsigned_to_sql("pending_upload.batch_id",pending.batch_id)?,pending.created_at_unix_ms,pending.context,unsigned_to_sql("pending_upload.generation",generation)?])?;
            if changed == 0 { return Ok(false); }
            if !admit_job(tx, job)? { return Err(invalid("pending_upload.existing_job")); }
            Ok(true)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_batch_stop_uses_history_only_without_an_authoritative_receipt() -> StorageResult<()> {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("legacy.sqlite");
        let mut db = Database::open(&path)?;
        for id in 1..=8 {
            let mut item = pending(id);
            if id == 4 {
                item.account_id = 8;
            }
            if matches!(id, 5 | 6) {
                if id == 6 {
                    item.batch_id = 99;
                }
                db.admit_pending_vault_uploads(std::slice::from_ref(&item))?;
                assert!(db.promote_pending_vault_upload(&item, 0, &job(&item))?);
            } else {
                assert!(db.admit_vault_job(&job(&item))?);
            }
            db.connection.execute("INSERT INTO vault_upload_history(account_id,id,chat_id,batch_id,queued_at,file_name,size_bytes,transferred_bytes,completed_parts,part_count,started_at,state) VALUES(?1,?2,?3,?4,100,'legacy.bin',8,0,0,1,0,'queued')", params![item.account_id,id as i64,if id == 7 { 12 } else { 11 },if id == 3 { 99 } else { 13 }])?;
        }
        db.connection.execute(
            "UPDATE vault_pending_uploads SET codec_version=2 WHERE account_id=7 AND id=5",
            [],
        )?;
        db.connection.execute(
            "UPDATE vault_transfer_jobs SET context_version=2 WHERE account_id=7 AND id=8",
            [],
        )?;
        assert!(db.transition_vault_job(
            VaultJobLease {
                account_id: 7,
                id: 2,
                generation: 0
            },
            VaultJobState::Queued,
            VaultJobTransition::Start,
            101,
            None
        )?);
        let plan = {
            let mut statement = db
                .connection
                .prepare(&format!("EXPLAIN QUERY PLAN {STOP_FORMAL_BATCH_SQL}"))?;
            statement
                .query_map(params![7, 13, None::<i64>, 200], |row| {
                    row.get::<_, String>(3)
                })?
                .collect::<Result<Vec<_>, _>>()?
                .join("\n")
        };
        assert!(
            plan.contains("vault_upload_history_batch"),
            "legacy batch lookup: {plan}"
        );
        assert!(
            plan.contains("vault_pending_uploads_batch (account_id=? AND batch_id=? AND state=?)"),
            "promoted batch lookup must constrain the batch: {plan}"
        );
        assert_eq!(db.cancel_queued_vault_upload_batch(7, 13, None, 200)?, 1);
        drop(db);
        let db = Database::open(&path)?;
        for id in 1..=8 {
            let state: String = db.connection.query_row(
                "SELECT state FROM vault_transfer_jobs WHERE id=?1",
                [id],
                |row| row.get(0),
            )?;
            assert_eq!(
                state,
                match id {
                    1 => "cancelled",
                    2 => "running",
                    _ => "queued",
                },
                "member {id}"
            );
        }
        Ok(())
    }

    #[test]
    fn queued_batch_stop_is_atomic_scoped_and_survives_reopen() -> StorageResult<()> {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("batch.sqlite");
        let mut db = Database::open(&path)?;
        db.admit_pending_vault_upload_selection((1..=513).map(|id| Ok(pending(id))))?;
        let mut other = pending(900);
        other.batch_id = 99;
        let mut foreign = pending(1);
        foreign.account_id = 8;
        db.admit_pending_vault_upload_selection([Ok(other), Ok(foreign)])?;
        let promoted = pending(514);
        db.admit_pending_vault_upload_selection([Ok(promoted.clone())])?;
        assert!(db.promote_pending_vault_upload(&promoted, 0, &job(&promoted))?);
        for id in [515, 516] {
            let item = pending(id);
            db.admit_pending_vault_upload_selection([Ok(item.clone())])?;
            assert!(db.promote_pending_vault_upload(&item, 0, &job(&item))?);
        }
        assert!(db.transition_vault_job(
            VaultJobLease {
                account_id: 7,
                id: 515,
                generation: 0
            },
            VaultJobState::Queued,
            VaultJobTransition::Start,
            101,
            None
        )?);
        db.connection.execute(
            "UPDATE vault_transfer_jobs SET context_version=2 WHERE account_id=7 AND id=516",
            [],
        )?;

        db.connection.execute(
            "UPDATE vault_pending_uploads SET codec_version=2 WHERE account_id=7 AND id=513",
            [],
        )?;
        db.connection.execute_batch("CREATE TRIGGER reject_batch_stop BEFORE UPDATE ON vault_pending_uploads WHEN NEW.id=400 BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        assert!(
            db.cancel_queued_vault_upload_batch(7, 13, Some(1), 200)
                .is_err()
        );
        assert_eq!(
            db.pending_vault_upload(7, 2)?.expect("rolled back").state,
            PendingVaultUploadState::Queued
        );
        db.connection
            .execute_batch("DROP TRIGGER reject_batch_stop")?;
        db.connection.execute_batch("CREATE TRIGGER reject_promoted_stop BEFORE UPDATE ON vault_transfer_jobs WHEN NEW.id=514 BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        assert!(
            db.cancel_queued_vault_upload_batch(7, 13, Some(1), 200)
                .is_err()
        );
        assert_eq!(
            db.pending_vault_upload(7, 2)?
                .expect("pending rollback with formal failure")
                .state,
            PendingVaultUploadState::Queued
        );
        db.connection
            .execute_batch("DROP TRIGGER reject_promoted_stop")?;
        assert_eq!(
            db.cancel_queued_vault_upload_batch(7, 13, Some(1), 200)?,
            512
        );
        assert_eq!(db.cancel_queued_vault_upload_batch(7, 13, Some(1), 200)?, 0);
        assert!(!db.transition_vault_job(
            VaultJobLease {
                account_id: 7,
                id: 514,
                generation: 0
            },
            VaultJobState::Queued,
            VaultJobTransition::Start,
            201,
            None
        )?);
        assert_eq!(
            db.vault_job(7, 514)?.expect("stopped").updated_at_unix_ms,
            200
        );
        assert_eq!(
            db.vault_job(7, 515)?.expect("already running").state,
            VaultJobState::Running
        );
        let future_state: String = db.connection.query_row(
            "SELECT state FROM vault_transfer_jobs WHERE account_id=7 AND id=516",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(future_state, "queued");

        drop(db);
        let db = Database::open(&path)?;
        for id in 2..=512 {
            let saved = db.pending_vault_upload(7, id)?.expect("cancelled member");
            assert_eq!(saved.state, PendingVaultUploadState::Cancelled);
            assert_eq!(saved.generation, 1);
        }
        for (account, id) in [(7, 1), (7, 513), (7, 900), (8, 1)] {
            let saved = db
                .pending_vault_upload(account, id)?
                .expect("preserved member");
            assert_eq!(saved.state, PendingVaultUploadState::Queued);
            assert_eq!(saved.generation, 0);
        }
        assert_eq!(
            db.pending_vault_upload(7, 514)?.expect("promoted").state,
            PendingVaultUploadState::Promoted
        );
        assert_eq!(
            db.vault_job(7, 514)?.expect("publication owner").state,
            VaultJobState::Cancelled
        );
        Ok(())
    }

    fn pending(id: u64) -> PendingVaultUpload {
        PendingVaultUpload {
            account_id: 7,
            id,
            chat_id: 11,
            batch_id: 13,
            created_at_unix_ms: 100,
            codec_version: 1,
            context: vec![1, 2, 3],
        }
    }
    fn job(pending: &PendingVaultUpload) -> VaultJobRecord {
        VaultJobRecord {
            account_id: pending.account_id,
            id: pending.id,
            chat_id: pending.chat_id,
            direction: VaultJobDirection::Upload,
            package_id: [1; 16],
            context_version: 1,
            context: vec![4, 5, 6],
            state: VaultJobState::Queued,
            generation: 0,
            created_at_unix_ms: 100,
            updated_at_unix_ms: 100,
            failure_code: None,
        }
    }

    #[test]
    fn queued_metadata_survives_reopen_and_promotion_is_atomic() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("queue.sqlite");
        let mut db = Database::open(&path).expect("database");
        let records: Vec<_> = (1..=128).map(pending).collect();
        db.admit_pending_vault_uploads(&records).expect("admit");
        db.admit_pending_vault_uploads(&records)
            .expect("idempotent");
        drop(db);
        let mut db = Database::open(&path).expect("reopen");
        assert_eq!(
            db.queued_pending_vault_uploads(7, 0, 128).expect("queue"),
            records
        );
        assert!(
            db.queued_pending_vault_uploads(8, 0, 128)
                .expect("foreign")
                .is_empty()
        );
        assert_eq!(
            db.queued_pending_vault_uploads(7, 126, 2).expect("page"),
            records[126..]
        );
        let target = &records[0];
        let executable = job(target);
        assert!(
            !db.promote_pending_vault_upload(target, 1, &executable)
                .expect("stale")
        );
        assert!(db.vault_job(7, 1).expect("job").is_none());
        db.connection.execute_batch("CREATE TRIGGER reject_job BEFORE INSERT ON vault_transfer_jobs BEGIN SELECT RAISE(ABORT,'injected'); END;").expect("gate");
        assert!(
            db.promote_pending_vault_upload(target, 0, &executable)
                .is_err()
        );
        assert_eq!(
            db.queued_pending_vault_uploads(7, 0, 1).expect("rollback"),
            vec![target.clone()]
        );
        db.connection
            .execute_batch("DROP TRIGGER reject_job")
            .expect("release");
        assert!(
            db.promote_pending_vault_upload(target, 0, &executable)
                .expect("promote")
        );
        assert!(
            !db.promote_pending_vault_upload(target, 0, &executable)
                .expect("repeat")
        );
        db.admit_pending_vault_uploads(std::slice::from_ref(target))
            .expect("late repeated admission");
        assert_eq!(
            db.queued_pending_vault_uploads(7, 0, 1)
                .expect("not rewound")[0]
                .id,
            2
        );
        assert_eq!(
            db.vault_job(7, 1).expect("job").expect("present"),
            executable
        );
    }

    #[test]
    fn admission_conflict_rolls_back_whole_group_and_future_codecs_stay_intact() {
        let mut db = Database::open_in_memory().expect("database");
        let original = pending(1);
        db.admit_pending_vault_uploads(std::slice::from_ref(&original))
            .expect("admit");
        let mut conflict = original.clone();
        conflict.context.push(4);
        assert!(
            db.admit_pending_vault_uploads(&[pending(2), conflict])
                .is_err()
        );
        assert_eq!(
            db.queued_pending_vault_uploads(7, 0, 128)
                .expect("rollback"),
            vec![original.clone()]
        );
        db.connection
            .execute(
                "UPDATE vault_pending_uploads SET codec_version=99 WHERE account_id=7 AND id=1",
                [],
            )
            .expect("future");
        let saved = db.queued_pending_vault_uploads(7, 0, 1).expect("preserved");
        assert_eq!(saved[0].codec_version, 99);
        assert!(
            !db.promote_pending_vault_upload(&original, 0, &job(&original))
                .expect("cannot promote future")
        );
        assert_eq!(
            db.queued_pending_vault_uploads(7, 0, 1).expect("unchanged"),
            saved
        );
    }
    #[test]
    fn migration_twenty_rolls_back_partial_schema_and_preserves_recovery_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::migration::{APPLICATION_ID, MIGRATIONS};
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("migration.sqlite");
        let connection = rusqlite::Connection::open(&path)?;
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 19)
        {
            connection.execute_batch(migration.sql)?;
        }
        connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        connection.pragma_update(None, "user_version", 19)?;
        connection.execute_batch("INSERT INTO vault_transfer_jobs(account_id,id,chat_id,direction,package_id,context_version,context,state,generation,created_at,updated_at) VALUES(7,1,11,'upload',zeroblob(16),1,X'010203','paused',5,100,101); CREATE INDEX vault_pending_uploads_schedule ON vault_transfer_jobs(chat_id);")?;
        drop(connection);
        assert!(Database::open(&path).is_err());
        let connection = rusqlite::Connection::open(&path)?;
        assert_eq!(
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?,
            19
        );
        assert_eq!(
            connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='vault_pending_uploads'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        assert_eq!(
            connection.query_row(
                "SELECT context FROM vault_transfer_jobs WHERE account_id=7 AND id=1",
                [],
                |row| row.get::<_, Vec<u8>>(0)
            )?,
            vec![1, 2, 3]
        );
        connection.execute_batch("DROP INDEX vault_pending_uploads_schedule")?;
        drop(connection);
        let mut db = Database::open(&path)?;
        let saved = db.vault_job(7, 1)?.expect("old record");
        assert_eq!(saved.state, VaultJobState::Paused);
        assert_eq!(saved.generation, 5);
        db.admit_pending_vault_uploads(&[pending(2)])?;
        drop(db);
        assert_eq!(
            Database::open(&path)?.queued_pending_vault_uploads(7, 0, 1)?,
            vec![pending(2)]
        );
        Ok(())
    }

    #[test]
    fn pending_queue_uses_index_at_deep_cursor_and_rejects_stopped_promotion() -> StorageResult<()>
    {
        let mut db = Database::open_in_memory()?;
        let record = pending(i64::MAX as u64);
        db.admit_pending_vault_uploads(std::slice::from_ref(&record))?;
        assert_eq!(
            db.queued_pending_vault_uploads(7, i64::MAX as u64 - 1, 1)?,
            vec![record.clone()]
        );
        let mut statement = db.connection.prepare("EXPLAIN QUERY PLAN SELECT id,chat_id,batch_id,created_at,codec_version,context FROM vault_pending_uploads INDEXED BY vault_pending_uploads_schedule WHERE account_id=?1 AND state='queued' AND id>?2 ORDER BY id LIMIT ?3")?;
        let plan = statement
            .query_map(params![7, i64::MAX - 1, 1], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        assert!(plan.contains("vault_pending_uploads_schedule"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        drop(statement);
        let occupied = pending(2);
        db.admit_vault_job(&job(&occupied))?;
        assert!(
            db.admit_pending_vault_uploads(&[pending(3), occupied])
                .is_err()
        );
        assert_eq!(
            db.queued_pending_vault_uploads(7, 0, 128)?,
            vec![record.clone()]
        );
        for state in ["paused", "cancelled"] {
            db.connection.execute(
                "UPDATE vault_pending_uploads SET state=?1 WHERE account_id=7",
                [state],
            )?;
            assert!(!db.promote_pending_vault_upload(&record, 0, &job(&record))?);
            assert!(db.vault_job(7, record.id)?.is_none());
        }
        Ok(())
    }
    #[test]
    fn pending_controls_persist_and_fence_preflight_across_pause_resume_and_retry()
    -> StorageResult<()> {
        use PendingVaultUploadState as S;
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("controls.sqlite");
        let mut db = Database::open(&path)?;
        let record = pending(1);
        db.admit_pending_vault_uploads(std::slice::from_ref(&record))?;
        let mut lease = VaultJobLease {
            account_id: 7,
            id: 1,
            generation: 0,
        };
        assert!(db.transition_pending_vault_upload(
            lease,
            S::Queued,
            VaultJobTransition::RequestPause,
            None
        )?);
        drop(db);
        let mut db = Database::open(&path)?;
        assert_eq!(
            db.pending_vault_upload(7, 1)?
                .expect("restored pause")
                .state,
            S::Paused
        );
        assert!(!db.promote_pending_vault_upload(&record, 0, &job(&record))?);
        assert!(!db.transition_pending_vault_upload(
            lease,
            S::Paused,
            VaultJobTransition::Resume,
            None
        )?);
        lease.generation = 1;
        assert!(db.transition_pending_vault_upload(
            lease,
            S::Paused,
            VaultJobTransition::Resume,
            None
        )?);
        assert!(
            !db.promote_pending_vault_upload(&record, 0, &job(&record))?,
            "resume cannot revive an old preflight"
        );
        lease.generation = 2;
        assert!(db.transition_pending_vault_upload(
            lease,
            S::Queued,
            VaultJobTransition::FailRetryable,
            Some("network")
        )?);
        let failed = db.pending_vault_upload(7, 1)?.expect("failed record");
        assert_eq!(failed.state, S::Retryable);
        assert_eq!(failed.generation, 3);
        assert_eq!(failed.failure_code.as_deref(), Some("network"));
        lease.generation = 3;
        assert!(db.transition_pending_vault_upload(
            lease,
            S::Retryable,
            VaultJobTransition::Retry,
            None
        )?);
        let ready = db.pending_vault_upload(7, 1)?.expect("ready");
        assert_eq!(ready.record, record);
        assert_eq!(ready.generation, 4);
        assert_eq!(ready.failure_code, None);
        lease.generation = 4;
        assert!(db.transition_pending_vault_upload(
            lease,
            S::Queued,
            VaultJobTransition::RequestCancel,
            None
        )?);
        assert!(!db.promote_pending_vault_upload(&record, 4, &job(&record))?);
        assert!(db.pending_vault_upload(8, 1)?.is_none());
        db.admit_pending_vault_uploads(std::slice::from_ref(&record))?;
        assert_eq!(
            db.pending_vault_upload(7, 1)?.expect("not rewound").state,
            S::Cancelled
        );
        Ok(())
    }
    #[test]
    fn pending_history_uses_state_index_and_prioritizes_queued_work() -> StorageResult<()> {
        let mut db = Database::open_in_memory()?;
        db.admit_pending_vault_uploads(&[pending(1), pending(2)])?;
        db.transition_pending_vault_upload(
            VaultJobLease {
                account_id: 7,
                id: 2,
                generation: 0,
            },
            PendingVaultUploadState::Queued,
            VaultJobTransition::RequestCancel,
            None,
        )?;
        assert_eq!(db.pending_vault_upload_history_ids(7, 1)?, (vec![1], 2));
        assert_eq!(db.pending_vault_upload_history_ids(8, 256)?, (vec![], 0));
        let mut statement=db.connection.prepare("EXPLAIN QUERY PLAN SELECT id FROM vault_pending_uploads INDEXED BY vault_pending_uploads_schedule WHERE account_id=?1 AND state=?2 ORDER BY id DESC LIMIT ?3")?;
        let plan = statement
            .query_map(params![7, "queued", 256], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        assert!(plan.contains("vault_pending_uploads_schedule"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        Ok(())
    }

    #[test]
    fn complete_selection_is_invisible_until_commit_and_rolls_back_iterator_failure() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("atomic.sqlite");
        let mut db = Database::open(&path).expect("database");
        db.admit_pending_vault_uploads(&[pending(9999)])
            .expect("existing queue");
        let reader = Database::open(&path).expect("independent reader");
        let result = db.admit_pending_vault_upload_selection((1..=513).map(|id| {
            if id == 257 {
                assert_eq!(
                    reader
                        .vault_transfer_history_count(7)
                        .expect("committed count"),
                    1
                );
                return Err(invalid("synthetic.admission_interruption"));
            }
            Ok(pending(id))
        }));
        assert!(result.is_err());
        assert_eq!(
            reader
                .vault_transfer_history_count(7)
                .expect("rolled back count"),
            1
        );
        db.admit_pending_vault_upload_selection((1..=513).map(|id| {
            if id == 257 {
                assert_eq!(
                    reader
                        .vault_transfer_history_count(7)
                        .expect("uncommitted prefix hidden"),
                    1
                );
            }
            Ok(pending(id))
        }))
        .expect("whole selection");
        assert_eq!(
            reader
                .vault_transfer_history_count(7)
                .expect("committed selection"),
            514
        );
        drop(db);
        let db = Database::open(&path).expect("reopen");
        assert_eq!(
            db.vault_transfer_history_count(7)
                .expect("durable selection"),
            514
        );
    }

    #[test]
    fn admission_process_exit_helper() {
        let Some(path) = std::env::var_os("TELEARK_TEST_ATOMIC_SELECTION_PATH") else {
            return;
        };
        let mut db = Database::open(std::path::PathBuf::from(path)).expect("child database");
        db.admit_pending_vault_upload_selection((1..=513).map(|id| {
            if id == 257 {
                // Deliberately bypass transaction Drop to model process death.
                std::process::exit(73);
            }
            Ok(pending(id))
        }))
        .expect("unreachable commit");
        panic!("child must stop inside uncommitted selection");
    }

    #[test]
    fn process_death_during_selection_leaves_no_runnable_prefix() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("crash.sqlite");
        let mut db = Database::open(&path).expect("database");
        db.admit_pending_vault_uploads(&[pending(9999)])
            .expect("existing queue");
        drop(db);
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "database::vault_jobs::pending_uploads::tests::admission_process_exit_helper",
                "--nocapture",
            ])
            .env("TELEARK_TEST_ATOMIC_SELECTION_PATH", &path)
            .output()
            .expect("crash subprocess");
        assert_eq!(
            output.status.code(),
            Some(73),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let db = Database::open(&path).expect("recover database after process death");
        assert_eq!(
            db.queued_pending_vault_uploads(7, 0, 128).expect("queue"),
            vec![pending(9999)]
        );
        assert_eq!(db.vault_transfer_history_count(7).expect("no prefix"), 1);
    }
}
