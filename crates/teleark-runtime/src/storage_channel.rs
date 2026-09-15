//! One fixed channel per account; management metadata can be repaired in place.

use teleark_core::{ApplicationError, ApplicationErrorKind};
use teleark_storage::{Database, SettingRecord};

use crate::{DesktopLibrary, StorageRequest, TelegramChatSummary, map_storage_error};

const FIXED_BINDING_PREFIX: &str = "storage-channel.v2.account.";
const BINDING_PREFIX: &str = "storage-channel.v1.account.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageChannelStatus {
    Missing,
    Ready(TelegramChatSummary),
    Degraded {
        channel: TelegramChatSummary,
        health: StorageChannelHealth,
    },
    /// Never silently substitute another channel for a saved binding.
    Unavailable {
        chat_id: i64,
        candidates: Vec<TelegramChatSummary>,
    },
    Choose(Vec<TelegramChatSummary>),
}

pub use teleark_telegram::{StorageChannelHealth, StorageMaintenancePhase};

impl StorageChannelStatus {
    pub fn channel(&self) -> Option<&TelegramChatSummary> {
        match self {
            Self::Ready(channel) | Self::Degraded { channel, .. } => Some(channel),
            _ => None,
        }
    }
    pub fn usable_channel(&self) -> Option<&TelegramChatSummary> {
        match self {
            Self::Ready(channel) => Some(channel),
            Self::Degraded { channel, health } if health.permits_files() => Some(channel),
            _ => None,
        }
    }
    pub fn health(&self) -> Option<StorageChannelHealth> {
        match self {
            Self::Ready(_) => Some(StorageChannelHealth::Healthy),
            Self::Degraded { health, .. } => Some(*health),
            _ => None,
        }
    }
}

/// Result of automatic management; this is transient presentation metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedStorageChannel {
    pub channel: TelegramChatSummary,
    pub created: bool,
    /// A previously bound peer was confirmed unavailable and replaced.
    pub replaced: bool,
    pub health: StorageChannelHealth,
}

/// Complete remote discovery must yield exactly one candidate. Local identity
/// caches never resolve conflicting remote evidence or authorize repair.
pub(crate) fn resolve_managed_storage_channel(
    preferred: Option<i64>,
    candidates: Vec<TelegramChatSummary>,
) -> StorageChannelStatus {
    resolve_storage_channel(preferred, candidates)
}

/// An ambiguous creation may have reached Telegram. Subsequent automatic work
/// may discover its result but must not issue another create in this process.
#[derive(Default)]
pub(crate) struct StorageCreationGuard(std::collections::BTreeSet<i64>);

impl StorageCreationGuard {
    pub(crate) fn begin(&mut self, account: i64) -> Result<(), ApplicationError> {
        if self.0.insert(account) {
            Ok(())
        } else {
            Err(ApplicationError::new(ApplicationErrorKind::Conflict))
        }
    }

    pub(crate) fn resolved(&mut self, account: i64) {
        self.0.remove(&account);
    }
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

