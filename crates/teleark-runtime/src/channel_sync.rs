//! Account-owned channel synchronization. Selecting a source changes priority,
//! never freshness. All public snapshots are local and cheap to read.
use std::{
    collections::{BTreeMap, VecDeque},
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
        if self.events.len() == EVENT_CAPACITY {
            self.events.pop_front();
            self.dropped_events = self.dropped_events.saturating_add(1);
        }
        self.events.push_back(ChannelSyncEvent {
            phase,
            chat_id,
            at: now,
            failure,
        });
        true
    }
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
        if chats.len() > MAX_SOURCES {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        let scheduler = Scheduler::new(chats);
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(ChannelSyncSnapshot::new(account.id, scheduler.queue.len())),
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
    pub fn cancel(&self, chat: i64) -> Result<(), ApplicationError> {
        if let Ok(active) = self.inner.shared.active.lock()
            && let Some(token) = active.get(&chat)
        {
            token.cancel();
        }
        self.command(Command::Cancel(chat))
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
                Some((ChannelSyncPhase::Queued, *chat))
            }
            Command::Cancel(chat) => Some((ChannelSyncPhase::Cancelled, *chat)),
            Command::Watch(chat) | Command::Acknowledge(chat, _) => {
                Some((ChannelSyncPhase::Queued, *chat))
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
                snapshot.transition(phase, Some(chat), None, None);
            } else {
                if snapshot.events.len() == EVENT_CAPACITY {
                    snapshot.events.pop_front();
                    snapshot.dropped_events = snapshot.dropped_events.saturating_add(1);
                }
                snapshot.events.push_back(ChannelSyncEvent {
                    phase,
                    chat_id: Some(chat),
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
}

// Retry policy is independent of wall-clock time. The owner supplies an entropy
// sample; tests supply fixed samples and advance Instant without sleeping.
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

impl Job {
    fn fail(&mut self, error: &ChannelSyncFailure, now: Instant, entropy: u64) -> ChannelSyncPhase {
        self.failure = Some(error.kind);
        if error.kind == ApplicationErrorKind::Conflict {
            // Reload the committed winner before retrying a failed CAS.
            self.state = None;
        }
        let delay = error
            .retry_after
            .or_else(|| recovery_delay(error.kind, self.retries, entropy));
        self.retry_at = delay.and_then(|delay| now.checked_add(delay));
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
    preferred: Option<i64>,
    preferred_turns: u8,
    source_failure: Option<ApplicationErrorKind>,
    overflow_count: u64,
    in_flight: std::collections::BTreeSet<i64>,
}

impl Scheduler {
    fn new(chats: Vec<TelegramChatSummary>) -> Self {
        let jobs: BTreeMap<_, _> = chats
            .into_iter()
            .filter(|c| c.kind == TelegramChatKind::Channel)
            .map(|c| {
                (
                    c.id,
                    Job {
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
                    },
                )
            })
            .collect();
        Self {
            queue: jobs.keys().copied().collect(),
            jobs,
            preferred: None,
            preferred_turns: 0,
            source_failure: None,
            overflow_count: 0,
            in_flight: Default::default(),
        }
    }
    fn update_sources(&mut self, chats: Vec<TelegramChatSummary>) {
        if chats.len() > MAX_SOURCES {
            self.source_failure = Some(ApplicationErrorKind::Capacity);
            return;
        }
        // Discovery is a complete snapshot. Retire departed sources so stale
        // jobs cannot consume the bounded capacity or issue inaccessible RPCs.
        self.jobs.retain(|id, job| {
            job.catalog_ready.is_some()
                || chats
                    .iter()
                    .any(|chat| chat.id == *id && chat.kind == TelegramChatKind::Channel)
        });
        self.queue.retain(|id| self.jobs.contains_key(id));
        self.source_failure = None;
        for chat in chats
            .into_iter()
            .filter(|chat| chat.kind == TelegramChatKind::Channel)
        {
            if !self.jobs.contains_key(&chat.id) && self.jobs.len() < MAX_SOURCES {
                self.jobs.insert(
                    chat.id,
                    Job {
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
                    },
                );
                self.enqueue(chat.id);
            } else if let Some(pts) = chat.sync_pts {
                self.hint(chat.id, pts);
            }
        }
    }
    fn enqueue(&mut self, id: i64) {
        if self.jobs.contains_key(&id) && !self.queue.contains(&id) {
            self.queue.push_back(id);
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
            Command::Watch(_) | Command::Acknowledge(_, _) => return,
            Command::Sources(chats) => {
                self.update_sources(chats);
                return;
            }
            Command::Prioritize(id)
            | Command::History(id)
            | Command::Refresh(id)
            | Command::Cancel(id) => id,
        };
        if let Command::Prioritize(_) = command {
            self.preferred = Some(id);
            return;
        }
        let Some(job) = self.jobs.get_mut(&id) else {
            return;
        };
        match command {
            Command::Cancel(_) => {
                job.paused = true;
                job.failure = Some(ApplicationErrorKind::Cancelled);
                self.queue.retain(|queued| *queued != id);
            }
            Command::History(_) | Command::Refresh(_) => {
                job.paused = false;
                job.failure = None;
                job.retries = 0;
                // Do not erase a server rate-limit deadline on a manual retry.
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
            | Command::Acknowledge(_, _) => {}
        }
    }
    fn transport_available(&mut self, now: Instant, flood_until: Option<Instant>) {
        if flood_until.is_some_and(|at| at > now) {
            return;
        }
        // Actual pushed data proves delivery resumed. A close/gap hint alone
        // does not. Recover network waits early, preserving non-network errors.
        for id in &self.queue {
            if let Some(job) = self.jobs.get_mut(id)
                && !job.paused
                && job.failure == Some(ApplicationErrorKind::Network)
            {
                job.retry_at = None;
            }
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
    fn pop(&mut self, now: Instant) -> Option<i64> {
        let ready = |id: &i64| {
            !self.in_flight.contains(id)
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
        let id = self.queue.remove(position)?;
        if priority(&id) {
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
        Command::Watch(chat) => {
            for (id, job) in &mut scheduler.jobs {
                if *id != chat && job.catalog_ready.is_some() {
                    job.catalog_ready = None;
                }
            }
            let job = scheduler.jobs.entry(chat).or_insert_with(|| Job {
                head: None,
                state: None,
                force: true,
                history: false,
                history_request: None,
                paused: false,
                failure: None,
                retries: 0,
                retry_at: None,
                pushes: VecDeque::new(),
                catalog_ready: Some(false),
                watch_loaded: false,
            });
            // Registration belongs to the retryable job, so a temporary disk
            // failure cannot silently disable protection or advance its PTS.
            job.catalog_ready = Some(false);
            job.watch_loaded = false;
            job.paused = false;
            job.failure = None;
            shared.managed_id.store(chat, Ordering::Release);
            if let Ok(mut snapshot) = shared.snapshot.lock() {
                if snapshot.managed_chat_id != Some(chat) {
                    snapshot.managed_review_pending = false;
                    snapshot.managed_watch = None;
                }
                snapshot.managed_chat_id = Some(chat);
            }
            scheduler.enqueue(chat);
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
            && snapshot.active.len() < 5
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
) {
    publish(&shared, ChannelSyncPhase::ReadingLocal, None, None, None);
    match library.cached_channel_directory(account.id) {
        Ok(chats) if !chats.is_empty() => {
            publish_sources(&shared, &chats);
            scheduler.update_sources(chats);
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
    let mut last_check = Instant::now();
    let mut last_delivery = last_check;
    let mut source_probe = false;
    let mut source_probe_started: Option<Instant> = None;
    let mut source_retry_at = Some(Instant::now());
    let mut source_attempt = 0_u32;
    let mut flood_until = None;
    let mut source_refresh_pending = false;
    let mut executions = execution::Executions::new();
    let mut metadata_ready = false;
    while !shared.stop.load(Ordering::Acquire) {
        while let Some(completion) = executions.take() {
            if let Ok(mut active) = shared.active.lock() {
                active.remove(&completion.id);
            }
            scheduler.in_flight.remove(&completion.id);
            if completion.id == 0 {
                source_probe_started = None;
            }
            let current_revision = lifecycle.snapshot().0;
            match completion.outcome {
                execution::Outcome::Failed(error) => {
                    if completion.id == 0 {
                        scheduler.source_failure = Some(error);
                        source_retry_at = None;
                    } else if let Some(job) = scheduler.jobs.get_mut(&completion.id) {
                        job.paused = true;
                        job.failure = Some(error);
                        job.state = None;
                        job.force = true;
                        if let Ok(mut history) = shared.history.lock() {
                            history.fail(completion.id, error);
                        }
                    }
                    publish(
                        &shared,
                        ChannelSyncPhase::Failed,
                        (completion.id != 0).then_some(completion.id),
                        Some(error),
                        None,
                    );
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
                            scheduler.update_sources(chats);
                            source_attempt = 0;
                            source_retry_at = None;
                        }
                        Err(error) => {
                            scheduler.source_failure = Some(error.kind);
                            let delay = error.retry_after.or_else(|| {
                                recovery_delay(
                                    error.kind,
                                    source_attempt,
                                    recovery_entropy(account.id, source_attempt),
                                )
                            });
                            source_retry_at =
                                delay.and_then(|delay| Instant::now().checked_add(delay));
                            source_attempt = source_attempt.saturating_add(1);
                            if error.retry_after.is_some() {
                                flood_until = source_retry_at;
                            }
                            publish(
                                &shared,
                                if error.retry_after.is_some() {
                                    ChannelSyncPhase::RateLimited
                                } else if source_retry_at.is_some() {
                                    ChannelSyncPhase::Waiting
                                } else {
                                    ChannelSyncPhase::Failed
                                },
                                None,
                                Some(error.kind),
                                source_retry_at,
                            );
                        }
                    }
                    last_check = Instant::now();
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
                                    }
                                    Err(error) if error.kind == ApplicationErrorKind::Cancelled => {
                                        completed.history = shared
                                            .history
                                            .lock()
                                            .is_ok_and(|history| history.pending(id));
                                        completed.failure = None;
                                        completed.retry_at = None;
                                    }
                                    Err(error) => {
                                        let phase = completed.fail(
                                            &error,
                                            Instant::now(),
                                            recovery_entropy(id, completed.retries),
                                        );
                                        if error.retry_after.is_some() {
                                            flood_until = completed.retry_at;
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
                            scheduler.enqueue(id);
                        }
                    }
                }
            }
        }
        if source_probe_started.is_some_and(|at| at.elapsed() >= Duration::from_secs(1)) {
            // An ordinary empty liveness response is silent; a slow request is visible once.
            source_probe = false;
            source_probe_started = None;
            publish(&shared, ChannelSyncPhase::Discovering, None, None, None);
        }
        while let Ok(command) = receiver.try_recv() {
            if matches!(command, Command::Refresh(0))
                || matches!(command, Command::Refresh(_)) && scheduler.source_failure.is_some()
            {
                source_refresh_pending = true;
                source_retry_at = Some(Instant::now());
                last_check = Instant::now() - Duration::from_secs(30);
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
                source_retry_at = Some(Instant::now());
                source_attempt = 0;
                for job in scheduler.jobs.values_mut() {
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
                    }
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
            let now = Instant::now();
            scheduler.transport_available(now, flood_until);
            if scheduler.source_failure == Some(ApplicationErrorKind::Network)
                && flood_until.is_none_or(|at| at <= now)
            {
                source_retry_at = Some(now);
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
            last_delivery = Instant::now();
        }
        for (id, pts) in hints.channels {
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
            && (source_retry_at.is_some_and(|at| at <= Instant::now())
                || (source_retry_at.is_none()
                    && scheduler.source_failure.is_none()
                    && (last_check.max(last_delivery).elapsed() >= UPDATE_SILENCE_RECOVERY
                        || (source_refresh_pending
                            && last_check.elapsed() >= Duration::from_secs(1)))))
            && flood_until.is_none_or(|at| at <= Instant::now())
        {
            source_probe = metadata_ready && !source_refresh_pending && source_retry_at.is_none();
            source_probe_started = source_probe.then(Instant::now);
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
                    source_retry_at = None;
                    last_check = Instant::now();
                }
                Err(error) => {
                    source_probe_started = None;
                    if let Ok(mut active) = shared.active.lock() {
                        active.remove(&0);
                    }
                    scheduler.source_failure = Some(error.kind());
                    source_retry_at = None;
                    publish(
                        &shared,
                        ChannelSyncPhase::Failed,
                        None,
                        Some(error.kind()),
                        None,
                    );
                }
            }
        }
        if let Ok(mut snapshot) = shared.snapshot.lock() {
            snapshot.queued = scheduler.queue.len();
            snapshot.failed_channels = scheduler
                .jobs
                .values()
                .filter(|job| job.failure.is_some() && job.paused)
                .count();
        }
        let now = Instant::now();
        if let Ok(mut journal) = shared.deltas.lock() {
            journal.retain_sources(&scheduler.jobs);
        }
        let next = if !metadata_ready
            || !executions.channel_slot_available()
            || flood_until.is_some_and(|at| at > now)
        {
            None
        } else {
            scheduler.pop(now)
        };
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
            let retry_at = flood_until.filter(|at| *at > now).or_else(|| {
                scheduler
                    .queue
                    .iter()
                    .filter(|id| !scheduler.in_flight.contains(id))
                    .filter_map(|id| scheduler.jobs.get(id)?.retry_at)
                    .filter(|at| *at > now)
                    .chain(source_retry_at)
                    .min()
            });
            let (failure_chat, failure) = scheduler
                .source_failure
                .map(|error| (None, Some(error)))
                .or_else(|| {
                    scheduler
                        .jobs
                        .iter()
                        .find_map(|(id, job)| job.failure.map(|error| (Some(*id), Some(error))))
                })
                .unwrap_or((None, None));
            if !executions.any() {
                publish(
                    &shared,
                    if flood_until.is_some_and(|at| at > now) {
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
            let metadata_deadline = source_retry_at.or_else(|| {
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
                thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
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
}

impl ChannelSource for DesktopTelegram {
    fn read(
        &self,
        account: i64,
        chat: i64,
        request: ChannelRead,
        cancellation: TelegramScanCancellation,
    ) -> Result<ChannelReadPage, ChannelSyncFailure> {
        self.sync_channel(account, chat, request, cancellation)
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
        telegram.read(account, chat, read.clone(), cancellation.clone())?
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
        scheduler.jobs.get_mut(&2).expect("job").retry_at = Some(now + Duration::from_secs(30));
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
