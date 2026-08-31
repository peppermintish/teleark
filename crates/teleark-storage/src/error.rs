use std::error::Error;
use std::fmt;

/// Result type returned by the storage adapter.
pub type StorageResult<T> = Result<T, StorageError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EntityKind {
    Account,
    Chat,
    LogicalFile,
    Collection,
    TransferTask,
    NativeDownload,
    IndexJob,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InputReason {
    Empty,
    TooLong,
    OutOfRange,
    InvalidCombination,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CursorError {
    Malformed,
    UnsupportedVersion { version: u32 },
    QueryMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InvariantViolation {
    IncompleteSourceIdentity,
    TransferPartIndices,
    TransferPartOffsets,
    TransferPartTotal,
    TransferProgress,
    IndexCheckpointOutsideRange,
    OverlappingIndexRange,
    RemoteRevisionConflict,
    SmartCollectionMembership,
}

/// Locale-neutral failures at the SQLite adapter boundary.
#[derive(Debug)]
#[non_exhaustive]
pub enum StorageError {
    Sqlite(rusqlite::Error),
    UnsupportedSchema {
        found: u32,
        latest: u32,
    },
    WrongApplication {
        found: u32,
    },
    InvalidInput {
        field: &'static str,
        reason: InputReason,
    },
    NotFound {
        entity: EntityKind,
        id: i64,
    },
    InvalidCursor(CursorError),
    Invariant(InvariantViolation),
    CorruptData {
        entity: &'static str,
        field: &'static str,
        value: String,
    },
    Domain(teleark_core::DomainValidationError),
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "SQLite operation failed: {error}"),
            Self::UnsupportedSchema { found, latest } => write!(
                formatter,
                "database schema version {found} is newer than supported version {latest}"
            ),
            Self::WrongApplication { found } => {
                write!(
                    formatter,
                    "database application identifier {found:#x} is not TeleArk"
                )
            }
            Self::InvalidInput { field, reason } => {
                write!(formatter, "invalid storage input for {field}: {reason:?}")
            }
            Self::NotFound { entity, id } => write!(formatter, "{entity:?} {id} was not found"),
            Self::InvalidCursor(error) => write!(formatter, "invalid search cursor: {error:?}"),
            Self::Invariant(error) => write!(formatter, "storage invariant failed: {error:?}"),
            Self::CorruptData {
                entity,
                field,
                value,
            } => write!(
                formatter,
                "stored {entity} contains unsupported {field} value {value:?}"
            ),
            Self::Domain(error) => write!(formatter, "domain validation failed: {error}"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Domain(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

impl From<teleark_core::DomainValidationError> for StorageError {
    fn from(value: teleark_core::DomainValidationError) -> Self {
        Self::Domain(value)
    }
}
