//! Immutable manifest reservation, ciphertext and verified remote receipt.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultManifestOutbox {
    pub codec_version: u32,
    pub commitment: [u8; 32],
    pub random_id: i64,
    pub envelope: Option<Vec<u8>>,
    pub message_id: Option<i64>,
}

impl Database {
    pub fn vault_manifest_outbox(
        &self,
        account: i64,
        task: u64,
    ) -> StorageResult<Option<VaultManifestOutbox>> {
        Ok(self.connection.query_row(
            "SELECT codec_version,commitment,random_id,envelope,message_id FROM vault_manifest_outbox WHERE account_id=?1 AND task_id=?2",
            params![account, unsigned_to_sql("vault_job.id", task)?],
            |row| Ok(VaultManifestOutbox {
                codec_version: row.get(0)?, commitment: row.get(1)?, random_id: row.get(2)?,
                envelope: row.get(3)?, message_id: row.get(4)?,
            }),
        ).optional()?)
    }

    /// Commits the exact pre-encryption content commitment and deduplication ID.
    /// Repeating a reservation cannot change either, including after restart.
    pub fn reserve_vault_manifest(
        &mut self,
        lease: VaultJobLease,
        record: &VaultManifestOutbox,
    ) -> StorageResult<bool> {
        if record.codec_version != 1
            || record.random_id == 0
            || record.envelope.is_some()
            || record.message_id.is_some()
        {
            return Err(invalid("vault_manifest.reservation"));
        }
        self.durable_vault_write(|tx| {
            if !active_lease(tx, lease, false)? { return Ok(false); }
            let id = unsigned_to_sql("vault_job.id", lease.id)?;
            tx.execute("INSERT INTO vault_manifest_outbox(account_id,task_id,codec_version,commitment,random_id) VALUES(?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING", params![lease.account_id,id,record.codec_version,record.commitment,record.random_id])?;
            let same: bool = tx.query_row("SELECT codec_version=?3 AND commitment=?4 AND random_id=?5 FROM vault_manifest_outbox WHERE account_id=?1 AND task_id=?2", params![lease.account_id,id,record.codec_version,record.commitment,record.random_id], |row| row.get(0))?;
            if !same { return Err(invalid("vault_manifest.identity")); }
            Ok(true)
        })
    }

    /// Ciphertext is immutable and durable before the first remote request.
    pub fn save_vault_manifest_envelope(
        &mut self,
        lease: VaultJobLease,
        envelope: &[u8],
    ) -> StorageResult<bool> {
        if envelope.is_empty() || envelope.len() > 67_239_936 {
            return Err(invalid("vault_manifest.envelope"));
        }
        self.durable_vault_write(|tx| {
            if !active_lease(tx, lease, false)? { return Ok(false); }
            Ok(tx.execute("UPDATE vault_manifest_outbox SET envelope=?3 WHERE account_id=?1 AND task_id=?2 AND codec_version=1 AND (envelope IS NULL OR envelope=?3)", params![lease.account_id,unsigned_to_sql("vault_job.id",lease.id)?,envelope])? == 1)
        })
    }

    /// A verified late success is retained while stopping, before acknowledgment.
    pub fn confirm_vault_manifest(
        &mut self,
        lease: VaultJobLease,
        message_id: i64,
    ) -> StorageResult<bool> {
        if message_id <= 0 {
            return Err(invalid("vault_manifest.message_id"));
        }
        self.durable_vault_write(|tx| {
            if !active_lease(tx, lease, true)? { return Ok(false); }
            Ok(tx.execute("UPDATE vault_manifest_outbox SET message_id=?3 WHERE account_id=?1 AND task_id=?2 AND codec_version=1 AND envelope IS NOT NULL AND (message_id IS NULL OR message_id=?3)", params![lease.account_id,unsigned_to_sql("vault_job.id",lease.id)?,message_id])? == 1)
        })
    }
}
