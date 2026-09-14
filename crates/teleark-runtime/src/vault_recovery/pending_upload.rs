//! Pending source identity is distinct from an executable encryption checkpoint.
use super::*;
use teleark_core::TransferError;
use teleark_transfer::{NativeFileSystem, SourceId, SourcePort};

const PENDING_MAGIC: &[u8; 8] = b"TARKVQ01";
const MAX_PENDING: usize = 131_072;

/// Metadata-only queue entry. No encryption identity or content hash is invented
/// before reading the file. Vault/key scope prevents silent retargeting on restart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultPendingUploadContext {
    pub account_id: i64,
    pub task_id: u64,
    pub chat_id: i64,
    pub batch_id: u64,
    pub created_at_unix_ms: u64,
    pub vault_id: [u8; 16],
    pub master_key_generation: u32,
    pub file_name: String,
    pub source: PathBuf,
    pub identity: SourceIdentity,
}

impl VaultPendingUploadContext {
    /// Background-only metadata inspection; never reads source contents. The
    /// chosen canonical target and full native identity are retained separately
    /// from the user-visible filename.
    pub fn inspect_source(&mut self, path: &std::path::Path) -> Result<(), TransferError> {
        let source = std::fs::canonicalize(path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => TransferError::SourceMissing,
            std::io::ErrorKind::PermissionDenied => TransferError::PermissionDenied,
            _ => TransferError::SourceChanged,
        })?;
        let mut files = NativeFileSystem::new();
        files.register_source(SourceId(self.task_id), &source)?;
        let identity = files.source_identity(SourceId(self.task_id))?;
        if identity.size_bytes == 0 {
            return Err(TransferError::SourceMissing);
        }
        self.source = source;
        self.identity = identity;
        Ok(())
    }

    pub fn admission_record(
        &self,
    ) -> Result<teleark_storage::PendingVaultUpload, RecoveryContextError> {
        Ok(teleark_storage::PendingVaultUpload {
            account_id: self.account_id,
            id: self.task_id,
            chat_id: self.chat_id,
            batch_id: self.batch_id,
            created_at_unix_ms: self.created_at_unix_ms as i64,
            codec_version: 1,
            context: self.encode()?,
        })
    }

    pub fn from_record(
        record: &teleark_storage::PendingVaultUpload,
    ) -> Result<Self, RecoveryContextError> {
        if record.codec_version != 1 {
            return Err(RecoveryContextError::UnsupportedVersion);
        }
        let context = Self::decode(&record.context)?;
        if context.account_id != record.account_id
            || context.task_id != record.id
            || context.chat_id != record.chat_id
            || context.batch_id != record.batch_id
            || record.created_at_unix_ms < 0
            || context.created_at_unix_ms != record.created_at_unix_ms as u64
        {
            return Err(RecoveryContextError::Corrupt);
        }
        Ok(context)
    }

    /// Called after source hashing, before atomic promotion. New encryption keys
    /// may be allocated only within the admitted source/vault scope. A whole hash
    /// alone cannot justify replacing the queued file with a same-byte sibling.
    pub fn verify_executable(
        &self,
        executable: &VaultRecoveryContext,
    ) -> Result<(), TransferError> {
        self.validate()
            .map_err(|_| TransferError::ManifestCorrupted)?;
        executable
            .validate()
            .map_err(|_| TransferError::ManifestCorrupted)?;
        if self.account_id != executable.account_id
            || self.chat_id != executable.chat_id
            || self.task_id != executable.task_id
            || self.created_at_unix_ms != executable.created_at_unix_ms
            || self.file_name != executable.file_name
        {
            return Err(TransferError::ManifestCorrupted);
        }
        if self.vault_id != executable.vault_id
            || self.master_key_generation != executable.master_key_generation
        {
            return Err(TransferError::KeyUnavailable);
        }
        let VaultRecoveryDirection::Upload {
            source, identity, ..
        } = &executable.direction
        else {
            return Err(TransferError::ManifestCorrupted);
        };
        if self.source != *source
            || self.identity != *identity
            || self.identity.size_bytes != executable.size_bytes
        {
            return Err(TransferError::SourceChanged);
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, RecoveryContextError> {
        self.validate()?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(PENDING_MAGIC);
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&self.account_id.to_le_bytes());
        bytes.extend_from_slice(&self.task_id.to_le_bytes());
        bytes.extend_from_slice(&self.chat_id.to_le_bytes());
        bytes.extend_from_slice(&self.batch_id.to_le_bytes());
        bytes.extend_from_slice(&self.created_at_unix_ms.to_le_bytes());
        bytes.extend_from_slice(&self.vault_id);
        bytes.extend_from_slice(&self.master_key_generation.to_le_bytes());
        bytes.extend_from_slice(&self.identity.filesystem_id.to_le_bytes());
        bytes.extend_from_slice(&self.identity.size_bytes.to_le_bytes());
        bytes.extend_from_slice(&self.identity.modified_at_units.to_le_bytes());
        bytes.extend_from_slice(&self.identity.revision.to_le_bytes());
        append(&mut bytes, self.file_name.as_bytes(), MAX_NAME)?;
        encode_path(&mut bytes, &self.source)?;
        let checksum = blake3::hash(&bytes);
        bytes.extend_from_slice(checksum.as_bytes());
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RecoveryContextError> {
        if bytes.len() < 44 || bytes.len() > MAX_PENDING || &bytes[..8] != PENDING_MAGIC {
            return Err(RecoveryContextError::Invalid);
        }
        if bytes[8..12] != 1_u32.to_le_bytes() {
            return Err(RecoveryContextError::UnsupportedVersion);
        }
        let payload = bytes.len() - 32;
        if blake3::hash(&bytes[..payload]).as_bytes().as_slice() != &bytes[payload..] {
            return Err(RecoveryContextError::Corrupt);
        }
        let mut reader = Reader(&bytes[12..payload]);
        let context = Self {
            account_id: i64::from_le_bytes(reader.array()?),
            task_id: u64::from_le_bytes(reader.array()?),
            chat_id: i64::from_le_bytes(reader.array()?),
            batch_id: u64::from_le_bytes(reader.array()?),
            created_at_unix_ms: u64::from_le_bytes(reader.array()?),
            vault_id: reader.array()?,
            master_key_generation: u32::from_le_bytes(reader.array()?),
            identity: SourceIdentity {
                filesystem_id: u128::from_le_bytes(reader.array()?),
                size_bytes: u64::from_le_bytes(reader.array()?),
                modified_at_units: u64::from_le_bytes(reader.array()?),
                revision: u64::from_le_bytes(reader.array()?),
            },
            file_name: String::from_utf8(reader.bytes(MAX_NAME)?.to_vec())
                .map_err(|_| RecoveryContextError::Invalid)?,
            source: decode_path(&mut reader)?,
        };
        if !reader.0.is_empty() {
            return Err(RecoveryContextError::Invalid);
        }
        context.validate()?;
        Ok(context)
    }

    fn validate(&self) -> Result<(), RecoveryContextError> {
        if self.account_id <= 0
            || self.chat_id <= 0
            || self.task_id == 0
            || self.task_id > i64::MAX as u64
            || self.batch_id == 0
            || self.batch_id > i64::MAX as u64
            || self.created_at_unix_ms > i64::MAX as u64
            || self.vault_id == [0; 16]
            || self.master_key_generation == 0
            || self.identity.size_bytes == 0
            || !self.source.is_absolute()
            || self.file_name.trim().is_empty()
            || self.file_name.len() > MAX_NAME
            || self.file_name.contains('\0')
        {
            return Err(RecoveryContextError::Invalid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> VaultPendingUploadContext {
        let executable = super::super::tests::fixture();
        let VaultRecoveryDirection::Upload {
            source, identity, ..
        } = executable.direction
        else {
            panic!("upload fixture")
        };
        VaultPendingUploadContext {
            account_id: 7,
            task_id: 9,
            chat_id: 11,
            batch_id: 13,
            created_at_unix_ms: 100,
            vault_id: executable.vault_id,
            master_key_generation: 1,
            file_name: executable.file_name,
            source,
            identity,
        }
    }

    #[test]
    fn pending_codec_preserves_scope_and_rejects_truncation_corruption_and_future_bytes() {
        let context = fixture();
        let bytes = context.encode().expect("encode");
        assert_eq!(&bytes[..12], b"TARKVQ01\x01\0\0\0");
        assert_eq!(
            VaultPendingUploadContext::decode(&bytes).expect("decode"),
            context
        );
        let record = context.admission_record().expect("record");
        assert_eq!(
            VaultPendingUploadContext::from_record(&record).expect("bound"),
            context
        );
        for length in 0..bytes.len() {
            assert!(
                VaultPendingUploadContext::decode(&bytes[..length]).is_err(),
                "{length}"
            );
        }
        for index in 0..bytes.len() {
            let mut changed = bytes.clone();
            changed[index] ^= 1;
            assert!(
                VaultPendingUploadContext::decode(&changed).is_err(),
                "{index}"
            );
        }
        let mut future = bytes.clone();
        future[8] = 2;
        assert_eq!(
            VaultPendingUploadContext::decode(&future),
            Err(RecoveryContextError::UnsupportedVersion)
        );
        for field in 0..6 {
            let mut changed = record.clone();
            match field {
                0 => changed.account_id += 1,
                1 => changed.id += 1,
                2 => changed.chat_id += 1,
                3 => changed.batch_id += 1,
                4 => changed.created_at_unix_ms += 1,
                _ => changed.codec_version += 1,
            }
            assert!(VaultPendingUploadContext::from_record(&changed).is_err());
        }
        assert!(
            VaultRecoveryContext::decode(&bytes).is_err(),
            "pending bytes never mean an executable checkpoint"
        );
    }

    #[test]
    fn promotion_requires_original_source_and_vault_even_with_matching_content_hash() {
        let pending = fixture();
        let original = super::super::tests::fixture();
        pending.verify_executable(&original).expect("same source");
        for field in 0..11 {
            let mut changed = original.clone();
            match field {
                0 => changed.account_id += 1,
                1 => changed.task_id += 1,
                2 => changed.chat_id += 1,
                3 => changed.created_at_unix_ms += 1,
                4 => changed.vault_id[0] ^= 1,
                5 => changed.master_key_generation += 1,
                6 => changed.file_name.push('x'),
                7 => changed.size_bytes += 1,
                _ => {
                    let VaultRecoveryDirection::Upload {
                        source, identity, ..
                    } = &mut changed.direction
                    else {
                        panic!("upload")
                    };
                    match field {
                        8 => *source = source.with_file_name("replacement.bin"),
                        9 => identity.filesystem_id += 1,
                        _ => identity.revision += 1,
                    }
                }
            }
            assert!(
                pending.verify_executable(&changed).is_err(),
                "field {field}"
            );
        }
    }

    #[test]
    fn metadata_only_admission_reopens_and_rejects_changed_source_before_executable_promotion() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("source.bin");
        std::fs::write(&path, b"abcdefgh").expect("source");
        let mut pending = fixture();
        pending.inspect_source(&path).expect("metadata");
        let record = pending.admission_record().expect("encode");
        let db_path = dir.path().join("jobs.sqlite");
        let mut db = teleark_storage::Database::open(&db_path).expect("database");
        db.admit_pending_vault_uploads(&[record])
            .expect("durable admission");
        drop(db);
        let db = teleark_storage::Database::open(&db_path).expect("restart");
        let record = db
            .queued_pending_vault_uploads(7, 0, 1)
            .expect("queue")
            .remove(0);
        let restored = VaultPendingUploadContext::from_record(&record).expect("decode saved");
        assert_eq!(restored, pending);
        assert!(
            db.vault_job(7, 9).expect("job").is_none(),
            "no executable identity before hashing"
        );
        let mut executable = super::super::tests::fixture();
        executable.size_bytes = 8;
        executable.direction = VaultRecoveryDirection::Upload {
            source: restored.source.clone(),
            identity: restored.identity,
            source_blake3: *blake3::hash(b"abcdefgh").as_bytes(),
        };
        restored
            .verify_executable(&executable)
            .expect("original scope");
        std::fs::write(&path, b"replacement-longer").expect("changed source");
        let mut changed = restored.clone();
        changed.inspect_source(&path).expect("new metadata");
        let mut replacement = executable.clone();
        replacement.size_bytes = changed.identity.size_bytes;
        replacement.direction = VaultRecoveryDirection::Upload {
            source: changed.source,
            identity: changed.identity,
            source_blake3: *blake3::hash(b"replacement-longer").as_bytes(),
        };
        assert_eq!(
            restored.verify_executable(&replacement),
            Err(TransferError::SourceChanged)
        );
        assert!(db.vault_job(7, 9).expect("unclaimed").is_none());
        assert_eq!(
            db.queued_pending_vault_uploads(7, 0, 1)
                .expect("retained queue"),
            vec![record]
        );
    }

    #[cfg(unix)]
    #[test]
    fn pending_paths_preserve_native_non_utf8_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let mut context = fixture();
        context.source = std::ffi::OsString::from_vec(b"/queued/\xff.bin".to_vec()).into();
        assert_eq!(
            VaultPendingUploadContext::decode(&context.encode().expect("native path"))
                .expect("decode"),
            context
        );
    }
    #[cfg(unix)]
    #[test]
    fn pending_v1_has_a_frozen_native_wire_fixture() {
        // Fixed bytes from the documented field widths/order, independent of
        // the encoder. Future formats must retain this reader unchanged.
        const PAYLOAD: &[u8] = &[
            84, 65, 82, 75, 86, 81, 48, 49, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0,
            0, 0, 11, 0, 0, 0, 0, 0, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0, 100, 0, 0, 0, 0, 0, 0, 0, 3, 3,
            3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 1, 0, 0, 0, 210, 4, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 123, 0, 0, 0, 0, 0, 0, 0, 99, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
            0, 0, 11, 0, 0, 0, 102, 105, 120, 116, 117, 114, 101, 46, 116, 120, 116, 1, 17, 0, 0,
            0, 47, 113, 117, 101, 117, 101, 47, 115, 111, 117, 114, 99, 101, 46, 98, 105, 110,
        ];
        let mut expected = PAYLOAD.to_vec();
        expected.extend_from_slice(blake3::hash(PAYLOAD).as_bytes());
        let mut context = fixture();
        context.source = PathBuf::from("/queue/source.bin");
        assert_eq!(context.encode().expect("encode v1"), expected);
        assert_eq!(
            VaultPendingUploadContext::decode(&expected).expect("read frozen v1"),
            context
        );
    }
}
