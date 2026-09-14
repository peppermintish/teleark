use std::collections::BTreeSet;
use std::fmt;

use zeroize::{Zeroize, Zeroizing};

use crate::aead::{ZERO_NONCE, decrypt_detached, encrypt_detached};
use crate::cbor::{Decoder, Encoder};
use crate::kdf::manifest_key;
use crate::part::PlaintextPartSummary;
use crate::wrap::{FileKeyWrap, unwrap_file_key};
use crate::{
    AeadUsageRegistry, CRYPTO_SUITE_ID, CryptoError, FORMAT_MAJOR, FORMAT_MINOR, FileKey,
    FormatKind, LayoutViolation, NONCE_STRATEGY_ID, VaultMasterKey,
};

const MANIFEST_MAGIC: &[u8; 8] = b"TARKMAN\0";
const ENVELOPE_PREFIX_LENGTH: usize = 24;
const TAG_LENGTH: usize = 16;
const FILE_KEY_WRAP_ALGORITHM: u16 = 1;
const PART_CONTAINER_HEADER_LENGTH: u64 = 96;
const FRAME_RECORD_OVERHEAD: u64 = 32;
const MIN_PART_DESCRIPTOR_BYTES: usize = 112;

/// Parser and allocation policy for an untrusted recovery manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManifestLimits {
    pub max_public_header_bytes: usize,
    pub max_encrypted_metadata_bytes: usize,
    pub max_parts: u32,
    pub max_name_bytes: usize,
    pub max_relative_path_bytes: usize,
    pub max_mime_type_bytes: usize,
    pub max_remote_name_bytes: usize,
    pub max_locator_extension_bytes: usize,
    pub max_metadata_fields: usize,
    pub max_metadata_value_bytes: usize,
}

impl Default for ManifestLimits {
    fn default() -> Self {
        Self {
            max_public_header_bytes: 64 * 1024,
            max_encrypted_metadata_bytes: 64 * 1024 * 1024,
            max_parts: 1_000_000,
            max_name_bytes: 4 * 1024,
            max_relative_path_bytes: 32 * 1024,
            max_mime_type_bytes: 255,
            max_remote_name_bytes: 255,
            max_locator_extension_bytes: 16 * 1024,
            max_metadata_fields: 1024,
            max_metadata_value_bytes: 1024 * 1024,
        }
    }
}

/// Public authenticated manifest fields needed for key resolution and bounded
/// parsing. Names, locators, and content hashes are not present here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestPublicHeader {
    pub package_id: [u8; 16],
    pub vault_id: [u8; 16],
    pub manifest_generation: u32,
    pub created_at_unix_ms: u64,
    pub logical_file_size: u64,
    pub part_count: u32,
    pub application_part_target: u64,
    pub frame_plaintext_max: u32,
    pub nonce_strategy_id: u16,
    pub crypto_suite_id: u16,
    pub file_key_wrap: FileKeyWrap,
    pub master_key_generation: u32,
    pub flags: u32,
}

/// Stable locale-neutral media classification used by the candidate codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum MediaKind {
    Other = 0,
    Video = 1,
    Document = 2,
    Archive = 3,
    Audio = 4,
    Image = 5,
    DiskImage = 6,
}

impl MediaKind {
    fn from_u16(value: u16) -> Result<Self, CryptoError> {
        match value {
            0 => Ok(Self::Other),
            1 => Ok(Self::Video),
            2 => Ok(Self::Document),
            3 => Ok(Self::Archive),
            4 => Ok(Self::Audio),
            5 => Ok(Self::Image),
            6 => Ok(Self::DiskImage),
            _ => Err(CryptoError::InvalidField {
                field: "media kind",
            }),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RemoteLocator {
    pub account_id: i64,
    pub chat_id: i64,
    pub message_id: i64,
    pub remote_name: String,
    pub locator_version: u16,
    pub locator_extension: Option<Vec<u8>>,
}

impl fmt::Debug for RemoteLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteLocator")
            .field("account_id", &"[REDACTED]")
            .field("chat_id", &"[REDACTED]")
            .field("message_id", &"[REDACTED]")
            .field("remote_name", &"[REDACTED]")
            .field("locator_version", &self.locator_version)
            .field(
                "locator_extension_bytes",
                &self.locator_extension.as_ref().map_or(0, Vec::len),
            )
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ManifestPart {
    pub part_index: u32,
    pub part_instance_id: [u8; 16],
    pub plaintext_offset: u64,
    pub plaintext_length: u64,
    pub encoded_length: u64,
    pub frame_count: u32,
    pub plaintext_blake3: [u8; 32],
    pub encoded_ciphertext_blake3: [u8; 32],
    pub remote_locator: RemoteLocator,
}

impl fmt::Debug for ManifestPart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManifestPart")
            .field("part_index", &self.part_index)
            .field("part_instance_id", &"[REDACTED]")
            .field("plaintext_offset", &self.plaintext_offset)
            .field("plaintext_length", &self.plaintext_length)
            .field("encoded_length", &self.encoded_length)
            .field("frame_count", &self.frame_count)
            .field("plaintext_blake3", &"[REDACTED]")
            .field("encoded_ciphertext_blake3", &"[REDACTED]")
            .field("remote_locator", &self.remote_locator)
            .finish()
    }
}

impl ManifestPart {
    /// Bind a successfully authenticated part container to its authenticated
    /// manifest descriptor before a partial output is considered complete.
    pub fn verify_decrypted_part(
        &self,
        public: &ManifestPublicHeader,
        summary: &PlaintextPartSummary,
    ) -> Result<(), CryptoError> {
        let header = &summary.header;
        if header.format_major != if public.flags == 1 { 2 } else { 1 }
            || header.package_id != public.package_id
            || header.part_instance_id.0 != self.part_instance_id
            || header.part_index != self.part_index
            || header.part_count != public.part_count
            || header.logical_file_size != public.logical_file_size
            || header.plaintext_offset != self.plaintext_offset
            || header.plaintext_length != self.plaintext_length
            || header.frame_plaintext_max != public.frame_plaintext_max
            || header.frame_count != self.frame_count
            || summary.encoded_length != self.encoded_length
            || summary.plaintext_blake3 != self.plaintext_blake3
            || summary.encoded_blake3 != self.encoded_ciphertext_blake3
        {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InconsistentField,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalTimestamps {
    pub created_at_unix_ms: Option<i64>,
    pub modified_at_unix_ms: Option<i64>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct SourceMetadataField {
    pub key: u64,
    pub value: Vec<u8>,
}

impl fmt::Debug for SourceMetadataField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceMetadataField")
            .field("key", &self.key)
            .field("value_bytes", &self.value.len())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ExtensionField {
    pub key: u64,
    pub critical: bool,
    pub value: Vec<u8>,
}

impl fmt::Debug for ExtensionField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionField")
            .field("key", &self.key)
            .field("critical", &self.critical)
            .field("value_bytes", &self.value.len())
            .finish()
    }
}

/// Authenticated encrypted manifest metadata.
#[derive(Clone, Eq, PartialEq)]
pub struct ManifestMetadata {
    pub logical_name: String,
    pub relative_path: Option<String>,
    pub mime_type: Option<String>,
    pub media_kind: MediaKind,
    pub whole_plaintext_blake3: [u8; 32],
    pub parts: Vec<ManifestPart>,
    pub logical_timestamps: Option<LogicalTimestamps>,
    pub source_metadata: Vec<SourceMetadataField>,
    pub format_extensions: Vec<ExtensionField>,
}

impl fmt::Debug for ManifestMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plaintext_bytes: u128 = self
            .parts
            .iter()
            .map(|part| u128::from(part.plaintext_length))
            .sum();
        let encoded_bytes: u128 = self
            .parts
            .iter()
            .map(|part| u128::from(part.encoded_length))
            .sum();
        formatter
            .debug_struct("ManifestMetadata")
            .field("logical_name", &"[REDACTED]")
            .field("has_relative_path", &self.relative_path.is_some())
            .field("has_mime_type", &self.mime_type.is_some())
            .field("media_kind", &self.media_kind)
            .field("part_count", &self.parts.len())
            .field("part_plaintext_bytes", &plaintext_bytes)
            .field("part_encoded_bytes", &encoded_bytes)
            .field("has_logical_timestamps", &self.logical_timestamps.is_some())
            .field("source_metadata_field_count", &self.source_metadata.len())
            .field("format_extension_count", &self.format_extensions.len())
            .finish()
    }
}

impl Drop for ManifestMetadata {
    fn drop(&mut self) {
        self.logical_name.zeroize();
        self.relative_path.zeroize();
        self.mime_type.zeroize();
        for part in &mut self.parts {
            part.remote_locator.remote_name.zeroize();
            part.remote_locator.locator_extension.zeroize();
        }
        for field in &mut self.source_metadata {
            field.value.zeroize();
        }
        for field in &mut self.format_extensions {
            field.value.zeroize();
        }
    }
}

/// Successfully authenticated manifest plus its in-memory File Key.
pub struct OpenedManifest {
    pub public_header: ManifestPublicHeader,
    pub metadata: ManifestMetadata,
    file_key: FileKey,
}

impl std::fmt::Debug for OpenedManifest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedManifest")
            .field("public_header", &self.public_header)
            .field("metadata", &self.metadata)
            .field("file_key", &self.file_key)
            .finish()
    }
}

