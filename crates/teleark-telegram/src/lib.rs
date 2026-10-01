//! Telegram transport adapter for TeleArk.
//!
//! All `grammers` values stay private to this crate. Callers receive stable,
//! frontend-neutral records and structured errors instead.

use std::{
    collections::VecDeque,
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use grammers_client::{
    Client, SignInError,
    media::{Document, Media},
    message::InputMessage,
    tl,
};
use grammers_mtsender::{InvocationError, SenderPool};
use grammers_session::{
    Session as _,
    types::{PeerKind, PeerRef},
    updates::UpdatesLike,
};
use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};
use tokio::task::{JoinHandle, JoinSet};

mod download_stream;
pub use download_stream::download_stream_blocks;
mod upload;
pub use upload::{
    StreamUploadOptions, UPLOAD_PART_BYTES, UPLOAD_RESUME_WINDOW_MS, UploadCheckpoint, UploadStream,
};
mod tuning;
pub use tuning::TransferTuning;
mod bandwidth;
mod byte_progress;
use bandwidth::LimitedReader;
pub use bandwidth::{
    BandwidthBudget, BandwidthEvent, BandwidthEventKind, BandwidthSnapshot, BandwidthSubscription,
    TransferBandwidth, TransferSpeedLimits,
};
mod authorization;
mod connection;
mod qr_login_signal;
pub use authorization::{
    AuthorizationMonitor, AuthorizationPhase, AuthorizationSnapshot, AuthorizationUpdates,
};
use byte_progress::UploadReader;
pub use byte_progress::{ByteTransferEvent, ByteTransferObserver};
pub use qr_login_signal::QrLoginSignal;

mod channel_sync;
pub mod network;
mod publication;
mod session;
pub use publication::UploadPublicationOptions;
mod storage_channel;
pub use channel_sync::{
    ChannelDifferencePage, ChannelFileUpdate, ChannelPush, ChannelUpdateHints,
    ChannelUpdateSignals, ChannelWake,
};
pub use storage_channel::{StorageChannelHealth, StorageMaintenancePhase};

use session::FileSession;

const MAX_DIALOGS_PER_REQUEST: usize = 10_000;
const MAX_MESSAGES_PER_SCAN: usize = 10_000;
const MAX_SEARCH_RESULTS: usize = 1_000;
const RECENT_SEARCH_MESSAGES: usize = 512;
/// Small in-memory documents; large containers must use the disk/stream path.
pub const MAX_TRANSFER_OBJECT_BYTES: usize = 64 * 1024 * 1024;
/// Floor(1.9 GiB), including the container header and authentication tags.
pub const MAX_STREAM_OBJECT_BYTES: u64 = 19 * 1024 * 1024 * 1024 / 10;
const DOWNLOAD_CHUNK_SIZE: u64 = 128 * 1024;
pub const DOWNLOAD_PART_SIZE_BYTES: u64 = 1024 * 1024;
const MAX_DOWNLOAD_INFLIGHT_PARTS: usize = 64;
const MAX_DOWNLOAD_PART_ATTEMPTS: u32 = 4;
const DOWNLOAD_RETRY_BASE_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadPartState {
    Inflight,
    Completed,
    Retry,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadPartFailureKind {
    Timeout,
    Network,
    Server,
    RateLimited,
    Authorization,
    UnexpectedResponse,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadPartEvent {
    pub part_index: u64,
    pub offset_bytes: u64,
    pub length_bytes: u64,
    pub state: DownloadPartState,
    pub attempt: u32,
    pub elapsed_millis: u64,
    /// Zero-based local transfer connection slot, not a Telegram DC or network lane.
    pub connection_slot: u16,
    pub failure: Option<DownloadPartFailureKind>,
}

/// Cooperative command sampled between bounded Telegram download chunks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadControl {
    Continue,
    Pause,
    Cancel,
    /// Stops the current process owner while retaining the resumable partial.
    Stop,
}

/// Frontend-neutral progress and control surface for a native Telegram download.
///
/// Implementations must return quickly and must not expose secrets or file
/// content. Telegram invokes the observer after each durable in-process write.
pub trait DownloadReceiptObserver: Send + Sync {
    /// Cumulative bytes of this logical range confirmed by individual RPC replies.
    /// Retries may report the same prefix; observers must count only its increase.
    fn acknowledged(&self, part: u64, bytes: u64);
    fn completed(&self, _part: u64) {}
}

pub trait DownloadObserver: Send + Sync {
    fn control(&self) -> DownloadControl;
    /// Optional wake for pause/cancel/stop. Without one, active work uses a
    /// slower compatibility check while waiting for an RPC or retry deadline.
    fn control_updates(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        None
    }
    fn control_poll_fallback(&self) -> bool {
        true
    }
    fn progressed(&self, transferred_bytes: u64);
    fn receipt_observer(&self) -> Option<Arc<dyn DownloadReceiptObserver>> {
        None
    }

    fn desired_inflight_parts(&self) -> usize {
        4
    }

    fn desired_connections(&self) -> u16 {
        2
    }

    fn max_part_attempts(&self) -> u32 {
        MAX_DOWNLOAD_PART_ATTEMPTS
    }

    fn part_retry(&self, event: DownloadPartEvent, _server_wait: Option<Duration>) {
        self.part_event(event);
    }

    fn server_throttled(&self, _code: i32, _wait_seconds: u32) {}

    fn part_event(&self, _event: DownloadPartEvent) {}
}

/// Cooperative cancellation shared by a bounded Telegram history scan.
///
/// Cancellation wakes an in-flight network wait as well as being sampled
/// between returned messages, so abandoning one source cannot keep the
/// serialized desktop Telegram owner occupied indefinitely.
#[derive(Clone, Debug)]
pub struct ScanCancellation {
    sender: tokio::sync::watch::Sender<bool>,
}

impl Default for ScanCancellation {
    fn default() -> Self {
        let (sender, _) = tokio::sync::watch::channel(false);
        Self { sender }
    }
}

impl ScanCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Configuration that is safe to persist as ordinary application settings.
///
/// The Telegram API hash is intentionally not stored here. The application
/// must obtain and protect that value separately before requesting login.
#[derive(Clone)]
pub struct TelegramConfig {
    pub api_id: i32,
    pub session_path: PathBuf,
    pub network_route: network::NetworkRoute,
    pub network_monitor: network::NetworkMonitor,
    pub network_generation: u64,
    pub authorization_monitor: AuthorizationMonitor,
    pub qr_login_signal: QrLoginSignal,
}

/// Stable error categories used by Core and frontends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TelegramErrorKind {
    InvalidConfiguration,
    Session,
    Network,
    Timeout,
    UnexpectedResponse,
    Server,
    FloodWait,
    Authorization,
    SignUpRequired,
    InvalidCode,
    InvalidPassword,
    SourceMissing,
    PermissionDenied,
    StorageAccessDenied,
    StorageConfigurationUnsafe,
    StorageIdentityDamaged,
    StorageIdentityUnsupported,
    LimitExceeded,
    Cancelled,
    Interrupted,
}

/// Server rate limit / throttle status snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferServerStatus {
    pub code: i32,
    pub flag: String,
    pub wait_until_unix_ms: i64,
}

impl TransferServerStatus {
    pub fn wait_remaining_seconds(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        (self.wait_until_unix_ms.saturating_sub(now).max(0) as u64) / 1000
    }

    pub fn is_active(&self) -> bool {
        self.wait_remaining_seconds() > 0
    }
}

/// A locale-neutral Telegram adapter failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramError {
    kind: TelegramErrorKind,
    retry_after: Option<Duration>,
    server_code: Option<i32>,
    server_message: Option<String>,
}

impl TelegramError {
    pub const fn kind(&self) -> TelegramErrorKind {
        self.kind
    }

    pub const fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }

    pub const fn server_code(&self) -> Option<i32> {
        self.server_code
    }

    pub fn server_message(&self) -> Option<&str> {
        self.server_message.as_deref()
    }

    pub(crate) const fn new(kind: TelegramErrorKind) -> Self {
        Self {
            kind,
            retry_after: None,
            server_code: None,
            server_message: None,
        }
    }

    pub(crate) fn with_server_detail(mut self, code: i32, message: &str) -> Self {
        self.server_code = Some(code);
        self.server_message = Some(message.to_owned());
        self
    }

    pub(crate) const fn flood_wait(retry_after: Duration) -> Self {
        Self {
            kind: TelegramErrorKind::FloodWait,
            retry_after: Some(retry_after),
            server_code: Some(420),
            server_message: None,
        }
    }
}

impl fmt::Display for TelegramError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (
            self.kind,
            self.retry_after,
            self.server_code,
            self.server_message.as_deref(),
        ) {
            (TelegramErrorKind::FloodWait, Some(duration), Some(code), Some(msg)) => {
                write!(
                    formatter,
                    "Telegram requested a {} second retry delay ({code} {msg})",
                    duration.as_secs()
                )
            }
            (TelegramErrorKind::FloodWait, Some(duration), ..) => {
                write!(
                    formatter,
                    "Telegram requested a {} second retry delay",
                    duration.as_secs()
                )
            }
            (kind, _, Some(code), Some(msg)) => {
                write!(
                    formatter,
                    "Telegram adapter failure: {kind:?} ({code} {msg})"
                )
            }
            (kind, ..) => write!(formatter, "Telegram adapter failure: {kind:?}"),
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

/// A short-lived Telegram QR authorization link.
///
/// The link contains an authorization secret. Its debug representation is
/// intentionally redacted and callers must never persist it.
#[derive(Clone, Eq, PartialEq)]
pub struct QrLoginCode {
    deep_link: String,
    expires_at_unix_seconds: i64,
}

impl QrLoginCode {
    pub fn deep_link(&self) -> &str {
        &self.deep_link
    }

    pub const fn expires_at_unix_seconds(&self) -> i64 {
        self.expires_at_unix_seconds
    }
}

impl fmt::Debug for QrLoginCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QrLoginCode")
            .field("deep_link", &"[REDACTED]")
            .field("expires_at_unix_seconds", &self.expires_at_unix_seconds)
            .finish()
    }
}

pub enum QrLoginOutcome {
    Pending(QrLoginCode),
    Authorized(TelegramAccount),
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
    owned_channel: bool,
    sync_pts: Option<i32>,
}

impl TelegramChat {
    pub const fn sync_pts(&self) -> Option<i32> {
        self.sync_pts
    }
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
    sent_at_unix_ms: i64,
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

