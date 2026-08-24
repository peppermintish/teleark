use std::collections::BTreeSet;
use std::fmt;

use crate::{
    AccountId, ChatId, DomainValidationError, LogicalFileId, MessageId, PackageId, PartIndex,
    RemoteObjectId,
};

/// A canonical BLAKE3 digest kept as bytes rather than localized text.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Blake3Digest([u8; Self::LENGTH]);

impl Blake3Digest {
    pub const LENGTH: usize = 32;

    pub const fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }
}

impl TryFrom<&[u8]> for Blake3Digest {
    type Error = DomainValidationError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes: [u8; Self::LENGTH] =
            value
                .try_into()
                .map_err(|_| DomainValidationError::InvalidDigestLength {
                    actual: value.len(),
                })?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for Blake3Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum FileKind {
    Video,
    Document,
    Archive,
    Audio,
    Image,
    DiskImage,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum RemoteState {
    LocalOnly,
    Uploading,
    Uploaded,
    RemoteMissing,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum EncryptionState {
    Unencrypted,
    Encrypted,
    Locked,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum VerificationState {
    Unverified,
    Verifying,
    Verified,
    Failed,
}

/// The file-centric object presented by every TeleArk frontend.
///
/// Telegram messages and multipart pieces are implementation details attached
/// through [`RemoteObject`] and [`Package`]. User/source text in `name` is kept
/// verbatim and must never be translated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalFile {
    pub id: LogicalFileId,
    pub name: String,
    pub size_bytes: u64,
    pub kind: FileKind,
    pub source_account_id: Option<AccountId>,
    pub source_chat_id: Option<ChatId>,
    pub modified_at_unix_ms: Option<i64>,
    pub remote_state: RemoteState,
    pub encryption_state: EncryptionState,
    pub verification_state: VerificationState,
    pub package_id: Option<PackageId>,
}

impl LogicalFile {
    pub fn new(
        id: LogicalFileId,
        name: impl Into<String>,
        size_bytes: u64,
        kind: FileKind,
    ) -> Result<Self, DomainValidationError> {
        let file = Self {
            id,
            name: name.into(),
            size_bytes,
            kind,
            source_account_id: None,
            source_chat_id: None,
            modified_at_unix_ms: None,
            remote_state: RemoteState::LocalOnly,
            encryption_state: EncryptionState::Unencrypted,
            verification_state: VerificationState::Unverified,
            package_id: None,
        };
        file.validate()?;
        Ok(file)
    }

    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if self.name.trim().is_empty() {
            return Err(DomainValidationError::EmptyFileName);
        }
        Ok(())
    }

    /// Returns the filename extension without a leading dot.
    pub fn extension(&self) -> Option<&str> {
        let (_, extension) = self.name.rsplit_once('.')?;
        (!extension.is_empty()).then_some(extension)
    }
}

/// Fully scoped Telegram identity for one remote object.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RemoteLocator {
    pub account_id: AccountId,
    pub chat_id: ChatId,
    pub message_id: MessageId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum RemoteObjectRole {
    NativeFile,
    Manifest {
        package_id: PackageId,
    },
    ApplicationPart {
        package_id: PackageId,
        index: PartIndex,
    },
}

/// A Telegram-backed object. It is not itself a user-visible library item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteObject {
    pub id: RemoteObjectId,
    pub logical_file_id: LogicalFileId,
    pub locator: RemoteLocator,
    pub role: RemoteObjectRole,
    pub size_bytes: u64,
    pub content_blake3: Option<Blake3Digest>,
}

/// One contiguous application-level part of a multipart package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilePart {
    pub package_id: PackageId,
    pub index: PartIndex,
    pub plaintext_offset_bytes: u64,
    pub plaintext_size_bytes: u64,
    pub ciphertext_size_bytes: u64,
    pub remote_object_id: RemoteObjectId,
    pub plaintext_blake3: Option<Blake3Digest>,
    pub ciphertext_blake3: Option<Blake3Digest>,
}

