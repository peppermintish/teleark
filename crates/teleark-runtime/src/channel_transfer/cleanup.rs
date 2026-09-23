//! Independent durable cancellation controls and a retained filesystem cleanup worker.
use super::*;
use teleark_storage::{Database, NativeDownloadCleanup};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelDownloadCleanupPhase {
    WaitingForWriter,
    RemovingPartial,
    Failed(ApplicationErrorKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelDownloadCleanup {
    pub phase: ChannelDownloadCleanupPhase,
    pub requested_at_unix_ms: i64,
    pub phase_since_unix_ms: i64,
    pub last_activity_at_unix_ms: i64,
    pub retry_requested: bool,
}

pub(super) struct CleanupContext {
    pub completed_reservations: Vec<(u64, PathBuf, u64)>,
    pub backend: Arc<dyn ChannelDownloadBackend>,
    pub snapshots: Arc<TransferSnapshots<ChannelDownloadSnapshot>>,
    pub controls: Arc<Mutex<BTreeMap<u64, Arc<AtomicU8>>>>,
    pub scheduled: Arc<Mutex<BTreeSet<u64>>>,
    pub shutdown: Arc<AtomicBool>,
    pub library: DesktopLibrary,
    pub pump: Arc<QueuePump>,
}

type Reply<T> = mpsc::SyncSender<Result<T, ApplicationError>>;
enum Command {
    Cancel(u64, Reply<()>),
    Stop(Vec<u64>, Reply<()>),
    Retry(u64, Reply<bool>),
    Removing(u64),
    Finished(u64, u32, Result<(), ApplicationError>),
    WriterSettled,
}

#[derive(Default)]
pub(super) struct CleanupSignal {
    sender: Mutex<Option<mpsc::SyncSender<Command>>>,
    gate: Mutex<()>,
    changed: std::sync::Condvar,
}

impl CleanupSignal {
    fn attach(&self, sender: mpsc::SyncSender<Command>) {
        *self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
    }

    pub(super) fn notify(&self) {
        let _guard = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.changed.notify_all();
        drop(_guard);
        if let Some(sender) = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            let _ = sender.try_send(Command::WriterSettled);
        }
    }

    fn wait_for_writer(
        &self,
        scheduled: &Mutex<BTreeSet<u64>>,
        shutdown: &AtomicBool,
        id: u64,
    ) -> Result<(), ApplicationError> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut gate = self.gate.lock().map_err(persistence)?;
        while scheduled.lock().map_err(persistence)?.contains(&id) {
            if shutdown.load(Ordering::Acquire) {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(conflict());
            }
            gate = self
                .changed
                .wait_timeout(gate, remaining)
                .map_err(persistence)?
                .0;
        }
        Ok(())
    }
}
struct Work {
    id: u64,
    attempt: u32,
    destination: PathBuf,
}

pub(super) struct CleanupOwner {
    sender: mpsc::SyncSender<Command>,
    _control: JoinHandle<()>,
    _worker: JoinHandle<()>,
    _reservation_worker: Option<JoinHandle<()>>,
}

fn persistence(_: impl std::fmt::Debug) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Persistence)
}
fn conflict() -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Conflict)
}

#[cfg_attr(not(unix), allow(dead_code))]
fn cleanup_io(error: std::io::Error) -> ApplicationError {
    ApplicationError::new(match error.kind() {
        std::io::ErrorKind::PermissionDenied => ApplicationErrorKind::PermissionDenied,
        _ => ApplicationErrorKind::Persistence,
    })
}

