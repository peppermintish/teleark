use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use teleark_core::{ApplicationError, ApplicationErrorKind, FileKind};
use teleark_telegram::{
    DownloadObserver, PasswordChallenge, PasswordOutcome, PendingLogin, QrLoginOutcome,
    ScanCancellation, SignInOutcome, TelegramAccount, TelegramChat, TelegramChatKind,
    TelegramConfig, TelegramConnection, TelegramError, TelegramErrorKind,
};
use zeroize::Zeroizing;

use crate::{
    DesktopLibrary, TelegramCredentialSource, TelegramCredentialsStatus,
    credentials::{ActiveTelegramCredentials, distribution_credentials},
};

const TELEGRAM_QUEUE_CAPACITY: usize = 32;
const MAX_DIALOGS: usize = 10_000;
const MAX_BATCH_SCAN_MESSAGES: usize = 50_000;
const MAX_BATCH_FILES: usize = 2_000;
const SCAN_PAGE_TIMEOUT: Duration = Duration::from_secs(20);

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
    inner: Arc<TelegramWorkerInner>,
}

struct TelegramWorkerInner {
    sender: tokio::sync::mpsc::Sender<TelegramRequest>,
    join: Mutex<Option<JoinHandle<()>>>,
    session_path: PathBuf,
}

