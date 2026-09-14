//! Generation-fenced manifest publication after every part is verified.
use super::*;
use std::num::NonZeroI64;
use teleark_crypto::{RandomSource, manifest_content_commitment};
use teleark_storage::{Database, VaultJobLease, VaultManifestOutbox};

impl<S: ReservedPublicationStore> EncryptedRemoteTransport<S> {
    pub(crate) fn resume_saved_manifest(
        &mut self,
        database: &mut Database,
        lease: VaultJobLease,
        context: &crate::VaultRecoveryContext,
        master: &VaultMasterKey,
    ) -> Result<Option<RemoteByteObject>, TransferError> {
        let Some(outbox) = database
            .vault_manifest_outbox(lease.account_id, lease.id)
            .map_err(|_| TransferError::Database)?
        else {
            return Ok(None);
        };
        if outbox.codec_version != 1 {
            return Err(TransferError::ManifestCorrupted);
        }
        let Some(envelope) = outbox.envelope else {
            return Ok(None);
        };
        let opened = open_manifest(&envelope, master, ManifestLimits::default())
            .map_err(map_crypto_error)?;
        // The saved authenticated envelope was committed only after verified
        // parts. Reconstruct its publication inputs without reading plaintext.
        // The shared publisher checks the current lease, context, commitment,
        // exact envelope and remote receipt before acknowledging success.
        self.manifest_parts = opened
            .metadata
            .parts
            .iter()
            .map(|part| (part.part_index, part.clone()))
            .collect();
        let request = ManifestPublishRequest {
            vault_id: opened.public_header.vault_id,
            master_key_generation: opened.public_header.master_key_generation,
            wrap_generation: opened.public_header.file_key_wrap.wrap_generation,
            created_at_unix_ms: opened.public_header.created_at_unix_ms,
            manifest_generation: opened.public_header.manifest_generation,
            logical_name: opened.metadata.logical_name.clone(),
            relative_path: opened.metadata.relative_path.clone(),
            mime_type: opened.metadata.mime_type.clone(),
            media_kind: opened.metadata.media_kind,
            whole_plaintext_blake3: opened.metadata.whole_plaintext_blake3,
        };
        drop(opened);
        drop(envelope);
        self.publish_durable_manifest(database, lease, context, master, request)
            .map(Some)
    }