impl OpenedManifest {
    /// Borrow the recovered File Key for authenticated part decoding. The key's
    /// raw bytes are never exposed by the public API.
    #[must_use]
    pub const fn file_key(&self) -> &FileKey {
        &self.file_key
    }

    /// Consume an authenticated manifest and transfer ownership of its File
    /// Key to the recovery runtime without exposing the key bytes.
    #[must_use]
    pub fn into_parts(self) -> (ManifestPublicHeader, ManifestMetadata, FileKey) {
        (self.public_header, self.metadata, self.file_key)
    }
}

/// Opaque manifest remote name derived only from package identity and version.
#[must_use]
pub fn remote_manifest_name(package_id: &[u8; 16]) -> String {
    let mut value = hex_id(package_id);
    value.push_str(".v1.manifest.tam");
    value
}

/// Opaque part remote name derived only from package identity, version, and
/// zero-based part index.
#[must_use]
pub fn remote_part_name(package_id: &[u8; 16], part_index: u32) -> String {
    let mut value = hex_id(package_id);
    value.push_str(".v1.");
    value.push_str(&format!("{part_index:06}"));
    value.push_str(".part.tav");
    value
}

fn hex_id(bytes: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(32);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

/// BLAKE3 commitment to the exact version-1 manifest input before AEAD use.
/// Persist this commitment before sealing an immutable generation. It is not
/// an authentication result; remote envelopes still require open_manifest.
pub fn manifest_content_commitment(
    public_header: &ManifestPublicHeader,
    metadata: &ManifestMetadata,
    limits: ManifestLimits,
) -> Result<[u8; 32], CryptoError> {
    let (public_bytes, plaintext) = manifest_payload(public_header, metadata, limits)?;
    let prefix = encode_envelope_prefix(public_bytes.len(), plaintext.len(), public_header.flags)?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"TARK manifest commitment v1\0");
    hash.update(&prefix);
    hash.update(&public_bytes);
    hash.update(&plaintext);
    Ok(*hash.finalize().as_bytes())
}

fn manifest_payload(
    public_header: &ManifestPublicHeader,
    metadata: &ManifestMetadata,
    limits: ManifestLimits,
) -> Result<(Vec<u8>, Zeroizing<Vec<u8>>), CryptoError> {
    public_header.validate(limits)?;
    metadata.validate(public_header, limits)?;
    let public_bytes = encode_public_header(public_header)?;
    if public_bytes.len() > limits.max_public_header_bytes {
        return Err(CryptoError::LimitExceeded {
            field: "manifest public header",
            limit: limits.max_public_header_bytes as u64,
            actual: public_bytes.len() as u64,
        });
    }
    let encrypted_metadata = Zeroizing::new(encode_metadata(metadata)?);
    if encrypted_metadata.len() > limits.max_encrypted_metadata_bytes {
        return Err(CryptoError::LimitExceeded {
            field: "manifest encrypted metadata",
            limit: limits.max_encrypted_metadata_bytes as u64,
            actual: encrypted_metadata.len() as u64,
        });
    }
    Ok((public_bytes, encrypted_metadata))
}

/// Encrypt one canonical metadata payload for an immutable manifest generation.
pub fn seal_manifest(
    public_header: &ManifestPublicHeader,
    metadata: &ManifestMetadata,
    file_key: &FileKey,
    limits: ManifestLimits,
    usage: &mut AeadUsageRegistry,
) -> Result<Vec<u8>, CryptoError> {
    let (public_bytes, mut encrypted_metadata) = manifest_payload(public_header, metadata, limits)?;
    let prefix = encode_envelope_prefix(
        public_bytes.len(),
        encrypted_metadata.len(),
        public_header.flags,
    )?;
    let mut aad = Vec::with_capacity(prefix.len() + public_bytes.len());
    aad.extend_from_slice(&prefix);
    aad.extend_from_slice(&public_bytes);
    let key = manifest_key(
        file_key.as_bytes(),
        &public_header.package_id,
        public_header.manifest_generation,
    )?;
    usage.reserve_manifest(
        key.as_ref(),
        public_header.vault_id,
        public_header.package_id,
        public_header.manifest_generation,
    )?;
    let tag = encrypt_detached(key.as_ref(), &ZERO_NONCE, &aad, encrypted_metadata.as_mut())?;

    let capacity = aad
        .len()
        .checked_add(encrypted_metadata.len())
        .and_then(|value| value.checked_add(TAG_LENGTH))
        .ok_or(CryptoError::ArithmeticOverflow {
            field: "manifest envelope length",
        })?;
    let mut output = Vec::with_capacity(capacity);
    output.extend_from_slice(&prefix);
    output.extend_from_slice(&public_bytes);
    output.extend_from_slice(&encrypted_metadata);
    output.extend_from_slice(&tag);
    Ok(output)
}

/// Parse a bounded, unauthenticated key-routing hint. Callers must authenticate
/// the complete manifest before trusting its identity, metadata or locators.
/// This never returns decrypted metadata and must not authorize remote writes.
pub fn manifest_vault_id_hint(
    bytes: &[u8],
    limits: ManifestLimits,
) -> Result<[u8; 16], CryptoError> {
    let envelope = parse_envelope(bytes, limits)?;
    let header = decode_public_header(envelope.public_header, limits)?;
    if u16::from_be_bytes([bytes[8], bytes[9]]) != if header.flags == 1 { 2 } else { 1 } {
        return Err(CryptoError::InvalidField {
            field: "manifest codec flags",
        });
    }
    header.validate(limits)?;
    Ok(header.vault_id)
}

/// Strictly parse, resolve the File Key, authenticate, and validate a recovery
/// manifest. No encrypted names or locators are returned before authentication.
pub fn open_manifest(
    bytes: &[u8],
    master_key: &VaultMasterKey,
    limits: ManifestLimits,
) -> Result<OpenedManifest, CryptoError> {
    let envelope = parse_envelope(bytes, limits)?;
    let public_header = decode_public_header(envelope.public_header, limits)?;
    if u16::from_be_bytes([bytes[8], bytes[9]]) != if public_header.flags == 1 { 2 } else { 1 } {
        return Err(CryptoError::InvalidField {
            field: "manifest codec flags",
        });
    }
    public_header.validate(limits)?;
    let file_key = unwrap_file_key(
        master_key,
        &public_header.file_key_wrap,
        &public_header.vault_id,
        &public_header.package_id,
        public_header.master_key_generation,
    )?;
    let key = manifest_key(
        file_key.as_bytes(),
        &public_header.package_id,
        public_header.manifest_generation,
    )?;
    let mut metadata_bytes = Zeroizing::new(envelope.encrypted_metadata.to_vec());
    decrypt_detached(
        key.as_ref(),
        &ZERO_NONCE,
        envelope.aad,
        metadata_bytes.as_mut(),
        &envelope.tag,
    )?;
    let metadata = decode_metadata(metadata_bytes.as_slice(), limits)?;
    metadata.validate(&public_header, limits)?;
    Ok(OpenedManifest {
        public_header,
        metadata,
        file_key,
    })
}

impl ManifestPublicHeader {
    fn validate(&self, limits: ManifestLimits) -> Result<(), CryptoError> {
        if self.part_count > limits.max_parts {
            return Err(CryptoError::LimitExceeded {
                field: "manifest part count",
                limit: u64::from(limits.max_parts),
                actual: u64::from(self.part_count),
            });
        }
        if self.logical_file_size > 0 && self.part_count == 0 {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidPartCount,
            });
        }
        if self.flags == 1 && self.frame_plaintext_max != 512 * 1024 - 32 {
            return Err(CryptoError::InvalidField {
                field: "aligned manifest frame size",
            });
        }
        if self.application_part_target == 0 || self.frame_plaintext_max == 0 {
            return Err(CryptoError::InvalidField {
                field: "manifest geometry",
            });
        }
        if self.crypto_suite_id != CRYPTO_SUITE_ID {
            return Err(CryptoError::UnsupportedSuite {
                suite_id: self.crypto_suite_id,
            });
        }
        if self.nonce_strategy_id != NONCE_STRATEGY_ID {
            return Err(CryptoError::UnsupportedNonceStrategy {
                strategy_id: self.nonce_strategy_id,
            });
        }
        if self.file_key_wrap.algorithm_id != FILE_KEY_WRAP_ALGORITHM {
            return Err(CryptoError::UnsupportedAlgorithm {
                algorithm_id: self.file_key_wrap.algorithm_id,
            });
        }
        if self.flags > 1 {
            return Err(CryptoError::UnknownFlags {
                field: "manifest public header",
                flags: self.flags,
            });
        }
        Ok(())
    }
}

