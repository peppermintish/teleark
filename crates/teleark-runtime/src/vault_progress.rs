use crate::transfer_updates::TransferSnapshots;
use crate::vault::{VaultTransferSnapshot, VaultTransferState};
use std::{
    collections::VecDeque,
    sync::Arc,
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
    /// New RPC acknowledgements in this owner generation; restored receipts excluded.
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
    pub queued: u16,
    pub active: u16,
    pub wait_until: Option<Instant>,
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
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultUploadSample {
    pub elapsed_millis: u64,
    pub interval_millis: u64,
    pub bytes_per_second: u64,
}

impl VaultUploadActivity {
    pub fn new(phase: VaultUploadPhase) -> Self {
        let now = Instant::now();
        let mut activity = Self {
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
            wait_until: None,
        };
        activity.event(None, false, 0, 0);
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
        });
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
    log: Option<crate::session_log_writer::SessionLogSink>,
    started: Instant,
}

impl VaultUploadObserver {
    pub(crate) fn new(transfers: Arc<TransferSnapshots<VaultTransferSnapshot>>, id: u64) -> Self {
        Self {
            transfers,
            id,
            log: None,
            started: Instant::now(),
        }
    }
    pub(crate) fn with_log(mut self, log: crate::session_log_writer::SessionLogSink) -> Self {
        self.log = Some(log);
        self
    }
    fn log(&self, event: impl std::fmt::Debug) {
        if let Some(log) = &self.log {
            log.record(format!("{{\"schema\":1,\"event\":\"upload_activity\",\"elapsed_ms\":{},\"detail\":\"{:?}\"}}\n", self.started.elapsed().as_millis(), event));
        }
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
}

impl ByteTransferObserver for VaultUploadObserver {
    fn observe(&self, event: ByteTransferEvent) {
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
                    activity.queued = queued;
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
                        activity.set_phase(VaultUploadPhase::Downloading);
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
                        activity.active = u16::from(bytes < total);
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
}
