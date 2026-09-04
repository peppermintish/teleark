mod collections;
mod index;
mod native_downloads;
mod remote;
mod search;
mod telegram_index;
mod transfers;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Row, Transaction, params};
use teleark_core::{
    AccountId, ChatId, EncryptionState, FileKind, LogicalFileId, PackageId, RemoteState,
    VerificationState,
};

use crate::error::InputReason;
use crate::migration;
use crate::model::VaultMetadataRecord;
use crate::model::{
    AccountRecord, ChatRecord, LibraryStatisticsRecord, LogicalFileRecord, NewLogicalFileRecord,
    encryption_state_code, file_kind_code, remote_state_code, verification_state_code,
};
use crate::{StorageError, StorageResult};

pub const LATEST_SCHEMA_VERSION: u32 = migration::LATEST_SCHEMA_VERSION;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const LOGICAL_FILE_COLUMNS: &str = r#"
    id, name, relative_path, size_bytes, kind, mime_type, extension, caption,
    source_account_id, source_chat_id, created_at_unix_ms, modified_at_unix_ms,
    remote_state, encryption_state, verification_state, package_id, locally_available,
    local_source_path_encoding, local_source_path
"#;

/// One configured, migrated SQLite database.
///
/// The connection is deliberately not wrapped in shared synchronization.
/// Move a `Database` to its owning storage worker and issue bounded commands to
/// that owner; do not perform database work on the GUI thread.
pub struct Database {
    connection: Connection,
}

