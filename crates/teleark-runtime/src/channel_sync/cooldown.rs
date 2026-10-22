//! Independent v1 account-scoped cooldown codec. Older installations have no
//! record. Unknown versions are preserved and reported, never overwritten.
use super::*;
use teleark_storage::{Database, SettingRecord};

fn key(account: i64) -> String {
    format!("channel-sync-cooldown.{account}")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SavedCooldown {
    pub until_unix_ms: i64,
    pub interval_ms: u64,
    pub saved_at_unix_ms: i64,
}

pub(crate) fn load(
    database: &Database,
    account: i64,
) -> Result<Option<SavedCooldown>, ApplicationError> {
    let Some(record) = database
        .setting(&key(account))
        .map_err(crate::map_storage_error)?
    else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(&record.value)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    if value["version"].as_u64() != Some(1) {
        return Err(ApplicationError::new(ApplicationErrorKind::Persistence));
    }
    let invalid = || ApplicationError::new(ApplicationErrorKind::Persistence);
    let until_unix_ms = value["until_unix_ms"].as_i64().ok_or_else(invalid)?;
    // A legacy record already proves that this account received a server wait.
    // Additive v1 readers restore a conservative interval without changing its deadline.
    let interval_ms = match value.get("interval_ms") {
        None => 2_000,
        Some(value) => value
            .as_u64()
            .filter(|ms| (1_000..=30_000).contains(ms))
            .ok_or_else(invalid)?,
    };
    Ok(Some(SavedCooldown {
        until_unix_ms,
        interval_ms,
        saved_at_unix_ms: record.updated_at_unix_ms,
    }))
}

pub(crate) fn save(
    database: &mut Database,
    account: i64,
    until: i64,
    interval_ms: u64,
) -> Result<(), ApplicationError> {
    if !(1_000..=30_000).contains(&interval_ms) {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    let until = load(database, account)?.map_or(until, |old| old.until_unix_ms.max(until));
    database
        .set_setting(&SettingRecord {
            key: key(account),
            value: serde_json::json!({"version": 1, "until_unix_ms": until, "interval_ms": interval_ms}).to_string(),
            updated_at_unix_ms: crate::system_time_unix_ms(std::time::SystemTime::now())
                .unwrap_or(0),
        })
        .map_err(crate::map_storage_error)
}

impl DesktopLibrary {
    pub(super) fn sync_cooldown(
        &self,
        account: i64,
    ) -> Result<Option<SavedCooldown>, ApplicationError> {
        self.worker
            .request("sync_cooldown", |reply| StorageRequest::SyncCooldown {
                account,
                reply,
            })
    }
    pub(super) fn save_sync_cooldown(
        &self,
        account: i64,
        until: i64,
        interval_ms: u64,
    ) -> Result<(), ApplicationError> {
        self.worker.request("save_sync_cooldown", |reply| {
            StorageRequest::SaveSyncCooldown {
                account,
                until,
                interval_ms,
                reply,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_v1_automatically_adopts_safe_spacing_and_optional_fields_stay_compatible() {
        let mut db = Database::open_in_memory().expect("database");
        let original = r#"{"version":1,"until_unix_ms":120000}"#;
        db.set_setting(&SettingRecord {
            key: key(7),
            value: original.into(),
            updated_at_unix_ms: 1,
        })
        .expect("legacy v1");
        assert_eq!(
            load(&db, 7).expect("legacy reader"),
            Some(SavedCooldown {
                until_unix_ms: 120_000,
                interval_ms: 2_000,
                saved_at_unix_ms: 1,
            })
        );
        assert!(save(&mut db, 7, 30_000, 0).is_err());
        assert_eq!(
            db.setting(&key(7))
                .expect("retained")
                .expect("record")
                .value,
            original
        );
        save(&mut db, 7, 30_000, 4_000).expect("automatic additive upgrade");
        let saved = load(&db, 7).expect("new reader").expect("record");
        assert_eq!((saved.until_unix_ms, saved.interval_ms), (120_000, 4_000));
        assert!(
            saved.saved_at_unix_ms > 1,
            "retain the setting's actual update timestamp for restart recovery"
        );
        let raw: serde_json::Value =
            serde_json::from_str(&db.setting(&key(7)).expect("row").expect("record").value)
                .expect("json");
        assert_eq!(raw["version"].as_u64(), Some(1));
        assert_eq!(
            raw["until_unix_ms"].as_i64(),
            Some(120_000),
            "older readers retain the exact deadline meaning"
        );
        save(&mut db, 7, 120_000, 3_000).expect("persist healthy recovery");
        assert_eq!(
            load(&db, 7)
                .expect("healthy interval")
                .expect("record")
                .interval_ms,
            3_000
        );
    }

    #[test]
    fn cooldown_is_account_scoped_monotonic_and_survives_restart() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("cooldown.sqlite3");
        {
            let mut db = Database::open(&path).expect("database");
            assert_eq!(load(&db, 7).expect("old installation"), None);
            save(&mut db, 7, 120_000, 4_000).expect("cooldown");
            save(&mut db, 7, 30_000, 3_000).expect("shorter wait");
            assert_eq!(load(&db, 8).expect("other account"), None);
        }
        let mut db = Database::open(&path).expect("restart");
        let saved = load(&db, 7).expect("saved wait").expect("record");
        assert_eq!((saved.until_unix_ms, saved.interval_ms), (120_000, 3_000));
        assert!(saved.saved_at_unix_ms > 0);
        db.set_setting(&SettingRecord {
            key: key(7),
            value: r#"{"version":2,"until_unix_ms":999999}"#.into(),
            updated_at_unix_ms: 1,
        })
        .expect("future record");
        assert!(load(&db, 7).is_err());
        assert!(save(&mut db, 7, 200_000, 8_000).is_err());
        assert!(
            db.setting(&key(7))
                .expect("intact")
                .expect("record")
                .value
                .contains("999999")
        );
    }
}
