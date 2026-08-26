use std::collections::BTreeMap;

use teleark_core::{IndexCoverage, IndexJob, IndexJobState};

use crate::{
    BatchCommitReceipt, BatchId, BatchSize, ControlState, ControlToken, ExecutionFailure,
    HistoryPage, HistoryPageRequest, HistoryRecord, HistorySource, IndexBatch, IndexCheckpoint,
    IndexError, IndexJobRecord, IndexRepository, IndexRequest, IndexedRecord, JobOperation,
    MAX_PAGE_METADATA_BYTES, ProgressSink, ProgressSnapshot, ProtocolError, RunOutcome,
};

pub struct IndexCoordinator<S, R> {
    source: S,
    repository: R,
    batch_size: BatchSize,
}

impl<S, R> IndexCoordinator<S, R>
where
    S: HistorySource,
    R: IndexRepository,
{
    pub const fn new(source: S, repository: R, batch_size: BatchSize) -> Self {
        Self {
            source,
            repository,
            batch_size,
        }
    }

    pub const fn batch_size(&self) -> BatchSize {
        self.batch_size
    }

    pub fn source(&self) -> &S {
        &self.source
    }

    pub fn repository(&self) -> &R {
        &self.repository
    }

    pub fn into_parts(self) -> (S, R) {
        (self.source, self.repository)
    }

    pub async fn create_job(
        &mut self,
        request: IndexRequest,
    ) -> Result<IndexJobRecord, IndexError> {
        let job = IndexJobRecord::new(request);
        self.repository
            .create_job(job.clone())
            .await
            .map_err(|error| IndexError::Repository {
                job_id: job.id(),
                error,
            })?;
        Ok(job)
    }

    pub async fn load_job(
        &mut self,
        job_id: teleark_core::IndexJobId,
    ) -> Result<IndexJobRecord, IndexError> {
        self.repository
            .load_job(job_id)
            .await
            .map_err(|error| IndexError::Repository { job_id, error })?
            .ok_or(IndexError::JobNotFound { job_id })
    }

    pub async fn retry_failed_job(
        &mut self,
        job_id: teleark_core::IndexJobId,
    ) -> Result<IndexJobRecord, IndexError> {
        let job = self.load_job(job_id).await?;
        if job.state() != IndexJobState::Failed {
            return Err(IndexError::InvalidJobState {
                job_id,
                state: job.state(),
                operation: JobOperation::Retry,
            });
        }
        let queued = transition_record(&job, IndexJobState::Queued, None)?;
        self.replace_job(&job, &queued).await?;
        Ok(queued)
    }

    pub async fn run<P>(
        &mut self,
        job_id: teleark_core::IndexJobId,
        control: &ControlToken,
        progress: &mut P,
    ) -> Result<RunOutcome, IndexError>
    where
        P: ProgressSink + Send,
    {
        let mut job = self.load_job(job_id).await?;

        match job.state() {
            IndexJobState::Completed => {
                let snapshot = ProgressSnapshot::from_job(&job, None);
                progress.publish(snapshot.clone()).await;
                return Ok(RunOutcome::Completed(snapshot));
            }
            IndexJobState::Cancelled => {
                let snapshot = ProgressSnapshot::from_job(&job, None);
                progress.publish(snapshot.clone()).await;
                return Ok(RunOutcome::Cancelled(snapshot));
            }
            IndexJobState::Failed => {
                return Err(IndexError::InvalidJobState {
                    job_id,
                    state: job.state(),
                    operation: JobOperation::Run,
                });
            }
            IndexJobState::Queued | IndexJobState::Paused | IndexJobState::Running => {}
        }

        if let Some(outcome) = self
            .honor_control(job, control.state(), progress, None)
            .await?
        {
            return Ok(outcome);
        }
        job = self.load_job(job_id).await?;

        if job.state() != IndexJobState::Running {
            let running = transition_record(&job, IndexJobState::Running, None)?;
            self.replace_job(&job, &running).await?;
            job = running;
        }
        progress
            .publish(ProgressSnapshot::from_job(&job, None))
            .await;

        loop {
            if let Some(outcome) = self
                .honor_control(job.clone(), control.state(), progress, None)
                .await?
            {
                return Ok(outcome);
            }

            let range_index = job.checkpoint().next_range_index();
            let Some(range) = job.request().ranges().get(range_index).copied() else {
                let completed = transition_record(&job, IndexJobState::Completed, None)?;
                self.replace_job(&job, &completed).await?;
                let snapshot = ProgressSnapshot::from_job(&completed, None);
                progress.publish(snapshot.clone()).await;
                return Ok(RunOutcome::Completed(snapshot));
            };

            let request = HistoryPageRequest {
                account_id: job.request().account_id(),
                chat_id: job.request().chat_id(),
                range,
                cursor: job.checkpoint().cursor().cloned(),
                limit: self.batch_size,
            };
            let current_cursor = request.cursor.clone();
            let page = match self.source.fetch_page(request).await {
                Ok(page) => page,
                Err(error) => {
                    return Err(self
                        .record_execution_failure(&job, ExecutionFailure::History(error), progress)
                        .await);
                }
            };
            let normalized =
                match normalize_page(page, &job, range, current_cursor.as_ref(), self.batch_size) {
                    Ok(normalized) => normalized,
                    Err(error) => {
                        return Err(self
                            .record_execution_failure(
                                &job,
                                ExecutionFailure::Protocol(error),
                                progress,
                            )
                            .await);
                    }
                };

            let next_range_index = if normalized.exhausted {
                range_index
                    .checked_add(1)
                    .ok_or(IndexError::CounterOverflow {
                        field: "checkpoint.next_range_index",
                    })?
            } else {
                range_index
            };
            let next_checkpoint = IndexCheckpoint::try_new(
                next_range_index,
                normalized.next_cursor.clone(),
                job.request().ranges().len(),
            )?;
            let sequence = job.statistics().batches_committed.checked_add(1).ok_or(
                IndexError::CounterOverflow {
                    field: "statistics.batches_committed",
                },
            )?;
            let batch = IndexBatch::new(
                BatchId { job_id, sequence },
                job.request().clone(),
                job.revision(),
                job.checkpoint().clone(),
                next_checkpoint,
                range_index,
                range,
                if normalized.exhausted {
                    IndexCoverage::Complete
                } else {
                    IndexCoverage::Partial
                },
                normalized.records,
                normalized.messages_examined,
            );
            let receipt = self
                .repository
                .commit_batch(batch.clone())
                .await
                .map_err(|error| IndexError::Repository { job_id, error })?;
            validate_receipt(&job, &batch, &receipt)?;
            job = receipt.job;
            let evidence = receipt.range;
            progress
                .publish(ProgressSnapshot::from_job(&job, Some(evidence.clone())))
                .await;

            if let Some(outcome) = self
                .honor_control(job.clone(), control.state(), progress, Some(evidence))
                .await?
            {
                return Ok(outcome);
            }
        }
    }

    async fn replace_job(
        &mut self,
        previous: &IndexJobRecord,
        next: &IndexJobRecord,
    ) -> Result<(), IndexError> {
        self.repository
            .replace_job(previous.revision(), next.clone())
            .await
            .map_err(|error| IndexError::Repository {
                job_id: previous.id(),
                error,
            })
    }

    async fn honor_control<P>(
        &mut self,
        job: IndexJobRecord,
        control: ControlState,
        progress: &mut P,
        evidence: Option<crate::RangeEvidence>,
    ) -> Result<Option<RunOutcome>, IndexError>
    where
        P: ProgressSink + Send,
    {
        let target = match control {
            ControlState::Running => return Ok(None),
            ControlState::PauseRequested if job.state() == IndexJobState::Paused => {
                let snapshot = ProgressSnapshot::from_job(&job, evidence);
                progress.publish(snapshot.clone()).await;
                return Ok(Some(RunOutcome::Paused(snapshot)));
            }
            ControlState::PauseRequested => IndexJobState::Paused,
            ControlState::CancelRequested if job.state() == IndexJobState::Cancelled => {
                let snapshot = ProgressSnapshot::from_job(&job, evidence);
                progress.publish(snapshot.clone()).await;
                return Ok(Some(RunOutcome::Cancelled(snapshot)));
            }
            ControlState::CancelRequested => IndexJobState::Cancelled,
        };
        let stopped = transition_record(&job, target, None)?;
        self.replace_job(&job, &stopped).await?;
        let snapshot = ProgressSnapshot::from_job(&stopped, evidence);
        progress.publish(snapshot.clone()).await;
        Ok(Some(match target {
            IndexJobState::Paused => RunOutcome::Paused(snapshot),
            IndexJobState::Cancelled => RunOutcome::Cancelled(snapshot),
            IndexJobState::Queued
            | IndexJobState::Running
            | IndexJobState::Completed
            | IndexJobState::Failed => {
                return Err(IndexError::InvalidJobState {
                    job_id: stopped.id(),
                    state: target,
                    operation: JobOperation::Run,
                });
            }
        }))
    }

    async fn record_execution_failure<P>(
        &mut self,
        job: &IndexJobRecord,
        failure: ExecutionFailure,
        progress: &mut P,
    ) -> IndexError
    where
        P: ProgressSink + Send,
    {
        let failed = match transition_record(job, IndexJobState::Failed, Some(failure.clone())) {
            Ok(failed) => failed,
            Err(error) => return error,
        };
        let persistence_error = self
            .repository
            .replace_job(job.revision(), failed.clone())
            .await
            .err();
        if persistence_error.is_none() {
            progress
                .publish(ProgressSnapshot::from_job(&failed, None))
                .await;
        }
        IndexError::Execution {
            job_id: job.id(),
            account_id: job.request().account_id(),
            failure,
            persistence_error,
        }
    }
}

