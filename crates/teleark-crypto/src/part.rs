use std::collections::BTreeSet;
use std::io::{self, Read, Write};

use zeroize::Zeroizing;

use crate::aead::{decrypt_detached, encrypt_detached};
use crate::kdf::part_content_key;
use crate::{
    AeadUsageRegistry, CRYPTO_SUITE_ID, CryptoError, FORMAT_MAJOR, FORMAT_MINOR, FileKey,
    FormatKind, LayoutViolation, NONCE_STRATEGY_ID, RandomSource,
};

const PART_MAGIC: &[u8; 8] = b"TARKPART";
const PART_HEADER_LENGTH: usize = 96;
const FRAME_HEADER_LENGTH: usize = 16;
const TAG_LENGTH: usize = 16;
const FINAL_FRAME_FLAG: u8 = 1;
const CONTENT_FRAME_DOMAIN: &[u8] = b"teleark/content-frame/v1";

/// Bounded parsing/streaming policy for application-part containers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartLimits {
    pub max_frame_plaintext: u32,
    pub max_frames_per_part: u32,
    pub max_part_plaintext: u64,
    pub max_encoded_part: u64,
    pub max_parts_per_package: u32,
}

impl Default for PartLimits {
    fn default() -> Self {
        Self {
            max_frame_plaintext: 16 * 1024 * 1024,
            max_frames_per_part: 1_000_000,
            max_part_plaintext: 2 * 1024 * 1024 * 1024,
            max_encoded_part: 3 * 1024 * 1024 * 1024,
            max_parts_per_package: 1_000_000,
        }
    }
}

/// Fresh random identity which domain-separates each application-part encoding.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PartInstanceId(pub [u8; 16]);

/// Package-local registry used to reject an observable random identity collision.
#[derive(Debug, Default)]
pub struct PartInstanceRegistry {
    seen: BTreeSet<PartInstanceId>,
}

impl PartInstanceRegistry {
    pub fn generate(
        &mut self,
        random: &mut impl RandomSource,
    ) -> Result<PartInstanceId, CryptoError> {
        // A healthy CSPRNG will succeed on the first attempt. The finite retry
        // bound also prevents a faulty source from spinning forever.
        for _ in 0..16 {
            let mut bytes = [0_u8; 16];
            random.fill_bytes(&mut bytes)?;
            let id = PartInstanceId(bytes);
            if self.seen.insert(id) {
                return Ok(id);
            }
        }
        Err(CryptoError::InvalidLayout {
            violation: LayoutViolation::DuplicatePartInstance,
        })
    }

    pub fn register(&mut self, id: PartInstanceId) -> Result<(), CryptoError> {
        if self.seen.insert(id) {
            Ok(())
        } else {
            Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::DuplicatePartInstance,
            })
        }
    }
}

/// Canonical public application-part header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartHeader {
    /// Part codec major version. Version 2 aligns AEAD frames to upload parts.
    pub format_major: u16,
    pub package_id: [u8; 16],
    pub part_instance_id: PartInstanceId,
    pub part_index: u32,
    pub part_count: u32,
    pub logical_file_size: u64,
    pub plaintext_offset: u64,
    pub plaintext_length: u64,
    pub frame_plaintext_max: u32,
    pub frame_count: u32,
}