    pub const fn sent_at_unix_ms(&self) -> i64 {
        self.sent_at_unix_ms
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
    sync_client: Client,
    transfer_pools: tokio::sync::Mutex<Vec<TransferPool>>,
    gateway_url: String,
    session: Arc<FileSession>,
    api_id: i32,
    qr_login_signal: QrLoginSignal,
    download_flood_gate: Arc<DownloadFloodGate>,
    upload_flood_gate: Arc<DownloadFloodGate>,
    bandwidth: TransferBandwidth,
    runner: Option<JoinHandle<()>>,
    gateway: Option<JoinHandle<()>>,
    update_drain: Option<JoinHandle<()>>,
    channel_updates: ChannelUpdateSignals,
    authorization_monitor: AuthorizationMonitor,
    network_generation: u64,
    authorization_task: Option<JoinHandle<()>>,
}

impl TelegramConnection {
    /// Background-only, after every transport owner using this file is retired.
    pub fn clear_revoked_session(path: &Path) -> Result<(), TelegramError> {
        FileSession::open(path)
            .and_then(|session| session.clear_authorization())
            .map_err(|_| TelegramError::new(TelegramErrorKind::Session))
    }

    /// Opens an isolated Telegram session and starts its retained network tasks.
    pub async fn connect(config: TelegramConfig) -> Result<Self, TelegramError> {
        validate_config(&config)?;
        prepare_session_parent(&config.session_path).await?;

        let path = config.session_path.clone();
        let session = tokio::task::spawn_blocking(move || FileSession::open(&path))
            .await
            .map_err(|_| TelegramError::new(TelegramErrorKind::Session))?
            .map(Arc::new)
            .map_err(|_| TelegramError::new(TelegramErrorKind::Session))?;
        restrict_session_permissions(&config.session_path).await?;

        let gateway = network::Gateway::start(
            config.network_route,
            config.network_monitor.clone(),
            config.network_generation,
        )
        .await
        .map_err(|_| TelegramError::new(TelegramErrorKind::Network))?;
        let SenderPool {
            runner,
            handle,
            mut updates,
        } = sender_pool(
            Arc::clone(&session),
            config.api_id,
            gateway.proxy_url.clone(),
        );

        let sync_client = Client::with_configuration(
            handle.clone(),
            grammers_client::client::ClientConfiguration {
                retry_policy: Box::new(authorization::ObservedRetryPolicy {
                    monitor: config.authorization_monitor.clone(),
                    network_generation: config.network_generation,
                    inner: Box::new(grammers_client::client::NoRetries),
                }),
                ..Default::default()
            },
        );
        // This client deliberately does not feed its own failures back into the
        // event source: one failed confirmation cannot start a request loop.
        let check_client = Client::with_configuration(
            handle.clone(),
            grammers_client::client::ClientConfiguration {
                retry_policy: Box::new(grammers_client::client::NoRetries),
                ..Default::default()
            },
        );
        let disconnected = Arc::new(AtomicBool::new(false));
        let checked_connection = disconnected.clone();
        let authorization_task = tokio::spawn(authorization::confirm_events(
            config.authorization_monitor.clone(),
            config.network_generation,
            move || {
                let client = check_client.clone();
                let connected = checked_connection.clone();
                async move {
                    let result = client
                        .invoke(&tl::functions::updates::GetState {})
                        .await
                        .map(drop);
                    if result.is_ok() {
                        connected.store(false, Ordering::Release);
                    }
                    result
                }
            },
        ));
        let mut client_config = grammers_client::client::ClientConfiguration::default();
        client_config.retry_policy = Box::new(authorization::ObservedRetryPolicy {
            monitor: config.authorization_monitor.clone(),
            network_generation: config.network_generation,
            inner: client_config.retry_policy,
        });
        let client = Client::with_configuration(handle, client_config);
        let runner = tokio::spawn(runner.run());
        // Drain the transport continuously into bounded/coalesced channel PTS
        // hints. Durable differences, not these hints, advance the catalog.
        let channel_updates = ChannelUpdateSignals::default();
        let channel_signal = channel_updates.clone();
        let qr_login_signal = config.qr_login_signal.clone();
        let drain_signal = qr_login_signal.clone();
        let authorization_monitor = config.authorization_monitor.clone();
        let shared_flood_gate = Arc::new(DownloadFloodGate::default());
        let update_drain = tokio::spawn(async move {
            while let Some(update) = updates.recv().await {
                if matches!(&update, UpdatesLike::ConnectionClosed) {
                    if !disconnected.swap(true, Ordering::AcqRel) {
                        authorization_monitor.request_check(config.network_generation);
                    }
                    config
                        .network_monitor
                        .transport_closed(config.network_generation);
                } else if matches!(&update, UpdatesLike::Updates(_)) {
                    disconnected.store(false, Ordering::Release);
                }
                channel_signal.observe(&update);
                if contains_qr_login_update(&update) {
                    drain_signal.announce();
                }
            }
        });

        Ok(Self {
            client,
            sync_client,
            transfer_pools: tokio::sync::Mutex::new(Vec::new()),
            gateway_url: gateway.proxy_url,
            session,
            api_id: config.api_id,
            qr_login_signal,
            download_flood_gate: Arc::clone(&shared_flood_gate),
            upload_flood_gate: shared_flood_gate,
            bandwidth: TransferBandwidth::default(),
            runner: Some(runner),
            gateway: Some(gateway.task),
            update_drain: Some(update_drain),
            channel_updates,
            authorization_monitor: config.authorization_monitor,
            network_generation: config.network_generation,
            authorization_task: Some(authorization_task),
        })
    }

    pub fn server_throttle_status(&self) -> Option<TransferServerStatus> {
        self.download_flood_gate.status()
    }