    pub(crate) fn publish_durable_manifest(
        &mut self,
        database: &mut Database,
        lease: VaultJobLease,
        context: &crate::VaultRecoveryContext,
        master: &VaultMasterKey,
        request: ManifestPublishRequest,
    ) -> Result<RemoteByteObject, TransferError> {
        let record = database
            .vault_job(lease.account_id, lease.id)
            .map_err(|_| TransferError::Database)?
            .ok_or(TransferError::ManifestCorrupted)?;
        if record.generation != lease.generation
            || record.state != teleark_storage::VaultJobState::Running
        {
            return Err(TransferError::Cancelled);
        }
        let saved = crate::VaultRecoveryContext::from_record(&record)
            .map_err(|_| TransferError::ManifestCorrupted)?;
        if &saved != context
            || context.account_id != self.account_id.get()
            || context.task_id != lease.id
            || context.package_id != self.package_bytes
            || context.chat_id != self.chat_id
            || context.size_bytes != self.logical_file_size
            || request.vault_id != context.vault_id
            || request.master_key_generation != context.master_key_generation
            || request.wrap_generation != context.file_key_wrap.wrap_generation
            || request.created_at_unix_ms != context.created_at_unix_ms
            || request.manifest_generation != 1
            || request.logical_name != context.file_name
        {
            return Err(TransferError::ManifestCorrupted);
        }
        let crate::VaultRecoveryDirection::Upload { source_blake3, .. } = context.direction else {
            return Err(TransferError::ManifestCorrupted);
        };
        if request.whole_plaintext_blake3 != source_blake3
            || self.manifest_parts.len() != self.part_sizes.len()
        {
            return Err(TransferError::ManifestCorrupted);
        }
        let parts = (0..self.part_sizes.len())
            .map(|index| {
                let index = u32::try_from(index).map_err(|_| TransferError::ManifestCorrupted)?;
                self.manifest_parts
                    .get(&index)
                    .cloned()
                    .ok_or(TransferError::ManifestCorrupted)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let public = ManifestPublicHeader {
            package_id: self.package_bytes,
            vault_id: context.vault_id,
            manifest_generation: request.manifest_generation,
            created_at_unix_ms: context.created_at_unix_ms,
            logical_file_size: self.logical_file_size,
            part_count: u32::try_from(parts.len()).map_err(|_| TransferError::ManifestCorrupted)?,
            application_part_target: *self
                .part_sizes
                .first()
                .ok_or(TransferError::ManifestCorrupted)?,
            frame_plaintext_max: FRAME_PLAINTEXT_BYTES,
            nonce_strategy_id: NONCE_STRATEGY_ID,
            crypto_suite_id: CRYPTO_SUITE_ID,
            file_key_wrap: context.file_key_wrap.clone(),
            master_key_generation: context.master_key_generation,
            flags: 1,
        };
        let metadata = ManifestMetadata {
            logical_name: request.logical_name,
            relative_path: request.relative_path,
            mime_type: request.mime_type,
            media_kind: request.media_kind,
            whole_plaintext_blake3: request.whole_plaintext_blake3,
            parts,
            logical_timestamps: None,
            source_metadata: Vec::new(),
            format_extensions: Vec::new(),
        };
        let limits = ManifestLimits::default();
        let commitment =
            manifest_content_commitment(&public, &metadata, limits).map_err(map_crypto_error)?;
        let outbox = match database
            .vault_manifest_outbox(lease.account_id, lease.id)
            .map_err(|_| TransferError::Database)?
        {
            Some(saved) => saved,
            None => {
                let mut random = [0; 8];
                let mut random_id = 0;
                for _ in 0..16 {
                    OsRandom.fill_bytes(&mut random).map_err(map_crypto_error)?;
                    random_id = i64::from_le_bytes(random);
                    if random_id != 0 {
                        break;
                    }
                }
                if random_id == 0 {
                    return Err(TransferError::KeyUnavailable);
                }
                VaultManifestOutbox {
                    codec_version: 1,
                    commitment,
                    random_id,
                    envelope: None,
                    message_id: None,
                }
            }
        };
        if outbox.codec_version != 1 || outbox.commitment != commitment {
            return Err(TransferError::ManifestCorrupted);
        }
        let reservation = VaultManifestOutbox {
            envelope: None,
            message_id: None,
            codec_version: outbox.codec_version,
            commitment: outbox.commitment,
            random_id: outbox.random_id,
        };
        if !database
            .reserve_vault_manifest(lease, &reservation)
            .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        // No AEAD operation precedes the immutable commitment transaction.
        let envelope = match outbox.envelope {
            Some(bytes) => bytes,
            None => seal_manifest(&public, &metadata, &self.file_key, limits, &mut self.usage)
                .map_err(map_crypto_error)?,
        };
        let opened = open_manifest(&envelope, master, limits).map_err(map_crypto_error)?;
        if envelope.len() > MAX_TRANSFER_OBJECT_BYTES
            || opened.public_header != public
            || opened.metadata != metadata
        {
            return Err(TransferError::HashMismatch);
        }
        if !database
            .save_vault_manifest_envelope(lease, &envelope)
            .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        let name = remote_manifest_name(&self.package_bytes);
        let mut object = outbox.message_id.map(|id| RemoteByteObject {
            object_id: id as u64,
            name: name.clone(),
            encoded_size: envelope.len() as u64,
        });
        if object.is_none() {
            let candidates = self
                .store
                .search_exact_caption(MANIFEST_CAPTION, MAX_RECONCILIATION_RESULTS)?;
            if candidates.len() >= MAX_RECONCILIATION_RESULTS {
                return Err(TransferError::RemoteMissing);
            }
            for candidate in candidates
                .into_iter()
                .filter(|candidate| candidate.name == name)
            {
                if candidate.encoded_size != envelope.len() as u64
                    || self.store.download(candidate.object_id)? != envelope
                {
                    return Err(TransferError::HashMismatch);
                }
                if object.replace(candidate).is_some() {
                    return Err(TransferError::ManifestCorrupted);
                }
            }
        }
        let object = match object {
            Some(object) => object,
            None => self
                .store
                .upload_reserved(
                    &name,
                    MANIFEST_CAPTION,
                    envelope.clone(),
                    NonZeroI64::new(outbox.random_id).ok_or(TransferError::ManifestCorrupted)?,
                )
                .map_err(|error| match error {
                    UploadError::Definite(error) => error,
                    UploadError::AmbiguousSuccess => TransferError::Network,
                })?,
        };
        if object.object_id == 0
            || object.object_id > i64::MAX as u64
            || object.name != name
            || object.encoded_size != envelope.len() as u64
            || self.store.download(object.object_id)? != envelope
        {
            return Err(TransferError::HashMismatch);
        }
        if !database
            .confirm_vault_manifest(lease, object.object_id as i64)
            .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        self.published_manifest_envelope = Some(envelope);
        Ok(object)
    }
}
