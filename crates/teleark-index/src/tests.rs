use std::{
    collections::{BTreeMap, btree_map::Entry},
    error::Error,
    future::{Future, ready},
    task::{Context, Poll, Waker},
    time::Duration,
};

use teleark_core::{
    AccountId, ChatId, FileKind, IndexCoverage, IndexJobId, IndexJobState, MessageId,
};

use crate::{
    BatchCommitDisposition, BatchCommitReceipt, BatchId, BatchSize, ContentPolicy, ControlToken,
    ExecutionFailure, HistoryContent, HistoryCursor, HistoryError, HistoryMedia, HistoryPage,
    HistoryPageRequest, HistoryRecord, HistorySource, IndexBatch, IndexCheckpoint,
    IndexCoordinator, IndexError, IndexJobRecord, IndexRepository, IndexRequest, IndexStatistics,
    IndexedContent, IndexedRecord, JobOperation, MAX_BATCH_SIZE, MessageRange, ProgressSnapshot,
    ProtocolError, RangeEvidence, RemoteMediaKey, RepositoryError, RetryScope, RunOutcome,
    SourceKey, ValidationError,
};

type TestResult = Result<(), Box<dyn Error>>;

const ACCOUNT: AccountId = AccountId::new(7);
const CHAT: ChatId = ChatId::new(11);

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn range(start: i64, end: i64) -> Result<MessageRange, ValidationError> {
    MessageRange::try_new(MessageId::new(start), MessageId::new(end))
}

fn request(
    job_id: u64,
    ranges: Vec<MessageRange>,
    policy: ContentPolicy,
) -> Result<IndexRequest, ValidationError> {
    IndexRequest::try_new(IndexJobId::new(job_id), ACCOUNT, CHAT, ranges, policy)
}

fn source_key(message_id: i64) -> SourceKey {
    SourceKey::new(ACCOUNT, CHAT, MessageId::new(message_id))
}

fn media(message_id: i64, revision: u64) -> Result<HistoryRecord, ValidationError> {
    media_with_kind(message_id, revision, FileKind::Document)
}

fn media_with_kind(
    message_id: i64,
    revision: u64,
    kind: FileKind,
) -> Result<HistoryRecord, ValidationError> {
    let key = RemoteMediaKey::try_new(message_id.to_be_bytes().to_vec())?;
    Ok(HistoryRecord::new(
        source_key(message_id),
        revision,
        message_id * 1_000,
        HistoryContent::Media(HistoryMedia::new(
            key,
            format!("file-{message_id}.bin"),
            1_024,
            kind,
        )),
    ))
}

fn plain_text(message_id: i64, text: impl Into<String>) -> HistoryRecord {
    HistoryRecord::new(
        source_key(message_id),
        1,
        message_id * 1_000,
        HistoryContent::PlainText(text.into()),
    )
}

fn cursor_for(offset: usize) -> Result<HistoryCursor, HistoryError> {
    let offset = u64::try_from(offset).map_err(|_| HistoryError::SourceMissing)?;
    HistoryCursor::try_new(offset.to_be_bytes().to_vec()).map_err(|_| HistoryError::SourceMissing)
}

fn cursor_offset(cursor: Option<&HistoryCursor>) -> Result<usize, HistoryError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let bytes: [u8; 8] = cursor
        .as_bytes()
        .try_into()
        .map_err(|_| HistoryError::SourceMissing)?;
    usize::try_from(u64::from_be_bytes(bytes)).map_err(|_| HistoryError::SourceMissing)
}

#[derive(Default)]
struct FakeHistorySource {
    records: Vec<HistoryRecord>,
    requested_limits: Vec<usize>,
    next_error: Option<HistoryError>,
    exceed_first_limit: bool,
}

impl FakeHistorySource {
    fn new(records: Vec<HistoryRecord>) -> Self {
        Self {
            records,
            ..Self::default()
        }
    }
}

