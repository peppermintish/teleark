use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
use teleark_telegram::{
    DOWNLOAD_PART_SIZE_BYTES, DownloadControl, DownloadObserver, DownloadPartEvent,
    DownloadPartState,
};
use teleark_transfer::{
    AdaptiveControllerConfig, AdaptiveTransferController, ControllerPhase, MemoryCounters,
    ParameterBounds, PartCounters, PerformanceSample, QueueCounters, TransferBottleneck,
    TransferControlParameters, TransferTelemetrySnapshot,
};

use crate::{
    DesktopLibrary, DesktopTelegram, DownloadThroughputStrategy,
    vault::{TransferSessionKind, TransferSessionLog},
};

const CHANNEL_DOWNLOAD_QUEUE_CAPACITY: usize = 32;
const CHANNEL_DOWNLOAD_BATCH_CAPACITY: usize = 5_000;
const PROGRESS_PERSIST_INTERVAL: Duration = Duration::from_millis(750);
const CONTROL_RUNNING: u8 = 0;
const CONTROL_PAUSED: u8 = 1;
const CONTROL_CANCELLED: u8 = 2;
const CONTROL_RESUME_PENDING: u8 = 3;
const CONTROL_RETRY_PENDING: u8 = 4;

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
    pub account_id: i64,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelDownloadPartEvent {
    pub part_index: u64,
    pub offset_bytes: u64,
    pub length_bytes: u64,
    pub state: DownloadPartState,
    pub attempt: u32,
    pub elapsed_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelDownloadSnapshot {
    pub account_id: Option<i64>,
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
    pub part_events: Vec<ChannelDownloadPartEvent>,
    pub session_log_path: Option<PathBuf>,
    pub telemetry: TransferTelemetrySnapshot,
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
    active_account: std::sync::atomic::AtomicI64,
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
        account_id: Option<i64>,
        chat_id: i64,
        message_id: i64,
        destination: &Path,
        observer: Arc<dyn DownloadObserver>,
    ) -> Result<(), ApplicationError>;

    fn discard_partial(&self, _destination: &Path) -> Result<(), ApplicationError> {
        Ok(())
    }

    fn cleanup_failed_partial(
        &self,
        _destination: &Path,
        _expected_bytes: u64,
        _retain_for_resume: bool,
    ) -> Result<(), ApplicationError> {
        Ok(())
    }
}

impl ChannelDownloadBackend for DesktopTelegram {
    fn download(
        &self,
        account_id: Option<i64>,
        chat_id: i64,
        message_id: i64,
        destination: &Path,
        observer: Arc<dyn DownloadObserver>,
    ) -> Result<(), ApplicationError> {
        let account_id =
            account_id.ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        self.download_file_observed(account_id, chat_id, message_id, destination, observer)
    }

    fn discard_partial(&self, destination: &Path) -> Result<(), ApplicationError> {
        self.discard_partial_download(destination)
    }

    fn cleanup_failed_partial(
        &self,
        destination: &Path,
        expected_bytes: u64,
        retain_for_resume: bool,
    ) -> Result<(), ApplicationError> {
        self.cleanup_failed_partial_download(destination, expected_bytes, retain_for_resume)
    }
}

struct ProgressSample {
    observations: VecDeque<(u64, Instant)>,
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
    controller: Mutex<AdaptiveTransferController>,
    download_strategy: DownloadThroughputStrategy,
    session_log: Mutex<Option<TransferSessionLog>>,
}

impl RuntimeDownloadObserver {
    fn finish_session_log(&self, elapsed_ms: u64, error_kind: Option<ApplicationErrorKind>) {
        let telemetry = self
            .controller
            .lock()
            .map(|controller| controller.snapshot())
            .unwrap_or_else(|_| empty_download_telemetry());
        if let Ok(mut log) = self.session_log.lock()
            && let Some(log) = log.as_mut()
        {
            let _ = log.append_finished(elapsed_ms, error_kind, &telemetry);
        }
    }
}

impl DownloadObserver for RuntimeDownloadObserver {
    fn control(&self) -> DownloadControl {
        if self.shutdown.load(Ordering::Acquire) {
            return DownloadControl::Stop;
        }
        match self.control.load(Ordering::Acquire) {
            CONTROL_PAUSED | CONTROL_RESUME_PENDING => DownloadControl::Pause,
            CONTROL_CANCELLED | CONTROL_RETRY_PENDING => DownloadControl::Cancel,
            _ => DownloadControl::Continue,
        }
    }

