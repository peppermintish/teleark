use std::path::PathBuf;

use teleark_core::{
    AccountId, ChatId, CollectionId, EncryptionState, FileKind, IndexCoverage, IndexJob,
    IndexJobId, IndexJobState, IndexRange, LogicalFile, LogicalFileId, MessageId, PackageId,
    PartIndex, PartState, RemoteObjectId, RemoteState, TransferDirection, TransferId,
    TransferState, TransferTask, VerificationState,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountRecord {
    pub id: AccountId,
    /// A user/source label, stored verbatim and never localized by storage.
    pub display_name: String,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatRecord {
    pub account_id: AccountId,
    pub id: ChatId,
    /// A Telegram/source title, stored verbatim and never localized by storage.
    pub title: String,
    pub username: Option<String>,
    pub updated_at_unix_ms: i64,
}

/// Telegram-backed identity and metadata used to rediscover a logical file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteObjectRecord {
    pub id: RemoteObjectId,
    pub logical_file_id: LogicalFileId,
    pub account_id: AccountId,
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub revision: u64,
    /// Adapter-owned opaque data; storage bounds but never interprets it.
    pub remote_key: Vec<u8>,
    pub encoded_size_bytes: u64,
    pub modified_at_unix_ms: i64,
}

/// One normalized Telegram document awaiting an atomic logical-file/remote
/// object upsert. Identity is `(account_id, chat_id, message_id)`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteFileUpsert {
    pub account_id: AccountId,
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub revision: u64,
    pub remote_key: Vec<u8>,
    pub name: String,
    pub size_bytes: u64,
    pub kind: FileKind,
    pub mime_type: Option<String>,
    pub caption: Option<String>,
    pub sent_at_unix_ms: i64,
    pub modified_at_unix_ms: i64,
}

/// Cached Telegram-native file metadata projected from an already examined
/// source message. This does not imply complete index coverage for the chat.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedTelegramFileRecord {
    pub message_id: MessageId,
    pub file_name: String,
    pub caption: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub sent_at_unix_ms: i64,
    pub modified_at_unix_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramIndexStateRecord {
    pub account_id: AccountId,
    pub chat_id: ChatId,
    pub before_message_id: Option<MessageId>,
    pub exhausted: bool,
    pub messages_scanned: u64,
    pub files_indexed: u64,
    pub updated_at_unix_ms: i64,
}

