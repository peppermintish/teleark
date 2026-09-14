//! Versioned encrypted-transfer recovery context. Wrapped keys only; no raw secrets.
use std::path::PathBuf;
use teleark_crypto::{CRYPTO_SUITE_ID, FileKeyWrap};
use teleark_transfer::SourceIdentity;

mod pending_upload;
pub use pending_upload::VaultPendingUploadContext;

const MAGIC: &[u8; 8] = b"TARKVR01";
const MAX_CONTEXT: usize = 2 * 1024 * 1024;
const MAX_PATH: usize = 64 * 1024;
const MAX_NAME: usize = 4096;

/// Vault source screening, not content authentication. Unix `revision` is
/// ctime, which also changes for permissions/extended attributes without a
/// content write. Keep recording it in v1 contexts, but do not reject a file on
/// that field alone. Canonical paths are checked separately; full-source and
/// per-part digests remain mandatory before any persisted nonce is reused.
pub(crate) fn upload_source_metadata_matches(
    saved: SourceIdentity,
    current: SourceIdentity,
) -> bool {
    saved.filesystem_id == current.filesystem_id
        && saved.size_bytes == current.size_bytes
        && saved.modified_at_units == current.modified_at_units
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryContextError {
    Invalid,
    UnsupportedVersion,
    ForeignPathPlatform,
    Corrupt,
    Authentication,
}

/// The immutable identity admitted before remote work. Owner checks compare all
/// three scope fields against the database row before unwrapping any file key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultRecoveryContext {
    pub account_id: i64,
    pub task_id: u64,
    pub chat_id: i64,
    pub package_id: [u8; 16],
    pub vault_id: [u8; 16],
    pub master_key_generation: u32,
    pub file_key_wrap: FileKeyWrap,
    pub file_name: String,
    pub created_at_unix_ms: u64,
    pub size_bytes: u64,
    pub direction: VaultRecoveryDirection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VaultRecoveryDirection {
    Upload {
        source: PathBuf,
        identity: SourceIdentity,
        /// Full source hash established before admission. Metadata equality alone
        /// must never authorize re-encryption using a persisted part identity.
        source_blake3: [u8; 32],
    },
    Download {
        destination: PathBuf,
        manifest_message_id: i64,
        /// Binds reload to the exact authenticated envelope used at admission.
        manifest_blake3: [u8; 32],
        whole_plaintext_blake3: [u8; 32],
    },
}

impl VaultRecoveryContext {
    /// Bind decoded bytes to the independently stored immutable task scope.
    pub fn from_record(
        record: &teleark_storage::VaultJobRecord,
    ) -> Result<Self, RecoveryContextError> {
        if record.context_version != 1 {
            return Err(RecoveryContextError::UnsupportedVersion);
        }
        let context = Self::decode(&record.context)?;
        let direction = match context.direction {
            VaultRecoveryDirection::Upload { .. } => teleark_storage::VaultJobDirection::Upload,
            VaultRecoveryDirection::Download { .. } => teleark_storage::VaultJobDirection::Download,
        };
        if context.account_id != record.account_id
            || context.task_id != record.id
            || context.chat_id != record.chat_id
            || context.package_id != record.package_id
            || context.created_at_unix_ms != record.created_at_unix_ms as u64
            || direction != record.direction
        {
            return Err(RecoveryContextError::Corrupt);
        }
        Ok(context)
    }

    pub fn admission_record(
        &self,
    ) -> Result<teleark_storage::VaultJobRecord, RecoveryContextError> {
        let context = self.encode()?;
        Ok(teleark_storage::VaultJobRecord {
            account_id: self.account_id,
            id: self.task_id,
            chat_id: self.chat_id,
            package_id: self.package_id,
            context_version: 1,
            context,
            direction: match self.direction {
                VaultRecoveryDirection::Upload { .. } => teleark_storage::VaultJobDirection::Upload,
                VaultRecoveryDirection::Download { .. } => {
                    teleark_storage::VaultJobDirection::Download
                }
            },
            state: teleark_storage::VaultJobState::Queued,
            generation: 0,
            created_at_unix_ms: self.created_at_unix_ms as i64,
            updated_at_unix_ms: self.created_at_unix_ms as i64,
            failure_code: None,
        })
    }

    /// Revalidation of the admitted source before restoring its encryption key.
    /// The caller hashes the entire file and passes its canonical native path.
    pub(crate) fn verify_upload_source(
        &self,
        path: &std::path::Path,
        identity: SourceIdentity,
        digest: [u8; 32],
    ) -> Result<(), teleark_core::TransferError> {
        self.validate()
            .map_err(|_| teleark_core::TransferError::ManifestCorrupted)?;
        let VaultRecoveryDirection::Upload {
            source,
            identity: saved_identity,
            source_blake3,
        } = &self.direction
        else {
            return Err(teleark_core::TransferError::ManifestCorrupted);
        };
        if source != path
            || !upload_source_metadata_matches(*saved_identity, identity)
            || self.size_bytes != identity.size_bytes
            || *source_blake3 != digest
        {
            return Err(teleark_core::TransferError::SourceChanged);
        }
        Ok(())
    }

    pub fn file_key(
        &self,
        master: &teleark_crypto::VaultMasterKey,
    ) -> Result<teleark_crypto::FileKey, RecoveryContextError> {
        self.validate()?;
        teleark_crypto::unwrap_file_key(
            master,
            &self.file_key_wrap,
            &self.vault_id,
            &self.package_id,
            self.master_key_generation,
        )
        .map_err(|_| RecoveryContextError::Authentication)
    }
    pub fn encode(&self) -> Result<Vec<u8>, RecoveryContextError> {
        self.validate()?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.push(match self.direction {
            VaultRecoveryDirection::Upload { .. } => 1,
            VaultRecoveryDirection::Download { .. } => 2,
        });
        bytes.extend_from_slice(&self.account_id.to_le_bytes());
        bytes.extend_from_slice(&self.task_id.to_le_bytes());
        bytes.extend_from_slice(&self.chat_id.to_le_bytes());
        bytes.extend_from_slice(&self.package_id);
        bytes.extend_from_slice(&self.vault_id);
        bytes.extend_from_slice(&self.master_key_generation.to_le_bytes());
        bytes.extend_from_slice(&self.file_key_wrap.algorithm_id.to_le_bytes());
        bytes.extend_from_slice(&self.file_key_wrap.wrap_generation.to_le_bytes());
        bytes.extend_from_slice(&self.file_key_wrap.ciphertext);
        bytes.extend_from_slice(&self.file_key_wrap.tag);
        bytes.extend_from_slice(&self.created_at_unix_ms.to_le_bytes());
        bytes.extend_from_slice(&self.size_bytes.to_le_bytes());
        append(&mut bytes, self.file_name.as_bytes(), MAX_NAME)?;
        match &self.direction {
            VaultRecoveryDirection::Upload {
                source,
                identity,
                source_blake3,
            } => {
                encode_path(&mut bytes, source)?;
                bytes.extend_from_slice(&identity.filesystem_id.to_le_bytes());
                bytes.extend_from_slice(&identity.size_bytes.to_le_bytes());
                bytes.extend_from_slice(&identity.modified_at_units.to_le_bytes());
                bytes.extend_from_slice(&identity.revision.to_le_bytes());
                bytes.extend_from_slice(source_blake3);
            }
            VaultRecoveryDirection::Download {
                destination,
                manifest_message_id,
                manifest_blake3,
                whole_plaintext_blake3,
            } => {
                encode_path(&mut bytes, destination)?;
                bytes.extend_from_slice(&manifest_message_id.to_le_bytes());
                bytes.extend_from_slice(manifest_blake3);
                bytes.extend_from_slice(whole_plaintext_blake3);
            }
        }
        let digest = blake3::hash(&bytes);
        bytes.extend_from_slice(digest.as_bytes());
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RecoveryContextError> {
        if bytes.len() < 12 || bytes.len() > MAX_CONTEXT || &bytes[..8] != MAGIC {
            return Err(RecoveryContextError::Invalid);
        }
        let mut input = Reader(bytes);
        input.array::<8>()?;
        if u32::from_le_bytes(input.array()?) != 1 {
            return Err(RecoveryContextError::UnsupportedVersion);
        }
        if bytes.len() < 44 {
            return Err(RecoveryContextError::Invalid);
        }
        let (body, checksum) = bytes.split_at(bytes.len() - 32);
        if blake3::hash(body).as_bytes().as_slice() != checksum {
            return Err(RecoveryContextError::Corrupt);
        }
        input = Reader(&body[12..]);
        let direction = input.array::<1>()?[0];
        let account_id = i64::from_le_bytes(input.array()?);
        let task_id = u64::from_le_bytes(input.array()?);
        let chat_id = i64::from_le_bytes(input.array()?);
        let package_id = input.array()?;
        let vault_id = input.array()?;
        let master_key_generation = u32::from_le_bytes(input.array()?);
        let file_key_wrap = FileKeyWrap {
            algorithm_id: u16::from_le_bytes(input.array()?),
            wrap_generation: u32::from_le_bytes(input.array()?),
            ciphertext: input.array()?,
            tag: input.array()?,
        };
        let created_at_unix_ms = u64::from_le_bytes(input.array()?);
        let size_bytes = u64::from_le_bytes(input.array()?);
        let file_name = String::from_utf8(input.bytes(MAX_NAME)?.to_vec())
            .map_err(|_| RecoveryContextError::Invalid)?;
        let path = decode_path(&mut input)?;
        let direction = match direction {
            1 => VaultRecoveryDirection::Upload {
                source: path,
                identity: SourceIdentity {
                    filesystem_id: u128::from_le_bytes(input.array()?),
                    size_bytes: u64::from_le_bytes(input.array()?),
                    modified_at_units: u64::from_le_bytes(input.array()?),
                    revision: u64::from_le_bytes(input.array()?),
                },
                source_blake3: input.array()?,
            },
            2 => VaultRecoveryDirection::Download {
                destination: path,
                manifest_message_id: i64::from_le_bytes(input.array()?),
                manifest_blake3: input.array()?,
                whole_plaintext_blake3: input.array()?,
            },
            _ => return Err(RecoveryContextError::Invalid),
        };
        if !input.0.is_empty() {
            return Err(RecoveryContextError::Invalid);
        }
        let result = Self {
            account_id,
            task_id,
            chat_id,
            package_id,
            vault_id,
            master_key_generation,
            file_key_wrap,
            file_name,
            created_at_unix_ms,
            size_bytes,
            direction,
        };
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> Result<(), RecoveryContextError> {
        if self.account_id <= 0
            || self.chat_id <= 0
            || self.task_id == 0
            || self.task_id > i64::MAX as u64
            || self.package_id == [0; 16]
            || self.vault_id == [0; 16]
            || self.master_key_generation == 0
            || self.file_key_wrap.algorithm_id != CRYPTO_SUITE_ID
            || self.file_key_wrap.wrap_generation == 0
            || self.file_name.is_empty()
            || self.file_name.len() > MAX_NAME
            || self.file_name.contains('\0')
            || self.created_at_unix_ms > i64::MAX as u64
            || self.size_bytes == 0
            || self.size_bytes > i64::MAX as u64
        {
            return Err(RecoveryContextError::Invalid);
        }
        match &self.direction {
            VaultRecoveryDirection::Upload {
                source, identity, ..
            } if source.is_absolute() && identity.size_bytes == self.size_bytes => Ok(()),
            VaultRecoveryDirection::Download {
                destination,
                manifest_message_id,
                ..
            } if destination.is_absolute() && *manifest_message_id > 0 => Ok(()),
            _ => Err(RecoveryContextError::Invalid),
        }
    }
}
fn append(out: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Result<(), RecoveryContextError> {
    if bytes.is_empty() || bytes.len() > limit {
        return Err(RecoveryContextError::Invalid);
    }
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn array<const N: usize>(&mut self) -> Result<[u8; N], RecoveryContextError> {
        let value = self.0.get(..N).ok_or(RecoveryContextError::Invalid)?;
        let result = value
            .try_into()
            .map_err(|_| RecoveryContextError::Invalid)?;
        self.0 = &self.0[N..];
        Ok(result)
    }
    fn bytes(&mut self, max: usize) -> Result<&'a [u8], RecoveryContextError> {
        let n = u32::from_le_bytes(self.array()?) as usize;
        if n == 0 || n > max {
            return Err(RecoveryContextError::Invalid);
        }
        let value = self.0.get(..n).ok_or(RecoveryContextError::Invalid)?;
        self.0 = &self.0[n..];
        Ok(value)
    }
}
#[cfg(unix)]
fn encode_path(out: &mut Vec<u8>, path: &std::path::Path) -> Result<(), RecoveryContextError> {
    use std::os::unix::ffi::OsStrExt as _;
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(RecoveryContextError::Invalid);
    }
    out.push(1);
    append(out, bytes, MAX_PATH)
}
#[cfg(windows)]
fn encode_path(out: &mut Vec<u8>, path: &std::path::Path) -> Result<(), RecoveryContextError> {
    use std::os::windows::ffi::OsStrExt as _;
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(RecoveryContextError::Invalid);
    }
    let bytes = units
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    out.push(2);
    append(out, &bytes, MAX_PATH)
}
#[cfg(unix)]
fn decode_path(input: &mut Reader<'_>) -> Result<PathBuf, RecoveryContextError> {
    use std::os::unix::ffi::OsStringExt as _;
    if input.array::<1>()?[0] != 1 {
        return Err(RecoveryContextError::ForeignPathPlatform);
    }
    let bytes = input.bytes(MAX_PATH)?;
    if bytes.contains(&0) {
        return Err(RecoveryContextError::Invalid);
    }
    Ok(std::ffi::OsString::from_vec(bytes.to_vec()).into())
}
#[cfg(windows)]
fn decode_path(input: &mut Reader<'_>) -> Result<PathBuf, RecoveryContextError> {
    use std::os::windows::ffi::OsStringExt as _;
    if input.array::<1>()?[0] != 2 {
        return Err(RecoveryContextError::ForeignPathPlatform);
    }
    let bytes = input.bytes(MAX_PATH)?;
    if bytes.len() % 2 != 0 {
        return Err(RecoveryContextError::Invalid);
    }
    let units = bytes
        .chunks_exact(2)
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
        .collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(RecoveryContextError::Invalid);
    }
    Ok(std::ffi::OsString::from_wide(&units).into())
}