impl HistorySource for FakeHistorySource {
    fn fetch_page(
        &mut self,
        request: HistoryPageRequest,
    ) -> impl Future<Output = Result<HistoryPage, HistoryError>> + Send {
        self.requested_limits.push(request.limit.get());
        if let Some(error) = self.next_error.take() {
            return ready(Err(error));
        }

        let result = (|| {
            let offset = cursor_offset(request.cursor.as_ref())?;
            let matching = self
                .records
                .iter()
                .filter(|record| {
                    record.source.account_id == request.account_id
                        && record.source.chat_id == request.chat_id
                        && request.range.contains(record.source.message_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            let extra = usize::from(self.exceed_first_limit && offset == 0);
            let requested = request.limit.get().saturating_add(extra);
            let end = offset.saturating_add(requested).min(matching.len());
            let records = matching.get(offset..end).unwrap_or_default().to_vec();
            let exhausted = end >= matching.len();
            let next_cursor = if exhausted {
                None
            } else {
                Some(cursor_for(end)?)
            };
            Ok(HistoryPage::new(records, next_cursor, exhausted))
        })();
        ready(result)
    }
}

#[derive(Default)]
struct FakeRepository {
    jobs: BTreeMap<IndexJobId, IndexJobRecord>,
    records: BTreeMap<SourceKey, IndexedRecord>,
    ranges: BTreeMap<(IndexJobId, usize), RangeEvidence>,
    committed: BTreeMap<BatchId, (IndexBatch, BatchCommitReceipt)>,
    maximum_batch_records: usize,
}

impl FakeRepository {
    fn checked_add(left: u64, right: u64) -> Result<u64, RepositoryError> {
        left.checked_add(right).ok_or(RepositoryError::Corrupt)
    }

    fn changed_record(&self, incoming: &IndexedRecord) -> Result<bool, RepositoryError> {
        match self.records.get(&incoming.source) {
            None => Ok(true),
            Some(existing) if existing.revision < incoming.revision => Ok(true),
            Some(existing) if existing.revision > incoming.revision => Ok(false),
            Some(existing) if existing == incoming => Ok(false),
            Some(_) => Err(RepositoryError::Conflict),
        }
    }

    fn apply_change(
        statistics: &mut IndexStatistics,
        record: &IndexedRecord,
    ) -> Result<(), RepositoryError> {
        statistics.records_written = Self::checked_add(statistics.records_written, 1)?;
        match &record.content {
            IndexedContent::Media(media) => {
                statistics.files_indexed = Self::checked_add(statistics.files_indexed, 1)?;
                statistics.bytes_indexed =
                    Self::checked_add(statistics.bytes_indexed, media.size_bytes)?;
            }
            IndexedContent::PlainText(_) => {
                statistics.plain_text_indexed =
                    Self::checked_add(statistics.plain_text_indexed, 1)?;
            }
            IndexedContent::Deleted => {
                statistics.deletions_applied = Self::checked_add(statistics.deletions_applied, 1)?;
            }
        }
        Ok(())
    }
}

impl IndexRepository for FakeRepository {
    fn create_job(
        &mut self,
        job: IndexJobRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send {
        let result = match self.jobs.entry(job.id()) {
            Entry::Vacant(entry) => {
                entry.insert(job);
                Ok(())
            }
            Entry::Occupied(_) => Err(RepositoryError::Conflict),
        };
        ready(result)
    }

    fn load_job(
        &mut self,
        job_id: IndexJobId,
    ) -> impl Future<Output = Result<Option<IndexJobRecord>, RepositoryError>> + Send {
        ready(Ok(self.jobs.get(&job_id).cloned()))
    }

    fn replace_job(
        &mut self,
        expected_revision: u64,
        job: IndexJobRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send {
        let result = match self.jobs.get(&job.id()) {
            Some(current)
                if current.revision() == expected_revision
                    && job.revision() == expected_revision.saturating_add(1)
                    && current.request() == job.request() =>
            {
                self.jobs.insert(job.id(), job);
                Ok(())
            }
            _ => Err(RepositoryError::Conflict),
        };
        ready(result)
    }

    fn commit_batch(
        &mut self,
        batch: IndexBatch,
    ) -> impl Future<Output = Result<BatchCommitReceipt, RepositoryError>> + Send {
        let result = (|| {
            self.maximum_batch_records = self.maximum_batch_records.max(batch.records().len());
            if let Some((committed_batch, receipt)) = self.committed.get(&batch.id()) {
                if committed_batch != &batch {
                    return Err(RepositoryError::Conflict);
                }
                return Ok(BatchCommitReceipt::new(
                    BatchCommitDisposition::AlreadyCommitted,
                    receipt.job.clone(),
                    receipt.range.clone(),
                ));
            }

            let current = self
                .jobs
                .get(&batch.id().job_id)
                .cloned()
                .ok_or(RepositoryError::Rejected)?;
            if current.state() != IndexJobState::Running
                || current.revision() != batch.expected_job_revision()
                || current.checkpoint() != batch.expected_checkpoint()
                || current.request() != batch.request()
            {
                return Err(RepositoryError::Conflict);
            }

            let mut statistics = current.statistics().clone();
            statistics.messages_scanned =
                Self::checked_add(statistics.messages_scanned, batch.messages_examined())?;
            statistics.batches_committed = Self::checked_add(statistics.batches_committed, 1)?;

            let range_key = (current.id(), batch.range_index());
            let mut range_statistics = self
                .ranges
                .get(&range_key)
                .map_or_else(IndexStatistics::default, |range| range.statistics.clone());
            range_statistics.messages_scanned =
                Self::checked_add(range_statistics.messages_scanned, batch.messages_examined())?;
            range_statistics.batches_committed =
                Self::checked_add(range_statistics.batches_committed, 1)?;

            for record in batch.records() {
                if self.changed_record(record)? {
                    Self::apply_change(&mut statistics, record)?;
                    Self::apply_change(&mut range_statistics, record)?;
                    self.records.insert(record.source, record.clone());
                }
            }

            let revision = current
                .revision()
                .checked_add(1)
                .ok_or(RepositoryError::Corrupt)?;
            let job = IndexJobRecord::try_restore(
                current.request().clone(),
                IndexJobState::Running,
                batch.next_checkpoint().clone(),
                statistics,
                revision,
                None,
            )
            .map_err(|_| RepositoryError::Rejected)?;
            let evidence = RangeEvidence {
                job_id: current.id(),
                range_index: batch.range_index(),
                range: batch.range(),
                coverage: batch.coverage(),
                cursor: batch.next_checkpoint().cursor().cloned(),
                statistics: range_statistics,
            };
            let receipt = BatchCommitReceipt::new(
                BatchCommitDisposition::Applied,
                job.clone(),
                evidence.clone(),
            );
            self.jobs.insert(job.id(), job);
            self.ranges.insert(range_key, evidence);
            self.committed.insert(batch.id(), (batch, receipt.clone()));
            Ok(receipt)
        })();
        ready(result)
    }
}

fn no_progress(_: ProgressSnapshot) {}

#[test]
fn default_policy_indexes_files_and_tombstones_but_not_plain_text() -> TestResult {
    let mut records = vec![
        HistoryRecord::new(source_key(1), 1, 1_000, HistoryContent::Empty),
        plain_text(2, "not opted in"),
        media(3, 1)?,
        HistoryRecord::new(source_key(4), 2, 4_000, HistoryContent::Deleted),
    ];
    records.push(media_with_kind(5, 1, FileKind::Video)?);
    let source = FakeHistorySource::new(records);
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(8)?);
    let request = request(1, vec![range(1, 10)?], ContentPolicy::default())?;
    block_on(coordinator.create_job(request))?;
    let mut progress = no_progress;
    let outcome =
        block_on(coordinator.run(IndexJobId::new(1), &ControlToken::new(), &mut progress))?;

    assert!(matches!(outcome, RunOutcome::Completed(_)));
    let (_, repository) = coordinator.into_parts();
    assert_eq!(repository.records.len(), 3);
    assert!(repository.records.contains_key(&source_key(3)));
    assert!(repository.records.contains_key(&source_key(4)));
    assert!(!repository.records.contains_key(&source_key(2)));
    Ok(())
}

#[test]
fn plain_text_requires_explicit_opt_in() -> TestResult {
    let source = FakeHistorySource::new(vec![plain_text(1, "indexed text"), media(2, 1)?]);
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(4)?);
    let policy = ContentPolicy::files_only()
        .with_media(false)
        .with_plain_text(true);
    block_on(coordinator.create_job(request(2, vec![range(1, 2)?], policy)?))?;
    let mut progress = no_progress;
    block_on(coordinator.run(IndexJobId::new(2), &ControlToken::new(), &mut progress))?;

    let (_, repository) = coordinator.into_parts();
    assert_eq!(repository.records.len(), 1);
    assert!(matches!(
        repository.records.get(&source_key(1)).map(|item| &item.content),
        Some(IndexedContent::PlainText(text)) if text == "indexed text"
    ));
    Ok(())
}

#[test]
fn duplicate_and_out_of_order_rows_keep_the_newest_revision_once() -> TestResult {
    let old = media(7, 1)?;
    let newest = media(7, 3)?;
    let middle = media(7, 2)?;
    let source = FakeHistorySource::new(vec![
        old,
        newest.clone(),
        middle,
        newest.clone(),
        media(8, 1)?,
    ]);
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(8)?);
    block_on(coordinator.create_job(request(3, vec![range(1, 10)?], ContentPolicy::default())?))?;
    let mut progress = no_progress;
    block_on(coordinator.run(IndexJobId::new(3), &ControlToken::new(), &mut progress))?;

    let (_, repository) = coordinator.into_parts();
    assert_eq!(repository.records.len(), 2);
    assert_eq!(
        repository.records.get(&source_key(7)),
        Some(&IndexedRecord {
            source: newest.source,
            revision: newest.revision,
            modified_at_unix_ms: newest.modified_at_unix_ms,
            content: match newest.content {
                HistoryContent::Media(media) => IndexedContent::Media(media),
                _ => return Err("expected media fixture".into()),
            },
        })
    );
    Ok(())
}

#[test]
fn non_contiguous_ranges_leave_the_gap_unindexed() -> TestResult {
    let records = (1..=100)
        .map(|id| media(id, 1))
        .collect::<Result<Vec<_>, _>>()?;
    let source = FakeHistorySource::new(records);
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(6)?);
    block_on(coordinator.create_job(request(
        4,
        vec![range(91, 100)?, range(1, 10)?],
        ContentPolicy::default(),
    )?))?;
    let mut progress = no_progress;
    block_on(coordinator.run(IndexJobId::new(4), &ControlToken::new(), &mut progress))?;

    let (_, repository) = coordinator.into_parts();
    assert_eq!(repository.records.len(), 20);
    assert!(!repository.records.contains_key(&source_key(50)));
    assert_eq!(repository.ranges.len(), 2);
    assert!(
        repository
            .ranges
            .values()
            .all(|evidence| evidence.coverage == IndexCoverage::Complete)
    );
    Ok(())
}

#[test]
fn ten_thousand_record_job_pauses_and_resumes_without_duplication() -> TestResult {
    let records = (1..=10_000)
        .map(|id| media(id, 1))
        .collect::<Result<Vec<_>, _>>()?;
    let source = FakeHistorySource::new(records);
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(127)?);
    block_on(coordinator.create_job(request(
        5,
        vec![range(1, 10_000)?],
        ContentPolicy::default(),
    )?))?;

