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
    AdaptiveControllerConfig, AdaptiveTransferController, ContentDigest, DestinationId,
    EncryptionPipelineConfig, EncryptionPipelineError, FileSystemPort, MemoryCounters,
    NativeFileSystem, ParameterBounds, PartCounters, PerformanceSample, PipelinePart,
    QueueCounters, RemotePartKey, RemoteTransport, SourceId, SourcePort, TransferControlParameters,
    TransferTelemetrySnapshot, run_encryption_upload_pipeline,
};
use zeroize::Zeroizing;

use crate::transfer::{hex_id, package_id_from_bytes};
use crate::vault_progress::{VaultUploadActivity, VaultUploadObserver, VaultUploadPhase};
use crate::{
    DesktopLibrary, DesktopTelegram, EncryptedRemoteTransport, ManifestPublishRequest,
    TelegramObjectStore, encrypted_part_sizes, recover_remote_manifests,
};

mod catalog;
mod health;
mod key_progress;
mod session;
mod upload_history;
pub use key_progress::{VaultKeyPhase, VaultKeyProgress, VaultKeySnapshot};
use session::{VaultEnvelope, VaultSession};

const VAULT_QUEUE_CAPACITY: usize = 16;
pub const MAX_VAULT_UPLOAD_BATCH: usize = 128;
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
    Interrupted,
    Cancelled,
    Completed,
    Failed(ApplicationErrorKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultTransferSnapshot {
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
    pub completed: Vec<ManagedVaultFile>,
    pub failed: Vec<VaultUploadFailure>,
    pub cancelled: Vec<PathBuf>,
}

/// Bounded picker preflight. The worker repeats it before queuing a batch and
/// each item checks its size/mtime again before any encryption or publication.
pub fn inspect_upload_sources(
    paths: &[PathBuf],
) -> Result<Vec<VaultUploadSource>, ApplicationError> {
    if paths.is_empty() || paths.len() > MAX_VAULT_UPLOAD_BATCH {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut sources = Vec::with_capacity(paths.len());
    let mut total = 0_u64;
    for path in paths {
        let canonical = path.canonicalize().map_err(map_source_io)?;
        if !seen.insert(canonical) {
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

struct QueuedUpload {
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
    sender: Mutex<Option<mpsc::SyncSender<VaultEnvelope>>>,
    transfer_sender: Mutex<Option<mpsc::SyncSender<VaultEnvelope>>>,
    scan_sender: Mutex<Option<mpsc::SyncSender<VaultEnvelope>>>,
    session: Arc<Mutex<VaultSession>>,
    transfers: Arc<TransferSnapshots<VaultTransferSnapshot>>,
    active_upload_batch: ActiveUploadBatch,
    joins: Mutex<Vec<JoinHandle<()>>>,
}

enum VaultCommand {
    RestoreUploadHistory {
        account: i64,
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
    UploadBatch {
        account_id: i64,
        chat_id: i64,
        sources: Vec<PathBuf>,
        reply: mpsc::SyncSender<Result<VaultUploadReport, ApplicationError>>,
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
        let mut senders = Vec::new();
        let mut joins = Vec::new();
        for name in [
            "teleark-vault-keys",
            "teleark-vault-transfers",
            "teleark-vault-scan",
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
                sender: Mutex::new(senders.next()),
                transfer_sender: Mutex::new(senders.next()),
                scan_sender: Mutex::new(senders.next()),
                session,
                transfers,
                active_upload_batch,
                joins: Mutex::new(joins),
            }),
        })
    }

    /// Background-only, works while locked and before network catalog loading.
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
                        VaultTransferState::Queued | VaultTransferState::Running
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
                    VaultTransferState::Queued | VaultTransferState::Running
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
        if sources.is_empty() || sources.len() > MAX_VAULT_UPLOAD_BATCH {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        self.submit(|reply| VaultCommand::UploadBatch {
            account_id,
            chat_id,
            sources,
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
        let (reply, response) = mpsc::sync_channel(1);
        let command = build(reply);
        let envelope = self
            .inner
            .session
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .admit(command)?;
        let queue = if envelope.command.is_transfer() {
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
            VaultCommand::RestoreUploadHistory { account, reply } => {
                let _ = reply.send(self.restore_upload_history(account));
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
                let _ = reply.send(self.upload(account_id, chat_id, &source, None));
            }
            VaultCommand::UploadBatch {
                account_id,
                chat_id,
                sources,
                reply,
            } => {
                let _ = reply.send(self.upload_batch(account_id, chat_id, sources));
            }
            VaultCommand::Download {
                account_id,
                chat_id,
                package_id,
                reply,
            } => {
                let _ = reply.send(self.download(account_id, chat_id, PackageId::new(package_id)));
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

    fn upload_batch(
        &mut self,
        account_id: i64,
        chat_id: i64,
        paths: Vec<PathBuf>,
    ) -> Result<VaultUploadReport, ApplicationError> {
        self.upload_batch_with_validation(account_id, chat_id, paths, |owner| {
            owner
                .telegram
                .validate_storage_channel(account_id, chat_id)
                .map(|_| ())
        })
    }

    fn upload_batch_with_validation(
        &mut self,
        account_id: i64,
        chat_id: i64,
        paths: Vec<PathBuf>,
        validate: impl FnOnce(&Self) -> Result<(), ApplicationError>,
    ) -> Result<VaultUploadReport, ApplicationError> {
        if self.master_key.is_none() {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let sources = inspect_upload_sources(&paths)?;
        let batch_id = random_transfer_id()?;
        let queued_at = now_unix_ms()?;
        let plans = sources
            .into_iter()
            .map(|source| {
                Ok(QueuedUpload {
                    id: random_transfer_id()?,
                    batch_id,
                    queued_at,
                    source,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        let queued_telemetry = transfer_controller(
            true,
            0,
            self.library.preferences()?.transfer_soft_limit_policy,
        )?
        .snapshot();
        let cancel = Arc::new(AtomicBool::new(false));
        *self
            .active_upload_batch
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))? =
            Some((account_id, batch_id, cancel.clone()));
        let snapshots = plans
            .iter()
            .map(|plan| VaultTransferSnapshot {
                restored: false,
                upload_activity: Some(VaultUploadActivity::new(VaultUploadPhase::Persisting)),
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
        if let Err(error) = self.admit_upload_window(snapshots) {
            *self
                .active_upload_batch
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            return Err(error);
        }
        for plan in &plans {
            VaultUploadObserver::new(self.transfers.clone(), plan.id)
                .phase(VaultUploadPhase::CheckingStorage);
        }
        // Publish all queue rows before any network preflight. One complete
        // discovery covers the batch; each file still revalidates its target.
        let validation = if cancel.load(Ordering::Acquire) {
            Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
        } else {
            validate(self)
        };
        let validation_succeeded = validation.is_ok();
        let mut report = VaultUploadReport::default();
        let mut policy = UploadBatchPolicy {
            blocked: validation.err().map(|error| error.kind()),
        };
        if validation_succeeded {
            for plan in &plans {
                self.update_transfer(plan.id, |snapshot| snapshot.upload_activity = None);
            }
        }
        for plan in &plans {
            let result = policy.execute(&cancel, || {
                self.upload(account_id, chat_id, &plan.source.path, Some(plan))
            });
            match result {
                Ok(file) => report.completed.push(file),
                Err(error) => {
                    self.update_transfer(plan.id, |snapshot| {
                        snapshot.state = if error.kind() == ApplicationErrorKind::Cancelled {
                            VaultTransferState::Cancelled
                        } else {
                            VaultTransferState::Failed(error.kind())
                        }
                    });
                    if let Err(error) = self.persist_upload_id(plan.id) {
                        self.fail_pending_upload_window(account_id, error.kind());
                        *self
                            .active_upload_batch
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = None;
                        return Err(error);
                    }
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
        *self
            .active_upload_batch
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))? = None;
        Ok(report)
    }

    fn upload(
        &mut self,
        account_id: i64,
        chat_id: i64,
        source: &Path,
        queued: Option<&QueuedUpload>,
    ) -> Result<ManagedVaultFile, ApplicationError> {
        if self.master_key.is_none() {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
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
        let logical_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?
            .to_owned();
        let part_sizes = encrypted_part_sizes(metadata.len()).map_err(map_transfer_error)?;
        let package_id = random_nonzero_u64()?;
        let transfer_id = queued
            .map(|plan| plan.id)
            .map_or_else(random_transfer_id, Ok)?;
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
        self.push_transfer(VaultTransferSnapshot {
            restored: false,
            upload_activity: Some(VaultUploadActivity::new(VaultUploadPhase::Preparing)),
            id: transfer_id,
            account_id,
            chat_id,
            batch_id: queued.map(|plan| plan.batch_id),
            queued_at_unix_ms: queued.map_or(started_at, |plan| plan.queued_at),
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
        })?;
        let started = Instant::now();
        let result: Result<ManagedVaultFile, ApplicationError> = (|| {
            let mut files = NativeFileSystem::new();
            let source_id = SourceId(transfer_id);
            files
                .register_source(source_id, source)
                .map_err(map_transfer_error)?;
            let source_identity = files
                .source_identity(source_id)
                .map_err(map_transfer_error)?;
            let file_key = generate_file_key(&mut OsRandom).map_err(map_crypto_error)?;
            let observer = Arc::new(VaultUploadObserver::new(
                self.transfers.clone(),
                transfer_id,
            ));
            let store = TelegramObjectStore::new(self.telegram.clone(), account_id, chat_id)
                .with_observer(observer.clone());
            let mut remote = EncryptedRemoteTransport::new(
                store,
                AccountId::new(account_id),
                chat_id,
                PackageId::new(package_id),
                file_key,
                metadata.len(),
                part_sizes.clone(),
            )
            .map_err(map_transfer_error)?;
            let mut encoded_size = 0_u64;
            let mut plaintext_offset = 0_u64;
            let mut pipeline_parts = Vec::with_capacity(part_sizes.len());
            let mut encryption_plans = Vec::with_capacity(part_sizes.len());
            for (position, plaintext_length) in part_sizes.iter().copied().enumerate() {
                let part_index = u32::try_from(position)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
                let key = RemotePartKey {
                    account_id: AccountId::new(account_id),
                    package_id: PackageId::new(package_id),
                    part_index: PartIndex::new(part_index),
                };
                pipeline_parts.push(PipelinePart {
                    part_index,
                    plaintext_offset,
                    plaintext_length,
                });
                encryption_plans.push(
                    remote
                        .plan_part_encryption(key, None)
                        .map_err(map_transfer_error)?,
                );
                plaintext_offset = plaintext_offset
                    .checked_add(plaintext_length)
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
            }
            let encryption_context = remote.encryption_context();
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
            let report = run_encryption_upload_pipeline(
                pipeline_config,
                &pipeline_parts,
                move |descriptor| {
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
                        let position = usize::try_from(descriptor.part_index)
                            .map_err(|_| TransferError::SourceChanged)?;
                        let plan = encryption_plans
                            .get(position)
                            .cloned()
                            .ok_or(TransferError::SourceChanged)?;
                        EncryptedRemoteTransport::<TelegramObjectStore>::encrypt_planned_part(
                            &encryption_context,
                            plan,
                            plaintext,
                        )
                    }
                },
                |part_index, prepared| {
                    let plaintext_length = prepared.manifest_part.plaintext_length;
                    observer.begin_part(plaintext_length);
                    if let Ok(mut duration) = encryption_time.lock() {
                        *duration = duration.saturating_add(prepared.encryption_duration_micros);
                    }
                    let object = remote.upload_prepared_part(prepared)?;
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
            if files
                .source_identity(source_id)
                .map_err(map_transfer_error)?
                != source_identity
            {
                return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
            }
            observer.phase(VaultUploadPhase::Publishing);
            let manifest_object = remote
                .publish_manifest(
                    self.master_key.as_ref().ok_or_else(|| {
                        ApplicationError::new(ApplicationErrorKind::Authorization)
                    })?,
                    ManifestPublishRequest {
                        vault_id: self
                            .record
                            .as_ref()
                            .map(|record| record.vault_id)
                            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?,
                        manifest_generation: 1,
                        master_key_generation: 1,
                        wrap_generation: 1,
                        created_at_unix_ms: u64::try_from(started_at).unwrap_or_default(),
                        logical_name: logical_name.clone(),
                        relative_path: None,
                        mime_type: None,
                        media_kind: media_kind(crate::classify_file(source)),
                        whole_plaintext_blake3: whole_digest.0,
                    },
                )
                .map_err(map_transfer_error)?;
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
        if let Err(error) = &result {
            self.update_transfer(transfer_id, |snapshot| {
                snapshot.state = VaultTransferState::Failed(error.kind());
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

    fn download(
        &mut self,
        expected_account_id: i64,
        chat_id: i64,
        package_id: PackageId,
    ) -> Result<PathBuf, ApplicationError> {
        let active = self
            .record
            .as_ref()
            .zip(self.master_key.as_deref())
            .map(|(record, key)| (record.vault_id, key));
        if active.is_none() && self.historical_key.is_none() {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let mut store =
            TelegramObjectStore::new(self.telegram.clone(), expected_account_id, chat_id);
        if expected_account_id != chat_id {
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
        let recovered = health::recover_target(
            &mut store,
            (expected_account_id, chat_id),
            package_id,
            active,
            self.historical_key.as_ref(),
        )?;
        let logical_name = recovered.manifest.metadata.logical_name.clone();
        let size_bytes = recovered.manifest.public_header.logical_file_size;
        let part_count = recovered.manifest.public_header.part_count;
        let package_text = hex_id(&recovered.manifest.public_header.package_id);
        let whole_digest = ContentDigest(recovered.manifest.metadata.whole_plaintext_blake3);
        let account_id = recovered
            .manifest
            .metadata
            .parts
            .first()
            .map(|part| AccountId::new(part.remote_locator.account_id))
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        let destination = self.library.next_download_destination(&logical_name)?;
        let transfer_id = random_transfer_id()?;
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
        self.push_transfer(VaultTransferSnapshot {
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
        })?;
        let started = Instant::now();
        let result = (|| {
            let store =
                TelegramObjectStore::new(self.telegram.clone(), expected_account_id, chat_id);
            let parts = recovered.manifest.metadata.parts.clone();
            let mut remote =
                EncryptedRemoteTransport::from_opened_manifest(store, recovered.manifest)
                    .map_err(map_transfer_error)?;
            let destination_id = DestinationId(transfer_id);
            let mut files = NativeFileSystem::new();
            files
                .register_destination(destination_id, &destination)
                .map_err(map_transfer_error)?;
            files
                .prepare_partial(destination_id, size_bytes)
                .map_err(map_transfer_error)?;
            let mut completed_bytes = 0_u64;
            let mut completed_parts = 0_u32;
            for part in parts {
                let key = RemotePartKey {
                    account_id,
                    package_id,
                    part_index: PartIndex::new(part.part_index),
                };
                let object = remote
                    .discover_remote(key)
                    .map_err(map_transfer_error)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
                let plaintext = remote
                    .download_remote(&object)
                    .map_err(map_transfer_error)?;
                files
                    .write_partial(destination_id, part.plaintext_offset, &plaintext)
                    .map_err(map_transfer_error)?;
                completed_bytes = completed_bytes.saturating_add(part.plaintext_length);
                completed_parts = completed_parts.saturating_add(1);
                let elapsed_ms = u64::try_from(started.elapsed().as_millis())
                    .unwrap_or(u64::MAX)
                    .max(1);
                let average_bytes_per_second = completed_bytes
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
            if files
                .digest_partial(destination_id)
                .map_err(map_transfer_error)?
                != whole_digest
            {
                return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
            }
            files
                .flush_partial(destination_id)
                .map_err(map_transfer_error)?;
            files
                .atomic_finalize(destination_id)
                .map_err(map_transfer_error)?;
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
        if result.is_err() {
            discard_partial(&destination);
        }
        let _ = session_log.append_finished(
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            result.as_ref().err().map(ApplicationError::kind),
            &controller.snapshot(),
        );
        self.finish_transfer(transfer_id, started, None);
        if let Err(error) = &result {
            self.update_transfer(transfer_id, |snapshot| {
                snapshot.state = VaultTransferState::Failed(error.kind());
            });
        }
        result
    }

    fn push_transfer(&self, snapshot: VaultTransferSnapshot) -> Result<(), ApplicationError> {
        self.transfers
            .insert_pruning(snapshot.clone(), vault_transfer_evictions);
        if snapshot.direction == VaultTransferDirection::Upload {
            self.persist_upload(&snapshot)?;
        }
        Ok(())
    }

    fn update_transfer(&self, id: u64, update: impl FnOnce(&mut VaultTransferSnapshot)) {
        self.transfers.update(id, update);
    }

    fn finish_transfer(&self, id: u64, started: Instant, package_id: Option<String>) {
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.update_transfer(id, |snapshot| {
            if let Some(package_id) = package_id {
                snapshot.package_id = Some(package_id);
            }
            snapshot.duration_ms = Some(duration_ms);
            snapshot.average_bytes_per_second = snapshot
                .transferred_bytes
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
            !matches!(
                item.state,
                VaultTransferState::Queued | VaultTransferState::Running
            ) && item.batch_id.is_none_or(|batch| {
                transfers.iter().all(|member| {
                    member.batch_id != Some(batch)
                        || member.account_id != item.account_id
                        || !matches!(
                            member.state,
                            VaultTransferState::Queued | VaultTransferState::Running
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

fn discard_partial(destination: &Path) {
    let Some(name) = destination.file_name() else {
        return;
    };
    let mut partial_name = name.to_os_string();
    partial_name.push(".partial");
    let partial = destination.with_file_name(partial_name);
    let _ = std::fs::remove_file(partial);
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

fn map_transfer_error(error: TransferError) -> ApplicationError {
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
        let paths = vec![temp.path().join("one.txt"), temp.path().join("two.txt")];
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
                record: None,
                master_key: Some(Arc::new(VaultMasterKey::from_bytes([7; 32]))),
                historical_key: None,
                health_worker: None,
                session: Arc::new(Mutex::new(VaultSession::new(None))),
                session_generation: 0,
                transfers: Arc::new(TransferSnapshots::new(Vec::new()).expect("snapshot store")),
                active_upload_batch: Arc::new(Mutex::new(None)),
            };
            let mut calls = 0;
            let report = owner.upload_batch_with_validation(100, 700, paths.clone(), |owner| {
                calls += 1;
                let rows = owner.transfers.all().expect("snapshots");
                assert_eq!(rows.len(), 2);
                assert!(
                    rows.iter()
                        .all(|row| row.state == VaultTransferState::Queued)
                );
                assert!(
                    rows.iter()
                        .all(|row| row.transferred_bytes == 0 && row.package_id.is_none())
                );
                assert!(rows.iter().all(|row| {
                    row.upload_activity
                        .as_ref()
                        .is_some_and(|activity| activity.phase == VaultUploadPhase::CheckingStorage)
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
            assert!(report.completed.is_empty());
            if cancel_during_validation {
                assert_eq!(report.cancelled.len(), 2);
            } else {
                assert_eq!(report.failed.len(), 2);
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
    fn upload_preflight_is_bounded_deduplicates_sources_and_keeps_names() {
        let root = tempfile::tempdir().expect("fixture");
        let source = root.path().join("旅の写真.zip");
        std::fs::write(&source, b"test archive").expect("fixture file");
        let files = inspect_upload_sources(&[source.clone(), source.clone()]).expect("preflight");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name, "旅の写真.zip");
        assert_eq!(files[0].size_bytes, 12);
        assert!(inspect_upload_sources(&[]).is_err());
        assert!(inspect_upload_sources(&vec![source.clone(); MAX_VAULT_UPLOAD_BATCH + 1]).is_err());
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
