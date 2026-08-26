use std::collections::BTreeSet;
use std::fmt;

use crate::kdf::{manifest_key, package_wrap_key, part_content_key, recovery_kek};
use crate::wrap::{PasswordWrap, derive_password_kek};
use crate::{CryptoError, FileKey, PartInstanceId, Password, RecoveryKey, VaultMasterKey};

const REGISTRY_FINGERPRINT_DOMAIN: &str = "teleark/aead-usage-registry/v1";

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum RecordIdentity {
    PasswordWrap {
        vault_id: [u8; 16],
        generation: u32,
    },
    RecoveryWrap {
        vault_id: [u8; 16],
        generation: u32,
    },
    FileKeyWrap {
        vault_id: [u8; 16],
        package_id: [u8; 16],
        master_key_generation: u32,
        wrap_generation: u32,
    },
    Manifest {
        vault_id: [u8; 16],
        package_id: [u8; 16],
        generation: u32,
    },
    Part {
        package_id: [u8; 16],
        part_index: u32,
        instance_id: PartInstanceId,
    },
}

/// Tracks both durable record identities and derived AES encryption-key
/// identities already consumed by a writer. Each key is reserved for exactly
/// one immutable record or container, which is stricter than tracking only the
/// individual nonces used under it.
///
/// Keep one registry for the active Vault service and hydrate it from existing
/// wrap/manifest/part records before allowing new encryption after restart.
/// Reservation is intentionally not rolled back after a later failure: the
/// identity must be abandoned rather than reused with possibly changed input.
pub struct AeadUsageRegistry {
    records: BTreeSet<RecordIdentity>,
    key_fingerprints: BTreeSet<[u8; 32]>,
}

impl AeadUsageRegistry {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            records: BTreeSet::new(),
            key_fingerprints: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty() && self.key_fingerprints.is_empty()
    }

    /// Reserve an already-published password-wrap identity while rebuilding
    /// the registry after restart.
    pub fn reserve_existing_password_wrap(
        &mut self,
        record: &PasswordWrap,
        password: &Password,
    ) -> Result<(), CryptoError> {
        let key = derive_password_kek(password, &record.salt, record.parameters)?;
        self.reserve_password_wrap(key.as_ref(), record.vault_id, record.wrap_generation)
    }

    /// Reserve an already-published recovery-wrap identity while rebuilding
    /// the registry after restart.
    pub fn reserve_existing_recovery_wrap(
        &mut self,
        recovery_key: &RecoveryKey,
        vault_id: &[u8; 16],
        recovery_generation: u32,
    ) -> Result<(), CryptoError> {
        let key = recovery_kek(recovery_key.as_bytes(), vault_id)?;
        self.reserve_recovery_wrap(key.as_ref(), *vault_id, recovery_generation)
    }

    /// Reserve an existing package File Key-wrap generation.
    pub fn reserve_existing_file_key_wrap(
        &mut self,
        master_key: &VaultMasterKey,
        vault_id: &[u8; 16],
        package_id: &[u8; 16],
        master_key_generation: u32,
        wrap_generation: u32,
    ) -> Result<(), CryptoError> {
        let key = package_wrap_key(
            master_key.as_bytes(),
            package_id,
            master_key_generation,
            wrap_generation,
        )?;
        self.reserve_file_key_wrap(
            key.as_ref(),
            *vault_id,
            *package_id,
            master_key_generation,
            wrap_generation,
        )
    }

    /// Reserve an existing immutable manifest generation.
    pub fn reserve_existing_manifest(
        &mut self,
        file_key: &FileKey,
        vault_id: &[u8; 16],
        package_id: &[u8; 16],
        manifest_generation: u32,
    ) -> Result<(), CryptoError> {
        let key = manifest_key(file_key.as_bytes(), package_id, manifest_generation)?;
        self.reserve_manifest(key.as_ref(), *vault_id, *package_id, manifest_generation)
    }

    /// Reserve an existing encoded content-part identity.
    pub fn reserve_existing_part(
        &mut self,
        file_key: &FileKey,
        package_id: &[u8; 16],
        part_index: u32,
        instance_id: PartInstanceId,
    ) -> Result<(), CryptoError> {
        let key = part_content_key(file_key.as_bytes(), package_id, part_index, &instance_id.0)?;
        self.reserve_part(key.as_ref(), *package_id, part_index, instance_id)
    }

    pub(crate) fn reserve_password_wrap(
        &mut self,
        key: &[u8],
        vault_id: [u8; 16],
        generation: u32,
    ) -> Result<(), CryptoError> {
        self.reserve(
            RecordIdentity::PasswordWrap {
                vault_id,
                generation,
            },
            key,
        )
    }

    pub(crate) fn reserve_recovery_wrap(
        &mut self,
        key: &[u8],
        vault_id: [u8; 16],
        generation: u32,
    ) -> Result<(), CryptoError> {
        self.reserve(
            RecordIdentity::RecoveryWrap {
                vault_id,
                generation,
            },
            key,
        )
    }

    pub(crate) fn reserve_file_key_wrap(
        &mut self,
        key: &[u8],
        vault_id: [u8; 16],
        package_id: [u8; 16],
        master_key_generation: u32,
        wrap_generation: u32,
    ) -> Result<(), CryptoError> {
        self.reserve(
            RecordIdentity::FileKeyWrap {
                vault_id,
                package_id,
                master_key_generation,
                wrap_generation,
            },
            key,
        )
    }

    pub(crate) fn reserve_manifest(
        &mut self,
        key: &[u8],
        vault_id: [u8; 16],
        package_id: [u8; 16],
        generation: u32,
    ) -> Result<(), CryptoError> {
        self.reserve(
            RecordIdentity::Manifest {
                vault_id,
                package_id,
                generation,
            },
            key,
        )
    }

    pub(crate) fn reserve_part(
        &mut self,
        key: &[u8],
        package_id: [u8; 16],
        part_index: u32,
        instance_id: PartInstanceId,
    ) -> Result<(), CryptoError> {
        self.reserve(
            RecordIdentity::Part {
                package_id,
                part_index,
                instance_id,
            },
            key,
        )
    }

    fn reserve(&mut self, record: RecordIdentity, key: &[u8]) -> Result<(), CryptoError> {
        let mut hasher = blake3::Hasher::new_derive_key(REGISTRY_FINGERPRINT_DOMAIN);
        hasher.update(&(key.len() as u64).to_be_bytes());
        hasher.update(key);
        let fingerprint = *hasher.finalize().as_bytes();
        if self.records.contains(&record) || self.key_fingerprints.contains(&fingerprint) {
            return Err(CryptoError::AeadIdentityAlreadyUsed);
        }
        self.records.insert(record);
        self.key_fingerprints.insert(fingerprint);
        Ok(())
    }
}

impl Default for AeadUsageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AeadUsageRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AeadUsageRegistry")
            .field("reserved_record_count", &self.records.len())
            .field(
                "reserved_key_fingerprint_count",
                &self.key_fingerprints.len(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_derived_identity_is_rejected_without_debug_fingerprint() {
        let mut registry = AeadUsageRegistry::new();
        let recovery = RecoveryKey::from_bytes([7; 32]);
        assert!(
            registry
                .reserve_existing_recovery_wrap(&recovery, &[8; 16], 1)
                .is_ok()
        );
        assert_eq!(
            registry.reserve_existing_recovery_wrap(&recovery, &[8; 16], 2),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
        assert_eq!(
            format!("{registry:?}"),
            "AeadUsageRegistry { reserved_record_count: 1, reserved_key_fingerprint_count: 1 }"
        );
    }
}