impl Database {
    /// Opens or creates a database, applies ordered migrations, and enables
    /// foreign keys, WAL, NORMAL synchronous durability, and a five-second busy timeout.
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(path, flags)?;
        Self::initialize(connection)
    }

    /// Opens a fully migrated isolated database for deterministic tests or ephemeral use.
    pub fn open_in_memory() -> StorageResult<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut connection: Connection) -> StorageResult<Self> {
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "wal_autocheckpoint", 1_000_i64)?;
        connection.pragma_update(None, "trusted_schema", false)?;
        migration::migrate(&mut connection)?;

        let foreign_keys: i64 =
            connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
        if foreign_keys != 1 {
            return Err(StorageError::CorruptData {
                entity: "database",
                field: "foreign_keys",
                value: foreign_keys.to_string(),
            });
        }
        Ok(Self { connection })
    }

    pub fn schema_version(&self) -> StorageResult<u32> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        u32::try_from(version).map_err(|_| StorageError::CorruptData {
            entity: "database",
            field: "user_version",
            value: version.to_string(),
        })
    }

    pub fn journal_mode(&self) -> StorageResult<String> {
        Ok(self
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))?)
    }

    pub fn foreign_keys_enabled(&self) -> StorageResult<bool> {
        let enabled: i64 = self
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
        Ok(enabled == 1)
    }

    /// Runs SQLite's quick structural check and returns a structured corruption error.
    pub fn quick_check(&self) -> StorageResult<()> {
        let result: String = self
            .connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if result == "ok" {
            Ok(())
        } else {
            Err(StorageError::CorruptData {
                entity: "database",
                field: "quick_check",
                value: result,
            })
        }
    }

    /// Returns the single configured Vault record, if one exists.
    pub fn vault_metadata(&self) -> StorageResult<Option<VaultMetadataRecord>> {
        let mut statement = self.connection.prepare(
            r#"
SELECT vault_id, password_wrap, recovery_wrap, password_generation,
       recovery_generation, created_at_unix_ms, updated_at_unix_ms
FROM vault_metadata WHERE singleton_id = 1
"#,
        )?;
        let mut rows = statement.query([])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let vault_id: Vec<u8> = row.get(0)?;
        let vault_id = vault_id
            .try_into()
            .map_err(|value: Vec<u8>| StorageError::CorruptData {
                entity: "vault_metadata",
                field: "vault_id",
                value: value.len().to_string(),
            })?;
        let password_generation =
            u32::try_from(row.get::<_, i64>(3)?).map_err(|_| StorageError::CorruptData {
                entity: "vault_metadata",
                field: "password_generation",
                value: "out_of_range".to_owned(),
            })?;
        let recovery_generation =
            u32::try_from(row.get::<_, i64>(4)?).map_err(|_| StorageError::CorruptData {
                entity: "vault_metadata",
                field: "recovery_generation",
                value: "out_of_range".to_owned(),
            })?;
        Ok(Some(VaultMetadataRecord {
            vault_id,
            password_wrap: row.get(1)?,
            recovery_wrap: row.get(2)?,
            password_generation,
            recovery_generation,
            created_at_unix_ms: row.get(5)?,
            updated_at_unix_ms: row.get(6)?,
        }))
    }

    /// Atomically creates or replaces the single Vault metadata record.
    pub fn save_vault_metadata(&mut self, record: &VaultMetadataRecord) -> StorageResult<()> {
        if record.password_wrap.len() < 124
            || record.password_wrap.len() > 172
            || record.recovery_wrap.len() != 88
            || record.password_generation == 0
            || record.recovery_generation == 0
            || record.updated_at_unix_ms < record.created_at_unix_ms
        {
            return Err(StorageError::InvalidInput {
                field: "vault_metadata",
                reason: InputReason::InvalidCombination,
            });
        }
        let password_generation = i64::from(record.password_generation);
        let recovery_generation = i64::from(record.recovery_generation);
        let transaction = self.connection.transaction()?;
        transaction.execute(
            r#"
INSERT INTO vault_metadata (
    singleton_id, vault_id, password_wrap, recovery_wrap, password_generation,
    recovery_generation, created_at_unix_ms, updated_at_unix_ms
) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
ON CONFLICT(singleton_id) DO UPDATE SET
    vault_id = excluded.vault_id,
    password_wrap = excluded.password_wrap,
    recovery_wrap = excluded.recovery_wrap,
    password_generation = excluded.password_generation,
    recovery_generation = excluded.recovery_generation,
    created_at_unix_ms = excluded.created_at_unix_ms,
    updated_at_unix_ms = excluded.updated_at_unix_ms
"#,
            params![
                record.vault_id.as_slice(),
                record.password_wrap,
                record.recovery_wrap,
                password_generation,
                recovery_generation,
                record.created_at_unix_ms,
                record.updated_at_unix_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_account(&mut self, account: &AccountRecord) -> StorageResult<()> {
        validate_nonempty("account.display_name", &account.display_name)?;
        self.connection.execute(
            r#"
INSERT INTO accounts (id, display_name, created_at_unix_ms, updated_at_unix_ms)
VALUES (?1, ?2, ?3, ?4)
ON CONFLICT(id) DO UPDATE SET
    display_name = excluded.display_name,
    updated_at_unix_ms = excluded.updated_at_unix_ms
"#,
            params![
                account.id.get(),
                account.display_name,
                account.created_at_unix_ms,
                account.updated_at_unix_ms
            ],
        )?;
        Ok(())
    }

    pub fn upsert_chat(&mut self, chat: &ChatRecord) -> StorageResult<()> {
        validate_nonempty("chat.title", &chat.title)?;
        self.connection.execute(
            r#"
INSERT INTO chats (account_id, id, title, username, updated_at_unix_ms)
VALUES (?1, ?2, ?3, ?4, ?5)
ON CONFLICT(account_id, id) DO UPDATE SET
    title = excluded.title,
    username = excluded.username,
    updated_at_unix_ms = excluded.updated_at_unix_ms
"#,
            params![
                chat.account_id.get(),
                chat.id.get(),
                chat.title,
                chat.username,
                chat.updated_at_unix_ms
            ],
        )?;
        Ok(())
    }

    pub fn upsert_logical_file(&mut self, file: &LogicalFileRecord) -> StorageResult<()> {
        validate_file(file)?;
        upsert_logical_file_on(&self.connection, file)
    }

    /// Allocates a never-reused logical-file ID and inserts the record in one transaction.
    pub fn insert_logical_file(
        &mut self,
        file: &NewLogicalFileRecord,
    ) -> StorageResult<LogicalFileRecord> {
        let validation_record = file.with_id(LogicalFileId::new(1));
        validate_file(&validation_record)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let next_id: i64 = transaction.query_row(
            "SELECT next_id FROM id_allocators WHERE entity = 'logical_file'",
            [],
            |row| row.get(0),
        )?;
        let following_id = next_id.checked_add(1).ok_or(StorageError::InvalidInput {
            field: "logical_file.id",
            reason: InputReason::OutOfRange,
        })?;
        transaction.execute(
            "UPDATE id_allocators SET next_id = ?1 WHERE entity = 'logical_file'",
            [following_id],
        )?;
        let allocated_id = LogicalFileId::new(id_from_sql("id_allocators", "next_id", next_id)?);
        let record = file.with_id(allocated_id);
        upsert_logical_file_on(&transaction, &record)?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn upsert_logical_files(&mut self, files: &[LogicalFileRecord]) -> StorageResult<()> {
        for file in files {
            validate_file(file)?;
        }
        let transaction = self.connection.transaction()?;
        for file in files {
            upsert_logical_file_on(&transaction, file)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn logical_file(&self, id: LogicalFileId) -> StorageResult<Option<LogicalFileRecord>> {
        let id = id_to_sql("logical_file.id", id.get())?;
        let sql = format!("SELECT {LOGICAL_FILE_COLUMNS} FROM logical_files WHERE id = ?1");
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query([id])?;
        rows.next()?.map(row_to_logical_file).transpose()
    }

    /// Deletes only local metadata. Remote deletion is intentionally outside this adapter call.
    pub fn delete_logical_file(&mut self, id: LogicalFileId) -> StorageResult<bool> {
        let id = id_to_sql("logical_file.id", id.get())?;
        Ok(self
            .connection
            .execute("DELETE FROM logical_files WHERE id = ?1", [id])?
            != 0)
    }

    pub fn logical_file_count(&self) -> StorageResult<u64> {
        let count: i64 =
            self.connection
                .query_row("SELECT count(*) FROM logical_files", [], |row| row.get(0))?;
        nonnegative_from_sql("logical_files", "count", count)
    }

    pub fn library_statistics(&self) -> StorageResult<LibraryStatisticsRecord> {
        let (count, local, remote): (i64, i64, i64) = self.connection.query_row(
            r#"
SELECT
    count(*),
    COALESCE(sum(locally_available), 0),
    COALESCE(sum(CASE WHEN remote_state != 'local_only' THEN 1 ELSE 0 END), 0)
FROM logical_files
"#,
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let active: i64 = self.connection.query_row(
            "SELECT count(*) FROM transfer_tasks WHERE state NOT IN ('completed', 'cancelled')",
            [],
            |row| row.get(0),
        )?;
        let mut statement = self
            .connection
            .prepare("SELECT size_bytes FROM logical_files")?;
        let mut rows = statement.query([])?;
        let mut logical_bytes = 0_u64;
        while let Some(row) = rows.next()? {
            let size: i64 = row.get(0)?;
            logical_bytes = logical_bytes
                .checked_add(nonnegative_from_sql("logical_files", "size_bytes", size)?)
                .ok_or_else(|| corrupt("logical_files", "total_size_bytes", "overflow"))?;
        }
        Ok(LibraryStatisticsRecord {
            logical_file_count: nonnegative_from_sql("logical_files", "count", count)?,
            logical_bytes,
            local_file_count: nonnegative_from_sql("logical_files", "local_count", local)?,
            remote_file_count: nonnegative_from_sql("logical_files", "remote_count", remote)?,
            active_transfer_count: nonnegative_from_sql("transfer_tasks", "active_count", active)?,
        })
    }
}

fn validate_file(file: &LogicalFileRecord) -> StorageResult<()> {
    validate_nonempty("logical_file.name", &file.name)?;
    if file.source_account_id.is_some() != file.source_chat_id.is_some() {
        return Err(StorageError::Invariant(
            crate::InvariantViolation::IncompleteSourceIdentity,
        ));
    }
    if file
        .local_source_path
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        return Err(StorageError::InvalidInput {
            field: "logical_file.local_source_path",
            reason: InputReason::InvalidCombination,
        });
    }
    let file_id = id_to_sql("logical_file.id", file.id.get())?;
    if file_id == i64::MAX {
        return Err(StorageError::InvalidInput {
            field: "logical_file.id",
            reason: InputReason::OutOfRange,
        });
    }
    unsigned_to_sql("logical_file.size_bytes", file.size_bytes)?;
    if let Some(package_id) = file.package_id {
        id_to_sql("logical_file.package_id", package_id.get())?;
    }
    Ok(())
}

fn upsert_logical_file_on(connection: &Connection, file: &LogicalFileRecord) -> StorageResult<()> {
    let id = id_to_sql("logical_file.id", file.id.get())?;
    let size_bytes = unsigned_to_sql("logical_file.size_bytes", file.size_bytes)?;
    let package_id = file
        .package_id
        .map(|value| id_to_sql("logical_file.package_id", value.get()))
        .transpose()?;
    let (path_encoding, path_bytes) = file
        .local_source_path
        .as_deref()
        .map(encode_local_path)
        .transpose()?
        .map_or((None, None), |(encoding, bytes)| {
            (Some(encoding), Some(bytes))
        });
    connection.execute(
        r#"
INSERT INTO logical_files (
    id, name, relative_path, size_bytes, kind, mime_type, extension, caption,
    source_account_id, source_chat_id, created_at_unix_ms, modified_at_unix_ms,
    remote_state, encryption_state, verification_state, package_id, locally_available,
    local_source_path_encoding, local_source_path
) VALUES (
    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19
)
ON CONFLICT(id) DO UPDATE SET
    name = excluded.name,
    relative_path = excluded.relative_path,
    size_bytes = excluded.size_bytes,
    kind = excluded.kind,
    mime_type = excluded.mime_type,
    extension = excluded.extension,
    caption = excluded.caption,
    source_account_id = excluded.source_account_id,
    source_chat_id = excluded.source_chat_id,
    created_at_unix_ms = excluded.created_at_unix_ms,
    modified_at_unix_ms = excluded.modified_at_unix_ms,
    remote_state = excluded.remote_state,
    encryption_state = excluded.encryption_state,
    verification_state = excluded.verification_state,
    package_id = excluded.package_id,
    locally_available = excluded.locally_available,
    local_source_path_encoding = excluded.local_source_path_encoding,
    local_source_path = excluded.local_source_path
"#,
        params![
            id,
            file.name,
            file.relative_path,
            size_bytes,
            file_kind_code(file.kind)?,
            file.mime_type,
            file.extension,
            file.caption,
            file.source_account_id.map(AccountId::get),
            file.source_chat_id.map(ChatId::get),
            file.created_at_unix_ms,
            file.modified_at_unix_ms,
            remote_state_code(file.remote_state)?,
            encryption_state_code(file.encryption_state)?,
            verification_state_code(file.verification_state)?,
            package_id,
            i64::from(file.locally_available),
            path_encoding,
            path_bytes,
        ],
    )?;
    let following_id = id.checked_add(1).ok_or(StorageError::InvalidInput {
        field: "logical_file.id",
        reason: InputReason::OutOfRange,
    })?;
    connection.execute(
        r#"
UPDATE id_allocators
SET next_id = max(next_id, ?1)
WHERE entity = 'logical_file'
"#,
        [following_id],
    )?;
    Ok(())
}

pub(super) fn row_to_logical_file(row: &Row<'_>) -> StorageResult<LogicalFileRecord> {
    let raw_id: i64 = row.get(0)?;
    let raw_size: i64 = row.get(3)?;
    let raw_kind: String = row.get(4)?;
    let raw_remote: String = row.get(12)?;
    let raw_encryption: String = row.get(13)?;
    let raw_verification: String = row.get(14)?;
    let raw_package_id: Option<i64> = row.get(15)?;
    let locally_available: i64 = row.get(16)?;
    let path_encoding: Option<String> = row.get(17)?;
    let path_bytes: Option<Vec<u8>> = row.get(18)?;

    Ok(LogicalFileRecord {
        id: LogicalFileId::new(id_from_sql("logical_files", "id", raw_id)?),
        name: row.get(1)?,
        relative_path: row.get(2)?,
        size_bytes: nonnegative_from_sql("logical_files", "size_bytes", raw_size)?,
        kind: parse_file_kind(&raw_kind)?,
        mime_type: row.get(5)?,
        extension: row.get(6)?,
        caption: row.get(7)?,
        source_account_id: row.get::<_, Option<i64>>(8)?.map(AccountId::new),
        source_chat_id: row.get::<_, Option<i64>>(9)?.map(ChatId::new),
        created_at_unix_ms: row.get(10)?,
        modified_at_unix_ms: row.get(11)?,
        remote_state: parse_remote_state(&raw_remote)?,
        encryption_state: parse_encryption_state(&raw_encryption)?,
        verification_state: parse_verification_state(&raw_verification)?,
        package_id: raw_package_id
            .map(|value| id_from_sql("logical_files", "package_id", value).map(PackageId::new))
            .transpose()?,
        locally_available: match locally_available {
            0 => false,
            1 => true,
            value => {
                return Err(corrupt("logical_files", "locally_available", value));
            }
        },
        local_source_path: match (path_encoding, path_bytes) {
            (None, None) => None,
            (Some(encoding), Some(bytes)) => Some(decode_local_path(&encoding, bytes)?),
            _ => return Err(corrupt("logical_files", "local_source_path", "incomplete")),
        },
    })
}

#[cfg(unix)]
fn encode_local_path(path: &Path) -> StorageResult<(String, Vec<u8>)> {
    use std::os::unix::ffi::OsStrExt;
    Ok((
        "unix-bytes-v1".to_owned(),
        path.as_os_str().as_bytes().to_vec(),
    ))
}

#[cfg(windows)]
fn encode_local_path(path: &Path) -> StorageResult<(String, Vec<u8>)> {
    use std::os::windows::ffi::OsStrExt;
    let mut bytes = Vec::new();
    for code_unit in path.as_os_str().encode_wide() {
        bytes.extend_from_slice(&code_unit.to_le_bytes());
    }
    Ok(("windows-utf16le-v1".to_owned(), bytes))
}

#[cfg(not(any(unix, windows)))]
fn encode_local_path(path: &Path) -> StorageResult<(String, Vec<u8>)> {
    let value = path.to_str().ok_or(StorageError::InvalidInput {
        field: "logical_file.local_source_path",
        reason: InputReason::InvalidCombination,
    })?;
    Ok(("utf8-v1".to_owned(), value.as_bytes().to_vec()))
}

#[cfg(unix)]
fn decode_local_path(encoding: &str, bytes: Vec<u8>) -> StorageResult<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    if encoding != "unix-bytes-v1" {
        return Err(corrupt(
            "logical_files",
            "local_source_path_encoding",
            encoding,
        ));
    }
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(windows)]
fn decode_local_path(encoding: &str, bytes: Vec<u8>) -> StorageResult<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    if encoding != "windows-utf16le-v1" || bytes.len() % 2 != 0 {
        return Err(corrupt(
            "logical_files",
            "local_source_path_encoding",
            encoding,
        ));
    }
    let wide = bytes
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}

#[cfg(not(any(unix, windows)))]
fn decode_local_path(encoding: &str, bytes: Vec<u8>) -> StorageResult<PathBuf> {
    if encoding != "utf8-v1" {
        return Err(corrupt(
            "logical_files",
            "local_source_path_encoding",
            encoding,
        ));
    }
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|error| corrupt("logical_files", "local_source_path", error))
}

fn parse_file_kind(value: &str) -> StorageResult<FileKind> {
    match value {
        "video" => Ok(FileKind::Video),
        "document" => Ok(FileKind::Document),
        "archive" => Ok(FileKind::Archive),
        "audio" => Ok(FileKind::Audio),
        "image" => Ok(FileKind::Image),
        "disk_image" => Ok(FileKind::DiskImage),
        "other" => Ok(FileKind::Other),
        value => Err(corrupt("logical_files", "kind", value)),
    }
}

fn parse_remote_state(value: &str) -> StorageResult<RemoteState> {
    match value {
        "local_only" => Ok(RemoteState::LocalOnly),
        "uploading" => Ok(RemoteState::Uploading),
        "uploaded" => Ok(RemoteState::Uploaded),
        "remote_missing" => Ok(RemoteState::RemoteMissing),
        value => Err(corrupt("logical_files", "remote_state", value)),
    }
}

fn parse_encryption_state(value: &str) -> StorageResult<EncryptionState> {
    match value {
        "unencrypted" => Ok(EncryptionState::Unencrypted),
        "encrypted" => Ok(EncryptionState::Encrypted),
        "locked" => Ok(EncryptionState::Locked),
        value => Err(corrupt("logical_files", "encryption_state", value)),
    }
}

fn parse_verification_state(value: &str) -> StorageResult<VerificationState> {
    match value {
        "unverified" => Ok(VerificationState::Unverified),
        "verifying" => Ok(VerificationState::Verifying),
        "verified" => Ok(VerificationState::Verified),
        "failed" => Ok(VerificationState::Failed),
        value => Err(corrupt("logical_files", "verification_state", value)),
    }
}

fn validate_nonempty(field: &'static str, value: &str) -> StorageResult<()> {
    if value.trim().is_empty() {
        return Err(StorageError::InvalidInput {
            field,
            reason: InputReason::Empty,
        });
    }
    Ok(())
}

fn id_to_sql(field: &'static str, value: u64) -> StorageResult<i64> {
    i64::try_from(value).map_err(|_| StorageError::InvalidInput {
        field,
        reason: InputReason::OutOfRange,
    })
}

fn unsigned_to_sql(field: &'static str, value: u64) -> StorageResult<i64> {
    id_to_sql(field, value)
}

fn id_from_sql(entity: &'static str, field: &'static str, value: i64) -> StorageResult<u64> {
    nonnegative_from_sql(entity, field, value)
}

fn nonnegative_from_sql(
    entity: &'static str,
    field: &'static str,
    value: i64,
) -> StorageResult<u64> {
    u64::try_from(value).map_err(|_| corrupt(entity, field, value))
}

fn corrupt(entity: &'static str, field: &'static str, value: impl ToString) -> StorageError {
    StorageError::CorruptData {
        entity,
        field,
        value: value.to_string(),
    }
}

fn transaction<'connection>(
    connection: &'connection mut Connection,
) -> StorageResult<Transaction<'connection>> {
    Ok(connection.transaction()?)
}

#[cfg(test)]
mod tests;
