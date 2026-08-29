use rusqlite::params;
use teleark_core::{CollectionId, LogicalFileId};

use super::{Database, corrupt, id_from_sql, id_to_sql, transaction, validate_nonempty};
use crate::error::{EntityKind, InputReason, InvariantViolation};
use crate::model::{CollectionKind, CollectionRecord, SettingRecord};
use crate::{StorageError, StorageResult};

const MAX_SETTING_VALUE_BYTES: usize = 1_048_576;

fn upsert_setting(connection: &rusqlite::Connection, setting: &SettingRecord) -> StorageResult<()> {
    connection.execute(
        r#"
INSERT INTO settings (key, value, updated_at_unix_ms)
VALUES (?1, ?2, ?3)
ON CONFLICT(key) DO UPDATE SET
    value = excluded.value,
    updated_at_unix_ms = excluded.updated_at_unix_ms
"#,
        params![setting.key, setting.value, setting.updated_at_unix_ms],
    )?;
    Ok(())
}

impl Database {
    pub fn set_setting(&mut self, setting: &SettingRecord) -> StorageResult<()> {
        validate_setting(setting)?;
        upsert_setting(&self.connection, setting)?;
        Ok(())
    }

    /// Saves a group of settings atomically after validating the entire batch.
    pub fn set_settings(&mut self, settings: &[SettingRecord]) -> StorageResult<()> {
        for setting in settings {
            validate_setting(setting)?;
        }
        let transaction = transaction(&mut self.connection)?;
        for setting in settings {
            upsert_setting(&transaction, setting)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Deletes a group of settings atomically.
    pub fn delete_settings(&mut self, keys: &[&str]) -> StorageResult<usize> {
        for key in keys {
            validate_setting_key(key)?;
        }
        let transaction = transaction(&mut self.connection)?;
        let mut deleted = 0_usize;
        for key in keys {
            deleted += transaction.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        }
        transaction.commit()?;
        Ok(deleted)
    }

    pub fn setting(&self, key: &str) -> StorageResult<Option<SettingRecord>> {
        validate_setting_key(key)?;
        let mut statement = self
            .connection
            .prepare("SELECT key, value, updated_at_unix_ms FROM settings WHERE key = ?1")?;
        let mut rows = statement.query([key])?;
        match rows.next()? {
            None => Ok(None),
            Some(row) => Ok(Some(SettingRecord {
                key: row.get(0)?,
                value: row.get(1)?,
                updated_at_unix_ms: row.get(2)?,
            })),
        }
    }

    pub fn settings(&self) -> StorageResult<Vec<SettingRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT key, value, updated_at_unix_ms FROM settings ORDER BY key COLLATE BINARY",
        )?;
        let mut rows = statement.query([])?;
        let mut settings = Vec::new();
        while let Some(row) = rows.next()? {
            settings.push(SettingRecord {
                key: row.get(0)?,
                value: row.get(1)?,
                updated_at_unix_ms: row.get(2)?,
            });
        }
        Ok(settings)
    }

    pub fn delete_setting(&mut self, key: &str) -> StorageResult<bool> {
        validate_setting_key(key)?;
        Ok(self
            .connection
            .execute("DELETE FROM settings WHERE key = ?1", [key])?
            != 0)
    }

