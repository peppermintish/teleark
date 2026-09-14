//! Ledger-backed part execution. Call only on a retained background owner.
use crate::{
    EncryptedRemoteTransport, ReservedPublicationStore, VaultPartRecovery, VaultRecoveryContext,
    VaultRecoveryDirection,
};
use std::num::NonZeroI64;
use teleark_core::{AccountId, PartIndex, TransferError};
use teleark_crypto::{OsRandom, RandomSource, VaultMasterKey};
use teleark_storage::{Database, VaultJobLease, VaultJobState, VaultPartRecord};
#[cfg(test)]
use teleark_transfer::{Blake3Digest, DigestPort};
use teleark_transfer::{RemoteObject, RemotePartKey};

/// Binds an authenticated upload context and one execution generation. Source
/// admission/revalidation belongs to the source owner; this worker checks each
/// retained part digest again before any nonce reuse.
pub struct DurableUploadParts<S> {
    lease: VaultJobLease,
    context: VaultRecoveryContext,
    encoded_context: Vec<u8>,
    transport: EncryptedRemoteTransport<S>,
}
/// One committed reservation ready for a bounded encryption worker. It owns
/// no database connection or transport, so expensive encryption is independent
/// of the ledger and network owners.
pub(crate) struct DurableUploadEncryption {
    #[cfg(test)]
    lease: VaultJobLease,
    #[cfg(test)]
    context_digest: [u8; 32],
    identity: Vec<u8>,
    reservation: VaultPartRecovery,
    receipt_id: Option<u64>,
    #[cfg(test)]
    encryption: crate::transfer::PartEncryptionContext,
    #[cfg(test)]
    plan: crate::transfer::PartEncryptionPlan,
}

/// Opaque encrypted result retains the execution scope of its reservation.
#[cfg(test)]
pub(crate) struct DurablePreparedUpload {
    lease: VaultJobLease,
    context_digest: [u8; 32],
    identity: Vec<u8>,
    reservation: VaultPartRecovery,
    receipt_id: Option<u64>,
    prepared: crate::transfer::PreparedEncryptedPart,
}
#[cfg(test)]
impl DurableUploadEncryption {
    pub fn encrypt(self, plaintext: Vec<u8>) -> Result<DurablePreparedUpload, TransferError> {
        let prepared =
            EncryptedRemoteTransport::<crate::TelegramObjectStore>::encrypt_planned_part(
                &self.encryption,
                self.plan,
                plaintext,
            )?;
        Ok(DurablePreparedUpload {
            lease: self.lease,
            context_digest: self.context_digest,
            identity: self.identity,
            reservation: self.reservation,
            receipt_id: self.receipt_id,
            prepared,
        })
    }
}

impl<S: ReservedPublicationStore> DurableUploadParts<S> {
    pub fn open(
        database: &Database,
        lease: VaultJobLease,
        store: S,
        master: &VaultMasterKey,
    ) -> Result<Self, TransferError> {
        let record = database
            .vault_job(lease.account_id, lease.id)
            .map_err(|_| TransferError::Database)?
            .ok_or(TransferError::Database)?;
        if record.generation != lease.generation || record.state != VaultJobState::Running {
            return Err(TransferError::Cancelled);
        }
        let context = VaultRecoveryContext::from_record(&record)
            .map_err(|_| TransferError::ManifestCorrupted)?;
        if !matches!(context.direction, VaultRecoveryDirection::Upload { .. }) {
            return Err(TransferError::ManifestCorrupted);
        }
        let file_key = context
            .file_key(master)
            .map_err(|_| TransferError::KeyUnavailable)?;
        let transport = EncryptedRemoteTransport::new(
            store,
            AccountId::new(context.account_id),
            context.chat_id,
            crate::transfer::package_id_from_bytes(context.package_id)?,
            file_key,
            context.size_bytes,
            context.part_sizes()?,
        )?;
        Ok(Self {
            lease,
            context,
            encoded_context: record.context,
            transport,
        })
    }

    pub(crate) fn with_cancellation(
        mut self,
        cancellation: crate::TelegramScanCancellation,
    ) -> Self {
        self.transport = self.transport.with_cancellation(cancellation);
        self
    }

