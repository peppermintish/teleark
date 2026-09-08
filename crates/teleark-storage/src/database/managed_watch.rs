//! Durable, account-scoped observations. These records never authorize remote writes.
use rusqlite::{OptionalExtension, params};
use teleark_core::{AccountId, ChatId, MessageId};

use super::Database;
use crate::{CachedTelegramFileRecord, InputReason, StorageError, StorageResult};

pub const MANAGED_CHANGE_HISTORY_LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedChannelChangeKind {
    Edited,
    Deleted,
    Gap,
}

impl ManagedChannelChangeKind {
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::Edited => "edited",
            Self::Deleted => "deleted",
            Self::Gap => "gap",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedChannelChange {
    pub sequence: u64,
    pub message_id: i64,
    pub kind: ManagedChannelChangeKind,
    pub observed_at_unix_ms: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ManagedChannelWatch {
    pub catalog_ready: bool,
    pub change_count: u64,
    pub acknowledged_count: u64,
    pub last_changed_at_unix_ms: Option<i64>,
    pub changes: Vec<ManagedChannelChange>,
}

impl ManagedChannelWatch {
    pub fn unacknowledged(&self) -> u64 {
        self.change_count.saturating_sub(self.acknowledged_count)
    }
    pub fn omitted_changes(&self) -> u64 {
        self.change_count.saturating_sub(self.changes.len() as u64)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedManifestCandidate {
    pub file: CachedTelegramFileRecord,
    /// Independent cache invalidation version; does not replace Telegram's PTS or file codec.
    pub revision: i64,
}

impl Database {
    pub fn watch_managed_channel(
        &mut self,
        account: AccountId,
        chat: ChatId,
    ) -> StorageResult<ManagedChannelWatch> {
        self.connection.execute(
            "INSERT OR IGNORE INTO managed_channel_watches(account_id,chat_id) VALUES(?1,?2)",
            params![account.get(), chat.get()],
        )?;
        self.managed_channel_watch(account, chat)
    }

    pub fn managed_channel_watch(
        &self,
        account: AccountId,
        chat: ChatId,
    ) -> StorageResult<ManagedChannelWatch> {
        read_watch(&self.connection, account, chat)
    }

    pub fn acknowledge_managed_changes(
        &mut self,
        account: AccountId,
        chat: ChatId,
        through: u64,
    ) -> StorageResult<ManagedChannelWatch> {
        let through = i64::try_from(through).map_err(|_| StorageError::InvalidInput {
            field: "managed_watch.sequence",
            reason: InputReason::OutOfRange,
        })?;
        self.connection.execute("UPDATE managed_channel_watches SET acknowledged_count=MAX(acknowledged_count,MIN(change_count,?3)) WHERE account_id=?1 AND chat_id=?2", params![account.get(),chat.get(),through])?;
        read_watch(&self.connection, account, chat)
    }

    pub fn cached_manifest_candidates(
        &self,
        account: AccountId,
        chat: ChatId,
        caption: &str,
        limit: usize,
    ) -> StorageResult<Vec<CachedManifestCandidate>> {
        if limit == 0 || limit > 1_001 {
            return Err(StorageError::InvalidInput {
                field: "managed_manifest.limit",
                reason: InputReason::OutOfRange,
            });
        }
        let mut statement = self.connection.prepare("SELECT ro.message_id,f.name,f.caption,f.mime_type,f.size_bytes,COALESCE(f.created_at_unix_ms,f.modified_at_unix_ms,0),COALESCE(f.modified_at_unix_ms,f.created_at_unix_ms,0),COALESCE(v.revision,0) FROM logical_files f JOIN remote_objects ro ON ro.logical_file_id=f.id LEFT JOIN channel_file_versions v ON v.account_id=ro.account_id AND v.chat_id=ro.chat_id AND v.message_id=ro.message_id WHERE f.source_account_id=?1 AND f.source_chat_id=?2 AND ro.account_id=?1 AND ro.chat_id=?2 AND f.caption=?3 AND f.remote_state='uploaded' AND NOT EXISTS(SELECT 1 FROM channel_sync_tombstones t WHERE t.account_id=ro.account_id AND t.chat_id=ro.chat_id AND t.message_id=ro.message_id) ORDER BY f.created_at_unix_ms DESC,ro.message_id DESC LIMIT ?4")?;
        let rows = statement.query_map(
            params![account.get(), chat.get(), caption, limit as i64],
            |row| {
                Ok(CachedManifestCandidate {
                    file: CachedTelegramFileRecord {
                        message_id: MessageId::new(row.get(0)?),
                        file_name: row.get(1)?,
                        caption: row.get(2)?,
                        mime_type: row.get(3)?,
                        size_bytes: row_u64(row, 4)?,
                        sent_at_unix_ms: row.get(5)?,
                        modified_at_unix_ms: row.get(6)?,
                    },
                    revision: row.get(7)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

pub(super) fn read_watch(
    connection: &rusqlite::Connection,
    account: AccountId,
    chat: ChatId,
) -> StorageResult<ManagedChannelWatch> {
    let mut watch = connection.query_row("SELECT catalog_ready,change_count,acknowledged_count,last_changed_at FROM managed_channel_watches WHERE account_id=?1 AND chat_id=?2", params![account.get(),chat.get()], |row| Ok(ManagedChannelWatch { catalog_ready: row.get(0)?, change_count: row_u64(row, 1)?, acknowledged_count: row_u64(row, 2)?, last_changed_at_unix_ms: row.get(3)?, changes: Vec::new() })).optional()?.unwrap_or_default();
    let mut statement = connection.prepare("SELECT sequence,message_id,kind,observed_at FROM managed_channel_changes WHERE account_id=?1 AND chat_id=?2 ORDER BY sequence DESC LIMIT 128")?;
    let rows = statement.query_map(params![account.get(), chat.get()], |row| {
        Ok((
            row_u64(row, 0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (sequence, message_id, kind, at) = row?;
        let kind = match kind.as_str() {
            "edited" => ManagedChannelChangeKind::Edited,
            "deleted" => ManagedChannelChangeKind::Deleted,
            "gap" => ManagedChannelChangeKind::Gap,
            _ => {
                return Err(StorageError::CorruptData {
                    entity: "managed_channel_changes",
                    field: "kind",
                    value: kind,
                });
            }
        };
        watch.changes.push(ManagedChannelChange {
            sequence,
            message_id,
            kind,
            observed_at_unix_ms: at,
        });
    }
    Ok(watch)
}

pub(super) fn record_change(
    connection: &rusqlite::Connection,
    account: AccountId,
    chat: ChatId,
    message: i64,
    kind: ManagedChannelChangeKind,
    pts: i32,
    observed_at: i64,
) -> StorageResult<()> {
    let inserted = connection.execute("INSERT OR IGNORE INTO managed_channel_changes(account_id,chat_id,sequence,message_id,kind,pts,observed_at) SELECT account_id,chat_id,change_count+1,?3,?4,?5,?6 FROM managed_channel_watches WHERE account_id=?1 AND chat_id=?2", params![account.get(),chat.get(),message,kind.code(),pts,observed_at])?;
    if inserted > 0 {
        connection.execute("UPDATE managed_channel_watches SET change_count=change_count+1,last_changed_at=?3 WHERE account_id=?1 AND chat_id=?2", params![account.get(),chat.get(),observed_at])?;
        connection.execute("DELETE FROM managed_channel_changes WHERE account_id=?1 AND chat_id=?2 AND sequence <= (SELECT change_count-128 FROM managed_channel_watches WHERE account_id=?1 AND chat_id=?2)", params![account.get(),chat.get()])?;
    }
    Ok(())
}

pub(super) fn row_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