    pub(crate) fn replace_storage_channel_id(
        &self,
        account_id: i64,
        expected_chat_id: i64,
        replacement_chat_id: i64,
    ) -> Result<(), ApplicationError> {
        validate_id(account_id)?;
        validate_id(expected_chat_id)?;
        validate_id(replacement_chat_id)?;
        self.worker.request("replace_storage_channel", |reply| {
            StorageRequest::ReplaceStorageChannel {
                account_id,
                expected_chat_id,
                replacement_chat_id,
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
    if let Some(record) = database
        .setting(&format!("{FIXED_BINDING_PREFIX}{account_id}"))
        .map_err(map_storage_error)?
    {
        let fixed: serde_json::Value = serde_json::from_str(&record.value)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let id = fixed
            .get("channel_id")
            .and_then(serde_json::Value::as_i64)
            .filter(|id| *id > 0);
        if fixed.get("version").and_then(serde_json::Value::as_u64) != Some(2)
            || fixed.as_object().is_none_or(|v| v.len() != 2)
            || id.is_none()
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Persistence));
        }
        return Ok(id);
    }
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
    if load_binding(database, account_id)?.is_some_and(|bound| bound != chat_id) {
        return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
    }
    database
        .set_setting(&SettingRecord {
            key: format!("{FIXED_BINDING_PREFIX}{account_id}"),
            value: format!("{{\"version\":2,\"channel_id\":{chat_id}}}"),
            updated_at_unix_ms: crate::system_time_unix_ms(std::time::SystemTime::now())
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?,
        })
        .map_err(map_storage_error)
}

pub(crate) fn replace_binding(
    database: &mut Database,
    account_id: i64,
    expected_chat_id: i64,
    replacement_chat_id: i64,
) -> Result<(), ApplicationError> {
    validate_id(account_id)?;
    validate_id(expected_chat_id)?;
    validate_id(replacement_chat_id)?;
    if load_binding(database, account_id)? != Some(expected_chat_id) {
        return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
    }
    let fixed_key = format!("{FIXED_BINDING_PREFIX}{account_id}");
    let legacy_key = format!("{BINDING_PREFIX}{account_id}");
    let fixed = database.setting(&fixed_key).map_err(map_storage_error)?;
    let fallback = if fixed.is_none() {
        database.setting(&legacy_key).map_err(map_storage_error)?
    } else {
        None
    };
    let replaced = database
        .compare_and_swap_setting(
            &SettingRecord {
                key: fixed_key,
                value: format!("{{\"version\":2,\"channel_id\":{replacement_chat_id}}}"),
                updated_at_unix_ms: crate::system_time_unix_ms(std::time::SystemTime::now())
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?,
            },
            fixed.as_ref().map(|record| record.value.as_str()),
            fallback
                .as_ref()
                .map(|record| (legacy_key.as_str(), record.value.as_str())),
        )
        .map_err(map_storage_error)?;
    if replaced {
        Ok(())
    } else {
        Err(ApplicationError::new(ApplicationErrorKind::Conflict))
    }
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
            sync_pts: None,
            id,
            name: name.into(),
            username: None,
            kind: TelegramChatKind::Channel,
        }
    }

    #[test]
    fn automatic_management_preserves_a_fixed_binding() {
        assert_eq!(
            resolve_managed_storage_channel(None, vec![channel(8, "TeleArk")]),
            StorageChannelStatus::Ready(channel(8, "TeleArk"))
        );
        assert_eq!(
            resolve_managed_storage_channel(None, vec![]),
            StorageChannelStatus::Missing
        );
        for bound in [7, 99] {
            assert_eq!(
                resolve_managed_storage_channel(Some(bound), vec![channel(8, "TeleArk")]),
                StorageChannelStatus::Unavailable {
                    chat_id: bound,
                    candidates: vec![channel(8, "TeleArk")]
                }
            );
            assert_eq!(
                resolve_managed_storage_channel(Some(bound), vec![]),
                StorageChannelStatus::Unavailable {
                    chat_id: bound,
                    candidates: vec![]
                }
            );
        }
        let candidates = vec![channel(7, "Renamed"), channel(8, "TeleArk")];
        assert_eq!(
            resolve_managed_storage_channel(Some(7), candidates.clone()),
            StorageChannelStatus::Ready(channel(7, "Renamed"))
        );
        assert_eq!(
            resolve_managed_storage_channel(None, candidates.clone()),
            StorageChannelStatus::Choose(candidates)
        );
    }

    #[test]
    fn uncertain_creation_blocks_duplicate_requests_until_discovery_resolves_it() {
        let mut guard = StorageCreationGuard::default();
        assert!(guard.begin(100).is_ok());
        // Includes dropped/timeout RPC futures: begin remains recorded.
        assert_eq!(
            guard.begin(100).expect_err("no duplicate").kind(),
            ApplicationErrorKind::Conflict
        );
        assert!(guard.begin(200).is_ok());
        guard.resolved(100);
        assert!(guard.begin(100).is_ok());
        assert_eq!(
            guard
                .begin(200)
                .expect_err("other account still unresolved")
                .kind(),
            ApplicationErrorKind::Conflict
        );
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
    fn unavailable_binding_replacement_is_compare_and_swap() -> Result<(), ApplicationError> {
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = temp.path().join("catalog.sqlite3");
        let mut database = Database::open(&path).expect("open database");
        save_binding(&mut database, 100, 7)?;
        save_binding(&mut database, 200, 11)?;

        let conflict =
            replace_binding(&mut database, 100, 8, 9).expect_err("stale replacement must fail");
        assert_eq!(conflict.kind(), ApplicationErrorKind::Conflict);
        assert_eq!(load_binding(&database, 100)?, Some(7));

        replace_binding(&mut database, 100, 7, 9)?;
        assert_eq!(load_binding(&database, 100)?, Some(9));
        assert_eq!(load_binding(&database, 200)?, Some(11));
        drop(database);
        let database = Database::open(&path).expect("reopen database");
        assert_eq!(load_binding(&database, 100)?, Some(9));
        assert_eq!(load_binding(&database, 200)?, Some(11));
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
