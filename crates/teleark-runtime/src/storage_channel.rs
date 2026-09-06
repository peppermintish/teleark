//! Account-scoped private storage selection. SQLite is a convenience binding;
//! the Telegram description marker permits rediscovery after database loss.

use teleark_core::{ApplicationError, ApplicationErrorKind};
use teleark_storage::{Database, SettingRecord};

use crate::{DesktopLibrary, StorageRequest, TelegramChatSummary, map_storage_error};

const BINDING_PREFIX: &str = "storage-channel.v1.account.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageChannelStatus {
    Missing,
    Ready(TelegramChatSummary),
    /// Never silently substitute another channel for a saved binding.
    Unavailable {
        chat_id: i64,
        candidates: Vec<TelegramChatSummary>,
    },
    Choose(Vec<TelegramChatSummary>),
}

pub(crate) fn resolve_storage_channel(
    preferred: Option<i64>,
    candidates: Vec<TelegramChatSummary>,
) -> StorageChannelStatus {
    if let Some(chat_id) = preferred {
        return candidates
            .iter()
            .find(|chat| chat.id == chat_id)
            .cloned()
            .map_or(
                StorageChannelStatus::Unavailable {
                    chat_id,
                    candidates,
                },
                StorageChannelStatus::Ready,
            );
    }
    match candidates.as_slice() {
        [] => StorageChannelStatus::Missing,
        [channel] => StorageChannelStatus::Ready(channel.clone()),
        _ => StorageChannelStatus::Choose(candidates),
    }
}

impl DesktopLibrary {
    pub(crate) fn storage_channel_id(
        &self,
        account_id: i64,
    ) -> Result<Option<i64>, ApplicationError> {
        validate_id(account_id)?;
        self.worker
            .request("storage_channel", |reply| StorageRequest::StorageChannel {
                account_id,
                reply,
            })
    }

    pub(crate) fn save_storage_channel_id(
        &self,
        account_id: i64,
        chat_id: i64,
    ) -> Result<(), ApplicationError> {
        validate_id(account_id)?;
        validate_id(chat_id)?;
        self.worker.request("save_storage_channel", |reply| {
            StorageRequest::SaveStorageChannel {
                account_id,
                chat_id,
                reply,
            }
        })
    }
}

pub(crate) fn load_binding(
    database: &Database,
    account_id: i64,
) -> Result<Option<i64>, ApplicationError> {
    validate_id(account_id)?;
    database
        .setting(&format!("{BINDING_PREFIX}{account_id}"))
        .map_err(map_storage_error)?
        .map(|record| parse_binding(&record.value))
        .transpose()
}

pub(crate) fn save_binding(
    database: &mut Database,
    account_id: i64,
    chat_id: i64,
) -> Result<(), ApplicationError> {
    validate_id(account_id)?;
    validate_id(chat_id)?;
    database
        .set_setting(&SettingRecord {
            key: format!("{BINDING_PREFIX}{account_id}"),
            value: chat_id.to_string(),
            updated_at_unix_ms: crate::system_time_unix_ms(std::time::SystemTime::now())
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?,
        })
        .map_err(map_storage_error)
}

fn validate_id(id: i64) -> Result<(), ApplicationError> {
    if id > 0 {
        Ok(())
    } else {
        Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest))
    }
}

fn parse_binding(value: &str) -> Result<i64, ApplicationError> {
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0 && id.to_string() == value)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TelegramChatKind;

    fn channel(id: i64, name: &str) -> TelegramChatSummary {
        TelegramChatSummary {
            id,
            name: name.into(),
            username: None,
            kind: TelegramChatKind::Channel,
        }
    }

    #[test]
    fn refresh_preserves_a_renamed_binding_and_never_substitutes_a_missing_channel() {
        let chosen = channel(7, "私の保管庫");
        assert_eq!(
            resolve_storage_channel(Some(7), vec![channel(8, "TeleArk"), chosen.clone()]),
            StorageChannelStatus::Ready(chosen)
        );
        assert!(matches!(
            resolve_storage_channel(Some(7), vec![channel(8, "TeleArk")]),
            StorageChannelStatus::Unavailable { chat_id: 7, .. }
        ));
    }

    #[test]
    fn database_loss_rediscovers_one_channel_but_ambiguity_requires_selection() {
        assert_eq!(
            resolve_storage_channel(None, vec![]),
            StorageChannelStatus::Missing
        );
        let found = channel(7, "Renamed");
        assert_eq!(
            resolve_storage_channel(None, vec![found.clone()]),
            StorageChannelStatus::Ready(found)
        );
        assert!(matches!(
            resolve_storage_channel(None, vec![channel(7, "TeleArk"), channel(8, "TeleArk")]),
            StorageChannelStatus::Choose(_)
        ));
    }

    #[test]
    fn bindings_are_account_scoped_and_survive_reopening() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("catalog.sqlite3");
        {
            let library = DesktopLibrary::open(&path)?;
            assert_eq!(library.storage_channel_id(100)?, None);
            library.save_storage_channel_id(100, 7)?;
            library.save_storage_channel_id(200, 8)?;
        }
        let library = DesktopLibrary::open(&path)?;
        assert_eq!(library.storage_channel_id(100)?, Some(7));
        assert_eq!(library.storage_channel_id(200)?, Some(8));
        assert!(library.save_storage_channel_id(100, 0).is_err());
        assert_eq!(library.storage_channel_id(100)?, Some(7));
        Ok(())
    }

    #[test]
    fn unknown_or_malformed_binding_values_fail_closed() {
        for invalid in [
            "",
            "-1",
            "0",
            "07",
            "+7",
            " 7",
            "v2:7",
            "9223372036854775808",
        ] {
            assert!(parse_binding(invalid).is_err());
        }
        assert_eq!(parse_binding("7").map_err(|e| e.kind()), Ok(7));
    }
}
