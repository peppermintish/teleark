//! Credential payloads are opaque to Storage. Runtime authenticates Vault bundles.
use super::*;
use rusqlite::OptionalExtension;
use zeroize::Zeroizing;

pub const CREDENTIAL_ITEM_LIMIT: usize = 4096;
pub const CREDENTIAL_TOTAL_BYTES_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CredentialBackend {
    pub keychain_enabled: bool,
    pub revision: i64,
}

/// Deliberately has no Debug implementation: a payload can contain private keys.
#[derive(Clone, Eq, PartialEq)]
pub struct CredentialItem {
    pub namespace: String,
    pub identity: String,
    pub keychain_account: String,
    pub payload: Option<Zeroizing<Vec<u8>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialCleanup {
    pub namespace: String,
    pub keychain_account: String,
}

impl Database {
    /// A copied macOS database cannot bring operating-system secrets with it.
    /// Preserve every reference, mark the effective backend local, and leave
    /// unavailable payloads explicit so a later credential/recovery import repairs them.
    pub fn credential_backend_for_platform(
        &mut self,
        supported: bool,
    ) -> StorageResult<CredentialBackend> {
        let state = self.credential_backend(supported)?;
        if state.keychain_enabled && !supported {
            self.connection.execute("UPDATE credential_backend SET keychain_enabled=0,revision=revision+1 WHERE singleton_id=1 AND keychain_enabled=1", [])?;
            return self.credential_backend(false);
        }
        Ok(state)
    }

    pub fn credential_backend(
        &mut self,
        default_enabled: bool,
    ) -> StorageResult<CredentialBackend> {
        self.connection.execute(
            "INSERT OR IGNORE INTO credential_backend VALUES (1,?1,0)",
            [default_enabled],
        )?;
        Ok(self.connection.query_row(
            "SELECT keychain_enabled,revision FROM credential_backend WHERE singleton_id=1",
            [],
            |row| {
                Ok(CredentialBackend {
                    keychain_enabled: row.get(0)?,
                    revision: row.get(1)?,
                })
            },
        )?)
    }

    pub fn credential_item(
        &self,
        namespace: &str,
        identity: &str,
    ) -> StorageResult<Option<CredentialItem>> {
        Ok(self.connection.query_row(
            "SELECT keychain_account,payload FROM credential_items WHERE namespace=?1 AND identity=?2",
            params![namespace, identity],
            |row| Ok(CredentialItem { namespace: namespace.into(), identity: identity.into(), keychain_account: row.get(0)?, payload: row.get::<_,Option<Vec<u8>>>(1)?.map(Zeroizing::new) }),
        ).optional()?)
    }

