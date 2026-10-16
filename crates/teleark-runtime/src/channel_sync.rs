//! Account-owned channel synchronization. Selecting a source changes priority,
//! never freshness. All public snapshots are local and cheap to read.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use teleark_core::{AccountId, ApplicationError, ApplicationErrorKind, ChatId};
use teleark_storage::{ChannelSyncCommit, ChannelSyncState};

use crate::{
    DesktopLibrary, DesktopTelegram, StorageRequest, TelegramAccount, TelegramChatKind,
    TelegramChatSummary, TelegramFileSummary, TelegramScanCancellation,
};

pub(crate) mod directory;
mod execution;
mod feed;
mod history;
pub use history::HistoryStatus;
mod manifest_activity;
pub use feed::{ChannelChanges, ChannelDelta, ChannelSyncSubscription};
pub use manifest_activity::{ManagedScanObserver, ManagedScanStatus};

const MAX_SOURCES: usize = 10_000;
const EVENT_CAPACITY: usize = 128;
const COMMAND_CAPACITY: usize = 64;
/// Delivery-silence policy shared with frontend explanations; this is not a UI timer.
pub const CHANNEL_UPDATE_SILENCE_RECOVERY_MINUTES: u64 = 7;
const UPDATE_SILENCE_RECOVERY: Duration =
    Duration::from_secs(CHANNEL_UPDATE_SILENCE_RECOVERY_MINUTES * 60);
type SyncClock = Arc<dyn Fn() -> Instant + Send + Sync>;
type SyncEntropy = Arc<dyn Fn(i64, u32) -> u64 + Send + Sync>;

struct RetryControls {
    clock: SyncClock,
    entropy: SyncEntropy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelSyncPhase {
    ManifestQueued,
    ManifestReading,
    ManifestReceiving,
    ManifestVerifying,
    ManifestCompleted,
    ManifestFailed,
    ManifestCancelled,
    Queued,
    ReadingLocal,
    Connecting,
    Discovering,
    Seeding,
    History,
    CheckingPts,
    PollingDifferences,
    Receiving,
    Persisting,
    Verifying,
    Waiting,
    RateLimited,
    Idle,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct ChannelSyncEvent {
    pub phase: ChannelSyncPhase,
    pub chat_id: Option<i64>,
    pub at: Instant,
    pub failure: Option<ApplicationErrorKind>,
}

#[derive(Clone, Debug)]
pub struct ChannelSyncSnapshot {
    pub account_id: i64,
    pub phase: ChannelSyncPhase,
    pub chat_id: Option<i64>,
    pub phase_started: Instant,
    pub last_activity: Instant,
    /// Most recent successful synchronization event, retained beyond timeline eviction.
    pub last_completed_at: Option<Instant>,
    pub retry_at: Option<Instant>,
    pub failure: Option<ApplicationErrorKind>,
    pub queued: usize,
    pub failed_channels: usize,
    pub committed_pages: u64,
    pub data_revision: u64,
    pub events: VecDeque<ChannelSyncEvent>,
    pub active: Vec<ChannelSyncEvent>,
    pub dropped_events: u64,
    pub overflow_signals: u64,
    pub managed_chat_id: Option<i64>,
    pub managed_watch: Option<teleark_storage::ManagedChannelWatch>,
    pub managed_review_pending: bool,
    pub managed_scan: Option<ManagedScanStatus>,
}

impl ChannelSyncSnapshot {
    fn new(account_id: i64, queued: usize) -> Self {
        let now = Instant::now();
        Self {
            account_id,
            phase: ChannelSyncPhase::Queued,
            chat_id: None,
            phase_started: now,
            last_activity: now,
            last_completed_at: None,
            retry_at: None,
            failure: None,
            queued,
            failed_channels: 0,
            committed_pages: 0,
            data_revision: 0,
            active: Vec::new(),
            events: VecDeque::from([ChannelSyncEvent {
                phase: ChannelSyncPhase::Queued,
                chat_id: None,
                at: now,
                failure: None,
            }]),
            dropped_events: 0,
            overflow_signals: 0,
            managed_chat_id: None,
            managed_watch: None,
            managed_review_pending: false,
            managed_scan: None,
        }
    }

    fn transition(
        &mut self,
        phase: ChannelSyncPhase,
        chat_id: Option<i64>,
        failure: Option<ApplicationErrorKind>,
        retry_at: Option<Instant>,
    ) -> bool {
        if (self.phase, self.chat_id, self.failure, self.retry_at)
            == (phase, chat_id, failure, retry_at)
        {
            return false;
        }
        let now = Instant::now();
        self.phase = phase;
        self.chat_id = chat_id;
        self.failure = failure;
        self.retry_at = retry_at;
        self.phase_started = now;
        self.last_activity = now;
        self.record_event(ChannelSyncEvent {
            phase,
            chat_id,
            at: now,
            failure,
        });
        true
    }

    /// Returns the target of the newest unresolved failure event. Its event is
    /// retained in the bounded timeline until a later success or terminal
    /// cancellation resolves that target. A returned event with no chat ID
    /// targets the directory.
    pub fn retry_target(&self) -> Option<ChannelSyncEvent> {
        unresolved_retry_event_index(&self.events).map(|index| self.events[index].clone())
    }

    /// Whether the directory has a queued automatic retry awaiting its deadline.
    pub fn directory_retry_waiting(&self) -> bool {
        directory_retry_event_index(&self.events).is_some()
    }

    fn record_event(&mut self, event: ChannelSyncEvent) {
        if self.events.len() >= EVENT_CAPACITY {
            let retry_target = unresolved_retry_event_index(&self.events);
            let directory_retry = directory_retry_event_index(&self.events);
            let remove = (0..self.events.len())
                .find(|index| Some(*index) != retry_target && Some(*index) != directory_retry)
                .unwrap_or(0);
            self.events.remove(remove);
            self.dropped_events = self.dropped_events.saturating_add(1);
        }
        self.events.push_back(event);
    }
}

fn unresolved_retry_event_index(events: &VecDeque<ChannelSyncEvent>) -> Option<usize> {
    let mut resolved = BTreeSet::new();
    for (index, event) in events.iter().enumerate().rev() {
        if resolved.contains(&event.chat_id) {
            continue;
        }
        if event.phase == ChannelSyncPhase::Cancelled {
            resolved.insert(event.chat_id);
            continue;
        }
        if event.failure.is_some() {
            return Some(index);
        }
        if event.phase == ChannelSyncPhase::Idle {
            resolved.insert(event.chat_id);
        }
    }
    None
}

fn directory_retry_event_index(events: &VecDeque<ChannelSyncEvent>) -> Option<usize> {
    events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, event)| event.chat_id.is_none())
        .and_then(|(index, event)| {
            matches!(
                event.phase,
                ChannelSyncPhase::Waiting | ChannelSyncPhase::RateLimited
            )
            .then_some(index)
        })
}

#[derive(Clone)]
pub struct ChannelSync {
    inner: Arc<Owner>,
}

struct Shared {
    changes: tokio::sync::watch::Sender<()>,
    deltas: Mutex<feed::DeltaJournal>,
    sources: Mutex<(u64, Arc<Vec<TelegramChatSummary>>)>,
    history: Mutex<history::Requests>,
    managed_id: AtomicI64,
    observation: AtomicU64,
    manifest_generation: AtomicU64,
    snapshot: Mutex<ChannelSyncSnapshot>,
    pending_channel_retries: AtomicU64,
    stop: AtomicBool,
    active: Mutex<BTreeMap<i64, TelegramScanCancellation>>,
}

struct Owner {
    sender: mpsc::SyncSender<Command>,
    shared: Arc<Shared>,
    join: Mutex<Option<JoinHandle<()>>>,
    worker: thread::Thread,
}

#[derive(Clone)]
enum Command {
    Sources(Vec<TelegramChatSummary>),
    Prioritize(i64),
    Watch(i64),
    Acknowledge(i64, u64),
    History(i64),
    Refresh(i64),
    RefreshDirectory,
    CancelDirectory,
    CancelPendingRetries,
    Cancel(i64),
}

impl ChannelSync {
    pub fn start(
        telegram: DesktopTelegram,
        library: DesktopLibrary,
        account: TelegramAccount,
        chats: Vec<TelegramChatSummary>,
    ) -> Result<Self, ApplicationError> {
        Self::start_with(telegram, library, account, chats)
    }

    fn start_with<T: AccountSource>(
        telegram: T,
        library: DesktopLibrary,
        account: TelegramAccount,
        chats: Vec<TelegramChatSummary>,
    ) -> Result<Self, ApplicationError> {
        Self::start_with_clock(telegram, library, account, chats, Arc::new(Instant::now))
    }

    fn start_with_clock<T: AccountSource>(
        telegram: T,
        library: DesktopLibrary,
        account: TelegramAccount,
        chats: Vec<TelegramChatSummary>,
        clock: SyncClock,
    ) -> Result<Self, ApplicationError> {
        Self::start_with_clock_and_entropy(
            telegram,
            library,
            account,
            chats,
            clock,
            Arc::new(recovery_entropy),
        )
    }

    fn start_with_clock_and_entropy<T: AccountSource>(
        telegram: T,
        library: DesktopLibrary,
        account: TelegramAccount,
        chats: Vec<TelegramChatSummary>,
        clock: SyncClock,
        entropy: SyncEntropy,
    ) -> Result<Self, ApplicationError> {
        if chats.len() > MAX_SOURCES {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        let scheduler = Scheduler::new(chats);
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(ChannelSyncSnapshot::new(account.id, scheduler.queue.len())),
            pending_channel_retries: AtomicU64::new(0),
            changes: tokio::sync::watch::channel(()).0,
            deltas: Mutex::new(feed::DeltaJournal::default()),
            sources: Mutex::new((0, Arc::new(Vec::new()))),
            history: Mutex::new(history::Requests::default()),
            managed_id: AtomicI64::new(0),
            observation: AtomicU64::new(0),
            manifest_generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            active: Mutex::new(BTreeMap::new()),
        });
        let (sender, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let worker_shared = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("teleark-channel-sync".into())
            .spawn(move || {
                run(
                    telegram,
                    library,
                    account,
                    scheduler,
                    receiver,
                    worker_shared,
                    RetryControls { clock, entropy },
                );
            })
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        Ok(Self {
            inner: Arc::new(Owner {
                sender,
                shared,
                worker: join.thread().clone(),
                join: Mutex::new(Some(join)),
            }),
        })
    }

