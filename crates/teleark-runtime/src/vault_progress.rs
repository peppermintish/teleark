use crate::transfer_updates::TransferSnapshots;
use crate::vault::{VaultTransferSnapshot, VaultTransferState};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use teleark_telegram::{ByteTransferEvent, ByteTransferObserver};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultUploadPhase {
    RestartingExpired,
    RestartingUnsealed,
    UpgradingUpload,
    CheckingStorage,
    CheckingTarget,
    CheckingSource,
    SavingRecovery,
    Preparing,
    SealingContainer,
    WaitingForTelegram,
    Uploading,
    Downloading,
    SendingMessage,
    Verifying,
    Publishing,
    Persisting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultUploadOutcome {
    Completed,
    Failed,
    Paused,
    Cancelled,
}

/// Memory-only presentation data, never a checkpoint or verification receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultUploadActivity {
    pub phase: VaultUploadPhase,
    pub since: Instant,
    pub bytes: u64,
    pub total: u64,
    /// Logical-size equivalent of acknowledged ciphertext on the streaming path.
    /// Publication and whole-file verification remain separate.
    pub uploaded_bytes: u64,
    /// Cumulative transport bytes in this owner generation; restored receipts excluded.
    pub acknowledged_bytes: u64,
    pub persistence_since: Option<Instant>,
    part_plaintext_bytes: u64,
    pub started: Instant,
    pub last_activity: Instant,
    pub parts: Vec<VaultUploadPart>,
    pub events: VecDeque<VaultUploadEvent>,
    pub omitted_events: u64,
    pub samples: VecDeque<VaultUploadSample>,
    pub omitted_samples: u64,
    /// Upload transport buffers, or unacknowledged download containers.
    pub queued: u32,
    pub active: u16,
    /// Telegram stream chunks currently receiving for the current download.
    pub transport_chunks_active: u16,
    /// Telegram stream chunks waiting on a retry or shared FloodWait gate.
    pub transport_chunks_waiting: u16,
    pub wait_until: Option<Instant>,
    pub server_status: Option<teleark_telegram::TransferServerStatus>,
    current_part_index: Option<u32>,
    /// Completed extents beyond the bounded part-map window. Validated
    /// manifests require sequential part indices, so this remains constant-space.
    completed_unmapped_parts: u32,
    /// An acknowledged extent being revalidated after a resume or bad local receipt.
    reopened_unmapped_part: Option<u32>,
    sample_at: Instant,
    sample_bytes: u64,
    acknowledged_base: u64,
    acknowledged_current: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultUploadPartState {
    Queued,
    Uploading,
    Waiting,
    Acknowledged,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultUploadPart {
    pub state: VaultUploadPartState,
    pub attempt: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultUploadEvent {
    pub at: Instant,
    pub phase: VaultUploadPhase,
    pub part: Option<u32>,
    pub acknowledged: bool,
    pub attempt: u16,
    pub wait_millis: u64,
    pub outcome: Option<VaultUploadOutcome>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultUploadSample {
    pub elapsed_millis: u64,
    pub interval_millis: u64,
    pub bytes_per_second: u64,
}

impl VaultUploadActivity {
    pub fn new(phase: VaultUploadPhase) -> Self {
        let mut activity = Self::empty(phase);
        activity.event(None, false, 0, 0);
        activity
    }

    fn empty(phase: VaultUploadPhase) -> Self {
        let now = Instant::now();
        Self {
            phase,
            since: Instant::now(),
            bytes: 0,
            total: 0,
            uploaded_bytes: 0,
            acknowledged_bytes: 0,
            persistence_since: None,
            part_plaintext_bytes: 0,
            started: now,
            last_activity: now,
            sample_at: now,
            sample_bytes: 0,
            acknowledged_base: 0,
            acknowledged_current: 0,
            parts: Vec::new(),
            events: VecDeque::new(),
            samples: VecDeque::new(),
            omitted_events: 0,
            omitted_samples: 0,
            queued: 0,
            active: 0,
            transport_chunks_active: 0,
            transport_chunks_waiting: 0,
            wait_until: None,
            server_status: None,
            current_part_index: None,
            completed_unmapped_parts: 0,
            reopened_unmapped_part: None,
        }
    }

    pub(crate) fn terminal_only(outcome: VaultUploadOutcome) -> Self {
        let mut activity = Self::empty(VaultUploadPhase::Preparing);
        activity.record_terminal(outcome);
        activity
    }
    fn event(&mut self, part: Option<u32>, acknowledged: bool, attempt: u16, wait_millis: u64) {
        self.last_activity = Instant::now();
        if self.events.len() == 128 {
            self.events.pop_front();
            self.omitted_events += 1;
        }
        self.events.push_back(VaultUploadEvent {
            at: self.last_activity,
            phase: self.phase,
            part,
            acknowledged,
            attempt,
            wait_millis,
            outcome: None,
        });
    }

    pub(crate) fn record_terminal(&mut self, outcome: VaultUploadOutcome) {
        self.active = 0;
        self.transport_chunks_active = 0;
        self.transport_chunks_waiting = 0;
        self.wait_until = None;
        self.current_part_index = None;
        for part in &mut self.parts {
            if matches!(
                part.state,
                VaultUploadPartState::Uploading | VaultUploadPartState::Waiting
            ) {
                part.state = VaultUploadPartState::Queued;
            }
        }
        if self
            .events
            .back()
            .is_some_and(|event| event.outcome == Some(outcome))
        {
            return;
        }
        self.last_activity = Instant::now();
        if self.events.len() == 128 {
            self.events.pop_front();
            self.omitted_events += 1;
        }
        self.events.push_back(VaultUploadEvent {
            at: self.last_activity,
            phase: self.phase,
            part: None,
            acknowledged: false,
            attempt: 0,
            wait_millis: 0,
            outcome: Some(outcome),
        });
    }

    pub(crate) fn record_transition(&mut self) {
        self.event(None, false, 0, 0);
    }
    pub(crate) fn sample(&mut self, now: Instant, force: bool) -> bool {
        let elapsed = now.duration_since(self.sample_at);
        if elapsed < Duration::from_secs(1) && !force {
            return false;
        }
        if elapsed.is_zero() {
            return false;
        }
        let total = self
            .acknowledged_base
            .saturating_add(self.acknowledged_current);
        let bytes = total.saturating_sub(self.sample_bytes);
        let millis = elapsed.as_millis().max(1).min(u64::MAX as u128) as u64;
        if self.samples.len() == 96 {
            self.samples.pop_front();
            self.omitted_samples += 1;
        }
        self.samples.push_back(VaultUploadSample {
            elapsed_millis: now
                .duration_since(self.started)
                .as_millis()
                .min(u64::MAX as u128) as u64,
            interval_millis: millis,
            bytes_per_second: bytes.saturating_mul(1000) / millis,
        });
        self.sample_at = now;
        self.sample_bytes = total;
        true
    }
    fn acknowledged(&mut self, bytes: u64, total: u64, verified: u64, file_size: u64) {
        self.bytes = bytes.min(total);
        self.total = total;
        self.last_activity = Instant::now();
        let logical = (u128::from(self.part_plaintext_bytes) * u128::from(bytes.min(total)))
            .checked_div(u128::from(total))
            .unwrap_or_default() as u64;
        self.uploaded_bytes = self
            .uploaded_bytes
            .max(verified.saturating_add(logical).min(file_size));
    }

    fn set_phase(&mut self, phase: VaultUploadPhase) {
        if self.phase != phase {
            self.phase = phase;
            self.since = Instant::now();
            if !matches!(
                phase,
                VaultUploadPhase::Uploading
                    | VaultUploadPhase::SavingRecovery
                    | VaultUploadPhase::WaitingForTelegram
                    | VaultUploadPhase::SealingContainer
                    | VaultUploadPhase::SendingMessage
            ) {
                self.bytes = 0;
                self.total = 0;
            }
            self.event(None, false, 0, 0);
        }
    }
}

#[derive(Clone)]
pub(crate) struct VaultUploadObserver {
    transfers: Arc<TransferSnapshots<VaultTransferSnapshot>>,
    id: u64,
    direction: crate::vault::VaultTransferDirection,
    log: Option<crate::session_log_writer::SessionLogSink>,
    started: Instant,
    last_download_log_at: Arc<Mutex<Option<Instant>>>,
}

impl VaultUploadObserver {
    pub(crate) fn new(transfers: Arc<TransferSnapshots<VaultTransferSnapshot>>, id: u64) -> Self {
        let direction = transfers
            .read(id, |snapshot| snapshot.direction)
            .unwrap_or(crate::vault::VaultTransferDirection::Upload);
        Self {
            transfers,
            id,
            direction,
            log: None,
            started: Instant::now(),
            last_download_log_at: Arc::new(Mutex::new(None)),
        }
    }
    pub(crate) fn with_log(mut self, log: crate::session_log_writer::SessionLogSink) -> Self {
        self.log = Some(log);
        self
    }
    fn log(&self, event: impl std::fmt::Debug) {
        if let Some(log) = &self.log {
            log.record(format!("{{\"schema\":1,\"event\":\"vault_transfer_activity\",\"direction\":\"{}\",\"elapsed_ms\":{},\"detail\":\"{:?}\"}}\n", direction_name(self.direction), self.started.elapsed().as_millis(), event));
        }
    }

    fn log_download_progress(&self, bytes: u64, total: u64) {
        let Some(log) = &self.log else {
            return;
        };
        let now = Instant::now();
        let Ok(mut last) = self.last_download_log_at.lock() else {
            return;
        };
        if !download_progress_log_due(&mut last, now, bytes, total) {
            return;
        }
        log.record(format!("{{\"schema\":1,\"event\":\"download_progress\",\"direction\":\"download\",\"elapsed_ms\":{},\"received_bytes\":{bytes},\"total_bytes\":{total}}}\n", self.started.elapsed().as_millis()));
    }

    fn update(&self, update: impl FnOnce(&mut VaultTransferSnapshot)) {
        self.transfers.update(self.id, |snapshot| {
            if matches!(
                snapshot.state,
                VaultTransferState::Queued | VaultTransferState::Running
            ) {
                update(snapshot);
            }
        });
    }

    pub(crate) fn phase(&self, phase: VaultUploadPhase) {
        self.log(phase);
        self.update(|snapshot| {
            snapshot
                .upload_activity
                .get_or_insert_with(|| VaultUploadActivity::new(phase))
                .set_phase(phase);
        });
    }

    pub(crate) fn source_progress(&self, bytes: u64, total: u64) {
        self.update(|snapshot| {
            let activity = snapshot
                .upload_activity
                .get_or_insert_with(|| VaultUploadActivity::new(VaultUploadPhase::CheckingSource));
            activity.set_phase(VaultUploadPhase::CheckingSource);
            activity.bytes = bytes.min(total);
            activity.total = total;
            activity.last_activity = Instant::now();
        });
    }

    pub(crate) fn begin_part(&self, plaintext_bytes: u64) {
        self.update(|snapshot| {
            if let Some(activity) = &mut snapshot.upload_activity {
                activity.part_plaintext_bytes = plaintext_bytes;
                activity.acknowledged_base = activity
                    .acknowledged_base
                    .saturating_add(activity.acknowledged_current);
                activity.acknowledged_current = 0;
                activity.parts.clear();
                activity.bytes = 0;
                activity.total = 0;
            }
        });
    }

    pub(crate) fn begin_download_part(&self, index: u32, plaintext_bytes: u64) {
        self.update(|snapshot| {
            let Some(activity) = &mut snapshot.upload_activity else {
                return;
            };
            if activity.parts.is_empty() {
                activity.parts = vec![
                    VaultUploadPart {
                        state: VaultUploadPartState::Queued,
                        attempt: 0,
                    };
                    snapshot.part_count.min(4096) as usize
                ];
                activity.queued = snapshot.part_count;
                activity.completed_unmapped_parts = 0;
                activity.reopened_unmapped_part = None;
            }
            activity.part_plaintext_bytes = plaintext_bytes;
            activity.acknowledged_base = activity
                .acknowledged_base
                .saturating_add(activity.acknowledged_current);
            activity.acknowledged_current = 0;
            activity.current_part_index = Some(index);
            activity.bytes = 0;
            activity.total = 0;
            activity.active = 0;
            activity.transport_chunks_active = 0;
            activity.transport_chunks_waiting = 0;
            let map_end = u32::try_from(activity.parts.len()).unwrap_or(u32::MAX);
            let attempt = if let Some(part) = activity.parts.get_mut(index as usize) {
                let was_acknowledged = part.state == VaultUploadPartState::Acknowledged;
                if was_acknowledged {
                    // A previously receipted extent may need to be fetched again
                    // when its local copy no longer matches the saved receipt.
                    activity.queued = activity.queued.saturating_add(1);
                }
                part.state = VaultUploadPartState::Queued;
                part.attempt
            } else {
                let completed_end = map_end.saturating_add(activity.completed_unmapped_parts);
                let was_acknowledged = index >= map_end && index < completed_end;
                if was_acknowledged {
                    if activity.reopened_unmapped_part != Some(index) {
                        activity.queued = activity.queued.saturating_add(1);
                        activity.reopened_unmapped_part = Some(index);
                    }
                } else if activity.reopened_unmapped_part != Some(index) {
                    activity.reopened_unmapped_part = None;
                }
                0
            };
            activity.event(Some(index), false, attempt, 0);
        });
    }

    pub(crate) fn download_attempt_started(&self, index: u32) {
        self.update(|snapshot| {
            let Some(activity) = &mut snapshot.upload_activity else {
                return;
            };
            activity.current_part_index = Some(index);
            activity.acknowledged_current = 0;
            activity.bytes = 0;
            activity.total = 0;
            activity.active = 1;
            activity.transport_chunks_active = 0;
            activity.transport_chunks_waiting = 0;
            activity.wait_until = None;
            if let Some(part) = activity.parts.get_mut(index as usize) {
                part.state = VaultUploadPartState::Uploading;
            }
            activity.set_phase(VaultUploadPhase::Downloading);
            let attempt = activity
                .parts
                .get(index as usize)
                .map_or(0, |part| part.attempt);
            activity.event(Some(index), false, attempt, 0);
        });
    }

    pub(crate) fn retry_download_part(&self, index: u32, attempt: u8, delay: Duration) {
        self.update(|snapshot| {
            let Some(activity) = &mut snapshot.upload_activity else {
                return;
            };
            if let Some(part) = activity.parts.get_mut(index as usize) {
                part.state = VaultUploadPartState::Waiting;
                part.attempt = u16::from(attempt);
            }
            activity.acknowledged_base = activity
                .acknowledged_base
                .saturating_add(activity.acknowledged_current);
            activity.acknowledged_current = 0;
            activity.bytes = 0;
            activity.total = 0;
            activity.active = 0;
            activity.transport_chunks_active = 0;
            activity.transport_chunks_waiting = 0;
            activity.current_part_index = Some(index);
            activity.wait_until = Instant::now().checked_add(delay);
            activity.set_phase(VaultUploadPhase::WaitingForTelegram);
            activity.event(
                Some(index),
                false,
                u16::from(attempt),
                delay.as_millis().min(u64::MAX as u128) as u64,
            );
        });
        self.log(("download_retry", index, attempt, delay.as_millis()));
    }

    pub(crate) fn complete_download_part(&self, index: u32) {
        self.update(|snapshot| {
            let Some(activity) = &mut snapshot.upload_activity else {
                return;
            };
            let (newly_acknowledged, attempt) =
                if let Some(part) = activity.parts.get_mut(index as usize) {
                    let newly_acknowledged = part.state != VaultUploadPartState::Acknowledged;
                    part.state = VaultUploadPartState::Acknowledged;
                    (newly_acknowledged, part.attempt)
                } else {
                    let map_end = u32::try_from(activity.parts.len()).unwrap_or(u32::MAX);
                    let completed_end = map_end.saturating_add(activity.completed_unmapped_parts);
                    let reopened = activity.reopened_unmapped_part == Some(index);
                    let newly_acknowledged = reopened || index == completed_end;
                    if reopened {
                        activity.reopened_unmapped_part = None;
                    } else if index == completed_end {
                        activity.completed_unmapped_parts =
                            activity.completed_unmapped_parts.saturating_add(1);
                    }
                    (newly_acknowledged, 0)
                };
            if newly_acknowledged {
                activity.queued = activity.queued.saturating_sub(1);
            }
            activity.active = 0;
            activity.transport_chunks_active = 0;
            activity.transport_chunks_waiting = 0;
            activity.current_part_index = None;
            activity.wait_until = None;
            activity.event(Some(index), true, attempt, 0);
        });
    }
}

fn download_progress_log_due(
    last: &mut Option<Instant>,
    now: Instant,
    bytes: u64,
    total: u64,
) -> bool {
    let terminal = bytes >= total;
    if !terminal
        && last.is_some_and(|previous| {
            now.saturating_duration_since(previous) < Duration::from_secs(1)
        })
    {
        return false;
    }
    *last = Some(now);
    true
}

const fn direction_name(direction: crate::vault::VaultTransferDirection) -> &'static str {
    match direction {
        crate::vault::VaultTransferDirection::Upload => "upload",
        crate::vault::VaultTransferDirection::Download => "download",
    }
}

fn update_download_transport_activity(
    activity: &mut VaultUploadActivity,
    active_chunks: u16,
    waiting_chunks: u16,
    next_wait_millis: u64,
    server_waiting: bool,
    container_finishing: bool,
    now: Instant,
) {
    activity.transport_chunks_active = active_chunks;
    activity.transport_chunks_waiting = waiting_chunks;
    activity.wait_until = if server_waiting {
        activity
            .server_status
            .as_ref()
            .and_then(transfer_server_wait_remaining)
            .and_then(|remaining| now.checked_add(remaining))
    } else if waiting_chunks > 0 {
        now.checked_add(Duration::from_millis(next_wait_millis))
    } else {
        None
    };

    let container_active =
        !server_waiting && (active_chunks > 0 || (container_finishing && waiting_chunks == 0));
    let container_waiting = !container_active && (waiting_chunks > 0 || server_waiting);
    activity.active = u16::from(container_active);
    if container_waiting {
        activity.set_phase(VaultUploadPhase::WaitingForTelegram);
    } else {
        activity.set_phase(VaultUploadPhase::Downloading);
    }
    if let Some(index) = activity.current_part_index
        && let Some(part) = activity.parts.get_mut(index as usize)
    {
        part.state = if container_waiting {
            VaultUploadPartState::Waiting
        } else {
            VaultUploadPartState::Uploading
        };
    }
}

fn transfer_server_waiting(status: &teleark_telegram::TransferServerStatus) -> bool {
    transfer_server_wait_remaining(status).is_some()
}

fn transfer_server_wait_remaining(
    status: &teleark_telegram::TransferServerStatus,
) -> Option<Duration> {
    let now_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        });
    let millis = status.wait_until_unix_ms.checked_sub(now_unix_ms)?;
    (millis > 0).then(|| Duration::from_millis(millis as u64))
}

impl ByteTransferObserver for VaultUploadObserver {
    fn observe(&self, event: ByteTransferEvent) {
        if let ByteTransferEvent::Downloading { bytes, total } = event
            && self.direction == crate::vault::VaultTransferDirection::Download
        {
            self.log_download_progress(bytes, total);
        }
        if !matches!(
            event,
            ByteTransferEvent::Uploading { .. }
                | ByteTransferEvent::Downloading { .. }
                | ByteTransferEvent::UploadQueue { .. }
                | ByteTransferEvent::PartAcknowledged { .. }
                | ByteTransferEvent::PartStarted { .. }
        ) {
            self.log(event);
        }
        self.update(|snapshot| {
            let Some(activity) = &mut snapshot.upload_activity else {
                return;
            };
            // Manifest send/readback remains finalization, never another file part.
            if activity.phase == VaultUploadPhase::Publishing {
                return;
            }
            match event {
                ByteTransferEvent::WaitingForUpload => {
                    activity.set_phase(VaultUploadPhase::WaitingForTelegram)
                }
                ByteTransferEvent::UploadPlan { parts, total } => {
                    activity.parts = vec![
                        VaultUploadPart {
                            state: VaultUploadPartState::Queued,
                            attempt: 0
                        };
                        parts.min(4096) as usize
                    ];
                    activity.total = total;
                    // Restored acknowledgements are a baseline, never a burst of new throughput.
                    activity.sample_at = Instant::now();
                    activity.sample_bytes = activity.acknowledged_base;
                }
                ByteTransferEvent::UploadQueue { queued, active } => {
                    activity.queued = u32::from(queued);
                    activity.active = active;
                }
                ByteTransferEvent::Uploading { bytes, total } => {
                    activity.set_phase(VaultUploadPhase::Uploading);
                    activity.acknowledged(
                        bytes,
                        total,
                        snapshot.transferred_bytes,
                        snapshot.size_bytes,
                    );
                }
                ByteTransferEvent::PartAcknowledged {
                    index,
                    bytes,
                    total,
                } => {
                    let attempt = if let Some(part) = activity.parts.get_mut(index as usize) {
                        part.state = VaultUploadPartState::Acknowledged;
                        part.attempt
                    } else {
                        0
                    };
                    activity.set_phase(VaultUploadPhase::Uploading);
                    activity.event(Some(index), true, attempt, 0);
                    activity.acknowledged(
                        bytes,
                        total,
                        snapshot.transferred_bytes,
                        snapshot.size_bytes,
                    );
                    if attempt > 0 {
                        activity.acknowledged_bytes = activity
                            .acknowledged_bytes
                            .saturating_add(bytes.saturating_sub(activity.acknowledged_current));
                    }
                    activity.acknowledged_current = bytes;
                    if activity
                        .server_status
                        .as_ref()
                        .is_some_and(|s| !s.is_active())
                    {
                        activity.server_status = None;
                        snapshot.server_status = None;
                    }
                    if attempt == 0 {
                        activity.sample_bytes = activity.acknowledged_base.saturating_add(bytes);
                    } else {
                        // Rate and chart publication belongs to the one-second owner.
                    }
                }
                ByteTransferEvent::PartStarted { index, attempt } => {
                    if let Some(part) = activity.parts.get_mut(index as usize) {
                        part.state = VaultUploadPartState::Uploading;
                        part.attempt = attempt;
                    }
                    activity.set_phase(VaultUploadPhase::Uploading);
                    activity.event(Some(index), false, attempt, 0);
                }
                ByteTransferEvent::PartRetry {
                    index,
                    attempt,
                    wait_millis,
                } => {
                    if let Some(part) = activity.parts.get_mut(index as usize) {
                        part.state = VaultUploadPartState::Waiting;
                        part.attempt = attempt;
                    }
                    activity.wait_until =
                        Instant::now().checked_add(Duration::from_millis(wait_millis));
                    activity.set_phase(VaultUploadPhase::WaitingForTelegram);
                    activity.event(Some(index), false, attempt, wait_millis);
                }
                ByteTransferEvent::ServerThrottled { code, wait_seconds } => {
                    let now_unix = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as i64);
                    let wait_until_unix_ms = now_unix + i64::from(wait_seconds) * 1000;
                    let status = teleark_telegram::TransferServerStatus {
                        code,
                        flag: format!("FLOOD_WAIT_{wait_seconds}"),
                        wait_until_unix_ms,
                    };
                    snapshot.server_status = Some(status.clone());
                    activity.server_status = Some(status);
                    let now = Instant::now();
                    if snapshot.direction == crate::VaultTransferDirection::Download {
                        // FloodWait closes the shared gate before sibling RPCs
                        // necessarily return; do not present that wait as active
                        // receiving work.
                        activity.transport_chunks_active = 0;
                        activity.transport_chunks_waiting =
                            activity.transport_chunks_waiting.max(1);
                        activity.active = 0;
                        activity.set_phase(VaultUploadPhase::WaitingForTelegram);
                        activity.wait_until =
                            now.checked_add(Duration::from_secs(u64::from(wait_seconds)));
                        if let Some(index) = activity.current_part_index
                            && let Some(part) = activity.parts.get_mut(index as usize)
                        {
                            part.state = VaultUploadPartState::Waiting;
                        }
                    } else {
                        activity.set_phase(VaultUploadPhase::WaitingForTelegram);
                        activity.wait_until =
                            now.checked_add(Duration::from_secs(u64::from(wait_seconds)));
                    }
                }
                ByteTransferEvent::DownloadServerThrottled {
                    code,
                    wait_until_unix_ms,
                } => {
                    if snapshot.direction == crate::VaultTransferDirection::Download {
                        let status = teleark_telegram::TransferServerStatus {
                            code,
                            flag: "FLOOD_WAIT".to_owned(),
                            wait_until_unix_ms,
                        };
                        snapshot.server_status = Some(status.clone());
                        activity.server_status = Some(status);
                        let now_unix_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |duration| {
                                duration.as_millis().min(i64::MAX as u128) as i64
                            });
                        let wait_millis =
                            wait_until_unix_ms.saturating_sub(now_unix_ms).max(0) as u64;
                        let now = Instant::now();
                        activity.transport_chunks_active = 0;
                        activity.transport_chunks_waiting =
                            activity.transport_chunks_waiting.max(1);
                        activity.active = 0;
                        activity.wait_until = now.checked_add(Duration::from_millis(wait_millis));
                        activity.set_phase(VaultUploadPhase::WaitingForTelegram);
                        if let Some(index) = activity.current_part_index
                            && let Some(part) = activity.parts.get_mut(index as usize)
                        {
                            part.state = VaultUploadPartState::Waiting;
                        }
                        activity.event(activity.current_part_index, false, 0, wait_millis);
                    }
                }
                ByteTransferEvent::DownloadChunkStarted {
                    attempt,
                    active_chunks,
                    waiting_chunks,
                    next_wait_millis,
                    ..
                } => {
                    if snapshot.direction == crate::VaultTransferDirection::Download {
                        let server_waiting = snapshot
                            .server_status
                            .as_ref()
                            .is_some_and(transfer_server_waiting);
                        if !server_waiting {
                            snapshot.server_status = None;
                            activity.server_status = None;
                        }
                        let now = Instant::now();
                        update_download_transport_activity(
                            activity,
                            active_chunks,
                            waiting_chunks,
                            next_wait_millis,
                            server_waiting,
                            false,
                            now,
                        );
                        activity.event(
                            activity.current_part_index,
                            false,
                            attempt,
                            next_wait_millis,
                        );
                    }
                }
                ByteTransferEvent::DownloadChunkRetry {
                    attempt,
                    wait_millis,
                    active_chunks,
                    waiting_chunks,
                    next_wait_millis,
                    ..
                }
                | ByteTransferEvent::DownloadChunkWaiting {
                    attempt,
                    wait_millis,
                    active_chunks,
                    waiting_chunks,
                    next_wait_millis,
                    ..
                } => {
                    if snapshot.direction == crate::VaultTransferDirection::Download {
                        if let ByteTransferEvent::DownloadChunkWaiting {
                            server_code: Some(code),
                            server_wait_until_unix_ms: Some(wait_until_unix_ms),
                            ..
                        } = event
                        {
                            let status = teleark_telegram::TransferServerStatus {
                                code,
                                flag: "FLOOD_WAIT".to_owned(),
                                wait_until_unix_ms,
                            };
                            snapshot.server_status = Some(status.clone());
                            activity.server_status = Some(status);
                        }
                        let server_waiting = snapshot
                            .server_status
                            .as_ref()
                            .is_some_and(transfer_server_waiting);
                        if !server_waiting {
                            snapshot.server_status = None;
                            activity.server_status = None;
                        }
                        let now = Instant::now();
                        update_download_transport_activity(
                            activity,
                            active_chunks,
                            waiting_chunks,
                            next_wait_millis,
                            server_waiting,
                            false,
                            now,
                        );
                        activity.event(activity.current_part_index, false, attempt, wait_millis);
                    }
                }
                ByteTransferEvent::DownloadChunkFinished {
                    active_chunks,
                    waiting_chunks,
                    next_wait_millis,
                    ..
                } => {
                    if snapshot.direction == crate::VaultTransferDirection::Download {
                        let server_waiting = snapshot
                            .server_status
                            .as_ref()
                            .is_some_and(transfer_server_waiting);
                        if !server_waiting {
                            snapshot.server_status = None;
                            activity.server_status = None;
                        }
                        let now = Instant::now();
                        update_download_transport_activity(
                            activity,
                            active_chunks,
                            waiting_chunks,
                            next_wait_millis,
                            server_waiting,
                            true,
                            now,
                        );
                        activity.event(activity.current_part_index, false, 0, next_wait_millis);
                    }
                }
                ByteTransferEvent::SavingCheckpoint => {
                    activity.persistence_since = Some(Instant::now());
                    activity.event(None, false, 0, 0);
                }
                ByteTransferEvent::CheckpointSaved => {
                    activity.persistence_since = None;
                    activity.event(None, false, 0, 0);
                }
                ByteTransferEvent::WaitingForSeal => {
                    activity.set_phase(VaultUploadPhase::SealingContainer)
                }
                ByteTransferEvent::SendingMessage => {
                    activity.set_phase(VaultUploadPhase::SendingMessage)
                }
                ByteTransferEvent::Downloading { bytes, total } => {
                    if snapshot.direction == crate::VaultTransferDirection::Download {
                        activity.acknowledged_bytes = activity
                            .acknowledged_bytes
                            .saturating_add(bytes.saturating_sub(activity.acknowledged_current));
                        activity.acknowledged_current = bytes;
                        activity.acknowledged(
                            bytes,
                            total,
                            snapshot.transferred_bytes,
                            snapshot.size_bytes,
                        );
                        let server_waiting = snapshot
                            .server_status
                            .as_ref()
                            .is_some_and(transfer_server_waiting);
                        if server_waiting {
                            activity.active = 0;
                            activity.set_phase(VaultUploadPhase::WaitingForTelegram);
                            if let Some(index) = activity.current_part_index
                                && let Some(part) = activity.parts.get_mut(index as usize)
                            {
                                part.state = VaultUploadPartState::Waiting;
                            }
                        } else {
                            activity.set_phase(VaultUploadPhase::Downloading);
                            // The final stream byte still needs container authentication,
                            // extent verification, and receipt confirmation.
                            activity.active = 1;
                            if let Some(index) = activity.current_part_index
                                && let Some(part) = activity.parts.get_mut(index as usize)
                            {
                                part.state = VaultUploadPartState::Uploading;
                            }
                        }
                    } else {
                        activity.set_phase(VaultUploadPhase::Verifying);
                        activity.bytes = bytes.min(total);
                        activity.total = total;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_progress_logs_coalesce_zero_and_partial_updates_within_one_second() {
        let start = Instant::now();
        let mut last = None;

        assert!(download_progress_log_due(&mut last, start, 0, 4096));
        assert!(!download_progress_log_due(
            &mut last,
            start + Duration::from_millis(100),
            0,
            4096,
        ));
        assert!(!download_progress_log_due(
            &mut last,
            start + Duration::from_millis(500),
            1024,
            4096,
        ));
        assert!(!download_progress_log_due(
            &mut last,
            start + Duration::from_millis(999),
            2048,
            4096,
        ));
        assert!(download_progress_log_due(
            &mut last,
            start + Duration::from_secs(1),
            2048,
            4096,
        ));
        assert!(download_progress_log_due(
            &mut last,
            start + Duration::from_millis(1001),
            4096,
            4096,
        ));
    }

    #[test]
    fn activity_retains_transitions_with_visible_bounds_and_coalesces_measured_samples() {
        let mut activity = VaultUploadActivity::new(VaultUploadPhase::Preparing);
        let start = activity.sample_at;
        for index in 0..1000 {
            activity.event(Some(index), false, 1, 0);
            activity.acknowledged_current += 512 * 1024;
            activity.sample(start + Duration::from_millis(index as u64 + 1), false);
        }
        assert_eq!(activity.events.len(), 128);
        assert_eq!(activity.omitted_events, 873);
        assert_eq!(activity.samples.len(), 1);
        assert_eq!(activity.samples[0].interval_millis, 1000);
        assert_eq!(activity.samples[0].bytes_per_second, 1000 * 512 * 1024);
        for index in 1..=150 {
            activity.acknowledged_current += 512 * 1024;
            activity.sample(start + Duration::from_millis(1000 + index * 1000), false);
        }
        assert_eq!(activity.samples.len(), 96);
        assert_eq!(activity.omitted_samples, 55);
        activity.set_phase(VaultUploadPhase::SendingMessage);
        activity.set_phase(VaultUploadPhase::Persisting);
        assert_eq!(
            activity.events.back().expect("last").phase,
            VaultUploadPhase::Persisting
        );
    }

    #[test]
    fn terminal_activity_is_bounded_idempotent_and_replayable_across_attempts() {
        let mut activity = VaultUploadActivity::new(VaultUploadPhase::Downloading);
        activity.parts = vec![
            VaultUploadPart {
                state: VaultUploadPartState::Waiting,
                attempt: 2,
            },
            VaultUploadPart {
                state: VaultUploadPartState::Uploading,
                attempt: 1,
            },
            VaultUploadPart {
                state: VaultUploadPartState::Queued,
                attempt: 0,
            },
            VaultUploadPart {
                state: VaultUploadPartState::Acknowledged,
                attempt: 1,
            },
        ];
        activity.queued = 1;
        activity.active = 2;
        activity.transport_chunks_active = 3;
        activity.transport_chunks_waiting = 4;
        activity.wait_until = Some(Instant::now() + Duration::from_secs(5));
        activity.record_terminal(VaultUploadOutcome::Failed);
        assert_eq!(activity.active, 0, "terminal activity is not active");
        assert_eq!(activity.transport_chunks_active, 0);
        assert_eq!(activity.transport_chunks_waiting, 0);
        assert_eq!(
            activity.wait_until, None,
            "terminal activity is not waiting"
        );
        assert_eq!(
            activity.queued, 1,
            "terminalization does not derive transport buffers from the part map"
        );
        assert_eq!(
            activity
                .parts
                .iter()
                .map(|part| part.state)
                .collect::<Vec<_>>(),
            [
                VaultUploadPartState::Queued,
                VaultUploadPartState::Queued,
                VaultUploadPartState::Queued,
                VaultUploadPartState::Acknowledged,
            ],
            "terminal outcomes preserve completed parts and leave unfinished parts resumable"
        );
        let failed_at = activity.events.len();
        activity.record_terminal(VaultUploadOutcome::Failed);
        assert_eq!(
            activity.events.len(),
            failed_at,
            "owner and stop paths dedupe"
        );
        assert_eq!(
            activity.queued, 1,
            "duplicate terminals preserve the queue counter"
        );
        assert_eq!(
            activity.events.back().and_then(|event| event.outcome),
            Some(VaultUploadOutcome::Failed)
        );

        activity.record_transition();
        activity.record_terminal(VaultUploadOutcome::Completed);
        let outcomes = activity
            .events
            .iter()
            .filter_map(|event| event.outcome)
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes,
            [VaultUploadOutcome::Failed, VaultUploadOutcome::Completed]
        );

        for index in 0..128 {
            activity.event(Some(index), false, 1, 0);
        }
        activity.record_terminal(VaultUploadOutcome::Completed);
        assert_eq!(activity.events.len(), 128);
        assert_eq!(activity.omitted_events, 5);
        assert_eq!(
            activity.events.back().and_then(|event| event.outcome),
            Some(VaultUploadOutcome::Completed)
        );
    }
}
