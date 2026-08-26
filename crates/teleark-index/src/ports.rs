use std::{future::Future, future::ready};

use teleark_core::IndexJobId;

use crate::{
    BatchCommitReceipt, HistoryError, HistoryPage, HistoryPageRequest, IndexBatch, IndexJobRecord,
    ProgressSnapshot, RepositoryError,
};

/// Project-owned port for one bounded page of normalized history.
///
/// Implementations must honor `request.limit`, scope every record to the
/// requested account/chat/range, and advance an opaque cursor on every
/// non-final page. Transport-specific types must not cross this boundary.
pub trait HistorySource {
    fn fetch_page(
        &mut self,
        request: HistoryPageRequest,
    ) -> impl Future<Output = Result<HistoryPage, HistoryError>> + Send;
}

/// Durable indexing port with compare-and-set state updates.
///
/// `commit_batch` is an atomic, idempotent contract. An implementation must:
///
/// - compare the job revision and checkpoint before applying a new batch;
/// - upsert records by `SourceKey`, retaining the greatest revision;
/// - persist the next checkpoint, statistics, and range evidence together;
/// - remember `BatchId` so an identical replay returns `AlreadyCommitted`;
/// - reject a reused `BatchId` with different contents as a conflict.
///
/// A returned receipt represents committed truth. The coordinator does not
/// queue another fetch until this future and progress publication complete.
pub trait IndexRepository {
    fn create_job(
        &mut self,
        job: IndexJobRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send;

    fn load_job(
        &mut self,
        job_id: IndexJobId,
    ) -> impl Future<Output = Result<Option<IndexJobRecord>, RepositoryError>> + Send;

    fn replace_job(
        &mut self,
        expected_revision: u64,
        job: IndexJobRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send;

    fn commit_batch(
        &mut self,
        batch: IndexBatch,
    ) -> impl Future<Output = Result<BatchCommitReceipt, RepositoryError>> + Send;
}

/// Backpressured progress consumer. One snapshot is awaited before more
/// history is fetched.
pub trait ProgressSink {
    fn publish(&mut self, snapshot: ProgressSnapshot) -> impl Future<Output = ()> + Send;
}

impl<F> ProgressSink for F
where
    F: FnMut(ProgressSnapshot) + Send,
{
    fn publish(&mut self, snapshot: ProgressSnapshot) -> impl Future<Output = ()> + Send {
        self(snapshot);
        ready(())
    }
}