    fn progressed(&self, transferred_bytes: u64) {
        let now = Instant::now();
        let current_speed = self.sample.lock().ok().and_then(|mut sample| {
            sample.observations.push_back((transferred_bytes, now));
            while sample.observations.len() > 2
                && sample.observations.get(1).is_some_and(|(_, measured_at)| {
                    now.duration_since(*measured_at) >= Duration::from_secs(5)
                })
            {
                sample.observations.pop_front();
            }
            let (earliest_bytes, earliest_time) = sample.observations.front().copied()?;
            rate_for(
                transferred_bytes.saturating_sub(earliest_bytes),
                now.duration_since(earliest_time),
            )
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

    fn max_part_attempts(&self) -> u32 {
        if self.download_strategy == DownloadThroughputStrategy::MaxThroughput {
            8
        } else {
            4
        }
    }

    fn desired_inflight_parts(&self) -> usize {
        self.controller
            .lock()
            .map(|controller| usize::from(controller.parameters().inflight_parts_per_file))
            .unwrap_or(1)
    }

    fn part_retry(&self, event: DownloadPartEvent, server_wait: Option<Duration>) {
        self.process_part_event(event, server_wait);
    }

    fn part_event(&self, event: DownloadPartEvent) {
        self.process_part_event(event, None);
    }
}

impl RuntimeDownloadObserver {
    fn process_part_event(&self, event: DownloadPartEvent, server_wait: Option<Duration>) {
        let part_event = ChannelDownloadPartEvent {
            part_index: event.part_index,
            offset_bytes: event.offset_bytes,
            length_bytes: event.length_bytes,
            state: event.state,
            attempt: event.attempt,
            elapsed_millis: event.elapsed_millis,
        };
        let snapshot = update_snapshot(&self.snapshots, self.id, |snapshot| {
            if snapshot.part_events.len() >= 8_192 {
                snapshot.part_events.remove(0);
            }
            snapshot.part_events.push(part_event);
            snapshot.telemetry.parts = current_part_counters(
                &snapshot.part_events,
                snapshot.size_bytes.div_ceil(DOWNLOAD_PART_SIZE_BYTES),
                0,
            );
        });
        if event.state != DownloadPartState::Completed
            && let Ok(mut log) = self.session_log.lock()
            && let Some(log) = log.as_mut()
        {
            let _ =
                log.append_native_part_state(event, elapsed_millis(self.started.elapsed()).max(1));
        }
        if event.state == DownloadPartState::Retry {
            let elapsed_ms = elapsed_millis(self.started.elapsed()).max(1);
            let current_speed = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.current_bytes_per_second)
                .unwrap_or_default();
            let part_counters = snapshot
                .as_ref()
                .map(|snapshot| snapshot.telemetry.parts)
                .unwrap_or_default();
            if let Ok(mut controller) = self.controller.lock() {
                let inflight_parts_per_file = controller.parameters().inflight_parts_per_file;
                let decision = controller.recover_from_part_retry(PerformanceSample {
                    flood_wait_seconds: server_wait
                        .map(|delay| u32::try_from(delay.as_secs()).unwrap_or(u32::MAX)),
                    observed_at_millis: elapsed_ms,
                    goodput_bytes_per_second: current_speed,
                    disk_bytes_per_second: current_speed,
                    round_trip_time_p95_millis: event.elapsed_millis,
                    inflight_bytes: DOWNLOAD_PART_SIZE_BYTES
                        .saturating_mul(u64::from(inflight_parts_per_file)),
                    active_large_files: 1,
                    parts: part_counters,
                    queues: QueueCounters {
                        large_files_active: 1,
                        large_queue_weight: 100,
                        ..QueueCounters::default()
                    },
                    memory: MemoryCounters {
                        network_inflight_bytes: DOWNLOAD_PART_SIZE_BYTES
                            .saturating_mul(u64::from(inflight_parts_per_file)),
                        ..MemoryCounters::default()
                    },
                    ..PerformanceSample::default()
                });
                let telemetry = controller.snapshot();
                drop(controller);
                if let Ok(mut log) = self.session_log.lock()
                    && let Some(log) = log.as_mut()
                {
                    let _ = log.append_native_retry_decision(
                        event.part_index,
                        elapsed_ms,
                        &decision,
                        &telemetry,
                    );
                }
                let _ = update_snapshot(&self.snapshots, self.id, |snapshot| {
                    snapshot.telemetry = telemetry;
                });
            }
        }
        if event.state != DownloadPartState::Completed {
            return;
        }
        let elapsed_ms = elapsed_millis(self.started.elapsed()).max(1);
        let current_speed = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.current_bytes_per_second)
            .unwrap_or_default();
        let completed_parts = snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.telemetry.parts.completed_parts);
        let total_parts = self.size_bytes.div_ceil(DOWNLOAD_PART_SIZE_BYTES);
        let completed_parts_per_second_milli = completed_parts
            .saturating_mul(1_000_000)
            .checked_div(elapsed_ms)
            .unwrap_or_default();
        let part_counters = snapshot.as_ref().map_or_else(
            || PartCounters {
                total_parts,
                completed_parts,
                missing_parts: total_parts.saturating_sub(completed_parts),
                completed_parts_per_second_milli,
                ..PartCounters::default()
            },
            |snapshot| {
                current_part_counters(
                    &snapshot.part_events,
                    total_parts,
                    completed_parts_per_second_milli,
                )
            },
        );
        let mut controller = match self.controller.lock() {
            Ok(controller) => controller,
            Err(_) => return,
        };
        let inflight_parts_per_file = controller.parameters().inflight_parts_per_file;
        let decision = controller.observe(PerformanceSample {
            observed_at_millis: elapsed_ms,
            goodput_bytes_per_second: current_speed,
            disk_bytes_per_second: current_speed,
            round_trip_time_p95_millis: event.elapsed_millis,
            inflight_bytes: DOWNLOAD_PART_SIZE_BYTES
                .saturating_mul(u64::from(inflight_parts_per_file)),
            active_large_files: 1,
            parts: part_counters,
            queues: QueueCounters {
                large_files_active: 1,
                large_queue_weight: 100,
                ..QueueCounters::default()
            },
            memory: MemoryCounters {
                network_inflight_bytes: DOWNLOAD_PART_SIZE_BYTES
                    .saturating_mul(u64::from(inflight_parts_per_file)),
                writer_queue_bytes: event.length_bytes,
                ..MemoryCounters::default()
            },
            ..PerformanceSample::default()
        });
        let telemetry = controller.snapshot();
        drop(controller);
        if let Ok(mut log) = self.session_log.lock()
            && let Some(log) = log.as_mut()
        {
            let _ = log.append_part_confirmed(
                u32::try_from(event.part_index).unwrap_or(u32::MAX),
                elapsed_ms,
                event.length_bytes,
                &decision,
                &telemetry,
            );
        }
        let _ = update_snapshot(&self.snapshots, self.id, |snapshot| {
            snapshot.telemetry = telemetry;
        });
    }
}