    /// Commit the immutable reservation before encrypting/sending. A late
    /// successful send may be recorded while pausing/cancelling, but never
    /// after acknowledgment or after another generation has started.
    #[cfg(test)]
    pub(crate) fn upload_part(
        &mut self,
        database: &mut Database,
        index: u32,
        plaintext: Vec<u8>,
    ) -> Result<RemoteObject, TransferError> {
        let encryption = self.prepare_part(database, index, &plaintext)?;
        let prepared = encryption.encrypt(plaintext)?;
        self.publish_prepared(database, prepared)
    }

    /// Run on the admission owner before handing this job to parallel crypto
    /// workers. Only the immutable reservation crosses the worker boundary.
    #[cfg(test)]
    pub(crate) fn prepare_part(
        &mut self,
        database: &mut Database,
        index: u32,
        plaintext: &[u8],
    ) -> Result<DurableUploadEncryption, TransferError> {
        self.prepare_part_digest(
            database,
            index,
            plaintext.len() as u64,
            Blake3Digest.digest(plaintext),
        )
    }

    /// Accepts the digest produced by source admission; encryption still
    /// checks the actual bytes against it before using the saved nonce.
    pub(crate) fn prepare_part_digest(
        &mut self,
        database: &mut Database,
        index: u32,
        plaintext_length: u64,
        digest: teleark_transfer::ContentDigest,
    ) -> Result<DurableUploadEncryption, TransferError> {
        let record = database
            .vault_job(self.lease.account_id, self.lease.id)
            .map_err(|_| TransferError::Database)?
            .ok_or(TransferError::Database)?;
        if record.generation != self.lease.generation || record.state != VaultJobState::Running {
            return Err(TransferError::Cancelled);
        }
        if record.context != self.encoded_context {
            return Err(TransferError::ManifestCorrupted);
        }
        let offset = u64::from(index) * self.context.container_plaintext_limit;
        let expected_size = self
            .context
            .size_bytes
            .checked_sub(offset)
            .filter(|size| *size != 0)
            .ok_or(TransferError::ManifestCorrupted)?
            .min(self.context.container_plaintext_limit);
        if plaintext_length != expected_size {
            return Err(TransferError::SourceChanged);
        }
        let key = RemotePartKey {
            account_id: AccountId::new(self.context.account_id),
            package_id: crate::transfer::package_id_from_bytes(self.context.package_id)?,
            part_index: PartIndex::new(index),
        };
        let existing = database
            .vault_parts(
                self.lease.account_id,
                self.lease.id,
                index.checked_sub(1),
                1,
            )
            .map_err(|_| TransferError::Database)?
            .into_iter()
            .find(|part| part.part_index == index);
        let part = if let Some(part) = existing {
            part
        } else {
            let mut random_bytes = [0u8; 8];
            let mut random_id = None;
            for _ in 0..16 {
                OsRandom
                    .fill_bytes(&mut random_bytes)
                    .map_err(|_| TransferError::KeyUnavailable)?;
                random_id = NonZeroI64::new(i64::from_le_bytes(random_bytes));
                if random_id.is_some() {
                    break;
                }
            }
            let reservation = self.transport.reserve_part_identity(
                key,
                digest,
                random_id.ok_or(TransferError::KeyUnavailable)?,
            )?;
            VaultPartRecord {
                part_index: index,
                identity: reservation
                    .encode()
                    .map_err(|_| TransferError::ManifestCorrupted)?,
                receipt: None,
            }
        };
        let reservation = VaultPartRecovery::decode(&part.identity)
            .map_err(|_| TransferError::ManifestCorrupted)?;
        // Recheck the lease transactionally for both new and existing parts.
        let unreceipted = VaultPartRecord {
            receipt: None,
            ..part.clone()
        };
        if !database
            .reserve_vault_part(self.lease, &unreceipted)
            .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        let receipt_id = part
            .receipt
            .as_deref()
            .map(|receipt| decode_receipt(receipt, &part.identity))
            .transpose()?;
        let _plan = self.transport.restore_reserved_plan(key, &reservation)?;
        Ok(DurableUploadEncryption {
            #[cfg(test)]
            lease: self.lease,
            #[cfg(test)]
            context_digest: *blake3::hash(&self.encoded_context).as_bytes(),
            identity: part.identity,
            reservation,
            receipt_id,
            #[cfg(test)]
            encryption: self.transport.encryption_context(),
            #[cfg(test)]
            plan: _plan,
        })
    }

