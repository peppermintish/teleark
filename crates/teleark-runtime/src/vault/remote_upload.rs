//! Authenticated, discoverable pending uploads; no source paths or bare keys go remote.
use super::*;
use crate::{RemoteObjectStore, VaultRecoveryContext};
use teleark_crypto::{ManifestLimits, ManifestMetadata, ManifestPublicHeader, PendingUpload};

pub(crate) const CAPTION: &str = "TeleArk pending upload v1";
// Tiny, independently versioned locator; never contains source or container bytes.
fn locator_path(database: &Path, account: i64, task: u64) -> std::path::PathBuf {
    database
        .with_extension("upload-spool")
        .join(account.to_string())
        .join(task.to_string())
        .join("remote-upload.locator")
}
pub(super) fn remember_remote(
    database: &Path,
    pending: &teleark_storage::PendingVaultUpload,
    message: i64,
) -> Result<(), ApplicationError> {
    use std::io::Write;
    let fail = |_| ApplicationError::new(ApplicationErrorKind::Persistence);
    let path = locator_path(database, pending.account_id, pending.id);
    let parent = path
        .parent()
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    std::fs::create_dir_all(parent).map_err(fail)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).map_err(fail)?;
    }
    let mut bytes = b"TARKRI01".to_vec();
    bytes.extend_from_slice(&message.to_le_bytes());
    bytes.extend_from_slice(blake3::hash(&pending.context).as_bytes());
    bytes.extend_from_slice(blake3::hash(&bytes).as_bytes());
    let temporary = path.with_extension("pending");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(fail)?;
    file.write_all(&bytes).map_err(fail)?;
    file.sync_all().map_err(fail)?;
    std::fs::rename(&temporary, &path).map_err(fail)?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(fail)?;
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}
pub(super) fn remembered_remote(
    database: &Path,
    pending: &teleark_storage::PendingVaultUpload,
) -> Result<Option<i64>, ApplicationError> {
    use std::io::Read;
    let path = locator_path(database, pending.account_id, pending.id);
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
    };
    let mut bytes = Vec::new();
    file.take(81)
        .read_to_end(&mut bytes)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
    if bytes.len() != 80
        || &bytes[..8] != b"TARKRI01"
        || &bytes[16..48] != blake3::hash(&pending.context).as_bytes()
        || &bytes[48..] != blake3::hash(&bytes[..48]).as_bytes()
    {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    let message = i64::from_le_bytes(
        bytes[8..16]
            .try_into()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?,
    );
    if message <= 0 {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok(Some(message))
}
fn name(package: &[u8; 16]) -> String {
    format!("{}.tarku", hex_id(package))
}
fn metadata(
    context: &VaultRecoveryContext,
    parts: Vec<teleark_crypto::ManifestPart>,
    known: bool,
) -> ManifestMetadata {
    let hash = match context.direction {
        crate::VaultRecoveryDirection::Upload { source_blake3, .. } if known => source_blake3,
        _ => [0; 32],
    };
    ManifestMetadata {
        logical_name: context.file_name.clone(),
        relative_path: None,
        mime_type: None,
        media_kind: teleark_crypto::MediaKind::Other,
        whole_plaintext_blake3: hash,
        parts,
        logical_timestamps: None,
        source_metadata: Vec::new(),
        format_extensions: Vec::new(),
    }
}
fn public(context: &VaultRecoveryContext) -> Result<ManifestPublicHeader, ApplicationError> {
    Ok(ManifestPublicHeader {
        package_id: context.package_id,
        vault_id: context.vault_id,
        manifest_generation: 1,
        created_at_unix_ms: context.created_at_unix_ms,
        logical_file_size: context.size_bytes,
        part_count: context.part_sizes().map_err(map_transfer_error)?.len() as u32,
        application_part_target: context.container_plaintext_limit,
        frame_plaintext_max: 512 * 1024 - 32,
        nonce_strategy_id: teleark_crypto::NONCE_STRATEGY_ID,
        crypto_suite_id: teleark_crypto::CRYPTO_SUITE_ID,
        file_key_wrap: context.file_key_wrap.clone(),
        master_key_generation: context.master_key_generation,
        flags: 1,
    })
}
pub(super) fn publish(
    store: &mut impl RemoteObjectStore,
    context: &VaultRecoveryContext,
    master: &VaultMasterKey,
    parts: Vec<teleark_crypto::ManifestPart>,
    known: bool,
) -> Result<u32, ApplicationError> {
    let file_key = context
        .file_key(master)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
    let bytes = teleark_crypto::seal_pending_upload(
        &public(context)?,
        &metadata(context, parts, known),
        known,
        teleark_crypto::PendingUploadScope {
            account_id: context.account_id,
            chat_id: context.chat_id,
        },
        &file_key,
        ManifestLimits::default(),
    )
    .map_err(map_crypto_error)?;
    if bytes.len() > teleark_telegram::MAX_TRANSFER_OBJECT_BYTES {
        return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
    }
    let object = store
        .upload(&name(&context.package_id), CAPTION, bytes)
        .map_err(|error| match error {
            teleark_transfer::UploadError::Definite(error) => map_transfer_error(error),
            teleark_transfer::UploadError::AmbiguousSuccess => {
                ApplicationError::new(ApplicationErrorKind::Network)
            }
        })?;
    // Telegram message IDs provide disjoint completed-manifest generations for
    // concurrent devices using the same file key, including repeated handoffs.
    u32::try_from(object.object_id)
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))
}
fn open(
    bytes: &[u8],
    master: &VaultMasterKey,
    account: i64,
    chat: i64,
) -> Result<PendingUpload, ApplicationError> {
    let pending = teleark_crypto::open_pending_upload(bytes, master, ManifestLimits::default())
        .map_err(map_crypto_error)?;
    if pending.scope
        != (teleark_crypto::PendingUploadScope {
            account_id: account,
            chat_id: chat,
        })
        || pending.public_header.manifest_generation != 1
        || pending.public_header.flags != 1
        || pending.public_header.application_part_target > crate::encrypted_part_plaintext_limit()
        || pending.metadata.parts.iter().any(|part| {
            part.remote_locator.account_id != account
                || part.remote_locator.chat_id != chat
                || part.remote_locator.message_id <= 0
        })
    {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok(pending)
}
// Select a verified monotone prefix even when a device was opened on an older announcement.
fn newest_pending(
    store: &mut impl RemoteObjectStore,
    mut selected: PendingUpload,
    master: &VaultMasterKey,
    account: i64,
    chat: i64,
) -> Result<PendingUpload, ApplicationError> {
    let expected_name = name(&selected.public_header.package_id);
    for object in store
        .search_exact_caption(CAPTION, MAX_MANIFEST_SCAN)
        .map_err(map_transfer_error)?
    {
        if object.name != expected_name {
            continue;
        }
        let bytes = store
            .download(object.object_id)
            .map_err(map_transfer_error)?;
        let Ok(candidate) = open(&bytes, master, account, chat) else {
            continue;
        };
        if candidate.public_header != selected.public_header
            || candidate.metadata.logical_name != selected.metadata.logical_name
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        if selected.source_hash_known
            && candidate.source_hash_known
            && selected.metadata.whole_plaintext_blake3 != candidate.metadata.whole_plaintext_blake3
        {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
        }
        if candidate.source_hash_known
            && (!selected.source_hash_known
                || candidate.metadata.parts.len() > selected.metadata.parts.len())
        {
            selected = candidate;
        }
    }
    Ok(selected)
}
pub(super) fn load(
    store: &mut impl RemoteObjectStore,
    object: crate::RemoteByteObject,
    master: Option<&VaultMasterKey>,
    account: i64,
    chat: i64,
) -> Result<Option<ManagedVaultFile>, ApplicationError> {
    let Some(master) = master else {
        return Ok(None);
    };
    if object.encoded_size > teleark_telegram::MAX_TRANSFER_OBJECT_BYTES as u64 {
        return Ok(None);
    }
    let bytes = store
        .download(object.object_id)
        .map_err(map_transfer_error)?;
    let pending = match open(&bytes, master, account, chat) {
        Ok(pending) => pending,
        Err(_) => return Ok(None),
    };
    if name(&pending.public_header.package_id) != object.name {
        return Ok(None);
    }
    let package_numeric_id =
        crate::transfer::package_id_from_bytes(pending.public_header.package_id)
            .map_err(map_transfer_error)?
            .get();
    Ok(Some(ManagedVaultFile {
        vault_id: Some(pending.public_header.vault_id),
        health: crate::VaultFileHealth::PendingUpload,
        part_message_ids: pending
            .metadata
            .parts
            .iter()
            .map(|p| p.remote_locator.message_id)
            .collect(),
        package_numeric_id,
        package_id: hex_id(&pending.public_header.package_id),
        logical_name: pending.metadata.logical_name.clone(),
        relative_path: None,
        mime_type: None,
        media_kind: FileKind::Other,
        size_bytes: pending.public_header.logical_file_size,
        encoded_size_bytes: pending
            .metadata
            .parts
            .iter()
            .map(|p| p.encoded_length)
            .sum(),
        part_count: pending.public_header.part_count,
        created_at_unix_ms: pending.public_header.created_at_unix_ms as i64,
        manifest_message_id: object.object_id as i64,
        related_remote_names: pending
            .metadata
            .parts
            .iter()
            .map(|p| p.remote_locator.remote_name.clone())
            .collect(),
    }))
}
impl VaultOwner {
    pub(super) fn resume_remote_upload(
        &mut self,
        account: i64,
        chat: i64,
        message: i64,
        source: &Path,
    ) -> Result<ManagedVaultFile, ApplicationError> {
        self.resume_remote_upload_inner(account, chat, message, source, None)
    }

    pub(super) fn resume_remote_upload_inner(
        &mut self,
        account: i64,
        chat: i64,
        message: i64,
        source: &Path,
        saved: Option<teleark_storage::PendingVaultUploadSnapshot>,
    ) -> Result<ManagedVaultFile, ApplicationError> {
        self.telegram.validate_storage_channel(account, chat)?;
        let mut early_registration = saved
            .as_ref()
            .map(|saved| {
                self.register_transfer(teleark_storage::VaultJobLease {
                    account_id: account,
                    id: saved.record.id,
                    generation: saved.generation,
                })
            })
            .transpose()?;
        let cancellation = early_registration.as_ref().map_or_else(
            || self.upload_controls.exit_pause.cancellation(),
            |owner| owner.cancellation.clone(),
        );
        let master = self
            .master_key
            .as_deref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
        let mut store = TelegramObjectStore::new(self.telegram.clone(), account, chat)
            .with_cancellation(cancellation.clone());
        let bytes = store
            .download(
                u64::try_from(message)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?,
            )
            .map_err(map_transfer_error)?;
        let pending = newest_pending(
            &mut store,
            open(&bytes, master, account, chat)?,
            master,
            account,
            chat,
        )?;
        if self.record.as_ref().map(|r| r.vault_id) != Some(pending.public_header.vault_id) {
            return Err(ApplicationError::new(
                ApplicationErrorKind::VaultKeyUnavailable,
            ));
        }
        if saved.is_none() {
            let name = teleark_crypto::remote_manifest_name(&pending.public_header.package_id);
            for object in store
                .search_exact_caption(crate::transfer::MANIFEST_CAPTION, MAX_MANIFEST_SCAN)
                .map_err(map_transfer_error)?
            {
                if object.name != name {
                    continue;
                }
                let manifest = crate::transfer::recover_manifest(&mut store, master, &object)
                    .map_err(map_transfer_error)?;
                if manifest.public_header.package_id != pending.public_header.package_id
                    || manifest.public_header.vault_id != pending.public_header.vault_id
                    || (pending.source_hash_known
                        && manifest.metadata.whole_plaintext_blake3
                            != pending.metadata.whole_plaintext_blake3)
                {
                    return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
                }
                return managed_file_from_recovered(&crate::RecoveredManifest { object, manifest });
            }
        }
        let sizes = crate::transfer::encrypted_part_sizes_with_limit(
            pending.public_header.logical_file_size,
            pending.public_header.application_part_target,
        )
        .map_err(map_transfer_error)?;
        if sizes.len() != pending.public_header.part_count as usize {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let task = saved
            .as_ref()
            .map_or_else(random_transfer_id, |s| Ok(s.record.id))?;
        let mut files = NativeFileSystem::new();
        files
            .register_source(SourceId(task), source)
            .map_err(map_transfer_error)?;
        let identity = files
            .source_identity(SourceId(task))
            .map_err(map_transfer_error)?;
        if identity.size_bytes != pending.public_header.logical_file_size {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
        }
        let mut local = crate::VaultPendingUploadContext {
            account_id: account,
            task_id: task,
            chat_id: chat,
            batch_id: saved.as_ref().map_or(task, |s| s.record.batch_id),
            created_at_unix_ms: pending.public_header.created_at_unix_ms,
            vault_id: pending.public_header.vault_id,
            master_key_generation: pending.public_header.master_key_generation,
            file_name: pending.metadata.logical_name.clone(),
            source: std::fs::canonicalize(source).map_err(map_source_io)?,
            identity,
        };
        if let Some(saved) = &saved {
            let prior = crate::VaultPendingUploadContext::from_record(&saved.record)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
            if prior.source != local.source
                || !crate::vault_recovery::upload_source_metadata_matches(prior.identity, identity)
                || prior.vault_id != local.vault_id
                || prior.master_key_generation != local.master_key_generation
                || prior.file_name != local.file_name
                || prior.created_at_unix_ms != local.created_at_unix_ms
            {
                return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
            }
            local = prior;
        }
        let admission = local
            .admission_record()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        let mut db = teleark_storage::Database::open(self.library.database_path.as_ref())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let generation = saved.as_ref().map_or(0, |s| s.generation);
        if saved.as_ref().is_some_and(|s| s.record != admission) {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
        }
        db.admit_pending_vault_uploads(std::slice::from_ref(&admission))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let registration = match early_registration.take() {
            Some(owner) => owner,
            None => self.register_transfer(teleark_storage::VaultJobLease {
                account_id: account,
                id: task,
                generation,
            })?,
        };
        let current = db
            .pending_vault_upload(account, task)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        if current.generation != generation
            || current.state != teleark_storage::PendingVaultUploadState::Queued
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        remember_remote(&self.library.database_path, &admission, message)?;
        let cancellation = registration.cancellation.clone();
        let plan = QueuedUpload {
            id: task,
            batch_id: local.batch_id,
            queued_at: local.created_at_unix_ms as i64,
            generation,
            source: VaultUploadSource {
                path: local.source.clone(),
                file_name: local.file_name.clone(),
                size_bytes: identity.size_bytes,
                modified: UNIX_EPOCH
                    .checked_add(std::time::Duration::from_nanos(identity.modified_at_units)),
            },
            pending: local,
        };
        let mut row = VaultTransferSnapshot {
            recovery_state: Some(teleark_storage::VaultJobState::Queued),
            restored: false,
            upload_activity: Some(VaultUploadActivity::new(VaultUploadPhase::CheckingSource)),
            id: task,
            account_id: account,
            chat_id: chat,
            batch_id: None,
            queued_at_unix_ms: now_unix_ms()?,
            direction: VaultTransferDirection::Upload,
            file_name: pending.metadata.logical_name.clone(),
            package_id: Some(hex_id(&pending.public_header.package_id)),
            size_bytes: identity.size_bytes,
            transferred_bytes: 0,
            completed_parts: 0,
            part_count: sizes.len() as u32,
            started_at_unix_ms: now_unix_ms()?,
            duration_ms: None,
            average_bytes_per_second: None,
            destination: None,
            session_log_path: None,
            telemetry: transfer_controller(true, 0, self.library.preferences()?.transfer_tuning)?
                .snapshot(),
            server_status: None,
            state: VaultTransferState::Running,
        };
        if self.transfers.get(task).is_some() {
            if let Some(activity) = self
                .transfers
                .get(task)
                .and_then(|existing| existing.upload_activity)
            {
                row.upload_activity = Some(activity);
            }
            self.update_transfer(task, |existing| *existing = row);
        } else {
            self.push_transfer(row)?;
        }
        let observer = VaultUploadObserver::new(self.transfers.clone(), task);
        let result = (|| {
            let digests = source_digest::inspect(
                source,
                &sizes,
                |bytes, total| {
                    observer.source_progress(bytes, total);
                    Ok(())
                },
                || cancellation.is_cancelled(),
            )
            .map_err(map_transfer_error)?;
            if pending.source_hash_known
                && digests.whole.0 != pending.metadata.whole_plaintext_blake3
            {
                return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
            }
            let context = VaultRecoveryContext {
                account_id: account,
                task_id: task,
                chat_id: chat,
                package_id: pending.public_header.package_id,
                vault_id: pending.public_header.vault_id,
                master_key_generation: pending.public_header.master_key_generation,
                file_key_wrap: pending.public_header.file_key_wrap.clone(),
                file_name: pending.metadata.logical_name.clone(),
                created_at_unix_ms: pending.public_header.created_at_unix_ms,
                size_bytes: identity.size_bytes,
                container_plaintext_limit: pending.public_header.application_part_target,
                direction: crate::VaultRecoveryDirection::Upload {
                    source: std::fs::canonicalize(source).map_err(map_source_io)?,
                    identity,
                    source_blake3: digests.whole.0,
                },
            };
            let mut parts = Vec::with_capacity(pending.metadata.parts.len());
            for part in &pending.metadata.parts {
                if digests
                    .parts
                    .get(part.part_index as usize)
                    .map(|digest| digest.0)
                    != Some(part.plaintext_blake3)
                {
                    return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
                }
                let header = teleark_crypto::PartHeader::new(
                    context.package_id,
                    teleark_crypto::PartInstanceId(part.part_instance_id),
                    part.part_index,
                    sizes.len() as u32,
                    context.size_bytes,
                    part.plaintext_offset,
                    part.plaintext_length,
                    pending.public_header.frame_plaintext_max,
                    teleark_crypto::PartLimits::default(),
                )
                .and_then(|header| header.aligned(teleark_crypto::PartLimits::default()))
                .map_err(map_crypto_error)?;
                let reservation = crate::VaultPartRecovery {
                    header,
                    plaintext_blake3: part.plaintext_blake3,
                    publication_random_id: random_nonzero_u64()? as i64,
                };
                let identity = reservation
                    .encode()
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
                let receipt = crate::durable_upload::encode_receipt(
                    part.remote_locator.message_id as u64,
                    &identity,
                )
                .map_err(map_transfer_error)?;
                parts.push(teleark_storage::VaultPartRecord {
                    part_index: part.part_index,
                    identity,
                    receipt: Some(receipt),
                });
            }
            observer.phase(VaultUploadPhase::SavingRecovery);
            plan.pending
                .verify_executable(&context)
                .map_err(map_transfer_error)?;
            if cancellation.is_cancelled() {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            let record = context
                .admission_record()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
            if !db
                .import_vault_upload(&admission, generation, &record, &parts)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            drop(registration);
            self.upload_with_digests(account, chat, source, None, Some(record), Some(digests))
        })();
        if let Err(error) = &result {
            self.settle_pending_upload(&plan, error.kind())?;
            if let Some(saved) = db
                .pending_vault_upload(account, task)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                && saved.state != teleark_storage::PendingVaultUploadState::Promoted
            {
                let mut row = super::pending_upload::pending_snapshot(&saved)?;
                row.upload_activity = self
                    .transfers
                    .get(task)
                    .and_then(|existing| existing.upload_activity);
                self.update_transfer(task, |existing| *existing = row);
            }
        }
        result
    }
}
