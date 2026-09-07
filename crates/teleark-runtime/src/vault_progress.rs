use crate::vault::{VaultTransferSnapshot, VaultTransferState};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use teleark_telegram::{ByteTransferEvent, ByteTransferObserver};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultUploadPhase {
    CheckingStorage,
    CheckingTarget,
    Preparing,
    WaitingForTelegram,
    Uploading,
    SendingMessage,
    Verifying,
    Publishing,
}

/// Memory-only presentation data, never a checkpoint or verification receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultUploadActivity {
    pub phase: VaultUploadPhase,
    pub since: Instant,
    pub bytes: u64,
    pub total: u64,
    /// Logical-size equivalent of ciphertext read by the Telegram transport.
    /// Buffered/in-flight bytes are included; verified bytes stay separate.
    pub uploaded_bytes: u64,
    part_plaintext_bytes: u64,
}

impl VaultUploadActivity {
    pub fn new(phase: VaultUploadPhase) -> Self {
        Self {
            phase,
            since: Instant::now(),
            bytes: 0,
            total: 0,
            uploaded_bytes: 0,
            part_plaintext_bytes: 0,
        }
    }

    fn set_phase(&mut self, phase: VaultUploadPhase) {
        if self.phase != phase {
            self.phase = phase;
            self.since = Instant::now();
            self.bytes = 0;
            self.total = 0;
        }
    }
}

#[derive(Clone)]
pub(crate) struct VaultUploadObserver {
    transfers: Arc<Mutex<Vec<VaultTransferSnapshot>>>,
    id: u64,
}

impl VaultUploadObserver {
    pub(crate) fn new(transfers: Arc<Mutex<Vec<VaultTransferSnapshot>>>, id: u64) -> Self {
        Self { transfers, id }
    }

    fn update(&self, update: impl FnOnce(&mut VaultTransferSnapshot)) {
        if let Ok(mut transfers) = self.transfers.lock()
            && let Some(snapshot) = transfers.iter_mut().find(|item| item.id == self.id)
            && matches!(
                snapshot.state,
                VaultTransferState::Queued | VaultTransferState::Running
            )
        {
            update(snapshot);
        }
    }

    pub(crate) fn phase(&self, phase: VaultUploadPhase) {
        self.update(|snapshot| {
            snapshot
                .upload_activity
                .get_or_insert_with(|| VaultUploadActivity::new(phase))
                .set_phase(phase);
        });
    }

    pub(crate) fn begin_part(&self, plaintext_bytes: u64) {
        self.update(|snapshot| {
            if let Some(activity) = &mut snapshot.upload_activity {
                activity.part_plaintext_bytes = plaintext_bytes;
            }
        });
    }
}

impl ByteTransferObserver for VaultUploadObserver {
    fn observe(&self, event: ByteTransferEvent) {
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
                ByteTransferEvent::Uploading { bytes, total } => {
                    activity.set_phase(VaultUploadPhase::Uploading);
                    activity.bytes = bytes.min(total);
                    activity.total = total;
                    let logical_bytes = (u128::from(activity.part_plaintext_bytes)
                        * u128::from(bytes.min(total)))
                    .checked_div(u128::from(total))
                    .unwrap_or_default() as u64;
                    activity.uploaded_bytes = activity.uploaded_bytes.max(
                        snapshot
                            .transferred_bytes
                            .saturating_add(logical_bytes)
                            .min(snapshot.size_bytes),
                    );
                }
                ByteTransferEvent::SendingMessage => {
                    activity.set_phase(VaultUploadPhase::SendingMessage)
                }
                ByteTransferEvent::Downloading { bytes, total } => {
                    activity.set_phase(VaultUploadPhase::Verifying);
                    activity.bytes = bytes.min(total);
                    activity.total = total;
                }
            }
        });
    }
}
