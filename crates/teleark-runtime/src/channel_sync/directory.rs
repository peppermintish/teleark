//! Version 1 restart cache. Pages and header are committed atomically; the
//! authoritative channel cursors and recoverable file data remain independent.
use super::*;
use serde_json::{Value, json};
use std::time::SystemTime;
use teleark_storage::{Database, SettingRecord};

const VERSION: u64 = 1;
const PAGE_SIZE: usize = 128;
const MAX_PAGES: usize = MAX_SOURCES.div_ceil(PAGE_SIZE);
const MAX_PAGE_BYTES: usize = 1_048_576;

fn key(account: i64) -> String {
    format!("channel-directory.{account}")
}
fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Persistence)
}

fn header(database: &Database, account: i64) -> Result<Option<usize>, ApplicationError> {
    let Some(record) = database
        .setting(&key(account))
        .map_err(crate::map_storage_error)?
    else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(&record.value).map_err(|_| invalid())?;
    if value["version"].as_u64() != Some(VERSION) {
        return Err(invalid());
    }
    let pages = value["pages"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n <= MAX_PAGES)
        .ok_or_else(invalid)?;
    Ok(Some(pages))
}

pub(crate) fn load(
    database: &Database,
    account: i64,
) -> Result<Vec<TelegramChatSummary>, ApplicationError> {
    let Some(pages) = header(database, account)? else {
        return database
            .cached_channel_sources(AccountId::new(account))
            .map_err(crate::map_storage_error)
            .map(|chats| {
                chats
                    .into_iter()
                    .map(|chat| TelegramChatSummary {
                        id: chat.id.get(),
                        name: chat.title,
                        username: chat.username,
                        kind: TelegramChatKind::Channel,
                        sync_pts: None,
                    })
                    .collect()
            });
    };
    let mut chats = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for page in 0..pages {
        let record = database
            .setting(&format!("{}.{page}", key(account)))
            .map_err(crate::map_storage_error)?
            .ok_or_else(invalid)?;
        if record.value.len() > MAX_PAGE_BYTES {
            return Err(invalid());
        }
        let entries: Value = serde_json::from_str(&record.value).map_err(|_| invalid())?;
        let entries = entries
            .as_array()
            .filter(|entries| entries.len() <= PAGE_SIZE)
            .ok_or_else(invalid)?;
        for entry in entries {
            if entry.as_array().is_none_or(|fields| fields.len() != 4) {
                return Err(invalid());
            }
            let id = entry[0]
                .as_i64()
                .filter(|id| seen.insert(*id))
                .ok_or_else(invalid)?;
            let kind = match entry[3].as_str() {
                Some("channel") => TelegramChatKind::Channel,
                Some("user") => TelegramChatKind::User,
                Some("group") => TelegramChatKind::Group,
                _ => return Err(invalid()),
            };
            chats.push(TelegramChatSummary {
                id,
                name: entry[1].as_str().ok_or_else(invalid)?.to_owned(),
                username: if entry[2].is_null() {
                    None
                } else {
                    Some(entry[2].as_str().ok_or_else(invalid)?.to_owned())
                },
                kind,
                // Cached PTS is never treated as fresh server coverage.
                sync_pts: None,
            });
            if chats.len() > MAX_SOURCES {
                return Err(invalid());
            }
        }
    }
    Ok(chats)
}