impl ManifestMetadata {
    pub fn validate(
        &self,
        public: &ManifestPublicHeader,
        limits: ManifestLimits,
    ) -> Result<(), CryptoError> {
        validate_string(&self.logical_name, limits.max_name_bytes, "logical name")?;
        if self.logical_name.trim().is_empty() {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::EmptyName,
            });
        }
        if let Some(path) = &self.relative_path {
            validate_string(path, limits.max_relative_path_bytes, "relative path")?;
            validate_relative_path(path)?;
        }
        if let Some(mime_type) = &self.mime_type {
            validate_string(mime_type, limits.max_mime_type_bytes, "MIME type")?;
        }
        if self.parts.len() != public.part_count as usize {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidPartCount,
            });
        }
        if public.logical_file_size == 0 && !self.parts.is_empty() {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidPartCount,
            });
        }

        let mut expected_offset = 0_u64;
        let mut instances = BTreeSet::new();
        for (ordinal, part) in self.parts.iter().enumerate() {
            let expected_index =
                u32::try_from(ordinal).map_err(|_| CryptoError::ArithmeticOverflow {
                    field: "manifest part ordinal",
                })?;
            if part.part_index != expected_index {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::InvalidPartIndex,
                });
            }
            if !instances.insert(part.part_instance_id) {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::DuplicatePartInstance,
                });
            }
            if part.plaintext_offset < expected_offset {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::Overlap,
                });
            }
            if part.plaintext_offset > expected_offset {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::Gap,
                });
            }
            if part.plaintext_length == 0 {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::OutOfRange,
                });
            }
            let is_final = ordinal + 1 == self.parts.len();
            if (!is_final && part.plaintext_length != public.application_part_target)
                || (is_final && part.plaintext_length > public.application_part_target)
            {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::InconsistentField,
                });
            }
            expected_offset = part
                .plaintext_offset
                .checked_add(part.plaintext_length)
                .ok_or(CryptoError::ArithmeticOverflow {
                    field: "manifest plaintext part range",
                })?;
            if expected_offset > public.logical_file_size {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::OutOfRange,
                });
            }
            let framed_length = part
                .plaintext_length
                .checked_add(if public.flags == 1 { 96 } else { 0 })
                .ok_or(CryptoError::ArithmeticOverflow {
                    field: "aligned manifest frame count",
                })?;
            let expected_frames = frame_count(framed_length, public.frame_plaintext_max)?;
            if expected_frames != part.frame_count {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::InvalidFrameCount,
                });
            }
            let expected_encoded = PART_CONTAINER_HEADER_LENGTH
                .checked_add(
                    u64::from(part.frame_count)
                        .checked_mul(FRAME_RECORD_OVERHEAD)
                        .ok_or(CryptoError::ArithmeticOverflow {
                            field: "manifest frame overhead",
                        })?,
                )
                .and_then(|value| value.checked_add(part.plaintext_length))
                .ok_or(CryptoError::ArithmeticOverflow {
                    field: "manifest encoded part length",
                })?;
            if part.encoded_length != expected_encoded {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::InconsistentField,
                });
            }
            let expected_name = remote_part_name(&public.package_id, part.part_index);
            if part.remote_locator.remote_name != expected_name {
                return Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::RemoteNameMismatch,
                });
            }
            validate_string(
                &part.remote_locator.remote_name,
                limits.max_remote_name_bytes,
                "remote name",
            )?;
            if let Some(extension) = &part.remote_locator.locator_extension
                && extension.len() > limits.max_locator_extension_bytes
            {
                return Err(CryptoError::LimitExceeded {
                    field: "locator extension",
                    limit: limits.max_locator_extension_bytes as u64,
                    actual: extension.len() as u64,
                });
            }
        }
        if expected_offset != public.logical_file_size {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::Gap,
            });
        }
        validate_metadata_fields(&self.source_metadata, limits)?;
        validate_extensions(&self.format_extensions, limits)?;
        Ok(())
    }
}

fn validate_string(value: &str, maximum: usize, field: &'static str) -> Result<(), CryptoError> {
    if value.len() > maximum {
        return Err(CryptoError::LimitExceeded {
            field,
            limit: maximum as u64,
            actual: value.len() as u64,
        });
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), CryptoError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
    {
        return Err(CryptoError::InvalidLayout {
            violation: LayoutViolation::UnsafeRelativePath,
        });
    }
    for (index, component) in path.split('/').enumerate() {
        if component.is_empty()
            || component == "."
            || component == ".."
            || (index == 0 && component.ends_with(':'))
        {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::UnsafeRelativePath,
            });
        }
    }
    Ok(())
}

fn validate_metadata_fields(
    fields: &[SourceMetadataField],
    limits: ManifestLimits,
) -> Result<(), CryptoError> {
    if fields.len() > limits.max_metadata_fields {
        return Err(CryptoError::LimitExceeded {
            field: "source metadata fields",
            limit: limits.max_metadata_fields as u64,
            actual: fields.len() as u64,
        });
    }
    let mut previous = None;
    for field in fields {
        if previous.is_some_and(|key| field.key <= key) {
            return Err(CryptoError::DuplicateOrUnorderedMapKey);
        }
        if field.value.len() > limits.max_metadata_value_bytes {
            return Err(CryptoError::LimitExceeded {
                field: "source metadata value",
                limit: limits.max_metadata_value_bytes as u64,
                actual: field.value.len() as u64,
            });
        }
        previous = Some(field.key);
    }
    Ok(())
}

fn validate_extensions(
    fields: &[ExtensionField],
    limits: ManifestLimits,
) -> Result<(), CryptoError> {
    if fields.len() > limits.max_metadata_fields {
        return Err(CryptoError::LimitExceeded {
            field: "format extension fields",
            limit: limits.max_metadata_fields as u64,
            actual: fields.len() as u64,
        });
    }
    let mut previous = None;
    for field in fields {
        if previous.is_some_and(|key| field.key <= key) {
            return Err(CryptoError::DuplicateOrUnorderedMapKey);
        }
        if field.critical {
            return Err(CryptoError::InvalidField {
                field: "unknown critical format extension",
            });
        }
        if field.value.len() > limits.max_metadata_value_bytes {
            return Err(CryptoError::LimitExceeded {
                field: "format extension value",
                limit: limits.max_metadata_value_bytes as u64,
                actual: field.value.len() as u64,
            });
        }
        previous = Some(field.key);
    }
    Ok(())
}

fn frame_count(length: u64, frame_max: u32) -> Result<u32, CryptoError> {
    if length == 0 || frame_max == 0 {
        return Err(CryptoError::InvalidLayout {
            violation: LayoutViolation::InvalidFrameCount,
        });
    }
    let maximum = u64::from(frame_max);
    let count = length
        .checked_add(maximum - 1)
        .ok_or(CryptoError::ArithmeticOverflow {
            field: "manifest frame count",
        })?
        / maximum;
    u32::try_from(count).map_err(|_| CryptoError::ArithmeticOverflow {
        field: "manifest frame count",
    })
}

struct Envelope<'a> {
    aad: &'a [u8],
    public_header: &'a [u8],
    encrypted_metadata: &'a [u8],
    tag: [u8; 16],
}

fn encode_envelope_prefix(
    public_length: usize,
    metadata_length: usize,
    flags: u32,
) -> Result<[u8; ENVELOPE_PREFIX_LENGTH], CryptoError> {
    let public_length =
        u32::try_from(public_length).map_err(|_| CryptoError::ArithmeticOverflow {
            field: "manifest public header length",
        })?;
    let metadata_length =
        u64::try_from(metadata_length).map_err(|_| CryptoError::ArithmeticOverflow {
            field: "manifest metadata length",
        })?;
    let mut prefix = [0_u8; ENVELOPE_PREFIX_LENGTH];
    prefix[..8].copy_from_slice(MANIFEST_MAGIC);
    prefix[8..10].copy_from_slice(&(if flags == 1 { 2u16 } else { FORMAT_MAJOR }).to_be_bytes());
    prefix[10..12].copy_from_slice(&FORMAT_MINOR.to_be_bytes());
    prefix[12..16].copy_from_slice(&public_length.to_be_bytes());
    prefix[16..24].copy_from_slice(&metadata_length.to_be_bytes());
    Ok(prefix)
}