impl FilePart {
    pub fn new(
        package_id: PackageId,
        index: PartIndex,
        plaintext_offset_bytes: u64,
        plaintext_size_bytes: u64,
        ciphertext_size_bytes: u64,
        remote_object_id: RemoteObjectId,
    ) -> Result<Self, DomainValidationError> {
        if plaintext_size_bytes == 0 {
            return Err(DomainValidationError::ZeroSizedPart { index });
        }
        plaintext_offset_bytes
            .checked_add(plaintext_size_bytes)
            .ok_or(DomainValidationError::ArithmeticOverflow)?;

        Ok(Self {
            package_id,
            index,
            plaintext_offset_bytes,
            plaintext_size_bytes,
            ciphertext_size_bytes,
            remote_object_id,
            plaintext_blake3: None,
            ciphertext_blake3: None,
        })
    }

    pub fn plaintext_end_exclusive(&self) -> Result<u64, DomainValidationError> {
        self.plaintext_offset_bytes
            .checked_add(self.plaintext_size_bytes)
            .ok_or(DomainValidationError::ArithmeticOverflow)
    }
}

/// One authoritative manifest plus the ordered parts of a logical file.
///
/// Construction validates that indices begin at zero, offsets are gap-free and
/// non-overlapping, remote objects are unique, and the plaintext total equals
/// the logical file size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Package {
    id: PackageId,
    logical_file_id: LogicalFileId,
    manifest_remote_object_id: RemoteObjectId,
    format_version: u32,
    parts: Vec<FilePart>,
}

impl Package {
    pub fn try_new(
        id: PackageId,
        logical_file: &LogicalFile,
        manifest_remote_object_id: RemoteObjectId,
        format_version: u32,
        parts: Vec<FilePart>,
    ) -> Result<Self, DomainValidationError> {
        let package = Self {
            id,
            logical_file_id: logical_file.id,
            manifest_remote_object_id,
            format_version,
            parts,
        };
        package.validate_for(logical_file)?;
        Ok(package)
    }

    pub const fn id(&self) -> PackageId {
        self.id
    }

    pub const fn logical_file_id(&self) -> LogicalFileId {
        self.logical_file_id
    }

    pub const fn manifest_remote_object_id(&self) -> RemoteObjectId {
        self.manifest_remote_object_id
    }

    pub const fn format_version(&self) -> u32 {
        self.format_version
    }

    pub fn parts(&self) -> &[FilePart] {
        &self.parts
    }

