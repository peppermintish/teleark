//! Opaque local manifest recovery inventory. Never stores decrypted names or keys.
use super::Database;
use crate::{InputReason, StorageError, StorageResult};
use rusqlite::{OptionalExtension as _, params};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VaultFileHealth {
    #[default]
    Unchecked,
    Present,
    MissingParts,
    MissingManifest,
    KeyUnavailable,
    InvalidManifest,
}

#[derive(Clone)]
pub struct VaultInventoryRecord {
    pub account_id: i64,
    pub chat_id: i64,
    pub manifest_message_id: i64,
    pub remote_name: String,
    pub vault_id: [u8; 16],
    pub sealed_manifest: Vec<u8>,
    pub observed_at_unix_ms: i64,
    pub manifest_invalid: bool,
}

impl Database {
    pub fn save_vault_inventory(&mut self, record: &VaultInventoryRecord) -> StorageResult<()> {
        if record.account_id <= 0
            || record.chat_id <= 0
            || record.manifest_message_id <= 0
            || record.sealed_manifest.is_empty()
            || record.sealed_manifest.len() > 16 * 1024 * 1024
            || record.remote_name.len() > 128
        {
            return Err(StorageError::InvalidInput {
                field: "vault_inventory",
                reason: InputReason::OutOfRange,
            });
        }
        // Keep the first authenticated envelope if the remote message is later
        // replaced or corrupted; updates to existing bytes require another record.
        self.connection.execute("INSERT OR IGNORE INTO vault_inventory(account_id,chat_id,manifest_message_id,remote_name,vault_id,sealed_manifest,observed_at) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![record.account_id,record.chat_id,record.manifest_message_id,record.remote_name,record.vault_id.as_slice(),record.sealed_manifest,record.observed_at_unix_ms])?;
        Ok(())
    }

    pub fn vault_inventory_page(
        &self,
        account: i64,
        chat: i64,
        before: i64,
        limit: usize,
    ) -> StorageResult<Vec<VaultInventoryRecord>> {
        if limit == 0 || limit > 32 {
            return Err(StorageError::InvalidInput {
                field: "vault_inventory.limit",
                reason: InputReason::OutOfRange,
            });
        }
        let mut statement = self.connection.prepare("SELECT manifest_message_id,remote_name,vault_id,sealed_manifest,observed_at,manifest_invalid FROM vault_inventory WHERE account_id=?1 AND chat_id=?2 AND manifest_message_id<?3 ORDER BY manifest_message_id DESC LIMIT ?4")?;
        let rows = statement.query_map(params![account, chat, before, limit as i64], |row| {
            let id: Vec<u8> = row.get(2)?;
            let vault_id: [u8; 16] = id.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(VaultInventoryRecord {
                account_id: account,
                chat_id: chat,
                manifest_message_id: row.get(0)?,
                remote_name: row.get(1)?,
                vault_id,
                sealed_manifest: row.get(3)?,
                observed_at_unix_ms: row.get(4)?,
                manifest_invalid: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn vault_manifest_locator(
        &self,
        account: i64,
        chat: i64,
        name: &str,
    ) -> StorageResult<Option<(i64, u64)>> {
        Ok(self.connection.query_row("SELECT manifest_message_id,length(sealed_manifest) FROM vault_inventory WHERE account_id=?1 AND chat_id=?2 AND remote_name=?3 ORDER BY manifest_message_id DESC LIMIT 1",params![account,chat,name],|row|Ok((row.get(0)?,u64::try_from(row.get::<_,i64>(1)?).map_err(|_|rusqlite::Error::InvalidQuery)?))).optional()?)
    }

    pub fn set_vault_manifest_invalid(
        &mut self,
        account: i64,
        chat: i64,
        manifest: i64,
        invalid: bool,
    ) -> StorageResult<()> {
        self.connection.execute("UPDATE vault_inventory SET manifest_invalid=?4 WHERE account_id=?1 AND chat_id=?2 AND manifest_message_id=?3",params![account,chat,manifest,invalid])?;
        Ok(())
    }

    pub fn mark_vault_health_scan(
        &mut self,
        account: i64,
        chat: i64,
        manifest: i64,
        run: i64,
    ) -> StorageResult<()> {
        self.connection.execute("UPDATE vault_inventory SET health_scan_run=?4 WHERE account_id=?1 AND chat_id=?2 AND manifest_message_id=?3", params![account,chat,manifest,run])?;
        Ok(())
    }

    pub fn vault_inventory_unchecked_page(
        &self,
        account: i64,
        chat: i64,
        before: i64,
        run: i64,
    ) -> StorageResult<Option<VaultInventoryRecord>> {
        let id: Option<i64> = self.connection.query_row("SELECT manifest_message_id FROM vault_inventory WHERE account_id=?1 AND chat_id=?2 AND manifest_message_id<?3 AND health_scan_run<>?4 ORDER BY manifest_message_id DESC LIMIT 1", params![account,chat,before,run], |row| row.get(0)).optional()?;
        match id {
            Some(id) => Ok(self
                .vault_inventory_page(account, chat, id.saturating_add(1), 1)?
                .pop()),
            None => Ok(None),
        }
    }

    pub fn save_vault_message_health(
        &mut self,
        account: i64,
        chat: i64,
        messages: &[(i64, bool)],
        observed_at: i64,
    ) -> StorageResult<()> {
        if account <= 0
            || chat <= 0
            || messages.len() > 100
            || messages.iter().any(|(id, _)| *id <= 0)
        {
            return Err(StorageError::InvalidInput {
                field: "vault_health",
                reason: InputReason::OutOfRange,
            });
        }
        let tx = self.connection.transaction()?;
        for (id, present) in messages {
            tx.execute("INSERT INTO vault_message_health VALUES(?1,?2,?3,?4,?5) ON CONFLICT(account_id,chat_id,message_id) DO UPDATE SET present=excluded.present,observed_at=excluded.observed_at", params![account,chat,id,present,observed_at])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Indexed point probes against durable remote observations. Absence from an
    /// incomplete index is unknown; only explicit deletion evidence means missing.
    pub fn vault_message_health(
        &self,
        account: i64,
        chat: i64,
        messages: &[i64],
    ) -> StorageResult<Vec<VaultFileHealth>> {
        if messages.len() > 256 {
            return Err(StorageError::InvalidInput {
                field: "vault_health.messages",
                reason: InputReason::OutOfRange,
            });
        }
        let mut deleted = self.connection.prepare("SELECT 1 FROM channel_sync_tombstones WHERE account_id=?1 AND chat_id=?2 AND message_id=?3")?;
        let mut present = self.connection.prepare("SELECT f.remote_state FROM remote_objects ro JOIN logical_files f ON f.id=ro.logical_file_id WHERE ro.account_id=?1 AND ro.chat_id=?2 AND ro.message_id=?3")?;
        let mut checked = self.connection.prepare("SELECT present FROM vault_message_health WHERE account_id=?1 AND chat_id=?2 AND message_id=?3")?;
        messages
            .iter()
            .map(|id| {
                if deleted
                    .query_row(params![account, chat, id], |_| Ok(()))
                    .optional()?
                    .is_some()
                {
                    return Ok(VaultFileHealth::MissingParts);
                }
                if let Some(exists) = checked
                    .query_row(params![account, chat, id], |row| row.get::<_, bool>(0))
                    .optional()?
                {
                    return Ok(if exists {
                        VaultFileHealth::Present
                    } else {
                        VaultFileHealth::MissingParts
                    });
                }
                Ok(
                    match present
                        .query_row(params![account, chat, id], |row| row.get::<_, String>(0))
                        .optional()?
                        .as_deref()
                    {
                        Some("uploaded") => VaultFileHealth::Present,
                        Some("remote_missing") => VaultFileHealth::MissingParts,
                        _ => VaultFileHealth::Unchecked,
                    },
                )
            })
            .collect()
    }
}