fn parse_envelope(bytes: &[u8], limits: ManifestLimits) -> Result<Envelope<'_>, CryptoError> {
    if bytes.len() < ENVELOPE_PREFIX_LENGTH + TAG_LENGTH {
        return Err(CryptoError::Truncated {
            context: "manifest envelope",
        });
    }
    if bytes.get(..8) != Some(MANIFEST_MAGIC) {
        return Err(CryptoError::InvalidMagic {
            format: FormatKind::Manifest,
        });
    }
    let major = u16::from_be_bytes(array_at(bytes, 8, "manifest major version")?);
    let minor = u16::from_be_bytes(array_at(bytes, 10, "manifest minor version")?);
    if !matches!(major, 1 | 2) || minor != FORMAT_MINOR {
        return Err(CryptoError::UnsupportedVersion {
            format: FormatKind::Manifest,
            major,
            minor,
        });
    }
    let public_length = usize::try_from(u32::from_be_bytes(array_at(
        bytes,
        12,
        "manifest public header length",
    )?))
    .map_err(|_| CryptoError::ArithmeticOverflow {
        field: "manifest public header length",
    })?;
    let metadata_length_u64 = u64::from_be_bytes(array_at(bytes, 16, "manifest metadata length")?);
    if public_length > limits.max_public_header_bytes {
        return Err(CryptoError::LimitExceeded {
            field: "manifest public header",
            limit: limits.max_public_header_bytes as u64,
            actual: public_length as u64,
        });
    }
    if metadata_length_u64 > limits.max_encrypted_metadata_bytes as u64 {
        return Err(CryptoError::LimitExceeded {
            field: "manifest encrypted metadata",
            limit: limits.max_encrypted_metadata_bytes as u64,
            actual: metadata_length_u64,
        });
    }
    let metadata_length =
        usize::try_from(metadata_length_u64).map_err(|_| CryptoError::ArithmeticOverflow {
            field: "manifest metadata length",
        })?;
    let public_end = ENVELOPE_PREFIX_LENGTH.checked_add(public_length).ok_or(
        CryptoError::ArithmeticOverflow {
            field: "manifest public range",
        },
    )?;
    let metadata_end =
        public_end
            .checked_add(metadata_length)
            .ok_or(CryptoError::ArithmeticOverflow {
                field: "manifest metadata range",
            })?;
    let expected_total =
        metadata_end
            .checked_add(TAG_LENGTH)
            .ok_or(CryptoError::ArithmeticOverflow {
                field: "manifest envelope length",
            })?;
    if bytes.len() != expected_total {
        return if bytes.len() < expected_total {
            Err(CryptoError::Truncated {
                context: "manifest envelope",
            })
        } else {
            Err(CryptoError::TrailingData {
                context: "manifest envelope",
            })
        };
    }
    Ok(Envelope {
        aad: &bytes[..public_end],
        public_header: &bytes[ENVELOPE_PREFIX_LENGTH..public_end],
        encrypted_metadata: &bytes[public_end..metadata_end],
        tag: array_at(bytes, metadata_end, "manifest tag")?,
    })
}

fn array_at<const N: usize>(
    bytes: &[u8],
    offset: usize,
    context: &'static str,
) -> Result<[u8; N], CryptoError> {
    bytes
        .get(offset..offset.saturating_add(N))
        .and_then(|value| value.try_into().ok())
        .ok_or(CryptoError::Truncated { context })
}

fn encode_public_header(header: &ManifestPublicHeader) -> Result<Vec<u8>, CryptoError> {
    let mut encoder = Encoder::default();
    encoder.map(13)?;
    encoder.unsigned(1)?;
    encoder.byte_string(&header.package_id)?;
    encoder.unsigned(2)?;
    encoder.byte_string(&header.vault_id)?;
    encoder.unsigned(3)?;
    encoder.unsigned(u64::from(header.manifest_generation))?;
    encoder.unsigned(4)?;
    encoder.unsigned(header.created_at_unix_ms)?;
    encoder.unsigned(5)?;
    encoder.unsigned(header.logical_file_size)?;
    encoder.unsigned(6)?;
    encoder.unsigned(u64::from(header.part_count))?;
    encoder.unsigned(7)?;
    encoder.unsigned(header.application_part_target)?;
    encoder.unsigned(8)?;
    encoder.unsigned(u64::from(header.frame_plaintext_max))?;
    encoder.unsigned(9)?;
    encoder.unsigned(u64::from(header.nonce_strategy_id))?;
    encoder.unsigned(10)?;
    encoder.unsigned(u64::from(header.crypto_suite_id))?;
    encoder.unsigned(11)?;
    encode_file_key_wrap(&mut encoder, &header.file_key_wrap)?;
    encoder.unsigned(12)?;
    encoder.unsigned(u64::from(header.master_key_generation))?;
    encoder.unsigned(13)?;
    encoder.unsigned(u64::from(header.flags))?;
    Ok(encoder.into_bytes())
}

fn encode_file_key_wrap(encoder: &mut Encoder, record: &FileKeyWrap) -> Result<(), CryptoError> {
    encoder.map(4)?;
    encoder.unsigned(1)?;
    encoder.unsigned(u64::from(record.algorithm_id))?;
    encoder.unsigned(2)?;
    encoder.unsigned(u64::from(record.wrap_generation))?;
    encoder.unsigned(3)?;
    encoder.byte_string(&record.ciphertext)?;
    encoder.unsigned(4)?;
    encoder.byte_string(&record.tag)?;
    Ok(())
}

fn decode_public_header(
    bytes: &[u8],
    limits: ManifestLimits,
) -> Result<ManifestPublicHeader, CryptoError> {
    let mut decoder = Decoder::new(bytes);
    let fields = decoder.map(13)?;
    if fields != 13 {
        return Err(CryptoError::InvalidField {
            field: "manifest public header map length",
        });
    }
    let mut previous = None;
    expect_key(&mut decoder, &mut previous, 1)?;
    let package_id = fixed_bytes(
        decoder.byte_string(16, "manifest package ID")?,
        "manifest package ID",
    )?;
    expect_key(&mut decoder, &mut previous, 2)?;
    let vault_id = fixed_bytes(
        decoder.byte_string(16, "manifest vault ID")?,
        "manifest vault ID",
    )?;
    expect_key(&mut decoder, &mut previous, 3)?;
    let manifest_generation = decoder.u32("manifest generation")?;
    expect_key(&mut decoder, &mut previous, 4)?;
    let created_at_unix_ms = decoder.unsigned()?;
    expect_key(&mut decoder, &mut previous, 5)?;
    let logical_file_size = decoder.unsigned()?;
    expect_key(&mut decoder, &mut previous, 6)?;
    let part_count = decoder.u32("manifest part count")?;
    if part_count > limits.max_parts {
        return Err(CryptoError::LimitExceeded {
            field: "manifest part count",
            limit: u64::from(limits.max_parts),
            actual: u64::from(part_count),
        });
    }
    expect_key(&mut decoder, &mut previous, 7)?;
    let application_part_target = decoder.unsigned()?;
    expect_key(&mut decoder, &mut previous, 8)?;
    let frame_plaintext_max = decoder.u32("manifest frame size")?;
    expect_key(&mut decoder, &mut previous, 9)?;
    let nonce_strategy_id = decoder.u16("manifest nonce strategy")?;
    expect_key(&mut decoder, &mut previous, 10)?;
    let crypto_suite_id = decoder.u16("manifest crypto suite")?;
    expect_key(&mut decoder, &mut previous, 11)?;
    let file_key_wrap = decode_file_key_wrap(&mut decoder)?;
    expect_key(&mut decoder, &mut previous, 12)?;
    let master_key_generation = decoder.u32("master key generation")?;
    expect_key(&mut decoder, &mut previous, 13)?;
    let flags = decoder.u32("manifest flags")?;
    decoder.finish()?;
    Ok(ManifestPublicHeader {
        package_id,
        vault_id,
        manifest_generation,
        created_at_unix_ms,
        logical_file_size,
        part_count,
        application_part_target,
        frame_plaintext_max,
        nonce_strategy_id,
        crypto_suite_id,
        file_key_wrap,
        master_key_generation,
        flags,
    })
}

fn decode_file_key_wrap(decoder: &mut Decoder<'_>) -> Result<FileKeyWrap, CryptoError> {
    if decoder.map(4)? != 4 {
        return Err(CryptoError::InvalidField {
            field: "file key wrap map length",
        });
    }
    let mut previous = None;
    expect_key(decoder, &mut previous, 1)?;
    let algorithm_id = decoder.u16("file key wrap algorithm")?;
    expect_key(decoder, &mut previous, 2)?;
    let wrap_generation = decoder.u32("file key wrap generation")?;
    expect_key(decoder, &mut previous, 3)?;
    let ciphertext = fixed_bytes(
        decoder.byte_string(32, "wrapped file key")?,
        "wrapped file key",
    )?;
    expect_key(decoder, &mut previous, 4)?;
    let tag = fixed_bytes(
        decoder.byte_string(16, "file key wrap tag")?,
        "file key wrap tag",
    )?;
    Ok(FileKeyWrap {
        algorithm_id,
        wrap_generation,
        ciphertext,
        tag,
    })
}

