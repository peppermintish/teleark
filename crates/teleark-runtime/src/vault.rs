use crate::transfer_updates::{
    TransferRecord, TransferSnapshotView, TransferSnapshots, TransferSubscription,
};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use teleark_core::{
    AccountId, ApplicationError, ApplicationErrorKind, FileKind, PackageId, PartIndex,
    TransferError,
};
use teleark_crypto::{
    AeadUsageRegistry, OsRandom, Password, PasswordWrap, RandomSource, RecoveryKey, RecoveryWrap,
    VaultMasterKey, generate_file_key, generate_recovery_key, generate_vault_master_key,
    unwrap_master_key_with_password, unwrap_master_key_with_recovery,
    wrap_master_key_with_password, wrap_master_key_with_recovery,
};
use teleark_storage::VaultMetadataRecord;
use teleark_transfer::{
    AdaptiveControllerConfig, AdaptiveTransferController, ContentDigest, EncryptionPipelineConfig,
    EncryptionPipelineError, MemoryCounters, NativeFileSystem, ParameterBounds, PartCounters,
    PerformanceSample, PipelinePart, QueueCounters, RemotePartKey, SourceId, SourcePort,
    TransferControlParameters, TransferTelemetrySnapshot, run_encryption_upload_pipeline,
};
use zeroize::Zeroizing;

use crate::transfer::{hex_id, package_id_from_bytes};
use crate::vault_progress::{VaultUploadActivity, VaultUploadObserver, VaultUploadPhase};
use crate::{
    DesktopLibrary, DesktopTelegram, EncryptedRemoteTransport, ManifestPublishRequest,
    TelegramObjectStore, encrypted_part_sizes, recover_remote_manifests,
};

mod catalog;
mod control;
mod health;
mod key_progress;
use control::UploadControls;
pub use control::VaultUploadControl;
pub use control::VaultUploadControl as VaultTransferControl;
mod download_history;
mod manifest_resume;
mod pending_upload;
#[cfg(test)]
mod recovery_acceptance;
mod source_digest;
mod sync_retry;
mod upload_history;
mod upload_progress;
pub use upload_progress::{
    VaultUploadSelectionPhase, VaultUploadSelectionProgress, VaultUploadSelectionSnapshot,
};
mod session;
pub use key_progress::{VaultKeyPhase, VaultKeyProgress, VaultKeySnapshot};
use session::{VaultEnvelope, VaultSession};

const VAULT_QUEUE_CAPACITY: usize = 16;
// Internal scheduling window, never a user selection limit.
const VAULT_UPLOAD_WINDOW: usize = 128;
const MAX_MANIFEST_SCAN: usize = 1_000;
const RECOVERY_BUNDLE_PREFIX: &str = "TARK-RB1-";
const TRANSFER_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
// With 60 MiB application parts, three workers plus the bounded reader,
// encrypted queue, and active upload stay inside the 512 MiB transfer budget.
const MAX_ENCRYPTION_WORKERS: u16 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultStatus {
    pub active_key_locked: bool,
    pub historical_key_unlocked: bool,
    pub active_vault_id: Option<[u8; 16]>,
    pub configured: bool,
    pub locked: bool,
    pub created_at_unix_ms: Option<i64>,
    pub password_generation: Option<u32>,
    pub recovery_generation: Option<u32>,
}