struct NormalizedPage {
    records: Vec<IndexedRecord>,
    next_cursor: Option<crate::HistoryCursor>,
    exhausted: bool,
    messages_examined: u64,
}

fn normalize_page(
    page: HistoryPage,
    job: &IndexJobRecord,
    range: crate::MessageRange,
    current_cursor: Option<&crate::HistoryCursor>,
    batch_size: BatchSize,
) -> Result<NormalizedPage, ProtocolError> {
    let (records, next_cursor, exhausted) = page.into_parts();
    if records.len() > batch_size.get() {
        return Err(ProtocolError::TooManyRecords {
            received: records.len(),
            limit: batch_size.get(),
        });
    }
    match (exhausted, next_cursor.as_ref()) {
        (false, None) => return Err(ProtocolError::MissingNextCursor),
        (true, Some(_)) => return Err(ProtocolError::UnexpectedNextCursor),
        (false, Some(next)) if current_cursor == Some(next) => {
            return Err(ProtocolError::StalledCursor);
        }
        _ => {}
    }

    let messages_examined =
        u64::try_from(records.len()).map_err(|_| ProtocolError::MetadataLimitExceeded {
            bytes: usize::MAX,
            maximum: MAX_PAGE_METADATA_BYTES,
        })?;
    let mut metadata_bytes = 0usize;
    let mut newest = BTreeMap::<crate::SourceKey, HistoryRecord>::new();
    for record in records {
        if record.source.account_id != job.request().account_id()
            || record.source.chat_id != job.request().chat_id()
            || !range.contains(record.source.message_id)
        {
            return Err(ProtocolError::RecordOutOfScope {
                source: record.source,
            });
        }
        if !record.has_valid_field_lengths() {
            return Err(ProtocolError::MetadataLimitExceeded {
                bytes: record.metadata_bytes(),
                maximum: MAX_PAGE_METADATA_BYTES,
            });
        }
        metadata_bytes = metadata_bytes.checked_add(record.metadata_bytes()).ok_or(
            ProtocolError::MetadataLimitExceeded {
                bytes: usize::MAX,
                maximum: MAX_PAGE_METADATA_BYTES,
            },
        )?;
        if metadata_bytes > MAX_PAGE_METADATA_BYTES {
            return Err(ProtocolError::MetadataLimitExceeded {
                bytes: metadata_bytes,
                maximum: MAX_PAGE_METADATA_BYTES,
            });
        }

        match newest.get(&record.source) {
            Some(existing) if existing.revision > record.revision => {}
            Some(existing) if existing.revision == record.revision && existing != &record => {
                return Err(ProtocolError::ConflictingRevision {
                    source: record.source,
                    revision: record.revision,
                });
            }
            Some(existing) if existing.revision == record.revision => {}
            _ => {
                newest.insert(record.source, record);
            }
        }
    }

    let policy = job.request().policy();
    let records = newest
        .into_values()
        .filter(|record| policy.accepts(&record.content))
        .filter_map(IndexedRecord::from_history)
        .collect();
    Ok(NormalizedPage {
        records,
        next_cursor,
        exhausted,
        messages_examined,
    })
}

