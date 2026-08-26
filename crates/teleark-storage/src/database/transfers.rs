use rusqlite::params;
use teleark_core::{AccountId, LogicalFileId, PartIndex, TransferId};

use super::{
    Database, corrupt, id_from_sql, id_to_sql, nonnegative_from_sql, transaction, unsigned_to_sql,
};
use crate::error::{InputReason, InvariantViolation};
use crate::model::{
    StoredPartState, StoredTransferDirection, StoredTransferState, TransferPartCheckpoint,
    TransferTaskRecord,
};
use crate::{StorageError, StorageResult};

const MAX_CHECKPOINT_BYTES: usize = 16 * 1_048_576;

impl Database {
    /// Atomically persists a logical-file transfer and all of its application-part checkpoints.
    pub fn save_transfer(
        &mut self,
        task: &TransferTaskRecord,
        parts: &[TransferPartCheckpoint],
    ) -> StorageResult<()> {
        validate_transfer(task, parts)?;
        let transaction = transaction(&mut self.connection)?;
        upsert_transfer_task(&transaction, task)?;
        let task_id = id_to_sql("transfer.id", task.id.get())?;
        transaction.execute(
            "DELETE FROM transfer_parts WHERE transfer_id = ?1",
            [task_id],
        )?;
        for part in parts {
            insert_transfer_part(&transaction, part)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn transfer(
        &self,
        id: TransferId,
    ) -> StorageResult<Option<(TransferTaskRecord, Vec<TransferPartCheckpoint>)>> {
        let id = id_to_sql("transfer.id", id.get())?;
        let task = query_transfer_task(&self.connection, id)?;
        task.map(|task| query_transfer_parts(&self.connection, id).map(|parts| (task, parts)))
            .transpose()
    }

    /// Returns all non-terminal work in deterministic scheduler order.
    pub fn resumable_transfers(
        &self,
    ) -> StorageResult<Vec<(TransferTaskRecord, Vec<TransferPartCheckpoint>)>> {
        let mut statement = self.connection.prepare(
            r#"
SELECT
    id, logical_file_id, account_id, direction, priority, state,
    total_bytes, transferred_bytes, source_path, destination_path,
    retry_count, next_retry_at_unix_ms, last_error_code,
    created_at_unix_ms, updated_at_unix_ms
FROM transfer_tasks
WHERE state NOT IN ('completed', 'cancelled')
ORDER BY priority DESC, created_at_unix_ms, id
"#,
        )?;
        let mut rows = statement.query([])?;
        let mut tasks = Vec::new();
        while let Some(row) = rows.next()? {
            tasks.push(row_to_transfer_task(row)?);
        }
        drop(rows);
        drop(statement);

        let mut result = Vec::with_capacity(tasks.len());
        for task in tasks {
            let id = id_to_sql("transfer.id", task.id.get())?;
            let parts = query_transfer_parts(&self.connection, id)?;
            result.push((task, parts));
        }
        Ok(result)
    }

    pub fn delete_transfer(&mut self, id: TransferId) -> StorageResult<bool> {
        let id = id_to_sql("transfer.id", id.get())?;
        Ok(self
            .connection
            .execute("DELETE FROM transfer_tasks WHERE id = ?1", [id])?
            != 0)
    }

    /// Requeues work interrupted by process termination while preserving durable bytes/checkpoints.
    /// Returns `(transfer_tasks_requeued, index_jobs_requeued)`.
    pub fn recover_interrupted_work(
        &mut self,
        timestamp_unix_ms: i64,
    ) -> StorageResult<(u64, u64)> {
        let transaction = transaction(&mut self.connection)?;
        let transfer_count = transaction.execute(
            r#"
UPDATE transfer_tasks
SET state = 'queued', updated_at_unix_ms = ?1
WHERE state IN ('running', 'verifying')
"#,
            [timestamp_unix_ms],
        )?;
        transaction.execute(
            r#"
UPDATE transfer_parts
SET state = 'queued', updated_at_unix_ms = ?1
WHERE state IN ('running', 'verifying')
"#,
            [timestamp_unix_ms],
        )?;
        let index_count = transaction.execute(
            r#"
UPDATE index_jobs
SET state = 'queued', updated_at_unix_ms = ?1
WHERE state = 'running'
"#,
            [timestamp_unix_ms],
        )?;
        transaction.commit()?;
        Ok((transfer_count as u64, index_count as u64))
    }
}

fn validate_transfer(
    task: &TransferTaskRecord,
    parts: &[TransferPartCheckpoint],
) -> StorageResult<()> {
    id_to_sql("transfer.id", task.id.get())?;
    id_to_sql("transfer.logical_file_id", task.logical_file_id.get())?;
    unsigned_to_sql("transfer.total_bytes", task.total_bytes)?;
    unsigned_to_sql("transfer.transferred_bytes", task.transferred_bytes)?;
    if task.transferred_bytes > task.total_bytes {
        return Err(StorageError::Invariant(
            InvariantViolation::TransferProgress,
        ));
    }

    let mut expected_offset = 0_u64;
    let mut transferred_total = 0_u64;
    for (position, part) in parts.iter().enumerate() {
        let expected_index = u32::try_from(position).map_err(|_| StorageError::InvalidInput {
            field: "transfer.parts",
            reason: InputReason::OutOfRange,
        })?;
        if part.transfer_id != task.id || part.index.get() != expected_index {
            return Err(StorageError::Invariant(
                InvariantViolation::TransferPartIndices,
            ));
        }
        if part.offset_bytes != expected_offset {
            return Err(StorageError::Invariant(
                InvariantViolation::TransferPartOffsets,
            ));
        }
        if part.size_bytes == 0 || part.transferred_bytes > part.size_bytes {
            return Err(StorageError::Invariant(
                InvariantViolation::TransferProgress,
            ));
        }
        if part.checkpoint_version.is_some() != part.checkpoint_data.is_some() {
            return Err(StorageError::InvalidInput {
                field: "transfer_part.checkpoint",
                reason: InputReason::InvalidCombination,
            });
        }
        if part
            .checkpoint_data
            .as_ref()
            .is_some_and(|data| data.len() > MAX_CHECKPOINT_BYTES)
        {
            return Err(StorageError::InvalidInput {
                field: "transfer_part.checkpoint_data",
                reason: InputReason::TooLong,
            });
        }
        expected_offset =
            expected_offset
                .checked_add(part.size_bytes)
                .ok_or(StorageError::InvalidInput {
                    field: "transfer.parts",
                    reason: InputReason::OutOfRange,
                })?;
        transferred_total = transferred_total
            .checked_add(part.transferred_bytes)
            .ok_or(StorageError::InvalidInput {
                field: "transfer.transferred_bytes",
                reason: InputReason::OutOfRange,
            })?;
    }
    if expected_offset != task.total_bytes {
        return Err(StorageError::Invariant(
            InvariantViolation::TransferPartTotal,
        ));
    }
    if transferred_total != task.transferred_bytes {
        return Err(StorageError::Invariant(
            InvariantViolation::TransferProgress,
        ));
    }
    if task.state == StoredTransferState::Completed
        && (!parts.iter().all(|part| {
            part.state == StoredPartState::Verified && part.transferred_bytes == part.size_bytes
        }) || task.transferred_bytes != task.total_bytes)
    {
        return Err(StorageError::Invariant(
            InvariantViolation::TransferProgress,
        ));
    }
    Ok(())
}

fn upsert_transfer_task(
    connection: &rusqlite::Connection,
    task: &TransferTaskRecord,
) -> StorageResult<()> {
    connection.execute(
        r#"
INSERT INTO transfer_tasks (
    id, logical_file_id, account_id, direction, priority, state,
    total_bytes, transferred_bytes, source_path, destination_path,
    retry_count, next_retry_at_unix_ms, last_error_code,
    created_at_unix_ms, updated_at_unix_ms
) VALUES (
    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
)
ON CONFLICT(id) DO UPDATE SET
    logical_file_id = excluded.logical_file_id,
    account_id = excluded.account_id,
    direction = excluded.direction,
    priority = excluded.priority,
    state = excluded.state,
    total_bytes = excluded.total_bytes,
    transferred_bytes = excluded.transferred_bytes,
    source_path = excluded.source_path,
    destination_path = excluded.destination_path,
    retry_count = excluded.retry_count,
    next_retry_at_unix_ms = excluded.next_retry_at_unix_ms,
    last_error_code = excluded.last_error_code,
    updated_at_unix_ms = excluded.updated_at_unix_ms
"#,
        params![
            id_to_sql("transfer.id", task.id.get())?,
            id_to_sql("transfer.logical_file_id", task.logical_file_id.get())?,
            task.account_id.map(AccountId::get),
            transfer_direction_code(task.direction),
            i64::from(task.priority),
            transfer_state_code(task.state),
            unsigned_to_sql("transfer.total_bytes", task.total_bytes)?,
            unsigned_to_sql("transfer.transferred_bytes", task.transferred_bytes)?,
            task.source_path,
            task.destination_path,
            i64::from(task.retry_count),
            task.next_retry_at_unix_ms,
            task.last_error_code,
            task.created_at_unix_ms,
            task.updated_at_unix_ms,
        ],
    )?;
    Ok(())
}

fn insert_transfer_part(
    connection: &rusqlite::Connection,
    part: &TransferPartCheckpoint,
) -> StorageResult<()> {
    connection.execute(
        r#"
INSERT INTO transfer_parts (
    transfer_id, part_index, offset_bytes, size_bytes, transferred_bytes,
    state, attempts, remote_object_id, checkpoint_version, checkpoint_data,
    updated_at_unix_ms
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
"#,
        params![
            id_to_sql("transfer_part.transfer_id", part.transfer_id.get())?,
            i64::from(part.index.get()),
            unsigned_to_sql("transfer_part.offset_bytes", part.offset_bytes)?,
            unsigned_to_sql("transfer_part.size_bytes", part.size_bytes)?,
            unsigned_to_sql("transfer_part.transferred_bytes", part.transferred_bytes)?,
            part_state_code(part.state),
            i64::from(part.attempts),
            part.remote_object_id
                .map(|value| id_to_sql("transfer_part.remote_object_id", value))
                .transpose()?,
            part.checkpoint_version.map(i64::from),
            part.checkpoint_data,
            part.updated_at_unix_ms,
        ],
    )?;
    Ok(())
}

fn query_transfer_task(
    connection: &rusqlite::Connection,
    id: i64,
) -> StorageResult<Option<TransferTaskRecord>> {
    let mut statement = connection.prepare(
        r#"
SELECT
    id, logical_file_id, account_id, direction, priority, state,
    total_bytes, transferred_bytes, source_path, destination_path,
    retry_count, next_retry_at_unix_ms, last_error_code,
    created_at_unix_ms, updated_at_unix_ms
FROM transfer_tasks WHERE id = ?1
"#,
    )?;
    let mut rows = statement.query([id])?;
    rows.next()?.map(row_to_transfer_task).transpose()
}

fn query_transfer_parts(
    connection: &rusqlite::Connection,
    id: i64,
) -> StorageResult<Vec<TransferPartCheckpoint>> {
    let mut statement = connection.prepare(
        r#"
SELECT
    transfer_id, part_index, offset_bytes, size_bytes, transferred_bytes,
    state, attempts, remote_object_id, checkpoint_version, checkpoint_data,
    updated_at_unix_ms
FROM transfer_parts WHERE transfer_id = ?1 ORDER BY part_index
"#,
    )?;
    let mut rows = statement.query([id])?;
    let mut parts = Vec::new();
    while let Some(row) = rows.next()? {
        parts.push(row_to_transfer_part(row)?);
    }
    Ok(parts)
}

fn row_to_transfer_task(row: &rusqlite::Row<'_>) -> StorageResult<TransferTaskRecord> {
    let id: i64 = row.get(0)?;
    let logical_file_id: i64 = row.get(1)?;
    let direction: String = row.get(3)?;
    let priority: i64 = row.get(4)?;
    let state: String = row.get(5)?;
    let total_bytes: i64 = row.get(6)?;
    let transferred_bytes: i64 = row.get(7)?;
    let retry_count: i64 = row.get(10)?;
    Ok(TransferTaskRecord {
        id: TransferId::new(id_from_sql("transfer_tasks", "id", id)?),
        logical_file_id: LogicalFileId::new(id_from_sql(
            "transfer_tasks",
            "logical_file_id",
            logical_file_id,
        )?),
        account_id: row.get::<_, Option<i64>>(2)?.map(AccountId::new),
        direction: parse_transfer_direction(&direction)?,
        priority: i16::try_from(priority)
            .map_err(|_| corrupt("transfer_tasks", "priority", priority))?,
        state: parse_transfer_state(&state)?,
        total_bytes: nonnegative_from_sql("transfer_tasks", "total_bytes", total_bytes)?,
        transferred_bytes: nonnegative_from_sql(
            "transfer_tasks",
            "transferred_bytes",
            transferred_bytes,
        )?,
        source_path: row.get(8)?,
        destination_path: row.get(9)?,
        retry_count: u32::try_from(retry_count)
            .map_err(|_| corrupt("transfer_tasks", "retry_count", retry_count))?,
        next_retry_at_unix_ms: row.get(11)?,
        last_error_code: row.get(12)?,
        created_at_unix_ms: row.get(13)?,
        updated_at_unix_ms: row.get(14)?,
    })
}

fn row_to_transfer_part(row: &rusqlite::Row<'_>) -> StorageResult<TransferPartCheckpoint> {
    let transfer_id: i64 = row.get(0)?;
    let part_index: i64 = row.get(1)?;
    let offset_bytes: i64 = row.get(2)?;
    let size_bytes: i64 = row.get(3)?;
    let transferred_bytes: i64 = row.get(4)?;
    let state: String = row.get(5)?;
    let attempts: i64 = row.get(6)?;
    let remote_object_id: Option<i64> = row.get(7)?;
    let checkpoint_version: Option<i64> = row.get(8)?;
    Ok(TransferPartCheckpoint {
        transfer_id: TransferId::new(id_from_sql("transfer_parts", "transfer_id", transfer_id)?),
        index: PartIndex::new(
            u32::try_from(part_index)
                .map_err(|_| corrupt("transfer_parts", "part_index", part_index))?,
        ),
        offset_bytes: nonnegative_from_sql("transfer_parts", "offset_bytes", offset_bytes)?,
        size_bytes: nonnegative_from_sql("transfer_parts", "size_bytes", size_bytes)?,
        transferred_bytes: nonnegative_from_sql(
            "transfer_parts",
            "transferred_bytes",
            transferred_bytes,
        )?,
        state: parse_part_state(&state)?,
        attempts: u32::try_from(attempts)
            .map_err(|_| corrupt("transfer_parts", "attempts", attempts))?,
        remote_object_id: remote_object_id
            .map(|value| id_from_sql("transfer_parts", "remote_object_id", value))
            .transpose()?,
        checkpoint_version: checkpoint_version
            .map(|value| {
                u32::try_from(value)
                    .map_err(|_| corrupt("transfer_parts", "checkpoint_version", value))
            })
            .transpose()?,
        checkpoint_data: row.get(9)?,
        updated_at_unix_ms: row.get(10)?,
    })
}

fn transfer_direction_code(value: StoredTransferDirection) -> &'static str {
    match value {
        StoredTransferDirection::Upload => "upload",
        StoredTransferDirection::Download => "download",
    }
}

fn parse_transfer_direction(value: &str) -> StorageResult<StoredTransferDirection> {
    match value {
        "upload" => Ok(StoredTransferDirection::Upload),
        "download" => Ok(StoredTransferDirection::Download),
        value => Err(corrupt("transfer_tasks", "direction", value)),
    }
}

fn transfer_state_code(value: StoredTransferState) -> &'static str {
    match value {
        StoredTransferState::Queued => "queued",
        StoredTransferState::Running => "running",
        StoredTransferState::Paused => "paused",
        StoredTransferState::WaitingRetry => "waiting_retry",
        StoredTransferState::Verifying => "verifying",
        StoredTransferState::Completed => "completed",
        StoredTransferState::Failed => "failed",
        StoredTransferState::Cancelled => "cancelled",
    }
}

fn parse_transfer_state(value: &str) -> StorageResult<StoredTransferState> {
    match value {
        "queued" => Ok(StoredTransferState::Queued),
        "running" => Ok(StoredTransferState::Running),
        "paused" => Ok(StoredTransferState::Paused),
        "waiting_retry" => Ok(StoredTransferState::WaitingRetry),
        "verifying" => Ok(StoredTransferState::Verifying),
        "completed" => Ok(StoredTransferState::Completed),
        "failed" => Ok(StoredTransferState::Failed),
        "cancelled" => Ok(StoredTransferState::Cancelled),
        value => Err(corrupt("transfer_tasks", "state", value)),
    }
}

fn part_state_code(value: StoredPartState) -> &'static str {
    match value {
        StoredPartState::Queued => "queued",
        StoredPartState::Running => "running",
        StoredPartState::Paused => "paused",
        StoredPartState::WaitingRetry => "waiting_retry",
        StoredPartState::Transferred => "transferred",
        StoredPartState::Verifying => "verifying",
        StoredPartState::Verified => "verified",
        StoredPartState::Failed => "failed",
        StoredPartState::Cancelled => "cancelled",
    }
}

fn parse_part_state(value: &str) -> StorageResult<StoredPartState> {
    match value {
        "queued" => Ok(StoredPartState::Queued),
        "running" => Ok(StoredPartState::Running),
        "paused" => Ok(StoredPartState::Paused),
        "waiting_retry" => Ok(StoredPartState::WaitingRetry),
        "transferred" => Ok(StoredPartState::Transferred),
        "verifying" => Ok(StoredPartState::Verifying),
        "verified" => Ok(StoredPartState::Verified),
        "failed" => Ok(StoredPartState::Failed),
        "cancelled" => Ok(StoredPartState::Cancelled),
        value => Err(corrupt("transfer_parts", "state", value)),
    }
}