    pub fn with_bandwidth(mut self, bandwidth: TransferBandwidth) -> Self {
        self.bandwidth = bandwidth;
        self
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

    /// Exports or refreshes a short-lived QR login token.
    ///
    /// Calling this method again is also how Telegram reports that a QR code
    /// scanned in an authorized mobile application has been accepted.
    pub async fn export_qr_login(
        &self,
        api_hash: &str,
        except_user_ids: &[i64],
    ) -> Result<QrLoginOutcome, TelegramError> {
        if api_hash.trim().is_empty() {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        self.qr_login_signal.clear();
        let request = tl::functions::auth::ExportLoginToken {
            api_id: self.api_id,
            api_hash: api_hash.to_owned(),
            except_ids: except_user_ids.to_vec(),
        };
        let response = self.client.invoke(&request).await.map_err(map_invocation)?;
        self.resolve_qr_response(response).await
    }

    /// Returns whether Telegram announced that the active QR token changed or
    /// was accepted. Reading consumes the signal.
    pub fn take_qr_login_update(&self) -> bool {
        self.qr_login_signal.take()
    }

    async fn resolve_qr_response(
        &self,
        response: tl::enums::auth::LoginToken,
    ) -> Result<QrLoginOutcome, TelegramError> {
        match response {
            tl::enums::auth::LoginToken::Token(token) => Ok(QrLoginOutcome::Pending(
                qr_login_code(token.token, token.expires)?,
            )),
            tl::enums::auth::LoginToken::Success(_) => self.qr_account().await,
            tl::enums::auth::LoginToken::MigrateTo(migration) => {
                validate_qr_migration(migration.dc_id, &migration.token)?;
                let imported = self
                    .client
                    .invoke_in_dc(
                        migration.dc_id,
                        &tl::functions::auth::ImportLoginToken {
                            token: migration.token,
                        },
                    )
                    .await
                    .map_err(map_invocation)?;
                self.session
                    .set_home_dc_id(migration.dc_id)
                    .await
                    .map_err(|_| TelegramError::new(TelegramErrorKind::Session))?;
                match imported {
                    tl::enums::auth::LoginToken::Success(_) => self.qr_account().await,
                    tl::enums::auth::LoginToken::Token(token) => Ok(QrLoginOutcome::Pending(
                        qr_login_code(token.token, token.expires)?,
                    )),
                    tl::enums::auth::LoginToken::MigrateTo(_) => {
                        Err(TelegramError::new(TelegramErrorKind::Network))
                    }
                }
            }
        }
    }

    async fn qr_account(&self) -> Result<QrLoginOutcome, TelegramError> {
        self.client
            .get_me()
            .await
            .map(|user| QrLoginOutcome::Authorized(account_from_user(&user)))
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
        list_dialogs_with_client(&self.client, limit).await
    }

    pub async fn list_sync_dialogs(
        &self,
        cancellation: &ScanCancellation,
    ) -> Result<Vec<TelegramChat>, TelegramError> {
        tokio::select! {
            _ = cancellation.cancelled() => Err(TelegramError::new(TelegramErrorKind::Cancelled)),
            result = list_dialogs_with_client(&self.sync_client, MAX_DIALOGS_PER_REQUEST) => result,
        }
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
        self.scan_file_page_cancellable(chat, before_message_id, limit, &ScanCancellation::new())
            .await
    }

    /// Scans one bounded history page and cooperatively cancels an in-flight
    /// Telegram request when the caller no longer needs its result.
    pub async fn scan_file_page_cancellable(
        &self,
        chat: &TelegramChat,
        before_message_id: Option<i64>,
        limit: usize,
        cancellation: &ScanCancellation,
    ) -> Result<TelegramFilePage, TelegramError> {
        scan_page_with_client(&self.client, chat, before_message_id, limit, cancellation).await
    }

    /// Bounded history requested by the sync owner; rate limits are surfaced
    /// immediately so the account scheduler can publish and honor the deadline.
    pub async fn scan_sync_history(
        &self,
        chat: &TelegramChat,
        before: Option<i64>,
        cancellation: &ScanCancellation,
    ) -> Result<TelegramFilePage, TelegramError> {
        if chat.kind != TelegramChatKind::Channel {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        scan_page_with_client(&self.sync_client, chat, before, 200, cancellation).await
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

    /// Searches exact document captions through Telegram's search index.
    pub async fn search_files_exact_caption(
        &self,
        chat: &TelegramChat,
        caption: &str,
        limit: usize,
    ) -> Result<Vec<TelegramFile>, TelegramError> {
        self.search_files_exact_caption_with_recent(chat, caption, limit, false)
            .await
    }

    /// Optionally supplements indexed results with bounded recent history,
    /// so freshly published manifests need not wait for search indexing.
    pub async fn search_files_exact_caption_with_recent(
        &self,
        chat: &TelegramChat,
        caption: &str,
        limit: usize,
        include_recent: bool,
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
        if !include_recent {
            return Ok(files);
        }
        let mut recent_messages = self
            .client
            .iter_messages(chat.peer_ref)
            .limit(RECENT_SEARCH_MESSAGES);
        let mut recent = Vec::new();
        while let Some(message) = recent_messages.next().await.map_err(map_invocation)? {
            if message.text() != caption {
                continue;
            }
            if let Some(file) = file_from_message(message)? {
                recent.push(file);
            }
        }
        Ok(merge_search_candidates(files, recent, limit, |file| {
            file.message_id
        }))
    }

    /// Downloads a previously indexed document to an explicit caller-selected path.
    pub async fn download_file(
        &self,
        file: &TelegramFile,
        destination: impl AsRef<Path>,
    ) -> Result<(), TelegramError> {
        self.download_file_observed(file, destination, &UncontrolledDownload)
            .await
    }

    /// Downloads a document with resumable partial-file handling, cooperative
    /// pause/cancel control, and chunk-level byte progress.
    pub async fn download_file_observed(
        &self,
        file: &TelegramFile,
        destination: impl AsRef<Path>,
        observer: &dyn DownloadObserver,
    ) -> Result<(), TelegramError> {
        check_download_control(observer)?;
        let destination = destination.as_ref();
        validate_download_destination(destination).await?;
        let partial = partial_download_path(destination)?;
        let part_map_path = partial_download_map_path(destination)?;
        let mut part_map =
            prepare_download_part_map(&partial, &part_map_path, file.size_bytes).await?;
        let mut received = part_map.completed_bytes();
        observer.progressed(received);
        let result = async {
            check_download_control(observer)?;
            if part_map.is_complete() {
                let _ = tokio::fs::remove_file(&part_map_path).await;
                publish_partial(&partial, destination).await?;
                return Ok(());
            }
            let mut output = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&partial)
                .await
                .map_err(map_io)?;
            let mut missing_parts = part_map
                .missing_parts()
                .into_iter()
                .map(|part_index| PendingDownloadPart {
                    part_index,
                    attempt: 1,
                    ready_at: Instant::now(),
                })
                .collect::<VecDeque<_>>();
            let clients = self.transfer_clients(false, observer.desired_connections()).await;
            let mut timed_out_slots = vec![false; clients.len()];
            let mut inflight_downloads = JoinSet::new();
            let flood_gate = Arc::clone(&self.download_flood_gate);
            let mut control_updates = observer.control_updates();
            loop {
                check_download_control(observer)?;
                let desired_inflight = observer
                    .desired_inflight_parts()
                    .clamp(1, MAX_DOWNLOAD_INFLIGHT_PARTS);
                while inflight_downloads.len() < desired_inflight && flood_gate.remaining().is_zero() {
                    let Some(pending_part) = take_ready_part(&mut missing_parts, Instant::now()) else {
                        break;
                    };
                    let part_index = pending_part.part_index;
                    let (offset_bytes, length_bytes) = part_map.part_range(part_index)?;
                    let connection_slot = choose_download_connection_slot(
                        part_index,
                        pending_part.attempt,
                        &timed_out_slots,
                    );
                    observer.part_event(DownloadPartEvent {
                        part_index,
                        offset_bytes,
                        length_bytes,
                        state: DownloadPartState::Inflight,
                        attempt: pending_part.attempt,
                        elapsed_millis: 0,
                        connection_slot: connection_slot as u16,
                        failure: None,
                    });
                    let client = clients[connection_slot].clone();
                    let document = file.document.clone();
                    let flood_gate = Arc::clone(&flood_gate);
                    let bandwidth = self.bandwidth.download.clone();
                    let receipts = observer.receipt_observer();
                    inflight_downloads.spawn(async move {
                        let attempt_started = Instant::now();
                        let result = download_logical_part(
                            client,
                            document,
                            part_index,
                            offset_bytes,
                            length_bytes,
                            flood_gate,
                            bandwidth,
                            receipts,
                            None,
                            pending_part.attempt as u16,
                        )
                        .await;
                        (
                            part_index,
                            offset_bytes,
                            length_bytes,
                            pending_part.attempt,
                            connection_slot,
                            u64::try_from(attempt_started.elapsed().as_millis())
                                .unwrap_or(u64::MAX),
                            result,
                        )
                    });
                }
                if inflight_downloads.is_empty() && missing_parts.is_empty() { break; }
                // Join completions, control changes and actual retry/FloodWait
                // deadlines wake the owner. No short transfer polling is needed.
                let retry_at = if inflight_downloads.len() < desired_inflight {
                    let flood_ready = Instant::now() + flood_gate.remaining();
                    missing_parts
                        .iter()
                        .map(|part| part.ready_at.max(flood_ready))
                        .min()
                } else {
                    None
                };
                let completion = tokio::select! {
                    joined = inflight_downloads.join_next(), if !inflight_downloads.is_empty() => joined,
                    () = wait_download_retry(retry_at) => continue,
                    () = wait_download_control(&mut control_updates, observer.control_poll_fallback()) => continue,
                };
                let (
                    part_index,
                    offset_bytes,
                    length_bytes,
                    attempt,
                    connection_slot,
                    attempt_elapsed_millis,
                    joined,
                ) = completion
                    .ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))?
                    .map_err(|_| TelegramError::new(TelegramErrorKind::Network))?;
                let downloaded = match joined {
                    Ok(downloaded) => downloaded,
                    Err(error) => {
                        if error.kind() == TelegramErrorKind::Timeout {
                            timed_out_slots[connection_slot] = true;
                        }
                        let failure = Some(download_part_failure_kind(&error));
                        let is_flood = error.kind() == TelegramErrorKind::FloodWait;
                        let next_attempt = if is_flood { attempt } else { attempt.saturating_add(1) };
                        let retry_delay = if is_flood {
                            error.retry_after()
                        } else {
                            download_part_retry_delay(&error, attempt, observer.max_part_attempts())
                        };
                        if let Some(retry_delay) = retry_delay {
                            if is_flood {
                                let wait_secs = error
                                    .retry_after()
                                    .map_or(1, |d| d.as_secs().min(u32::MAX as u64) as u32);
                                observer.server_throttled(
                                    error.server_code().unwrap_or(420),
                                    wait_secs,
                                );
                            }
                            observer.part_retry(DownloadPartEvent {
                                part_index,
                                offset_bytes,
                                length_bytes,
                                state: DownloadPartState::Retry,
                                attempt,
                                elapsed_millis: attempt_elapsed_millis,
                                connection_slot: connection_slot as u16,
                                failure,
                            }, error.retry_after());
                            missing_parts.push_front(PendingDownloadPart {
                                part_index,
                                attempt: next_attempt,
                                ready_at: Instant::now() + retry_delay,
                            });
                            continue;
                        }
                        observer.part_event(DownloadPartEvent {
                            part_index,
                            offset_bytes,
                            length_bytes,
                            state: DownloadPartState::Failed,
                            attempt,
                            elapsed_millis: attempt_elapsed_millis,
                            connection_slot: connection_slot as u16,
                            failure,
                        });
                        inflight_downloads.abort_all();
                        while inflight_downloads.join_next().await.is_some() {}
                        return Err(error);
                    }
                };
                output
                    .seek(std::io::SeekFrom::Start(downloaded.offset_bytes))
                    .await
                    .map_err(map_io)?;
                output.write_all(&downloaded.bytes).await.map_err(map_io)?;
                output.flush().await.map_err(map_io)?;
                part_map.mark_completed(downloaded.part_index)?;
                persist_download_part_map(&part_map_path, &part_map).await?;
                received = part_map.completed_bytes();
                observer.progressed(received);
                if let Some(receipts) = observer.receipt_observer() { receipts.completed(part_index); }
                observer.part_event(DownloadPartEvent {
                    part_index: downloaded.part_index,
                    offset_bytes: downloaded.offset_bytes,
                    length_bytes: downloaded.bytes.len() as u64,
                    state: DownloadPartState::Completed,
                    attempt,
                    elapsed_millis: downloaded.elapsed_millis,
                    connection_slot: connection_slot as u16,
                    failure: None,
                });
            }
            if !part_map.is_complete() || received != file.size_bytes {
                return Err(TelegramError::new(TelegramErrorKind::Network));
            }
            output.flush().await.map_err(map_io)?;
            output.sync_all().await.map_err(map_io)?;
            drop(output);
            check_download_control(observer)?;
            tokio::fs::remove_file(&part_map_path)
                .await
                .map_err(map_io)?;
            publish_partial(&partial, destination).await?;
            Ok(())
        }
        .await;
        if result
            .as_ref()
            .is_err_and(|error| error.kind() == TelegramErrorKind::Cancelled)
        {
            let _ = tokio::fs::remove_file(&partial).await;
            let _ = tokio::fs::remove_file(&part_map_path).await;
        }
        result
    }

    /// Downloads one bounded transfer object without creating a temporary
    /// ciphertext file. Application-part policy keeps this buffer bounded.
    pub async fn download_bytes(&self, file: &TelegramFile) -> Result<Vec<u8>, TelegramError> {
        self.download_bytes_observed(file, None).await
    }

    pub async fn download_bytes_observed(
        &self,
        file: &TelegramFile,
        observer: Option<&dyn ByteTransferObserver>,
    ) -> Result<Vec<u8>, TelegramError> {
        let expected = usize::try_from(file.size_bytes)
            .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
        if expected > MAX_TRANSFER_OBJECT_BYTES {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        if let Some(observer) = observer {
            observer.observe(ByteTransferEvent::Downloading {
                bytes: 0,
                total: expected as u64,
            });
        }
        let tuning = observer.map_or_else(
            TransferTuning::default,
            ByteTransferObserver::transfer_tuning,
        );
        if !tuning.validate() {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        let clients = self
            .transfer_clients(false, tuning.download_connections)
            .await;
        // Allocate/zero the bounded object on a blocking owner, away from the reactor.
        let mut bytes = tokio::task::spawn_blocking(move || vec![0u8; expected])
            .await
            .map_err(|_| TelegramError::new(TelegramErrorKind::Session))?;
        let mut next = 0usize;
        let mut received = 0usize;
        let mut inflight = JoinSet::new();
        while received < expected {
            while next < expected && inflight.len() < usize::from(tuning.download_parts) {
                let offset = next;
                let length = (expected - next).min(DOWNLOAD_PART_SIZE_BYTES as usize);
                next += length;
                let index = offset as u64 / DOWNLOAD_PART_SIZE_BYTES;
                let client = clients[index as usize % clients.len()].clone();
                let document = file.document.clone();
                let flood_gate = self.download_flood_gate.clone();
                let bandwidth = self.bandwidth.download.clone();
                inflight.spawn(async move {
                    for attempt in 1..=u32::from(tuning.download_attempts) {
                        let result = download_logical_part(
                            client.clone(),
                            document.clone(),
                            index,
                            offset as u64,
                            length as u64,
                            flood_gate.clone(),
                            bandwidth.clone(),
                            None,
                            None,
                            attempt as u16,
                        )
                        .await;
                        match result {
                            Ok(part) => return Ok(part),
                            Err(error) => {
                                let Some(delay) = download_part_retry_delay(
                                    &error,
                                    attempt,
                                    u32::from(tuning.download_attempts),
                                ) else {
                                    return Err(error);
                                };
                                tokio::time::sleep(delay).await;
                            }
                        }
                    }
                    Err(TelegramError::new(TelegramErrorKind::Network))
                });
            }
            let part = inflight
                .join_next()
                .await
                .ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))?
                .map_err(|_| TelegramError::new(TelegramErrorKind::Network))??;
            let offset = part.offset_bytes as usize;
            bytes[offset..offset + part.bytes.len()].copy_from_slice(&part.bytes);
            received += part.bytes.len();
            if let Some(observer) = observer {
                observer.observe(ByteTransferEvent::Downloading {
                    bytes: received as u64,
                    total: expected as u64,
                });
            }
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
        let file = tokio::fs::File::open(source).await.map_err(map_io)?;
        let mut stream = LimitedReader::new(file, size, self.bandwidth.upload.clone());
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
        self.upload_bytes_observed(chat, bytes, file_name, caption, None)
            .await
    }

    pub async fn upload_bytes_observed(
        &self,
        chat: &TelegramChat,
        bytes: &[u8],
        file_name: &str,
        caption: &str,
        observer: Option<&dyn ByteTransferObserver>,
    ) -> Result<SentDocument, TelegramError> {
        self.upload_bytes_with_publication(
            chat,
            bytes,
            file_name,
            caption,
            UploadPublicationOptions {
                observer,
                random_id: None,
            },
        )
        .await
    }

    /// Reuses a durably reserved publication ID across ambiguous sends. Callers
    /// must bind the ID to immutable content and reconcile missing receipts.
    pub async fn upload_bytes_with_publication(
        &self,
        chat: &TelegramChat,
        bytes: &[u8],
        file_name: &str,
        caption: &str,
        options: UploadPublicationOptions<'_>,
    ) -> Result<SentDocument, TelegramError> {
        let UploadPublicationOptions {
            observer,
            random_id: publication_random_id,
        } = options;
        if publication_random_id == Some(0)
            || bytes.is_empty()
            || bytes.len() > MAX_TRANSFER_OBJECT_BYTES
            || file_name.is_empty()
            || caption.is_empty()
        {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let mut stream = LimitedReader::new(
            UploadReader::new(bytes, observer),
            bytes.len(),
            self.bandwidth.upload.clone(),
        );
        let uploaded = self
            .client
            .upload_stream(&mut stream, bytes.len(), file_name.to_owned())
            .await
            .map_err(map_io)?;
        if let Some(observer) = observer {
            observer.observe(ByteTransferEvent::SendingMessage);
        }
        if let Some(random_id) = publication_random_id {
            let request = publication::document_request(
                chat.peer_ref.into(),
                uploaded.raw,
                file_name,
                caption,
                random_id,
            );
            let updates = self.client.invoke(&request).await.map_err(map_invocation)?;
            let message_id = publication::sent_message_id(updates, random_id)
                .filter(|id| *id > 0)
                .ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))?;
            return Ok(SentDocument {
                message_id: i64::from(message_id),
            });
        }
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
        if let Some(task) = self.authorization_task.take() {
            task.abort();
        }
        self.client.disconnect();
        if let Some(task) = self.gateway.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(task) = self.runner.take() {
            let _ = task.await;
        }
        if let Some(task) = self.update_drain.take() {
            let _ = task.await;
        }
    }
}

struct UncontrolledDownload;

impl DownloadObserver for UncontrolledDownload {
    fn control(&self) -> DownloadControl {
        DownloadControl::Continue
    }

