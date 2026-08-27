//! Frontend-neutral desktop composition for TeleArk.
//!
//! This crate owns adapter worker lifecycles so frontends never execute SQL or
//! other blocking infrastructure work directly.

use std::sync::mpsc::SyncSender;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use teleark_core::{
    ApplicationError, ApplicationErrorKind, FileKind, ImportLocalFile, LibraryItem, LibraryPage,
    LibraryQuery, LibraryRepository, LibraryService, LibrarySort, LibraryStatistics, LogicalFile,
    LogicalFileId,
};
use teleark_storage::{
    AccountRecord, ChatRecord, Database, FileSearchFacets, LogicalFileRecord, NewLogicalFileRecord,
    PageCursor, RemoteFileUpsert, SearchQuery, SettingRecord, StorageError,
    TelegramIndexStateRecord,
};
use teleark_telegram::TelegramAccount;

mod telegram;
mod transfer;

pub use telegram::{
    DesktopTelegram, TelegramAuthState, TelegramChatSummary, TelegramFilePage, TelegramFileSummary,
    default_telegram_session_path,
};
pub use transfer::{
    EncryptedRemoteTransport, ManifestPublishRequest, ManifestRecoveryReport, ProductionTransferIo,
    RecoveredManifest, RejectedManifest, RemoteByteObject, RemoteObjectStore,
    SqliteCheckpointStore, TelegramObjectStore, encrypted_part_sizes, recover_remote_manifests,
};

const STORAGE_QUEUE_CAPACITY: usize = 64;
const LOCALE_OVERRIDE_SETTING_KEY: &str = "locale.override";

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

    /// Persists the authorized account and its currently visible dialogs.
    pub fn save_telegram_sources(
        &self,
        account: &TelegramAccount,
        chats: &[TelegramChatSummary],
    ) -> Result<(), ApplicationError> {
        self.worker.save_telegram_sources(account, chats)
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
        let page = telegram.scan_file_page(chat_id.get(), durable_before, limit)?;
        let files = page
            .files
            .iter()
            .map(|file| RemoteFileUpsert {
                account_id,
                chat_id,
                message_id: teleark_core::MessageId::new(file.message_id),
                revision: u64::try_from(file.modified_at_unix_ms.max(0)).unwrap_or_default(),
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
                modified_at_unix_ms: file.modified_at_unix_ms,
            })
            .collect();
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
    SaveTelegramSources {
        account: AccountRecord,
        chats: Vec<ChatRecord>,
        reply: SyncSender<Result<(), ApplicationError>>,
    },
    UpsertRemoteFiles {
        files: Vec<RemoteFileUpsert>,
        reply: SyncSender<Result<u64, ApplicationError>>,
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
    Shutdown,
}

impl StorageWorker {
    fn open(path: PathBuf) -> Result<Self, ApplicationError> {
        let (sender, receiver) = mpsc::sync_channel(STORAGE_QUEUE_CAPACITY);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("teleark-storage".to_owned())
            .spawn(move || match Database::open(path) {
                Ok(database) => {
                    let _ = ready_sender.send(Ok(()));
                    storage_loop(database, receiver);
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
        build: impl FnOnce(SyncSender<Result<T, ApplicationError>>) -> StorageRequest,
    ) -> Result<T, ApplicationError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.inner
            .sender
            .send(build(reply))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        response
            .recv()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
    }

    fn locale_override(&self) -> Result<Option<String>, ApplicationError> {
        self.request(|reply| StorageRequest::LocaleOverride { reply })
    }

    fn set_locale_override(&self, locale: Option<&str>) -> Result<(), ApplicationError> {
        self.request(|reply| StorageRequest::SetLocaleOverride {
            locale: locale.map(str::to_owned),
            reply,
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
        self.request(|reply| StorageRequest::SaveTelegramSources {
            account,
            chats,
            reply,
        })
    }

    fn upsert_remote_files(&self, files: Vec<RemoteFileUpsert>) -> Result<u64, ApplicationError> {
        self.request(|reply| StorageRequest::UpsertRemoteFiles { files, reply })
    }

    fn telegram_index_state(
        &self,
        account_id: teleark_core::AccountId,
        chat_id: teleark_core::ChatId,
    ) -> Result<Option<TelegramIndexStateRecord>, ApplicationError> {
        self.request(|reply| StorageRequest::TelegramIndexState {
            account_id,
            chat_id,
            reply,
        })
    }

    fn save_telegram_index_state(
        &self,
        state: TelegramIndexStateRecord,
    ) -> Result<(), ApplicationError> {
        self.request(|reply| StorageRequest::SaveTelegramIndexState { state, reply })
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
        self.request(|reply| StorageRequest::Search {
            query: query.clone(),
            reply,
        })
    }

    fn statistics(&self) -> Result<LibraryStatistics, ApplicationError> {
        self.request(|reply| StorageRequest::Statistics { reply })
    }

    fn import_local_file(
        &self,
        command: &ImportLocalFile,
    ) -> Result<LogicalFile, ApplicationError> {
        self.request(|reply| StorageRequest::Import {
            command: command.clone(),
            reply,
        })
    }

    fn file(&self, id: LogicalFileId) -> Result<Option<LogicalFile>, ApplicationError> {
        self.request(|reply| StorageRequest::File { id, reply })
    }

    fn delete_file(&self, id: LogicalFileId) -> Result<bool, ApplicationError> {
        self.request(|reply| StorageRequest::Delete { id, reply })
    }
}

fn storage_loop(mut database: Database, receiver: mpsc::Receiver<StorageRequest>) {
    while let Ok(request) = receiver.recv() {
        match request {
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
    let page = database
        .search_files(&storage_query)
        .map_err(map_storage_error)?;
    let items = page
        .files
        .into_iter()
        .map(record_to_library_item)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LibraryPage {
        items,
        next: page.next_cursor.map(|cursor| cursor.as_str().to_owned()),
        total_matching: page.total_matching,
    })
}

fn record_to_library_item(record: LogicalFileRecord) -> Result<LibraryItem, ApplicationError> {
    let local_source_path = record.local_source_path.clone();
    let part_count = u32::from(record.package_id.is_none());
    record_to_logical_file(record).map(|file| LibraryItem {
        file,
        source_name: None,
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