pub(crate) fn save(
    database: &mut Database,
    account: &TelegramAccount,
    chats: &[TelegramChatSummary],
) -> Result<(), ApplicationError> {
    // Reject unknown/newer cache formats intact, before writing any fields.
    header(database, account.id)?;
    if chats.len() > MAX_SOURCES {
        return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
    }
    let mut ids = std::collections::BTreeSet::new();
    if chats.iter().any(|chat| !ids.insert(chat.id)) {
        return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
    }
    let now = crate::system_time_unix_ms(SystemTime::now()).ok_or_else(invalid)?;
    let entries = chats
        .iter()
        .map(|chat| {
            let kind = match chat.kind {
                TelegramChatKind::Channel => "channel",
                TelegramChatKind::User => "user",
                TelegramChatKind::Group => "group",
                _ => return Err(invalid()),
            };
            Ok(json!([chat.id, chat.name, chat.username, kind]))
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    let mut settings = Vec::new();
    for (page, values) in entries.chunks(PAGE_SIZE).enumerate() {
        let value = serde_json::to_string(values).map_err(|_| invalid())?;
        if value.len() > MAX_PAGE_BYTES {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        settings.push(SettingRecord {
            key: format!("{}.{page}", key(account.id)),
            value,
            updated_at_unix_ms: now,
        });
    }
    settings.push(SettingRecord {
        key: key(account.id),
        value: json!({"version": VERSION, "pages": settings.len()}).to_string(),
        updated_at_unix_ms: now,
    });
    let stored_account = teleark_storage::AccountRecord {
        id: AccountId::new(account.id),
        display_name: account.display_name.clone(),
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
    };
    let stored_chats = chats
        .iter()
        .map(|chat| teleark_storage::ChatRecord {
            account_id: stored_account.id,
            id: ChatId::new(chat.id),
            title: if chat.name.trim().is_empty() {
                chat.id.to_string()
            } else {
                chat.name.clone()
            },
            username: chat.username.clone(),
            updated_at_unix_ms: now,
        })
        .collect::<Vec<_>>();
    crate::save_telegram_sources(database, &stored_account, &stored_chats)?;
    database
        .set_settings(&settings)
        .map_err(crate::map_storage_error)
}

impl DesktopLibrary {
    pub(super) fn cached_channel_directory(
        &self,
        account: i64,
    ) -> Result<Vec<TelegramChatSummary>, ApplicationError> {
        self.worker.request("cached_channel_directory", |reply| {
            StorageRequest::ChannelDirectory { account, reply }
        })
    }
    pub(super) fn save_channel_directory(
        &self,
        account: TelegramAccount,
        chats: Vec<TelegramChatSummary>,
        cancellation: TelegramScanCancellation,
    ) -> Result<(), ApplicationError> {
        self.worker.request("save_channel_directory", |reply| {
            StorageRequest::SaveChannelDirectory {
                account,
                chats,
                cancellation,
                reply,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_cache_preserves_names_departures_account_scope_and_newer_formats() {
        let mut database = Database::open_in_memory().expect("database");
        let account = TelegramAccount {
            id: 7,
            display_name: "Fixture".into(),
            username: None,
        };
        let chats = (0..300)
            .map(|id| TelegramChatSummary {
                id,
                name: format!("Source {id}"),
                username: None,
                kind: TelegramChatKind::Channel,
                sync_pts: Some(50),
            })
            .collect::<Vec<_>>();
        assert!(load(&database, 7).expect("old installation").is_empty());
        save(&mut database, &account, &chats).expect("save pages");
        let restored = load(&database, 7).expect("restore");
        assert_eq!(restored.len(), chats.len());
        assert_eq!(restored[299].name, chats[299].name);
        assert_eq!(restored[299].sync_pts, None);
        assert!(load(&database, 8).expect("other account").is_empty());
        save(&mut database, &account, &chats[..1]).expect("departure");
        assert_eq!(load(&database, 7).expect("updated membership").len(), 1);
        let mut oversized = chats[0].clone();
        oversized.name = "x".repeat(MAX_PAGE_BYTES + 1);
        assert!(save(&mut database, &account, &[oversized]).is_err());
        assert_eq!(
            load(&database, 7)
                .expect("failed replacement preserves prior cache")
                .len(),
            1
        );
        assert!(
            save(
                &mut database,
                &account,
                &[chats[0].clone(), chats[0].clone()]
            )
            .is_err()
        );
        let future = SettingRecord {
            key: key(7),
            value: r#"{"version":2,"pages":1}"#.into(),
            updated_at_unix_ms: 1,
        };
        database.set_setting(&future).expect("future fixture");
        assert!(load(&database, 7).is_err());
        assert!(save(&mut database, &account, &[]).is_err());
        assert_eq!(
            database
                .setting(&key(7))
                .expect("read")
                .expect("intact")
                .value,
            future.value
        );
    }
}
