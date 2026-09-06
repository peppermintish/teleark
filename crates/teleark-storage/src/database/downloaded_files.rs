//! Durable output identities, with paged observations performed by Runtime.
use super::{
    Database, decode_local_path, encode_local_path, nonnegative_from_sql, unsigned_to_sql,
};
use crate::{
    DownloadedFileRecord, DownloadedFilesCursor, InputReason, StorageError, StorageResult,
    VaultDownloadRecord,
};
use rusqlite::params;

impl Database {
    pub fn record_vault_download(&mut self, record: &VaultDownloadRecord) -> StorageResult<()> {
        if record.account_id <= 0
            || record.chat_id <= 0
            || record.package_id.len() != 32
            || !record
                .package_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !record.destination.is_absolute()
        {
            return Err(StorageError::InvalidInput {
                field: "vault_download",
                reason: InputReason::OutOfRange,
            });
        }
        let (encoding, path) = encode_local_path(&record.destination)?;
        if path.is_empty() || path.len() > 32768 {
            return Err(StorageError::InvalidInput {
                field: "vault_download.destination",
                reason: InputReason::OutOfRange,
            });
        }
        self.connection.execute("INSERT INTO vault_downloaded_files (account_id, chat_id, package_id, path_encoding, destination_path, size_bytes, completed_at_unix_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (account_id, path_encoding, destination_path) DO UPDATE SET chat_id=excluded.chat_id, package_id=excluded.package_id, size_bytes=excluded.size_bytes, completed_at_unix_ms=excluded.completed_at_unix_ms", params![record.account_id, record.chat_id, record.package_id, encoding, path, unsigned_to_sql("vault_download.size_bytes", record.size_bytes)?, record.completed_at_unix_ms])?;
        Ok(())
    }

    /// At most 128 outputs; native history is projected directly so pre-v10
    /// downloads require no speculative backfill or account reassignment.
    pub fn downloaded_files_page(
        &self,
        account_id: i64,
        after: Option<DownloadedFilesCursor>,
    ) -> StorageResult<Vec<DownloadedFileRecord>> {
        if account_id <= 0 || after.is_some_and(|cursor| cursor.kind > 1) {
            return Err(StorageError::InvalidInput {
                field: "downloaded_files.cursor",
                reason: InputReason::OutOfRange,
            });
        }
        let bound = |kind: u8| -> Result<i64, StorageError> {
            match after {
                Some(cursor) if cursor.kind > kind => Ok(0),
                Some(cursor) if cursor.kind == kind => {
                    unsigned_to_sql("downloaded_files.id", cursor.id.saturating_sub(1))
                }
                _ => Ok(i64::MAX),
            }
        };
        let native_bound = bound(0)?;
        let vault_bound = bound(1)?;
        let mut statement = self.connection.prepare(
            r#"
SELECT * FROM (
 SELECT * FROM (
  SELECT 0 AS kind, id, account_id, chat_id, message_id, NULL AS package_id,
         'utf8' AS path_encoding, CAST(destination_path AS BLOB) AS destination_path,
         size_bytes, COALESCE(finished_at_unix_ms, updated_at_unix_ms) AS completed_at
  FROM native_download_tasks WHERE account_id=?1 AND state='completed' AND id<=?2
  ORDER BY id DESC LIMIT 128
 )
 UNION ALL
 SELECT * FROM (
  SELECT 1 AS kind, id, account_id, chat_id, NULL AS message_id, package_id,
         path_encoding, destination_path, size_bytes, completed_at_unix_ms
  FROM vault_downloaded_files WHERE account_id=?1 AND id<=?3
  ORDER BY id DESC LIMIT 128
 )
) ORDER BY kind, id DESC LIMIT 128
"#,
        )?;
        let mut rows = statement.query(params![account_id, native_bound, vault_bound])?;
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            let encoding: String = row.get(6)?;
            let bytes: Vec<u8> = row.get(7)?;
            // Native task paths have an explicitly UTF-8 durable representation.
            let destination = if encoding == "utf8" {
                std::path::PathBuf::from(String::from_utf8(bytes).map_err(|_| {
                    StorageError::CorruptData {
                        entity: "downloaded_files",
                        field: "destination",
                        value: "invalid utf8".into(),
                    }
                })?)
            } else {
                decode_local_path(&encoding, bytes)?
            };
            results.push(DownloadedFileRecord {
                cursor: DownloadedFilesCursor {
                    kind: row.get(0)?,
                    id: nonnegative_from_sql("downloaded_files", "id", row.get(1)?)?,
                },
                account_id: row.get(2)?,
                chat_id: row.get(3)?,
                message_id: row.get(4)?,
                package_id: row.get(5)?,
                destination,
                size_bytes: nonnegative_from_sql("downloaded_files", "size_bytes", row.get(8)?)?,
                completed_at_unix_ms: row.get(9)?,
            });
        }
        Ok(results)
    }
}