    /// Bound both retained key count and byte size before materializing migration input.
    pub fn credential_items(&self) -> StorageResult<Vec<CredentialItem>> {
        let (count, bytes): (i64, i64) = self.connection.query_row(
            "SELECT count(*),coalesce(sum(length(payload)),0) FROM credential_items",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if count > CREDENTIAL_ITEM_LIMIT as i64 || bytes > CREDENTIAL_TOTAL_BYTES_LIMIT as i64 {
            return Err(StorageError::InvalidInput {
                field: "credential_capacity",
                reason: InputReason::OutOfRange,
            });
        }
        let mut query = self.connection.prepare("SELECT namespace,identity,keychain_account,payload FROM credential_items ORDER BY namespace,identity")?;
        Ok(query
            .query_map([], |row| {
                Ok(CredentialItem {
                    namespace: row.get(0)?,
                    identity: row.get(1)?,
                    keychain_account: row.get(2)?,
                    payload: row.get::<_, Option<Vec<u8>>>(3)?.map(Zeroizing::new),
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn save_credential_item(
        &mut self,
        expected: CredentialBackend,
        namespace: &str,
        identity: &str,
        item: Option<&CredentialItem>,
    ) -> StorageResult<bool> {
        self.connection.pragma_update(None, "secure_delete", true)?;
        let tx = self.connection.transaction()?;
        if tx.execute("UPDATE credential_backend SET revision=revision+1 WHERE singleton_id=1 AND revision=?1 AND keychain_enabled=?2", params![expected.revision,expected.keychain_enabled])? != 1 {
            return Ok(false);
        }
        if expected.keychain_enabled && namespace != "app.teleark.encryption-key.v1" {
            let count: i64 =
                tx.query_row("SELECT count(*) FROM credential_cleanup", [], |row| {
                    row.get(0)
                })?;
            if count >= CREDENTIAL_ITEM_LIMIT as i64 {
                return Err(StorageError::InvalidInput {
                    field: "credential_capacity",
                    reason: InputReason::OutOfRange,
                });
            }
            tx.execute("INSERT OR IGNORE INTO credential_cleanup SELECT namespace,keychain_account FROM credential_items WHERE namespace=?1 AND identity=?2 AND keychain_account != coalesce(?3,'')", params![namespace,identity,item.map(|item| item.keychain_account.as_str())])?;
        }
        if let Some(item) = item {
            if item.namespace != namespace
                || item.identity != identity
                || expected.keychain_enabled == item.payload.is_some()
            {
                return Err(StorageError::InvalidInput {
                    field: "credential_backend",
                    reason: InputReason::InvalidCombination,
                });
            }
            let existing: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM credential_items WHERE namespace=?1 AND identity=?2)",
                params![namespace, identity],
                |row| row.get(0),
            )?;
            if !existing {
                let count: i64 =
                    tx.query_row("SELECT count(*) FROM credential_items", [], |row| {
                        row.get(0)
                    })?;
                if count >= CREDENTIAL_ITEM_LIMIT as i64 {
                    return Err(StorageError::InvalidInput {
                        field: "credential_capacity",
                        reason: InputReason::OutOfRange,
                    });
                }
            }
            tx.execute("INSERT INTO credential_items VALUES (?1,?2,1,?3,?4) ON CONFLICT(namespace,identity) DO UPDATE SET keychain_account=excluded.keychain_account,payload=excluded.payload", params![namespace,identity,item.keychain_account,item.payload.as_deref().map(Vec::as_slice)])?;
        } else {
            tx.execute(
                "DELETE FROM credential_items WHERE namespace=?1 AND identity=?2",
                params![namespace, identity],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// One CAS transaction publishes every verified copy and the backend preference.
    pub fn replace_credential_backend(
        &mut self,
        expected: CredentialBackend,
        enabled: bool,
        items: &[CredentialItem],
    ) -> StorageResult<bool> {
        self.connection.pragma_update(None, "secure_delete", true)?;
        let tx = self.connection.transaction()?;
        if tx.execute("UPDATE credential_backend SET revision=revision+1,keychain_enabled=?3 WHERE singleton_id=1 AND revision=?1 AND keychain_enabled=?2", params![expected.revision,expected.keychain_enabled,enabled])? != 1 {
            return Ok(false);
        }
        if expected.keychain_enabled && !enabled {
            tx.execute("INSERT OR IGNORE INTO credential_cleanup SELECT namespace,keychain_account FROM credential_items WHERE namespace != 'app.teleark.encryption-key.v1'", [])?;
        }
        tx.execute("DELETE FROM credential_items", [])?;
        for item in items {
            if enabled == item.payload.is_some() {
                return Err(StorageError::InvalidInput {
                    field: "credential_backend",
                    reason: InputReason::InvalidCombination,
                });
            }
            tx.execute(
                "INSERT INTO credential_items VALUES (?1,?2,1,?3,?4)",
                params![
                    item.namespace,
                    item.identity,
                    item.keychain_account,
                    item.payload.as_deref().map(Vec::as_slice)
                ],
            )?;
        }
        tx.commit()?;
        // SQLite may retain pages in backups or readers' old snapshots. Do not
        // promise secure erasure; checkpoint only after the durable new selection.
        if enabled {
            let _ = self
                .connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
        }
        Ok(true)
    }

    pub fn credential_cleanup(&self) -> StorageResult<Vec<CredentialCleanup>> {
        let mut query = self.connection.prepare("SELECT namespace,keychain_account FROM credential_cleanup ORDER BY namespace,keychain_account LIMIT 32")?;
        Ok(query
            .query_map([], |row| {
                Ok(CredentialCleanup {
                    namespace: row.get(0)?,
                    keychain_account: row.get(1)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn credential_cleanup_count(&self) -> StorageResult<u64> {
        let count: i64 =
            self.connection
                .query_row("SELECT count(*) FROM credential_cleanup", [], |row| {
                    row.get(0)
                })?;
        u64::try_from(count).map_err(|_| StorageError::InvalidInput {
            field: "credential_cleanup",
            reason: InputReason::OutOfRange,
        })
    }

    pub fn unavailable_credential_count(&self) -> StorageResult<u64> {
        let count: i64 = self.connection.query_row("SELECT count(*) FROM credential_items WHERE payload IS NULL AND EXISTS (SELECT 1 FROM credential_backend WHERE singleton_id=1 AND keychain_enabled=0)", [], |row| row.get(0))?;
        u64::try_from(count).map_err(|_| StorageError::InvalidInput {
            field: "credential_count",
            reason: InputReason::OutOfRange,
        })
    }

    pub fn complete_credential_cleanup(&mut self, item: &CredentialCleanup) -> StorageResult<()> {
        self.connection.execute(
            "DELETE FROM credential_cleanup WHERE namespace=?1 AND keychain_account=?2",
            params![item.namespace, item.keychain_account],
        )?;
        Ok(())
    }

    pub fn enqueue_credential_cleanup(&mut self, item: &CredentialCleanup) -> StorageResult<()> {
        // Failed publication can be ambiguous to its caller. Never retire a
        // reference that SQLite still considers authoritative.
        self.connection.execute("INSERT OR IGNORE INTO credential_cleanup SELECT ?1,?2 WHERE ?1 != 'app.teleark.encryption-key.v1' AND NOT EXISTS (SELECT 1 FROM credential_items WHERE namespace=?1 AND keychain_account=?2)",params![item.namespace,item.keychain_account])?;
        Ok(())
    }

    pub fn vault_key_epochs_for_credentials(&self) -> StorageResult<Vec<VaultMetadataRecord>> {
        let mut query = self
            .connection
            .prepare("SELECT vault_id FROM vault_key_epochs ORDER BY vault_id LIMIT ?1")?;
        let ids: Vec<Vec<u8>> = query
            .query_map([CREDENTIAL_ITEM_LIMIT as i64 + 1], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        if ids.len() > CREDENTIAL_ITEM_LIMIT {
            return Err(StorageError::InvalidInput {
                field: "credential_capacity",
                reason: InputReason::OutOfRange,
            });
        }
        ids.into_iter()
            .map(|id| {
                let id = id.try_into().map_err(|_| StorageError::InvalidInput {
                    field: "vault_id",
                    reason: InputReason::InvalidCombination,
                })?;
                self.vault_key_epoch(id)?.ok_or(StorageError::InvalidInput {
                    field: "vault_id",
                    reason: InputReason::InvalidCombination,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(local: bool) -> CredentialItem {
        CredentialItem {
            namespace: "synthetic.service.v1".into(),
            identity: "synthetic".into(),
            keychain_account: "opaque-reference".into(),
            payload: local.then(|| Zeroizing::new(b"synthetic credential".to_vec())),
        }
    }

    #[test]
    fn backend_switch_is_atomic_and_rejects_stale_writers() -> StorageResult<()> {
        let mut db = Database::open_in_memory()?;
        let original = db.credential_backend(false)?;
        let local = item(true);
        assert!(db.save_credential_item(
            original,
            &local.namespace,
            &local.identity,
            Some(&local)
        )?);
        assert!(!db.replace_credential_backend(original, true, &[item(false)])?);
        assert!(!db.credential_backend(true)?.keychain_enabled);
        assert!(
            db.credential_item(&local.namespace, &local.identity)?
                .is_some_and(|stored| stored == local)
        );
        let revision = db.credential_backend(false)?;
        assert!(db.replace_credential_backend(revision, true, &[item(false)])?);
        assert!(db.credential_backend(false)?.keychain_enabled);
        assert!(
            db.credential_items()?
                .iter()
                .all(|item| item.payload.is_none())
        );
        assert!(!db.save_credential_item(
            revision,
            &local.namespace,
            &local.identity,
            Some(&local)
        )?);
        Ok(())
    }

    #[test]
    fn malformed_target_rolls_back_selection_and_preserves_source_bytes() -> StorageResult<()> {
        let mut db = Database::open_in_memory()?;
        let local = item(true);
        let initial = db.credential_backend(false)?;
        assert!(db.save_credential_item(
            initial,
            &local.namespace,
            &local.identity,
            Some(&local)
        )?);
        let expected = db.credential_backend(false)?;
        assert!(
            db.replace_credential_backend(expected, true, std::slice::from_ref(&local))
                .is_err()
        );
        assert_eq!(db.credential_backend(false)?, expected);
        assert!(
            db.credential_item(&local.namespace, &local.identity)?
                .is_some_and(|stored| stored == local)
        );
        Ok(())
    }

    #[test]
    fn schema_22_upgrades_without_reinterpreting_existing_records() -> StorageResult<()> {
        let connection = Connection::open_in_memory()?;
        for migration in migration::MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 22)
        {
            connection.execute_batch(migration.sql)?;
        }
        connection.pragma_update(None, "user_version", 22)?;
        connection.pragma_update(None, "application_id", migration::APPLICATION_ID)?;
        connection.execute(
            "INSERT INTO settings VALUES ('legacy.fixture','synthetic persisted value',1)",
            [],
        )?;
        let mut db = Database::initialize(connection, |_| {})?;
        assert_eq!(
            db.schema_version()?,
            crate::migration::LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            db.setting("legacy.fixture")?.map(|row| row.value),
            Some("synthetic persisted value".into())
        );
        assert!(db.credential_items()?.is_empty());
        assert!(db.credential_backend(true)?.keychain_enabled);
        Ok(())
    }

    #[test]
    fn admission_bounds_new_credentials_and_existing_values_remain_updatable() -> StorageResult<()>
    {
        let mut db = Database::open_in_memory()?;
        let backend = db.credential_backend(false)?;
        db.connection.execute("WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n<?1) INSERT INTO credential_items SELECT 'synthetic.service.v1',CAST(n AS TEXT),1,'synthetic-reference',x'00' FROM ids",[CREDENTIAL_ITEM_LIMIT as i64])?;
        let mut candidate = item(true);
        assert!(matches!(
            db.save_credential_item(
                backend,
                &candidate.namespace,
                &candidate.identity,
                Some(&candidate)
            ),
            Err(StorageError::InvalidInput {
                field: "credential_capacity",
                ..
            })
        ));
        assert_eq!(db.credential_backend(false)?, backend);
        candidate.identity = "1".into();
        assert!(db.save_credential_item(
            backend,
            &candidate.namespace,
            &candidate.identity,
            Some(&candidate)
        )?);
        assert_eq!(db.credential_items()?.len(), CREDENTIAL_ITEM_LIMIT);
        Ok(())
    }
}
