//! Fresh local availability, independent of retained transfer history.
//! A retained controller, inventory reader and fixed probe pool isolate slow paths.
use crate::{
    DesktopLibrary, DownloadedFileRecord, DownloadedFilesCursor, LocalFilePresence,
    local_file_presence,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use teleark_core::{ApplicationError, ApplicationErrorKind};
use tokio::sync::watch;

const CAPACITY: usize = 10_000;
const PROBE_WORKERS: usize = 4;
const PROBE_QUEUE: usize = 32;
const MAX_AGE: Duration = Duration::from_secs(15);
const VISIBLE_INTERVAL: Duration = Duration::from_secs(3);
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(100);

type Scope = Option<(i64, u64)>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalDownloadObservation {
    pub file: DownloadedFileRecord,
    pub presence: LocalFilePresence,
}

/// Coalesced path changes. A reset replaces the previous account's whole view.
#[derive(Default)]
pub struct LocalDownloadUpdates {
    pub scope: Scope,
    pub reset: bool,
    /// Older destinations were omitted from the bounded observation index.
    pub limited: bool,
    pub changes: BTreeMap<PathBuf, Option<LocalDownloadObservation>>,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
struct Context {
    scope: Scope,
    chat: Option<i64>,
}

struct Shared {
    context: Mutex<Context>,
    shutdown: AtomicBool,
    epoch: AtomicU64,
    #[cfg(test)]
    inventory_reads: AtomicU64,
    updates: Mutex<LocalDownloadUpdates>,
    revision: watch::Sender<u64>,
}

/// Construction is background-only. Context changes and subscriptions never do I/O.
/// Dropping the owner signals shutdown without joining blocked filesystem calls.
pub struct LocalDownloadMonitor {
    shared: Arc<Shared>,
    _workers: Vec<JoinHandle<()>>,
}

impl LocalDownloadMonitor {
    pub fn new(library: DesktopLibrary) -> Result<Self, ApplicationError> {
        Self::with_probe(
            library,
            Arc::new(|file| local_file_presence(&file.destination, file.size_bytes)),
        )
    }

    fn with_probe(
        library: DesktopLibrary,
        probe: Arc<dyn Fn(&DownloadedFileRecord) -> LocalFilePresence + Send + Sync>,
    ) -> Result<Self, ApplicationError> {
        let (revision, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            context: Mutex::new(Context::default()),
            shutdown: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            #[cfg(test)]
            inventory_reads: AtomicU64::new(0),
            updates: Mutex::new(LocalDownloadUpdates::default()),
            revision,
        });
        let (results_tx, results_rx) = mpsc::sync_channel(PROBE_QUEUE + 1);
        let (probes_tx, probes_rx) = mpsc::sync_channel::<Probe>(PROBE_QUEUE);
        let probes_rx = Arc::new(Mutex::new(probes_rx));
        let mut monitor = Self {
            shared: shared.clone(),
            _workers: Vec::new(),
        };
        for index in 0..PROBE_WORKERS {
            let jobs = probes_rx.clone();
            let results = results_tx.clone();
            let shared = shared.clone();
            let probe = probe.clone();
            monitor._workers.push(
                thread::Builder::new()
                    .name(format!("teleark-local-probe-{index}"))
                    .spawn(move || {
                        loop {
                            let job = jobs.lock().unwrap_or_else(|e| e.into_inner()).recv();
                            let Ok(job) = job else { break };
                            if shared.shutdown.load(Ordering::Acquire) {
                                break;
                            }
                            if job.epoch != shared.epoch.load(Ordering::Acquire) {
                                continue;
                            }
                            let started = Instant::now();
                            let presence = probe(&job.file);
                            if results
                                .send(ResultEvent::Probe(job, started, presence))
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .map_err(worker_error)?,
            );
        }
        let (inventory_tx, inventory_rx) = mpsc::sync_channel::<Inventory>(1);
        let reader = library.clone();
        let inventory_shared = shared.clone();
        monitor._workers.push(
            thread::Builder::new()
                .name("teleark-local-inventory".into())
                .spawn(move || {
                    while let Ok(job) = inventory_rx.recv() {
                        if inventory_shared.shutdown.load(Ordering::Acquire) {
                            break;
                        }
                        #[cfg(test)]
                        inventory_shared
                            .inventory_reads
                            .fetch_add(1, Ordering::Release);
                        let result = reader.downloaded_files_page(job.account, job.cursor);
                        if results_tx
                            .send(ResultEvent::Inventory(job, result))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .map_err(worker_error)?,
        );
        monitor._workers.push(
            thread::Builder::new()
                .name("teleark-local-observations".into())
                .spawn(move || controller(library, shared, inventory_tx, probes_tx, results_rx))
                .map_err(worker_error)?,
        );
        Ok(monitor)
    }

    pub fn set_context(&self, scope: Scope, chat: Option<i64>) {
        *self
            .shared
            .context
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Context {
            scope: scope.filter(|(account, _)| *account > 0),
            chat,
        };
    }

    pub fn subscribe(&self) -> LocalDownloadSubscription {
        LocalDownloadSubscription {
            receiver: self.shared.revision.subscribe(),
            shared: self.shared.clone(),
        }
    }

    pub fn take_updates(&self) -> LocalDownloadUpdates {
        let mut updates = self
            .shared
            .updates
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        LocalDownloadUpdates {
            scope: updates.scope,
            reset: std::mem::take(&mut updates.reset),
            limited: updates.limited,
            changes: std::mem::take(&mut updates.changes),
        }
    }
}

pub struct LocalDownloadSubscription {
    receiver: watch::Receiver<u64>,
    shared: Arc<Shared>,
}
impl LocalDownloadSubscription {
    pub async fn changed(&mut self) -> bool {
        !self.shared.shutdown.load(Ordering::Acquire)
            && self.receiver.changed().await.is_ok()
            && !self.shared.shutdown.load(Ordering::Acquire)
    }

    pub fn take_updates(&self) -> LocalDownloadUpdates {
        take_updates(&self.shared)
    }
}

impl Drop for LocalDownloadMonitor {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared
            .revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

fn take_updates(shared: &Shared) -> LocalDownloadUpdates {
    let mut updates = shared.updates.lock().unwrap_or_else(|e| e.into_inner());
    LocalDownloadUpdates {
        scope: updates.scope,
        reset: std::mem::take(&mut updates.reset),
        limited: updates.limited,
        changes: std::mem::take(&mut updates.changes),
    }
}

fn worker_error(_: std::io::Error) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Persistence)
}

struct Probe {
    epoch: u64,
    token: u64,
    file: DownloadedFileRecord,
}
struct Inventory {
    epoch: u64,
    account: i64,
    cursor: Option<DownloadedFilesCursor>,
}
enum ResultEvent {
    Probe(Probe, Instant, LocalFilePresence),
    Inventory(
        Inventory,
        Result<Vec<DownloadedFileRecord>, ApplicationError>,
    ),
}

struct Entry {
    observation: LocalDownloadObservation,
    due: Option<Instant>,
    expires: Option<Instant>,
    token: Option<u64>,
}

#[derive(Default)]
struct Observations {
    entries: BTreeMap<PathBuf, Entry>,
    ages: BTreeSet<(i64, PathBuf)>,
    due: BTreeSet<(Instant, PathBuf)>,
    expiries: BTreeSet<(Instant, PathBuf)>,
    token: u64,
    changes: BTreeMap<PathBuf, Option<LocalDownloadObservation>>,
    limited: bool,
    limit_changed: bool,
}

impl Observations {
    fn insert(&mut self, file: DownloadedFileRecord, now: Instant) {
        let path = file.destination.clone();
        if let Some(entry) = self.entries.get(&path) {
            // Old pages must not reassign a reused path to an older source.
            if entry.observation.file == file
                || entry.observation.file.completed_at_unix_ms > file.completed_at_unix_ms
            {
                return;
            }
        } else if self.entries.len() == CAPACITY
            && self
                .ages
                .first()
                .is_some_and(|oldest| (file.completed_at_unix_ms, &path) <= (oldest.0, &oldest.1))
        {
            self.limit_changed |= !self.limited;
            self.limited = true;
            return;
        }
        self.remove(&path);
        let observation = LocalDownloadObservation {
            file,
            presence: LocalFilePresence::Checking,
        };
        self.ages
            .insert((observation.file.completed_at_unix_ms, path.clone()));
        self.due.insert((now, path.clone()));
        self.changes.insert(path.clone(), Some(observation.clone()));
        self.entries.insert(
            path,
            Entry {
                observation,
                due: Some(now),
                expires: None,
                token: None,
            },
        );
        while self.entries.len() > CAPACITY {
            self.limit_changed |= !self.limited;
            self.limited = true;
            if let Some((_, oldest)) = self.ages.first().cloned() {
                self.remove(&oldest);
            }
        }
    }

    fn remove(&mut self, path: &PathBuf) {
        if let Some(entry) = self.entries.remove(path) {
            self.ages
                .remove(&(entry.observation.file.completed_at_unix_ms, path.clone()));
            if let Some(due) = entry.due {
                self.due.remove(&(due, path.clone()));
            }
            if let Some(expires) = entry.expires {
                self.expiries.remove(&(expires, path.clone()));
            }
            self.changes.insert(path.clone(), None);
        }
    }

    fn prioritize(&mut self, chat: Option<i64>, now: Instant) {
        for (path, entry) in &mut self.entries {
            if Some(entry.observation.file.chat_id) == chat && entry.token.is_none() {
                if let Some(due) = entry.due {
                    self.due.remove(&(due, path.clone()));
                }
                entry.due = Some(now);
                self.due.insert((now, path.clone()));
            }
        }
    }

    fn expire(&mut self, now: Instant) {
        while self.expiries.first().is_some_and(|(at, _)| *at <= now) {
            let (_, path) = self.expiries.pop_first().expect("expiration");
            if let Some(entry) = self.entries.get_mut(&path) {
                entry.expires = None;
                if entry.observation.presence != LocalFilePresence::Checking {
                    entry.observation.presence = LocalFilePresence::Checking;
                    self.changes.insert(path, Some(entry.observation.clone()));
                }
            }
        }
    }

    fn dispatch(&mut self, epoch: u64, now: Instant, sender: &mpsc::SyncSender<Probe>) {
        // Bound work per turn even if all 10,000 retained paths become due together.
        for _ in 0..PROBE_QUEUE {
            if !self.due.first().is_some_and(|(at, _)| *at <= now) {
                break;
            }
            let (at, path) = self.due.pop_first().expect("due path");
            let Some(entry) = self.entries.get_mut(&path) else {
                continue;
            };
            self.token = self.token.wrapping_add(1);
            let token = self.token;
            match sender.try_send(Probe {
                epoch,
                token,
                file: entry.observation.file.clone(),
            }) {
                Ok(()) => {
                    entry.due = None;
                    entry.token = Some(token);
                }
                Err(_) => {
                    self.due.insert((at, path));
                    break;
                }
            }
        }
    }

    fn complete(
        &mut self,
        job: Probe,
        started: Instant,
        presence: LocalFilePresence,
        chat: Option<i64>,
        now: Instant,
    ) {
        let path = job.file.destination;
        let Some(entry) = self.entries.get_mut(&path) else {
            return;
        };
        if entry.token != Some(job.token) {
            return;
        }
        entry.token = None;
        let presence = if now.saturating_duration_since(started) >= MAX_AGE {
            LocalFilePresence::Unavailable
        } else {
            presence
        };
        let due = now
            + if Some(entry.observation.file.chat_id) == chat {
                VISIBLE_INTERVAL
            } else {
                BACKGROUND_INTERVAL
            };
        entry.due = Some(due);
        self.due.insert((due, path.clone()));
        if let Some(expires) = entry.expires {
            self.expiries.remove(&(expires, path.clone()));
        }
        let expires = now + MAX_AGE;
        entry.expires = Some(expires);
        self.expiries.insert((expires, path.clone()));
        if entry.observation.presence != presence {
            entry.observation.presence = presence;
            self.changes.insert(path, Some(entry.observation.clone()));
        }
    }

    fn publish(&mut self, shared: &Shared, scope: Scope, reset: bool) {
        if self.changes.is_empty() && !reset && !self.limit_changed {
            return;
        }
        let mut updates = shared.updates.lock().unwrap_or_else(|e| e.into_inner());
        if reset {
            updates.changes.clear();
            updates.reset = true;
        }
        updates.scope = scope;
        updates.limited = self.limited;
        self.limit_changed = false;
        updates.changes.append(&mut self.changes);
        if updates.changes.len() > CAPACITY * 2 {
            // A stalled frontend gets a bounded replacement, never an incomplete delta.
            updates.reset = true;
            updates.changes = self
                .entries
                .iter()
                .map(|(path, entry)| (path.clone(), Some(entry.observation.clone())))
                .collect();
        }
        drop(updates);
        shared
            .revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

fn controller(
    library: DesktopLibrary,
    shared: Arc<Shared>,
    inventory: mpsc::SyncSender<Inventory>,
    probes: mpsc::SyncSender<Probe>,
    results: mpsc::Receiver<ResultEvent>,
) {
    let mut context = Context::default();
    let mut epoch = 0_u64;
    let mut observations = Observations::default();
    let mut inventory_busy = false;
    let mut cursor = None;
    let mut discovering = false;
    let mut seen_revision = None;
    let mut inventory_retry = Instant::now();
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        let now = Instant::now();
        let requested = *shared.context.lock().unwrap_or_else(|e| e.into_inner());
        if requested.scope != context.scope {
            epoch = epoch.wrapping_add(1);
            shared.epoch.store(epoch, Ordering::Release);
            observations = Observations::default();
            cursor = None;
            discovering = false;
            seen_revision = None;
            inventory_retry = now;
            observations.publish(&shared, requested.scope, true);
        }
        if requested.chat != context.chat {
            observations.prioritize(requested.chat, now);
        }
        context = requested;
        let revision = library.downloaded_files_revision.load(Ordering::Acquire);
        if !discovering && seen_revision != Some(revision) && now >= inventory_retry {
            cursor = None;
            discovering = context.scope.is_some();
            seen_revision = Some(revision);
        }
        if let Some((account, _)) = context.scope
            && discovering
            && !inventory_busy
            && inventory
                .try_send(Inventory {
                    epoch,
                    account,
                    cursor,
                })
                .is_ok()
        {
            inventory_busy = true;
        }
        observations.expire(now);
        // Publish Checking before admitting filesystem work.
        observations.publish(&shared, context.scope, false);
        observations.dispatch(epoch, now, &probes);
        match results.recv_timeout(TICK) {
            Ok(ResultEvent::Probe(job, started, presence)) => {
                if job.epoch == epoch {
                    observations.complete(job, started, presence, context.chat, Instant::now());
                }
            }
            Ok(ResultEvent::Inventory(job, result)) => {
                inventory_busy = false;
                if job.epoch != epoch {
                    continue;
                }
                match result {
                    Ok(files) => {
                        discovering = files.len() == 128;
                        cursor = files.last().map(|file| file.cursor);
                        for file in files {
                            observations.insert(file, Instant::now());
                        }
                    }
                    Err(_) => {
                        discovering = false;
                        seen_revision = None;
                        inventory_retry = Instant::now() + VISIBLE_INTERVAL;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
#[path = "local_downloads/tests.rs"]
mod tests;
