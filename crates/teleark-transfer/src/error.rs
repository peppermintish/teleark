use std::error::Error;
use std::fmt;

use teleark_core::{
    DomainValidationError, PartIndex, TransferError, TransferId, TransferTransitionError,
};

use crate::{CrashPoint, RemotePartKey};

/// Invalid bounded-engine configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigurationError {
    ZeroLimit {
        field: &'static str,
    },
    LimitExceedsGlobal {
        field: &'static str,
        limit: usize,
        global: usize,
    },
    InvalidRetryPolicy {
        field: &'static str,
    },
    InvalidProgressPolicy {
        field: &'static str,
    },
    InvalidAdaptivePolicy {
        field: &'static str,
    },
    InvalidPipelinePolicy {
        field: &'static str,
    },
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLimit { field } => write!(formatter, "{field} must be greater than zero"),
            Self::LimitExceedsGlobal {
                field,
                limit,
                global,
            } => write!(
                formatter,
                "{field} limit {limit} exceeds global worker limit {global}"
            ),
            Self::InvalidRetryPolicy { field } => {
                write!(formatter, "invalid retry policy field: {field}")
            }
            Self::InvalidProgressPolicy { field } => {
                write!(formatter, "invalid progress policy field: {field}")
            }
            Self::InvalidAdaptivePolicy { field } => {
                write!(formatter, "invalid adaptive controller field: {field}")
            }
            Self::InvalidPipelinePolicy { field } => {
                write!(formatter, "invalid encryption pipeline field: {field}")
            }
        }
    }
}

impl Error for ConfigurationError {}

/// Structured, locale-neutral transfer orchestration failure.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransferEngineError {
    Configuration(ConfigurationError),
    Domain(DomainValidationError),
    Transition(TransferTransitionError),
    Transfer(TransferError),
    QueueFull {
        limit: usize,
    },
    DuplicateWork {
        transfer_id: TransferId,
        part_index: PartIndex,
    },
    UnknownTransfer {
        transfer_id: TransferId,
    },
    WrongDirection {
        transfer_id: TransferId,
    },
    InvalidPlan {
        field: &'static str,
    },
    RemoteConflict {
        key: RemotePartKey,
        object_count: usize,
    },
    EventBackpressure {
        capacity: usize,
    },
    InjectedCrash {
        point: CrashPoint,
    },
    ArithmeticOverflow {
        field: &'static str,
    },
}

impl fmt::Display for TransferEngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => error.fmt(formatter),
            Self::Domain(error) => error.fmt(formatter),
            Self::Transition(error) => error.fmt(formatter),
            Self::Transfer(error) => error.fmt(formatter),
            Self::QueueFull { limit } => write!(formatter, "transfer queue limit {limit} reached"),
            Self::DuplicateWork {
                transfer_id,
                part_index,
            } => write!(
                formatter,
                "transfer {transfer_id} part {part_index} is already scheduled"
            ),
            Self::UnknownTransfer { transfer_id } => {
                write!(
                    formatter,
                    "transfer {transfer_id} is not owned by this engine"
                )
            }
            Self::WrongDirection { transfer_id } => {
                write!(formatter, "transfer {transfer_id} has the wrong direction")
            }
            Self::InvalidPlan { field } => write!(formatter, "invalid transfer plan: {field}"),
            Self::RemoteConflict { key, object_count } => write!(
                formatter,
                "remote identity {key:?} resolves to {object_count} objects"
            ),
            Self::EventBackpressure { capacity } => write!(
                formatter,
                "significant transfer event cannot fit bounded capacity {capacity}"
            ),
            Self::InjectedCrash { point } => write!(formatter, "injected crash at {point:?}"),
            Self::ArithmeticOverflow { field } => {
                write!(formatter, "arithmetic overflow in {field}")
            }
        }
    }
}

impl Error for TransferEngineError {}

impl From<ConfigurationError> for TransferEngineError {
    fn from(value: ConfigurationError) -> Self {
        Self::Configuration(value)
    }
}

impl From<DomainValidationError> for TransferEngineError {
    fn from(value: DomainValidationError) -> Self {
        Self::Domain(value)
    }
}

impl From<TransferTransitionError> for TransferEngineError {
    fn from(value: TransferTransitionError) -> Self {
        Self::Transition(value)
    }
}

impl From<TransferError> for TransferEngineError {
    fn from(value: TransferError) -> Self {
        Self::Transfer(value)
    }
}
