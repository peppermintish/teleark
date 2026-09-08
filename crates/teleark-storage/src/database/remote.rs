use rusqlite::{OptionalExtension, params};
use teleark_core::{
    EncryptionState, LogicalFileId, RemoteObjectId, RemoteState, VerificationState,
};

use super::{Database, id_from_sql, transaction, unsigned_to_sql, upsert_logical_file_on};
use crate::error::{InputReason, InvariantViolation};
use crate::model::{
    CachedTelegramFileRecord, LogicalFileRecord, RemoteFileUpsert, RemoteObjectRecord,
};
use crate::{StorageError, StorageResult};

const MAX_REMOTE_KEY_BYTES: usize = 16 * 1024;
const MAX_CACHED_TELEGRAM_FILES: usize = 5_000;

impl Database {
    /// Atomically upserts a Telegram identity and its file-centric projection.
    /// Older revisions are ignored; equal conflicting revisions are rejected.
    pub fn upsert_remote_file(
        &mut self,
        incoming: &RemoteFileUpsert,
    ) -> StorageResult<(LogicalFileRecord, RemoteObjectRecord)> {
        let transaction = transaction(&mut self.connection)?;
        let result = upsert_remote_file_on(&transaction, incoming, false)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn remote_object_by_source(
        &self,
        account_id: teleark_core::AccountId,
        chat_id: teleark_core::ChatId,
        message_id: teleark_core::MessageId,
    ) -> StorageResult<Option<RemoteObjectRecord>> {
        self.connection
            .query_row(
                r#"
SELECT id, logical_file_id, revision, remote_key, encoded_size_bytes, modified_at_unix_ms
FROM remote_objects
WHERE account_id = ?1 AND chat_id = ?2 AND message_id = ?3
"#,
                params![account_id.get(), chat_id.get(), message_id.get()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()?
            .map(|(id, logical_id, revision, key, size, modified)| {
                Ok(RemoteObjectRecord {
                    id: RemoteObjectId::new(id_from_sql("remote_objects", "id", id)?),
                    logical_file_id: LogicalFileId::new(id_from_sql(
                        "remote_objects",
                        "logical_file_id",
                        logical_id,
                    )?),
                    account_id,
                    chat_id,
                    message_id,
                    revision: stored_u64("revision", revision)?,
                    remote_key: key,
                    encoded_size_bytes: stored_u64("encoded_size_bytes", size)?,
                    modified_at_unix_ms: modified,
                })
            })
            .transpose()
    }

    /// Returns the newest cached files for one Telegram source without making
    /// any claim that the source history is fully indexed.
    pub fn cached_telegram_files(
        &self,
        account_id: teleark_core::AccountId,
        chat_id: teleark_core::ChatId,
        limit: usize,
    ) -> StorageResult<Vec<CachedTelegramFileRecord>> {
        if limit == 0 || limit > MAX_CACHED_TELEGRAM_FILES {
            return Err(StorageError::InvalidInput {
                field: "cached_telegram_files.limit",
                reason: InputReason::OutOfRange,
            });
        }
        let mut statement = self.connection.prepare(
            r#"
SELECT ro.message_id, f.name, f.caption, f.mime_type, f.size_bytes,
       COALESCE(f.created_at_unix_ms, f.modified_at_unix_ms, 0),
       COALESCE(f.modified_at_unix_ms, f.created_at_unix_ms, 0)
FROM remote_objects ro
JOIN logical_files f ON f.id = ro.logical_file_id
WHERE ro.account_id = ?1 AND ro.chat_id = ?2
  AND NOT EXISTS (SELECT 1 FROM channel_sync_tombstones t
      WHERE t.account_id = ro.account_id AND t.chat_id = ro.chat_id
        AND t.message_id = ro.message_id)
ORDER BY COALESCE(f.created_at_unix_ms, f.modified_at_unix_ms, 0) DESC,
         ro.message_id DESC
LIMIT ?3
"#,
        )?;
        let limit = i64::try_from(limit).map_err(|_| StorageError::InvalidInput {
            field: "cached_telegram_files.limit",
            reason: InputReason::OutOfRange,
        })?;
        let rows = statement.query_map(params![account_id.get(), chat_id.get(), limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (message_id, file_name, caption, mime_type, size, sent_at, modified_at) = row?;
            Ok(CachedTelegramFileRecord {
                message_id: teleark_core::MessageId::new(message_id),
                file_name,
                caption,
                mime_type,
                size_bytes: stored_u64("encoded_size_bytes", size)?,
                sent_at_unix_ms: sent_at,
                modified_at_unix_ms: modified_at,
            })
        })
        .collect()
    }
}

fn validate_remote_file(file: &RemoteFileUpsert) -> StorageResult<()> {
    if file.name.trim().is_empty() {
        return Err(StorageError::InvalidInput {
            field: "remote_file.name",
            reason: InputReason::Empty,
        });
    }
    if file.remote_key.is_empty() {
        return Err(StorageError::InvalidInput {
            field: "remote_file.remote_key",
            reason: InputReason::Empty,
        });
    }
    if file.remote_key.len() > MAX_REMOTE_KEY_BYTES {
        return Err(StorageError::InvalidInput {
            field: "remote_file.remote_key",
            reason: InputReason::TooLong,
        });
    }
    unsigned_to_sql("remote_file.revision", file.revision)?;
    unsigned_to_sql("remote_file.size_bytes", file.size_bytes)?;
    Ok(())
}

fn logical_record(id: LogicalFileId, incoming: &RemoteFileUpsert) -> LogicalFileRecord {
    let extension = incoming
        .name
        .rsplit_once('.')
        .and_then(|(_, value)| (!value.is_empty()).then(|| value.to_owned()));
    LogicalFileRecord {
        id,
        name: incoming.name.clone(),
        relative_path: None,
        size_bytes: incoming.size_bytes,
        kind: incoming.kind,
        mime_type: incoming.mime_type.clone(),
        extension,
        caption: incoming.caption.clone(),
        source_account_id: Some(incoming.account_id),
        source_chat_id: Some(incoming.chat_id),
        created_at_unix_ms: Some(incoming.sent_at_unix_ms),
        modified_at_unix_ms: Some(incoming.modified_at_unix_ms),
        remote_state: RemoteState::Uploaded,
        encryption_state: EncryptionState::Unencrypted,
        verification_state: VerificationState::Unverified,
        package_id: None,
        locally_available: false,
        local_source_path: None,
    }
}

fn allocate_id(connection: &rusqlite::Connection, entity: &'static str) -> StorageResult<u64> {
    let current: i64 = connection.query_row(
        "SELECT next_id FROM id_allocators WHERE entity = ?1",
        [entity],
        |row| row.get(0),
    )?;
    let next = current.checked_add(1).ok_or(StorageError::InvalidInput {
        field: "id_allocator.next_id",
        reason: InputReason::OutOfRange,
    })?;
    connection.execute(
        "UPDATE id_allocators SET next_id = ?1 WHERE entity = ?2",
        params![next, entity],
    )?;
    id_from_sql("id_allocators", "next_id", current)
}

fn query_file_on(
    connection: &rusqlite::Connection,
    id: LogicalFileId,
) -> StorageResult<LogicalFileRecord> {
    let columns = super::LOGICAL_FILE_COLUMNS;
    let sql = format!("SELECT {columns} FROM logical_files WHERE id = ?1");
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query([sql_id("logical_file.id", id.get())?])?;
    let row = rows.next()?.ok_or(StorageError::CorruptData {
        entity: "remote_objects",
        field: "logical_file_id",
        value: id.to_string(),
    })?;
    super::row_to_logical_file(row)
}

fn sql_id(field: &'static str, value: u64) -> StorageResult<i64> {
    i64::try_from(value).map_err(|_| StorageError::InvalidInput {
        field,
        reason: InputReason::OutOfRange,
    })
}

fn stored_u64(field: &'static str, value: i64) -> StorageResult<u64> {
    u64::try_from(value).map_err(|_| StorageError::CorruptData {
        entity: "remote_objects",
        field,
        value: value.to_string(),
    })
}

pub(super) fn upsert_remote_file_on(
    connection: &rusqlite::Connection,
    incoming: &RemoteFileUpsert,
    authoritative: bool,
) -> StorageResult<(LogicalFileRecord, RemoteObjectRecord)> {
    validate_remote_file(incoming)?;
    let existing = connection
        .query_row(
            r#"
SELECT id, logical_file_id, revision, remote_key, encoded_size_bytes, modified_at_unix_ms
FROM remote_objects
WHERE account_id = ?1 AND chat_id = ?2 AND message_id = ?3
"#,
            params![
                incoming.account_id.get(),
                incoming.chat_id.get(),
                incoming.message_id.get()
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;

    if let Some((remote_id, logical_id, revision, key, size, modified)) = existing {
        let revision = stored_u64("revision", revision)?;
        let logical_file_id = LogicalFileId::new(id_from_sql(
            "remote_objects",
            "logical_file_id",
            logical_id,
        )?);
        if !authoritative && revision > incoming.revision {
            let file = query_file_on(connection, logical_file_id)?;
            let remote = RemoteObjectRecord {
                id: RemoteObjectId::new(id_from_sql("remote_objects", "id", remote_id)?),
                logical_file_id,
                account_id: incoming.account_id,
                chat_id: incoming.chat_id,
                message_id: incoming.message_id,
                revision,
                remote_key: key,
                encoded_size_bytes: stored_u64("encoded_size_bytes", size)?,
                modified_at_unix_ms: modified,
            };
            return Ok((file, remote));
        }
        if !authoritative
            && revision == incoming.revision
            && (key != incoming.remote_key
                || u64::try_from(size).ok() != Some(incoming.size_bytes)
                || modified != incoming.modified_at_unix_ms)
        {
            return Err(StorageError::Invariant(
                InvariantViolation::RemoteRevisionConflict,
            ));
        }

        let previous = query_file_on(connection, logical_file_id)?;
        let mut file = logical_record(logical_file_id, incoming);
        file.locally_available = previous.locally_available;
        file.local_source_path = previous.local_source_path;
        file.relative_path = previous.relative_path;
        file.package_id = previous.package_id;
        file.encryption_state = previous.encryption_state;
        // A new remote revision does not delete local paths, but must not
        // inherit verification of the previous remote contents.
        upsert_logical_file_on(connection, &file)?;
        connection.execute(
            r#"
UPDATE remote_objects
SET revision = ?1, remote_key = ?2, encoded_size_bytes = ?3, modified_at_unix_ms = ?4
WHERE id = ?5
"#,
            params![
                unsigned_to_sql("remote_object.revision", incoming.revision)?,
                incoming.remote_key,
                unsigned_to_sql("remote_object.encoded_size_bytes", incoming.size_bytes)?,
                incoming.modified_at_unix_ms,
                remote_id
            ],
        )?;
        let remote = RemoteObjectRecord {
            id: RemoteObjectId::new(id_from_sql("remote_objects", "id", remote_id)?),
            logical_file_id,
            account_id: incoming.account_id,
            chat_id: incoming.chat_id,
            message_id: incoming.message_id,
            revision: incoming.revision,
            remote_key: incoming.remote_key.clone(),
            encoded_size_bytes: incoming.size_bytes,
            modified_at_unix_ms: incoming.modified_at_unix_ms,
        };
        return Ok((file, remote));
    }

    let logical_id = allocate_id(connection, "logical_file")?;
    let remote_id = allocate_id(connection, "remote_object")?;
    let logical_file_id = LogicalFileId::new(logical_id);
    let file = logical_record(logical_file_id, incoming);
    upsert_logical_file_on(connection, &file)?;
    connection.execute(
        r#"
INSERT INTO remote_objects (
    id, logical_file_id, account_id, chat_id, message_id, revision,
    remote_key, encoded_size_bytes, modified_at_unix_ms
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
"#,
        params![
            sql_id("remote_object.id", remote_id)?,
            sql_id("logical_file.id", logical_id)?,
            incoming.account_id.get(),
            incoming.chat_id.get(),
            incoming.message_id.get(),
            unsigned_to_sql("remote_object.revision", incoming.revision)?,
            incoming.remote_key,
            unsigned_to_sql("remote_object.encoded_size_bytes", incoming.size_bytes)?,
            incoming.modified_at_unix_ms,
        ],
    )?;
    let remote = RemoteObjectRecord {
        id: RemoteObjectId::new(remote_id),
        logical_file_id,
        account_id: incoming.account_id,
        chat_id: incoming.chat_id,
        message_id: incoming.message_id,
        revision: incoming.revision,
        remote_key: incoming.remote_key.clone(),
        encoded_size_bytes: incoming.size_bytes,
        modified_at_unix_ms: incoming.modified_at_unix_ms,
    };
    Ok((file, remote))
}