    pub fn update_sources(&self, chats: Vec<TelegramChatSummary>) -> Result<(), ApplicationError> {
        if chats.len() > MAX_SOURCES {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        self.command(Command::Sources(chats))
    }

    /// Coherent latest committed metadata; coalesced notifications cannot lose a rename or departure.
    pub fn sources_since(&self, revision: u64) -> Option<(u64, Arc<Vec<TelegramChatSummary>>)> {
        self.inner.shared.sources.lock().ok().and_then(|sources| {
            (sources.0 != revision).then(|| (sources.0, Arc::clone(&sources.1)))
        })
    }

    pub fn snapshot(&self) -> Result<ChannelSyncSnapshot, ApplicationError> {
        self.inner
            .shared
            .snapshot
            .lock()
            .map(|s| s.clone())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Conflict))
    }

    /// Number of queued channel retries, including entries older than the retained event log.
    pub fn pending_channel_retries(&self) -> usize {
        usize::try_from(
            self.inner
                .shared
                .pending_channel_retries
                .load(Ordering::Acquire),
        )
        .unwrap_or(usize::MAX)
    }

    pub fn subscribe(&self) -> ChannelSyncSubscription {
        ChannelSyncSubscription {
            receiver: self.inner.shared.changes.subscribe(),
        }
    }
    pub fn changes_since(
        &self,
        chat: i64,
        revision: i64,
    ) -> Result<ChannelChanges, ApplicationError> {
        self.inner
            .shared
            .deltas
            .lock()
            .map(|journal| journal.changes(chat, revision))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Conflict))
    }
    /// Observation only. Upload authorization still requires remote identity validation.
    pub fn watch_managed_channel(&self, chat: i64) -> Result<(), ApplicationError> {
        self.command(Command::Watch(chat))?;
        // Route mutation warnings immediately, even while the account owner is
        // awaiting another channel. Persistence/registration remain queued work.
        self.inner.shared.managed_id.store(chat, Ordering::Release);
        if let Ok(mut snapshot) = self.inner.shared.snapshot.lock() {
            if snapshot.managed_chat_id != Some(chat) {
                snapshot.managed_watch = None;
                snapshot.managed_review_pending = false;
            }
            snapshot.managed_chat_id = Some(chat);
        }
        self.inner.shared.changes.send_replace(());
        Ok(())
    }
    pub fn acknowledge_managed_changes(
        &self,
        chat: i64,
        through: u64,
    ) -> Result<(), ApplicationError> {
        self.command(Command::Acknowledge(chat, through))
    }
    /// Only promotes work already known to be necessary; a clean channel
    /// remains clean even after repeated selection.
    pub fn prioritize(&self, chat: i64) -> Result<(), ApplicationError> {
        self.command(Command::Prioritize(chat))
    }
    pub fn refresh(&self, chat: i64) -> Result<(), ApplicationError> {
        self.command(Command::Refresh(chat))
    }
    /// Requests a new Telegram directory snapshot after a directory-level failure.
    pub fn refresh_directory(&self) -> Result<(), ApplicationError> {
        self.command(Command::RefreshDirectory)
    }
    pub fn cancel(&self, chat: i64) -> Result<(), ApplicationError> {
        if let Ok(active) = self.inner.shared.active.lock()
            && let Some(token) = active.get(&chat)
        {
            token.cancel();
        }
        self.command(Command::Cancel(chat))
    }
    /// Cancels active directory work or its pending automatic retry without a chat identifier.
    pub fn cancel_directory(&self) -> Result<(), ApplicationError> {
        if let Ok(active) = self.inner.shared.active.lock()
            && let Some(token) = active.get(&0)
        {
            token.cancel();
        }
        self.command(Command::CancelDirectory)
    }
    /// Cancels every queued channel retry while leaving active work and the directory unchanged.
    pub fn cancel_pending_retries(&self) -> Result<(), ApplicationError> {
        self.command(Command::CancelPendingRetries)
    }
    pub fn stop(&self) {
        self.inner.shared.stop.store(true, Ordering::Release);
        cancel_active(&self.inner.shared);
        self.inner.worker.unpark();
    }
    pub fn is_stopped(&self) -> bool {
        self.inner.shared.stop.load(Ordering::Acquire)
    }
    fn command(&self, command: Command) -> Result<(), ApplicationError> {
        let acknowledgment = match &command {
            Command::History(chat) | Command::Refresh(chat) => {
                Some((ChannelSyncPhase::Queued, Some(*chat)))
            }
            Command::RefreshDirectory => Some((ChannelSyncPhase::Queued, None)),
            Command::Cancel(chat) => Some((ChannelSyncPhase::Cancelled, Some(*chat))),
            Command::CancelDirectory => Some((ChannelSyncPhase::Cancelled, None)),
            Command::Watch(chat) | Command::Acknowledge(chat, _) => {
                Some((ChannelSyncPhase::Queued, Some(*chat)))
            }
            _ => None,
        };
        self.inner
            .sender
            .try_send(command)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        if let Some((phase, chat)) = acknowledgment
            && let Ok(mut snapshot) = self.inner.shared.snapshot.lock()
        {
            if matches!(
                snapshot.phase,
                ChannelSyncPhase::Idle | ChannelSyncPhase::Failed | ChannelSyncPhase::Cancelled
            ) {
                snapshot.transition(phase, chat, None, None);
            } else {
                snapshot.record_event(ChannelSyncEvent {
                    phase,
                    chat_id: chat,
                    at: Instant::now(),
                    failure: None,
                });
            }
        }
        self.inner.shared.changes.send_replace(());
        self.inner.worker.unpark();
        Ok(())
    }
}

