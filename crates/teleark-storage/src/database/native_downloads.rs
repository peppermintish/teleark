use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension as _, Row, TransactionBehavior, params};

use super::{Database, corrupt, nonnegative_from_sql, unsigned_to_sql};
use crate::error::{EntityKind, InputReason};
use crate::model::{
    NativeDownloadBatchRecord, NativeDownloadTaskRecord, NewNativeDownloadBatchRecord,
    NewNativeDownloadTaskRecord, StoredNativeDownloadState, StoredNativeDownloadVerification,
};
use crate::{StorageError, StorageResult};

const MAX_NATIVE_DOWNLOAD_HISTORY: usize = 10_000;

const COLUMNS: &str = r#"
    id, chat_id, message_id, file_name, size_bytes, destination_path,
    state, verification, transferred_bytes, started_at_unix_ms,
    finished_at_unix_ms, queue_wait_ms, duration_ms,
    average_bytes_per_second, attempts, failure_code,
    created_at_unix_ms, updated_at_unix_ms, batch_id,
    message_sent_at_unix_ms, caption, mime_type
"#;

impl Database {
    pub fn insert_native_download(
        &mut self,
        task: &NewNativeDownloadTaskRecord,
    ) -> StorageResult<NativeDownloadTaskRecord> {
        validate_new(task)?;
        let id = insert_task(&self.connection, task, None)?;
        self.native_download_by_sql_id(id)?
            .ok_or(StorageError::NotFound {
                entity: EntityKind::NativeDownload,
                id,
            })
    }

    pub fn insert_native_download_batch(
        &mut self,
        batch: &NewNativeDownloadBatchRecord,
        tasks: &[NewNativeDownloadTaskRecord],
    ) -> StorageResult<(NativeDownloadBatchRecord, Vec<NativeDownloadTaskRecord>)> {
        if batch.chat_id <= 0 || tasks.is_empty() || tasks.len() > MAX_NATIVE_DOWNLOAD_HISTORY {
            return Err(StorageError::InvalidInput {
                field: "native_download_batch",
                reason: InputReason::OutOfRange,
            });
        }
        for task in tasks {
            validate_new(task)?;
            if task.chat_id != batch.chat_id {
                return Err(StorageError::InvalidInput {
                    field: "native_download_batch.chat_id",
                    reason: InputReason::InvalidCombination,
                });
            }
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO native_download_batches (chat_id, created_at_unix_ms) VALUES (?1, ?2)",
            params![batch.chat_id, batch.created_at_unix_ms],
        )?;
        let batch_sql_id = transaction.last_insert_rowid();
        let batch_id = nonnegative_from_sql("native_download_batches", "id", batch_sql_id)?;
        let mut inserted = Vec::with_capacity(tasks.len());
        for task in tasks {
            let id = insert_task(&transaction, task, Some(batch_sql_id))?;
            inserted.push(new_task_record(
                nonnegative_from_sql("native_download_tasks", "id", id)?,
                Some(batch_id),
                task,
            ));
        }
        transaction.commit()?;
        Ok((
            NativeDownloadBatchRecord {
                id: batch_id,
                chat_id: batch.chat_id,
                created_at_unix_ms: batch.created_at_unix_ms,
            },
            inserted,
        ))
    }

    pub fn save_native_download(&mut self, task: &NativeDownloadTaskRecord) -> StorageResult<()> {
        validate_task(task)?;
        let id = unsigned_to_sql("native_download.id", task.id)?;
        let changed = self.connection.execute(
            r#"
UPDATE native_download_tasks SET
    state = ?2,
    verification = ?3,
    transferred_bytes = ?4,
    started_at_unix_ms = ?5,
    finished_at_unix_ms = ?6,
    queue_wait_ms = ?7,
    duration_ms = ?8,
    average_bytes_per_second = ?9,
    attempts = ?10,
    failure_code = ?11,
    updated_at_unix_ms = ?12
WHERE id = ?1
"#,
            params![
                id,
                state_code(task.state),
                verification_code(task.verification),
                unsigned_to_sql("native_download.transferred_bytes", task.transferred_bytes)?,
                task.started_at_unix_ms,
                task.finished_at_unix_ms,
                optional_u64("native_download.queue_wait_ms", task.queue_wait_ms)?,
                optional_u64("native_download.duration_ms", task.duration_ms)?,
                optional_u64(
                    "native_download.average_bytes_per_second",
                    task.average_bytes_per_second,
                )?,
                i64::from(task.attempts),
                task.failure_code,
                task.updated_at_unix_ms,
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound {
                entity: EntityKind::NativeDownload,
                id,
            });
        }
        Ok(())
    }

