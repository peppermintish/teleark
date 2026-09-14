//! Version 16 upload receipts. This table records local operations, never inferred remote provenance.
use super::{Database, corrupt, nonnegative_from_sql, unsigned_to_sql};
use crate::{InputReason, StorageError, StorageResult};
use rusqlite::{Row, params};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredVaultUploadState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
impl StoredVaultUploadState {
    fn code(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
    fn parse(code: &str) -> StorageResult<Self> {
        Ok(match code {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => return Err(corrupt("vault_upload_history", "state", code)),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultUploadRecord {
    pub account_id: i64,
    pub id: u64,
    pub chat_id: i64,
    pub batch_id: Option<u64>,
    pub queued_at_unix_ms: i64,
    pub file_name: String,
    pub package_id: Option<String>,
    pub size_bytes: u64,
    pub transferred_bytes: u64,
    pub completed_parts: u32,
    pub part_count: u32,
    pub started_at_unix_ms: i64,
    pub duration_ms: Option<u64>,
    pub average_bytes_per_second: Option<u64>,
    pub state: StoredVaultUploadState,
    pub failure_code: Option<String>,
}
#[derive(Clone, Debug)]
pub struct VaultUploadHistory {
    pub records: Vec<VaultUploadRecord>,
    pub omitted: u64,
}
const COLUMNS: &str = "account_id,id,chat_id,batch_id,queued_at,file_name,package_id,size_bytes,transferred_bytes,completed_parts,part_count,started_at,duration_ms,average_bps,state,failure_code";
const PAGE: usize = 256;

impl Database {
    /// Acknowledged only at admission, start and completion on the transfer owner.
    /// Samples never synchronously call this method from a network callback.
    pub fn save_vault_uploads(&mut self, records: &[VaultUploadRecord]) -> StorageResult<()> {
        if records.len() > 128 {
            return Err(StorageError::InvalidInput {
                field: "vault_upload_history.batch",
                reason: InputReason::OutOfRange,
            });
        }
        let tx = self.connection.transaction()?;
        for r in records {
            if r.account_id <= 0
                || r.id == 0
                || r.file_name.is_empty()
                || r.file_name.len() > 4096
                || r.package_id.as_ref().is_some_and(|id| id.len() > 128)
                || r.failure_code.as_ref().is_some_and(|id| id.len() > 64)
            {
                return Err(StorageError::InvalidInput {
                    field: "vault_upload_history",
                    reason: InputReason::OutOfRange,
                });
            }
            let changed = tx.execute(&format!("INSERT INTO vault_upload_history ({COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
                ON CONFLICT(account_id,id) DO UPDATE SET chat_id=excluded.chat_id,batch_id=excluded.batch_id,queued_at=excluded.queued_at,file_name=excluded.file_name,package_id=excluded.package_id,size_bytes=excluded.size_bytes,transferred_bytes=excluded.transferred_bytes,completed_parts=excluded.completed_parts,part_count=excluded.part_count,started_at=excluded.started_at,duration_ms=excluded.duration_ms,average_bps=excluded.average_bps,state=excluded.state,failure_code=excluded.failure_code WHERE vault_upload_history.chat_id=excluded.chat_id AND vault_upload_history.batch_id IS excluded.batch_id AND vault_upload_history.queued_at=excluded.queued_at AND vault_upload_history.file_name=excluded.file_name AND vault_upload_history.size_bytes=excluded.size_bytes"),
                params![r.account_id, unsigned_to_sql("upload.id",r.id)?, r.chat_id,r.batch_id.map(|v|unsigned_to_sql("upload.batch",v)).transpose()?,r.queued_at_unix_ms,r.file_name,r.package_id,unsigned_to_sql("upload.size",r.size_bytes)?,unsigned_to_sql("upload.bytes",r.transferred_bytes)?,r.completed_parts,r.part_count,r.started_at_unix_ms,r.duration_ms.map(|v|unsigned_to_sql("upload.duration",v)).transpose()?,r.average_bytes_per_second.map(|v|unsigned_to_sql("upload.speed",v)).transpose()?,r.state.code(),r.failure_code])?;
            if changed != 1 {
                return Err(StorageError::InvalidInput {
                    field: "vault_upload_history.identity",
                    reason: InputReason::InvalidCombination,
                });
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Run once when constructing the service, before it accepts new operations.
    /// No resend: a published manifest may have outlived an unacknowledged local commit.
    pub fn interrupt_previous_vault_uploads(&mut self) -> StorageResult<()> {
        self.connection.execute("UPDATE vault_upload_history SET state='interrupted',failure_code=NULL WHERE state IN ('queued','running')", [])?;
        Ok(())
    }

    /// Bounded account-scoped view. Keep the boundary batch together (at most 128 members).
    /// Older rows stay on disk and are counted explicitly.
    pub fn vault_upload_history(&self, account: i64) -> StorageResult<VaultUploadHistory> {
        if account <= 0 {
            return Err(StorageError::InvalidInput {
                field: "upload.account",
                reason: InputReason::OutOfRange,
            });
        }
        let mut records = read_records(
            &self.connection,
            &format!(
                "SELECT {COLUMNS} FROM vault_upload_history WHERE account_id=?1 ORDER BY sequence DESC LIMIT 256"
            ),
            params![account],
        )?;
        if let Some(batch) = records.last().and_then(|r| r.batch_id) {
            let members = read_records(
                &self.connection,
                &format!(
                    "SELECT {COLUMNS} FROM vault_upload_history WHERE account_id=?1 AND batch_id=?2 ORDER BY sequence DESC LIMIT 129"
                ),
                params![account, unsigned_to_sql("upload.batch", batch)?],
            )?;
            if members.len() > 128 {
                return Err(corrupt("vault_upload_history", "batch", "too_many_members"));
            }
            let ids: std::collections::BTreeSet<_> = records.iter().map(|r| r.id).collect();
            records.extend(members.into_iter().filter(|r| !ids.contains(&r.id)));
        }
        debug_assert!(records.len() < PAGE + 128);
        let total: i64 = self.connection.query_row(
            "SELECT count(*) FROM vault_upload_history WHERE account_id=?1",
            [account],
            |row| row.get(0),
        )?;
        records.reverse();
        Ok(VaultUploadHistory {
            omitted: (total as u64).saturating_sub(records.len() as u64),
            records,
        })
    }
}
fn read_records(
    connection: &rusqlite::Connection,
    sql: &str,
    parameters: impl rusqlite::Params,
) -> StorageResult<Vec<VaultUploadRecord>> {
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement.query(parameters)?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(read(row)?);
    }
    Ok(records)
}
fn read(row: &Row<'_>) -> StorageResult<VaultUploadRecord> {
    Ok(VaultUploadRecord {
        account_id: row.get(0)?,
        id: nonnegative_from_sql("vault_upload_history", "integer", row.get(1)?)?,
        chat_id: row.get(2)?,
        batch_id: row
            .get::<_, Option<i64>>(3)?
            .map(|v| nonnegative_from_sql("vault_upload_history", "integer", v))
            .transpose()?,
        queued_at_unix_ms: row.get(4)?,
        file_name: row.get(5)?,
        package_id: row.get(6)?,
        size_bytes: nonnegative_from_sql("vault_upload_history", "integer", row.get(7)?)?,
        transferred_bytes: nonnegative_from_sql("vault_upload_history", "integer", row.get(8)?)?,
        completed_parts: row.get(9)?,
        part_count: row.get(10)?,
        started_at_unix_ms: row.get(11)?,
        duration_ms: row
            .get::<_, Option<i64>>(12)?
            .map(|v| nonnegative_from_sql("vault_upload_history", "integer", v))
            .transpose()?,
        average_bytes_per_second: row
            .get::<_, Option<i64>>(13)?
            .map(|v| nonnegative_from_sql("vault_upload_history", "integer", v))
            .transpose()?,
        state: StoredVaultUploadState::parse(&row.get::<_, String>(14)?)?,
        failure_code: row.get(15)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(id: u64, state: StoredVaultUploadState) -> VaultUploadRecord {
        VaultUploadRecord {
            account_id: 7,
            id,
            chat_id: 90,
            batch_id: Some(50),
            queued_at_unix_ms: 100,
            file_name: format!("京都 – {id}.bin"),
            package_id: (state == StoredVaultUploadState::Completed)
                .then(|| "0001020304050607".into()),
            size_bytes: 128,
            transferred_bytes: if state == StoredVaultUploadState::Completed {
                128
            } else {
                64
            },
            completed_parts: if state == StoredVaultUploadState::Completed {
                2
            } else {
                1
            },
            part_count: 2,
            started_at_unix_ms: 110,
            duration_ms: Some(25),
            average_bytes_per_second: Some(5120),
            state,
            failure_code: (state == StoredVaultUploadState::Failed).then(|| "network".into()),
        }
    }
    #[test]
    fn reopen_preserves_results_and_interrupts_only_unfinished_operations()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("history.sqlite3");
        let states = [
            StoredVaultUploadState::Queued,
            StoredVaultUploadState::Running,
            StoredVaultUploadState::Completed,
            StoredVaultUploadState::Failed,
            StoredVaultUploadState::Cancelled,
        ];
        let rows: Vec<_> = states
            .into_iter()
            .enumerate()
            .map(|(i, state)| row(i as u64 + 1, state))
            .collect();
        {
            let mut db = Database::open(&path)?;
            db.save_vault_uploads(&rows)?;
        }
        for _ in 0..2 {
            let mut db = Database::open(&path)?;
            db.interrupt_previous_vault_uploads()?;
            let history = db.vault_upload_history(7)?;
            assert_eq!(history.omitted, 0);
            let expected: Vec<_> = rows
                .iter()
                .cloned()
                .map(|mut r| {
                    if matches!(
                        r.state,
                        StoredVaultUploadState::Queued | StoredVaultUploadState::Running
                    ) {
                        r.state = StoredVaultUploadState::Interrupted;
                    }
                    r
                })
                .collect();
            assert_eq!(history.records, expected);
            assert!(db.vault_upload_history(8)?.records.is_empty());
        }
        Ok(())
    }
    #[test]
    fn bounded_account_history_keeps_boundary_batch_and_reports_older_rows()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut db = Database::open_in_memory()?;
        for id in 1..=600 {
            let mut r = row(id, StoredVaultUploadState::Completed);
            r.batch_id = Some((id - 1) / 128 + 1);
            // Identical timestamps, random-looking task IDs; insertion sequence remains deterministic.
            r.id = 601 - id;
            db.save_vault_uploads(&[r])?;
        }
        let mut foreign = row(601, StoredVaultUploadState::Completed);
        foreign.account_id = 8;
        db.save_vault_uploads(&[foreign.clone()])?;
        let history = db.vault_upload_history(7)?;
        assert_eq!(history.records.len(), 344);
        assert_eq!(history.omitted, 256);
        assert_eq!(history.records.first().map(|r| r.id), Some(344));
        assert_eq!(history.records.last().map(|r| r.id), Some(1));
        assert_eq!(db.vault_upload_history(8)?.records, vec![foreign]);
        // Updating an old receipt cannot move it into the recent view.
        let mut old = row(1, StoredVaultUploadState::Failed);
        old.id = 600;
        old.batch_id = Some(1);
        db.save_vault_uploads(&[old])?;
        assert_eq!(db.vault_upload_history(7)?.omitted, 256);
        for sql in [
            format!(
                "EXPLAIN QUERY PLAN SELECT {COLUMNS} FROM vault_upload_history WHERE account_id=7 ORDER BY sequence DESC LIMIT 256"
            ),
            format!(
                "EXPLAIN QUERY PLAN SELECT {COLUMNS} FROM vault_upload_history WHERE account_id=7 AND batch_id=3 ORDER BY sequence DESC LIMIT 129"
            ),
        ] {
            let mut statement = db.connection.prepare(&sql)?;
            let details = statement
                .query_map([], |r| r.get::<_, String>(3))?
                .collect::<Result<Vec<_>, _>>()?
                .join(" ");
            assert!(
                details.contains("SEARCH vault_upload_history USING INDEX vault_upload_history_"),
                "{details}"
            );
            assert!(!details.contains("TEMP B-TREE"), "{details}");
        }
        Ok(())
    }
    #[test]
    fn failed_batch_commit_rolls_back_and_failed_upgrade_preserves_v15_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut db = Database::open_in_memory()?;
        let valid = row(1, StoredVaultUploadState::Completed);
        let mut invalid = row(2, StoredVaultUploadState::Running);
        invalid.transferred_bytes = 129;
        assert!(db.save_vault_uploads(&[valid.clone(), invalid]).is_err());
        assert!(db.vault_upload_history(7)?.records.is_empty());
        // Simulate disk full before the transaction can acknowledge completion.
        db.connection.execute_batch("CREATE TRIGGER upload_full BEFORE INSERT ON vault_upload_history BEGIN SELECT RAISE(ABORT, 'synthetic full'); END;")?;
        assert!(db.save_vault_uploads(&[valid]).is_err());
        assert!(db.vault_upload_history(7)?.records.is_empty());
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("upgrade.sqlite3");
        {
            let connection = rusqlite::Connection::open(&path)?;
            for migration in crate::migration::MIGRATIONS
                .iter()
                .filter(|m| m.version <= 15)
            {
                connection.execute_batch(migration.sql)?;
            }
            connection.pragma_update(None, "application_id", crate::migration::APPLICATION_ID)?;
            connection.pragma_update(None, "user_version", 15)?;
            connection.execute(
                "INSERT INTO vault_key_epochs VALUES (?1,?2,?3,1,1,10,10)",
                params![vec![7_u8; 16], vec![8_u8; 132], vec![9_u8; 88]],
            )?;
            connection.execute_batch("CREATE TABLE vault_upload_history (sentinel TEXT); INSERT INTO vault_upload_history VALUES ('recoverable original');")?;
        }
        assert!(Database::open(&path).is_err());
        let connection = rusqlite::Connection::open(&path)?;
        assert_eq!(
            connection.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))?,
            15
        );
        assert_eq!(
            connection.query_row("SELECT password_wrap FROM vault_key_epochs", [], |r| r
                .get::<_, Vec<u8>>(0))?,
            vec![8; 132]
        );
        assert_eq!(
            connection.query_row("SELECT sentinel FROM vault_upload_history", [], |r| r
                .get::<_, String>(0))?,
            "recoverable original"
        );
        connection.execute_batch("DROP TABLE vault_upload_history")?;
        drop(connection);
        let db = Database::open(&path)?;
        assert_eq!(db.schema_version()?, crate::LATEST_SCHEMA_VERSION);
        assert_eq!(
            db.connection
                .query_row("SELECT recovery_wrap FROM vault_key_epochs", [], |r| r
                    .get::<_, Vec<u8>>(0))?,
            vec![9; 88]
        );
        Ok(())
    }
}
