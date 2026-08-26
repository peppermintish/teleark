//! Frontend-neutral, resumable history indexing coordination for TeleArk.
//!
//! This crate owns orchestration only. Telegram-specific values remain behind
//! [`HistorySource`], while SQL and transaction details remain behind
//! [`IndexRepository`]. The coordinator never depends on a GUI runtime.

mod control;
mod coordinator;
mod error;
mod model;
mod ports;

pub use control::{ControlState, ControlToken};
pub use coordinator::IndexCoordinator;
pub use error::{
    ExecutionFailure, HistoryError, IndexError, JobOperation, ProtocolError, RepositoryError,
    RetryAdvice, RetryScope, ValidationError,
};
pub use model::{
    BatchCommitDisposition, BatchCommitReceipt, BatchId, BatchSize, CONTENT_POLICY_VERSION,
    ContentPolicy, DEFAULT_BATCH_SIZE, HistoryContent, HistoryCursor, HistoryMedia, HistoryPage,
    HistoryPageRequest, HistoryRecord, IndexBatch, IndexCheckpoint, IndexJobRecord, IndexRequest,
    IndexStatistics, IndexedContent, IndexedRecord, MAX_BATCH_SIZE, MAX_CURSOR_BYTES,
    MAX_PAGE_METADATA_BYTES, MAX_REMOTE_KEY_BYTES, MessageRange, ProgressSnapshot, RangeEvidence,
    RemoteMediaKey, RunOutcome, SourceKey,
};
pub use ports::{HistorySource, IndexRepository, ProgressSink};

#[cfg(test)]
mod tests;