impl PartHeader {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        package_id: [u8; 16],
        part_instance_id: PartInstanceId,
        part_index: u32,
        part_count: u32,
        logical_file_size: u64,
        plaintext_offset: u64,
        plaintext_length: u64,
        frame_plaintext_max: u32,
        limits: PartLimits,
    ) -> Result<Self, CryptoError> {
        let frame_count = expected_frame_count(plaintext_length, frame_plaintext_max)?;
        let header = Self {
            format_major: FORMAT_MAJOR,
            package_id,
            part_instance_id,
            part_index,
            part_count,
            logical_file_size,
            plaintext_offset,
            plaintext_length,
            frame_plaintext_max,
            frame_count,
        };
        header.validate(limits)?;
        Ok(header)
    }

    /// Opt into the streaming codec; legacy v1 headers retain their original geometry.
    pub fn aligned(mut self, limits: PartLimits) -> Result<Self, CryptoError> {
        self.format_major = 2;
        self.frame_plaintext_max = 512 * 1024 - 32;
        self.frame_count = self.expected_frame_count()?;
        self.validate(limits)?;
        Ok(self)
    }

    fn expected_frame_count(&self) -> Result<u32, CryptoError> {
        let length = if self.format_major == 2 {
            self.plaintext_length
                .checked_add(PART_HEADER_LENGTH as u64)
                .ok_or(CryptoError::ArithmeticOverflow {
                    field: "aligned frame count",
                })?
        } else {
            self.plaintext_length
        };
        expected_frame_count(length, self.frame_plaintext_max)
    }

    pub fn validate(&self, limits: PartLimits) -> Result<(), CryptoError> {
        if !matches!(self.format_major, 1 | 2) {
            return Err(CryptoError::UnsupportedVersion {
                format: FormatKind::Part,
                major: self.format_major,
                minor: 0,
            });
        }
        if self.format_major == 2 && self.frame_plaintext_max != 512 * 1024 - 32 {
            return Err(CryptoError::InvalidField {
                field: "aligned frame size",
            });
        }
        if self.part_count == 0 || self.part_count > limits.max_parts_per_package {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidPartCount,
            });
        }
        if self.part_index >= self.part_count {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidPartIndex,
            });
        }
        if self.frame_plaintext_max == 0 || self.frame_plaintext_max > limits.max_frame_plaintext {
            return Err(CryptoError::LimitExceeded {
                field: "frame_plaintext_max",
                limit: u64::from(limits.max_frame_plaintext),
                actual: u64::from(self.frame_plaintext_max),
            });
        }
        if self.plaintext_length > limits.max_part_plaintext {
            return Err(CryptoError::LimitExceeded {
                field: "part_plaintext_length",
                limit: limits.max_part_plaintext,
                actual: self.plaintext_length,
            });
        }
        let end = self
            .plaintext_offset
            .checked_add(self.plaintext_length)
            .ok_or(CryptoError::ArithmeticOverflow {
                field: "part plaintext range",
            })?;
        if end > self.logical_file_size {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::OutOfRange,
            });
        }
        if self.plaintext_length == 0
            && (self.logical_file_size != 0
                || self.plaintext_offset != 0
                || self.part_index != 0
                || self.part_count != 1)
        {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::OutOfRange,
            });
        }
        let expected_frames = self.expected_frame_count()?;
        if self.frame_count != expected_frames || self.frame_count > limits.max_frames_per_part {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidFrameCount,
            });
        }
        let encoded_length = self.expected_encoded_length()?;
        if encoded_length > limits.max_encoded_part {
            return Err(CryptoError::LimitExceeded {
                field: "encoded_part_length",
                limit: limits.max_encoded_part,
                actual: encoded_length,
            });
        }
        Ok(())
    }

    pub fn expected_encoded_length(&self) -> Result<u64, CryptoError> {
        let overhead = u64::from(self.frame_count)
            .checked_mul((FRAME_HEADER_LENGTH + TAG_LENGTH) as u64)
            .and_then(|value| value.checked_add(PART_HEADER_LENGTH as u64))
            .ok_or(CryptoError::ArithmeticOverflow {
                field: "encoded part overhead",
            })?;
        overhead
            .checked_add(self.plaintext_length)
            .ok_or(CryptoError::ArithmeticOverflow {
                field: "encoded part length",
            })
    }

    #[must_use]
    pub fn encode(&self) -> [u8; PART_HEADER_LENGTH] {
        let mut output = [0_u8; PART_HEADER_LENGTH];
        output[..8].copy_from_slice(PART_MAGIC);
        output[8..10].copy_from_slice(&self.format_major.to_be_bytes());
        output[10..12].copy_from_slice(&FORMAT_MINOR.to_be_bytes());
        output[12..16].copy_from_slice(&(PART_HEADER_LENGTH as u32).to_be_bytes());
        output[16..32].copy_from_slice(&self.package_id);
        output[32..48].copy_from_slice(&self.part_instance_id.0);
        output[48..52].copy_from_slice(&self.part_index.to_be_bytes());
        output[52..56].copy_from_slice(&self.part_count.to_be_bytes());
        output[56..64].copy_from_slice(&self.logical_file_size.to_be_bytes());
        output[64..72].copy_from_slice(&self.plaintext_offset.to_be_bytes());
        output[72..80].copy_from_slice(&self.plaintext_length.to_be_bytes());
        output[80..84].copy_from_slice(&self.frame_plaintext_max.to_be_bytes());
        output[84..88].copy_from_slice(&self.frame_count.to_be_bytes());
        output[88..90].copy_from_slice(&CRYPTO_SUITE_ID.to_be_bytes());
        output[90..92].copy_from_slice(&NONCE_STRATEGY_ID.to_be_bytes());
        // bytes 92..96 are the all-zero v1 flags field.
        output
    }

    /// Decode and validate a persisted header before restoring its encryption identity.
    pub fn decode(
        bytes: &[u8; PART_HEADER_LENGTH],
        limits: PartLimits,
    ) -> Result<Self, CryptoError> {
        if &bytes[..8] != PART_MAGIC {
            return Err(CryptoError::InvalidMagic {
                format: FormatKind::Part,
            });
        }
        let major = u16_at(bytes, 8, "part major version")?;
        let minor = u16_at(bytes, 10, "part minor version")?;
        if !matches!(major, 1 | 2) || minor != FORMAT_MINOR {
            return Err(CryptoError::UnsupportedVersion {
                format: FormatKind::Part,
                major,
                minor,
            });
        }
        if u32_at(bytes, 12, "part header length")? != PART_HEADER_LENGTH as u32 {
            return Err(CryptoError::InvalidField {
                field: "part header length",
            });
        }
        let suite = u16_at(bytes, 88, "part suite")?;
        if suite != CRYPTO_SUITE_ID {
            return Err(CryptoError::UnsupportedSuite { suite_id: suite });
        }
        let strategy = u16_at(bytes, 90, "part nonce strategy")?;
        if strategy != NONCE_STRATEGY_ID {
            return Err(CryptoError::UnsupportedNonceStrategy {
                strategy_id: strategy,
            });
        }
        let flags = u32_at(bytes, 92, "part flags")?;
        if flags != 0 {
            return Err(CryptoError::UnknownFlags {
                field: "part header",
                flags,
            });
        }
        let header = Self {
            format_major: major,
            package_id: array_at(bytes, 16, "part package ID")?,
            part_instance_id: PartInstanceId(array_at(bytes, 32, "part instance ID")?),
            part_index: u32_at(bytes, 48, "part index")?,
            part_count: u32_at(bytes, 52, "part count")?,
            logical_file_size: u64_at(bytes, 56, "logical file size")?,
            plaintext_offset: u64_at(bytes, 64, "part plaintext offset")?,
            plaintext_length: u64_at(bytes, 72, "part plaintext length")?,
            frame_plaintext_max: u32_at(bytes, 80, "frame plaintext max")?,
            frame_count: u32_at(bytes, 84, "frame count")?,
        };
        header.validate(limits)?;
        Ok(header)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedPartSummary {
    pub header: PartHeader,
    pub plaintext_blake3: [u8; 32],
    pub encoded_blake3: [u8; 32],
    pub encoded_length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaintextPartSummary {
    pub header: PartHeader,
    pub plaintext_blake3: [u8; 32],
    pub encoded_blake3: [u8; 32],
    pub encoded_length: u64,
}

/// Deterministic 96-bit nonce: part index (big-endian) plus zero-extended
/// frame index (big-endian u64).
#[must_use]
pub fn frame_nonce(part_index: u32, frame_index: u32) -> [u8; 12] {
    let mut nonce = [0_u8; 12];
    nonce[..4].copy_from_slice(&part_index.to_be_bytes());
    nonce[4..].copy_from_slice(&u64::from(frame_index).to_be_bytes());
    nonce
}

pub fn encrypt_part(
    source: &mut impl Read,
    destination: &mut impl Write,
    header: &PartHeader,
    file_key: &FileKey,
    limits: PartLimits,
    usage: &mut AeadUsageRegistry,
) -> Result<EncryptedPartSummary, CryptoError> {
    encrypt_part_cancellable(source, destination, header, file_key, limits, usage, || {
        false
    })
}

/// Check cancellation before allocating or processing each authenticated frame.
/// A cancelled output is partial and must never be published as a complete part.
pub fn encrypt_part_cancellable(
    source: &mut impl Read,
    destination: &mut impl Write,
    header: &PartHeader,
    file_key: &FileKey,
    limits: PartLimits,
    usage: &mut AeadUsageRegistry,
    mut cancelled: impl FnMut() -> bool,
) -> Result<EncryptedPartSummary, CryptoError> {
    if cancelled() {
        return Err(CryptoError::Cancelled);
    }
    header.validate(limits)?;
    let header_bytes = header.encode();
    let header_digest = *blake3::hash(&header_bytes).as_bytes();
    let content_key = part_content_key(
        file_key.as_bytes(),
        &header.package_id,
        header.part_index,
        &header.part_instance_id.0,
    )?;
    usage.reserve_part(
        content_key.as_ref(),
        header.package_id,
        header.part_index,
        header.part_instance_id,
    )?;
    write_all(destination, &header_bytes, "part header")?;

    let mut plaintext_hasher = blake3::Hasher::new();
    let mut encoded_hasher = blake3::Hasher::new();
    encoded_hasher.update(&header_bytes);
    let mut remaining = header.plaintext_length;

    let mut buffer = Zeroizing::new(Vec::with_capacity(header.frame_plaintext_max as usize));
    for frame_index in 0..header.frame_count {
        if cancelled() {
            return Err(CryptoError::Cancelled);
        }
        let is_final = frame_index + 1 == header.frame_count;
        let plaintext_length = expected_frame_length(header, remaining, is_final)?;
        let allocation_length =
            usize::try_from(plaintext_length).map_err(|_| CryptoError::ArithmeticOverflow {
                field: "frame allocation length",
            })?;
        buffer.resize(allocation_length, 0);
        read_exact(source, &mut buffer, "part plaintext frame")?;
        if cancelled() {
            return Err(CryptoError::Cancelled);
        }
        plaintext_hasher.update(&buffer);

        let flags = if is_final { FINAL_FRAME_FLAG } else { 0 };
        let frame_header = encode_frame_header(frame_index, plaintext_length, flags);
        let aad = frame_aad(header, &header_digest, frame_index, plaintext_length, flags);
        let nonce = frame_nonce(header.part_index, frame_index);
        let tag = encrypt_detached(content_key.as_ref(), &nonce, &aad, buffer.as_mut())?;

        if cancelled() {
            return Err(CryptoError::Cancelled);
        }
        write_all(destination, &frame_header, "frame header")?;
        write_all(destination, &buffer, "frame ciphertext")?;
        write_all(destination, &tag, "frame authentication tag")?;
        encoded_hasher.update(&frame_header);
        encoded_hasher.update(&buffer);
        encoded_hasher.update(&tag);
        remaining = remaining.checked_sub(u64::from(plaintext_length)).ok_or(
            CryptoError::ArithmeticOverflow {
                field: "remaining part plaintext",
            },
        )?;
    }

    if remaining != 0 {
        return Err(CryptoError::InvalidLayout {
            violation: LayoutViolation::InvalidFrameLength,
        });
    }
    Ok(EncryptedPartSummary {
        header: header.clone(),
        plaintext_blake3: *plaintext_hasher.finalize().as_bytes(),
        encoded_blake3: *encoded_hasher.finalize().as_bytes(),
        encoded_length: header.expected_encoded_length()?,
    })
}

/// Authenticate and decrypt a container into a caller-controlled partial-file
/// destination. The caller must not expose/rename that destination until this
/// function and the manifest/whole-file checks have all succeeded.
pub fn decrypt_part(
    source: &mut impl Read,
    destination: &mut impl Write,
    file_key: &FileKey,
    limits: PartLimits,
) -> Result<PlaintextPartSummary, CryptoError> {
    decrypt_part_cancellable(source, destination, file_key, limits, || false)
}

/// Cancel at frame boundaries without producing a successful integrity summary.
pub fn decrypt_part_cancellable(
    source: &mut impl Read,
    destination: &mut impl Write,
    file_key: &FileKey,
    limits: PartLimits,
    mut cancelled: impl FnMut() -> bool,
) -> Result<PlaintextPartSummary, CryptoError> {
    if cancelled() {
        return Err(CryptoError::Cancelled);
    }
    let mut header_bytes = [0_u8; PART_HEADER_LENGTH];
    read_exact(source, &mut header_bytes, "part header")?;
    let header = PartHeader::decode(&header_bytes, limits)?;
    let header_digest = *blake3::hash(&header_bytes).as_bytes();
    let content_key = part_content_key(
        file_key.as_bytes(),
        &header.package_id,
        header.part_index,
        &header.part_instance_id.0,
    )?;
    let mut plaintext_hasher = blake3::Hasher::new();
    let mut encoded_hasher = blake3::Hasher::new();
    encoded_hasher.update(&header_bytes);
    let mut remaining = header.plaintext_length;

    for expected_index in 0..header.frame_count {
        if cancelled() {
            return Err(CryptoError::Cancelled);
        }
        let mut frame_header = [0_u8; FRAME_HEADER_LENGTH];
        read_exact(source, &mut frame_header, "frame header")?;
        let frame_index = u32_at(&frame_header, 0, "frame index")?;
        let plaintext_length = u32_at(&frame_header, 4, "frame plaintext length")?;
        let ciphertext_length = u32_at(&frame_header, 8, "frame ciphertext length")?;
        let flags = frame_header[12];
        if frame_header[13..] != [0, 0, 0] {
            return Err(CryptoError::InvalidField {
                field: "frame reserved bytes",
            });
        }
        if frame_index != expected_index {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidPartIndex,
            });
        }
        if ciphertext_length != plaintext_length {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidFrameLength,
            });
        }
        let is_final = expected_index + 1 == header.frame_count;
        let expected_length = expected_frame_length(&header, remaining, is_final)?;
        let expected_flags = if is_final { FINAL_FRAME_FLAG } else { 0 };
        if plaintext_length != expected_length || flags != expected_flags {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidFinalFrame,
            });
        }
        let allocation_length =
            usize::try_from(ciphertext_length).map_err(|_| CryptoError::ArithmeticOverflow {
                field: "frame allocation length",
            })?;
        let mut buffer = Zeroizing::new(vec![0_u8; allocation_length]);
        let mut tag = [0_u8; TAG_LENGTH];
        read_exact(source, &mut buffer, "frame ciphertext")?;
        read_exact(source, &mut tag, "frame authentication tag")?;
        if cancelled() {
            return Err(CryptoError::Cancelled);
        }
        encoded_hasher.update(&frame_header);
        encoded_hasher.update(&buffer);
        encoded_hasher.update(&tag);

        let aad = frame_aad(
            &header,
            &header_digest,
            frame_index,
            plaintext_length,
            flags,
        );
        let nonce = frame_nonce(header.part_index, frame_index);
        decrypt_detached(content_key.as_ref(), &nonce, &aad, buffer.as_mut(), &tag)?;
        plaintext_hasher.update(&buffer);
        if cancelled() {
            return Err(CryptoError::Cancelled);
        }
        write_all(destination, &buffer, "authenticated plaintext frame")?;
        remaining = remaining.checked_sub(u64::from(plaintext_length)).ok_or(
            CryptoError::ArithmeticOverflow {
                field: "remaining part plaintext",
            },
        )?;
    }

    if remaining != 0 {
        return Err(CryptoError::InvalidLayout {
            violation: LayoutViolation::InvalidFrameLength,
        });
    }
    ensure_eof(source, "part container")?;
    Ok(PlaintextPartSummary {
        header: header.clone(),
        plaintext_blake3: *plaintext_hasher.finalize().as_bytes(),
        encoded_blake3: *encoded_hasher.finalize().as_bytes(),
        encoded_length: header.expected_encoded_length()?,
    })
}