fn current_part_counters(
    events: &[ChannelDownloadPartEvent],
    total_parts: u64,
    completed_parts_per_second_milli: u64,
) -> PartCounters {
    let mut current_states = BTreeMap::new();
    for event in events {
        current_states.insert(event.part_index, event.state);
    }
    let count = |state| {
        current_states
            .values()
            .filter(|current| **current == state)
            .count() as u64
    };
    let completed_parts = count(DownloadPartState::Completed);
    PartCounters {
        total_parts,
        completed_parts,
        inflight_parts: count(DownloadPartState::Inflight),
        retry_parts: count(DownloadPartState::Retry),
        failed_parts: count(DownloadPartState::Failed),
        missing_parts: total_parts.saturating_sub(completed_parts),
        completed_parts_per_second_milli,
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
        let transfer_log_directory = library.managed_directories()?.logs.join("Transfers");
        let mut snapshots = Vec::with_capacity(restored.len());
        let mut controls = BTreeMap::new();
        for record in restored {
            let mut snapshot = snapshot_from_record(record);
            let session_log_path =
                transfer_log_directory.join(format!("native-download-{}.jsonl", snapshot.id));
            if session_log_path.is_file() {
                snapshot.session_log_path = Some(session_log_path);
            }
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
                active_account: std::sync::atomic::AtomicI64::new(0),
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
        self.require_active_account(Some(request.account_id))?;
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
                account_id: request.account_id,
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
            self.require_active_account(Some(request.account_id))?;
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
                account_id: request.account_id,
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
        for snapshot in self.snapshots()?.into_iter().filter(|snapshot| {
            snapshot.state == ChannelDownloadState::Queued
                && self.require_active_account(snapshot.account_id).is_ok()
        }) {
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
        self.require_task_account(id)?;
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
        self.require_task_account(id)?;
        // Serialize with worker retirement so a retry cannot revive an attempt
        // that still owns the cancelled partial and outstanding requests.
        let scheduled = self
            .inner
            .scheduled
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        if !matches!(
            self.snapshot_state(id)?,
            ChannelDownloadState::Failed(_) | ChannelDownloadState::Cancelled
        ) {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let cancelled = self.snapshot_state(id)? == ChannelDownloadState::Cancelled;
        let now = unix_time_millis()?;
        let snapshot = update_snapshot(&self.inner.snapshots, id, |snapshot| {
            if cancelled {
                snapshot.transferred_bytes = 0;
                snapshot.telemetry = empty_download_telemetry();
            }
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
        if scheduled.contains(&id) {
            control.store(CONTROL_RETRY_PENDING, Ordering::Release);
            return Ok(());
        }
        control.store(CONTROL_RUNNING, Ordering::Release);
        drop(scheduled);
        self.try_schedule(snapshot, control)
    }

    /// Deletes one terminal task's history and TeleArk-owned recovery data.
    /// A successfully downloaded destination is user data and is never removed.
    pub fn delete(&self, id: u64) -> Result<(), ApplicationError> {
        let snapshot = self
            .snapshots()?
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        if !is_terminal(snapshot.state)
            || self
                .inner
                .scheduled
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .contains(&id)
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }

        self.inner.library.delete_native_download(id)?;
        self.inner
            .snapshots
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .retain(|snapshot| snapshot.id != id);
        self.inner
            .controls
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .remove(&id);

        if let Err(error) = self.inner.backend.discard_partial(&snapshot.destination) {
            tracing::warn!(
                event = "transfer.download.deleted_partial_cleanup_failed",
                task_id = id,
                error_kind = ?error.kind(),
                "deleted task partial cleanup could not be completed"
            );
        }
        if let Some(session_log_path) = snapshot.session_log_path.as_deref()
            && let Err(error) = remove_file_if_present(session_log_path)
        {
            tracing::warn!(
                event = "transfer.download.deleted_log_cleanup_failed",
                task_id = id,
                error_kind = ?error.kind(),
                "deleted task session log cleanup could not be completed"
            );
        }
        tracing::info!(
            event = "transfer.download.deleted",
            task_id = id,
            "terminal download task and recovery data deleted"
        );
        Ok(())
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

    /// Enable scheduling after the frontend has refreshed the authorized account's sources.
    /// Legacy history ownership was resolved durably by the Telegram connection owner.
    pub fn activate_account(&self, account_id: i64) -> Result<(), ApplicationError> {
        if account_id <= 0 {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        if self.inner.active_account.load(Ordering::Acquire) == account_id {
            return Ok(());
        }
        if !self
            .inner
            .scheduled
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .is_empty()
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        self.inner.active_account.store(0, Ordering::Release);
        // The serialized Telegram owner already made the durable one-time decision
        // before returning a configured connection. Refresh only in-memory ownership.
        for record in self.inner.library.native_downloads()? {
            update_snapshot(&self.inner.snapshots, record.id, |snapshot| {
                snapshot.account_id = record.account_id
            });
        }
        self.inner
            .active_account
            .store(account_id, Ordering::Release);
        Ok(())
    }

    /// Pause active work and wait for its retained workers to close partial files before logout.
    pub fn suspend_account(&self) -> Result<(), ApplicationError> {
        self.inner.active_account.store(0, Ordering::Release);
        for snapshot in self.snapshots()? {
            if matches!(
                snapshot.state,
                ChannelDownloadState::Queued | ChannelDownloadState::Running
            ) {
                self.pause(snapshot.id)?;
            }
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if self
                .inner
                .scheduled
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .is_empty()
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn require_active_account(&self, account_id: Option<i64>) -> Result<(), ApplicationError> {
        let active = self.inner.active_account.load(Ordering::Acquire);
        if active > 0 && account_id == Some(active) {
            Ok(())
        } else {
            Err(ApplicationError::new(ApplicationErrorKind::Authorization))
        }
    }

    fn require_task_account(&self, id: u64) -> Result<(), ApplicationError> {
        let snapshot = self
            .snapshots()?
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        self.require_active_account(snapshot.account_id)
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

fn new_download_controller(
    soft_limit_policy: teleark_transfer::SoftLimitPolicy,
    strategy: DownloadThroughputStrategy,
) -> Result<AdaptiveTransferController, ApplicationError> {
    let available_parallelism = thread::available_parallelism()
        .ok()
        .and_then(|count| u16::try_from(count.get()).ok())
        .unwrap_or(1)
        .clamp(1, 16);
    let mut config =
        AdaptiveControllerConfig::maximum_throughput(512 * 1024 * 1024, available_parallelism)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.transfer_connections = ParameterBounds::new(1, 1, 1)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.inflight_rpcs_per_connection = ParameterBounds::new(1, 1, 1)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.active_files = ParameterBounds::new(1, 1, 1)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    let aggressive = strategy == DownloadThroughputStrategy::MaxThroughput;
    config.download_strategy = strategy;
    config.inflight_parts_per_file = ParameterBounds::new(
        if aggressive { 1 } else { 4 },
        if aggressive { 64 } else { 24 },
        if aggressive { 16 } else { 4 },
    )
    .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.probe_settle_millis = if aggressive { 1_000 } else { 2_000 };
    config.soft_limit_policy = soft_limit_policy;
    let initial_parameters = TransferControlParameters {
        inflight_parts_per_file: 4,
        inflight_rpcs_per_connection: 1,
        ..TransferControlParameters::conservative_download()
    };
    AdaptiveTransferController::with_initial_parameters(config, false, initial_parameters)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))
}

fn empty_download_telemetry() -> TransferTelemetrySnapshot {
    TransferTelemetrySnapshot {
        phase: ControllerPhase::Ramp,
        parameters: TransferControlParameters {
            inflight_rpcs_per_connection: 1,
            ..TransferControlParameters::conservative_download()
        },
        goodput_bytes_per_second: 0,
        encryption_bytes_per_second: 0,
        disk_bytes_per_second: 0,
        round_trip_time_p95_millis: 0,
        estimated_bdp_bytes: 0,
        inflight_bytes: 0,
        target_inflight_bytes: 0,
        cpu_utilization_basis_points: 0,
        encrypted_queue_length: 0,
        network_waiting_for_encryption_millis: 0,
        encryption_waiting_for_network_millis: 0,
        bottleneck: TransferBottleneck::Unknown,
        parts: PartCounters::default(),
        queues: QueueCounters::default(),
        memory: MemoryCounters::default(),
        memory_budget_bytes: 512 * 1024 * 1024,
        lanes: Vec::new(),
        decisions: Vec::new(),
    }
}

fn validate_request(request: &ChannelDownloadRequest) -> Result<(), ApplicationError> {
    if request.account_id <= 0
        || request.chat_id <= 0
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
                mut snapshot,
                mut queued_at,
                control,
            } => {
                let id = snapshot.id;
                loop {
                    run_download(
                        &*backend,
                        DownloadWorkerState {
                            snapshots: &snapshots,
                            scheduled: &scheduled,
                        },
                        &shutdown,
                        &library,
                        snapshot,
                        queued_at,
                        Arc::clone(&control),
                    );
                    let Ok(mut scheduled) = scheduled.lock() else {
                        break;
                    };
                    if control.load(Ordering::Acquire) == CONTROL_RETRY_PENDING
                        && !shutdown.load(Ordering::Acquire)
                    {
                        // The previous backend has now released all request/file
                        // ownership. Use fresh progress for the replacement attempt.
                        if let Some(retry) = update_snapshot(&snapshots, id, |current| {
                            current.state = ChannelDownloadState::Queued;
                            current.transferred_bytes = 0;
                            current.telemetry = empty_download_telemetry();
                            current.verification = ChannelDownloadVerification::Pending;
                            current.finished_at_unix_ms = None;
                            current.failure = None;
                        }) {
                            let _ = persist_snapshot(&library, &retry);
                            control.store(CONTROL_RUNNING, Ordering::Release);
                            snapshot = retry;
                            queued_at = Instant::now();
                            continue;
                        }
                    }
                    scheduled.remove(&id);
                    break;
                }
            }
        }
    }
}

struct DownloadWorkerState<'a> {
    snapshots: &'a Arc<Mutex<Vec<ChannelDownloadSnapshot>>>,
    scheduled: &'a Mutex<BTreeSet<u64>>,
}

fn run_download(
    backend: &dyn ChannelDownloadBackend,
    state: DownloadWorkerState<'_>,
    shutdown: &Arc<AtomicBool>,
    library: &DesktopLibrary,
    snapshot: ChannelDownloadSnapshot,
    queued_at: Instant,
    control: Arc<AtomicU8>,
) {
    let snapshots = state.snapshots;
    let queue_wait_ms = elapsed_millis(queued_at.elapsed());
    let started_at_unix_ms = unix_time_millis().ok();
    let initial_control = control.load(Ordering::Acquire);
    let initial_state = if matches!(
        initial_control,
        CONTROL_RESUME_PENDING | CONTROL_RETRY_PENDING
    ) {
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
    let (controller, download_strategy) = match library.preferences().and_then(|preferences| {
        new_download_controller(
            preferences.transfer_soft_limit_policy,
            preferences.download_throughput_strategy,
        )
        .map(|controller| (controller, preferences.download_throughput_strategy))
    }) {
        Ok(controller) => controller,
        Err(error) => {
            fail_download(
                FailedDownloadOwner {
                    backend,
                    snapshots,
                    library,
                    snapshot: &snapshot,
                },
                error,
                queue_wait_ms,
                previous_duration_ms,
                started_at_unix_ms,
            );
            return;
        }
    };
    let mut session_log =
        match TransferSessionLog::create(library, TransferSessionKind::NativeDownload, snapshot.id)
        {
            Ok(log) => log,
            Err(error) => {
                fail_download(
                    FailedDownloadOwner {
                        backend,
                        snapshots,
                        library,
                        snapshot: &snapshot,
                    },
                    error,
                    queue_wait_ms,
                    previous_duration_ms,
                    started_at_unix_ms,
                );
                return;
            }
        };
    if let Err(error) = session_log.append_started(
        false,
        started_at_unix_ms.unwrap_or(snapshot.queued_at_unix_ms),
        snapshot.size_bytes,
        u32::try_from(snapshot.size_bytes.div_ceil(DOWNLOAD_PART_SIZE_BYTES)).unwrap_or(u32::MAX),
        &controller.snapshot(),
    ) {
        fail_download(
            FailedDownloadOwner {
                backend,
                snapshots,
                library,
                snapshot: &snapshot,
            },
            error,
            queue_wait_ms,
            previous_duration_ms,
            started_at_unix_ms,
        );
        return;
    }
    let session_log_path = Some(session_log.path.clone());
    let initial_telemetry = controller.snapshot();
    let _ = update_snapshot(snapshots, snapshot.id, |current| {
        current.session_log_path = session_log_path;
        current.telemetry = initial_telemetry;
    });
    let observer = Arc::new(RuntimeDownloadObserver {
        id: snapshot.id,
        size_bytes: snapshot.size_bytes,
        control: Arc::clone(&control),
        shutdown: Arc::clone(shutdown),
        snapshots: Arc::clone(snapshots),
        library: library.clone(),
        started,
        previous_duration_ms,
        sample: Mutex::new(ProgressSample {
            observations: VecDeque::from([(snapshot.transferred_bytes, started)]),
        }),
        last_persisted: Mutex::new(started),
        download_strategy,
        controller: Mutex::new(controller),
        session_log: Mutex::new(Some(session_log)),
    });
    let backend_observer: Arc<dyn DownloadObserver> = observer.clone();
    let result = backend.download(
        snapshot.account_id,
        snapshot.chat_id,
        snapshot.message_id,
        &snapshot.destination,
        backend_observer,
    );
    let duration_ms = previous_duration_ms.saturating_add(elapsed_millis(started.elapsed()));
    let finished_at_unix_ms = unix_time_millis().ok();
    observer.finish_session_log(
        duration_ms,
        result.as_ref().err().map(ApplicationError::kind),
    );
    // Serialize terminal publication with retry: an old cancellation must not
    // overwrite the queued replacement after the retry command has returned.
    let Ok(_retirement) = state.scheduled.lock() else {
        return;
    };
    match result {
        _ if control.load(Ordering::Acquire) == CONTROL_RETRY_PENDING => {
            // transfer_loop starts the replacement only after this owner exits.
        }
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
            FailedDownloadOwner {
                backend,
                snapshots,
                library,
                snapshot: &snapshot,
            },
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

struct FailedDownloadOwner<'a> {
    backend: &'a dyn ChannelDownloadBackend,
    snapshots: &'a Mutex<Vec<ChannelDownloadSnapshot>>,
    library: &'a DesktopLibrary,
    snapshot: &'a ChannelDownloadSnapshot,
}

fn fail_download(
    owner: FailedDownloadOwner<'_>,
    error: ApplicationError,
    queue_wait_ms: u64,
    duration_ms: u64,
    finished_at_unix_ms: Option<i64>,
) {
    let FailedDownloadOwner {
        backend,
        snapshots,
        library,
        snapshot,
    } = owner;
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
    if let Err(cleanup_error) = backend.cleanup_failed_partial(
        &snapshot.destination,
        snapshot.size_bytes,
        failure.retryable,
    ) {
        tracing::warn!(
            event = "transfer.download.failed_partial_cleanup_failed",
            task_id = snapshot.id,
            error_kind = ?cleanup_error.kind(),
            retain_for_resume = failure.retryable,
            "failed download partial cleanup could not be completed"
        );
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
        account_id: record.account_id,
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
        part_events: Vec::new(),
        session_log_path: None,
        telemetry: empty_download_telemetry(),
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
        account_id: snapshot.account_id,
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

fn remove_file_if_present(path: &Path) -> Result<(), ApplicationError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Err(
            ApplicationError::new(ApplicationErrorKind::PermissionDenied),
        ),
        Err(_) => Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
    }
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
    fn another_account_cannot_enqueue_resume_or_retry_prior_work() {
        let directory = tempfile::tempdir().expect("directory");
        let backend = Arc::new(FakeBackend {
            outcome: Err(ApplicationErrorKind::Network),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend.clone(), library(&directory)).expect("transfers");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("a.zip")))
            .expect("enqueue");
        wait_for_terminal(&transfers, id);
        transfers.suspend_account().expect("suspend");
        transfers.activate_account(2).expect("switch");
        assert_eq!(
            transfers.retry(id).expect_err("other account retry").kind(),
            ApplicationErrorKind::Authorization
        );
        assert_eq!(
            transfers
                .resume(id)
                .expect_err("other account resume")
                .kind(),
            ApplicationErrorKind::Authorization
        );
        assert_eq!(
            transfers
                .enqueue_channel_download(request(directory.path().join("b.zip")))
                .expect_err("wrong request account")
                .kind(),
            ApplicationErrorKind::Authorization
        );
        assert_eq!(backend.calls.lock().expect("calls").len(), 1);
        assert_eq!(
            transfers.snapshots().expect("history")[0].account_id,
            Some(1)
        );
    }

    #[test]
    fn native_profiles_change_real_parts_without_inventing_connections() {
        for (strategy, initial, maximum) in [
            (DownloadThroughputStrategy::Balanced, 4, 24),
            (DownloadThroughputStrategy::MaxThroughput, 4, 64),
        ] {
            let mut controller =
                new_download_controller(crate::SoftLimitPolicy::AdaptiveOverride, strategy)
                    .expect("valid test fixture");
            assert_eq!(controller.parameters().inflight_parts_per_file, initial);
            let mut reached_maximum = false;
            for step in 0..100 {
                controller.observe(PerformanceSample {
                    observed_at_millis: step * 2_000,
                    goodput_bytes_per_second: u64::from(
                        controller.parameters().inflight_parts_per_file,
                    ) * 1_000_000,
                    active_large_files: 1,
                    ..PerformanceSample::default()
                });
                let parts = controller.parameters().inflight_parts_per_file;
                assert!(parts <= maximum);
                reached_maximum |= parts == maximum;
            }
            assert!(reached_maximum);
            let parameters = controller.parameters();
            assert_eq!(parameters.transfer_connection_count, 1);
            assert_eq!(parameters.inflight_rpcs_per_connection, 1);
            assert_eq!(parameters.active_file_count, 1);
        }
    }

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

    #[test]
    fn part_counters_use_each_parts_latest_state() {
        let events = [
            ChannelDownloadPartEvent {
                part_index: 0,
                offset_bytes: 0,
                length_bytes: DOWNLOAD_PART_SIZE_BYTES,
                state: DownloadPartState::Inflight,
                attempt: 1,
                elapsed_millis: 0,
            },
            ChannelDownloadPartEvent {
                part_index: 0,
                offset_bytes: 0,
                length_bytes: DOWNLOAD_PART_SIZE_BYTES,
                state: DownloadPartState::Completed,
                attempt: 1,
                elapsed_millis: 25,
            },
            ChannelDownloadPartEvent {
                part_index: 1,
                offset_bytes: DOWNLOAD_PART_SIZE_BYTES,
                length_bytes: DOWNLOAD_PART_SIZE_BYTES,
                state: DownloadPartState::Failed,
                attempt: 1,
                elapsed_millis: 40,
            },
        ];

        let counters = current_part_counters(&events, 3, 1_500);

        assert_eq!(counters.completed_parts, 1);
        assert_eq!(counters.inflight_parts, 0);
        assert_eq!(counters.failed_parts, 1);
        assert_eq!(counters.missing_parts, 2);
        assert_eq!(counters.completed_parts_per_second_milli, 1_500);
    }

    fn test_transfers(
        backend: Arc<dyn ChannelDownloadBackend>,
        library: DesktopLibrary,
    ) -> Result<DesktopTransfers, ApplicationError> {
        let transfers = DesktopTransfers::with_backend(backend, library)?;
        transfers.activate_account(1)?;
        Ok(transfers)
    }

    struct FakeBackend {
        outcome: Result<(), ApplicationErrorKind>,
        calls: StdMutex<Vec<(i64, i64, PathBuf)>>,
    }

    impl ChannelDownloadBackend for FakeBackend {
        fn download(
            &self,
            _account_id: Option<i64>,
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
            account_id: 1,
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
        let transfers = test_transfers(backend.clone(), library.clone()).expect("worker");
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
        let restored = test_transfers(backend, library)
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
        let transfers = test_transfers(backend.clone(), library.clone()).expect("download worker");
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

        let restored = test_transfers(backend, library)
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
        let transfers = test_transfers(backend.clone(), library.clone()).expect("worker");
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
        let restored = test_transfers(backend, library)
            .expect("restored worker")
            .snapshots()
            .expect("restored snapshots");
        assert_eq!(
            restored[0].state,
            ChannelDownloadState::Failed(ApplicationErrorKind::Network)
        );
    }

    #[test]
    fn deleting_a_terminal_task_removes_history_and_log_but_preserves_output() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let destination = directory.path().join("archive.zip");
        let library = library(&directory);
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend.clone(), library.clone()).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("enqueue");
        let completed = wait_for_terminal(&transfers, id);
        let session_log_path = completed.session_log_path.expect("session log path");
        assert!(session_log_path.is_file());

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match transfers.delete(id) {
                Ok(()) => break,
                Err(error) if error.kind() == ApplicationErrorKind::Conflict => {
                    assert!(Instant::now() < deadline, "worker did not release task");
                    thread::yield_now();
                }
                Err(error) => panic!("unexpected delete failure: {error}"),
            }
        }
        assert!(transfers.snapshots().expect("snapshots").is_empty());
        assert!(
            destination.is_file(),
            "completed user file must be preserved"
        );
        assert!(!session_log_path.exists());
        drop(transfers);
        assert!(
            test_transfers(backend, library)
                .expect("restored worker")
                .snapshots()
                .expect("restored snapshots")
                .is_empty()
        );
    }

    #[test]
    fn failed_download_cleanup_retains_only_retryable_recovery_data() {
        struct CleanupRecordingBackend {
            failure_kind: ApplicationErrorKind,
            cleanup_calls: StdMutex<Vec<bool>>,
        }
        impl ChannelDownloadBackend for CleanupRecordingBackend {
            fn download(
                &self,
                _account_id: Option<i64>,
                _chat_id: i64,
                _message_id: i64,
                _destination: &Path,
                _observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                Err(ApplicationError::new(self.failure_kind))
            }

            fn cleanup_failed_partial(
                &self,
                _destination: &Path,
                _expected_bytes: u64,
                retain_for_resume: bool,
            ) -> Result<(), ApplicationError> {
                self.cleanup_calls
                    .lock()
                    .expect("cleanup calls")
                    .push(retain_for_resume);
                Ok(())
            }
        }

        for (failure_kind, expected_retention) in [
            (ApplicationErrorKind::Network, true),
            (ApplicationErrorKind::PermissionDenied, false),
        ] {
            let directory = tempfile::tempdir().expect("temporary directory");
            let backend = Arc::new(CleanupRecordingBackend {
                failure_kind,
                cleanup_calls: StdMutex::new(Vec::new()),
            });
            let transfers = test_transfers(backend.clone(), library(&directory)).expect("worker");
            let id = transfers
                .enqueue_channel_download(request(directory.path().join("failure.bin")))
                .expect("enqueue");
            wait_for_terminal(&transfers, id);
            let deadline = Instant::now() + Duration::from_secs(1);
            while backend
                .cleanup_calls
                .lock()
                .expect("cleanup calls")
                .is_empty()
            {
                assert!(Instant::now() < deadline, "cleanup was not observed");
                thread::yield_now();
            }
            assert_eq!(
                backend
                    .cleanup_calls
                    .lock()
                    .expect("cleanup calls")
                    .as_slice(),
                &[expected_retention]
            );
        }
    }

    #[test]
    fn downloads_from_different_channels_keep_their_source_identity() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend.clone(), library(&directory)).expect("worker");
        let first = transfers
            .enqueue_channel_download(request(directory.path().join("first.zip")))
            .expect("first enqueue");
        let second = transfers
            .enqueue_channel_download(ChannelDownloadRequest {
                account_id: 1,
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
                _account_id: Option<i64>,
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
        let transfers = test_transfers(backend.clone(), library(&directory)).expect("worker");
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
        transfers.retry(cancelled_id).expect("retry");
        assert_eq!(
            wait_for_terminal(&transfers, cancelled_id).state,
            ChannelDownloadState::Completed
        );
        assert!(backend.attempts.load(AtomicOrdering::Relaxed) >= 2);
    }

    #[test]
    fn immediate_cancel_retry_waits_for_old_attempt_to_release_ownership() {
        struct RetryBackend {
            attempts: AtomicUsize,
            started: mpsc::SyncSender<()>,
            release: StdMutex<mpsc::Receiver<()>>,
        }
        impl ChannelDownloadBackend for RetryBackend {
            fn download(
                &self,
                _account_id: Option<i64>,
                _chat_id: i64,
                _message_id: i64,
                _destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                if self.attempts.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                    observer.progressed(7);
                    self.started.send(()).expect("started");
                    self.release
                        .lock()
                        .expect("release lock")
                        .recv_timeout(Duration::from_secs(3))
                        .expect("release");
                    assert_eq!(observer.control(), DownloadControl::Cancel);
                    return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                }
                assert_eq!(observer.control(), DownloadControl::Continue);
                observer.progressed(14);
                Ok(())
            }
        }
        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let backend = Arc::new(RetryBackend {
            attempts: AtomicUsize::new(0),
            started: started_tx,
            release: StdMutex::new(release_rx),
        });
        let transfers = test_transfers(backend.clone(), library.clone()).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("retry.zip")))
            .expect("enqueue");
        started_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("started");
        transfers.cancel(id).expect("cancel");
        transfers.retry(id).expect("immediate retry");
        let queued = transfers.snapshots().expect("snapshots").remove(0);
        assert_eq!(queued.state, ChannelDownloadState::Queued);
        assert_eq!(queued.transferred_bytes, 0);
        assert_eq!(backend.attempts.load(AtomicOrdering::SeqCst), 1);
        release_tx.send(()).expect("release");
        let completed = wait_for_terminal(&transfers, id);
        assert_eq!(completed.state, ChannelDownloadState::Completed);
        assert_eq!(completed.attempts, 2);
        assert_eq!(backend.attempts.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(
            transfers
                .retry(id)
                .expect_err("completed cannot retry")
                .kind(),
            ApplicationErrorKind::Conflict
        );
    }

    #[test]
    fn restored_cancelled_download_can_retry_with_the_same_identity() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let backend = Arc::new(FakeBackend {
            outcome: Err(ApplicationErrorKind::Network),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend, library.clone()).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("restore.zip")))
            .expect("enqueue");
        wait_for_terminal(&transfers, id);
        let mut cancelled = transfers.snapshots().expect("snapshots").remove(0);
        cancelled.state = ChannelDownloadState::Cancelled;
        cancelled.failure = None;
        cancelled.transferred_bytes = 7;
        drop(transfers);
        persist_snapshot(&library, &cancelled).expect("persist cancelled fixture");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let restored = test_transfers(backend.clone(), library).expect("restore");
        assert_eq!(
            restored.snapshot_state(id).expect("state"),
            ChannelDownloadState::Cancelled
        );
        restored.retry(id).expect("retry restored cancellation");
        let completed = wait_for_terminal(&restored, id);
        assert_eq!(completed.state, ChannelDownloadState::Completed);
        assert_eq!(completed.id, id);
        assert_eq!(completed.destination, cancelled.destination);
        assert_eq!(backend.calls.lock().expect("calls").len(), 1);
    }

    #[test]
    fn interrupted_active_download_restores_its_checkpoint_after_restart() {
        struct ShutdownAwareBackend;
        impl ChannelDownloadBackend for ShutdownAwareBackend {
            fn download(
                &self,
                _account_id: Option<i64>,
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
            test_transfers(Arc::new(ShutdownAwareBackend), library.clone()).expect("worker");
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
        let restored = test_transfers(backend, library).expect("restored worker");
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
                _account_id: Option<i64>,
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
        let transfers = test_transfers(
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
                _account_id: Option<i64>,
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
        let transfers = test_transfers(
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
            account_id: 1,
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