impl VaultStatus {
    const fn unconfigured() -> Self {
        Self {
            active_key_locked: true,
            historical_key_unlocked: false,
            active_vault_id: None,
            configured: false,
            locked: true,
            created_at_unix_ms: None,
            password_generation: None,
            recovery_generation: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultTransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultTransferState {
    Queued,
    Running,
    Pausing,
    Paused,
    Cancelling,
    Interrupted,
    Cancelled,
    Completed,
    Failed(ApplicationErrorKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultTransferSnapshot {
    /// Available durable control state; absent for legacy summaries and pre-admission work.
    pub recovery_state: Option<teleark_storage::VaultJobState>,
    /// Historical totals are durable; high-frequency telemetry is not replayed as live measurements.
    pub restored: bool,
    pub upload_activity: Option<VaultUploadActivity>,
    pub id: u64,
    pub account_id: i64,
    pub chat_id: i64,
    pub batch_id: Option<u64>,
    pub queued_at_unix_ms: i64,
    pub direction: VaultTransferDirection,
    pub file_name: String,
    pub package_id: Option<String>,
    pub size_bytes: u64,
    pub transferred_bytes: u64,
    pub completed_parts: u32,
    pub part_count: u32,
    pub started_at_unix_ms: i64,
    pub duration_ms: Option<u64>,
    pub average_bytes_per_second: Option<u64>,
    pub destination: Option<PathBuf>,
    pub session_log_path: Option<PathBuf>,
    pub telemetry: TransferTelemetrySnapshot,
    pub state: VaultTransferState,
}

impl TransferRecord for VaultTransferSnapshot {
    type Phase = (VaultTransferState, Option<VaultUploadPhase>);
    fn id(&self) -> u64 {
        self.id
    }
    fn phase(&self) -> Self::Phase {
        (
            self.state,
            self.upload_activity.as_ref().map(|activity| activity.phase),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedVaultFile {
    pub vault_id: Option<[u8; 16]>,
    pub health: crate::VaultFileHealth,
    pub part_message_ids: Vec<i64>,
    pub package_numeric_id: u64,
    pub package_id: String,
    pub logical_name: String,
    pub relative_path: Option<String>,
    pub mime_type: Option<String>,
    pub media_kind: FileKind,
    pub size_bytes: u64,
    pub encoded_size_bytes: u64,
    pub part_count: u32,
    pub created_at_unix_ms: i64,
    pub manifest_message_id: i64,
    pub related_remote_names: Vec<String>,
}

impl ManagedVaultFile {
    pub(crate) fn estimated_bytes(&self) -> usize {
        256 + self.package_id.len()
            + self.logical_name.len()
            + self.relative_path.as_ref().map_or(0, String::len)
            + self.mime_type.as_ref().map_or(0, String::len)
            + self.part_message_ids.len() * std::mem::size_of::<i64>()
            + self
                .related_remote_names
                .iter()
                .map(|name| 24 + name.len())
                .sum::<usize>()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ManagedVaultScan {
    pub health_checked_files: Option<usize>,
    pub files: Vec<ManagedVaultFile>,
    pub rejected_manifests: usize,
    pub catalog_pending: bool,
    pub catalog_limited: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultUploadSource {
    pub path: PathBuf,
    pub file_name: String,
    pub size_bytes: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultUploadFailure {
    pub source: PathBuf,
    pub kind: ApplicationErrorKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VaultUploadReport {
    /// Total successes. `completed` retains only recent receipts for immediate UI refresh.
    pub completed_count: usize,
    pub paused_count: usize,
    pub completed: Vec<ManagedVaultFile>,
    pub failed: Vec<VaultUploadFailure>,
    pub cancelled: Vec<PathBuf>,
}

/// Sequential picker preflight with no fixed file-count limit. The worker repeats it and
/// each item checks its size/mtime again before any encryption or publication.
pub fn inspect_upload_sources(
    paths: &[PathBuf],
) -> Result<Vec<VaultUploadSource>, ApplicationError> {
    inspect_upload_sources_observed(paths, None)
}

pub fn inspect_upload_sources_observed(
    paths: &[PathBuf],
    progress: Option<&VaultUploadSelectionProgress>,
) -> Result<Vec<VaultUploadSource>, ApplicationError> {
    if paths.is_empty() {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    if let Some(progress) = progress {
        progress.phase(VaultUploadSelectionPhase::Inspecting);
        progress.set_total(paths.len());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut sources = Vec::with_capacity(paths.len());
    let mut total = 0_u64;
    for path in paths {
        if let Some(progress) = progress {
            progress.check_cancelled()?;
        }
        let canonical = path.canonicalize().map_err(map_source_io)?;
        if !seen.insert(canonical) {
            if let Some(progress) = progress {
                progress.inspected();
            }
            continue;
        }
        let metadata = std::fs::metadata(path).map_err(map_source_io)?;
        if metadata.is_dir() {
            return Err(ApplicationError::new(
                ApplicationErrorKind::UploadFolderUnsupported,
            ));
        }
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceMissing));
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?
            .to_owned();
        total = total
            .checked_add(metadata.len())
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        sources.push(VaultUploadSource {
            path: path.clone(),
            file_name,
            size_bytes: metadata.len(),
            modified: metadata.modified().ok(),
        });
        if let Some(progress) = progress {
            progress.inspected();
        }
    }
    if let Some(progress) = progress {
        progress.check_cancelled()?;
    }
    Ok(sources)
}

#[derive(Default)]
struct UploadBatchPolicy {
    blocked: Option<ApplicationErrorKind>,
}
impl UploadBatchPolicy {
    fn execute<T>(
        &mut self,
        cancelled: &AtomicBool,
        upload: impl FnOnce() -> Result<T, ApplicationError>,
    ) -> Result<T, ApplicationError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        if let Some(kind) = self.blocked {
            return Err(ApplicationError::new(kind));
        }
        let result = upload();
        if let Err(error) = &result
            && matches!(
                error.kind(),
                ApplicationErrorKind::Authorization
                    | ApplicationErrorKind::PermissionDenied
                    | ApplicationErrorKind::Network
                    | ApplicationErrorKind::Server
            )
        {
            self.blocked = Some(error.kind());
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VaultUploadRecoveryReport {
    pub resumed: u64,
    pub failed: u64,
}

struct QueuedUpload {
    pending: crate::VaultPendingUploadContext,
    generation: u64,
    id: u64,
    batch_id: u64,
    queued_at: i64,
    source: VaultUploadSource,
}

type ActiveUploadBatch = Arc<Mutex<Option<(i64, u64, Arc<AtomicBool>)>>>;

#[derive(Clone)]
pub struct DesktopVault {
    inner: Arc<VaultInner>,
}

/// An already admitted operation. Wait only on a background executor.
/// Admission retains only this operation's key lease; locking does not revoke it.
#[must_use]
pub struct VaultJob<T> {
    response: mpsc::Receiver<Result<T, ApplicationError>>,
    _owner: Arc<VaultInner>,
}
impl<T> VaultJob<T> {
    pub fn wait(self) -> Result<T, ApplicationError> {
        self.response
            .recv()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
    }
}

#[derive(Clone, Copy)]
pub enum ManagedScanMode {
    Remote,
    Cached,
    CheckHealth,
}

struct VaultInner {
    lifecycle: crate::telegram::lifecycle::Lifecycle,
    sender: Mutex<Option<mpsc::SyncSender<VaultEnvelope>>>,
    transfer_sender: Mutex<Option<mpsc::SyncSender<VaultEnvelope>>>,
    scan_sender: Mutex<Option<mpsc::SyncSender<VaultEnvelope>>>,
    control_sender: Mutex<Option<mpsc::SyncSender<VaultEnvelope>>>,
    session: Arc<Mutex<VaultSession>>,
    transfers: Arc<TransferSnapshots<VaultTransferSnapshot>>,
    active_upload_batch: ActiveUploadBatch,
    joins: Mutex<Vec<JoinHandle<()>>>,
}

enum VaultCommand {
    StopUploadBatch {
        account: i64,
        batch: u64,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    ControlUpload {
        account: i64,
        id: u64,
        action: VaultUploadControl,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    RestoreUploadHistory {
        account: i64,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    #[cfg(test)]
    TestControlledUpload {
        lease: teleark_storage::VaultJobLease,
        entered: mpsc::SyncSender<crate::TelegramScanCancellation>,
        release: mpsc::Receiver<()>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    #[cfg(test)]
    TestScan {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        cancellation: crate::TelegramScanCancellation,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    #[cfg(test)]
    TestTransfer {
        entered: mpsc::SyncSender<std::sync::Weak<VaultMasterKey>>,
        release: mpsc::Receiver<()>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    StartNewEpoch {
        progress: VaultKeyProgress,
        password: Zeroizing<String>,
        reply: mpsc::SyncSender<Result<String, ApplicationError>>,
    },
    Initialize {
        password: Zeroizing<String>,
        reply: mpsc::SyncSender<Result<String, ApplicationError>>,
    },
    UnlockPassword {
        password: Zeroizing<String>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    UnlockRecovery {
        recovery_key: Zeroizing<String>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    RestoreRecovery {
        recovery_bundle: Zeroizing<String>,
        new_password: Zeroizing<String>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    Invalidate,
    ChangePassword {
        password: Zeroizing<String>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    RotateRecovery {
        reply: mpsc::SyncSender<Result<String, ApplicationError>>,
    },
    Scan {
        verify_health: bool,
        observer: Option<crate::ManagedScanObserver>,
        cached: bool,
        account_id: i64,
        chat_id: i64,
        cancellation: crate::TelegramScanCancellation,
        reply: mpsc::SyncSender<Result<ManagedVaultScan, ApplicationError>>,
    },
    Upload {
        account_id: i64,
        chat_id: i64,
        source: PathBuf,
        reply: mpsc::SyncSender<Result<ManagedVaultFile, ApplicationError>>,
    },
    ResumeQueuedTransfers {
        account_id: i64,
        reply: mpsc::SyncSender<Result<VaultUploadRecoveryReport, ApplicationError>>,
    },
    ResumeQueuedUploads {
        account_id: i64,
        reply: mpsc::SyncSender<Result<VaultUploadRecoveryReport, ApplicationError>>,
    },
    ResumeUpload {
        account_id: i64,
        task_id: u64,
        reply: mpsc::SyncSender<Result<ManagedVaultFile, ApplicationError>>,
    },
    UploadBatch {
        account_id: i64,
        chat_id: i64,
        sources: Vec<PathBuf>,
        progress: VaultUploadSelectionProgress,
        reply: mpsc::SyncSender<Result<VaultUploadReport, ApplicationError>>,
    },
    ResumeDownload {
        account_id: i64,
        task_id: u64,
        reply: mpsc::SyncSender<Result<PathBuf, ApplicationError>>,
    },
    Download {
        account_id: i64,
        chat_id: i64,
        package_id: u64,
        reply: mpsc::SyncSender<Result<PathBuf, ApplicationError>>,
    },
    Shutdown,
}

struct VaultOwner {
    catalog: catalog::ManifestCache,
    catalog_key_revision: u64,
    library: DesktopLibrary,
    telegram: DesktopTelegram,
    record: Option<VaultMetadataRecord>,
    master_key: Option<Arc<VaultMasterKey>>,
    historical_key: Option<([u8; 16], Arc<VaultMasterKey>)>,
    health_worker: Option<health::HealthWorker>,
    session: Arc<Mutex<VaultSession>>,
    session_generation: u64,
    transfers: Arc<TransferSnapshots<VaultTransferSnapshot>>,
    active_upload_batch: ActiveUploadBatch,
    upload_controls: UploadControls,
}

impl DesktopVault {
    pub fn new(
        telegram: DesktopTelegram,
        library: DesktopLibrary,
    ) -> Result<Self, ApplicationError> {
        library
            .worker
            .request("interrupt_previous_uploads", |reply| {
                crate::StorageRequest::InterruptVaultUploads { reply }
            })?;
        let record = library.worker.vault_metadata()?;
        if let Some(record) = record.as_ref() {
            validate_record(record)?;
        }
        let session = Arc::new(Mutex::new(VaultSession::new(record.clone())));
        let transfers = Arc::new(TransferSnapshots::new(Vec::new())?);
        let active_upload_batch: ActiveUploadBatch = Arc::new(Mutex::new(None));
        let upload_controls = UploadControls::default();
        let mut senders = Vec::new();
        let mut joins = Vec::new();
        for name in [
            "teleark-vault-keys",
            "teleark-vault-transfers",
            "teleark-vault-scan",
            "teleark-vault-controls",
        ] {
            let (sender, receiver) = mpsc::sync_channel(VAULT_QUEUE_CAPACITY);
            let owner = VaultOwner {
                catalog: catalog::ManifestCache::default(),
                catalog_key_revision: 0,
                library: library.clone(),
                telegram: telegram.clone(),
                record: record.clone(),
                master_key: None,
                historical_key: None,
                health_worker: None,
                session: session.clone(),
                session_generation: 0,
                transfers: transfers.clone(),
                active_upload_batch: active_upload_batch.clone(),
                upload_controls: upload_controls.clone(),
            };
            joins.push(
                thread::Builder::new()
                    .name(name.into())
                    .spawn(move || owner.run(receiver))
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?,
            );
            senders.push(sender);
        }
        let mut senders = senders.into_iter();
        Ok(Self {
            inner: Arc::new(VaultInner {
                lifecycle: telegram.lifecycle(),
                sender: Mutex::new(senders.next()),
                transfer_sender: Mutex::new(senders.next()),
                scan_sender: Mutex::new(senders.next()),
                control_sender: Mutex::new(senders.next()),
                session,
                transfers,
                active_upload_batch,
                joins: Mutex::new(joins),
            }),
        })
    }

    /// Background-only, works while the vault is locked and before network catalog loading.
    pub fn restore_upload_history(&self, account: i64) -> Result<(), ApplicationError> {
        self.submit_upload_history_restore(account)?.wait()
    }

    /// Admit in frontend request order; retain the job and wait off the UI thread.
    pub fn submit_upload_history_restore(
        &self,
        account: i64,
    ) -> Result<VaultJob<()>, ApplicationError> {
        self.submit(|reply| VaultCommand::RestoreUploadHistory { account, reply })
    }

    #[must_use]
    pub fn status(&self) -> VaultStatus {
        self.inner
            .session
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
    }

    #[must_use]
    pub fn transfers(&self) -> Vec<VaultTransferSnapshot> {
        self.inner.transfers.all().unwrap_or_default()
    }

    pub fn has_active_transfers(&self) -> bool {
        if self
            .inner
            .session
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending_transfers
            .load(Ordering::Acquire)
            != 0
        {
            return true;
        }
        self.inner
            .transfers
            .fold(false, |active, row| {
                active
                    || matches!(
                        row.state,
                        VaultTransferState::Queued
                            | VaultTransferState::Running
                            | VaultTransferState::Pausing
                            | VaultTransferState::Cancelling
                    )
            })
            .unwrap_or(true)
    }

    pub fn active_upload_batches(&self) -> Vec<(i64, u64)> {
        self.inner
            .transfers
            .fold(std::collections::BTreeSet::new(), |mut batches, row| {
                if matches!(
                    row.state,
                    VaultTransferState::Queued
                        | VaultTransferState::Running
                        | VaultTransferState::Pausing
                        | VaultTransferState::Cancelling
                ) && let Some(batch) = row.batch_id
                {
                    batches.insert((row.account_id, batch));
                }
                batches
            })
            .unwrap_or_default()
            .into_iter()
            .collect()
    }

    pub fn subscribe(&self) -> TransferSubscription {
        self.inner.transfers.subscribe()
    }

    pub fn snapshot_view(&self) -> Option<TransferSnapshotView<VaultTransferSnapshot>> {
        self.inner.transfers.view()
    }

    /// Call only after an explicit loss-of-keys confirmation. Existing wrapped
    /// keys and remote ciphertext are retained; no replacement channel is created.
    pub fn start_new_key_epoch(&self, password: String) -> Result<String, ApplicationError> {
        self.start_new_key_epoch_observed(password, VaultKeyProgress::new())
    }
    pub fn start_new_key_epoch_observed(
        &self,
        password: String,
        progress: VaultKeyProgress,
    ) -> Result<String, ApplicationError> {
        let observer = progress.clone();
        let result = self.request(|reply| VaultCommand::StartNewEpoch {
            progress,
            password: Zeroizing::new(password),
            reply,
        });
        if let Err(error) = &result {
            observer.finish(Some(error.kind()));
        }
        result
    }

    pub fn initialize(&self, password: String) -> Result<String, ApplicationError> {
        self.request(|reply| VaultCommand::Initialize {
            password: Zeroizing::new(password),
            reply,
        })
    }

    pub fn unlock_with_password(&self, password: String) -> Result<(), ApplicationError> {
        self.request(|reply| VaultCommand::UnlockPassword {
            password: Zeroizing::new(password),
            reply,
        })
    }

    pub fn unlock_with_recovery(&self, recovery_key: String) -> Result<(), ApplicationError> {
        self.request(|reply| VaultCommand::UnlockRecovery {
            recovery_key: Zeroizing::new(recovery_key),
            reply,
        })
    }

    pub fn restore_with_recovery(
        &self,
        recovery_bundle: String,
        new_password: String,
    ) -> Result<(), ApplicationError> {
        self.request(|reply| VaultCommand::RestoreRecovery {
            recovery_bundle: Zeroizing::new(recovery_bundle),
            new_password: Zeroizing::new(new_password),
            reply,
        })
    }

    pub fn lock(&self) -> Result<(), ApplicationError> {
        let generation = self
            .inner
            .session
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .lock();
        // Wake idle owners only. A full queue already guarantees cleanup at the
        // end of the running operation; never wait behind I/O to lock admission.
        for queue in [
            &self.inner.sender,
            &self.inner.transfer_sender,
            &self.inner.scan_sender,
        ] {
            if let Ok(sender) = queue.lock()
                && let Some(sender) = sender.as_ref()
            {
                let _ = sender.try_send(VaultEnvelope::invalidate(generation));
            }
        }
        Ok(())
    }

    pub fn change_password(&self, password: String) -> Result<(), ApplicationError> {
        self.request(|reply| VaultCommand::ChangePassword {
            password: Zeroizing::new(password),
            reply,
        })
    }

    pub fn rotate_recovery_key(&self) -> Result<String, ApplicationError> {
        self.request(|reply| VaultCommand::RotateRecovery { reply })
    }

    pub fn scan_managed_files(
        &self,
        account_id: i64,
        chat_id: i64,
        cancellation: crate::TelegramScanCancellation,
    ) -> Result<ManagedVaultScan, ApplicationError> {
        self.request(|reply| VaultCommand::Scan {
            verify_health: false,
            observer: None,
            cached: false,
            account_id,
            chat_id,
            cancellation,
            reply,
        })
    }

    /// Uses the unified local message catalog; authenticates only changed manifests.
    pub fn scan_cached_managed_files(
        &self,
        account_id: i64,
        chat_id: i64,
        cancellation: crate::TelegramScanCancellation,
    ) -> Result<ManagedVaultScan, ApplicationError> {
        self.request(|reply| VaultCommand::Scan {
            verify_health: false,
            observer: None,
            cached: true,
            account_id,
            chat_id,
            cancellation,
            reply,
        })
    }

    pub fn scan_cached_managed_files_observed(
        &self,
        account_id: i64,
        chat_id: i64,
        cancellation: crate::TelegramScanCancellation,
        observer: crate::ManagedScanObserver,
    ) -> Result<ManagedVaultScan, ApplicationError> {
        let failure_observer = observer.clone();
        let result = self.request(|reply| VaultCommand::Scan {
            verify_health: false,
            observer: Some(observer),
            cached: true,
            account_id,
            chat_id,
            cancellation,
            reply,
        });
        // Also covers queue rejection before the retained owner receives work.
        if let Err(error) = &result {
            failure_observer.finish(Some(error.kind()));
        }
        result
    }

    pub fn check_managed_file_health(
        &self,
        account_id: i64,
        chat_id: i64,
        cancellation: crate::TelegramScanCancellation,
        observer: Option<crate::ManagedScanObserver>,
    ) -> Result<ManagedVaultScan, ApplicationError> {
        self.request(|reply| VaultCommand::Scan {
            verify_health: true,
            observer,
            cached: true,
            account_id,
            chat_id,
            cancellation,
            reply,
        })
    }

    pub fn upload_file(
        &self,
        account_id: i64,
        chat_id: i64,
        source: PathBuf,
    ) -> Result<ManagedVaultFile, ApplicationError> {
        self.request(|reply| VaultCommand::Upload {
            account_id,
            chat_id,
            source,
            reply,
        })
    }

    pub fn submit_transfer_control(
        &self,
        account: i64,
        id: u64,
        action: VaultTransferControl,
    ) -> Result<VaultJob<()>, ApplicationError> {
        self.submit_upload_control(account, id, action)
    }

    pub fn submit_upload_control(
        &self,
        account: i64,
        id: u64,
        action: VaultUploadControl,
    ) -> Result<VaultJob<()>, ApplicationError> {
        self.submit(|reply| VaultCommand::ControlUpload {
            account,
            id,
            action,
            reply,
        })
    }

    /// Restores only queued work after unlock. Explicitly paused, cancelled,
    /// failed and unknown-version jobs are never silently restarted.
    /// Resume queued uploads and downloads for which this session has keys.
    pub fn submit_resume_queued_transfers(
        &self,
        account_id: i64,
    ) -> Result<VaultJob<VaultUploadRecoveryReport>, ApplicationError> {
        self.submit(|reply| VaultCommand::ResumeQueuedTransfers { account_id, reply })
    }

    pub fn submit_resume_queued_uploads(
        &self,
        account_id: i64,
    ) -> Result<VaultJob<VaultUploadRecoveryReport>, ApplicationError> {
        self.submit(|reply| VaultCommand::ResumeQueuedUploads { account_id, reply })
    }

    /// Resumes a paused/retryable durable upload using its original identity.
    /// The returned job is retained by the caller; waiting stays off the UI.
    pub fn submit_resume_upload(
        &self,
        account_id: i64,
        task_id: u64,
    ) -> Result<VaultJob<ManagedVaultFile>, ApplicationError> {
        self.submit(|reply| VaultCommand::ResumeUpload {
            account_id,
            task_id,
            reply,
        })
    }

    pub fn upload_files(
        &self,
        account_id: i64,
        chat_id: i64,
        sources: Vec<PathBuf>,
    ) -> Result<VaultUploadReport, ApplicationError> {
        self.submit_upload_files(account_id, chat_id, sources)?
            .wait()
    }

    /// Admission is bounded and does no filesystem, crypto or network work.
    pub fn submit_upload_files(
        &self,
        account_id: i64,
        chat_id: i64,
        sources: Vec<PathBuf>,
    ) -> Result<VaultJob<VaultUploadReport>, ApplicationError> {
        let progress = VaultUploadSelectionProgress::new(sources.len());
        self.submit_upload_files_observed(account_id, chat_id, sources, progress)
    }

    pub fn submit_upload_files_observed(
        &self,
        account_id: i64,
        chat_id: i64,
        sources: Vec<PathBuf>,
        progress: VaultUploadSelectionProgress,
    ) -> Result<VaultJob<VaultUploadReport>, ApplicationError> {
        if sources.is_empty() {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        self.submit(|reply| VaultCommand::UploadBatch {
            account_id,
            chat_id,
            sources,
            progress,
            reply,
        })
    }

    pub fn submit_stop_upload_batch(
        &self,
        account: i64,
        batch: u64,
    ) -> Result<VaultJob<()>, ApplicationError> {
        self.submit(|reply| VaultCommand::StopUploadBatch {
            account,
            batch,
            reply,
        })
    }

    /// Stops queued files after the current file finishes safely. This is an
    /// in-memory batch boundary, not a durable Vault pause/checkpoint promise.
    pub fn stop_upload_batch(
        &self,
        account_id: i64,
        batch_id: u64,
    ) -> Result<(), ApplicationError> {
        let active = self
            .inner
            .active_upload_batch
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let Some((owner, id, cancelled)) = active.as_ref() else {
            return Err(ApplicationError::new(ApplicationErrorKind::NotFound));
        };
        if *owner != account_id || *id != batch_id {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        cancelled.store(true, Ordering::Release);
        Ok(())
    }

    pub fn download_file(
        &self,
        account_id: i64,
        chat_id: i64,
        package_id: u64,
    ) -> Result<PathBuf, ApplicationError> {
        self.submit_download_file(account_id, chat_id, package_id)?
            .wait()
    }

    pub fn submit_resume_download(
        &self,
        account_id: i64,
        task_id: u64,
    ) -> Result<VaultJob<PathBuf>, ApplicationError> {
        self.submit(|reply| VaultCommand::ResumeDownload {
            account_id,
            task_id,
            reply,
        })
    }

    pub fn submit_download_file(
        &self,
        account_id: i64,
        chat_id: i64,
        package_id: u64,
    ) -> Result<VaultJob<PathBuf>, ApplicationError> {
        self.submit(|reply| VaultCommand::Download {
            account_id,
            chat_id,
            package_id,
            reply,
        })
    }

    pub fn submit_managed_scan(
        &self,
        account_id: i64,
        chat_id: i64,
        mode: ManagedScanMode,
        cancellation: crate::TelegramScanCancellation,
        observer: Option<crate::ManagedScanObserver>,
    ) -> Result<VaultJob<ManagedVaultScan>, ApplicationError> {
        let failure_observer = observer.clone();
        let result = self.submit(|reply| VaultCommand::Scan {
            account_id,
            chat_id,
            cached: !matches!(mode, ManagedScanMode::Remote),
            verify_health: matches!(mode, ManagedScanMode::CheckHealth),
            cancellation,
            observer,
            reply,
        });
        if let Err(error) = &result
            && let Some(observer) = failure_observer
        {
            observer.finish(Some(error.kind()));
        }
        result
    }

    fn request<T>(
        &self,
        build: impl FnOnce(mpsc::SyncSender<Result<T, ApplicationError>>) -> VaultCommand,
    ) -> Result<T, ApplicationError> {
        self.submit(build)?.wait()
    }

    fn submit<T>(
        &self,
        build: impl FnOnce(mpsc::SyncSender<Result<T, ApplicationError>>) -> VaultCommand,
    ) -> Result<VaultJob<T>, ApplicationError> {
        self.submit_in_session(None, build)
    }

    fn submit_in_session<T>(
        &self,
        revision: Option<(u64, u64)>,
        build: impl FnOnce(mpsc::SyncSender<Result<T, ApplicationError>>) -> VaultCommand,
    ) -> Result<VaultJob<T>, ApplicationError> {
        let (reply, response) = mpsc::sync_channel(1);
        let command = build(reply);
        let session = self
            .inner
            .session
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        if revision.is_some_and(|revision| revision != session.scan_revision()) {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        let envelope = session.admit(command)?;
        drop(session);
        let queue = if matches!(
            envelope.command,
            VaultCommand::ControlUpload { .. } | VaultCommand::StopUploadBatch { .. }
        ) {
            &self.inner.control_sender
        } else if envelope.command.is_transfer() {
            &self.inner.transfer_sender
        } else if envelope.command.is_scan() {
            &self.inner.scan_sender
        } else {
            &self.inner.sender
        };
        let sender = queue
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .clone()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        sender.try_send(envelope).map_err(|error| {
            ApplicationError::new(match error {
                mpsc::TrySendError::Full(_) => ApplicationErrorKind::Conflict,
                mpsc::TrySendError::Disconnected(_) => ApplicationErrorKind::Persistence,
            })
        })?;
        Ok(VaultJob {
            response,
            _owner: self.inner.clone(),
        })
    }
}

impl Drop for VaultInner {
    fn drop(&mut self) {
        if let Ok(mut session) = self.session.lock() {
            session.lock();
        }
        if let Ok(active) = self.active_upload_batch.lock()
            && let Some((_, _, cancel)) = active.as_ref()
        {
            cancel.store(true, Ordering::Release);
        }
        for queue in [
            &mut self.sender,
            &mut self.transfer_sender,
            &mut self.scan_sender,
            &mut self.control_sender,
        ] {
            if let Ok(sender) = queue.get_mut()
                && let Some(sender) = sender.take()
            {
                let _ = sender.try_send(VaultEnvelope::shutdown());
            }
        }
        if let Ok(joins) = self.joins.get_mut() {
            for join in joins.drain(..) {
                if join.is_finished() {
                    let _ = join.join();
                }
            }
        }
    }
}

impl VaultOwner {
    fn run(mut self, receiver: mpsc::Receiver<VaultEnvelope>) {
        while let Ok(envelope) = receiver.recv() {
            if matches!(envelope.command, VaultCommand::Shutdown) {
                break;
            }
            if self.catalog_key_revision != envelope.keys.revision {
                self.catalog.clear();
                self.catalog_key_revision = envelope.keys.revision;
            }
            self.session_generation = envelope.generation;
            if !envelope.command.is_key_operation() {
                self.record = envelope.keys.record.clone();
            }
            self.master_key = envelope.keys.active;
            self.historical_key = envelope.keys.historical;
            self.execute(envelope.command);
            // The operation owns these references only for its lifetime. A
            // locked session cannot borrow them to admit another operation.
            self.master_key = None;
            self.historical_key = None;
            if self
                .session
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .status
                .locked
            {
                self.catalog.clear();
            }
        }
    }

    fn execute(&mut self, command: VaultCommand) {
        match command {
            VaultCommand::StopUploadBatch {
                account,
                batch,
                reply,
            } => {
                let _ = reply.send(self.stop_saved_upload_batch(account, batch));
            }
            VaultCommand::ControlUpload {
                account,
                id,
                action,
                reply,
            } => {
                let _ = reply.send(self.control_upload(account, id, action));
            }
            VaultCommand::RestoreUploadHistory { account, reply } => {
                let _ = reply.send(self.restore_upload_history(account));
            }
            #[cfg(test)]
            VaultCommand::TestControlledUpload {
                lease,
                entered,
                release,
                reply,
            } => {
                let result = (|| {
                    let registration = self.upload_controls.register(lease)?;
                    let _ = entered.send(registration.cancellation.clone());
                    let _ = release.recv();
                    let mut database =
                        teleark_storage::Database::open(self.library.database_path.as_ref())
                            .map_err(|_| {
                                ApplicationError::new(ApplicationErrorKind::Persistence)
                            })?;
                    control::finish_job(&mut database, lease, Some(ApplicationErrorKind::Cancelled))
                        .map(|_| ())
                })();
                let _ = reply.send(result);
            }
            #[cfg(test)]
            VaultCommand::TestScan {
                entered,
                release,
                cancellation,
                reply,
            } => {
                let key = self.master_key.clone().expect("admitted scan key");
                let check_cancel = cancellation.clone();
                self.health_worker = Some(
                    health::HealthWorker::spawn(cancellation, move || {
                        let _ = entered.send(());
                        let _ = release.recv();
                        let result = if check_cancel.is_cancelled() {
                            Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
                        } else {
                            Ok(())
                        };
                        drop(key);
                        let _ = reply.send(result);
                    })
                    .expect("health worker"),
                );
            }
            #[cfg(test)]
            VaultCommand::TestTransfer {
                entered,
                release,
                reply,
            } => {
                let key = self.master_key.as_ref().expect("admitted key");
                let _ = entered.send(Arc::downgrade(key));
                let _ = release.recv();
                let _ = reply.send(Ok(()));
            }
            VaultCommand::StartNewEpoch {
                password,
                progress,
                reply,
            } => {
                let result = self.create_key_epoch_observed(&password, &progress);
                progress.finish(result.as_ref().err().map(ApplicationError::kind));
                let _ = reply.send(result);
            }
            VaultCommand::Initialize { password, reply } => {
                let _ = reply.send(self.initialize(&password));
            }
            VaultCommand::UnlockPassword { password, reply } => {
                let _ = reply.send(self.unlock_password(&password));
            }
            VaultCommand::UnlockRecovery {
                recovery_key,
                reply,
            } => {
                let _ = reply.send(self.unlock_recovery(&recovery_key));
            }
            VaultCommand::RestoreRecovery {
                recovery_bundle,
                new_password,
                reply,
            } => {
                let _ = reply.send(self.restore_recovery(&recovery_bundle, &new_password));
            }
            VaultCommand::Invalidate => {
                self.catalog.clear();
            }
            VaultCommand::ChangePassword { password, reply } => {
                let _ = reply.send(self.change_password(&password));
            }
            VaultCommand::RotateRecovery { reply } => {
                let _ = reply.send(self.rotate_recovery());
            }
            VaultCommand::Scan {
                verify_health,
                observer,
                cached,
                account_id,
                chat_id,
                cancellation,
                reply,
            } => {
                let observer =
                    observer.unwrap_or_else(|| crate::ManagedScanObserver::silent(chat_id));
                if verify_health && cached {
                    if self
                        .health_worker
                        .as_ref()
                        .is_some_and(|worker| !worker.is_finished())
                    {
                        observer.finish(Some(ApplicationErrorKind::Conflict));
                        let _ =
                            reply.send(Err(ApplicationError::new(ApplicationErrorKind::Conflict)));
                        return;
                    }
                    let active = self
                        .record
                        .as_ref()
                        .zip(self.master_key.as_ref())
                        .map(|(record, key)| (record.vault_id, Arc::clone(key)));
                    let historical = self
                        .historical_key
                        .as_ref()
                        .map(|(id, key)| (*id, Arc::clone(key)));
                    let telegram = self.telegram.clone();
                    let library = self.library.clone();
                    let failure_observer = observer.clone();
                    let worker_cancel = cancellation.clone();
                    let spawned = health::HealthWorker::spawn(cancellation, move || {
                        let result = (|| {
                            if active.is_none() && historical.is_none() {
                                return Err(ApplicationError::new(
                                    ApplicationErrorKind::Authorization,
                                ));
                            }
                            if worker_cancel.is_cancelled() {
                                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                            }
                            telegram.validate_storage_channel(account_id, chat_id)?;
                            health::full_check(
                                &telegram,
                                &library,
                                (account_id, chat_id),
                                active.as_ref().map(|(id, key)| (*id, key.as_ref())),
                                historical.as_ref(),
                                &worker_cancel,
                                &observer,
                            )
                        })();
                        observer.finish(result.as_ref().err().map(ApplicationError::kind));
                        let _ = reply.send(result);
                    });
                    match spawned {
                        Ok(worker) => self.health_worker = Some(worker),
                        Err(error) => failure_observer.finish(Some(error.kind())),
                    }
                    return;
                }
                let result = if cancellation.is_cancelled() {
                    Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
                } else if cached {
                    self.scan_cached(account_id, chat_id, cancellation, &observer)
                } else {
                    self.scan(account_id, chat_id, cancellation)
                };
                observer.finish(result.as_ref().err().map(ApplicationError::kind));
                let _ = reply.send(result);
            }
            VaultCommand::Upload {
                account_id,
                chat_id,
                source,
                reply,
            } => {
                let _ = reply.send(self.upload(account_id, chat_id, &source, None, None));
            }
            VaultCommand::ResumeQueuedTransfers { account_id, reply } => {
                let _ = reply.send(self.resume_queued_jobs(account_id, true));
            }
            VaultCommand::ResumeQueuedUploads { account_id, reply } => {
                let _ = reply.send(self.resume_queued_uploads(account_id));
            }
            VaultCommand::ResumeUpload {
                account_id,
                task_id,
                reply,
            } => {
                let _ = reply.send(self.resume_upload(account_id, task_id));
            }
            VaultCommand::UploadBatch {
                account_id,
                chat_id,
                sources,
                progress,
                reply,
            } => {
                let result = self.upload_batch_with_progress(
                    account_id,
                    chat_id,
                    sources,
                    &progress,
                    |owner| {
                        owner
                            .telegram
                            .validate_storage_channel(account_id, chat_id)
                            .map(|_| ())
                    },
                );
                let error = result
                    .as_ref()
                    .err()
                    .map(ApplicationError::kind)
                    .or_else(|| {
                        result.as_ref().ok().and_then(|report| {
                            report.failed.first().map(|f| f.kind).or_else(|| {
                                (!report.cancelled.is_empty())
                                    .then_some(ApplicationErrorKind::Cancelled)
                            })
                        })
                    });
                progress.finish(error);
                let _ = reply.send(result);
            }
            VaultCommand::ResumeDownload {
                account_id,
                task_id,
                reply,
            } => {
                let _ = reply.send(self.resume_download(account_id, task_id));
            }
            VaultCommand::Download {
                account_id,
                chat_id,
                package_id,
                reply,
            } => {
                let _ = reply.send(self.download(
                    account_id,
                    chat_id,
                    PackageId::new(package_id),
                    None,
                ));
            }
            VaultCommand::Shutdown => {}
        }
    }

    fn initialize(&mut self, password: &str) -> Result<String, ApplicationError> {
        if self.record.is_some() {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        self.create_key_epoch(password)
    }

    fn create_key_epoch(&mut self, password: &str) -> Result<String, ApplicationError> {
        self.create_key_epoch_observed(password, &VaultKeyProgress::new())
    }

    fn create_key_epoch_observed(
        &mut self,
        password: &str,
        progress: &VaultKeyProgress,
    ) -> Result<String, ApplicationError> {
        progress.phase(VaultKeyPhase::Generating)?;
        if let Some(worker) = &self.health_worker {
            worker.cancel();
        }
        let password = Password::new(password.as_bytes().to_vec()).map_err(map_crypto_error)?;
        let mut random = OsRandom;
        let mut vault_id = [0_u8; 16];
        random.fill_bytes(&mut vault_id).map_err(map_crypto_error)?;
        if self.library.worker.vault_key_epoch(vault_id)?.is_some() {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let master_key = generate_vault_master_key(&mut random).map_err(map_crypto_error)?;
        let recovery_key = generate_recovery_key(&mut random).map_err(map_crypto_error)?;
        let mut usage = AeadUsageRegistry::new();
        progress.phase(VaultKeyPhase::WrappingPassword)?;
        let password_wrap = wrap_master_key_with_password(
            &master_key,
            &password,
            vault_id,
            1,
            &mut random,
            &mut usage,
        )
        .map_err(map_crypto_error)?;
        progress.phase(VaultKeyPhase::WrappingRecovery)?;
        let recovery_wrap =
            wrap_master_key_with_recovery(&master_key, &recovery_key, vault_id, 1, &mut usage)
                .map_err(map_crypto_error)?;
        let now = now_unix_ms()?;
        let record = VaultMetadataRecord {
            vault_id,
            password_wrap: password_wrap.encode().map_err(map_crypto_error)?,
            recovery_wrap: recovery_wrap.encode(),
            password_generation: 1,
            recovery_generation: 1,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        progress.phase(VaultKeyPhase::Saving)?;
        self.library.worker.save_vault_metadata(
            record.clone(),
            self.record.as_ref().map(|old| {
                (
                    old.vault_id,
                    old.password_generation,
                    old.recovery_generation,
                )
            }),
        )?;
        let recovery_text = encode_recovery_bundle(&recovery_key, &recovery_wrap);
        self.record = Some(record);
        self.master_key = Some(Arc::new(master_key));
        self.historical_key = None;
        self.catalog.clear();
        self.refresh_status();
        Ok(recovery_text)
    }

    fn unlock_password(&mut self, password: &str) -> Result<(), ApplicationError> {
        let record = self
            .record
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        let password_wrap =
            PasswordWrap::decode(&record.password_wrap).map_err(map_crypto_error)?;
        let password = Password::new(password.as_bytes().to_vec()).map_err(map_crypto_error)?;
        let master = unwrap_master_key_with_password(&password_wrap, &password)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        self.master_key = Some(Arc::new(master));
        self.refresh_status();
        Ok(())
    }

    fn unlock_recovery(&mut self, recovery_text: &str) -> Result<(), ApplicationError> {
        if recovery_text.starts_with(RECOVERY_BUNDLE_PREFIX) {
            let (key, wrap) = decode_recovery_bundle(recovery_text)?;
            if self
                .record
                .as_ref()
                .is_some_and(|record| record.vault_id != wrap.vault_id)
            {
                let old = self
                    .library
                    .worker
                    .vault_key_epoch(wrap.vault_id)?
                    .ok_or_else(|| {
                        ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable)
                    })?;
                validate_record(&old)?;
                if old.recovery_wrap != wrap.encode() {
                    return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
                }
                let master = unwrap_master_key_with_recovery(&wrap, &key)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Authorization))?;
                // At most one historical key is unlocked. Uploads keep the active epoch.
                self.historical_key = Some((wrap.vault_id, Arc::new(master)));
                self.catalog.clear();
                self.refresh_status();
                return Ok(());
            }
        }
        let record = self
            .record
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        let recovery_wrap =
            RecoveryWrap::decode(&record.recovery_wrap).map_err(map_crypto_error)?;
        let recovery_key = if recovery_text.starts_with(RECOVERY_BUNDLE_PREFIX) {
            let (key, supplied_wrap) = decode_recovery_bundle(recovery_text)?;
            if supplied_wrap.encode() != record.recovery_wrap {
                return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
            }
            key
        } else {
            RecoveryKey::from_text(recovery_text)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Authorization))?
        };
        let master = unwrap_master_key_with_recovery(&recovery_wrap, &recovery_key)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        self.master_key = Some(Arc::new(master));
        self.refresh_status();
        Ok(())
    }

    fn restore_recovery(
        &mut self,
        recovery_bundle: &str,
        new_password: &str,
    ) -> Result<(), ApplicationError> {
        if self.record.is_some() {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let (recovery_key, recovery_wrap) = decode_recovery_bundle(recovery_bundle)?;
        let master = unwrap_master_key_with_recovery(&recovery_wrap, &recovery_key)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        let password = Password::new(new_password.as_bytes().to_vec()).map_err(map_crypto_error)?;
        let mut random = OsRandom;
        let mut usage = AeadUsageRegistry::new();
        let password_wrap = wrap_master_key_with_password(
            &master,
            &password,
            recovery_wrap.vault_id,
            1,
            &mut random,
            &mut usage,
        )
        .map_err(map_crypto_error)?;
        let now = now_unix_ms()?;
        let record = VaultMetadataRecord {
            vault_id: recovery_wrap.vault_id,
            password_wrap: password_wrap.encode().map_err(map_crypto_error)?,
            recovery_wrap: recovery_wrap.encode(),
            password_generation: 1,
            recovery_generation: recovery_wrap.recovery_generation,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        self.library.worker.save_vault_metadata(
            record.clone(),
            self.record.as_ref().map(|old| {
                (
                    old.vault_id,
                    old.password_generation,
                    old.recovery_generation,
                )
            }),
        )?;
        self.record = Some(record);
        self.master_key = Some(Arc::new(master));
        self.refresh_status();
        Ok(())
    }

    fn change_password(&mut self, new_password: &str) -> Result<(), ApplicationError> {
        let master = self
            .master_key
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        let mut record = self
            .record
            .clone()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        let generation = record
            .password_generation
            .checked_add(1)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        let password = Password::new(new_password.as_bytes().to_vec()).map_err(map_crypto_error)?;
        let mut random = OsRandom;
        let mut usage = AeadUsageRegistry::new();
        let wrapped = wrap_master_key_with_password(
            master,
            &password,
            record.vault_id,
            generation,
            &mut random,
            &mut usage,
        )
        .map_err(map_crypto_error)?;
        record.password_wrap = wrapped.encode().map_err(map_crypto_error)?;
        record.password_generation = generation;
        record.updated_at_unix_ms = now_unix_ms()?;
        self.library.worker.save_vault_metadata(
            record.clone(),
            self.record.as_ref().map(|old| {
                (
                    old.vault_id,
                    old.password_generation,
                    old.recovery_generation,
                )
            }),
        )?;
        self.record = Some(record);
        self.refresh_status();
        Ok(())
    }

    fn rotate_recovery(&mut self) -> Result<String, ApplicationError> {
        let master = self
            .master_key
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        let mut record = self
            .record
            .clone()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        let generation = record
            .recovery_generation
            .checked_add(1)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        let mut random = OsRandom;
        let recovery_key = generate_recovery_key(&mut random).map_err(map_crypto_error)?;
        let mut usage = AeadUsageRegistry::new();
        let wrapped = wrap_master_key_with_recovery(
            master,
            &recovery_key,
            record.vault_id,
            generation,
            &mut usage,
        )
        .map_err(map_crypto_error)?;
        record.recovery_wrap = wrapped.encode();
        record.recovery_generation = generation;
        record.updated_at_unix_ms = now_unix_ms()?;
        self.library.worker.save_vault_metadata(
            record.clone(),
            self.record.as_ref().map(|old| {
                (
                    old.vault_id,
                    old.password_generation,
                    old.recovery_generation,
                )
            }),
        )?;
        self.record = Some(record);
        self.refresh_status();
        Ok(encode_recovery_bundle(&recovery_key, &wrapped))
    }

    fn scan(
        &self,
        account_id: i64,
        chat_id: i64,
        cancellation: crate::TelegramScanCancellation,
    ) -> Result<ManagedVaultScan, ApplicationError> {
        let master = self
            .master_key
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        let mut store = TelegramObjectStore::new(self.telegram.clone(), account_id, chat_id)
            .with_cancellation(cancellation);
        let report = recover_remote_manifests(&mut store, master, MAX_MANIFEST_SCAN)
            .map_err(map_transfer_error)?;
        let mut files = report
            .recovered
            .iter()
            .map(managed_file_from_recovered)
            .collect::<Result<Vec<_>, _>>()?;
        files.sort_by(|left, right| {
            right
                .created_at_unix_ms
                .cmp(&left.created_at_unix_ms)
                .then_with(|| left.logical_name.cmp(&right.logical_name))
        });
        Ok(ManagedVaultScan {
            health_checked_files: None,
            files,
            rejected_manifests: report.rejected.len(),
            catalog_pending: false,
            catalog_limited: false,
        })
    }

    fn scan_cached(
        &mut self,
        account: i64,
        chat: i64,
        cancellation: crate::TelegramScanCancellation,
        observer: &crate::ManagedScanObserver,
    ) -> Result<ManagedVaultScan, ApplicationError> {
        let active = self
            .record
            .as_ref()
            .zip(self.master_key.as_deref())
            .map(|(record, key)| (record.vault_id, key));
        if active.is_none() && self.historical_key.is_none() {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        observer.phase(crate::ChannelSyncPhase::ManifestReading);
        let watch = self.library.managed_channel_watch(account, chat, false)?;
        let candidates = self.library.cached_manifest_candidates(account, chat)?;
        let mut store = TelegramObjectStore::new(self.telegram.clone(), account, chat)
            .with_cancellation(cancellation.clone());
        let library = &self.library;
        let historical = self.historical_key.as_ref();
        let mut report = self.catalog.project_observed(
            (account, chat),
            candidates,
            &cancellation,
            |candidate| {
                health::load(
                    &mut store,
                    library,
                    (account, chat),
                    catalog::byte_object(candidate)?,
                    active,
                    historical,
                    observer,
                )
            },
            Some(observer),
        )?;
        for file in &mut report.files {
            if file.health != crate::VaultFileHealth::KeyUnavailable {
                file.health = health::check(
                    library,
                    (account, chat),
                    file.manifest_message_id,
                    &file.part_message_ids,
                )?;
            }
        }
        health::retain_missing(
            &mut report,
            library,
            (account, chat),
            active,
            historical,
            &cancellation,
        )?;
        report.catalog_pending = !watch.catalog_ready;
        Ok(report)
    }

    fn upload_batch_with_progress(
        &mut self,
        account_id: i64,
        chat_id: i64,
        paths: Vec<PathBuf>,
        progress: &VaultUploadSelectionProgress,
        validate: impl FnOnce(&Self) -> Result<(), ApplicationError>,
    ) -> Result<VaultUploadReport, ApplicationError> {
        self.execute_upload_selection(
            (account_id, chat_id),
            paths,
            progress,
            validate,
            |owner, plan| owner.upload(account_id, chat_id, &plan.source.path, Some(plan), None),
        )
    }

    fn execute_upload_selection(
        &mut self,
        scope: (i64, i64),
        paths: Vec<PathBuf>,
        progress: &VaultUploadSelectionProgress,
        validate: impl FnOnce(&Self) -> Result<(), ApplicationError>,
        mut upload: impl FnMut(&mut Self, &QueuedUpload) -> Result<ManagedVaultFile, ApplicationError>,
    ) -> Result<VaultUploadReport, ApplicationError> {
        let (account_id, chat_id) = scope;
        if self.master_key.is_none() {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let sources = inspect_upload_sources_observed(&paths, Some(progress))?;
        drop(paths);
        progress.set_total(sources.len());
        let queued_at = now_unix_ms()?;
        let vault_id = self
            .record
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?
            .vault_id;
        progress.phase(VaultUploadSelectionPhase::SavingQueue);
        let mut plans = Vec::with_capacity(sources.len());
        for window in sources.chunks(VAULT_UPLOAD_WINDOW) {
            let batch_id = random_transfer_id()?;
            for source in window {
                progress.check_cancelled()?;
                let id = random_transfer_id()?;
                let mut pending = crate::VaultPendingUploadContext {
                    account_id,
                    task_id: id,
                    chat_id,
                    batch_id,
                    created_at_unix_ms: queued_at as u64,
                    vault_id,
                    master_key_generation: 1,
                    file_name: source.file_name.clone(),
                    source: source.path.clone(),
                    identity: teleark_transfer::SourceIdentity {
                        filesystem_id: 0,
                        size_bytes: source.size_bytes,
                        modified_at_units: 0,
                        revision: 0,
                    },
                };
                pending
                    .inspect_source(&source.path)
                    .map_err(map_transfer_error)?;
                if pending.identity.size_bytes != source.size_bytes {
                    return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
                }
                plans.push(QueuedUpload {
                    id,
                    batch_id,
                    queued_at,
                    source: source.clone(),
                    pending,
                    generation: 0,
                });
            }
        }
        let mut queue_database =
            teleark_storage::Database::open(self.library.database_path.as_ref())
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let mut admission_error = None;
        let admission =
            queue_database.admit_pending_vault_upload_selection(plans.iter().map(|plan| {
                let record = progress.check_cancelled().and_then(|()| {
                    plan.pending
                        .admission_record()
                        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))
                });
                record.map_err(|error| {
                    admission_error = Some(error);
                    teleark_storage::StorageError::InvalidInput {
                        field: "pending_upload.admission",
                        reason: teleark_storage::InputReason::InvalidCombination,
                    }
                })
            }));
        admission.map_err(|_| {
            admission_error
                .unwrap_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))
        })?;
        progress.saved(plans.len());

        let queued_telemetry = transfer_controller(
            true,
            0,
            self.library.preferences()?.transfer_soft_limit_policy,
        )?
        .snapshot();
        let cancel = progress.cancellation.clone();
        let mut validation = Some(validate);
        let mut policy = UploadBatchPolicy::default();
        let mut report = VaultUploadReport::default();
        let mut recent = std::collections::VecDeque::new();
        let mut recent_bytes = 0_usize;
        // Only one window of queue rows is live. Completed windows can be
        // evicted normally, so selections larger than history retention still
        // publish every running file and terminal result.
        let scheduling_result = (|| -> Result<(), ApplicationError> {
            for plans in plans.chunks(VAULT_UPLOAD_WINDOW) {
                let batch_id = plans[0].batch_id;
                *self
                    .active_upload_batch
                    .lock()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))? =
                    Some((account_id, batch_id, cancel.clone()));
                let snapshots = plans
                    .iter()
                    .map(|plan| VaultTransferSnapshot {
                        recovery_state: Some(teleark_storage::VaultJobState::Queued),
                        restored: false,
                        upload_activity: Some(VaultUploadActivity::new(
                            VaultUploadPhase::Persisting,
                        )),
                        id: plan.id,
                        account_id,
                        chat_id,
                        batch_id: Some(batch_id),
                        queued_at_unix_ms: queued_at,
                        direction: VaultTransferDirection::Upload,
                        file_name: plan.source.file_name.clone(),
                        package_id: None,
                        size_bytes: plan.source.size_bytes,
                        transferred_bytes: 0,
                        completed_parts: 0,
                        part_count: 0,
                        started_at_unix_ms: 0,
                        duration_ms: None,
                        average_bytes_per_second: None,
                        destination: None,
                        session_log_path: None,
                        telemetry: queued_telemetry.clone(),
                        state: VaultTransferState::Queued,
                    })
                    .collect::<Vec<_>>();
                self.admit_upload_window(snapshots)?;
                for plan in plans {
                    VaultUploadObserver::new(self.transfers.clone(), plan.id)
                        .phase(VaultUploadPhase::CheckingStorage);
                }
                if let Some(validate) = validation.take() {
                    progress.phase(VaultUploadSelectionPhase::CheckingStorage);
                    policy.blocked = if cancel.load(Ordering::Acquire) {
                        Some(ApplicationErrorKind::Cancelled)
                    } else {
                        validate(self).err().map(|error| error.kind())
                    };
                    if policy.blocked.is_none() {
                        progress.phase(VaultUploadSelectionPhase::Uploading);
                    }
                }
                if policy.blocked.is_none() {
                    for plan in plans {
                        self.update_transfer(plan.id, |snapshot| snapshot.upload_activity = None);
                    }
                }
                for plan in plans {
                    let result = policy.execute(&cancel, || upload(self, plan));
                    if let Err(error) = &result {
                        self.settle_pending_upload(plan, error.kind())?;
                    }
                    if self
                        .transfers
                        .get(plan.id)
                        .is_some_and(|row| row.state == VaultTransferState::Paused)
                    {
                        progress.record_paused();
                        report.paused_count += 1;
                        continue;
                    }
                    progress.record(result.as_ref().map(|_| ()).map_err(ApplicationError::kind));
                    match result {
                        Ok(file) => {
                            report.completed_count += 1;
                            // Receipts are already durable in the manifest catalog. Keep
                            // bounded recent metadata; never retain every large manifest.
                            recent_bytes = recent_bytes.saturating_add(file.estimated_bytes());
                            recent.push_back(file);
                            while recent.len() > VAULT_UPLOAD_WINDOW
                                || recent_bytes > 16 * 1024 * 1024
                            {
                                if let Some(file) = recent.pop_front() {
                                    recent_bytes =
                                        recent_bytes.saturating_sub(file.estimated_bytes());
                                }
                            }
                        }
                        Err(error) => {
                            self.update_transfer(plan.id, |snapshot| {
                                snapshot.state = if error.kind() == ApplicationErrorKind::Cancelled
                                {
                                    VaultTransferState::Cancelled
                                } else {
                                    VaultTransferState::Failed(error.kind())
                                }
                            });
                            self.persist_upload_id(plan.id)?;
                            if error.kind() == ApplicationErrorKind::Cancelled {
                                report.cancelled.push(plan.source.path.clone());
                            } else {
                                report.failed.push(VaultUploadFailure {
                                    source: plan.source.path.clone(),
                                    kind: error.kind(),
                                });
                            }
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = scheduling_result {
            // A rejected durable boundary must not leave the rest of the admitted window queued forever.
            for plan in &plans {
                self.settle_pending_upload(plan, error.kind())?;
            }
            self.fail_pending_upload_window(account_id, error.kind());
            let processed = report.completed_count
                + report.paused_count
                + report.failed.len()
                + report.cancelled.len();
            for source in &sources[processed..] {
                progress.record(Err(error.kind()));
                report.failed.push(VaultUploadFailure {
                    source: source.path.clone(),
                    kind: error.kind(),
                });
            }
        }
        *self
            .active_upload_batch
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        report.completed = recent.into_iter().collect();
        Ok(report)
    }

    fn settle_pending_upload(
        &self,
        plan: &QueuedUpload,
        failure: ApplicationErrorKind,
    ) -> Result<(), ApplicationError> {
        use teleark_storage::{
            Database, PendingVaultUploadState as S, VaultJobLease, VaultJobTransition as T,
        };
        let mut db = Database::open(self.library.database_path.as_ref())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let Some(saved) = db
            .pending_vault_upload(plan.pending.account_id, plan.id)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
        else {
            return Ok(());
        };
        if matches!(saved.state, S::Paused | S::Cancelled) {
            self.update_transfer(plan.id, |row| {
                row.state = if saved.state == S::Paused {
                    VaultTransferState::Paused
                } else {
                    VaultTransferState::Cancelled
                };
                row.upload_activity = None;
            });
        }
        if saved.state != S::Queued || saved.generation != plan.generation {
            return Ok(());
        }
        let cancelled = failure == ApplicationErrorKind::Cancelled;
        let action = if cancelled {
            T::RequestCancel
        } else if retryable_upload_failure(failure) {
            T::FailRetryable
        } else {
            T::FailBlocked
        };
        db.transition_pending_vault_upload(
            VaultJobLease {
                account_id: plan.pending.account_id,
                id: plan.id,
                generation: plan.generation,
            },
            S::Queued,
            action,
            (!cancelled).then(|| upload_history::error_code(failure)),
        )
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        Ok(())
    }

    fn resume_queued_uploads(
        &mut self,
        account_id: i64,
    ) -> Result<VaultUploadRecoveryReport, ApplicationError> {
        self.resume_queued_jobs(account_id, false)
    }

    fn resume_queued_jobs(
        &mut self,
        account_id: i64,
        include_downloads: bool,
    ) -> Result<VaultUploadRecoveryReport, ApplicationError> {
        let (account_revision, account, _) = self.telegram.lifecycle().snapshot();
        if account != Some(account_id) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let database = teleark_storage::Database::open(self.library.database_path.as_ref())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let mut report = VaultUploadRecoveryReport::default();
        let mut after = 0;
        loop {
            let (revision, account, _) = self.telegram.lifecycle().snapshot();
            if revision != account_revision || account != Some(account_id) {
                return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
            }
            let ids = database
                .vault_job_ids(
                    account_id,
                    teleark_storage::VaultJobState::Queued,
                    after,
                    32,
                )
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            if ids.is_empty() {
                break;
            }
            for id in ids {
                after = id;
                let record = match database.vault_job(account_id, id) {
                    Ok(Some(record)) => record,
                    Ok(_) => continue,
                    Err(_) => {
                        report.failed = report.failed.saturating_add(1);
                        continue;
                    }
                };
                if record.state != teleark_storage::VaultJobState::Queued {
                    continue;
                }
                let result = match record.direction {
                    teleark_storage::VaultJobDirection::Upload if self.master_key.is_some() => {
                        self.resume_upload(account_id, id).map(|_| ())
                    }
                    teleark_storage::VaultJobDirection::Download if include_downloads => {
                        self.resume_download(account_id, id).map(|_| ())
                    }
                    _ => continue,
                };
                match result {
                    Ok(()) => report.resumed = report.resumed.saturating_add(1),
                    Err(error) if error.kind() == ApplicationErrorKind::Cancelled => {}
                    Err(_) => report.failed = report.failed.saturating_add(1),
                }
            }
        }

        if self.master_key.is_some() {
            let mut after = 0;
            loop {
                let records = database
                    .queued_pending_vault_uploads(account_id, after, 32)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
                if records.is_empty() {
                    break;
                }
                for record in records {
                    after = record.id;
                    let (revision, account, _) = self.telegram.lifecycle().snapshot();
                    if revision != account_revision || account != Some(account_id) {
                        return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
                    }
                    match self.resume_pending_upload(account_id, record.id, true) {
                        Ok(_) => report.resumed = report.resumed.saturating_add(1),
                        Err(error) if error.kind() == ApplicationErrorKind::Cancelled => {}
                        Err(_) => report.failed = report.failed.saturating_add(1),
                    }
                }
            }
        }
        Ok(report)
    }

    fn resume_upload(
        &mut self,
        account_id: i64,
        task_id: u64,
    ) -> Result<ManagedVaultFile, ApplicationError> {
        use teleark_storage::{VaultJobLease, VaultJobState, VaultJobTransition};
        let (account_revision, account, _) = self.telegram.lifecycle().snapshot();
        if account != Some(account_id) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let master = self
            .master_key
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
        let mut db = teleark_storage::Database::open(self.library.database_path.as_ref())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let Some(mut record) = db
            .vault_job(account_id, task_id)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
        else {
            return self.resume_pending_upload(account_id, task_id, false);
        };
        let context = crate::VaultRecoveryContext::from_record(&record)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        context
            .file_key(master)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
        let crate::VaultRecoveryDirection::Upload { source, .. } = &context.direction else {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        };
        let lease = VaultJobLease {
            account_id,
            id: task_id,
            generation: record.generation,
        };
        let transition = match record.state {
            VaultJobState::Paused => Some(VaultJobTransition::Resume),
            VaultJobState::Retryable => Some(VaultJobTransition::Retry),
            VaultJobState::Queued => None,
            _ => return Err(ApplicationError::new(ApplicationErrorKind::Conflict)),
        };
        let (revision, account, _) = self.telegram.lifecycle().snapshot();
        if revision != account_revision || account != Some(account_id) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        if let Some(transition) = transition {
            if !db
                .transition_vault_job(lease, record.state, transition, now_unix_ms()?, None)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
            }
            record.state = VaultJobState::Queued;
            record.failure_code = None;
        }
        let previous = self.transfers.get(task_id);
        self.push_transfer(VaultTransferSnapshot {
            recovery_state: Some(VaultJobState::Queued),
            restored: false,
            upload_activity: Some(VaultUploadActivity::new(VaultUploadPhase::CheckingTarget)),
            id: task_id,
            account_id,
            chat_id: context.chat_id,
            batch_id: previous.as_ref().and_then(|row| row.batch_id),
            queued_at_unix_ms: context.created_at_unix_ms as i64,
            direction: VaultTransferDirection::Upload,
            file_name: context.file_name.clone(),
            package_id: previous.as_ref().and_then(|row| row.package_id.clone()),
            size_bytes: context.size_bytes,
            transferred_bytes: 0,
            completed_parts: 0,
            part_count: u32::try_from(
                encrypted_part_sizes(context.size_bytes)
                    .map_err(map_transfer_error)?
                    .len(),
            )
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?,
            started_at_unix_ms: now_unix_ms()?,
            duration_ms: None,
            average_bytes_per_second: None,
            destination: None,
            session_log_path: None,
            telemetry: transfer_controller(true, 0, teleark_transfer::SoftLimitPolicy::Respect)?
                .snapshot(),
            state: VaultTransferState::Queued,
        })?;
        let result = (|| {
            let saved_manifest = db
                .vault_manifest_outbox(account_id, task_id)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .is_some_and(|outbox| outbox.envelope.is_some());
            if saved_manifest {
                self.resume_manifest_upload(&mut db, &record, &context)
            } else {
                self.upload(account_id, context.chat_id, source, None, Some(record))
            }
        })();
        // Preflight can fail before the upload claims its Running lease. Keep
        // that failure durable rather than leaving an endlessly queued record.
        if let Err(error) = &result {
            if db
                .transition_vault_job(
                    lease,
                    VaultJobState::Queued,
                    VaultJobTransition::Start,
                    now_unix_ms()?,
                    None,
                )
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            {
                let failed = VaultJobLease {
                    generation: lease.generation + 1,
                    ..lease
                };
                db.transition_vault_job(
                    failed,
                    VaultJobState::Running,
                    if retryable_upload_failure(error.kind()) {
                        VaultJobTransition::FailRetryable
                    } else {
                        VaultJobTransition::FailBlocked
                    },
                    now_unix_ms()?,
                    Some(upload_history::error_code(error.kind())),
                )
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            }
            let recovery_state = db
                .vault_job(account_id, task_id)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .map(|record| record.state);
            let stopped = recovery_state.and_then(|state| match state {
                VaultJobState::Paused => Some(VaultTransferState::Paused),
                VaultJobState::Cancelled => Some(VaultTransferState::Cancelled),
                _ => None,
            });
            self.update_transfer(task_id, |snapshot| {
                snapshot.recovery_state = recovery_state;
                snapshot.state = stopped.unwrap_or(VaultTransferState::Failed(error.kind()));
                if stopped.is_some() {
                    snapshot.upload_activity = None;
                }
            });
            self.persist_upload_id(task_id)?;
        }
        result
    }

    fn upload(
        &mut self,
        account_id: i64,
        chat_id: i64,
        source: &Path,
        queued: Option<&QueuedUpload>,
        resumed: Option<teleark_storage::VaultJobRecord>,
    ) -> Result<ManagedVaultFile, ApplicationError> {
        if self.master_key.is_none() {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let mut preflight_registration = if let Some(record) = &resumed {
            let lease = teleark_storage::VaultJobLease {
                account_id,
                id: record.id,
                generation: record.generation,
            };
            let registration = self.register_transfer(lease)?;
            // Register first so a control arriving after this state read cannot
            // disappear between the queued check and source preparation.
            let database = teleark_storage::Database::open(self.library.database_path.as_ref())
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            let current = database
                .vault_job(account_id, record.id)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
            if current.generation != lease.generation
                || current.state != teleark_storage::VaultJobState::Queued
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            Some(registration)
        } else if let Some(plan) = queued {
            let lease = teleark_storage::VaultJobLease {
                account_id,
                id: plan.id,
                generation: plan.generation,
            };
            let registration = self.register_transfer(lease)?;
            let database = teleark_storage::Database::open(self.library.database_path.as_ref())
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            let current = database
                .pending_vault_upload(account_id, plan.id)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
            if current.generation != lease.generation
                || current.state != teleark_storage::PendingVaultUploadState::Queued
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            if current.record
                != plan
                    .pending
                    .admission_record()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                || plan.pending.chat_id != chat_id
                || self.record.as_ref().map(|record| record.vault_id) != Some(plan.pending.vault_id)
            {
                return Err(ApplicationError::new(
                    ApplicationErrorKind::VaultKeyUnavailable,
                ));
            }
            Some(registration)
        } else {
            None
        };
        let preflight_cancellation = preflight_registration
            .as_ref()
            .map(|owner| owner.cancellation.clone());
        // Destination policy is enforced before touching plaintext or allocating
        // keys. A frontend cannot turn Saved Messages or an arbitrary channel
        // into a TeleArk upload target by supplying its numeric id.
        if let Some(plan) = queued {
            VaultUploadObserver::new(self.transfers.clone(), plan.id)
                .phase(VaultUploadPhase::CheckingTarget);
            self.telegram.validate_storage_target(account_id, chat_id)?;
        } else {
            self.telegram
                .validate_storage_channel(account_id, chat_id)?;
        }
        if let Some(plan) = queued {
            VaultUploadObserver::new(self.transfers.clone(), plan.id)
                .phase(VaultUploadPhase::Preparing);
        }
        let restored_context = resumed
            .as_ref()
            .map(crate::VaultRecoveryContext::from_record)
            .transpose()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        let metadata = std::fs::metadata(source).map_err(map_source_io)?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceMissing));
        }
        if queued.is_some_and(|plan| {
            metadata.len() != plan.source.size_bytes
                || metadata.modified().ok() != plan.source.modified
        }) {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
        }
        let logical_name = if let Some(context) = &restored_context {
            context.file_name.clone()
        } else if let Some(plan) = queued {
            // A pending restart uses the canonical target path, which can have
            // a different basename from the originally selected symlink.
            plan.pending.file_name.clone()
        } else {
            source
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?
                .to_owned()
        };
        let part_sizes = encrypted_part_sizes(metadata.len()).map_err(map_transfer_error)?;
        let package_id = if let Some(context) = &restored_context {
            crate::transfer::package_id_from_bytes(context.package_id)
                .map_err(map_transfer_error)?
                .get()
        } else {
            random_nonzero_u64()?
        };
        let transfer_id = if let Some(record) = &resumed {
            record.id
        } else {
            queued
                .map(|plan| plan.id)
                .map_or_else(random_transfer_id, Ok)?
        };
        let part_count = u32::try_from(part_sizes.len())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        let started_at = now_unix_ms()?;
        let encryption_worker_count = recommended_encryption_worker_count();
        let mut controller = transfer_controller(
            true,
            encryption_worker_count,
            self.library.preferences()?.transfer_soft_limit_policy,
        )?;
        let mut session_log =
            TransferSessionLog::create(&self.library, TransferSessionKind::Vault, transfer_id)?;
        session_log.append_started(
            true,
            started_at,
            metadata.len(),
            part_count,
            &controller.snapshot(),
        )?;
        let previous = self.transfers.get(transfer_id);
        let initial_snapshot = VaultTransferSnapshot {
            recovery_state: resumed
                .as_ref()
                .map(|_| teleark_storage::VaultJobState::Queued),
            restored: false,
            upload_activity: Some(VaultUploadActivity::new(VaultUploadPhase::Preparing)),
            id: transfer_id,
            account_id,
            chat_id,
            batch_id: queued
                .map(|plan| plan.batch_id)
                .or_else(|| previous.as_ref().and_then(|row| row.batch_id)),
            queued_at_unix_ms: queued.map_or_else(
                || {
                    restored_context
                        .as_ref()
                        .map_or(started_at, |context| context.created_at_unix_ms as i64)
                },
                |plan| plan.queued_at,
            ),
            direction: VaultTransferDirection::Upload,
            file_name: logical_name.clone(),
            package_id: None,
            size_bytes: metadata.len(),
            transferred_bytes: 0,
            completed_parts: 0,
            part_count,
            started_at_unix_ms: started_at,
            duration_ms: None,
            average_bytes_per_second: None,
            destination: None,
            session_log_path: Some(session_log.path.clone()),
            telemetry: controller.snapshot(),
            state: VaultTransferState::Running,
        };
        if resumed.is_some() || queued.is_some() {
            // Resume already published this task. Preserve any Pause/Cancel
            // which arrived during target validation instead of replacing it.
            self.update_transfer(transfer_id, |row| {
                row.session_log_path = initial_snapshot.session_log_path;
                row.telemetry = initial_snapshot.telemetry;
                // Queue/history rows do not know part geometry yet. Preserve
                // control intent while refreshing fields needed by receipts.
                row.part_count = initial_snapshot.part_count;
                row.started_at_unix_ms = initial_snapshot.started_at_unix_ms;
                row.restored = false;
            });
        } else {
            self.push_transfer(initial_snapshot)?;
        }

        let started = Instant::now();
        let mut durable_database = None;
        let mut durable_lease = None;
        let mut result: Result<ManagedVaultFile, ApplicationError> = (|| {
            let mut files = NativeFileSystem::new();
            let source_id = SourceId(transfer_id);
            files
                .register_source(source_id, source)
                .map_err(map_transfer_error)?;
            let source_identity = files
                .source_identity(source_id)
                .map_err(map_transfer_error)?;
            if queued.is_some_and(|plan| {
                !crate::vault_recovery::upload_source_metadata_matches(
                    plan.pending.identity,
                    source_identity,
                )
            }) {
                return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
            }
            let observer = Arc::new(VaultUploadObserver::new(
                self.transfers.clone(),
                transfer_id,
            ));
            let source_digests = source_digest::inspect(
                source,
                &part_sizes,
                |bytes, total| {
                    observer.source_progress(bytes, total);
                    Ok(())
                },
                || {
                    preflight_cancellation
                        .as_ref()
                        .is_some_and(|token| token.is_cancelled())
                },
            )
            .map_err(map_transfer_error)?;
            if !crate::vault_recovery::upload_source_metadata_matches(
                source_identity,
                files
                    .source_identity(source_id)
                    .map_err(map_transfer_error)?,
            ) {
                return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
            }
            let master = self
                .master_key
                .as_ref()
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Authorization))?;
            let context = if let Some(context) = &restored_context {
                if context.account_id != account_id || context.chat_id != chat_id {
                    return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
                }
                context
                    .verify_upload_source(
                        &std::fs::canonicalize(source).map_err(map_source_io)?,
                        source_identity,
                        source_digests.whole.0,
                    )
                    .map_err(map_transfer_error)?;
                context.file_key(master).map_err(|_| {
                    ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable)
                })?;
                context.clone()
            } else {
                let file_key = generate_file_key(&mut OsRandom).map_err(map_crypto_error)?;
                let vault_id = self
                    .record
                    .as_ref()
                    .map(|record| record.vault_id)
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
                let package_bytes = crate::transfer::package_bytes(PackageId::new(package_id));
                let file_key_wrap = teleark_crypto::wrap_file_key(
                    master,
                    &file_key,
                    &vault_id,
                    &package_bytes,
                    1,
                    1,
                    &mut AeadUsageRegistry::new(),
                )
                .map_err(map_crypto_error)?;
                crate::VaultRecoveryContext {
                    account_id,
                    task_id: transfer_id,
                    chat_id,
                    package_id: package_bytes,
                    vault_id,
                    master_key_generation: 1,
                    file_key_wrap,
                    file_name: logical_name.clone(),
                    created_at_unix_ms: queued.map_or(started_at, |plan| plan.queued_at) as u64,
                    size_bytes: metadata.len(),
                    direction: crate::VaultRecoveryDirection::Upload {
                        source: std::fs::canonicalize(source).map_err(map_source_io)?,
                        identity: source_identity,
                        source_blake3: source_digests.whole.0,
                    },
                }
            };
            observer.phase(VaultUploadPhase::SavingRecovery);
            let mut database = teleark_storage::Database::open(self.library.database_path.as_ref())
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            let admission = context
                .admission_record()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            if let Some(plan) = queued {
                plan.pending
                    .verify_executable(&context)
                    .map_err(map_transfer_error)?;
                if !database
                    .promote_pending_vault_upload(
                        &plan.pending.admission_record().map_err(|_| {
                            ApplicationError::new(ApplicationErrorKind::Persistence)
                        })?,
                        plan.generation,
                        &admission,
                    )
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                {
                    return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                }
            } else {
                database
                    .admit_vault_job(&admission)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            }
            let mut lease = teleark_storage::VaultJobLease {
                account_id,
                id: transfer_id,
                generation: resumed.as_ref().map_or(0, |record| record.generation),
            };
            drop(preflight_registration.take());
            if !database
                .transition_vault_job(
                    lease,
                    teleark_storage::VaultJobState::Queued,
                    teleark_storage::VaultJobTransition::Start,
                    started_at,
                    None,
                )
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            lease.generation = lease
                .generation
                .checked_add(1)
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
            durable_lease = Some(lease);
            self.update_transfer(transfer_id, |row| {
                if matches!(
                    row.state,
                    VaultTransferState::Queued | VaultTransferState::Running
                ) {
                    row.recovery_state = Some(teleark_storage::VaultJobState::Running);
                    row.state = VaultTransferState::Running;
                }
            });
            durable_database = Some(database);
            let database = durable_database
                .as_mut()
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            let registration = self.register_transfer(lease)?;
            let cancellation = registration.cancellation.clone();
            let store = TelegramObjectStore::new(self.telegram.clone(), account_id, chat_id)
                .with_observer(observer.clone())
                .with_cancellation(cancellation.clone());
            let mut durable = crate::DurableUploadParts::open(database, lease, store, master)
                .map_err(map_transfer_error)?
                .with_cancellation(cancellation.clone());
            let mut encoded_size = 0_u64;
            let mut plaintext_offset = 0_u64;
            let mut pipeline_parts = Vec::with_capacity(part_sizes.len());
            let mut encryption_plans = Vec::with_capacity(part_sizes.len());
            for (position, plaintext_length) in part_sizes.iter().copied().enumerate() {
                let part_index = u32::try_from(position)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
                pipeline_parts.push(PipelinePart {
                    part_index,
                    plaintext_offset,
                    plaintext_length,
                });
                let digest =
                    source_digests.parts.get(position).copied().ok_or_else(|| {
                        ApplicationError::new(ApplicationErrorKind::SourceChanged)
                    })?;
                encryption_plans.push(Mutex::new(Some(
                    durable
                        .prepare_part_digest(database, part_index, plaintext_length, digest)
                        .map_err(map_transfer_error)?,
                )));
                plaintext_offset = plaintext_offset
                    .checked_add(plaintext_length)
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
            }
            let encryption_plans = Arc::new(encryption_plans);
            let source_path = source.to_owned();
            let whole_plaintext_hasher = Arc::new(Mutex::new(blake3::Hasher::new()));
            let reader_hasher = Arc::clone(&whole_plaintext_hasher);
            let pipeline_config = EncryptionPipelineConfig::new(
                usize::from(encryption_worker_count),
                1,
                usize::from(encryption_worker_count),
            )
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
            let completed_plaintext_bytes = Arc::new(Mutex::new(0_u64));
            let completed_part_count = Arc::new(Mutex::new(0_u32));
            let cumulative_encryption_micros = Arc::new(Mutex::new(0_u64));
            let progress_bytes = Arc::clone(&completed_plaintext_bytes);
            let progress_parts = Arc::clone(&completed_part_count);
            let encryption_time = Arc::clone(&cumulative_encryption_micros);
            observer.phase(VaultUploadPhase::Preparing);
            let reader_cancellation = cancellation.clone();
            let crypto_cancellation = cancellation.clone();
            let report = run_encryption_upload_pipeline(
                pipeline_config,
                &pipeline_parts,
                move |descriptor| {
                    if reader_cancellation.is_cancelled() {
                        return Err(TransferError::Cancelled);
                    }
                    let plaintext = read_source_part(&source_path, descriptor)?;
                    reader_hasher
                        .lock()
                        .map_err(|_| TransferError::SourceChanged)?
                        .update(&plaintext);
                    Ok(plaintext)
                },
                {
                    let encryption_plans = Arc::clone(&encryption_plans);
                    move |descriptor, plaintext| {
                        if crypto_cancellation.is_cancelled() {
                            return Err(TransferError::Cancelled);
                        }
                        let position = usize::try_from(descriptor.part_index)
                            .map_err(|_| TransferError::SourceChanged)?;
                        let job = encryption_plans
                            .get(position)
                            .ok_or(TransferError::SourceChanged)?
                            .lock()
                            .map_err(|_| TransferError::SourceChanged)?
                            .take()
                            .ok_or(TransferError::SourceChanged)?;
                        job.encrypt(plaintext)
                    }
                },
                |part_index, prepared| {
                    let plaintext_length = prepared.plaintext_length();
                    observer.begin_part(plaintext_length);
                    if let Ok(mut duration) = encryption_time.lock() {
                        *duration = duration.saturating_add(prepared.encryption_duration_micros());
                    }
                    let object = durable.publish_prepared(database, prepared)?;
                    encoded_size = encoded_size.saturating_add(object.encoded_size);
                    let transferred_bytes = progress_bytes.lock().map_or(0, |mut bytes| {
                        *bytes = bytes.saturating_add(plaintext_length);
                        *bytes
                    });
                    let completed_parts = progress_parts.lock().map_or(0, |mut count| {
                        *count = count.saturating_add(1);
                        *count
                    });
                    let elapsed_ms = u64::try_from(started.elapsed().as_millis())
                        .unwrap_or(u64::MAX)
                        .max(1);
                    let average_bytes_per_second = transferred_bytes
                        .saturating_mul(1_000)
                        .checked_div(elapsed_ms)
                        .unwrap_or_default();
                    let encryption_micros = cumulative_encryption_micros
                        .lock()
                        .map(|duration| (*duration).max(1))
                        .unwrap_or(1);
                    let encryption_bytes_per_second = transferred_bytes
                        .saturating_mul(1_000_000)
                        .checked_div(encryption_micros)
                        .unwrap_or_default();
                    let encryption_cpu_utilization_basis_points = encryption_micros
                        .saturating_mul(10_000)
                        .checked_div(
                            elapsed_ms
                                .saturating_mul(1_000)
                                .saturating_mul(u64::from(encryption_worker_count)),
                        )
                        .unwrap_or_default()
                        .min(10_000)
                        as u16;
                    let performance = PerformanceSample {
                        observed_at_millis: elapsed_ms,
                        goodput_bytes_per_second: average_bytes_per_second,
                        encryption_bytes_per_second,
                        cpu_utilization_basis_points: encryption_cpu_utilization_basis_points,
                        inflight_bytes: plaintext_length.saturating_mul(u64::from(
                            controller.parameters().inflight_parts_per_file,
                        )),
                        encrypted_queue_length: 0,
                        active_large_files: 1,
                        parts: PartCounters {
                            total_parts: u64::from(part_count),
                            completed_parts: u64::from(completed_parts),
                            inflight_parts: u64::from(part_count.saturating_sub(completed_parts)),
                            missing_parts: u64::from(part_count.saturating_sub(completed_parts)),
                            completed_parts_per_second_milli: u64::from(completed_parts)
                                .saturating_mul(1_000_000)
                                .checked_div(elapsed_ms)
                                .unwrap_or_default(),
                            ..PartCounters::default()
                        },
                        queues: QueueCounters {
                            large_files_active: 1,
                            large_queue_weight: 100,
                            ..QueueCounters::default()
                        },
                        memory: MemoryCounters {
                            plaintext_buffer_bytes: plaintext_length.saturating_mul(u64::from(
                                encryption_worker_count.saturating_add(1),
                            )),
                            encrypted_buffer_bytes: plaintext_length
                                .saturating_mul(u64::from(encryption_worker_count)),
                            network_inflight_bytes: object.encoded_size,
                            writer_queue_bytes: 0,
                        },
                        ..PerformanceSample::default()
                    };
                    let decision = controller.observe(performance);
                    let telemetry = controller.snapshot();
                    session_log
                        .append_part_confirmed(
                            part_index,
                            elapsed_ms,
                            plaintext_length,
                            &decision,
                            &telemetry,
                        )
                        .map_err(|_| TransferError::Database)?;
                    self.update_transfer(transfer_id, |snapshot| {
                        snapshot.transferred_bytes = transferred_bytes;
                        snapshot.completed_parts = completed_parts;
                        snapshot.average_bytes_per_second = Some(average_bytes_per_second);
                        snapshot.telemetry = telemetry;
                    });
                    observer.phase(VaultUploadPhase::Preparing);
                    Ok::<(), TransferError>(())
                },
            )
            .map_err(map_pipeline_error)?;
            if report.completed_parts != part_sizes.len() {
                return Err(ApplicationError::new(ApplicationErrorKind::Network));
            }
            let whole_digest = ContentDigest(
                *whole_plaintext_hasher
                    .lock()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                    .finalize()
                    .as_bytes(),
            );
            if !crate::vault_recovery::upload_source_metadata_matches(
                source_identity,
                files
                    .source_identity(source_id)
                    .map_err(map_transfer_error)?,
            ) {
                return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
            }
            if whole_digest != source_digests.whole {
                return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
            }
            observer.phase(VaultUploadPhase::Publishing);
            let manifest_object = durable
                .publish_manifest(
                    database,
                    self.master_key.as_ref().ok_or_else(|| {
                        ApplicationError::new(ApplicationErrorKind::Authorization)
                    })?,
                    ManifestPublishRequest {
                        vault_id: context.vault_id,
                        manifest_generation: 1,
                        master_key_generation: context.master_key_generation,
                        wrap_generation: context.file_key_wrap.wrap_generation,
                        created_at_unix_ms: context.created_at_unix_ms,
                        logical_name: logical_name.clone(),
                        relative_path: None,
                        mime_type: None,
                        media_kind: media_kind(crate::classify_file(source)),
                        whole_plaintext_blake3: whole_digest.0,
                    },
                )
                .map_err(map_transfer_error)?;
            let mut remote = durable.into_transport();
            self.catalog.remember_receipt(
                (account_id, chat_id),
                package_id,
                manifest_object.clone(),
            );
            observer.phase(VaultUploadPhase::Persisting);
            let sealed_manifest = remote
                .take_published_manifest_envelope()
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            let active = self
                .record
                .as_ref()
                .zip(self.master_key.as_deref())
                .map(|(record, key)| (record.vault_id, key));
            let manifest = health::open_with_keys(&sealed_manifest, active, None)?;
            let record = teleark_storage::VaultInventoryRecord {
                account_id,
                chat_id,
                manifest_message_id: manifest_object.object_id as i64,
                remote_name: manifest_object.name.clone(),
                vault_id: manifest.public_header.vault_id,
                sealed_manifest,
                observed_at_unix_ms: now_unix_ms()?,
                manifest_invalid: false,
            };
            self.library
                .worker
                .request("save_upload_manifest", |reply| {
                    crate::StorageRequest::SaveVaultInventory { record, reply }
                })?;
            let mut file = managed_file_from_recovered(&crate::RecoveredManifest {
                object: manifest_object,
                manifest,
            })?;
            file.health = crate::VaultFileHealth::Present;
            let ids = std::iter::once(file.manifest_message_id)
                .chain(file.part_message_ids.iter().copied())
                .collect::<Vec<_>>();
            for chunk in ids.chunks(100) {
                self.library.worker.request("save_upload_health", |reply| {
                    crate::StorageRequest::SaveVaultMessageHealth {
                        account: account_id,
                        chat: chat_id,
                        messages: chunk.iter().map(|id| (*id, true)).collect(),
                        observed_at: now_unix_ms().unwrap_or(0),
                        reply,
                    }
                })?;
            }
            Ok(file)
        })();
        if resumed.is_some() && durable_lease.is_none() && result.is_err() {
            // The resume owner settles an unclaimed preflight from the ledger.
            // Do not transiently publish Failed/Queued over its accepted stop.
            let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.update_transfer(transfer_id, |row| row.duration_ms = Some(elapsed));
            let _ = session_log.append_finished(
                elapsed,
                result.as_ref().err().map(ApplicationError::kind),
                &controller.snapshot(),
            );
            return result;
        }
        let mut stopped = None;
        if let (Some(database), Some(lease)) = (durable_database.as_mut(), durable_lease) {
            match control::finish_job(
                database,
                lease,
                result.as_ref().err().map(ApplicationError::kind),
            ) {
                Ok(teleark_storage::VaultJobState::Paused) => {
                    stopped = Some(VaultTransferState::Paused);
                    result = Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                }
                Ok(teleark_storage::VaultJobState::Cancelled) => {
                    stopped = Some(VaultTransferState::Cancelled);
                    result = Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                }
                Ok(_) => {}
                Err(error) => result = Err(error),
            }
        }
        if let Some(database) = durable_database.as_ref()
            && let Ok(Some(record)) = database.vault_job(account_id, transfer_id)
        {
            self.update_transfer(transfer_id, |row| row.recovery_state = Some(record.state));
        }
        if let Some(state) = stopped {
            self.update_transfer(transfer_id, |snapshot| {
                snapshot.state = state;
                snapshot.upload_activity = None;
            });
        } else if let Err(error) = &result {
            self.update_transfer(transfer_id, |snapshot| {
                snapshot.state = VaultTransferState::Failed(error.kind())
            });
        }
        let _ = session_log.append_finished(
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            result.as_ref().err().map(ApplicationError::kind),
            &controller.snapshot(),
        );
        self.finish_upload(
            transfer_id,
            started,
            result.as_ref().ok().map(|file| file.package_id.clone()),
        )?;
        result
    }

    fn resume_download(
        &mut self,
        account_id: i64,
        task_id: u64,
    ) -> Result<PathBuf, ApplicationError> {
        use teleark_storage::{VaultJobLease, VaultJobState, VaultJobTransition};
        let (revision, account, _) = self.telegram.lifecycle().snapshot();
        if account != Some(account_id) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let mut database = teleark_storage::Database::open(self.library.database_path.as_ref())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let mut record = database
            .vault_job(account_id, task_id)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        let context = crate::VaultRecoveryContext::from_record(&record)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        if !matches!(
            context.direction,
            crate::VaultRecoveryDirection::Download { .. }
        ) {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let package_id = crate::transfer::package_id_from_bytes(context.package_id)
            .map_err(map_transfer_error)?;
        let lease = VaultJobLease {
            account_id,
            id: task_id,
            generation: record.generation,
        };
        let transition = match record.state {
            VaultJobState::Paused => Some(VaultJobTransition::Resume),
            VaultJobState::Retryable => Some(VaultJobTransition::Retry),
            VaultJobState::Queued => None,
            _ => return Err(ApplicationError::new(ApplicationErrorKind::Conflict)),
        };
        let (current_revision, account, _) = self.telegram.lifecycle().snapshot();
        if revision != current_revision || account != Some(account_id) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        if let Some(transition) = transition {
            if !database
                .transition_vault_job(lease, record.state, transition, now_unix_ms()?, None)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
            }
            record.state = VaultJobState::Queued;
            record.failure_code = None;
        }
        self.update_transfer(task_id, |row| {
            row.state = VaultTransferState::Queued;
            row.recovery_state = Some(VaultJobState::Queued);
            row.upload_activity = None;
        });
        let result = self.download(account_id, context.chat_id, package_id, Some(record));
        if let Err(error) = &result {
            // Manifest fetch/key validation can fail before claiming the job.
            // Preserve explicit stop intent and make other failures durable.
            if database
                .transition_vault_job(
                    lease,
                    VaultJobState::Queued,
                    VaultJobTransition::Start,
                    now_unix_ms()?,
                    None,
                )
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            {
                let failed_lease = VaultJobLease {
                    generation: lease
                        .generation
                        .checked_add(1)
                        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?,
                    ..lease
                };
                control::finish_job(&mut database, failed_lease, Some(error.kind()))?;
            }
            let state = database
                .vault_job(account_id, task_id)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .map(|record| record.state);
            self.update_transfer(task_id, |row| {
                row.recovery_state = state;
                row.state = match state {
                    Some(VaultJobState::Paused) => VaultTransferState::Paused,
                    Some(VaultJobState::Cancelled) => VaultTransferState::Cancelled,
                    _ => VaultTransferState::Failed(error.kind()),
                };
            });
        }
        result
    }

    fn download(
        &mut self,
        expected_account_id: i64,
        chat_id: i64,
        package_id: PackageId,
        resumed: Option<teleark_storage::VaultJobRecord>,
    ) -> Result<PathBuf, ApplicationError> {
        let saved_context = resumed
            .as_ref()
            .map(crate::VaultRecoveryContext::from_record)
            .transpose()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        let expected_manifest = saved_context
            .as_ref()
            .map(|context| {
                if context.account_id != expected_account_id
                    || context.chat_id != chat_id
                    || context.package_id != package_bytes(package_id.get())
                {
                    return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
                }
                match &context.direction {
                    crate::VaultRecoveryDirection::Download {
                        manifest_message_id,
                        manifest_blake3,
                        ..
                    } => Ok((*manifest_message_id, *manifest_blake3)),
                    _ => Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest)),
                }
            })
            .transpose()?;
        let active = self
            .record
            .as_ref()
            .zip(self.master_key.as_deref())
            .map(|(record, key)| (record.vault_id, key));
        if active.is_none() && self.historical_key.is_none() {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let preflight_registration = resumed
            .as_ref()
            .map(|record| {
                self.upload_controls
                    .register(teleark_storage::VaultJobLease {
                        account_id: expected_account_id,
                        id: record.id,
                        generation: record.generation,
                    })
            })
            .transpose()?;
        let mut store =
            TelegramObjectStore::new(self.telegram.clone(), expected_account_id, chat_id);
        if let Some(registration) = &preflight_registration {
            store = store.with_cancellation(registration.cancellation.clone());
        }
        if expected_manifest.is_none() && expected_account_id != chat_id {
            let catalog = if let Some(object) = self
                .catalog
                .locator_for((expected_account_id, chat_id), package_id)
            {
                vec![object]
            } else if let Some((id, encoded_size)) =
                self.library
                    .worker
                    .request("vault_manifest_locator", |reply| {
                        crate::StorageRequest::VaultManifestLocator {
                            account: expected_account_id,
                            chat: chat_id,
                            name: teleark_crypto::remote_manifest_name(&package_bytes(
                                package_id.get(),
                            )),
                            reply,
                        }
                    })?
            {
                vec![crate::RemoteByteObject {
                    object_id: id as u64,
                    name: teleark_crypto::remote_manifest_name(&package_bytes(package_id.get())),
                    encoded_size,
                }]
            } else {
                self.library
                    .cached_manifest_candidates(expected_account_id, chat_id)?
                    .iter()
                    .take(MAX_MANIFEST_SCAN)
                    .map(catalog::byte_object)
                    .collect::<Result<Vec<_>, _>>()?
            };
            store = store.with_manifest_catalog(catalog);
        }
        let (recovered, manifest_blake3) = health::recover_target(
            &mut store,
            (expected_account_id, chat_id),
            package_id,
            expected_manifest,
            active,
            self.historical_key.as_ref(),
        )?;
        let logical_name = recovered.manifest.metadata.logical_name.clone();
        let size_bytes = recovered.manifest.public_header.logical_file_size;
        let part_count = recovered.manifest.public_header.part_count;
        let package_text = hex_id(&recovered.manifest.public_header.package_id);
        let account_id = recovered
            .manifest
            .metadata
            .parts
            .first()
            .map(|part| AccountId::new(part.remote_locator.account_id))
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        let destination = match saved_context.as_ref().map(|context| &context.direction) {
            Some(crate::VaultRecoveryDirection::Download { destination, .. }) => {
                destination.clone()
            }
            _ => self.library.next_download_destination(&logical_name)?,
        };
        let transfer_id = match &saved_context {
            Some(context) => context.task_id,
            None => random_transfer_id()?,
        };
        let started_at = now_unix_ms()?;
        let mut controller = transfer_controller(
            false,
            0,
            self.library.preferences()?.transfer_soft_limit_policy,
        )?;
        let mut session_log =
            TransferSessionLog::create(&self.library, TransferSessionKind::Vault, transfer_id)?;
        session_log.append_started(
            false,
            started_at,
            size_bytes,
            part_count,
            &controller.snapshot(),
        )?;
        let header = &recovered.manifest.public_header;
        let context = crate::VaultRecoveryContext {
            account_id: expected_account_id,
            task_id: transfer_id,
            chat_id,
            package_id: header.package_id,
            vault_id: header.vault_id,
            master_key_generation: header.master_key_generation,
            file_key_wrap: header.file_key_wrap.clone(),
            file_name: logical_name.clone(),
            created_at_unix_ms: header.created_at_unix_ms,
            size_bytes,
            direction: crate::VaultRecoveryDirection::Download {
                destination: destination.clone(),
                manifest_message_id: i64::try_from(recovered.object.object_id)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?,
                manifest_blake3,
                whole_plaintext_blake3: recovered.manifest.metadata.whole_plaintext_blake3,
            },
        };
        if saved_context
            .as_ref()
            .is_some_and(|saved| saved != &context)
        {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
        }
        let mut database = teleark_storage::Database::open(self.library.database_path.as_ref())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        database
            .admit_vault_job(
                &context
                    .admission_record()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?,
            )
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let mut lease = teleark_storage::VaultJobLease {
            account_id: expected_account_id,
            id: transfer_id,
            generation: resumed.as_ref().map_or(0, |record| record.generation),
        };
        drop(preflight_registration);
        if !database
            .transition_vault_job(
                lease,
                teleark_storage::VaultJobState::Queued,
                teleark_storage::VaultJobTransition::Start,
                started_at,
                None,
            )
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        lease.generation = lease
            .generation
            .checked_add(1)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        if let Err(error) = self.push_transfer(VaultTransferSnapshot {
            recovery_state: Some(teleark_storage::VaultJobState::Running),
            restored: false,
            upload_activity: None,
            id: transfer_id,
            account_id: expected_account_id,
            chat_id,
            batch_id: None,
            queued_at_unix_ms: started_at,
            direction: VaultTransferDirection::Download,
            file_name: logical_name,
            package_id: Some(package_text.clone()),
            size_bytes,
            transferred_bytes: 0,
            completed_parts: 0,
            part_count,
            started_at_unix_ms: started_at,
            duration_ms: None,
            average_bytes_per_second: None,
            destination: Some(destination.clone()),
            session_log_path: Some(session_log.path.clone()),
            telemetry: controller.snapshot(),
            state: VaultTransferState::Running,
        }) {
            control::finish_job(&mut database, lease, Some(error.kind()))?;
            return Err(error);
        }
        let started = Instant::now();
        let mut received_bytes = 0_u64;
        let mut result = (|| {
            let registration = self.register_transfer(lease)?;
            let store =
                TelegramObjectStore::new(self.telegram.clone(), expected_account_id, chat_id)
                    .with_cancellation(registration.cancellation.clone());
            let parts = recovered.manifest.metadata.parts.clone();
            let mut remote =
                EncryptedRemoteTransport::from_opened_manifest(store, recovered.manifest)
                    .map_err(map_transfer_error)?
                    .with_cancellation(registration.cancellation.clone());
            let mut durable = crate::durable_download::DurableDownload::open_cancellable(
                &database,
                lease,
                registration.cancellation.clone(),
            )
            .map_err(map_transfer_error)?;
            let mut completed_bytes = 0_u64;
            let mut completed_parts = 0_u32;
            for part in parts {
                let key = RemotePartKey {
                    account_id,
                    package_id,
                    part_index: PartIndex::new(part.part_index),
                };
                let extent_source = durable
                    .restore_part(&mut database, &part, || remote.download_manifest_part(key))
                    .map_err(map_transfer_error)?;
                if extent_source == crate::durable_download::ExtentSource::Remote {
                    received_bytes = received_bytes.saturating_add(part.plaintext_length);
                }
                completed_bytes = completed_bytes.saturating_add(part.plaintext_length);
                completed_parts = completed_parts.saturating_add(1);
                let elapsed_ms = u64::try_from(started.elapsed().as_millis())
                    .unwrap_or(u64::MAX)
                    .max(1);
                let average_bytes_per_second = received_bytes
                    .saturating_mul(1_000)
                    .checked_div(elapsed_ms)
                    .unwrap_or_default();
                let decision = controller.observe(PerformanceSample {
                    observed_at_millis: elapsed_ms,
                    goodput_bytes_per_second: average_bytes_per_second,
                    disk_bytes_per_second: average_bytes_per_second,
                    inflight_bytes: part.plaintext_length,
                    active_large_files: 1,
                    parts: PartCounters {
                        total_parts: u64::from(part_count),
                        completed_parts: u64::from(completed_parts),
                        inflight_parts: u64::from(part_count.saturating_sub(completed_parts)),
                        missing_parts: u64::from(part_count.saturating_sub(completed_parts)),
                        completed_parts_per_second_milli: u64::from(completed_parts)
                            .saturating_mul(1_000_000)
                            .checked_div(elapsed_ms)
                            .unwrap_or_default(),
                        ..PartCounters::default()
                    },
                    queues: QueueCounters {
                        large_files_active: 1,
                        large_queue_weight: 100,
                        ..QueueCounters::default()
                    },
                    memory: MemoryCounters {
                        writer_queue_bytes: part.plaintext_length,
                        ..MemoryCounters::default()
                    },
                    ..PerformanceSample::default()
                });
                let telemetry = controller.snapshot();
                session_log.append_part_confirmed(
                    part.part_index,
                    elapsed_ms,
                    part.plaintext_length,
                    &decision,
                    &telemetry,
                )?;
                self.update_transfer(transfer_id, |snapshot| {
                    snapshot.transferred_bytes = completed_bytes;
                    snapshot.completed_parts = completed_parts;
                    snapshot.average_bytes_per_second = Some(average_bytes_per_second);
                    snapshot.telemetry = telemetry;
                });
            }
            durable.finalize(&database).map_err(map_transfer_error)?;
            self.library
                .record_vault_download(teleark_storage::VaultDownloadRecord {
                    account_id: expected_account_id,
                    chat_id,
                    package_id: package_text.clone(),
                    destination: destination.clone(),
                    size_bytes,
                    completed_at_unix_ms: now_unix_ms()?,
                })?;
            Ok(destination.clone())
        })();
        let mut stopped = None;
        match control::finish_job(
            &mut database,
            lease,
            result.as_ref().err().map(ApplicationError::kind),
        ) {
            Ok(teleark_storage::VaultJobState::Paused) => {
                stopped = Some(VaultTransferState::Paused);
                result = Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            Ok(teleark_storage::VaultJobState::Cancelled) => {
                stopped = Some(VaultTransferState::Cancelled);
                result = Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            Ok(_) => {}
            Err(error) => result = Err(error),
        }
        let recovery_state = database
            .vault_job(expected_account_id, transfer_id)
            .ok()
            .flatten()
            .map(|record| record.state);
        self.update_transfer(transfer_id, |snapshot| {
            snapshot.recovery_state = recovery_state
        });
        let _ = session_log.append_finished(
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            result.as_ref().err().map(ApplicationError::kind),
            &controller.snapshot(),
        );
        self.finish_transfer(transfer_id, started, received_bytes);
        if let Err(error) = &result {
            self.update_transfer(transfer_id, |snapshot| {
                snapshot.state = stopped.unwrap_or(VaultTransferState::Failed(error.kind()));
            });
        }
        result
    }

    fn push_transfer(&self, snapshot: VaultTransferSnapshot) -> Result<(), ApplicationError> {
        if !self
            .transfers
            .insert_pruning(snapshot.clone(), vault_transfer_evictions)
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        if snapshot.direction == VaultTransferDirection::Upload {
            self.persist_upload(&snapshot)?;
        }
        Ok(())
    }

    fn update_transfer(&self, id: u64, update: impl FnOnce(&mut VaultTransferSnapshot)) {
        self.transfers.update(id, update);
    }

    fn finish_transfer(&self, id: u64, started: Instant, received_bytes: u64) {
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.update_transfer(id, |snapshot| {
            snapshot.duration_ms = Some(duration_ms);
            snapshot.average_bytes_per_second = received_bytes
                .saturating_mul(1_000)
                .checked_div(duration_ms.max(1));
            if snapshot.state == VaultTransferState::Running {
                snapshot.state = VaultTransferState::Completed;
            }
        });
    }

    fn refresh_status(&self) {
        if let Ok(mut session) = self.session.lock() {
            session.publish(
                self.session_generation,
                self.record.clone(),
                self.master_key.clone(),
                self.historical_key.clone(),
            );
        }
    }
}

pub(crate) struct TransferSessionLog {
    pub(crate) path: PathBuf,
    writer: crate::session_log_writer::SessionLogWriter,
}

#[derive(Clone, Copy)]
pub(crate) enum TransferSessionKind {
    NativeDownload,
    Vault,
}

impl TransferSessionKind {
    const fn file_prefix(self) -> &'static str {
        match self {
            Self::NativeDownload => "native-download",
            Self::Vault => "vault-transfer",
        }
    }
}

impl TransferSessionLog {
    pub(crate) fn create(
        library: &DesktopLibrary,
        session_kind: TransferSessionKind,
        transfer_id: u64,
    ) -> Result<Self, ApplicationError> {
        let directory = library.managed_directories()?.logs.join("Transfers");
        let path = directory.join(format!(
            "{}-{transfer_id}.jsonl",
            session_kind.file_prefix()
        ));
        // This constructor runs on the transfer owner, before network work.
        // Once opened, callbacks only stage bounded records for the log actor.
        let file = (|| -> std::io::Result<std::fs::File> {
            std::fs::create_dir_all(&directory)?;
            let mut options = std::fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let file = options.open(&path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(file)
        })();
        let file = match file {
            Ok(file) => Some(file),
            Err(error) => {
                tracing::warn!(event = "transfer.logs.open_failed", error_kind = ?error.kind(),
                    "diagnostic log unavailable; transfer remains enabled");
                None
            }
        };
        Ok(Self {
            path,
            writer: crate::session_log_writer::SessionLogWriter::new(file),
        })
    }

    pub(crate) fn append_started(
        &mut self,
        upload: bool,
        started_at_unix_ms: i64,
        total_bytes: u64,
        total_parts: u32,
        telemetry: &TransferTelemetrySnapshot,
    ) -> Result<(), ApplicationError> {
        let parameters = telemetry.parameters;
        writeln!(
            self.writer,
            "{{\"schema\":1,\"event\":\"session_started\",\"timestamp_ms\":{started_at_unix_ms},\"direction\":\"{}\",\"total_bytes\":{total_bytes},\"total_parts\":{total_parts},\"transfer_connections\":{},\"inflight_rpcs_per_connection\":{},\"active_files\":{},\"inflight_parts_per_file\":{},\"encryption_workers\":{},\"encrypted_queue_depth\":{},\"memory_budget_bytes\":{}}}",
            if upload { "upload" } else { "download" },
            parameters.transfer_connection_count,
            parameters.inflight_rpcs_per_connection,
            parameters.active_file_count,
            parameters.inflight_parts_per_file,
            parameters.encryption_worker_count,
            parameters.encrypted_part_queue_depth,
            telemetry.memory_budget_bytes,
        )
        .map_err(map_log_io)?;
        self.writer.flush().map_err(map_log_io)
    }

    pub(crate) fn append_part_confirmed(
        &mut self,
        part_index: u32,
        elapsed_ms: u64,
        plaintext_bytes: u64,
        decision: &teleark_transfer::ControllerDecision,
        telemetry: &TransferTelemetrySnapshot,
    ) -> Result<(), ApplicationError> {
        let before = decision.before;
        let after = decision.after;
        let lane_telemetry_json = lane_telemetry_json(&telemetry.lanes);
        writeln!(
            self.writer,
            "{{\"schema\":1,\"event\":\"part_confirmed\",\"elapsed_ms\":{elapsed_ms},\"part_index\":{part_index},\"plaintext_bytes\":{plaintext_bytes},\"phase\":\"{:?}\",\"parameter\":\"{:?}\",\"outcome\":\"{:?}\",\"reason\":\"{:?}\",\"affected_lane\":{},\"flood_wait_seconds\":{},\"goodput_before\":{},\"goodput_after\":{},\"goodput_change_basis_points\":{},\"before\":{{\"connections\":{},\"rpcs_per_connection\":{},\"active_files\":{},\"parts_per_file\":{},\"encryption_workers\":{},\"encrypted_queue_depth\":{}}},\"after\":{{\"connections\":{},\"rpcs_per_connection\":{},\"active_files\":{},\"parts_per_file\":{},\"encryption_workers\":{},\"encrypted_queue_depth\":{}}},\"rtt_p95_ms\":{},\"estimated_bdp_bytes\":{},\"inflight_bytes\":{},\"target_inflight_bytes\":{},\"encryption_bytes_per_second\":{},\"disk_bytes_per_second\":{},\"cpu_basis_points\":{},\"encrypted_queue_length\":{},\"network_waiting_for_encryption_ms\":{},\"encryption_waiting_for_network_ms\":{},\"plaintext_buffer_bytes\":{},\"encrypted_buffer_bytes\":{},\"network_inflight_bytes\":{},\"writer_queue_bytes\":{},\"small_queue_weight\":{},\"large_queue_weight\":{},\"retry_parts\":{},\"failed_parts\":{},\"lanes\":{}}}",
            decision.phase,
            decision.parameter,
            decision.outcome,
            decision.reason,
            optional_u16_json(decision.affected_lane),
            optional_u32_json(decision.flood_wait_seconds),
            decision.baseline_goodput_bytes_per_second,
            decision.observed_goodput_bytes_per_second,
            decision.goodput_change_basis_points,
            before.transfer_connection_count,
            before.inflight_rpcs_per_connection,
            before.active_file_count,
            before.inflight_parts_per_file,
            before.encryption_worker_count,
            before.encrypted_part_queue_depth,
            after.transfer_connection_count,
            after.inflight_rpcs_per_connection,
            after.active_file_count,
            after.inflight_parts_per_file,
            after.encryption_worker_count,
            after.encrypted_part_queue_depth,
            telemetry.round_trip_time_p95_millis,
            telemetry.estimated_bdp_bytes,
            telemetry.inflight_bytes,
            telemetry.target_inflight_bytes,
            telemetry.encryption_bytes_per_second,
            telemetry.disk_bytes_per_second,
            telemetry.cpu_utilization_basis_points,
            telemetry.encrypted_queue_length,
            telemetry.network_waiting_for_encryption_millis,
            telemetry.encryption_waiting_for_network_millis,
            telemetry.memory.plaintext_buffer_bytes,
            telemetry.memory.encrypted_buffer_bytes,
            telemetry.memory.network_inflight_bytes,
            telemetry.memory.writer_queue_bytes,
            telemetry.queues.small_queue_weight,
            telemetry.queues.large_queue_weight,
            telemetry.parts.retry_parts,
            telemetry.parts.failed_parts,
            lane_telemetry_json,
        )
        .map_err(map_log_io)?;
        self.writer.flush().map_err(map_log_io)
    }

    pub(crate) fn append_native_part_state(
        &mut self,
        event: teleark_telegram::DownloadPartEvent,
        elapsed_ms: u64,
    ) -> Result<(), ApplicationError> {
        writeln!(
            self.writer,
            "{{\"schema\":1,\"event\":\"part_state\",\"elapsed_ms\":{elapsed_ms},\"part_index\":{},\"offset_bytes\":{},\"length_bytes\":{},\"state\":\"{:?}\",\"attempt\":{},\"attempt_elapsed_ms\":{}}}",
            event.part_index,
            event.offset_bytes,
            event.length_bytes,
            event.state,
            event.attempt,
            event.elapsed_millis,
        )
        .map_err(map_log_io)?;
        self.writer.flush().map_err(map_log_io)
    }

    pub(crate) fn append_native_retry_decision(
        &mut self,
        part_index: u64,
        elapsed_ms: u64,
        decision: &teleark_transfer::ControllerDecision,
        telemetry: &TransferTelemetrySnapshot,
    ) -> Result<(), ApplicationError> {
        writeln!(
            self.writer,
            "{{\"schema\":1,\"event\":\"controller_decision\",\"trigger\":\"part_retry\",\"elapsed_ms\":{elapsed_ms},\"part_index\":{part_index},\"phase\":\"{:?}\",\"parameter\":\"{:?}\",\"outcome\":\"{:?}\",\"reason\":\"{:?}\",\"goodput_before\":{},\"goodput_after\":{},\"before_parts_per_file\":{},\"after_parts_per_file\":{},\"retry_parts\":{},\"failed_parts\":{}}}",
            decision.phase,
            decision.parameter,
            decision.outcome,
            decision.reason,
            decision.baseline_goodput_bytes_per_second,
            decision.observed_goodput_bytes_per_second,
            decision.before.inflight_parts_per_file,
            decision.after.inflight_parts_per_file,
            telemetry.parts.retry_parts,
            telemetry.parts.failed_parts,
        )
        .map_err(map_log_io)?;
        self.writer.flush().map_err(map_log_io)
    }

    pub(crate) fn append_finished(
        &mut self,
        elapsed_ms: u64,
        error_kind: Option<ApplicationErrorKind>,
        telemetry: &TransferTelemetrySnapshot,
    ) -> Result<(), ApplicationError> {
        writeln!(
            self.writer,
            "{{\"schema\":1,\"event\":\"session_finished\",\"elapsed_ms\":{elapsed_ms},\"result\":\"{}\",\"goodput_bytes_per_second\":{},\"completed_parts\":{},\"failed_parts\":{}}}",
            error_kind.map_or_else(|| "completed".to_owned(), |kind| format!("{kind:?}")),
            telemetry.goodput_bytes_per_second,
            telemetry.parts.completed_parts,
            telemetry.parts.failed_parts,
        )
        .map_err(map_log_io)?;
        self.writer.flush().map_err(map_log_io)
    }
}

fn optional_u16_json(value: Option<u16>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

fn optional_u32_json(value: Option<u32>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

fn lane_telemetry_json(lanes: &[teleark_transfer::LaneTelemetry]) -> String {
    let entries = lanes
        .iter()
        .map(|lane| {
            format!(
                "{{\"dc\":{},\"lane\":{},\"inflight\":{},\"throughput_bytes_per_second\":{},\"rtt_p95_ms\":{},\"errors\":{},\"requests\":{},\"paused_until_ms\":{}}}",
                lane.data_center_id,
                lane.lane_id,
                lane.inflight_rpc_count,
                lane.throughput_bytes_per_second,
                lane.round_trip_time_p95_millis,
                lane.error_count,
                lane.request_count,
                lane.paused_until_millis.map_or_else(|| "null".to_owned(), |value| value.to_string()),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{entries}]")
}

fn recommended_encryption_worker_count() -> u16 {
    thread::available_parallelism()
        .ok()
        .and_then(|count| u16::try_from(count.get()).ok())
        .unwrap_or(1)
        .clamp(1, MAX_ENCRYPTION_WORKERS)
}

fn vault_transfer_evictions(transfers: &[&VaultTransferSnapshot]) -> Option<Vec<u64>> {
    if transfers.len() < 256 {
        return Some(Vec::new());
    }
    let item = transfers
        .iter()
        .filter(|item| {
            (item.restored
                || !matches!(
                    item.state,
                    VaultTransferState::Queued
                        | VaultTransferState::Running
                        | VaultTransferState::Pausing
                        | VaultTransferState::Cancelling
                ))
                && item.batch_id.is_none_or(|batch| {
                    transfers.iter().all(|member| {
                        member.batch_id != Some(batch)
                            || member.account_id != item.account_id
                            || member.restored
                            || !matches!(
                                member.state,
                                VaultTransferState::Queued
                                    | VaultTransferState::Running
                                    | VaultTransferState::Pausing
                                    | VaultTransferState::Cancelling
                            )
                    })
                })
        })
        .min_by_key(|item| (item.queued_at_unix_ms, item.started_at_unix_ms, item.id))?;
    Some(match item.batch_id {
        Some(batch) => transfers
            .iter()
            .filter(|member| member.batch_id == Some(batch) && member.account_id == item.account_id)
            .map(|member| member.id)
            .collect(),
        None => vec![item.id],
    })
}

#[cfg(test)]
fn retain_transfer_snapshot(
    transfers: &mut Vec<VaultTransferSnapshot>,
    snapshot: VaultTransferSnapshot,
) {
    if let Some(existing) = transfers.iter_mut().find(|item| item.id == snapshot.id) {
        *existing = snapshot;
        return;
    }
    let Some(removed) = vault_transfer_evictions(&transfers.iter().collect::<Vec<_>>()) else {
        return;
    };
    transfers.retain(|item| !removed.contains(&item.id));
    transfers.push(snapshot);
}

fn transfer_controller(
    upload: bool,
    encryption_worker_count: u16,
    soft_limit_policy: teleark_transfer::SoftLimitPolicy,
) -> Result<AdaptiveTransferController, ApplicationError> {
    let available_parallelism = recommended_encryption_worker_count();
    let mut config = AdaptiveControllerConfig::maximum_throughput(
        TRANSFER_MEMORY_BUDGET_BYTES,
        available_parallelism,
    )
    .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.transfer_connections = ParameterBounds::new(1, 1, 1)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.inflight_rpcs_per_connection = ParameterBounds::new(1, 1, 1)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.active_files = ParameterBounds::new(1, 1, 1)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.inflight_parts_per_file = ParameterBounds::new(1, 1, 1)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.encryption_workers = ParameterBounds::new(
        encryption_worker_count.max(1),
        encryption_worker_count.max(1),
        1,
    )
    .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.encrypted_queue_depth = ParameterBounds::new(
        encryption_worker_count.max(1),
        encryption_worker_count.max(1),
        1,
    )
    .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    config.soft_limit_policy = soft_limit_policy;
    let parameters = if upload {
        TransferControlParameters {
            inflight_rpcs_per_connection: 1,
            inflight_parts_per_file: 1,
            encryption_worker_count: encryption_worker_count.max(1),
            encrypted_part_queue_depth: encryption_worker_count.max(1),
            ..TransferControlParameters::conservative_upload()
        }
    } else {
        TransferControlParameters {
            inflight_rpcs_per_connection: 1,
            inflight_parts_per_file: 1,
            ..TransferControlParameters::conservative_download()
        }
    };
    AdaptiveTransferController::with_initial_parameters(config, upload, parameters)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))
}

fn read_source_part(source: &Path, descriptor: PipelinePart) -> Result<Vec<u8>, TransferError> {
    let length =
        usize::try_from(descriptor.plaintext_length).map_err(|_| TransferError::SourceChanged)?;
    let mut file = std::fs::File::open(source).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => TransferError::SourceMissing,
        std::io::ErrorKind::PermissionDenied => TransferError::PermissionDenied,
        _ => TransferError::SourceChanged,
    })?;
    file.seek(SeekFrom::Start(descriptor.plaintext_offset))
        .map_err(|_| TransferError::SourceChanged)?;
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes)
        .map_err(|_| TransferError::SourceChanged)?;
    Ok(bytes)
}

fn map_pipeline_error(error: EncryptionPipelineError<TransferError>) -> ApplicationError {
    match error {
        EncryptionPipelineError::Read {
            source: TransferError::PermissionDenied,
            ..
        } => ApplicationError::new(ApplicationErrorKind::SourcePermissionDenied),
        EncryptionPipelineError::Read { source, .. }
        | EncryptionPipelineError::Encrypt { source, .. }
        | EncryptionPipelineError::Upload { source, .. } => map_transfer_error(source),
        EncryptionPipelineError::Configuration(_) => {
            ApplicationError::new(ApplicationErrorKind::InvalidRequest)
        }
        EncryptionPipelineError::WorkerStopped => {
            ApplicationError::new(ApplicationErrorKind::Network)
        }
    }
}

fn validate_record(record: &VaultMetadataRecord) -> Result<(), ApplicationError> {
    let password = PasswordWrap::decode(&record.password_wrap).map_err(map_crypto_error)?;
    let recovery = RecoveryWrap::decode(&record.recovery_wrap).map_err(map_crypto_error)?;
    if password.vault_id != record.vault_id
        || recovery.vault_id != record.vault_id
        || password.wrap_generation != record.password_generation
        || recovery.recovery_generation != record.recovery_generation
    {
        return Err(ApplicationError::new(ApplicationErrorKind::Persistence));
    }
    Ok(())
}

fn encode_recovery_bundle(recovery_key: &RecoveryKey, recovery_wrap: &RecoveryWrap) -> String {
    let key = recovery_key.expose_text();
    let encoded_wrap = recovery_wrap.encode();
    let mut output = String::with_capacity(
        RECOVERY_BUNDLE_PREFIX.len() + key.len() + 1 + encoded_wrap.len() * 2,
    );
    output.push_str(RECOVERY_BUNDLE_PREFIX);
    output.push_str(&key);
    output.push('-');
    push_upper_hex(&mut output, &encoded_wrap);
    output
}

fn decode_recovery_bundle(value: &str) -> Result<(RecoveryKey, RecoveryWrap), ApplicationError> {
    const KEY_TEXT_LENGTH: usize = 82;
    const WRAP_BYTES: usize = 88;
    let body = value
        .strip_prefix(RECOVERY_BUNDLE_PREFIX)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    let expected = KEY_TEXT_LENGTH + 1 + WRAP_BYTES * 2;
    if body.len() != expected || body.as_bytes().get(KEY_TEXT_LENGTH) != Some(&b'-') {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    let key_text = body
        .get(..KEY_TEXT_LENGTH)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    let wrap_text = body
        .get(KEY_TEXT_LENGTH + 1..)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    let key = RecoveryKey::from_text(key_text)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Authorization))?;
    let wrap_bytes = decode_upper_hex::<WRAP_BYTES>(wrap_text)?;
    let wrap = RecoveryWrap::decode(&wrap_bytes).map_err(map_crypto_error)?;
    unwrap_master_key_with_recovery(&wrap, &key)
        .map(|_| ())
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Authorization))?;
    Ok((key, wrap))
}

fn push_upper_hex(output: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in bytes {
        output.push(char::from(HEX[(byte >> 4) as usize]));
        output.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
}

fn decode_upper_hex<const N: usize>(value: &str) -> Result<[u8; N], ApplicationError> {
    if value.len() != N.saturating_mul(2) {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    let mut output = [0_u8; N];
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    for (index, pair) in pairs.iter().enumerate() {
        output[index] = (upper_hex_nibble(pair[0])? << 4) | upper_hex_nibble(pair[1])?;
    }
    Ok(output)
}

fn upper_hex_nibble(value: u8) -> Result<u8, ApplicationError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest)),
    }
}

fn status_for(record: Option<&VaultMetadataRecord>, locked: bool) -> VaultStatus {
    record.map_or_else(VaultStatus::unconfigured, |record| VaultStatus {
        active_key_locked: locked,
        historical_key_unlocked: false,
        active_vault_id: Some(record.vault_id),
        configured: true,
        locked,
        created_at_unix_ms: Some(record.created_at_unix_ms),
        password_generation: Some(record.password_generation),
        recovery_generation: Some(record.recovery_generation),
    })
}

fn managed_file_from_recovered(
    recovered: &crate::RecoveredManifest,
) -> Result<ManagedVaultFile, ApplicationError> {
    let manifest = &recovered.manifest;
    let encoded_size_bytes = manifest
        .metadata
        .parts
        .iter()
        .fold(recovered.object.encoded_size, |total, part| {
            total.saturating_add(part.encoded_length)
        });
    let mut related_remote_names = manifest
        .metadata
        .parts
        .iter()
        .map(|part| part.remote_locator.remote_name.clone())
        .collect::<Vec<_>>();
    related_remote_names.push(recovered.object.name.clone());
    Ok(ManagedVaultFile {
        vault_id: Some(manifest.public_header.vault_id),
        health: crate::VaultFileHealth::Unchecked,
        part_message_ids: manifest
            .metadata
            .parts
            .iter()
            .map(|part| part.remote_locator.message_id)
            .collect(),
        package_numeric_id: package_id_from_bytes(manifest.public_header.package_id)
            .map_err(map_transfer_error)?
            .get(),
        package_id: hex_id(&manifest.public_header.package_id),
        logical_name: manifest.metadata.logical_name.clone(),
        relative_path: manifest.metadata.relative_path.clone(),
        mime_type: manifest.metadata.mime_type.clone(),
        media_kind: file_kind(manifest.metadata.media_kind),
        size_bytes: manifest.public_header.logical_file_size,
        encoded_size_bytes,
        part_count: manifest.public_header.part_count,
        created_at_unix_ms: i64::try_from(manifest.public_header.created_at_unix_ms)
            .unwrap_or(i64::MAX),
        manifest_message_id: i64::try_from(recovered.object.object_id)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?,
        related_remote_names,
    })
}

fn package_bytes(package_id: u64) -> [u8; 16] {
    let mut bytes = *b"TARKPKG1\0\0\0\0\0\0\0\0";
    bytes[8..].copy_from_slice(&package_id.to_be_bytes());
    bytes
}

fn random_transfer_id() -> Result<u64, ApplicationError> {
    Ok((random_nonzero_u64()? & 0x1fff_ffff_ffff_ffff).max(1))
}

fn random_nonzero_u64() -> Result<u64, ApplicationError> {
    let mut bytes = [0_u8; 8];
    OsRandom.fill_bytes(&mut bytes).map_err(map_crypto_error)?;
    let value = u64::from_be_bytes(bytes) & i64::MAX as u64;
    Ok(value.max(1))
}

fn now_unix_ms() -> Result<i64, ApplicationError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    i64::try_from(duration.as_millis())
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))
}

fn media_kind(kind: FileKind) -> teleark_crypto::MediaKind {
    match kind {
        FileKind::Video => teleark_crypto::MediaKind::Video,
        FileKind::Document => teleark_crypto::MediaKind::Document,
        FileKind::Archive => teleark_crypto::MediaKind::Archive,
        FileKind::Audio => teleark_crypto::MediaKind::Audio,
        FileKind::Image => teleark_crypto::MediaKind::Image,
        FileKind::DiskImage => teleark_crypto::MediaKind::DiskImage,
        FileKind::Other => teleark_crypto::MediaKind::Other,
        _ => teleark_crypto::MediaKind::Other,
    }
}

fn file_kind(kind: teleark_crypto::MediaKind) -> FileKind {
    match kind {
        teleark_crypto::MediaKind::Video => FileKind::Video,
        teleark_crypto::MediaKind::Document => FileKind::Document,
        teleark_crypto::MediaKind::Archive => FileKind::Archive,
        teleark_crypto::MediaKind::Audio => FileKind::Audio,
        teleark_crypto::MediaKind::Image => FileKind::Image,
        teleark_crypto::MediaKind::DiskImage => FileKind::DiskImage,
        teleark_crypto::MediaKind::Other => FileKind::Other,
    }
}

fn map_source_io(error: std::io::Error) -> ApplicationError {
    ApplicationError::new(match error.kind() {
        std::io::ErrorKind::NotFound => ApplicationErrorKind::SourceMissing,
        std::io::ErrorKind::PermissionDenied => ApplicationErrorKind::SourcePermissionDenied,
        _ => ApplicationErrorKind::SourceChanged,
    })
}

fn map_log_io(error: std::io::Error) -> ApplicationError {
    ApplicationError::new(match error.kind() {
        std::io::ErrorKind::PermissionDenied => ApplicationErrorKind::PermissionDenied,
        std::io::ErrorKind::StorageFull => ApplicationErrorKind::Capacity,
        _ => ApplicationErrorKind::Persistence,
    })
}

fn map_crypto_error(error: teleark_crypto::CryptoError) -> ApplicationError {
    ApplicationError::new(match error {
        teleark_crypto::CryptoError::AuthenticationFailed => ApplicationErrorKind::Authorization,
        teleark_crypto::CryptoError::RandomSourceFailed => ApplicationErrorKind::Persistence,
        teleark_crypto::CryptoError::InvalidField { .. }
        | teleark_crypto::CryptoError::PasswordParametersRejected => {
            ApplicationErrorKind::InvalidRequest
        }
        _ => ApplicationErrorKind::Persistence,
    })
}

fn retryable_upload_failure(kind: ApplicationErrorKind) -> bool {
    matches!(
        kind,
        ApplicationErrorKind::Network
            | ApplicationErrorKind::Cancelled
            | ApplicationErrorKind::Server
            | ApplicationErrorKind::Persistence
            | ApplicationErrorKind::SourceMissing
            | ApplicationErrorKind::PermissionDenied
            | ApplicationErrorKind::SourcePermissionDenied
            | ApplicationErrorKind::StorageAccessDenied
            | ApplicationErrorKind::Authorization
            | ApplicationErrorKind::VaultKeyUnavailable
            | ApplicationErrorKind::Capacity
    )
}

fn map_transfer_error(error: TransferError) -> ApplicationError {
    if let TransferError::FloodWait { retry_after } = error {
        return ApplicationError::new(ApplicationErrorKind::Network).with_retry_after(retry_after);
    }
    ApplicationError::new(match error {
        TransferError::Network | TransferError::FloodWait { .. } => ApplicationErrorKind::Network,
        TransferError::Authorization
        | TransferError::AuthenticationFailed
        | TransferError::KeyUnavailable
        | TransferError::WrongPassword => ApplicationErrorKind::Authorization,
        TransferError::SourceMissing | TransferError::RemoteMissing => {
            ApplicationErrorKind::SourceMissing
        }
        TransferError::SourceChanged | TransferError::HashMismatch => {
            ApplicationErrorKind::SourceChanged
        }
        TransferError::DiskFull => ApplicationErrorKind::Capacity,
        TransferError::PermissionDenied => ApplicationErrorKind::PermissionDenied,
        TransferError::ManifestCorrupted | TransferError::UnsupportedManifestVersion { .. } => {
            ApplicationErrorKind::InvalidRequest
        }
        TransferError::Database => ApplicationErrorKind::Persistence,
        TransferError::Cancelled => ApplicationErrorKind::Cancelled,
        _ => ApplicationErrorKind::InvalidRequest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_resume_preserves_context_and_partial_on_preflight_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        use teleark_storage::{Database, VaultJobLease, VaultJobState, VaultJobTransition};
        let dir = tempfile::tempdir()?;
        let library = DesktopLibrary::open(dir.path().join("catalog.sqlite"))?;
        let telegram = DesktopTelegram::open_direct(dir.path().join("synthetic.session"))?;
        telegram.lifecycle().publish(1, Some(7), None);
        let vault = DesktopVault::new(telegram, library.clone())?;
        let master = Arc::new(VaultMasterKey::from_bytes([3; 32]));
        let destination = dir.path().join("restored.bin");
        let partial = dir.path().join("restored.bin.partial");
        std::fs::write(&partial, b"data")?;
        let package = crate::transfer::package_bytes(PackageId::new(1));
        let context = crate::VaultRecoveryContext {
            account_id: 7,
            task_id: 9,
            chat_id: 11,
            package_id: package,
            vault_id: [2; 16],
            master_key_generation: 1,
            file_key_wrap: teleark_crypto::wrap_file_key(
                &master,
                &teleark_crypto::FileKey::from_bytes([4; 32]),
                &[2; 16],
                &package,
                1,
                1,
                &mut teleark_crypto::AeadUsageRegistry::new(),
            )?,
            file_name: "restored.bin".into(),
            created_at_unix_ms: 100,
            size_bytes: 4,
            direction: crate::VaultRecoveryDirection::Download {
                destination: destination.clone(),
                manifest_message_id: 17,
                manifest_blake3: [5; 32],
                whole_plaintext_blake3: *blake3::hash(b"data").as_bytes(),
            },
        };
        let mut db = Database::open(library.database_path.as_ref())?;
        let original = context.admission_record().expect("valid download context");
        db.admit_vault_job(&original)?;
        let lease = VaultJobLease {
            account_id: 7,
            id: 9,
            generation: 0,
        };
        db.transition_vault_job(
            lease,
            VaultJobState::Queued,
            VaultJobTransition::RequestPause,
            101,
            None,
        )?;
        let mut corrupt = original.clone();
        corrupt.id = 12;
        corrupt.context = vec![0];
        db.admit_vault_job(&corrupt)?;
        db.transition_vault_job(
            VaultJobLease { id: 12, ..lease },
            VaultJobState::Queued,
            VaultJobTransition::RequestPause,
            102,
            None,
        )?;
        assert!(vault.status().locked);
        vault.restore_upload_history(7)?;
        let unavailable = vault
            .transfers()
            .into_iter()
            .find(|row| row.id == 12)
            .expect("damaged task remains visible");
        assert!(unavailable.file_name.is_empty());
        assert_eq!(
            unavailable.recovery_state, None,
            "damaged context cannot advertise resume"
        );
        assert_eq!(
            unavailable.state,
            VaultTransferState::Failed(ApplicationErrorKind::InvalidRequest)
        );
        let restored = vault
            .transfers()
            .into_iter()
            .find(|row| row.id == 9)
            .expect("paused download survives locked restart");
        assert_eq!(restored.state, VaultTransferState::Paused);
        assert_eq!(restored.recovery_state, Some(VaultJobState::Paused));
        assert_eq!(restored.destination, Some(destination.clone()));
        assert!(restored.restored);
        assert_eq!(
            restored.part_count, 0,
            "part geometry waits for authenticated manifest"
        );
        let mut other = context.clone();
        other.account_id = 8;
        other.file_name = "Other account.bin".into();
        db.admit_vault_job(&other.admission_record().expect("other account context"))?;
        vault.restore_upload_history(8)?;
        assert!(vault.transfers().iter().all(|row| row.account_id == 8));
        assert_eq!(
            vault.transfers()[0].file_name,
            "Other account.bin",
            "equal task IDs cannot reuse a foreign account projection"
        );
        vault.restore_upload_history(7)?;
        vault.inner.session.lock().expect("session").publish(
            0,
            None,
            None,
            Some(([2; 16], master.clone())),
        );
        for expected_generation in [1, 2] {
            assert_eq!(
                vault
                    .submit_resume_download(7, 9)?
                    .wait()
                    .expect_err("transport is deliberately unauthorized")
                    .kind(),
                ApplicationErrorKind::Authorization
            );
            let saved = db.vault_job(7, 9)?.expect("retained task");
            assert_eq!(saved.state, VaultJobState::Retryable);
            assert_eq!(saved.generation, expected_generation);
            assert_eq!(saved.context, original.context);
            assert_eq!(saved.package_id, original.package_id);
            assert_eq!(std::fs::read(&partial)?, b"data");
            assert!(!destination.exists());
        }
        let mut queued_download = context.clone();
        queued_download.task_id = 10;
        db.admit_vault_job(&queued_download.admission_record().expect("queued download"))?;
        let mut queued_upload = context.clone();
        queued_upload.task_id = 11;
        queued_upload.direction = crate::VaultRecoveryDirection::Upload {
            source: dir.path().join("source.bin"),
            identity: teleark_transfer::SourceIdentity {
                filesystem_id: 1,
                size_bytes: 4,
                modified_at_units: 1,
                revision: 1,
            },
            source_blake3: *blake3::hash(b"data").as_bytes(),
        };
        db.admit_vault_job(&queued_upload.admission_record().expect("queued upload"))?;
        assert_eq!(
            vault.submit_resume_queued_transfers(7)?.wait()?,
            VaultUploadRecoveryReport {
                resumed: 0,
                failed: 1
            }
        );
        assert_eq!(
            db.vault_job(7, 10)?.expect("download attempted").state,
            VaultJobState::Retryable
        );
        assert_eq!(
            db.vault_job(7, 11)?
                .expect("upload waits for active key")
                .state,
            VaultJobState::Queued
        );
        assert_eq!(
            db.vault_job(7, 11)?.expect("unclaimed upload").generation,
            0
        );
        assert_eq!(
            vault.submit_resume_queued_transfers(7)?.wait()?,
            VaultUploadRecoveryReport::default(),
            "failed downloads require explicit retry"
        );
        assert_eq!(
            vault
                .submit_resume_download(8, 9)?
                .wait()
                .expect_err("other account")
                .kind(),
            ApplicationErrorKind::Authorization
        );
        vault.lock()?;
        assert_eq!(
            vault
                .submit_resume_download(7, 9)
                .err()
                .expect("locked admission")
                .kind(),
            ApplicationErrorKind::VaultKeyUnavailable
        );
        assert_eq!(db.vault_job(7, 9)?.expect("unchanged").generation, 2);
        Ok(())
    }

    #[test]
    fn queued_recovery_preserves_pause_and_continues_after_an_unavailable_transport()
    -> Result<(), Box<dyn std::error::Error>> {
        use teleark_storage::{Database, VaultJobLease, VaultJobState, VaultJobTransition};
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3"))?;
        let telegram = DesktopTelegram::open_direct(temp.path().join("synthetic.session"))?;
        // Readiness projection only; the request owner remains unauthorized,
        // deterministically rejecting transport before network work.
        telegram.lifecycle().publish(1, Some(7), None);
        let master = Arc::new(VaultMasterKey::from_bytes([17; 32]));
        let mut owner = VaultOwner {
            catalog: catalog::ManifestCache::default(),
            catalog_key_revision: 0,
            telegram,
            library: library.clone(),
            record: None,
            master_key: Some(master.clone()),
            historical_key: None,
            health_worker: None,
            session: Arc::new(Mutex::new(VaultSession::new(None))),
            session_generation: 0,
            transfers: Arc::new(TransferSnapshots::new(Vec::new())?),
            active_upload_batch: Arc::new(Mutex::new(None)),
            upload_controls: UploadControls::default(),
        };
        let source = temp.path().join("source.bin");
        std::fs::write(&source, b"data")?;
        let mut files = NativeFileSystem::new();
        files.register_source(SourceId(1), &source).expect("source");
        let identity = files.source_identity(SourceId(1)).expect("identity");
        let mut db = Database::open(library.database_path.as_ref())?;
        for id in 1..=3 {
            let package_id = crate::transfer::package_bytes(PackageId::new(id));
            let file_key_wrap = teleark_crypto::wrap_file_key(
                &master,
                &teleark_crypto::FileKey::from_bytes([42; 32]),
                &[3; 16],
                &package_id,
                1,
                1,
                &mut AeadUsageRegistry::new(),
            )
            .expect("wrap");
            let context = crate::VaultRecoveryContext {
                account_id: 7,
                task_id: id,
                chat_id: 11,
                package_id,
                vault_id: [3; 16],
                master_key_generation: 1,
                file_key_wrap,
                file_name: "source.bin".into(),
                created_at_unix_ms: 100,
                size_bytes: 4,
                direction: crate::VaultRecoveryDirection::Upload {
                    source: source.clone(),
                    identity,
                    source_blake3: *blake3::hash(b"data").as_bytes(),
                },
            };
            db.admit_vault_job(&context.admission_record().expect("admission"))?;
        }
        db.transition_vault_job(
            VaultJobLease {
                account_id: 7,
                id: 3,
                generation: 0,
            },
            VaultJobState::Queued,
            VaultJobTransition::RequestPause,
            101,
            None,
        )?;
        db.transition_vault_job(
            VaultJobLease {
                account_id: 7,
                id: 1,
                generation: 0,
            },
            VaultJobState::Queued,
            VaultJobTransition::Start,
            102,
            None,
        )?;
        let _facade = DesktopVault::new(owner.telegram.clone(), library.clone())?;
        let recovered = db
            .vault_job(7, 1)?
            .expect("constructor recovered interrupted upload");
        assert_eq!(
            (recovered.state, recovered.generation),
            (VaultJobState::Queued, 2)
        );
        let report = owner.resume_queued_uploads(7)?;
        assert_eq!(
            report,
            VaultUploadRecoveryReport {
                resumed: 0,
                failed: 2
            }
        );
        for id in [1, 2] {
            assert_eq!(
                db.vault_job(7, id)?.expect("failed job").state,
                VaultJobState::Retryable
            );
            let row = owner.transfers.get(id).expect("visible failed recovery");
            assert_eq!(
                row.state,
                VaultTransferState::Failed(ApplicationErrorKind::Authorization)
            );
            assert!(!row.restored);
        }
        assert_eq!(
            db.vault_job(7, 3)?.expect("paused job").state,
            VaultJobState::Paused
        );
        assert_eq!(
            owner.resume_queued_uploads(7)?,
            VaultUploadRecoveryReport::default()
        );
        assert_eq!(
            owner
                .resume_queued_uploads(8)
                .expect_err("foreign account")
                .kind(),
            ApplicationErrorKind::Authorization
        );
        Ok(())
    }

    #[test]
    fn upload_preflight_distinguishes_folders_and_app_bundles_from_missing_files() {
        let root = tempfile::tempdir().expect("fixture");
        let folder = root.path().join("Photos");
        let bundle = root.path().join("Example.app");
        let ordinary_file = root.path().join("ordinary-file.app");
        let empty_file = root.path().join("empty.zip");
        std::fs::create_dir(&folder).expect("folder");
        std::fs::create_dir_all(bundle.join("Contents")).expect("application bundle");
        std::fs::write(&ordinary_file, b"ordinary file, despite its extension").expect("file");
        std::fs::write(&empty_file, []).expect("empty file");
        for path in [&folder, &bundle] {
            for paths in [
                vec![path.clone()],
                vec![ordinary_file.clone(), path.clone()],
            ] {
                assert_eq!(
                    inspect_upload_sources(&paths)
                        .expect_err("directory rejected")
                        .kind(),
                    ApplicationErrorKind::UploadFolderUnsupported,
                );
            }
        }
        #[cfg(unix)]
        {
            let alias = root.path().join("bundle-link");
            std::os::unix::fs::symlink(&bundle, &alias).expect("symlink");
            assert_eq!(
                inspect_upload_sources(&[alias])
                    .expect_err("directory alias rejected")
                    .kind(),
                ApplicationErrorKind::UploadFolderUnsupported,
            );
        }
        for path in [root.path().join("missing.zip"), empty_file] {
            assert_eq!(
                inspect_upload_sources(&[path])
                    .expect_err("unavailable source")
                    .kind(),
                ApplicationErrorKind::SourceMissing,
            );
        }
        assert_eq!(
            inspect_upload_sources(&[ordinary_file])
                .expect("ordinary file")
                .len(),
            1
        );
    }

    #[test]
    fn transfer_session_log_is_versioned_private_and_content_free()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let library = DesktopLibrary::open(directory.path().join("library.sqlite3"))?;
        let mut controller =
            transfer_controller(false, 0, teleark_transfer::SoftLimitPolicy::Respect)?;
        let mut log = TransferSessionLog::create(&library, TransferSessionKind::Vault, 41)?;
        let initial = controller.snapshot();
        log.append_started(false, 1_000, 2_048, 1, &initial)?;
        let decision = controller.observe(PerformanceSample {
            observed_at_millis: 25,
            goodput_bytes_per_second: 81_000_000,
            inflight_bytes: 1_048_576,
            ..PerformanceSample::default()
        });
        let telemetry = controller.snapshot();
        log.append_part_confirmed(0, 25, 2_048, &decision, &telemetry)?;
        log.append_finished(30, None, &telemetry)?;
        let path = log.path.clone();
        log.writer.wait_until_idle();
        drop(log);

        let contents = std::fs::read_to_string(&path)?;
        assert!(contents.contains("\"schema\":1"));
        assert!(contents.contains("\"event\":\"session_started\""));
        assert!(contents.contains("\"event\":\"part_confirmed\""));
        assert!(contents.contains("\"event\":\"session_finished\""));
        assert!(!contents.contains("library.sqlite3"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn upload_activity_advances_before_first_verified_part_and_never_claims_completion() {
        use teleark_telegram::{ByteTransferEvent, ByteTransferObserver};
        let fixture = VaultTransferSnapshot {
            recovery_state: None,
            restored: false,
            upload_activity: Some(VaultUploadActivity::new(VaultUploadPhase::Preparing)),
            id: 1,
            account_id: 7,
            chat_id: 90,
            batch_id: Some(1),
            queued_at_unix_ms: 1,
            direction: VaultTransferDirection::Upload,
            file_name: "京都 — fixture.bin".into(),
            package_id: None,
            size_bytes: 60 * 1024 * 1024,
            transferred_bytes: 0,
            completed_parts: 0,
            part_count: 1,
            started_at_unix_ms: 1,
            duration_ms: None,
            average_bytes_per_second: None,
            destination: None,
            session_log_path: None,
            telemetry: transfer_controller(true, 1, teleark_transfer::SoftLimitPolicy::Respect)
                .expect("controller")
                .snapshot(),
            state: VaultTransferState::Running,
        };
        let transfers = Arc::new(TransferSnapshots::new(vec![fixture]).expect("snapshot store"));
        let observer = VaultUploadObserver::new(transfers.clone(), 1);
        let snapshot = || transfers.get(1).expect("test snapshot");
        observer.source_progress(1024, 1024);
        let checked = snapshot();
        let checking = checked.upload_activity.expect("source checking");
        assert_eq!(checking.phase, VaultUploadPhase::CheckingSource);
        assert_eq!((checking.bytes, checking.total), (1024, 1024));
        assert_eq!(checking.uploaded_bytes, 0);
        assert_eq!(checked.transferred_bytes, 0);
        assert_eq!(checked.state, VaultTransferState::Running);
        observer.phase(VaultUploadPhase::SavingRecovery);
        assert_eq!(
            snapshot()
                .upload_activity
                .expect("saving recovery")
                .uploaded_bytes,
            0
        );
        observer.begin_part(60 * 1024 * 1024);
        observer.observe(ByteTransferEvent::Uploading {
            bytes: 512 * 1024,
            total: 64 * 1024 * 1024,
        });
        let row = snapshot();
        assert_eq!(row.transferred_bytes, 0);
        assert_eq!(row.completed_parts, 0);
        assert!(row.upload_activity.expect("upload activity").uploaded_bytes > 0);
        observer.observe(ByteTransferEvent::Uploading {
            bytes: 64 * 1024 * 1024,
            total: 64 * 1024 * 1024,
        });
        observer.observe(ByteTransferEvent::SendingMessage);
        assert_eq!(
            snapshot().upload_activity.expect("upload activity").phase,
            VaultUploadPhase::SendingMessage
        );
        observer.observe(ByteTransferEvent::Downloading {
            bytes: 512 * 1024,
            total: 64 * 1024 * 1024,
        });
        let row = snapshot();
        let activity = row.upload_activity.expect("upload activity");
        assert_eq!(activity.phase, VaultUploadPhase::Verifying);
        assert_eq!(activity.uploaded_bytes, row.size_bytes);
        assert_eq!(row.transferred_bytes, 0);
        assert_eq!(row.state, VaultTransferState::Running);
        observer.phase(VaultUploadPhase::Publishing);
        observer.observe(ByteTransferEvent::Uploading {
            bytes: 100,
            total: 100,
        });
        observer.observe(ByteTransferEvent::Downloading {
            bytes: 100,
            total: 100,
        });
        assert_eq!(
            snapshot().upload_activity.expect("upload activity").phase,
            VaultUploadPhase::Publishing
        );
        transfers.update(1, |row| {
            row.state = VaultTransferState::Failed(ApplicationErrorKind::Network)
        });
        let before = snapshot();
        observer.phase(VaultUploadPhase::Preparing);
        observer.observe(ByteTransferEvent::Uploading {
            bytes: u64::MAX,
            total: 0,
        });
        assert_eq!(snapshot(), before);
    }

    #[test]
    fn batch_publishes_queue_before_remote_validation_and_keeps_failures_visible()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let paths = (0..513)
            .map(|i| temp.path().join(format!("source-{i:04}.txt")))
            .collect::<Vec<_>>();
        for path in &paths {
            std::fs::write(path, b"synthetic upload")?;
        }
        for cancel_during_validation in [false, true] {
            let library = DesktopLibrary::open(temp.path().join("catalog.sqlite3"))?;
            assert_eq!(library.storage_channel_id(100)?, None);
            let mut owner = VaultOwner {
                catalog: catalog::ManifestCache::default(),
                catalog_key_revision: 0,
                telegram: DesktopTelegram::open_direct(temp.path().join("test.session"))?,
                library,
                record: Some(VaultMetadataRecord {
                    vault_id: [3; 16],
                    password_wrap: vec![],
                    recovery_wrap: vec![],
                    password_generation: 1,
                    recovery_generation: 1,
                    created_at_unix_ms: 1,
                    updated_at_unix_ms: 1,
                }),
                master_key: Some(Arc::new(VaultMasterKey::from_bytes([7; 32]))),
                historical_key: None,
                health_worker: None,
                session: Arc::new(Mutex::new(VaultSession::new(None))),
                session_generation: 0,
                transfers: Arc::new(TransferSnapshots::new(Vec::new()).expect("snapshot store")),
                active_upload_batch: Arc::new(Mutex::new(None)),
                upload_controls: UploadControls::default(),
            };
            let mut calls = 0;
            let mut admitted_ids = Vec::new();
            let progress = VaultUploadSelectionProgress::new(paths.len());
            let report =
                owner.upload_batch_with_progress(100, 700, paths.clone(), &progress, |owner| {
                    calls += 1;
                    let database =
                        teleark_storage::Database::open(owner.library.database_path.as_ref())
                            .map_err(|_| {
                                ApplicationError::new(ApplicationErrorKind::Persistence)
                            })?;
                    let mut after = 0;
                    loop {
                        let page = database
                            .queued_pending_vault_uploads(100, after, 128)
                            .map_err(|_| {
                                ApplicationError::new(ApplicationErrorKind::Persistence)
                            })?;
                        if page.is_empty() {
                            break;
                        }
                        for record in page {
                            after = record.id;
                            let context = crate::VaultPendingUploadContext::from_record(&record)
                                .expect("real pending codec");
                            assert_eq!(context.vault_id, [3; 16]);
                            assert!(
                                database
                                    .vault_job(100, record.id)
                                    .expect("formal ledger")
                                    .is_none()
                            );
                            admitted_ids.push(record.id);
                        }
                    }
                    assert_eq!(
                        admitted_ids.len(),
                        paths.len(),
                        "every window is durable before remote validation"
                    );
                    assert_eq!(progress.snapshot().saved, paths.len());

                    let rows = owner.transfers.all().expect("snapshots");
                    assert_eq!(rows.len(), VAULT_UPLOAD_WINDOW);
                    assert!(
                        rows.iter()
                            .all(|row| row.state == VaultTransferState::Queued)
                    );
                    assert!(
                        rows.iter()
                            .all(|row| row.transferred_bytes == 0 && row.package_id.is_none())
                    );
                    assert!(rows.iter().all(|row| {
                        row.upload_activity.as_ref().is_some_and(|activity| {
                            activity.phase == VaultUploadPhase::CheckingStorage
                        })
                    }));
                    let active = owner.active_upload_batch.lock().expect("active batch");
                    let (_, _, cancel) = active.as_ref().expect("cancellable before network");
                    if cancel_during_validation {
                        cancel.store(true, Ordering::Release);
                        Ok(())
                    } else {
                        Err(ApplicationError::new(ApplicationErrorKind::Conflict))
                    }
                })?;
            assert_eq!(calls, 1);
            let database = teleark_storage::Database::open(owner.library.database_path.as_ref())?;
            for id in admitted_ids {
                let saved = database
                    .pending_vault_upload(100, id)?
                    .expect("retained pending result");
                assert_eq!(
                    saved.state,
                    if cancel_during_validation {
                        teleark_storage::PendingVaultUploadState::Cancelled
                    } else {
                        teleark_storage::PendingVaultUploadState::Blocked
                    }
                );
            }

            let retained = owner.transfers.all().expect("bounded history");
            assert!(retained.len() <= 256);
            assert!(
                retained
                    .iter()
                    .any(|row| row.file_name == "source-0512.txt"),
                "later windows must remain visible"
            );
            assert_eq!(progress.snapshot().total, paths.len());
            assert!(progress.snapshot().timeline.len() <= 6);
            assert!(report.completed.is_empty());
            if cancel_during_validation {
                assert_eq!(report.cancelled.len(), paths.len());
                assert_eq!(progress.snapshot().cancelled, paths.len());
            } else {
                assert_eq!(report.failed.len(), paths.len());
                assert_eq!(progress.snapshot().failed, paths.len());
                assert!(owner.transfers.all().expect("snapshots").iter().all(|row| {
                    row.upload_activity
                        .as_ref()
                        .is_some_and(|activity| activity.phase == VaultUploadPhase::CheckingStorage)
                }));
            }
            assert!(
                owner
                    .active_upload_batch
                    .lock()
                    .expect("batch released")
                    .is_none()
            );
            assert!(
                owner
                    .transfers
                    .all()
                    .expect("snapshots")
                    .iter()
                    .all(|row| row.state
                        == if cancel_during_validation {
                            VaultTransferState::Cancelled
                        } else {
                            VaultTransferState::Failed(ApplicationErrorKind::Conflict)
                        })
            );
        }
        Ok(())
    }

    #[test]
    fn large_selections_publish_every_file_and_preserve_stop_policy_across_windows()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let paths = (0..513)
            .map(|i| temp.path().join(format!("file-{i:04}.txt")))
            .collect::<Vec<_>>();
        for path in &paths {
            std::fs::write(path, b"synthetic")?;
        }
        for stop in [
            None,
            Some(ApplicationErrorKind::Cancelled),
            Some(ApplicationErrorKind::Network),
        ] {
            let library = DesktopLibrary::open(temp.path().join("library.sqlite3"))?;
            let mut owner = VaultOwner {
                catalog: catalog::ManifestCache::default(),
                catalog_key_revision: 0,
                telegram: DesktopTelegram::open_direct(temp.path().join("test.session"))?,
                library,
                record: Some(VaultMetadataRecord {
                    vault_id: [3; 16],
                    password_wrap: vec![],
                    recovery_wrap: vec![],
                    password_generation: 1,
                    recovery_generation: 1,
                    created_at_unix_ms: 1,
                    updated_at_unix_ms: 1,
                }),
                master_key: Some(Arc::new(VaultMasterKey::from_bytes([7; 32]))),
                historical_key: None,
                health_worker: None,
                session: Arc::new(Mutex::new(VaultSession::new(None))),
                session_generation: 0,
                transfers: Arc::new(TransferSnapshots::new(Vec::new()).expect("snapshots")),
                active_upload_batch: Arc::new(Mutex::new(None)),
                upload_controls: UploadControls::default(),
            };
            let progress = VaultUploadSelectionProgress::new(paths.len());
            let mut calls = 0;
            let mut validations = 0;
            let report = owner.execute_upload_selection(
                (100, 700),
                paths.clone(),
                &progress,
                |_| {
                    validations += 1;
                    Ok(())
                },
                |owner, plan| {
                    let rows = owner.transfers.all().expect("visible window");
                    assert!(rows.len() <= 256);
                    assert!(
                        rows.iter().any(|row| row.id == plan.id),
                        "every executing file must be visible"
                    );
                    calls += 1;
                    if calls == 131
                        && let Some(kind) = stop
                    {
                        if kind == ApplicationErrorKind::Cancelled {
                            progress.cancel();
                        }
                        return Err(ApplicationError::new(kind));
                    }
                    owner.update_transfer(plan.id, |row| row.state = VaultTransferState::Completed);
                    Ok(ManagedVaultFile {
                        vault_id: Some([1; 16]),
                        health: crate::VaultFileHealth::Present,
                        part_message_ids: vec![calls as i64],
                        package_numeric_id: calls as u64,
                        package_id: calls.to_string(),
                        logical_name: plan.source.file_name.clone(),
                        relative_path: None,
                        mime_type: None,
                        media_kind: FileKind::Document,
                        size_bytes: plan.source.size_bytes,
                        encoded_size_bytes: 64,
                        part_count: 1,
                        created_at_unix_ms: 1,
                        manifest_message_id: calls as i64,
                        related_remote_names: Vec::new(),
                    })
                },
            )?;
            assert_eq!(validations, 1);
            let expected = if stop.is_none() { 513 } else { 130 };
            assert_eq!(report.completed_count, expected);
            assert_eq!(
                report.completed.len(),
                128,
                "receipt metadata is bounded independently of selection size"
            );
            assert_eq!(progress.snapshot().completed, expected);
            match stop {
                None => {
                    assert_eq!(calls, 513);
                    assert!(report.failed.is_empty() && report.cancelled.is_empty());
                    assert_eq!(
                        report.completed.last().expect("last receipt").logical_name,
                        "file-0512.txt"
                    );
                }
                Some(ApplicationErrorKind::Cancelled) => {
                    assert_eq!(calls, 131);
                    assert_eq!(report.cancelled.len(), 383);
                }
                Some(_) => {
                    assert_eq!(calls, 131);
                    assert_eq!(report.failed.len(), 383);
                }
            }
            assert_eq!(
                report.completed_count + report.failed.len() + report.cancelled.len(),
                paths.len()
            );
            assert!(owner.active_upload_batch.lock().expect("owner").is_none());
        }
        Ok(())
    }

    #[test]
    fn recovery_bundle_encoding_has_a_fixed_candidate_vector() {
        let key = RecoveryKey::from_bytes([0x5a; 32]);
        let wrap = RecoveryWrap {
            vault_id: [1; 16],
            recovery_generation: 2,
            ciphertext: [3; 32],
            tag: [4; 16],
        };
        assert_eq!(
            encode_recovery_bundle(&key, &wrap),
            concat!(
                "TARK-RB1-",
                "TARK-RK1-5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A-8AB6868B",
                "-5441524B52574B0000010000",
                "01010101010101010101010101010101",
                "000000020001000000000000",
                "0303030303030303030303030303030303030303030303030303030303030303",
                "04040404040404040404040404040404",
            )
        );
    }

    #[test]
    fn vault_lifecycle_persists_wrapped_keys_and_recovery_rotation()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let database_path = directory.path().join("library.sqlite3");
        let session_path = directory.path().join("telegram.session");
        let library = DesktopLibrary::open(&database_path)?;
        let telegram = DesktopTelegram::open_direct(&session_path)?;
        let vault = DesktopVault::new(telegram.clone(), library.clone())?;

        assert_eq!(vault.status(), VaultStatus::unconfigured());
        let first_recovery = vault.initialize("initial password".to_owned())?;
        assert!(vault.status().configured);
        assert!(!vault.status().locked);
        assert!(decode_recovery_bundle(&first_recovery).is_ok());
        let mut damaged = first_recovery.clone().into_bytes();
        let last = damaged
            .last_mut()
            .ok_or("recovery bundle unexpectedly empty")?;
        *last = if *last == b'A' { b'B' } else { b'A' };
        let damaged = String::from_utf8(damaged)?;
        assert!(decode_recovery_bundle(&damaged).is_err());

        vault.lock()?;
        assert!(vault.status().locked);
        assert!(
            vault
                .unlock_with_password("wrong password".to_owned())
                .is_err()
        );
        vault.unlock_with_recovery(first_recovery.clone())?;
        vault.change_password("replacement password".to_owned())?;
        let second_recovery = vault.rotate_recovery_key()?;
        assert_ne!(first_recovery, second_recovery);

        vault.lock()?;
        assert!(vault.unlock_with_recovery(first_recovery).is_err());
        vault.unlock_with_recovery(second_recovery.clone())?;
        vault.lock()?;
        assert!(
            vault
                .unlock_with_password("initial password".to_owned())
                .is_err()
        );
        vault.unlock_with_password("replacement password".to_owned())?;

        let reopened = DesktopVault::new(telegram, library)?;
        assert!(reopened.status().configured);
        assert!(reopened.status().locked);
        reopened.unlock_with_password("replacement password".to_owned())?;

        let recovery_database = directory.path().join("recovered.sqlite3");
        let recovery_session = directory.path().join("recovered.session");
        let recovered_library = DesktopLibrary::open(recovery_database)?;
        let recovered_telegram = DesktopTelegram::open_direct(recovery_session)?;
        let recovered = DesktopVault::new(recovered_telegram, recovered_library)?;
        recovered.restore_with_recovery(second_recovery, "post-disaster password".to_owned())?;
        assert!(recovered.status().configured);
        assert!(!recovered.status().locked);
        Ok(())
    }
    #[test]
    fn key_epochs_preserve_old_recovery_and_cancel_before_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("epochs.db"))?;
        let telegram = DesktopTelegram::open_direct(temp.path().join("test.session"))?;
        let vault = DesktopVault::new(telegram.clone(), library.clone())?;
        let old_recovery = vault.initialize("old synthetic password".into())?;
        let old = library.worker.vault_metadata()?.expect("old");
        let cancelled = VaultKeyProgress::new();
        cancelled.cancel();
        assert_eq!(
            vault
                .start_new_key_epoch_observed(
                    "cancelled synthetic password".into(),
                    cancelled.clone()
                )
                .expect_err("cancelled")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        assert_eq!(library.worker.vault_metadata()?, Some(old.clone()));
        assert!(cancelled.snapshot().finished);
        let progress = VaultKeyProgress::new();
        let new_recovery = vault
            .start_new_key_epoch_observed("new synthetic password".into(), progress.clone())?;
        let new = library.worker.vault_metadata()?.expect("new");
        assert_ne!(new.vault_id, old.vault_id);
        assert_ne!(new_recovery, old_recovery);
        assert_eq!(
            library.worker.vault_key_epoch(old.vault_id)?,
            Some(old.clone())
        );
        assert!(progress.snapshot().finished);
        assert_eq!(progress.snapshot().phase, VaultKeyPhase::Completed);
        assert_eq!(
            progress
                .snapshot()
                .timeline
                .iter()
                .map(|(phase, _)| *phase)
                .collect::<Vec<_>>(),
            vec![
                VaultKeyPhase::Queued,
                VaultKeyPhase::Generating,
                VaultKeyPhase::WrappingPassword,
                VaultKeyPhase::WrappingRecovery,
                VaultKeyPhase::Saving,
                VaultKeyPhase::Completed
            ]
        );
        vault.lock()?;
        vault.unlock_with_recovery(old_recovery.clone())?;
        assert!(!vault.status().locked);
        assert!(vault.status().active_key_locked);
        assert!(vault.status().historical_key_unlocked);
        assert_eq!(library.worker.vault_metadata()?, Some(new.clone()));
        vault.unlock_with_password("new synthetic password".into())?;
        assert!(!vault.status().active_key_locked);
        let reopened = DesktopVault::new(telegram, library.clone())?;
        reopened.unlock_with_recovery(old_recovery)?;
        assert!(reopened.status().historical_key_unlocked);
        assert_eq!(library.worker.vault_key_epoch(old.vault_id)?, Some(old));
        assert_eq!(library.worker.vault_metadata()?, Some(new));
        Ok(())
    }

    #[test]
    fn blocked_health_owner_does_not_block_vault_operations()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let (release, wait) = mpsc::channel();
        let (started, ready) = mpsc::channel();
        let worker =
            health::HealthWorker::spawn(crate::TelegramScanCancellation::new(), move || {
                started.send(()).expect("started");
                let _ = wait.recv();
            })?;
        ready.recv()?;
        let mut owner = VaultOwner {
            catalog: catalog::ManifestCache::default(),
            catalog_key_revision: 0,
            library: DesktopLibrary::open(temp.path().join("health.db"))?,
            telegram: DesktopTelegram::open_direct(temp.path().join("test.session"))?,
            record: None,
            master_key: None,
            historical_key: None,
            health_worker: Some(worker),
            session: Arc::new(Mutex::new(VaultSession::new(None))),
            session_generation: 0,
            transfers: Arc::new(TransferSnapshots::new(Vec::new())?),
            active_upload_batch: Arc::new(Mutex::new(None)),
            upload_controls: UploadControls::default(),
        };
        owner.initialize("synthetic independent password")?;
        assert!(
            !owner
                .health_worker
                .as_ref()
                .expect("retained worker")
                .is_finished()
        );
        assert!(owner.master_key.is_some());
        release.send(())?;
        Ok(())
    }

    #[test]
    fn bounded_history_evicts_whole_completed_batches_and_keeps_active_members() {
        let fixture = VaultTransferSnapshot {
            recovery_state: None,
            restored: false,
            upload_activity: None,
            id: 1,
            account_id: 7,
            chat_id: 90,
            batch_id: Some(1),
            queued_at_unix_ms: 1,
            direction: VaultTransferDirection::Upload,
            file_name: "fixture.bin".into(),
            package_id: None,
            size_bytes: 12,
            transferred_bytes: 12,
            completed_parts: 1,
            part_count: 1,
            started_at_unix_ms: 1,
            duration_ms: Some(10),
            average_bytes_per_second: None,
            destination: None,
            session_log_path: None,
            telemetry: transfer_controller(true, 1, teleark_transfer::SoftLimitPolicy::Respect)
                .expect("controller")
                .snapshot(),
            state: VaultTransferState::Completed,
        };
        let restored = (1..=256)
            .map(|id| {
                let mut row = fixture.clone();
                row.id = id;
                row.batch_id = None;
                row.state = VaultTransferState::Queued;
                row.restored = true;
                row
            })
            .collect();
        let projection = TransferSnapshots::new(restored).expect("restored history");
        let mut live = fixture.clone();
        live.id = 257;
        live.batch_id = None;
        live.state = VaultTransferState::Running;
        assert!(
            projection.insert_pruning(live.clone(), vault_transfer_evictions),
            "restored queued metadata must yield to live execution"
        );
        assert!(projection.get(257).is_some());
        let view = projection.view().expect("projection");
        assert_eq!(view.items.len(), 256);
        assert_eq!(view.omitted_items, 1);
        let active = (1..=256)
            .map(|id| {
                let mut row = live.clone();
                row.id = id;
                row
            })
            .collect();
        let projection = TransferSnapshots::new(active).expect("owned tasks");
        assert!(
            !projection.insert_pruning(live, vault_transfer_evictions),
            "owned work must not be evicted"
        );
        let mut history = Vec::new();
        for id in 1..=256 {
            let mut item = fixture.clone();
            item.id = id;
            if id > 128 {
                item.batch_id = Some(2);
                item.state = VaultTransferState::Queued;
            }
            retain_transfer_snapshot(&mut history, item);
        }
        let mut next = fixture.clone();
        next.id = 257;
        next.batch_id = None;
        retain_transfer_snapshot(&mut history, next);
        assert_eq!(history.len(), 129);
        assert!(history.iter().all(|item| item.batch_id != Some(1)));
        assert_eq!(
            history
                .iter()
                .filter(|item| item.batch_id == Some(2))
                .count(),
            128
        );
        let mut running = fixture;
        running.id = 129;
        running.batch_id = Some(2);
        running.state = VaultTransferState::Running;
        retain_transfer_snapshot(&mut history, running);
        assert_eq!(history.len(), 129);
        assert_eq!(
            history
                .iter()
                .find(|item| item.id == 129)
                .expect("running")
                .state,
            VaultTransferState::Running
        );
    }

    #[test]
    fn upload_preflight_has_no_count_limit_deduplicates_sources_and_keeps_names() {
        let root = tempfile::tempdir().expect("fixture");
        let source = root.path().join("旅の写真.zip");
        std::fs::write(&source, b"test archive").expect("fixture file");
        let files = inspect_upload_sources(&[source.clone(), source.clone()]).expect("preflight");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name, "旅の写真.zip");
        assert_eq!(files[0].size_bytes, 12);
        assert!(inspect_upload_sources(&[]).is_err());
        assert_eq!(
            inspect_upload_sources(&vec![source.clone(); 4096])
                .expect("no count cap")
                .len(),
            1
        );
        let many = (0..513)
            .map(|i| {
                let path = root.path().join(format!("file-{i}.txt"));
                std::fs::write(&path, b"fixture").expect("source");
                path
            })
            .collect::<Vec<_>>();
        assert_eq!(
            inspect_upload_sources(&many)
                .expect("distinct files over old cap")
                .len(),
            513
        );
        assert!(inspect_upload_sources(&[root.path().to_path_buf()]).is_err());
        std::fs::remove_file(&source).expect("remove source");
        assert!(inspect_upload_sources(&[source]).is_err());
    }

    #[test]
    fn batch_cancellation_and_shared_failures_never_start_remaining_files() {
        let cancel = AtomicBool::new(false);
        let mut policy = UploadBatchPolicy::default();
        let mut calls = 0;
        assert_eq!(
            policy.execute(&cancel, || {
                calls += 1;
                Ok(10)
            }),
            Ok(10)
        );
        let changed = policy.execute::<()>(&cancel, || {
            calls += 1;
            Err(ApplicationError::new(ApplicationErrorKind::SourceChanged))
        });
        assert_eq!(
            changed.expect_err("source changed").kind(),
            ApplicationErrorKind::SourceChanged
        );
        assert!(
            policy
                .execute(&cancel, || {
                    calls += 1;
                    Ok(())
                })
                .is_ok()
        );
        assert_eq!(calls, 3, "one changed source does not discard other files");
        let network = policy.execute::<()>(&cancel, || {
            calls += 1;
            Err(ApplicationError::new(ApplicationErrorKind::Network))
        });
        assert_eq!(
            network.expect_err("network").kind(),
            ApplicationErrorKind::Network
        );
        assert_eq!(
            policy
                .execute(&cancel, || {
                    calls += 1;
                    Ok(())
                })
                .expect_err("shared failure")
                .kind(),
            ApplicationErrorKind::Network
        );
        assert_eq!(calls, 4);
        cancel.store(true, Ordering::Release);
        assert_eq!(
            policy
                .execute(&cancel, || {
                    calls += 1;
                    Ok(())
                })
                .expect_err("cancelled")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        assert_eq!(calls, 4);
    }
}