#[cfg(test)]
mod tests;

/// One immutable reservation persisted before ciphertext generation. The digest
/// prevents changed source bytes from being encrypted under an old part nonce.
/// Publication uses the same random id after restart and ambiguous responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultPartRecovery {
    pub header: teleark_crypto::PartHeader,
    pub plaintext_blake3: [u8; 32],
    pub publication_random_id: i64,
}
impl VaultPartRecovery {
    pub fn encode(&self) -> Result<Vec<u8>, RecoveryContextError> {
        self.header
            .validate(teleark_crypto::PartLimits::default())
            .map_err(|_| RecoveryContextError::Invalid)?;
        if self.publication_random_id == 0 {
            return Err(RecoveryContextError::Invalid);
        }
        let mut out = Vec::with_capacity(180);
        out.extend_from_slice(b"TARKVP01");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&self.header.encode());
        out.extend_from_slice(&self.plaintext_blake3);
        out.extend_from_slice(&self.publication_random_id.to_le_bytes());
        let hash = blake3::hash(&out);
        out.extend_from_slice(hash.as_bytes());
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, RecoveryContextError> {
        if bytes.len() < 12 || &bytes[..8] != b"TARKVP01" {
            return Err(RecoveryContextError::Invalid);
        }
        let mut reader = Reader(&bytes[8..]);
        if u32::from_le_bytes(reader.array()?) != 1 {
            return Err(RecoveryContextError::UnsupportedVersion);
        }
        if bytes.len() != 180 {
            return Err(RecoveryContextError::Invalid);
        }
        if blake3::hash(&bytes[..148]).as_bytes().as_slice() != &bytes[148..] {
            return Err(RecoveryContextError::Corrupt);
        }
        let header = teleark_crypto::PartHeader::decode(
            &reader.array()?,
            teleark_crypto::PartLimits::default(),
        )
        .map_err(|_| RecoveryContextError::Invalid)?;
        let plaintext_blake3 = reader.array()?;
        let publication_random_id = i64::from_le_bytes(reader.array()?);
        if publication_random_id == 0 {
            return Err(RecoveryContextError::Invalid);
        }
        Ok(Self {
            header,
            plaintext_blake3,
            publication_random_id,
        })
    }
}
