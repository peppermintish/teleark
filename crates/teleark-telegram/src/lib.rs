//! Telegram transport adapter for TeleArk.
//!
//! All `grammers` values stay private to this crate. Callers receive stable,
//! frontend-neutral records and structured errors instead.

use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use grammers_client::{
    Client, SignInError,
    media::{Document, Media},
    message::InputMessage,
};
use grammers_mtsender::{InvocationError, SenderPool};
use grammers_session::types::{PeerKind, PeerRef};
use tokio::task::JoinHandle;

mod session;

use session::FileSession;

const MAX_DIALOGS_PER_REQUEST: usize = 10_000;
const MAX_MESSAGES_PER_SCAN: usize = 10_000;
const MAX_SEARCH_RESULTS: usize = 1_000;
pub const MAX_TRANSFER_OBJECT_BYTES: usize = 64 * 1024 * 1024;

/// Configuration that is safe to persist as ordinary application settings.
///
/// The Telegram API hash is intentionally not stored here. The application
/// must obtain and protect that value separately before requesting login.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramConfig {
    pub api_id: i32,
    pub session_path: PathBuf,
}

/// Stable error categories used by Core and frontends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TelegramErrorKind {
    InvalidConfiguration,
    Session,
    Network,
    FloodWait,
    Authorization,
    SignUpRequired,
    InvalidCode,
    InvalidPassword,
    SourceMissing,
    PermissionDenied,
    LimitExceeded,
    Cancelled,
}

/// A locale-neutral Telegram adapter failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramError {
    kind: TelegramErrorKind,
    retry_after: Option<Duration>,
}

impl TelegramError {
    pub const fn kind(&self) -> TelegramErrorKind {
        self.kind
    }

    pub const fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }

    const fn new(kind: TelegramErrorKind) -> Self {
        Self {
            kind,
            retry_after: None,
        }
    }

    const fn flood_wait(retry_after: Duration) -> Self {
        Self {
            kind: TelegramErrorKind::FloodWait,
            retry_after: Some(retry_after),
        }
    }
}

impl fmt::Display for TelegramError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.kind, self.retry_after) {
            (TelegramErrorKind::FloodWait, Some(duration)) => {
                write!(
                    formatter,
                    "Telegram requested a {} second retry delay",
                    duration.as_secs()
                )
            }
            (kind, _) => write!(formatter, "Telegram adapter failure: {kind:?}"),
        }
    }
}

impl Error for TelegramError {}

/// Opaque login state returned after Telegram sends an authorization code.
pub struct PendingLogin {
    token: grammers_client::client::LoginToken,
}

/// Opaque state needed to complete an account's two-factor sign-in.
pub struct PasswordChallenge {
    token: grammers_client::client::PasswordToken,
    hint: Option<String>,
}