fn encode_metadata(metadata: &ManifestMetadata) -> Result<Vec<u8>, CryptoError> {
    let optional_count = usize::from(metadata.relative_path.is_some())
        + usize::from(metadata.mime_type.is_some())
        + usize::from(metadata.logical_timestamps.is_some())
        + usize::from(!metadata.source_metadata.is_empty())
        + usize::from(!metadata.format_extensions.is_empty());
    let mut encoder = Encoder::default();
    encoder.map(4 + optional_count)?;
    encoder.unsigned(1)?;
    encoder.text(&metadata.logical_name)?;
    if let Some(path) = &metadata.relative_path {
        encoder.unsigned(2)?;
        encoder.text(path)?;
    }
    if let Some(mime_type) = &metadata.mime_type {
        encoder.unsigned(3)?;
        encoder.text(mime_type)?;
    }
    encoder.unsigned(4)?;
    encoder.unsigned(metadata.media_kind as u64)?;
    encoder.unsigned(5)?;
    encoder.byte_string(&metadata.whole_plaintext_blake3)?;
    encoder.unsigned(6)?;
    encoder.array(metadata.parts.len())?;
    for part in &metadata.parts {
        encode_part(&mut encoder, part)?;
    }
    if let Some(timestamps) = metadata.logical_timestamps {
        encoder.unsigned(7)?;
        encode_timestamps(&mut encoder, timestamps)?;
    }
    if !metadata.source_metadata.is_empty() {
        encoder.unsigned(8)?;
        encoder.map(metadata.source_metadata.len())?;
        for field in &metadata.source_metadata {
            encoder.unsigned(field.key)?;
            encoder.byte_string(&field.value)?;
        }
    }
    if !metadata.format_extensions.is_empty() {
        encoder.unsigned(9)?;
        encoder.map(metadata.format_extensions.len())?;
        for field in &metadata.format_extensions {
            encoder.unsigned(field.key)?;
            encoder.map(2)?;
            encoder.unsigned(1)?;
            encoder.unsigned(u64::from(field.critical))?;
            encoder.unsigned(2)?;
            encoder.byte_string(&field.value)?;
        }
    }
    Ok(encoder.into_bytes())
}

fn encode_part(encoder: &mut Encoder, part: &ManifestPart) -> Result<(), CryptoError> {
    encoder.map(9)?;
    encoder.unsigned(1)?;
    encoder.unsigned(u64::from(part.part_index))?;
    encoder.unsigned(2)?;
    encoder.byte_string(&part.part_instance_id)?;
    encoder.unsigned(3)?;
    encoder.unsigned(part.plaintext_offset)?;
    encoder.unsigned(4)?;
    encoder.unsigned(part.plaintext_length)?;
    encoder.unsigned(5)?;
    encoder.unsigned(part.encoded_length)?;
    encoder.unsigned(6)?;
    encoder.unsigned(u64::from(part.frame_count))?;
    encoder.unsigned(7)?;
    encoder.byte_string(&part.plaintext_blake3)?;
    encoder.unsigned(8)?;
    encoder.byte_string(&part.encoded_ciphertext_blake3)?;
    encoder.unsigned(9)?;
    encode_locator(encoder, &part.remote_locator)
}

fn encode_locator(encoder: &mut Encoder, locator: &RemoteLocator) -> Result<(), CryptoError> {
    encoder.map(if locator.locator_extension.is_some() {
        6
    } else {
        5
    })?;
    encoder.unsigned(1)?;
    encoder.signed(locator.account_id)?;
    encoder.unsigned(2)?;
    encoder.signed(locator.chat_id)?;
    encoder.unsigned(3)?;
    encoder.signed(locator.message_id)?;
    encoder.unsigned(4)?;
    encoder.text(&locator.remote_name)?;
    encoder.unsigned(5)?;
    encoder.unsigned(u64::from(locator.locator_version))?;
    if let Some(extension) = &locator.locator_extension {
        encoder.unsigned(6)?;
        encoder.byte_string(extension)?;
    }
    Ok(())
}

fn encode_timestamps(
    encoder: &mut Encoder,
    timestamps: LogicalTimestamps,
) -> Result<(), CryptoError> {
    let length = usize::from(timestamps.created_at_unix_ms.is_some())
        + usize::from(timestamps.modified_at_unix_ms.is_some());
    encoder.map(length)?;
    if let Some(created) = timestamps.created_at_unix_ms {
        encoder.unsigned(1)?;
        encoder.signed(created)?;
    }
    if let Some(modified) = timestamps.modified_at_unix_ms {
        encoder.unsigned(2)?;
        encoder.signed(modified)?;
    }
    Ok(())
}

fn decode_metadata(bytes: &[u8], limits: ManifestLimits) -> Result<ManifestMetadata, CryptoError> {
    let mut decoder = Decoder::new(bytes);
    let field_count = decoder.map(9)?;
    if !(4..=9).contains(&field_count) {
        return Err(CryptoError::InvalidField {
            field: "manifest metadata map length",
        });
    }
    let mut previous = None;
    let mut logical_name = None;
    let mut relative_path = None;
    let mut mime_type = None;
    let mut media_kind = None;
    let mut whole_plaintext_blake3 = None;
    let mut parts = None;
    let mut logical_timestamps = None;
    let mut source_metadata = Vec::new();
    let mut format_extensions = Vec::new();

    for _ in 0..field_count {
        match decoder.map_key(&mut previous)? {
            1 => {
                logical_name = Some(
                    decoder
                        .text(limits.max_name_bytes, "logical name")?
                        .to_owned(),
                );
            }
            2 => {
                relative_path = Some(
                    decoder
                        .text(limits.max_relative_path_bytes, "relative path")?
                        .to_owned(),
                );
            }
            3 => {
                mime_type = Some(
                    decoder
                        .text(limits.max_mime_type_bytes, "MIME type")?
                        .to_owned(),
                );
            }
            4 => media_kind = Some(MediaKind::from_u16(decoder.u16("media kind")?)?),
            5 => {
                whole_plaintext_blake3 = Some(fixed_bytes(
                    decoder.byte_string(32, "whole plaintext BLAKE3")?,
                    "whole plaintext BLAKE3",
                )?);
            }
            6 => parts = Some(decode_parts(&mut decoder, limits)?),
            7 => logical_timestamps = Some(decode_timestamps(&mut decoder)?),
            8 => source_metadata = decode_source_metadata(&mut decoder, limits)?,
            9 => format_extensions = decode_extensions(&mut decoder, limits)?,
            _ => {
                return Err(CryptoError::InvalidField {
                    field: "unknown manifest metadata field",
                });
            }
        }
    }
    decoder.finish()?;
    Ok(ManifestMetadata {
        logical_name: logical_name.ok_or(CryptoError::InvalidField {
            field: "missing logical name",
        })?,
        relative_path,
        mime_type,
        media_kind: media_kind.ok_or(CryptoError::InvalidField {
            field: "missing media kind",
        })?,
        whole_plaintext_blake3: whole_plaintext_blake3.ok_or(CryptoError::InvalidField {
            field: "missing whole plaintext BLAKE3",
        })?,
        parts: parts.ok_or(CryptoError::InvalidField {
            field: "missing manifest parts",
        })?,
        logical_timestamps,
        source_metadata,
        format_extensions,
    })
}

fn decode_parts(
    decoder: &mut Decoder<'_>,
    limits: ManifestLimits,
) -> Result<Vec<ManifestPart>, CryptoError> {
    let count = decoder.array(limits.max_parts as usize)?;
    if count > decoder.remaining() / MIN_PART_DESCRIPTOR_BYTES {
        return Err(CryptoError::Truncated {
            context: "manifest parts",
        });
    }
    let mut parts = Vec::new();
    parts
        .try_reserve(count.min(4096))
        .map_err(|_| CryptoError::LimitExceeded {
            field: "manifest parts allocation",
            limit: u64::from(limits.max_parts),
            actual: count as u64,
        })?;
    for _ in 0..count {
        parts.push(decode_part(decoder, limits)?);
    }
    Ok(parts)
}

fn decode_part(
    decoder: &mut Decoder<'_>,
    limits: ManifestLimits,
) -> Result<ManifestPart, CryptoError> {
    if decoder.map(9)? != 9 {
        return Err(CryptoError::InvalidField {
            field: "manifest part map length",
        });
    }
    let mut previous = None;
    expect_key(decoder, &mut previous, 1)?;
    let part_index = decoder.u32("part index")?;
    expect_key(decoder, &mut previous, 2)?;
    let part_instance_id = fixed_bytes(
        decoder.byte_string(16, "part instance ID")?,
        "part instance ID",
    )?;
    expect_key(decoder, &mut previous, 3)?;
    let plaintext_offset = decoder.unsigned()?;
    expect_key(decoder, &mut previous, 4)?;
    let plaintext_length = decoder.unsigned()?;
    expect_key(decoder, &mut previous, 5)?;
    let encoded_length = decoder.unsigned()?;
    expect_key(decoder, &mut previous, 6)?;
    let frame_count = decoder.u32("part frame count")?;
    expect_key(decoder, &mut previous, 7)?;
    let plaintext_blake3 = fixed_bytes(
        decoder.byte_string(32, "part plaintext BLAKE3")?,
        "part plaintext BLAKE3",
    )?;
    expect_key(decoder, &mut previous, 8)?;
    let encoded_ciphertext_blake3 = fixed_bytes(
        decoder.byte_string(32, "part ciphertext BLAKE3")?,
        "part ciphertext BLAKE3",
    )?;
    expect_key(decoder, &mut previous, 9)?;
    let remote_locator = decode_locator(decoder, limits)?;
    Ok(ManifestPart {
        part_index,
        part_instance_id,
        plaintext_offset,
        plaintext_length,
        encoded_length,
        frame_count,
        plaintext_blake3,
        encoded_ciphertext_blake3,
        remote_locator,
    })
}

