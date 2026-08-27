use rusqlite::{OptionalExtension, params};
use teleark_core::{AccountId, ChatId, MessageId};

use super::{Database, nonnegative_from_sql, unsigned_to_sql};
use crate::StorageResult;
use crate::model::TelegramIndexStateRecord;

impl Database {
    pub fn save_telegram_index_state(
        &mut self,
        state: &TelegramIndexStateRecord,
    ) -> StorageResult<()> {
        self.connection.execute(
            r#"
INSERT INTO telegram_index_state (
    account_id, chat_id, before_message_id, exhausted,
    messages_scanned, files_indexed, updated_at_unix_ms
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
ON CONFLICT(account_id, chat_id) DO UPDATE SET
    before_message_id = excluded.before_message_id,
    exhausted = excluded.exhausted,
    messages_scanned = excluded.messages_scanned,
    files_indexed = excluded.files_indexed,
    updated_at_unix_ms = excluded.updated_at_unix_ms
"#,
            params![
                state.account_id.get(),
                state.chat_id.get(),
                state.before_message_id.map(MessageId::get),
                state.exhausted,
                unsigned_to_sql("telegram_index.messages_scanned", state.messages_scanned)?,
                unsigned_to_sql("telegram_index.files_indexed", state.files_indexed)?,
                state.updated_at_unix_ms,
            ],
        )?;
        Ok(())
    }

    pub fn telegram_index_state(
        &self,
        account_id: AccountId,
        chat_id: ChatId,
    ) -> StorageResult<Option<TelegramIndexStateRecord>> {
        self.connection
            .query_row(
                r#"
SELECT before_message_id, exhausted, messages_scanned, files_indexed, updated_at_unix_ms
FROM telegram_index_state WHERE account_id = ?1 AND chat_id = ?2
"#,
                params![account_id.get(), chat_id.get()],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?
            .map(|(before, exhausted, scanned, files, updated)| {
                Ok(TelegramIndexStateRecord {
                    account_id,
                    chat_id,
                    before_message_id: before.map(MessageId::new),
                    exhausted,
                    messages_scanned: nonnegative_from_sql(
                        "telegram_index_state",
                        "messages_scanned",
                        scanned,
                    )?,
                    files_indexed: nonnegative_from_sql(
                        "telegram_index_state",
                        "files_indexed",
                        files,
                    )?,
                    updated_at_unix_ms: updated,
                })
            })
            .transpose()
    }
}