fn expected_frame_count(length: u64, frame_max: u32) -> Result<u32, CryptoError> {
    if frame_max == 0 {
        return Err(CryptoError::InvalidField {
            field: "frame_plaintext_max",
        });
    }
    if length == 0 {
        return Ok(1);
    }
    let frame_max = u64::from(frame_max);
    let count = length
        .checked_add(frame_max - 1)
        .ok_or(CryptoError::ArithmeticOverflow {
            field: "frame count rounding",
        })?
        / frame_max;
    u32::try_from(count).map_err(|_| CryptoError::ArithmeticOverflow {
        field: "frame count",
    })
}

fn expected_frame_length(
    header: &PartHeader,
    remaining: u64,
    is_final: bool,
) -> Result<u32, CryptoError> {
    let frame_max = header.frame_plaintext_max
        - if header.format_major == 2 && remaining == header.plaintext_length {
            PART_HEADER_LENGTH as u32
        } else {
            0
        };
    if !is_final {
        if remaining < u64::from(frame_max) {
            return Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidFrameLength,
            });
        }
        return Ok(frame_max);
    }
    u32::try_from(remaining).map_err(|_| CryptoError::ArithmeticOverflow {
        field: "final frame length",
    })
}

fn encode_frame_header(frame_index: u32, plaintext_length: u32, flags: u8) -> [u8; 16] {
    let mut output = [0_u8; FRAME_HEADER_LENGTH];
    output[..4].copy_from_slice(&frame_index.to_be_bytes());
    output[4..8].copy_from_slice(&plaintext_length.to_be_bytes());
    output[8..12].copy_from_slice(&plaintext_length.to_be_bytes());
    output[12] = flags;
    output
}

