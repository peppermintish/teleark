use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use fluent_bundle::FluentArgs;

/// Stable semantic identifier for a localized message.
///
/// Project Fluent identifiers use hyphens, so TeleArk uses readable IDs such as
/// `upload-dialog-title` and `transfer-state-verifying` consistently in code and
/// resources. Callers should prefer constants from [`message_ids`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MessageId(&'static str);

impl MessageId {
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }

    pub(crate) fn fluent_key(self) -> Cow<'static, str> {
        if self.0.contains('.') || self.0.contains('_') {
            Cow::Owned(
                self.0
                    .chars()
                    .map(|character| match character {
                        '.' | '_' => '-',
                        other => other,
                    })
                    .collect(),
            )
        } else {
            Cow::Borrowed(self.0)
        }
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MessageArgument {
    Text(String),
    Number(f64),
}

impl From<String> for MessageArgument {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for MessageArgument {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

macro_rules! number_argument {
    ($($type:ty),+ $(,)?) => {
        $(
            impl From<$type> for MessageArgument {
                fn from(value: $type) -> Self {
                    Self::Number(value as f64)
                }
            }
        )+
    };
}

number_argument!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageArgs {
    values: BTreeMap<&'static str, MessageArgument>,
}

impl MessageArgs {
    pub const fn new() -> Self {
        Self {
            values: BTreeMap::new(),
        }
    }

    pub fn set(&mut self, name: &'static str, value: impl Into<MessageArgument>) -> &mut Self {
        self.values.insert(name, value.into());
        self
    }

    pub fn with(mut self, name: &'static str, value: impl Into<MessageArgument>) -> Self {
        self.set(name, value);
        self
    }

    pub fn get(&self, name: &str) -> Option<&MessageArgument> {
        self.values.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn to_fluent_args(&self) -> FluentArgs<'_> {
        let mut args = FluentArgs::with_capacity(self.values.len());
        for (name, value) in &self.values {
            match value {
                MessageArgument::Text(value) => args.set(*name, value.as_str()),
                MessageArgument::Number(value) => args.set(*name, *value),
            }
        }
        args
    }
}

pub mod message_ids {
    use super::MessageId;

    macro_rules! messages {
        ($($name:ident => $id:literal),+ $(,)?) => {
            $(pub const $name: MessageId = MessageId::new($id);)+
        };
    }

    pub mod common {
        use super::MessageId;
        messages! {
            SEARCH => "common-search",
            FILTER => "common-filter",
            VIEW => "common-view",
            UPLOAD => "common-upload",
            NEW_COLLECTION => "common-new-collection",
            CANCEL => "common-cancel",
            SAVE => "common-save",
            PAUSE => "common-pause",
            RESUME => "common-resume",
            RETRY => "common-retry",
            DELETE => "common-delete",
            OPEN_FILE => "common-open-file",
            OPEN_FOLDER => "common-open-folder",
            MORE => "common-more",
            STATUS => "common-status",
            SIZE => "common-size",
            TYPE => "common-type",
            SOURCE => "common-source",
            MODIFIED => "common-modified",
            ENCRYPTED => "common-encrypted",
            PARTS => "common-parts",
            DETAILS => "common-details",
            ALL => "common-all",
            CONNECTED => "common-connected",
            ACCOUNT_COUNT => "common-account-count",
        }
    }

    pub mod library {
        use super::MessageId;
        messages! {
            TITLE => "library-title",
            ALL_FILES => "library-all-files",
            RECENT => "library-recent",
            VIDEOS => "library-videos",
            DOCUMENTS => "library-documents",
            ARCHIVES => "library-archives",
            AUDIO => "library-audio",
            IMAGES => "library-images",
            OTHER => "library-other",
            CHANNELS => "library-channels",
            COLLECTIONS => "library-collections",
            SAVED_MESSAGES => "library-saved-messages",
            TRANSFERS => "library-transfers",
            UPLOADS => "library-uploads",
            DOWNLOADS => "library-downloads",
            WAITING => "library-waiting",
            COMPLETED => "library-completed",
            FAILED => "library-failed",
            SEARCH_PLACEHOLDER => "library-search-placeholder",
            ITEM_COUNT => "library-item-count",
            NAME_COLUMN => "library-column-name",
            LOCAL_STORAGE => "library-local-storage",
            TELEGRAM_STORAGE => "library-telegram-storage",
            STORAGE_USED => "library-storage-used",
            EMPTY_TITLE => "library-empty-title",
            EMPTY_DESCRIPTION => "library-empty-description",
        }
    }

