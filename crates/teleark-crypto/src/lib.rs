//! Versioned cryptographic containers and recovery manifests.
//!
//! The byte formats implemented here are explicit codecs; no Rust or Serde
//! layout is a persistence contract. Incompatible changes require a new format
//! version and a verified migration path; existing bytes retain their meaning.

#![forbid(unsafe_code)]

mod aead;
mod cbor;
mod error;
mod kdf;
mod manifest;
mod part;
mod random;
mod secret;
mod usage;
mod wrap;

pub use error::{CryptoError, FormatKind, LayoutViolation};
pub use manifest::{
    ExtensionField, LogicalTimestamps, ManifestLimits, ManifestMetadata, ManifestPart,
    ManifestPublicHeader, MediaKind, OpenedManifest, RemoteLocator, SourceMetadataField,
    manifest_vault_id_hint, open_manifest, remote_manifest_name, remote_part_name, seal_manifest,
};
pub use part::{
    EncryptedPartSummary, PartHeader, PartInstanceId, PartInstanceRegistry, PartLimits,
    PlaintextPartSummary, decrypt_part, encrypt_part, frame_nonce,
};
#[cfg(any(test, feature = "test-support"))]
pub use random::DeterministicRandom;
pub use random::{OsRandom, RandomSource};
pub use secret::{FileKey, Password, RecoveryKey, VaultMasterKey};
pub use usage::AeadUsageRegistry;
pub use wrap::{
    Argon2Parameters, FileKeyWrap, PasswordWrap, RecoveryWrap, generate_file_key,
    generate_recovery_key, generate_vault_master_key, unwrap_file_key,
    unwrap_master_key_with_password, unwrap_master_key_with_recovery, wrap_file_key,
    wrap_master_key_with_password, wrap_master_key_with_password_parameters,
    wrap_master_key_with_recovery,
};

/// Format major version; incompatible changes require a new version and migration.
pub const FORMAT_MAJOR: u16 = 1;
/// Candidate format minor version.
pub const FORMAT_MINOR: u16 = 0;
/// AES-256-GCM/HKDF-SHA-256/BLAKE3 suite identifier.
pub const CRYPTO_SUITE_ID: u16 = 1;
/// Part-index plus frame-index nonce strategy identifier.
pub const NONCE_STRATEGY_ID: u16 = 1;