    let control = ControlToken::new();
    let pause_handle = control.clone();
    let mut progress = move |snapshot: ProgressSnapshot| {
        if snapshot.state == IndexJobState::Running && snapshot.statistics.batches_committed == 17 {
            pause_handle.request_pause();
        }
    };
    let first = block_on(coordinator.run(IndexJobId::new(5), &control, &mut progress))?;
    assert!(matches!(first, RunOutcome::Paused(_)));
    assert!(control.resume());

    let mut progress = no_progress;
    let second = block_on(coordinator.run(IndexJobId::new(5), &control, &mut progress))?;
    assert!(matches!(second, RunOutcome::Completed(_)));
    let (source, repository) = coordinator.into_parts();
    assert_eq!(repository.records.len(), 10_000);
    assert_eq!(
        repository.jobs[&IndexJobId::new(5)].state(),
        IndexJobState::Completed
    );
    assert!(source.requested_limits.iter().all(|limit| *limit == 127));
    assert!(repository.maximum_batch_records <= 127);
    Ok(())
}

#[test]
fn every_fetch_and_commit_obeys_the_configured_batch_limit() -> TestResult {
    let records = (1..=35)
        .map(|id| media(id, 1))
        .collect::<Result<Vec<_>, _>>()?;
    let source = FakeHistorySource::new(records);
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(7)?);
    block_on(coordinator.create_job(request(6, vec![range(1, 35)?], ContentPolicy::default())?))?;
    let mut progress = no_progress;
    block_on(coordinator.run(IndexJobId::new(6), &ControlToken::new(), &mut progress))?;

    let (source, repository) = coordinator.into_parts();
    assert_eq!(source.requested_limits, vec![7; 5]);
    assert_eq!(repository.maximum_batch_records, 7);
    assert_eq!(
        BatchSize::try_new(0),
        Err(ValidationError::InvalidBatchSize {
            requested: 0,
            maximum: MAX_BATCH_SIZE,
        })
    );
    assert!(BatchSize::try_new(MAX_BATCH_SIZE + 1).is_err());
    Ok(())
}

