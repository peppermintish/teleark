use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
};

use teleark_core::{ApplicationError, ApplicationErrorKind};
use teleark_telegram::{
    PasswordChallenge, PasswordOutcome, PendingLogin, SignInOutcome, TelegramAccount, TelegramChat,
    TelegramChatKind, TelegramConfig, TelegramConnection, TelegramError, TelegramErrorKind,
};

const TELEGRAM_QUEUE_CAPACITY: usize = 32;
const MAX_DIALOGS: usize = 10_000;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TelegramAuthState {
    Disconnected,
    Unauthorized,
    CodeSent,
    PasswordRequired { hint: Option<String> },
    Authorized(TelegramAccount),
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
        api_hash: String,
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
        reply: mpsc::SyncSender<Result<TelegramFilePage, ApplicationError>>,
    },
    Download {
        chat_id: i64,
        message_id: i64,
        destination: PathBuf,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    SignOut {
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    Shutdown,
}

enum LoginState {
    None,
    Code(PendingLogin),
    Password(Box<PasswordChallenge>),
}

struct WorkerState {
    connection: Option<TelegramConnection>,
    login: LoginState,
    chats: BTreeMap<i64, TelegramChat>,
}

impl Default for WorkerState {
    fn default() -> Self {
        Self {
            connection: None,
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
        self.request(|reply| TelegramRequest::Connect {
            api_id,
            session_path,
            reply,
        })
    }

    pub fn request_login_code(
        &self,
        phone: impl Into<String>,
        api_hash: impl Into<String>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        self.request(|reply| TelegramRequest::RequestCode {
            phone: phone.into(),
            api_hash: api_hash.into(),
            reply,
        })
    }

    pub fn submit_code(
        &self,
        code: impl Into<String>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        self.request(|reply| TelegramRequest::SubmitCode {
            code: code.into(),
            reply,
        })
    }

    pub fn submit_password(
        &self,
        password: impl Into<Vec<u8>>,
    ) -> Result<TelegramAuthState, ApplicationError> {
        self.request(|reply| TelegramRequest::SubmitPassword {
            password: password.into(),
            reply,
        })
    }

    pub fn list_dialogs(&self) -> Result<Vec<TelegramChatSummary>, ApplicationError> {
        self.request(|reply| TelegramRequest::ListDialogs { reply })
    }

    pub fn scan_file_page(
        &self,
        chat_id: i64,
        before_message_id: Option<i64>,
        limit: usize,
    ) -> Result<TelegramFilePage, ApplicationError> {
        self.request(|reply| TelegramRequest::ScanPage {
            chat_id,
            before_message_id,
            limit,
            reply,
        })
    }

    pub fn download_file(
        &self,
        chat_id: i64,
        message_id: i64,
        destination: impl AsRef<Path>,
    ) -> Result<(), ApplicationError> {
        self.request(|reply| TelegramRequest::Download {
            chat_id,
            message_id,
            destination: destination.as_ref().to_owned(),
            reply,
        })
    }

    pub fn sign_out(&self) -> Result<(), ApplicationError> {
        self.request(|reply| TelegramRequest::SignOut { reply })
    }

    fn request<T>(
        &self,
        build: impl FnOnce(mpsc::SyncSender<Result<T, ApplicationError>>) -> TelegramRequest,
    ) -> Result<T, ApplicationError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.inner
            .sender
            .blocking_send(build(reply))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?;
        response
            .recv()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?
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
                reply,
            } => {
                let result = scan_page(&state, chat_id, before_message_id, limit).await;
                let _ = reply.send(result);
            }
            TelegramRequest::Download {
                chat_id,
                message_id,
                destination,
                reply,
            } => {
                let result = download(&state, chat_id, message_id, destination).await;
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
    if state.connection.is_some() {
        return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
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
) -> Result<TelegramFilePage, ApplicationError> {
    let chat = state
        .chats
        .get(&chat_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
    let page = connection_ref(state)?
        .scan_file_page(chat, before_message_id, limit)
        .await
        .map_err(map_telegram_error)?;
    let (files, next_before_message_id, exhausted, examined_messages) = page.into_parts();
    Ok(TelegramFilePage {
        files: files
            .into_iter()
            .map(|file| TelegramFileSummary {
                message_id: file.message_id(),
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

async fn download(
    state: &WorkerState,
    chat_id: i64,
    message_id: i64,
    destination: PathBuf,
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
    connection
        .download_file(&file, destination)
        .await
        .map_err(map_telegram_error)
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
        TelegramErrorKind::Cancelled => ApplicationErrorKind::Cancelled,
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
}