fn cancel_active(shared: &Shared) {
    if let Ok(active) = shared.active.lock() {
        for token in active.values() {
            token.cancel();
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        cancel_active(&self.shared);
        self.worker.unpark();
        // Never join a network/storage owner on the UI thread. The reaper
        // retains its handle until the cancelled worker has released adapters.
        if let Ok(mut join) = self.join.lock()
            && let Some(join) = join.take()
        {
            let _ = thread::Builder::new()
                .name("teleark-sync-reaper".into())
                .spawn(move || {
                    let _ = join.join();
                });
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ChannelRead {
    Difference(i32),
    Push,
    ManagedManifests,
    History(Option<i64>),
    GapHistory(Option<i64>),
    Verify(Vec<i64>),
}

pub(crate) struct ChannelReadPage {
    pub files: Vec<TelegramFileSummary>,
    pub removed: Vec<i64>,
    pub pts: Option<i32>,
    pub complete: bool,
    pub history_gap: bool,
    pub before: Option<i64>,
    pub edited: Vec<i64>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ChannelSyncFailure {
    pub kind: ApplicationErrorKind,
    pub retry_after: Option<Duration>,
}
impl From<ApplicationError> for ChannelSyncFailure {
    fn from(error: ApplicationError) -> Self {
        Self {
            kind: error.kind(),
            retry_after: None,
        }
    }
}

#[derive(Clone)]
struct Job {
    quiet_deadline: Option<Instant>,
    pushes: VecDeque<teleark_telegram::ChannelPush>,
    catalog_ready: Option<bool>,
    watch_loaded: bool,
    head: Option<i32>,
    state: Option<ChannelSyncState>,
    force: bool,
    history: bool,
    history_request: Option<u64>,
    paused: bool,
    failure: Option<ApplicationErrorKind>,
    retries: u32,
    retry_at: Option<Instant>,
    rate_limited: bool,
}

// Retry only typed transport/server/write-conflict failures. Authentication,
// permission, capacity, and persistence failures stay visible for an event or
// explicit retry. The owner supplies entropy; tests advance Instant directly.
fn recovery_delay(kind: ApplicationErrorKind, attempt: u32, entropy: u64) -> Option<Duration> {
    if !matches!(
        kind,
        ApplicationErrorKind::Network
            | ApplicationErrorKind::Server
            | ApplicationErrorKind::Conflict
    ) {
        return None;
    }
    let ceiling_ms = (1_u64 << attempt.min(6)).min(60) * 1_000;
    // Equal jitter avoids synchronized clients while retaining a nonzero floor.
    Some(Duration::from_millis(
        ceiling_ms / 2 + entropy % (ceiling_ms / 2 + 1),
    ))
}

fn recovery_entropy(chat: i64, attempt: u32) -> u64 {
    use std::hash::BuildHasher;
    std::collections::hash_map::RandomState::new().hash_one((chat, attempt))
}

fn retry_deadline(
    kind: ApplicationErrorKind,
    retry_after: Option<Duration>,
    attempt: u32,
    now: Instant,
    entropy: u64,
) -> Option<Instant> {
    retry_after
        .or_else(|| recovery_delay(kind, attempt, entropy))
        .and_then(|delay| now.checked_add(delay))
}

fn merged_flood_deadline(
    current: Option<Instant>,
    incoming: Option<Instant>,
    now: Instant,
) -> Option<Instant> {
    match (
        current.filter(|deadline| *deadline > now),
        incoming.filter(|deadline| *deadline > now),
    ) {
        (Some(current), Some(incoming)) => Some(current.max(incoming)),
        (current, incoming) => incoming.or(current),
    }
}

#[derive(Clone, Copy)]
struct DirectoryRetry {
    attempt: u32,
    retry_at: Option<Instant>,
}

impl DirectoryRetry {
    fn starting(now: Instant) -> Self {
        Self {
            attempt: 0,
            retry_at: Some(now),
        }
    }

    #[cfg(test)]
    fn failed(
        &mut self,
        error: &ChannelSyncFailure,
        now: Instant,
        account: i64,
    ) -> Option<Instant> {
        self.failed_with_entropy(error, now, recovery_entropy(account, self.attempt))
    }

    fn failed_with_entropy(
        &mut self,
        error: &ChannelSyncFailure,
        now: Instant,
        entropy: u64,
    ) -> Option<Instant> {
        let retry_at = retry_deadline(error.kind, error.retry_after, self.attempt, now, entropy);
        if error.retry_after.is_none() && retry_at.is_some() {
            self.attempt = self.attempt.saturating_add(1);
        }
        self.retry_at = retry_at;
        retry_at
    }

    fn request(&mut self, now: Instant, flood_until: Option<Instant>) {
        // A manual refresh bypasses local backoff, but never Telegram's server gate.
        self.retry_at = Some(flood_until.filter(|at| *at > now).unwrap_or(now));
    }

    fn succeeded(&mut self) {
        self.attempt = 0;
        self.retry_at = None;
    }

    fn new_connection(&mut self, now: Instant) {
        self.attempt = 0;
        self.retry_at = Some(now);
    }

    fn cancelled(&mut self) {
        self.attempt = 0;
        self.retry_at = None;
    }
}

fn refreshes_directory(command: &Command, source_failure: Option<ApplicationErrorKind>) -> bool {
    matches!(command, Command::RefreshDirectory | Command::Refresh(0))
        || matches!(command, Command::Refresh(_))
            && source_failure.is_some_and(|failure| failure != ApplicationErrorKind::Cancelled)
}

fn request_directory_refresh(
    retry: &mut DirectoryRetry,
    pending: &mut bool,
    last_check: &mut Instant,
    now: Instant,
    flood_until: Option<Instant>,
) {
    *pending = true;
    retry.request(now, flood_until);
    *last_check = now.checked_sub(Duration::from_secs(30)).unwrap_or(now);
}

impl Job {
    fn fail(&mut self, error: &ChannelSyncFailure, now: Instant, entropy: u64) -> ChannelSyncPhase {
        self.failure = Some(error.kind);
        if error.kind == ApplicationErrorKind::Conflict {
            // Reload the committed winner before retrying a failed CAS.
            self.state = None;
        }
        self.retry_at = retry_deadline(error.kind, error.retry_after, self.retries, now, entropy);
        self.rate_limited = error.retry_after.is_some();
        self.paused = self.retry_at.is_none();
        if error.retry_after.is_some() {
            ChannelSyncPhase::RateLimited
        } else if self.retry_at.is_some() {
            self.retries = self.retries.saturating_add(1);
            ChannelSyncPhase::Waiting
        } else if error.kind == ApplicationErrorKind::Cancelled {
            ChannelSyncPhase::Cancelled
        } else {
            ChannelSyncPhase::Failed
        }
    }
}

struct Scheduler {
    jobs: BTreeMap<i64, Job>,
    queue: VecDeque<i64>,
    pending_retry_count: usize,
    preferred: Option<i64>,
    preferred_turns: u8,
    source_failure: Option<ApplicationErrorKind>,
    overflow_count: u64,
    in_flight: std::collections::BTreeSet<i64>,
}

impl Scheduler {
    fn recover_quiet_channels(&mut self, now: Instant) {
        let due: Vec<_> = self
            .jobs
            .iter()
            .filter_map(|(id, job)| {
                (!job.paused
                    && !self.in_flight.contains(id)
                    && job.quiet_deadline.is_some_and(|at| at <= now))
                .then_some(*id)
            })
            .collect();
        for id in due {
            if let Some(job) = self.jobs.get_mut(&id) {
                job.force = true;
                job.quiet_deadline = Some(now + UPDATE_SILENCE_RECOVERY);
            }
            self.enqueue(id);
        }
    }

    fn quiet_deadline(&self) -> Option<Instant> {
        self.jobs
            .iter()
            .filter(|(id, job)| !job.paused && !self.in_flight.contains(id))
            .filter_map(|(_, job)| job.quiet_deadline)
            .min()
    }

    fn new(chats: Vec<TelegramChatSummary>) -> Self {
        let jobs: BTreeMap<_, _> = chats
            .into_iter()
            .filter(|c| c.kind == TelegramChatKind::Channel)
            .map(|c| {
                (
                    c.id,
                    Job {
                        quiet_deadline: None,
                        pushes: VecDeque::new(),
                        catalog_ready: None,
                        watch_loaded: false,
                        head: c.sync_pts,
                        state: None,
                        force: false,
                        history: false,
                        history_request: None,
                        paused: false,
                        failure: None,
                        retries: 0,
                        retry_at: None,
                        rate_limited: false,
                    },
                )
            })
            .collect();
        Self {
            queue: jobs.keys().copied().collect(),
            jobs,
            pending_retry_count: 0,
            preferred: None,
            preferred_turns: 0,
            source_failure: None,
            overflow_count: 0,
            in_flight: Default::default(),
        }
    }
    fn update_sources(&mut self, chats: Vec<TelegramChatSummary>) -> Vec<i64> {
        if chats.len() > MAX_SOURCES {
            self.source_failure = Some(ApplicationErrorKind::Capacity);
            return Vec::new();
        }
        let source_ids: BTreeSet<_> = chats
            .iter()
            .filter(|chat| chat.kind == TelegramChatKind::Channel)
            .map(|chat| chat.id)
            .collect();
        let removed: Vec<_> = self
            .jobs
            .iter()
            .filter(|(id, job)| job.catalog_ready.is_none() && !source_ids.contains(id))
            .map(|(id, _)| *id)
            .collect();
        let removed_waiters = removed
            .iter()
            .filter(|id| self.is_waiting_retry(**id))
            .count();
        // Discovery is a complete snapshot. Retire departed sources so stale
        // jobs cannot consume the bounded capacity or issue inaccessible RPCs.
        self.jobs
            .retain(|id, job| job.catalog_ready.is_some() || source_ids.contains(id));
        self.queue.retain(|id| self.jobs.contains_key(id));
        self.pending_retry_count = self.pending_retry_count.saturating_sub(removed_waiters);
        self.source_failure = None;
        for chat in chats
            .into_iter()
            .filter(|chat| chat.kind == TelegramChatKind::Channel)
        {
            if !self.jobs.contains_key(&chat.id) && self.jobs.len() < MAX_SOURCES {
                self.jobs.insert(
                    chat.id,
                    Job {
                        quiet_deadline: None,
                        pushes: VecDeque::new(),
                        catalog_ready: None,
                        watch_loaded: false,
                        head: chat.sync_pts,
                        state: None,
                        force: false,
                        history: false,
                        history_request: None,
                        paused: false,
                        failure: None,
                        retries: 0,
                        retry_at: None,
                        rate_limited: false,
                    },
                );
                self.enqueue(chat.id);
            } else if let Some(pts) = chat.sync_pts {
                self.hint(chat.id, pts);
            }
        }
        removed
    }
    fn enqueue(&mut self, id: i64) {
        let was_waiting = self.is_waiting_retry(id);
        self.queue_job(id);
        self.adjust_pending_retry_count(id, was_waiting);
    }

    fn enqueue_after_completion(&mut self, id: i64, was_waiting: bool) {
        self.queue_job(id);
        self.adjust_pending_retry_count(id, was_waiting);
    }

    fn queue_job(&mut self, id: i64) {
        if self.jobs.contains_key(&id) && !self.queue.contains(&id) {
            self.queue.push_back(id);
        }
    }
    fn is_waiting_retry(&self, id: i64) -> bool {
        self.queue.contains(&id)
            && !self.in_flight.contains(&id)
            && self
                .jobs
                .get(&id)
                .is_some_and(|job| !job.paused && job.failure.is_some() && job.retry_at.is_some())
    }
    fn adjust_pending_retry_count(&mut self, id: i64, was_waiting: bool) {
        let is_waiting = self.is_waiting_retry(id);
        match (was_waiting, is_waiting) {
            (false, true) => self.pending_retry_count = self.pending_retry_count.saturating_add(1),
            (true, false) => self.pending_retry_count = self.pending_retry_count.saturating_sub(1),
            _ => {}
        }
    }
    fn hint(&mut self, id: i64, pts: i32) {
        let Some(job) = self.jobs.get_mut(&id) else {
            return;
        };
        if pts == 0 {
            job.force = true;
        } else {
            job.head = Some(job.head.unwrap_or(0).max(pts));
        }
        if !job.paused && needed(job) {
            self.enqueue(id);
        }
    }
    fn command(&mut self, command: Command) {
        let id = match command {
            Command::Watch(_)
            | Command::Acknowledge(_, _)
            | Command::RefreshDirectory
            | Command::CancelDirectory
            | Command::CancelPendingRetries => return,
            // Source snapshots need the owner to cancel active tokens and record
            // retirement events, so they are applied by `handle_command`.
            Command::Sources(_) => return,
            Command::Prioritize(id)
            | Command::History(id)
            | Command::Refresh(id)
            | Command::Cancel(id) => id,
        };
        if let Command::Prioritize(_) = command {
            self.preferred = Some(id);
            return;
        }
        let was_waiting = self.is_waiting_retry(id);
        let Some(job) = self.jobs.get_mut(&id) else {
            return;
        };
        match command {
            Command::Cancel(_) => {
                job.paused = true;
                job.failure = Some(ApplicationErrorKind::Cancelled);
                job.retries = 0;
                job.retry_at = None;
                job.rate_limited = false;
                self.queue.retain(|queued| *queued != id);
            }
            Command::History(_) | Command::Refresh(_) => {
                if !job.rate_limited {
                    // A deliberate retry bypasses local backoff, while a Telegram
                    // FloodWait remains authoritative until its deadline.
                    job.retry_at = None;
                    job.failure = None;
                }
                job.paused = false;
                job.retries = 0;
                if matches!(command, Command::History(_)) {
                    job.history = true;
                } else {
                    job.force = true;
                }
                self.enqueue(id);
            }
            Command::Prioritize(_)
            | Command::Sources(_)
            | Command::Watch(_)
            | Command::Acknowledge(_, _)
            | Command::RefreshDirectory
            | Command::CancelDirectory
            | Command::CancelPendingRetries => {}
        }
        self.adjust_pending_retry_count(id, was_waiting);
    }

    fn cancel_pending_retries(&mut self) -> Vec<i64> {
        let cancelled: Vec<_> = self
            .queue
            .iter()
            .copied()
            .filter(|id| self.is_waiting_retry(*id))
            .collect();
        if cancelled.is_empty() {
            return cancelled;
        }
        let cancelled_set: BTreeSet<_> = cancelled.iter().copied().collect();
        self.queue.retain(|id| !cancelled_set.contains(id));
        self.pending_retry_count = self.pending_retry_count.saturating_sub(cancelled.len());
        for id in &cancelled {
            if let Some(job) = self.jobs.get_mut(id) {
                job.paused = true;
                job.failure = Some(ApplicationErrorKind::Cancelled);
                job.retries = 0;
                job.retry_at = None;
                job.rate_limited = false;
            }
        }
        cancelled
    }

    fn transport_available(&mut self, now: Instant, flood_until: Option<Instant>) {
        if flood_until.is_some_and(|at| at > now) {
            return;
        }
        // Actual pushed data proves delivery resumed. A close/gap hint alone
        // does not. Recover network waits early, preserving non-network errors.
        let resumed: Vec<_> = self
            .queue
            .iter()
            .copied()
            .filter(|id| {
                self.jobs.get(id).is_some_and(|job| {
                    !job.paused && job.failure == Some(ApplicationErrorKind::Network)
                })
            })
            .collect();
        for id in resumed {
            let was_waiting = self.is_waiting_retry(id);
            if let Some(job) = self.jobs.get_mut(&id)
                && !job.paused
                && job.failure == Some(ApplicationErrorKind::Network)
            {
                job.retry_at = None;
                job.rate_limited = false;
            }
            self.adjust_pending_retry_count(id, was_waiting);
        }
    }

    fn receive_push(&mut self, push: teleark_telegram::ChannelPush) {
        let id = push.channel_id;
        let count: usize = self.jobs.values().map(|job| job.pushes.len()).sum();
        let bytes: usize = self
            .jobs
            .values()
            .flat_map(|job| &job.pushes)
            .map(|p| p.estimated_bytes())
            .sum();
        if let Some(job) = self.jobs.get_mut(&id) {
            if job
                .state
                .as_ref()
                .is_some_and(|state| push.pts <= state.pts)
            {
                return;
            }
            if count >= 512 || bytes.saturating_add(push.estimated_bytes()) > 4 * 1024 * 1024 {
                job.pushes.clear();
                job.force = true;
                self.overflow_count = self.overflow_count.saturating_add(1);
            } else if !job.pushes.iter().any(|pending| pending.pts == push.pts) {
                job.pushes.push_back(push);
            }
        }
    }
    #[cfg(test)]
    fn pop(&mut self, now: Instant) -> Option<i64> {
        self.pop_available(now, |_| true)
    }

    fn pop_available(&mut self, now: Instant, available: impl Fn(i64) -> bool) -> Option<i64> {
        let ready = |id: &i64| {
            available(*id)
                && !self.in_flight.contains(id)
                && self
                    .jobs
                    .get(id)
                    .is_some_and(|job| !job.paused && job.retry_at.is_none_or(|at| at <= now))
        };
        let priority = |id: &i64| {
            Some(*id) == self.preferred
                || self
                    .jobs
                    .get(id)
                    .is_some_and(|job| job.catalog_ready.is_some())
        };
        let preferred = (self.preferred_turns < 3)
            .then(|| self.queue.iter().position(|id| priority(id) && ready(id)))
            .flatten();
        let position = preferred
            .or_else(|| self.queue.iter().position(|id| !priority(id) && ready(id)))
            .or_else(|| self.queue.iter().position(ready))?;
        let id = *self.queue.get(position)?;
        let was_priority = priority(&id);
        let was_waiting = self.is_waiting_retry(id);
        self.queue.remove(position)?;
        self.adjust_pending_retry_count(id, was_waiting);
        if was_priority {
            self.preferred_turns = self.preferred_turns.saturating_add(1);
        } else {
            self.preferred_turns = 0;
        }
        Some(id)
    }
}

pub(crate) fn update_summary(file: teleark_telegram::ChannelFileUpdate) -> TelegramFileSummary {
    TelegramFileSummary {
        message_id: file.message_id,
        sent_at_unix_ms: file.sent_at_unix_ms,
        modified_at_unix_ms: file.modified_at_unix_ms,
        file_name: file.file_name,
        caption: file.caption,
        mime_type: file.mime_type,
        size_bytes: file.size_bytes,
    }
}

fn handle_command(
    command: Command,
    library: &DesktopLibrary,
    account: i64,
    scheduler: &mut Scheduler,
    shared: &Shared,
) {
    let result = match command {
        Command::Sources(chats) => {
            publish_sources(shared, &chats);
            apply_source_snapshot(shared, scheduler, chats);
            Ok(())
        }
        Command::Watch(chat) => {
            let was_waiting = scheduler.is_waiting_retry(chat);
            for (id, job) in &mut scheduler.jobs {
                if *id != chat && job.catalog_ready.is_some() {
                    job.catalog_ready = None;
                }
            }
            let job = scheduler.jobs.entry(chat).or_insert_with(|| Job {
                quiet_deadline: None,
                head: None,
                state: None,
                force: true,
                history: false,
                history_request: None,
                paused: false,
                failure: None,
                retries: 0,
                retry_at: None,
                rate_limited: false,
                pushes: VecDeque::new(),
                catalog_ready: Some(false),
                watch_loaded: false,
            });
            // Registration belongs to the retryable job, so a temporary disk
            // failure cannot silently disable protection or advance its PTS.
            job.catalog_ready = Some(false);
            job.watch_loaded = false;
            job.paused = false;
            if job.rate_limited && job.retry_at.is_some() {
                // Watch is an explicit re-registration, so it may bypass a local
                // backoff; Telegram's server deadline remains authoritative.
            } else {
                job.failure = None;
                job.retry_at = None;
                job.rate_limited = false;
                job.retries = 0;
            }
            shared.managed_id.store(chat, Ordering::Release);
            if let Ok(mut snapshot) = shared.snapshot.lock() {
                if snapshot.managed_chat_id != Some(chat) {
                    snapshot.managed_review_pending = false;
                    snapshot.managed_watch = None;
                }
                snapshot.managed_chat_id = Some(chat);
            }
            scheduler.enqueue(chat);
            scheduler.adjust_pending_retry_count(chat, was_waiting);
            Ok(())
        }
        Command::Acknowledge(chat, through) => {
            publish(shared, ChannelSyncPhase::Persisting, Some(chat), None, None);
            library
                .acknowledge_managed_changes(account, chat, through)
                .map(|watch| {
                    if shared.managed_id.load(Ordering::Acquire) == chat
                        && let Ok(mut snapshot) = shared.snapshot.lock()
                    {
                        snapshot.managed_watch = Some(watch);
                    }
                    publish_completed(shared, Some(chat));
                })
        }
        command => {
            scheduler.command(command);
            Ok(())
        }
    };
    if let Err(error) = result {
        publish(
            shared,
            ChannelSyncPhase::Failed,
            None,
            Some(error.kind()),
            None,
        );
    }
    shared.changes.send_replace(());
}

fn apply_source_snapshot(
    shared: &Shared,
    scheduler: &mut Scheduler,
    chats: Vec<TelegramChatSummary>,
) {
    let removed = scheduler.update_sources(chats);
    if removed.is_empty() {
        return;
    }

    if let Ok(active) = shared.active.lock() {
        for (chat, token) in active.iter() {
            if removed.binary_search(chat).is_ok() {
                token.cancel();
            }
        }
    }

    if let Ok(mut snapshot) = shared.snapshot.lock() {
        let current_was_removed = snapshot
            .chat_id
            .is_some_and(|chat| removed.binary_search(&chat).is_ok());
        snapshot.active.retain(|activity| {
            activity
                .chat_id
                .is_none_or(|chat| removed.binary_search(&chat).is_err())
        });
        for chat in &removed {
            snapshot.record_event(ChannelSyncEvent {
                phase: ChannelSyncPhase::Cancelled,
                chat_id: Some(*chat),
                at: Instant::now(),
                failure: Some(ApplicationErrorKind::SourceMissing),
            });
        }
        if current_was_removed {
            if let Some(activity) = snapshot.active.first().cloned() {
                snapshot.phase = activity.phase;
                snapshot.chat_id = activity.chat_id;
                snapshot.phase_started = activity.at;
            } else {
                snapshot.phase = ChannelSyncPhase::Cancelled;
                snapshot.chat_id = None;
                snapshot.phase_started = Instant::now();
            }
            snapshot.failure = None;
            snapshot.retry_at = None;
            snapshot.last_activity = Instant::now();
        }
    }
    shared.changes.send_replace(());
}

fn needed(job: &Job) -> bool {
    job.state.as_ref().is_none_or(|state| {
        state.revision == 0
            || state.repair_pending
            || state.gap_pending
            || job.catalog_ready == Some(false)
            || job.force
            || job.head.is_some_and(|head| head > state.pts)
            || (job.history && !state.history_exhausted)
    })
}

fn publish_completed(shared: &Shared, chat: Option<i64>) {
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        snapshot.last_completed_at = Some(Instant::now());
    }
    publish(shared, ChannelSyncPhase::Idle, chat, None, None);
}

fn publish(
    shared: &Shared,
    phase: ChannelSyncPhase,
    chat: Option<i64>,
    failure: Option<ApplicationErrorKind>,
    retry_at: Option<Instant>,
) {
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        let running = matches!(
            phase,
            ChannelSyncPhase::ReadingLocal
                | ChannelSyncPhase::Discovering
                | ChannelSyncPhase::Seeding
                | ChannelSyncPhase::History
                | ChannelSyncPhase::CheckingPts
                | ChannelSyncPhase::PollingDifferences
                | ChannelSyncPhase::Receiving
                | ChannelSyncPhase::Persisting
                | ChannelSyncPhase::Verifying
        );
        snapshot
            .active
            .retain(|activity| activity.chat_id != chat || activity.phase == phase && running);
        if running
            && !snapshot
                .active
                .iter()
                .any(|activity| activity.chat_id == chat)
            && snapshot.active.len() < 6
        {
            snapshot.active.push(ChannelSyncEvent {
                phase,
                chat_id: chat,
                at: Instant::now(),
                failure,
            });
        }
        let changed = snapshot.transition(phase, chat, failure, retry_at);
        if let Some(activity) = snapshot.active.first().cloned() {
            snapshot.phase = activity.phase;
            snapshot.chat_id = activity.chat_id;
            snapshot.phase_started = activity.at;
        }
        if changed {
            shared.changes.send_replace(());
        }
    }
}

fn publish_cancelled_retries(shared: &Shared, cancelled: &[i64]) {
    if cancelled.is_empty() {
        return;
    }
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        for chat_id in cancelled {
            snapshot.record_event(ChannelSyncEvent {
                phase: ChannelSyncPhase::Cancelled,
                chat_id: Some(*chat_id),
                at: Instant::now(),
                failure: Some(ApplicationErrorKind::Cancelled),
            });
        }
        if snapshot.active.is_empty() {
            snapshot.phase = ChannelSyncPhase::Cancelled;
            snapshot.chat_id = cancelled.last().copied();
            snapshot.failure = Some(ApplicationErrorKind::Cancelled);
            snapshot.retry_at = None;
            snapshot.phase_started = Instant::now();
            snapshot.last_activity = snapshot.phase_started;
        }
    }
    shared.changes.send_replace(());
}

fn publish_pending_retry_count(shared: &Shared, count: usize) {
    let count = u64::try_from(count).unwrap_or(u64::MAX);
    if shared.pending_channel_retries.swap(count, Ordering::AcqRel) != count {
        shared.changes.send_replace(());
    }
}

fn transport_waker(shared: &Arc<Shared>, worker: thread::Thread) -> teleark_telegram::ChannelWake {
    let weak = Arc::downgrade(shared);
    Arc::new(move |chat, mutation| {
        if let Some(shared) = weak.upgrade() {
            if shared.stop.load(Ordering::Acquire) {
                return;
            }
            if mutation && chat == Some(shared.managed_id.load(Ordering::Acquire)) {
                shared.observation.fetch_add(1, Ordering::AcqRel);
                if let Ok(mut snapshot) = shared.snapshot.lock() {
                    snapshot.managed_review_pending = true;
                }
                shared.changes.send_replace(());
            }
            worker.unpark();
        }
    })
}

fn same_source_metadata(left: &[TelegramChatSummary], right: &[TelegramChatSummary]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.id == right.id
                && left.name == right.name
                && left.username == right.username
                && left.kind == right.kind
        })
}