fn decode_locator(
    decoder: &mut Decoder<'_>,
    limits: ManifestLimits,
) -> Result<RemoteLocator, CryptoError> {
    let field_count = decoder.map(6)?;
    if !(5..=6).contains(&field_count) {
        return Err(CryptoError::InvalidField {
            field: "remote locator map length",
        });
    }
    let mut previous = None;
    expect_key(decoder, &mut previous, 1)?;
    let account_id = decoder.signed("remote account ID")?;
    expect_key(decoder, &mut previous, 2)?;
    let chat_id = decoder.signed("remote chat ID")?;
    expect_key(decoder, &mut previous, 3)?;
    let message_id = decoder.signed("remote message ID")?;
    expect_key(decoder, &mut previous, 4)?;
    let remote_name = decoder
        .text(limits.max_remote_name_bytes, "remote name")?
        .to_owned();
    expect_key(decoder, &mut previous, 5)?;
    let locator_version = decoder.u16("locator version")?;
    let locator_extension = if field_count == 6 {
        expect_key(decoder, &mut previous, 6)?;
        Some(
            decoder
                .byte_string(limits.max_locator_extension_bytes, "locator extension")?
                .to_vec(),
        )
    } else {
        None
    };
    Ok(RemoteLocator {
        account_id,
        chat_id,
        message_id,
        remote_name,
        locator_version,
        locator_extension,
    })
}

fn decode_timestamps(decoder: &mut Decoder<'_>) -> Result<LogicalTimestamps, CryptoError> {
    let field_count = decoder.map(2)?;
    if field_count == 0 {
        return Err(CryptoError::InvalidField {
            field: "empty logical timestamps",
        });
    }
    let mut previous = None;
    let mut created = None;
    let mut modified = None;
    for _ in 0..field_count {
        match decoder.map_key(&mut previous)? {
            1 => created = Some(decoder.signed("logical created timestamp")?),
            2 => modified = Some(decoder.signed("logical modified timestamp")?),
            _ => {
                return Err(CryptoError::InvalidField {
                    field: "logical timestamp field",
                });
            }
        }
    }
    Ok(LogicalTimestamps {
        created_at_unix_ms: created,
        modified_at_unix_ms: modified,
    })
}

fn decode_source_metadata(
    decoder: &mut Decoder<'_>,
    limits: ManifestLimits,
) -> Result<Vec<SourceMetadataField>, CryptoError> {
    let count = decoder.map(limits.max_metadata_fields)?;
    let mut fields = Vec::new();
    fields
        .try_reserve(count.min(256))
        .map_err(|_| CryptoError::LimitExceeded {
            field: "source metadata allocation",
            limit: limits.max_metadata_fields as u64,
            actual: count as u64,
        })?;
    let mut previous = None;
    for _ in 0..count {
        fields.push(SourceMetadataField {
            key: decoder.map_key(&mut previous)?,
            value: decoder
                .byte_string(limits.max_metadata_value_bytes, "source metadata value")?
                .to_vec(),
        });
    }
    Ok(fields)
}

fn decode_extensions(
    decoder: &mut Decoder<'_>,
    limits: ManifestLimits,
) -> Result<Vec<ExtensionField>, CryptoError> {
    let count = decoder.map(limits.max_metadata_fields)?;
    let mut fields = Vec::new();
    fields
        .try_reserve(count.min(256))
        .map_err(|_| CryptoError::LimitExceeded {
            field: "format extension allocation",
            limit: limits.max_metadata_fields as u64,
            actual: count as u64,
        })?;
    let mut previous = None;
    for _ in 0..count {
        let key = decoder.map_key(&mut previous)?;
        if decoder.map(2)? != 2 {
            return Err(CryptoError::InvalidField {
                field: "format extension map length",
            });
        }
        let mut inner_previous = None;
        expect_key(decoder, &mut inner_previous, 1)?;
        let critical = match decoder.unsigned()? {
            0 => false,
            1 => true,
            _ => {
                return Err(CryptoError::InvalidField {
                    field: "format extension criticality",
                });
            }
        };
        expect_key(decoder, &mut inner_previous, 2)?;
        let value = decoder
            .byte_string(limits.max_metadata_value_bytes, "format extension value")?
            .to_vec();
        if critical {
            return Err(CryptoError::InvalidField {
                field: "unknown critical format extension",
            });
        }
        fields.push(ExtensionField {
            key,
            critical,
            value,
        });
    }
    Ok(fields)
}

fn expect_key(
    decoder: &mut Decoder<'_>,
    previous: &mut Option<u64>,
    expected: u64,
) -> Result<(), CryptoError> {
    let actual = decoder.map_key(previous)?;
    if actual == expected {
        Ok(())
    } else {
        Err(CryptoError::InvalidField {
            field: "manifest CBOR map key",
        })
    }
}