/// Non-secret Vault metadata and explicitly encoded key-wrap records.
/// Unwrapped keys and user secrets never cross this storage boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultMetadataRecord {
    pub vault_id: [u8; 16],
    pub password_wrap: Vec<u8>,
    pub recovery_wrap: Vec<u8>,
    pub password_generation: u32,
    pub recovery_generation: u32,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredNativeDownloadState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredNativeDownloadVerification {
    Pending,
    SizeChecked,
    NotReached,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewNativeDownloadTaskRecord {
    pub account_id: i64,
    pub chat_id: i64,
    pub message_id: i64,
    pub message_sent_at_unix_ms: Option<i64>,
    pub file_name: String,
    pub caption: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub destination: PathBuf,
    pub created_at_unix_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewNativeDownloadBatchRecord {
    pub chat_id: i64,
    pub created_at_unix_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDownloadBatchRecord {
    pub id: u64,
    pub chat_id: i64,
    pub created_at_unix_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDownloadTaskRecord {
    /// None only for history migrated from schemas before v9.
    pub account_id: Option<i64>,
    pub id: u64,
    pub batch_id: Option<u64>,
    pub chat_id: i64,
    pub message_id: i64,
    pub message_sent_at_unix_ms: Option<i64>,
    pub file_name: String,
    pub caption: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub destination: PathBuf,
    pub state: StoredNativeDownloadState,
    pub verification: StoredNativeDownloadVerification,
    pub transferred_bytes: u64,
    pub created_at_unix_ms: i64,
    pub started_at_unix_ms: Option<i64>,
    pub finished_at_unix_ms: Option<i64>,
    pub queue_wait_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub average_bytes_per_second: Option<u64>,
    pub attempts: u32,
    pub failure_code: Option<String>,
    pub updated_at_unix_ms: i64,
}

/// Durable projection of one file-centric library item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalFileRecord {
    pub id: LogicalFileId,
    pub name: String,
    pub relative_path: Option<String>,
    pub size_bytes: u64,
    pub kind: FileKind,
    pub mime_type: Option<String>,
    pub extension: Option<String>,
    pub caption: Option<String>,
    pub source_account_id: Option<AccountId>,
    pub source_chat_id: Option<ChatId>,
    pub created_at_unix_ms: Option<i64>,
    pub modified_at_unix_ms: Option<i64>,
    pub remote_state: RemoteState,
    pub encryption_state: EncryptionState,
    pub verification_state: VerificationState,
    pub package_id: Option<PackageId>,
    pub locally_available: bool,
    /// Absolute path to an imported local source on this machine.
    pub local_source_path: Option<PathBuf>,
}

impl From<&LogicalFile> for LogicalFileRecord {
    fn from(value: &LogicalFile) -> Self {
        Self {
            id: value.id,
            name: value.name.clone(),
            relative_path: None,
            size_bytes: value.size_bytes,
            kind: value.kind,
            mime_type: None,
            extension: value.extension().map(str::to_owned),
            caption: None,
            source_account_id: value.source_account_id,
            source_chat_id: value.source_chat_id,
            created_at_unix_ms: None,
            modified_at_unix_ms: value.modified_at_unix_ms,
            remote_state: value.remote_state,
            encryption_state: value.encryption_state,
            verification_state: value.verification_state,
            package_id: value.package_id,
            locally_available: value.remote_state == RemoteState::LocalOnly,
            local_source_path: None,
        }
    }
}

impl LogicalFileRecord {
    /// Rebuilds the complete Core projection. Storage-only metadata such as
    /// captions and the local source path remains available on this record.
    pub fn to_core(&self) -> Result<LogicalFile, teleark_core::DomainValidationError> {
        let mut file = LogicalFile::new(self.id, self.name.clone(), self.size_bytes, self.kind)?;
        file.source_account_id = self.source_account_id;
        file.source_chat_id = self.source_chat_id;
        file.modified_at_unix_ms = self.modified_at_unix_ms;
        file.remote_state = self.remote_state;
        file.encryption_state = self.encryption_state;
        file.verification_state = self.verification_state;
        file.package_id = self.package_id;
        Ok(file)
    }
}

/// A logical file awaiting an atomic, durable ID allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewLogicalFileRecord {
    pub name: String,
    pub relative_path: Option<String>,
    pub size_bytes: u64,
    pub kind: FileKind,
    pub mime_type: Option<String>,
    pub extension: Option<String>,
    pub caption: Option<String>,
    pub source_account_id: Option<AccountId>,
    pub source_chat_id: Option<ChatId>,
    pub created_at_unix_ms: Option<i64>,
    pub modified_at_unix_ms: Option<i64>,
    pub remote_state: RemoteState,
    pub encryption_state: EncryptionState,
    pub verification_state: VerificationState,
    pub package_id: Option<PackageId>,
    pub locally_available: bool,
    /// Absolute path to an imported source. It never leaves local persistence.
    pub local_source_path: Option<PathBuf>,
}

impl NewLogicalFileRecord {
    pub fn local_import(
        source_path: PathBuf,
        name: String,
        size_bytes: u64,
        kind: FileKind,
        modified_at_unix_ms: Option<i64>,
    ) -> Self {
        let extension = name
            .rsplit_once('.')
            .and_then(|(_, extension)| (!extension.is_empty()).then(|| extension.to_owned()));
        Self {
            name,
            relative_path: None,
            size_bytes,
            kind,
            mime_type: None,
            extension,
            caption: None,
            source_account_id: None,
            source_chat_id: None,
            created_at_unix_ms: None,
            modified_at_unix_ms,
            remote_state: RemoteState::LocalOnly,
            encryption_state: EncryptionState::Unencrypted,
            verification_state: VerificationState::Unverified,
            package_id: None,
            locally_available: true,
            local_source_path: Some(source_path),
        }
    }

    pub(crate) fn with_id(&self, id: LogicalFileId) -> LogicalFileRecord {
        LogicalFileRecord {
            id,
            name: self.name.clone(),
            relative_path: self.relative_path.clone(),
            size_bytes: self.size_bytes,
            kind: self.kind,
            mime_type: self.mime_type.clone(),
            extension: self.extension.clone(),
            caption: self.caption.clone(),
            source_account_id: self.source_account_id,
            source_chat_id: self.source_chat_id,
            created_at_unix_ms: self.created_at_unix_ms,
            modified_at_unix_ms: self.modified_at_unix_ms,
            remote_state: self.remote_state,
            encryption_state: self.encryption_state,
            verification_state: self.verification_state,
            package_id: self.package_id,
            locally_available: self.locally_available,
            local_source_path: self.local_source_path.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LibraryStatisticsRecord {
    pub logical_file_count: u64,
    pub logical_bytes: u64,
    pub local_file_count: u64,
    pub remote_file_count: u64,
    pub active_transfer_count: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileSearchFacets {
    pub account_id: Option<AccountId>,
    pub chat_id: Option<ChatId>,
    pub kind: Option<FileKind>,
    pub extension: Option<String>,
    pub minimum_size_bytes: Option<u64>,
    pub maximum_size_bytes: Option<u64>,
    pub modified_from_unix_ms: Option<i64>,
    pub modified_through_unix_ms: Option<i64>,
    pub remote_state: Option<RemoteState>,
    pub encryption_state: Option<EncryptionState>,
    pub verification_state: Option<VerificationState>,
    pub multipart: Option<bool>,
    pub locally_available: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageCursor(pub(crate) String);

impl PageCursor {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Performs syntax/version validation. Query compatibility is checked by
    /// [`crate::Database::search_files`].
    pub fn parse(value: String) -> crate::StorageResult<Self> {
        let fields = value.split(':').collect::<Vec<_>>();
        let version = fields
            .first()
            .and_then(|field| field.strip_prefix("ta"))
            .and_then(|field| field.parse::<u32>().ok())
            .ok_or(crate::StorageError::InvalidCursor(
                crate::CursorError::Malformed,
            ))?;
        if version != 1 {
            return Err(crate::StorageError::InvalidCursor(
                crate::CursorError::UnsupportedVersion { version },
            ));
        }
        if fields.len() != 4
            || fields[1..]
                .iter()
                .any(|field| field.len() != 16 || u64::from_str_radix(field, 16).is_err())
        {
            return Err(crate::StorageError::InvalidCursor(
                crate::CursorError::Malformed,
            ));
        }
        Ok(Self(value))
    }
}

impl TryFrom<String> for PageCursor {
    type Error = crate::StorageError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchQuery {
    pub text: Option<String>,
    pub facets: FileSearchFacets,
    pub cursor: Option<PageCursor>,
    pub limit: u32,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: None,
            facets: FileSearchFacets::default(),
            cursor: None,
            limit: 100,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPage {
    pub files: Vec<LogicalFileRecord>,
    pub next_cursor: Option<PageCursor>,
    /// Number of rows matching the query before keyset pagination is applied.
    pub total_matching: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionKind {
    Manual,
    Smart,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionRecord {
    pub id: CollectionId,
    pub name: String,
    pub kind: CollectionKind,
    /// Explicit version of the locale-neutral smart-rule representation.
    pub rule_version: Option<u32>,
    /// Adapter-neutral encoded rule. Storage does not interpret or localize it.
    pub rule_payload: Option<String>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingRecord {
    pub key: String,
    pub value: String,
    pub updated_at_unix_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredTransferDirection {
    Upload,
    Download,
}

impl From<TransferDirection> for StoredTransferDirection {
    fn from(value: TransferDirection) -> Self {
        match value {
            TransferDirection::Upload => Self::Upload,
            TransferDirection::Download => Self::Download,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredTransferState {
    Queued,
    Running,
    Paused,
    WaitingRetry,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl From<TransferState> for StoredTransferState {
    fn from(value: TransferState) -> Self {
        match value {
            TransferState::Queued => Self::Queued,
            TransferState::Running => Self::Running,
            TransferState::Paused => Self::Paused,
            TransferState::WaitingRetry => Self::WaitingRetry,
            TransferState::Verifying => Self::Verifying,
            TransferState::Completed => Self::Completed,
            TransferState::Failed => Self::Failed,
            TransferState::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredPartState {
    Queued,
    Running,
    Paused,
    WaitingRetry,
    Transferred,
    Verifying,
    Verified,
    Failed,
    Cancelled,
}

impl From<PartState> for StoredPartState {
    fn from(value: PartState) -> Self {
        match value {
            PartState::Queued => Self::Queued,
            PartState::Running => Self::Running,
            PartState::Paused => Self::Paused,
            PartState::WaitingRetry => Self::WaitingRetry,
            PartState::Transferred => Self::Transferred,
            PartState::Verifying => Self::Verifying,
            PartState::Verified => Self::Verified,
            PartState::Failed => Self::Failed,
            PartState::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferTaskRecord {
    pub id: TransferId,
    pub logical_file_id: LogicalFileId,
    pub account_id: Option<AccountId>,
    pub direction: StoredTransferDirection,
    pub priority: i16,
    pub state: StoredTransferState,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub source_path: Option<String>,
    pub destination_path: Option<String>,
    pub retry_count: u32,
    pub next_retry_at_unix_ms: Option<i64>,
    /// Stable locale-neutral failure code, never formatted error prose.
    pub last_error_code: Option<String>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferPartCheckpoint {
    pub transfer_id: TransferId,
    pub index: PartIndex,
    pub offset_bytes: u64,
    pub size_bytes: u64,
    pub transferred_bytes: u64,
    pub state: StoredPartState,
    pub attempts: u32,
    /// A storage-owned remote-object row identifier, when one is verified.
    pub remote_object_id: Option<u64>,
    /// Versioned, non-secret checkpoint bytes owned by the transfer adapter.
    pub checkpoint_version: Option<u32>,
    pub checkpoint_data: Option<Vec<u8>>,
    pub updated_at_unix_ms: i64,
}

impl TransferTaskRecord {
    /// Creates a persistence projection from Core without inventing paths,
    /// account selection, retry timing, or stringified errors.
    pub fn from_core(
        task: &TransferTask,
        timestamp_unix_ms: i64,
    ) -> (Self, Vec<TransferPartCheckpoint>) {
        let progress = task.progress();
        let record = Self {
            id: task.id(),
            logical_file_id: task.logical_file_id(),
            account_id: None,
            direction: task.direction().into(),
            priority: task.priority().get(),
            state: task.state().into(),
            total_bytes: progress.total_bytes,
            transferred_bytes: progress.transferred_bytes,
            source_path: None,
            destination_path: None,
            retry_count: 0,
            next_retry_at_unix_ms: None,
            last_error_code: None,
            created_at_unix_ms: timestamp_unix_ms,
            updated_at_unix_ms: timestamp_unix_ms,
        };
        let parts = task
            .parts()
            .iter()
            .map(|part| TransferPartCheckpoint {
                transfer_id: task.id(),
                index: part.index(),
                offset_bytes: part.offset_bytes(),
                size_bytes: part.size_bytes(),
                transferred_bytes: part.transferred_bytes(),
                state: part.state().into(),
                attempts: 0,
                remote_object_id: None,
                checkpoint_version: None,
                checkpoint_data: None,
                updated_at_unix_ms: timestamp_unix_ms,
            })
            .collect();
        (record, parts)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredIndexJobState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl From<IndexJobState> for StoredIndexJobState {
    fn from(value: IndexJobState) -> Self {
        match value {
            IndexJobState::Queued => Self::Queued,
            IndexJobState::Running => Self::Running,
            IndexJobState::Paused => Self::Paused,
            IndexJobState::Completed => Self::Completed,
            IndexJobState::Failed => Self::Failed,
            IndexJobState::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexJobRecord {
    pub id: IndexJobId,
    pub account_id: AccountId,
    pub chat_id: ChatId,
    pub state: StoredIndexJobState,
    pub policy_version: u32,
    pub policy_fingerprint: String,
    pub requested_start_message_id: Option<MessageId>,
    pub requested_end_message_id: Option<MessageId>,
    pub checkpoint_message_id: Option<MessageId>,
    pub messages_scanned: u64,
    pub files_indexed: u64,
    pub last_error_code: Option<String>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl IndexJobRecord {
    pub fn from_core(
        job: &IndexJob,
        policy_version: u32,
        policy_fingerprint: impl Into<String>,
        timestamp_unix_ms: i64,
    ) -> Self {
        Self {
            id: job.id(),
            account_id: job.account_id(),
            chat_id: job.chat_id(),
            state: job.state().into(),
            policy_version,
            policy_fingerprint: policy_fingerprint.into(),
            requested_start_message_id: None,
            requested_end_message_id: None,
            checkpoint_message_id: None,
            messages_scanned: 0,
            files_indexed: 0,
            last_error_code: None,
            created_at_unix_ms: timestamp_unix_ms,
            updated_at_unix_ms: timestamp_unix_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredIndexCoverage {
    Partial,
    Complete,
}

impl From<IndexCoverage> for StoredIndexCoverage {
    fn from(value: IndexCoverage) -> Self {
        match value {
            IndexCoverage::Partial => Self::Partial,
            IndexCoverage::Complete => Self::Complete,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexRangeRecord {
    pub account_id: AccountId,
    pub chat_id: ChatId,
    pub start_message_id: MessageId,
    pub end_message_id: MessageId,
    pub coverage: StoredIndexCoverage,
    pub checkpoint_message_id: Option<MessageId>,
    pub messages_scanned: u64,
    pub files_indexed: u64,
    pub policy_version: u32,
    pub policy_fingerprint: String,
    pub scan_generation: u64,
    pub updated_at_unix_ms: i64,
}

impl IndexRangeRecord {
    pub fn from_core(
        range: &IndexRange,
        policy_version: u32,
        policy_fingerprint: impl Into<String>,
        scan_generation: u64,
        updated_at_unix_ms: i64,
    ) -> Self {
        Self {
            account_id: range.account_id(),
            chat_id: range.chat_id(),
            start_message_id: range.start_message_id(),
            end_message_id: range.end_message_id(),
            coverage: range.coverage().into(),
            checkpoint_message_id: range.checkpoint_message_id(),
            messages_scanned: range.messages_scanned(),
            files_indexed: range.files_indexed(),
            policy_version,
            policy_fingerprint: policy_fingerprint.into(),
            scan_generation,
            updated_at_unix_ms,
        }
    }

    pub fn to_core(&self) -> Result<IndexRange, teleark_core::DomainValidationError> {
        IndexRange::try_new(
            self.account_id,
            self.chat_id,
            self.start_message_id,
            self.end_message_id,
            match self.coverage {
                StoredIndexCoverage::Partial => IndexCoverage::Partial,
                StoredIndexCoverage::Complete => IndexCoverage::Complete,
            },
            self.checkpoint_message_id,
            self.messages_scanned,
            self.files_indexed,
        )
    }
}

/// One atomic index write: file upserts, durable job progress, and coverage evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexBatch {
    pub files: Vec<LogicalFileRecord>,
    pub job: IndexJobRecord,
    pub range: IndexRangeRecord,
}

pub(crate) fn file_kind_code(value: FileKind) -> crate::StorageResult<&'static str> {
    match value {
        FileKind::Video => Ok("video"),
        FileKind::Document => Ok("document"),
        FileKind::Archive => Ok("archive"),
        FileKind::Audio => Ok("audio"),
        FileKind::Image => Ok("image"),
        FileKind::DiskImage => Ok("disk_image"),
        FileKind::Other => Ok("other"),
        _ => Err(unsupported_enum("logical_file.kind")),
    }
}

pub(crate) fn remote_state_code(value: RemoteState) -> crate::StorageResult<&'static str> {
    match value {
        RemoteState::LocalOnly => Ok("local_only"),
        RemoteState::Uploading => Ok("uploading"),
        RemoteState::Uploaded => Ok("uploaded"),
        RemoteState::RemoteMissing => Ok("remote_missing"),
        _ => Err(unsupported_enum("logical_file.remote_state")),
    }
}

pub(crate) fn encryption_state_code(value: EncryptionState) -> crate::StorageResult<&'static str> {
    match value {
        EncryptionState::Unencrypted => Ok("unencrypted"),
        EncryptionState::Encrypted => Ok("encrypted"),
        EncryptionState::Locked => Ok("locked"),
        _ => Err(unsupported_enum("logical_file.encryption_state")),
    }
}

pub(crate) fn verification_state_code(
    value: VerificationState,
) -> crate::StorageResult<&'static str> {
    match value {
        VerificationState::Unverified => Ok("unverified"),
        VerificationState::Verifying => Ok("verifying"),
        VerificationState::Verified => Ok("verified"),
        VerificationState::Failed => Ok("failed"),
        _ => Err(unsupported_enum("logical_file.verification_state")),
    }
}

fn unsupported_enum(field: &'static str) -> crate::StorageError {
    crate::StorageError::InvalidInput {
        field,
        reason: crate::InputReason::InvalidCombination,
    }
}
