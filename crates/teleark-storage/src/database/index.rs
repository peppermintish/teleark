use rusqlite::{OptionalExtension, params};
use teleark_core::{AccountId, ChatId, IndexJobId, MessageId};

use super::{
    Database, corrupt, id_from_sql, id_to_sql, nonnegative_from_sql, transaction, unsigned_to_sql,
    upsert_logical_file_on, validate_file,
};
use crate::error::{InputReason, InvariantViolation};
use crate::model::{
    IndexBatch, IndexJobRecord, IndexRangeRecord, StoredIndexCoverage, StoredIndexJobState,
};
use crate::{StorageError, StorageResult};

const MAX_POLICY_FINGERPRINT_BYTES: usize = 512;

impl Database {
    pub fn save_index_job(&mut self, job: &IndexJobRecord) -> StorageResult<()> {
        validate_index_job(job)?;
        upsert_index_job(&self.connection, job)
    }

    pub fn index_job(&self, id: IndexJobId) -> StorageResult<Option<IndexJobRecord>> {
        let id = id_to_sql("index_job.id", id.get())?;
        query_index_job(&self.connection, id)
    }

    pub fn resumable_index_jobs(&self) -> StorageResult<Vec<IndexJobRecord>> {
        let mut statement = self.connection.prepare(
            r#"
SELECT
    id, account_id, chat_id, state, policy_version, policy_fingerprint,
    requested_start_message_id, requested_end_message_id, checkpoint_message_id,
    messages_scanned, files_indexed, last_error_code,
    created_at_unix_ms, updated_at_unix_ms
FROM index_jobs
WHERE state NOT IN ('completed', 'cancelled')
ORDER BY updated_at_unix_ms, id
"#,
        )?;
        let mut rows = statement.query([])?;
        let mut jobs = Vec::new();
        while let Some(row) = rows.next()? {
            jobs.push(row_to_index_job(row)?);
        }
        Ok(jobs)
    }

    pub fn save_index_range(&mut self, range: &IndexRangeRecord) -> StorageResult<()> {
        validate_index_range(range)?;
        let transaction = transaction(&mut self.connection)?;
        ensure_no_range_overlap(&transaction, range)?;
        upsert_index_range(&transaction, range)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn index_ranges(
        &self,
        account_id: AccountId,
        chat_id: ChatId,
        policy_version: u32,
        policy_fingerprint: &str,
    ) -> StorageResult<Vec<IndexRangeRecord>> {
        validate_policy(policy_version, policy_fingerprint)?;
        let mut statement = self.connection.prepare(
            r#"
SELECT
    account_id, chat_id, start_message_id, end_message_id, coverage,
    checkpoint_message_id, messages_scanned, files_indexed,
    policy_version, policy_fingerprint, scan_generation, updated_at_unix_ms
FROM index_ranges
WHERE account_id = ?1 AND chat_id = ?2
  AND policy_version = ?3 AND policy_fingerprint = ?4
ORDER BY start_message_id, end_message_id
"#,
        )?;
        let mut rows = statement.query(params![
            account_id.get(),
            chat_id.get(),
            i64::from(policy_version),
            policy_fingerprint
        ])?;
        let mut ranges = Vec::new();
        while let Some(row) = rows.next()? {
            ranges.push(row_to_index_range(row)?);
        }
        Ok(ranges)
    }

    /// Atomically commits normalized file upserts, job progress, and range evidence.
    pub fn apply_index_batch(&mut self, batch: &IndexBatch) -> StorageResult<()> {
        validate_index_job(&batch.job)?;
        validate_index_range(&batch.range)?;
        if batch.job.account_id != batch.range.account_id
            || batch.job.chat_id != batch.range.chat_id
            || batch.job.policy_version != batch.range.policy_version
            || batch.job.policy_fingerprint != batch.range.policy_fingerprint
        {
            return Err(StorageError::InvalidInput {
                field: "index_batch.scope",
                reason: InputReason::InvalidCombination,
            });
        }
        for file in &batch.files {
            validate_file(file)?;
        }

        let transaction = transaction(&mut self.connection)?;
        upsert_index_job(&transaction, &batch.job)?;
        for file in &batch.files {
            upsert_logical_file_on(&transaction, file)?;
        }
        ensure_no_range_overlap(&transaction, &batch.range)?;
        upsert_index_range(&transaction, &batch.range)?;
        transaction.commit()?;
        Ok(())
    }
}

fn validate_policy(version: u32, fingerprint: &str) -> StorageResult<()> {
    if version == 0 {
        return Err(StorageError::InvalidInput {
            field: "index.policy_version",
            reason: InputReason::OutOfRange,
        });
    }
    if fingerprint.is_empty() {
        return Err(StorageError::InvalidInput {
            field: "index.policy_fingerprint",
            reason: InputReason::Empty,
        });
    }
    if fingerprint.len() > MAX_POLICY_FINGERPRINT_BYTES {
        return Err(StorageError::InvalidInput {
            field: "index.policy_fingerprint",
            reason: InputReason::TooLong,
        });
    }
    Ok(())
}

fn validate_index_job(job: &IndexJobRecord) -> StorageResult<()> {
    id_to_sql("index_job.id", job.id.get())?;
    validate_policy(job.policy_version, &job.policy_fingerprint)?;
    unsigned_to_sql("index_job.messages_scanned", job.messages_scanned)?;
    unsigned_to_sql("index_job.files_indexed", job.files_indexed)?;
    if job
        .requested_start_message_id
        .zip(job.requested_end_message_id)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(StorageError::InvalidInput {
            field: "index_job.requested_range",
            reason: InputReason::InvalidCombination,
        });
    }
    if job.checkpoint_message_id.is_some_and(|checkpoint| {
        job.requested_start_message_id
            .is_some_and(|start| checkpoint < start)
            || job
                .requested_end_message_id
                .is_some_and(|end| checkpoint > end)
    }) {
        return Err(StorageError::Invariant(
            InvariantViolation::IndexCheckpointOutsideRange,
        ));
    }
    Ok(())
}

