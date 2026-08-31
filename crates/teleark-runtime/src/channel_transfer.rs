use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use teleark_core::{ApplicationError, ApplicationErrorKind};
use teleark_storage::{
    NativeDownloadTaskRecord, NewNativeDownloadBatchRecord, NewNativeDownloadTaskRecord,
    StoredNativeDownloadState, StoredNativeDownloadVerification,
};
use teleark_telegram::{DownloadControl, DownloadObserver};

use crate::{DesktopLibrary, DesktopTelegram};

const CHANNEL_DOWNLOAD_QUEUE_CAPACITY: usize = 32;
const CHANNEL_DOWNLOAD_BATCH_CAPACITY: usize = 5_000;
const PROGRESS_PERSIST_INTERVAL: Duration = Duration::from_millis(750);
const CONTROL_RUNNING: u8 = 0;
const CONTROL_PAUSED: u8 = 1;
const CONTROL_CANCELLED: u8 = 2;
const CONTROL_RESUME_PENDING: u8 = 3;

pub fn available_download_destination(
    directory: &Path,
    suggested_file_name: &str,
) -> Result<PathBuf, ApplicationError> {
    let suggested = Path::new(suggested_file_name);
    if suggested_file_name.trim().is_empty()
        || suggested.file_name().and_then(|name| name.to_str()) != Some(suggested_file_name)
    {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    let stem = suggested
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(suggested_file_name);
    let extension = suggested
        .extension()
        .and_then(|extension| extension.to_str());
    for suffix in 0_u32..10_000 {
        let file_name = if suffix == 0 {
            suggested_file_name.to_owned()
        } else if let Some(extension) = extension {
            format!("{stem} ({suffix}).{extension}")
        } else {
            format!("{stem} ({suffix})")
        };
        let candidate = directory.join(file_name);
        match candidate.try_exists() {
            Ok(false) => return Ok(candidate),
            Ok(true) => {}
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(ApplicationError::new(
                    ApplicationErrorKind::PermissionDenied,
                ));
            }
            Err(_) => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
        }
    }
    Err(ApplicationError::new(ApplicationErrorKind::Capacity))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelDownloadRequest {
    pub chat_id: i64,
    pub message_id: i64,
    pub message_sent_at_unix_ms: Option<i64>,
    pub file_name: String,
    pub caption: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub destination: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelDownloadState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed(ApplicationErrorKind),
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelDownloadVerification {
    Pending,
    SizeChecked,
    NotReached,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelDownloadFailureStage {
    Transfer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelDownloadFailure {
    pub kind: ApplicationErrorKind,
    pub stage: ChannelDownloadFailureStage,
    pub retryable: bool,
    pub requires_user_action: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelDownloadEventKind {
    Queued,
    Started,
    Paused,
    Resumed,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelDownloadEvent {
    pub kind: ChannelDownloadEventKind,
    pub timestamp_unix_ms: i64,
    pub elapsed_ms: Option<u64>,
    pub failure_kind: Option<ApplicationErrorKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelDownloadSnapshot {
    pub id: u64,
    pub batch_id: Option<u64>,
    pub chat_id: i64,
    pub message_id: i64,
    pub message_sent_at_unix_ms: Option<i64>,
    pub file_name: String,
    pub caption: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub transferred_bytes: u64,
    pub destination: PathBuf,
    pub state: ChannelDownloadState,
    pub verification: ChannelDownloadVerification,
    pub queued_at_unix_ms: i64,
    pub started_at_unix_ms: Option<i64>,
    pub finished_at_unix_ms: Option<i64>,
    pub queue_wait_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub current_bytes_per_second: Option<u64>,
    pub average_bytes_per_second: Option<u64>,
    pub eta_ms: Option<u64>,
    pub attempts: u32,
    pub failure: Option<ChannelDownloadFailure>,
    pub events: Vec<ChannelDownloadEvent>,
}

#[derive(Clone)]
pub struct DesktopTransfers {
    inner: Arc<TransferWorkerInner>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TransferRates {
    pub download_bytes_per_second: u64,
    pub upload_bytes_per_second: u64,
}

struct TransferWorkerInner {
    sender: Mutex<Option<mpsc::SyncSender<TransferCommand>>>,
    snapshots: Arc<Mutex<Vec<ChannelDownloadSnapshot>>>,
    controls: Arc<Mutex<BTreeMap<u64, Arc<AtomicU8>>>>,
    scheduled: Arc<Mutex<BTreeSet<u64>>>,
    shutdown: Arc<AtomicBool>,
    library: DesktopLibrary,
    backend: Arc<dyn ChannelDownloadBackend>,
    join: Mutex<Option<JoinHandle<()>>>,
}

enum TransferCommand {
    Download {
        snapshot: ChannelDownloadSnapshot,
        queued_at: Instant,
        control: Arc<AtomicU8>,
    },
}

trait ChannelDownloadBackend: Send + Sync + 'static {
    fn download(
        &self,
        chat_id: i64,
        message_id: i64,
        destination: &Path,
        observer: Arc<dyn DownloadObserver>,
    ) -> Result<(), ApplicationError>;

    fn discard_partial(&self, _destination: &Path) -> Result<(), ApplicationError> {
        Ok(())
    }
}

impl ChannelDownloadBackend for DesktopTelegram {
    fn download(
        &self,
        chat_id: i64,
        message_id: i64,
        destination: &Path,
        observer: Arc<dyn DownloadObserver>,
    ) -> Result<(), ApplicationError> {
        self.download_file_observed(chat_id, message_id, destination, observer)
    }

    fn discard_partial(&self, destination: &Path) -> Result<(), ApplicationError> {
        self.discard_partial_download(destination)
    }
}

struct ProgressSample {
    transferred_bytes: u64,
    measured_at: Instant,
}

struct RuntimeDownloadObserver {
    id: u64,
    size_bytes: u64,
    control: Arc<AtomicU8>,
    shutdown: Arc<AtomicBool>,
    snapshots: Arc<Mutex<Vec<ChannelDownloadSnapshot>>>,
    library: DesktopLibrary,
    started: Instant,
    previous_duration_ms: u64,
    sample: Mutex<ProgressSample>,
    last_persisted: Mutex<Instant>,
}

impl DownloadObserver for RuntimeDownloadObserver {
    fn control(&self) -> DownloadControl {
        if self.shutdown.load(Ordering::Acquire) {
            return DownloadControl::Stop;
        }
        match self.control.load(Ordering::Acquire) {
            CONTROL_PAUSED | CONTROL_RESUME_PENDING => DownloadControl::Pause,
            CONTROL_CANCELLED => DownloadControl::Cancel,
            _ => DownloadControl::Continue,
        }
    }

    fn progressed(&self, transferred_bytes: u64) {
        let now = Instant::now();
        let current_speed = self.sample.lock().ok().and_then(|mut sample| {
            let elapsed = now.duration_since(sample.measured_at);
            let delta = transferred_bytes.saturating_sub(sample.transferred_bytes);
            sample.transferred_bytes = transferred_bytes;
            sample.measured_at = now;
            rate_for(delta, elapsed)
        });
        let eta_ms = current_speed.and_then(|speed| {
            self.size_bytes
                .saturating_sub(transferred_bytes)
                .checked_mul(1_000)
                .map(|remaining| remaining / speed.max(1))
        });
        let snapshot = update_snapshot(&self.snapshots, self.id, |snapshot| {
            snapshot.transferred_bytes = transferred_bytes.min(snapshot.size_bytes);
            snapshot.current_bytes_per_second = current_speed;
            snapshot.eta_ms = eta_ms;
            snapshot.duration_ms = Some(
                self.previous_duration_ms
                    .saturating_add(elapsed_millis(self.started.elapsed())),
            );
        });
        let should_persist = self.last_persisted.lock().is_ok_and(|mut persisted| {
            if now.duration_since(*persisted) >= PROGRESS_PERSIST_INTERVAL
                || transferred_bytes >= self.size_bytes
            {
                *persisted = now;
                true
            } else {
                false
            }
        });
        if should_persist
            && let Some(snapshot) = snapshot
            && let Err(error) = persist_snapshot(&self.library, &snapshot)
        {
            tracing::warn!(
                event = "transfer.download.checkpoint_failed",
                task_id = self.id,
                error_kind = ?error.kind(),
                "native Telegram download checkpoint could not be persisted"
            );
        }
    }
}

impl DesktopTransfers {
    pub fn new(
        telegram: DesktopTelegram,
        library: DesktopLibrary,
    ) -> Result<Self, ApplicationError> {
        Self::with_backend(Arc::new(telegram), library)
    }

    fn with_backend(
        backend: Arc<dyn ChannelDownloadBackend>,
        library: DesktopLibrary,
    ) -> Result<Self, ApplicationError> {
        let (sender, receiver) = mpsc::sync_channel(CHANNEL_DOWNLOAD_QUEUE_CAPACITY);
        let restored = library.native_downloads()?;
        let mut snapshots = Vec::with_capacity(restored.len());
        let mut controls = BTreeMap::new();
        for record in restored {
            let mut snapshot = snapshot_from_record(record);
            if snapshot.state == ChannelDownloadState::Running {
                snapshot.state = ChannelDownloadState::Queued;
                snapshot.current_bytes_per_second = None;
                snapshot.eta_ms = None;
                persist_snapshot(&library, &snapshot)?;
            }
            if matches!(
                snapshot.state,
                ChannelDownloadState::Queued
                    | ChannelDownloadState::Running
                    | ChannelDownloadState::Paused
            ) {
                controls.insert(
                    snapshot.id,
                    Arc::new(AtomicU8::new(
                        if snapshot.state == ChannelDownloadState::Paused {
                            CONTROL_PAUSED
                        } else {
                            CONTROL_RUNNING
                        },
                    )),
                );
            }
            snapshots.push(snapshot);
        }
        let snapshots = Arc::new(Mutex::new(snapshots));
        let controls = Arc::new(Mutex::new(controls));
        let scheduled = Arc::new(Mutex::new(BTreeSet::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_snapshots = Arc::clone(&snapshots);
        let worker_scheduled = Arc::clone(&scheduled);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_library = library.clone();
        let worker_backend = Arc::clone(&backend);
        let join = thread::Builder::new()
            .name("teleark-channel-downloads".to_owned())
            .spawn(move || {
                transfer_loop(
                    receiver,
                    worker_backend,
                    worker_snapshots,
                    worker_scheduled,
                    worker_shutdown,
                    worker_library,
                );
            })
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?;
        Ok(Self {
            inner: Arc::new(TransferWorkerInner {
                sender: Mutex::new(Some(sender)),
                snapshots,
                controls,
                scheduled,
                shutdown,
                library,
                backend,
                join: Mutex::new(Some(join)),
            }),
        })
    }

    pub fn enqueue_channel_download(
        &self,
        request: ChannelDownloadRequest,
    ) -> Result<u64, ApplicationError> {
        validate_request(&request)?;
        if self.snapshots()?.iter().any(|snapshot| {
            snapshot.destination == request.destination
                && matches!(
                    snapshot.state,
                    ChannelDownloadState::Queued
                        | ChannelDownloadState::Running
                        | ChannelDownloadState::Paused
                )
        }) {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let queued_at_unix_ms = unix_time_millis()?;
        let stored = self
            .inner
            .library
            .insert_native_download(NewNativeDownloadTaskRecord {
                chat_id: request.chat_id,
                message_id: request.message_id,
                message_sent_at_unix_ms: request.message_sent_at_unix_ms,
                file_name: request.file_name,
                caption: request.caption,
                mime_type: request.mime_type,
                size_bytes: request.size_bytes,
                destination: request.destination,
                created_at_unix_ms: queued_at_unix_ms,
            })?;
        let snapshot = snapshot_from_record(stored);
        let id = snapshot.id;
        let control = Arc::new(AtomicU8::new(CONTROL_RUNNING));
        self.inner
            .snapshots
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .push(snapshot.clone());
        self.inner
            .controls
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .insert(id, Arc::clone(&control));
        tracing::info!(
            event = "transfer.download.queued",
            task_id = id,
            chat_id = snapshot.chat_id,
            message_id = snapshot.message_id,
            size_bytes = snapshot.size_bytes,
            "native Telegram download queued"
        );
        self.try_schedule(snapshot, control)?;
        Ok(id)
    }

    pub fn enqueue_channel_download_batch(
        &self,
        requests: Vec<ChannelDownloadRequest>,
    ) -> Result<u64, ApplicationError> {
        validate_batch_size(requests.len())?;
        for request in &requests {
            validate_request(request)?;
        }
        let chat_id = requests[0].chat_id;
        if requests.iter().any(|request| request.chat_id != chat_id) {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let existing = self.snapshots()?;
        let mut destinations = BTreeSet::new();
        for request in &requests {
            if !destinations.insert(request.destination.clone())
                || existing.iter().any(|snapshot| {
                    snapshot.destination == request.destination
                        && matches!(
                            snapshot.state,
                            ChannelDownloadState::Queued
                                | ChannelDownloadState::Running
                                | ChannelDownloadState::Paused
                        )
                })
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
            }
        }
        let created_at_unix_ms = unix_time_millis()?;
        let tasks = requests
            .into_iter()
            .map(|request| NewNativeDownloadTaskRecord {
                chat_id: request.chat_id,
                message_id: request.message_id,
                message_sent_at_unix_ms: request.message_sent_at_unix_ms,
                file_name: request.file_name,
                caption: request.caption,
                mime_type: request.mime_type,
                size_bytes: request.size_bytes,
                destination: request.destination,
                created_at_unix_ms,
            })
            .collect();
        let (batch, records) = self.inner.library.insert_native_download_batch(
            NewNativeDownloadBatchRecord {
                chat_id,
                created_at_unix_ms,
            },
            tasks,
        )?;
        for record in records {
            let snapshot = snapshot_from_record(record);
            let control = Arc::new(AtomicU8::new(CONTROL_RUNNING));
            self.inner
                .snapshots
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .push(snapshot.clone());
            self.inner
                .controls
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .insert(snapshot.id, Arc::clone(&control));
            self.try_schedule(snapshot, control)?;
        }
        tracing::info!(
            event = "transfer.download.batch_queued",
            batch_id = batch.id,
            chat_id,
            "native Telegram download batch queued"
        );
        Ok(batch.id)
    }

    pub fn activate_pending_downloads(&self) -> Result<(), ApplicationError> {
        for snapshot in self
            .snapshots()?
            .into_iter()
            .filter(|snapshot| snapshot.state == ChannelDownloadState::Queued)
        {
            let control = self
                .inner
                .controls
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .entry(snapshot.id)
                .or_insert_with(|| Arc::new(AtomicU8::new(CONTROL_RUNNING)))
                .clone();
            self.try_schedule(snapshot, control)?;
        }
        Ok(())
    }

    pub fn pause(&self, id: u64) -> Result<(), ApplicationError> {
        if !matches!(
            self.snapshot_state(id)?,
            ChannelDownloadState::Queued | ChannelDownloadState::Running
        ) {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        self.set_controlled_state(id, CONTROL_PAUSED, ChannelDownloadState::Paused)?;
        tracing::info!(
            event = "transfer.download.paused",
            task_id = id,
            "download paused"
        );
        Ok(())
    }

    pub fn resume(&self, id: u64) -> Result<(), ApplicationError> {
        if self.snapshot_state(id)? != ChannelDownloadState::Paused {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let scheduled = self
            .inner
            .scheduled
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .contains(&id);
        // A scheduled task may already be inside Telegram's chunk iterator. In
        // that case it must first observe the pause and release its worker slot
        // before the resumable partial can be opened by a new attempt. The
        // worker atomically turns this pending value back into RUNNING if the
        // command had not actually started yet.
        let control_value = if scheduled {
            CONTROL_RESUME_PENDING
        } else {
            CONTROL_RUNNING
        };
        let snapshot =
            self.set_controlled_state(id, control_value, ChannelDownloadState::Queued)?;
        tracing::info!(
            event = "transfer.download.resumed",
            task_id = id,
            "download resumed"
        );
        if !scheduled {
            self.try_schedule(snapshot, self.control(id)?)?;
        }
        Ok(())
    }

    pub fn cancel(&self, id: u64) -> Result<(), ApplicationError> {
        if is_terminal(self.snapshot_state(id)?) {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let control = self.control(id)?;
        control.store(CONTROL_CANCELLED, Ordering::Release);
        let now = unix_time_millis()?;
        let snapshot = update_snapshot(&self.inner.snapshots, id, |snapshot| {
            if is_terminal(snapshot.state) {
                return;
            }
            snapshot.state = ChannelDownloadState::Cancelled;
            snapshot.verification = ChannelDownloadVerification::NotReached;
            snapshot.finished_at_unix_ms = Some(now);
            snapshot.current_bytes_per_second = None;
            snapshot.eta_ms = None;
            if snapshot.events.last().map(|event| event.kind)
                != Some(ChannelDownloadEventKind::Cancelled)
            {
                snapshot.events.push(ChannelDownloadEvent {
                    kind: ChannelDownloadEventKind::Cancelled,
                    timestamp_unix_ms: now,
                    elapsed_ms: snapshot.duration_ms,
                    failure_kind: None,
                });
            }
        })
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        persist_snapshot(&self.inner.library, &snapshot)?;
        self.inner.backend.discard_partial(&snapshot.destination)?;
        tracing::info!(
            event = "transfer.download.cancelled",
            task_id = id,
            "download cancelled"
        );
        Ok(())
    }

    pub fn retry(&self, id: u64) -> Result<(), ApplicationError> {
        if !matches!(self.snapshot_state(id)?, ChannelDownloadState::Failed(_)) {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let now = unix_time_millis()?;
        let snapshot = update_snapshot(&self.inner.snapshots, id, |snapshot| {
            snapshot.state = ChannelDownloadState::Queued;
            snapshot.verification = ChannelDownloadVerification::Pending;
            snapshot.finished_at_unix_ms = None;
            snapshot.current_bytes_per_second = None;
            snapshot.eta_ms = None;
            snapshot.failure = None;
            snapshot.events.push(ChannelDownloadEvent {
                kind: ChannelDownloadEventKind::Queued,
                timestamp_unix_ms: now,
                elapsed_ms: snapshot.duration_ms,
                failure_kind: None,
            });
        })
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        persist_snapshot(&self.inner.library, &snapshot)?;
        let control = {
            let mut controls = self
                .inner
                .controls
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            controls
                .entry(id)
                .or_insert_with(|| Arc::new(AtomicU8::new(CONTROL_RUNNING)))
                .clone()
        };
        control.store(CONTROL_RUNNING, Ordering::Release);
        self.try_schedule(snapshot, control)
    }

    pub fn snapshots(&self) -> Result<Vec<ChannelDownloadSnapshot>, ApplicationError> {
        self.inner
            .snapshots
            .lock()
            .map(|snapshots| snapshots.clone())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))
    }

    pub fn current_rates(&self) -> Result<TransferRates, ApplicationError> {
        let download_bytes_per_second = self
            .snapshots()?
            .into_iter()
            .filter(|snapshot| snapshot.state == ChannelDownloadState::Running)
            .fold(0_u64, |total, snapshot| {
                total.saturating_add(snapshot.current_bytes_per_second.unwrap_or(0))
            });
        Ok(TransferRates {
            download_bytes_per_second,
            // The encrypted upload engine is not yet retained by the desktop
            // owner. Zero is the truthful live rate, not a preview estimate.
            upload_bytes_per_second: 0,
        })
    }

    fn control(&self, id: u64) -> Result<Arc<AtomicU8>, ApplicationError> {
        self.inner
            .controls
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .get(&id)
            .cloned()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Conflict))
    }

    fn snapshot_state(&self, id: u64) -> Result<ChannelDownloadState, ApplicationError> {
        self.inner
            .snapshots
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .iter()
            .find(|snapshot| snapshot.id == id)
            .map(|snapshot| snapshot.state)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))
    }

    fn set_controlled_state(
        &self,
        id: u64,
        control_value: u8,
        state: ChannelDownloadState,
    ) -> Result<ChannelDownloadSnapshot, ApplicationError> {
        let control = self.control(id)?;
        let now = unix_time_millis()?;
        let event_kind = if state == ChannelDownloadState::Paused {
            ChannelDownloadEventKind::Paused
        } else {
            ChannelDownloadEventKind::Resumed
        };
        let snapshot = update_snapshot(&self.inner.snapshots, id, |snapshot| {
            if is_terminal(snapshot.state) {
                return;
            }
            snapshot.state = state;
            snapshot.current_bytes_per_second = None;
            snapshot.eta_ms = None;
            snapshot.events.push(ChannelDownloadEvent {
                kind: event_kind,
                timestamp_unix_ms: now,
                elapsed_ms: snapshot.duration_ms,
                failure_kind: None,
            });
        })
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        persist_snapshot(&self.inner.library, &snapshot)?;
        control.store(control_value, Ordering::Release);
        Ok(snapshot)
    }

    fn try_schedule(
        &self,
        snapshot: ChannelDownloadSnapshot,
        control: Arc<AtomicU8>,
    ) -> Result<(), ApplicationError> {
        let mut scheduled = self
            .inner
            .scheduled
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        if !scheduled.insert(snapshot.id) {
            return Ok(());
        }
        let send_result = self
            .inner
            .sender
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Cancelled))?
            .try_send(TransferCommand::Download {
                snapshot: snapshot.clone(),
                queued_at: Instant::now(),
                control,
            });
        if let Err(error) = send_result {
            scheduled.remove(&snapshot.id);
            match error {
                mpsc::TrySendError::Full(_) => {
                    tracing::debug!(
                        event = "transfer.download.queue_capacity_wait",
                        task_id = snapshot.id,
                        "download remains durably queued until capacity is available"
                    );
                    return Ok(());
                }
                mpsc::TrySendError::Disconnected(_) => {
                    return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                }
            }
        }
        Ok(())
    }
}

impl Drop for TransferWorkerInner {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // Do not make application shutdown depend on Telegram delivering one
        // more network chunk. Persist the latest in-memory byte checkpoint
        // synchronously, then detach the worker if its backend is still
        // blocked. The worker owns all Arcs it needs to finish safely later.
        let active_checkpoint = self.snapshots.lock().ok().and_then(|mut snapshots| {
            snapshots
                .iter_mut()
                .find(|snapshot| snapshot.state == ChannelDownloadState::Running)
                .map(|snapshot| {
                    snapshot.state = ChannelDownloadState::Queued;
                    snapshot.current_bytes_per_second = None;
                    snapshot.eta_ms = None;
                    snapshot.clone()
                })
        });
        if let Some(checkpoint) = active_checkpoint
            && let Err(error) = persist_snapshot(&self.library, &checkpoint)
        {
            tracing::warn!(
                event = "transfer.download.shutdown_checkpoint_failed",
                task_id = checkpoint.id,
                error_kind = ?error.kind(),
                "latest download checkpoint could not be persisted during shutdown"
            );
        }
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
        if let Ok(mut join) = self.join.lock()
            && let Some(join) = join.take()
        {
            drop(join);
        }
    }
}

fn validate_request(request: &ChannelDownloadRequest) -> Result<(), ApplicationError> {
    if request.chat_id <= 0
        || request.message_id <= 0
        || request.file_name.trim().is_empty()
        || request.destination.as_os_str().is_empty()
        || request.destination.file_name().is_none()
    {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok(())
}

fn validate_batch_size(size: usize) -> Result<(), ApplicationError> {
    if size == 0 || size > CHANNEL_DOWNLOAD_BATCH_CAPACITY {
        return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
    }
    Ok(())
}

fn transfer_loop(
    receiver: mpsc::Receiver<TransferCommand>,
    backend: Arc<dyn ChannelDownloadBackend>,
    snapshots: Arc<Mutex<Vec<ChannelDownloadSnapshot>>>,
    scheduled: Arc<Mutex<BTreeSet<u64>>>,
    shutdown: Arc<AtomicBool>,
    library: DesktopLibrary,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            TransferCommand::Download {
                snapshot,
                queued_at,
                control,
            } => {
                let id = snapshot.id;
                run_download(
                    &*backend, &snapshots, &shutdown, &library, snapshot, queued_at, control,
                );
                if let Ok(mut scheduled) = scheduled.lock() {
                    scheduled.remove(&id);
                }
            }
        }
    }
}

fn run_download(
    backend: &dyn ChannelDownloadBackend,
    snapshots: &Arc<Mutex<Vec<ChannelDownloadSnapshot>>>,
    shutdown: &Arc<AtomicBool>,
    library: &DesktopLibrary,
    snapshot: ChannelDownloadSnapshot,
    queued_at: Instant,
    control: Arc<AtomicU8>,
) {
    let queue_wait_ms = elapsed_millis(queued_at.elapsed());
    let started_at_unix_ms = unix_time_millis().ok();
    let initial_control = control.load(Ordering::Acquire);
    let initial_state = if initial_control == CONTROL_RESUME_PENDING {
        control.store(CONTROL_RUNNING, Ordering::Release);
        ChannelDownloadState::Running
    } else if initial_control == CONTROL_PAUSED {
        ChannelDownloadState::Paused
    } else if initial_control == CONTROL_CANCELLED {
        ChannelDownloadState::Cancelled
    } else {
        ChannelDownloadState::Running
    };
    let started_snapshot = update_snapshot(snapshots, snapshot.id, |current| {
        current.state = initial_state;
        current.started_at_unix_ms = current.started_at_unix_ms.or(started_at_unix_ms);
        current.queue_wait_ms = Some(queue_wait_ms);
        current.attempts = current.attempts.saturating_add(1);
        current.events.push(ChannelDownloadEvent {
            kind: ChannelDownloadEventKind::Started,
            timestamp_unix_ms: started_at_unix_ms.unwrap_or(current.queued_at_unix_ms),
            elapsed_ms: Some(queue_wait_ms),
            failure_kind: None,
        });
    });
    if let Some(started_snapshot) = started_snapshot {
        let _ = persist_snapshot(library, &started_snapshot);
    }
    tracing::info!(
        event = "transfer.download.started",
        task_id = snapshot.id,
        chat_id = snapshot.chat_id,
        message_id = snapshot.message_id,
        size_bytes = snapshot.size_bytes,
        queue_wait_ms,
        "native Telegram download started"
    );
    let started = Instant::now();
    let previous_duration_ms = snapshot.duration_ms.unwrap_or(0);
    let observer: Arc<dyn DownloadObserver> = Arc::new(RuntimeDownloadObserver {
        id: snapshot.id,
        size_bytes: snapshot.size_bytes,
        control: Arc::clone(&control),
        shutdown: Arc::clone(shutdown),
        snapshots: Arc::clone(snapshots),
        library: library.clone(),
        started,
        previous_duration_ms,
        sample: Mutex::new(ProgressSample {
            transferred_bytes: snapshot.transferred_bytes,
            measured_at: started,
        }),
        last_persisted: Mutex::new(started),
    });
    let result = backend.download(
        snapshot.chat_id,
        snapshot.message_id,
        &snapshot.destination,
        observer,
    );
    let duration_ms = previous_duration_ms.saturating_add(elapsed_millis(started.elapsed()));
    let finished_at_unix_ms = unix_time_millis().ok();
    match result {
        Ok(()) if control.load(Ordering::Acquire) == CONTROL_CANCELLED => {
            let cancelled = update_snapshot(snapshots, snapshot.id, |current| {
                current.state = ChannelDownloadState::Cancelled;
                current.verification = ChannelDownloadVerification::NotReached;
                current.finished_at_unix_ms = finished_at_unix_ms;
                current.duration_ms = Some(duration_ms);
                current.current_bytes_per_second = None;
                current.eta_ms = None;
                current.failure = None;
            });
            if let Some(cancelled) = cancelled {
                let _ = persist_snapshot(library, &cancelled);
            }
        }
        Ok(()) => complete_download(
            snapshots,
            library,
            &snapshot,
            queue_wait_ms,
            duration_ms,
            finished_at_unix_ms,
        ),
        Err(error) if shutdown.load(Ordering::Acquire) => {
            let interrupted = update_snapshot(snapshots, snapshot.id, |current| {
                if current.state != ChannelDownloadState::Paused {
                    current.state = ChannelDownloadState::Queued;
                }
                current.current_bytes_per_second = None;
                current.eta_ms = None;
                current.failure = None;
            });
            if let Some(interrupted) = interrupted {
                let _ = persist_snapshot(library, &interrupted);
            }
            tracing::info!(
                event = "transfer.download.interrupted",
                task_id = snapshot.id,
                error_kind = ?error.kind(),
                "download checkpoint retained for next application start"
            );
        }
        Err(_error) if control.load(Ordering::Acquire) == CONTROL_PAUSED => {
            let paused = update_snapshot(snapshots, snapshot.id, |current| {
                current.state = ChannelDownloadState::Paused;
                current.current_bytes_per_second = None;
                current.eta_ms = None;
                current.failure = None;
            });
            if let Some(paused) = paused {
                let _ = persist_snapshot(library, &paused);
            }
        }
        Err(_error) if control.load(Ordering::Acquire) == CONTROL_RESUME_PENDING => {
            control.store(CONTROL_RUNNING, Ordering::Release);
            let resumed = update_snapshot(snapshots, snapshot.id, |current| {
                current.state = ChannelDownloadState::Queued;
                current.current_bytes_per_second = None;
                current.eta_ms = None;
                current.failure = None;
            });
            if let Some(resumed) = resumed {
                let _ = persist_snapshot(library, &resumed);
            }
        }
        Err(_error) if control.load(Ordering::Acquire) == CONTROL_CANCELLED => {
            let cancelled = update_snapshot(snapshots, snapshot.id, |current| {
                current.state = ChannelDownloadState::Cancelled;
                current.verification = ChannelDownloadVerification::NotReached;
                current.finished_at_unix_ms = finished_at_unix_ms;
                current.duration_ms = Some(duration_ms);
                current.current_bytes_per_second = None;
                current.eta_ms = None;
                current.failure = None;
            });
            if let Some(cancelled) = cancelled {
                let _ = persist_snapshot(library, &cancelled);
            }
        }
        Err(error) => fail_download(
            snapshots,
            library,
            &snapshot,
            error,
            queue_wait_ms,
            duration_ms,
            finished_at_unix_ms,
        ),
    }
}

fn complete_download(
    snapshots: &Mutex<Vec<ChannelDownloadSnapshot>>,
    library: &DesktopLibrary,
    snapshot: &ChannelDownloadSnapshot,
    queue_wait_ms: u64,
    duration_ms: u64,
    finished_at_unix_ms: Option<i64>,
) {
    let average_bytes_per_second = average_rate(snapshot.size_bytes, duration_ms);
    let completed = update_snapshot(snapshots, snapshot.id, |current| {
        current.state = ChannelDownloadState::Completed;
        current.verification = ChannelDownloadVerification::SizeChecked;
        current.transferred_bytes = current.size_bytes;
        current.finished_at_unix_ms = finished_at_unix_ms;
        current.duration_ms = Some(duration_ms);
        current.current_bytes_per_second = None;
        current.average_bytes_per_second = average_bytes_per_second;
        current.eta_ms = Some(0);
        current.failure = None;
        current.events.push(ChannelDownloadEvent {
            kind: ChannelDownloadEventKind::Completed,
            timestamp_unix_ms: finished_at_unix_ms.unwrap_or(current.queued_at_unix_ms),
            elapsed_ms: Some(duration_ms),
            failure_kind: None,
        });
    });
    if let Some(completed) = completed
        && let Err(error) = persist_snapshot(library, &completed)
    {
        tracing::error!(
            event = "transfer.download.completion_checkpoint_failed",
            task_id = snapshot.id,
            error_kind = ?error.kind(),
            "completed download history could not be persisted"
        );
    }
    tracing::info!(
        event = "transfer.download.completed",
        task_id = snapshot.id,
        chat_id = snapshot.chat_id,
        message_id = snapshot.message_id,
        size_bytes = snapshot.size_bytes,
        queue_wait_ms,
        duration_ms,
        average_bytes_per_second = ?average_bytes_per_second,
        verification = "telegram_declared_size",
        "native Telegram download completed"
    );
}

fn fail_download(
    snapshots: &Mutex<Vec<ChannelDownloadSnapshot>>,
    library: &DesktopLibrary,
    snapshot: &ChannelDownloadSnapshot,
    error: ApplicationError,
    queue_wait_ms: u64,
    duration_ms: u64,
    finished_at_unix_ms: Option<i64>,
) {
    let failure = failure_diagnostic(error.kind());
    let failed = update_snapshot(snapshots, snapshot.id, |current| {
        current.state = ChannelDownloadState::Failed(error.kind());
        current.verification = ChannelDownloadVerification::NotReached;
        current.finished_at_unix_ms = finished_at_unix_ms;
        current.duration_ms = Some(duration_ms);
        current.current_bytes_per_second = None;
        current.eta_ms = None;
        current.failure = Some(failure);
        current.events.push(ChannelDownloadEvent {
            kind: ChannelDownloadEventKind::Failed,
            timestamp_unix_ms: finished_at_unix_ms.unwrap_or(current.queued_at_unix_ms),
            elapsed_ms: Some(duration_ms),
            failure_kind: Some(error.kind()),
        });
    });
    if let Some(failed) = failed {
        let _ = persist_snapshot(library, &failed);
    }
    tracing::error!(
        event = "transfer.download.failed",
        task_id = snapshot.id,
        chat_id = snapshot.chat_id,
        message_id = snapshot.message_id,
        size_bytes = snapshot.size_bytes,
        queue_wait_ms,
        duration_ms,
        error_kind = ?error.kind(),
        failure_stage = ?failure.stage,
        retryable = failure.retryable,
        requires_user_action = failure.requires_user_action,
        "native Telegram download failed"
    );
}

fn update_snapshot(
    snapshots: &Mutex<Vec<ChannelDownloadSnapshot>>,
    id: u64,
    update: impl FnOnce(&mut ChannelDownloadSnapshot),
) -> Option<ChannelDownloadSnapshot> {
    let mut snapshots = snapshots.lock().ok()?;
    let snapshot = snapshots.iter_mut().find(|snapshot| snapshot.id == id)?;
    update(snapshot);
    Some(snapshot.clone())
}

fn snapshot_from_record(record: NativeDownloadTaskRecord) -> ChannelDownloadSnapshot {
    let state = match record.state {
        StoredNativeDownloadState::Queued => ChannelDownloadState::Queued,
        StoredNativeDownloadState::Running => ChannelDownloadState::Running,
        StoredNativeDownloadState::Paused => ChannelDownloadState::Paused,
        StoredNativeDownloadState::Completed => ChannelDownloadState::Completed,
        StoredNativeDownloadState::Cancelled => ChannelDownloadState::Cancelled,
        StoredNativeDownloadState::Failed => ChannelDownloadState::Failed(
            record
                .failure_code
                .as_deref()
                .and_then(parse_error_code)
                .unwrap_or(ApplicationErrorKind::Persistence),
        ),
    };
    let verification = match record.verification {
        StoredNativeDownloadVerification::Pending => ChannelDownloadVerification::Pending,
        StoredNativeDownloadVerification::SizeChecked => ChannelDownloadVerification::SizeChecked,
        StoredNativeDownloadVerification::NotReached => ChannelDownloadVerification::NotReached,
    };
    let failure = match state {
        ChannelDownloadState::Failed(kind) => Some(failure_diagnostic(kind)),
        _ => None,
    };
    let mut events = vec![ChannelDownloadEvent {
        kind: ChannelDownloadEventKind::Queued,
        timestamp_unix_ms: record.created_at_unix_ms,
        elapsed_ms: None,
        failure_kind: None,
    }];
    if let Some(started) = record.started_at_unix_ms {
        events.push(ChannelDownloadEvent {
            kind: ChannelDownloadEventKind::Started,
            timestamp_unix_ms: started,
            elapsed_ms: record.queue_wait_ms,
            failure_kind: None,
        });
    }
    if let Some(finished) = record.finished_at_unix_ms {
        let event = match state {
            ChannelDownloadState::Completed => Some((ChannelDownloadEventKind::Completed, None)),
            ChannelDownloadState::Failed(kind) => {
                Some((ChannelDownloadEventKind::Failed, Some(kind)))
            }
            ChannelDownloadState::Cancelled => Some((ChannelDownloadEventKind::Cancelled, None)),
            _ => None,
        };
        if let Some((kind, failure_kind)) = event {
            events.push(ChannelDownloadEvent {
                kind,
                timestamp_unix_ms: finished,
                elapsed_ms: record.duration_ms,
                failure_kind,
            });
        }
    }
    ChannelDownloadSnapshot {
        id: record.id,
        batch_id: record.batch_id,
        chat_id: record.chat_id,
        message_id: record.message_id,
        message_sent_at_unix_ms: record.message_sent_at_unix_ms,
        file_name: record.file_name,
        caption: record.caption,
        mime_type: record.mime_type,
        size_bytes: record.size_bytes,
        transferred_bytes: record.transferred_bytes,
        destination: record.destination,
        state,
        verification,
        queued_at_unix_ms: record.created_at_unix_ms,
        started_at_unix_ms: record.started_at_unix_ms,
        finished_at_unix_ms: record.finished_at_unix_ms,
        queue_wait_ms: record.queue_wait_ms,
        duration_ms: record.duration_ms,
        current_bytes_per_second: None,
        average_bytes_per_second: record.average_bytes_per_second,
        eta_ms: None,
        attempts: record.attempts,
        failure,
        events,
    }
}

fn persist_snapshot(
    library: &DesktopLibrary,
    snapshot: &ChannelDownloadSnapshot,
) -> Result<(), ApplicationError> {
    let state = match snapshot.state {
        ChannelDownloadState::Queued => StoredNativeDownloadState::Queued,
        ChannelDownloadState::Running => StoredNativeDownloadState::Running,
        ChannelDownloadState::Paused => StoredNativeDownloadState::Paused,
        ChannelDownloadState::Completed => StoredNativeDownloadState::Completed,
        ChannelDownloadState::Failed(_) => StoredNativeDownloadState::Failed,
        ChannelDownloadState::Cancelled => StoredNativeDownloadState::Cancelled,
    };
    let verification = match snapshot.verification {
        ChannelDownloadVerification::Pending => StoredNativeDownloadVerification::Pending,
        ChannelDownloadVerification::SizeChecked => StoredNativeDownloadVerification::SizeChecked,
        ChannelDownloadVerification::NotReached => StoredNativeDownloadVerification::NotReached,
    };
    library.save_native_download(NativeDownloadTaskRecord {
        id: snapshot.id,
        batch_id: snapshot.batch_id,
        chat_id: snapshot.chat_id,
        message_id: snapshot.message_id,
        message_sent_at_unix_ms: snapshot.message_sent_at_unix_ms,
        file_name: snapshot.file_name.clone(),
        caption: snapshot.caption.clone(),
        mime_type: snapshot.mime_type.clone(),
        size_bytes: snapshot.size_bytes,
        destination: snapshot.destination.clone(),
        state,
        verification,
        transferred_bytes: snapshot.transferred_bytes,
        created_at_unix_ms: snapshot.queued_at_unix_ms,
        started_at_unix_ms: snapshot.started_at_unix_ms,
        finished_at_unix_ms: snapshot.finished_at_unix_ms,
        queue_wait_ms: snapshot.queue_wait_ms,
        duration_ms: snapshot.duration_ms,
        average_bytes_per_second: snapshot.average_bytes_per_second,
        attempts: snapshot.attempts,
        failure_code: snapshot
            .failure
            .map(|failure| error_code(failure.kind).to_owned()),
        updated_at_unix_ms: unix_time_millis()?,
    })
}

fn unix_time_millis() -> Result<i64, ApplicationError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
        .as_millis();
    i64::try_from(millis).map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))
}

fn elapsed_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn rate_for(bytes: u64, duration: Duration) -> Option<u64> {
    let millis = elapsed_millis(duration);
    (bytes > 0 && millis > 0)
        .then(|| bytes.saturating_mul(1_000) / millis)
        .filter(|speed| *speed > 0)
}

fn average_rate(size_bytes: u64, duration_ms: u64) -> Option<u64> {
    size_bytes
        .checked_mul(1_000)
        .map(|scaled| scaled / duration_ms.max(1))
}

fn is_terminal(state: ChannelDownloadState) -> bool {
    matches!(
        state,
        ChannelDownloadState::Completed
            | ChannelDownloadState::Failed(_)
            | ChannelDownloadState::Cancelled
    )
}

fn failure_diagnostic(kind: ApplicationErrorKind) -> ChannelDownloadFailure {
    let (retryable, requires_user_action) = match kind {
        ApplicationErrorKind::Network
        | ApplicationErrorKind::Persistence
        | ApplicationErrorKind::Capacity => (true, false),
        ApplicationErrorKind::InvalidRequest
        | ApplicationErrorKind::NotFound
        | ApplicationErrorKind::Conflict
        | ApplicationErrorKind::SourceMissing
        | ApplicationErrorKind::SourceChanged
        | ApplicationErrorKind::PermissionDenied
        | ApplicationErrorKind::Authorization => (false, true),
        ApplicationErrorKind::Cancelled => (false, false),
        _ => (false, false),
    };
    ChannelDownloadFailure {
        kind,
        stage: ChannelDownloadFailureStage::Transfer,
        retryable,
        requires_user_action,
    }
}

fn error_code(kind: ApplicationErrorKind) -> &'static str {
    match kind {
        ApplicationErrorKind::InvalidRequest => "invalid_request",
        ApplicationErrorKind::NotFound => "not_found",
        ApplicationErrorKind::Conflict => "conflict",
        ApplicationErrorKind::Persistence => "persistence",
        ApplicationErrorKind::SourceMissing => "source_missing",
        ApplicationErrorKind::SourceChanged => "source_changed",
        ApplicationErrorKind::PermissionDenied => "permission_denied",
        ApplicationErrorKind::Capacity => "capacity",
        ApplicationErrorKind::Authorization => "authorization",
        ApplicationErrorKind::Network => "network",
        ApplicationErrorKind::Cancelled => "cancelled",
        _ => "persistence",
    }
}

