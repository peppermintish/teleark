use std::fmt;
use std::io;

/// Durable format involved in an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatKind {
    Part,
    Manifest,
    PasswordWrap,
    RecoveryWrap,
}

/// Locale-neutral layout failure classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutViolation {
    EmptyName,
    UnsafeRelativePath,
    InvalidPartCount,
    InvalidPartIndex,
    InvalidFrameCount,
    InvalidFrameLength,
    InvalidFinalFrame,
    Gap,
    Overlap,
    OutOfRange,
    RemoteNameMismatch,
    DuplicatePartInstance,
    InconsistentField,
}

/// Structured crypto/codec failure. It never contains secret material.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CryptoError {
    Cancelled,
    AuthenticationFailed,
    AeadIdentityAlreadyUsed,
    InvalidMagic {
        format: FormatKind,
    },
    UnsupportedVersion {
        format: FormatKind,
        major: u16,
        minor: u16,
    },
    UnsupportedSuite {
        suite_id: u16,
    },
    UnsupportedNonceStrategy {
        strategy_id: u16,
    },
    UnsupportedAlgorithm {
        algorithm_id: u16,
    },
    UnknownFlags {
        field: &'static str,
        flags: u32,
    },
    InvalidField {
        field: &'static str,
    },
    InvalidLayout {
        violation: LayoutViolation,
    },
    LimitExceeded {
        field: &'static str,
        limit: u64,
        actual: u64,
    },
    ArithmeticOverflow {
        field: &'static str,
    },
    Truncated {
        context: &'static str,
    },
    TrailingData {
        context: &'static str,
    },
    NonCanonicalCbor,
    DuplicateOrUnorderedMapKey,
    InvalidUtf8 {
        field: &'static str,
    },
    RandomSourceFailed,
    PasswordParametersRejected,
    Io {
        operation: &'static str,
        kind: io::ErrorKind,
    },
}

impl CryptoError {
    pub(crate) fn io(operation: &'static str, error: &io::Error) -> Self {
        Self::Io {
            operation,
            kind: error.kind(),
        }
    }
}

impl fmt::Display for CryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("cryptographic operation cancelled"),
            Self::AuthenticationFailed => formatter.write_str("authentication failed"),
            Self::AeadIdentityAlreadyUsed => {
                formatter.write_str("AEAD key/nonce identity was already consumed")
            }
            Self::InvalidMagic { format } => write!(formatter, "invalid {format:?} magic"),
            Self::UnsupportedVersion {
                format,
                major,
                minor,
            } => write!(
                formatter,
                "unsupported {format:?} format version {major}.{minor}"
            ),
            Self::UnsupportedSuite { suite_id } => {
                write!(formatter, "unsupported cryptographic suite {suite_id}")
            }
            Self::UnsupportedNonceStrategy { strategy_id } => {
                write!(formatter, "unsupported nonce strategy {strategy_id}")
            }
            Self::UnsupportedAlgorithm { algorithm_id } => {
                write!(formatter, "unsupported algorithm {algorithm_id}")
            }
            Self::UnknownFlags { field, flags } => {
                write!(formatter, "unknown flags in {field}: {flags:#x}")
            }
            Self::InvalidField { field } => write!(formatter, "invalid field: {field}"),
            Self::InvalidLayout { violation } => {
                write!(formatter, "invalid layout: {violation:?}")
            }
            Self::LimitExceeded {
                field,
                limit,
                actual,
            } => write!(
                formatter,
                "limit exceeded for {field}: {actual} is greater than {limit}"
            ),
            Self::ArithmeticOverflow { field } => {
                write!(formatter, "arithmetic overflow in {field}")
            }
            Self::Truncated { context } => write!(formatter, "truncated {context}"),
            Self::TrailingData { context } => write!(formatter, "trailing data after {context}"),
            Self::NonCanonicalCbor => formatter.write_str("non-canonical CBOR"),
            Self::DuplicateOrUnorderedMapKey => {
                formatter.write_str("duplicate or unordered CBOR map key")
            }
            Self::InvalidUtf8 { field } => write!(formatter, "invalid UTF-8 in {field}"),
            Self::RandomSourceFailed => formatter.write_str("secure random source failed"),
            Self::PasswordParametersRejected => {
                formatter.write_str("password derivation parameters rejected")
            }
            Self::Io { operation, kind } => {
                write!(formatter, "I/O failure during {operation}: {kind:?}")
            }
        }
    }
}

impl std::error::Error for CryptoError {}
