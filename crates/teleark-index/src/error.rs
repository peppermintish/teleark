use std::{error::Error, fmt, time::Duration};

use teleark_core::{AccountId, IndexJobId, IndexJobState, IndexTransitionError};

use crate::SourceKey;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryScope {
    Job(IndexJobId),
    Account(AccountId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryAdvice {
    pub retryable: bool,
    pub retry_after: Option<Duration>,
    pub scope: RetryScope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HistoryError {
    Network { retry_after: Option<Duration> },
    FloodWait { retry_after: Duration },
    Authorization,
    AccessDenied,
    SourceMissing,
    Cancelled,
}

impl HistoryError {
    pub const fn retry_advice(&self, job_id: IndexJobId, account_id: AccountId) -> RetryAdvice {
        match self {
            Self::Network { retry_after } => RetryAdvice {
                retryable: true,
                retry_after: *retry_after,
                scope: RetryScope::Job(job_id),
            },
            Self::FloodWait { retry_after } => RetryAdvice {
                retryable: true,
                retry_after: Some(*retry_after),
                scope: RetryScope::Account(account_id),
            },
            Self::Authorization | Self::AccessDenied | Self::SourceMissing | Self::Cancelled => {
                RetryAdvice {
                    retryable: false,
                    retry_after: None,
                    scope: RetryScope::Job(job_id),
                }
            }
        }
    }
}

impl fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network { .. } => formatter.write_str("history source network failure"),
            Self::FloodWait { retry_after } => write!(
                formatter,
                "history source requested a {} second flood wait",
                retry_after.as_secs()
            ),
            Self::Authorization => formatter.write_str("history source authorization failure"),
            Self::AccessDenied => formatter.write_str("history source access denied"),
            Self::SourceMissing => formatter.write_str("history source is missing"),
            Self::Cancelled => formatter.write_str("history source operation was cancelled"),
        }
    }
}

impl Error for HistoryError {}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RepositoryError {
    Busy { retry_after: Option<Duration> },
    Unavailable { retry_after: Option<Duration> },
    Conflict,
    Corrupt,
    Rejected,
}

impl RepositoryError {
    pub const fn retry_advice(&self, job_id: IndexJobId) -> RetryAdvice {
        match self {
            Self::Busy { retry_after } | Self::Unavailable { retry_after } => RetryAdvice {
                retryable: true,
                retry_after: *retry_after,
                scope: RetryScope::Job(job_id),
            },
            Self::Conflict | Self::Corrupt | Self::Rejected => RetryAdvice {
                retryable: false,
                retry_after: None,
                scope: RetryScope::Job(job_id),
            },
        }
    }
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy { .. } => formatter.write_str("index repository is busy"),
            Self::Unavailable { .. } => formatter.write_str("index repository is unavailable"),
            Self::Conflict => formatter.write_str("index repository compare-and-set conflict"),
            Self::Corrupt => formatter.write_str("index repository data is corrupt"),
            Self::Rejected => formatter.write_str("index repository rejected the operation"),
        }
    }
}

impl Error for RepositoryError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobOperation {
    Run,
    Retry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ValidationError {
    ZeroJobId,
    EmptyRanges,
    InvalidRange {
        start_message_id: i64,
        end_message_id: i64,
    },
    OverlappingRanges {
        first_start_message_id: i64,
        first_end_message_id: i64,
        second_start_message_id: i64,
        second_end_message_id: i64,
    },
    NoContentSelected,
    InvalidBatchSize {
        requested: usize,
        maximum: usize,
    },
    EmptyCursor,
    CursorTooLarge {
        bytes: usize,
        maximum: usize,
    },
    EmptyRemoteMediaKey,
    RemoteMediaKeyTooLarge {
        bytes: usize,
        maximum: usize,
    },
    CheckpointOutsideRequest {
        next_range_index: usize,
        range_count: usize,
    },
    CursorAfterFinalRange,
    CompletedBeforeFinalRange,
    FailureDoesNotMatchState,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroJobId => formatter.write_str("index job ID must be non-zero"),
            Self::EmptyRanges => formatter.write_str("index request has no ranges"),
            Self::InvalidRange {
                start_message_id,
                end_message_id,
            } => write!(
                formatter,
                "invalid message range {start_message_id}..={end_message_id}"
            ),
            Self::OverlappingRanges { .. } => formatter.write_str("index request ranges overlap"),
            Self::NoContentSelected => formatter.write_str("index policy selects no content"),
            Self::InvalidBatchSize { requested, maximum } => write!(
                formatter,
                "index batch size {requested} is outside 1..={maximum}"
            ),
            Self::EmptyCursor => formatter.write_str("history cursor must not be empty"),
            Self::CursorTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "history cursor has {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::EmptyRemoteMediaKey => formatter.write_str("remote media key must not be empty"),
            Self::RemoteMediaKeyTooLarge { bytes, maximum } => write!(
                formatter,
                "remote media key has {bytes} bytes; maximum is {maximum}"
            ),
            Self::CheckpointOutsideRequest {
                next_range_index,
                range_count,
            } => write!(
                formatter,
                "checkpoint range index {next_range_index} exceeds {range_count} ranges"
            ),
            Self::CursorAfterFinalRange => {
                formatter.write_str("checkpoint after the final range contains a cursor")
            }
            Self::CompletedBeforeFinalRange => {
                formatter.write_str("completed index job has unfinished ranges")
            }
            Self::FailureDoesNotMatchState => {
                formatter.write_str("only a failed job may carry an active failure")
            }
        }
    }
}