fn frame_aad(
    header: &PartHeader,
    header_digest: &[u8; 32],
    frame_index: u32,
    plaintext_length: u32,
    flags: u8,
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(CONTENT_FRAME_DOMAIN.len() + 132);
    aad.extend_from_slice(CONTENT_FRAME_DOMAIN);
    aad.extend_from_slice(header_digest);
    aad.extend_from_slice(&header.package_id);
    aad.extend_from_slice(&header.part_instance_id.0);
    aad.extend_from_slice(&header.part_index.to_be_bytes());
    aad.extend_from_slice(&header.part_count.to_be_bytes());
    aad.extend_from_slice(&header.logical_file_size.to_be_bytes());
    aad.extend_from_slice(&header.plaintext_offset.to_be_bytes());
    aad.extend_from_slice(&header.plaintext_length.to_be_bytes());
    aad.extend_from_slice(&header.frame_plaintext_max.to_be_bytes());
    aad.extend_from_slice(&header.frame_count.to_be_bytes());
    aad.extend_from_slice(&CRYPTO_SUITE_ID.to_be_bytes());
    aad.extend_from_slice(&NONCE_STRATEGY_ID.to_be_bytes());
    aad.extend_from_slice(&frame_index.to_be_bytes());
    aad.extend_from_slice(&plaintext_length.to_be_bytes());
    aad.extend_from_slice(&plaintext_length.to_be_bytes());
    aad.push(flags);
    aad
}