    fn progressed(&self, _transferred_bytes: u64) {}

    fn control_poll_fallback(&self) -> bool {
        false
    }
}

fn check_download_control(observer: &dyn DownloadObserver) -> Result<(), TelegramError> {
    match observer.control() {
        DownloadControl::Continue => Ok(()),
        DownloadControl::Pause | DownloadControl::Stop => {
            Err(TelegramError::new(TelegramErrorKind::Interrupted))
        }
        DownloadControl::Cancel => Err(TelegramError::new(TelegramErrorKind::Cancelled)),
    }
}

async fn wait_download_retry(ready_at: Option<Instant>) {
    match ready_at {
        Some(ready_at) => tokio::time::sleep_until(ready_at.into()).await,
        None => std::future::pending().await,
    }
}

async fn wait_download_control(
    updates: &mut Option<tokio::sync::watch::Receiver<u64>>,
    fallback: bool,
) {
    match updates {
        Some(receiver) => {
            if receiver.changed().await.is_err() {
                // Compatibility fallback if the observer owner disappeared.
                *updates = None;
            }
        }
        None if fallback => tokio::time::sleep(Duration::from_secs(5)).await,
        None => std::future::pending().await,
    }
}

fn download_part_retry_delay(
    error: &TelegramError,
    attempt: u32,
    max_attempts: u32,
) -> Option<Duration> {
    if attempt >= max_attempts.clamp(1, 8) {
        return None;
    }
    match error.kind() {
        TelegramErrorKind::FloodWait => error.retry_after(),
        TelegramErrorKind::Network
        | TelegramErrorKind::Timeout
        | TelegramErrorKind::UnexpectedResponse
        | TelegramErrorKind::Server => Some(
            DOWNLOAD_RETRY_BASE_DELAY.saturating_mul(1_u32 << attempt.saturating_sub(1).min(5)),
        ),
        _ => None,
    }
}

fn random_jitter(max_millis: u64) -> Duration {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_ok() {
        let val = u64::from_le_bytes(bytes);
        Duration::from_millis(val % max_millis)
    } else {
        Duration::from_millis(500)
    }
}

/// Shared by every chunk in this native download. A server wait extends the
/// deadline immediately in the worker that receives it, before joining results.
#[derive(Default)]
pub(crate) struct DownloadFloodGate {
    deadline: std::sync::Mutex<Option<Instant>>,
    last_status: std::sync::Mutex<Option<TransferServerStatus>>,
}

impl DownloadFloodGate {
    pub(crate) fn extend_with_status(&self, delay: Duration, code: i32, flag: &str) {
        let mut deadline = self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let jitter = random_jitter(1000) + Duration::from_millis(250);
        let next = now + delay + jitter;
        let previous_deadline = *deadline;
        let actual_deadline = previous_deadline.map_or(next, |old| old.max(next));
        let new_wait_supplies_deadline = previous_deadline.is_none_or(|old| next >= old);
        *deadline = Some(actual_deadline);

        let status_now = Instant::now();
        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| {
                duration.as_millis().min(i64::MAX as u128) as i64
            });
        let actual_wait_ms = actual_deadline
            .saturating_duration_since(status_now)
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let wait_until_unix_ms = now_unix_ms.saturating_add(actual_wait_ms);
        let mut status = self
            .last_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (status_code, status_flag) = if new_wait_supplies_deadline {
            (code, flag.to_owned())
        } else if let Some(previous) = status.as_ref() {
            (previous.code, previous.flag.clone())
        } else {
            (code, flag.to_owned())
        };
        *status = Some(TransferServerStatus {
            code: status_code,
            flag: status_flag,
            wait_until_unix_ms,
        });
    }

    #[allow(dead_code)]
    pub(crate) fn extend(&self, delay: Duration) {
        self.extend_with_status(delay, 420, "FLOOD_WAIT");
    }

    pub(crate) fn remaining(&self) -> Duration {
        self.deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map_or(Duration::ZERO, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            })
    }

    pub(crate) fn status(&self) -> Option<TransferServerStatus> {
        let status = self
            .last_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        if status.is_active() {
            Some(status)
        } else {
            None
        }
    }

    pub(crate) async fn wait(&self) {
        loop {
            let delay = self.remaining();
            if delay.is_zero() {
                return;
            }
            tokio::time::sleep(delay).await;
        }
    }
}