#[test]
fn oversized_source_page_fails_without_persisting_records() -> TestResult {
    let records = (1..=5)
        .map(|id| media(id, 1))
        .collect::<Result<Vec<_>, _>>()?;
    let mut source = FakeHistorySource::new(records);
    source.exceed_first_limit = true;
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(4)?);
    block_on(coordinator.create_job(request(7, vec![range(1, 5)?], ContentPolicy::default())?))?;
    let mut progress = no_progress;
    let error = block_on(coordinator.run(IndexJobId::new(7), &ControlToken::new(), &mut progress))
        .err()
        .ok_or("expected oversized page failure")?;

    assert!(matches!(
        error,
        IndexError::Execution {
            failure: ExecutionFailure::Protocol(ProtocolError::TooManyRecords {
                received: 5,
                limit: 4,
            }),
            persistence_error: None,
            ..
        }
    ));
    let (_, repository) = coordinator.into_parts();
    assert!(repository.records.is_empty());
    assert_eq!(
        repository.jobs[&IndexJobId::new(7)].state(),
        IndexJobState::Failed
    );
    Ok(())
}

#[test]
fn pause_cancel_and_terminal_state_rules_are_persisted() -> TestResult {
    let source = FakeHistorySource::new(vec![media(1, 1)?]);
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(2)?);
    block_on(coordinator.create_job(request(8, vec![range(1, 1)?], ContentPolicy::default())?))?;

    let control = ControlToken::new();
    control.request_pause();
    let mut progress = no_progress;
    assert!(matches!(
        block_on(coordinator.run(IndexJobId::new(8), &control, &mut progress))?,
        RunOutcome::Paused(_)
    ));
    assert!(control.resume());
    assert!(matches!(
        block_on(coordinator.run(IndexJobId::new(8), &control, &mut progress))?,
        RunOutcome::Completed(_)
    ));

    block_on(coordinator.create_job(request(9, vec![range(1, 1)?], ContentPolicy::default())?))?;
    let cancelled = ControlToken::new();
    cancelled.request_cancel();
    assert!(matches!(
        block_on(coordinator.run(IndexJobId::new(9), &cancelled, &mut progress))?,
        RunOutcome::Cancelled(_)
    ));
    let retry_error = block_on(coordinator.retry_failed_job(IndexJobId::new(9)))
        .err()
        .ok_or("cancelled job must not retry")?;
    assert!(matches!(
        retry_error,
        IndexError::InvalidJobState {
            state: IndexJobState::Cancelled,
            operation: JobOperation::Retry,
            ..
        }
    ));
    Ok(())
}

