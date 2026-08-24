use std::error::Error;
use std::fmt;
use std::time::Duration;

use crate::{LogicalFileId, PackageId, PartIndex, RemoteObjectId};

/// Validation failures for locale-neutral domain data.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DomainValidationError {
    EmptyFileName,
    EmptyCollectionName,
    EmptyPackage {
        package_id: PackageId,
    },
    InvalidDigestLength {
        actual: usize,
    },
    ZeroSizedPart {
        index: PartIndex,
    },
    ArithmeticOverflow,
    NonContiguousPartIndex {
        expected: PartIndex,
        actual: PartIndex,
    },
    PartOffsetMismatch {
        index: PartIndex,
        expected_offset_bytes: u64,
        actual_offset_bytes: u64,
    },
    PartTotalMismatch {
        expected_bytes: u64,
        actual_bytes: u64,
    },
    LogicalFileMismatch {
        expected: LogicalFileId,
        actual: LogicalFileId,
    },
    PartPackageMismatch {
        index: PartIndex,
        expected: PackageId,
        actual: PackageId,
    },
    DuplicateRemoteObject {
        remote_object_id: RemoteObjectId,
    },
    InvalidIndexRange {
        start_message_id: i64,
        end_message_id: i64,
    },
    CheckpointOutsideRange {
        checkpoint_message_id: i64,
    },
    OverlappingIndexRange {
        existing_start_message_id: i64,
        existing_end_message_id: i64,
        new_start_message_id: i64,
        new_end_message_id: i64,
    },
    InvalidCollectionSizeBounds {
        minimum_bytes: u64,
        maximum_bytes: u64,
    },
}

impl fmt::Display for DomainValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFileName => formatter.write_str("logical file name is empty"),
            Self::EmptyCollectionName => formatter.write_str("collection name is empty"),
            Self::EmptyPackage { package_id } => {
                write!(formatter, "package {package_id} has no file parts")
            }
            Self::InvalidDigestLength { actual } => {
                write!(formatter, "BLAKE3 digest has {actual} bytes instead of 32")
            }
            Self::ZeroSizedPart { index } => write!(formatter, "file part {index} is empty"),
            Self::ArithmeticOverflow => formatter.write_str("domain byte arithmetic overflowed"),
            Self::NonContiguousPartIndex { expected, actual } => write!(
                formatter,
                "expected file part index {expected}, found {actual}"
            ),
            Self::PartOffsetMismatch {
                index,
                expected_offset_bytes,
                actual_offset_bytes,
            } => write!(
                formatter,
                "file part {index} starts at {actual_offset_bytes}, expected {expected_offset_bytes}"
            ),
            Self::PartTotalMismatch {
                expected_bytes,
                actual_bytes,
            } => write!(
                formatter,
                "file parts total {actual_bytes} bytes, expected {expected_bytes}"
            ),
            Self::LogicalFileMismatch { expected, actual } => write!(
                formatter,
                "package references logical file {actual}, expected {expected}"
            ),
            Self::PartPackageMismatch {
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "file part {index} references package {actual}, expected {expected}"
            ),
            Self::DuplicateRemoteObject { remote_object_id } => write!(
                formatter,
                "remote object {remote_object_id} is assigned to more than one file part"
            ),
            Self::InvalidIndexRange {
                start_message_id,
                end_message_id,
            } => write!(
                formatter,
                "index range start {start_message_id} exceeds end {end_message_id}"
            ),
            Self::CheckpointOutsideRange {
                checkpoint_message_id,
            } => write!(
                formatter,
                "index checkpoint {checkpoint_message_id} is outside its range"
            ),
            Self::OverlappingIndexRange {
                existing_start_message_id,
                existing_end_message_id,
                new_start_message_id,
                new_end_message_id,
            } => write!(
                formatter,
                "index range {new_start_message_id}..={new_end_message_id} overlaps {existing_start_message_id}..={existing_end_message_id}"
            ),
            Self::InvalidCollectionSizeBounds {
                minimum_bytes,
                maximum_bytes,
            } => write!(
                formatter,
                "collection minimum size {minimum_bytes} exceeds maximum {maximum_bytes}"
            ),
        }
    }
}

impl Error for DomainValidationError {}

/// Presentation-relevant traits of a structured error.
///
/// These flags are deliberately independent: for example, a missing key both
/// requires user action and belongs to the key-problem family.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ErrorDisposition {
    pub retryable: bool,
    pub requires_user_action: bool,
    pub data_corruption: bool,
    pub key_problem: bool,
}