fn read_exact(
    source: &mut impl Read,
    destination: &mut [u8],
    context: &'static str,
) -> Result<(), CryptoError> {
    source.read_exact(destination).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            CryptoError::Truncated { context }
        } else {
            CryptoError::io("read encrypted part", &error)
        }
    })
}

fn write_all(
    destination: &mut impl Write,
    bytes: &[u8],
    _context: &'static str,
) -> Result<(), CryptoError> {
    destination
        .write_all(bytes)
        .map_err(|error| CryptoError::io("write encrypted part", &error))
}

fn ensure_eof(source: &mut impl Read, context: &'static str) -> Result<(), CryptoError> {
    let mut byte = [0_u8; 1];
    match source.read(&mut byte) {
        Ok(0) => Ok(()),
        Ok(_) => Err(CryptoError::TrailingData { context }),
        Err(error) => Err(CryptoError::io("check encrypted part boundary", &error)),
    }
}

fn array_at<const N: usize>(
    bytes: &[u8],
    offset: usize,
    context: &'static str,
) -> Result<[u8; N], CryptoError> {
    bytes
        .get(offset..offset.saturating_add(N))
        .and_then(|slice| slice.try_into().ok())
        .ok_or(CryptoError::Truncated { context })
}

fn u16_at(bytes: &[u8], offset: usize, context: &'static str) -> Result<u16, CryptoError> {
    Ok(u16::from_be_bytes(array_at(bytes, offset, context)?))
}

fn u32_at(bytes: &[u8], offset: usize, context: &'static str) -> Result<u32, CryptoError> {
    Ok(u32::from_be_bytes(array_at(bytes, offset, context)?))
}

