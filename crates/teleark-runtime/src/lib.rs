//! Frontend-neutral desktop composition for TeleArk.
//!
//! This crate owns adapter worker lifecycles so frontends never execute SQL or
//! other blocking infrastructure work directly.

mod local_files;
pub use local_files::{
    LOCAL_FILE_PROBE_LIMIT, LocalFilePresence, VolumeSpace, local_file_presence, probe_local_files,
    volume_space,
};
pub use teleark_storage::{DownloadedFileRecord, DownloadedFilesCursor};

use std::sync::mpsc::SyncSender;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use teleark_core::{
    ApplicationError, ApplicationErrorKind, FileKind, ImportLocalFile, LibraryItem, LibraryPage,
    LibraryQuery, LibraryRepository, LibraryService, LibrarySort, LibraryStatistics, LogicalFile,
    LogicalFileId,
};
use teleark_storage::{
    AccountRecord, CachedTelegramFileRecord, ChatRecord, Database, FileSearchFacets,
    LogicalFileRecord, NativeDownloadBatchRecord, NativeDownloadTaskRecord, NewLogicalFileRecord,
    NewNativeDownloadBatchRecord, NewNativeDownloadTaskRecord, PageCursor, RemoteFileUpsert,
    SearchQuery, SettingRecord, StorageError, TelegramIndexStateRecord, VaultMetadataRecord,
};
pub use teleark_telegram::{TelegramAccount, TelegramChatKind};

mod channel_transfer;
mod credentials;
mod diagnostics;
mod telegram;
mod transfer;
mod vault;

pub use channel_transfer::{
    ChannelDownloadEvent, ChannelDownloadEventKind, ChannelDownloadFailure,
    ChannelDownloadFailureStage, ChannelDownloadPartEvent, ChannelDownloadRequest,
    ChannelDownloadSnapshot, ChannelDownloadState, ChannelDownloadVerification, DesktopTransfers,
    TransferRates, available_download_destination,
};
pub use credentials::TelegramCredentialSource;
mod storage_channel;
pub use diagnostics::{DiagnosticsStatus, diagnostics_status, initialize_diagnostics};
pub use storage_channel::StorageChannelStatus;
pub use teleark_telegram::DownloadPartState;
pub use teleark_transfer::{
    ControllerDecision, ControllerDecisionOutcome, ControllerDecisionReason, ControllerPhase,
    DOWNLOAD_PART_SIZE_BYTES, DownloadThroughputStrategy, LaneTelemetry, MemoryCounters,
    ParameterBounds, PartCounters, QueueCounters, SoftLimitPolicy, TransferBottleneck,
    TransferControlParameters, TransferTelemetrySnapshot, TunableParameter,
};
pub use telegram::{
    DesktopTelegram, TelegramAuthState, TelegramChatSummary, TelegramFileFilter, TelegramFilePage,
    TelegramFileSummary, TelegramScanCancellation, default_telegram_session_path,
};
pub use transfer::{
    EncryptedRemoteTransport, ManifestPublishRequest, ManifestRecoveryReport, ProductionTransferIo,
    RecoveredManifest, RejectedManifest, RemoteByteObject, RemoteObjectStore,
    SqliteCheckpointStore, TelegramObjectStore, encrypted_part_plaintext_limit,
    encrypted_part_sizes, recover_remote_manifests,
};
pub use vault::{
    DesktopVault, MAX_VAULT_UPLOAD_BATCH, ManagedVaultFile, ManagedVaultScan, VaultStatus,
    VaultTransferDirection, VaultTransferSnapshot, VaultTransferState, VaultUploadFailure,
    VaultUploadReport, VaultUploadSource, inspect_upload_sources,
};

const STORAGE_QUEUE_CAPACITY: usize = 64;
const LOCALE_OVERRIDE_SETTING_KEY: &str = "locale.override";
const TELEGRAM_API_ID_SETTING_KEY: &str = "telegram.api_id";
const TELEGRAM_API_HASH_SETTING_KEY: &str = "telegram.api_hash";
const PREFERENCE_PREFIX: &str = "preferences.v1.";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AppearancePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopPreferences {
    pub managed_files_root: Option<PathBuf>,
    pub reveal_completed_downloads: bool,
    pub upload_part_size_mib: u16,
    pub upload_encrypt_content: bool,
    pub upload_hide_file_name: bool,
    pub upload_encrypt_metadata: bool,
    pub transfer_soft_limit_policy: SoftLimitPolicy,
    pub download_throughput_strategy: DownloadThroughputStrategy,
    pub lock_vault_when_hidden: bool,
    pub index_batch_size: u16,
    pub notify_download_completed: bool,
    pub notify_download_failed: bool,
    pub appearance: AppearancePreference,
    pub sidebar_collapsed: bool,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            managed_files_root: None,
            reveal_completed_downloads: false,
            upload_part_size_mib: 1_900,
            upload_encrypt_content: true,
            upload_hide_file_name: true,
            upload_encrypt_metadata: true,
            transfer_soft_limit_policy: SoftLimitPolicy::AdaptiveOverride,
            download_throughput_strategy: DownloadThroughputStrategy::Balanced,
            lock_vault_when_hidden: true,
            index_batch_size: 1_000,
            notify_download_completed: true,
            notify_download_failed: true,
            appearance: AppearancePreference::System,
            sidebar_collapsed: true,
        }
    }
}

/// Filesystem layout owned by TeleArk for user-manageable data. The SQLite
/// database and Telegram session remain in the platform application-data
/// directory so changing this root never moves an open database or session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedDirectories {
    pub root: PathBuf,
    pub downloads: PathBuf,
    pub cache: PathBuf,
    pub logs: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagedStorageMetrics {
    pub app_used_bytes: u64,
    pub available_bytes: u64,
}

/// Non-secret credential presence exposed to frontends. The API Hash value is
/// loaded only inside the runtime when Telegram authentication starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TelegramCredentialsStatus {
    pub api_id: i32,
    pub source: TelegramCredentialSource,
}

/// Resolves the per-user database location without creating it.
pub fn default_database_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support/TeleArk/library.sqlite3"))
    }

    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|root| root.join("TeleArk/library.sqlite3"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local/share"))
            })
            .map(|root| root.join("teleark/library.sqlite3"))
    }
}

/// Resolves the default desktop managed-file layout without touching disk.
pub fn default_managed_directories(preferences: &DesktopPreferences) -> Option<ManagedDirectories> {
    default_database_path().and_then(|path| managed_directories_for(&path, preferences).ok())
}

