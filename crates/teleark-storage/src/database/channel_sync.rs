use rusqlite::{OptionalExtension, params};
use teleark_core::{AccountId, ChatId};

use super::{Database, remote::upsert_remote_file_on};
use crate::{
    CachedTelegramFileRecord, InputReason, InvariantViolation, ManagedChannelChangeKind,
    ManagedChannelWatch, RemoteFileUpsert, StorageError, StorageResult,
};

/// Versioned by SQLite schema 11. PTS is an update sequence, never a message ID.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChannelSyncState {
    pub pts: i32,
    pub history_before: Option<i64>,
    pub history_exhausted: bool,
    pub repair_pending: bool,
    pub repair_before: Option<i64>,
    pub gap_pending: bool,
    pub gap_before: Option<i64>,
    pub gap_until: Option<i64>,
    pub revision: i64,
}

pub struct ChannelSyncCommit {
    pub account_id: AccountId,
    pub chat_id: ChatId,
    pub expected_revision: i64,
    pub state: ChannelSyncState,
    pub files: Vec<RemoteFileUpsert>,
    pub removed: Vec<i64>,
    /// Only authoritative difference/verification responses may revive an
    /// edited document. A stale history page cannot undo a deletion.
    pub authoritative: bool,
    pub edited: Vec<i64>,
    /// Re-authenticate unchanged manifests after a recovered update gap, without inventing an edit.
    pub invalidate: Vec<i64>,
    pub managed_catalog_seed: bool,
    pub gap_detected: bool,
    pub observed_at_unix_ms: i64,
}

#[derive(Clone, Debug, Default)]
pub struct ChannelSyncCommitOutcome {
    /// Actual committed projection changes, excluding unchanged/obsolete pages.
    pub upserted: Vec<CachedTelegramFileRecord>,
    pub removed: Vec<i64>,
    pub managed_watch: Option<ManagedChannelWatch>,
}