    /// The sender rechecks the lease after waiting for encryption. No mutex or
    /// database transaction is held while uploading/verifying remote bytes.
    #[cfg(test)]
    pub(crate) fn publish_prepared(
        &mut self,
        database: &mut Database,
        prepared: DurablePreparedUpload,
    ) -> Result<RemoteObject, TransferError> {
        if prepared.lease != self.lease
            || prepared.context_digest != *blake3::hash(&self.encoded_context).as_bytes()
        {
            return Err(TransferError::Cancelled);
        }
        let index = prepared.prepared.key.part_index.get();
        let part = VaultPartRecord {
            part_index: index,
            identity: prepared.identity,
            receipt: None,
        };
        if !database
            .reserve_vault_part(self.lease, &part)
            .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        let object = self.transport.publish_reserved_prepared(
            prepared.prepared,
            &prepared.reservation,
            prepared.receipt_id,
        )?;
        let receipt = encode_receipt(object.object_id, &part.identity)?;
        if !database
            .confirm_vault_part(self.lease, index, &receipt)
            .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        Ok(object)
    }

    /// Stream one application container in bounded 512 KiB wire blocks. A
    /// surviving ciphertext spool is retransmitted verbatim; a lost/partial
    /// spool burns its reservation before any new encryption is admitted.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn upload_source_part(
        &mut self,
        database: &mut Database,
        index: u32,
        plaintext_length: u64,
        digest: teleark_transfer::ContentDigest,
        source: &std::path::Path,
        root: &std::path::Path,
        tuning: teleark_telegram::TransferTuning,
    ) -> Result<RemoteObject, TransferError> {
        std::fs::create_dir_all(root).map_err(|_| TransferError::Database)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| TransferError::Database)?;
        }
        let key = RemotePartKey {
            account_id: AccountId::new(self.context.account_id),
            package_id: crate::transfer::package_id_from_bytes(self.context.package_id)?,
            part_index: PartIndex::new(index),
        };
        let previous = database
            .vault_parts(
                self.lease.account_id,
                self.lease.id,
                index.checked_sub(1),
                1,
            )
            .map_err(|_| TransferError::Database)?
            .into_iter()
            .find(|part| part.part_index == index);
        if let Some(previous) = previous {
            let reservation = VaultPartRecovery::decode(&previous.identity)
                .map_err(|_| TransferError::ManifestCorrupted)?;
            if previous.receipt.is_none()
                && let Some(object) = self.transport.reconcile_summary_receipt(
                    key,
                    &reservation,
                    &previous.identity,
                    root,
                )?
            {
                let receipt = encode_receipt(object.object_id, &previous.identity)?;
                if !database
                    .confirm_vault_part(self.lease, index, &receipt)
                    .map_err(|_| TransferError::Database)?
                {
                    return Err(TransferError::Cancelled);
                }
                return Ok(object);
            }
            if previous.receipt.is_none()
                && !crate::transfer::streaming::reusable_spool(
                    root,
                    &previous.identity,
                    &reservation,
                    false,
                )
            {
                let mut bytes = [0; 8];
                OsRandom
                    .fill_bytes(&mut bytes)
                    .map_err(|_| TransferError::KeyUnavailable)?;
                let random_id = NonZeroI64::new(i64::from_le_bytes(bytes))
                    .ok_or(TransferError::KeyUnavailable)?;
                let replacement = self.transport.replacement_part_identity(
                    key,
                    &reservation,
                    digest,
                    random_id,
                )?;
                let encoded = replacement
                    .encode()
                    .map_err(|_| TransferError::ManifestCorrupted)?;
                if !database
                    .replace_unpublished_vault_part(self.lease, index, &previous.identity, &encoded)
                    .map_err(|_| TransferError::Database)?
                {
                    return Err(TransferError::Cancelled);
                }
                // The old identity is now permanently retired. Discard only its
                // uncommitted local spool, after the replacement transaction.
                let prefix = crate::transfer::streaming::spool_prefix(root, &previous.identity);
                for extension in ["ciphertext", "seal", "seal-pending", "upload", "pending"] {
                    let _ = std::fs::remove_file(prefix.with_extension(extension));
                }
            }
        }
        let prepared = self.prepare_part_digest(database, index, plaintext_length, digest)?;
        let object = self.transport.stream_reserved_source(
            key,
            &prepared.reservation,
            &prepared.identity,
            source,
            root,
            prepared.receipt_id,
            tuning,
        )?;
        let receipt = encode_receipt(object.object_id, &prepared.identity)?;
        if !database
            .confirm_vault_part(self.lease, index, &receipt)
            .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        // Keep the tiny summary for future manifest reconstruction; confirmed
        // remote bytes no longer require a full local ciphertext spool.
        let prefix = crate::transfer::streaming::spool_prefix(root, &prepared.identity);
        let _ = std::fs::remove_file(prefix.with_extension("ciphertext"));
        let _ = std::fs::remove_file(prefix.with_extension("upload"));
        Ok(object)
    }

    pub fn publish_manifest(
        &mut self,
        database: &mut Database,
        master: &VaultMasterKey,
        request: crate::ManifestPublishRequest,
    ) -> Result<crate::RemoteByteObject, TransferError> {
        self.transport.publish_durable_manifest(
            database,
            self.lease,
            &self.context,
            master,
            request,
        )
    }

    pub(crate) fn confirmed_manifest_parts(&self) -> Vec<teleark_crypto::ManifestPart> {
        self.transport.confirmed_manifest_parts()
    }
    pub fn into_transport(self) -> EncryptedRemoteTransport<S> {
        self.transport
    }

    /// Finish a saved manifest publication without reading the upload source.
    /// None means no complete envelope was persisted and source work is needed.
    pub fn resume_saved_manifest(
        &mut self,
        database: &mut Database,
        master: &VaultMasterKey,
    ) -> Result<Option<crate::RemoteByteObject>, TransferError> {
        self.transport
            .resume_saved_manifest(database, self.lease, &self.context, master)
    }
}

