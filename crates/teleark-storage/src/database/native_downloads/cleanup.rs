//! Durable cancellation cleanup; publication owners must drain before acknowledgment.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeDownloadCleanup {
    pub task_id: u64,
    pub codec_version: u32,
    pub attempt: u32,
    pub requested_at_unix_ms: i64,
    pub retry_requested: bool,
}

impl Database {
    /// Commit cancellation and its cleanup obligation together. A stale attempt
    /// or a completed task cannot acquire a cleanup lease.
    pub fn begin_native_download_cleanup(
        &mut self,
        task: &NativeDownloadTaskRecord,
    ) -> StorageResult<bool> {
        validate_task(task)?;
        if task.state != StoredNativeDownloadState::Cancelled {
            return Err(StorageError::InvalidInput {
                field: "native_cleanup.state",
                reason: InputReason::InvalidCombination,
            });
        }
        let id = unsigned_to_sql("native_cleanup.id", task.id)?;
        self.durable_vault_write(|tx| {
            let eligible: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_download_tasks WHERE id=?1 AND account_id IS ?2 AND attempts=?3 AND state!='completed')",
                params![id, task.account_id, task.attempts], |row| row.get(0))?;
            if !eligible { return Ok(false); }
            tx.execute("INSERT INTO native_download_cleanup(task_id,attempt,requested_at) VALUES(?1,?2,?3) ON CONFLICT(task_id) DO NOTHING", params![id,task.attempts,task.updated_at_unix_ms])?;
            let compatible: bool = tx.query_row("SELECT codec_version=1 AND attempt=?2 FROM native_download_cleanup WHERE task_id=?1", params![id,task.attempts], |row| row.get(0))?;
            if !compatible { return Err(StorageError::InvalidInput { field: "native_cleanup.lease", reason: InputReason::InvalidCombination }); }
            save_task(tx, task)?;
            Ok(true)
        })
    }

    pub fn native_download_cleanup(&self, id: u64) -> StorageResult<Option<NativeDownloadCleanup>> {
        Ok(self.connection.query_row(
            "SELECT task_id,codec_version,attempt,requested_at,retry_requested FROM native_download_cleanup WHERE task_id=?1",
            [unsigned_to_sql("native_cleanup.id", id)?],
            |row| Ok(NativeDownloadCleanup {
                task_id: u64::try_from(row.get::<_, i64>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)?, codec_version: row.get(1)?, attempt: row.get(2)?,
                requested_at_unix_ms: row.get(3)?, retry_requested: row.get(4)?,
            }),
        ).optional()?)
    }

    /// Keyset pages retain unknown future codecs for non-destructive explanation.
    pub fn pending_native_download_cleanups(
        &self,
        after: u64,
        limit: u16,
    ) -> StorageResult<Vec<NativeDownloadCleanup>> {
        if limit == 0 || limit > 128 {
            return Err(StorageError::InvalidInput {
                field: "native_cleanup.limit",
                reason: InputReason::OutOfRange,
            });
        }
        let mut statement = self.connection.prepare("SELECT task_id,codec_version,attempt,requested_at,retry_requested FROM native_download_cleanup WHERE task_id>?1 ORDER BY task_id LIMIT ?2")?;
        Ok(statement
            .query_map(
                params![unsigned_to_sql("native_cleanup.cursor", after)?, limit],
                |row| {
                    Ok(NativeDownloadCleanup {
                        task_id: u64::try_from(row.get::<_, i64>(0)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        codec_version: row.get(1)?,
                        attempt: row.get(2)?,
                        requested_at_unix_ms: row.get(3)?,
                        retry_requested: row.get(4)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Retry is durable while cleanup still owns the partial. It does not queue
    /// a new writer; only successful cleanup acknowledgment may do that.
    pub fn request_native_cleanup_retry(
        &mut self,
        task_id: u64,
        attempt: u32,
    ) -> StorageResult<bool> {
        self.durable_vault_write(|tx| Ok(tx.execute(
            "UPDATE native_download_cleanup SET retry_requested=1 WHERE task_id=?1 AND attempt=?2 AND codec_version=1",
            params![unsigned_to_sql("native_cleanup.id", task_id)?,attempt],
        )? == 1))
    }

    /// The caller has exclusively removed TeleArk-owned partial artifacts and
    /// synchronized their parent directory. Acknowledgment and queued retry are
    /// one commit, so a crash cannot strand an accepted retry between them.
    pub fn finish_native_download_cleanup(
        &mut self,
        task_id: u64,
        attempt: u32,
    ) -> StorageResult<Option<bool>> {
        let id = unsigned_to_sql("native_cleanup.id", task_id)?;
        self.durable_vault_write(|tx| {
            let retry: Option<bool> = tx.query_row("SELECT retry_requested FROM native_download_cleanup WHERE task_id=?1 AND attempt=?2 AND codec_version=1",params![id,attempt], |row| row.get(0)).optional()?;
            let Some(retry) = retry else { return Ok(None); };
            let changed = tx.execute("UPDATE native_download_tasks SET state=CASE WHEN ?3 THEN 'queued' ELSE 'cancelled' END, verification=CASE WHEN ?3 THEN 'pending' ELSE 'not_reached' END, transferred_bytes=0, average_bytes_per_second=NULL, failure_code=NULL, finished_at_unix_ms=CASE WHEN ?3 THEN NULL ELSE finished_at_unix_ms END WHERE id=?1 AND attempts=?2 AND state='cancelled'", params![id,attempt,retry])?;
            if changed != 1 { return Err(StorageError::InvalidInput { field: "native_cleanup.owner", reason: InputReason::InvalidCombination }); }
            tx.execute("DELETE FROM native_download_cleanup WHERE task_id=?1 AND attempt=?2 AND codec_version=1", params![id,attempt])?;
            Ok(Some(retry))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(db: &mut Database, path: &std::path::Path) -> NativeDownloadTaskRecord {
        let mut task = db
            .insert_native_download(&NewNativeDownloadTaskRecord {
                account_id: 7,
                chat_id: 11,
                message_id: 19,
                message_sent_at_unix_ms: None,
                file_name: "synthetic.bin".into(),
                caption: None,
                mime_type: None,
                size_bytes: 4096,
                destination: path.join("synthetic.bin"),
                created_at_unix_ms: 10,
            })
            .expect("task");
        task.state = StoredNativeDownloadState::Running;
        task.attempts = 1;
        task.transferred_bytes = 1024;
        task.started_at_unix_ms = Some(11);
        task.updated_at_unix_ms = 12;
        db.save_native_download(&task).expect("running");
        task
    }

    #[test]
    fn cancellation_cleanup_and_retry_survive_restart_and_fence_old_writes() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("cleanup.sqlite");
        let mut db = Database::open(&path).expect("database");
        let running = task(&mut db, dir.path());
        let mut cancelled = running.clone();
        cancelled.state = StoredNativeDownloadState::Cancelled;
        cancelled.verification = StoredNativeDownloadVerification::NotReached;
        cancelled.finished_at_unix_ms = Some(13);
        cancelled.updated_at_unix_ms = 13;
        assert!(
            db.begin_native_download_cleanup(&cancelled)
                .expect("durable cancellation")
        );
        assert!(
            db.save_native_download(&running).is_err(),
            "old owner cannot revive cancelled work"
        );
        assert!(
            !db.save_native_download_progress(&running)
                .expect("old progress")
        );
        assert!(
            db.delete_native_download(running.id).is_err(),
            "history cannot orphan cleanup"
        );
        assert!(
            !db.request_native_cleanup_retry(running.id, 0)
                .expect("stale retry")
        );
        assert!(
            db.request_native_cleanup_retry(running.id, 1)
                .expect("durable retry")
        );
        drop(db);
        let mut db = Database::open(&path).expect("reopen");
        assert_eq!(
            db.native_download(running.id)
                .expect("task")
                .expect("saved")
                .state,
            StoredNativeDownloadState::Cancelled
        );
        let cleanup = db
            .pending_native_download_cleanups(0, 128)
            .expect("pending");
        assert_eq!(cleanup.len(), 1);
        assert!(cleanup[0].retry_requested);
        assert_eq!(
            db.finish_native_download_cleanup(running.id, 0)
                .expect("stale finish"),
            None
        );
        assert_eq!(
            db.finish_native_download_cleanup(running.id, 1)
                .expect("acknowledged cleanup"),
            Some(true)
        );
        drop(db);
        let db = Database::open(&path).expect("reopen queued retry");
        assert!(
            db.native_download_cleanup(running.id)
                .expect("cleanup")
                .is_none()
        );
        let resumed = db
            .native_download(running.id)
            .expect("task")
            .expect("queued");
        assert_eq!(resumed.state, StoredNativeDownloadState::Queued);
        assert_eq!(resumed.transferred_bytes, 0);
        assert_eq!(resumed.finished_at_unix_ms, None);
        assert_eq!(resumed.destination, running.destination);
    }

    #[test]
    fn future_cleanup_and_completed_tasks_are_never_reinterpreted() {
        let dir = tempfile::tempdir().expect("directory");
        let mut db = Database::open_in_memory().expect("database");
        let mut row = task(&mut db, dir.path());
        row.state = StoredNativeDownloadState::Completed;
        row.transferred_bytes = row.size_bytes;
        row.verification = StoredNativeDownloadVerification::SizeChecked;
        row.finished_at_unix_ms = Some(20);
        db.save_native_download(&row).expect("completed");
        row.state = StoredNativeDownloadState::Cancelled;
        row.verification = StoredNativeDownloadVerification::NotReached;
        assert!(
            !db.begin_native_download_cleanup(&row)
                .expect("cannot cancel completion")
        );
        db.connection.execute("INSERT INTO native_download_cleanup(task_id,codec_version,attempt,requested_at) VALUES(?1,2,1,20)", [row.id as i64]).expect("future fixture");
        assert!(
            !db.request_native_cleanup_retry(row.id, 1)
                .expect("future retry")
        );
        assert_eq!(
            db.finish_native_download_cleanup(row.id, 1)
                .expect("future finish"),
            None
        );
        assert_eq!(
            db.native_download_cleanup(row.id)
                .expect("future record")
                .expect("preserved")
                .codec_version,
            2
        );
    }

    #[test]
    fn schema_twenty_upgrade_preserves_jobs_and_rolls_back_on_conflict()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::migration::{APPLICATION_ID, MIGRATIONS};
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("upgrade.sqlite");
        let connection = Connection::open(&path)?;
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 20)
        {
            connection.execute_batch(migration.sql)?;
        }
        connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        connection.pragma_update(None, "user_version", 20)?;
        connection.execute_batch("INSERT INTO vault_transfer_jobs(account_id,id,chat_id,direction,package_id,context_version,context,state,generation,created_at,updated_at) VALUES(7,1,11,'upload',zeroblob(16),1,X'010203','paused',5,100,101); CREATE TABLE native_download_cleanup (fixture TEXT); INSERT INTO native_download_cleanup VALUES('preserve');")?;
        drop(connection);
        assert!(Database::open(&path).is_err());
        let connection = Connection::open(&path)?;
        assert_eq!(
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?,
            20
        );
        assert_eq!(
            connection.query_row("SELECT fixture FROM native_download_cleanup", [], |row| row
                .get::<_, String>(0))?,
            "preserve"
        );
        assert_eq!(
            connection.query_row(
                "SELECT context FROM vault_transfer_jobs WHERE id=1",
                [],
                |row| row.get::<_, Vec<u8>>(0)
            )?,
            vec![1, 2, 3]
        );
        connection.execute_batch("DROP TABLE native_download_cleanup")?;
        drop(connection);
        let db = Database::open(&path)?;
        assert_eq!(
            db.connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?,
            crate::LATEST_SCHEMA_VERSION
        );
        assert!(db.pending_native_download_cleanups(0, 128)?.is_empty());
        assert_eq!(
            db.vault_job(7, 1)?.expect("preserved job").context,
            vec![1, 2, 3]
        );
        Ok(())
    }

    #[test]
    fn failed_cancel_commit_rolls_back_cleanup_intent_and_preserves_running_task() {
        let dir = tempfile::tempdir().expect("directory");
        let mut db = Database::open_in_memory().expect("database");
        let original = task(&mut db, dir.path());
        let mut cancelled = original.clone();
        cancelled.state = StoredNativeDownloadState::Cancelled;
        cancelled.verification = StoredNativeDownloadVerification::NotReached;
        cancelled.finished_at_unix_ms = Some(20);
        let previous: i64 = db
            .connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .expect("policy");
        db.connection.execute_batch("CREATE TRIGGER deny_cancel BEFORE UPDATE ON native_download_tasks WHEN NEW.state='cancelled' BEGIN SELECT RAISE(ABORT,'synthetic write failure'); END;").expect("failure fixture");
        assert!(db.begin_native_download_cleanup(&cancelled).is_err());
        assert!(
            db.native_download_cleanup(original.id)
                .expect("marker")
                .is_none()
        );
        assert_eq!(
            db.native_download(original.id)
                .expect("task")
                .expect("preserved"),
            original
        );
        assert_eq!(
            db.connection
                .pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))
                .expect("restored policy"),
            previous
        );
        db.connection
            .execute_batch("DROP TRIGGER deny_cancel")
            .expect("remove synthetic failure");
        cancelled.account_id = Some(8);
        assert!(
            !db.begin_native_download_cleanup(&cancelled)
                .expect("foreign account rejected")
        );
        cancelled.account_id = Some(7);
        assert!(
            db.begin_native_download_cleanup(&cancelled)
                .expect("retry cancel commit")
        );
        assert_eq!(
            db.finish_native_download_cleanup(cancelled.id, 1)
                .expect("cleanup ack"),
            Some(false)
        );
        assert_eq!(
            db.native_download(cancelled.id)
                .expect("task")
                .expect("terminal")
                .state,
            StoredNativeDownloadState::Cancelled
        );
    }
}