impl Error for ValidationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProtocolError {
    TooManyRecords { received: usize, limit: usize },
    MissingNextCursor,
    UnexpectedNextCursor,
    StalledCursor,
    RecordOutOfScope { source: SourceKey },
    ConflictingRevision { source: SourceKey, revision: u64 },
    MetadataLimitExceeded { bytes: usize, maximum: usize },
    InvalidRepositoryReceipt,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyRecords { received, limit } => write!(
                formatter,
                "history source returned {received} records for limit {limit}"
            ),
            Self::MissingNextCursor => {
                formatter.write_str("non-final history page omitted its next cursor")
            }
            Self::UnexpectedNextCursor => {
                formatter.write_str("final history page included a next cursor")
            }
            Self::StalledCursor => formatter.write_str("history source cursor did not advance"),
            Self::RecordOutOfScope { source } => write!(
                formatter,
                "history source returned out-of-scope message {}",
                source.message_id
            ),
            Self::ConflictingRevision { source, revision } => write!(
                formatter,
                "history source returned conflicting revision {revision} for message {}",
                source.message_id
            ),
            Self::MetadataLimitExceeded { bytes, maximum } => write!(
                formatter,
                "history page metadata has {bytes} bytes; maximum is {maximum}"
            ),
            Self::InvalidRepositoryReceipt => {
                formatter.write_str("index repository returned an invalid batch receipt")
            }
        }
    }
}

impl Error for ProtocolError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionFailure {
    History(HistoryError),
    Protocol(ProtocolError),
}

impl ExecutionFailure {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::History(HistoryError::Network { .. }) => "history_network",
            Self::History(HistoryError::FloodWait { .. }) => "history_flood_wait",
            Self::History(HistoryError::Authorization) => "history_authorization",
            Self::History(HistoryError::AccessDenied) => "history_access_denied",
            Self::History(HistoryError::SourceMissing) => "history_source_missing",
            Self::History(HistoryError::Cancelled) => "history_cancelled",
            Self::Protocol(_) => "history_protocol",
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum IndexError {
    Validation(ValidationError),
    JobNotFound {
        job_id: IndexJobId,
    },
    InvalidJobState {
        job_id: IndexJobId,
        state: IndexJobState,
        operation: JobOperation,
    },
    Transition(IndexTransitionError),
    CounterOverflow {
        field: &'static str,
    },
    Execution {
        job_id: IndexJobId,
        account_id: AccountId,
        failure: ExecutionFailure,
        persistence_error: Option<RepositoryError>,
    },
    Repository {
        job_id: IndexJobId,
        error: RepositoryError,
    },
    RepositoryContract {
        job_id: IndexJobId,
        error: ProtocolError,
    },
}

impl IndexError {
    pub const fn retry_advice(&self) -> Option<RetryAdvice> {
        match self {
            Self::Execution {
                job_id,
                account_id,
                failure: ExecutionFailure::History(error),
                persistence_error: None,
            } => Some(error.retry_advice(*job_id, *account_id)),
            Self::Execution {
                job_id,
                persistence_error: Some(error),
                ..
            }
            | Self::Repository { job_id, error } => Some(error.retry_advice(*job_id)),
            Self::Validation(_)
            | Self::JobNotFound { .. }
            | Self::InvalidJobState { .. }
            | Self::Transition(_)
            | Self::CounterOverflow { .. }
            | Self::Execution {
                failure: ExecutionFailure::Protocol(_),
                persistence_error: None,
                ..
            }
            | Self::RepositoryContract { .. } => None,
        }
    }
}

impl fmt::Display for IndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => error.fmt(formatter),
            Self::JobNotFound { job_id } => write!(formatter, "index job {job_id} was not found"),
            Self::InvalidJobState {
                job_id,
                state,
                operation,
            } => write!(
                formatter,
                "index job {job_id} cannot perform {operation:?} from {state:?}"
            ),
            Self::Transition(error) => error.fmt(formatter),
            Self::CounterOverflow { field } => {
                write!(formatter, "index counter {field} overflowed")
            }
            Self::Execution {
                job_id,
                failure,
                persistence_error,
                ..
            } => {
                write!(formatter, "index job {job_id} failed: {failure:?}")?;
                if let Some(error) = persistence_error {
                    write!(formatter, "; failed-state persistence also failed: {error}")?;
                }
                Ok(())
            }
            Self::Repository { job_id, error } => {
                write!(
                    formatter,
                    "index repository failed for job {job_id}: {error}"
                )
            }
            Self::RepositoryContract { job_id, error } => write!(
                formatter,
                "index repository contract failed for job {job_id}: {error}"
            ),
        }
    }
}

impl Error for IndexError {}

impl From<ValidationError> for IndexError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

impl From<IndexTransitionError> for IndexError {
    fn from(value: IndexTransitionError) -> Self {
        Self::Transition(value)
    }
}