pub(super) fn sync_cleanup_directory(destination: &Path) -> Result<(), ApplicationError> {
    #[cfg(unix)]
    {
        let mut parent = destination.parent().ok_or_else(conflict)?;
        loop {
            // A user may have removed the entire output directory while the app
            // was closed. Synchronize its nearest surviving ancestor instead of
            // leaving an already absent partial permanently awaiting cleanup.
            match std::fs::File::open(parent) {
                Ok(directory) => return directory.sync_all().map_err(cleanup_io),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    parent = parent.parent().ok_or_else(|| cleanup_io(error))?;
                }
                Err(error) => return Err(cleanup_io(error)),
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = destination;
        Ok(())
    }
}

impl CleanupOwner {
    pub fn start(mut context: CleanupContext) -> Result<Self, ApplicationError> {
        let (sender, receiver) = mpsc::sync_channel(CHANNEL_DOWNLOAD_QUEUE_CAPACITY);
        context.pump.cleanup_wake.attach(sender.clone());
        let (work_sender, work_receiver) = mpsc::sync_channel::<Work>(1);
        let completion = sender.clone();
        let backend = context.backend.clone();
        let scheduled = context.scheduled.clone();
        let signal = context.pump.clone();
        let shutdown = context.shutdown.clone();
        let completed_reservations = std::mem::take(&mut context.completed_reservations);
        let reservation_worker = if completed_reservations.is_empty() {
            None
        } else {
            let backend = backend.clone();
            let shutdown = shutdown.clone();
            Some(
                thread::Builder::new()
                    .name("teleark-download-reservations".into())
                    .spawn(move || {
                        // A slow legacy output volume must not stall startup, SQL,
                        // current downloads, or explicit cancellation cleanup.
                        for (id, destination, expected_bytes) in completed_reservations {
                            if shutdown.load(Ordering::Acquire) {
                                return;
                            }
                            release_completed_reservation(
                                backend.as_ref(),
                                id,
                                &destination,
                                expected_bytes,
                            );
                        }
                    })
                    .map_err(persistence)?,
            )
        };
        let worker = thread::Builder::new()
            .name("teleark-download-cleanup-files".into())
            .spawn(move || {
                while let Ok(work) = work_receiver.recv() {
                    let result = (|| {
                        signal
                            .cleanup_wake
                            .wait_for_writer(&scheduled, &shutdown, work.id)?;
                        if shutdown.load(Ordering::Acquire) {
                            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                        }
                        completion
                            .send(Command::Removing(work.id))
                            .map_err(persistence)?;
                        backend.discard_partial(&work.destination)?;
                        reservation::release_cancelled(&work.destination)?;
                        // Match the durable publication contract: deletion is not
                        // acknowledged before its directory entry is synchronized.
                        sync_cleanup_directory(&work.destination)?;
                        Ok(())
                    })();
                    if completion
                        .send(Command::Finished(work.id, work.attempt, result))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(persistence)?;
        let (ready, initialized) = mpsc::sync_channel(1);
        let control = thread::Builder::new()
            .name("teleark-download-cleanup-controls".into())
            .spawn(move || {
                let database =
                    Database::open(context.library.database_path.as_ref()).map_err(persistence);
                let mut owner = match database {
                    Ok(database) => Owner {
                        context,
                        database,
                        work_sender,
                        busy: false,
                        waiting_writers: false,
                        replies: BTreeMap::new(),
                    },
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if let Err(error) = owner.restore() {
                    let _ = ready.send(Err(error));
                    return;
                }
                let _ = ready.send(Ok(()));
                owner.dispatch();
                loop {
                    if owner.context.shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    let received = if owner.waiting_writers {
                        receiver.recv_timeout(Duration::from_secs(30))
                    } else {
                        receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    match received {
                        Ok(command) => {
                            owner.handle(command);
                            owner.dispatch();
                        }
                        // Reconsider waiting writers without placing one blocked
                        // writer ahead of every other task's filesystem cleanup.
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if owner.waiting_writers {
                                owner.dispatch();
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(persistence)?;
        initialized.recv().map_err(persistence)??;
        Ok(Self {
            sender,
            _control: control,
            _worker: worker,
            _reservation_worker: reservation_worker,
        })
    }

    pub fn cancel(&self, id: u64) -> Result<(), ApplicationError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .try_send(Command::Cancel(id, reply))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        response.recv().map_err(persistence)?
    }
    pub fn stop(&self, ids: Vec<u64>) -> Result<(), ApplicationError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .try_send(Command::Stop(ids, reply))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        response.recv().map_err(persistence)?
    }
    pub fn retry(&self, id: u64) -> Result<bool, ApplicationError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .try_send(Command::Retry(id, reply))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        response.recv().map_err(persistence)?
    }
}

struct Owner {
    context: CleanupContext,
    database: Database,
    work_sender: mpsc::SyncSender<Work>,
    busy: bool,
    waiting_writers: bool,
    replies: BTreeMap<u64, Reply<()>>,
}
impl Owner {
    fn stop(&mut self, ids: &[u64]) -> Result<(), ApplicationError> {
        let now = unix_time_millis()?;
        let records = {
            // This is the same short admission/retirement barrier used by workers.
            // Never hold it across persistence, transport or filesystem cleanup.
            let _admission = self.context.scheduled.lock().map_err(persistence)?;
            let account = self.context.pump.active_account.load(Ordering::Acquire);
            let mut rows = Vec::with_capacity(ids.len());
            for id in ids {
                let row = self.context.snapshots.get(*id).ok_or_else(conflict)?;
                if account <= 0 || row.account_id != Some(account) {
                    return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
                }
                if row.cleanup.is_some_and(|cleanup| {
                    cleanup.phase
                        == ChannelDownloadCleanupPhase::Failed(ApplicationErrorKind::InvalidRequest)
                }) {
                    return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
                }
                if row.state != ChannelDownloadState::Completed
                    && (row.state != ChannelDownloadState::Cancelled || row.cleanup.is_some())
                {
                    rows.push((row, self.control(*id)?));
                }
            }
            // Signal the whole selection before projecting or persisting any row.
            for (_, control) in &rows {
                control.store(CONTROL_CANCELLED, Ordering::Release);
            }
            let mut records = Vec::with_capacity(rows.len());
            for (mut row, _) in rows {
                row.state = ChannelDownloadState::Cancelled;
                row.verification = ChannelDownloadVerification::NotReached;
                row.finished_at_unix_ms = Some(now);
                let phase =
                    row.cleanup
                        .map_or(ChannelDownloadCleanupPhase::WaitingForWriter, |cleanup| {
                            if matches!(cleanup.phase, ChannelDownloadCleanupPhase::Failed(_)) {
                                ChannelDownloadCleanupPhase::WaitingForWriter
                            } else {
                                cleanup.phase
                            }
                        });
                self.project(
                    NativeDownloadCleanup {
                        task_id: row.id,
                        codec_version: 1,
                        attempt: row.attempts,
                        requested_at_unix_ms: row
                            .cleanup
                            .map_or(now, |cleanup| cleanup.requested_at_unix_ms),
                        retry_requested: false,
                    },
                    phase,
                );
                self.context.snapshots.update(row.id, |current| {
                    current.verification = row.verification;
                    current.finished_at_unix_ms = Some(now);
                    current.events.push(ChannelDownloadEvent {
                        kind: ChannelDownloadEventKind::Cancelled,
                        timestamp_unix_ms: now,
                        elapsed_ms: current.duration_ms,
                        failure_kind: None,
                    });
                    bound_lifecycle(current);
                });
                records.push(snapshot_record(&row)?);
            }
            records
        };
        if let Err(error) = self
            .database
            .stop_native_downloads(&records)
            .map_err(persistence)
        {
            // Keep the stop fence and make a failed durable write explicit. An
            // explicit Stop can retry it; never restart downloads as rollback.
            for row in &records {
                self.project(
                    NativeDownloadCleanup {
                        task_id: row.id,
                        codec_version: 1,
                        attempt: row.attempts,
                        requested_at_unix_ms: now,
                        retry_requested: false,
                    },
                    ChannelDownloadCleanupPhase::Failed(error.kind()),
                );
            }
            return Err(error);
        }
        Ok(())
    }

    fn restore(&mut self) -> Result<(), ApplicationError> {
        let mut cursor = 0;
        loop {
            let page = self
                .database
                .pending_native_download_cleanups(cursor, 128)
                .map_err(persistence)?;
            if page.is_empty() {
                break;
            }
            for saved in page {
                cursor = saved.task_id;
                let snapshot = self
                    .context
                    .snapshots
                    .get(saved.task_id)
                    .ok_or_else(conflict)?;
                let supported = saved.codec_version == 1 && saved.attempt == snapshot.attempts;
                self.project(
                    saved,
                    if supported {
                        ChannelDownloadCleanupPhase::WaitingForWriter
                    } else {
                        ChannelDownloadCleanupPhase::Failed(ApplicationErrorKind::InvalidRequest)
                    },
                );
                self.control(saved.task_id)?
                    .store(CONTROL_CANCELLED, Ordering::Release);
            }
        }
        Ok(())
    }
    fn control(&self, id: u64) -> Result<Arc<AtomicU8>, ApplicationError> {
        Ok(self
            .context
            .controls
            .lock()
            .map_err(persistence)?
            .entry(id)
            .or_insert_with(|| Arc::new(AtomicU8::new(CONTROL_CANCELLED)))
            .clone())
    }
    fn project(&self, saved: NativeDownloadCleanup, phase: ChannelDownloadCleanupPhase) {
        self.context.snapshots.update(saved.task_id, |row| {
            let now = unix_time_millis().unwrap_or(saved.requested_at_unix_ms);
            let previous = row.cleanup;
            let changed = previous.is_none_or(|cleanup| cleanup.phase != phase);
            if changed
                || previous.is_some_and(|cleanup| !cleanup.retry_requested && saved.retry_requested)
            {
                row.events.push(ChannelDownloadEvent {
                    kind: if !changed {
                        ChannelDownloadEventKind::CleanupRetryRequested
                    } else {
                        match phase {
                            ChannelDownloadCleanupPhase::WaitingForWriter => {
                                ChannelDownloadEventKind::CleanupWaiting
                            }
                            ChannelDownloadCleanupPhase::RemovingPartial => {
                                ChannelDownloadEventKind::CleanupRemoving
                            }
                            ChannelDownloadCleanupPhase::Failed(_) => {
                                ChannelDownloadEventKind::CleanupFailed
                            }
                        }
                    },
                    timestamp_unix_ms: now,
                    elapsed_ms: Some(now.saturating_sub(saved.requested_at_unix_ms).max(0) as u64),
                    failure_kind: if let ChannelDownloadCleanupPhase::Failed(kind) = phase {
                        Some(kind)
                    } else {
                        None
                    },
                });
                bound_lifecycle(row);
            }
            row.cleanup = Some(ChannelDownloadCleanup {
                phase_since_unix_ms: if changed {
                    now
                } else {
                    previous.map_or(now, |cleanup| cleanup.phase_since_unix_ms)
                },
                phase,
                requested_at_unix_ms: saved.requested_at_unix_ms,
                last_activity_at_unix_ms: unix_time_millis().unwrap_or(saved.requested_at_unix_ms),
                retry_requested: saved.retry_requested,
            });
            row.state = if saved.retry_requested
                && !matches!(phase, ChannelDownloadCleanupPhase::Failed(_))
            {
                ChannelDownloadState::Queued
            } else {
                ChannelDownloadState::Cancelled
            };
            row.current_bytes_per_second = None;
            row.eta_ms = None;
            if saved.retry_requested {
                row.transferred_bytes = 0;
            }
        });
    }
    fn begin(&mut self, id: u64) -> Result<(), ApplicationError> {
        if self.replies.len() >= CHANNEL_DOWNLOAD_QUEUE_CAPACITY {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        let mut snapshot = self.context.snapshots.get(id).ok_or_else(conflict)?;
        if snapshot.account_id != Some(self.context.pump.active_account.load(Ordering::Acquire)) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        if is_terminal(snapshot.state) || snapshot.cleanup.is_some() {
            return Err(conflict());
        }
        snapshot.state = ChannelDownloadState::Cancelled;
        snapshot.verification = ChannelDownloadVerification::NotReached;
        snapshot.finished_at_unix_ms = Some(unix_time_millis()?);
        let record = snapshot_record(&snapshot)?;
        let control = self.control(id)?;
        // Fence queue refill before admission, without retaining a lock across SQL.
        self.context
            .pump
            .cancel_cleanup
            .lock()
            .map_err(persistence)?
            .insert(id, false);
        let result = self
            .database
            .begin_native_download_cleanup(&record)
            .map_err(persistence);
        if !matches!(result, Ok(true)) {
            self.context
                .pump
                .cancel_cleanup
                .lock()
                .map_err(persistence)?
                .remove(&id);
            return result.and_then(|_| Err(conflict()));
        }
        // This owner serializes cleanup commands. No additional fallible read
        // may delay signalling a cancellation whose commit already succeeded.
        control.store(CONTROL_CANCELLED, Ordering::Release);
        let saved = NativeDownloadCleanup {
            task_id: id,
            codec_version: 1,
            attempt: record.attempts,
            requested_at_unix_ms: record.updated_at_unix_ms,
            retry_requested: false,
        };
        self.project(saved, ChannelDownloadCleanupPhase::WaitingForWriter);
        self.context.snapshots.update(id, |row| {
            row.verification = ChannelDownloadVerification::NotReached;
            row.finished_at_unix_ms = snapshot.finished_at_unix_ms;
            row.events.push(ChannelDownloadEvent {
                kind: ChannelDownloadEventKind::Cancelled,
                timestamp_unix_ms: saved.requested_at_unix_ms,
                elapsed_ms: row.duration_ms,
                failure_kind: None,
            });
            bound_lifecycle(row);
        });
        Ok(())
    }
    fn retry(&mut self, id: u64) -> Result<bool, ApplicationError> {
        let Some(saved) = self
            .database
            .native_download_cleanup(id)
            .map_err(persistence)?
        else {
            return Ok(false);
        };
        let snapshot = self.context.snapshots.get(id).ok_or_else(conflict)?;
        if snapshot.account_id != Some(self.context.pump.active_account.load(Ordering::Acquire)) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        if snapshot.attempts != saved.attempt
            || saved.codec_version != 1
            || !self
                .database
                .request_native_cleanup_retry(id, saved.attempt)
                .map_err(persistence)?
        {
            return Err(conflict());
        }
        let saved = self
            .database
            .native_download_cleanup(id)
            .map_err(persistence)?
            .ok_or_else(conflict)?;
        let phase = self
            .context
            .snapshots
            .read(id, |row| row.cleanup.map(|value| value.phase))
            .flatten()
            .filter(|phase| !matches!(phase, ChannelDownloadCleanupPhase::Failed(_)))
            .unwrap_or(ChannelDownloadCleanupPhase::WaitingForWriter);
        self.project(saved, phase);
        Ok(true)
    }
    fn handle(&mut self, command: Command) {
        match command {
            Command::WriterSettled => {}
            Command::Stop(ids, reply) => {
                let result = self.stop(&ids);
                // Acknowledge durable intent, not the last filesystem deletion.
                // Each task's cleanup projection owns that subsequent feedback.
                let _ = reply.send(result);
            }
            Command::Cancel(id, reply) => match self.begin(id) {
                Ok(()) => {
                    self.replies.insert(id, reply);
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
            },
            Command::Retry(id, reply) => {
                let result = self.retry(id);
                let _ = reply.send(result);
            }
            Command::Removing(id) => {
                if let Ok(Some(saved)) = self.database.native_download_cleanup(id) {
                    self.project(saved, ChannelDownloadCleanupPhase::RemovingPartial);
                }
            }
            Command::Finished(id, attempt, result) => {
                self.busy = false;
                let result = result.and_then(|()| self.finish(id, attempt));
                if let Err(error) = &result
                    && let Ok(Some(saved)) = self.database.native_download_cleanup(id)
                {
                    self.project(saved, ChannelDownloadCleanupPhase::Failed(error.kind()));
                }
                if let Ok(mut active) = self.context.pump.cancel_cleanup.lock() {
                    active.remove(&id);
                }
                if let Some(reply) = self.replies.remove(&id) {
                    let _ = reply.send(result);
                }
                let _ = self.context.pump.refill();
            }
        }
    }
    fn finish(&mut self, id: u64, attempt: u32) -> Result<(), ApplicationError> {
        let retry = self
            .database
            .finish_native_download_cleanup(id, attempt)
            .map_err(persistence)?
            .ok_or_else(conflict)?;
        self.control(id)?.store(
            if retry {
                CONTROL_RUNNING
            } else {
                CONTROL_CANCELLED
            },
            Ordering::Release,
        );
        self.context.snapshots.update(id, |row| {
            let now = unix_time_millis().unwrap_or(row.queued_at_unix_ms);
            row.events.push(ChannelDownloadEvent {
                kind: ChannelDownloadEventKind::CleanupFinished,
                timestamp_unix_ms: now,
                elapsed_ms: row
                    .cleanup
                    .map(|cleanup| now.saturating_sub(cleanup.requested_at_unix_ms).max(0) as u64),
                failure_kind: None,
            });
            bound_lifecycle(row);
            row.cleanup = None;
            row.state = if retry {
                ChannelDownloadState::Queued
            } else {
                ChannelDownloadState::Cancelled
            };
            row.transferred_bytes = 0;
            row.verification = if retry {
                ChannelDownloadVerification::Pending
            } else {
                ChannelDownloadVerification::NotReached
            };
            if retry {
                row.finished_at_unix_ms = None;
                row.failure = None;
                row.telemetry = empty_download_telemetry();
            }
        });
        Ok(())
    }
    fn dispatch(&mut self) {
        if self.busy {
            return;
        }
        let Ok(scheduled) = self.context.scheduled.lock() else {
            return;
        };
        self.waiting_writers = scheduled.iter().any(|id| {
            self.context
                .snapshots
                .read(*id, |row| {
                    row.cleanup.is_some_and(|cleanup| {
                        cleanup.phase == ChannelDownloadCleanupPhase::WaitingForWriter
                    })
                })
                .unwrap_or(false)
        });
        let Some(rows) = self.context.snapshots.select(
            |row| {
                !scheduled.contains(&row.id)
                    && row.cleanup.is_some_and(|cleanup| {
                        cleanup.phase == ChannelDownloadCleanupPhase::WaitingForWriter
                    })
            },
            1,
        ) else {
            return;
        };
        drop(scheduled);
        let Some(row) = rows.into_iter().next() else {
            return;
        };
        if let Ok(mut active) = self.context.pump.cancel_cleanup.lock() {
            active.insert(row.id, false);
        } else {
            return;
        }
        match self.work_sender.try_send(Work {
            id: row.id,
            attempt: row.attempts,
            destination: row.destination,
        }) {
            Ok(()) => self.busy = true,
            Err(_) => {
                if let Ok(Some(saved)) = self.database.native_download_cleanup(row.id) {
                    self.project(
                        saved,
                        ChannelDownloadCleanupPhase::Failed(ApplicationErrorKind::Capacity),
                    );
                }
                if let Ok(mut active) = self.context.pump.cancel_cleanup.lock() {
                    active.remove(&row.id);
                }
                if let Some(reply) = self.replies.remove(&row.id) {
                    let _ = reply.send(Err(ApplicationError::new(ApplicationErrorKind::Capacity)));
                }
            }
        }
    }
}

#[cfg(test)]
mod signal_tests {
    use super::*;

    #[test]
    fn writer_retirement_wakes_cleanup_without_a_short_poll() {
        let signal = Arc::new(CleanupSignal::default());
        let scheduled = Arc::new(Mutex::new(BTreeSet::from([7])));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (ready, started) = mpsc::sync_channel(1);
        let (finished, result) = mpsc::sync_channel(1);
        let waiting_signal = signal.clone();
        let waiting_scheduled = scheduled.clone();
        let waiting_shutdown = shutdown.clone();
        let worker = thread::spawn(move || {
            ready.send(()).expect("start");
            finished
                .send(waiting_signal.wait_for_writer(&waiting_scheduled, &waiting_shutdown, 7))
                .expect("result");
        });
        started.recv().expect("worker started");
        scheduled.lock().expect("scheduled").remove(&7);
        signal.notify();
        result
            .recv_timeout(Duration::from_secs(1))
            .expect("event wake")
            .expect("writer retired");
        worker.join().expect("worker");
    }
}
