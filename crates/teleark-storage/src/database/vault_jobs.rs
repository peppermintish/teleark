//! Durable admission and intent for the encrypted transfer owner. No plaintext keys.
mod manifest_outbox;
mod pending_uploads;
pub use manifest_outbox::VaultManifestOutbox;
pub use pending_uploads::{
    PendingVaultUpload, PendingVaultUploadSnapshot, PendingVaultUploadState,
};

use super::{Database, corrupt, nonnegative_from_sql, unsigned_to_sql};
use crate::{InputReason, StorageError, StorageResult};
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultJobDirection {
    Upload,
    Download,
}
impl VaultJobDirection {
    fn code(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultJobState {
    Queued,
    Running,
    Pausing,
    Paused,
    Cancelling,
    Cancelled,
    Retryable,
    Blocked,
    Completed,
}
impl VaultJobState {
    fn code(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
            Self::Retryable => "retryable",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
        }
    }
    fn parse(value: &str) -> StorageResult<Self> {
        Ok(match value {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "pausing" => Self::Pausing,
            "paused" => Self::Paused,
            "cancelling" => Self::Cancelling,
            "cancelled" => Self::Cancelled,
            "retryable" => Self::Retryable,
            "blocked" => Self::Blocked,
            "completed" => Self::Completed,
            _ => return Err(corrupt("vault_transfer_jobs", "state", "unknown")),
        })
    }
}

/// Immutable context is independently versioned by the runtime. It contains wrapped
/// keys and stable paths/identities, never raw file/master keys or account sessions.
/// A new context version requires a reader or migration before work can resume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultJobRecord {
    pub account_id: i64,
    pub id: u64,
    pub chat_id: i64,
    pub direction: VaultJobDirection,
    pub package_id: [u8; 16],
    pub context_version: u32,
    pub context: Vec<u8>,
    pub state: VaultJobState,
    pub generation: u64,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    pub failure_code: Option<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultJobLease {
    pub account_id: i64,
    pub id: u64,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultJobTransition {
    Start,
    RequestPause,
    AcknowledgePause,
    RequestCancel,
    AcknowledgeCancel,
    Resume,
    Retry,
    FailRetryable,
    FailBlocked,
    Complete,
}
impl VaultJobTransition {
    fn target(self, state: VaultJobState) -> Option<VaultJobState> {
        use VaultJobState as S;
        Some(match (self, state) {
            (Self::Start, S::Queued) => S::Running,
            (Self::RequestPause, S::Running) => S::Pausing,
            (Self::RequestPause, S::Queued) => S::Paused,
            (Self::AcknowledgePause, S::Pausing) => S::Paused,
            (Self::RequestCancel, S::Running | S::Pausing) => S::Cancelling,
            (Self::RequestCancel, S::Queued | S::Paused | S::Retryable | S::Blocked) => {
                S::Cancelled
            }
            (Self::AcknowledgeCancel, S::Cancelling) => S::Cancelled,
            (Self::Resume, S::Paused) | (Self::Retry, S::Retryable) => S::Queued,
            (Self::FailRetryable, S::Running) => S::Retryable,
            (Self::FailBlocked, S::Running) => S::Blocked,
            (Self::Complete, S::Running) => S::Completed,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultPartRecord {
    pub part_index: u32,
    /// Runtime-versioned AEAD reservation or authenticated download identity.
    /// Receipted identities are immutable. Unreceipted upload reservations can
    /// only be atomically retired before allocating fresh encryption material.
    pub identity: Vec<u8>,
    /// Authenticated remote receipt or fsynced local extent evidence.
    pub receipt: Option<Vec<u8>>,
}
fn invalid(field: &'static str) -> StorageError {
    StorageError::InvalidInput {
        field,
        reason: InputReason::InvalidCombination,
    }
}

impl Database {
    // Nonce reservation and control intent must survive power loss, not merely
    // process restart. The connection belongs to a single storage owner; no
    // other statement can interleave while changing its synchronous setting.
    pub(super) fn durable_vault_write<T>(
        &mut self,
        write: impl FnOnce(&Transaction<'_>) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let previous: i64 = self
            .connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))?;
        let previous_fullfsync: bool =
            self.connection
                .pragma_query_value(None, "fullfsync", |row| row.get(0))?;
        self.connection.pragma_update(None, "fullfsync", true)?;
        if let Err(error) = self.connection.pragma_update(None, "synchronous", "FULL") {
            let _ = self
                .connection
                .pragma_update(None, "fullfsync", previous_fullfsync);
            return Err(error.into());
        }
        let result: StorageResult<T> = (|| {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = write(&tx)?;
            tx.commit()?;
            Ok(result)
        })();
        let restored = self.connection.pragma_update(None, "synchronous", previous);
        let restored_fullfsync =
            self.connection
                .pragma_update(None, "fullfsync", previous_fullfsync);
        let value = result?;
        restored?;
        restored_fullfsync?;
        Ok(value)
    }

    /// Idempotent admission only if the complete immutable identity is identical.
    /// Never rewinds an existing task or overwrites recovery bytes on a collision.
    pub fn admit_vault_job(&mut self, job: &VaultJobRecord) -> StorageResult<bool> {
        validate_admission(job)?;
        self.durable_vault_write(|tx| admit_job(tx, job))
    }

    pub fn vault_job(&self, account: i64, id: u64) -> StorageResult<Option<VaultJobRecord>> {
        let mut stmt = self.connection.prepare("SELECT chat_id,direction,package_id,context_version,context,state,generation,created_at,updated_at,failure_code FROM vault_transfer_jobs WHERE account_id=?1 AND id=?2")?;
        let mut rows = stmt.query(params![account, unsigned_to_sql("vault_job.id", id)?])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let direction: String = row.get(1)?;
        let package: Vec<u8> = row.get(2)?;
        let version: u32 = row.get(3)?;
        if !matches!(version, 1 | 2) {
            return Err(corrupt(
                "vault_transfer_jobs",
                "context_version",
                "unsupported",
            ));
        }
        Ok(Some(VaultJobRecord {
            account_id: account,
            id,
            chat_id: row.get(0)?,
            direction: match direction.as_str() {
                "upload" => VaultJobDirection::Upload,
                "download" => VaultJobDirection::Download,
                _ => return Err(corrupt("vault_transfer_jobs", "direction", "unknown")),
            },
            package_id: package
                .try_into()
                .map_err(|_| corrupt("vault_transfer_jobs", "package_id", "length"))?,
            context_version: version,
            context: row.get(4)?,
            state: VaultJobState::parse(&row.get::<_, String>(5)?)?,
            generation: nonnegative_from_sql("vault_transfer_jobs", "generation", row.get(6)?)?,
            created_at_unix_ms: row.get(7)?,
            updated_at_unix_ms: row.get(8)?,
            failure_code: row.get(9)?,
        }))
    }

    /// Every state mutation is fenced by both generation and expected state.
    /// Start allocates a new generation before an execution owner sees its work.
    /// `false` means stale or concurrently controlled; do not report success.
    pub fn transition_vault_job(
        &mut self,
        lease: VaultJobLease,
        expected: VaultJobState,
        transition: VaultJobTransition,
        at: i64,
        failure: Option<&str>,
    ) -> StorageResult<bool> {
        let target = transition
            .target(expected)
            .ok_or_else(|| invalid("vault_job.transition"))?;
        if at < 0
            || failure.is_some_and(|value| value.is_empty() || value.len() > 64)
            || matches!(target, VaultJobState::Retryable | VaultJobState::Blocked)
                != failure.is_some()
        {
            return Err(invalid("vault_job.failure"));
        }
        let id = unsigned_to_sql("vault_job.id", lease.id)?;
        let generation = unsigned_to_sql("vault_job.generation", lease.generation)?;
        let next = if transition == VaultJobTransition::Start {
            generation
                .checked_add(1)
                .ok_or_else(|| invalid("vault_job.generation"))?
        } else {
            generation
        };
        self.durable_vault_write(|tx|Ok(tx.execute("UPDATE vault_transfer_jobs SET state=?5,generation=?6,updated_at=max(updated_at,?7),failure_code=?8 WHERE account_id=?1 AND id=?2 AND generation=?3 AND state=?4 AND context_version IN (1,2)",params![lease.account_id,id,generation,expected.code(),target.code(),next,at,failure])? == 1))
    }

    /// Call at cold process startup, before creating execution owners. Preserve
    /// explicit pause/cancel intent and block all old generations. Running work
    /// is queued for validation/reconciliation, never declared complete.
    pub fn recover_vault_jobs(&mut self, account: i64, at: i64) -> StorageResult<usize> {
        if account <= 0 || at < 0 {
            return Err(invalid("vault_job.recover"));
        }
        self.durable_vault_write(|tx|Ok(tx.execute("UPDATE vault_transfer_jobs SET state=CASE state WHEN 'running' THEN 'queued' WHEN 'pausing' THEN 'paused' WHEN 'cancelling' THEN 'cancelled' END,generation=generation+1,updated_at=max(updated_at,?2) WHERE account_id=?1 AND context_version IN (1,2) AND state IN ('running','pausing','cancelling')",params![account,at])?))
    }

    /// Cold-start recovery before any transfer owner is created. This covers
    /// all retained accounts without loading their job collections in memory.
    pub fn recover_all_vault_jobs(&mut self, at: i64) -> StorageResult<usize> {
        if at < 0 {
            return Err(invalid("vault_job.recover"));
        }
        self.durable_vault_write(|tx| Ok(tx.execute(
            "UPDATE vault_transfer_jobs SET state=CASE state WHEN 'running' THEN 'queued' WHEN 'pausing' THEN 'paused' WHEN 'cancelling' THEN 'cancelled' END,generation=generation+1,updated_at=max(updated_at,?1) WHERE context_version IN (1,2) AND state IN ('running','pausing','cancelling')", params![at],
        )?))
    }

    /// Keyset pages keep startup and queue hydration bounded, including deep pages.
    pub fn vault_job_ids(
        &self,
        account: i64,
        state: VaultJobState,
        after: u64,
        limit: u16,
    ) -> StorageResult<Vec<u64>> {
        if account <= 0 || limit == 0 || limit > 256 {
            return Err(invalid("vault_job.page"));
        }
        let mut stmt = self.connection.prepare("SELECT id FROM vault_transfer_jobs WHERE account_id=?1 AND state=?2 AND id>?3 ORDER BY id LIMIT ?4")?;
        let ids = stmt
            .query_map(
                params![
                    account,
                    state.code(),
                    unsigned_to_sql("vault_job.cursor", after)?,
                    limit
                ],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| nonnegative_from_sql("vault_transfer_jobs", "id", id))
            .collect()
    }

    /// Bounded visible history prioritizes live and recoverable work over terminal receipts.
    pub fn vault_job_history_ids(
        &self,
        account: i64,
        direction: VaultJobDirection,
        limit: u16,
    ) -> StorageResult<Vec<u64>> {
        if account <= 0 || limit == 0 || limit > 256 {
            return Err(invalid("vault_job.history_limit"));
        }
        let mut query = self.connection.prepare("SELECT id FROM vault_transfer_jobs INDEXED BY vault_transfer_jobs_history WHERE account_id=?1 AND direction=?2 ORDER BY CASE WHEN state IN ('queued','running','pausing','cancelling') THEN 0 WHEN state IN ('paused','retryable','blocked') THEN 1 ELSE 2 END,id DESC LIMIT ?3")?;
        query
            .query_map(params![account, direction.code(), limit], |row| {
                row.get::<_, i64>(0)
            })?
            .map(|id| nonnegative_from_sql("vault_transfer_jobs", "id", id?))
            .collect()
    }

    /// Bounded restart history page; index excludes unrelated upload jobs.
    pub fn vault_job_ids_by_direction(
        &self,
        account: i64,
        direction: VaultJobDirection,
        before: Option<u64>,
        limit: u16,
    ) -> StorageResult<Vec<u64>> {
        if account <= 0 || limit == 0 || limit > 256 {
            return Err(invalid("vault_job.direction_page"));
        }
        let sql = if before.is_some() {
            "SELECT id FROM vault_transfer_jobs WHERE account_id=?1 AND direction=?2 AND id<?3 ORDER BY id DESC LIMIT ?4"
        } else {
            "SELECT id FROM vault_transfer_jobs WHERE account_id=?1 AND direction=?2 AND id<=?3 ORDER BY id DESC LIMIT ?4"
        };
        let mut stmt = self.connection.prepare(sql)?;
        let ids = stmt
            .query_map(
                params![
                    account,
                    direction.code(),
                    before.map_or(Ok(i64::MAX), |id| unsigned_to_sql("vault_job.cursor", id))?,
                    limit
                ],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| nonnegative_from_sql("vault_transfer_jobs", "id", id))
            .collect()
    }

    pub fn vault_job_count_by_direction(
        &self,
        account: i64,
        direction: VaultJobDirection,
    ) -> StorageResult<u64> {
        if account <= 0 {
            return Err(invalid("vault_job.account"));
        }
        let count = self.connection.query_row(
            "SELECT count(*) FROM vault_transfer_jobs WHERE account_id=?1 AND direction=?2",
            params![account, direction.code()],
            |row| row.get::<_, i64>(0),
        )?;
        nonnegative_from_sql("vault_transfer_jobs", "count", count)
    }

    /// Removes one terminal Vault transfer and all of its durable recovery
    /// metadata in a single transaction. Downloaded output identities are
    /// deliberately kept: history cleanup must never remove a user's file.
    pub fn delete_vault_transfer(&mut self, account: i64, id: u64) -> StorageResult<()> {
        if account <= 0 {
            return Err(invalid("vault_job.account"));
        }
        let id = unsigned_to_sql("vault_job.id", id)?;
        self.durable_vault_write(|tx| {
            let formal: Option<String> = tx
                .query_row(
                    "SELECT state FROM vault_transfer_jobs WHERE account_id=?1 AND id=?2",
                    params![account, id],
                    |row| row.get(0),
                )
                .optional()?;
            let legacy: Option<String> = tx
                .query_row(
                    "SELECT state FROM vault_upload_history WHERE account_id=?1 AND id=?2",
                    params![account, id],
                    |row| row.get(0),
                )
                .optional()?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT state FROM vault_pending_uploads WHERE account_id=?1 AND id=?2",
                    params![account, id],
                    |row| row.get(0),
                )
                .optional()?;

            if formal.is_none() && legacy.is_none() && pending.is_none() {
                return Err(StorageError::NotFound {
                    entity: crate::EntityKind::TransferTask,
                    id,
                });
            }
            if formal
                .as_deref()
                .is_some_and(|state| !terminal_vault_job_state(state))
                || legacy
                    .as_deref()
                    .is_some_and(|state| !terminal_upload_history_state(state))
                || pending
                    .as_deref()
                    .is_some_and(|state| !terminal_pending_upload_state(state))
            {
                return Err(invalid("vault_job.delete_state"));
            }

            tx.execute(
                "DELETE FROM vault_transfer_parts WHERE account_id=?1 AND task_id=?2",
                params![account, id],
            )?;
            tx.execute(
                "DELETE FROM vault_manifest_outbox WHERE account_id=?1 AND task_id=?2",
                params![account, id],
            )?;
            tx.execute(
                "DELETE FROM vault_transfer_jobs WHERE account_id=?1 AND id=?2",
                params![account, id],
            )?;
            tx.execute(
                "DELETE FROM vault_pending_uploads WHERE account_id=?1 AND id=?2",
                params![account, id],
            )?;
            tx.execute(
                "DELETE FROM vault_upload_history WHERE account_id=?1 AND id=?2",
                params![account, id],
            )?;
            Ok(())
        })
    }

    /// Authenticated remote locators are imported atomically with their local job.
    /// Runtime rechecks their ciphertext before issuing any completion receipt.
    pub fn import_vault_upload(
        &mut self,
        pending: &PendingVaultUpload,
        generation: u64,
        job: &VaultJobRecord,
        parts: &[VaultPartRecord],
    ) -> StorageResult<bool> {
        validate_admission(job)?;
        if pending.account_id != job.account_id
            || pending.id != job.id
            || pending.chat_id != job.chat_id
            || pending.created_at_unix_ms != job.created_at_unix_ms
            || pending.codec_version != 1
            || job.direction != VaultJobDirection::Upload
            || parts.len() > 1_000_000
            || parts.iter().enumerate().any(|(index, part)| {
                part.part_index as usize != index
                    || part.identity.is_empty()
                    || part.identity.len() > 16384
                    || part
                        .receipt
                        .as_ref()
                        .is_none_or(|receipt| receipt.is_empty() || receipt.len() > 16384)
            })
        {
            return Err(invalid("vault_job.import"));
        }
        self.durable_vault_write(|tx| {
            let changed = tx.execute("UPDATE vault_pending_uploads SET state='promoted',generation=generation+1 WHERE account_id=?1 AND id=?2 AND context=?3 AND codec_version=1 AND state='queued' AND generation=?4", params![pending.account_id,unsigned_to_sql("pending_upload.id",pending.id)?,pending.context,unsigned_to_sql("pending_upload.generation",generation)?])?;
            if changed == 0 { return Ok(false); }
            if !admit_job(tx, job)? { return Err(invalid("vault_job.import_conflict")); }
            for part in parts {
                tx.execute("INSERT INTO vault_transfer_parts(account_id,task_id,part_index,identity,receipt) VALUES(?1,?2,?3,?4,?5)", params![job.account_id,unsigned_to_sql("vault_job.id",job.id)?,part.part_index,part.identity,part.receipt])?;
            }
            Ok(true)
        })
    }

    /// Reserve before encryption; retrying a different identity is forbidden.
    pub fn reserve_vault_part(
        &mut self,
        lease: VaultJobLease,
        part: &VaultPartRecord,
    ) -> StorageResult<bool> {
        if part.identity.is_empty() || part.identity.len() > 16384 || part.receipt.is_some() {
            return Err(invalid("vault_job.part"));
        }
        self.durable_vault_write(|tx| {
            if !active_lease(tx, lease, false)? { return Ok(false); }
            let id=unsigned_to_sql("vault_job.id",lease.id)?;
            tx.execute("INSERT INTO vault_transfer_parts(account_id,task_id,part_index,identity) VALUES(?1,?2,?3,?4) ON CONFLICT DO NOTHING",params![lease.account_id,id,part.part_index,part.identity])?;
            let same: bool=tx.query_row("SELECT identity=?4 FROM vault_transfer_parts WHERE account_id=?1 AND task_id=?2 AND part_index=?3",params![lease.account_id,id,part.part_index,part.identity],|row|row.get(0))?;
            if !same { return Err(invalid("vault_job.part_identity")); }
            Ok(true)
        })
    }

    /// Atomically start a fresh expired upload attempt, retaining the old attempt
    /// if any scope/state check fails. No newer codec is rewritten.
    pub fn restart_vault_upload(
        &mut self,
        lease: VaultJobLease,
        previous: &[u8],
        replacement: &VaultJobRecord,
    ) -> StorageResult<bool> {
        validate_admission(replacement)?;
        if replacement.account_id != lease.account_id
            || replacement.id != lease.id
            || !matches!(replacement.context_version, 1 | 2)
            || replacement.direction != VaultJobDirection::Upload
            || replacement.context == previous
        {
            return Err(invalid("vault_job.restart"));
        }
        self.durable_vault_write(|tx| {
            let id=unsigned_to_sql("vault_job.id",lease.id)?;
            let changed=tx.execute("UPDATE vault_transfer_jobs SET package_id=?5,context=?6,context_version=?9,created_at=?7,updated_at=?7 WHERE account_id=?1 AND id=?2 AND generation=?3 AND context=?4 AND context_version IN (1,2) AND state='queued' AND direction='upload' AND chat_id=?8",
                params![lease.account_id,id,unsigned_to_sql("vault_job.generation",lease.generation)?,previous,replacement.package_id,replacement.context,replacement.created_at_unix_ms,replacement.chat_id,replacement.context_version])?;
            if changed!=1 {return Ok(false);}
            tx.execute("DELETE FROM vault_transfer_parts WHERE account_id=?1 AND task_id=?2",params![lease.account_id,id])?;
            tx.execute("DELETE FROM vault_manifest_outbox WHERE account_id=?1 AND task_id=?2",params![lease.account_id,id])?;
            Ok(true)
        })
    }

    /// Burn an unreceipted reservation whose immutable ciphertext is unavailable.
    /// The replacement must have a fresh cryptographic/publication identity.
    pub fn replace_unpublished_vault_part(
        &mut self,
        lease: VaultJobLease,
        index: u32,
        previous: &[u8],
        replacement: &[u8],
    ) -> StorageResult<bool> {
        if replacement.is_empty() || replacement.len() > 16384 || previous == replacement {
            return Err(invalid("vault_job.part_identity"));
        }
        self.durable_vault_write(|tx| {
            if !active_lease(tx,lease,false)? { return Ok(false); }
            Ok(tx.execute("UPDATE vault_transfer_parts SET identity=?5 WHERE account_id=?1 AND task_id=?2 AND part_index=?3 AND identity=?4 AND receipt IS NULL",
                params![lease.account_id,unsigned_to_sql("vault_job.id",lease.id)?,index,previous,replacement])? == 1)
        })
    }

    /// A late successful operation is receipted while stopping, so resume does
    /// not resend blindly. A stopped/new-generation worker cannot write receipts.
    pub fn confirm_vault_part(
        &mut self,
        lease: VaultJobLease,
        part_index: u32,
        receipt: &[u8],
    ) -> StorageResult<bool> {
        if receipt.is_empty() || receipt.len() > 16384 {
            return Err(invalid("vault_job.receipt"));
        }
        self.durable_vault_write(|tx| {
            if !active_lease(tx,lease,true)? { return Ok(false); }
            let id=unsigned_to_sql("vault_job.id",lease.id)?;
            Ok(tx.execute("UPDATE vault_transfer_parts SET receipt=?4 WHERE account_id=?1 AND task_id=?2 AND part_index=?3 AND (receipt IS NULL OR receipt=?4)", params![lease.account_id,id,part_index,receipt])? == 1)
        })
    }

    pub fn last_confirmed_vault_part(&self, account: i64, task: u64) -> StorageResult<Option<u32>> {
        self.connection.query_row("SELECT part_index FROM vault_transfer_parts WHERE account_id=?1 AND task_id=?2 AND receipt IS NOT NULL ORDER BY part_index DESC LIMIT 1", params![account,unsigned_to_sql("vault_job.id",task)?], |row| row.get(0)).optional().map_err(Into::into)
    }

    pub fn vault_parts(
        &self,
        account: i64,
        task: u64,
        after: Option<u32>,
        limit: u16,
    ) -> StorageResult<Vec<VaultPartRecord>> {
        if account <= 0 || limit == 0 || limit > 256 {
            return Err(invalid("vault_job.parts_page"));
        }
        let mut stmt=self.connection.prepare("SELECT part_index,identity,receipt FROM vault_transfer_parts WHERE account_id=?1 AND task_id=?2 AND part_index>?3 ORDER BY part_index LIMIT ?4")?;
        Ok(stmt
            .query_map(
                params![
                    account,
                    unsigned_to_sql("vault_job.id", task)?,
                    after.map_or(-1, i64::from),
                    limit
                ],
                |row| {
                    Ok(VaultPartRecord {
                        part_index: row.get(0)?,
                        identity: row.get(1)?,
                        receipt: row.get(2)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?)
    }
}

fn terminal_vault_job_state(state: &str) -> bool {
    matches!(state, "cancelled" | "retryable" | "blocked" | "completed")
}

fn terminal_upload_history_state(state: &str) -> bool {
    matches!(state, "failed" | "cancelled" | "interrupted" | "completed")
}

fn terminal_pending_upload_state(state: &str) -> bool {
    matches!(state, "cancelled" | "retryable" | "blocked" | "promoted")
}

fn active_lease(tx: &Transaction<'_>, lease: VaultJobLease, stopping: bool) -> StorageResult<bool> {
    let value: Option<(String,i64,u32)>=tx.query_row("SELECT state,generation,context_version FROM vault_transfer_jobs WHERE account_id=?1 AND id=?2",params![lease.account_id,unsigned_to_sql("vault_job.id",lease.id)?],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    Ok(value.is_some_and(|(state, generation, version)| {
        matches!(version, 1 | 2)
            && u64::try_from(generation).ok() == Some(lease.generation)
            && (state == "running"
                || stopping && matches!(state.as_str(), "pausing" | "cancelling"))
    }))
}

#[cfg(test)]
mod tests;

fn validate_admission(job: &VaultJobRecord) -> StorageResult<()> {
    if job.account_id <= 0
        || job.chat_id <= 0
        || job.id == 0
        || job.generation != 0
        || job.state != VaultJobState::Queued
        || job.failure_code.is_some()
        || job.created_at_unix_ms < 0
        || job.updated_at_unix_ms != job.created_at_unix_ms
        || !matches!(job.context_version, 1 | 2)
        || job.context.is_empty()
        || job.context.len() > 2 * 1024 * 1024
        || job.package_id == [0; 16]
    {
        return Err(invalid("vault_job.admission"));
    }
    Ok(())
}

fn admit_job(tx: &Transaction<'_>, job: &VaultJobRecord) -> StorageResult<bool> {
    let id = unsigned_to_sql("vault_job.id", job.id)?;
    let changed = tx.execute("INSERT INTO vault_transfer_jobs(account_id,id,chat_id,direction,package_id,context_version,context,state,generation,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?8,?6,'queued',0,?7,?7) ON CONFLICT(account_id,id) DO NOTHING", params![job.account_id,id,job.chat_id,job.direction.code(),job.package_id.as_slice(),job.context,job.created_at_unix_ms,job.context_version])?;
    if changed == 0 {
        let same: bool = tx.query_row("SELECT chat_id=?3 AND direction=?4 AND package_id=?5 AND context_version=?8 AND context=?6 AND created_at=?7 FROM vault_transfer_jobs WHERE account_id=?1 AND id=?2", params![job.account_id,id,job.chat_id,job.direction.code(),job.package_id.as_slice(),job.context,job.created_at_unix_ms,job.context_version], |row|row.get(0))?;
        if !same {
            return Err(invalid("vault_job.identity"));
        }
    }
    Ok(changed == 1)
}