    pub fn validate_for(&self, logical_file: &LogicalFile) -> Result<(), DomainValidationError> {
        if self.logical_file_id != logical_file.id {
            return Err(DomainValidationError::LogicalFileMismatch {
                expected: logical_file.id,
                actual: self.logical_file_id,
            });
        }
        if self.parts.is_empty() {
            return Err(DomainValidationError::EmptyPackage {
                package_id: self.id,
            });
        }

        let mut expected_offset_bytes = 0_u64;
        let mut remote_objects = BTreeSet::from([self.manifest_remote_object_id]);

        for (position, part) in self.parts.iter().enumerate() {
            let expected_index = PartIndex::new(
                u32::try_from(position).map_err(|_| DomainValidationError::ArithmeticOverflow)?,
            );
            if part.package_id != self.id {
                return Err(DomainValidationError::PartPackageMismatch {
                    index: part.index,
                    expected: self.id,
                    actual: part.package_id,
                });
            }
            if part.index != expected_index {
                return Err(DomainValidationError::NonContiguousPartIndex {
                    expected: expected_index,
                    actual: part.index,
                });
            }
            if part.plaintext_size_bytes == 0 {
                return Err(DomainValidationError::ZeroSizedPart { index: part.index });
            }
            if part.plaintext_offset_bytes != expected_offset_bytes {
                return Err(DomainValidationError::PartOffsetMismatch {
                    index: part.index,
                    expected_offset_bytes,
                    actual_offset_bytes: part.plaintext_offset_bytes,
                });
            }
            if !remote_objects.insert(part.remote_object_id) {
                return Err(DomainValidationError::DuplicateRemoteObject {
                    remote_object_id: part.remote_object_id,
                });
            }
            expected_offset_bytes = part.plaintext_end_exclusive()?;
        }

        if expected_offset_bytes != logical_file.size_bytes {
            return Err(DomainValidationError::PartTotalMismatch {
                expected_bytes: logical_file.size_bytes,
                actual_bytes: expected_offset_bytes,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(size_bytes: u64) -> LogicalFile {
        LogicalFile::new(
            LogicalFileId::new(11),
            "archive.mkv",
            size_bytes,
            FileKind::Video,
        )
        .unwrap()
    }

    fn part(index: u32, offset: u64, size: u64) -> FilePart {
        FilePart::new(
            PackageId::new(2),
            PartIndex::new(index),
            offset,
            size,
            size + 16,
            RemoteObjectId::new(u64::from(index) + 100),
        )
        .unwrap()
    }

    #[test]
    fn valid_package_has_contiguous_gap_free_parts() {
        let logical_file = file(30);
        let package = Package::try_new(
            PackageId::new(2),
            &logical_file,
            RemoteObjectId::new(90),
            1,
            vec![part(0, 0, 10), part(1, 10, 20)],
        )
        .unwrap();

        assert_eq!(package.parts().len(), 2);
        assert_eq!(package.parts()[1].plaintext_end_exclusive().unwrap(), 30);
    }

    #[test]
    fn package_rejects_a_gap_even_when_total_part_sizes_match() {
        let logical_file = file(30);
        let error = Package::try_new(
            PackageId::new(2),
            &logical_file,
            RemoteObjectId::new(90),
            1,
            vec![part(0, 0, 10), part(1, 11, 20)],
        )
        .unwrap_err();

        assert_eq!(
            error,
            DomainValidationError::PartOffsetMismatch {
                index: PartIndex::new(1),
                expected_offset_bytes: 10,
                actual_offset_bytes: 11,
            }
        );
    }

    #[test]
    fn package_rejects_non_contiguous_indices() {
        let logical_file = file(30);
        let error = Package::try_new(
            PackageId::new(2),
            &logical_file,
            RemoteObjectId::new(90),
            1,
            vec![part(0, 0, 10), part(2, 10, 20)],
        )
        .unwrap_err();

        assert_eq!(
            error,
            DomainValidationError::NonContiguousPartIndex {
                expected: PartIndex::new(1),
                actual: PartIndex::new(2),
            }
        );
    }

    #[test]
    fn package_rejects_a_part_reusing_the_manifest_remote_object() {
        let logical_file = file(10);
        let mut file_part = part(0, 0, 10);
        file_part.remote_object_id = RemoteObjectId::new(90);

        let error = Package::try_new(
            PackageId::new(2),
            &logical_file,
            RemoteObjectId::new(90),
            1,
            vec![file_part],
        )
        .unwrap_err();

        assert_eq!(
            error,
            DomainValidationError::DuplicateRemoteObject {
                remote_object_id: RemoteObjectId::new(90),
            }
        );
    }

    #[test]
    fn package_rejects_a_part_owned_by_another_package() {
        let logical_file = file(10);
        let mut file_part = part(0, 0, 10);
        file_part.package_id = PackageId::new(99);

        let error = Package::try_new(
            PackageId::new(2),
            &logical_file,
            RemoteObjectId::new(90),
            1,
            vec![file_part],
        )
        .unwrap_err();

        assert_eq!(
            error,
            DomainValidationError::PartPackageMismatch {
                index: PartIndex::new(0),
                expected: PackageId::new(2),
                actual: PackageId::new(99),
            }
        );
    }

    #[test]
    fn digest_text_is_canonical_and_not_locale_dependent() {
        let digest = Blake3Digest::from_bytes([0xab; Blake3Digest::LENGTH]);
        assert_eq!(digest.to_string(), "ab".repeat(Blake3Digest::LENGTH));
        assert_eq!(
            Blake3Digest::try_from(&[0_u8; 31][..]).unwrap_err(),
            DomainValidationError::InvalidDigestLength { actual: 31 }
        );
    }
}
