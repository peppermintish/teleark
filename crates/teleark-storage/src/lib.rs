//! SQLite persistence adapter for TeleArk.
//!
//! [`Database`] owns one SQLite connection and is intentionally thread-confined.
//! Callers should place it behind a dedicated storage worker rather than a
//! process-wide mutex or the GUI thread. All public records are frontend-neutral
//! and contain no `rusqlite` types.

mod database;
mod error;
mod migration;
mod model;

pub use database::{Database, LATEST_SCHEMA_VERSION};
pub use error::{
    CursorError, EntityKind, InputReason, InvariantViolation, StorageError, StorageResult,
};
pub use model::{
    AccountRecord, ChatRecord, CollectionKind, CollectionRecord, FileSearchFacets, IndexBatch,
    IndexJobRecord, IndexRangeRecord, LibraryStatisticsRecord, LogicalFileRecord,
    NewLogicalFileRecord, PageCursor, RemoteFileUpsert, RemoteObjectRecord, SearchPage,
    SearchQuery, SettingRecord, StoredIndexCoverage, StoredIndexJobState, StoredPartState,
    StoredTransferDirection, StoredTransferState, TelegramIndexStateRecord, TransferPartCheckpoint,
    TransferTaskRecord,
};