fn u64_at(bytes: &[u8], offset: usize, context: &'static str) -> Result<u64, CryptoError> {
    Ok(u64::from_be_bytes(array_at(bytes, offset, context)?))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn header() -> PartHeader {
        match PartHeader::new(
            [1; 16],
            PartInstanceId([2; 16]),
            0,
            1,
            21,
            0,
            21,
            8,
            PartLimits::default(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("test header failed: {error}"),
        }
    }

    fn encoded_part() -> Vec<u8> {
        let mut encoded = Vec::new();
        let result = encrypt_part(
            &mut Cursor::new(b"bounded frame payload"),
            &mut encoded,
            &header(),
            &FileKey::from_bytes([3; 32]),
            PartLimits::default(),
            &mut AeadUsageRegistry::new(),
        );
        if let Err(error) = result {
            panic!("test encryption failed: {error}");
        }
        encoded
    }

    #[test]
    fn aligned_v2_frozen_fixture() {
        let header = header().aligned(PartLimits::default()).expect("aligned");
        let mut encoded = Vec::new();
        encrypt_part(
            &mut Cursor::new(b"bounded frame payload"),
            &mut encoded,
            &header,
            &FileKey::from_bytes([3; 32]),
            PartLimits::default(),
            &mut AeadUsageRegistry::new(),
        )
        .expect("encrypt");
        let fixture = include_str!("../tests/vectors/crypto_v2/aligned_part.txt");
        assert_eq!(hex(&encoded), fixture_value(fixture, "encoded_hex"));
        assert_eq!(
            hex(blake3::hash(&encoded).as_bytes()),
            fixture_value(fixture, "encoded_blake3")
        );
        let mut decoded = Vec::new();
        decrypt_part(
            &mut Cursor::new(&encoded),
            &mut decoded,
            &FileKey::from_bytes([3; 32]),
            PartLimits::default(),
        )
        .expect("decode");
        assert_eq!(decoded, b"bounded frame payload");
    }

    #[test]
    fn aligned_frames_match_upload_boundaries_and_preserve_legacy_reader() {
        for length in [0, 1, 524_160, 524_161, 1_048_416, 1_048_417, 2_000_000] {
            let data = vec![0x5a; length];
            let header = PartHeader::new(
                [7; 16],
                PartInstanceId([8; 16]),
                0,
                1,
                length as u64,
                0,
                length as u64,
                8 * 1024 * 1024,
                PartLimits::default(),
            )
            .expect("header")
            .aligned(PartLimits::default())
            .expect("aligned");
            let mut encoded = Vec::new();
            let key = FileKey::from_bytes([9; 32]);
            encrypt_part(
                &mut Cursor::new(&data),
                &mut encoded,
                &header,
                &key,
                PartLimits::default(),
                &mut AeadUsageRegistry::new(),
            )
            .expect("encrypt");
            assert_eq!(&encoded[8..12], &[0, 2, 0, 0]);
            for index in 0..header.frame_count {
                let offset = if index == 0 {
                    96
                } else {
                    index as usize * 512 * 1024
                };
                assert_eq!(
                    u32::from_be_bytes(encoded[offset..offset + 4].try_into().expect("index")),
                    index
                );
                let payload =
                    u32::from_be_bytes(encoded[offset + 4..offset + 8].try_into().expect("length"))
                        as usize;
                if index + 1 < header.frame_count {
                    assert_eq!(offset + 32 + payload, (index as usize + 1) * 512 * 1024);
                }
            }
            let mut clear = Vec::new();
            decrypt_part(
                &mut Cursor::new(&encoded),
                &mut clear,
                &key,
                PartLimits::default(),
            )
            .expect("decrypt v2");
            assert_eq!(clear, data);
            let mut corrupt = encoded.clone();
            corrupt[8..10].copy_from_slice(&1u16.to_be_bytes());
            assert!(
                decrypt_part(
                    &mut Cursor::new(corrupt),
                    &mut Vec::new(),
                    &key,
                    PartLimits::default()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn aligned_download_writes_authenticated_plaintext_before_requesting_the_next_block() {
        use std::{cell::Cell, rc::Rc};
        const BLOCK: usize = 512 * 1024;
        struct Output(Rc<Cell<usize>>);
        impl Write for Output {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.set(self.0.get() + bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        struct Network {
            bytes: Cursor<Vec<u8>>,
            written: Rc<Cell<usize>>,
        }
        impl Read for Network {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let position = self.bytes.position() as usize;
                if position == BLOCK {
                    assert_eq!(
                        self.written.get(),
                        BLOCK - PART_HEADER_LENGTH - FRAME_HEADER_LENGTH - TAG_LENGTH,
                        "first block is authenticated and written before the next arrives"
                    );
                }
                let count = out.len().min(BLOCK - position % BLOCK);
                self.bytes.read(&mut out[..count])
            }
        }
        let key = FileKey::from_bytes([3; 32]);
        let plain = vec![0x5a; 2 * BLOCK];
        let header = PartHeader::new(
            [1; 16],
            PartInstanceId([2; 16]),
            0,
            1,
            plain.len() as u64,
            0,
            plain.len() as u64,
            (BLOCK - 32) as u32,
            PartLimits::default(),
        )
        .expect("header")
        .aligned(PartLimits::default())
        .expect("aligned frames");
        let mut bytes = Vec::new();
        encrypt_part(
            &mut Cursor::new(&plain),
            &mut bytes,
            &header,
            &key,
            PartLimits::default(),
            &mut AeadUsageRegistry::new(),
        )
        .expect("encrypted fixture");
        for corrupt in [false, true] {
            let mut encoded = bytes.clone();
            if corrupt {
                encoded[2 * BLOCK - 1] ^= 1;
            }
            let written = Rc::new(Cell::new(0));
            let mut input = Network {
                bytes: Cursor::new(encoded),
                written: written.clone(),
            };
            let result = decrypt_part(
                &mut input,
                &mut Output(written.clone()),
                &key,
                PartLimits::default(),
            );
            if corrupt {
                assert!(result.is_err());
                assert_eq!(
                    written.get(),
                    BLOCK - 128,
                    "unauthenticated second-frame plaintext never reaches the file writer"
                );
            } else {
                assert!(result.is_ok());
                assert_eq!(written.get(), plain.len());
            }
        }
    }

    #[test]
    fn cancellation_stops_at_frame_boundary_without_a_success_summary() {
        use std::{cell::Cell, rc::Rc};
        struct StopWriter {
            bytes: Vec<u8>,
            after: usize,
            stop: Rc<Cell<bool>>,
        }
        impl Write for StopWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                if self.bytes.len() >= self.after {
                    self.stop.set(true);
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let stop = Rc::new(Cell::new(false));
        let mut plaintext = Cursor::new(b"bounded frame payload");
        let first_frame_end = PART_HEADER_LENGTH + FRAME_HEADER_LENGTH + 8 + TAG_LENGTH;
        let mut encoded = StopWriter {
            bytes: Vec::new(),
            after: first_frame_end,
            stop: stop.clone(),
        };
        assert_eq!(
            encrypt_part_cancellable(
                &mut plaintext,
                &mut encoded,
                &header(),
                &FileKey::from_bytes([3; 32]),
                PartLimits::default(),
                &mut AeadUsageRegistry::new(),
                || stop.get()
            ),
            Err(CryptoError::Cancelled)
        );
        assert_eq!(
            plaintext.position(),
            8,
            "no plaintext from the second frame was read"
        );
        assert_eq!(encoded.bytes.len(), first_frame_end);
        stop.set(false);
        let complete = encoded_part();
        let mut source = Cursor::new(complete);
        let mut decoded = StopWriter {
            bytes: Vec::new(),
            after: 8,
            stop: stop.clone(),
        };
        assert_eq!(
            decrypt_part_cancellable(
                &mut source,
                &mut decoded,
                &FileKey::from_bytes([3; 32]),
                PartLimits::default(),
                || stop.get()
            ),
            Err(CryptoError::Cancelled)
        );
        assert_eq!(decoded.bytes, b"bounded ");
        assert_eq!(
            source.position() as usize,
            first_frame_end,
            "no second frame was read or authenticated"
        );
        let mut source = Cursor::new(b"bounded frame payload");
        let mut output = Vec::new();
        assert_eq!(
            encrypt_part_cancellable(
                &mut source,
                &mut output,
                &header(),
                &FileKey::from_bytes([3; 32]),
                PartLimits::default(),
                &mut AeadUsageRegistry::new(),
                || true
            ),
            Err(CryptoError::Cancelled)
        );
        assert_eq!(source.position(), 0);
        assert!(output.is_empty());
    }

    #[test]
    fn multi_frame_roundtrip_and_hashes() {
        let encoded = encoded_part();
        let mut plaintext = Vec::new();
        let result = decrypt_part(
            &mut Cursor::new(&encoded),
            &mut plaintext,
            &FileKey::from_bytes([3; 32]),
            PartLimits::default(),
        );
        let summary = match result {
            Ok(value) => value,
            Err(error) => panic!("test decryption failed: {error}"),
        };
        assert_eq!(plaintext, b"bounded frame payload");
        assert_eq!(
            summary.plaintext_blake3,
            *blake3::hash(&plaintext).as_bytes()
        );
        assert_eq!(summary.encoded_blake3, *blake3::hash(&encoded).as_bytes());
        assert_eq!(summary.header.frame_count, 3);
    }

    #[test]
    fn wrong_key_and_every_authenticated_region_fail() {
        let encoded = encoded_part();
        let wrong_key = decrypt_part(
            &mut Cursor::new(&encoded),
            &mut Vec::new(),
            &FileKey::from_bytes([4; 32]),
            PartLimits::default(),
        );
        assert_eq!(wrong_key, Err(CryptoError::AuthenticationFailed));

        for offset in [16_usize, 32, 56, 100, 112] {
            let mut tampered = encoded.clone();
            tampered[offset] ^= 1;
            assert!(
                decrypt_part(
                    &mut Cursor::new(tampered),
                    &mut Vec::new(),
                    &FileKey::from_bytes([3; 32]),
                    PartLimits::default()
                )
                .is_err(),
                "tamper offset {offset} was accepted"
            );
        }
    }

    #[test]
    fn reorder_duplicate_omission_truncation_and_trailing_data_fail() {
        let encoded = encoded_part();
        let first_record = 16 + 8 + 16;
        let second_record = 16 + 8 + 16;
        let first_start = PART_HEADER_LENGTH;
        let second_start = first_start + first_record;

        let mut reordered = encoded.clone();
        let first = reordered[first_start..first_start + first_record].to_vec();
        let second = reordered[second_start..second_start + second_record].to_vec();
        reordered[first_start..first_start + second_record].copy_from_slice(&second);
        reordered[second_start..second_start + first_record].copy_from_slice(&first);
        assert!(decrypt_bytes(&reordered).is_err());

        let mut duplicated = encoded.clone();
        duplicated[second_start..second_start + first_record].copy_from_slice(&first);
        assert!(decrypt_bytes(&duplicated).is_err());

        let omitted = &encoded[..encoded.len() - 37];
        assert!(matches!(
            decrypt_bytes(omitted),
            Err(CryptoError::Truncated { .. })
        ));
        assert!(matches!(
            decrypt_bytes(&encoded[..encoded.len() - 1]),
            Err(CryptoError::Truncated { .. })
        ));

        let mut trailing = encoded;
        trailing.push(0);
        assert!(matches!(
            decrypt_bytes(&trailing),
            Err(CryptoError::TrailingData { .. })
        ));
    }

    fn decrypt_bytes(bytes: &[u8]) -> Result<PlaintextPartSummary, CryptoError> {
        decrypt_part(
            &mut Cursor::new(bytes),
            &mut Vec::new(),
            &FileKey::from_bytes([3; 32]),
            PartLimits::default(),
        )
    }

    #[test]
    fn invalid_final_flag_and_oversized_claims_fail_before_allocation() {
        let mut encoded = encoded_part();
        encoded[PART_HEADER_LENGTH + 12] = FINAL_FRAME_FLAG;
        assert!(matches!(
            decrypt_bytes(&encoded),
            Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::InvalidFinalFrame
            })
        ));

        let mut header = header().encode();
        header[80..84].copy_from_slice(&(32 * 1024 * 1024_u32).to_be_bytes());
        assert!(PartHeader::decode(&header, PartLimits::default()).is_err());

        assert!(
            PartHeader::new(
                [0; 16],
                PartInstanceId([1; 16]),
                0,
                1,
                u64::MAX,
                u64::MAX,
                1,
                8,
                PartLimits::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn empty_container_has_one_authenticated_final_frame() {
        let header = match PartHeader::new(
            [1; 16],
            PartInstanceId([2; 16]),
            0,
            1,
            0,
            0,
            0,
            1024,
            PartLimits::default(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("empty header rejected: {error}"),
        };
        let mut encoded = Vec::new();
        assert!(
            encrypt_part(
                &mut Cursor::new(Vec::<u8>::new()),
                &mut encoded,
                &header,
                &FileKey::from_bytes([3; 32]),
                PartLimits::default(),
                &mut AeadUsageRegistry::new()
            )
            .is_ok()
        );
        assert!(decrypt_bytes(&encoded).is_ok());
    }

    #[test]
    fn nonce_layout_is_unique_over_broad_index_ranges() {
        let mut nonces = BTreeSet::new();
        for part in 0..64 {
            for frame in 0..4096 {
                assert!(nonces.insert(frame_nonce(part, frame)));
            }
        }
        assert_eq!(nonces.len(), 64 * 4096);
        assert_eq!(
            frame_nonce(0x0102_0304, 0x0506_0708),
            [1, 2, 3, 4, 0, 0, 0, 0, 5, 6, 7, 8]
        );
    }

    #[test]
    fn registry_rejects_duplicate_instance_identity() {
        let mut registry = PartInstanceRegistry::default();
        let id = PartInstanceId([7; 16]);
        assert!(registry.register(id).is_ok());
        assert!(matches!(
            registry.register(id),
            Err(CryptoError::InvalidLayout {
                violation: LayoutViolation::DuplicatePartInstance
            })
        ));
    }

    #[test]
    fn encoder_rejects_reusing_a_content_key_identity() {
        let mut usage = AeadUsageRegistry::new();
        let header = header();
        let file_key = FileKey::from_bytes([3; 32]);
        assert!(
            encrypt_part(
                &mut Cursor::new(b"bounded frame payload"),
                &mut Vec::new(),
                &header,
                &file_key,
                PartLimits::default(),
                &mut usage,
            )
            .is_ok()
        );
        assert_eq!(
            encrypt_part(
                &mut Cursor::new(b"changed frame payload"),
                &mut Vec::new(),
                &header,
                &file_key,
                PartLimits::default(),
                &mut usage,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
        assert_eq!(
            encrypt_part(
                &mut Cursor::new(b"bounded frame payload"),
                &mut Vec::new(),
                &header,
                &FileKey::from_bytes([4; 32]),
                PartLimits::default(),
                &mut usage,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );

        let mut restored = AeadUsageRegistry::new();
        assert!(
            restored
                .reserve_existing_part(
                    &file_key,
                    &header.package_id,
                    header.part_index,
                    header.part_instance_id,
                )
                .is_ok()
        );
        assert_eq!(
            encrypt_part(
                &mut Cursor::new(b"bounded frame payload"),
                &mut Vec::new(),
                &header,
                &file_key,
                PartLimits::default(),
                &mut restored,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
    }

    #[test]
    fn hostile_mutation_and_truncation_corpus_never_panics() {
        let encoded = encoded_part();
        for length in 0..encoded.len() {
            let _ = decrypt_bytes(&encoded[..length]);
        }
        for index in 0..encoded.len() {
            let mut mutated = encoded.clone();
            mutated[index] ^= 0x80;
            let _ = decrypt_bytes(&mutated);
        }
    }

    #[test]
    fn candidate_vector_matches_fixture() {
        let encoded = encoded_part();
        let fixture = include_str!("../tests/vectors/crypto_v1/part_multiframe.txt");
        assert_eq!(hex(&encoded), fixture_value(fixture, "encoded_hex"));
        assert_eq!(
            hex(blake3::hash(&encoded).as_bytes()),
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
