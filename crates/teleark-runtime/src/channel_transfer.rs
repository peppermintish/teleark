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
use teleark_telegram::{
    DOWNLOAD_PART_SIZE_BYTES, DownloadControl, DownloadObserver, DownloadPartEvent,
    DownloadPartFailureKind, DownloadPartState,
};
use teleark_transfer::{
    AdaptiveControllerConfig, AdaptiveTransferController, ControllerPhase, MemoryCounters,
    PartCounters, PerformanceSample, QueueCounters, TransferBottleneck, TransferControlParameters,
    TransferTelemetrySnapshot,
};

use crate::{
    DesktopLibrary, DesktopTelegram,
    transfer_updates::{
        TransferRecord, TransferSnapshotView, TransferSnapshots, TransferSubscription,
    },
    vault::{TransferSessionKind, TransferSessionLog},
};

mod batch_retry;
mod filter_batch;
pub use filter_batch::{
    ChannelBatchFilter, ChannelBatchPreparation, ChannelBatchPreparationPhase,
    ChannelBatchPreparationSnapshot, FilteredChannelBatch,
};
mod cleanup;
mod reservation;
pub use cleanup::{ChannelDownloadCleanup, ChannelDownloadCleanupPhase};
mod history_retention;
use history_retention::{HistoryRetention, bound_lifecycle, compact_idle_replays};
mod part_history;
pub use part_history::PartEventHistory;

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
    available_download_destination_with_reservations(directory, suggested_file_name, |_| Ok(false))
}

pub(crate) fn available_download_destination_with_reservations(
    directory: &Path,
    suggested_file_name: &str,
    reserved: impl FnMut(&Path) -> Result<bool, ApplicationError>,
) -> Result<PathBuf, ApplicationError> {
    available_download_destination_with_claim(directory, suggested_file_name, reserved, |_| {
        Ok(true)
    })
}