    pub fn upsert_collection(&mut self, collection: &CollectionRecord) -> StorageResult<()> {
        validate_collection(collection)?;
        let id = id_to_sql("collection.id", collection.id.get())?;
        let (kind, rule_version, rule_payload) = collection_parts(collection);
        self.connection.execute(
            r#"
INSERT INTO collections (
    id, name, kind, rule_version, rule_payload, created_at_unix_ms, updated_at_unix_ms
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
ON CONFLICT(id) DO UPDATE SET
    name = excluded.name,
    kind = excluded.kind,
    rule_version = excluded.rule_version,
    rule_payload = excluded.rule_payload,
    updated_at_unix_ms = excluded.updated_at_unix_ms
"#,
            params![
                id,
                collection.name,
                kind,
                rule_version,
                rule_payload,
                collection.created_at_unix_ms,
                collection.updated_at_unix_ms,
            ],
        )?;
        Ok(())
    }

    pub fn collection(&self, id: CollectionId) -> StorageResult<Option<CollectionRecord>> {
        let id = id_to_sql("collection.id", id.get())?;
        let mut statement = self.connection.prepare(
            r#"
SELECT id, name, kind, rule_version, rule_payload, created_at_unix_ms, updated_at_unix_ms
FROM collections WHERE id = ?1
"#,
        )?;
        let mut rows = statement.query([id])?;
        rows.next()?.map(row_to_collection).transpose()
    }

    pub fn collections(&self) -> StorageResult<Vec<CollectionRecord>> {
        let mut statement = self.connection.prepare(
            r#"
SELECT id, name, kind, rule_version, rule_payload, created_at_unix_ms, updated_at_unix_ms
FROM collections ORDER BY name COLLATE NOCASE, id
"#,
        )?;
        let mut rows = statement.query([])?;
        let mut collections = Vec::new();
        while let Some(row) = rows.next()? {
            collections.push(row_to_collection(row)?);
        }
        Ok(collections)
    }

    pub fn delete_collection(&mut self, id: CollectionId) -> StorageResult<bool> {
        let id = id_to_sql("collection.id", id.get())?;
        Ok(self
            .connection
            .execute("DELETE FROM collections WHERE id = ?1", [id])?
            != 0)
    }

    pub fn add_collection_file(
        &mut self,
        collection_id: CollectionId,
        logical_file_id: LogicalFileId,
        added_at_unix_ms: i64,
    ) -> StorageResult<bool> {
        let collection_id = id_to_sql("collection.id", collection_id.get())?;
        let logical_file_id = id_to_sql("logical_file.id", logical_file_id.get())?;
        let transaction = transaction(&mut self.connection)?;
        ensure_manual_collection(&transaction, collection_id)?;
        let changed = transaction.execute(
            r#"
INSERT INTO collection_items (collection_id, logical_file_id, added_at_unix_ms)
VALUES (?1, ?2, ?3)
ON CONFLICT(collection_id, logical_file_id) DO NOTHING
"#,
            params![collection_id, logical_file_id, added_at_unix_ms],
        )?;
        transaction.commit()?;
        Ok(changed != 0)
    }

    pub fn remove_collection_file(
        &mut self,
        collection_id: CollectionId,
        logical_file_id: LogicalFileId,
    ) -> StorageResult<bool> {
        let collection_id = id_to_sql("collection.id", collection_id.get())?;
        let logical_file_id = id_to_sql("logical_file.id", logical_file_id.get())?;
        let transaction = transaction(&mut self.connection)?;
        ensure_manual_collection(&transaction, collection_id)?;
        let changed = transaction.execute(
            "DELETE FROM collection_items WHERE collection_id = ?1 AND logical_file_id = ?2",
            params![collection_id, logical_file_id],
        )?;
        transaction.commit()?;
        Ok(changed != 0)
    }

    pub fn collection_file_ids(
        &self,
        collection_id: CollectionId,
    ) -> StorageResult<Vec<LogicalFileId>> {
        let collection_id = id_to_sql("collection.id", collection_id.get())?;
        let mut statement = self.connection.prepare(
            "SELECT logical_file_id FROM collection_items WHERE collection_id = ?1 ORDER BY logical_file_id",
        )?;
        let mut rows = statement.query([collection_id])?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next()? {
            let value: i64 = row.get(0)?;
            ids.push(LogicalFileId::new(id_from_sql(
                "collection_items",
                "logical_file_id",
                value,
            )?));
        }
        Ok(ids)
    }
}

fn validate_setting(setting: &SettingRecord) -> StorageResult<()> {
    validate_setting_key(&setting.key)?;
    if setting.value.len() > MAX_SETTING_VALUE_BYTES {
        return Err(StorageError::InvalidInput {
            field: "setting.value",
            reason: InputReason::TooLong,
        });
    }
    Ok(())
}

fn validate_setting_key(key: &str) -> StorageResult<()> {
    if key.is_empty() {
        return Err(StorageError::InvalidInput {
            field: "setting.key",
            reason: InputReason::Empty,
        });
    }
    if key.len() > 128 {
        return Err(StorageError::InvalidInput {
            field: "setting.key",
            reason: InputReason::TooLong,
        });
    }
    Ok(())
}

fn validate_collection(collection: &CollectionRecord) -> StorageResult<()> {
    validate_nonempty("collection.name", &collection.name)?;
    id_to_sql("collection.id", collection.id.get())?;
    match collection.kind {
        CollectionKind::Manual
            if collection.rule_version.is_some() || collection.rule_payload.is_some() =>
        {
            Err(StorageError::InvalidInput {
                field: "collection.rule",
                reason: InputReason::InvalidCombination,
            })
        }
        CollectionKind::Smart
            if collection.rule_version.is_none_or(|version| version == 0)
                || collection.rule_payload.as_deref().is_none_or(str::is_empty) =>
        {
            Err(StorageError::InvalidInput {
                field: "collection.rule",
                reason: InputReason::InvalidCombination,
            })
        }
        _ => Ok(()),
    }
}

fn collection_parts(collection: &CollectionRecord) -> (&'static str, Option<i64>, Option<&str>) {
    match collection.kind {
        CollectionKind::Manual => ("manual", None, None),
        CollectionKind::Smart => (
            "smart",
            collection.rule_version.map(i64::from),
            collection.rule_payload.as_deref(),
        ),
    }
}

fn row_to_collection(row: &rusqlite::Row<'_>) -> StorageResult<CollectionRecord> {
    let raw_id: i64 = row.get(0)?;
    let raw_kind: String = row.get(2)?;
    let raw_rule_version: Option<i64> = row.get(3)?;
    Ok(CollectionRecord {
        id: CollectionId::new(id_from_sql("collections", "id", raw_id)?),
        name: row.get(1)?,
        kind: match raw_kind.as_str() {
            "manual" => CollectionKind::Manual,
            "smart" => CollectionKind::Smart,
            value => return Err(corrupt("collections", "kind", value)),
        },
        rule_version: raw_rule_version
            .map(|value| {
                u32::try_from(value).map_err(|_| corrupt("collections", "rule_version", value))
            })
            .transpose()?,
        rule_payload: row.get(4)?,
        created_at_unix_ms: row.get(5)?,
        updated_at_unix_ms: row.get(6)?,
    })
}

fn ensure_manual_collection(
    connection: &rusqlite::Connection,
    collection_id: i64,
) -> StorageResult<()> {
    let kind = connection
        .query_row(
            "SELECT kind FROM collections WHERE id = ?1",
            [collection_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match kind.as_deref() {
        None => Err(StorageError::NotFound {
            entity: EntityKind::Collection,
            id: collection_id,
        }),
        Some("manual") => Ok(()),
        Some("smart") => Err(StorageError::Invariant(
            InvariantViolation::SmartCollectionMembership,
        )),
        Some(value) => Err(corrupt("collections", "kind", value)),
    }
}

use rusqlite::OptionalExtension;