    pub mod upload {
        use super::MessageId;
        messages! {
            DIALOG_TITLE => "upload-dialog-title",
            TARGET_ACCOUNT => "upload-target-account",
            TARGET_CHANNEL => "upload-target-channel",
            STORAGE_METHOD => "upload-storage-method",
            AUTOMATIC_MULTIPART => "upload-automatic-multipart",
            AUTOMATIC_DESCRIPTION => "upload-automatic-description",
            PART_SIZE => "upload-part-size",
            COMPATIBILITY_MODE => "upload-compatibility-mode",
            CUSTOM_PART_SIZE => "upload-custom-part-size",
            SECURITY => "upload-security",
            CLIENT_ENCRYPTION => "upload-client-encryption",
            ENCRYPTION_PROFILE => "upload-encryption-profile",
            HIDE_FILENAME => "upload-hide-filename",
            ENCRYPT_METADATA => "upload-encrypt-metadata",
            ESTIMATE => "upload-estimate",
            PART_COUNT => "upload-part-count",
            TOTAL_SIZE => "upload-total-size",
            TELEGRAM_MESSAGES => "upload-telegram-messages",
            ESTIMATED_TIME => "upload-estimated-time",
            CHANGE_FILE => "upload-change-file",
            ADD_TO_QUEUE => "upload-add-to-queue",
            SELECT_FILE => "upload-select-file",
        }
    }

    pub mod transfer {
        use super::MessageId;
        messages! {
            TITLE => "transfer-title",
            ALL_TASKS => "transfer-all-tasks",
            UPLOADS => "transfer-uploads",
            DOWNLOADS => "transfer-downloads",
            WAITING => "transfer-waiting",
            COMPLETED => "transfer-completed",
            FAILED => "transfer-failed",
            START_ALL => "transfer-start-all",
            PAUSE_ALL => "transfer-pause-all",
            RETRY_FAILED => "transfer-retry-failed",
            CLEAR_COMPLETED => "transfer-clear-completed",
            NEW_QUEUE => "transfer-new-queue",
            SETTINGS => "transfer-settings",
            DOWNLOADING_COUNT => "transfer-downloading-count",
            WAITING_COUNT => "transfer-waiting-count",
            COMPLETED_COUNT => "transfer-completed-count",
            FAILED_COUNT => "transfer-failed-count",
            TOTAL_SPEED => "transfer-total-speed",
            TODAY_TRANSFERRED => "transfer-today-transferred",
            STATE_QUEUED => "transfer-state-queued",
            STATE_UPLOADING => "transfer-state-uploading",
            STATE_DOWNLOADING => "transfer-state-downloading",
            STATE_PAUSED => "transfer-state-paused",
            STATE_WAITING_RETRY => "transfer-state-waiting-retry",
            STATE_VERIFYING => "transfer-state-verifying",
            STATE_COMPLETED => "transfer-state-completed",
            STATE_FAILED => "transfer-state-failed",
            STATE_CANCELLED => "transfer-state-cancelled",
            ETA => "transfer-eta",
            REMAINING_TIME => "transfer-remaining-time",
            PARTS_COMPLETED => "transfer-parts-completed",
            QUEUE_STATS => "transfer-queue-stats",
        }
    }

    pub mod file_detail {
        use super::MessageId;
        messages! {
            TITLE => "file-detail-title",
            OVERVIEW => "file-detail-overview",
            PARTS => "file-detail-parts",
            DETAILS => "file-detail-details",
            ACTIVITY => "file-detail-activity",
            UPLOADED_VERIFIED => "file-detail-uploaded-verified",
            FILE_NAME => "file-detail-file-name",
            SIZE => "file-detail-size",
            TYPE => "file-detail-type",
            SOURCE => "file-detail-source",
            UPLOADED_AT => "file-detail-uploaded-at",
            PART_COUNT => "file-detail-part-count",
            ENCRYPTION_STATUS => "file-detail-encryption-status",
            ENCRYPTION_PROFILE => "file-detail-encryption-profile",
            FILE_HASH => "file-detail-file-hash",
            PART_NUMBER => "file-detail-part-number",
            TELEGRAM_MESSAGE_ID => "file-detail-telegram-message-id",
            OPEN_LOCATION => "file-detail-open-location",
            MORE_ACTIONS => "file-detail-more-actions",
        }
    }

    pub mod vault {
        use super::MessageId;
        messages! {
            TITLE => "vault-title",
            KEY_MANAGEMENT => "vault-key-management",
            PERSONAL_MASTER_KEY => "vault-personal-master-key",
            UNLOCKED => "vault-unlocked",
            LOCKED => "vault-locked",
            CREATED_AT => "vault-created-at",
            KDF => "vault-kdf",
            STATUS => "vault-status",
            CHANGE_PASSWORD => "vault-change-password",
            LOCK_VAULT => "vault-lock-vault",
            RECOVERY_BACKUP => "vault-recovery-backup",
            RECOVERY_KEY => "vault-recovery-key",
            BACKED_UP => "vault-backed-up",
            NOT_BACKED_UP => "vault-not-backed-up",
            SHOW => "vault-show",
            EXPORT => "vault-export",
            WARNING_KEY_LOSS => "vault-warning-key-loss",
        }
    }

