mod dispatch;
pub(crate) mod lifecycle;
mod network_owner;
use teleark_telegram::network::{NetworkMonitor, NetworkRoute};

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use teleark_core::{ApplicationError, ApplicationErrorKind, FileKind};
use teleark_telegram::{
    ByteTransferObserver, DownloadObserver, PasswordChallenge, PasswordOutcome, PendingLogin,
    QrLoginOutcome, ScanCancellation, SignInOutcome, TelegramAccount, TelegramChat,
    TelegramChatKind, TelegramConfig, TelegramConnection, TelegramError, TelegramErrorKind,
};
use zeroize::Zeroizing;

use crate::{
    DesktopLibrary, ManagedStorageChannel, StorageChannelStatus, TelegramCredentialSource,
    TelegramCredentialsStatus,
    credentials::{ActiveTelegramCredentials, distribution_credentials},
};

const TELEGRAM_QUEUE_CAPACITY: usize = 32;
const MAX_DIALOGS: usize = 10_000;
const MAX_BATCH_SCAN_MESSAGES: usize = 50_000;
const MAX_BATCH_FILES: usize = 2_000;
const SCAN_PAGE_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(test)]
pub(crate) mod test_vault_remote;

pub type TelegramScanCancellation = ScanCancellation;

pub fn default_telegram_session_path() -> Option<PathBuf> {
    super::default_database_path().map(|path| path.with_file_name("telegram.session"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramChatSummary {
    pub id: i64,
    pub name: String,
    pub username: Option<String>,
    pub kind: TelegramChatKind,
    pub sync_pts: Option<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramFileSummary {
    pub message_id: i64,
    pub sent_at_unix_ms: i64,
    pub modified_at_unix_ms: i64,
    pub file_name: String,
    pub caption: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramFilePage {
    pub files: Vec<TelegramFileSummary>,
    pub next_before_message_id: Option<i64>,
    pub exhausted: bool,
    pub examined_messages: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TelegramFileFilter {
    pub after_unix_ms: Option<i64>,
    pub before_unix_ms: Option<i64>,
    pub kind: Option<FileKind>,
}

#[derive(Clone, Eq, PartialEq)]
pub enum TelegramAuthState {
    Disconnected,
    Unauthorized,
    QrCode {
        deep_link: String,
        expires_at_unix_seconds: i64,
    },
    CodeSent,
    PasswordRequired {
        hint: Option<String>,
    },
    Authorized(TelegramAccount),
}

impl std::fmt::Debug for TelegramAuthState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => formatter.write_str("Disconnected"),
            Self::Unauthorized => formatter.write_str("Unauthorized"),
            Self::QrCode {
                expires_at_unix_seconds,
                ..
            } => formatter
                .debug_struct("QrCode")
                .field("deep_link", &"[REDACTED]")
                .field("expires_at_unix_seconds", expires_at_unix_seconds)
                .finish(),
            Self::CodeSent => formatter.write_str("CodeSent"),
            Self::PasswordRequired { hint } => formatter
                .debug_struct("PasswordRequired")
                .field("hint", hint)
                .finish(),
            Self::Authorized(account) => {
                formatter.debug_tuple("Authorized").field(account).finish()
            }
        }
    }
}

#[derive(Clone)]
pub struct DesktopTelegram {
    #[cfg(test)]
    test_vault_remote: Option<Arc<test_vault_remote::TestVaultRemote>>,
    inner: Arc<TelegramWorkerInner>,
}

struct TelegramWorkerInner {
    lifecycle: lifecycle::Lifecycle,
    endpoint: Mutex<Option<network_owner::Endpoint>>,
    bandwidth: teleark_telegram::TransferBandwidth,
    changing: std::sync::atomic::AtomicBool,
    route: Mutex<NetworkRoute>,
    monitor: NetworkMonitor,
    active_probe: Mutex<Option<(u64, ScanCancellation)>>,
    session_path: PathBuf,
}

enum TelegramRequest {
    TestProxy {
        cancellation: ScanCancellation,
        reply: mpsc::SyncSender<Result<Duration, ApplicationError>>,
    },
    SyncSources {
        account_id: i64,
        cancellation: TelegramScanCancellation,
        reply: mpsc::SyncSender<
            Result<
                Result<Vec<TelegramChatSummary>, crate::channel_sync::ChannelSyncFailure>,
                ApplicationError,
            >,
        >,
    },
    ChannelSignals {
        reply: mpsc::SyncSender<Result<teleark_telegram::ChannelUpdateSignals, ApplicationError>>,
    },
    SyncChannel {
        account_id: i64,
        chat_id: i64,
        request: crate::channel_sync::ChannelRead,
        cancellation: TelegramScanCancellation,
        reply: mpsc::SyncSender<
            Result<
                Result<
                    crate::channel_sync::ChannelReadPage,
                    crate::channel_sync::ChannelSyncFailure,
                >,
                ApplicationError,
            >,
        >,
    },
    DiscoverStorage {
        account_id: i64,
        preferred: Option<i64>,
        create: Option<(String, String)>,
        reply: mpsc::SyncSender<Result<(StorageChannelStatus, bool), ApplicationError>>,
    },
    MaintainStorage {
        account_id: i64,
        chat_id: i64,
        title: String,
        description: String,
        random_id: i64,
        archive: bool,
        progress: crate::StorageMaintenance,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    ValidateStorage {
        account_id: i64,
        chat_id: i64,
        discover: bool,
        reply: mpsc::SyncSender<Result<TelegramChatSummary, ApplicationError>>,
    },
    AccountAvatar {
        account_id: i64,
        reply: mpsc::SyncSender<Result<Option<Vec<u8>>, ApplicationError>>,
    },
    Connect {
        api_id: i32,
        session_path: PathBuf,
        reply: mpsc::SyncSender<Result<TelegramAuthState, ApplicationError>>,
    },
    RequestCode {
        phone: String,
        api_hash: Zeroizing<String>,
        reply: mpsc::SyncSender<Result<TelegramAuthState, ApplicationError>>,
    },
    BeginQrLogin {
        api_hash: Zeroizing<String>,
        reply: mpsc::SyncSender<Result<TelegramAuthState, ApplicationError>>,
    },
    PollQrLogin {
        reply: mpsc::SyncSender<Result<TelegramAuthState, ApplicationError>>,
    },
    SubmitCode {
        code: String,
        reply: mpsc::SyncSender<Result<TelegramAuthState, ApplicationError>>,
    },
    SubmitPassword {
        password: Vec<u8>,
        reply: mpsc::SyncSender<Result<TelegramAuthState, ApplicationError>>,
    },
    ListDialogs {
        account_id: i64,
        cancellation: TelegramScanCancellation,
        reply: mpsc::SyncSender<Result<Vec<TelegramChatSummary>, ApplicationError>>,
    },
    ScanPage {
        account_id: i64,
        chat_id: i64,
        before_message_id: Option<i64>,
        limit: usize,
        cancellation: TelegramScanCancellation,
        reply: mpsc::SyncSender<Result<TelegramFilePage, ApplicationError>>,
    },
    ScanFilteredFiles {
        account_id: i64,
        chat_id: i64,
        filter: TelegramFileFilter,
        reply: mpsc::SyncSender<Result<Vec<TelegramFileSummary>, ApplicationError>>,
    },
    Download {
        account_id: Option<i64>,
        chat_id: i64,
        message_id: i64,
        destination: PathBuf,
        observer: Option<Arc<dyn DownloadObserver>>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    DownloadBytes {
        observer: Option<Arc<dyn ByteTransferObserver>>,
        account_id: i64,
        chat_id: i64,
        message_id: i64,
        cancellation: Option<TelegramScanCancellation>,
        reply: mpsc::SyncSender<Result<Vec<u8>, ApplicationError>>,
    },
    SearchFiles {
        account_id: i64,
        chat_id: i64,
        caption: String,
        limit: usize,
        cancellation: Option<TelegramScanCancellation>,
        reply: mpsc::SyncSender<Result<Vec<TelegramFileSummary>, ApplicationError>>,
    },
    UploadStream {
        account_id: i64,
        chat_id: i64,
        file_name: String,
        caption: String,
        stream: teleark_telegram::UploadStream,
        options: teleark_telegram::StreamUploadOptions,
        cancellation: Option<TelegramScanCancellation>,
        reply: mpsc::SyncSender<Result<i64, ApplicationError>>,
    },
    UploadBytes {
        publication_random_id: Option<i64>,
        observer: Option<Arc<dyn ByteTransferObserver>>,
        cancellation: Option<TelegramScanCancellation>,
        account_id: i64,
        chat_id: i64,
        file_name: String,
        caption: String,
        bytes: Vec<u8>,
        reply: mpsc::SyncSender<Result<i64, ApplicationError>>,
    },
    SignOut {
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    Shutdown,
}

enum LoginState {
    None,
    Qr {
        api_hash: Zeroizing<String>,
        deep_link: String,
        expires_at_unix_seconds: i64,
    },
    Code(PendingLogin),
    Password(Box<PasswordChallenge>),
}

struct WorkerState {
    lifecycle: lifecycle::Lifecycle,
    bandwidth: teleark_telegram::TransferBandwidth,
    network_route: NetworkRoute,
    network_monitor: NetworkMonitor,
    network_generation: u64,
    connection: Option<Arc<TelegramConnection>>,
    api_id: Option<i32>,
    authorized_account_id: Option<i64>,
    login: LoginState,
    chats: Arc<BTreeMap<i64, TelegramChat>>,
    storage_creation_guard: crate::storage_channel::StorageCreationGuard,
    storage_binding: Option<(i64, i64)>,
}

impl Default for WorkerState {
    fn default() -> Self {
        Self {
            lifecycle: lifecycle::Lifecycle::default(),
            bandwidth: teleark_telegram::TransferBandwidth::default(),
            network_route: NetworkRoute::Direct,
            network_monitor: NetworkMonitor::new(&NetworkRoute::Direct),
            network_generation: 0,
            connection: None,
            api_id: None,
            authorized_account_id: None,
            login: LoginState::None,
            chats: Arc::new(BTreeMap::new()),
            storage_creation_guard: Default::default(),
            storage_binding: None,
        }
    }
}

impl DesktopTelegram {
    pub(crate) fn sync_sources(
        &self,
        account_id: i64,
        cancellation: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, crate::channel_sync::ChannelSyncFailure> {
        self.request("sync_sources", |reply| TelegramRequest::SyncSources {
            account_id,
            cancellation,
            reply,
        })
        .map_err(crate::channel_sync::ChannelSyncFailure::from)?
    }
    pub(crate) fn channel_signals(
        &self,
    ) -> Result<teleark_telegram::ChannelUpdateSignals, ApplicationError> {
        self.request("channel_signals", |reply| TelegramRequest::ChannelSignals {
            reply,
        })
    }

    pub(crate) fn sync_channel(
        &self,
        account_id: i64,
        chat_id: i64,
        request: crate::channel_sync::ChannelRead,
        cancellation: TelegramScanCancellation,
    ) -> Result<crate::channel_sync::ChannelReadPage, crate::channel_sync::ChannelSyncFailure> {
        self.request("sync_channel", |reply| TelegramRequest::SyncChannel {
            account_id,
            chat_id,
            request,
            cancellation,
            reply,
        })
        .map_err(crate::channel_sync::ChannelSyncFailure::from)?
    }
    /// Finds the account's remote storage without creating or changing it.
    pub fn discover_storage_channel(
        &self,
        library: &DesktopLibrary,
        account_id: i64,
    ) -> Result<StorageChannelStatus, ApplicationError> {
        self.resolve_storage_channel(library, account_id, None)
    }

    /// Discovers and manages storage without asking the frontend to choose a
    /// channel. Creation follows a complete successful discovery only.
    pub fn ensure_storage_channel(
        &self,
        library: &DesktopLibrary,
        account_id: i64,
        title: String,
        description: String,
    ) -> Result<ManagedStorageChannel, ApplicationError> {
        let preferred = library.storage_channel_id(account_id)?;
        let (status, created) = self.request("ensure_storage_channel", |reply| {
            TelegramRequest::DiscoverStorage {
                account_id,
                preferred,
                create: Some((title, description)),
                reply,
            }
        })?;
        let (channel, health) = match status {
            StorageChannelStatus::Ready(channel) => (channel, crate::StorageChannelHealth::Healthy),
            StorageChannelStatus::Degraded { channel, health } => (channel, health),
            StorageChannelStatus::Unavailable { .. } => {
                return Err(ApplicationError::new(
                    ApplicationErrorKind::StorageAccessDenied,
                ));
            }
            _ => return Err(ApplicationError::new(ApplicationErrorKind::Conflict)),
        };
        library.save_storage_channel_id(account_id, channel.id)?;
        Ok(ManagedStorageChannel {
            channel,
            created,
            health,
        })
    }

    fn resolve_storage_channel(
        &self,
        library: &DesktopLibrary,
        account_id: i64,
        create: Option<(String, String)>,
    ) -> Result<StorageChannelStatus, ApplicationError> {
        let preferred = library.storage_channel_id(account_id)?;
        let (status, _) = self.request("resolve_storage_channel", |reply| {
            TelegramRequest::DiscoverStorage {
                account_id,
                preferred,
                create,
                reply,
            }
        })?;
        if let StorageChannelStatus::Ready(channel) = &status {
            library.save_storage_channel_id(account_id, channel.id)?;
        }
        Ok(status)
    }

    pub fn select_storage_channel(
        &self,
        library: &DesktopLibrary,
        account_id: i64,
        chat_id: i64,
    ) -> Result<TelegramChatSummary, ApplicationError> {
        let channel = self.validate_storage_channel(account_id, chat_id)?;
        library.save_storage_channel_id(account_id, chat_id)?;
        Ok(channel)
    }

    pub(crate) fn validate_storage_channel(
        &self,
        account_id: i64,
        chat_id: i64,
    ) -> Result<TelegramChatSummary, ApplicationError> {
        self.validate_storage_scope(account_id, chat_id, true)
    }

    pub(crate) fn validate_storage_target(
        &self,
        account_id: i64,
        chat_id: i64,
    ) -> Result<TelegramChatSummary, ApplicationError> {
        self.validate_storage_scope(account_id, chat_id, false)
    }

    fn validate_storage_scope(
        &self,
        account_id: i64,
        chat_id: i64,
        discover: bool,
    ) -> Result<TelegramChatSummary, ApplicationError> {
        self.request("validate_storage_channel", |reply| {
            TelegramRequest::ValidateStorage {
                account_id,
                chat_id,
                discover,
                reply,
            }
        })
    }

    pub fn maintain_storage_channel(
        &self,
        library: &DesktopLibrary,
        account_id: i64,
        title: String,
        description: String,
        archive: bool,
        progress: crate::StorageMaintenance,
    ) -> Result<(), ApplicationError> {
        let result = (|| {
            use teleark_crypto::RandomSource as _;
            progress.phase(crate::StorageMaintenancePhase::Checking);
            let chat_id = library
                .storage_channel_id(account_id)?
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::StorageAccessDenied))?;
            let mut bytes = [0; 8];
            teleark_crypto::OsRandom
                .fill_bytes(&mut bytes)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            let proposed = (i64::from_le_bytes(bytes) & i64::MAX).max(1);
            let random_id = if archive {
                proposed
            } else {
                library.worker.request("storage_repair_token", |reply| {
                    crate::StorageRequest::StorageRepairToken {
                        account: account_id,
                        chat: chat_id,
                        proposed,
                        clear: false,
                        reply,
                    }
                })?
            };
            self.request("maintain_storage_channel", |reply| {
                TelegramRequest::MaintainStorage {
                    account_id,
                    chat_id,
                    title,
                    description,
                    random_id,
                    archive,
                    progress: progress.clone(),
                    reply,
                }
            })?;
            if !archive {
                library.worker.request("storage_repair_complete", |reply| {
                    crate::StorageRequest::StorageRepairToken {
                        account: account_id,
                        chat: chat_id,
                        proposed: random_id,
                        clear: true,
                        reply,
                    }
                })?;
            }
            Ok(())
        })();
        progress.finish(result.as_ref().err().map(ApplicationError::kind));
        result
    }

    pub fn account_avatar(&self, account_id: i64) -> Result<Option<Vec<u8>>, ApplicationError> {
        self.request("account_avatar", |reply| TelegramRequest::AccountAvatar {
            account_id,
            reply,
        })
    }

    pub fn connect(&self, api_id: i32) -> Result<TelegramAuthState, ApplicationError> {
        let session_path = self.inner.session_path.clone();
        self.request("connect", |reply| TelegramRequest::Connect {
            api_id,
            session_path,
            reply,
        })
    }

    pub fn connect_configured(
        &self,
        library: &DesktopLibrary,
    ) -> Result<TelegramAuthState, ApplicationError> {
        let credentials = self.active_credentials(library)?;
        let state = self.connect(credentials.api_id)?;
        library.resolve_legacy_native_download_accounts(match &state {
            TelegramAuthState::Authorized(account) => Some(account.id),
            _ => None,
        })?;
        Ok(state)
    }

    /// Reports the credential pair that would be used for authentication.
    /// User-saved credentials take precedence over distributor-provided
    /// TeleArk release credentials.
    pub fn effective_credentials_status(
        &self,
        library: &DesktopLibrary,
    ) -> Result<Option<TelegramCredentialsStatus>, ApplicationError> {
        if let Some(status) = library.telegram_credentials_status()? {
            return Ok(Some(status));
        }
        distribution_credentials().map(|credentials| {
            credentials.map(|credentials| TelegramCredentialsStatus {
                api_id: credentials.api_id,
                source: TelegramCredentialSource::Distribution,
            })
        })
    }

    pub fn request_login_code_configured(
        &self,
        library: &DesktopLibrary,
        phone: impl Into<String>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        let credentials = self.active_credentials(library)?;
        self.request_login_code_secret(phone.into(), credentials.api_hash)
    }

    pub fn begin_qr_login_configured(
        &self,
        library: &DesktopLibrary,
    ) -> Result<TelegramAuthState, ApplicationError> {
        let credentials = self.active_credentials(library)?;
        self.begin_qr_login_secret(credentials.api_hash)
    }

    fn active_credentials(
        &self,
        library: &DesktopLibrary,
    ) -> Result<ActiveTelegramCredentials, ApplicationError> {
        if let Some(credentials) = library.stored_telegram_credentials()? {
            return Ok(credentials);
        }
        distribution_credentials()?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))
    }

    pub fn request_login_code(
        &self,
        phone: impl Into<String>,
        api_hash: impl Into<String>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        self.request_login_code_secret(phone.into(), Zeroizing::new(api_hash.into()))
    }

    fn request_login_code_secret(
        &self,
        phone: String,
        api_hash: Zeroizing<String>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        self.request("request_login_code", |reply| TelegramRequest::RequestCode {
            phone,
            api_hash,
            reply,
        })
    }

    pub fn begin_qr_login(
        &self,
        api_hash: impl Into<String>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        self.begin_qr_login_secret(Zeroizing::new(api_hash.into()))
    }

    fn begin_qr_login_secret(
        &self,
        api_hash: Zeroizing<String>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        self.request("begin_qr_login", |reply| TelegramRequest::BeginQrLogin {
            api_hash,
            reply,
        })
    }

    pub fn poll_qr_login(&self) -> Result<TelegramAuthState, ApplicationError> {
        self.request("poll_qr_login", |reply| TelegramRequest::PollQrLogin {
            reply,
        })
    }

    pub fn submit_code(
        &self,
        code: impl Into<String>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        self.request("submit_login_code", |reply| TelegramRequest::SubmitCode {
            code: code.into(),
            reply,
        })
    }

    pub fn submit_password(
        &self,
        password: impl Into<Vec<u8>>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        self.request("submit_password", |reply| TelegramRequest::SubmitPassword {
            password: password.into(),
            reply,
        })
    }

    pub fn list_dialogs(
        &self,
        account_id: i64,
    ) -> Result<Vec<TelegramChatSummary>, ApplicationError> {
        self.list_dialogs_cancellable(account_id, TelegramScanCancellation::new())
    }

    /// Complete catalog read; cancellation never publishes a partial roster.
    pub fn list_dialogs_cancellable(
        &self,
        account_id: i64,
        cancellation: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ApplicationError> {
        self.request("list_dialogs", |reply| TelegramRequest::ListDialogs {
            account_id,
            cancellation,
            reply,
        })
    }

    pub fn scan_file_page(
        &self,
        account_id: i64,
        chat_id: i64,
        before_message_id: Option<i64>,
        limit: usize,
    ) -> Result<TelegramFilePage, ApplicationError> {
        self.scan_file_page_cancellable(
            account_id,
            chat_id,
            before_message_id,
            limit,
            TelegramScanCancellation::new(),
        )
    }

    pub fn scan_file_page_cancellable(
        &self,
        account_id: i64,
        chat_id: i64,
        before_message_id: Option<i64>,
        limit: usize,
        cancellation: TelegramScanCancellation,
    ) -> Result<TelegramFilePage, ApplicationError> {
        self.request("scan_file_page", |reply| TelegramRequest::ScanPage {
            account_id,
            chat_id,
            before_message_id,
            limit,
            cancellation,
            reply,
        })
    }

    pub fn scan_filtered_files(
        &self,
        account_id: i64,
        chat_id: i64,
        filter: TelegramFileFilter,
    ) -> Result<Vec<TelegramFileSummary>, ApplicationError> {
        self.request("scan_filtered_files", |reply| {
            TelegramRequest::ScanFilteredFiles {
                account_id,
                chat_id,
                filter,
                reply,
            }
        })
    }

    pub fn download_file(
        &self,
        account_id: i64,
        chat_id: i64,
        message_id: i64,
        destination: impl AsRef<Path>,
    ) -> Result<(), ApplicationError> {
        self.request("download_file", |reply| TelegramRequest::Download {
            account_id: Some(account_id),
            chat_id,
            message_id,
            destination: destination.as_ref().to_owned(),
            observer: None,
            reply,
        })
    }

    pub(crate) fn download_file_observed(
        &self,
        account_id: i64,
        chat_id: i64,
        message_id: i64,
        destination: impl AsRef<Path>,
        observer: Arc<dyn DownloadObserver>,
    ) -> Result<(), ApplicationError> {
        self.request("download_file_observed", |reply| {
            TelegramRequest::Download {
                account_id: Some(account_id),
                chat_id,
                message_id,
                destination: destination.as_ref().to_owned(),
                observer: Some(observer),
                reply,
            }
        })
    }

    pub(crate) fn discard_partial_download(
        &self,
        destination: impl AsRef<Path>,
    ) -> Result<(), ApplicationError> {
        teleark_telegram::discard_partial_download(destination).map_err(map_telegram_error)
    }

    pub(crate) fn cleanup_failed_partial_download(
        &self,
        destination: impl AsRef<Path>,
        expected_bytes: u64,
        retain_for_resume: bool,
    ) -> Result<(), ApplicationError> {
        teleark_telegram::cleanup_failed_partial_download(
            destination,
            expected_bytes,
            retain_for_resume,
        )
        .map_err(map_telegram_error)
    }

    pub fn download_bytes(
        &self,
        account_id: i64,
        chat_id: i64,
        message_id: i64,
        cancellation: Option<TelegramScanCancellation>,
    ) -> Result<Vec<u8>, ApplicationError> {
        self.download_bytes_observed(account_id, chat_id, message_id, cancellation, None)
    }

    pub(crate) fn download_bytes_observed(
        &self,
        account_id: i64,
        chat_id: i64,
        message_id: i64,
        cancellation: Option<TelegramScanCancellation>,
        observer: Option<Arc<dyn ByteTransferObserver>>,
    ) -> Result<Vec<u8>, ApplicationError> {
        self.request("download_bytes", |reply| TelegramRequest::DownloadBytes {
            observer,
            account_id,
            chat_id,
            message_id,
            cancellation,
            reply,
        })
    }

    pub fn search_files_exact_caption(
        &self,
        account_id: i64,
        chat_id: i64,
        caption: impl Into<String>,
        limit: usize,
        cancellation: Option<TelegramScanCancellation>,
    ) -> Result<Vec<TelegramFileSummary>, ApplicationError> {
        self.request("search_files", |reply| TelegramRequest::SearchFiles {
            account_id,
            chat_id,
            caption: caption.into(),
            limit,
            cancellation,
            reply,
        })
    }

    pub fn upload_bytes(
        &self,
        account_id: i64,
        chat_id: i64,
        file_name: impl Into<String>,
        caption: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Result<i64, ApplicationError> {
        self.upload_bytes_observed(
            account_id,
            chat_id,
            file_name.into(),
            caption.into(),
            bytes,
            UploadByteOptions::default(),
        )
    }

    pub(crate) fn upload_bytes_observed(
        &self,
        account_id: i64,
        chat_id: i64,
        file_name: String,
        caption: String,
        bytes: Vec<u8>,
        options: UploadByteOptions,
    ) -> Result<i64, ApplicationError> {
        self.request("upload_bytes", |reply| TelegramRequest::UploadBytes {
            publication_random_id: options.publication_random_id,
            observer: options.observer,
            cancellation: options.cancellation,
            account_id,
            chat_id,
            file_name,
            caption,
            bytes,
            reply,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn upload_stream_observed(
        &self,
        account_id: i64,
        chat_id: i64,
        file_name: String,
        caption: String,
        stream: teleark_telegram::UploadStream,
        options: teleark_telegram::StreamUploadOptions,
        cancellation: Option<TelegramScanCancellation>,
    ) -> Result<i64, ApplicationError> {
        self.request("upload_stream", |reply| TelegramRequest::UploadStream {
            account_id,
            chat_id,
            file_name,
            caption,
            stream,
            options,
            cancellation,
            reply,
        })
    }

    pub fn sign_out(&self) -> Result<(), ApplicationError> {
        self.request("sign_out", |reply| TelegramRequest::SignOut { reply })
    }

    fn request<T>(
        &self,
        operation: &'static str,
        build: impl FnOnce(mpsc::SyncSender<Result<T, ApplicationError>>) -> TelegramRequest,
    ) -> Result<T, ApplicationError> {
        let started = Instant::now();
        let (reply, response) = mpsc::sync_channel(1);
        if self
            .inner
            .changing
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        #[cfg(test)]
        if let Some(remote) = &self.test_vault_remote {
            remote.handle(build(reply));
            return response
                .recv()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?;
        }
        let (sender, generation) = {
            let endpoint = self
                .inner
                .endpoint
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let endpoint = endpoint
                .as_ref()
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Network))?;
            (endpoint.sender.clone(), endpoint.generation)
        };
        let result = sender
            .blocking_send(build(reply))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))
            .and_then(|()| {
                response
                    .recv()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))
            })
            .and_then(|result| result);
        if self.inner.monitor.snapshot().generation != generation {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &result {
            Ok(_) => tracing::debug!(
                event = "telegram.operation.completed",
                operation,
                elapsed_ms,
                "Telegram operation completed"
            ),
            Err(error) => tracing::warn!(
                event = "telegram.operation.failed",
                operation,
                elapsed_ms,
                error_kind = ?error.kind(),
                "Telegram operation failed"
            ),
        }
        result
    }
}

impl WorkerState {
    fn record_auth_result(&mut self, result: &Result<TelegramAuthState, ApplicationError>) {
        self.authorized_account_id = match result {
            Ok(TelegramAuthState::Authorized(account)) => Some(account.id),
            _ => None,
        };
        self.lifecycle.publish(
            self.network_generation,
            self.authorized_account_id,
            self.connection
                .as_ref()
                .map(|connection| connection.channel_update_signals()),
        );
    }

    fn read_snapshot(&self) -> Self {
        Self {
            lifecycle: self.lifecycle.clone(),
            network_route: self.network_route.clone(),
            network_monitor: self.network_monitor.clone(),
            network_generation: self.network_generation,
            connection: self.connection.clone(),
            api_id: self.api_id,
            authorized_account_id: self.authorized_account_id,
            chats: Arc::clone(&self.chats),
            storage_binding: self.storage_binding,
            ..Self::default()
        }
    }
}

impl TelegramRequest {
    fn reject_capacity(self) {
        match self {
            Self::TestProxy { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::SyncSources { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::ChannelSignals { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::SyncChannel { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::DiscoverStorage { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::MaintainStorage { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::ValidateStorage { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::AccountAvatar { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::Connect { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::RequestCode { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::BeginQrLogin { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::PollQrLogin { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::SubmitCode { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::SubmitPassword { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::ListDialogs { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::ScanPage { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::ScanFilteredFiles { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::Download { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::DownloadBytes { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::SearchFiles { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::UploadStream { reply, .. } | Self::UploadBytes { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::SignOut { reply, .. } => {
                let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
            }
            Self::Shutdown => {}
        }
    }

    fn lane(&self) -> dispatch::Lane {
        use dispatch::Lane;
        match self {
            Self::TestProxy { .. } => Lane::Probe,
            Self::Download { .. }
            | Self::DownloadBytes { .. }
            | Self::UploadBytes { .. }
            | Self::UploadStream { .. } => Lane::Transfer,
            Self::ChannelSignals { .. }
            | Self::SyncChannel { .. }
            | Self::AccountAvatar { .. }
            | Self::ScanPage { .. }
            | Self::ScanFilteredFiles { .. }
            | Self::SearchFiles { .. } => Lane::Read,
            Self::SyncSources { .. }
            | Self::DiscoverStorage { .. }
            | Self::MaintainStorage { .. }
            | Self::ValidateStorage { .. }
            | Self::ListDialogs { .. } => Lane::Control,
            Self::Connect { .. }
            | Self::RequestCode { .. }
            | Self::BeginQrLogin { .. }
            | Self::PollQrLogin { .. }
            | Self::SubmitCode { .. }
            | Self::SubmitPassword { .. }
            | Self::SignOut { .. } => Lane::Barrier,
            Self::Shutdown => Lane::Shutdown,
        }
    }
}

async fn telegram_loop(receiver: tokio::sync::mpsc::Receiver<TelegramRequest>, state: WorkerState) {
    dispatch::run(
        receiver,
        state,
        WorkerState::read_snapshot,
        TelegramRequest::lane,
        |mut state, request, cancellation| async move {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {},
                _ = handle_request(&mut state, request) => {},
            }
            state
        },
        TelegramRequest::reject_capacity,
    )
    .await;
}

async fn handle_request(state: &mut WorkerState, request: TelegramRequest) {
    match request {
        TelegramRequest::TestProxy {
            reply,
            cancellation,
        } => {
            let result = teleark_telegram::network::test_proxy(
                &state.network_route,
                &state.network_monitor,
                state.network_generation,
                &cancellation,
            )
            .await
            .map_err(|reason| {
                ApplicationError::new(
                    if reason == teleark_telegram::network::ProxyFailure::Cancelled {
                        ApplicationErrorKind::Cancelled
                    } else {
                        ApplicationErrorKind::Network
                    },
                )
            });
            let _ = reply.send(result);
        }
        TelegramRequest::SyncSources {
            account_id,
            cancellation,
            reply,
        } => {
            let operation = async {
                require_account(state, account_id)
                    .map_err(crate::channel_sync::ChannelSyncFailure::from)?;
                let dialogs = connection_ref(state)
                    .map_err(crate::channel_sync::ChannelSyncFailure::from)?
                    .list_sync_dialogs(&cancellation)
                    .await
                    .map_err(|error| crate::channel_sync::ChannelSyncFailure {
                        retry_after: error.retry_after(),
                        kind: map_telegram_error(error).kind(),
                    })?;
                let summaries = dialogs.iter().map(chat_summary).collect();
                state.chats = Arc::new(dialogs.into_iter().map(|chat| (chat.id(), chat)).collect());
                Ok(summaries)
            };
            let result = tokio::select! {
                _ = cancellation.cancelled() => Err(crate::channel_sync::ChannelSyncFailure::from(ApplicationError::new(ApplicationErrorKind::Cancelled))),
                result = tokio::time::timeout(Duration::from_secs(60), operation) => result.unwrap_or_else(|_| Err(crate::channel_sync::ChannelSyncFailure::from(ApplicationError::new(ApplicationErrorKind::Network)))),
            };
            let _ = reply.send(Ok(result));
        }
        TelegramRequest::ChannelSignals { reply } => {
            let _ = reply.send(connection_ref(state).map(|c| c.channel_update_signals()));
        }
        TelegramRequest::SyncChannel {
            account_id,
            chat_id,
            request,
            cancellation,
            reply,
        } => {
            let operation = async {
                require_account(state, account_id)
                    .map_err(crate::channel_sync::ChannelSyncFailure::from)?;
                let chat = state.chats.get(&chat_id).ok_or_else(|| {
                    crate::channel_sync::ChannelSyncFailure::from(ApplicationError::new(
                        ApplicationErrorKind::NotFound,
                    ))
                })?;
                let connection =
                    connection_ref(state).map_err(crate::channel_sync::ChannelSyncFailure::from)?;
                channel_read(connection, chat, request, &cancellation).await
            };
            let result = tokio::select! {
                _ = cancellation.cancelled() => Err(crate::channel_sync::ChannelSyncFailure::from(ApplicationError::new(ApplicationErrorKind::Cancelled))),
                result = tokio::time::timeout(SCAN_PAGE_TIMEOUT, operation) => result.unwrap_or_else(|_| Err(crate::channel_sync::ChannelSyncFailure::from(ApplicationError::new(ApplicationErrorKind::Network)))),
            };
            let _ = reply.send(Ok(result));
        }
        TelegramRequest::DiscoverStorage {
            account_id,
            preferred,
            create,
            reply,
        } => {
            // A timeout abandons the actual RPC future. Creation is not
            // blindly retried: the guard permits only discovery after an
            // uncertain create, including when this timeout drops its future.
            let result = tokio::time::timeout(
                Duration::from_secs(60),
                discover_storage(state, account_id, preferred, create),
            )
            .await
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))
            .and_then(|r| r);
            let _ = reply.send(result);
        }
        TelegramRequest::MaintainStorage {
            account_id,
            chat_id,
            title,
            description,
            random_id,
            archive,
            progress,
            reply,
        } => {
            let result = tokio::select! {
                _ = progress.cancellation.cancelled() => Err(ApplicationError::new(ApplicationErrorKind::Cancelled)),
                result = tokio::time::timeout(Duration::from_secs(60), async {
                    require_account(state, account_id)?;
                    if state.storage_binding != Some((account_id, chat_id)) { return Err(ApplicationError::new(ApplicationErrorKind::StorageAccessDenied)); }
                    let chat = state.chats.get(&chat_id).ok_or_else(|| ApplicationError::new(ApplicationErrorKind::StorageAccessDenied))?;
                    let connection = connection_ref(state)?;
                    let observe = |phase| progress.phase(phase);
                    if archive {
                        connection.archive_bound_storage(chat, account_id, &progress.cancellation, &observe).await
                    } else {
                        connection.repair_bound_storage(chat, account_id, (&title, &description), random_id, &progress.cancellation, &observe).await
                    }.map_err(map_telegram_error)
                }) => result.unwrap_or_else(|_| Err(ApplicationError::new(ApplicationErrorKind::Network))),
            };
            let _ = reply.send(result);
        }
        TelegramRequest::ValidateStorage {
            account_id,
            chat_id,
            discover,
            reply,
        } => {
            let result = tokio::time::timeout(
                SCAN_PAGE_TIMEOUT,
                validate_storage(state, account_id, chat_id, discover),
            )
            .await
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))
            .and_then(|r| r);
            let _ = reply.send(result);
        }
        TelegramRequest::AccountAvatar { account_id, reply } => {
            let result =
                match require_account(state, account_id).and_then(|()| connection_ref(state)) {
                    Ok(connection) => {
                        tokio::time::timeout(Duration::from_secs(10), connection.account_avatar())
                            .await
                            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))
                            .and_then(|result| result.map_err(map_telegram_error))
                    }
                    Err(error) => Err(error),
                };
            let _ = reply.send(result);
        }
        TelegramRequest::Connect {
            api_id,
            session_path,
            reply,
        } => {
            state.authorized_account_id = None;
            let result = connect(state, api_id, session_path).await;
            state.record_auth_result(&result);
            let _ = reply.send(result);
        }
        TelegramRequest::RequestCode {
            phone,
            api_hash,
            reply,
        } => {
            state.authorized_account_id = None;
            let result = request_code(state, &phone, &api_hash).await;
            state.record_auth_result(&result);
            let _ = reply.send(result);
        }
        TelegramRequest::BeginQrLogin { api_hash, reply } => {
            state.authorized_account_id = None;
            let result = begin_qr_login(state, api_hash).await;
            state.record_auth_result(&result);
            let _ = reply.send(result);
        }
        TelegramRequest::PollQrLogin { reply } => {
            state.authorized_account_id = None;
            let result = poll_qr_login(state).await;
            state.record_auth_result(&result);
            let _ = reply.send(result);
        }
        TelegramRequest::SubmitCode { code, reply } => {
            state.authorized_account_id = None;
            let result = submit_code(state, &code).await;
            state.record_auth_result(&result);
            let _ = reply.send(result);
        }
        TelegramRequest::SubmitPassword { password, reply } => {
            state.authorized_account_id = None;
            let result = submit_password(state, &password).await;
            state.record_auth_result(&result);
            let _ = reply.send(result);
        }
        TelegramRequest::ListDialogs {
            account_id,
            cancellation,
            reply,
        } => {
            let result = match require_account(state, account_id) {
                Ok(()) => tokio::select! {
                    biased;
                    () = cancellation.cancelled() => Err(ApplicationError::new(ApplicationErrorKind::Cancelled)),
                    result = tokio::time::timeout(SCAN_PAGE_TIMEOUT, list_dialogs(state)) => result
                        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network)).and_then(|r| r),
                },
                Err(error) => Err(error),
            };
            let _ = reply.send(result);
        }
        TelegramRequest::ScanPage {
            account_id,
            chat_id,
            before_message_id,
            limit,
            cancellation,
            reply,
        } => {
            let result = scan_page(
                state,
                account_id,
                chat_id,
                before_message_id,
                limit,
                &cancellation,
            )
            .await;
            let _ = reply.send(result);
        }
        TelegramRequest::ScanFilteredFiles {
            account_id,
            chat_id,
            filter,
            reply,
        } => {
            let result = scan_filtered_files(state, account_id, chat_id, filter).await;
            let _ = reply.send(result);
        }
        TelegramRequest::Download {
            account_id,
            chat_id,
            message_id,
            destination,
            observer,
            reply,
        } => {
            let result = download(
                state,
                account_id,
                chat_id,
                message_id,
                destination,
                observer,
            )
            .await;
            let _ = reply.send(result);
        }
        TelegramRequest::DownloadBytes {
            observer,
            account_id,
            chat_id,
            message_id,
            cancellation,
            reply,
        } => {
            let operation =
                download_bytes(state, account_id, chat_id, message_id, observer.as_deref());
            let result = if let Some(cancellation) = cancellation {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => Err(ApplicationError::new(ApplicationErrorKind::Cancelled)),
                    result = operation => result,
                }
            } else {
                operation.await
            };
            let _ = reply.send(result);
        }
        TelegramRequest::SearchFiles {
            account_id,
            chat_id,
            caption,
            limit,
            cancellation,
            reply,
        } => {
            let operation = search_files(state, account_id, chat_id, &caption, limit);
            let result = if let Some(cancellation) = cancellation {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => Err(ApplicationError::new(ApplicationErrorKind::Cancelled)),
                    result = tokio::time::timeout(Duration::from_secs(30), operation) => result.unwrap_or_else(|_| Err(ApplicationError::new(ApplicationErrorKind::Network))),
                }
            } else {
                operation.await
            };
            let _ = reply.send(result);
        }
        TelegramRequest::UploadStream {
            account_id,
            chat_id,
            file_name,
            caption,
            stream,
            options,
            cancellation,
            reply,
        } => {
            let operation = async {
                require_account(state, account_id)?;
                let chat = state
                    .chats
                    .get(&chat_id)
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
                connection_ref(state)?
                    .upload_stream_document(chat, stream, &file_name, &caption, options)
                    .await
                    .map(|sent| sent.message_id)
                    .map_err(map_telegram_error)
            };
            let result = interruptible_upload(operation, cancellation.as_ref()).await;
            let _ = reply.send(result);
        }
        TelegramRequest::UploadBytes {
            publication_random_id,
            observer,
            cancellation,
            account_id,
            chat_id,
            file_name,
            caption,
            bytes,
            reply,
        } => {
            let options = UploadByteOptions {
                observer,
                cancellation,
                publication_random_id,
            };
            let operation = upload_bytes(
                state, account_id, chat_id, &file_name, &caption, &bytes, &options,
            );
            let result = interruptible_upload(operation, options.cancellation.as_ref()).await;
            let _ = reply.send(result);
        }
        TelegramRequest::SignOut { reply } => {
            state.authorized_account_id = None;
            state
                .lifecycle
                .publish(state.network_generation, None, None);
            let result = sign_out(state).await;
            let _ = reply.send(result);
        }
        TelegramRequest::Shutdown => {
            state.connection.take();
        }
    }
}

async fn connect(
    state: &mut WorkerState,
    api_id: i32,
    session_path: PathBuf,
) -> Result<TelegramAuthState, ApplicationError> {
    if let Some(connection) = state.connection.as_ref() {
        if state.api_id != Some(api_id) {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        return if connection
            .is_authorized()
            .await
            .map_err(map_telegram_error)?
        {
            Ok(TelegramAuthState::Authorized(
                connection
                    .current_account()
                    .await
                    .map_err(map_telegram_error)?,
            ))
        } else {
            Ok(TelegramAuthState::Unauthorized)
        };
    }
    let connection = TelegramConnection::connect(TelegramConfig {
        api_id,
        session_path,
        network_route: state.network_route.clone(),
        network_monitor: state.network_monitor.clone(),
        network_generation: state.network_generation,
    })
    .await
    .map_err(map_telegram_error)?
    .with_bandwidth(state.bandwidth.clone());
    let authorized = connection
        .is_authorized()
        .await
        .map_err(map_telegram_error)?;
    let result = if authorized {
        TelegramAuthState::Authorized(
            connection
                .current_account()
                .await
                .map_err(map_telegram_error)?,
        )
    } else {
        TelegramAuthState::Unauthorized
    };
    state.connection = Some(Arc::new(connection));
    state.api_id = Some(api_id);
    Ok(result)
}

async fn request_code(
    state: &mut WorkerState,
    phone: &str,
    api_hash: &str,
) -> Result<TelegramAuthState, ApplicationError> {
    let connection = connection(state)?;
    let pending = connection
        .request_login_code(phone, api_hash)
        .await
        .map_err(map_telegram_error)?;
    state.login = LoginState::Code(pending);
    Ok(TelegramAuthState::CodeSent)
}

async fn begin_qr_login(
    state: &mut WorkerState,
    api_hash: Zeroizing<String>,
) -> Result<TelegramAuthState, ApplicationError> {
    if api_hash.trim().is_empty() {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    let outcome = connection(state)?
        .export_qr_login(&api_hash, &[])
        .await
        .map_err(map_telegram_error)?;
    apply_qr_outcome(state, outcome, Some(api_hash))
}

async fn poll_qr_login(state: &mut WorkerState) -> Result<TelegramAuthState, ApplicationError> {
    let LoginState::Qr {
        api_hash,
        deep_link,
        expires_at_unix_seconds,
    } = &state.login
    else {
        return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
    };
    let api_hash = api_hash.clone();
    let existing = TelegramAuthState::QrCode {
        deep_link: deep_link.clone(),
        expires_at_unix_seconds: *expires_at_unix_seconds,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?
        .as_secs();
    let now =
        i64::try_from(now).map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
    let update_received = connection_ref(state)?.take_qr_login_update();
    if !should_refresh_qr(now, *expires_at_unix_seconds, update_received) {
        return Ok(existing);
    }
    let outcome = connection(state)?
        .export_qr_login(&api_hash, &[])
        .await
        .map_err(map_telegram_error)?;
    apply_qr_outcome(state, outcome, Some(api_hash))
}

fn should_refresh_qr(now: i64, expires_at: i64, update_received: bool) -> bool {
    update_received || now >= expires_at
}

fn apply_qr_outcome(
    state: &mut WorkerState,
    outcome: QrLoginOutcome,
    api_hash: Option<Zeroizing<String>>,
) -> Result<TelegramAuthState, ApplicationError> {
    match outcome {
        QrLoginOutcome::Pending(code) => {
            let api_hash = api_hash
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
            let deep_link = code.deep_link().to_owned();
            let expires_at_unix_seconds = code.expires_at_unix_seconds();
            state.login = LoginState::Qr {
                api_hash,
                deep_link: deep_link.clone(),
                expires_at_unix_seconds,
            };
            Ok(TelegramAuthState::QrCode {
                deep_link,
                expires_at_unix_seconds,
            })
        }
        QrLoginOutcome::Authorized(account) => {
            state.login = LoginState::None;
            Ok(TelegramAuthState::Authorized(account))
        }
    }
}

async fn submit_code(
    state: &mut WorkerState,
    code: &str,
) -> Result<TelegramAuthState, ApplicationError> {
    let LoginState::Code(pending) = std::mem::replace(&mut state.login, LoginState::None) else {
        return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
    };
    let outcome = connection(state)?.sign_in(&pending, code).await;
    match outcome.map_err(map_telegram_error)? {
        SignInOutcome::Authorized(account) => Ok(TelegramAuthState::Authorized(account)),
        SignInOutcome::PasswordRequired(challenge) => {
            let hint = challenge.hint().map(str::to_owned);
            state.login = LoginState::Password(challenge);
            Ok(TelegramAuthState::PasswordRequired { hint })
        }
    }
}

async fn submit_password(
    state: &mut WorkerState,
    password: &[u8],
) -> Result<TelegramAuthState, ApplicationError> {
    let LoginState::Password(challenge) = std::mem::replace(&mut state.login, LoginState::None)
    else {
        return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
    };
    match connection(state)?
        .check_password(*challenge, password)
        .await
        .map_err(map_telegram_error)?
    {
        PasswordOutcome::Authorized(account) => Ok(TelegramAuthState::Authorized(account)),
        PasswordOutcome::InvalidPassword(challenge) => {
            let hint = challenge.hint().map(str::to_owned);
            state.login = LoginState::Password(challenge);
            Ok(TelegramAuthState::PasswordRequired { hint })
        }
    }
}

async fn list_dialogs(
    state: &mut WorkerState,
) -> Result<Vec<TelegramChatSummary>, ApplicationError> {
    let dialogs = connection(state)?
        .list_dialogs(MAX_DIALOGS)
        .await
        .map_err(map_telegram_error)?;
    let summaries = dialogs
        .iter()
        .map(|chat| TelegramChatSummary {
            sync_pts: chat.sync_pts(),
            id: chat.id(),
            name: chat.name().to_owned(),
            username: chat.username().map(str::to_owned),
            kind: chat.kind(),
        })
        .collect();
    state.chats = Arc::new(dialogs.into_iter().map(|chat| (chat.id(), chat)).collect());
    Ok(summaries)
}

fn chat_summary(chat: &TelegramChat) -> TelegramChatSummary {
    TelegramChatSummary {
        sync_pts: chat.sync_pts(),
        id: chat.id(),
        name: chat.name().to_owned(),
        username: chat.username().map(str::to_owned),
        kind: chat.kind(),
    }
}

fn require_account(state: &WorkerState, account_id: i64) -> Result<(), ApplicationError> {
    // This is session-local routing identity established by successful authentication.
    // Telegram still authorizes each RPC, and private storage validates fresh remote
    // metadata/account-bound markers independently in the Telegram adapter.
    if state.authorized_account_id != Some(account_id) {
        return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
    }
    Ok(())
}

async fn discover_storage(
    state: &mut WorkerState,
    account_id: i64,
    preferred: Option<i64>,
    create: Option<(String, String)>,
) -> Result<(StorageChannelStatus, bool), ApplicationError> {
    require_account(state, account_id)?;
    let dialogs = connection_ref(state)?
        .list_dialogs(MAX_DIALOGS)
        .await
        .map_err(map_telegram_error)?;
    if let Some(chat_id) = preferred {
        state.storage_binding = Some((account_id, chat_id));
        state.chats = Arc::new(dialogs.into_iter().map(|chat| (chat.id(), chat)).collect());
        let Some(chat) = state.chats.get(&chat_id) else {
            return Ok((
                StorageChannelStatus::Unavailable {
                    chat_id,
                    candidates: Vec::new(),
                },
                false,
            ));
        };
        let health = connection_ref(state)?
            .storage_channel_health(chat, account_id)
            .await
            .map_err(map_telegram_error)?;
        let channel = chat_summary(chat);
        return Ok((
            if health == crate::StorageChannelHealth::Healthy {
                StorageChannelStatus::Ready(channel)
            } else {
                StorageChannelStatus::Degraded { channel, health }
            },
            false,
        ));
    }
    let candidates = connection_ref(state)?
        .discover_storage_channels(&dialogs)
        .await
        .map_err(map_telegram_error)?;
    let summaries = candidates.iter().map(chat_summary).collect();
    let status = if create.is_some() {
        crate::storage_channel::resolve_managed_storage_channel(preferred, summaries)
    } else {
        crate::storage_channel::resolve_storage_channel(preferred, summaries)
    };
    state.chats = Arc::new(dialogs.into_iter().map(|chat| (chat.id(), chat)).collect());
    if let (StorageChannelStatus::Ready(summary), Some((title, description))) = (&status, &create) {
        let chat = state
            .chats
            .get(&summary.id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        let channel = connection_ref(state)?
            .ensure_storage_branding(chat, title, description)
            .await
            .map_err(map_telegram_error)?;
        let summary = chat_summary(&channel);
        Arc::make_mut(&mut state.chats).insert(channel.id(), channel);
        state.storage_creation_guard.resolved(account_id);
        state.storage_binding = Some((account_id, summary.id));
        return Ok((StorageChannelStatus::Ready(summary), false));
    }
    if let (StorageChannelStatus::Missing, Some((title, description))) = (&status, create) {
        state.storage_creation_guard.begin(account_id)?;
        let channel = connection_ref(state)?
            .create_storage_channel(&title, &description)
            .await
            .map_err(map_telegram_error)?;
        Arc::make_mut(&mut state.chats).insert(channel.id(), channel.clone());
        // Creation is not atomic across devices. Re-discover before enabling
        // storage and reject competing remote candidates rather than choosing.
        let dialogs = connection_ref(state)?
            .list_dialogs(MAX_DIALOGS)
            .await
            .map_err(map_telegram_error)?;
        let candidates = connection_ref(state)?
            .discover_storage_channels(&dialogs)
            .await
            .map_err(map_telegram_error)?;
        if candidates.len() != 1 || candidates[0].id() != channel.id() {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let channel = connection_ref(state)?
            .ensure_storage_branding(&channel, &title, &description)
            .await
            .map_err(map_telegram_error)?;
        let summary = chat_summary(&channel);
        Arc::make_mut(&mut state.chats).insert(channel.id(), channel);
        state.storage_creation_guard.resolved(account_id);
        state.storage_binding = Some((account_id, summary.id));
        Ok((StorageChannelStatus::Ready(summary), true))
    } else {
        Ok((status, false))
    }
}

async fn validate_storage(
    state: &mut WorkerState,
    account_id: i64,
    chat_id: i64,
    discover: bool,
) -> Result<TelegramChatSummary, ApplicationError> {
    require_account(state, account_id)?;
    // Full discovery is needed only before a session has established its binding.
    // Every file still checks fresh owner/private metadata; no history scan per part.
    if state.storage_binding.is_none() && discover {
        let (status, _) = discover_storage(state, account_id, None, None).await?;
        if let Some(channel) = status.usable_channel() {
            state.storage_binding = Some((account_id, channel.id));
        }
    }
    if state.storage_binding != Some((account_id, chat_id)) {
        return Err(ApplicationError::new(
            ApplicationErrorKind::StorageAccessDenied,
        ));
    }
    let chat = state
        .chats
        .get(&chat_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::StorageAccessDenied))?;
    let health = connection_ref(state)?
        .storage_channel_health(chat, account_id)
        .await
        .map_err(map_telegram_error)?;
    if !health.permits_files() {
        return Err(ApplicationError::new(match health {
            crate::StorageChannelHealth::UnsafeConfiguration => {
                ApplicationErrorKind::StorageConfigurationUnsafe
            }
            crate::StorageChannelHealth::UnsupportedIdentity => {
                ApplicationErrorKind::StorageIdentityUnsupported
            }
            _ => ApplicationErrorKind::StorageAccessDenied,
        }));
    }
    Ok(chat_summary(chat))
}

async fn channel_read(
    connection: &TelegramConnection,
    chat: &TelegramChat,
    request: crate::channel_sync::ChannelRead,
    cancellation: &TelegramScanCancellation,
) -> Result<crate::channel_sync::ChannelReadPage, crate::channel_sync::ChannelSyncFailure> {
    use crate::channel_sync::{ChannelRead, ChannelReadPage, ChannelSyncFailure};
    let failure = |error: TelegramError| ChannelSyncFailure {
        retry_after: error.retry_after(),
        kind: map_telegram_error(error).kind(),
    };
    let summary = |file: teleark_telegram::TelegramFile| TelegramFileSummary {
        message_id: file.message_id(),
        sent_at_unix_ms: file.sent_at_unix_ms(),
        modified_at_unix_ms: file.modified_at_unix_ms(),
        file_name: file.file_name().to_owned(),
        caption: file.caption().to_owned(),
        mime_type: file.mime_type().map(ToOwned::to_owned),
        size_bytes: file.size_bytes(),
    };
    match request {
        ChannelRead::Difference(pts) => {
            let page = connection
                .channel_difference(chat, pts, cancellation)
                .await
                .map_err(failure)?;
            Ok(ChannelReadPage {
                files: page
                    .files
                    .into_iter()
                    .map(crate::channel_sync::update_summary)
                    .collect(),
                removed: page.removed,
                pts: Some(page.pts),
                complete: page.complete,
                history_gap: page.history_gap,
                before: None,
                edited: page.edited,
                timeout_seconds: page.timeout_seconds,
            })
        }
        ChannelRead::History(before) | ChannelRead::GapHistory(before) => {
            let page = connection
                .scan_sync_history(chat, before, cancellation)
                .await
                .map_err(failure)?;
            let (files, before, complete, _) = page.into_parts();
            Ok(ChannelReadPage {
                files: files.into_iter().map(summary).collect(),
                removed: Vec::new(),
                pts: None,
                complete,
                history_gap: false,
                before,
                edited: Vec::new(),
                timeout_seconds: None,
            })
        }
        ChannelRead::Verify(ids) => {
            let (files, removed) = connection
                .verify_channel_messages(chat, &ids, cancellation)
                .await
                .map_err(failure)?;
            Ok(ChannelReadPage {
                files: files.into_iter().map(summary).collect(),
                removed,
                pts: None,
                complete: true,
                history_gap: false,
                before: None,
                edited: Vec::new(),
                timeout_seconds: None,
            })
        }
        ChannelRead::ManagedManifests => {
            let files = connection
                .search_files_exact_caption(chat, crate::transfer::MANIFEST_CAPTION, 1_000)
                .await
                .map_err(failure)?;
            Ok(ChannelReadPage {
                files: files.into_iter().map(summary).collect(),
                removed: Vec::new(),
                edited: Vec::new(),
                pts: None,
                complete: true,
                history_gap: false,
                before: None,
                timeout_seconds: None,
            })
        }
        ChannelRead::Push => {
            Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest).into())
        }
    }
}

async fn scan_page(
    state: &WorkerState,
    account_id: i64,
    chat_id: i64,
    before_message_id: Option<i64>,
    limit: usize,
    cancellation: &TelegramScanCancellation,
) -> Result<TelegramFilePage, ApplicationError> {
    require_account(state, account_id)?;
    let chat = state
        .chats
        .get(&chat_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    let page = tokio::time::timeout(
        SCAN_PAGE_TIMEOUT,
        connection_ref(state)?.scan_file_page_cancellable(
            chat,
            before_message_id,
            limit,
            cancellation,
        ),
    )
    .await
    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?
    .map_err(map_telegram_error)?;
    let (files, next_before_message_id, exhausted, examined_messages) = page.into_parts();
    Ok(TelegramFilePage {
        files: files
            .into_iter()
            .map(|file| TelegramFileSummary {
                message_id: file.message_id(),
                sent_at_unix_ms: file.sent_at_unix_ms(),
                modified_at_unix_ms: file.modified_at_unix_ms(),
                file_name: file.file_name().to_owned(),
                caption: file.caption().to_owned(),
                mime_type: file.mime_type().map(str::to_owned),
                size_bytes: file.size_bytes(),
            })
            .collect(),
        next_before_message_id,
        exhausted,
        examined_messages: u64::try_from(examined_messages)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?,
    })
}

async fn scan_filtered_files(
    state: &WorkerState,
    account_id: i64,
    chat_id: i64,
    filter: TelegramFileFilter,
) -> Result<Vec<TelegramFileSummary>, ApplicationError> {
    validate_file_filter(filter)?;
    let mut cursor = None;
    let mut examined = 0_usize;
    let mut matches = Vec::new();
    while examined < MAX_BATCH_SCAN_MESSAGES && matches.len() < MAX_BATCH_FILES {
        let limit = 1_000.min(MAX_BATCH_SCAN_MESSAGES - examined);
        let page = scan_page(
            state,
            account_id,
            chat_id,
            cursor,
            limit,
            &TelegramScanCancellation::new(),
        )
        .await?;
        examined = examined.saturating_add(
            usize::try_from(page.examined_messages)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?,
        );
        let reached_lower_bound = filter
            .after_unix_ms
            .is_some_and(|after| page.files.iter().any(|file| file.sent_at_unix_ms < after));
        matches.extend(
            page.files
                .into_iter()
                .filter(|file| file_matches_filter(file, filter)),
        );
        matches.truncate(MAX_BATCH_FILES);
        if page.exhausted || reached_lower_bound {
            break;
        }
        let Some(next) = page.next_before_message_id else {
            break;
        };
        cursor = Some(next);
    }
    Ok(matches)
}

fn validate_file_filter(filter: TelegramFileFilter) -> Result<(), ApplicationError> {
    if filter
        .after_unix_ms
        .zip(filter.before_unix_ms)
        .is_some_and(|(after, before)| after > before)
    {
        Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest))
    } else {
        Ok(())
    }
}

fn file_matches_filter(file: &TelegramFileSummary, filter: TelegramFileFilter) -> bool {
    filter
        .after_unix_ms
        .is_none_or(|after| file.sent_at_unix_ms >= after)
        && filter
            .before_unix_ms
            .is_none_or(|before| file.sent_at_unix_ms <= before)
        && filter
            .kind
            .is_none_or(|kind| crate::classify_file(Path::new(&file.file_name)) == kind)
}

async fn download(
    state: &WorkerState,
    account_id: Option<i64>,
    chat_id: i64,
    message_id: i64,
    destination: PathBuf,
    observer: Option<Arc<dyn DownloadObserver>>,
) -> Result<(), ApplicationError> {
    if let Some(account_id) = account_id {
        require_account(state, account_id)?;
    }
    let chat = state
        .chats
        .get(&chat_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    let connection = connection_ref(state)?;
    let file = connection
        .fetch_file(chat, message_id)
        .await
        .map_err(map_telegram_error)?
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    match observer {
        Some(observer) => connection
            .download_file_observed(&file, destination, observer.as_ref())
            .await
            .map_err(map_telegram_error),
        None => connection
            .download_file(&file, destination)
            .await
            .map_err(map_telegram_error),
    }
}

async fn download_bytes(
    state: &WorkerState,
    account_id: i64,
    chat_id: i64,
    message_id: i64,
    observer: Option<&dyn ByteTransferObserver>,
) -> Result<Vec<u8>, ApplicationError> {
    require_account(state, account_id)?;
    let (connection, file) = fetch_file(state, chat_id, message_id).await?;
    connection
        .download_bytes_observed(&file, observer)
        .await
        .map_err(map_telegram_error)
}

async fn search_files(
    state: &WorkerState,
    account_id: i64,
    chat_id: i64,
    caption: &str,
    limit: usize,
) -> Result<Vec<TelegramFileSummary>, ApplicationError> {
    require_account(state, account_id)?;
    let chat = state
        .chats
        .get(&chat_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    connection_ref(state)?
        .search_files_exact_caption_with_recent(
            chat,
            caption,
            limit,
            caption == crate::transfer::MANIFEST_CAPTION,
        )
        .await
        .map_err(map_telegram_error)
        .map(|files| files.iter().map(file_summary).collect())
}

async fn upload_bytes(
    state: &WorkerState,
    account_id: i64,
    chat_id: i64,
    file_name: &str,
    caption: &str,
    bytes: &[u8],
    options: &UploadByteOptions,
) -> Result<i64, ApplicationError> {
    require_account(state, account_id)?;
    let chat = state
        .chats
        .get(&chat_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    connection_ref(state)?
        .upload_bytes_with_publication(
            chat,
            bytes,
            file_name,
            caption,
            teleark_telegram::UploadPublicationOptions {
                observer: options.observer.as_deref(),
                random_id: options.publication_random_id,
            },
        )
        .await
        .map(|sent| sent.message_id)
        .map_err(map_telegram_error)
}

async fn fetch_file(
    state: &WorkerState,
    chat_id: i64,
    message_id: i64,
) -> Result<(&TelegramConnection, teleark_telegram::TelegramFile), ApplicationError> {
    let chat = state
        .chats
        .get(&chat_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    let connection = connection_ref(state)?;
    let file = connection
        .fetch_file(chat, message_id)
        .await
        .map_err(map_telegram_error)?
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    Ok((connection, file))
}

fn file_summary(file: &teleark_telegram::TelegramFile) -> TelegramFileSummary {
    TelegramFileSummary {
        message_id: file.message_id(),
        sent_at_unix_ms: file.sent_at_unix_ms(),
        modified_at_unix_ms: file.modified_at_unix_ms(),
        file_name: file.file_name().to_owned(),
        caption: file.caption().to_owned(),
        mime_type: file.mime_type().map(str::to_owned),
        size_bytes: file.size_bytes(),
    }
}

async fn sign_out(state: &mut WorkerState) -> Result<(), ApplicationError> {
    connection(state)?
        .sign_out()
        .await
        .map_err(map_telegram_error)?;
    state.login = LoginState::None;
    Arc::make_mut(&mut state.chats).clear();
    Ok(())
}

fn connection(state: &mut WorkerState) -> Result<&TelegramConnection, ApplicationError> {
    state
        .connection
        .as_deref()
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Conflict))
}

fn connection_ref(state: &WorkerState) -> Result<&TelegramConnection, ApplicationError> {
    state
        .connection
        .as_deref()
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Conflict))
}

fn map_telegram_error(error: TelegramError) -> ApplicationError {
    let retry_after = error.retry_after();
    let kind = match error.kind() {
        TelegramErrorKind::InvalidConfiguration
        | TelegramErrorKind::SignUpRequired
        | TelegramErrorKind::InvalidCode
        | TelegramErrorKind::InvalidPassword => ApplicationErrorKind::InvalidRequest,
        TelegramErrorKind::Server => ApplicationErrorKind::Server,
        TelegramErrorKind::Session => ApplicationErrorKind::Persistence,
        TelegramErrorKind::Network | TelegramErrorKind::FloodWait => ApplicationErrorKind::Network,
        TelegramErrorKind::Authorization => ApplicationErrorKind::Authorization,
        TelegramErrorKind::SourceMissing => ApplicationErrorKind::SourceMissing,
        TelegramErrorKind::PermissionDenied => ApplicationErrorKind::PermissionDenied,
        TelegramErrorKind::StorageAccessDenied => ApplicationErrorKind::StorageAccessDenied,
        TelegramErrorKind::StorageConfigurationUnsafe => {
            ApplicationErrorKind::StorageConfigurationUnsafe
        }
        TelegramErrorKind::StorageIdentityDamaged => ApplicationErrorKind::StorageIdentityDamaged,
        TelegramErrorKind::StorageIdentityUnsupported => {
            ApplicationErrorKind::StorageIdentityUnsupported
        }
        TelegramErrorKind::LimitExceeded => ApplicationErrorKind::Capacity,
        TelegramErrorKind::Cancelled | TelegramErrorKind::Interrupted => {
            ApplicationErrorKind::Cancelled
        }
        _ => ApplicationErrorKind::Network,
    };
    let mapped = ApplicationError::new(kind);
    match retry_after {
        Some(delay) => mapped.with_retry_after(delay),
        None => mapped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_catalog_request_keeps_account_and_publishes_no_partial_roster() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let mut state = WorkerState {
                authorized_account_id: Some(17),
                ..Default::default()
            };
            let chats = Arc::clone(&state.chats);
            let cancellation = TelegramScanCancellation::new();
            cancellation.cancel();
            let (reply, result) = mpsc::sync_channel(1);
            handle_request(
                &mut state,
                TelegramRequest::ListDialogs {
                    account_id: 17,
                    cancellation,
                    reply,
                },
            )
            .await;
            assert_eq!(
                result.recv().expect("reply").expect_err("cancelled").kind(),
                ApplicationErrorKind::Cancelled
            );
            assert_eq!(state.authorized_account_id, Some(17));
            assert!(Arc::ptr_eq(&state.chats, &chats));
        });
    }

    #[test]
    fn dropping_telegram_owner_does_not_wait_for_a_full_queue_or_blocked_worker() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        sender
            .try_send(TelegramRequest::Shutdown)
            .expect("fill queue");
        let (release, blocked) = mpsc::sync_channel(1);
        let (retired, retirement) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            blocked.recv().expect("release worker");
            assert!(receiver.blocking_recv().is_some());
            retired.send(()).expect("retirement receiver");
        });
        let inner = TelegramWorkerInner {
            lifecycle: lifecycle::Lifecycle::default(),
            bandwidth: teleark_telegram::TransferBandwidth::default(),
            endpoint: Mutex::new(Some(network_owner::Endpoint {
                sender,
                generation: 0,
                stop: ScanCancellation::default(),
                join: Some(worker),
            })),
            changing: std::sync::atomic::AtomicBool::new(false),
            route: Mutex::new(NetworkRoute::Direct),
            monitor: NetworkMonitor::new(&NetworkRoute::Direct),
            active_probe: Mutex::new(None),
            session_path: PathBuf::new(),
        };
        let (finished, completion) = mpsc::sync_channel(1);
        let dropper = thread::spawn(move || {
            drop(inner);
            finished.send(()).expect("completion receiver");
        });
        let result = completion.recv_timeout(std::time::Duration::from_secs(2));
        release.send(()).expect("release worker");
        dropper.join().expect("dropper");
        retirement
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("worker retains ownership until exit");
        result.expect("drop cannot await queue capacity or thread completion");
    }

    #[test]
    fn session_identity_checks_are_local_and_auth_transitions_clear_old_identity() {
        let mut state = WorkerState::default();
        assert_eq!(
            require_account(&state, 17)
                .expect_err("not logged in")
                .kind(),
            ApplicationErrorKind::Authorization
        );
        let authorized = |id| {
            Ok(TelegramAuthState::Authorized(TelegramAccount {
                id,
                display_name: "Fixture".to_owned(),
                username: None,
            }))
        };
        state.record_auth_result(&authorized(17));
        // No connection or network runtime exists in this fixture. Repeated account
        // routing checks and immutable read snapshots need no remote get_me query.
        for _ in 0..10_000 {
            require_account(&state, 17).expect("authenticated account");
        }
        require_account(&state.read_snapshot(), 17).expect("same session snapshot");
        assert!(require_account(&state, 18).is_err());
        for outcome in [
            Ok(TelegramAuthState::Disconnected),
            Ok(TelegramAuthState::Unauthorized),
            Ok(TelegramAuthState::CodeSent),
            Ok(TelegramAuthState::PasswordRequired { hint: None }),
            Ok(TelegramAuthState::QrCode {
                deep_link: "fixture".to_owned(),
                expires_at_unix_seconds: 1,
            }),
            Err(ApplicationError::new(ApplicationErrorKind::Network)),
        ] {
            state.record_auth_result(&authorized(17));
            state.record_auth_result(&outcome);
            assert!(require_account(&state, 17).is_err());
        }
        state.record_auth_result(&authorized(18));
        assert!(require_account(&state, 17).is_err());
        require_account(&state, 18).expect("replacement account");
    }

    #[test]
    fn session_path_is_sibling_of_library_database() {
        let database = super::super::default_database_path();
        let session = default_telegram_session_path();
        assert_eq!(
            session,
            database.map(|path| path.with_file_name("telegram.session"))
        );
    }

    #[test]
    fn telegram_error_mapping_is_structured() {
        let error = TelegramErrorKind::Authorization;
        let mapped = match error {
            TelegramErrorKind::Authorization => ApplicationErrorKind::Authorization,
            _ => ApplicationErrorKind::Network,
        };
        assert_eq!(mapped, ApplicationErrorKind::Authorization);
    }

    #[test]
    fn qr_auth_debug_output_never_contains_the_login_secret() {
        let state = TelegramAuthState::QrCode {
            deep_link: "tg://login?token=highly-secret".to_owned(),
            expires_at_unix_seconds: 1_900_000_000,
        };
        let debug = format!("{state:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("highly-secret"));
    }

    #[test]
    fn qr_poll_refreshes_only_for_acceptance_updates_or_expiry() {
        assert!(!should_refresh_qr(99, 100, false));
        assert!(should_refresh_qr(99, 100, true));
        assert!(should_refresh_qr(100, 100, false));
        assert!(should_refresh_qr(101, 100, false));
    }

    #[test]
    fn configured_login_fails_before_network_when_credentials_are_missing() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library =
            DesktopLibrary::open(directory.path().join("library.sqlite3")).expect("open library");
        assert!(
            library
                .stored_telegram_credentials()
                .expect("read stored credentials")
                .is_none()
        );
        if distribution_credentials()
            .expect("read distribution credentials")
            .is_some()
        {
            return;
        }
        let telegram = DesktopTelegram::open_direct(directory.path().join("telegram.session"))
            .expect("open Telegram worker");
        assert_eq!(
            telegram
                .connect_configured(&library)
                .expect_err("missing credentials must fail closed")
                .kind(),
            ApplicationErrorKind::InvalidRequest
        );
    }

    #[test]
    fn effective_credentials_prefer_the_user_pair() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library =
            DesktopLibrary::open(directory.path().join("library.sqlite3")).expect("open library");
        library
            .set_telegram_credentials(54_321, "fedcba9876543210fedcba9876543210")
            .expect("save user credentials");
        let telegram = DesktopTelegram::open_direct(directory.path().join("telegram.session"))
            .expect("open Telegram worker");

        assert_eq!(
            telegram
                .effective_credentials_status(&library)
                .expect("resolve effective credentials"),
            Some(TelegramCredentialsStatus {
                api_id: 54_321,
                source: TelegramCredentialSource::User,
            })
        );
    }

    #[test]
    fn clearing_the_user_pair_reveals_the_build_pair_when_present() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library =
            DesktopLibrary::open(directory.path().join("library.sqlite3")).expect("open library");
        library
            .set_telegram_credentials(54_321, "fedcba9876543210fedcba9876543210")
            .expect("save user credentials");
        library
            .clear_telegram_credentials()
            .expect("clear user credentials");
        let telegram = DesktopTelegram::open_direct(directory.path().join("telegram.session"))
            .expect("open Telegram worker");
        let expected = distribution_credentials()
            .expect("read distribution credentials")
            .map(|credentials| TelegramCredentialsStatus {
                api_id: credentials.api_id,
                source: TelegramCredentialSource::Distribution,
            });

        assert_eq!(
            telegram
                .effective_credentials_status(&library)
                .expect("resolve effective credentials"),
            expected
        );
    }

    #[test]
    fn batch_file_filters_are_inclusive_and_classify_names_without_live_telegram() {
        let file = TelegramFileSummary {
            message_id: 7,
            sent_at_unix_ms: 1_700,
            modified_at_unix_ms: 1_800,
            file_name: "clip.MP4".to_owned(),
            caption: "full caption".to_owned(),
            mime_type: Some("video/mp4".to_owned()),
            size_bytes: 10,
        };
        let filter = TelegramFileFilter {
            after_unix_ms: Some(1_700),
            before_unix_ms: Some(1_700),
            kind: Some(FileKind::Video),
        };
        assert!(validate_file_filter(filter).is_ok());
        assert!(file_matches_filter(&file, filter));
        assert!(!file_matches_filter(
            &file,
            TelegramFileFilter {
                kind: Some(FileKind::Document),
                ..filter
            }
        ));
        assert_eq!(
            validate_file_filter(TelegramFileFilter {
                after_unix_ms: Some(2),
                before_unix_ms: Some(1),
                kind: None,
            })
            .expect_err("reversed range must fail")
            .kind(),
            ApplicationErrorKind::InvalidRequest
        );
    }
}

#[derive(Default)]
pub(crate) struct UploadByteOptions {
    pub publication_random_id: Option<i64>,
    pub observer: Option<Arc<dyn ByteTransferObserver>>,
    pub cancellation: Option<TelegramScanCancellation>,
}

/// Dropping a pending send can leave its remote result unknown. The object
/// adapter must reconcile the stable publication identity before any resend.
async fn interruptible_upload<T>(
    operation: impl std::future::Future<Output = Result<T, ApplicationError>>,
    cancellation: Option<&TelegramScanCancellation>,
) -> Result<T, ApplicationError> {
    match cancellation {
        Some(cancellation) => tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(ApplicationError::new(ApplicationErrorKind::Cancelled)),
            result = operation => result,
        },
        None => operation.await,
    }
}

#[cfg(test)]
mod upload_cancellation_tests {
    use super::*;
    #[tokio::test]
    async fn already_cancelled_upload_does_not_poll_or_publish() {
        let cancel = TelegramScanCancellation::new();
        cancel.cancel();
        let polled = std::cell::Cell::new(false);
        let result = interruptible_upload(
            async {
                polled.set(true);
                Ok::<_, ApplicationError>(())
            },
            Some(&cancel),
        )
        .await;
        assert_eq!(
            result.expect_err("cancelled").kind(),
            ApplicationErrorKind::Cancelled
        );
        assert!(!polled.get(), "cancelled upload must not begin");
    }
    #[tokio::test]
    async fn cancellation_drops_a_stalled_upload_without_stopping_other_work() {
        struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                if let Some(signal) = self.0.take() {
                    let _ = signal.send(());
                }
            }
        }
        let cancellation = TelegramScanCancellation::new();
        let worker_cancel = cancellation.clone();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (dropped, finished) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            interruptible_upload(
                async move {
                    let _drop = DropSignal(Some(dropped));
                    let _ = entered.send(());
                    std::future::pending::<Result<(), ApplicationError>>().await
                },
                Some(&worker_cancel),
            )
            .await
        });
        ready.await.expect("entered pending transport");
        cancellation.cancel();
        assert_eq!(
            worker.await.expect("worker").expect_err("cancelled").kind(),
            ApplicationErrorKind::Cancelled
        );
        finished.await.expect("transport future dropped");
        assert_eq!(
            interruptible_upload(async { Ok(42) }, None)
                .await
                .expect("unrelated operation"),
            42
        );
    }
}