    pub fn native_downloads(&self) -> StorageResult<Vec<NativeDownloadTaskRecord>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM native_download_tasks \
             ORDER BY created_at_unix_ms DESC, id DESC LIMIT ?1"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query([MAX_NATIVE_DOWNLOAD_HISTORY as i64])?;
        let mut tasks = Vec::new();
        while let Some(row) = rows.next()? {
            tasks.push(row_to_task(row)?);
        }
        tasks.reverse();
        Ok(tasks)
    }

    pub fn delete_native_download(&mut self, task_id: u64) -> StorageResult<()> {
        let sql_task_id = unsigned_to_sql("native_download.id", task_id)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let batch_id = transaction
            .query_row(
                "SELECT batch_id FROM native_download_tasks WHERE id = ?1",
                [sql_task_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        let Some(batch_id) = batch_id else {
            return Err(StorageError::NotFound {
                entity: EntityKind::NativeDownload,
                id: sql_task_id,
            });
        };
        transaction.execute(
            "DELETE FROM native_download_tasks WHERE id = ?1",
            [sql_task_id],
        )?;
        if let Some(batch_id) = batch_id {
            transaction.execute(
                "DELETE FROM native_download_batches WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM native_download_tasks WHERE batch_id = ?1)",
                [batch_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn native_download_by_sql_id(
        &self,
        id: i64,
    ) -> StorageResult<Option<NativeDownloadTaskRecord>> {
        let sql = format!("SELECT {COLUMNS} FROM native_download_tasks WHERE id = ?1");
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query([id])?;
        rows.next()?.map(row_to_task).transpose()
    }
}

fn insert_task(
    connection: &Connection,
    task: &NewNativeDownloadTaskRecord,
    batch_id: Option<i64>,
) -> StorageResult<i64> {
    connection.execute(
        r#"
INSERT INTO native_download_tasks (
    chat_id, message_id, message_sent_at_unix_ms, file_name, caption,
    mime_type, size_bytes, destination_path, batch_id, state, verification,
    transferred_bytes, attempts, created_at_unix_ms, updated_at_unix_ms
) VALUES (
    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
    'queued', 'pending', 0, 0, ?10, ?10
)
"#,
        params![
            task.chat_id,
            task.message_id,
            task.message_sent_at_unix_ms,
            task.file_name,
            task.caption,
            task.mime_type,
            unsigned_to_sql("native_download.size_bytes", task.size_bytes)?,
            path_text(&task.destination)?,
            batch_id,
            task.created_at_unix_ms,
        ],
    )?;
    Ok(connection.last_insert_rowid())
}

fn new_task_record(
    id: u64,
    batch_id: Option<u64>,
    task: &NewNativeDownloadTaskRecord,
) -> NativeDownloadTaskRecord {
    NativeDownloadTaskRecord {
        id,
        batch_id,
        chat_id: task.chat_id,
        message_id: task.message_id,
        message_sent_at_unix_ms: task.message_sent_at_unix_ms,
        file_name: task.file_name.clone(),
        caption: task.caption.clone(),
        mime_type: task.mime_type.clone(),
        size_bytes: task.size_bytes,
        destination: task.destination.clone(),
        state: StoredNativeDownloadState::Queued,
        verification: StoredNativeDownloadVerification::Pending,
        transferred_bytes: 0,
        created_at_unix_ms: task.created_at_unix_ms,
        started_at_unix_ms: None,
        finished_at_unix_ms: None,
        queue_wait_ms: None,
        duration_ms: None,
        average_bytes_per_second: None,
        attempts: 0,
        failure_code: None,
        updated_at_unix_ms: task.created_at_unix_ms,
    }
}

fn validate_new(task: &NewNativeDownloadTaskRecord) -> StorageResult<()> {
    if task.chat_id <= 0 || task.message_id <= 0 {
        return Err(StorageError::InvalidInput {
            field: "native_download.remote_identity",
            reason: InputReason::OutOfRange,
        });
    }
    if task.file_name.trim().is_empty() || task.file_name.len() > 4_096 {
        return Err(StorageError::InvalidInput {
            field: "native_download.file_name",
            reason: InputReason::TooLong,
        });
    }
    if task
        .caption
        .as_ref()
        .is_some_and(|caption| caption.len() > 1_048_576)
        || task.mime_type.as_ref().is_some_and(|mime| mime.len() > 512)
    {
        return Err(StorageError::InvalidInput {
            field: "native_download.message_metadata",
            reason: InputReason::TooLong,
        });
    }
    let destination = path_text(&task.destination)?;
    if destination.len() > 32_768 {
        return Err(StorageError::InvalidInput {
            field: "native_download.destination",
            reason: InputReason::TooLong,
        });
    }
    unsigned_to_sql("native_download.size_bytes", task.size_bytes)?;
    Ok(())
}

fn validate_task(task: &NativeDownloadTaskRecord) -> StorageResult<()> {
    unsigned_to_sql("native_download.id", task.id)?;
    unsigned_to_sql("native_download.size_bytes", task.size_bytes)?;
    unsigned_to_sql("native_download.transferred_bytes", task.transferred_bytes)?;
    if task.transferred_bytes > task.size_bytes || task.attempts > i32::MAX as u32 {
        return Err(StorageError::InvalidInput {
            field: "native_download.progress",
            reason: InputReason::OutOfRange,
        });
    }
    if task.failure_code.as_ref().is_some_and(|code| {
        code.is_empty()
            || code.len() > 64
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    }) {
        return Err(StorageError::InvalidInput {
            field: "native_download.failure_code",
            reason: InputReason::InvalidCombination,
        });
    }
    Ok(())
}

fn row_to_task(row: &Row<'_>) -> StorageResult<NativeDownloadTaskRecord> {
    let id: i64 = row.get(0)?;
    let size_bytes: i64 = row.get(4)?;
    let transferred_bytes: i64 = row.get(8)?;
    let queue_wait_ms: Option<i64> = row.get(11)?;
    let duration_ms: Option<i64> = row.get(12)?;
    let average_bytes_per_second: Option<i64> = row.get(13)?;
    let attempts: i64 = row.get(14)?;
    Ok(NativeDownloadTaskRecord {
        id: nonnegative_from_sql("native_download_tasks", "id", id)?,
        batch_id: row
            .get::<_, Option<i64>>(18)?
            .map(|value| nonnegative_from_sql("native_download_tasks", "batch_id", value))
            .transpose()?,
        chat_id: row.get(1)?,
        message_id: row.get(2)?,
        message_sent_at_unix_ms: row.get(19)?,
        file_name: row.get(3)?,
        caption: row.get(20)?,
        mime_type: row.get(21)?,
        size_bytes: nonnegative_from_sql("native_download_tasks", "size_bytes", size_bytes)?,
        destination: PathBuf::from(row.get::<_, String>(5)?),
        state: parse_state(&row.get::<_, String>(6)?)?,
        verification: parse_verification(&row.get::<_, String>(7)?)?,
        transferred_bytes: nonnegative_from_sql(
            "native_download_tasks",
            "transferred_bytes",
            transferred_bytes,
        )?,
        started_at_unix_ms: row.get(9)?,
        finished_at_unix_ms: row.get(10)?,
        queue_wait_ms: optional_nonnegative("queue_wait_ms", queue_wait_ms)?,
        duration_ms: optional_nonnegative("duration_ms", duration_ms)?,
        average_bytes_per_second: optional_nonnegative(
            "average_bytes_per_second",
            average_bytes_per_second,
        )?,
        attempts: u32::try_from(attempts)
            .map_err(|_| corrupt("native_download_tasks", "attempts", attempts))?,
        failure_code: row.get(15)?,
        created_at_unix_ms: row.get(16)?,
        updated_at_unix_ms: row.get(17)?,
    })
}

fn path_text(path: &std::path::Path) -> StorageResult<&str> {
    path.to_str()
        .filter(|path| !path.is_empty())
        .ok_or(StorageError::InvalidInput {
            field: "native_download.destination",
            reason: InputReason::InvalidCombination,
        })
}

fn optional_u64(field: &'static str, value: Option<u64>) -> StorageResult<Option<i64>> {
    value.map(|value| unsigned_to_sql(field, value)).transpose()
}

fn optional_nonnegative(field: &'static str, value: Option<i64>) -> StorageResult<Option<u64>> {
    value
        .map(|value| nonnegative_from_sql("native_download_tasks", field, value))
        .transpose()
}

fn state_code(state: StoredNativeDownloadState) -> &'static str {
    match state {
        StoredNativeDownloadState::Queued => "queued",
        StoredNativeDownloadState::Running => "running",
        StoredNativeDownloadState::Paused => "paused",
        StoredNativeDownloadState::Completed => "completed",
        StoredNativeDownloadState::Failed => "failed",
        StoredNativeDownloadState::Cancelled => "cancelled",
    }
}

fn parse_state(value: &str) -> StorageResult<StoredNativeDownloadState> {
    match value {
        "queued" => Ok(StoredNativeDownloadState::Queued),
        "running" => Ok(StoredNativeDownloadState::Running),
        "paused" => Ok(StoredNativeDownloadState::Paused),
        "completed" => Ok(StoredNativeDownloadState::Completed),
        "failed" => Ok(StoredNativeDownloadState::Failed),
        "cancelled" => Ok(StoredNativeDownloadState::Cancelled),
        value => Err(corrupt("native_download_tasks", "state", value)),
    }
}

fn verification_code(verification: StoredNativeDownloadVerification) -> &'static str {
    match verification {
        StoredNativeDownloadVerification::Pending => "pending",
        StoredNativeDownloadVerification::SizeChecked => "size_checked",
        StoredNativeDownloadVerification::NotReached => "not_reached",
    }
}

fn parse_verification(value: &str) -> StorageResult<StoredNativeDownloadVerification> {
    match value {
        "pending" => Ok(StoredNativeDownloadVerification::Pending),
        "size_checked" => Ok(StoredNativeDownloadVerification::SizeChecked),
        "not_reached" => Ok(StoredNativeDownloadVerification::NotReached),
        value => Err(corrupt("native_download_tasks", "verification", value)),
    }
}