    pub mod index {
        use super::MessageId;
        messages! {
            TITLE => "index-title",
            STATUS => "index-status",
            MESSAGES_SCANNED => "index-messages-scanned",
            FILES_INDEXED => "index-files-indexed",
            INDEXED_SIZE => "index-indexed-size",
            LATEST_SYNC => "index-latest-sync",
            NEW_FILES => "index-new-files",
            CURRENT_PROGRESS => "index-current-progress",
            CURRENT_DATE => "index-current-date",
            SCAN_SPEED => "index-scan-speed",
            ETA => "index-eta",
            START => "index-start",
            PAUSE => "index-pause",
            RESUME => "index-resume",
            CANCEL => "index-cancel",
            COVERAGE => "index-coverage",
            COVERAGE_COMPLETE => "index-coverage-complete",
            COVERAGE_PARTIAL => "index-coverage-partial",
            COVERAGE_NOT_SCANNED => "index-coverage-not-scanned",
            LAST_30_DAYS => "index-last-30-days",
            LAST_6_MONTHS => "index-last-6-months",
            LAST_YEAR => "index-last-year",
            ALL_HISTORY => "index-all-history",
            CUSTOM_RANGE => "index-custom-range",
            CONTENT_TYPES => "index-content-types",
            FILES => "index-files",
            VIDEOS => "index-videos",
            IMAGES => "index-images",
            AUDIO => "index-audio",
            PLAIN_TEXT => "index-plain-text",
            SKIP_SMALL_MEDIA => "index-skip-small-media",
        }
    }

    pub mod settings {
        use super::MessageId;
        messages! {
            TITLE => "settings-title",
            GENERAL => "settings-general",
            ACCOUNTS => "settings-accounts",
            STORAGE => "settings-storage",
            DOWNLOADS => "settings-downloads",
            UPLOADS => "settings-uploads",
            KEY_VAULT => "settings-key-vault",
            INDEX => "settings-index",
            NOTIFICATIONS => "settings-notifications",
            APPEARANCE => "settings-appearance",
            ADVANCED => "settings-advanced",
            LANGUAGE_TITLE => "settings-language-title",
            LANGUAGE_DESCRIPTION => "settings-language-description",
            SYSTEM_DEFAULT => "settings-language-system-default",
            ENGLISH => "settings-language-english",
            CHINESE => "settings-language-chinese",
            JAPANESE => "settings-language-japanese",
            THEME => "settings-theme",
            THEME_SYSTEM => "settings-theme-system",
            THEME_LIGHT => "settings-theme-light",
            THEME_DARK => "settings-theme-dark",
            CONCURRENCY => "settings-concurrency",
            BANDWIDTH => "settings-bandwidth",
        }
    }

    pub mod error {
        use super::MessageId;
        messages! {
            TRANSFER_NETWORK => "error-transfer-network",
            TRANSFER_FLOOD_WAIT => "error-transfer-flood-wait",
            TRANSFER_AUTHORIZATION => "error-transfer-authorization",
            TRANSFER_SOURCE_MISSING => "error-transfer-source-missing",
            TRANSFER_SOURCE_CHANGED => "error-transfer-source-changed",
            TRANSFER_DISK_FULL => "error-transfer-disk-full",
            TRANSFER_PERMISSION_DENIED => "error-transfer-permission-denied",
            TRANSFER_REMOTE_MISSING => "error-transfer-remote-missing",
            TRANSFER_HASH_MISMATCH => "error-transfer-hash-mismatch",
            TRANSFER_AUTHENTICATION_FAILED => "error-transfer-authentication-failed",
            TRANSFER_MANIFEST_CORRUPTED => "error-transfer-manifest-corrupted",
            TRANSFER_UNSUPPORTED_MANIFEST => "error-transfer-unsupported-manifest",
            TRANSFER_KEY_UNAVAILABLE => "error-transfer-key-unavailable",
            TRANSFER_WRONG_PASSWORD => "error-transfer-wrong-password",
            TRANSFER_DATABASE => "error-transfer-database",
            TRANSFER_CANCELLED => "error-transfer-cancelled",
            TRANSFER_UNKNOWN => "error-transfer-unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_args_replace_existing_values_by_name() {
        let mut args = MessageArgs::new();
        args.set("count", 1_u64).set("count", 2_u64);
        assert_eq!(args.get("count"), Some(&MessageArgument::Number(2.0)));
    }
}