pub(crate) fn available_download_destination_with_claim(
    directory: &Path,
    suggested_file_name: &str,
    mut reserved: impl FnMut(&Path) -> Result<bool, ApplicationError>,
    mut claim: impl FnMut(&Path) -> Result<bool, ApplicationError>,
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
            Ok(false) if !reserved(&candidate)? => {
                // Retained encrypted downloads occupy their destination even
                // before final publication. Never reuse another task's partial.
                let mut partial_name = candidate
                    .file_name()
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?
                    .to_os_string();
                partial_name.push(".partial");
                match std::fs::symlink_metadata(candidate.with_file_name(partial_name)) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        if claim(&candidate)? {
                            return Ok(candidate);
                        }
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                        return Err(ApplicationError::new(
                            ApplicationErrorKind::PermissionDenied,
                        ));
                    }
                    Err(_) => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
                }
            }
            Ok(_) => {}
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
    pub part: Option<ChannelDownloadPartFailure>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelDownloadPartFailure {
    pub part_index: u64,
    pub attempt: u32,
    pub elapsed_millis: u64,
    pub connection_slot: Option<u16>,
    pub kind: Option<DownloadPartFailureKind>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelDownloadEventKind {
    CleanupWaiting,
    CleanupRemoving,
    CleanupFailed,
    CleanupFinished,
    CleanupRetryRequested,
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
    pub task_attempt: u32,
    pub part_index: u64,
    pub offset_bytes: u64,
    pub length_bytes: u64,
    pub state: DownloadPartState,
    pub attempt: u32,
    pub elapsed_millis: u64,
    pub connection_slot: u16,
    pub failure: Option<DownloadPartFailureKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelDownloadSnapshot {
    pub cleanup: Option<ChannelDownloadCleanup>,
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
    /// Live RPC acknowledgements in this attempt; never persisted or restored as speed.
    pub acknowledged_bytes: u64,
    pub current_bytes_per_second: Option<u64>,
    pub average_bytes_per_second: Option<u64>,
    pub eta_ms: Option<u64>,
    pub attempts: u32,
    pub failure: Option<ChannelDownloadFailure>,
    pub events: Vec<ChannelDownloadEvent>,
    pub event_history_omitted: u64,
    pub part_events: PartEventHistory,
    pub session_log_path: Option<PathBuf>,
    pub telemetry: TransferTelemetrySnapshot,
    pub server_status: Option<teleark_telegram::TransferServerStatus>,
}

impl ChannelDownloadSnapshot {
    pub fn server_status(&self) -> Option<&teleark_telegram::TransferServerStatus> {
        self.server_status.as_ref().filter(|s| s.is_active())
    }
}

impl TransferRecord for ChannelDownloadSnapshot {
    fn rate_input(&self) -> crate::transfer_rate::RateInput {
        crate::transfer_rate::RateInput {
            account: self.account_id.unwrap_or_default(),
            upload: false,
            active: self.state == ChannelDownloadState::Running,
            bytes: self.acknowledged_bytes,
            logical_bytes: self.transferred_bytes,
            total: self.size_bytes,
            in_flight: self.state == ChannelDownloadState::Running,
        }
    }

    fn sample_activity(&mut self, _now: Instant, rate: crate::TransferRate) -> bool {
        let changed = self.current_bytes_per_second != rate.bytes_per_second
            || self.eta_ms != rate.eta_millis;
        self.current_bytes_per_second = rate.bytes_per_second;
        self.eta_ms = rate.eta_millis;
        changed
    }
    type Phase = (
        ChannelDownloadState,
        ChannelDownloadVerification,
        Option<i64>,
        Option<ChannelDownloadCleanupPhase>,
    );
    fn id(&self) -> u64 {
        self.id
    }
    fn phase(&self) -> Self::Phase {
        (
            self.state,
            self.verification,
            self.account_id,
            self.cleanup.map(|cleanup| cleanup.phase),
        )
    }
}

#[derive(Clone)]
pub struct DesktopTransfers {
    inner: Arc<TransferWorkerInner>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TransferRates {
    pub download_bytes_per_second: u64,
    pub upload_bytes_per_second: u64,
    pub uploads_sampling: u32,
    pub downloads_sampling: u32,
}

struct TransferWorkerInner {
    active_account: Arc<std::sync::atomic::AtomicI64>,
    sender: Arc<Mutex<Option<mpsc::SyncSender<TransferCommand>>>>,
    snapshots: Arc<TransferSnapshots<ChannelDownloadSnapshot>>,
    controls: Arc<Mutex<BTreeMap<u64, Arc<AtomicU8>>>>,
    scheduled: Arc<Mutex<BTreeSet<u64>>>,
    shutdown: Arc<AtomicBool>,
    library: DesktopLibrary,
    backend: Arc<dyn ChannelDownloadBackend>,
    join: Mutex<Option<JoinHandle<()>>>,
    pump: Arc<QueuePump>,
    retention: HistoryRetention,
    cleanup_owner: cleanup::CleanupOwner,
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

    fn release_completed_reservation(
        &self,
        destination: &Path,
        expected_bytes: u64,
    ) -> Result<(), ApplicationError> {
        reservation::release_completed(destination, expected_bytes)
    }

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

struct NativeReceipts {
    id: u64,
    account: Option<i64>,
    attempt: u32,
    snapshots: Arc<TransferSnapshots<ChannelDownloadSnapshot>>,
    // Only unfinished ranges; a completed range cannot be scheduled again by this owner.
    state: Mutex<(u64, BTreeMap<u64, u64>)>,
}
impl teleark_telegram::DownloadReceiptObserver for NativeReceipts {
    fn acknowledged(&self, part: u64, bytes: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let previous = state.1.entry(part).or_default();
        let delta = bytes.saturating_sub(*previous);
        *previous = (*previous).max(bytes);
        let baseline = state.0;
        self.snapshots.update(self.id, |snapshot| {
            if snapshot.state != ChannelDownloadState::Running
                || snapshot.attempts != self.attempt
                || snapshot.account_id != self.account
            {
                return;
            }
            snapshot.acknowledged_bytes = snapshot.acknowledged_bytes.saturating_add(delta);
            snapshot.transferred_bytes = snapshot
                .transferred_bytes
                .max(baseline.saturating_add(snapshot.acknowledged_bytes))
                .min(snapshot.size_bytes);
        });
    }
    fn completed(&self, part: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.1.remove(&part);
        }
    }
}

struct RuntimeDownloadObserver {
    id: u64,
    size_bytes: u64,
    control: Arc<AtomicU8>,
    shutdown: Arc<AtomicBool>,
    snapshots: Arc<TransferSnapshots<ChannelDownloadSnapshot>>,
    library: DesktopLibrary,
    started: Instant,
    previous_duration_ms: u64,
    receipts: Arc<NativeReceipts>,
    last_persisted: Mutex<Instant>,
    controller: Mutex<AdaptiveTransferController>,
    tuning: teleark_telegram::TransferTuning,
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

    fn receipt_observer(&self) -> Option<Arc<dyn teleark_telegram::DownloadReceiptObserver>> {
        Some(self.receipts.clone())
    }

    fn progressed(&self, transferred_bytes: u64) {
        let now = Instant::now();
        if let Ok(mut state) = self.receipts.state.lock()
            && state.1.is_empty()
        {
            // Initial checkpoint baseline precedes any network requests.
            if self
                .snapshots
                .read(self.id, |row| row.acknowledged_bytes == 0)
                .unwrap_or(false)
            {
                state.0 = transferred_bytes;
            }
        }
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
        let snapshot = mutate_snapshot(&self.snapshots, self.id, |snapshot| {
            if snapshot.state != ChannelDownloadState::Running
                || snapshot.attempts != self.receipts.attempt
            {
                return None;
            }
            snapshot.transferred_bytes = snapshot
                .transferred_bytes
                .max(transferred_bytes.min(snapshot.size_bytes));
            snapshot.duration_ms = Some(
                self.previous_duration_ms
                    .saturating_add(elapsed_millis(self.started.elapsed())),
            );
            should_persist.then(|| snapshot.clone())
        })
        .flatten();
        if let Some(snapshot) = snapshot {
            let queued = snapshot_record(&snapshot)
                .is_ok_and(|record| self.library.try_save_native_download_progress(record));
            if !queued {
                tracing::warn!(
                    event = "transfer.download.checkpoint_deferred",
                    task_id = self.id,
                    "storage queue unavailable; live progress retained and final state uses acknowledged persistence"
                );
            }
        }
    }

    fn max_part_attempts(&self) -> u32 {
        u32::from(self.tuning.download_attempts)
    }
    fn desired_connections(&self) -> u16 {
        self.tuning.download_connections
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

    fn server_throttled(&self, code: i32, wait_seconds: u32) {
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let wait_until_unix_ms = now_unix + i64::from(wait_seconds) * 1000;
        let status = teleark_telegram::TransferServerStatus {
            code,
            flag: format!("FLOOD_WAIT_{wait_seconds}"),
            wait_until_unix_ms,
        };
        let _ = mutate_snapshot(&self.snapshots, self.id, |snapshot| {
            snapshot.server_status = Some(status);
        });
    }

    fn part_event(&self, event: DownloadPartEvent) {
        self.process_part_event(event, None);
    }
}

impl RuntimeDownloadObserver {
    fn process_part_event(&self, event: DownloadPartEvent, server_wait: Option<Duration>) {
        let part_event = ChannelDownloadPartEvent {
            task_attempt: self.receipts.attempt,
            part_index: event.part_index,
            offset_bytes: event.offset_bytes,
            length_bytes: event.length_bytes,
            state: event.state,
            attempt: event.attempt,
            elapsed_millis: event.elapsed_millis,
            connection_slot: event.connection_slot,
            failure: event.failure,
        };
        let snapshot = mutate_snapshot(&self.snapshots, self.id, |snapshot| {
            if let Some(wait) = server_wait {
                let wait_seconds = wait.as_secs().min(u32::MAX as u64) as u32;
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                let wait_until_unix_ms = now_unix + i64::from(wait_seconds) * 1000;
                snapshot.server_status = Some(teleark_telegram::TransferServerStatus {
                    code: 420,
                    flag: format!("FLOOD_WAIT_{wait_seconds}"),
                    wait_until_unix_ms,
                });
            } else if event.state == DownloadPartState::Completed
                && snapshot
                    .server_status
                    .as_ref()
                    .is_some_and(|s| !s.is_active())
            {
                snapshot.server_status = None;
            }
            snapshot.part_events.push(part_event);
            snapshot.telemetry.parts = snapshot.part_events.counters(
                snapshot.size_bytes.div_ceil(DOWNLOAD_PART_SIZE_BYTES),
                snapshot
                    .transferred_bytes
                    .div_ceil(DOWNLOAD_PART_SIZE_BYTES),
                0,
            );
            (snapshot.current_bytes_per_second, snapshot.telemetry.parts)
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
                .and_then(|(speed, _)| *speed)
                .unwrap_or_default();
            let part_counters = snapshot
                .as_ref()
                .map(|(_, parts)| *parts)
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
                let _ = mutate_snapshot(&self.snapshots, self.id, |snapshot| {
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
            .and_then(|(speed, _)| *speed)
            .unwrap_or_default();
        let completed_parts = snapshot
            .as_ref()
            .map_or(0, |(_, parts)| parts.completed_parts);
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
            |(_, parts)| PartCounters {
                completed_parts_per_second_milli,
                ..*parts
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
        let _ = mutate_snapshot(&self.snapshots, self.id, |snapshot| {
            snapshot.telemetry = telemetry;
        });
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
        let (restored, omitted) = library.native_download_history()?;
        let retention = HistoryRetention::new(
            restored.len(),
            omitted,
            teleark_storage::NATIVE_DOWNLOAD_HISTORY_LIMIT,
        );
        let transfer_log_directory = library.managed_directories()?.logs.join("Transfers");
        let mut snapshots = Vec::with_capacity(restored.len());
        let mut completed_reservations = Vec::new();
        let mut controls = BTreeMap::new();
        for record in restored {
            let mut snapshot = snapshot_from_record(record);
            if snapshot.state == ChannelDownloadState::Completed
                && completed_reservations.len() < teleark_storage::NATIVE_DOWNLOAD_HISTORY_LIMIT
            {
                completed_reservations.push((
                    snapshot.id,
                    snapshot.destination.clone(),
                    snapshot.size_bytes,
                ));
            }
            let session_log_path =
                transfer_log_directory.join(format!("native-download-{}.jsonl", snapshot.id));
            if session_log_path.is_file() {
                if let Some(failure) = snapshot.failure.as_mut() {
                    failure.part = last_failed_part_from_log(&session_log_path);
                }
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
        let snapshots = Arc::new(TransferSnapshots::new(snapshots)?);
        let controls = Arc::new(Mutex::new(controls));
        let scheduled = Arc::new(Mutex::new(BTreeSet::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let active_account = Arc::new(std::sync::atomic::AtomicI64::new(0));
        let sender = Arc::new(Mutex::new(Some(sender)));
        let pump = Arc::new(QueuePump {
            active_account: active_account.clone(),
            sender: sender.clone(),
            snapshots: snapshots.clone(),
            controls: controls.clone(),
            scheduled: scheduled.clone(),
            shutdown: shutdown.clone(),
            cancel_cleanup: Mutex::new(BTreeMap::new()),
            cleanup_wake: cleanup::CleanupSignal::default(),
        });
        let worker_pump = pump.clone();
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
                    worker_pump,
                );
            })
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?;
        let cleanup_owner = cleanup::CleanupOwner::start(cleanup::CleanupContext {
            completed_reservations,
            backend: backend.clone(),
            snapshots: snapshots.clone(),
            controls: controls.clone(),
            scheduled: scheduled.clone(),
            shutdown: shutdown.clone(),
            library: library.clone(),
            pump: pump.clone(),
        })?;
        Ok(Self {
            inner: Arc::new(TransferWorkerInner {
                active_account,
                sender,
                snapshots,
                controls,
                scheduled,
                shutdown,
                library,
                backend,
                join: Mutex::new(Some(join)),
                pump,
                retention,
                cleanup_owner,
            }),
        })
    }

    pub fn enqueue_channel_download(
        &self,
        request: ChannelDownloadRequest,
    ) -> Result<u64, ApplicationError> {
        validate_request(&request)?;
        self.require_active_account(Some(request.account_id))?;
        let conflict = self
            .inner
            .snapshots
            .fold(false, |found, snapshot| {
                found
                    || (snapshot.destination == request.destination
                        && matches!(
                            snapshot.state,
                            ChannelDownloadState::Queued
                                | ChannelDownloadState::Running
                                | ChannelDownloadState::Paused
                        ))
            })
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        if conflict {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let reservation = self.inner.retention.reserve(
            1,
            &self.inner.snapshots,
            &self.inner.controls,
            &self.inner.scheduled,
        )?;
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
        reservation.commit();
        let snapshot = snapshot_from_record(stored);
        let id = snapshot.id;
        let control = Arc::new(AtomicU8::new(CONTROL_RUNNING));
        if !self.inner.snapshots.insert(snapshot.clone()) {
            return Err(ApplicationError::new(ApplicationErrorKind::Persistence));
        }
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

    /// Creates a new attempt from historical source metadata, allocating a new
    /// destination. The completed record and any existing file remain intact.
    pub fn redownload_completed(&self, task_id: u64) -> Result<u64, ApplicationError> {
        let snapshot = match self.inner.snapshots.get(task_id) {
            Some(snapshot) => snapshot,
            None => self
                .inner
                .library
                .native_download(task_id)?
                .map(snapshot_from_record)
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?,
        };
        self.require_active_account(snapshot.account_id)?;
        if snapshot.state != ChannelDownloadState::Completed {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let account_id = snapshot
            .account_id
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        let destination = self
            .inner
            .library
            .next_download_destination(&snapshot.file_name)?;
        self.enqueue_channel_download(ChannelDownloadRequest {
            account_id,
            chat_id: snapshot.chat_id,
            message_id: snapshot.message_id,
            message_sent_at_unix_ms: snapshot.message_sent_at_unix_ms,
            file_name: snapshot.file_name,
            caption: snapshot.caption,
            mime_type: snapshot.mime_type,
            size_bytes: snapshot.size_bytes,
            destination,
        })
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
        let existing = self
            .inner
            .snapshots
            .fold(BTreeSet::new(), |mut paths, snapshot| {
                if matches!(
                    snapshot.state,
                    ChannelDownloadState::Queued
                        | ChannelDownloadState::Running
                        | ChannelDownloadState::Paused
                ) {
                    paths.insert(snapshot.destination.clone());
                }
                paths
            })
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let mut destinations = BTreeSet::new();
        for request in &requests {
            if !destinations.insert(request.destination.clone())
                || existing.contains(&request.destination)
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
            }
        }
        let reservation = self.inner.retention.reserve(
            requests.len(),
            &self.inner.snapshots,
            &self.inner.controls,
            &self.inner.scheduled,
        )?;
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
        reservation.commit();
        for record in records {
            let snapshot = snapshot_from_record(record);
            let control = Arc::new(AtomicU8::new(CONTROL_RUNNING));
            if !self.inner.snapshots.insert(snapshot.clone()) {
                return Err(ApplicationError::new(ApplicationErrorKind::Persistence));
            }
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
        self.inner.pump.refill()
    }

    pub fn pause(&self, id: u64) -> Result<(), ApplicationError> {
        if !matches!(
            self.snapshot_state(id)?,
            ChannelDownloadState::Queued
                | ChannelDownloadState::Running
                | ChannelDownloadState::Paused
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
        // Ownership may have ended while the resume checkpoint was waiting.
        // Always retry admission after acknowledgement; schedule deduplicates
        // against a worker which still owns the task.
        self.try_schedule(snapshot, self.control(id)?)?;
        Ok(())
    }

    pub fn cancel(&self, id: u64) -> Result<(), ApplicationError> {
        self.require_task_account(id)?;
        self.inner.cleanup_owner.cancel(id)
    }

    /// Stop a selection as one admission decision. Returns after stop intent is
    /// durable; per-task cleanup remains visible until all writers and files drain.
    /// Background-only: callers must not wait for persistence on a UI reactor.
    pub fn stop(&self, ids: &[u64]) -> Result<(), ApplicationError> {
        if ids.len() > teleark_storage::NATIVE_DOWNLOAD_HISTORY_LIMIT {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        let ids = ids.iter().copied().collect::<BTreeSet<_>>();
        self.inner.cleanup_owner.stop(ids.into_iter().collect())
    }

    pub fn retry(&self, id: u64) -> Result<(), ApplicationError> {
        self.require_task_account(id)?;
        if self.inner.cleanup_owner.retry(id)? {
            return Ok(());
        }
        // Serialize with worker retirement so a retry cannot revive an attempt
        // that still owns the cancelled partial and outstanding requests.
        let mut scheduled = self
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
            // The retained worker persists the replacement after its old write
            // and partial cleanup finish. Never race those writes from this caller.
            return Ok(());
        }
        control.store(CONTROL_RUNNING, Ordering::Release);
        // Reserve this identity while persisting, so a concurrent refill cannot
        // start the new attempt before its queued checkpoint is acknowledged.
        scheduled.insert(id);
        drop(scheduled);
        let persisted = persist_snapshot(&self.inner.library, &snapshot);
        self.inner
            .scheduled
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .remove(&id);
        self.inner.pump.cleanup_wake.notify();
        persisted?;
        self.try_schedule(snapshot, control)
    }

    /// Deletes one terminal task's history and TeleArk-owned recovery data.
    /// A successfully downloaded destination is user data and is never removed.
    pub fn delete(&self, id: u64) -> Result<(), ApplicationError> {
        let snapshot = self
            .inner
            .snapshots
            .get(id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        if snapshot.cleanup.is_some()
            || !is_terminal(snapshot.state)
            || self
                .inner
                .scheduled
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .contains(&id)
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }

        if self
            .inner
            .pump
            .cancel_cleanup
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .contains_key(&id)
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        self.inner.library.delete_native_download(id)?;
        if self.inner.snapshots.remove(id) {
            self.inner.retention.removed();
        }
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
            .all()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))
    }

    pub fn subscribe(&self) -> TransferSubscription {
        self.inner.snapshots.subscribe()
    }

    /// Immutable presentation view; unchanged records and idle views share their allocations.
    pub fn snapshot_view(
        &self,
    ) -> Result<TransferSnapshotView<ChannelDownloadSnapshot>, ApplicationError> {
        let mut view = self
            .inner
            .snapshots
            .view()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        view.omitted_items = self.inner.retention.omitted();
        Ok(view)
    }

    pub fn snapshot(&self, id: u64) -> Option<ChannelDownloadSnapshot> {
        self.inner.snapshots.get(id)
    }

    pub fn current_rates(&self) -> Result<TransferRates, ApplicationError> {
        self.inner
            .snapshots
            .current_rates()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))
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
        self.inner.pump.refill()
    }

    /// Background-only graceful exit. The current lifecycle caller decides whether
    /// to reopen admission on failure; a stale exit must not undo session loss.
    pub fn pause_for_shutdown(&self) -> Result<(), ApplicationError> {
        self.suspend_account()
    }

    /// Reopen admission after an abandoned exit; saved pause intent is unchanged.
    pub fn abandon_shutdown(&self, account: i64) {
        self.inner.active_account.store(account, Ordering::Release);
    }

    /// Pause active work and wait for its retained workers to close partial files before logout.
    pub fn suspend_account(&self) -> Result<(), ApplicationError> {
        self.inner.active_account.store(0, Ordering::Release);
        let active = self
            .inner
            .snapshots
            .fold(Vec::new(), |mut ids, snapshot| {
                if matches!(
                    snapshot.state,
                    ChannelDownloadState::Queued
                        | ChannelDownloadState::Running
                        | ChannelDownloadState::Paused
                ) {
                    ids.push(snapshot.id);
                }
                ids
            })
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        for id in active {
            if let Err(error) = self.pause(id)
                && (error.kind() != ApplicationErrorKind::Conflict
                    || !is_terminal(self.snapshot_state(id)?))
            {
                return Err(error);
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
        let account = self
            .inner
            .snapshots
            .read(id, |snapshot| snapshot.account_id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        self.require_active_account(account)
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
            .read(id, |snapshot| snapshot.state)
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
        self.inner.pump.schedule(snapshot, control)
    }
}

struct QueuePump {
    active_account: Arc<std::sync::atomic::AtomicI64>,
    sender: Arc<Mutex<Option<mpsc::SyncSender<TransferCommand>>>>,
    snapshots: Arc<TransferSnapshots<ChannelDownloadSnapshot>>,
    controls: Arc<Mutex<BTreeMap<u64, Arc<AtomicU8>>>>,
    scheduled: Arc<Mutex<BTreeSet<u64>>>,
    shutdown: Arc<AtomicBool>,
    cancel_cleanup: Mutex<BTreeMap<u64, bool>>,
    cleanup_wake: cleanup::CleanupSignal,
}

impl QueuePump {
    fn refill(&self) -> Result<(), ApplicationError> {
        let account = self.active_account.load(Ordering::Acquire);
        if account <= 0 || self.shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        let scheduled = self
            .scheduled
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .clone();
        let slots = CHANNEL_DOWNLOAD_QUEUE_CAPACITY.saturating_sub(scheduled.len());
        if slots == 0 {
            return Ok(());
        }
        let pending = self
            .snapshots
            .select(
                |snapshot| {
                    snapshot.state == ChannelDownloadState::Queued
                        && snapshot.cleanup.is_none()
                        && snapshot.account_id == Some(account)
                        && !scheduled.contains(&snapshot.id)
                },
                slots,
            )
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        for snapshot in pending {
            if self.active_account.load(Ordering::Acquire) != account {
                break;
            }
            let control = self
                .controls
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .entry(snapshot.id)
                .or_insert_with(|| Arc::new(AtomicU8::new(CONTROL_RUNNING)))
                .clone();
            self.schedule(snapshot, control)?;
        }
        Ok(())
    }

    fn schedule(
        &self,
        snapshot: ChannelDownloadSnapshot,
        control: Arc<AtomicU8>,
    ) -> Result<(), ApplicationError> {
        let mut scheduled = self
            .scheduled
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        if self
            .cancel_cleanup
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .contains_key(&snapshot.id)
        {
            return Ok(());
        }
        // A Queued projection can precede the durable resume acknowledgement.
        // Do not reopen the partial while its control token still says paused.
        if self
            .snapshots
            .read(snapshot.id, |row| row.cleanup.is_some())
            .unwrap_or(true)
            || matches!(
                control.load(Ordering::Acquire),
                CONTROL_PAUSED | CONTROL_CANCELLED
            )
            || self.snapshots.read(snapshot.id, |row| row.state)
                != Some(ChannelDownloadState::Queued)
        {
            return Ok(());
        }
        if !scheduled.insert(snapshot.id) {
            return Ok(());
        }
        let send_result = self
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
            drop(scheduled);
            self.cleanup_wake.notify();
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
        self.pump.cleanup_wake.notify();
        // Do not wait for Telegram or SQLite from a frontend destructor. Submit
        // the latest sample without copying replay histories; a saturated queue
        // retains the previous checkpoint and the independently saved part map.
        // The worker owns its state and performs acknowledged retirement later.
        let active_id = self
            .snapshots
            .fold(None, |found, snapshot| {
                found.or_else(|| {
                    (snapshot.state == ChannelDownloadState::Running).then_some(snapshot.id)
                })
            })
            .flatten();
        let active_checkpoint = active_id.and_then(|id| {
            self.snapshots.update(id, |snapshot| {
                snapshot.state = ChannelDownloadState::Queued;
                snapshot.current_bytes_per_second = None;
                snapshot.eta_ms = None;
                snapshot_record(snapshot)
            })
        });
        if let Some(checkpoint) = active_checkpoint {
            let queued = checkpoint
                .is_ok_and(|record| self.library.try_save_native_download_progress(record));
            if !queued {
                tracing::warn!(
                    event = "transfer.download.shutdown_checkpoint_deferred",
                    "storage queue unavailable; previous checkpoint and part map retained for recovery"
                );
            }
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
    tuning: teleark_telegram::TransferTuning,
) -> Result<AdaptiveTransferController, ApplicationError> {
    let config = AdaptiveControllerConfig::maximum_throughput(512 * 1024 * 1024, 1)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    let parameters = TransferControlParameters {
        active_file_count: tuning.download_tasks,
        transfer_connection_count: tuning.download_connections,
        inflight_parts_per_file: tuning.download_parts,
        inflight_rpcs_per_connection: tuning.download_parts.div_ceil(tuning.download_connections),
        ..TransferControlParameters::conservative_download()
    };
    AdaptiveTransferController::new(config, false)
        .map(|controller| controller.manual(parameters))
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
    snapshots: Arc<TransferSnapshots<ChannelDownloadSnapshot>>,
    scheduled: Arc<Mutex<BTreeSet<u64>>>,
    shutdown: Arc<AtomicBool>,
    library: DesktopLibrary,
    pump: Arc<QueuePump>,
) {
    let receiver = Arc::new(Mutex::new(receiver));
    thread::scope(|scope| {
        for _ in 0..8 {
            let receiver = receiver.clone();
            let backend = backend.clone();
            let snapshots = snapshots.clone();
            let scheduled = scheduled.clone();
            let shutdown = shutdown.clone();
            let library = library.clone();
            let pump = pump.clone();
            scope.spawn(move || {
                download_worker(
                    receiver, backend, snapshots, scheduled, shutdown, library, pump,
                )
            });
        }
    });
}
#[allow(clippy::too_many_arguments)]
fn download_worker(
    receiver: Arc<Mutex<mpsc::Receiver<TransferCommand>>>,
    backend: Arc<dyn ChannelDownloadBackend>,
    snapshots: Arc<TransferSnapshots<ChannelDownloadSnapshot>>,
    scheduled: Arc<Mutex<BTreeSet<u64>>>,
    shutdown: Arc<AtomicBool>,
    library: DesktopLibrary,
    pump: Arc<QueuePump>,
) {
    loop {
        let command = { receiver.lock().unwrap_or_else(|e| e.into_inner()).recv() };
        let Ok(command) = command else {
            break;
        };
        match command {
            TransferCommand::Download {
                mut snapshot,
                mut queued_at,
                control,
            } => {
                let id = snapshot.id;
                let slot = library.download_slots.acquire_while(|| {
                    !shutdown.load(Ordering::Acquire)
                        && !matches!(
                            control.load(Ordering::Acquire),
                            CONTROL_CANCELLED | CONTROL_PAUSED
                        )
                });
                if slot.is_none() {
                    if let Ok(mut scheduled) = scheduled.lock() {
                        scheduled.remove(&id);
                    }
                    pump.cleanup_wake.notify();
                    let _ = pump.refill();
                    continue;
                }
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
                            control.store(CONTROL_RUNNING, Ordering::Release);
                            drop(scheduled);
                            let _ = persist_snapshot(&library, &retry);
                            snapshot = retry;
                            queued_at = Instant::now();
                            continue;
                        }
                    }
                    scheduled.remove(&id);
                    drop(scheduled);
                    pump.cleanup_wake.notify();
                    break;
                }
            }
        }
        compact_idle_replays(&snapshots, &scheduled);
        if let Err(error) = pump.refill() {
            tracing::warn!(event = "transfer.download.refill_failed", error_kind = ?error.kind(), "download queue refill failed");
        }
    }
}

#[derive(Clone, Copy)]
struct DownloadWorkerState<'a> {
    snapshots: &'a Arc<TransferSnapshots<ChannelDownloadSnapshot>>,
    scheduled: &'a Mutex<BTreeSet<u64>>,
}

impl DownloadWorkerState<'_> {
    fn resume_after_pause(self, control: &AtomicU8) {
        let Ok(_guard) = self.scheduled.lock() else {
            return;
        };
        // The resume caller already persisted Queued before publishing this
        // token. Retirement only consumes that token; another snapshot/write
        // could overwrite a newer pause or cancellation.
        let _ = control.compare_exchange(
            CONTROL_RESUME_PENDING,
            CONTROL_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    // Only publication is serialized with retry. Storage and backend cleanup run
    // after this guard is dropped, while `scheduled` still retains task ownership.
    fn retire(
        self,
        control: &AtomicU8,
        id: u64,
        mutate: impl FnOnce(&mut ChannelDownloadSnapshot),
    ) -> Option<ChannelDownloadSnapshot> {
        let _guard = self.scheduled.lock().ok()?;
        if control.load(Ordering::Acquire) == CONTROL_RETRY_PENDING {
            return None;
        }
        if self
            .snapshots
            .read(id, |row| row.cleanup.is_some())
            .unwrap_or(true)
        {
            return None;
        }
        update_snapshot(self.snapshots, id, mutate)
    }
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
    let Ok(admission) = state.scheduled.lock() else {
        return;
    };
    let initial_control = match control.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
        matches!(value, CONTROL_RESUME_PENDING | CONTROL_RETRY_PENDING).then_some(CONTROL_RUNNING)
    }) {
        Ok(_) => CONTROL_RUNNING,
        Err(current) => current,
    };
    let mut admitted = false;
    let started_snapshot = update_snapshot(snapshots, snapshot.id, |current| {
        // The queued command can outlive a newer pause/cancel. Do not enter
        // filesystem/transport work or overwrite that command's projection.
        if initial_control != CONTROL_RUNNING || current.state != ChannelDownloadState::Queued {
            return;
        }
        admitted = true;
        current.state = ChannelDownloadState::Running;
        current.started_at_unix_ms = current.started_at_unix_ms.or(started_at_unix_ms);
        current.queue_wait_ms = Some(queue_wait_ms);
        current.attempts = current.attempts.saturating_add(1);
        current.acknowledged_bytes = 0;
        current.events.push(ChannelDownloadEvent {
            kind: ChannelDownloadEventKind::Started,
            timestamp_unix_ms: started_at_unix_ms.unwrap_or(current.queued_at_unix_ms),
            elapsed_ms: Some(queue_wait_ms),
            failure_kind: None,
        });
    });
    drop(admission);
    if !admitted {
        return;
    }
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
    let (controller, tuning) = match library.preferences().and_then(|preferences| {
        new_download_controller(preferences.transfer_tuning)
            .map(|controller| (controller, preferences.transfer_tuning))
    }) {
        Ok(controller) => controller,
        Err(error) => {
            fail_download(
                DownloadRetirementOwner {
                    backend,
                    state,
                    control: &control,
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
                    DownloadRetirementOwner {
                        backend,
                        state,
                        control: &control,
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
            DownloadRetirementOwner {
                backend,
                state,
                control: &control,
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
        receipts: Arc::new(NativeReceipts {
            id: snapshot.id,
            account: snapshot.account_id,
            attempt: snapshot.attempts.saturating_add(1),
            snapshots: snapshots.clone(),
            state: Mutex::new((snapshot.transferred_bytes, BTreeMap::new())),
        }),
        last_persisted: Mutex::new(started),
        tuning,
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
    match result {
        _ if control.load(Ordering::Acquire) == CONTROL_RETRY_PENDING => {
            // transfer_loop starts the replacement only after this owner exits.
        }
        Ok(()) if control.load(Ordering::Acquire) == CONTROL_CANCELLED => {
            let cancelled = state.retire(&control, snapshot.id, |current| {
                if current.state == ChannelDownloadState::Queued {
                    return;
                }
                current.state = ChannelDownloadState::Cancelled;
                current.verification = ChannelDownloadVerification::NotReached;
                current.finished_at_unix_ms = finished_at_unix_ms;
                current.duration_ms = Some(duration_ms);
                current.current_bytes_per_second = None;
                current.eta_ms = None;
                current.failure = None;
            });
            if let Some(cancelled) = cancelled
                && cancelled.state == ChannelDownloadState::Cancelled
            {
                let _ = persist_snapshot(library, &cancelled);
            }
        }
        Ok(()) => complete_download(
            DownloadRetirementOwner {
                backend,
                state,
                control: &control,
                library,
                snapshot: &snapshot,
            },
            queue_wait_ms,
            duration_ms,
            finished_at_unix_ms,
        ),
        Err(error) if shutdown.load(Ordering::Acquire) => {
            let interrupted = state.retire(&control, snapshot.id, |current| {
                if !matches!(
                    current.state,
                    ChannelDownloadState::Paused | ChannelDownloadState::Cancelled
                ) && current.cleanup.is_none()
                {
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
            // Pause was projected and persisted by the control caller before
            // signalling the backend. Do not replay it over a newer command.
        }
        Err(_error) if control.load(Ordering::Acquire) == CONTROL_RESUME_PENDING => {
            state.resume_after_pause(&control);
        }
        Err(_error) if control.load(Ordering::Acquire) == CONTROL_CANCELLED => {
            let cancelled = state.retire(&control, snapshot.id, |current| {
                if current.state == ChannelDownloadState::Queued {
                    return;
                }
                current.state = ChannelDownloadState::Cancelled;
                current.verification = ChannelDownloadVerification::NotReached;
                current.finished_at_unix_ms = finished_at_unix_ms;
                current.duration_ms = Some(duration_ms);
                current.current_bytes_per_second = None;
                current.eta_ms = None;
                current.failure = None;
            });
            if let Some(cancelled) = cancelled
                && cancelled.state == ChannelDownloadState::Cancelled
            {
                let _ = persist_snapshot(library, &cancelled);
            }
        }
        Err(error) => fail_download(
            DownloadRetirementOwner {
                backend,
                state,
                control: &control,
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
    owner: DownloadRetirementOwner<'_>,
    queue_wait_ms: u64,
    duration_ms: u64,
    finished_at_unix_ms: Option<i64>,
) {
    let DownloadRetirementOwner {
        backend,
        state,
        control,
        library,
        snapshot,
    } = owner;
    // Native transport publishes its own hidden partial. The separate, shared
    // destination reservation is no longer needed once the output is complete.
    // Keep this filesystem work outside the retirement lock and before Completed.
    release_completed_reservation(
        backend,
        snapshot.id,
        &snapshot.destination,
        snapshot.size_bytes,
    );
    let average_bytes_per_second = average_rate(snapshot.size_bytes, duration_ms);
    let completed = state.retire(control, snapshot.id, |current| {
        // Cancellation can arrive while the final reservation cleanup waits on
        // the filesystem. Its owner has already projected/persisted that intent.
        if control.load(Ordering::Acquire) == CONTROL_CANCELLED {
            return;
        }
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
    let Some(completed) = completed.filter(|row| row.state == ChannelDownloadState::Completed)
    else {
        return;
    };
    if let Err(error) = persist_snapshot(library, &completed) {
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

fn release_completed_reservation(
    backend: &dyn ChannelDownloadBackend,
    id: u64,
    destination: &Path,
    expected_bytes: u64,
) {
    if let Err(error) = backend.release_completed_reservation(destination, expected_bytes) {
        // A cleanup error cannot invalidate the published user file. Restored
        // completed history retries this maintenance on the next startup.
        tracing::warn!(
            event = "transfer.download.completed_reservation_cleanup_failed",
            task_id = id,
            error_kind = ?error.kind(),
            "completed download reservation cleanup will be retried at startup"
        );
    }
}

struct DownloadRetirementOwner<'a> {
    backend: &'a dyn ChannelDownloadBackend,
    state: DownloadWorkerState<'a>,
    control: &'a AtomicU8,
    library: &'a DesktopLibrary,
    snapshot: &'a ChannelDownloadSnapshot,
}

fn fail_download(
    owner: DownloadRetirementOwner<'_>,
    error: ApplicationError,
    queue_wait_ms: u64,
    duration_ms: u64,
    finished_at_unix_ms: Option<i64>,
) {
    let DownloadRetirementOwner {
        backend,
        state,
        control,
        library,
        snapshot,
    } = owner;
    let failure = failure_diagnostic(error.kind());
    let failed = state.retire(control, snapshot.id, |current| {
        current.state = ChannelDownloadState::Failed(error.kind());
        current.verification = ChannelDownloadVerification::NotReached;
        current.finished_at_unix_ms = finished_at_unix_ms;
        current.duration_ms = Some(duration_ms);
        current.current_bytes_per_second = None;
        current.eta_ms = None;
        let part = current.part_events.iter().rev().find_map(|event| {
            (event.state == DownloadPartState::Failed && event.task_attempt == current.attempts)
                .then_some(ChannelDownloadPartFailure {
                    part_index: event.part_index,
                    attempt: event.attempt,
                    elapsed_millis: event.elapsed_millis,
                    connection_slot: Some(event.connection_slot),
                    kind: event.failure,
                })
        });
        current.failure = Some(ChannelDownloadFailure { part, ..failure });
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

fn mutate_snapshot<T>(
    snapshots: &TransferSnapshots<ChannelDownloadSnapshot>,
    id: u64,
    update: impl FnOnce(&mut ChannelDownloadSnapshot) -> T,
) -> Option<T> {
    snapshots.update(id, update)
}

fn update_snapshot(
    snapshots: &TransferSnapshots<ChannelDownloadSnapshot>,
    id: u64,
    update: impl FnOnce(&mut ChannelDownloadSnapshot),
) -> Option<ChannelDownloadSnapshot> {
    mutate_snapshot(snapshots, id, |snapshot| {
        update(snapshot);
        bound_lifecycle(snapshot);
        snapshot.clone()
    })
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
        cleanup: None,
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
        acknowledged_bytes: 0,
        current_bytes_per_second: None,
        average_bytes_per_second: record.average_bytes_per_second,
        eta_ms: None,
        attempts: record.attempts,
        failure,
        events,
        event_history_omitted: 0,
        part_events: PartEventHistory::default(),
        session_log_path: None,
        telemetry: empty_download_telemetry(),
        server_status: None,
    }
}

fn persist_snapshot(
    library: &DesktopLibrary,
    snapshot: &ChannelDownloadSnapshot,
) -> Result<(), ApplicationError> {
    library.save_native_download(snapshot_record(snapshot)?)
}

fn snapshot_record(
    snapshot: &ChannelDownloadSnapshot,
) -> Result<NativeDownloadTaskRecord, ApplicationError> {
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
    Ok(NativeDownloadTaskRecord {
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
        | ApplicationErrorKind::Server
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
        part: None,
    }
}

pub(crate) const fn download_part_failure_code(kind: DownloadPartFailureKind) -> &'static str {
    match kind {
        DownloadPartFailureKind::Timeout => "timeout",
        DownloadPartFailureKind::Network => "network",
        DownloadPartFailureKind::Server => "server",
        DownloadPartFailureKind::RateLimited => "rate_limited",
        DownloadPartFailureKind::Authorization => "authorization",
        DownloadPartFailureKind::UnexpectedResponse => "unexpected_response",
        DownloadPartFailureKind::Other => "other",
    }
}

fn parse_download_part_failure_code(code: &str) -> Option<DownloadPartFailureKind> {
    Some(match code {
        "timeout" => DownloadPartFailureKind::Timeout,
        "network" => DownloadPartFailureKind::Network,
        "server" => DownloadPartFailureKind::Server,
        "rate_limited" => DownloadPartFailureKind::RateLimited,
        "authorization" => DownloadPartFailureKind::Authorization,
        "unexpected_response" => DownloadPartFailureKind::UnexpectedResponse,
        "other" => DownloadPartFailureKind::Other,
        _ => return None,
    })
}

fn last_failed_part_from_log(path: &Path) -> Option<ChannelDownloadPartFailure> {
    use std::io::{Read as _, Seek as _, SeekFrom};

    const MAX_TAIL_BYTES: u64 = 64 * 1024;
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(MAX_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::with_capacity((length - start) as usize);
    file.take(MAX_TAIL_BYTES).read_to_end(&mut tail).ok()?;
    let first_complete = if start == 0 {
        0
    } else {
        tail.iter().position(|&byte| byte == b'\n')? + 1
    };
    for line in tail[first_complete..].split(|&byte| byte == b'\n').rev() {
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        let event = record.get("event").and_then(serde_json::Value::as_str);
        if event == Some("session_started") {
            break;
        }
        if event != Some("part_state")
            || record.get("state").and_then(serde_json::Value::as_str) != Some("Failed")
        {
            continue;
        }
        let schema = record.get("schema")?.as_u64()?;
        if schema != 1 && schema != 2 {
            return None;
        }
        return Some(ChannelDownloadPartFailure {
            part_index: record.get("part_index")?.as_u64()?,
            attempt: u32::try_from(record.get("attempt")?.as_u64()?).ok()?,
            elapsed_millis: record.get("attempt_elapsed_ms")?.as_u64()?,
            connection_slot: (schema == 2)
                .then(|| u16::try_from(record.get("connection_slot")?.as_u64()?).ok())
                .flatten(),
            kind: (schema == 2)
                .then(|| parse_download_part_failure_code(record.get("failure")?.as_str()?))
                .flatten(),
        });
    }
    None
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
        // Preserve the existing durable codec: server failures historically used network.
        ApplicationErrorKind::Network | ApplicationErrorKind::Server => "network",
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
    #[path = "batch_redownload.rs"]
    mod batch_redownload;
    #[path = "batch_stop.rs"]
    mod batch_stop;
    #[path = "filter_batch.rs"]
    mod filter_batch;
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
    fn retained_encrypted_partial_reserves_its_destination() {
        let directory = tempfile::tempdir().expect("directory");
        let partial = directory.path().join("report.bin.partial");
        std::fs::write(&partial, b"recoverable bytes").expect("partial");
        assert_eq!(
            available_download_destination(directory.path(), "report.bin")
                .expect("new destination"),
            directory.path().join("report (1).bin")
        );
        assert_eq!(
            std::fs::read(partial).expect("preserved"),
            b"recoverable bytes"
        );
    }

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
    fn manual_download_parameters_never_change_with_traffic_or_retries() {
        let tuning = teleark_telegram::TransferTuning {
            download_parts: 13,
            download_connections: 3,
            ..Default::default()
        };
        let mut controller = new_download_controller(tuning).expect("controller");
        let expected = controller.parameters();
        for step in 0..100 {
            let sample = PerformanceSample {
                observed_at_millis: step * 2000,
                goodput_bytes_per_second: step * 100000,
                flood_wait_seconds: Some(30),
                memory: MemoryCounters {
                    network_inflight_bytes: u64::MAX,
                    ..Default::default()
                },
                ..Default::default()
            };
            controller.observe(sample);
            controller.recover_from_part_retry(sample);
            assert_eq!(controller.parameters(), expected);
        }
        assert_eq!(expected.inflight_parts_per_file, 13);
        assert_eq!(expected.transfer_connection_count, 3);
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
                task_attempt: 1,
                part_index: 0,
                offset_bytes: 0,
                length_bytes: DOWNLOAD_PART_SIZE_BYTES,
                state: DownloadPartState::Inflight,
                attempt: 1,
                elapsed_millis: 0,
                connection_slot: 0,
                failure: None,
            },
            ChannelDownloadPartEvent {
                task_attempt: 1,
                part_index: 0,
                offset_bytes: 0,
                length_bytes: DOWNLOAD_PART_SIZE_BYTES,
                state: DownloadPartState::Completed,
                attempt: 1,
                elapsed_millis: 25,
                connection_slot: 0,
                failure: None,
            },
            ChannelDownloadPartEvent {
                task_attempt: 1,
                part_index: 1,
                offset_bytes: DOWNLOAD_PART_SIZE_BYTES,
                length_bytes: DOWNLOAD_PART_SIZE_BYTES,
                state: DownloadPartState::Failed,
                attempt: 1,
                elapsed_millis: 40,
                connection_slot: 0,
                failure: Some(DownloadPartFailureKind::Network),
            },
        ];

        let mut history = PartEventHistory::default();
        for event in events {
            history.push(event);
        }
        let counters = history.counters(3, 1, 1_500);

        assert_eq!(counters.completed_parts, 1);
        assert_eq!(counters.inflight_parts, 0);
        assert_eq!(counters.failed_parts, 1);
        assert_eq!(counters.missing_parts, 2);
        assert_eq!(counters.completed_parts_per_second_milli, 1_500);
    }

    #[test]
    fn failed_part_diagnostic_restores_only_from_the_latest_session() {
        let root = tempfile::tempdir().expect("fixture");
        let path = root.path().join("native-download.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"schema\":1,\"event\":\"session_started\"}\n",
                "{\"schema\":1,\"event\":\"part_state\",\"state\":\"Failed\"}\n",
                "{\"schema\":1,\"event\":\"session_started\"}\n",
                "{\"schema\":2,\"event\":\"part_state\",\"state\":\"Failed\",\"part_index\":5,\"attempt\":4,\"attempt_elapsed_ms\":60001,\"connection_slot\":5,\"failure\":\"timeout\"}\n",
                "{\"schema\":1,\"event\":\"session_finished\",\"result\":\"Network\"}\n",
            ),
        )
        .expect("write fixture");
        assert_eq!(
            last_failed_part_from_log(&path),
            Some(ChannelDownloadPartFailure {
                part_index: 5,
                attempt: 4,
                elapsed_millis: 60_001,
                connection_slot: Some(5),
                kind: Some(DownloadPartFailureKind::Timeout),
            })
        );
        use std::io::Write as _;
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open fixture");
        writeln!(log, "{{\"schema\":1,\"event\":\"session_started\"}}").expect("new session");
        writeln!(log, "{{\"schema\":1,\"event\":\"session_finished\"}}").expect("new finish");
        assert_eq!(last_failed_part_from_log(&path), None);
        drop(log);
        std::fs::write(
            &path,
            concat!(
                "{\"schema\":1,\"event\":\"session_started\"}\n",
                "{\"schema\":1,\"event\":\"part_state\",\"state\":\"Failed\",\"part_index\":13,\"attempt\":4,\"attempt_elapsed_ms\":60002}\n",
                "{\"schema\":1,\"event\":\"session_finished\",\"result\":\"Network\"}\n",
            ),
        )
        .expect("legacy fixture");
        assert_eq!(
            last_failed_part_from_log(&path),
            Some(ChannelDownloadPartFailure {
                part_index: 13,
                attempt: 4,
                elapsed_millis: 60_002,
                connection_slot: None,
                kind: None,
            })
        );
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

    struct CleanupRelease(Option<mpsc::SyncSender<Result<(), ApplicationError>>>);

    impl CleanupRelease {
        fn release(mut self) {
            self.0
                .take()
                .expect("cleanup release sender")
                .send(Ok(()))
                .expect("release cleanup");
        }
    }

    impl Drop for CleanupRelease {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                // Unwind must release the fixture without letting it touch files
                // after the temporary directory has been removed.
                let _ =
                    sender.try_send(Err(ApplicationError::new(ApplicationErrorKind::Cancelled)));
            }
        }
    }

    fn cleanup_gate() -> (CleanupRelease, mpsc::Receiver<Result<(), ApplicationError>>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        (CleanupRelease(Some(sender)), receiver)
    }

    fn assert_schedule_available_during_cleanup(transfers: &DesktopTransfers) {
        let scheduled = transfers.inner.scheduled.clone();
        let (acquired, acknowledgement) = mpsc::sync_channel(1);
        let waiter = thread::spawn(move || {
            let available = scheduled.lock().is_ok();
            let _ = acquired.send(available);
        });
        // Live workers may briefly acquire this lock for retirement or refill.
        // Require acquisition while cleanup remains gated, not at one instant.
        let result = acknowledgement.recv_timeout(Duration::from_secs(3));
        if result.is_ok() {
            waiter.join().expect("schedule lock observer");
        }
        assert_eq!(
            result,
            Ok(true),
            "blocked filesystem cleanup must not retain the scheduling lock"
        );
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
            // A terminal UI state precedes the final durable write/cleanup. Fixtures
            // that reopen or replace stored records must wait for worker ownership
            // to be released, otherwise the old writer can overwrite the fixture.
            if is_terminal(snapshot.state)
                && !transfers
                    .inner
                    .scheduled
                    .lock()
                    .expect("scheduled tasks")
                    .contains(&id)
            {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "download worker timed out");
            thread::yield_now();
        }
    }

    #[test]
    fn real_worker_publishes_live_progress_and_persistent_completion() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let destination = library
            .next_download_destination("archive.zip")
            .expect("reserved output");
        let reservation = destination.with_file_name("archive.zip.partial");
        assert_eq!(fs::metadata(&reservation).expect("reservation").len(), 0);
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
        assert!(
            !reservation.exists(),
            "completed native output releases its admission marker"
        );
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
    fn completed_reservation_cleanup_failure_preserves_output_and_retries_at_startup() {
        struct CleanupBackend {
            download: FakeBackend,
            attempts: AtomicUsize,
            repaired: mpsc::SyncSender<()>,
        }
        impl ChannelDownloadBackend for CleanupBackend {
            fn download(
                &self,
                account: Option<i64>,
                chat: i64,
                message: i64,
                destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                self.download
                    .download(account, chat, message, destination, observer)
            }
            fn release_completed_reservation(
                &self,
                destination: &Path,
                bytes: u64,
            ) -> Result<(), ApplicationError> {
                if self.attempts.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                    return Err(ApplicationError::new(
                        ApplicationErrorKind::PermissionDenied,
                    ));
                }
                reservation::release_completed(destination, bytes)?;
                self.repaired.send(()).expect("repair notification");
                Ok(())
            }
        }
        let root = tempfile::tempdir().expect("fixture");
        let library = library(&root);
        let destination = library
            .next_download_destination("archive.zip")
            .expect("output");
        let marker = destination.with_file_name("archive.zip.partial");
        let (repaired, repair) = mpsc::sync_channel(1);
        let backend = Arc::new(CleanupBackend {
            download: FakeBackend {
                outcome: Ok(()),
                calls: StdMutex::new(Vec::new()),
            },
            attempts: AtomicUsize::new(0),
            repaired,
        });
        let transfers = test_transfers(backend.clone(), library.clone()).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("enqueue");
        assert_eq!(
            wait_for_terminal(&transfers, id).state,
            ChannelDownloadState::Completed
        );
        assert_eq!(
            fs::read(&destination).expect("published output"),
            b"telegram bytes"
        );
        assert!(
            marker.exists(),
            "injected cleanup failure retains the marker"
        );
        drop(transfers);
        let restored = DesktopTransfers::with_backend(backend.clone(), library).expect("restart");
        repair
            .recv_timeout(Duration::from_secs(5))
            .expect("startup repair");
        assert!(!marker.exists());
        assert_eq!(
            fs::read(&destination).expect("preserved output"),
            b"telegram bytes"
        );
        assert_eq!(
            restored.snapshots().expect("history")[0].state,
            ChannelDownloadState::Completed
        );
        assert_eq!(
            backend.download.calls.lock().expect("calls").len(),
            1,
            "repair does not redownload"
        );
    }

    #[test]
    fn blocked_startup_reservation_cleanup_does_not_block_new_downloads() {
        struct BlockingCleanup {
            download: FakeBackend,
            old_output: PathBuf,
            entered: mpsc::SyncSender<()>,
            release: StdMutex<mpsc::Receiver<()>>,
            repaired: mpsc::SyncSender<()>,
        }
        impl ChannelDownloadBackend for BlockingCleanup {
            fn download(
                &self,
                account: Option<i64>,
                chat: i64,
                message: i64,
                destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                if destination
                    .file_name()
                    .is_some_and(|name| name == "cancel.zip")
                {
                    self.entered.send(()).expect("cancellable transfer entered");
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while observer.control() == DownloadControl::Continue {
                        assert!(Instant::now() < deadline, "cancel must signal the writer");
                        thread::yield_now();
                    }
                    return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                }
                self.download
                    .download(account, chat, message, destination, observer)
            }
            fn release_completed_reservation(
                &self,
                destination: &Path,
                bytes: u64,
            ) -> Result<(), ApplicationError> {
                let blocked = destination == self.old_output
                    || destination
                        .file_name()
                        .is_some_and(|name| name == "late.zip");
                if blocked {
                    self.entered.send(()).expect("entered");
                    self.release
                        .lock()
                        .expect("gate")
                        .recv_timeout(Duration::from_secs(10))
                        .expect("released");
                }
                reservation::release_completed(destination, bytes)?;
                if blocked {
                    self.repaired.send(()).expect("repaired");
                }
                Ok(())
            }
        }
        let root = tempfile::tempdir().expect("fixture");
        let library = library(&root);
        let old_output = library
            .next_download_destination("old.zip")
            .expect("old output");
        let marker = old_output.with_file_name("old.zip.partial");
        let transfers = test_transfers(
            Arc::new(FakeBackend {
                outcome: Ok(()),
                calls: StdMutex::new(Vec::new()),
            }),
            library.clone(),
        )
        .expect("worker");
        let id = transfers
            .enqueue_channel_download(request(old_output.clone()))
            .expect("enqueue");
        wait_for_terminal(&transfers, id);
        drop(transfers);
        fs::write(&marker, b"").expect("legacy empty reservation");
        let (entered, started) = mpsc::sync_channel(1);
        let (release, gate) = mpsc::sync_channel(1);
        let (repaired, repair) = mpsc::sync_channel(1);
        let transfers = test_transfers(
            Arc::new(BlockingCleanup {
                download: FakeBackend {
                    outcome: Ok(()),
                    calls: StdMutex::new(Vec::new()),
                },
                old_output: old_output.clone(),
                entered,
                release: StdMutex::new(gate),
                repaired,
            }),
            library.clone(),
        )
        .expect("startup does not wait for filesystem repair");
        started
            .recv_timeout(Duration::from_secs(5))
            .expect("repair entered");
        let new_output = library
            .next_download_destination("new.zip")
            .expect("new output");
        let id = transfers
            .enqueue_channel_download(request(new_output.clone()))
            .expect("new task");
        assert_eq!(
            wait_for_terminal(&transfers, id).state,
            ChannelDownloadState::Completed
        );
        assert_eq!(
            fs::read(new_output).expect("independent output"),
            b"telegram bytes"
        );
        assert!(marker.exists(), "old filesystem operation is still blocked");
        let cancel_output = library
            .next_download_destination("cancel.zip")
            .expect("cancel output");
        let cancel_id = transfers
            .enqueue_channel_download(request(cancel_output))
            .expect("cancellable task");
        started
            .recv_timeout(Duration::from_secs(5))
            .expect("cancellable transfer entered");
        transfers
            .cancel(cancel_id)
            .expect("cancellation cleanup is independent of legacy repair");
        assert_eq!(
            transfers.snapshot_state(cancel_id).expect("cancelled"),
            ChannelDownloadState::Cancelled
        );
        assert!(
            marker.exists(),
            "legacy repair remains blocked after cancellation finishes"
        );
        release.send(()).expect("release repair");
        repair
            .recv_timeout(Duration::from_secs(5))
            .expect("repair finishes");
        assert!(!marker.exists());
        assert_eq!(fs::read(old_output).expect("old output"), b"telegram bytes");

        let late_output = library
            .next_download_destination("late.zip")
            .expect("late output");
        let id = transfers
            .enqueue_channel_download(request(late_output.clone()))
            .expect("late task");
        started
            .recv_timeout(Duration::from_secs(5))
            .expect("completion cleanup entered");
        let cancelling = transfers.clone();
        let cancel = thread::spawn(move || cancelling.cancel(id));
        let deadline = Instant::now() + Duration::from_secs(5);
        while transfers.snapshot_state(id).expect("state") != ChannelDownloadState::Cancelled {
            assert!(
                Instant::now() < deadline,
                "cancel intent must precede filesystem settlement"
            );
            thread::yield_now();
        }
        release.send(()).expect("release completion cleanup");
        repair
            .recv_timeout(Duration::from_secs(5))
            .expect("completion cleanup finishes");
        cancel
            .join()
            .expect("cancel owner")
            .expect("cancel settled");
        let cancelled = wait_for_terminal(&transfers, id);
        assert_eq!(cancelled.state, ChannelDownloadState::Cancelled);
        assert!(
            !cancelled
                .events
                .iter()
                .any(|event| event.kind == ChannelDownloadEventKind::Completed),
            "late completion must not overwrite cancellation"
        );
        assert_eq!(
            fs::read(late_output).expect("published file survives cancellation"),
            b"telegram bytes"
        );
    }

    #[test]
    fn durable_queue_refills_without_a_frontend_poll_or_subscriber() {
        struct GatedBackend {
            first: AtomicBool,
            entered: mpsc::SyncSender<()>,
            release: StdMutex<mpsc::Receiver<()>>,
        }
        impl ChannelDownloadBackend for GatedBackend {
            fn download(
                &self,
                _: Option<i64>,
                _: i64,
                _: i64,
                destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                if !self.first.swap(true, Ordering::AcqRel) {
                    self.entered.send(()).expect("entered");
                    self.release
                        .lock()
                        .expect("gate")
                        .recv_timeout(Duration::from_secs(5))
                        .expect("released");
                }
                fs::write(destination, b"telegram bytes").expect("fixture output");
                observer.progressed(14);
                Ok(())
            }
        }
        let root = tempfile::tempdir().expect("fixture");
        let (entered, started) = mpsc::sync_channel(1);
        let (release, gate) = mpsc::sync_channel(1);
        let backend = Arc::new(GatedBackend {
            first: AtomicBool::new(false),
            entered,
            release: StdMutex::new(gate),
        });
        let transfers = test_transfers(backend, library(&root)).expect("worker");
        let total = CHANNEL_DOWNLOAD_QUEUE_CAPACITY * 2 + 3;
        let requests = (0..total)
            .map(|index| request(root.path().join(format!("{index}.bin"))))
            .collect();
        transfers
            .enqueue_channel_download_batch(requests)
            .expect("batch");
        started
            .recv_timeout(Duration::from_secs(5))
            .expect("backend entered");
        assert_eq!(transfers.snapshots().expect("queued").len(), total);
        assert!(transfers.inner.scheduled.lock().expect("scheduled").len() < total);
        release.send(()).expect("release backend");
        let ids = transfers
            .snapshots()
            .expect("tasks")
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();
        for id in ids {
            assert_eq!(
                wait_for_terminal(&transfers, id).state,
                ChannelDownloadState::Completed
            );
        }
        assert!(
            transfers
                .snapshots()
                .expect("finished")
                .iter()
                .all(|row| row.state == ChannelDownloadState::Completed)
        );
    }

    #[test]
    #[ignore = "manual before/after performance measurement"]
    fn perf_transfer_snapshot_views() {
        use std::hint::black_box;
        let root = tempfile::tempdir().expect("fixture");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend, library(&root)).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(root.path().join("fixture.bin")))
            .expect("enqueue");
        let mut template = wait_for_terminal(&transfers, id);
        for part_index in 0..512 {
            template.part_events.push(ChannelDownloadPartEvent {
                task_attempt: 1,
                part_index,
                offset_bytes: part_index * DOWNLOAD_PART_SIZE_BYTES,
                length_bytes: DOWNLOAD_PART_SIZE_BYTES,
                state: DownloadPartState::Completed,
                attempt: 1,
                elapsed_millis: 1,
                connection_slot: 0,
                failure: None,
            });
        }
        let store = TransferSnapshots::new(
            (0..128)
                .map(|id| {
                    let mut row = template.clone();
                    row.id = id;
                    row
                })
                .collect(),
        )
        .expect("store");
        let view = store.view().expect("initial view");
        let started = Instant::now();
        for _ in 0..100 {
            black_box(store.all().expect("legacy full clone"));
        }
        let before = started.elapsed();
        let started = Instant::now();
        for _ in 0..100 {
            black_box(store.view().expect("reusable view"));
        }
        let idle = started.elapsed();
        let started = Instant::now();
        for _ in 0..100 {
            store.update(64, |row| row.transferred_bytes += 1);
            let changed = store.view().expect("changed view");
            assert!(Arc::ptr_eq(&view.items[0], &changed.items[0]));
            black_box(changed);
        }
        println!(
            "transfer_snapshot_views tasks=128 events_per_task=512 reads=100 old_us={} idle_us={} one_changed_us={}",
            before.as_micros(),
            idle.as_micros(),
            started.elapsed().as_micros()
        );
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
    fn resident_history_eviction_preserves_database_records_and_completed_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let mut transfers = test_transfers(backend, library.clone()).expect("worker");
        let requests = (0..4)
            .map(|index| request(directory.path().join(format!("old-{index}.bin"))))
            .collect();
        transfers
            .enqueue_channel_download_batch(requests)
            .expect("original batch");
        let ids: Vec<_> = transfers
            .snapshots()
            .expect("records")
            .iter()
            .map(|snapshot| snapshot.id)
            .collect();
        for id in &ids {
            wait_for_terminal(&transfers, *id);
        }
        Arc::get_mut(&mut transfers.inner)
            .expect("unique frontend")
            .retention = HistoryRetention::new(4, 0, 4);
        let fresh = transfers
            .enqueue_channel_download(request(directory.path().join("new.bin")))
            .expect("new task");
        wait_for_terminal(&transfers, fresh);
        let view = transfers.snapshot_view().expect("retained view");
        assert_eq!(view.items.len(), 1); // Old completed batch was evicted as a whole.
        assert_eq!(view.omitted_items, 4);
        assert_eq!(
            library.native_downloads().expect("durable history").len(),
            5
        );
        for index in 0..4 {
            assert_eq!(
                fs::read(directory.path().join(format!("old-{index}.bin")))
                    .expect("completed file"),
                b"telegram bytes"
            );
        }
        // A failed insert must release its admission, allowing the remaining slots.
        assert!(
            transfers
                .enqueue_channel_download(request(directory.path().join("old-0.bin")))
                .is_err()
        );
        let requests = (0..3)
            .map(|index| request(directory.path().join(format!("more-{index}.bin"))))
            .collect();
        transfers
            .enqueue_channel_download_batch(requests)
            .expect("failed insert released capacity");
        assert_eq!(transfers.snapshots().expect("bounded view").len(), 4);
        for snapshot in transfers.snapshots().expect("remaining tasks") {
            wait_for_terminal(&transfers, snapshot.id);
        }
        let redownload = transfers
            .redownload_completed(ids[0])
            .expect("cold historical redownload");
        let finished = wait_for_terminal(&transfers, redownload);
        assert_eq!(finished.state, ChannelDownloadState::Completed);
        assert_ne!(redownload, ids[0]);
        assert_eq!(
            fs::read(&finished.destination).expect("new output"),
            b"telegram bytes"
        );
        assert!(
            library
                .native_download(ids[0])
                .expect("old history")
                .is_some()
        );
        // Account isolation is still checked after loading cold source metadata.
        let mut foreign = library
            .insert_native_download(NewNativeDownloadTaskRecord {
                account_id: 99,
                chat_id: 100,
                message_id: 200,
                message_sent_at_unix_ms: None,
                file_name: "foreign.bin".into(),
                caption: None,
                mime_type: None,
                size_bytes: 14,
                destination: directory.path().join("foreign.bin"),
                created_at_unix_ms: 1,
            })
            .expect("foreign historical fixture");
        foreign.state = StoredNativeDownloadState::Completed;
        library
            .save_native_download(foreign.clone())
            .expect("foreign fixture state");
        assert!(transfers.snapshot(foreign.id).is_none());
        assert_eq!(
            transfers
                .redownload_completed(foreign.id)
                .expect_err("foreign history must not authorize a transfer")
                .kind(),
            ApplicationErrorKind::Authorization
        );
    }

    #[test]
    fn unavailable_diagnostic_log_does_not_fail_a_download() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let logs = library.managed_directories().expect("directories").logs;
        fs::write(logs.join("Transfers"), b"path collision").expect("blocked log directory");
        let destination = directory.path().join("output.zip");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend, library).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("enqueue");
        assert_eq!(
            wait_for_terminal(&transfers, id).state,
            ChannelDownloadState::Completed
        );
        assert_eq!(fs::read(destination).expect("output"), b"telegram bytes");
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
            (ApplicationErrorKind::Server, true),
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
        let mut identities = calls
            .iter()
            .map(|call| (call.0, call.1))
            .collect::<Vec<_>>();
        identities.sort_unstable();
        assert_eq!(identities, [(100, 200), (101, 201)]);
    }

    #[test]
    fn graceful_exit_pauses_native_workers_and_retains_partial_files_after_restart() {
        struct ExitBackend {
            started: mpsc::Sender<()>,
            discarded: AtomicUsize,
        }
        impl ChannelDownloadBackend for ExitBackend {
            fn download(
                &self,
                _: Option<i64>,
                _: i64,
                _: i64,
                destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                fs::write(destination.with_extension("partial"), b"saved part")
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
                observer.progressed(4);
                let _ = self.started.send(());
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    if observer.control() != DownloadControl::Continue {
                        return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                    }
                    assert!(Instant::now() < deadline, "exit must signal the worker");
                    thread::yield_now();
                }
            }
            fn discard_partial(&self, _: &Path) -> Result<(), ApplicationError> {
                self.discarded.fetch_add(1, AtomicOrdering::Relaxed);
                Ok(())
            }
        }
        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let (started, ready) = mpsc::channel();
        let backend = Arc::new(ExitBackend {
            started,
            discarded: AtomicUsize::new(0),
        });
        let transfers = test_transfers(backend.clone(), library.clone()).expect("worker");
        let destination = library
            .next_download_destination("retained.zip")
            .expect("reserved output");
        let marker = destination.with_file_name("retained.zip.partial");
        let id = transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("enqueue");
        ready
            .recv_timeout(Duration::from_secs(5))
            .expect("worker started");
        transfers
            .pause_for_shutdown()
            .expect("durable pause and settlement");
        assert!(
            transfers
                .inner
                .scheduled
                .lock()
                .expect("workers")
                .is_empty()
        );
        assert_eq!(
            transfers.snapshot_state(id).expect("state"),
            ChannelDownloadState::Paused
        );
        assert_eq!(backend.discarded.load(AtomicOrdering::Relaxed), 0);
        assert_eq!(
            fs::read(destination.with_extension("partial")).expect("retained bytes"),
            b"saved part"
        );
        drop(transfers);
        let restarted = test_transfers(backend, library).expect("restart");
        assert_eq!(
            fs::metadata(&marker)
                .expect("paused reservation survives restart")
                .len(),
            0
        );
        restarted.activate_pending_downloads().expect("scheduler");
        assert_eq!(
            restarted.snapshot_state(id).expect("saved pause"),
            ChannelDownloadState::Paused
        );
        assert!(
            restarted
                .inner
                .scheduled
                .lock()
                .expect("workers")
                .is_empty()
        );
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
    fn resume_checkpoint_wait_cannot_be_overwritten_by_old_pause_retirement() {
        struct PausedBackend {
            started: mpsc::SyncSender<()>,
            release: Mutex<mpsc::Receiver<()>>,
            attempts: AtomicUsize,
        }
        impl ChannelDownloadBackend for PausedBackend {
            fn download(
                &self,
                _: Option<i64>,
                _: i64,
                _: i64,
                destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                if self.attempts.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                    observer.progressed(1);
                    self.started.send(()).expect("started");
                    self.release
                        .lock()
                        .expect("gate")
                        .recv()
                        .expect("release old attempt");
                    return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                }
                fs::write(destination, b"telegram bytes").expect("output");
                observer.progressed(14);
                Ok(())
            }
            fn discard_partial(&self, _: &Path) -> Result<(), ApplicationError> {
                Ok(())
            }
        }
        let directory = tempfile::tempdir().expect("directory");
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let backend = Arc::new(PausedBackend {
            started: started_tx,
            release: Mutex::new(release_rx),
            attempts: AtomicUsize::new(0),
        });
        let mut transfers =
            test_transfers(backend.clone(), library(&directory)).expect("transfers");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("resumed.zip")))
            .expect("enqueue");
        started_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("backend running");
        transfers.pause(id).expect("pause acknowledged");
        let snapshots = transfers.inner.snapshots.clone();
        let scheduled = transfers.inner.scheduled.clone();
        let pump = transfers.inner.pump.clone();
        let original_worker = transfers.inner.library.worker.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        Arc::get_mut(&mut transfers.inner)
            .expect("frontend ownership")
            .library
            .worker = crate::StorageWorker {
            inner: Arc::new(crate::WorkerInner {
                sender,
                join: Mutex::new(None),
            }),
        };
        let resume = thread::spawn(move || {
            transfers.resume(id).expect("resume acknowledged");
            transfers
        });
        let checkpoint = receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("resume checkpoint waiting");
        // Resume has published Queued, but its durable write has not yet
        // acknowledged and the old backend still sees the pause token.
        release_tx.send(()).expect("retire paused attempt");
        let deadline = Instant::now() + Duration::from_secs(3);
        while scheduled.lock().expect("scheduled").contains(&id) {
            assert!(
                Instant::now() < deadline,
                "old attempt must release ownership"
            );
            thread::yield_now();
        }
        let state_during_checkpoint = snapshots.get(id).expect("snapshot").state;
        pump.refill()
            .expect("capacity notification during checkpoint");
        let admitted_before_ack = scheduled.lock().expect("scheduled").contains(&id);
        let attempts_before_ack = backend.attempts.load(AtomicOrdering::SeqCst);
        match checkpoint {
            crate::StorageRequest::SaveNativeDownload { reply, .. } => {
                reply.send(Ok(())).expect("acknowledge resume");
            }
            _ => panic!("expected resume checkpoint"),
        }
        let mut transfers = resume.join().expect("resume caller");
        Arc::get_mut(&mut transfers.inner)
            .expect("frontend ownership")
            .library
            .worker = original_worker;
        drop(receiver);
        assert_eq!(
            state_during_checkpoint,
            ChannelDownloadState::Queued,
            "old pause retirement cannot overwrite a newer resume"
        );
        assert!(
            !admitted_before_ack,
            "resume must await durable acknowledgement"
        );
        assert_eq!(attempts_before_ack, 1);
        assert_eq!(
            wait_for_terminal(&transfers, id).state,
            ChannelDownloadState::Completed
        );
        assert_eq!(backend.attempts.load(AtomicOrdering::SeqCst), 2);
    }

    #[test]
    fn rpc_receipts_deduplicate_prefixes_and_reject_stopped_or_stale_attempts() {
        use teleark_telegram::DownloadReceiptObserver;
        let directory = tempfile::tempdir().expect("directory");
        let transfers = test_transfers(
            Arc::new(FakeBackend {
                outcome: Ok(()),
                calls: StdMutex::new(Vec::new()),
            }),
            library(&directory),
        )
        .expect("transfers");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("test.zip")))
            .expect("enqueue");
        let mut row = wait_for_terminal(&transfers, id);
        row.state = ChannelDownloadState::Running;
        row.attempts = 5;
        row.acknowledged_bytes = 0;
        row.transferred_bytes = 0;
        let snapshots = Arc::new(TransferSnapshots::new(vec![row]).expect("snapshots"));
        let observer = NativeReceipts {
            id,
            account: Some(1),
            attempt: 5,
            snapshots: snapshots.clone(),
            state: Mutex::new((0, BTreeMap::new())),
        };
        observer.acknowledged(0, 7);
        observer.acknowledged(0, 7);
        observer.acknowledged(1, 7);
        assert_eq!(
            snapshots.read(id, |row| (row.acknowledged_bytes, row.transferred_bytes)),
            Some((14, 14))
        );
        observer.completed(0);
        assert_eq!(
            observer
                .state
                .lock()
                .expect("bounded unfinished ranges")
                .1
                .len(),
            1
        );
        for (attempt, account, state, prefix) in [
            (6, Some(1), ChannelDownloadState::Running, 7),
            (5, Some(2), ChannelDownloadState::Running, 14),
            (5, Some(1), ChannelDownloadState::Paused, 21),
        ] {
            snapshots.update(id, |row| {
                row.attempts = attempt;
                row.account_id = account;
                row.state = state;
                row.acknowledged_bytes = 0;
                row.transferred_bytes = 0;
            });
            observer.acknowledged(2, prefix);
            assert_eq!(snapshots.read(id, |row| row.acknowledged_bytes), Some(0));
        }
    }

    #[test]
    fn stale_resume_retirement_preserves_a_newer_stop_command() {
        let directory = tempfile::tempdir().expect("directory");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend, library(&directory)).expect("transfers");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("fixture.zip")))
            .expect("enqueue");
        let template = wait_for_terminal(&transfers, id);
        for (token, newer_state) in [
            (CONTROL_CANCELLED, ChannelDownloadState::Cancelled),
            (CONTROL_PAUSED, ChannelDownloadState::Paused),
            (CONTROL_RETRY_PENDING, ChannelDownloadState::Queued),
        ] {
            let mut snapshot = template.clone();
            snapshot.state = newer_state;
            let snapshots = Arc::new(TransferSnapshots::new(vec![snapshot]).expect("snapshots"));
            let scheduled = Mutex::new(BTreeSet::from([id]));
            let control = AtomicU8::new(token);
            // The worker selected the resume branch before this newer command
            // arrived. Retirement must revalidate, not replay the stale choice.
            DownloadWorkerState {
                snapshots: &snapshots,
                scheduled: &scheduled,
            }
            .resume_after_pause(&control);
            assert_eq!(control.load(Ordering::Acquire), token);
            assert_eq!(snapshots.get(id).expect("retained task").state, newer_state);
        }
    }

    #[test]
    fn queued_worker_does_not_open_output_after_a_newer_stop() {
        let directory = tempfile::tempdir().expect("directory");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let library = library(&directory);
        let transfers = test_transfers(backend.clone(), library.clone()).expect("transfers");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("fixture.zip")))
            .expect("enqueue");
        let template = wait_for_terminal(&transfers, id);
        backend.calls.lock().expect("calls").clear();
        for (state, token) in [
            (ChannelDownloadState::Cancelled, CONTROL_RUNNING),
            (ChannelDownloadState::Paused, CONTROL_RUNNING),
            (ChannelDownloadState::Queued, CONTROL_PAUSED),
            (ChannelDownloadState::Cancelled, CONTROL_RESUME_PENDING),
            (ChannelDownloadState::Cancelled, CONTROL_RETRY_PENDING),
        ] {
            let mut current = template.clone();
            current.state = state;
            let snapshots = Arc::new(TransferSnapshots::new(vec![current]).expect("snapshots"));
            let scheduled = Mutex::new(BTreeSet::from([id]));
            run_download(
                &*backend,
                DownloadWorkerState {
                    snapshots: &snapshots,
                    scheduled: &scheduled,
                },
                &Arc::new(AtomicBool::new(false)),
                &library,
                template.clone(),
                Instant::now(),
                Arc::new(AtomicU8::new(token)),
            );
            let after = snapshots.get(id).expect("task retained");
            assert_eq!(after.state, state);
            assert_eq!(after.attempts, template.attempts);
            assert_eq!(after.events, template.events);
            assert!(
                backend.calls.lock().expect("calls").is_empty(),
                "stale queued command must not touch backend"
            );
        }
    }

    #[test]
    fn terminal_checkpoint_wait_does_not_hold_schedule_lock() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend.clone(), library(&directory)).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("done.zip")))
            .expect("enqueue");
        let snapshot = wait_for_terminal(&transfers, id);
        let mut blocked_library = library(&directory);
        let (sender, receiver) = mpsc::sync_channel(1);
        blocked_library.worker = crate::StorageWorker {
            inner: Arc::new(crate::WorkerInner {
                sender,
                join: Mutex::new(None),
            }),
        };
        let snapshots =
            Arc::new(TransferSnapshots::new(vec![snapshot.clone()]).expect("snapshots"));
        let scheduled = Arc::new(Mutex::new(BTreeSet::from([id])));
        let worker_scheduled = scheduled.clone();
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            complete_download(
                DownloadRetirementOwner {
                    backend: &*backend,
                    state: DownloadWorkerState {
                        snapshots: &snapshots,
                        scheduled: &worker_scheduled,
                    },
                    control: &AtomicU8::new(CONTROL_RUNNING),
                    library: &blocked_library,
                    snapshot: &snapshot,
                },
                0,
                1,
                Some(1),
            );
            finished_tx.send(()).expect("finished");
        });
        let request = receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("checkpoint awaiting acknowledgement");
        assert!(finished_rx.try_recv().is_err());
        assert!(
            scheduled.try_lock().is_ok(),
            "SQLite acknowledgement cannot hold scheduling lock"
        );
        drop(request);
        finished_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("checkpoint released");
        worker.join().expect("owner joined");
    }

    #[test]
    fn blocked_cleanup_does_not_hold_schedule_lock_or_start_retry_early() {
        struct CleanupBackend {
            attempts: AtomicUsize,
            entered: mpsc::SyncSender<()>,
            release: StdMutex<mpsc::Receiver<Result<(), ApplicationError>>>,
        }
        impl ChannelDownloadBackend for CleanupBackend {
            fn download(
                &self,
                _account: Option<i64>,
                _chat: i64,
                _message: i64,
                _destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                if self.attempts.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                    return Err(ApplicationError::new(ApplicationErrorKind::Network));
                }
                observer.progressed(14);
                Ok(())
            }
            fn cleanup_failed_partial(
                &self,
                _destination: &Path,
                _size: u64,
                _retain: bool,
            ) -> Result<(), ApplicationError> {
                self.entered.send(()).expect("cleanup entered");
                self.release
                    .lock()
                    .expect("release lock")
                    .recv()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))??;
                Ok(())
            }
        }
        let directory = tempfile::tempdir().expect("temporary directory");
        let library = library(&directory);
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = cleanup_gate();
        let backend = Arc::new(CleanupBackend {
            attempts: AtomicUsize::new(0),
            entered: entered_tx,
            release: StdMutex::new(release_rx),
        });
        let transfers = test_transfers(backend.clone(), library.clone()).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("first.zip")))
            .expect("enqueue");
        entered_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("cleanup blocked");
        assert_schedule_available_during_cleanup(&transfers);
        transfers.retry(id).expect("request retry during cleanup");
        assert_eq!(
            transfers.snapshot_state(id).expect("state"),
            ChannelDownloadState::Queued
        );
        let next = transfers
            .enqueue_channel_download(request(directory.path().join("next.zip")))
            .expect("unrelated admission remains available");
        assert!(
            transfers
                .inner
                .scheduled
                .lock()
                .expect("scheduled")
                .contains(&next)
        );
        assert_eq!(
            backend.attempts.load(AtomicOrdering::SeqCst),
            1,
            "retry cannot share old partial owner"
        );
        release_tx.release();
        assert_eq!(
            wait_for_terminal(&transfers, id).state,
            ChannelDownloadState::Completed
        );
        assert_eq!(
            wait_for_terminal(&transfers, next).state,
            ChannelDownloadState::Completed
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !transfers
            .inner
            .scheduled
            .lock()
            .expect("scheduled")
            .is_empty()
        {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert_eq!(
            library
                .native_download(id)
                .expect("stored")
                .expect("record")
                .state,
            StoredNativeDownloadState::Completed
        );
        assert_eq!(backend.attempts.load(AtomicOrdering::SeqCst), 3);
    }

    #[test]
    fn immediate_cancel_retry_waits_for_old_attempt_to_release_ownership() {
        struct RetryBackend {
            attempts: AtomicUsize,
            active: AtomicBool,
            fail_cleanup: bool,
            discarded: AtomicUsize,
            cleanup_started: mpsc::SyncSender<()>,
            cleanup_release: StdMutex<mpsc::Receiver<Result<(), ApplicationError>>>,
            started: mpsc::SyncSender<()>,
            release: StdMutex<mpsc::Receiver<()>>,
        }
        impl ChannelDownloadBackend for RetryBackend {
            fn download(
                &self,
                _account_id: Option<i64>,
                _chat_id: i64,
                _message_id: i64,
                destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                if self.attempts.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                    fs::write(destination.with_extension("partial"), b"partial bytes")
                        .expect("partial");
                    self.active.store(true, Ordering::Release);
                    observer.progressed(7);
                    self.started.send(()).expect("started");
                    self.release
                        .lock()
                        .expect("release lock")
                        .recv_timeout(Duration::from_secs(3))
                        .expect("release");
                    assert_eq!(observer.control(), DownloadControl::Cancel);
                    self.active.store(false, Ordering::Release);
                    return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                }
                assert_eq!(observer.control(), DownloadControl::Continue);
                observer.progressed(14);
                Ok(())
            }
            fn discard_partial(&self, destination: &Path) -> Result<(), ApplicationError> {
                assert!(
                    !self.active.load(Ordering::Acquire),
                    "cleanup cannot overlap old writer"
                );
                self.discarded.fetch_add(1, Ordering::SeqCst);
                self.cleanup_started.send(()).expect("cleanup started");
                self.cleanup_release
                    .lock()
                    .expect("cleanup gate")
                    .recv()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))??;
                if self.fail_cleanup {
                    return Err(ApplicationError::new(
                        ApplicationErrorKind::PermissionDenied,
                    ));
                }
                fs::remove_file(destination.with_extension("partial"))
                    .expect("remove owned partial");
                Ok(())
            }
        }
        for fail_cleanup in [false, true] {
            let directory = tempfile::tempdir().expect("temporary directory");
            let library = library(&directory);
            let (started_tx, started_rx) = mpsc::sync_channel(1);
            let (release_tx, release_rx) = mpsc::sync_channel(1);
            let (cleanup_started_tx, cleanup_started_rx) = mpsc::sync_channel(1);
            let (cleanup_release_tx, cleanup_release_rx) = cleanup_gate();
            let backend = Arc::new(RetryBackend {
                active: AtomicBool::new(false),
                fail_cleanup,
                discarded: AtomicUsize::new(0),
                cleanup_started: cleanup_started_tx,
                cleanup_release: StdMutex::new(cleanup_release_rx),
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
            let caller = transfers.clone();
            let cancellation = thread::spawn(move || caller.cancel(id));
            let deadline = Instant::now() + Duration::from_secs(3);
            while transfers.snapshot_state(id).expect("state") != ChannelDownloadState::Cancelled {
                assert!(
                    Instant::now() < deadline,
                    "cancel must be visible before waiting"
                );
                thread::yield_now();
            }
            assert_eq!(backend.discarded.load(Ordering::Acquire), 0);
            transfers.retry(id).expect("immediate retry");
            let queued = transfers.snapshots().expect("snapshots").remove(0);
            assert_eq!(queued.state, ChannelDownloadState::Queued);
            assert_eq!(queued.transferred_bytes, 0);
            assert_eq!(backend.attempts.load(AtomicOrdering::SeqCst), 1);
            release_tx.send(()).expect("release");
            cleanup_started_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("cleanup after writer release");
            assert_schedule_available_during_cleanup(&transfers);
            transfers
                .activate_pending_downloads()
                .expect("refill during cleanup");
            assert_eq!(backend.attempts.load(Ordering::Acquire), 1);
            assert_eq!(
                transfers
                    .delete(id)
                    .expect_err("cleanup owns recovery data")
                    .kind(),
                ApplicationErrorKind::Conflict
            );
            cleanup_release_tx.release();
            let cancellation = cancellation.join().expect("cancel caller");
            if fail_cleanup {
                assert_eq!(
                    cancellation.expect_err("cleanup failed").kind(),
                    ApplicationErrorKind::PermissionDenied
                );
                assert_eq!(
                    transfers.snapshot_state(id).expect("state"),
                    ChannelDownloadState::Cancelled
                );
                assert!(
                    transfers
                        .inner
                        .pump
                        .cancel_cleanup
                        .lock()
                        .expect("cleanup owners")
                        .is_empty()
                );
                assert_eq!(backend.attempts.load(Ordering::Acquire), 1);
                assert_eq!(
                    fs::read(directory.path().join("retry.partial")).expect("retained bytes"),
                    b"partial bytes"
                );
                let saved = library.native_downloads().expect("durable history");
                assert_eq!(
                    saved.iter().find(|row| row.id == id).expect("task").state,
                    StoredNativeDownloadState::Cancelled
                );
                continue;
            }
            cancellation.expect("cancel complete");
            assert!(!directory.path().join("retry.partial").exists());
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

        struct GatedRestartBackend {
            entered: mpsc::SyncSender<()>,
            release: StdMutex<mpsc::Receiver<()>>,
            backend: FakeBackend,
        }
        impl ChannelDownloadBackend for GatedRestartBackend {
            fn download(
                &self,
                account: Option<i64>,
                chat: i64,
                message: i64,
                destination: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                self.entered
                    .send(())
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))?;
                self.release
                    .lock()
                    .expect("release lock")
                    .recv()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))?;
                self.backend
                    .download(account, chat, message, destination, observer)
            }
        }
        let (entered, started) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::channel();
        let backend = Arc::new(GatedRestartBackend {
            entered,
            release: StdMutex::new(wait),
            backend: FakeBackend {
                outcome: Ok(()),
                calls: StdMutex::new(Vec::new()),
            },
        });
        // Hydration and account activation are separate boundaries. The usual
        // test helper activates immediately and may already be Running here.
        let restored = DesktopTransfers::with_backend(backend.clone(), library)
            .expect("restored, inactive worker");
        let checkpoint = restored
            .snapshots()
            .expect("restored snapshots")
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("restored checkpoint");
        assert_eq!(checkpoint.state, ChannelDownloadState::Queued);
        assert_eq!(checkpoint.transferred_bytes, 7);
        assert!(
            matches!(started.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "hydration alone must not enter the backend"
        );
        assert!(backend.backend.calls.lock().expect("calls").is_empty());
        restored
            .activate_account(1)
            .expect("activate restored account");
        started
            .recv_timeout(Duration::from_secs(5))
            .expect("automatic account activation starts recovered task");
        assert_eq!(
            restored.snapshot_state(id).expect("running state"),
            ChannelDownloadState::Running
        );
        assert!(
            backend
                .backend
                .calls
                .lock()
                .expect("gated backend")
                .is_empty()
        );
        release.send(()).expect("release backend");
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
    fn shutdown_does_not_wait_for_stalled_telegram_or_storage() {
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
        let mut transfers = test_transfers(
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

        // Keep the Drop path's storage queue full while the real transfer owner
        // remains blocked in its fake network read.
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(crate::StorageRequest::Shutdown)
            .expect("full storage queue");
        Arc::get_mut(&mut transfers.inner)
            .expect("unique frontend owner")
            .library
            .worker = crate::StorageWorker {
            inner: Arc::new(crate::WorkerInner {
                sender,
                join: Mutex::new(None),
            }),
        };
        let (finished, completion) = mpsc::sync_channel(1);
        let dropper = thread::spawn(move || {
            drop(transfers);
            finished.send(()).expect("completion receiver");
        });
        let result = completion.recv_timeout(Duration::from_secs(2));
        drop(receiver); // Release a regressed blocking sender before joining.
        let mut state = lock.lock().expect("gate lock");
        state.1 = true;
        condition.notify_all();
        drop(state);
        dropper.join().expect("dropper");
        result.expect("shutdown cannot wait for network or storage");
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
    #[test]
    fn destination_reservation_errors_fail_closed_and_search_is_bounded() {
        let directory = tempfile::tempdir().expect("fixture");
        let error = available_download_destination_with_reservations(
            directory.path(),
            "archive.zip",
            |_| Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
        )
        .expect_err("cannot ignore unavailable history");
        assert_eq!(error.kind(), ApplicationErrorKind::Persistence);
        let mut probes = 0;
        let error = available_download_destination_with_reservations(
            directory.path(),
            "archive.zip",
            |_| {
                probes += 1;
                Ok(true)
            },
        )
        .expect_err("all candidate paths reserved");
        assert_eq!(error.kind(), ApplicationErrorKind::Capacity);
        assert_eq!(probes, 10_000);
    }

    #[test]
    fn library_download_after_external_deletion_allocates_a_fresh_history_path() {
        let directory = tempfile::tempdir().expect("fixture");
        let library = library(&directory);
        library
            .set_preferences(&crate::DesktopPreferences {
                managed_files_root: Some(directory.path().join("managed")),
                ..crate::DesktopPreferences::default()
            })
            .expect("isolated downloads");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend, library.clone()).expect("worker");
        let destination = library
            .next_download_destination("archive.zip")
            .expect("first destination");
        let first = transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("first download");
        assert_eq!(
            wait_for_terminal(&transfers, first).state,
            ChannelDownloadState::Completed
        );
        std::fs::remove_file(&destination).expect("external deletion");

        // All Files prepares a destination and enqueues from catalog metadata.
        let repeated_destination = library
            .next_download_destination("archive.zip")
            .expect("second destination");
        let second = transfers
            .enqueue_channel_download(request(repeated_destination.clone()))
            .expect("download again from Library");
        assert_ne!(first, second);
        assert_ne!(destination, repeated_destination);
        assert_eq!(
            wait_for_terminal(&transfers, second).state,
            ChannelDownloadState::Completed
        );
        assert_eq!(
            std::fs::read(&repeated_destination).expect("restored bytes"),
            b"telegram bytes"
        );
        let history = library.native_downloads().expect("history");
        assert_eq!(history.len(), 2);
        assert!(
            history
                .iter()
                .any(|item| item.id == first && item.destination == destination)
        );
    }

    #[test]
    fn redownload_preserves_history_and_rejects_another_account() {
        let directory = tempfile::tempdir().expect("fixture");
        let destination = directory.path().join("archive.zip");
        let library = library(&directory);
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend.clone(), library.clone()).expect("worker");
        let first = transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("first download");
        assert_eq!(
            wait_for_terminal(&transfers, first).state,
            ChannelDownloadState::Completed
        );
        std::fs::remove_file(&destination).expect("external deletion");
        assert_eq!(
            crate::local_file_presence(&destination, 14),
            crate::LocalFilePresence::Missing
        );
        let second = transfers
            .redownload_completed(first)
            .expect("download again");
        assert_ne!(first, second);
        let repeated = wait_for_terminal(&transfers, second);
        assert_eq!(repeated.state, ChannelDownloadState::Completed);
        assert!(repeated.destination.exists());
        assert_eq!(
            library
                .downloaded_files_page(1, None)
                .expect("persisted output inventory")
                .len(),
            2
        );
        transfers.activate_account(2).expect("switch account");
        assert_eq!(
            transfers
                .redownload_completed(first)
                .expect_err("cross-account source must not execute")
                .kind(),
            ApplicationErrorKind::Authorization
        );
    }

    #[test]
    fn restart_cleanup_preserves_durable_retry_and_does_not_block_other_downloads() {
        struct Initial {
            database_path: PathBuf,
            entered: mpsc::SyncSender<()>,
            cleanup_entered: mpsc::SyncSender<()>,
            release: Mutex<mpsc::Receiver<Result<(), ApplicationError>>>,
        }
        impl ChannelDownloadBackend for Initial {
            fn download(
                &self,
                _: Option<i64>,
                _: i64,
                _: i64,
                _: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                self.entered.send(()).expect("entered");
                let deadline = Instant::now() + Duration::from_secs(5);
                while observer.control() != DownloadControl::Cancel {
                    assert!(Instant::now() < deadline, "cancel signal");
                    thread::yield_now();
                }
                let db = teleark_storage::Database::open(&self.database_path)
                    .expect("observe durable intent");
                let pending = db
                    .pending_native_download_cleanups(0, 128)
                    .expect("cleanup intent before signal");
                assert_eq!(pending.len(), 1);
                assert_eq!(
                    db.native_download(pending[0].task_id)
                        .expect("task")
                        .expect("saved cancellation")
                        .state,
                    StoredNativeDownloadState::Cancelled
                );
                Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
            }
            fn discard_partial(&self, _: &Path) -> Result<(), ApplicationError> {
                self.cleanup_entered.send(()).expect("cleanup entered");
                self.release
                    .lock()
                    .expect("release")
                    .recv()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))??;
                Err(ApplicationError::new(
                    ApplicationErrorKind::PermissionDenied,
                ))
            }
        }
        struct Restarted {
            entered: mpsc::SyncSender<()>,
            release: Mutex<mpsc::Receiver<Result<(), ApplicationError>>>,
            calls: Mutex<Vec<PathBuf>>,
        }
        impl ChannelDownloadBackend for Restarted {
            fn download(
                &self,
                _: Option<i64>,
                _: i64,
                _: i64,
                path: &Path,
                observer: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                self.calls.lock().expect("calls").push(path.to_path_buf());
                fs::write(path, b"verified bytes").expect("synthetic output");
                observer.progressed(14);
                Ok(())
            }
            fn discard_partial(&self, path: &Path) -> Result<(), ApplicationError> {
                self.entered.send(()).expect("restored cleanup");
                self.release
                    .lock()
                    .expect("release")
                    .recv()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))??;
                fs::remove_file(path.with_extension("owned-partial")).expect("owned partial");
                Ok(())
            }
        }
        let dir = tempfile::tempdir().expect("directory");
        let library = library(&dir);
        let destination = dir.path().join("recover.zip");
        fs::write(
            destination.with_extension("owned-partial"),
            b"partial bytes",
        )
        .expect("partial");
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (cleanup_tx, cleanup_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = cleanup_gate();
        let transfers = test_transfers(
            Arc::new(Initial {
                database_path: library.database_path.as_ref().clone(),
                entered: entered_tx,
                cleanup_entered: cleanup_tx,
                release: Mutex::new(release_rx),
            }),
            library.clone(),
        )
        .expect("initial owner");
        let id = transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("download");
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("running");
        let canceller = transfers.clone();
        let cancel = thread::spawn(move || canceller.cancel(id));
        cleanup_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("cleaning");
        assert_schedule_available_during_cleanup(&transfers);
        transfers.retry(id).expect("durable retry while cleaning");
        let db = teleark_storage::Database::open(library.database_path.as_ref())
            .expect("independent database");
        assert!(
            db.native_download_cleanup(id)
                .expect("cleanup")
                .expect("saved obligation")
                .retry_requested
        );
        release_tx.release();
        assert_eq!(
            cancel
                .join()
                .expect("cancel caller")
                .expect_err("cleanup denied")
                .kind(),
            ApplicationErrorKind::PermissionDenied
        );
        assert!(matches!(
            transfers
                .inner
                .snapshots
                .get(id)
                .expect("failed cleanup")
                .cleanup
                .expect("visible")
                .phase,
            ChannelDownloadCleanupPhase::Failed(ApplicationErrorKind::PermissionDenied)
        ));
        drop(transfers);
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = cleanup_gate();
        let backend = Arc::new(Restarted {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            calls: Mutex::new(Vec::new()),
        });
        let transfers = test_transfers(backend.clone(), library.clone()).expect("restart");
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("automatic cleanup resumed");
        let held = transfers.inner.snapshots.get(id).expect("restored task");
        assert!(held.cleanup.expect("waiting UI").retry_requested);
        assert!(
            backend.calls.lock().expect("calls").is_empty(),
            "no retry before cleanup acknowledgment"
        );
        assert_schedule_available_during_cleanup(&transfers);
        let other = dir.path().join("unrelated.zip");
        let other_id = transfers
            .enqueue_channel_download(request(other.clone()))
            .expect("unrelated download");
        assert_eq!(
            wait_for_terminal(&transfers, other_id).state,
            ChannelDownloadState::Completed
        );
        assert_eq!(*backend.calls.lock().expect("calls"), vec![other]);
        assert_schedule_available_during_cleanup(&transfers);
        release_tx.release();
        let completed = wait_for_terminal(&transfers, id);
        assert_eq!(completed.state, ChannelDownloadState::Completed);
        assert!(completed.cleanup.is_none());
        assert_eq!(completed.attempts, 2);
        assert!(!destination.with_extension("owned-partial").exists());
        assert_eq!(fs::read(&destination).expect("output"), b"verified bytes");
        assert!(
            db.native_download_cleanup(id)
                .expect("finished cleanup")
                .is_none()
        );
        assert_eq!(
            backend
                .calls
                .lock()
                .expect("calls")
                .iter()
                .filter(|path| **path == destination)
                .count(),
            1
        );
    }

    fn seed_cancelled_cleanup(library: &DesktopLibrary, destination: &Path) -> u64 {
        let mut db =
            teleark_storage::Database::open(library.database_path.as_ref()).expect("database");
        let mut task = db
            .insert_native_download(&NewNativeDownloadTaskRecord {
                account_id: 1,
                chat_id: 11,
                message_id: 19,
                message_sent_at_unix_ms: None,
                file_name: "saved.zip".into(),
                caption: None,
                mime_type: None,
                size_bytes: 14,
                destination: destination.into(),
                created_at_unix_ms: 10,
            })
            .expect("saved task");
        task.state = StoredNativeDownloadState::Cancelled;
        task.verification = StoredNativeDownloadVerification::NotReached;
        task.finished_at_unix_ms = Some(12);
        task.updated_at_unix_ms = 12;
        assert!(
            db.begin_native_download_cleanup(&task)
                .expect("saved cleanup")
        );
        task.id
    }

    #[test]
    fn startup_cleanup_without_retry_preserves_final_file_and_performs_no_download() {
        startup_cleanup_without_retry(false);
        startup_cleanup_without_retry(true);
    }

    fn startup_cleanup_without_retry(remove_directory: bool) {
        struct CleanupOnly;
        impl ChannelDownloadBackend for CleanupOnly {
            fn download(
                &self,
                _: Option<i64>,
                _: i64,
                _: i64,
                _: &Path,
                _: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                panic!("cancelled task must not download");
            }
            fn discard_partial(&self, path: &Path) -> Result<(), ApplicationError> {
                teleark_telegram::discard_partial_download(path)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))
            }
        }
        let dir = tempfile::tempdir().expect("directory");
        let library = library(&dir);
        let output = dir.path().join("output/nested");
        fs::create_dir_all(&output).expect("output directory");
        let destination = output.join("saved.zip");
        fs::write(&destination, b"existing final file").expect("final fixture");
        let partial = output.join(".saved.zip.teleark-partial");
        let map = output.join(".saved.zip.teleark-partial.map");
        fs::write(&partial, b"owned partial").expect("partial fixture");
        fs::write(&map, b"owned bitmap").expect("bitmap fixture");
        let id = seed_cancelled_cleanup(&library, &destination);
        if remove_directory {
            fs::remove_file(&partial).expect("remove partial");
            fs::remove_file(&map).expect("remove map");
            fs::remove_file(&destination).expect("remove synthetic final");
            fs::remove_dir(&output).expect("remove nested output");
            fs::remove_dir(output.parent().expect("parent")).expect("remove output");
        }
        let transfers = test_transfers(Arc::new(CleanupOnly), library.clone()).expect("restart");
        let deadline = Instant::now() + Duration::from_secs(5);
        while transfers
            .inner
            .snapshots
            .get(id)
            .expect("task")
            .cleanup
            .is_some()
        {
            assert!(
                Instant::now() < deadline,
                "cleanup completes without a retry"
            );
            thread::yield_now();
        }
        assert_eq!(
            transfers.snapshot_state(id).expect("state"),
            ChannelDownloadState::Cancelled
        );
        assert!(!partial.exists() && !map.exists());
        if remove_directory {
            assert!(
                !output.exists(),
                "cleanup must not recreate removed directories"
            );
        } else {
            assert_eq!(
                fs::read(&destination).expect("final retained"),
                b"existing final file"
            );
        }
        let db = teleark_storage::Database::open(library.database_path.as_ref()).expect("database");
        assert!(
            db.native_download_cleanup(id)
                .expect("acknowledgment")
                .is_none()
        );
    }

    #[test]
    fn dropping_frontend_does_not_wait_for_blocked_cleanup_or_lose_its_record() {
        struct BlockedCleanup {
            entered: mpsc::SyncSender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl ChannelDownloadBackend for BlockedCleanup {
            fn download(
                &self,
                _: Option<i64>,
                _: i64,
                _: i64,
                _: &Path,
                _: Arc<dyn DownloadObserver>,
            ) -> Result<(), ApplicationError> {
                panic!("no download");
            }
            fn discard_partial(&self, _: &Path) -> Result<(), ApplicationError> {
                self.entered.send(()).expect("entered");
                self.release
                    .lock()
                    .expect("release")
                    .recv_timeout(Duration::from_secs(5))
                    .expect("release blocked cleanup");
                Ok(())
            }
        }
        let dir = tempfile::tempdir().expect("directory");
        let library = library(&dir);
        let id = seed_cancelled_cleanup(&library, &dir.path().join("blocked.zip"));
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let transfers = test_transfers(
            Arc::new(BlockedCleanup {
                entered: entered_tx,
                release: Mutex::new(release_rx),
            }),
            library.clone(),
        )
        .expect("restart");
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("cleanup blocked");
        let (dropped_tx, dropped_rx) = mpsc::sync_channel(1);
        let dropper = thread::spawn(move || {
            drop(transfers);
            dropped_tx.send(()).expect("frontend dropped");
        });
        dropped_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("destructor does not wait for cleanup");
        let db = teleark_storage::Database::open(library.database_path.as_ref()).expect("database");
        assert!(
            db.native_download_cleanup(id)
                .expect("durable obligation")
                .is_some()
        );
        release_tx.send(()).expect("release cleanup");
        dropper.join().expect("dropper");
    }
}