#[test]
fn flood_wait_is_account_scoped_and_failed_job_can_retry() -> TestResult {
    let mut source = FakeHistorySource::new(Vec::new());
    source.next_error = Some(HistoryError::FloodWait {
        retry_after: Duration::from_secs(42),
    });
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(2)?);
    block_on(coordinator.create_job(request(10, vec![range(1, 1)?], ContentPolicy::default())?))?;
    let mut progress = no_progress;
    let error = block_on(coordinator.run(IndexJobId::new(10), &ControlToken::new(), &mut progress))
        .err()
        .ok_or("expected flood wait")?;
    let advice = error.retry_advice().ok_or("missing retry advice")?;
    assert!(advice.retryable);
    assert_eq!(advice.retry_after, Some(Duration::from_secs(42)));
    assert_eq!(advice.scope, RetryScope::Account(ACCOUNT));

    let queued = block_on(coordinator.retry_failed_job(IndexJobId::new(10)))?;
    assert_eq!(queued.state(), IndexJobState::Queued);
    assert!(queued.last_failure().is_none());
    assert!(matches!(
        block_on(coordinator.run(IndexJobId::new(10), &ControlToken::new(), &mut progress))?,
        RunOutcome::Completed(_)
    ));
    Ok(())
}

#[test]
fn empty_range_commits_complete_evidence_and_finishes() -> TestResult {
    let source = FakeHistorySource::default();
    let repository = FakeRepository::default();
    let mut coordinator = IndexCoordinator::new(source, repository, BatchSize::try_new(3)?);
    block_on(coordinator.create_job(request(11, vec![range(1, 50)?], ContentPolicy::default())?))?;
    let mut snapshots = Vec::new();
    let mut progress = |snapshot| snapshots.push(snapshot);
    let outcome =
        block_on(coordinator.run(IndexJobId::new(11), &ControlToken::new(), &mut progress))?;

    assert!(matches!(outcome, RunOutcome::Completed(_)));
    let (_, repository) = coordinator.into_parts();
    assert!(repository.records.is_empty());
    assert_eq!(repository.ranges.len(), 1);
    assert_eq!(
        repository.ranges[&(IndexJobId::new(11), 0)].coverage,
        IndexCoverage::Complete
    );
    assert!(snapshots.iter().any(|snapshot| {
        snapshot.statistics.batches_committed == 1 && snapshot.statistics.messages_scanned == 0
    }));
    Ok(())
}