fn parse_error_code(value: &str) -> Option<ApplicationErrorKind> {
    match value {
        "invalid_request" => Some(ApplicationErrorKind::InvalidRequest),
        "not_found" => Some(ApplicationErrorKind::NotFound),
        "conflict" => Some(ApplicationErrorKind::Conflict),
        "persistence" => Some(ApplicationErrorKind::Persistence),
        "source_missing" => Some(ApplicationErrorKind::SourceMissing),
        "source_changed" => Some(ApplicationErrorKind::SourceChanged),
        "permission_denied" => Some(ApplicationErrorKind::PermissionDenied),
        "capacity" => Some(ApplicationErrorKind::Capacity),
        "authorization" => Some(ApplicationErrorKind::Authorization),
        "network" => Some(ApplicationErrorKind::Network),
        "cancelled" => Some(ApplicationErrorKind::Cancelled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{
            Condvar, Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering as AtomicOrdering},
        },
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn batch_capacity_accepts_one_full_channel_page() {
        assert!(validate_batch_size(CHANNEL_DOWNLOAD_BATCH_CAPACITY).is_ok());
        assert_eq!(
            validate_batch_size(CHANNEL_DOWNLOAD_BATCH_CAPACITY + 1)
                .expect_err("oversized batch must fail")
                .kind(),
            ApplicationErrorKind::Capacity
        );
    }

    struct FakeBackend {
        outcome: Result<(), ApplicationErrorKind>,
        calls: StdMutex<Vec<(i64, i64, PathBuf)>>,
    }

    impl ChannelDownloadBackend for FakeBackend {
        fn download(
            &self,
            chat_id: i64,
            message_id: i64,
            destination: &Path,
            observer: Arc<dyn DownloadObserver>,
        ) -> Result<(), ApplicationError> {
            self.calls.lock().expect("fake call lock").push((
                chat_id,
                message_id,
                destination.to_owned(),
            ));
            match self.outcome {
                Ok(()) => {
                    observer.progressed(7);
                    thread::sleep(Duration::from_millis(2));
                    observer.progressed(14);
                    fs::write(destination, b"telegram bytes")
                        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
                    Ok(())
                }
                Err(kind) => Err(ApplicationError::new(kind)),
            }
        }
    }

    fn library(directory: &tempfile::TempDir) -> DesktopLibrary {
        DesktopLibrary::open(directory.path().join("library.sqlite3")).expect("desktop library")
    }

    fn request(destination: PathBuf) -> ChannelDownloadRequest {
        ChannelDownloadRequest {
            chat_id: 100,
            message_id: 200,
            message_sent_at_unix_ms: Some(1_700_000_000_000),
            file_name: "archive.zip".to_owned(),
            caption: Some("Archive caption".to_owned()),
            mime_type: Some("application/zip".to_owned()),
            size_bytes: 14,
            destination,
        }
    }

    fn wait_for_terminal(transfers: &DesktopTransfers, id: u64) -> ChannelDownloadSnapshot {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snapshot = transfers
                .snapshots()
                .expect("snapshots")
                .into_iter()
                .find(|snapshot| snapshot.id == id)
                .expect("queued snapshot");
            if is_terminal(snapshot.state) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "download worker timed out");
            thread::yield_now();
        }
    }

    #[test]
    fn real_worker_publishes_live_progress_and_persistent_completion() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let destination = directory.path().join("archive.zip");
        let library = library(&directory);
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers =
            DesktopTransfers::with_backend(backend.clone(), library.clone()).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("enqueue");
        let snapshot = wait_for_terminal(&transfers, id);
        assert_eq!(snapshot.state, ChannelDownloadState::Completed);
        assert_eq!(snapshot.transferred_bytes, 14);
        assert_eq!(
            snapshot.verification,
            ChannelDownloadVerification::SizeChecked
        );
        assert_eq!(snapshot.attempts, 1);
        assert!(snapshot.started_at_unix_ms.is_some());
        assert!(snapshot.finished_at_unix_ms.is_some());
        assert!(snapshot.average_bytes_per_second.is_some());
        assert_eq!(
            fs::read(destination).expect("download result"),
            b"telegram bytes"
        );
        drop(transfers);
        let restored = DesktopTransfers::with_backend(backend, library)
            .expect("restored worker")
            .snapshots()
            .expect("restored snapshots");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, id);
        assert_eq!(restored[0].state, ChannelDownloadState::Completed);
        assert_eq!(restored[0].transferred_bytes, 14);
    }

    #[test]
    fn batch_enqueue_persists_one_group_identity_and_all_message_metadata() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = DesktopTransfers::with_backend(backend.clone(), library.clone())
            .expect("download worker");
        let first = request(directory.path().join("first.zip"));
        let mut second = request(directory.path().join("second.zip"));
        second.message_id = 201;
        second.file_name = "second.zip".to_owned();
        second.caption = Some("Second complete caption".to_owned());
        let batch_id = transfers
            .enqueue_channel_download_batch(vec![first, second])
            .expect("batch enqueue");
        let ids = transfers
            .snapshots()
            .expect("snapshots")
            .into_iter()
            .filter(|snapshot| snapshot.batch_id == Some(batch_id))
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 2);
        for id in ids {
            assert_eq!(wait_for_terminal(&transfers, id).batch_id, Some(batch_id));
        }
        let snapshots = transfers.snapshots().expect("completed snapshots");
        assert_eq!(
            snapshots[1].caption.as_deref(),
            Some("Second complete caption")
        );
        assert_eq!(snapshots[1].mime_type.as_deref(), Some("application/zip"));
        drop(transfers);

        let restored = DesktopTransfers::with_backend(backend, library)
            .expect("restored worker")
            .snapshots()
            .expect("restored snapshots");
        assert_eq!(restored.len(), 2);
        assert!(
            restored
                .iter()
                .all(|snapshot| snapshot.batch_id == Some(batch_id))
        );
        assert!(
            restored
                .iter()
                .all(|snapshot| snapshot.state == ChannelDownloadState::Completed)
        );
    }

    #[test]
    fn backend_failures_remain_structured_and_persistent() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let backend = Arc::new(FakeBackend {
            outcome: Err(ApplicationErrorKind::Network),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers =
            DesktopTransfers::with_backend(backend.clone(), library.clone()).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("failed.zip")))
            .expect("enqueue");
        let snapshot = wait_for_terminal(&transfers, id);
        assert_eq!(
            snapshot.state,
            ChannelDownloadState::Failed(ApplicationErrorKind::Network)
        );
        assert_eq!(
            snapshot.failure,
            Some(failure_diagnostic(ApplicationErrorKind::Network))
        );
        drop(transfers);
        let restored = DesktopTransfers::with_backend(backend, library)
            .expect("restored worker")
            .snapshots()
            .expect("restored snapshots");
        assert_eq!(
            restored[0].state,
            ChannelDownloadState::Failed(ApplicationErrorKind::Network)
        );
    }

    #[test]
    fn downloads_from_different_channels_keep_their_source_identity() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers =
            DesktopTransfers::with_backend(backend.clone(), library(&directory)).expect("worker");
        let first = transfers
            .enqueue_channel_download(request(directory.path().join("first.zip")))
            .expect("first enqueue");
        let second = transfers
            .enqueue_channel_download(ChannelDownloadRequest {
                chat_id: 101,
                message_id: 201,
                message_sent_at_unix_ms: Some(1_700_000_001_000),
                file_name: "second.pdf".to_owned(),
                caption: Some("Second caption".to_owned()),
                mime_type: Some("application/pdf".to_owned()),
                size_bytes: 14,
                destination: directory.path().join("second.pdf"),
            })
            .expect("second enqueue");
        assert_eq!(wait_for_terminal(&transfers, first).chat_id, 100);
        let second_snapshot = wait_for_terminal(&transfers, second);
        assert_eq!(second_snapshot.chat_id, 101);
        let calls = backend.calls.lock().expect("calls");
        assert_eq!((calls[0].0, calls[0].1), (100, 200));
        assert_eq!((calls[1].0, calls[1].1), (101, 201));
    }

    #[test]
    fn pause_resume_and_cancel_control_the_worker() {
        struct ControlledBackend {
            attempts: AtomicUsize,
            discarded: AtomicUsize,
        }
        impl ChannelDownloadBackend for ControlledBackend {
            fn download(
                &self,
                _chat_id: i64,
                _message_id: i64,
                _destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                self.attempts.fetch_add(1, AtomicOrdering::Relaxed);
                for bytes in 1..=14 {
                    match observer.control() {
                        DownloadControl::Continue => {}
                        DownloadControl::Pause
                        | DownloadControl::Cancel
                        | DownloadControl::Stop => {
                            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                        }
                    }
                    observer.progressed(bytes);
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(())
            }

            fn discard_partial(&self, _destination: &Path) -> Result<(), ApplicationError> {
                self.discarded.fetch_add(1, AtomicOrdering::Relaxed);
                Ok(())
            }
        }
        let directory = tempfile::tempdir().expect("temporary directory");
        let backend = Arc::new(ControlledBackend {
            attempts: AtomicUsize::new(0),
            discarded: AtomicUsize::new(0),
        });
        let transfers =
            DesktopTransfers::with_backend(backend.clone(), library(&directory)).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("controlled.zip")))
            .expect("enqueue");
        let deadline = Instant::now() + Duration::from_secs(1);
        while transfers.snapshots().expect("snapshots")[0].transferred_bytes == 0 {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        transfers.pause(id).expect("pause");
        assert_eq!(
            transfers.snapshots().expect("snapshots")[0].state,
            ChannelDownloadState::Paused
        );
        transfers.resume(id).expect("resume");
        let deadline = Instant::now() + Duration::from_secs(3);
        let resumed = loop {
            transfers
                .activate_pending_downloads()
                .expect("activate resumed task");
            let snapshot = transfers
                .snapshots()
                .expect("snapshots")
                .into_iter()
                .find(|snapshot| snapshot.id == id)
                .expect("resumed snapshot");
            if snapshot.state == ChannelDownloadState::Completed {
                break snapshot;
            }
            assert!(Instant::now() < deadline, "resumed download timed out");
            thread::yield_now();
        };
        assert!(backend.attempts.load(AtomicOrdering::Relaxed) >= 2);
        assert!(
            resumed
                .events
                .iter()
                .any(|event| event.kind == ChannelDownloadEventKind::Paused)
        );
        assert!(
            resumed
                .events
                .iter()
                .any(|event| event.kind == ChannelDownloadEventKind::Resumed)
        );

        let cancelled_id = transfers
            .enqueue_channel_download(request(directory.path().join("cancelled.zip")))
            .expect("cancel enqueue");
        let deadline = Instant::now() + Duration::from_secs(1);
        while transfers
            .snapshots()
            .expect("snapshots")
            .into_iter()
            .find(|snapshot| snapshot.id == cancelled_id)
            .expect("cancel snapshot")
            .transferred_bytes
            == 0
        {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        transfers.cancel(cancelled_id).expect("cancel");
        assert_eq!(
            wait_for_terminal(&transfers, cancelled_id).state,
            ChannelDownloadState::Cancelled
        );
        assert_eq!(backend.discarded.load(AtomicOrdering::Relaxed), 1);
    }

    #[test]
    fn interrupted_active_download_restores_its_checkpoint_after_restart() {
        struct ShutdownAwareBackend;
        impl ChannelDownloadBackend for ShutdownAwareBackend {
            fn download(
                &self,
                _chat_id: i64,
                _message_id: i64,
                _destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                observer.progressed(7);
                loop {
                    match observer.control() {
                        DownloadControl::Continue => thread::sleep(Duration::from_millis(2)),
                        DownloadControl::Pause
                        | DownloadControl::Cancel
                        | DownloadControl::Stop => {
                            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                        }
                    }
                }
            }
        }

        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let transfers =
            DesktopTransfers::with_backend(Arc::new(ShutdownAwareBackend), library.clone())
                .expect("worker");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("restart.zip")))
            .expect("enqueue");
        let deadline = Instant::now() + Duration::from_secs(1);
        while transfers.snapshots().expect("snapshots")[0].transferred_bytes < 7 {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        drop(transfers);

        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let restored = DesktopTransfers::with_backend(backend, library).expect("restored worker");
        let checkpoint = restored
            .snapshots()
            .expect("restored snapshots")
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("restored checkpoint");
        assert_eq!(checkpoint.state, ChannelDownloadState::Queued);
        assert_eq!(checkpoint.transferred_bytes, 7);
        restored
            .activate_pending_downloads()
            .expect("activate restored task");
        let completed = wait_for_terminal(&restored, id);
        assert_eq!(completed.state, ChannelDownloadState::Completed);
        let completed_duration = completed.duration_ms.expect("completed duration");
        assert!(completed_duration >= checkpoint.duration_ms.unwrap_or(0));
        assert_eq!(
            completed.average_bytes_per_second,
            average_rate(14, completed_duration)
        );
    }

    #[test]
    fn shutdown_does_not_wait_for_a_stalled_telegram_read() {
        struct StalledBackend {
            gate: Arc<(StdMutex<(bool, bool)>, Condvar)>,
        }
        impl ChannelDownloadBackend for StalledBackend {
            fn download(
                &self,
                _chat_id: i64,
                _message_id: i64,
                _destination: &Path,
                _observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                let (lock, condition) = &*self.gate;
                let mut state = lock.lock().expect("gate lock");
                state.0 = true;
                condition.notify_all();
                while !state.1 {
                    state = condition.wait(state).expect("gate wait");
                }
                Ok(())
            }
        }

        let gate = Arc::new((StdMutex::new((false, false)), Condvar::new()));
        let directory = tempfile::tempdir().expect("temporary directory");
        let transfers = DesktopTransfers::with_backend(
            Arc::new(StalledBackend {
                gate: Arc::clone(&gate),
            }),
            library(&directory),
        )
        .expect("worker");
        transfers
            .enqueue_channel_download(request(directory.path().join("stalled.zip")))
            .expect("enqueue");
        let (lock, condition) = &*gate;
        let mut state = lock.lock().expect("gate lock");
        while !state.0 {
            state = condition.wait(state).expect("gate wait");
        }
        drop(state);

        let started = Instant::now();
        drop(transfers);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "shutdown waited for a stalled backend"
        );

        let mut state = lock.lock().expect("gate lock");
        state.1 = true;
        condition.notify_all();
    }

    #[test]
    fn invalid_and_duplicate_active_destinations_are_rejected() {
        struct BlockingBackend {
            gate: Arc<(StdMutex<bool>, Condvar)>,
        }
        impl ChannelDownloadBackend for BlockingBackend {
            fn download(
                &self,
                _chat_id: i64,
                _message_id: i64,
                _destination: &Path,
                _observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                let (lock, condition) = &*self.gate;
                let mut released = lock.lock().expect("gate lock");
                while !*released {
                    released = condition.wait(released).expect("gate wait");
                }
                Ok(())
            }
        }
        let gate = Arc::new((StdMutex::new(false), Condvar::new()));
        let directory = tempfile::tempdir().expect("temporary directory");
        let transfers = DesktopTransfers::with_backend(
            Arc::new(BlockingBackend {
                gate: Arc::clone(&gate),
            }),
            library(&directory),
        )
        .expect("worker");
        let destination = directory.path().join("same.zip");
        transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("first enqueue");
        let error = transfers
            .enqueue_channel_download(request(destination))
            .expect_err("duplicate active destination must fail");
        assert_eq!(error.kind(), ApplicationErrorKind::Conflict);
        let invalid = ChannelDownloadRequest {
            chat_id: 0,
            ..request(directory.path().join("invalid.zip"))
        };
        assert_eq!(
            transfers
                .enqueue_channel_download(invalid)
                .expect_err("invalid chat must fail")
                .kind(),
            ApplicationErrorKind::InvalidRequest
        );
        let (lock, condition) = &*gate;
        *lock.lock().expect("gate lock") = true;
        condition.notify_all();
    }

    #[test]
    fn default_download_destination_is_safe_and_never_overwrites() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let existing = directory.path().join("archive.tar.gz");
        fs::write(&existing, b"existing").expect("write existing file");
        assert_eq!(
            available_download_destination(directory.path(), "archive.tar.gz")
                .expect("collision-safe destination"),
            directory.path().join("archive.tar (1).gz")
        );
        assert_eq!(
            available_download_destination(directory.path(), "../escape")
                .expect_err("path traversal must fail")
                .kind(),
            ApplicationErrorKind::InvalidRequest
        );
    }
}