impl Database {
    /// Legacy directory baseline for installations predating the versioned restart cache.
    /// Only sources with a channel cursor are channels; generic chats are never guessed.
    pub fn cached_channel_sources(
        &self,
        account: AccountId,
    ) -> StorageResult<Vec<crate::ChatRecord>> {
        let mut statement = self.connection.prepare("SELECT c.id,c.title,c.username,c.updated_at_unix_ms FROM channel_sync_state s JOIN chats c ON c.account_id=s.account_id AND c.id=s.chat_id WHERE s.account_id=?1 ORDER BY s.chat_id LIMIT 10000")?;
        let rows = statement.query_map([account.get()], |row| {
            Ok(crate::ChatRecord {
                account_id: account,
                id: ChatId::new(row.get(0)?),
                title: row.get(1)?,
                username: row.get(2)?,
                updated_at_unix_ms: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// A single SQLite snapshot closes races between loading rows and their revision.
    pub fn cached_channel_view(
        &self,
        account: AccountId,
        chat: ChatId,
        limit: usize,
    ) -> StorageResult<(ChannelSyncState, Vec<CachedTelegramFileRecord>)> {
        let tx = self.connection.unchecked_transaction()?;
        let state = read_state(&tx, account, chat)?;
        let files = self.cached_telegram_files(account, chat, limit)?;
        tx.commit()?;
        Ok((state, files))
    }
    pub fn channel_sync_state(
        &self,
        account: AccountId,
        chat: ChatId,
    ) -> StorageResult<ChannelSyncState> {
        read_state(&self.connection, account, chat)
    }

    /// Metadata, deletion evidence and cursor move in one transaction. A
    /// failed/stale commit retains the previous cursor for safe replay.
    pub fn commit_channel_sync(
        &mut self,
        batch: &ChannelSyncCommit,
    ) -> StorageResult<ChannelSyncCommitOutcome> {
        if batch.files.len() + batch.removed.len() > 2_000
            || batch.edited.len() > 2_000
            || batch.invalidate.len() > 2_000
            || batch.state.pts < 0
            || batch.expected_revision < 0
            || Some(batch.state.revision) != batch.expected_revision.checked_add(1)
            || batch.files.iter().any(|file| {
                file.account_id != batch.account_id
                    || file.chat_id != batch.chat_id
                    || file.message_id.get() <= 0
            })
            || batch.removed.iter().any(|id| *id <= 0)
            || batch.edited.iter().any(|id| *id <= 0)
            || batch
                .invalidate
                .iter()
                .any(|id| !batch.files.iter().any(|file| file.message_id.get() == *id))
        {
            return Err(StorageError::InvalidInput {
                field: "channel_sync.batch",
                reason: InputReason::InvalidCombination,
            });
        }
        let tx = self.connection.transaction()?;
        let previous = read_state(&tx, batch.account_id, batch.chat_id)?;
        if previous.revision != batch.expected_revision || batch.state.pts < previous.pts {
            return Err(StorageError::Invariant(
                InvariantViolation::RemoteRevisionConflict,
            ));
        }
        let scope = (batch.account_id.get(), batch.chat_id.get());
        let watched: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM managed_channel_watches WHERE account_id=?1 AND chat_id=?2)", params![scope.0,scope.1], |row| row.get(0))?;
        let mut outcome = ChannelSyncCommitOutcome::default();
        for file in &batch.files {
            let previous =
                read_cached_file(&tx, batch.account_id, batch.chat_id, file.message_id.get())?;
            if batch.authoritative {
                tx.execute("DELETE FROM channel_sync_tombstones WHERE account_id=?1 AND chat_id=?2 AND message_id=?3", params![scope.0, scope.1, file.message_id.get()])?;
            } else if tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_sync_tombstones WHERE account_id=?1 AND chat_id=?2 AND message_id=?3)", params![scope.0, scope.1, file.message_id.get()], |row| row.get::<_, bool>(0))? {
                continue;
            }
            // History/search responses have no channel sequence. They may fill
            // unknown IDs, but cannot overwrite a newer push (including edits
            // within the same timestamp second). Differences/verification own edits.
            if !batch.authoritative && previous.is_some() {
                continue;
            }
            let incoming = CachedTelegramFileRecord {
                message_id: file.message_id,
                file_name: file.name.clone(),
                caption: file.caption.clone(),
                mime_type: file.mime_type.clone(),
                size_bytes: file.size_bytes,
                sent_at_unix_ms: file.sent_at_unix_ms,
                modified_at_unix_ms: file.modified_at_unix_ms,
            };
            let explicitly_edited = batch.edited.contains(&file.message_id.get());
            if previous.as_ref() == Some(&incoming) && !explicitly_edited {
                if batch.invalidate.contains(&file.message_id.get()) {
                    tx.execute("INSERT INTO channel_file_versions(account_id,chat_id,message_id,revision) VALUES(?1,?2,?3,?4) ON CONFLICT(account_id,chat_id,message_id) DO UPDATE SET revision=excluded.revision", params![scope.0,scope.1,file.message_id.get(),batch.state.revision])?;
                    outcome.upserted.push(incoming);
                }
                continue;
            }
            let (stored, _) = upsert_remote_file_on(&tx, file, batch.authoritative)?;
            let committed = CachedTelegramFileRecord {
                message_id: file.message_id,
                file_name: stored.name,
                caption: stored.caption,
                mime_type: stored.mime_type,
                size_bytes: stored.size_bytes,
                sent_at_unix_ms: stored.created_at_unix_ms.unwrap_or(0),
                modified_at_unix_ms: stored.modified_at_unix_ms.unwrap_or(0),
            };
            if previous.as_ref() != Some(&committed) || explicitly_edited {
                outcome.upserted.push(committed);
                tx.execute("INSERT INTO channel_file_versions(account_id,chat_id,message_id,revision) VALUES(?1,?2,?3,?4) ON CONFLICT(account_id,chat_id,message_id) DO UPDATE SET revision=excluded.revision", params![scope.0,scope.1,file.message_id.get(),batch.state.revision])?;
                if watched && previous.is_some() {
                    super::managed_watch::record_change(
                        &tx,
                        batch.account_id,
                        batch.chat_id,
                        file.message_id.get(),
                        ManagedChannelChangeKind::Edited,
                        batch.state.pts,
                        batch.observed_at_unix_ms,
                    )?;
                }
            }
        }
        for id in &batch.removed {
            if read_cached_file(&tx, batch.account_id, batch.chat_id, *id)?.is_some() {
                outcome.removed.push(*id);
            }
            let inserted = tx.execute("INSERT OR IGNORE INTO channel_sync_tombstones(account_id,chat_id,message_id) VALUES(?1,?2,?3)", params![scope.0, scope.1, id])?;
            if watched && inserted > 0 && !batch.edited.contains(id) {
                super::managed_watch::record_change(
                    &tx,
                    batch.account_id,
                    batch.chat_id,
                    *id,
                    ManagedChannelChangeKind::Deleted,
                    batch.state.pts,
                    batch.observed_at_unix_ms,
                )?;
            }
            // Keep local copies, transfers and logical identity intact.
            tx.execute("UPDATE logical_files SET remote_state='remote_missing' WHERE id IN (SELECT logical_file_id FROM remote_objects WHERE account_id=?1 AND chat_id=?2 AND message_id=?3)", params![scope.0, scope.1, id])?;
        }
        if watched {
            for id in &batch.edited {
                super::managed_watch::record_change(
                    &tx,
                    batch.account_id,
                    batch.chat_id,
                    *id,
                    ManagedChannelChangeKind::Edited,
                    batch.state.pts,
                    batch.observed_at_unix_ms,
                )?;
            }
            if batch.gap_detected {
                super::managed_watch::record_change(
                    &tx,
                    batch.account_id,
                    batch.chat_id,
                    0,
                    ManagedChannelChangeKind::Gap,
                    batch.state.pts,
                    batch.observed_at_unix_ms,
                )?;
            }
            if batch.managed_catalog_seed {
                tx.execute("UPDATE managed_channel_watches SET catalog_ready=1 WHERE account_id=?1 AND chat_id=?2", params![scope.0,scope.1])?;
            }
            outcome.managed_watch = Some(super::managed_watch::read_watch(
                &tx,
                batch.account_id,
                batch.chat_id,
            )?);
        }
        let state = &batch.state;
        tx.execute("INSERT INTO channel_sync_state(account_id,chat_id,pts,history_before,history_exhausted,repair_pending,repair_before,gap_pending,gap_before,gap_until,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(account_id,chat_id) DO UPDATE SET pts=excluded.pts,history_before=excluded.history_before,history_exhausted=excluded.history_exhausted,repair_pending=excluded.repair_pending,repair_before=excluded.repair_before,gap_pending=excluded.gap_pending,gap_before=excluded.gap_before,gap_until=excluded.gap_until,revision=excluded.revision", params![scope.0, scope.1, state.pts, state.history_before, state.history_exhausted, state.repair_pending, state.repair_before, state.gap_pending, state.gap_before, state.gap_until, state.revision])?;
        tx.commit()?;
        Ok(outcome)
    }

    /// Stable bounded traversal for recovering a server-declared difference
    /// overflow. Missing IDs in ordinary message history are not gaps.
    pub fn channel_cached_message_ids(
        &self,
        account: AccountId,
        chat: ChatId,
        before: Option<i64>,
    ) -> StorageResult<Vec<i64>> {
        let mut statement = self.connection.prepare("SELECT message_id FROM remote_objects WHERE account_id=?1 AND chat_id=?2 AND (?3 IS NULL OR message_id<?3) ORDER BY message_id DESC LIMIT 100")?;
        Ok(statement
            .query_map(params![account.get(), chat.get(), before], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    }
}

fn read_cached_file(
    connection: &rusqlite::Connection,
    account: AccountId,
    chat: ChatId,
    message: i64,
) -> StorageResult<Option<CachedTelegramFileRecord>> {
    Ok(connection.query_row("SELECT f.name,f.caption,f.mime_type,f.size_bytes,COALESCE(f.created_at_unix_ms,f.modified_at_unix_ms,0),COALESCE(f.modified_at_unix_ms,f.created_at_unix_ms,0) FROM remote_objects ro JOIN logical_files f ON f.id=ro.logical_file_id WHERE ro.account_id=?1 AND ro.chat_id=?2 AND ro.message_id=?3 AND f.remote_state='uploaded' AND NOT EXISTS(SELECT 1 FROM channel_sync_tombstones t WHERE t.account_id=ro.account_id AND t.chat_id=ro.chat_id AND t.message_id=ro.message_id)", params![account.get(),chat.get(),message], |row| Ok(CachedTelegramFileRecord {
        message_id: teleark_core::MessageId::new(message), file_name: row.get(0)?, caption: row.get(1)?, mime_type: row.get(2)?, size_bytes: super::managed_watch::row_u64(row, 3)?, sent_at_unix_ms: row.get(4)?, modified_at_unix_ms: row.get(5)?,
    })).optional()?)
}

fn read_state(
    connection: &rusqlite::Connection,
    account: AccountId,
    chat: ChatId,
) -> StorageResult<ChannelSyncState> {
    Ok(connection.query_row("SELECT pts,history_before,history_exhausted,repair_pending,repair_before,gap_pending,gap_before,gap_until,revision FROM channel_sync_state WHERE account_id=?1 AND chat_id=?2", params![account.get(), chat.get()], |row| Ok(ChannelSyncState {
        pts: row.get(0)?, history_before: row.get(1)?, history_exhausted: row.get(2)?, repair_pending: row.get(3)?, repair_before: row.get(4)?, gap_pending: row.get(5)?, gap_before: row.get(6)?, gap_until: row.get(7)?, revision: row.get(8)?,
    })).optional()?.unwrap_or_default())
}

#[cfg(test)]
mod directory_tests {
    use super::*;
    #[test]
    fn legacy_directory_query_uses_account_and_channel_indexes() {
        let database = Database::open_in_memory().expect("database");
        let mut statement = database.connection.prepare("EXPLAIN QUERY PLAN SELECT c.id,c.title,c.username,c.updated_at_unix_ms FROM channel_sync_state s JOIN chats c ON c.account_id=s.account_id AND c.id=s.chat_id WHERE s.account_id=?1 ORDER BY s.chat_id LIMIT 10000").expect("plan");
        let plan = statement
            .query_map([42_i64], |row| row.get::<_, String>(3))
            .expect("query plan")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows")
            .join("\n");
        assert!(
            plan.contains("SEARCH s USING") && plan.contains("account_id=?"),
            "{plan}"
        );
        assert!(
            plan.contains("SEARCH c USING") && plan.contains("id=?"),
            "{plan}"
        );
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        assert!(
            database
                .cached_channel_sources(AccountId::new(42))
                .expect("empty")
                .is_empty()
        );
    }
}