struct DownloadedLogicalPart {
    part_index: u64,
    offset_bytes: u64,
    bytes: Vec<u8>,
    elapsed_millis: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DownloadChunkActivityUpdate {
    Started {
        index: u64,
        attempt: u16,
        now: Instant,
    },
    RetryScheduled {
        index: u64,
        attempt: u16,
        delay: Duration,
        now: Instant,
    },
    FloodWaitRetryScheduled {
        index: u64,
        attempt: u16,
        delay: Duration,
        code: i32,
        wait_until_unix_ms: i64,
        now: Instant,
    },
    WaitingForGate {
        index: u64,
        attempt: u16,
        delay: Duration,
        server_code: Option<i32>,
        server_wait_until_unix_ms: Option<i64>,
        now: Instant,
    },
    Finished {
        index: u64,
        now: Instant,
    },
}

pub(crate) type DownloadChunkActivitySink =
    Arc<dyn Fn(DownloadChunkActivityUpdate) + Send + Sync + 'static>;

#[derive(Clone, Copy)]
struct PendingDownloadPart {
    part_index: u64,
    attempt: u32,
    ready_at: Instant,
}

fn take_ready_part(
    queue: &mut VecDeque<PendingDownloadPart>,
    now: Instant,
) -> Option<PendingDownloadPart> {
    let position = queue.iter().position(|part| part.ready_at <= now)?;
    queue.remove(position)
}

fn choose_download_connection_slot(
    part_index: u64,
    attempt: u32,
    timed_out_slots: &[bool],
) -> usize {
    let count = timed_out_slots.len();
    debug_assert!(count > 0);
    let first = (part_index as usize % count + attempt.saturating_sub(1) as usize) % count;
    (0..count)
        .map(|step| (first + step) % count)
        .find(|&slot| !timed_out_slots[slot])
        .unwrap_or(first)
}

fn download_part_failure_kind(error: &TelegramError) -> DownloadPartFailureKind {
    match error.kind() {
        TelegramErrorKind::Timeout => DownloadPartFailureKind::Timeout,
        TelegramErrorKind::UnexpectedResponse => DownloadPartFailureKind::UnexpectedResponse,
        TelegramErrorKind::Network => DownloadPartFailureKind::Network,
        TelegramErrorKind::Server => DownloadPartFailureKind::Server,
        TelegramErrorKind::FloodWait => DownloadPartFailureKind::RateLimited,
        TelegramErrorKind::Authorization => DownloadPartFailureKind::Authorization,
        _ => DownloadPartFailureKind::Other,
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_logical_part(
    client: Client,
    document: Document,
    part_index: u64,
    offset_bytes: u64,
    length_bytes: u64,
    flood_gate: Arc<DownloadFloodGate>,
    bandwidth: BandwidthBudget,
    receipts: Option<Arc<dyn DownloadReceiptObserver>>,
    activity: Option<DownloadChunkActivitySink>,
    attempt: u16,
) -> Result<DownloadedLogicalPart, TelegramError> {
    let chunk_size = bandwidth.download_chunk_size().min(UPLOAD_PART_BYTES);
    let skipped_chunks = i32::try_from(offset_bytes / chunk_size as u64)
        .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
    let expected_length = usize::try_from(length_bytes)
        .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
    let mut bytes = Vec::with_capacity(expected_length);
    let started = Instant::now();
    let mut download = client
        .iter_download(&document)
        .chunk_size(chunk_size as i32)
        .skip_chunks(skipped_chunks);
    let mut activity_started = false;
    while bytes.len() < expected_length {
        let gate_wait = flood_gate.remaining();
        if !gate_wait.is_zero()
            && let Some(activity) = &activity
        {
            let gate_status = flood_gate.status();
            activity(DownloadChunkActivityUpdate::WaitingForGate {
                index: part_index,
                attempt,
                delay: gate_wait,
                server_code: gate_status.as_ref().map(|status| status.code),
                server_wait_until_unix_ms: gate_status
                    .as_ref()
                    .map(|status| status.wait_until_unix_ms),
                now: Instant::now(),
            });
            activity_started = false;
        }
        flood_gate.wait().await;
        bandwidth
            .acquire(chunk_size.min(expected_length - bytes.len()))
            .await;
        if !activity_started {
            if let Some(activity) = &activity {
                activity(DownloadChunkActivityUpdate::Started {
                    index: part_index,
                    attempt,
                    now: Instant::now(),
                });
            }
            activity_started = true;
        }
        let chunk = tokio::time::timeout(Duration::from_secs(60), download.next())
            .await
            .map_err(|_| TelegramError::new(TelegramErrorKind::Timeout))?
            .map_err(|error| {
                let error = map_invocation(error);
                if let Some(delay) = error.retry_after() {
                    flood_gate.extend_with_status(
                        delay,
                        error.server_code().unwrap_or(420),
                        error.server_message().unwrap_or("FLOOD_WAIT"),
                    );
                }
                error
            })?
            .ok_or_else(|| TelegramError::new(TelegramErrorKind::UnexpectedResponse))?;
        let remaining = expected_length.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            return Err(TelegramError::new(TelegramErrorKind::UnexpectedResponse));
        }
        bytes.extend_from_slice(&chunk);
        if let Some(receipts) = &receipts {
            receipts.acknowledged(part_index, bytes.len() as u64);
        }
    }
    Ok(DownloadedLogicalPart {
        part_index,
        offset_bytes,
        bytes,
        elapsed_millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

const DOWNLOAD_PART_MAP_MAGIC: &[u8; 8] = b"TARKDPM1";
const DOWNLOAD_PART_MAP_HEADER_BYTES: usize = 40;

struct DownloadPartMap {
    total_bytes: u64,
    part_count: usize,
    completed: Vec<u8>,
    completed_part_count: usize,
    completed_byte_count: u64,
}

impl DownloadPartMap {
    fn new(total_bytes: u64) -> Result<Self, TelegramError> {
        let part_count = total_bytes.div_ceil(DOWNLOAD_PART_SIZE_BYTES);
        let part_count = usize::try_from(part_count)
            .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
        Ok(Self {
            total_bytes,
            part_count,
            completed: vec![0; part_count.div_ceil(8)],
            completed_part_count: 0,
            completed_byte_count: 0,
        })
    }

    fn part_range(&self, part_index: u64) -> Result<(u64, u64), TelegramError> {
        let position = usize::try_from(part_index)
            .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
        if position >= self.part_count {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let offset = part_index
            .checked_mul(DOWNLOAD_PART_SIZE_BYTES)
            .ok_or_else(|| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
        Ok((
            offset,
            self.total_bytes
                .saturating_sub(offset)
                .min(DOWNLOAD_PART_SIZE_BYTES),
        ))
    }

    fn mark_completed(&mut self, part_index: u64) -> Result<(), TelegramError> {
        let (_, length) = self.part_range(part_index)?;
        let position = part_index as usize; // checked by part_range
        let byte = &mut self.completed[position / 8];
        let bit = 1 << (position % 8);
        if *byte & bit == 0 {
            *byte |= bit;
            self.completed_part_count += 1;
            self.completed_byte_count += length;
        }
        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.completed_part_count == self.part_count
    }

    fn completed_bytes(&self) -> u64 {
        self.completed_byte_count
    }

    fn missing_parts(&self) -> VecDeque<u64> {
        (0..self.part_count)
            .filter(|position| self.completed[position / 8] & (1 << (position % 8)) == 0)
            .map(|position| position as u64)
            .collect()
    }

    fn encode(&self) -> Result<Vec<u8>, TelegramError> {
        let mut output = Vec::with_capacity(DOWNLOAD_PART_MAP_HEADER_BYTES + self.completed.len());
        output.extend_from_slice(DOWNLOAD_PART_MAP_MAGIC);
        output.extend_from_slice(&DOWNLOAD_PART_SIZE_BYTES.to_be_bytes());
        output.extend_from_slice(&self.total_bytes.to_be_bytes());
        output.extend_from_slice(&(self.part_count as u64).to_be_bytes());
        output.extend_from_slice(&(self.completed.len() as u64).to_be_bytes());
        output.extend_from_slice(&self.completed);
        Ok(output)
    }

    fn decode(bytes: &[u8], expected_total_bytes: u64) -> Result<Self, TelegramError> {
        if bytes.len() < DOWNLOAD_PART_MAP_HEADER_BYTES
            || bytes.get(..8) != Some(DOWNLOAD_PART_MAP_MAGIC)
        {
            return Err(TelegramError::new(TelegramErrorKind::Network));
        }
        let read_u64 = |start: usize| {
            bytes
                .get(start..start + 8)
                .and_then(|value| value.try_into().ok())
                .map(u64::from_be_bytes)
                .ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))
        };
        let part_size = read_u64(8)?;
        let total_bytes = read_u64(16)?;
        let part_count = read_u64(24)?;
        let bit_bytes = read_u64(32)?;
        let expected_part_count = expected_total_bytes.div_ceil(DOWNLOAD_PART_SIZE_BYTES);
        let expected_bit_bytes = expected_part_count.div_ceil(8);
        if part_size != DOWNLOAD_PART_SIZE_BYTES
            || total_bytes != expected_total_bytes
            || part_count != expected_part_count
            || bit_bytes != expected_bit_bytes
            || bytes.len() as u64 != DOWNLOAD_PART_MAP_HEADER_BYTES as u64 + bit_bytes
        {
            return Err(TelegramError::new(TelegramErrorKind::Network));
        }
        let part_count = usize::try_from(part_count)
            .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
        let mut completed = bytes[DOWNLOAD_PART_MAP_HEADER_BYTES..].to_vec();
        // Version 1 ignores unused high bits; preserve its reader behavior and
        // canonical writer bytes instead of assigning new meaning to those bits.
        if !part_count.is_multiple_of(8)
            && let Some(last) = completed.last_mut()
        {
            *last &= (1 << (part_count % 8)) - 1;
        }
        let completed_part_count = completed
            .iter()
            .map(|byte| byte.count_ones() as usize)
            .sum::<usize>();
        let tail_length = total_bytes % DOWNLOAD_PART_SIZE_BYTES;
        let tail_completed = tail_length != 0
            && completed
                .last()
                .is_some_and(|last| last & (1 << ((part_count - 1) % 8)) != 0);
        let completed_byte_count = ((completed_part_count - usize::from(tail_completed)) as u64)
            * DOWNLOAD_PART_SIZE_BYTES
            + if tail_completed { tail_length } else { 0 };
        Ok(Self {
            total_bytes,
            part_count,
            completed,
            completed_part_count,
            completed_byte_count,
        })
    }
}

async fn prepare_download_part_map(
    partial: &Path,
    map_path: &Path,
    expected_bytes: u64,
) -> Result<DownloadPartMap, TelegramError> {
    let part_map = match tokio::fs::read(map_path).await {
        Ok(bytes) => {
            let metadata = tokio::fs::symlink_metadata(partial).await.map_err(map_io)?;
            if !metadata.file_type().is_file() || metadata.len() != expected_bytes {
                return Err(TelegramError::new(TelegramErrorKind::Network));
            }
            DownloadPartMap::decode(&bytes, expected_bytes)?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let resumable_bytes = prepare_partial_download(partial, expected_bytes).await?;
            let mut map = DownloadPartMap::new(expected_bytes)?;
            let completed_parts = resumable_bytes / DOWNLOAD_PART_SIZE_BYTES;
            for part_index in 0..completed_parts {
                map.mark_completed(part_index)?;
            }
            map
        }
        Err(error) => return Err(map_io(error)),
    };
    let output = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(partial)
        .await
        .map_err(map_io)?;
    output.set_len(expected_bytes).await.map_err(map_io)?;
    if part_map.part_count != expected_bytes.div_ceil(DOWNLOAD_PART_SIZE_BYTES) as usize {
        return Err(TelegramError::new(TelegramErrorKind::Network));
    }
    persist_download_part_map(map_path, &part_map).await?;
    Ok(part_map)
}

async fn persist_download_part_map(
    map_path: &Path,
    part_map: &DownloadPartMap,
) -> Result<(), TelegramError> {
    let bytes = part_map.encode()?;
    let temporary = map_path.with_extension("map.tmp");
    tokio::fs::write(&temporary, bytes).await.map_err(map_io)?;
    restrict_download_permissions(&temporary).await?;
    tokio::fs::rename(&temporary, map_path)
        .await
        .map_err(map_io)?;
    restrict_download_permissions(map_path).await
}

async fn restrict_download_permissions(path: &Path) -> Result<(), TelegramError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(map_io)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

async fn prepare_partial_download(
    partial: &Path,
    expected_bytes: u64,
) -> Result<u64, TelegramError> {
    let existing_bytes = match tokio::fs::symlink_metadata(partial).await {
        Ok(metadata) if metadata.file_type().is_file() => metadata.len(),
        Ok(_) => return Err(TelegramError::new(TelegramErrorKind::PermissionDenied)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(map_io(error)),
    };
    if existing_bytes > expected_bytes {
        return Err(TelegramError::new(TelegramErrorKind::Network));
    }
    let resumable_bytes = if existing_bytes == expected_bytes {
        existing_bytes
    } else {
        existing_bytes - (existing_bytes % DOWNLOAD_CHUNK_SIZE)
    };
    let output = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(partial)
        .await
        .map_err(map_io)?;
    if resumable_bytes != existing_bytes {
        output.set_len(resumable_bytes).await.map_err(map_io)?;
    }
    Ok(resumable_bytes)
}

impl Drop for TelegramConnection {
    fn drop(&mut self) {
        if let Some(task) = self.authorization_task.take() {
            task.abort();
        }
        self.client.disconnect();
        if let Some(task) = self.gateway.take() {
            task.abort();
        }
        if let Some(task) = self.runner.take() {
            task.abort();
        }
        if let Some(task) = self.update_drain.take() {
            task.abort();
        }
    }
}

// One factory for every API, sync, avatar and transfer client. The gateway URL
// is mandatory, including in direct mode; no caller can omit proxy routing.
fn sender_pool<S>(session: Arc<S>, api_id: i32, gateway_url: String) -> SenderPool
where
    S: grammers_session::Session + Sized,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    SenderPool::with_configuration(
        session,
        api_id,
        grammers_mtsender::ConnectionParams {
            proxy_url: Some(gateway_url),
            ..connection::params()
        },
    )
}

fn validate_config(config: &TelegramConfig) -> Result<(), TelegramError> {
    if config.api_id <= 0 || config.session_path.as_os_str().is_empty() {
        return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
    }
    Ok(())
}

fn qr_login_code(token: Vec<u8>, expires: i32) -> Result<QrLoginCode, TelegramError> {
    if token.is_empty() || expires <= 0 {
        return Err(TelegramError::new(TelegramErrorKind::Network));
    }
    Ok(QrLoginCode {
        deep_link: format!("tg://login?token={}", URL_SAFE_NO_PAD.encode(token)),
        expires_at_unix_seconds: i64::from(expires),
    })
}

fn contains_qr_login_update(update: &UpdatesLike) -> bool {
    let contains = |updates: &[tl::enums::Update]| {
        updates
            .iter()
            .any(|update| matches!(update, tl::enums::Update::LoginToken))
    };
    match update {
        UpdatesLike::Updates(tl::enums::Updates::UpdateShort(update)) => {
            matches!(update.update, tl::enums::Update::LoginToken)
        }
        UpdatesLike::Updates(tl::enums::Updates::Combined(updates)) => contains(&updates.updates),
        UpdatesLike::Updates(tl::enums::Updates::Updates(updates)) => contains(&updates.updates),
        _ => false,
    }
}

fn validate_qr_migration(dc_id: i32, token: &[u8]) -> Result<(), TelegramError> {
    if dc_id <= 0 || token.is_empty() {
        Err(TelegramError::new(TelegramErrorKind::Network))
    } else {
        Ok(())
    }
}

async fn validate_download_destination(destination: &Path) -> Result<(), TelegramError> {
    if destination.as_os_str().is_empty() || destination.file_name().is_none() {
        return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
    }
    match tokio::fs::symlink_metadata(destination).await {
        Ok(_) => Err(TelegramError::new(TelegramErrorKind::PermissionDenied)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(map_io(error)),
    }
}

fn partial_download_path(destination: &Path) -> Result<PathBuf, TelegramError> {
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| TelegramError::new(TelegramErrorKind::InvalidConfiguration))?;
    Ok(destination.with_file_name(format!(".{file_name}.teleark-partial")))
}

fn partial_download_map_path(destination: &Path) -> Result<PathBuf, TelegramError> {
    let mut partial = partial_download_path(destination)?.into_os_string();
    partial.push(".map");
    Ok(PathBuf::from(partial))
}

/// Paths exclusively owned by a native download before final publication.
/// Batch planners reserve these alongside destinations so a remote filename
/// cannot collide with another member's private partial or checkpoint map.
pub fn native_download_artifact_paths(destination: &Path) -> Result<[PathBuf; 2], TelegramError> {
    Ok([
        partial_download_path(destination)?,
        partial_download_map_path(destination)?,
    ])
}

/// Removes the private resumable partial for an explicitly cancelled native
/// download. A missing partial is already the desired state.
pub fn discard_partial_download(destination: impl AsRef<Path>) -> Result<(), TelegramError> {
    let destination = destination.as_ref();
    let partial = partial_download_path(destination)?;
    let map = partial_download_map_path(destination)?;
    for path in [partial, map] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(map_io(error)),
        }
    }
    Ok(())
}

/// Retains only a valid, useful resume pair after a failed native download.
/// Terminal failures and incomplete/corrupt pairs are removed so they cannot
/// accumulate as unreachable managed data.
pub fn cleanup_failed_partial_download(
    destination: impl AsRef<Path>,
    expected_bytes: u64,
    retain_for_resume: bool,
) -> Result<(), TelegramError> {
    let destination = destination.as_ref();
    if !retain_for_resume {
        return discard_partial_download(destination);
    }
    let partial = partial_download_path(destination)?;
    let map = partial_download_map_path(destination)?;
    let resume_pair_is_useful = match (std::fs::symlink_metadata(&partial), std::fs::read(&map)) {
        (Ok(metadata), Ok(encoded_map)) => {
            metadata.file_type().is_file()
                && metadata.len() == expected_bytes
                && DownloadPartMap::decode(&encoded_map, expected_bytes)
                    .is_ok_and(|part_map| part_map.completed_bytes() > 0)
        }
        (Err(error), _) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(map_io(error));
        }
        (_, Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(map_io(error));
        }
        _ => false,
    };
    if resume_pair_is_useful {
        Ok(())
    } else {
        discard_partial_download(destination)
    }
}

async fn publish_partial(partial: &Path, destination: &Path) -> Result<(), TelegramError> {
    // Both paths are siblings, so a hard link provides atomic no-replace
    // publication on supported desktop filesystems. Unlike rename, it cannot
    // overwrite a destination created after the initial collision check.
    tokio::fs::hard_link(partial, destination)
        .await
        .map_err(map_io)?;
    let _ = tokio::fs::remove_file(partial).await;
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
    #[cfg(not(unix))]
    let _ = path;
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
        sent_at_unix_ms: message.date().timestamp_millis(),
        modified_at_unix_ms,
        file_name: document.name().unwrap_or_default().to_owned(),
        caption: message.text().to_owned(),
        mime_type: document.mime_type().map(ToOwned::to_owned),
        size_bytes,
        document,
    }))
}