fn transition_record(
    job: &IndexJobRecord,
    target: IndexJobState,
    failure: Option<ExecutionFailure>,
) -> Result<IndexJobRecord, IndexError> {
    let mut core = core_job_in_state(job)?;
    core.transition_to(target)?;
    let revision = job
        .revision()
        .checked_add(1)
        .ok_or(IndexError::CounterOverflow {
            field: "job.revision",
        })?;
    Ok(job.with_state(target, revision, failure))
}

fn core_job_in_state(job: &IndexJobRecord) -> Result<IndexJob, IndexError> {
    let mut core = IndexJob::new(
        job.id(),
        job.request().account_id(),
        job.request().chat_id(),
    );
    match job.state() {
        IndexJobState::Queued => {}
        IndexJobState::Running => core.transition_to(IndexJobState::Running)?,
        IndexJobState::Paused => core.transition_to(IndexJobState::Paused)?,
        IndexJobState::Completed => {
            core.transition_to(IndexJobState::Running)?;
            core.transition_to(IndexJobState::Completed)?;
        }
        IndexJobState::Failed => {
            core.transition_to(IndexJobState::Running)?;
            core.transition_to(IndexJobState::Failed)?;
        }
        IndexJobState::Cancelled => core.transition_to(IndexJobState::Cancelled)?,
    }
    Ok(core)
}

