//! Independent v1 account-scoped cooldown codec. Older installations have no
//! record. Unknown versions are preserved and reported, never overwritten.
use super::*;
use teleark_storage::{Database, SettingRecord};

fn key(account: i64) -> String {
    format!("channel-sync-cooldown.{account}")
}

pub(crate) fn load(database: &Database, account: i64) -> Result<Option<i64>, ApplicationError> {
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
    value["until_unix_ms"]
        .as_i64()
        .map(Some)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))
}

pub(crate) fn save(
    database: &mut Database,
    account: i64,
    until: i64,
) -> Result<(), ApplicationError> {
    let until = load(database, account)?.map_or(until, |old| old.max(until));
    database
        .set_setting(&SettingRecord {
            key: key(account),
            value: serde_json::json!({"version": 1, "until_unix_ms": until}).to_string(),
            updated_at_unix_ms: crate::system_time_unix_ms(std::time::SystemTime::now())
                .unwrap_or(0),
        })
        .map_err(crate::map_storage_error)
}

impl DesktopLibrary {
    pub(super) fn sync_cooldown(&self, account: i64) -> Result<Option<i64>, ApplicationError> {
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
    ) -> Result<(), ApplicationError> {
        self.worker.request("save_sync_cooldown", |reply| {
            StorageRequest::SaveSyncCooldown {
                account,
                until,
                reply,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_is_account_scoped_monotonic_and_survives_restart() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("cooldown.sqlite3");
        {
            let mut db = Database::open(&path).expect("database");
            assert_eq!(load(&db, 7).expect("old installation"), None);
            save(&mut db, 7, 120_000).expect("cooldown");
            save(&mut db, 7, 30_000).expect("shorter wait");
            assert_eq!(load(&db, 8).expect("other account"), None);
        }
        let mut db = Database::open(&path).expect("restart");
        assert_eq!(load(&db, 7).expect("saved wait"), Some(120_000));
        db.set_setting(&SettingRecord {
            key: key(7),
            value: r#"{"version":2,"until_unix_ms":999999}"#.into(),
            updated_at_unix_ms: 1,
        })
        .expect("future record");
        assert!(load(&db, 7).is_err());
        assert!(save(&mut db, 7, 200_000).is_err());
        assert!(
            db.setting(&key(7))
                .expect("intact")
                .expect("record")
                .value
                .contains("999999")
        );
    }
}