fn managed_directories_for(
    database_path: &Path,
    preferences: &DesktopPreferences,
) -> Result<ManagedDirectories, ApplicationError> {
    let root = match preferences.managed_files_root.as_ref() {
        Some(root) => root.clone(),
        None => database_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?,
    };
    if root.as_os_str().is_empty() || root.to_str().is_none() {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok(ManagedDirectories {
        downloads: root.join("Downloads"),
        cache: root.join("Cache"),
        logs: root.join("Logs"),
        root,
    })
}

fn prepare_managed_directories(
    database_path: &Path,
    preferences: &DesktopPreferences,
) -> Result<ManagedDirectories, ApplicationError> {
    let directories = managed_directories_for(database_path, preferences)?;
    std::fs::create_dir_all(&directories.downloads).map_err(map_filesystem_error)?;
    std::fs::create_dir_all(&directories.cache).map_err(map_filesystem_error)?;
    std::fs::create_dir_all(&directories.logs).map_err(map_filesystem_error)?;
    Ok(directories)
}

fn managed_storage_metrics(
    managed_root: &Path,
    database_path: &Path,
) -> Result<ManagedStorageMetrics, ApplicationError> {
    const MAX_STORAGE_ENTRIES: usize = 1_000_000;
    let database_root = database_path
        .parent()
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    let roots: Vec<&Path> = if managed_root.starts_with(database_root) {
        vec![database_root]
    } else if database_root.starts_with(managed_root) {
        vec![managed_root]
    } else {
        vec![managed_root, database_root]
    };
    let mut remaining_entries = MAX_STORAGE_ENTRIES;
    let mut app_used_bytes = 0_u64;
    for root in roots {
        app_used_bytes =
            app_used_bytes.saturating_add(directory_size_bounded(root, &mut remaining_entries)?);
    }
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let available_bytes = disks
        .list()
        .iter()
        .filter(|disk| managed_root.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space())
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    Ok(ManagedStorageMetrics {
        app_used_bytes,
        available_bytes,
    })
}

fn directory_size_bounded(
    root: &Path,
    remaining_entries: &mut usize,
) -> Result<u64, ApplicationError> {
    let mut pending = vec![root.to_owned()];
    let mut total = 0_u64;
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(map_filesystem_error(error)),
        };
        for entry in entries {
            if *remaining_entries == 0 {
                return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
            }
            *remaining_entries -= 1;
            let entry = entry.map_err(map_filesystem_error)?;
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(map_filesystem_error)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

fn map_filesystem_error(error: std::io::Error) -> ApplicationError {
    let kind = if error.kind() == std::io::ErrorKind::PermissionDenied {
        ApplicationErrorKind::PermissionDenied
    } else {
        ApplicationErrorKind::Persistence
    };
    ApplicationError::new(kind)
}

/// Classifies a local filename without altering the user's name or extension.
pub fn classify_file(path: &Path) -> FileKind {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("mkv" | "mp4" | "mov" | "avi" | "webm" | "m4v") => FileKind::Video,
        Some("pdf" | "doc" | "docx" | "odt" | "rtf" | "txt" | "md") => FileKind::Document,
        Some("zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst") => FileKind::Archive,
        Some("mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "opus") => FileKind::Audio,
        Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "tif" | "tiff") => FileKind::Image,
        Some("iso" | "dmg" | "img" | "vhd" | "vhdx") => FileKind::DiskImage,
        _ => FileKind::Other,
    }
}

/// Ready-to-use local library facade for desktop frontends.
#[derive(Clone)]
pub struct DesktopLibrary {
    service: Arc<LibraryService<StorageWorker>>,
    worker: StorageWorker,
    database_path: Arc<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramIndexPage {
    pub files_indexed: u64,
    pub messages_scanned: u64,
    pub next_before_message_id: Option<i64>,
    pub exhausted: bool,
}

impl DesktopLibrary {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        let path = path.as_ref();
        prepare_database_parent(path)?;
        let worker = StorageWorker::open(path.to_owned())?;
        Ok(Self {
            service: Arc::new(LibraryService::new(worker.clone())),
            worker,
            database_path: Arc::new(path.to_owned()),
        })
    }

    pub fn open_default() -> Result<Self, ApplicationError> {
        let path = default_database_path()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        Self::open(path)
    }

    pub fn search(&self, query: &LibraryQuery) -> Result<LibraryPage, ApplicationError> {
        self.service.search_files(query)
    }

    pub fn statistics(&self) -> Result<LibraryStatistics, ApplicationError> {
        self.service.library_statistics()
    }

    /// Inspects and imports caller-selected files into the persistent library.
    pub fn import_paths(
        &self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> Vec<Result<LogicalFile, ApplicationError>> {
        paths
            .into_iter()
            .map(|path| {
                inspect_local_file(path)
                    .and_then(|command| self.service.import_local_file(&command))
            })
            .collect()
    }

    pub fn delete_file(&self, id: LogicalFileId) -> Result<bool, ApplicationError> {
        self.service.delete_file(id)
    }

    /// Returns the persisted language override, or `None` when the application
    /// should follow the operating system language.
    pub fn locale_override(&self) -> Result<Option<String>, ApplicationError> {
        self.worker.locale_override()
    }

    /// Persists a language override. Passing `None` restores System Default.
    pub fn set_locale_override(&self, locale: Option<&str>) -> Result<(), ApplicationError> {
        if locale.is_some_and(|locale| locale.trim().is_empty() || locale.len() > 35) {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        self.worker.set_locale_override(locale)
    }

    /// Reports whether a complete, validated Telegram API credential pair is
    /// stored without exposing the API Hash to the frontend.
    pub fn telegram_credentials_status(
        &self,
    ) -> Result<Option<TelegramCredentialsStatus>, ApplicationError> {
        self.worker.telegram_credentials_status()
    }

    /// Persists a complete Telegram API credential pair in the Library
    /// database. Callers must not log or retain `api_hash` after this returns.
    pub fn set_telegram_credentials(
        &self,
        api_id: i32,
        api_hash: &str,
    ) -> Result<(), ApplicationError> {
        if api_id <= 0 {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        credentials::validate_api_hash(api_hash)?;
        self.worker.set_telegram_credentials(api_id, api_hash)
    }

    /// Removes both stored Telegram application credentials atomically.
    pub fn clear_telegram_credentials(&self) -> Result<(), ApplicationError> {
        self.worker.clear_telegram_credentials()
    }

    pub fn preferences(&self) -> Result<DesktopPreferences, ApplicationError> {
        self.worker.preferences()
    }

    pub fn set_preferences(
        &self,
        preferences: &DesktopPreferences,
    ) -> Result<(), ApplicationError> {
        validate_preferences(preferences)?;
        self.worker.set_preferences(preferences.clone())
    }

    /// Returns the managed layout, creating its download and cache
    /// directories when necessary.
    pub fn managed_directories(&self) -> Result<ManagedDirectories, ApplicationError> {
        self.worker.managed_directories()
    }

    /// Cheap volume query for the configured download location, without a
    /// recursive app-data walk or creating an unavailable destination.
    pub fn download_volume_space(&self) -> Result<VolumeSpace, ApplicationError> {
        let directories = managed_directories_for(&self.database_path, &self.preferences()?)?;
        volume_space(&directories.downloads)
    }

    /// Measures TeleArk-owned data without following symlinks. Callers should
    /// run this bounded filesystem walk away from the GUI thread.
    pub fn managed_storage_metrics(&self) -> Result<ManagedStorageMetrics, ApplicationError> {
        let managed = self.managed_directories()?;
        managed_storage_metrics(&managed.root, &self.database_path)
    }

    /// Allocates a non-existing path in the managed Downloads directory.
    /// This keeps destination selection and collision handling outside the UI.
    pub fn next_download_destination(
        &self,
        suggested_file_name: &str,
    ) -> Result<PathBuf, ApplicationError> {
        self.worker
            .next_download_destination(suggested_file_name.to_owned())
    }

    pub(crate) fn insert_native_download(
        &self,
        task: NewNativeDownloadTaskRecord,
    ) -> Result<NativeDownloadTaskRecord, ApplicationError> {
        self.worker.insert_native_download(task)
    }

    pub(crate) fn insert_native_download_batch(
        &self,
        batch: NewNativeDownloadBatchRecord,
        tasks: Vec<NewNativeDownloadTaskRecord>,
    ) -> Result<(NativeDownloadBatchRecord, Vec<NativeDownloadTaskRecord>), ApplicationError> {
        self.worker.insert_native_download_batch(batch, tasks)
    }

    pub(crate) fn save_native_download(
        &self,
        task: NativeDownloadTaskRecord,
    ) -> Result<(), ApplicationError> {
        self.worker.save_native_download(task)
    }

    pub(crate) fn resolve_legacy_native_download_accounts(
        &self,
        account_id: Option<i64>,
    ) -> Result<(), ApplicationError> {
        self.worker
            .request("resolve_legacy_download_accounts", |reply| {
                StorageRequest::ResolveLegacyDownloadAccounts { account_id, reply }
            })
    }

    pub(crate) fn record_vault_download(
        &self,
        record: teleark_storage::VaultDownloadRecord,
    ) -> Result<(), ApplicationError> {
        self.worker.request("record_vault_download", |reply| {
            StorageRequest::RecordVaultDownload { record, reply }
        })
    }

    /// Bounded inventory of successful downloads for the specified account.
    pub fn downloaded_files_page(
        &self,
        account_id: i64,
        after: Option<DownloadedFilesCursor>,
    ) -> Result<Vec<DownloadedFileRecord>, ApplicationError> {
        self.worker.request("downloaded_files_page", |reply| {
            StorageRequest::DownloadedFilesPage {
                account_id,
                after,
                reply,
            }
        })
    }

    pub(crate) fn native_downloads(
        &self,
    ) -> Result<Vec<NativeDownloadTaskRecord>, ApplicationError> {
        self.worker.native_downloads()
    }

    pub(crate) fn delete_native_download(&self, task_id: u64) -> Result<(), ApplicationError> {
        self.worker.delete_native_download(task_id)
    }

    pub(crate) fn stored_telegram_credentials(
        &self,
    ) -> Result<Option<credentials::ActiveTelegramCredentials>, ApplicationError> {
        self.worker.active_telegram_credentials()
    }

    /// Persists the authorized account and its currently visible dialogs.
    pub fn save_telegram_sources(
        &self,
        account: &TelegramAccount,
        chats: &[TelegramChatSummary],
    ) -> Result<(), ApplicationError> {
        self.worker.save_telegram_sources(account, chats)
    }

    /// Returns already projected files for immediate source browsing. These
    /// rows are a cache only and do not imply complete history coverage.
    pub fn cached_telegram_files(
        &self,
        account_id: i64,
        chat_id: i64,
        limit: usize,
    ) -> Result<Vec<TelegramFileSummary>, ApplicationError> {
        self.worker
            .cached_telegram_files(
                teleark_core::AccountId::new(account_id),
                teleark_core::ChatId::new(chat_id),
                limit,
            )
            .map(|files| files.into_iter().map(cached_file_summary).collect())
    }

    /// Projects files observed by interactive browsing into the local cache
    /// without advancing durable index coverage or its cursor.
    pub fn cache_telegram_files(
        &self,
        account_id: i64,
        chat_id: i64,
        files: &[TelegramFileSummary],
    ) -> Result<u64, ApplicationError> {
        let account_id = teleark_core::AccountId::new(account_id);
        let chat_id = teleark_core::ChatId::new(chat_id);
        let files = telegram_file_upserts(account_id, chat_id, files)?;
        self.worker.upsert_remote_files(files)
    }

    /// Fetches one bounded Telegram history page and atomically projects every
    /// document into SQLite. The returned cursor advances over non-file
    /// messages too, so restart cannot stall on a text-only history region.
    pub fn index_telegram_page(
        &self,
        telegram: &DesktopTelegram,
        account_id: i64,
        chat_id: i64,
        before_message_id: Option<i64>,
        limit: usize,
    ) -> Result<TelegramIndexPage, ApplicationError> {
        let account_id = teleark_core::AccountId::new(account_id);
        let chat_id = teleark_core::ChatId::new(chat_id);
        let previous = self.worker.telegram_index_state(account_id, chat_id)?;
        if previous.as_ref().is_some_and(|state| state.exhausted) {
            let previous =
                previous.ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            return Ok(TelegramIndexPage {
                files_indexed: previous.files_indexed,
                messages_scanned: previous.messages_scanned,
                next_before_message_id: None,
                exhausted: true,
            });
        }
        let durable_before = previous
            .as_ref()
            .and_then(|state| state.before_message_id)
            .map(teleark_core::MessageId::get)
            .or(before_message_id);
        let page =
            telegram.scan_file_page(account_id.get(), chat_id.get(), durable_before, limit)?;
        let files = telegram_file_upserts(account_id, chat_id, &page.files)?;
        let page_files = self.worker.upsert_remote_files(files)?;
        let files_indexed = previous
            .as_ref()
            .map_or(0, |state| state.files_indexed)
            .checked_add(page_files)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        let messages_scanned = previous
            .as_ref()
            .map_or(0, |state| state.messages_scanned)
            .checked_add(page.examined_messages)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        let state = TelegramIndexStateRecord {
            account_id,
            chat_id,
            before_message_id: page
                .next_before_message_id
                .map(teleark_core::MessageId::new),
            exhausted: page.exhausted,
            messages_scanned,
            files_indexed,
            updated_at_unix_ms: system_time_unix_ms(SystemTime::now())
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?,
        };
        self.worker.save_telegram_index_state(state)?;
        Ok(TelegramIndexPage {
            files_indexed,
            messages_scanned,
            next_before_message_id: page.next_before_message_id,
            exhausted: page.exhausted,
        })
    }
}

#[derive(Clone)]
struct StorageWorker {
    inner: Arc<WorkerInner>,
}

struct WorkerInner {
    sender: SyncSender<StorageRequest>,
    join: Mutex<Option<JoinHandle<()>>>,
}

enum StorageRequest {
    ResolveLegacyDownloadAccounts {
        account_id: Option<i64>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },

    StorageChannel {
        account_id: i64,
        reply: SyncSender<Result<Option<i64>, ApplicationError>>,
    },
    SaveStorageChannel {
        account_id: i64,
        chat_id: i64,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    Search {
        query: LibraryQuery,
        reply: SyncSender<Result<LibraryPage, ApplicationError>>,
    },
    Statistics {
        reply: SyncSender<Result<LibraryStatistics, ApplicationError>>,
    },
    Import {
        command: ImportLocalFile,
        reply: SyncSender<Result<LogicalFile, ApplicationError>>,
    },
    File {
        id: LogicalFileId,
        reply: SyncSender<Result<Option<LogicalFile>, ApplicationError>>,
    },
    Delete {
        id: LogicalFileId,
        reply: SyncSender<Result<bool, ApplicationError>>,
    },
    LocaleOverride {
        reply: SyncSender<Result<Option<String>, ApplicationError>>,
    },
    SetLocaleOverride {
        locale: Option<String>,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    Preferences {
        reply: SyncSender<Result<DesktopPreferences, ApplicationError>>,
    },
    SetPreferences {
        preferences: DesktopPreferences,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    ManagedDirectories {
        reply: SyncSender<Result<ManagedDirectories, ApplicationError>>,
    },
    NextDownloadDestination {
        suggested_file_name: String,
        reply: SyncSender<Result<PathBuf, ApplicationError>>,
    },
    TelegramCredentialsStatus {
        reply: SyncSender<Result<Option<TelegramCredentialsStatus>, ApplicationError>>,
    },
    ActiveTelegramCredentials {
        reply: SyncSender<Result<Option<credentials::ActiveTelegramCredentials>, ApplicationError>>,
    },
    SetTelegramCredentials {
        api_id: i32,
        api_hash: zeroize::Zeroizing<String>,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    ClearTelegramCredentials {
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    VaultMetadata {
        reply: SyncSender<Result<Option<VaultMetadataRecord>, ApplicationError>>,
    },
    SaveVaultMetadata {
        record: VaultMetadataRecord,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    SaveTelegramSources {
        account: AccountRecord,
        chats: Vec<ChatRecord>,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    UpsertRemoteFiles {
        files: Vec<RemoteFileUpsert>,
        reply: SyncSender<Result<u64, ApplicationError>>,
    },
    CachedTelegramFiles {
        account_id: teleark_core::AccountId,
        chat_id: teleark_core::ChatId,
        limit: usize,
        reply: SyncSender<Result<Vec<CachedTelegramFileRecord>, ApplicationError>>,
    },
    TelegramIndexState {
        account_id: teleark_core::AccountId,
        chat_id: teleark_core::ChatId,
        reply: SyncSender<Result<Option<TelegramIndexStateRecord>, ApplicationError>>,
    },
    SaveTelegramIndexState {
        state: TelegramIndexStateRecord,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    InsertNativeDownload {
        task: NewNativeDownloadTaskRecord,
        reply: SyncSender<Result<NativeDownloadTaskRecord, ApplicationError>>,
    },
    InsertNativeDownloadBatch {
        batch: NewNativeDownloadBatchRecord,
        tasks: Vec<NewNativeDownloadTaskRecord>,
        reply: SyncSender<
            Result<(NativeDownloadBatchRecord, Vec<NativeDownloadTaskRecord>), ApplicationError>,
        >,
    },
    SaveNativeDownload {
        task: NativeDownloadTaskRecord,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    RecordVaultDownload {
        record: teleark_storage::VaultDownloadRecord,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    DownloadedFilesPage {
        account_id: i64,
        after: Option<DownloadedFilesCursor>,
        reply: SyncSender<Result<Vec<DownloadedFileRecord>, ApplicationError>>,
    },
    NativeDownloads {
        reply: SyncSender<Result<Vec<NativeDownloadTaskRecord>, ApplicationError>>,
    },
    DeleteNativeDownload {
        task_id: u64,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    Shutdown,
}

impl StorageWorker {
    fn open(path: PathBuf) -> Result<Self, ApplicationError> {
        let (sender, receiver) = mpsc::sync_channel(STORAGE_QUEUE_CAPACITY);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("teleark-storage".to_owned())
            .spawn(move || match Database::open(&path) {
                Ok(database) => {
                    let _ = ready_sender.send(Ok(()));
                    storage_loop(database, path, receiver);
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(map_storage_error(error)));
                }
            })
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                inner: Arc::new(WorkerInner {
                    sender,
                    join: Mutex::new(Some(join)),
                }),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(_) => {
                let _ = join.join();
                Err(ApplicationError::new(ApplicationErrorKind::Persistence))
            }
        }
    }

    fn request<T>(
        &self,
        operation: &'static str,
        build: impl FnOnce(SyncSender<Result<T, ApplicationError>>) -> StorageRequest,
    ) -> Result<T, ApplicationError> {
        let started = Instant::now();
        let (reply, response) = mpsc::sync_channel(1);
        let result = self
            .inner
            .sender
            .send(build(reply))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))
            .and_then(|()| {
                response
                    .recv()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))
            })
            .and_then(|result| result);
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &result {
            Ok(_) => tracing::debug!(
                event = "storage.operation.completed",
                operation,
                elapsed_ms,
                "storage operation completed"
            ),
            Err(error) => tracing::warn!(
                event = "storage.operation.failed",
                operation,
                elapsed_ms,
                error_kind = ?error.kind(),
                "storage operation failed"
            ),
        }
        result
    }

    fn locale_override(&self) -> Result<Option<String>, ApplicationError> {
        self.request("locale_override", |reply| StorageRequest::LocaleOverride {
            reply,
        })
    }

    fn set_locale_override(&self, locale: Option<&str>) -> Result<(), ApplicationError> {
        self.request("set_locale_override", |reply| {
            StorageRequest::SetLocaleOverride {
                locale: locale.map(str::to_owned),
                reply,
            }
        })
    }

    fn preferences(&self) -> Result<DesktopPreferences, ApplicationError> {
        self.request("preferences", |reply| StorageRequest::Preferences { reply })
    }

    fn set_preferences(&self, preferences: DesktopPreferences) -> Result<(), ApplicationError> {
        self.request("set_preferences", |reply| StorageRequest::SetPreferences {
            preferences,
            reply,
        })
    }

    fn insert_native_download(
        &self,
        task: NewNativeDownloadTaskRecord,
    ) -> Result<NativeDownloadTaskRecord, ApplicationError> {
        self.request("insert_native_download", |reply| {
            StorageRequest::InsertNativeDownload { task, reply }
        })
    }

    fn insert_native_download_batch(
        &self,
        batch: NewNativeDownloadBatchRecord,
        tasks: Vec<NewNativeDownloadTaskRecord>,
    ) -> Result<(NativeDownloadBatchRecord, Vec<NativeDownloadTaskRecord>), ApplicationError> {
        self.request("insert_native_download_batch", |reply| {
            StorageRequest::InsertNativeDownloadBatch {
                batch,
                tasks,
                reply,
            }
        })
    }

    fn save_native_download(&self, task: NativeDownloadTaskRecord) -> Result<(), ApplicationError> {
        self.request("save_native_download", |reply| {
            StorageRequest::SaveNativeDownload { task, reply }
        })
    }

    fn native_downloads(&self) -> Result<Vec<NativeDownloadTaskRecord>, ApplicationError> {
        self.request("native_downloads", |reply| {
            StorageRequest::NativeDownloads { reply }
        })
    }

    fn delete_native_download(&self, task_id: u64) -> Result<(), ApplicationError> {
        self.request("delete_native_download", |reply| {
            StorageRequest::DeleteNativeDownload { task_id, reply }
        })
    }

    fn managed_directories(&self) -> Result<ManagedDirectories, ApplicationError> {
        self.request("managed_directories", |reply| {
            StorageRequest::ManagedDirectories { reply }
        })
    }

    fn next_download_destination(
        &self,
        suggested_file_name: String,
    ) -> Result<PathBuf, ApplicationError> {
        self.request("next_download_destination", |reply| {
            StorageRequest::NextDownloadDestination {
                suggested_file_name,
                reply,
            }
        })
    }

    fn telegram_credentials_status(
        &self,
    ) -> Result<Option<TelegramCredentialsStatus>, ApplicationError> {
        self.request("telegram_credentials_status", |reply| {
            StorageRequest::TelegramCredentialsStatus { reply }
        })
    }

    fn active_telegram_credentials(
        &self,
    ) -> Result<Option<credentials::ActiveTelegramCredentials>, ApplicationError> {
        self.request("active_telegram_credentials", |reply| {
            StorageRequest::ActiveTelegramCredentials { reply }
        })
    }

    fn set_telegram_credentials(
        &self,
        api_id: i32,
        api_hash: &str,
    ) -> Result<(), ApplicationError> {
        self.request("set_telegram_credentials", |reply| {
            StorageRequest::SetTelegramCredentials {
                api_id,
                api_hash: zeroize::Zeroizing::new(api_hash.to_owned()),
                reply,
            }
        })
    }

    fn clear_telegram_credentials(&self) -> Result<(), ApplicationError> {
        self.request("clear_telegram_credentials", |reply| {
            StorageRequest::ClearTelegramCredentials { reply }
        })
    }

    fn vault_metadata(&self) -> Result<Option<VaultMetadataRecord>, ApplicationError> {
        self.request("vault_metadata", |reply| StorageRequest::VaultMetadata {
            reply,
        })
    }

    fn save_vault_metadata(&self, record: VaultMetadataRecord) -> Result<(), ApplicationError> {
        self.request("save_vault_metadata", |reply| {
            StorageRequest::SaveVaultMetadata { record, reply }
        })
    }

    fn save_telegram_sources(
        &self,
        account: &TelegramAccount,
        chats: &[TelegramChatSummary],
    ) -> Result<(), ApplicationError> {
        let now = system_time_unix_ms(SystemTime::now())
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let account = AccountRecord {
            id: teleark_core::AccountId::new(account.id),
            display_name: account.display_name.clone(),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let chats = chats
            .iter()
            .map(|chat| ChatRecord {
                account_id: account.id,
                id: teleark_core::ChatId::new(chat.id),
                title: if chat.name.trim().is_empty() {
                    chat.username.clone().unwrap_or_else(|| chat.id.to_string())
                } else {
                    chat.name.clone()
                },
                username: chat.username.clone(),
                updated_at_unix_ms: now,
            })
            .collect();
        self.request("save_telegram_sources", |reply| {
            StorageRequest::SaveTelegramSources {
                account,
                chats,
                reply,
            }
        })
    }

    fn upsert_remote_files(&self, files: Vec<RemoteFileUpsert>) -> Result<u64, ApplicationError> {
        self.request("upsert_remote_files", |reply| {
            StorageRequest::UpsertRemoteFiles { files, reply }
        })
    }

    fn cached_telegram_files(
        &self,
        account_id: teleark_core::AccountId,
        chat_id: teleark_core::ChatId,
        limit: usize,
    ) -> Result<Vec<CachedTelegramFileRecord>, ApplicationError> {
        self.request("cached_telegram_files", |reply| {
            StorageRequest::CachedTelegramFiles {
                account_id,
                chat_id,
                limit,
                reply,
            }
        })
    }

    fn telegram_index_state(
        &self,
        account_id: teleark_core::AccountId,
        chat_id: teleark_core::ChatId,
    ) -> Result<Option<TelegramIndexStateRecord>, ApplicationError> {
        self.request("telegram_index_state", |reply| {
            StorageRequest::TelegramIndexState {
                account_id,
                chat_id,
                reply,
            }
        })
    }

    fn save_telegram_index_state(
        &self,
        state: TelegramIndexStateRecord,
    ) -> Result<(), ApplicationError> {
        self.request("save_telegram_index_state", |reply| {
            StorageRequest::SaveTelegramIndexState { state, reply }
        })
    }
}

impl Drop for WorkerInner {
    fn drop(&mut self) {
        let _ = self.sender.send(StorageRequest::Shutdown);
        if let Ok(mut join) = self.join.lock()
            && let Some(join) = join.take()
        {
            let _ = join.join();
        }
    }
}

impl LibraryRepository for StorageWorker {
    fn search(&self, query: &LibraryQuery) -> Result<LibraryPage, ApplicationError> {
        self.request("search", |reply| StorageRequest::Search {
            query: query.clone(),
            reply,
        })
    }

    fn statistics(&self) -> Result<LibraryStatistics, ApplicationError> {
        self.request("statistics", |reply| StorageRequest::Statistics { reply })
    }

    fn import_local_file(
        &self,
        command: &ImportLocalFile,
    ) -> Result<LogicalFile, ApplicationError> {
        self.request("import", |reply| StorageRequest::Import {
            command: command.clone(),
            reply,
        })
    }

    fn file(&self, id: LogicalFileId) -> Result<Option<LogicalFile>, ApplicationError> {
        self.request("file", |reply| StorageRequest::File { id, reply })
    }

    fn delete_file(&self, id: LogicalFileId) -> Result<bool, ApplicationError> {
        self.request("delete", |reply| StorageRequest::Delete { id, reply })
    }
}

fn storage_loop(
    mut database: Database,
    database_path: PathBuf,
    receiver: mpsc::Receiver<StorageRequest>,
) {
    while let Ok(request) = receiver.recv() {
        match request {
            StorageRequest::StorageChannel { account_id, reply } => {
                let _ = reply.send(storage_channel::load_binding(&database, account_id));
            }
            StorageRequest::SaveStorageChannel {
                account_id,
                chat_id,
                reply,
            } => {
                let _ = reply.send(storage_channel::save_binding(
                    &mut database,
                    account_id,
                    chat_id,
                ));
            }
            StorageRequest::Search { query, reply } => {
                let _ = reply.send(search_database(&database, &query));
            }
            StorageRequest::Statistics { reply } => {
                let result = database
                    .library_statistics()
                    .map(|statistics| LibraryStatistics {
                        logical_file_count: statistics.logical_file_count,
                        logical_bytes: statistics.logical_bytes,
                        local_file_count: statistics.local_file_count,
                        remote_file_count: statistics.remote_file_count,
                        active_transfer_count: statistics.active_transfer_count,
                    })
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::Import { command, reply } => {
                let _ = reply.send(import_into_database(&mut database, &command));
            }
            StorageRequest::File { id, reply } => {
                let result = database
                    .logical_file(id)
                    .map_err(map_storage_error)
                    .and_then(|record| record.map(record_to_logical_file).transpose());
                let _ = reply.send(result);
            }
            StorageRequest::Delete { id, reply } => {
                let _ = reply.send(database.delete_logical_file(id).map_err(map_storage_error));
            }
            StorageRequest::LocaleOverride { reply } => {
                let result = database
                    .setting(LOCALE_OVERRIDE_SETTING_KEY)
                    .map(|setting| setting.map(|setting| setting.value))
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::SetLocaleOverride { locale, reply } => {
                let result = set_locale_override(&mut database, locale.as_deref());
                let _ = reply.send(result);
            }
            StorageRequest::Preferences { reply } => {
                let _ = reply.send(load_preferences(&database));
            }
            StorageRequest::SetPreferences { preferences, reply } => {
                let result = prepare_managed_directories(&database_path, &preferences)
                    .and_then(|_| store_preferences(&mut database, &preferences));
                let _ = reply.send(result);
            }
            StorageRequest::ManagedDirectories { reply } => {
                let result = load_preferences(&database).and_then(|preferences| {
                    prepare_managed_directories(&database_path, &preferences)
                });
                let _ = reply.send(result);
            }
            StorageRequest::NextDownloadDestination {
                suggested_file_name,
                reply,
            } => {
                let result = load_preferences(&database)
                    .and_then(|preferences| {
                        prepare_managed_directories(&database_path, &preferences)
                    })
                    .and_then(|directories| {
                        available_download_destination(&directories.downloads, &suggested_file_name)
                    });
                let _ = reply.send(result);
            }
            StorageRequest::TelegramCredentialsStatus { reply } => {
                let result = telegram_credentials_status(&database);
                let _ = reply.send(result);
            }
            StorageRequest::ActiveTelegramCredentials { reply } => {
                let result = active_telegram_credentials(&database);
                let _ = reply.send(result);
            }
            StorageRequest::SetTelegramCredentials {
                api_id,
                api_hash,
                reply,
            } => {
                let result = set_telegram_credentials(&mut database, api_id, &api_hash);
                let _ = reply.send(result);
            }
            StorageRequest::ClearTelegramCredentials { reply } => {
                let result = clear_telegram_credentials(&mut database);
                let _ = reply.send(result);
            }
            StorageRequest::VaultMetadata { reply } => {
                let result = database.vault_metadata().map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::SaveVaultMetadata { record, reply } => {
                let result = database
                    .save_vault_metadata(&record)
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::SaveTelegramSources {
                account,
                chats,
                reply,
            } => {
                let result = save_telegram_sources(&mut database, &account, &chats);
                let _ = reply.send(result);
            }
            StorageRequest::UpsertRemoteFiles { files, reply } => {
                let result = upsert_remote_files(&mut database, &files);
                let _ = reply.send(result);
            }
            StorageRequest::CachedTelegramFiles {
                account_id,
                chat_id,
                limit,
                reply,
            } => {
                let result = database
                    .cached_telegram_files(account_id, chat_id, limit)
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::TelegramIndexState {
                account_id,
                chat_id,
                reply,
            } => {
                let result = database
                    .telegram_index_state(account_id, chat_id)
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::SaveTelegramIndexState { state, reply } => {
                let result = database
                    .save_telegram_index_state(&state)
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::InsertNativeDownload { task, reply } => {
                let result = database
                    .insert_native_download(&task)
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::InsertNativeDownloadBatch {
                batch,
                tasks,
                reply,
            } => {
                let result = database
                    .insert_native_download_batch(&batch, &tasks)
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::SaveNativeDownload { task, reply } => {
                let result = database
                    .save_native_download(&task)
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::ResolveLegacyDownloadAccounts { account_id, reply } => {
                let _ = reply.send(
                    database
                        .resolve_legacy_native_download_accounts(account_id)
                        .map_err(map_storage_error),
                );
            }
            StorageRequest::RecordVaultDownload { record, reply } => {
                let _ = reply.send(
                    database
                        .record_vault_download(&record)
                        .map_err(map_storage_error),
                );
            }
            StorageRequest::DownloadedFilesPage {
                account_id,
                after,
                reply,
            } => {
                let _ = reply.send(
                    database
                        .downloaded_files_page(account_id, after)
                        .map_err(map_storage_error),
                );
            }
            StorageRequest::NativeDownloads { reply } => {
                let result = database.native_downloads().map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::DeleteNativeDownload { task_id, reply } => {
                let result = database
                    .delete_native_download(task_id)
                    .map_err(map_storage_error);
                let _ = reply.send(result);
            }
            StorageRequest::Shutdown => break,
        }
    }
}

fn save_telegram_sources(
    database: &mut Database,
    account: &AccountRecord,
    chats: &[ChatRecord],
) -> Result<(), ApplicationError> {
    database
        .upsert_account(account)
        .map_err(map_storage_error)?;
    for chat in chats {
        database.upsert_chat(chat).map_err(map_storage_error)?;
    }
    Ok(())
}

fn upsert_remote_files(
    database: &mut Database,
    files: &[RemoteFileUpsert],
) -> Result<u64, ApplicationError> {
    for file in files {
        database
            .upsert_remote_file(file)
            .map_err(map_storage_error)?;
    }
    u64::try_from(files.len()).map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))
}

fn telegram_file_upserts(
    account_id: teleark_core::AccountId,
    chat_id: teleark_core::ChatId,
    files: &[TelegramFileSummary],
) -> Result<Vec<RemoteFileUpsert>, ApplicationError> {
    files
        .iter()
        .map(|file| {
            let revision = u64::try_from(file.modified_at_unix_ms.max(0))
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
            Ok(RemoteFileUpsert {
                account_id,
                chat_id,
                message_id: teleark_core::MessageId::new(file.message_id),
                revision,
                remote_key: file.message_id.to_be_bytes().to_vec(),
                name: if file.file_name.trim().is_empty() {
                    format!("telegram-document-{}", file.message_id)
                } else {
                    file.file_name.clone()
                },
                size_bytes: file.size_bytes,
                kind: classify_file(Path::new(&file.file_name)),
                mime_type: file.mime_type.clone(),
                caption: (!file.caption.is_empty()).then(|| file.caption.clone()),
                sent_at_unix_ms: file.sent_at_unix_ms,
                modified_at_unix_ms: file.modified_at_unix_ms,
            })
        })
        .collect()
}

fn cached_file_summary(file: CachedTelegramFileRecord) -> TelegramFileSummary {
    TelegramFileSummary {
        message_id: file.message_id.get(),
        sent_at_unix_ms: file.sent_at_unix_ms,
        modified_at_unix_ms: file.modified_at_unix_ms,
        file_name: file.file_name,
        caption: file.caption.unwrap_or_default(),
        mime_type: file.mime_type,
        size_bytes: file.size_bytes,
    }
}

fn search_database(
    database: &Database,
    query: &LibraryQuery,
) -> Result<LibraryPage, ApplicationError> {
    if query.sort != LibrarySort::ModifiedNewest || query.filter.kinds.len() > 1 {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    let cursor = query
        .after
        .clone()
        .map(PageCursor::parse)
        .transpose()
        .map_err(map_storage_error)?;
    let storage_query = SearchQuery {
        text: (!query.text.trim().is_empty()).then(|| query.text.clone()),
        facets: FileSearchFacets {
            account_id: query.filter.source_account_id,
            chat_id: query.filter.source_chat_id,
            kind: query.filter.kinds.first().copied(),
            minimum_size_bytes: query.filter.minimum_size_bytes,
            maximum_size_bytes: query.filter.maximum_size_bytes,
            remote_state: query.filter.remote_state,
            verification_state: query.filter.verification_state,
            ..FileSearchFacets::default()
        },
        cursor,
        limit: u32::try_from(query.page_size)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?,
    };
    let mut page = database
        .search_files(&storage_query)
        .map_err(map_storage_error)?;
    let items = page
        .files
        .into_iter()
        .map(|record| {
            let source = page.sources.remove(&record.id);
            record_to_library_item(record, source)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LibraryPage {
        items,
        next: page.next_cursor.map(|cursor| cursor.as_str().to_owned()),
        total_matching: page.total_matching,
    })
}

fn record_to_library_item(
    record: LogicalFileRecord,
    source: Option<teleark_storage::LibrarySourceRecord>,
) -> Result<LibraryItem, ApplicationError> {
    let local_source_path = record.local_source_path.clone();
    let part_count = u32::from(record.package_id.is_none());
    record_to_logical_file(record).map(|file| LibraryItem {
        file,
        source_message_id: source.as_ref().and_then(|source| source.message_id),
        source_name: source.map(|source| source.name),
        local_source_path,
        part_count,
    })
}

fn set_locale_override(
    database: &mut Database,
    locale: Option<&str>,
) -> Result<(), ApplicationError> {
    match locale {
        Some(locale) => {
            let updated_at_unix_ms = system_time_unix_ms(SystemTime::now())
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            database
                .set_setting(&SettingRecord {
                    key: LOCALE_OVERRIDE_SETTING_KEY.to_owned(),
                    value: locale.to_owned(),
                    updated_at_unix_ms,
                })
                .map_err(map_storage_error)
        }
        None => database
            .delete_setting(LOCALE_OVERRIDE_SETTING_KEY)
            .map(|_| ())
            .map_err(map_storage_error),
    }
}

fn load_preferences(database: &Database) -> Result<DesktopPreferences, ApplicationError> {
    let mut preferences = DesktopPreferences::default();
    let mut managed_root_seen = false;
    let mut legacy_download_directory = None;
    for setting in database.settings().map_err(map_storage_error)? {
        let Some(key) = setting.key.strip_prefix(PREFERENCE_PREFIX) else {
            continue;
        };
        match key {
            "managed_files_root" => {
                managed_root_seen = true;
                preferences.managed_files_root = if setting.value.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(setting.value))
                };
            }
            "download_directory" => {
                legacy_download_directory = if setting.value.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(setting.value))
                };
            }
            // Retained only so databases written by 0.2.0 remain readable.
            // Downloads are always automatic in the managed Downloads folder.
            "ask_download_destination" => {}
            "reveal_completed_downloads" => {
                preferences.reveal_completed_downloads = parse_bool_setting(&setting.value)?;
            }
            "upload_part_size_mib" => {
                preferences.upload_part_size_mib = setting
                    .value
                    .parse()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            }
            "upload_encrypt_content" => {
                preferences.upload_encrypt_content = parse_bool_setting(&setting.value)?;
            }
            "upload_hide_file_name" => {
                preferences.upload_hide_file_name = parse_bool_setting(&setting.value)?;
            }
            "upload_encrypt_metadata" => {
                preferences.upload_encrypt_metadata = parse_bool_setting(&setting.value)?;
            }
            "download_throughput_strategy" => {
                preferences.download_throughput_strategy = match setting.value.as_str() {
                    "balanced" => DownloadThroughputStrategy::Balanced,
                    "max_throughput" => DownloadThroughputStrategy::MaxThroughput,
                    _ => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
                };
            }
            "transfer_soft_limit_policy" => {
                preferences.transfer_soft_limit_policy = match setting.value.as_str() {
                    "respect" => SoftLimitPolicy::Respect,
                    "adaptive_override" => SoftLimitPolicy::AdaptiveOverride,
                    "ignore" => SoftLimitPolicy::Ignore,
                    _ => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
                };
            }
            "lock_vault_when_hidden" => {
                preferences.lock_vault_when_hidden = parse_bool_setting(&setting.value)?;
            }
            "index_batch_size" => {
                preferences.index_batch_size = setting
                    .value
                    .parse()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            }
            "notify_download_completed" => {
                preferences.notify_download_completed = parse_bool_setting(&setting.value)?;
            }
            "notify_download_failed" => {
                preferences.notify_download_failed = parse_bool_setting(&setting.value)?;
            }
            "sidebar_collapsed" => {
                preferences.sidebar_collapsed = parse_bool_setting(&setting.value)?;
            }
            "appearance" => {
                preferences.appearance = match setting.value.as_str() {
                    "system" => AppearancePreference::System,
                    "light" => AppearancePreference::Light,
                    "dark" => AppearancePreference::Dark,
                    _ => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
                };
            }
            _ => {}
        }
    }
    if !managed_root_seen {
        preferences.managed_files_root = legacy_download_directory.map(|directory| {
            let is_downloads_directory = directory
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("downloads"));
            if is_downloads_directory {
                directory
                    .parent()
                    .map_or(directory.clone(), Path::to_path_buf)
            } else {
                directory
            }
        });
    }
    validate_preferences(&preferences)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    Ok(preferences)
}

fn store_preferences(
    database: &mut Database,
    preferences: &DesktopPreferences,
) -> Result<(), ApplicationError> {
    validate_preferences(preferences)?;
    let updated_at_unix_ms = system_time_unix_ms(SystemTime::now())
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    let managed_root = preferences
        .managed_files_root
        .as_ref()
        .map(|path| {
            path.to_str()
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))
        })
        .transpose()?
        .unwrap_or_default();
    let appearance = match preferences.appearance {
        AppearancePreference::System => "system",
        AppearancePreference::Light => "light",
        AppearancePreference::Dark => "dark",
    };
    let transfer_soft_limit_policy = match preferences.transfer_soft_limit_policy {
        SoftLimitPolicy::Respect => "respect",
        SoftLimitPolicy::AdaptiveOverride => "adaptive_override",
        SoftLimitPolicy::Ignore => "ignore",
    };
    let download_strategy = match preferences.download_throughput_strategy {
        DownloadThroughputStrategy::Balanced => "balanced",
        DownloadThroughputStrategy::MaxThroughput => "max_throughput",
    };
    let values = [
        (
            "sidebar_collapsed",
            bool_setting(preferences.sidebar_collapsed),
        ),
        ("download_throughput_strategy", download_strategy.to_owned()),
        ("managed_files_root", managed_root.to_owned()),
        // Clear the retired values so older releases cannot reopen a stale
        // per-download prompt configuration after this version has run.
        ("download_directory", String::new()),
        ("ask_download_destination", bool_setting(false)),
        (
            "reveal_completed_downloads",
            bool_setting(preferences.reveal_completed_downloads),
        ),
        (
            "upload_part_size_mib",
            preferences.upload_part_size_mib.to_string(),
        ),
        (
            "upload_encrypt_content",
            bool_setting(preferences.upload_encrypt_content),
        ),
        (
            "upload_hide_file_name",
            bool_setting(preferences.upload_hide_file_name),
        ),
        (
            "upload_encrypt_metadata",
            bool_setting(preferences.upload_encrypt_metadata),
        ),
        (
            "transfer_soft_limit_policy",
            transfer_soft_limit_policy.to_owned(),
        ),
        (
            "lock_vault_when_hidden",
            bool_setting(preferences.lock_vault_when_hidden),
        ),
        ("index_batch_size", preferences.index_batch_size.to_string()),
        (
            "notify_download_completed",
            bool_setting(preferences.notify_download_completed),
        ),
        (
            "notify_download_failed",
            bool_setting(preferences.notify_download_failed),
        ),
        ("appearance", appearance.to_owned()),
    ];
    let settings = values.map(|(key, value)| SettingRecord {
        key: format!("{PREFERENCE_PREFIX}{key}"),
        value,
        updated_at_unix_ms,
    });
    database.set_settings(&settings).map_err(map_storage_error)
}

fn validate_preferences(preferences: &DesktopPreferences) -> Result<(), ApplicationError> {
    if !matches!(preferences.upload_part_size_mib, 1_024 | 1_900)
        || !matches!(preferences.index_batch_size, 200 | 500 | 1_000)
        || preferences.managed_files_root.as_ref().is_some_and(|path| {
            path.as_os_str().is_empty() || path.to_str().is_none() || !path.is_absolute()
        })
    {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok(())
}

fn parse_bool_setting(value: &str) -> Result<bool, ApplicationError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
    }
}

fn bool_setting(value: bool) -> String {
    value.to_string()
}

fn telegram_credentials_status(
    database: &Database,
) -> Result<Option<TelegramCredentialsStatus>, ApplicationError> {
    active_telegram_credentials(database).map(|credentials| {
        credentials.map(|credentials| TelegramCredentialsStatus {
            api_id: credentials.api_id,
            source: TelegramCredentialSource::User,
        })
    })
}

fn active_telegram_credentials(
    database: &Database,
) -> Result<Option<credentials::ActiveTelegramCredentials>, ApplicationError> {
    let api_id = database
        .setting(TELEGRAM_API_ID_SETTING_KEY)
        .map_err(map_storage_error)?;
    let api_hash = database
        .setting(TELEGRAM_API_HASH_SETTING_KEY)
        .map_err(map_storage_error)?;
    match (api_id, api_hash) {
        (None, None) => Ok(None),
        (Some(api_id), Some(api_hash)) => {
            let api_id = api_id
                .value
                .parse::<i32>()
                .ok()
                .filter(|api_id| *api_id > 0)
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            credentials::validate_api_hash(&api_hash.value)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            Ok(Some(credentials::ActiveTelegramCredentials {
                api_id,
                api_hash: zeroize::Zeroizing::new(api_hash.value),
            }))
        }
        _ => Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
    }
}

fn set_telegram_credentials(
    database: &mut Database,
    api_id: i32,
    api_hash: &str,
) -> Result<(), ApplicationError> {
    if api_id <= 0 {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    credentials::validate_api_hash(api_hash)?;
    let updated_at_unix_ms = system_time_unix_ms(SystemTime::now())
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    database
        .set_settings(&[
            SettingRecord {
                key: TELEGRAM_API_ID_SETTING_KEY.to_owned(),
                value: api_id.to_string(),
                updated_at_unix_ms,
            },
            SettingRecord {
                key: TELEGRAM_API_HASH_SETTING_KEY.to_owned(),
                value: api_hash.to_owned(),
                updated_at_unix_ms,
            },
        ])
        .map_err(map_storage_error)
}

fn clear_telegram_credentials(database: &mut Database) -> Result<(), ApplicationError> {
    database
        .delete_settings(&[TELEGRAM_API_ID_SETTING_KEY, TELEGRAM_API_HASH_SETTING_KEY])
        .map(|_| ())
        .map_err(map_storage_error)
}

fn import_into_database(
    database: &mut Database,
    command: &ImportLocalFile,
) -> Result<LogicalFile, ApplicationError> {
    let input = NewLogicalFileRecord::local_import(
        command.source_path.clone(),
        command.name.clone(),
        command.size_bytes,
        command.kind,
        command.modified_at_unix_ms,
    );
    let record = database
        .insert_logical_file(&input)
        .map_err(map_storage_error)?;
    record_to_logical_file(record)
}

fn record_to_logical_file(record: LogicalFileRecord) -> Result<LogicalFile, ApplicationError> {
    let file = LogicalFile {
        id: record.id,
        name: record.name,
        size_bytes: record.size_bytes,
        kind: record.kind,
        source_account_id: record.source_account_id,
        source_chat_id: record.source_chat_id,
        modified_at_unix_ms: record.modified_at_unix_ms,
        remote_state: record.remote_state,
        encryption_state: record.encryption_state,
        verification_state: record.verification_state,
        package_id: record.package_id,
    };
    file.validate()
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    Ok(file)
}

fn inspect_local_file(path: PathBuf) -> Result<ImportLocalFile, ApplicationError> {
    let metadata = std::fs::metadata(&path).map_err(map_io_error)?;
    if !metadata.is_file() {
        return Err(ApplicationError::new(ApplicationErrorKind::SourceMissing));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?
        .to_owned();
    let modified_at_unix_ms = metadata.modified().ok().and_then(system_time_unix_ms);
    Ok(ImportLocalFile {
        source_path: path.clone(),
        name,
        size_bytes: metadata.len(),
        kind: classify_file(&path),
        modified_at_unix_ms,
    })
}

fn prepare_database_parent(path: &Path) -> Result<(), ApplicationError> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(map_io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(map_io_error)?;
    }
    Ok(())
}

fn system_time_unix_ms(time: SystemTime) -> Option<i64> {
    let millis = time.duration_since(UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(millis).ok()
}

fn map_io_error(error: std::io::Error) -> ApplicationError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ApplicationError::new(ApplicationErrorKind::SourceMissing),
        std::io::ErrorKind::PermissionDenied => {
            ApplicationError::new(ApplicationErrorKind::PermissionDenied)
        }
        _ => ApplicationError::new(ApplicationErrorKind::Persistence),
    }
}

fn map_storage_error(error: StorageError) -> ApplicationError {
    match error {
        StorageError::InvalidInput { .. }
        | StorageError::InvalidCursor(_)
        | StorageError::Invariant(_)
        | StorageError::Domain(_) => ApplicationError::new(ApplicationErrorKind::InvalidRequest),
        StorageError::NotFound { .. } => ApplicationError::new(ApplicationErrorKind::NotFound),
        StorageError::Sqlite(_)
        | StorageError::UnsupportedSchema { .. }
        | StorageError::WrongApplication { .. }
        | StorageError::CorruptData { .. } => {
            ApplicationError::new(ApplicationErrorKind::Persistence)
        }
        _ => ApplicationError::new(ApplicationErrorKind::Persistence),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_is_case_insensitive_and_preserves_unknowns() {
        assert_eq!(classify_file(Path::new("Movie.MKV")), FileKind::Video);
        assert_eq!(
            classify_file(Path::new("archive.tar.gz")),
            FileKind::Archive
        );
        assert_eq!(classify_file(Path::new("no-extension")), FileKind::Other);
    }

    #[test]
    fn imported_file_is_persistent_and_searchable() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("library.sqlite3");
        let source_path = directory.path().join("example.pdf");
        std::fs::write(&source_path, b"persistent data").expect("write source fixture");

        let library = DesktopLibrary::open(&database_path).expect("open library");
        let outcomes = library.import_paths([source_path.clone()]);
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].is_ok());
        let statistics = library.statistics().expect("library statistics");
        assert_eq!(statistics.logical_file_count, 1);
        assert_eq!(statistics.logical_bytes, 15);
        assert_eq!(statistics.local_file_count, 1);
        drop(library);

        let reopened = DesktopLibrary::open(&database_path).expect("reopen library");
        let page = reopened
            .search(&LibraryQuery {
                text: "example.pdf".to_owned(),
                ..LibraryQuery::default()
            })
            .expect("search imported files");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total_matching, 1);
        assert_eq!(page.items[0].file.name, "example.pdf");
        assert_eq!(page.items[0].source_name, None);
        assert_eq!(page.items[0].source_message_id, None);
        assert_eq!(
            page.items[0].local_source_path.as_deref(),
            Some(source_path.as_path())
        );
    }

    #[test]
    fn locale_override_persists_and_system_default_removes_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("library.sqlite3");
        let library = DesktopLibrary::open(&database_path).expect("open library");

        assert_eq!(library.locale_override().expect("read setting"), None);
        library
            .set_locale_override(Some("ja-JP"))
            .expect("persist override");
        assert_eq!(
            library.locale_override().expect("read override"),
            Some("ja-JP".to_owned())
        );
        drop(library);

        let reopened = DesktopLibrary::open(&database_path).expect("reopen library");
        assert_eq!(
            reopened.locale_override().expect("read persisted override"),
            Some("ja-JP".to_owned())
        );
        reopened
            .set_locale_override(None)
            .expect("restore system default");
        assert_eq!(reopened.locale_override().expect("read default"), None);
    }

    #[test]
    fn throughput_preference_accepts_legacy_missing_and_rejects_unknown_values() {
        let directory = tempfile::tempdir().expect("valid test fixture");
        let mut database = Database::open(directory.path().join("preferences.sqlite3"))
            .expect("valid test fixture");
        database
            .set_setting(&SettingRecord {
                key: "preferences.v1.transfer_soft_limit_policy".to_owned(),
                value: "ignore".to_owned(),
                updated_at_unix_ms: 0,
            })
            .expect("valid test fixture");
        let legacy = load_preferences(&database).expect("valid test fixture");
        assert_eq!(
            legacy.download_throughput_strategy,
            DownloadThroughputStrategy::Balanced
        );
        assert_eq!(legacy.transfer_soft_limit_policy, SoftLimitPolicy::Ignore);
        database
            .set_setting(&SettingRecord {
                key: "preferences.v1.download_throughput_strategy".to_owned(),
                value: "unlimited".to_owned(),
                updated_at_unix_ms: 0,
            })
            .expect("valid test fixture");
        assert_eq!(
            load_preferences(&database)
                .expect_err("fixture must fail")
                .kind(),
            ApplicationErrorKind::Persistence
        );
    }

    #[test]
    fn desktop_preferences_round_trip_as_one_validated_versioned_set() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("library.sqlite3");
        let managed_files_root = directory.path().join("managed-files");
        let library = DesktopLibrary::open(&database_path).expect("open library");
        assert_eq!(
            library.preferences().expect("read defaults"),
            DesktopPreferences::default()
        );

        let preferences = DesktopPreferences {
            managed_files_root: Some(managed_files_root.clone()),
            reveal_completed_downloads: true,
            upload_part_size_mib: 1_024,
            upload_encrypt_content: false,
            upload_hide_file_name: false,
            upload_encrypt_metadata: false,
            transfer_soft_limit_policy: SoftLimitPolicy::Ignore,
            download_throughput_strategy: DownloadThroughputStrategy::MaxThroughput,
            lock_vault_when_hidden: false,
            index_batch_size: 500,
            notify_download_completed: false,
            notify_download_failed: false,
            appearance: AppearancePreference::Dark,
            sidebar_collapsed: false,
        };
        library
            .set_preferences(&preferences)
            .expect("persist preferences");
        drop(library);

        let reopened = DesktopLibrary::open(&database_path).expect("reopen library");
        assert_eq!(
            reopened.preferences().expect("read preferences"),
            preferences
        );

        let invalid = DesktopPreferences {
            index_batch_size: 999,
            ..DesktopPreferences::default()
        };
        assert_eq!(
            reopened
                .set_preferences(&invalid)
                .expect_err("invalid preference must fail")
                .kind(),
            ApplicationErrorKind::InvalidRequest
        );
        assert_eq!(
            reopened.preferences().expect("unchanged preferences"),
            preferences
        );

        let relative_root = DesktopPreferences {
            managed_files_root: Some(PathBuf::from("relative-root")),
            ..DesktopPreferences::default()
        };
        assert_eq!(
            reopened
                .set_preferences(&relative_root)
                .expect_err("relative managed root must fail")
                .kind(),
            ApplicationErrorKind::InvalidRequest
        );
    }

    #[test]
    fn managed_directories_are_created_together_and_download_names_never_overwrite() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("database/library.sqlite3");
        let managed_root = directory.path().join("user-managed");
        let library = DesktopLibrary::open(&database_path).expect("open library");
        library
            .set_preferences(&DesktopPreferences {
                managed_files_root: Some(managed_root.clone()),
                ..DesktopPreferences::default()
            })
            .expect("save managed root");

        let directories = library.managed_directories().expect("managed directories");
        assert_eq!(directories.root, managed_root);
        assert_eq!(directories.downloads, managed_root.join("Downloads"));
        assert_eq!(directories.cache, managed_root.join("Cache"));
        assert_eq!(directories.logs, managed_root.join("Logs"));
        assert!(directories.downloads.is_dir());
        assert!(directories.cache.is_dir());
        assert!(directories.logs.is_dir());

        let first = library
            .next_download_destination("report.pdf")
            .expect("first destination");
        assert_eq!(first, directories.downloads.join("report.pdf"));
        std::fs::write(&first, b"existing").expect("write collision fixture");
        let second = library
            .next_download_destination("report.pdf")
            .expect("collision-safe destination");
        assert_eq!(second, directories.downloads.join("report (1).pdf"));
    }

    #[test]
    fn managed_storage_metrics_include_managed_files_and_report_destination_capacity() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("database/library.sqlite3");
        let managed_root = directory.path().join("managed");
        let library = DesktopLibrary::open(&database_path).expect("open library");
        library
            .set_preferences(&DesktopPreferences {
                managed_files_root: Some(managed_root),
                ..DesktopPreferences::default()
            })
            .expect("save managed root");
        let directories = library.managed_directories().expect("managed directories");
        std::fs::write(directories.downloads.join("download.bin"), [0_u8; 7])
            .expect("write managed download");
        std::fs::write(directories.cache.join("cache.bin"), [0_u8; 5])
            .expect("write managed cache");

        let metrics = library
            .managed_storage_metrics()
            .expect("managed storage metrics");
        assert!(metrics.app_used_bytes >= 12);
        assert!(metrics.available_bytes > 0);
    }

    #[cfg(unix)]
    #[test]
    fn managed_storage_scan_does_not_follow_symbolic_links() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("root");
        let outside = directory.path().join("outside.bin");
        std::fs::create_dir(&root).expect("create scan root");
        std::fs::write(root.join("inside.bin"), [0_u8; 3]).expect("write inside file");
        std::fs::write(&outside, [0_u8; 100]).expect("write outside file");
        symlink(outside, root.join("linked.bin")).expect("create symbolic link");
        let mut remaining = 100;
        assert_eq!(
            directory_size_bounded(&root, &mut remaining).expect("bounded scan"),
            3
        );
    }

    #[test]
    fn legacy_download_directory_becomes_the_managed_root() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("library.sqlite3");
        let legacy_root = directory.path().join("legacy-root");
        let legacy_directory = legacy_root.join("Downloads");
        let library = DesktopLibrary::open(&database_path).expect("open library");
        drop(library);

        let mut database = Database::open(&database_path).expect("open storage fixture");
        database
            .set_setting(&SettingRecord {
                key: format!("{PREFERENCE_PREFIX}download_directory"),
                value: legacy_directory.to_string_lossy().into_owned(),
                updated_at_unix_ms: 1,
            })
            .expect("write legacy preference");
        database
            .set_setting(&SettingRecord {
                key: format!("{PREFERENCE_PREFIX}ask_download_destination"),
                value: "true".to_owned(),
                updated_at_unix_ms: 1,
            })
            .expect("write retired prompt preference");
        drop(database);

        let reopened = DesktopLibrary::open(&database_path).expect("reopen library");
        assert_eq!(
            reopened
                .preferences()
                .expect("migrated preferences")
                .managed_files_root,
            Some(legacy_root.clone())
        );
        assert_eq!(
            reopened
                .next_download_destination("legacy.bin")
                .expect("automatic destination"),
            legacy_directory.join("legacy.bin")
        );
    }

    #[test]
    fn malformed_known_preference_fails_closed_instead_of_silently_changing_behavior() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("library.sqlite3");
        let library = DesktopLibrary::open(&database_path).expect("open library");
        drop(library);

        let mut database = Database::open(&database_path).expect("open storage fixture");
        database
            .set_setting(&SettingRecord {
                key: format!("{PREFERENCE_PREFIX}index_batch_size"),
                value: "999".to_owned(),
                updated_at_unix_ms: 1,
            })
            .expect("write malformed known preference");
        drop(database);

        let reopened = DesktopLibrary::open(&database_path).expect("reopen library");
        assert_eq!(
            reopened
                .preferences()
                .expect_err("malformed persisted value must fail")
                .kind(),
            ApplicationErrorKind::Persistence
        );
    }

    #[test]
    fn telegram_credentials_persist_as_a_pair_clear_and_reject_invalid_values() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("library.sqlite3");
        let library = DesktopLibrary::open(&database_path).expect("open library");
        let api_hash = "0123456789abcdef0123456789abcdef";

        assert_eq!(
            library
                .telegram_credentials_status()
                .expect("read empty credential status"),
            None
        );
        let error = library
            .set_telegram_credentials(0, api_hash)
            .expect_err("zero API ID must be rejected");
        assert_eq!(error.kind(), ApplicationErrorKind::InvalidRequest);
        let error = library
            .set_telegram_credentials(12_345, "short")
            .expect_err("malformed API Hash must be rejected");
        assert_eq!(error.kind(), ApplicationErrorKind::InvalidRequest);

        library
            .set_telegram_credentials(12_345, api_hash)
            .expect("persist credential pair");
        assert_eq!(
            library
                .telegram_credentials_status()
                .expect("read saved credential status"),
            Some(TelegramCredentialsStatus {
                api_id: 12_345,
                source: TelegramCredentialSource::User,
            })
        );
        let active = library
            .stored_telegram_credentials()
            .expect("load saved credential pair")
            .expect("saved credential pair");
        assert_eq!(active.api_id, 12_345);
        assert_eq!(active.api_hash.as_str(), api_hash);
        assert!(!format!("{active:?}").contains(api_hash));
        drop(library);

        let reopened = DesktopLibrary::open(&database_path).expect("reopen library");
        assert_eq!(
            reopened
                .telegram_credentials_status()
                .expect("read persisted credential status"),
            Some(TelegramCredentialsStatus {
                api_id: 12_345,
                source: TelegramCredentialSource::User,
            })
        );
        reopened
            .clear_telegram_credentials()
            .expect("clear persisted credential pair");
        assert_eq!(
            reopened
                .telegram_credentials_status()
                .expect("read cleared credentials"),
            None
        );
        drop(reopened);

        let cleared = DesktopLibrary::open(&database_path).expect("reopen cleared library");
        assert_eq!(
            cleared
                .telegram_credentials_status()
                .expect("read cleared credentials"),
            None
        );
    }

    #[test]
    fn interactive_telegram_files_are_cached_without_index_coverage() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library =
            DesktopLibrary::open(directory.path().join("library.sqlite3")).expect("open library");
        let account = TelegramAccount {
            id: 11,
            display_name: "fixture account".to_owned(),
            username: None,
        };
        let chat = TelegramChatSummary {
            id: 22,
            name: "fixture chat".to_owned(),
            username: None,
            kind: teleark_telegram::TelegramChatKind::Channel,
        };
        library
            .save_telegram_sources(&account, std::slice::from_ref(&chat))
            .expect("save source identity");
        let file = TelegramFileSummary {
            message_id: 33,
            sent_at_unix_ms: 1_000,
            modified_at_unix_ms: 1_100,
            file_name: "cached.pdf".to_owned(),
            caption: "cached caption".to_owned(),
            mime_type: Some("application/pdf".to_owned()),
            size_bytes: 44,
        };
        assert_eq!(
            library
                .cache_telegram_files(account.id, chat.id, std::slice::from_ref(&file))
                .expect("cache browsed file"),
            1
        );
        assert_eq!(
            library
                .cached_telegram_files(account.id, chat.id, 5_000)
                .expect("read cached file"),
            vec![file.clone()]
        );
        let page = library
            .search(&LibraryQuery::default())
            .expect("query catalog");
        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_eq!(item.source_name.as_deref(), Some(chat.name.as_str()));
        assert_eq!(
            item.source_message_id.map(|id| id.get()),
            Some(file.message_id)
        );
        assert_eq!(
            item.file.source_account_id.map(|id| id.get()),
            Some(account.id)
        );
        assert_eq!(item.file.source_chat_id.map(|id| id.get()), Some(chat.id));
        assert!(
            library
                .worker
                .native_downloads()
                .expect("download history")
                .is_empty()
        );
        assert_eq!(
            library
                .worker
                .telegram_index_state(
                    teleark_core::AccountId::new(account.id),
                    teleark_core::ChatId::new(chat.id),
                )
                .expect("read index state"),
            None
        );
    }

    #[test]
    fn incomplete_or_corrupt_persisted_credentials_fail_closed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("library.sqlite3");
        let mut database = Database::open(&database_path).expect("open database");
        database
            .set_setting(&SettingRecord {
                key: TELEGRAM_API_ID_SETTING_KEY.to_owned(),
                value: "12345".to_owned(),
                updated_at_unix_ms: 1,
            })
            .expect("write deliberately incomplete pair");
        drop(database);

        let library = DesktopLibrary::open(&database_path).expect("open library");
        assert_eq!(
            library
                .telegram_credentials_status()
                .expect_err("incomplete credentials must not enable login")
                .kind(),
            ApplicationErrorKind::Persistence
        );
    }

    #[test]
    fn malformed_page_requests_are_rejected_before_sql() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library =
            DesktopLibrary::open(directory.path().join("library.sqlite3")).expect("open library");
        let error = library
            .search(&LibraryQuery {
                sort: LibrarySort::NameAscending,
                ..LibraryQuery::default()
            })
            .expect_err("unsupported sort must fail structurally");
        assert_eq!(error.kind(), ApplicationErrorKind::InvalidRequest);
    }
}