fn fixed_bytes<const N: usize>(bytes: &[u8], field: &'static str) -> Result<[u8; N], CryptoError> {
    bytes
        .try_into()
        .map_err(|_| CryptoError::InvalidField { field })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wrap::wrap_file_key;

    fn sample() -> (
        VaultMasterKey,
        FileKey,
        ManifestPublicHeader,
        ManifestMetadata,
    ) {
        let master = VaultMasterKey::from_bytes([0x11; 32]);
        let file = FileKey::from_bytes([0x22; 32]);
        let package_id = [0x33; 16];
        let vault_id = [0x44; 16];
        let file_key_wrap = match wrap_file_key(
            &master,
            &file,
            &vault_id,
            &package_id,
            5,
            7,
            &mut AeadUsageRegistry::new(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("sample file-key wrap failed: {error}"),
        };
        let public = ManifestPublicHeader {
            package_id,
            vault_id,
            manifest_generation: 9,
            created_at_unix_ms: 1_787_600_000_000,
            logical_file_size: 13,
            part_count: 2,
            application_part_target: 8,
            frame_plaintext_max: 4,
            nonce_strategy_id: NONCE_STRATEGY_ID,
            crypto_suite_id: CRYPTO_SUITE_ID,
            file_key_wrap,
            master_key_generation: 5,
            flags: 0,
        };
        let metadata = ManifestMetadata {
            logical_name: "旅行の映像 🎬.mkv".to_owned(),
            relative_path: Some("旅行/2026".to_owned()),
            mime_type: Some("video/x-matroska".to_owned()),
            media_kind: MediaKind::Video,
            whole_plaintext_blake3: [0x55; 32],
            parts: vec![
                sample_part(&package_id, 0, 0, 8, 2, 41),
                sample_part(&package_id, 1, 8, 5, 2, 42),
            ],
            logical_timestamps: Some(LogicalTimestamps {
                created_at_unix_ms: Some(-1),
                modified_at_unix_ms: Some(1_787_600_000_000),
            }),
            source_metadata: vec![SourceMetadataField {
                key: 1,
                value: b"original caption".to_vec(),
            }],
            format_extensions: vec![ExtensionField {
                key: 100,
                critical: false,
                value: vec![1, 2, 3],
            }],
        };
        (master, file, public, metadata)
    }

    fn sample_part(
        package_id: &[u8; 16],
        index: u32,
        offset: u64,
        length: u64,
        frames: u32,
        message: i64,
    ) -> ManifestPart {
        ManifestPart {
            part_index: index,
            part_instance_id: [index as u8 + 1; 16],
            plaintext_offset: offset,
            plaintext_length: length,
            encoded_length: PART_CONTAINER_HEADER_LENGTH
                + u64::from(frames) * FRAME_RECORD_OVERHEAD
                + length,
            frame_count: frames,
            plaintext_blake3: [index as u8 + 3; 32],
            encoded_ciphertext_blake3: [index as u8 + 5; 32],
            remote_locator: RemoteLocator {
                account_id: 100,
                chat_id: -200,
                message_id: message,
                remote_name: remote_part_name(package_id, index),
                locator_version: 1,
                locator_extension: Some(vec![9, 8]),
            },
        }
    }

    fn sealed() -> (VaultMasterKey, Vec<u8>) {
        let (master, file, public, metadata) = sample();
        let bytes = match seal_manifest(
            &public,
            &metadata,
            &file,
            ManifestLimits::default(),
            &mut AeadUsageRegistry::new(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("sample manifest seal failed: {error}"),
        };
        (master, bytes)
    }

    #[test]
    fn content_commitment_binds_canonical_header_and_metadata_before_sealing() {
        let (_, _, public, metadata) = sample();
        let limits = ManifestLimits::default();
        let original = manifest_content_commitment(&public, &metadata, limits).expect("commitment");
        assert_eq!(
            original,
            manifest_content_commitment(&public, &metadata, limits).expect("same content")
        );
        let mut changed = metadata.clone();
        changed.logical_name.push('x');
        assert_ne!(
            original,
            manifest_content_commitment(&public, &changed, limits).expect("changed name")
        );
        changed = metadata.clone();
        changed.whole_plaintext_blake3[0] ^= 1;
        assert_ne!(
            original,
            manifest_content_commitment(&public, &changed, limits).expect("changed digest")
        );
        let mut header = public.clone();
        header.created_at_unix_ms += 1;
        assert_ne!(
            original,
            manifest_content_commitment(&header, &metadata, limits).expect("changed header")
        );
        let mut limited = limits;
        limited.max_encrypted_metadata_bytes = 1;
        assert!(manifest_content_commitment(&public, &metadata, limited).is_err());
    }

    #[test]
    fn aligned_v2_manifest_fixture_and_version_binding() {
        let (master, file, mut public, mut metadata) = sample();
        public.flags = 1;
        public.frame_plaintext_max = 524256;
        for part in &mut metadata.parts {
            part.frame_count = 1;
            part.encoded_length =
                PART_CONTAINER_HEADER_LENGTH + FRAME_RECORD_OVERHEAD + part.plaintext_length;
        }
        let encoded = seal_manifest(
            &public,
            &metadata,
            &file,
            ManifestLimits::default(),
            &mut AeadUsageRegistry::new(),
        )
        .expect("v2 seal");
        let fixture = include_str!("../tests/vectors/manifest_v2/aligned_manifest.txt");
        assert_eq!(hex(&encoded), fixture_value(fixture, "encoded_hex"));
        assert_eq!(
            hex(blake3::hash(&encoded).as_bytes()),
            fixture_value(fixture, "encoded_blake3")
        );
        let opened =
            open_manifest(&encoded, &master, ManifestLimits::default()).expect("v2 reader");
        assert_eq!(opened.public_header, public);
        assert_eq!(opened.metadata, metadata);
        for version in [0u16, 1, 3] {
            let mut changed = encoded.clone();
            changed[8..10].copy_from_slice(&version.to_be_bytes());
            assert!(open_manifest(&changed, &master, ManifestLimits::default()).is_err());
        }
        let (_, mut legacy) = sealed();
        legacy[8..10].copy_from_slice(&2u16.to_be_bytes());
        assert!(open_manifest(&legacy, &master, ManifestLimits::default()).is_err());
    }

    #[test]
    fn canonical_unicode_manifest_roundtrip_is_deterministic() {
        let (master, file, public, metadata) = sample();
        let first = match seal_manifest(
            &public,
            &metadata,
            &file,
            ManifestLimits::default(),
            &mut AeadUsageRegistry::new(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("manifest seal failed: {error}"),
        };
        let second = match seal_manifest(
            &public,
            &metadata,
            &file,
            ManifestLimits::default(),
            &mut AeadUsageRegistry::new(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("manifest reseal failed: {error}"),
        };
        assert_eq!(first, second);
        let opened = match open_manifest(&first, &master, ManifestLimits::default()) {
            Ok(value) => value,
            Err(error) => panic!("manifest open failed: {error}"),
        };
        assert_eq!(opened.public_header, public);
        assert_eq!(opened.metadata, metadata);
        assert_eq!(format!("{:?}", opened.file_key()), "FileKey([REDACTED])");
    }

    #[test]
    fn debug_output_redacts_names_paths_locators_and_source_metadata() {
        let (_, file, public, mut metadata) = sample();
        const NAME: &str = "SENTINEL_LOGICAL_NAME_73d9";
        const PATH: &str = "SENTINEL/PRIVATE/PATH_73d9";
        const MIME: &str = "application/x-sentinel-73d9";
        const REMOTE: &str = "SENTINEL_REMOTE_NAME_73d9";
        const SOURCE: &str = "SENTINEL_SOURCE_METADATA_73d9";
        const EXTENSION: &str = "SENTINEL_EXTENSION_73d9";
        metadata.logical_name = NAME.to_owned();
        metadata.relative_path = Some(PATH.to_owned());
        metadata.mime_type = Some(MIME.to_owned());
        metadata.parts[0].remote_locator.remote_name = REMOTE.to_owned();
        metadata.source_metadata[0].value = SOURCE.as_bytes().to_vec();
        metadata.format_extensions[0].value = EXTENSION.as_bytes().to_vec();

        let metadata_debug = format!("{metadata:?}");
        let nested_debug = format!(
            "{:?} {:?} {:?} {:?}",
            metadata.parts[0],
            metadata.parts[0].remote_locator,
            metadata.source_metadata[0],
            metadata.format_extensions[0],
        );
        let opened_debug = format!(
            "{:?}",
            OpenedManifest {
                public_header: public,
                metadata,
                file_key: file,
            }
        );
        for sentinel in [NAME, PATH, MIME, REMOTE, SOURCE, EXTENSION] {
            assert!(!metadata_debug.contains(sentinel));
            assert!(!nested_debug.contains(sentinel));
            assert!(!opened_debug.contains(sentinel));
        }
        assert!(metadata_debug.contains("part_count: 2"));
        assert!(opened_debug.contains("file_key: FileKey([REDACTED])"));
    }

    #[test]
    fn manifest_writer_and_hydration_reject_reused_generation_identity() {
        let (_, file, mut public, metadata) = sample();
        let mut usage = AeadUsageRegistry::new();
        assert!(
            seal_manifest(
                &public,
                &metadata,
                &file,
                ManifestLimits::default(),
                &mut usage,
            )
            .is_ok()
        );

        let mut changed_metadata = metadata.clone();
        changed_metadata.logical_name = "changed-name.mkv".to_owned();
        assert_eq!(
            seal_manifest(
                &public,
                &changed_metadata,
                &file,
                ManifestLimits::default(),
                &mut usage,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
        assert_eq!(
            seal_manifest(
                &public,
                &changed_metadata,
                &FileKey::from_bytes([0x7a; 32]),
                ManifestLimits::default(),
                &mut usage,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );

        public.manifest_generation += 1;
        assert!(
            seal_manifest(
                &public,
                &changed_metadata,
                &file,
                ManifestLimits::default(),
                &mut usage,
            )
            .is_ok()
        );

        let (_, file, public, metadata) = sample();
        let mut restored = AeadUsageRegistry::new();
        assert!(
            restored
                .reserve_existing_manifest(
                    &file,
                    &public.vault_id,
                    &public.package_id,
                    public.manifest_generation,
                )
                .is_ok()
        );
        assert_eq!(
            seal_manifest(
                &public,
                &metadata,
                &file,
                ManifestLimits::default(),
                &mut restored,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
    }

    #[test]
    fn wrong_key_ciphertext_tag_and_public_header_tamper_fail() {
        let (master, bytes) = sealed();
        assert_eq!(
            open_manifest(
                &bytes,
                &VaultMasterKey::from_bytes([0x12; 32]),
                ManifestLimits::default()
            )
            .map(|_| ()),
            Err(CryptoError::AuthenticationFailed)
        );

        let public_length = u32::from_be_bytes(match array_at(&bytes, 12, "test public length") {
            Ok(value) => value,
            Err(error) => panic!("test envelope malformed: {error}"),
        }) as usize;
        let encrypted_start = ENVELOPE_PREFIX_LENGTH + public_length;
        let mut ciphertext_tamper = bytes.clone();
        ciphertext_tamper[encrypted_start] ^= 1;
        assert_eq!(
            open_manifest(&ciphertext_tamper, &master, ManifestLimits::default()).map(|_| ()),
            Err(CryptoError::AuthenticationFailed)
        );
        let mut tag_tamper = bytes.clone();
        let last = tag_tamper.len() - 1;
        tag_tamper[last] ^= 1;
        assert_eq!(
            open_manifest(&tag_tamper, &master, ManifestLimits::default()).map(|_| ()),
            Err(CryptoError::AuthenticationFailed)
        );

        let package_pattern = [0x33; 16];
        let public_range = ENVELOPE_PREFIX_LENGTH..encrypted_start;
        let package_position = bytes[public_range.clone()]
            .windows(16)
            .position(|window| window == package_pattern);
        let package_position = match package_position {
            Some(value) => ENVELOPE_PREFIX_LENGTH + value,
            None => panic!("package ID not found in test public header"),
        };
        let mut public_tamper = bytes;
        public_tamper[package_position] ^= 1;
        assert_eq!(
            open_manifest(&public_tamper, &master, ManifestLimits::default()).map(|_| ()),
            Err(CryptoError::AuthenticationFailed)
        );
    }

    #[test]
    fn unsupported_version_suite_flags_and_length_claims_fail_safely() {
        let (master, mut bytes) = sealed();
        bytes[8..10].copy_from_slice(&3_u16.to_be_bytes());
        assert!(matches!(
            open_manifest(&bytes, &master, ManifestLimits::default()),
            Err(CryptoError::UnsupportedVersion { .. })
        ));

        let (_, file, mut public, metadata) = sample();
        public.crypto_suite_id = 99;
        assert!(matches!(
            seal_manifest(
                &public,
                &metadata,
                &file,
                ManifestLimits::default(),
                &mut AeadUsageRegistry::new(),
            ),
            Err(CryptoError::UnsupportedSuite { suite_id: 99 })
        ));
        public.crypto_suite_id = CRYPTO_SUITE_ID;
        public.flags = 2;
        assert!(matches!(
            seal_manifest(
                &public,
                &metadata,
                &file,
                ManifestLimits::default(),
                &mut AeadUsageRegistry::new(),
            ),
            Err(CryptoError::UnknownFlags { .. })
        ));

        let (master, mut bytes) = sealed();
        bytes[16..24].copy_from_slice(&((64_u64 * 1024 * 1024) + 1).to_be_bytes());
        assert!(matches!(
            open_manifest(&bytes, &master, ManifestLimits::default()),
            Err(CryptoError::LimitExceeded { .. })
        ));
    }

    #[test]
    fn part_layout_rejects_missing_reordered_gap_overlap_duplicate_and_name_mismatch() {
        let (_, _, public, metadata) = sample();

        let mut invalid = metadata.clone();
        invalid.parts.swap(0, 1);
        assert!(
            invalid
                .validate(&public, ManifestLimits::default())
                .is_err()
        );

        let mut invalid = metadata.clone();
        invalid.parts.pop();
        assert!(matches!(
            invalid.validate(&public, ManifestLimits::default()),
            Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidPartCount
            })
        ));

        let mut invalid = metadata.clone();
        invalid.parts[1].plaintext_offset = 9;
        assert!(matches!(
            invalid.validate(&public, ManifestLimits::default()),
            Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::Gap
            })
        ));

        let mut invalid = metadata.clone();
        invalid.parts[1].plaintext_offset = 7;
        assert!(matches!(
            invalid.validate(&public, ManifestLimits::default()),
            Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::Overlap
            })
        ));

        let mut invalid = metadata.clone();
        invalid.parts[1].part_instance_id = invalid.parts[0].part_instance_id;
        assert!(matches!(
            invalid.validate(&public, ManifestLimits::default()),
            Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::DuplicatePartInstance
            })
        ));

        let mut invalid = metadata;
        invalid.parts[0].remote_locator.remote_name = "original-name.mkv".to_owned();
        assert!(matches!(
            invalid.validate(&public, ManifestLimits::default()),
            Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::RemoteNameMismatch
            })
        ));
    }

    #[test]
    fn unsafe_paths_invalid_utf8_duplicate_keys_and_critical_extensions_fail() {
        let (_, _, public, mut metadata) = sample();
        for path in ["../escape", "/absolute", "C:/drive", "a//b", "a\\b"] {
            metadata.relative_path = Some(path.to_owned());
            assert!(matches!(
                metadata.validate(&public, ManifestLimits::default()),
                Err(CryptoError::InvalidLayout {
                    violation: LayoutViolation::UnsafeRelativePath
                })
            ));
        }

        let duplicate_key = [0xa4, 0x01, 0x61, b'a', 0x01, 0x61, b'b'];
        assert_eq!(
            decode_metadata(&duplicate_key, ManifestLimits::default()),
            Err(CryptoError::DuplicateOrUnorderedMapKey)
        );
        let invalid_utf8 = [0xa4, 0x01, 0x61, 0xff];
        assert!(matches!(
            decode_metadata(&invalid_utf8, ManifestLimits::default()),
            Err(CryptoError::InvalidUtf8 { .. })
        ));

        let (_, _, public, mut metadata) = sample();
        metadata.format_extensions[0].critical = true;
        assert!(
            metadata
                .validate(&public, ManifestLimits::default())
                .is_err()
        );
    }

    #[test]
    fn excessive_part_array_claim_is_rejected_before_reservation() {
        let mut encoded = vec![0xa4, 0x01, 0x61, b'a', 0x04, 0x00, 0x05, 0x58, 0x20];
        encoded.extend_from_slice(&[0; 32]);
        encoded.extend_from_slice(&[0x06, 0x9a, 0x00, 0x0f, 0x42, 0x40]);
        assert!(matches!(
            decode_metadata(&encoded, ManifestLimits::default()),
            Err(CryptoError::Truncated {
                context: "manifest parts"
            })
        ));

        let last = encoded.len() - 1;
        encoded[last] = 0x41;
        assert!(matches!(
            decode_metadata(&encoded, ManifestLimits::default()),
            Err(CryptoError::LimitExceeded { .. })
        ));
    }

    #[test]
    fn decrypted_part_must_match_every_manifest_binding() {
        let (_, _, public, metadata) = sample();
        let descriptor = &metadata.parts[0];
        let header = match crate::PartHeader::new(
            public.package_id,
            crate::PartInstanceId(descriptor.part_instance_id),
            descriptor.part_index,
            public.part_count,
            public.logical_file_size,
            descriptor.plaintext_offset,
            descriptor.plaintext_length,
            public.frame_plaintext_max,
            crate::PartLimits::default(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("part binding header rejected: {error}"),
        };
        let summary = PlaintextPartSummary {
            header,
            plaintext_blake3: descriptor.plaintext_blake3,
            encoded_blake3: descriptor.encoded_ciphertext_blake3,
            encoded_length: descriptor.encoded_length,
        };
        assert!(descriptor.verify_decrypted_part(&public, &summary).is_ok());
        let mut wrong = summary;
        wrong.encoded_blake3[0] ^= 1;
        assert!(matches!(
            descriptor.verify_decrypted_part(&public, &wrong),
            Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InconsistentField
            })
        ));
    }

    #[test]
    fn empty_manifest_roundtrip_has_no_content_parts() {
        let master = VaultMasterKey::from_bytes([1; 32]);
        let file = FileKey::from_bytes([2; 32]);
        let package = [3; 16];
        let vault = [4; 16];
        let wrap = match wrap_file_key(
            &master,
            &file,
            &vault,
            &package,
            1,
            1,
            &mut AeadUsageRegistry::new(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("empty manifest file wrap failed: {error}"),
        };
        let public = ManifestPublicHeader {
            package_id: package,
            vault_id: vault,
            manifest_generation: 1,
            created_at_unix_ms: 0,
            logical_file_size: 0,
            part_count: 0,
            application_part_target: 1900 * 1024 * 1024,
            frame_plaintext_max: 8 * 1024 * 1024,
            nonce_strategy_id: NONCE_STRATEGY_ID,
            crypto_suite_id: CRYPTO_SUITE_ID,
            file_key_wrap: wrap,
            master_key_generation: 1,
            flags: 0,
        };
        let metadata = ManifestMetadata {
            logical_name: "empty.bin".to_owned(),
            relative_path: None,
            mime_type: None,
            media_kind: MediaKind::Other,
            whole_plaintext_blake3: *blake3::hash(&[]).as_bytes(),
            parts: Vec::new(),
            logical_timestamps: None,
            source_metadata: Vec::new(),
            format_extensions: Vec::new(),
        };
        let bytes = match seal_manifest(
            &public,
            &metadata,
            &file,
            ManifestLimits::default(),
            &mut AeadUsageRegistry::new(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("empty manifest seal failed: {error}"),
        };
        assert!(open_manifest(&bytes, &master, ManifestLimits::default()).is_ok());
    }

    #[test]
    fn hostile_mutation_and_truncation_corpus_never_panics() {
        let (master, bytes) = sealed();
        for length in 0..bytes.len() {
            let _ = open_manifest(&bytes[..length], &master, ManifestLimits::default());
        }
        for index in 0..bytes.len() {
            let mut mutated = bytes.clone();
            mutated[index] ^= 0x80;
            let _ = open_manifest(&mutated, &master, ManifestLimits::default());
        }
    }

    #[test]
    fn candidate_vector_matches_fixture() {
        let (_, bytes) = sealed();
        let fixture = include_str!("../tests/vectors/manifest_v1/unicode_multipart.txt");
        assert_eq!(hex(&bytes), fixture_value(fixture, "encoded_hex"));
        assert_eq!(
            hex(blake3::hash(&bytes).as_bytes()),
            fixture_value(fixture, "encoded_blake3")
        );
    }

    fn hex(bytes: &[u8]) -> String {
        let mut value = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            if write!(value, "{byte:02x}").is_err() {
                panic!("writing to a String unexpectedly failed");
            }
        }
        value
    }

    fn fixture_value<'a>(fixture: &'a str, key: &str) -> &'a str {
        fixture
            .lines()
            .find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix('='))
            })
            .unwrap_or_else(|| panic!("fixture key missing: {key}"))
    }
}