#[test]
fn identical_batch_replay_does_not_duplicate_records_or_counters() -> TestResult {
    let request = request(12, vec![range(1, 1)?], ContentPolicy::default())?;
    let running = IndexJobRecord::try_restore(
        request.clone(),
        IndexJobState::Running,
        IndexCheckpoint::initial(),
        IndexStatistics::default(),
        1,
        None,
    )?;
    let history = media(1, 1)?;
    let indexed = match history.content {
        HistoryContent::Media(media) => IndexedRecord {
            source: history.source,
            revision: history.revision,
            modified_at_unix_ms: history.modified_at_unix_ms,
            content: IndexedContent::Media(media),
        },
        _ => return Err("expected media fixture".into()),
    };
    let batch = IndexBatch::new(
        BatchId {
            job_id: request.job_id(),
            sequence: 1,
        },
        request.clone(),
        running.revision(),
        running.checkpoint().clone(),
        IndexCheckpoint::try_new(1, None, 1)?,
        0,
        request.ranges()[0],
        IndexCoverage::Complete,
        vec![indexed],
        1,
    );
    let mut repository = FakeRepository::default();
    repository.jobs.insert(running.id(), running);

    let first = block_on(repository.commit_batch(batch.clone()))?;
    let replay = block_on(repository.commit_batch(batch))?;

    assert_eq!(first.disposition, BatchCommitDisposition::Applied);
    assert_eq!(replay.disposition, BatchCommitDisposition::AlreadyCommitted);
    assert_eq!(first.job.statistics(), replay.job.statistics());
    assert_eq!(repository.records.len(), 1);
    assert_eq!(replay.job.statistics().records_written, 1);
    assert_eq!(replay.job.statistics().batches_committed, 1);
    Ok(())
}

#[test]
fn request_validation_accepts_gaps_but_rejects_overlap() -> TestResult {
    let accepted = request(
        13,
        vec![range(50, 60)?, range(1, 10)?],
        ContentPolicy::default(),
    )?;
    assert_eq!(accepted.ranges()[0], range(1, 10)?);
    assert_eq!(accepted.ranges()[1], range(50, 60)?);

    let overlap = request(
        14,
        vec![range(1, 10)?, range(10, 20)?],
        ContentPolicy::default(),
    );
    assert!(matches!(
        overlap,
        Err(ValidationError::OverlappingRanges { .. })
    ));
    assert_eq!(
        IndexCheckpoint::try_new(2, HistoryCursor::try_new(vec![1]).ok(), 2),
        Err(ValidationError::CursorAfterFinalRange)
    );
    Ok(())
}