impl PasswordChallenge {
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramAccount {
    pub id: i64,
    pub display_name: String,
    pub username: Option<String>,
}

pub enum SignInOutcome {
    Authorized(TelegramAccount),
    PasswordRequired(Box<PasswordChallenge>),
}

pub enum PasswordOutcome {
    Authorized(TelegramAccount),
    InvalidPassword(Box<PasswordChallenge>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TelegramChatKind {
    User,
    Group,
    Channel,
}

/// A Telegram dialog with its transport reference retained privately.
#[derive(Clone, Debug)]
pub struct TelegramChat {
    id: i64,
    name: String,
    username: Option<String>,
    kind: TelegramChatKind,
    peer_ref: PeerRef,
}

impl TelegramChat {
    pub const fn id(&self) -> i64 {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub const fn kind(&self) -> TelegramChatKind {
        self.kind
    }
}

/// Metadata and an opaque media handle for one downloadable Telegram file.
#[derive(Clone, Debug)]
pub struct TelegramFile {
    message_id: i64,
    modified_at_unix_ms: i64,
    file_name: String,
    caption: String,
    mime_type: Option<String>,
    size_bytes: u64,
    document: Document,
}

#[derive(Clone, Debug)]
pub struct TelegramFilePage {
    files: Vec<TelegramFile>,
    next_before_message_id: Option<i64>,
    exhausted: bool,
    examined_messages: usize,
}

impl TelegramFilePage {
    pub fn into_parts(self) -> (Vec<TelegramFile>, Option<i64>, bool, usize) {
        (
            self.files,
            self.next_before_message_id,
            self.exhausted,
            self.examined_messages,
        )
    }
}

impl TelegramFile {
    pub const fn message_id(&self) -> i64 {
        self.message_id
    }

    pub const fn modified_at_unix_ms(&self) -> i64 {
        self.modified_at_unix_ms
    }

    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    pub fn caption(&self) -> &str {
        &self.caption
    }

    pub fn mime_type(&self) -> Option<&str> {
        self.mime_type.as_deref()
    }

    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SentDocument {
    pub message_id: i64,
}

/// Connected Telegram adapter with explicit ownership of all background work.
pub struct TelegramConnection {
    client: Client,
    runner: Option<JoinHandle<()>>,
    update_drain: Option<JoinHandle<()>>,
}

impl TelegramConnection {
    /// Opens an isolated Telegram session and starts its retained network tasks.
    pub async fn connect(config: TelegramConfig) -> Result<Self, TelegramError> {
        validate_config(&config)?;
        prepare_session_parent(&config.session_path).await?;

        let session = Arc::new(
            FileSession::open(&config.session_path)
                .map_err(|_| TelegramError::new(TelegramErrorKind::Session))?,
        );
        restrict_session_permissions(&config.session_path).await?;

        let SenderPool {
            runner,
            handle,
            mut updates,
        } = SenderPool::new(session, config.api_id);
        let client = Client::new(handle);
        let runner = tokio::spawn(runner.run());
        // Grammers receives updates through an unbounded transport channel. Drain
        // it continuously until a bounded Core update stream is connected.
        let update_drain = tokio::spawn(async move { while updates.recv().await.is_some() {} });

        Ok(Self {
            client,
            runner: Some(runner),
            update_drain: Some(update_drain),
        })
    }

    pub async fn is_authorized(&self) -> Result<bool, TelegramError> {
        self.client.is_authorized().await.map_err(map_invocation)
    }

    pub async fn current_account(&self) -> Result<TelegramAccount, TelegramError> {
        require_authorized(&self.client).await?;
        self.client
            .get_me()
            .await
            .map(|user| account_from_user(&user))
            .map_err(map_invocation)
    }

    pub async fn request_login_code(
        &self,
        phone: &str,
        api_hash: &str,
    ) -> Result<PendingLogin, TelegramError> {
        if phone.trim().is_empty() || api_hash.trim().is_empty() {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        self.client
            .request_login_code(phone, api_hash)
            .await
            .map(|token| PendingLogin { token })
            .map_err(map_invocation)
    }

    pub async fn sign_in(
        &self,
        pending: &PendingLogin,
        code: &str,
    ) -> Result<SignInOutcome, TelegramError> {
        if code.trim().is_empty() {
            return Err(TelegramError::new(TelegramErrorKind::InvalidCode));
        }
        match self.client.sign_in(&pending.token, code).await {
            Ok(user) => Ok(SignInOutcome::Authorized(account_from_user(&user))),
            Err(SignInError::PasswordRequired(token)) => {
                let hint = token.hint().map(ToOwned::to_owned);
                Ok(SignInOutcome::PasswordRequired(Box::new(
                    PasswordChallenge { token, hint },
                )))
            }
            Err(SignInError::SignUpRequired) => {
                Err(TelegramError::new(TelegramErrorKind::SignUpRequired))
            }
            Err(SignInError::InvalidCode) => {
                Err(TelegramError::new(TelegramErrorKind::InvalidCode))
            }
            Err(SignInError::InvalidPassword(_)) => {
                Err(TelegramError::new(TelegramErrorKind::InvalidPassword))
            }
            Err(SignInError::Other(error)) => Err(map_invocation(error)),
        }
    }

    pub async fn check_password(
        &self,
        challenge: PasswordChallenge,
        password: &[u8],
    ) -> Result<PasswordOutcome, TelegramError> {
        if password.is_empty() {
            return Err(TelegramError::new(TelegramErrorKind::InvalidPassword));
        }
        match self.client.check_password(challenge.token, password).await {
            Ok(user) => Ok(PasswordOutcome::Authorized(account_from_user(&user))),
            Err(SignInError::InvalidPassword(token))
            | Err(SignInError::PasswordRequired(token)) => {
                let hint = token.hint().map(ToOwned::to_owned);
                Ok(PasswordOutcome::InvalidPassword(Box::new(
                    PasswordChallenge { token, hint },
                )))
            }
            Err(SignInError::SignUpRequired) => {
                Err(TelegramError::new(TelegramErrorKind::SignUpRequired))
            }
            Err(SignInError::InvalidCode) => {
                Err(TelegramError::new(TelegramErrorKind::InvalidCode))
            }
            Err(SignInError::Other(error)) => Err(map_invocation(error)),
        }
    }

    pub async fn list_dialogs(&self, limit: usize) -> Result<Vec<TelegramChat>, TelegramError> {
        validate_limit(limit, MAX_DIALOGS_PER_REQUEST)?;
        require_authorized(&self.client).await?;

        let mut dialogs = self.client.iter_dialogs();
        let mut chats = Vec::with_capacity(limit.min(128));
        while chats.len() < limit {
            let Some(dialog) = dialogs.next().await.map_err(map_invocation)? else {
                break;
            };
            let peer = dialog.peer();
            let peer_id = peer.id();
            let Some(id) = peer_id.bare_id() else {
                continue;
            };
            let kind = match peer_id.kind() {
                PeerKind::User => TelegramChatKind::User,
                PeerKind::Chat => TelegramChatKind::Group,
                PeerKind::Channel => TelegramChatKind::Channel,
            };
            chats.push(TelegramChat {
                id,
                name: peer.name().unwrap_or_default().to_owned(),
                username: peer.username().map(ToOwned::to_owned),
                kind,
                peer_ref: dialog.peer_ref(),
            });
        }
        Ok(chats)
    }

    /// Scans a bounded page of document messages, newest first.
    ///
    /// `before_message_id` is exclusive and can be persisted as a checkpoint.
    pub async fn scan_files(
        &self,
        chat: &TelegramChat,
        before_message_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<TelegramFile>, TelegramError> {
        self.scan_file_page(chat, before_message_id, limit)
            .await
            .map(|page| page.files)
    }

    /// Scans a bounded history page while preserving the last examined
    /// message as a durable cursor even when the page contains non-documents.
    pub async fn scan_file_page(
        &self,
        chat: &TelegramChat,
        before_message_id: Option<i64>,
        limit: usize,
    ) -> Result<TelegramFilePage, TelegramError> {
        validate_limit(limit, MAX_MESSAGES_PER_SCAN)?;
        let offset = before_message_id
            .map(i32::try_from)
            .transpose()
            .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?
            .unwrap_or_default();

        let mut messages = self
            .client
            .iter_messages(chat.peer_ref)
            .offset_id(offset)
            .limit(limit);
        let mut files = Vec::new();
        let mut examined = 0_usize;
        let mut last_message_id = None;
        while let Some(message) = messages.next().await.map_err(map_invocation)? {
            examined = examined.saturating_add(1);
            last_message_id = Some(i64::from(message.id()));
            if let Some(file) = file_from_message(message)? {
                files.push(file);
            }
        }
        let exhausted = examined < limit;
        Ok(TelegramFilePage {
            files,
            next_before_message_id: (!exhausted).then_some(last_message_id).flatten(),
            exhausted,
            examined_messages: examined,
        })
    }

    /// Refetches one indexed document by its source message identity.
    pub async fn fetch_file(
        &self,
        chat: &TelegramChat,
        message_id: i64,
    ) -> Result<Option<TelegramFile>, TelegramError> {
        let message_id = i32::try_from(message_id)
            .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
        let mut messages = self
            .client
            .get_messages_by_id(chat.peer_ref, &[message_id])
            .await
            .map_err(map_invocation)?;
        let Some(message) = messages.pop().flatten() else {
            return Ok(None);
        };
        file_from_message(message)
    }

    /// Searches a bounded set of document messages and retains only exact
    /// caption matches. The exact comparison turns Telegram's fuzzy text
    /// search into a safe reconciliation lookup for opaque transfer keys.
    pub async fn search_files_exact_caption(
        &self,
        chat: &TelegramChat,
        caption: &str,
        limit: usize,
    ) -> Result<Vec<TelegramFile>, TelegramError> {
        validate_limit(limit, MAX_SEARCH_RESULTS)?;
        if caption.is_empty() {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        let mut messages = self
            .client
            .search_messages(chat.peer_ref)
            .query(caption)
            .limit(limit);
        let mut files = Vec::new();
        while let Some(message) = messages.next().await.map_err(map_invocation)? {
            if message.text() != caption {
                continue;
            }
            if let Some(file) = file_from_message(message)? {
                files.push(file);
            }
        }
        Ok(files)
    }

    /// Downloads a previously indexed document to an explicit caller-selected path.
    pub async fn download_file(
        &self,
        file: &TelegramFile,
        destination: impl AsRef<Path>,
    ) -> Result<(), TelegramError> {
        self.client
            .download_media(&file.document, destination)
            .await
            .map_err(map_invocation)
    }

    /// Downloads one bounded transfer object without creating a temporary
    /// ciphertext file. Application-part policy keeps this buffer bounded.
    pub async fn download_bytes(&self, file: &TelegramFile) -> Result<Vec<u8>, TelegramError> {
        let expected = usize::try_from(file.size_bytes)
            .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
        if expected > MAX_TRANSFER_OBJECT_BYTES {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let mut bytes = Vec::with_capacity(expected);
        let mut download = self.client.iter_download(&file.document);
        while let Some(chunk) = download.next().await.map_err(map_invocation)? {
            let next = bytes
                .len()
                .checked_add(chunk.len())
                .ok_or_else(|| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
            if next > MAX_TRANSFER_OBJECT_BYTES || next > expected {
                return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() != expected {
            return Err(TelegramError::new(TelegramErrorKind::Network));
        }
        Ok(bytes)
    }

    /// Uploads and sends one Telegram-native document.
    pub async fn upload_document(
        &self,
        chat: &TelegramChat,
        source: impl AsRef<Path>,
        caption: &str,
    ) -> Result<SentDocument, TelegramError> {
        let source = source.as_ref();
        let metadata = tokio::fs::metadata(source).await.map_err(map_io)?;
        if !metadata.is_file() {
            return Err(TelegramError::new(TelegramErrorKind::SourceMissing));
        }
        let size = usize::try_from(metadata.len())
            .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| TelegramError::new(TelegramErrorKind::SourceMissing))?
            .to_owned();
        let mut stream = tokio::fs::File::open(source).await.map_err(map_io)?;
        let uploaded = self
            .client
            .upload_stream(&mut stream, size, name)
            .await
            .map_err(map_io)?;
        let message = InputMessage::new().text(caption).document(uploaded);
        let sent = self
            .client
            .send_message(chat.peer_ref, message)
            .await
            .map_err(map_invocation)?;
        Ok(SentDocument {
            message_id: i64::from(sent.id()),
        })
    }

    /// Uploads one already encoded, bounded application object and sends it as
    /// a document. The opaque caption is used for crash reconciliation.
    pub async fn upload_bytes(
        &self,
        chat: &TelegramChat,
        bytes: &[u8],
        file_name: &str,
        caption: &str,
    ) -> Result<SentDocument, TelegramError> {
        if bytes.is_empty()
            || bytes.len() > MAX_TRANSFER_OBJECT_BYTES
            || file_name.is_empty()
            || caption.is_empty()
        {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let mut stream = std::io::Cursor::new(bytes);
        let uploaded = self
            .client
            .upload_stream(&mut stream, bytes.len(), file_name.to_owned())
            .await
            .map_err(map_io)?;
        let message = InputMessage::new().text(caption).document(uploaded);
        let sent = self
            .client
            .send_message(chat.peer_ref, message)
            .await
            .map_err(map_invocation)?;
        Ok(SentDocument {
            message_id: i64::from(sent.id()),
        })
    }

    pub async fn sign_out(&self) -> Result<(), TelegramError> {
        self.client
            .sign_out()
            .await
            .map(drop)
            .map_err(map_invocation)
    }

    /// Gracefully stops both retained background tasks.
    pub async fn shutdown(mut self) {
        self.client.disconnect();
        if let Some(task) = self.runner.take() {
            let _ = task.await;
        }
        if let Some(task) = self.update_drain.take() {
            let _ = task.await;
        }
    }
}

impl Drop for TelegramConnection {
    fn drop(&mut self) {
        self.client.disconnect();
        if let Some(task) = self.runner.take() {
            task.abort();
        }
        if let Some(task) = self.update_drain.take() {
            task.abort();
        }
    }
}

fn validate_config(config: &TelegramConfig) -> Result<(), TelegramError> {
    if config.api_id <= 0 || config.session_path.as_os_str().is_empty() {
        return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
    }
    Ok(())
}

fn validate_limit(limit: usize, maximum: usize) -> Result<(), TelegramError> {
    if limit == 0 || limit > maximum {
        Err(TelegramError::new(TelegramErrorKind::LimitExceeded))
    } else {
        Ok(())
    }
}

async fn prepare_session_parent(path: &Path) -> Result<(), TelegramError> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    tokio::fs::create_dir_all(parent).await.map_err(map_io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(map_io)?;
    }
    Ok(())
}

async fn restrict_session_permissions(path: &Path) -> Result<(), TelegramError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(map_io)?;
    }
    Ok(())
}

async fn require_authorized(client: &Client) -> Result<(), TelegramError> {
    if client.is_authorized().await.map_err(map_invocation)? {
        Ok(())
    } else {
        Err(TelegramError::new(TelegramErrorKind::Authorization))
    }
}

fn account_from_user(user: &grammers_client::peer::User) -> TelegramAccount {
    TelegramAccount {
        id: user.id().bare_id().unwrap_or_default(),
        display_name: user.full_name(),
        username: user.username().map(ToOwned::to_owned),
    }
}

fn file_from_message(
    message: grammers_client::message::Message,
) -> Result<Option<TelegramFile>, TelegramError> {
    let Some(Media::Document(document)) = message.media() else {
        return Ok(None);
    };
    let Some(size) = document.size() else {
        return Ok(None);
    };
    let size_bytes =
        u64::try_from(size).map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
    let modified_at_unix_ms = message
        .edit_date()
        .unwrap_or_else(|| message.date())
        .timestamp_millis();
    Ok(Some(TelegramFile {
        message_id: i64::from(message.id()),
        modified_at_unix_ms,
        file_name: document.name().unwrap_or_default().to_owned(),
        caption: message.text().to_owned(),
        mime_type: document.mime_type().map(ToOwned::to_owned),
        size_bytes,
        document,
    }))
}

fn map_invocation(error: InvocationError) -> TelegramError {
    match error {
        InvocationError::Rpc(rpc) if rpc.code == 401 => {
            TelegramError::new(TelegramErrorKind::Authorization)
        }
        InvocationError::Rpc(rpc) if rpc.code == 420 => TelegramError::flood_wait(
            Duration::from_secs(u64::from(rpc.value.unwrap_or_default())),
        ),
        InvocationError::Session(_) => TelegramError::new(TelegramErrorKind::Session),
        InvocationError::Dropped => TelegramError::new(TelegramErrorKind::Cancelled),
        InvocationError::Io(_)
        | InvocationError::Deserialize(_)
        | InvocationError::Transport(_)
        | InvocationError::InvalidDc
        | InvocationError::Authentication(_)
        | InvocationError::Rpc(_) => TelegramError::new(TelegramErrorKind::Network),
    }
}

fn map_io(error: std::io::Error) -> TelegramError {
    match error.kind() {
        std::io::ErrorKind::NotFound => TelegramError::new(TelegramErrorKind::SourceMissing),
        std::io::ErrorKind::PermissionDenied => {
            TelegramError::new(TelegramErrorKind::PermissionDenied)
        }
        _ => TelegramError::new(TelegramErrorKind::Network),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_config_is_rejected_without_opening_a_session() {
        let error = validate_config(&TelegramConfig {
            api_id: 0,
            session_path: PathBuf::from("account.session"),
        })
        .expect_err("zero API ID must fail validation");
        assert_eq!(error.kind(), TelegramErrorKind::InvalidConfiguration);
    }

    #[test]
    fn request_limits_are_explicit_and_bounded() {
        assert!(validate_limit(0, MAX_DIALOGS_PER_REQUEST).is_err());
        assert!(validate_limit(1, MAX_DIALOGS_PER_REQUEST).is_ok());
        assert!(validate_limit(MAX_MESSAGES_PER_SCAN, MAX_MESSAGES_PER_SCAN).is_ok());
        assert!(validate_limit(MAX_MESSAGES_PER_SCAN + 1, MAX_MESSAGES_PER_SCAN).is_err());
        assert!(validate_limit(MAX_SEARCH_RESULTS, MAX_SEARCH_RESULTS).is_ok());
        assert!(validate_limit(MAX_SEARCH_RESULTS + 1, MAX_SEARCH_RESULTS).is_err());
    }

    #[test]
    fn flood_wait_preserves_machine_readable_retry_time() {
        let error = map_invocation(InvocationError::Rpc(grammers_mtsender::RpcError {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(17),
            caused_by: None,
        }));
        assert_eq!(error.kind(), TelegramErrorKind::FloodWait);
        assert_eq!(error.retry_after(), Some(Duration::from_secs(17)));
    }

    #[test]
    fn authorization_is_classified_without_display_text_matching() {
        let error = map_invocation(InvocationError::Rpc(grammers_mtsender::RpcError {
            code: 401,
            name: "AUTH_KEY_UNREGISTERED".to_owned(),
            value: None,
            caused_by: None,
        }));
        assert_eq!(error.kind(), TelegramErrorKind::Authorization);
    }
}