fn source_reconciliation_deadline(
    last_check: Instant,
    last_delivery: Instant,
    pending: bool,
) -> Instant {
    if pending {
        last_check + Duration::from_secs(1)
    } else {
        last_check.max(last_delivery) + UPDATE_SILENCE_RECOVERY
    }
}

fn restored_quiet_deadline(saved: Option<i64>, wall_now: i64, now: Instant) -> Instant {
    // A future checkpoint indicates clock rollback; recheck immediately rather
    // than postponing recovery indefinitely. Missing/old checkpoints are due.
    let age = saved
        .filter(|at| *at <= wall_now)
        .map(|at| Duration::from_millis(wall_now.saturating_sub(at) as u64));
    now + age.map_or(Duration::ZERO, |age| {
        UPDATE_SILENCE_RECOVERY.saturating_sub(age)
    })
}

fn publish_sources(shared: &Shared, chats: &[TelegramChatSummary]) {
    let changed = if let Ok(mut sources) = shared.sources.lock() {
        if sources.0 != 0 && same_source_metadata(sources.1.as_slice(), chats) {
            false
        } else {
            sources.0 = sources.0.wrapping_add(1).max(1);
            sources.1 = Arc::new(chats.to_vec());
            true
        }
    } else {
        false
    };
    if changed {
        shared.changes.send_replace(());
    }
}