fn validate_index_range(range: &IndexRangeRecord) -> StorageResult<()> {
    validate_policy(range.policy_version, &range.policy_fingerprint)?;
    unsigned_to_sql("index_range.messages_scanned", range.messages_scanned)?;
    unsigned_to_sql("index_range.files_indexed", range.files_indexed)?;
    unsigned_to_sql("index_range.scan_generation", range.scan_generation)?;
    if range.start_message_id > range.end_message_id {
        return Err(StorageError::InvalidInput {
            field: "index_range.bounds",
            reason: InputReason::InvalidCombination,
        });
    }
    if range.checkpoint_message_id.is_some_and(|checkpoint| {
        checkpoint < range.start_message_id || checkpoint > range.end_message_id
    }) {
        return Err(StorageError::Invariant(
            InvariantViolation::IndexCheckpointOutsideRange,
        ));
    }
    Ok(())
}

fn upsert_index_job(connection: &rusqlite::Connection, job: &IndexJobRecord) -> StorageResult<()> {
    connection.execute(
        r#"
INSERT INTO index_jobs (
    id, account_id, chat_id, state, policy_version, policy_fingerprint,
    requested_start_message_id, requested_end_message_id, checkpoint_message_id,
    messages_scanned, files_indexed, last_error_code,
    created_at_unix_ms, updated_at_unix_ms
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
ON CONFLICT(id) DO UPDATE SET
    account_id = excluded.account_id,
    chat_id = excluded.chat_id,
    state = excluded.state,
    policy_version = excluded.policy_version,
    policy_fingerprint = excluded.policy_fingerprint,
    requested_start_message_id = excluded.requested_start_message_id,
    requested_end_message_id = excluded.requested_end_message_id,
    checkpoint_message_id = excluded.checkpoint_message_id,
    messages_scanned = excluded.messages_scanned,
    files_indexed = excluded.files_indexed,
    last_error_code = excluded.last_error_code,
    updated_at_unix_ms = excluded.updated_at_unix_ms
"#,
        params![
            id_to_sql("index_job.id", job.id.get())?,
            job.account_id.get(),
            job.chat_id.get(),
            index_job_state_code(job.state),
            i64::from(job.policy_version),
            job.policy_fingerprint,
            job.requested_start_message_id.map(MessageId::get),
            job.requested_end_message_id.map(MessageId::get),
            job.checkpoint_message_id.map(MessageId::get),
            unsigned_to_sql("index_job.messages_scanned", job.messages_scanned)?,
            unsigned_to_sql("index_job.files_indexed", job.files_indexed)?,
            job.last_error_code,
            job.created_at_unix_ms,
            job.updated_at_unix_ms,
        ],
    )?;
    Ok(())
}

fn query_index_job(
    connection: &rusqlite::Connection,
    id: i64,
) -> StorageResult<Option<IndexJobRecord>> {
    let mut statement = connection.prepare(
        r#"
SELECT
    id, account_id, chat_id, state, policy_version, policy_fingerprint,
    requested_start_message_id, requested_end_message_id, checkpoint_message_id,
    messages_scanned, files_indexed, last_error_code,
    created_at_unix_ms, updated_at_unix_ms
FROM index_jobs WHERE id = ?1
"#,
    )?;
    let mut rows = statement.query([id])?;
    rows.next()?.map(row_to_index_job).transpose()
}

fn row_to_index_job(row: &rusqlite::Row<'_>) -> StorageResult<IndexJobRecord> {
    let id: i64 = row.get(0)?;
    let raw_state: String = row.get(3)?;
    let policy_version: i64 = row.get(4)?;
    let messages_scanned: i64 = row.get(9)?;
    let files_indexed: i64 = row.get(10)?;
    Ok(IndexJobRecord {
        id: IndexJobId::new(id_from_sql("index_jobs", "id", id)?),
        account_id: AccountId::new(row.get(1)?),
        chat_id: ChatId::new(row.get(2)?),
        state: parse_index_job_state(&raw_state)?,
        policy_version: u32::try_from(policy_version)
            .map_err(|_| corrupt("index_jobs", "policy_version", policy_version))?,
        policy_fingerprint: row.get(5)?,
        requested_start_message_id: row.get::<_, Option<i64>>(6)?.map(MessageId::new),
        requested_end_message_id: row.get::<_, Option<i64>>(7)?.map(MessageId::new),
        checkpoint_message_id: row.get::<_, Option<i64>>(8)?.map(MessageId::new),
        messages_scanned: nonnegative_from_sql("index_jobs", "messages_scanned", messages_scanned)?,
        files_indexed: nonnegative_from_sql("index_jobs", "files_indexed", files_indexed)?,
        last_error_code: row.get(11)?,
        created_at_unix_ms: row.get(12)?,
        updated_at_unix_ms: row.get(13)?,
    })
}

fn ensure_no_range_overlap(
    connection: &rusqlite::Connection,
    range: &IndexRangeRecord,
) -> StorageResult<()> {
    let overlap: Option<i64> = connection
        .query_row(
            r#"
SELECT id FROM index_ranges
WHERE account_id = ?1 AND chat_id = ?2
  AND policy_version = ?3 AND policy_fingerprint = ?4
  AND NOT (end_message_id < ?5 OR start_message_id > ?6)
  AND NOT (start_message_id = ?5 AND end_message_id = ?6)
LIMIT 1
"#,
            params![
                range.account_id.get(),
                range.chat_id.get(),
                i64::from(range.policy_version),
                range.policy_fingerprint,
                range.start_message_id.get(),
                range.end_message_id.get(),
            ],
            |row| row.get(0),
        )
        .optional()?;
    if overlap.is_some() {
        Err(StorageError::Invariant(
            InvariantViolation::OverlappingIndexRange,
        ))
    } else {
        Ok(())
    }
}

fn upsert_index_range(
    connection: &rusqlite::Connection,
    range: &IndexRangeRecord,
) -> StorageResult<()> {
    connection.execute(
        r#"
INSERT INTO index_ranges (
    account_id, chat_id, start_message_id, end_message_id, coverage,
    checkpoint_message_id, messages_scanned, files_indexed,
    policy_version, policy_fingerprint, scan_generation, updated_at_unix_ms
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
ON CONFLICT(
    account_id, chat_id, policy_version, policy_fingerprint,
    start_message_id, end_message_id
) DO UPDATE SET
    coverage = excluded.coverage,
    checkpoint_message_id = excluded.checkpoint_message_id,
    messages_scanned = excluded.messages_scanned,
    files_indexed = excluded.files_indexed,
    scan_generation = excluded.scan_generation,
    updated_at_unix_ms = excluded.updated_at_unix_ms
"#,
        params![
            range.account_id.get(),
            range.chat_id.get(),
            range.start_message_id.get(),
            range.end_message_id.get(),
            index_coverage_code(range.coverage),
            range.checkpoint_message_id.map(MessageId::get),
            unsigned_to_sql("index_range.messages_scanned", range.messages_scanned)?,
            unsigned_to_sql("index_range.files_indexed", range.files_indexed)?,
            i64::from(range.policy_version),
            range.policy_fingerprint,
            unsigned_to_sql("index_range.scan_generation", range.scan_generation)?,
            range.updated_at_unix_ms,
        ],
    )?;
    Ok(())
}

fn row_to_index_range(row: &rusqlite::Row<'_>) -> StorageResult<IndexRangeRecord> {
    let coverage: String = row.get(4)?;
    let messages_scanned: i64 = row.get(6)?;
    let files_indexed: i64 = row.get(7)?;
    let policy_version: i64 = row.get(8)?;
    let scan_generation: i64 = row.get(10)?;
    Ok(IndexRangeRecord {
        account_id: AccountId::new(row.get(0)?),
        chat_id: ChatId::new(row.get(1)?),
        start_message_id: MessageId::new(row.get(2)?),
        end_message_id: MessageId::new(row.get(3)?),
        coverage: parse_index_coverage(&coverage)?,
        checkpoint_message_id: row.get::<_, Option<i64>>(5)?.map(MessageId::new),
        messages_scanned: nonnegative_from_sql(
            "index_ranges",
            "messages_scanned",
            messages_scanned,
        )?,
        files_indexed: nonnegative_from_sql("index_ranges", "files_indexed", files_indexed)?,
        policy_version: u32::try_from(policy_version)
            .map_err(|_| corrupt("index_ranges", "policy_version", policy_version))?,
        policy_fingerprint: row.get(9)?,
        scan_generation: nonnegative_from_sql("index_ranges", "scan_generation", scan_generation)?,
        updated_at_unix_ms: row.get(11)?,
    })
}

fn index_job_state_code(value: StoredIndexJobState) -> &'static str {
    match value {
        StoredIndexJobState::Queued => "queued",
        StoredIndexJobState::Running => "running",
        StoredIndexJobState::Paused => "paused",
        StoredIndexJobState::Completed => "completed",
        StoredIndexJobState::Failed => "failed",
        StoredIndexJobState::Cancelled => "cancelled",
    }
}

fn parse_index_job_state(value: &str) -> StorageResult<StoredIndexJobState> {
    match value {
        "queued" => Ok(StoredIndexJobState::Queued),
        "running" => Ok(StoredIndexJobState::Running),
        "paused" => Ok(StoredIndexJobState::Paused),
        "completed" => Ok(StoredIndexJobState::Completed),
        "failed" => Ok(StoredIndexJobState::Failed),
        "cancelled" => Ok(StoredIndexJobState::Cancelled),
        value => Err(corrupt("index_jobs", "state", value)),
    }
}

fn index_coverage_code(value: StoredIndexCoverage) -> &'static str {
    match value {
        StoredIndexCoverage::Partial => "partial",
        StoredIndexCoverage::Complete => "complete",
    }
}

fn parse_index_coverage(value: &str) -> StorageResult<StoredIndexCoverage> {
    match value {
        "partial" => Ok(StoredIndexCoverage::Partial),
        "complete" => Ok(StoredIndexCoverage::Complete),
        value => Err(corrupt("index_ranges", "coverage", value)),
    }
}