// TARKUR01, u32 codec version, positive i64-compatible message ID, BLAKE3
// reservation identity, BLAKE3 checksum. Exactly 84 bytes, all integers LE.
pub(crate) fn encode_receipt(id: u64, identity: &[u8]) -> Result<Vec<u8>, TransferError> {
    if id == 0 || id > i64::MAX as u64 {
        return Err(TransferError::ManifestCorrupted);
    }
    let mut bytes = Vec::with_capacity(84);
    bytes.extend_from_slice(b"TARKUR01");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(blake3::hash(identity).as_bytes());
    let checksum = blake3::hash(&bytes);
    bytes.extend_from_slice(checksum.as_bytes());
    Ok(bytes)
}
fn decode_receipt(bytes: &[u8], identity: &[u8]) -> Result<u64, TransferError> {
    if bytes.len() != 84
        || &bytes[..8] != b"TARKUR01"
        || bytes[8..12] != 1u32.to_le_bytes()
        || &bytes[20..52] != blake3::hash(identity).as_bytes()
        || &bytes[52..] != blake3::hash(&bytes[..52]).as_bytes()
    {
        return Err(TransferError::ManifestCorrupted);
    }
    let id = u64::from_le_bytes(
        bytes[12..20]
            .try_into()
            .map_err(|_| TransferError::ManifestCorrupted)?,
    );
    if id == 0 || id > i64::MAX as u64 {
        return Err(TransferError::ManifestCorrupted);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_codec_rejects_truncation_mutation_and_foreign_reservation() {
        let encoded = encode_receipt(99, b"immutable-part").expect("encode");
        assert_eq!(
            encoded.as_slice(),
            include_bytes!("vault_recovery/fixtures/upload-receipt-v1.bin")
        );
        assert_eq!(encoded.len(), 84);
        assert_eq!(decode_receipt(&encoded, b"immutable-part"), Ok(99));
        assert!(decode_receipt(&encoded, b"other-part").is_err());
        for length in 0..encoded.len() {
            assert!(decode_receipt(&encoded[..length], b"immutable-part").is_err());
        }
        for offset in 0..encoded.len() {
            let mut changed = encoded.clone();
            changed[offset] ^= 1;
            assert!(decode_receipt(&changed, b"immutable-part").is_err());
        }
        assert!(encode_receipt(0, b"immutable-part").is_err());
        assert!(encode_receipt(u64::MAX, b"immutable-part").is_err());
    }
}