fn run<T: AccountSource>(
    telegram: T,
    library: DesktopLibrary,
    account: TelegramAccount,
    mut scheduler: Scheduler,
    receiver: mpsc::Receiver<Command>,
    shared: Arc<Shared>,
    retry: RetryControls,
) {
    let RetryControls { clock, entropy } = retry;
    publish(&shared, ChannelSyncPhase::ReadingLocal, None, None, None);
    match library.cached_channel_directory(account.id) {
        Ok(chats) if !chats.is_empty() => {
            publish_sources(&shared, &chats);
            apply_source_snapshot(&shared, &mut scheduler, chats);
        }
        Ok(_) => {}
        Err(error) => publish(
            &shared,
            ChannelSyncPhase::Failed,
            None,
            Some(error.kind()),
            None,
        ),
    }
    let lifecycle = telegram.lifecycle();
    let weak = Arc::downgrade(&shared);
    let owner_thread = thread::current();
    let _lifecycle_subscription = match lifecycle.subscribe(Arc::new(move || {
        if let Some(shared) = weak.upgrade() {
            cancel_active(&shared);
        }
        owner_thread.unpark();
    })) {
        Ok(subscription) => subscription,
        Err(error) => {
            publish(
                &shared,
                ChannelSyncPhase::Failed,
                None,
                Some(error.kind()),
                None,
            );
            shared.stop.store(true, Ordering::Release);
            return;
        }
    };
    let mut connection_revision = u64::MAX;
    let mut signals: Option<teleark_telegram::ChannelUpdateSignals> = None;
    let mut bound_waker = None;
    match library.storage_channel_id(account.id) {
        Ok(Some(chat)) => handle_command(
            Command::Watch(chat),
            &library,
            account.id,
            &mut scheduler,
            &shared,
        ),
        Ok(None) => {}
        Err(error) => publish(
            &shared,
            ChannelSyncPhase::Failed,
            None,
            Some(error.kind()),
            None,
        ),
    }
    let mut last_check = clock();
    let mut last_delivery = last_check;
    let mut source_probe = false;
    let mut source_probe_started: Option<Instant> = None;
    let mut directory_retry = DirectoryRetry::starting(clock());
    let mut flood_until = None;
    let mut source_refresh_pending = false;
    let mut executions = execution::Executions::new();
    let mut metadata_ready = false;
    while !shared.stop.load(Ordering::Acquire) {
        while let Some(completion) = executions.take() {
            let was_waiting_before_completion = scheduler.is_waiting_retry(completion.id);
            let removed_active = shared
                .active
                .lock()
                .is_ok_and(|mut active| active.remove(&completion.id).is_some());
            scheduler.in_flight.remove(&completion.id);
            if removed_active && completion.id != 0 && !scheduler.jobs.contains_key(&completion.id)
            {
                shared.changes.send_replace(());
            }
            if completion.id == 0 {
                source_probe_started = None;
            }
            let current_revision = lifecycle.snapshot().0;
            match completion.outcome {
                execution::Outcome::Failed(error) => {
                    if completion.revision != current_revision {
                        continue;
                    }
                    let mut retry_channel = None;
                    if completion.id == 0 {
                        scheduler.source_failure = Some(error);
                        directory_retry.failed_with_entropy(
                            &ChannelSyncFailure {
                                kind: error,
                                retry_after: None,
                            },
                            clock(),
                            entropy(account.id, directory_retry.attempt),
                        );
                        publish(
                            &shared,
                            if directory_retry.retry_at.is_some() {
                                ChannelSyncPhase::Waiting
                            } else {
                                ChannelSyncPhase::Failed
                            },
                            None,
                            Some(error),
                            directory_retry.retry_at,
                        );
                    } else if let Some(job) = scheduler.jobs.get_mut(&completion.id) {
                        let phase = job.fail(
                            &ChannelSyncFailure {
                                kind: error,
                                retry_after: None,
                            },
                            clock(),
                            entropy(completion.id, job.retries),
                        );
                        if job.paused
                            && let Ok(mut history) = shared.history.lock()
                        {
                            history.fail(completion.id, error);
                        }
                        retry_channel = (!job.paused && needed(job)).then_some(completion.id);
                        publish(
                            &shared,
                            phase,
                            Some(completion.id),
                            Some(error),
                            job.retry_at,
                        );
                    }
                    if let Some(id) = retry_channel {
                        scheduler.enqueue_after_completion(id, was_waiting_before_completion);
                    }
                }
                execution::Outcome::Sources(result) => {
                    if completion.revision != current_revision {
                        continue;
                    }
                    match result {
                        Ok(chats) => {
                            metadata_ready = true;
                            let changed = shared.sources.lock().is_ok_and(|sources| {
                                !same_source_metadata(sources.1.as_slice(), chats.as_slice())
                            });
                            if !source_probe || changed {
                                publish_completed(&shared, None);
                            }
                            publish_sources(&shared, &chats);
                            apply_source_snapshot(&shared, &mut scheduler, chats);
                            directory_retry.succeeded();
                        }
                        Err(error) => {
                            scheduler.source_failure = Some(error.kind);
                            let now = clock();
                            let phase = if error.kind == ApplicationErrorKind::Cancelled {
                                // An explicit directory cancellation is terminal until the user
                                // requests another refresh. It must not be presented as a failed
                                // sync merely because cancellation has no retry deadline.
                                directory_retry.retry_at = None;
                                ChannelSyncPhase::Cancelled
                            } else {
                                directory_retry.failed_with_entropy(
                                    &error,
                                    now,
                                    entropy(account.id, directory_retry.attempt),
                                );
                                if error.retry_after.is_some() {
                                    ChannelSyncPhase::RateLimited
                                } else if directory_retry.retry_at.is_some() {
                                    ChannelSyncPhase::Waiting
                                } else {
                                    ChannelSyncPhase::Failed
                                }
                            };
                            if error.retry_after.is_some()
                                && error.kind != ApplicationErrorKind::Cancelled
                            {
                                flood_until = merged_flood_deadline(
                                    flood_until,
                                    directory_retry.retry_at,
                                    now,
                                );
                            }
                            publish(
                                &shared,
                                phase,
                                None,
                                Some(error.kind),
                                directory_retry.retry_at,
                            );
                        }
                    }
                    last_check = clock();
                }
                execution::Outcome::Channel(mut completed, result) => {
                    let id = completion.id;
                    // A failed or stale worker is never reported as a successful completion.
                    if let Ok(mut snapshot) = shared.snapshot.lock() {
                        snapshot
                            .active
                            .retain(|activity| activity.chat_id != Some(id));
                    }
                    if let Some(pending) = scheduler.jobs.get_mut(&id) {
                        if completion.revision == current_revision {
                            completed.pushes.append(&mut pending.pushes);
                            if completed.pushes.len() > 512
                                || completed
                                    .pushes
                                    .iter()
                                    .map(|push| push.estimated_bytes())
                                    .sum::<usize>()
                                    > 4 * 1024 * 1024
                            {
                                completed.pushes.clear();
                                completed.force = true;
                                scheduler.overflow_count =
                                    scheduler.overflow_count.saturating_add(1);
                            }
                            completed.head = completed.head.max(pending.head);
                            completed.quiet_deadline =
                                completed.quiet_deadline.max(pending.quiet_deadline);
                            completed.force |= pending.force;
                            completed.history |= pending.history;
                            if pending.catalog_ready.is_some() && completed.catalog_ready.is_none()
                            {
                                completed.catalog_ready = pending.catalog_ready;
                            }
                            completed.paused = pending.paused;
                            if pending.paused {
                                completed.failure = pending.failure;
                            } else {
                                match result {
                                    Ok(()) => {
                                        completed.retries = 0;
                                        completed.failure = None;
                                        completed.retry_at = None;
                                        completed.rate_limited = false;
                                    }
                                    Err(error) if error.kind == ApplicationErrorKind::Cancelled => {
                                        completed.history = shared
                                            .history
                                            .lock()
                                            .is_ok_and(|history| history.pending(id));
                                        completed.failure = None;
                                        completed.retry_at = None;
                                        completed.rate_limited = false;
                                    }
                                    Err(error) => {
                                        let now = clock();
                                        let phase = completed.fail(
                                            &error,
                                            now,
                                            entropy(id, completed.retries),
                                        );
                                        if error.retry_after.is_some() {
                                            flood_until = merged_flood_deadline(
                                                flood_until,
                                                completed.retry_at,
                                                now,
                                            );
                                        }
                                        if completed.paused
                                            && let Ok(mut history) = shared.history.lock()
                                        {
                                            if let Some(request) = completed.history_request {
                                                history.finish(
                                                    id,
                                                    request,
                                                    HistoryStatus::Failed(error.kind),
                                                );
                                            } else {
                                                history.fail(id, error.kind);
                                            }
                                        }
                                        publish(
                                            &shared,
                                            phase,
                                            Some(id),
                                            Some(error.kind),
                                            completed.retry_at,
                                        );
                                    }
                                }
                            }
                            *pending = *completed;
                        } else {
                            pending.state = None;
                            pending.force = true;
                        }
                        if !pending.paused && needed(pending) {
                            scheduler.enqueue_after_completion(id, was_waiting_before_completion);
                        }
                    }
                }
            }
        }
        if source_probe_started
            .is_some_and(|at| clock().saturating_duration_since(at) >= Duration::from_secs(1))
        {
            // An ordinary empty liveness response is silent; a slow request is visible once.
            source_probe = false;
            source_probe_started = None;
            publish(&shared, ChannelSyncPhase::Discovering, None, None, None);
        }
        while let Ok(command) = receiver.try_recv() {
            if let Command::CancelDirectory = &command {
                directory_retry.cancelled();
                source_refresh_pending = false;
                source_probe = false;
                source_probe_started = None;
                scheduler.source_failure = Some(ApplicationErrorKind::Cancelled);
                last_check = clock();
                let directory_active = shared
                    .active
                    .lock()
                    .is_ok_and(|active| active.contains_key(&0));
                if !directory_active {
                    publish(
                        &shared,
                        ChannelSyncPhase::Cancelled,
                        None,
                        Some(ApplicationErrorKind::Cancelled),
                        None,
                    );
                }
                continue;
            }
            if let Command::CancelPendingRetries = &command {
                let cancelled = scheduler.cancel_pending_retries();
                if let Ok(mut history) = shared.history.lock() {
                    for chat_id in &cancelled {
                        history.fail(*chat_id, ApplicationErrorKind::Cancelled);
                    }
                }
                publish_cancelled_retries(&shared, &cancelled);
                publish_pending_retry_count(&shared, scheduler.pending_retry_count);
                continue;
            }
            if refreshes_directory(&command, scheduler.source_failure) {
                scheduler.source_failure = None;
                let requested_at = clock();
                request_directory_refresh(
                    &mut directory_retry,
                    &mut source_refresh_pending,
                    &mut last_check,
                    requested_at,
                    flood_until,
                );
                let rate_limited = flood_until.is_some_and(|at| at > requested_at);
                publish(
                    &shared,
                    if rate_limited {
                        ChannelSyncPhase::RateLimited
                    } else {
                        ChannelSyncPhase::Queued
                    },
                    None,
                    rate_limited.then_some(scheduler.source_failure).flatten(),
                    directory_retry.retry_at,
                );
            }
            handle_command(command, &library, account.id, &mut scheduler, &shared);
        }
        let (revision, ready_account, next_signals) = lifecycle.snapshot();
        if revision != connection_revision {
            if let Some(old) = signals.take()
                && let Some(wake) = bound_waker.take()
            {
                old.clear_waker(&wake);
            }
            connection_revision = revision;
            metadata_ready = false;
            if ready_account == Some(account.id) {
                signals = next_signals;
                if let Some(signals) = &signals {
                    let wake = transport_waker(&shared, thread::current());
                    signals.set_waker(Some(wake.clone()));
                    bound_waker = Some(wake);
                }
                // A new reactor has its own peer cache and push coverage. Discover
                // metadata again, then recover from durable cursors, never reset them.
                scheduler.source_failure = None;
                directory_retry.new_connection(clock());
                for id in scheduler.jobs.keys().copied().collect::<Vec<_>>() {
                    let was_waiting = scheduler.is_waiting_retry(id);
                    if let Some(job) = scheduler.jobs.get_mut(&id) {
                        job.state = None;
                        job.pushes.clear();
                        job.force = true;
                        if matches!(
                            job.failure,
                            Some(
                                ApplicationErrorKind::Network
                                    | ApplicationErrorKind::Server
                                    | ApplicationErrorKind::Authorization
                            )
                        ) {
                            job.paused = false;
                            job.failure = None;
                            job.retry_at = None;
                            job.rate_limited = false;
                        }
                    }
                    scheduler.adjust_pending_retry_count(id, was_waiting);
                }
                for id in scheduler.jobs.keys().copied().collect::<Vec<_>>() {
                    scheduler.enqueue(id);
                }
            }
        }
        let Some(bound_signals) = &signals else {
            publish(
                &shared,
                ChannelSyncPhase::Connecting,
                None,
                Some(ApplicationErrorKind::Authorization),
                None,
            );
            thread::park();
            continue;
        };
        let hints = bound_signals.take();

        if let Ok(mut snapshot) = shared.snapshot.lock() {
            snapshot.overflow_signals = snapshot
                .overflow_signals
                .saturating_add(hints.overflow_count);
        }
        if !hints.pushes.is_empty() || !hints.channels.is_empty() {
            let now = clock();
            scheduler.transport_available(now, flood_until);
            if scheduler.source_failure == Some(ApplicationErrorKind::Network)
                && flood_until.is_none_or(|at| at <= now)
            {
                directory_retry.request(now, flood_until);
            }
        }
        for push in hints.pushes {
            if let Ok(history) = shared.history.lock() {
                history.yield_to_live(push.channel_id);
            }
            scheduler.receive_push(push);
        }
        if scheduler.overflow_count > 0 {
            if let Ok(mut snapshot) = shared.snapshot.lock() {
                snapshot.overflow_signals = snapshot
                    .overflow_signals
                    .saturating_add(std::mem::take(&mut scheduler.overflow_count));
            }
            shared.changes.send_replace(());
        }
        if !hints.channels.is_empty() || hints.metadata_changed || hints.reconcile_all {
            last_delivery = clock();
        }
        for (id, pts) in hints.channels {
            if pts > 0
                && let Some(job) = scheduler.jobs.get_mut(&id)
            {
                job.quiet_deadline = Some(clock() + UPDATE_SILENCE_RECOVERY);
            }
            source_refresh_pending |= !scheduler.jobs.contains_key(&id);
            scheduler.hint(id, pts);
        }
        source_refresh_pending |= hints.metadata_changed;
        if hints.reconcile_all {
            source_refresh_pending = true;
            // A broken transport/overflow may have lost which channel changed.
            // Keep local rows and recover from each committed PTS in the background.
            for id in scheduler.jobs.keys().copied().collect::<Vec<_>>() {
                scheduler.hint(id, 0);
            }
        }
        if executions.available(0)
            && (directory_retry.retry_at.is_some_and(|at| at <= clock())
                || (directory_retry.retry_at.is_none()
                    && scheduler.source_failure.is_none()
                    && (clock().saturating_duration_since(last_check.max(last_delivery))
                        >= UPDATE_SILENCE_RECOVERY
                        || (source_refresh_pending
                            && clock().saturating_duration_since(last_check)
                                >= Duration::from_secs(1)))))
            && flood_until.is_none_or(|at| at <= clock())
        {
            source_probe =
                metadata_ready && !source_refresh_pending && directory_retry.retry_at.is_none();
            source_probe_started = source_probe.then(|| clock());
            if !source_probe {
                publish(&shared, ChannelSyncPhase::Discovering, None, None, None);
            }
            let cancellation = TelegramScanCancellation::new();
            if let Ok(mut active) = shared.active.lock() {
                active.insert(0, cancellation.clone());
            }
            if lifecycle.snapshot().0 != connection_revision {
                cancellation.cancel();
            }
            let remote = telegram.clone();
            let storage = library.clone();
            let source_account = account.clone();
            let worker_shared = shared.clone();
            let probe = source_probe;
            let persist_baseline = !metadata_ready;
            let result = executions.spawn(0, connection_revision, move || {
                let result = remote
                    .sync_sources(source_account.id, cancellation.clone())
                    .and_then(|chats| {
                        if cancellation.is_cancelled() {
                            return Err(
                                ApplicationError::new(ApplicationErrorKind::Cancelled).into()
                            );
                        }
                        if chats.len() > MAX_SOURCES {
                            return Err(
                                ApplicationError::new(ApplicationErrorKind::Capacity).into()
                            );
                        }
                        let changed = {
                            let sources = worker_shared.sources.lock().map_err(|_| {
                                ApplicationError::new(ApplicationErrorKind::Conflict)
                            })?;
                            sources.0 == 0 || !same_source_metadata(sources.1.as_slice(), &chats)
                        };
                        if !changed && !persist_baseline {
                            return Ok(chats);
                        }
                        if probe {
                            publish(
                                &worker_shared,
                                ChannelSyncPhase::Discovering,
                                None,
                                None,
                                None,
                            );
                        }
                        publish(
                            &worker_shared,
                            ChannelSyncPhase::Persisting,
                            None,
                            None,
                            None,
                        );
                        storage.save_channel_directory(
                            source_account,
                            chats.clone(),
                            cancellation.clone(),
                        )?;
                        Ok(chats)
                    });
                execution::Outcome::Sources(result)
            });
            match result {
                Ok(()) => {
                    source_refresh_pending = false;
                    last_check = clock();
                }
                Err(error) => {
                    source_probe_started = None;
                    if let Ok(mut active) = shared.active.lock() {
                        active.remove(&0);
                    }
                    scheduler.source_failure = Some(error.kind());
                    directory_retry.failed_with_entropy(
                        &ChannelSyncFailure::from(error.clone()),
                        clock(),
                        entropy(account.id, directory_retry.attempt),
                    );
                    publish(
                        &shared,
                        if directory_retry.retry_at.is_some() {
                            ChannelSyncPhase::Waiting
                        } else {
                            ChannelSyncPhase::Failed
                        },
                        None,
                        Some(error.kind()),
                        directory_retry.retry_at,
                    );
                }
            }
        }
        let failed_channels = scheduler
            .jobs
            .values()
            .filter(|job| job.failure.is_some() && job.paused)
            .count();
        if let Ok(mut snapshot) = shared.snapshot.lock() {
            snapshot.queued = scheduler.queue.len();
            snapshot.failed_channels = failed_channels;
        }
        publish_pending_retry_count(&shared, scheduler.pending_retry_count);
        let now = clock();
        scheduler.recover_quiet_channels(now);
        if let Ok(mut journal) = shared.deltas.lock() {
            journal.retain_sources(&scheduler.jobs);
        }
        executions.reserve_managed(shared.managed_id.load(Ordering::Acquire));
        let next = if !metadata_ready || flood_until.is_some_and(|at| at > now) {
            None
        } else {
            scheduler.pop_available(now, |id| executions.available(id))
        };
        publish_pending_retry_count(&shared, scheduler.pending_retry_count);
        if let Some(id) = next {
            let token = TelegramScanCancellation::new();
            if let Ok(mut active) = shared.active.lock() {
                active.insert(id, token.clone());
            }
            if lifecycle.snapshot().0 != connection_revision {
                token.cancel();
            }
            let Some(pending) = scheduler.jobs.get_mut(&id) else {
                continue;
            };
            let pushes = std::mem::take(&mut pending.pushes);
            let mut job = pending.clone();
            job.pushes = pushes;
            let remote = telegram.clone();
            let storage = library.clone();
            let worker_shared = shared.clone();
            let account_id = account.id;
            match executions.spawn(id, connection_revision, move || {
                let result = step(
                    &remote,
                    &storage,
                    account_id,
                    id,
                    &mut job,
                    &worker_shared,
                    token,
                );
                execution::Outcome::Channel(Box::new(job), result)
            }) {
                Ok(()) => {
                    pending.pushes.clear();
                    pending.force = false;
                    pending.history = false;
                    pending.history_request = None;
                    scheduler.in_flight.insert(id);
                }
                Err(error) => {
                    if let Ok(mut active) = shared.active.lock() {
                        active.remove(&id);
                    }
                    pending.paused = true;
                    pending.force = true;
                    pending.failure = Some(error.kind());
                    scheduler.overflow_count = scheduler.overflow_count.saturating_add(1);
                    if let Ok(mut history) = shared.history.lock() {
                        history.fail(id, error.kind());
                    }
                    publish(
                        &shared,
                        ChannelSyncPhase::Failed,
                        Some(id),
                        Some(error.kind()),
                        None,
                    );
                }
            }
        } else {
            let pending_channel_chat = scheduler.queue.iter().find(|id| {
                !scheduler.in_flight.contains(id)
                    && scheduler
                        .jobs
                        .get(id)
                        .is_some_and(|job| !job.paused && needed(job))
            });
            let channel_work_pending = pending_channel_chat.is_some();
            let retry_work_pending = channel_work_pending
                || source_refresh_pending
                || directory_retry.retry_at.is_some();
            let retry_at = retry_work_pending
                .then_some(flood_until.filter(|at| *at > now))
                .flatten()
                .or_else(|| {
                    scheduler
                        .queue
                        .iter()
                        .filter(|id| !scheduler.in_flight.contains(id))
                        .filter_map(|id| scheduler.jobs.get(id)?.retry_at)
                        .filter(|at| *at > now)
                        .chain(directory_retry.retry_at.filter(|at| *at > now))
                        .min()
                });
            let directory_failure = (source_refresh_pending || directory_retry.retry_at.is_some())
                .then_some(scheduler.source_failure)
                .flatten();
            let (failure_chat, failure) = directory_failure
                .map(|error| (None, Some(error)))
                .or_else(|| {
                    scheduler
                        .jobs
                        .iter()
                        .find_map(|(id, job)| job.failure.map(|error| (Some(*id), Some(error))))
                })
                .or_else(|| pending_channel_chat.map(|id| (Some(*id), None)))
                .or_else(|| scheduler.source_failure.map(|error| (None, Some(error))))
                .unwrap_or((None, None));
            if !executions.any() {
                publish(
                    &shared,
                    if retry_work_pending && flood_until.is_some_and(|at| at > now) {
                        ChannelSyncPhase::RateLimited
                    } else if retry_at.is_some() {
                        ChannelSyncPhase::Waiting
                    } else if failure == Some(ApplicationErrorKind::Cancelled) {
                        ChannelSyncPhase::Cancelled
                    } else if failure.is_some() {
                        ChannelSyncPhase::Failed
                    } else {
                        ChannelSyncPhase::Idle
                    },
                    failure_chat,
                    failure,
                    retry_at,
                );
            }
            let metadata_deadline = directory_retry.retry_at.or_else(|| {
                (scheduler.source_failure.is_none() && executions.available(0)).then(|| {
                    source_reconciliation_deadline(
                        last_check,
                        last_delivery,
                        source_refresh_pending,
                    )
                })
            });
            let deadline = retry_at
                .into_iter()
                .chain(scheduler.quiet_deadline())
                .chain(metadata_deadline)
                .chain(source_probe_started.map(|at| at + Duration::from_secs(1)))
                .min()
                .map(|deadline| {
                    flood_until
                        .filter(|at| *at > now)
                        .map_or(deadline, |flood| deadline.max(flood))
                });
            // Only actual recovery/coalescing deadlines wake an otherwise idle owner.
            // Permanent failures and occupied workers wait for a command or completion.
            if let Some(deadline) = deadline {
                thread::park_timeout(deadline.saturating_duration_since(clock()));
            } else {
                thread::park();
            }
        }
    }
    cancel_active(&shared);
    drop(executions);
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        snapshot.active.clear();
    }
    if let Some(signals) = signals
        && let Some(wake) = bound_waker
    {
        signals.clear_waker(&wake);
    }
    publish(&shared, ChannelSyncPhase::Cancelled, None, None, None);
}