enum TelegramRequest {
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
        reply: mpsc::SyncSender<Result<Vec<TelegramChatSummary>, ApplicationError>>,
    },
    ScanPage {
        chat_id: i64,
        before_message_id: Option<i64>,
        limit: usize,
        cancellation: TelegramScanCancellation,
        reply: mpsc::SyncSender<Result<TelegramFilePage, ApplicationError>>,
    },
    ScanFilteredFiles {
        chat_id: i64,
        filter: TelegramFileFilter,
        reply: mpsc::SyncSender<Result<Vec<TelegramFileSummary>, ApplicationError>>,
    },
    Download {
        chat_id: i64,
        message_id: i64,
        destination: PathBuf,
        observer: Option<Arc<dyn DownloadObserver>>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    DownloadBytes {
        chat_id: i64,
        message_id: i64,
        reply: mpsc::SyncSender<Result<Vec<u8>, ApplicationError>>,
    },
    SearchFiles {
        chat_id: i64,
        caption: String,
        limit: usize,
        reply: mpsc::SyncSender<Result<Vec<TelegramFileSummary>, ApplicationError>>,
    },
    UploadBytes {
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
    connection: Option<TelegramConnection>,
    api_id: Option<i32>,
    login: LoginState,
    chats: BTreeMap<i64, TelegramChat>,
}

impl Default for WorkerState {
    fn default() -> Self {
        Self {
            connection: None,
            api_id: None,
            login: LoginState::None,
            chats: BTreeMap::new(),
        }
    }
}

impl DesktopTelegram {
    pub fn open(session_path: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        let session_path = session_path.as_ref().to_owned();
        if session_path.as_os_str().is_empty() {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let (sender, receiver) = tokio::sync::mpsc::channel(TELEGRAM_QUEUE_CAPACITY);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("teleark-telegram".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => {
                        let _ = ready_sender.send(Ok(()));
                        runtime.block_on(telegram_loop(receiver));
                    }
                    Err(_) => {
                        let _ = ready_sender
                            .send(Err(ApplicationError::new(ApplicationErrorKind::Network)));
                    }
                }
            })
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                inner: Arc::new(TelegramWorkerInner {
                    sender,
                    join: Mutex::new(Some(join)),
                    session_path,
                }),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(_) => {
                let _ = join.join();
                Err(ApplicationError::new(ApplicationErrorKind::Network))
            }
        }
    }

    pub fn open_default() -> Result<Self, ApplicationError> {
        let path = default_telegram_session_path()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        Self::open(path)
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
        self.connect(credentials.api_id)
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

    pub fn list_dialogs(&self) -> Result<Vec<TelegramChatSummary>, ApplicationError> {
        self.request("list_dialogs", |reply| TelegramRequest::ListDialogs {
            reply,
        })
    }

    pub fn scan_file_page(
        &self,
        chat_id: i64,
        before_message_id: Option<i64>,
        limit: usize,
    ) -> Result<TelegramFilePage, ApplicationError> {
        self.scan_file_page_cancellable(
            chat_id,
            before_message_id,
            limit,
            TelegramScanCancellation::new(),
        )
    }

    pub fn scan_file_page_cancellable(
        &self,
        chat_id: i64,
        before_message_id: Option<i64>,
        limit: usize,
        cancellation: TelegramScanCancellation,
    ) -> Result<TelegramFilePage, ApplicationError> {
        self.request("scan_file_page", |reply| TelegramRequest::ScanPage {
            chat_id,
            before_message_id,
            limit,
            cancellation,
            reply,
        })
    }

    pub fn scan_filtered_files(
        &self,
        chat_id: i64,
        filter: TelegramFileFilter,
    ) -> Result<Vec<TelegramFileSummary>, ApplicationError> {
        self.request("scan_filtered_files", |reply| {
            TelegramRequest::ScanFilteredFiles {
                chat_id,
                filter,
                reply,
            }
        })
    }

    pub fn download_file(
        &self,
        chat_id: i64,
        message_id: i64,
        destination: impl AsRef<Path>,
    ) -> Result<(), ApplicationError> {
        self.request("download_file", |reply| TelegramRequest::Download {
            chat_id,
            message_id,
            destination: destination.as_ref().to_owned(),
            observer: None,
            reply,
        })
    }

    pub(crate) fn download_file_observed(
        &self,
        chat_id: i64,
        message_id: i64,
        destination: impl AsRef<Path>,
        observer: Arc<dyn DownloadObserver>,
    ) -> Result<(), ApplicationError> {
        self.request("download_file_observed", |reply| {
            TelegramRequest::Download {
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

    pub fn download_bytes(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Vec<u8>, ApplicationError> {
        self.request("download_bytes", |reply| TelegramRequest::DownloadBytes {
            chat_id,
            message_id,
            reply,
        })
    }

    pub fn search_files_exact_caption(
        &self,
        chat_id: i64,
        caption: impl Into<String>,
        limit: usize,
    ) -> Result<Vec<TelegramFileSummary>, ApplicationError> {
        self.request("search_files", |reply| TelegramRequest::SearchFiles {
            chat_id,
            caption: caption.into(),
            limit,
            reply,
        })
    }

    pub fn upload_bytes(
        &self,
        chat_id: i64,
        file_name: impl Into<String>,
        caption: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Result<i64, ApplicationError> {
        self.request("upload_bytes", |reply| TelegramRequest::UploadBytes {
            chat_id,
            file_name: file_name.into(),
            caption: caption.into(),
            bytes,
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
        let result = self
            .inner
            .sender
            .blocking_send(build(reply))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))
            .and_then(|()| {
                response
                    .recv()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))
            })
            .and_then(|result| result);
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

impl Drop for TelegramWorkerInner {
    fn drop(&mut self) {
        let _ = self.sender.blocking_send(TelegramRequest::Shutdown);
        if let Ok(mut join) = self.join.lock()
            && let Some(join) = join.take()
        {
            let _ = join.join();
        }
    }
}

async fn telegram_loop(mut receiver: tokio::sync::mpsc::Receiver<TelegramRequest>) {
    let mut state = WorkerState::default();
    while let Some(request) = receiver.recv().await {
        match request {
            TelegramRequest::Connect {
                api_id,
                session_path,
                reply,
            } => {
                let result = connect(&mut state, api_id, session_path).await;
                let _ = reply.send(result);
            }
            TelegramRequest::RequestCode {
                phone,
                api_hash,
                reply,
            } => {
                let result = request_code(&mut state, &phone, &api_hash).await;
                let _ = reply.send(result);
            }
            TelegramRequest::BeginQrLogin { api_hash, reply } => {
                let result = begin_qr_login(&mut state, api_hash).await;
                let _ = reply.send(result);
            }
            TelegramRequest::PollQrLogin { reply } => {
                let result = poll_qr_login(&mut state).await;
                let _ = reply.send(result);
            }
            TelegramRequest::SubmitCode { code, reply } => {
                let result = submit_code(&mut state, &code).await;
                let _ = reply.send(result);
            }
            TelegramRequest::SubmitPassword { password, reply } => {
                let result = submit_password(&mut state, &password).await;
                let _ = reply.send(result);
            }
            TelegramRequest::ListDialogs { reply } => {
                let result = list_dialogs(&mut state).await;
                let _ = reply.send(result);
            }
            TelegramRequest::ScanPage {
                chat_id,
                before_message_id,
                limit,
                cancellation,
                reply,
            } => {
                let result =
                    scan_page(&state, chat_id, before_message_id, limit, &cancellation).await;
                let _ = reply.send(result);
            }
            TelegramRequest::ScanFilteredFiles {
                chat_id,
                filter,
                reply,
            } => {
                let result = scan_filtered_files(&state, chat_id, filter).await;
                let _ = reply.send(result);
            }
            TelegramRequest::Download {
                chat_id,
                message_id,
                destination,
                observer,
                reply,
            } => {
                let result = download(&state, chat_id, message_id, destination, observer).await;
                let _ = reply.send(result);
            }
            TelegramRequest::DownloadBytes {
                chat_id,
                message_id,
                reply,
            } => {
                let result = download_bytes(&state, chat_id, message_id).await;
                let _ = reply.send(result);
            }
            TelegramRequest::SearchFiles {
                chat_id,
                caption,
                limit,
                reply,
            } => {
                let result = search_files(&state, chat_id, &caption, limit).await;
                let _ = reply.send(result);
            }
            TelegramRequest::UploadBytes {
                chat_id,
                file_name,
                caption,
                bytes,
                reply,
            } => {
                let result = upload_bytes(&state, chat_id, &file_name, &caption, &bytes).await;
                let _ = reply.send(result);
            }
            TelegramRequest::SignOut { reply } => {
                let result = sign_out(&mut state).await;
                let _ = reply.send(result);
            }
            TelegramRequest::Shutdown => {
                if let Some(connection) = state.connection.take() {
                    connection.shutdown().await;
                }
                break;
            }
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
    })
    .await
    .map_err(map_telegram_error)?;
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
    state.connection = Some(connection);
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
            id: chat.id(),
            name: chat.name().to_owned(),
            username: chat.username().map(str::to_owned),
            kind: chat.kind(),
        })
        .collect();
    state.chats = dialogs.into_iter().map(|chat| (chat.id(), chat)).collect();
    Ok(summaries)
}

async fn scan_page(
    state: &WorkerState,
    chat_id: i64,
    before_message_id: Option<i64>,
    limit: usize,
    cancellation: &TelegramScanCancellation,
) -> Result<TelegramFilePage, ApplicationError> {
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
    chat_id: i64,
    message_id: i64,
    destination: PathBuf,
    observer: Option<Arc<dyn DownloadObserver>>,
) -> Result<(), ApplicationError> {
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
    chat_id: i64,
    message_id: i64,
) -> Result<Vec<u8>, ApplicationError> {
    let (connection, file) = fetch_file(state, chat_id, message_id).await?;
    connection
        .download_bytes(&file)
        .await
        .map_err(map_telegram_error)
}

async fn search_files(
    state: &WorkerState,
    chat_id: i64,
    caption: &str,
    limit: usize,
) -> Result<Vec<TelegramFileSummary>, ApplicationError> {
    let chat = state
        .chats
        .get(&chat_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    connection_ref(state)?
        .search_files_exact_caption(chat, caption, limit)
        .await
        .map_err(map_telegram_error)
        .map(|files| files.iter().map(file_summary).collect())
}

async fn upload_bytes(
    state: &WorkerState,
    chat_id: i64,
    file_name: &str,
    caption: &str,
    bytes: &[u8],
) -> Result<i64, ApplicationError> {
    let chat = state
        .chats
        .get(&chat_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    connection_ref(state)?
        .upload_bytes(chat, bytes, file_name, caption)
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
    state.chats.clear();
    Ok(())
}

fn connection(state: &mut WorkerState) -> Result<&TelegramConnection, ApplicationError> {
    state
        .connection
        .as_ref()
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Conflict))
}

fn connection_ref(state: &WorkerState) -> Result<&TelegramConnection, ApplicationError> {
    state
        .connection
        .as_ref()
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Conflict))
}

fn map_telegram_error(error: TelegramError) -> ApplicationError {
    let kind = match error.kind() {
        TelegramErrorKind::InvalidConfiguration
        | TelegramErrorKind::SignUpRequired
        | TelegramErrorKind::InvalidCode
        | TelegramErrorKind::InvalidPassword => ApplicationErrorKind::InvalidRequest,
        TelegramErrorKind::Session => ApplicationErrorKind::Persistence,
        TelegramErrorKind::Network | TelegramErrorKind::FloodWait => ApplicationErrorKind::Network,
        TelegramErrorKind::Authorization => ApplicationErrorKind::Authorization,
        TelegramErrorKind::SourceMissing => ApplicationErrorKind::SourceMissing,
        TelegramErrorKind::PermissionDenied => ApplicationErrorKind::PermissionDenied,
        TelegramErrorKind::LimitExceeded => ApplicationErrorKind::Capacity,
        TelegramErrorKind::Cancelled | TelegramErrorKind::Interrupted => {
            ApplicationErrorKind::Cancelled
        }
        _ => ApplicationErrorKind::Network,
    };
    ApplicationError::new(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let telegram = DesktopTelegram::open(directory.path().join("telegram.session"))
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
        let telegram = DesktopTelegram::open(directory.path().join("telegram.session"))
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
        let telegram = DesktopTelegram::open(directory.path().join("telegram.session"))
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