fn validate_receipt(
    previous: &IndexJobRecord,
    batch: &IndexBatch,
    receipt: &BatchCommitReceipt,
) -> Result<(), IndexError> {
    let expected_revision =
        previous
            .revision()
            .checked_add(1)
            .ok_or(IndexError::CounterOverflow {
                field: "job.revision",
            })?;
    let expected_messages = previous
        .statistics()
        .messages_scanned
        .checked_add(batch.messages_examined())
        .ok_or(IndexError::CounterOverflow {
            field: "statistics.messages_scanned",
        })?;
    let expected_batches = previous
        .statistics()
        .batches_committed
        .checked_add(1)
        .ok_or(IndexError::CounterOverflow {
            field: "statistics.batches_committed",
        })?;
    let next = &receipt.job;
    let stats = next.statistics();
    let records_delta = stats
        .records_written
        .checked_sub(previous.statistics().records_written);
    let category_delta = stats
        .files_indexed
        .checked_sub(previous.statistics().files_indexed)
        .and_then(|files| {
            stats
                .plain_text_indexed
                .checked_sub(previous.statistics().plain_text_indexed)
                .and_then(|text| files.checked_add(text))
        })
        .and_then(|subtotal| {
            stats
                .deletions_applied
                .checked_sub(previous.statistics().deletions_applied)
                .and_then(|deleted| subtotal.checked_add(deleted))
        });
    let valid = next.id() == previous.id()
        && next.request() == previous.request()
        && next.state() == IndexJobState::Running
        && next.last_failure().is_none()
        && next.revision() == expected_revision
        && next.checkpoint() == batch.next_checkpoint()
        && stats.messages_scanned == expected_messages
        && stats.batches_committed == expected_batches
        && stats.bytes_indexed >= previous.statistics().bytes_indexed
        && records_delta == category_delta
        && records_delta.is_some_and(|delta| {
            usize::try_from(delta).is_ok_and(|delta| delta <= batch.records().len())
        })
        && receipt.range.job_id == previous.id()
        && receipt.range.range_index == batch.range_index()
        && receipt.range.range == batch.range()
        && receipt.range.coverage == batch.coverage()
        && receipt.range.cursor.as_ref() == batch.next_checkpoint().cursor();
    if valid {
        Ok(())
    } else {
        Err(IndexError::RepositoryContract {
            job_id: previous.id(),
            error: ProtocolError::InvalidRepositoryReceipt,
        })
    }
}
