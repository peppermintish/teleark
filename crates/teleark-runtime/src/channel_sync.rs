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

mod feed;
mod manifest_activity;
pub use feed::{ChannelChanges, ChannelDelta, ChannelSyncSubscription};
pub use manifest_activity::{ManagedScanObserver, ManagedScanStatus};

const MAX_SOURCES: usize = 10_000;
const EVENT_CAPACITY: usize = 128;
const COMMAND_CAPACITY: usize = 64;
const IDLE_RECONCILE: Duration = Duration::from_secs(15 * 60);

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
    pub retry_at: Option<Instant>,
    pub failure: Option<ApplicationErrorKind>,
    pub queued: usize,
    pub failed_channels: usize,
    pub committed_pages: u64,
    pub data_revision: u64,
    pub events: VecDeque<ChannelSyncEvent>,
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
            retry_at: None,
            failure: None,
            queued,
            failed_channels: 0,
            committed_pages: 0,
            data_revision: 0,
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
    managed_id: AtomicI64,
    observation: AtomicU64,
    manifest_generation: AtomicU64,
    snapshot: Mutex<ChannelSyncSnapshot>,
    stop: AtomicBool,
    active: Mutex<Option<(i64, TelegramScanCancellation)>>,
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
    Observe(Option<i64>),
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
        if chats.len() > MAX_SOURCES {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        let scheduler = Scheduler::new(chats);
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(ChannelSyncSnapshot::new(account.id, scheduler.queue.len())),
            changes: tokio::sync::watch::channel(()).0,
            deltas: Mutex::new(feed::DeltaJournal::default()),
            managed_id: AtomicI64::new(0),
            observation: AtomicU64::new(0),
            manifest_generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            active: Mutex::new(None),
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
    /// Maintain the protocol's short-lived subscription for the visible channel.
    pub fn observe(&self, chat: Option<i64>) -> Result<(), ApplicationError> {
        self.command(Command::Observe(chat))
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
    pub fn request_history(&self, chat: i64) -> Result<(), ApplicationError> {
        self.command(Command::History(chat))
    }
    pub fn refresh(&self, chat: i64) -> Result<(), ApplicationError> {
        self.command(Command::Refresh(chat))
    }
    pub fn cancel(&self, chat: i64) -> Result<(), ApplicationError> {
        if let Ok(active) = self.inner.shared.active.lock()
            && let Some((id, token)) = &*active
            && (*id == chat || *id == 0)
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
    if let Ok(active) = shared.active.lock()
        && let Some((_, token)) = &*active
    {
        token.cancel();
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
    pub timeout_seconds: Option<u32>,
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

struct Job {
    pushes: VecDeque<teleark_telegram::ChannelPush>,
    followed: bool,
    next_check: Option<Instant>,
    catalog_ready: Option<bool>,
    watch_loaded: bool,
    head: Option<i32>,
    state: Option<ChannelSyncState>,
    force: bool,
    history: bool,
    paused: bool,
    failure: Option<ApplicationErrorKind>,
    retries: u32,
    retry_at: Option<Instant>,
}

struct Scheduler {
    jobs: BTreeMap<i64, Job>,
    queue: VecDeque<i64>,
    preferred: Option<i64>,
    observed: Option<i64>,
    preferred_turns: u8,
    source_failure: Option<ApplicationErrorKind>,
    overflow_count: u64,
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
                        followed: false,
                        next_check: None,
                        catalog_ready: None,
                        watch_loaded: false,
                        head: c.sync_pts,
                        state: None,
                        force: false,
                        history: false,
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
            observed: None,
            preferred_turns: 0,
            source_failure: None,
            overflow_count: 0,
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
                        followed: false,
                        next_check: None,
                        catalog_ready: None,
                        watch_loaded: false,
                        head: chat.sync_pts,
                        state: None,
                        force: false,
                        history: false,
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
            Command::Observe(chat) => {
                self.observed = chat;
                for (id, job) in &mut self.jobs {
                    let followed = Some(*id) == chat || job.catalog_ready.is_some();
                    if followed && !job.followed {
                        job.next_check = Some(Instant::now());
                    }
                    if !followed {
                        job.next_check = None;
                    }
                    job.followed = followed;
                }
                return;
            }
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
            | Command::Observe(_)
            | Command::Watch(_)
            | Command::Acknowledge(_, _) => {}
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
    fn due_subscriptions(&mut self, now: Instant) {
        let due: Vec<_> = self
            .jobs
            .iter_mut()
            .filter_map(|(id, job)| {
                if job.followed && !job.paused && job.next_check.is_some_and(|at| at <= now) {
                    job.force = true;
                    job.next_check = None;
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        for id in due {
            self.enqueue(id);
        }
    }
    fn pop(&mut self, now: Instant) -> Option<i64> {
        let ready = |id: &i64| {
            self.jobs
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
                    job.followed = Some(*id) == scheduler.observed;
                    if !job.followed {
                        job.next_check = None;
                    }
                }
            }
            let job = scheduler.jobs.entry(chat).or_insert_with(|| Job {
                head: None,
                state: None,
                force: true,
                history: false,
                paused: false,
                failure: None,
                retries: 0,
                retry_at: None,
                pushes: VecDeque::new(),
                followed: true,
                next_check: None,
                catalog_ready: Some(false),
                watch_loaded: false,
            });
            // Registration belongs to the retryable job, so a temporary disk
            // failure cannot silently disable protection or advance its PTS.
            job.catalog_ready = Some(false);
            job.watch_loaded = false;
            job.followed = true;
            job.paused = false;
            job.failure = None;
            job.next_check = Some(Instant::now());
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
                    publish(shared, ChannelSyncPhase::Idle, Some(chat), None, None);
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

fn publish(
    shared: &Shared,
    phase: ChannelSyncPhase,
    chat: Option<i64>,
    failure: Option<ApplicationErrorKind>,
    retry_at: Option<Instant>,
) {
    if let Ok(mut snapshot) = shared.snapshot.lock()
        && snapshot.transition(phase, chat, failure, retry_at)
    {
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

fn run(
    telegram: DesktopTelegram,
    library: DesktopLibrary,
    account: TelegramAccount,
    mut scheduler: Scheduler,
    receiver: mpsc::Receiver<Command>,
    shared: Arc<Shared>,
) {
    publish(&shared, ChannelSyncPhase::ReadingLocal, None, None, None);
    let signals = match telegram.channel_signals() {
        Ok(signals) => signals,
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
    signals.set_waker(Some(transport_waker(&shared, thread::current())));
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
    let mut flood_until = None;
    let mut source_refresh_pending = false;
    while !shared.stop.load(Ordering::Acquire) {
        while let Ok(command) = receiver.try_recv() {
            if matches!(command, Command::Refresh(_)) && scheduler.source_failure.is_some() {
                source_refresh_pending = true;
                last_check = Instant::now() - Duration::from_secs(30);
            }
            handle_command(command, &library, account.id, &mut scheduler, &shared);
        }
        let hints = signals.take();
        if let Ok(mut snapshot) = shared.snapshot.lock() {
            snapshot.overflow_signals = snapshot
                .overflow_signals
                .saturating_add(hints.overflow_count);
        }
        for push in hints.pushes {
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
        for (id, pts) in hints.channels {
            source_refresh_pending |= !scheduler.jobs.contains_key(&id);
            scheduler.hint(id, pts);
        }
        if hints.reconcile_all {
            // A broken transport/overflow may have lost which channel changed.
            // Keep local rows and recover from each committed PTS in the background.
            for id in scheduler.jobs.keys().copied().collect::<Vec<_>>() {
                scheduler.hint(id, 0);
            }
        }
        if (last_check.elapsed() >= IDLE_RECONCILE
            || (source_refresh_pending && last_check.elapsed() >= Duration::from_secs(30)))
            && flood_until.is_none_or(|at| at <= Instant::now())
        {
            // Refresh metadata after a passive subscription went quiet or a
            // previously unknown source pushed an update; never scan history.
            publish(&shared, ChannelSyncPhase::Receiving, None, None, None);
            let cancellation = TelegramScanCancellation::new();
            if let Ok(mut active) = shared.active.lock() {
                *active = Some((0, cancellation.clone()));
            }
            match telegram.sync_sources(account.id, cancellation) {
                Ok(chats) => {
                    publish(&shared, ChannelSyncPhase::Persisting, None, None, None);
                    match library.save_telegram_sources(&account, &chats) {
                        Ok(()) => {
                            scheduler.update_sources(chats);
                            source_refresh_pending = false;
                        }
                        Err(error) => {
                            scheduler.source_failure = Some(error.kind());
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
                Err(error) => {
                    scheduler.source_failure = Some(error.kind);
                    if let Some(delay) = error.retry_after {
                        flood_until = Instant::now().checked_add(delay);
                    }
                    publish(
                        &shared,
                        if flood_until.is_some() {
                            ChannelSyncPhase::RateLimited
                        } else {
                            ChannelSyncPhase::Failed
                        },
                        None,
                        Some(error.kind),
                        flood_until,
                    );
                }
            }
            if let Ok(mut active) = shared.active.lock() {
                *active = None;
            }
            last_check = Instant::now();
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
        scheduler.due_subscriptions(now);
        if let Ok(mut journal) = shared.deltas.lock() {
            journal.retain_sources(&scheduler.jobs);
        }
        let next = if flood_until.is_some_and(|at| at > now) {
            None
        } else {
            scheduler.pop(now)
        };
        if let Some(id) = next {
            if let Ok(mut snapshot) = shared.snapshot.lock() {
                snapshot.queued = scheduler.queue.len();
            }
            let token = TelegramScanCancellation::new();
            if let Ok(mut active) = shared.active.lock() {
                *active = Some((id, token.clone()));
            }
            let result = match scheduler.jobs.get_mut(&id) {
                Some(job) => step(&telegram, &library, account.id, id, job, &shared, token),
                None => continue,
            };
            if let Ok(mut active) = shared.active.lock() {
                *active = None;
            }
            if let Some(job) = scheduler.jobs.get_mut(&id) {
                match result {
                    Ok(()) => {
                        job.retries = 0;
                        job.failure = None;
                        job.retry_at = None;
                    }
                    Err(error) => {
                        job.failure = Some(error.kind);
                        if error.kind == ApplicationErrorKind::Conflict {
                            // A retry must reload the winning committed state,
                            // not repeatedly submit the same stale CAS revision.
                            job.state = None;
                        }
                        if let Some(delay) = error.retry_after {
                            let deadline = Instant::now()
                                .checked_add(delay)
                                .unwrap_or_else(Instant::now);
                            flood_until = Some(deadline);
                            job.retry_at = Some(deadline);
                            publish(
                                &shared,
                                ChannelSyncPhase::RateLimited,
                                Some(id),
                                Some(error.kind),
                                job.retry_at,
                            );
                        } else if matches!(
                            error.kind,
                            ApplicationErrorKind::Network
                                | ApplicationErrorKind::Server
                                | ApplicationErrorKind::Persistence
                        ) && job.retries < 3
                        {
                            job.retry_at =
                                Some(Instant::now() + Duration::from_secs(1 << job.retries));
                            job.retries += 1;
                            publish(
                                &shared,
                                ChannelSyncPhase::Waiting,
                                Some(id),
                                Some(error.kind),
                                job.retry_at,
                            );
                        } else {
                            job.paused = true;
                            publish(
                                &shared,
                                if error.kind == ApplicationErrorKind::Cancelled {
                                    ChannelSyncPhase::Cancelled
                                } else {
                                    ChannelSyncPhase::Failed
                                },
                                Some(id),
                                Some(error.kind),
                                None,
                            );
                        }
                    }
                }
                if !job.paused && needed(job) {
                    scheduler.enqueue(id);
                }
            }
        } else {
            let retry_at = flood_until.filter(|at| *at > now).or_else(|| {
                scheduler
                    .queue
                    .iter()
                    .filter_map(|id| scheduler.jobs.get(id)?.retry_at)
                    .min()
            });
            let failure = scheduler
                .source_failure
                .or_else(|| scheduler.jobs.values().find_map(|job| job.failure));
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
                None,
                failure,
                retry_at,
            );
            let metadata_deadline = last_check
                + if source_refresh_pending {
                    Duration::from_secs(30)
                } else {
                    IDLE_RECONCILE
                };
            let mut deadline = retry_at.unwrap_or(metadata_deadline).min(metadata_deadline);
            if let Some(subscription) = scheduler
                .jobs
                .values()
                .filter(|job| !job.paused)
                .filter_map(|job| job.next_check)
                .min()
            {
                deadline = deadline.min(subscription);
            }
            if let Some(flood) = flood_until.filter(|at| *at > now) {
                deadline = deadline.max(flood);
            }
            // Commands, transport pushes and cancellation unpark this owner. A timer
            // exists only for a real retry, protocol deadline or metadata fallback.
            thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
        }
    }
    publish(&shared, ChannelSyncPhase::Cancelled, None, None, None);
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
            timeout_seconds: None,
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
            timeout_seconds: None,
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
            if matches!(read, ChannelRead::Difference(_)) && page.complete && job.followed {
                job.next_check = Some(
                    Instant::now()
                        + Duration::from_secs(u64::from(page.timeout_seconds.unwrap_or(1).max(1))),
                );
            }
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
        publish(shared, ChannelSyncPhase::Idle, Some(chat), None, None);
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
    }
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        snapshot.data_revision = snapshot.data_revision.saturating_add(1);
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
    publish(shared, ChannelSyncPhase::Idle, Some(chat), None, None);
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
            managed_id: AtomicI64::new(0),
            observation: AtomicU64::new(0),
            manifest_generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            active: Mutex::new(None),
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
            timeout_seconds: None,
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