fn is_flood_or_slowmode(name: &str) -> bool {
    name.starts_with("FLOOD_WAIT")
        || name.starts_with("FLOOD_PREMIUM_WAIT")
        || name.starts_with("SLOWMODE_WAIT")
}

fn parse_trailing_seconds(name: &str) -> Option<u32> {
    name.rsplit('_').next().and_then(|s| s.parse::<u32>().ok())
}

fn map_invocation(error: InvocationError) -> TelegramError {
    match error {
        InvocationError::Rpc(rpc) if rpc.code == 401 => {
            TelegramError::new(TelegramErrorKind::Authorization)
                .with_server_detail(rpc.code, &rpc.name)
        }
        InvocationError::Rpc(rpc) if rpc.code == 420 || is_flood_or_slowmode(&rpc.name) => {
            let seconds = rpc
                .value
                .or_else(|| parse_trailing_seconds(&rpc.name))
                .unwrap_or(1);
            TelegramError::flood_wait(Duration::from_secs(u64::from(seconds)))
                .with_server_detail(rpc.code, &rpc.name)
        }
        InvocationError::Rpc(rpc) if rpc.code >= 500 => {
            TelegramError::new(TelegramErrorKind::Server).with_server_detail(rpc.code, &rpc.name)
        }
        InvocationError::Rpc(rpc) => {
            TelegramError::new(TelegramErrorKind::Network).with_server_detail(rpc.code, &rpc.name)
        }
        InvocationError::Session(_) => TelegramError::new(TelegramErrorKind::Session),
        InvocationError::Dropped => TelegramError::new(TelegramErrorKind::Cancelled),
        InvocationError::Io(_)
        | InvocationError::Deserialize(_)
        | InvocationError::Transport(_)
        | InvocationError::InvalidDc
        | InvocationError::Authentication(_) => TelegramError::new(TelegramErrorKind::Network),
    }
}

fn map_io(error: std::io::Error) -> TelegramError {
    match error.kind() {
        std::io::ErrorKind::NotFound => TelegramError::new(TelegramErrorKind::SourceMissing),
        std::io::ErrorKind::PermissionDenied => {
            TelegramError::new(TelegramErrorKind::PermissionDenied)
        }
        std::io::ErrorKind::AlreadyExists => {
            TelegramError::new(TelegramErrorKind::PermissionDenied)
        }
        _ => TelegramError::new(TelegramErrorKind::Network),
    }
}

/// Recent history wins when the same message is present in both responses.
/// Newest messages remain visible when the caller's result bound is reached.
fn merge_search_candidates<T>(
    indexed: Vec<T>,
    recent: Vec<T>,
    limit: usize,
    id: impl Fn(&T) -> i64,
) -> Vec<T> {
    let mut unique = std::collections::BTreeMap::new();
    for file in indexed.into_iter().chain(recent) {
        unique.insert(id(&file), file);
    }
    unique
        .into_iter()
        .rev()
        .take(limit)
        .map(|(_, file)| file)
        .collect()
}

async fn scan_page_with_client(
    client: &Client,
    chat: &TelegramChat,
    before_message_id: Option<i64>,
    limit: usize,
    cancellation: &ScanCancellation,
) -> Result<TelegramFilePage, TelegramError> {
    validate_limit(limit, MAX_MESSAGES_PER_SCAN)?;
    if cancellation.is_cancelled() {
        return Err(TelegramError::new(TelegramErrorKind::Cancelled));
    }
    let offset = before_message_id
        .map(i32::try_from)
        .transpose()
        .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?
        .unwrap_or_default();

    let mut messages = client
        .iter_messages(chat.peer_ref)
        .offset_id(offset)
        .limit(limit);
    let mut files = Vec::new();
    let mut examined = 0_usize;
    let mut last_message_id = None;
    loop {
        let next = tokio::select! {
            _ = cancellation.cancelled() => {
                return Err(TelegramError::new(TelegramErrorKind::Cancelled));
            }
            next = messages.next() => next.map_err(map_invocation)?,
        };
        let Some(message) = next else {
            break;
        };
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

async fn list_dialogs_with_client(
    client: &Client,
    limit: usize,
) -> Result<Vec<TelegramChat>, TelegramError> {
    validate_limit(limit, MAX_DIALOGS_PER_REQUEST)?;
    require_authorized(client).await?;

    let mut dialogs = client.iter_dialogs();
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
            sync_pts: match &dialog.raw {
                tl::enums::Dialog::Dialog(dialog) => dialog.pts,
                _ => None,
            },
            id,
            name: peer.name().unwrap_or_default().to_owned(),
            username: peer.username().map(ToOwned::to_owned),
            kind,
            peer_ref: dialog.peer_ref(),
            owned_channel: matches!(peer, grammers_client::peer::Peer::Channel(channel)
                    if channel.raw.creator),
        });
    }
    Ok(chats)
}

