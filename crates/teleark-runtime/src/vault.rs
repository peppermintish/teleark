use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
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
use crate::{
    DesktopLibrary, DesktopTelegram, EncryptedRemoteTransport, ManifestPublishRequest,
    TelegramObjectStore, encrypted_part_sizes, recover_remote_manifests,
};

const VAULT_QUEUE_CAPACITY: usize = 16;
const MAX_MANIFEST_SCAN: usize = 1_000;
const RECOVERY_BUNDLE_PREFIX: &str = "TARK-RB1-";
const TRANSFER_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
// With 60 MiB application parts, three workers plus the bounded reader,
// encrypted queue, and active upload stay inside the 512 MiB transfer budget.
const MAX_ENCRYPTION_WORKERS: u16 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultStatus {
    pub configured: bool,
    pub locked: bool,
    pub created_at_unix_ms: Option<i64>,
    pub password_generation: Option<u32>,
    pub recovery_generation: Option<u32>,
}

impl VaultStatus {
    const fn unconfigured() -> Self {
        Self {
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
    Running,
    Completed,
    Failed(ApplicationErrorKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultTransferSnapshot {
    pub id: u64,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedVaultFile {
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ManagedVaultScan {
    pub files: Vec<ManagedVaultFile>,
    pub rejected_manifests: usize,
}

#[derive(Clone)]
pub struct DesktopVault {
    inner: Arc<VaultInner>,
}

struct VaultInner {
    sender: Mutex<Option<mpsc::SyncSender<VaultCommand>>>,
    status: Arc<Mutex<VaultStatus>>,
    transfers: Arc<Mutex<Vec<VaultTransferSnapshot>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

enum VaultCommand {
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
    Lock {
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    ChangePassword {
        password: Zeroizing<String>,
        reply: mpsc::SyncSender<Result<(), ApplicationError>>,
    },
    RotateRecovery {
        reply: mpsc::SyncSender<Result<String, ApplicationError>>,
    },
    Scan {
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
    Download {
        account_id: i64,
        chat_id: i64,
        package_id: u64,
        reply: mpsc::SyncSender<Result<PathBuf, ApplicationError>>,
    },
    Shutdown,
}

struct VaultOwner {
    library: DesktopLibrary,
    telegram: DesktopTelegram,
    record: Option<VaultMetadataRecord>,
    master_key: Option<VaultMasterKey>,
    status: Arc<Mutex<VaultStatus>>,
    transfers: Arc<Mutex<Vec<VaultTransferSnapshot>>>,
}

impl DesktopVault {
    pub fn new(
        telegram: DesktopTelegram,
        library: DesktopLibrary,
    ) -> Result<Self, ApplicationError> {
        let record = library.worker.vault_metadata()?;
        if let Some(record) = record.as_ref() {
            validate_record(record)?;
        }
        let status = Arc::new(Mutex::new(status_for(record.as_ref(), true)));
        let transfers = Arc::new(Mutex::new(Vec::new()));
        let owner_status = Arc::clone(&status);
        let owner_transfers = Arc::clone(&transfers);
        let (sender, receiver) = mpsc::sync_channel(VAULT_QUEUE_CAPACITY);
        let join = thread::Builder::new()
            .name("teleark-vault".to_owned())
            .spawn(move || {
                VaultOwner {
                    library,
                    telegram,
                    record,
                    master_key: None,
                    status: owner_status,
                    transfers: owner_transfers,
                }
                .run(receiver);
            })
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        Ok(Self {
            inner: Arc::new(VaultInner {
                sender: Mutex::new(Some(sender)),
                status,
                transfers,
                join: Mutex::new(Some(join)),
            }),
        })
    }

    #[must_use]
    pub fn status(&self) -> VaultStatus {
        self.inner
            .status
            .lock()
            .map(|status| *status)
            .unwrap_or_else(|_| VaultStatus::unconfigured())
    }

    #[must_use]
    pub fn transfers(&self) -> Vec<VaultTransferSnapshot> {
        self.inner
            .transfers
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default()
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
        self.request(|reply| VaultCommand::Lock { reply })
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

    pub fn download_file(
        &self,
        account_id: i64,
        chat_id: i64,
        package_id: u64,
    ) -> Result<PathBuf, ApplicationError> {
        self.request(|reply| VaultCommand::Download {
            account_id,
            chat_id,
            package_id,
            reply,
        })
    }

    fn request<T>(
        &self,
        build: impl FnOnce(mpsc::SyncSender<Result<T, ApplicationError>>) -> VaultCommand,
    ) -> Result<T, ApplicationError> {
        let (reply, response) = mpsc::sync_channel(1);
        let sender = self
            .inner
            .sender
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .clone()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        sender
            .send(build(reply))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        response
            .recv()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
    }
}

impl Drop for VaultInner {
    fn drop(&mut self) {
        if let Ok(sender) = self.sender.get_mut()
            && let Some(sender) = sender.take()
        {
            let _ = sender.try_send(VaultCommand::Shutdown);
        }
        if let Ok(join) = self.join.get_mut()
            && let Some(join) = join.take()
            && join.is_finished()
        {
            let _ = join.join();
        }
    }
}

impl VaultOwner {
    fn run(mut self, receiver: mpsc::Receiver<VaultCommand>) {
        while let Ok(command) = receiver.recv() {
            match command {
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
                VaultCommand::Lock { reply } => {
                    self.master_key = None;
                    self.refresh_status();
                    let _ = reply.send(Ok(()));
                }
                VaultCommand::ChangePassword { password, reply } => {
                    let _ = reply.send(self.change_password(&password));
                }
                VaultCommand::RotateRecovery { reply } => {
                    let _ = reply.send(self.rotate_recovery());
                }
                VaultCommand::Scan {
                    account_id,
                    chat_id,
                    cancellation,
                    reply,
                } => {
                    let _ = reply.send(self.scan(account_id, chat_id, cancellation));
                }
                VaultCommand::Upload {
                    account_id,
                    chat_id,
                    source,
                    reply,
                } => {
                    let _ = reply.send(self.upload(account_id, chat_id, &source));
                }
                VaultCommand::Download {
                    account_id,
                    chat_id,
                    package_id,
                    reply,
                } => {
                    let _ =
                        reply.send(self.download(account_id, chat_id, PackageId::new(package_id)));
                }
                VaultCommand::Shutdown => break,
            }
        }
        self.master_key = None;
        self.refresh_status();
    }

    fn initialize(&mut self, password: &str) -> Result<String, ApplicationError> {
        if self.record.is_some() {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let password = Password::new(password.as_bytes().to_vec()).map_err(map_crypto_error)?;
        let mut random = OsRandom;
        let mut vault_id = [0_u8; 16];
        random.fill_bytes(&mut vault_id).map_err(map_crypto_error)?;
        let master_key = generate_vault_master_key(&mut random).map_err(map_crypto_error)?;
        let recovery_key = generate_recovery_key(&mut random).map_err(map_crypto_error)?;
        let mut usage = AeadUsageRegistry::new();
        let password_wrap = wrap_master_key_with_password(
            &master_key,
            &password,
            vault_id,
            1,
            &mut random,
            &mut usage,
        )
        .map_err(map_crypto_error)?;
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
        self.library.worker.save_vault_metadata(record.clone())?;
        let recovery_text = encode_recovery_bundle(&recovery_key, &recovery_wrap);
        self.record = Some(record);
        self.master_key = Some(master_key);
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
        self.master_key = Some(master);
        self.refresh_status();
        Ok(())
    }

    fn unlock_recovery(&mut self, recovery_text: &str) -> Result<(), ApplicationError> {
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
        self.master_key = Some(master);
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
        self.library.worker.save_vault_metadata(record.clone())?;
        self.record = Some(record);
        self.master_key = Some(master);
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
        self.library.worker.save_vault_metadata(record.clone())?;
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
        self.library.worker.save_vault_metadata(record.clone())?;
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
            files,
            rejected_manifests: report.rejected.len(),
        })
    }

    fn upload(
        &mut self,
        account_id: i64,
        chat_id: i64,
        source: &Path,
    ) -> Result<ManagedVaultFile, ApplicationError> {
        // Destination policy is enforced before touching plaintext or allocating
        // keys. A frontend cannot turn Saved Messages or an arbitrary channel
        // into a TeleArk upload target by supplying its numeric id.
        if self.library.storage_channel_id(account_id)? != Some(chat_id) {
            return Err(ApplicationError::new(
                ApplicationErrorKind::PermissionDenied,
            ));
        }
        self.telegram
            .validate_storage_channel(account_id, chat_id)?;
        let metadata = std::fs::metadata(source).map_err(map_source_io)?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceMissing));
        }
        let logical_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?
            .to_owned();
        let part_sizes = encrypted_part_sizes(metadata.len()).map_err(map_transfer_error)?;
        let package_id = random_nonzero_u64()?;
        let transfer_id = random_nonzero_u64()?;
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
            id: transfer_id,
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
        });
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
            let store = TelegramObjectStore::new(self.telegram.clone(), account_id, chat_id);
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
            let package_bytes = package_bytes(package_id);
            let related_remote_names = (0..part_count)
                .map(|index| teleark_crypto::remote_part_name(&package_bytes, index))
                .chain(std::iter::once(manifest_object.name.clone()))
                .collect();
            Ok(ManagedVaultFile {
                package_numeric_id: package_id,
                package_id: hex_id(&package_bytes),
                logical_name: logical_name.clone(),
                relative_path: None,
                mime_type: None,
                media_kind: crate::classify_file(source),
                size_bytes: metadata.len(),
                encoded_size_bytes: encoded_size.saturating_add(manifest_object.encoded_size),
                part_count,
                created_at_unix_ms: started_at,
                manifest_message_id: i64::try_from(manifest_object.object_id)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?,
                related_remote_names,
            })
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
        self.finish_transfer(
            transfer_id,
            started,
            result.as_ref().ok().map(|file| file.package_id.clone()),
        );
        result
    }

    fn download(
        &mut self,
        expected_account_id: i64,
        chat_id: i64,
        package_id: PackageId,
    ) -> Result<PathBuf, ApplicationError> {
        let master = self
            .master_key
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        let mut store =
            TelegramObjectStore::new(self.telegram.clone(), expected_account_id, chat_id);
        let mut report = recover_remote_manifests(&mut store, master, MAX_MANIFEST_SCAN)
            .map_err(map_transfer_error)?;
        let position = report
            .recovered
            .iter()
            .position(|candidate| {
                package_id_from_bytes(candidate.manifest.public_header.package_id)
                    .is_ok_and(|candidate_id| candidate_id == package_id)
            })
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        let recovered = report.recovered.swap_remove(position);
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
        let transfer_id = random_nonzero_u64()?;
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
            id: transfer_id,
            direction: VaultTransferDirection::Download,
            file_name: logical_name,
            package_id: Some(package_text),
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
        });
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

    fn push_transfer(&self, snapshot: VaultTransferSnapshot) {
        if let Ok(mut transfers) = self.transfers.lock() {
            if transfers.len() >= 256 {
                transfers.remove(0);
            }
            transfers.push(snapshot);
        }
    }

    fn update_transfer(&self, id: u64, update: impl FnOnce(&mut VaultTransferSnapshot)) {
        if let Ok(mut transfers) = self.transfers.lock()
            && let Some(snapshot) = transfers.iter_mut().find(|item| item.id == id)
        {
            update(snapshot);
        }
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
        if let Ok(mut status) = self.status.lock() {
            *status = status_for(self.record.as_ref(), self.master_key.is_none());
        }
    }
}

pub(crate) struct TransferSessionLog {
    pub(crate) path: PathBuf,
    writer: std::io::BufWriter<std::fs::File>,
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
        std::fs::create_dir_all(&directory).map_err(map_log_io)?;
        let path = directory.join(format!(
            "{}-{transfer_id}.jsonl",
            session_kind.file_prefix()
        ));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(map_log_io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(map_log_io)?;
        }
        Ok(Self {
            path,
            writer: std::io::BufWriter::new(file),
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
        std::io::ErrorKind::PermissionDenied => ApplicationErrorKind::PermissionDenied,
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
        let telegram = DesktopTelegram::open(&session_path)?;
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
        let recovered_telegram = DesktopTelegram::open(recovery_session)?;
        let recovered = DesktopVault::new(recovered_telegram, recovered_library)?;
        recovered.restore_with_recovery(second_recovery, "post-disaster password".to_owned())?;
        assert!(recovered.status().configured);
        assert!(!recovered.status().locked);
        Ok(())
    }
}