impl ErrorDisposition {
    const fn none() -> Self {
        Self {
            retryable: false,
            requires_user_action: false,
            data_corruption: false,
            key_problem: false,
        }
    }

    const fn retryable() -> Self {
        Self {
            retryable: true,
            requires_user_action: false,
            data_corruption: false,
            key_problem: false,
        }
    }

    const fn user_action() -> Self {
        Self {
            retryable: false,
            requires_user_action: true,
            data_corruption: false,
            key_problem: false,
        }
    }

    const fn corruption() -> Self {
        Self {
            retryable: false,
            requires_user_action: true,
            data_corruption: true,
            key_problem: false,
        }
    }

    const fn key_problem() -> Self {
        Self {
            retryable: false,
            requires_user_action: true,
            data_corruption: false,
            key_problem: true,
        }
    }
}

/// Stable, locale-neutral failures produced by transfer infrastructure.
///
/// Adapters map their implementation-specific errors into this enum. Frontends
/// must localize variants at the presentation boundary instead of inspecting
/// formatted error text.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransferError {
    Network,
    FloodWait { retry_after: Duration },
    Authorization,
    SourceMissing,
    SourceChanged,
    DiskFull,
    PermissionDenied,
    RemoteMissing,
    HashMismatch,
    AuthenticationFailed,
    ManifestCorrupted,
    UnsupportedManifestVersion { version: u32 },
    KeyUnavailable,
    WrongPassword,
    Database,
    Cancelled,
}

impl TransferError {
    /// Returns machine-readable handling characteristics for this failure.
    pub const fn disposition(&self) -> ErrorDisposition {
        match self {
            Self::Network | Self::FloodWait { .. } | Self::Database => {
                ErrorDisposition::retryable()
            }
            Self::Authorization
            | Self::SourceMissing
            | Self::SourceChanged
            | Self::DiskFull
            | Self::PermissionDenied
            | Self::RemoteMissing => ErrorDisposition::user_action(),
            Self::HashMismatch | Self::ManifestCorrupted => ErrorDisposition::corruption(),
            Self::AuthenticationFailed | Self::KeyUnavailable | Self::WrongPassword => {
                ErrorDisposition::key_problem()
            }
            Self::UnsupportedManifestVersion { .. } | Self::Cancelled => ErrorDisposition::none(),
        }
    }

    pub const fn is_retryable(&self) -> bool {
        self.disposition().retryable
    }
}

impl fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network => formatter.write_str("transfer network failure"),
            Self::FloodWait { retry_after } => write!(
                formatter,
                "Telegram flood wait for {} seconds",
                retry_after.as_secs()
            ),
            Self::Authorization => formatter.write_str("transfer authorization failure"),
            Self::SourceMissing => formatter.write_str("transfer source is missing"),
            Self::SourceChanged => formatter.write_str("transfer source changed"),
            Self::DiskFull => formatter.write_str("destination disk is full"),
            Self::PermissionDenied => formatter.write_str("transfer permission denied"),
            Self::RemoteMissing => formatter.write_str("remote object is missing"),
            Self::HashMismatch => formatter.write_str("transfer hash mismatch"),
            Self::AuthenticationFailed => {
                formatter.write_str("encrypted content authentication failed")
            }
            Self::ManifestCorrupted => formatter.write_str("manifest is corrupted"),
            Self::UnsupportedManifestVersion { version } => {
                write!(formatter, "manifest version {version} is unsupported")
            }
            Self::KeyUnavailable => formatter.write_str("encryption key is unavailable"),
            Self::WrongPassword => formatter.write_str("vault password is incorrect"),
            Self::Database => formatter.write_str("transfer database failure"),
            Self::Cancelled => formatter.write_str("transfer was cancelled"),
        }
    }
}

impl Error for TransferError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_dispositions_support_multiple_independent_traits() {
        let disposition = TransferError::HashMismatch.disposition();
        assert!(disposition.data_corruption);
        assert!(disposition.requires_user_action);
        assert!(!disposition.retryable);

        let disposition = TransferError::KeyUnavailable.disposition();
        assert!(disposition.key_problem);
        assert!(disposition.requires_user_action);
    }

    #[test]
    fn transient_failures_are_retryable_without_string_matching() {
        assert!(TransferError::Network.is_retryable());
        assert!(
            TransferError::FloodWait {
                retry_after: Duration::from_secs(12)
            }
            .is_retryable()
        );
        assert!(!TransferError::WrongPassword.is_retryable());
    }
}