struct TransferPool {
    upload: bool,
    client: Client,
    runner: JoinHandle<()>,
    drain: JoinHandle<()>,
}
impl Drop for TransferPool {
    fn drop(&mut self) {
        self.client.disconnect();
        self.runner.abort();
        self.drain.abort();
    }
}
impl TelegramConnection {
    async fn transfer_clients(&self, upload: bool, count: u16) -> Vec<Client> {
        let mut pools = self.transfer_pools.lock().await;
        while pools.iter().filter(|p| p.upload == upload).count() < usize::from(count.clamp(1, 8)) {
            let SenderPool {
                runner,
                handle,
                mut updates,
            } = sender_pool(
                Arc::clone(&self.session),
                self.api_id,
                self.gateway_url.clone(),
            );
            let client = Client::with_configuration(
                handle,
                grammers_client::client::ClientConfiguration {
                    retry_policy: Box::new(authorization::ObservedRetryPolicy {
                        monitor: self.authorization_monitor.clone(),
                        network_generation: self.network_generation,
                        inner: Box::new(grammers_client::client::NoRetries),
                    }),
                    ..Default::default()
                },
            );
            pools.push(TransferPool {
                upload,
                client,
                runner: tokio::spawn(runner.run()),
                drain: tokio::spawn(async move { while updates.recv().await.is_some() {} }),
            });
        }
        pools
            .iter()
            .filter(|p| p.upload == upload)
            .take(usize::from(count.clamp(1, 8)))
            .map(|p| p.client.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_history_fills_a_lagging_index_and_deduplicates_publications() {
        assert_eq!(
            merge_search_candidates(
                Vec::<(i64, &str)>::new(),
                vec![(7, "new manifest")],
                10,
                |file| file.0
            ),
            vec![(7, "new manifest")]
        );
        assert_eq!(
            merge_search_candidates(
                vec![(7, "indexed"), (4, "older")],
                vec![(7, "fresh metadata"), (8, "not indexed yet")],
                2,
                |file| file.0
            ),
            vec![(8, "not indexed yet"), (7, "fresh metadata")]
        );
        assert!(merge_search_candidates(vec![(7, "indexed")], vec![], 0, |file| file.0).is_empty());
    }

    #[test]
    fn invalid_config_is_rejected_without_opening_a_session() {
        let error = validate_config(&TelegramConfig {
            api_id: 0,
            session_path: PathBuf::from("account.session"),
            network_route: network::NetworkRoute::Direct,
            network_monitor: network::NetworkMonitor::new(&network::NetworkRoute::Direct),
            network_generation: 0,
            authorization_monitor: Default::default(),
            qr_login_signal: Default::default(),
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

    #[tokio::test]
    async fn scan_cancellation_is_shared_idempotent_and_wakes_waiters() {
        let cancellation = ScanCancellation::new();
        let observer = cancellation.clone();
        assert!(!observer.is_cancelled());
        let waiter = tokio::spawn(async move { observer.cancelled().await });
        tokio::task::yield_now().await;
        cancellation.cancel();
        cancellation.cancel();
        waiter.await.expect("cancellation waiter");
        assert!(cancellation.is_cancelled());
        cancellation.cancelled().await;
    }

    #[test]
    fn server_rpc_failure_is_not_a_transport_or_authentication_failure() {
        for (code, expected) in [
            (500, TelegramErrorKind::Server),
            (503, TelegramErrorKind::Server),
            (401, TelegramErrorKind::Authorization),
        ] {
            let error = map_invocation(InvocationError::Rpc(grammers_mtsender::RpcError {
                code,
                name: "RPC_CALL_FAIL".to_owned(),
                value: None,
                caused_by: Some(0xa0f4cb4f),
            }));
            assert_eq!(error.kind(), expected);
        }
        assert_eq!(
            map_invocation(InvocationError::Io(std::io::Error::from(
                std::io::ErrorKind::TimedOut
            )))
            .kind(),
            TelegramErrorKind::Network
        );
    }

    #[test]
    fn flood_wait_and_slowmode_parse_codes_and_durations() {
        for (code, name, expected_secs) in [
            (420, "FLOOD_WAIT_17", 17),
            (400, "FLOOD_PREMIUM_WAIT_300", 300),
            (400, "SLOWMODE_WAIT_60", 60),
        ] {
            let error = map_invocation(InvocationError::Rpc(grammers_mtsender::RpcError {
                code,
                name: name.to_owned(),
                value: None,
                caused_by: None,
            }));
            assert_eq!(error.kind(), TelegramErrorKind::FloodWait);
            assert_eq!(
                error.retry_after(),
                Some(Duration::from_secs(expected_secs))
            );
            assert_eq!(error.server_code(), Some(code));
            assert_eq!(error.server_message(), Some(name));
            assert!(
                error
                    .to_string()
                    .contains(&format!("{expected_secs} second"))
            );
            assert!(error.to_string().contains(name));
        }
    }

    #[test]
    fn shared_flood_gate_tracks_server_status_and_jitter() {
        let gate = DownloadFloodGate::default();
        assert!(gate.status().is_none());
        let before = Instant::now();
        gate.extend_with_status(Duration::from_secs(45), 420, "FLOOD_WAIT_45");
        let after = Instant::now();
        // Check the scheduled deadline: the remaining countdown can already be
        // below the minimum when jitter is small or this thread is descheduled.
        let deadline = gate
            .deadline
            .lock()
            .expect("deadline lock must be available")
            .expect("deadline must be present");
        assert!(deadline >= before + Duration::from_millis(45_250));
        assert!(deadline <= after + Duration::from_millis(46_249));
        let status = gate.status().expect("status must be present");
        assert_eq!(status.code, 420);
        assert_eq!(status.flag, "FLOOD_WAIT_45");
        assert!(status.is_active());
        assert!(status.wait_remaining_seconds() >= 44);
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
        assert_eq!(
            download_part_retry_delay(&error, 1, 4),
            Some(Duration::from_secs(17))
        );
    }

    #[test]
    fn delayed_retry_does_not_block_ready_parts_or_run_before_deadline() {
        let now = Instant::now();
        let mut queue = VecDeque::from([
            PendingDownloadPart {
                part_index: 0,
                attempt: 2,
                ready_at: now + Duration::from_secs(2),
            },
            PendingDownloadPart {
                part_index: 1,
                attempt: 1,
                ready_at: now,
            },
        ]);
        assert_eq!(
            take_ready_part(&mut queue, now)
                .expect("valid test fixture")
                .part_index,
            1
        );
        assert!(take_ready_part(&mut queue, now + Duration::from_secs(1)).is_none());
        let retry =
            take_ready_part(&mut queue, now + Duration::from_secs(2)).expect("valid test fixture");
        assert_eq!((retry.part_index, retry.attempt), (0, 2));
        assert!(queue.is_empty());
    }

    #[tokio::test]
    async fn flood_wait_workers_remain_cancellable_without_sending_chunks() {
        let gate = Arc::new(DownloadFloodGate::default());
        gate.extend(Duration::from_secs(120));
        let sent = Arc::new(AtomicBool::new(false));
        let mut workers = JoinSet::new();
        for _ in 0..64 {
            let gate = Arc::clone(&gate);
            let sent = Arc::clone(&sent);
            workers.spawn(async move {
                gate.wait().await;
                sent.store(true, Ordering::Release);
            });
        }
        tokio::task::yield_now().await;
        assert!(!sent.load(Ordering::Acquire));
        workers.abort_all();
        while let Some(result) = workers.join_next().await {
            assert!(result.expect_err("fixture must fail").is_cancelled());
        }
        assert!(!sent.load(Ordering::Acquire));
    }

    #[test]
    fn aggressive_retries_are_bounded_and_do_not_override_server_delays() {
        let network = TelegramError::new(TelegramErrorKind::Network);
        assert_eq!(
            download_part_retry_delay(&network, 4, 8),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            download_part_retry_delay(&network, 7, 8),
            Some(Duration::from_secs(8))
        );
        assert_eq!(download_part_retry_delay(&network, 8, 100), None);
        let flood = TelegramError::flood_wait(Duration::from_secs(120));
        assert_eq!(
            download_part_retry_delay(&flood, 7, 8),
            Some(Duration::from_secs(120))
        );
        assert_eq!(download_part_retry_delay(&flood, 8, 8), None);
    }

    #[test]
    fn shared_flood_gate_never_shortens_an_existing_deadline() {
        let gate = Arc::new(DownloadFloodGate::default());
        let second_part = Arc::clone(&gate);
        gate.extend_with_status(Duration::from_secs(120), 420, "FLOOD_WAIT_120");
        second_part.extend_with_status(Duration::from_secs(1), 429, "FLOOD_WAIT_1");
        assert!(gate.remaining() > Duration::from_secs(110));
        assert!(second_part.remaining() > Duration::from_secs(110));
        let status = gate.status().expect("active shared wait status");
        assert_eq!(status.code, 420);
        assert_eq!(status.flag, "FLOOD_WAIT_120");
        assert!(status.wait_remaining_seconds() > 110);
    }

    #[test]
    fn network_part_retry_is_bounded_with_exponential_backoff() {
        let network = TelegramError::new(TelegramErrorKind::Network);
        assert_eq!(
            download_part_retry_delay(&network, 1, 4),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            download_part_retry_delay(&network, 2, 4),
            Some(Duration::from_millis(500))
        );
        assert_eq!(download_part_retry_delay(&network, 4, 4), None);
        assert_eq!(
            download_part_retry_delay(&TelegramError::new(TelegramErrorKind::Authorization), 1, 8),
            None
        );
    }

    #[test]
    fn timed_out_connection_is_not_reused_for_retries_or_later_parts() {
        let mut timed_out_slots = [false; 8];
        assert_eq!(choose_download_connection_slot(5, 1, &timed_out_slots), 5);
        timed_out_slots[5] = true;
        assert_eq!(choose_download_connection_slot(5, 2, &timed_out_slots), 6);
        assert_eq!(choose_download_connection_slot(13, 1, &timed_out_slots), 6);
        assert_eq!(choose_download_connection_slot(21, 1, &timed_out_slots), 6);
        timed_out_slots[6] = true;
        assert_eq!(choose_download_connection_slot(5, 3, &timed_out_slots), 7);
        assert_eq!(
            download_part_retry_delay(&TelegramError::new(TelegramErrorKind::Timeout), 1, 4),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            download_part_failure_kind(&TelegramError::new(TelegramErrorKind::Timeout)),
            DownloadPartFailureKind::Timeout
        );
        assert_eq!(
            download_part_retry_delay(&TelegramError::new(TelegramErrorKind::Timeout), 4, 4),
            None
        );
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

    #[test]
    fn qr_login_link_uses_unpadded_url_safe_base64_and_redacts_debug() {
        let code = qr_login_code(vec![0xfb, 0xff, 0x00], 1_900_000_000)
            .expect("valid token should become a login link");
        assert_eq!(code.deep_link(), "tg://login?token=-_8A");
        assert_eq!(code.expires_at_unix_seconds(), 1_900_000_000);
        let debug = format!("{code:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("-_8A"));
    }

    #[test]
    fn malformed_qr_token_responses_are_rejected() {
        assert_eq!(
            qr_login_code(Vec::new(), 1)
                .expect_err("empty token must fail")
                .kind(),
            TelegramErrorKind::Network
        );
        assert_eq!(
            qr_login_code(vec![1], 0)
                .expect_err("non-positive expiry must fail")
                .kind(),
            TelegramErrorKind::Network
        );
    }

    #[test]
    fn qr_login_update_is_detected_without_consuming_other_updates() {
        let login = UpdatesLike::Updates(tl::enums::Updates::UpdateShort(tl::types::UpdateShort {
            update: tl::enums::Update::LoginToken,
            date: 123,
        }));
        assert!(contains_qr_login_update(&login));

        let unrelated = UpdatesLike::Updates(tl::enums::Updates::TooLong);
        assert!(!contains_qr_login_update(&unrelated));
    }

    #[test]
    fn qr_migration_requires_a_real_datacenter_and_import_token() {
        assert!(validate_qr_migration(4, b"import-token").is_ok());
        assert_eq!(
            validate_qr_migration(0, b"import-token")
                .expect_err("invalid DC must fail")
                .kind(),
            TelegramErrorKind::Network
        );
        assert_eq!(
            validate_qr_migration(4, b"")
                .expect_err("empty import token must fail")
                .kind(),
            TelegramErrorKind::Network
        );
    }

    #[test]
    fn download_destination_is_collision_safe_and_has_private_partial_name() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("Tokio runtime");
        let directory = tempfile::tempdir().expect("temporary directory");
        let destination = directory.path().join("report.pdf");
        assert!(
            runtime
                .block_on(validate_download_destination(&destination))
                .is_ok()
        );
        assert_eq!(
            partial_download_path(&destination).expect("partial path"),
            directory.path().join(".report.pdf.teleark-partial")
        );
        std::fs::write(&destination, b"existing").expect("write fixture");
        assert_eq!(
            runtime
                .block_on(validate_download_destination(&destination))
                .expect_err("existing destination must not be replaced")
                .kind(),
            TelegramErrorKind::PermissionDenied
        );
    }

    #[test]
    fn partial_publication_is_atomic_and_never_replaces_a_racing_destination() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("Tokio runtime");
        let directory = tempfile::tempdir().expect("temporary directory");
        let partial = directory.path().join(".file.teleark-partial");
        let destination = directory.path().join("file.bin");
        std::fs::write(&partial, b"complete").expect("write partial");
        runtime
            .block_on(publish_partial(&partial, &destination))
            .expect("publish partial");
        assert_eq!(
            std::fs::read(&destination).expect("final bytes"),
            b"complete"
        );
        assert!(!partial.exists());

        std::fs::write(&partial, b"replacement").expect("write second partial");
        let error = runtime
            .block_on(publish_partial(&partial, &destination))
            .expect_err("existing final path must win the race");
        assert_eq!(error.kind(), TelegramErrorKind::PermissionDenied);
        assert_eq!(
            std::fs::read(&destination).expect("original final"),
            b"complete"
        );
    }

    #[test]
    fn interrupted_partial_is_truncated_to_a_safe_resumable_chunk() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("Tokio runtime");
        let directory = tempfile::tempdir().expect("temporary directory");
        let partial = directory.path().join(".resume.teleark-partial");
        std::fs::write(&partial, vec![7_u8; DOWNLOAD_CHUNK_SIZE as usize + 123])
            .expect("write interrupted partial");
        let resume = runtime
            .block_on(prepare_partial_download(&partial, DOWNLOAD_CHUNK_SIZE * 3))
            .expect("prepare resumable partial");
        assert_eq!(resume, DOWNLOAD_CHUNK_SIZE);
        assert_eq!(
            std::fs::metadata(&partial).expect("partial metadata").len(),
            resume
        );
        discard_partial_download(directory.path().join("resume"))
            .expect("discard resumable partial");
        assert!(!partial.exists());
    }

    fn legacy_part_map(total: u64, completed: &[bool]) -> (Vec<u8>, u64, bool) {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(b"TARKDPM1");
        for value in [
            DOWNLOAD_PART_SIZE_BYTES,
            total,
            completed.len() as u64,
            completed.len().div_ceil(8) as u64,
        ] {
            encoded.extend_from_slice(&value.to_be_bytes());
        }
        encoded.resize(40 + completed.len().div_ceil(8), 0);
        let mut received = 0;
        for (index, complete) in completed.iter().enumerate() {
            if *complete {
                encoded[40 + index / 8] |= 1 << (index % 8);
                received += (total - (index as u64) * DOWNLOAD_PART_SIZE_BYTES)
                    .min(DOWNLOAD_PART_SIZE_BYTES);
            }
        }
        (
            encoded,
            received,
            completed.iter().all(|complete| *complete),
        )
    }

    #[test]
    fn packed_part_map_preserves_v1_bytes_and_incremental_counts() {
        let fixture: &[u8] = include_bytes!("fixtures/native-part-map-v1.bin");
        let map = DownloadPartMap::decode(fixture, DOWNLOAD_PART_SIZE_BYTES * 2 + 17)
            .expect("version 1 fixture");
        assert_eq!(map.encode().expect("canonical codec"), fixture);
        assert_eq!(map.completed_bytes(), DOWNLOAD_PART_SIZE_BYTES + 17);
        let mut ignored_high_bits = fixture.to_vec();
        ignored_high_bits[40] |= 0b1111_1000;
        assert_eq!(
            DownloadPartMap::decode(&ignored_high_bits, map.total_bytes)
                .expect("old reader ignores high bits")
                .encode()
                .expect("canonical"),
            fixture
        );
        for total in [
            0,
            1,
            DOWNLOAD_PART_SIZE_BYTES,
            DOWNLOAD_PART_SIZE_BYTES * 9 + 17,
            DOWNLOAD_PART_SIZE_BYTES * 4096 + 7,
        ] {
            let mut map = DownloadPartMap::new(total).expect("map");
            let mut reference = vec![false; map.part_count];
            assert_eq!(map.completed.len(), reference.len().div_ceil(8));
            for index in 0..map.part_count {
                let part = index * 37 % map.part_count;
                reference[part] = true;
                map.mark_completed(part as u64).expect("complete");
                map.mark_completed(part as u64)
                    .expect("idempotent duplicate");
                let (bytes, received, complete) = legacy_part_map(total, &reference);
                assert_eq!(map.encode().expect("encode"), bytes);
                assert_eq!(map.completed_bytes(), received);
                assert_eq!(map.is_complete(), complete);
                let restored = DownloadPartMap::decode(&bytes, total).expect("resume old bytes");
                assert_eq!(restored.completed_bytes(), received);
                assert_eq!(restored.is_complete(), complete);
            }
            assert!(map.is_complete());
            assert_eq!(map.completed_bytes(), total);
            assert!(map.missing_parts().is_empty());
            assert!(map.mark_completed(map.part_count as u64).is_err());
        }
    }

    #[test]
    #[ignore = "manual local algorithm comparison, no wall-time pass threshold"]
    fn perf_download_part_map() {
        let total = DOWNLOAD_PART_SIZE_BYTES * 10_000;
        let mut reference = vec![false; 10_000];
        reference[..5000].fill(true);
        let mut map =
            DownloadPartMap::decode(&legacy_part_map(total, &reference).0, total).expect("map");
        let started = Instant::now();
        for part in 5000..6000 {
            reference[part] = true;
            std::hint::black_box(legacy_part_map(total, &reference));
        }
        let previous_us = started.elapsed().as_micros();
        let started = Instant::now();
        for part in 5000..6000 {
            map.mark_completed(part).expect("complete");
            std::hint::black_box((
                map.encode().expect("encode"),
                map.completed_bytes(),
                map.is_complete(),
            ));
        }
        let packed_us = started.elapsed().as_micros();
        assert_eq!(
            map.encode().expect("encode"),
            legacy_part_map(total, &reference).0
        );
        eprintln!(
            "part-map: 10000 parts, 1000 completions; previous_us={previous_us}, packed_us={packed_us}; metadata bytes old=10000 new={}",
            map.completed.len()
        );
    }

    #[test]
    fn one_mib_part_bitmap_round_trips_out_of_order_completion() {
        let mut map =
            DownloadPartMap::new(DOWNLOAD_PART_SIZE_BYTES * 2 + 17).expect("valid part map");
        map.mark_completed(2).expect("complete tail first");
        map.mark_completed(0).expect("complete head second");
        let encoded = map.encode().expect("encode map");
        let decoded = DownloadPartMap::decode(&encoded, DOWNLOAD_PART_SIZE_BYTES * 2 + 17)
            .expect("decode map");
        assert_eq!(decoded.completed_bytes(), DOWNLOAD_PART_SIZE_BYTES + 17);
        assert_eq!(decoded.missing_parts(), VecDeque::from([1]));
        assert!(!decoded.is_complete());
    }

    #[test]
    fn failed_partial_cleanup_keeps_only_a_useful_valid_resume_pair() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let destination = directory.path().join("resume.bin");
        let partial = partial_download_path(&destination).expect("partial path");
        let map_path = partial_download_map_path(&destination).expect("map path");
        let expected_bytes = DOWNLOAD_PART_SIZE_BYTES * 2;
        let partial_file = std::fs::File::create(&partial).expect("create partial");
        partial_file
            .set_len(expected_bytes)
            .expect("size partial fixture");
        let empty_map = DownloadPartMap::new(expected_bytes).expect("empty map");
        std::fs::write(&map_path, empty_map.encode().expect("encode empty map"))
            .expect("write empty map");
        cleanup_failed_partial_download(&destination, expected_bytes, true)
            .expect("discard zero-progress pair");
        assert!(!partial.exists());
        assert!(!map_path.exists());

        let partial_file = std::fs::File::create(&partial).expect("recreate partial");
        partial_file
            .set_len(expected_bytes)
            .expect("resize partial fixture");
        let mut useful_map = DownloadPartMap::new(expected_bytes).expect("useful map");
        useful_map.mark_completed(1).expect("mark completed part");
        std::fs::write(&map_path, useful_map.encode().expect("encode useful map"))
            .expect("write useful map");
        cleanup_failed_partial_download(&destination, expected_bytes, true)
            .expect("retain useful pair");
        assert!(partial.exists());
        assert!(map_path.exists());

        cleanup_failed_partial_download(&destination, expected_bytes, false)
            .expect("terminal cleanup");
        assert!(!partial.exists());
        assert!(!map_path.exists());
    }

    #[test]
    fn part_bitmap_rejects_wrong_file_identity_and_trailing_bytes() {
        let map = DownloadPartMap::new(DOWNLOAD_PART_SIZE_BYTES + 1).expect("valid map");
        let mut encoded = map.encode().expect("encode map");
        assert!(DownloadPartMap::decode(&encoded, DOWNLOAD_PART_SIZE_BYTES + 2).is_err());
        encoded.push(0);
        assert!(DownloadPartMap::decode(&encoded, DOWNLOAD_PART_SIZE_BYTES + 1).is_err());
    }
    #[test]
    #[ignore = "manual temporary-disk checkpoint cost probe; no timing assertion"]
    fn perf_download_checkpoint_disk() {
        use tokio::io::AsyncWriteExt as _;
        let directory = tempfile::tempdir().expect("temporary directory");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let bytes = vec![0x55; DOWNLOAD_PART_SIZE_BYTES as usize];
            for checkpoints in [false, true] {
                let path = directory
                    .path()
                    .join(if checkpoints { "mapped" } else { "plain" });
                let mut file = tokio::fs::File::create(&path).await.expect("create");
                let mut map = DownloadPartMap::new(64 * DOWNLOAD_PART_SIZE_BYTES).expect("map");
                let started = std::time::Instant::now();
                for part in 0..64 {
                    file.write_all(&bytes).await.expect("write");
                    file.flush().await.expect("flush");
                    map.mark_completed(part).expect("mark");
                    if checkpoints {
                        persist_download_part_map(&path.with_extension("map"), &map)
                            .await
                            .expect("checkpoint");
                    }
                }
                file.sync_all().await.expect("final sync");
                eprintln!(
                    "temporary disk MiB=64 per_part_map={checkpoints} elapsed_us={}",
                    started.elapsed().as_micros()
                );
            }
        });
    }
}