trait AccountSource: ChannelSource + Clone + Send + Sync + 'static {
    fn lifecycle(&self) -> crate::telegram::lifecycle::Lifecycle;
    fn sync_sources(
        &self,
        account: i64,
        cancellation: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ChannelSyncFailure>;
}
impl AccountSource for DesktopTelegram {
    fn lifecycle(&self) -> crate::telegram::lifecycle::Lifecycle {
        self.lifecycle()
    }
    fn sync_sources(
        &self,
        account: i64,
        cancellation: TelegramScanCancellation,
    ) -> Result<Vec<TelegramChatSummary>, ChannelSyncFailure> {
        self.sync_sources(account, cancellation)
    }
}

trait ChannelSource {
    fn read(
        &self,
        account: i64,
        chat: i64,
        request: ChannelRead,
        cancellation: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure>;
    fn read_managed(
        &self,
        account: i64,
        chat: i64,
        request: ChannelRead,
        cancellation: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        self.read(account, chat, request, cancellation)
    }
}

impl ChannelSource for DesktopTelegram {
    fn read(
        &self,
        account: i64,
        chat: i64,
        request: ChannelRead,
        cancellation: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        self.sync_channel(account, chat, request, cancellation, false)
    }
    fn read_managed(
        &self,
        account: i64,
        chat: i64,
        request: ChannelRead,
        cancellation: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        self.sync_channel(account, chat, request, cancellation, true)
    }
}

fn step(
    telegram: &impl ChannelSource,
    library: &DesktopLibrary,
    account: i64,
    chat: i64,
    job: &mut Job,
    shared: &Shared,
    cancellation: TelegramScanCancellation,
) -> Result<(), ChannelSyncFailure> {
    if job.catalog_ready.is_some() && !job.watch_loaded {
        publish(
            shared,
            ChannelSyncPhase::ReadingLocal,
            Some(chat),
            None,
            None,
        );
        let watch = library.managed_channel_watch(account, chat, true)?;
        job.catalog_ready = Some(watch.catalog_ready);
        job.watch_loaded = true;
        if let Ok(mut snapshot) = shared.snapshot.lock() {
            snapshot.managed_watch = Some(watch);
        }
        shared.changes.send_replace(());
    }
    if job.state.is_none() {
        publish(
            shared,
            ChannelSyncPhase::ReadingLocal,
            Some(chat),
            None,
            None,
        );
        job.state = Some(library.worker.request("channel_sync_state", |reply| {
            StorageRequest::ChannelSyncState {
                account: AccountId::new(account),
                chat: ChatId::new(chat),
                reply,
            }
        })?);
        let now = Instant::now();
        let saved = job
            .state
            .as_ref()
            .and_then(|state| state.last_synced_at_unix_ms);
        job.quiet_deadline = Some(restored_quiet_deadline(
            saved,
            crate::system_time_unix_ms(std::time::SystemTime::now()).unwrap_or(0),
            now,
        ));
        job.force |= job.quiet_deadline.is_some_and(|deadline| deadline <= now);
    }
    if job.history
        && job
            .state
            .as_ref()
            .is_some_and(|state| state.history_exhausted)
    {
        if let Ok(mut history) = shared.history.lock()
            && let Some(id) = history.attach(chat, cancellation.clone())
        {
            history.finish(chat, id, HistoryStatus::Complete);
        }
        job.history = false;
        shared.changes.send_replace(());
    }
    if !needed(job) {
        return Ok(());
    }
    let mut state = job
        .state
        .clone()
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    let observation = shared.observation.load(Ordering::Acquire);
    while job.pushes.front().is_some_and(|push| push.pts <= state.pts) {
        job.pushes.pop_front();
    }
    let direct = !job.force
        && job.pushes.front().is_some_and(|push| {
            push.pts_count > 0 && state.pts.checked_add(push.pts_count) == Some(push.pts)
        });
    let bootstrap = state.revision == 0;
    let repair_existing = if bootstrap {
        !library
            .worker
            .request("channel_legacy_cache", |reply| {
                StorageRequest::ChannelCachedIds {
                    account: AccountId::new(account),
                    chat: ChatId::new(chat),
                    before: None,
                    reply,
                }
            })?
            .is_empty()
    } else {
        false
    };
    publish(
        shared,
        ChannelSyncPhase::CheckingPts,
        Some(chat),
        None,
        None,
    );
    let read = if bootstrap {
        // Capture the dialog PTS before seeding one bounded recent-history page;
        // always run a difference afterwards to cover edits during that read.
        ChannelRead::History(None)
    } else if direct {
        ChannelRead::Push
    } else if job.force || job.head.is_some_and(|pts| pts > state.pts) {
        ChannelRead::Difference(state.pts.max(1))
    } else if job.catalog_ready == Some(false) {
        ChannelRead::ManagedManifests
    } else if state.gap_pending {
        ChannelRead::GapHistory(state.gap_before)
    } else if state.repair_pending {
        publish(
            shared,
            ChannelSyncPhase::ReadingLocal,
            Some(chat),
            None,
            None,
        );
        let ids = library.worker.request("channel_cached_ids", |reply| {
            StorageRequest::ChannelCachedIds {
                account: AccountId::new(account),
                chat: ChatId::new(chat),
                before: state.repair_before,
                reply,
            }
        })?;
        ChannelRead::Verify(ids)
    } else {
        ChannelRead::History(state.history_before)
    };
    let history_id = if !bootstrap && matches!(read, ChannelRead::History(_)) {
        let id = shared
            .history
            .lock()
            .ok()
            .and_then(|mut history| history.attach(chat, cancellation.clone()));
        if id.is_none() {
            job.history = false;
            return Ok(());
        }
        job.history_request = id;
        id
    } else {
        None
    };
    publish(
        shared,
        if bootstrap {
            ChannelSyncPhase::Seeding
        } else if matches!(read, ChannelRead::Verify(_)) {
            ChannelSyncPhase::Verifying
        } else if matches!(read, ChannelRead::History(_) | ChannelRead::GapHistory(_)) {
            ChannelSyncPhase::History
        } else if matches!(read, ChannelRead::Difference(_)) {
            ChannelSyncPhase::PollingDifferences
        } else {
            ChannelSyncPhase::Receiving
        },
        Some(chat),
        None,
        None,
    );
    let page = if matches!(read, ChannelRead::Push) {
        let push = job
            .pushes
            .front()
            .cloned()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Conflict))?;
        ChannelReadPage {
            files: push.files.into_iter().map(update_summary).collect(),
            removed: push.removed,
            edited: push.edited,
            pts: Some(push.pts),
            complete: true,
            history_gap: false,
            before: None,
        }
    } else if matches!(&read, ChannelRead::Verify(ids) if ids.is_empty()) {
        ChannelReadPage {
            files: Vec::new(),
            removed: Vec::new(),
            pts: None,
            complete: true,
            history_gap: false,
            before: None,
            edited: Vec::new(),
        }
    } else {
        if shared.managed_id.load(Ordering::Acquire) == chat {
            telegram.read_managed(account, chat, read.clone(), cancellation.clone())?
        } else {
            telegram.read(account, chat, read.clone(), cancellation.clone())?
        }
    };
    if cancellation.is_cancelled() || shared.stop.load(Ordering::Acquire) {
        return Err(ApplicationError::new(ApplicationErrorKind::Cancelled).into());
    }
    let mut next_force = job.force;
    match &read {
        ChannelRead::Push | ChannelRead::Difference(_) => {
            state.pts = page
                .pts
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Network))?;
            next_force = !page.complete;
            if page.history_gap {
                state.repair_pending = true;
                state.repair_before = None;
                if !state.gap_pending {
                    // Capture the previous local head before applying the short
                    // server snapshot. Fill every intervening message page,
                    // even when none of those messages contain documents.
                    publish(
                        shared,
                        ChannelSyncPhase::ReadingLocal,
                        Some(chat),
                        None,
                        None,
                    );
                    let ids = library.worker.request("channel_gap_boundary", |reply| {
                        StorageRequest::ChannelCachedIds {
                            account: AccountId::new(account),
                            chat: ChatId::new(chat),
                            before: None,
                            reply,
                        }
                    })?;
                    state.gap_until = ids.first().copied().or(state.history_before);
                }
                state.gap_pending = true;
                state.gap_before = None;
            }
        }
        ChannelRead::GapHistory(_) => {
            state.gap_before = page.before;
            state.gap_pending = !page.complete
                && !page
                    .before
                    .zip(state.gap_until)
                    .is_some_and(|(before, until)| before <= until);
        }
        ChannelRead::History(_) => {
            state.history_before = page.before;
            state.history_exhausted = page.complete;
            if bootstrap {
                state.pts = job.head.unwrap_or(1).max(1);
                next_force = true;
                state.repair_pending = repair_existing;
            }
        }
        ChannelRead::ManagedManifests => {
            next_force = true;
        }
        ChannelRead::Verify(ids) => {
            state.repair_pending = ids.len() == 100;
            state.repair_before = ids.last().copied();
        }
    }
    // The protocol may return an unchanged final difference. Persisting a new
    // revision for that response would only create database and UI churn.
    if job.state.as_ref() == Some(&state)
        && page.files.is_empty()
        && page.removed.is_empty()
        && page.edited.is_empty()
        && !matches!(read, ChannelRead::ManagedManifests)
    {
        // Even an empty successful difference is durable synchronization
        // evidence. Keep the row revision stable while persisting that fact.
        let at = crate::system_time_unix_ms(std::time::SystemTime::now()).unwrap_or(0);
        publish(shared, ChannelSyncPhase::Persisting, Some(chat), None, None);
        library
            .worker
            .request("record_channel_sync_observation", |reply| {
                StorageRequest::RecordChannelSyncObservation {
                    account: AccountId::new(account),
                    chat: ChatId::new(chat),
                    revision: state.revision,
                    at,
                    reply,
                }
            })?;
        state.last_synced_at_unix_ms = Some(at.max(state.last_synced_at_unix_ms.unwrap_or(0)));
        job.state = Some(state.clone());
        job.quiet_deadline = Some(Instant::now() + UPDATE_SILENCE_RECOVERY);
        job.force = next_force;
        if let Some(id) = history_id {
            job.history = false;
            if let Ok(mut history) = shared.history.lock() {
                history.finish(chat, id, HistoryStatus::Complete);
            }
        }
        if let Ok(mut snapshot) = shared.snapshot.lock() {
            snapshot.last_activity = Instant::now();
            if shared.managed_id.load(Ordering::Acquire) == chat
                && !next_force
                && job.head.is_none_or(|head| head <= state.pts)
                && shared.observation.load(Ordering::Acquire) == observation
            {
                snapshot.managed_review_pending = false;
            }
        }
        publish_completed(shared, Some(chat));
        shared.changes.send_replace(());
        return Ok(());
    }
    let expected_revision = state.revision;
    state.last_synced_at_unix_ms =
        Some(crate::system_time_unix_ms(std::time::SystemTime::now()).unwrap_or(0));
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
    let files =
        super::telegram_file_upserts(AccountId::new(account), ChatId::new(chat), &page.files)?;
    let managed_files_changed = job.catalog_ready.is_some()
        && (matches!(read, ChannelRead::ManagedManifests)
            || !page.edited.is_empty()
            || !page.removed.is_empty()
            || page
                .files
                .iter()
                .any(|file| file.caption == crate::transfer::MANIFEST_CAPTION));
    let batch = ChannelSyncCommit {
        edited: page.edited,
        invalidate: if job.catalog_ready.is_some() && matches!(read, ChannelRead::Verify(_)) {
            page.files.iter().map(|file| file.message_id).collect()
        } else {
            vec![]
        },
        managed_catalog_seed: matches!(read, ChannelRead::ManagedManifests),
        gap_detected: page.history_gap,
        observed_at_unix_ms: crate::system_time_unix_ms(std::time::SystemTime::now()).unwrap_or(0),
        account_id: AccountId::new(account),
        chat_id: ChatId::new(chat),
        expected_revision,
        state: state.clone(),
        files,
        removed: page.removed,
        authoritative: !matches!(
            read,
            ChannelRead::History(_) | ChannelRead::GapHistory(_) | ChannelRead::ManagedManifests
        ),
    };
    publish(shared, ChannelSyncPhase::Persisting, Some(chat), None, None);
    let outcome = library.worker.request("commit_channel_sync", |reply| {
        StorageRequest::CommitChannelSync { batch, reply }
    })?;
    let material =
        !outcome.upserted.is_empty() || !outcome.removed.is_empty() || managed_files_changed;
    if let Ok(mut journal) = shared.deltas.lock() {
        journal.append(ChannelDelta {
            chat_id: chat,
            revision: state.revision,
            upserted: outcome
                .upserted
                .into_iter()
                .map(crate::cached_file_summary)
                .collect(),
            removed: outcome.removed,
            history_exhausted: state.history_exhausted,
            managed_files_changed,
        });
    }
    if matches!(read, ChannelRead::Push) {
        job.pushes.pop_front();
    }
    if matches!(read, ChannelRead::ManagedManifests) {
        job.catalog_ready = Some(true);
    }
    let caught_up = !next_force && job.head.is_none_or(|head| head <= state.pts);
    job.state = Some(state);
    job.quiet_deadline = Some(Instant::now() + UPDATE_SILENCE_RECOVERY);
    job.force = next_force;
    if !bootstrap && matches!(read, ChannelRead::History(_)) {
        job.history = false;
        if let Some(id) = history_id
            && let Ok(mut history) = shared.history.lock()
        {
            history.finish(chat, id, HistoryStatus::Complete);
        }
    }
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        if material {
            snapshot.data_revision = snapshot.data_revision.saturating_add(1);
        }
        snapshot.committed_pages = snapshot.committed_pages.saturating_add(1);
        snapshot.last_activity = Instant::now();
        if shared.managed_id.load(Ordering::Acquire) == chat {
            if let Some(watch) = outcome.managed_watch {
                snapshot.managed_watch = Some(watch);
            }
            if caught_up && shared.observation.load(Ordering::Acquire) == observation {
                snapshot.managed_review_pending = false;
            }
        }
    }
    shared.changes.send_replace(());
    publish_completed(shared, Some(chat));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn chat(id: i64, pts: i32) -> TelegramChatSummary {
        TelegramChatSummary {
            id,
            sync_pts: Some(pts),
            name: format!("Fixture {id}"),
            username: None,
            kind: TelegramChatKind::Channel,
        }
    }
    fn file(id: i64) -> TelegramFileSummary {
        TelegramFileSummary {
            message_id: id,
            sent_at_unix_ms: 1_000,
            modified_at_unix_ms: 1_000,
            file_name: format!("{id}.pdf"),
            caption: String::new(),
            mime_type: None,
            size_bytes: 42,
        }
    }
    fn shared() -> Shared {
        Shared {
            snapshot: Mutex::new(ChannelSyncSnapshot::new(1, 1)),
            pending_channel_retries: AtomicU64::new(0),
            changes: tokio::sync::watch::channel(()).0,
            deltas: Mutex::new(feed::DeltaJournal::default()),
            sources: Mutex::new((0, Arc::new(Vec::new()))),
            history: Mutex::new(history::Requests::default()),
            managed_id: AtomicI64::new(0),
            observation: AtomicU64::new(0),
            manifest_generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            active: Mutex::new(BTreeMap::new()),
        }
    }
    fn setup() -> (tempfile::TempDir, DesktopLibrary, Scheduler) {
        let temp = tempfile::tempdir().expect("fixture directory");
        let library =
            DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("fixture database");
        let chats = vec![chat(2, 50), chat(3, 50)];
        library
            .save_telegram_sources(
                &TelegramAccount {
                    id: 1,
                    display_name: "Fixture".into(),
                    username: None,
                },
                &chats,
            )
            .expect("fixture sources");
        (temp, library, Scheduler::new(chats))
    }
    struct Source {
        calls: RefCell<Vec<ChannelRead>>,
        pages: RefCell<VecDeque<ChannelReadPage>>,
    }
    impl ChannelSource for Source {
        fn read(
            &self,
            account: i64,
            chat: i64,
            request: ChannelRead,
            _: TelegramScanCancellation,
        ) -> Result<ChannelReadPage, ChannelSyncFailure> {
            assert_eq!((account, chat), (1, 2));
            self.calls.borrow_mut().push(request);
            Ok(self
                .pages
                .borrow_mut()
                .pop_front()
                .expect("planned response"))
        }
    }
    fn page(
        pts: Option<i32>,
        files: Vec<TelegramFileSummary>,
        removed: Vec<i64>,
    ) -> ChannelReadPage {
        ChannelReadPage {
            files,
            removed,
            pts,
            complete: true,
            history_gap: false,
            before: None,
            edited: Vec::new(),
        }
    }

    #[test]
    fn selection_only_promotes_existing_work_and_duplicate_hints_do_not_scan() {
        let mut scheduler = Scheduler::new(vec![chat(2, 50), chat(3, 50)]);
        for job in scheduler.jobs.values_mut() {
            job.state = Some(ChannelSyncState {
                revision: 1,
                pts: 50,
                ..Default::default()
            });
        }
        scheduler.queue.clear();
        for _ in 0..100 {
            scheduler.command(Command::Prioritize(2));
            scheduler.hint(2, 50);
            scheduler.hint(2, 49);
        }
        assert!(scheduler.pop(Instant::now()).is_none());
        scheduler.hint(3, 51);
        scheduler.hint(2, 52);
        scheduler.hint(2, 52);
        assert_eq!(scheduler.queue.len(), 2);
        assert_eq!(scheduler.pop(Instant::now()), Some(2));
        assert_eq!(scheduler.pop(Instant::now()), Some(3));
    }

    #[test]
    fn initial_cache_then_differences_and_explicit_history_have_separate_cursors() {
        let (_temp, library, mut scheduler) = setup();
        library
            .cache_telegram_files(1, 2, &[file(120)])
            .expect("legacy cache requiring verification");
        let source = Source {
            calls: RefCell::new(Vec::new()),
            pages: RefCell::new(VecDeque::from([
                ChannelReadPage {
                    complete: false,
                    before: Some(100),
                    ..page(None, vec![file(120)], vec![])
                },
                page(Some(51), vec![file(121)], vec![120]),
                page(None, vec![file(121)], vec![120]),
                ChannelReadPage {
                    before: Some(20),
                    complete: false,
                    ..page(None, vec![file(90)], vec![])
                },
            ])),
        };
        let shared = shared();
        let job = scheduler.jobs.get_mut(&2).expect("job");
        step(
            &source,
            &library,
            1,
            2,
            job,
            &shared,
            TelegramScanCancellation::new(),
        )
        .expect("seed");
        assert!(matches!(
            source.calls.borrow()[0],
            ChannelRead::History(None)
        ));
        assert_eq!(job.state.as_ref().expect("state").pts, 50);
        assert!(job.force);
        step(
            &source,
            &library,
            1,
            2,
            job,
            &shared,
            TelegramScanCancellation::new(),
        )
        .expect("difference");
        assert!(matches!(
            source.calls.borrow()[1],
            ChannelRead::Difference(50)
        ));
        assert_eq!(job.state.as_ref().expect("state").history_before, Some(100));
        assert_eq!(
            library
                .cached_telegram_files(1, 2, 100)
                .expect("local cache"),
            vec![file(121)]
        );
        step(
            &source,
            &library,
            1,
            2,
            job,
            &shared,
            TelegramScanCancellation::new(),
        )
        .expect("verify legacy cache");
        assert!(matches!(source.calls.borrow()[2], ChannelRead::Verify(_)));
        assert!(!needed(job));
        job.history = true;
        shared.history.lock().expect("history").begin(2);
        step(
            &source,
            &library,
            1,
            2,
            job,
            &shared,
            TelegramScanCancellation::new(),
        )
        .expect("explicit history");
        assert!(matches!(
            source.calls.borrow()[3],
            ChannelRead::History(Some(100))
        ));
        assert_eq!(job.state.as_ref().expect("state").pts, 51);
        assert_eq!(job.state.as_ref().expect("state").history_before, Some(20));
    }

    #[test]
    fn difference_overflow_backfills_new_messages_and_resumes_its_own_cursor() {
        let (temp, library, mut scheduler) = setup();
        let state = ChannelSyncState {
            pts: 50,
            revision: 1,
            history_before: Some(20),
            ..Default::default()
        };
        let batch = ChannelSyncCommit {
            edited: vec![],
            invalidate: vec![],
            managed_catalog_seed: false,
            gap_detected: false,
            observed_at_unix_ms: 0,
            account_id: AccountId::new(1),
            chat_id: ChatId::new(2),
            expected_revision: 0,
            state,
            files: super::super::telegram_file_upserts(
                AccountId::new(1),
                ChatId::new(2),
                &[file(100)],
            )
            .expect("upserts"),
            removed: vec![],
            authoritative: true,
        };
        library
            .worker
            .request("fixture", |reply| StorageRequest::CommitChannelSync {
                batch,
                reply,
            })
            .expect("seed");
        scheduler.hint(2, 90);
        let source = Source {
            calls: RefCell::new(vec![]),
            pages: RefCell::new(VecDeque::from([
                ChannelReadPage {
                    history_gap: true,
                    ..page(Some(90), vec![file(900)], vec![])
                },
                ChannelReadPage {
                    complete: false,
                    before: Some(500),
                    ..page(None, vec![file(800)], vec![])
                },
                ChannelReadPage {
                    complete: false,
                    before: Some(100),
                    ..page(None, vec![file(400)], vec![])
                },
            ])),
        };
        let shared = shared();
        let job = scheduler.jobs.get_mut(&2).expect("job");
        step(
            &source,
            &library,
            1,
            2,
            job,
            &shared,
            TelegramScanCancellation::new(),
        )
        .expect("overflow");
        assert_eq!(job.state.as_ref().expect("state").gap_until, Some(100));
        step(
            &source,
            &library,
            1,
            2,
            job,
            &shared,
            TelegramScanCancellation::new(),
        )
        .expect("first gap page");
        assert_eq!(job.state.as_ref().expect("state").gap_before, Some(500));
        drop(library);
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3")).expect("reopen");
        let mut restarted = Scheduler::new(vec![chat(2, 90)]);
        let job = restarted.jobs.get_mut(&2).expect("restarted job");
        step(
            &source,
            &library,
            1,
            2,
            job,
            &shared,
            TelegramScanCancellation::new(),
        )
        .expect("resume gap");
        assert!(matches!(
            source.calls.borrow()[2],
            ChannelRead::GapHistory(Some(500))
        ));
        let state = job.state.as_ref().expect("state");
        assert!(!state.gap_pending);
        assert!(state.repair_pending);
        assert_eq!((state.pts, state.history_before), (90, Some(20)));
        let ids: Vec<_> = library
            .cached_telegram_files(1, 2, 100)
            .expect("cache")
            .into_iter()
            .map(|file| file.message_id)
            .collect();
        assert_eq!(ids, vec![900, 800, 400, 100]);
    }

    #[test]
    fn cancelled_response_never_advances_the_durable_cursor() {
        struct CancellingSource;
        impl ChannelSource for CancellingSource {
            fn read(
                &self,
                _: i64,
                _: i64,
                _: ChannelRead,
                cancellation: TelegramScanCancellation,
            ) -> Result<ChannelReadPage, ChannelSyncFailure> {
                cancellation.cancel();
                Ok(page(None, vec![file(5)], vec![]))
            }
        }
        let (_temp, library, mut scheduler) = setup();
        let shared = shared();
        let result = step(
            &CancellingSource,
            &library,
            1,
            2,
            scheduler.jobs.get_mut(&2).expect("job"),
            &shared,
            TelegramScanCancellation::new(),
        );
        assert_eq!(
            result.expect_err("cancelled").kind,
            ApplicationErrorKind::Cancelled
        );
        assert!(
            library
                .cached_telegram_files(1, 2, 100)
                .expect("cache")
                .is_empty()
        );
        assert_eq!(
            scheduler.jobs[&2].state.as_ref().expect("state").revision,
            0
        );
    }

    #[test]
    fn blocked_network_keeps_cache_and_phase_readable() {
        struct Blocked {
            entered: mpsc::SyncSender<()>,
            release: mpsc::Receiver<()>,
        }
        impl ChannelSource for Blocked {
            fn read(
                &self,
                _: i64,
                _: i64,
                _: ChannelRead,
                cancellation: TelegramScanCancellation,
            ) -> Result<ChannelReadPage, ChannelSyncFailure> {
                self.entered.send(()).expect("report entry");
                self.release.recv().expect("release");
                cancellation.cancel();
                Ok(page(None, vec![], vec![]))
            }
        }
        let (_temp, library, mut scheduler) = setup();
        library
            .cache_telegram_files(1, 2, &[file(7)])
            .expect("old local cache");
        let shared = Arc::new(shared());
        let (entered, arrived) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let worker_library = library.clone();
        let worker_shared = shared.clone();
        let worker = thread::spawn(move || {
            step(
                &Blocked {
                    entered,
                    release: released,
                },
                &worker_library,
                1,
                2,
                scheduler.jobs.get_mut(&2).expect("job"),
                &worker_shared,
                TelegramScanCancellation::new(),
            )
        });
        arrived
            .recv_timeout(Duration::from_secs(5))
            .expect("network entered");
        assert_eq!(
            shared.snapshot.lock().expect("snapshot").phase,
            ChannelSyncPhase::Seeding
        );
        assert_eq!(
            library
                .cached_telegram_files(1, 2, 100)
                .expect("local reads remain responsive"),
            vec![file(7)]
        );
        release.send(()).expect("release");
        assert_eq!(
            worker.join().expect("join").expect_err("cancelled").kind,
            ApplicationErrorKind::Cancelled
        );
    }

    #[test]
    fn selection_and_manual_retry_cannot_bypass_a_rate_limit() {
        let now = Instant::now();
        let mut scheduler = Scheduler::new(vec![chat(2, 50)]);
        let job = scheduler.jobs.get_mut(&2).expect("job");
        job.failure = Some(ApplicationErrorKind::Server);
        job.rate_limited = true;
        job.retry_at = Some(now + Duration::from_secs(30));
        scheduler.command(Command::Prioritize(2));
        scheduler.command(Command::Refresh(2));
        assert_eq!(scheduler.pop(now + Duration::from_secs(29)), None);
        assert_eq!(scheduler.pop(now + Duration::from_secs(30)), Some(2));
        scheduler.command(Command::Cancel(2));
        scheduler.hint(2, 100);
        assert_eq!(scheduler.pop(now + Duration::from_secs(60)), None);
    }

    #[test]
    fn timeline_retains_terminal_events_and_reports_truncation() {
        let mut snapshot = ChannelSyncSnapshot::new(1, 1);
        for _ in 0..EVENT_CAPACITY {
            snapshot.transition(ChannelSyncPhase::Receiving, Some(2), None, None);
            snapshot.transition(ChannelSyncPhase::Persisting, Some(2), None, None);
        }
        snapshot.transition(
            ChannelSyncPhase::Failed,
            Some(2),
            Some(ApplicationErrorKind::Persistence),
            None,
        );
        assert_eq!(snapshot.events.len(), EVENT_CAPACITY);
        assert!(snapshot.dropped_events > 0);
        assert_eq!(
            snapshot.events.back().expect("terminal").phase,
            ChannelSyncPhase::Failed
        );
    }
}

#[cfg(test)]
mod event_tests;

#[cfg(test)]
mod acceptance_tests;
