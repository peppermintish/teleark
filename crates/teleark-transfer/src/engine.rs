use std::collections::BTreeMap;

use teleark_core::{
    AccountId, PackageId, PartIndex, PartState, TransferDirection, TransferError, TransferId,
    TransferState, TransferTask,
};

use crate::progress::ProgressEmitter;
use crate::{
    CheckpointPort, Clock, ContentDigest, DestinationId, JitterSource, PartCheckpoint,
    ProgressConfig, RemoteObject, RemotePartKey, RetryDecision, RetryPolicy, SchedulerConfig,
    SourceId, SourceIdentity, TransferCheckpoint, TransferEngineError, TransferEvent, TransferIo,
    TransferScheduler, UploadError, VerifiedPart, WorkItem,
};

/// Deliberate crash-injection boundary used by deterministic recovery tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrashPoint {
    AfterRemoteSuccessBeforeCheckpoint,
    AfterCheckpointCommit,
}

/// Complete bounded-engine policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferEngineConfig {
    scheduler: SchedulerConfig,
    retry: RetryPolicy,
    progress: ProgressConfig,
}

impl TransferEngineConfig {
    #[must_use]
    pub const fn new(
        scheduler: SchedulerConfig,
        retry: RetryPolicy,
        progress: ProgressConfig,
    ) -> Self {
        Self {
            scheduler,
            retry,
            progress,
        }
    }
}

/// Immutable upload plan captured before workers start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadSpec {
    pub account_id: AccountId,
    pub package_id: PackageId,
    pub source_id: SourceId,
    pub source_identity: SourceIdentity,
    pub part_digests: Vec<ContentDigest>,
    pub whole_digest: ContentDigest,
}

/// Immutable download plan resolved from already validated metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadSpec {
    pub account_id: AccountId,
    pub package_id: PackageId,
    pub destination_id: DestinationId,
    pub part_digests: Vec<ContentDigest>,
    pub whole_digest: ContentDigest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum JobKind {
    Upload(UploadSpec),
    Download(DownloadSpec),
}

impl JobKind {
    const fn account_id(&self) -> AccountId {
        match self {
            Self::Upload(spec) => spec.account_id,
            Self::Download(spec) => spec.account_id,
        }
    }

    const fn package_id(&self) -> PackageId {
        match self {
            Self::Upload(spec) => spec.package_id,
            Self::Download(spec) => spec.package_id,
        }
    }

    fn part_digests(&self) -> &[ContentDigest] {
        match self {
            Self::Upload(spec) => &spec.part_digests,
            Self::Download(spec) => &spec.part_digests,
        }
    }
}

struct ManagedTransfer {
    task: TransferTask,
    kind: JobKind,
    checkpoint: TransferCheckpoint,
    last_error: Option<TransferError>,
}

/// Deterministic result from one cooperative engine step.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StepReport {
    pub dispatched_parts: usize,
    pub verified_parts: usize,
    pub waiting_retries: usize,
    pub failed_tasks: usize,
    pub completed_tasks: usize,
}

enum WorkFailure {
    Transfer { error: TransferError, attempts: u32 },
    Engine(TransferEngineError),
}

/// Owner of the bounded scheduler, Core state machines, checkpoints, retries,
/// cooperative commands, and progress coalescer.
pub struct TransferEngine {
    scheduler: TransferScheduler,
    retry: RetryPolicy,
    progress: ProgressEmitter,
    jobs: BTreeMap<TransferId, ManagedTransfer>,
    crash_once: Option<CrashPoint>,
}

impl TransferEngine {
    #[must_use]
    pub fn new(config: TransferEngineConfig) -> Self {
        Self {
            scheduler: TransferScheduler::new(config.scheduler),
            retry: config.retry,
            progress: ProgressEmitter::new(config.progress),
            jobs: BTreeMap::new(),
            crash_once: None,
        }
    }

    pub fn inject_crash_once(&mut self, point: CrashPoint) {
        self.crash_once = Some(point);
    }

    pub fn enqueue_upload(
        &mut self,
        io: &mut impl TransferIo,
        task: TransferTask,
        spec: UploadSpec,
        clock: &impl Clock,
    ) -> Result<(), TransferEngineError> {
        if task.direction() != TransferDirection::Upload {
            return Err(TransferEngineError::WrongDirection {
                transfer_id: task.id(),
            });
        }
        let identity = io.source_identity(spec.source_id)?;
        if identity != spec.source_identity || identity.size_bytes != task_total_bytes(&task)? {
            return Err(TransferError::SourceChanged.into());
        }
        self.enqueue(io, task, JobKind::Upload(spec), clock)
    }

    pub fn enqueue_download(
        &mut self,
        io: &mut impl TransferIo,
        task: TransferTask,
        spec: DownloadSpec,
        clock: &impl Clock,
    ) -> Result<(), TransferEngineError> {
        if task.direction() != TransferDirection::Download {
            return Err(TransferEngineError::WrongDirection {
                transfer_id: task.id(),
            });
        }
        if io.final_exists(spec.destination_id) {
            return Err(TransferError::PermissionDenied.into());
        }
        io.prepare_partial(spec.destination_id, task_total_bytes(&task)?)?;
        self.enqueue(io, task, JobKind::Download(spec), clock)
    }

    fn enqueue(
        &mut self,
        io: &mut impl TransferIo,
        mut task: TransferTask,
        kind: JobKind,
        clock: &impl Clock,
    ) -> Result<(), TransferEngineError> {
        if task.state() != TransferState::Queued {
            return Err(TransferEngineError::InvalidPlan {
                field: "new task state",
            });
        }
        if self.jobs.contains_key(&task.id()) {
            return Err(TransferEngineError::DuplicateWork {
                transfer_id: task.id(),
                part_index: PartIndex::new(0),
            });
        }
        if kind.part_digests().len() != task.parts().len() {
            return Err(TransferEngineError::InvalidPlan {
                field: "part digest count",
            });
        }
        let mut checkpoint = match io.load_checkpoint(task.id())? {
            Some(value) => value,
            None => fresh_checkpoint(&task, &kind),
        };
        validate_checkpoint(&task, &kind, &checkpoint)?;
        hydrate_verified_parts(io, &mut task, &kind, &checkpoint)?;

        let work = task
            .parts()
            .iter()
            .filter_map(|part| {
                checkpoint
                    .part(part.index())
                    .filter(|entry| entry.verified.is_none())
                    .map(|entry| WorkItem {
                        transfer_id: task.id(),
                        part_index: part.index(),
                        logical_file_id: task.logical_file_id(),
                        account_id: kind.account_id(),
                        direction: task.direction(),
                        priority: task.priority(),
                        not_before_ms: entry.retry_not_before_ms,
                    })
            })
            .collect::<Vec<_>>();
        self.scheduler.enqueue_batch(&work)?;
        checkpoint.parts.shrink_to_fit();
        let transfer_id = task.id();
        self.jobs.insert(
            transfer_id,
            ManagedTransfer {
                task,
                kind,
                checkpoint,
                last_error: None,
            },
        );
        let observation = {
            let job = self
                .jobs
                .get(&transfer_id)
                .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
            self.progress
                .observe(&job.task, job.last_error.as_ref(), clock.now_millis())
        };
        if let Err(error) = observation {
            self.scheduler.remove_transfer(transfer_id);
            self.jobs.remove(&transfer_id);
            return Err(error);
        }
        Ok(())
    }

    /// Cooperatively run every part admitted by one scheduler dispatch. No
    /// unbounded task spawning or sleeping occurs.
    pub fn step(
        &mut self,
        io: &mut impl TransferIo,
        clock: &impl Clock,
        jitter: &mut impl JitterSource,
    ) -> Result<StepReport, TransferEngineError> {
        let mut report = StepReport::default();
        self.finalize_ready(io, clock, &mut report)?;
        let scheduled = self.scheduler.dispatch_ready(clock.now_millis());
        report.dispatched_parts = scheduled.len();
        for work in scheduled {
            let transfer_id = work.item.transfer_id;
            let now_ms = clock.now_millis();
            if let Some(deadline) = self
                .scheduler
                .account_blocked_until(work.item.account_id, now_ms)
            {
                self.scheduler.requeue(work, deadline)?;
                continue;
            }
            let state = self
                .jobs
                .get(&transfer_id)
                .map(|job| job.task.state())
                .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
            match state {
                TransferState::Queued | TransferState::Running => {}
                TransferState::WaitingRetry => {
                    let deadline = self
                        .jobs
                        .get(&transfer_id)
                        .map(job_retry_deadline)
                        .unwrap_or(now_ms)
                        .max(now_ms);
                    if deadline > now_ms {
                        self.scheduler.requeue(work, deadline)?;
                        continue;
                    }
                }
                TransferState::Paused => {
                    self.scheduler.requeue(work, now_ms)?;
                    continue;
                }
                TransferState::Cancelled
                | TransferState::Completed
                | TransferState::Failed
                | TransferState::Verifying => {
                    self.scheduler.release(work)?;
                    continue;
                }
            }
            let result = {
                let job = self
                    .jobs
                    .get_mut(&transfer_id)
                    .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
                prepare_running(job, work.item.part_index)?;
                execute_part(job, io, work.item.part_index, &mut self.crash_once)
            };
            self.scheduler.release(work)?;
            match result {
                Ok(()) => report.verified_parts += 1,
                Err(WorkFailure::Transfer { error, attempts }) => {
                    self.handle_failure(
                        io,
                        clock,
                        jitter,
                        work.item,
                        error,
                        attempts,
                        &mut report,
                    )?;
                }
                Err(WorkFailure::Engine(error)) => {
                    if !matches!(error, TransferEngineError::InjectedCrash { .. }) {
                        fail_job_for_engine_error(
                            self.jobs.get_mut(&transfer_id),
                            work.item.part_index,
                        )?;
                    }
                    return Err(error);
                }
            }
            let job = self
                .jobs
                .get(&transfer_id)
                .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
            self.progress
                .observe(&job.task, job.last_error.as_ref(), clock.now_millis())?;
        }
        self.finalize_ready(io, clock, &mut report)?;
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_failure(
        &mut self,
        io: &mut impl CheckpointPort,
        clock: &impl Clock,
        jitter: &mut impl JitterSource,
        item: WorkItem,
        error: TransferError,
        attempts: u32,
        report: &mut StepReport,
    ) -> Result<(), TransferEngineError> {
        match self.retry.decide(&error, attempts, clock, jitter)? {
            RetryDecision::RetryAt { not_before_ms, .. } => {
                let job = self.jobs.get_mut(&item.transfer_id).ok_or(
                    TransferEngineError::UnknownTransfer {
                        transfer_id: item.transfer_id,
                    },
                )?;
                job.task
                    .transition_part(item.part_index, PartState::WaitingRetry)?;
                job.task.transition_to(TransferState::WaitingRetry)?;
                let candidate =
                    retry_checkpoint(&job.checkpoint, item.part_index, attempts, not_before_ms)?;
                io.save_checkpoint(&candidate)?;
                job.checkpoint = candidate;
                job.last_error = Some(error.clone());
                if let TransferError::FloodWait { .. } = error {
                    self.scheduler
                        .block_account_until(item.account_id, not_before_ms);
                }
                let mut retry_item = item;
                retry_item.not_before_ms = not_before_ms;
                self.scheduler.enqueue(retry_item)?;
                report.waiting_retries += 1;
            }
            RetryDecision::GiveUp => {
                let job = self.jobs.get_mut(&item.transfer_id).ok_or(
                    TransferEngineError::UnknownTransfer {
                        transfer_id: item.transfer_id,
                    },
                )?;
                job.task
                    .transition_part(item.part_index, PartState::Failed)?;
                job.task.fail(error.clone())?;
                job.last_error = Some(error);
                self.scheduler.remove_transfer(item.transfer_id);
                report.failed_tasks += 1;
            }
        }
        Ok(())
    }

    fn finalize_ready(
        &mut self,
        io: &mut impl TransferIo,
        clock: &impl Clock,
        report: &mut StepReport,
    ) -> Result<(), TransferEngineError> {
        let ready = self
            .jobs
            .iter()
            .filter(|(_, job)| {
                matches!(
                    job.task.state(),
                    TransferState::Queued | TransferState::Running
                ) && job
                    .task
                    .parts()
                    .iter()
                    .all(|part| part.state() == PartState::Verified)
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for transfer_id in ready {
            let result = {
                let job = self
                    .jobs
                    .get_mut(&transfer_id)
                    .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
                if job.task.state() == TransferState::Queued {
                    job.task.transition_to(TransferState::Running)?;
                }
                job.task.transition_to(TransferState::Verifying)?;
                finalize_job(job, io)
            };
            match result {
                Ok(()) => {
                    let job = self
                        .jobs
                        .get_mut(&transfer_id)
                        .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
                    job.task.transition_to(TransferState::Completed)?;
                    job.last_error = None;
                    self.scheduler.remove_transfer(transfer_id);
                    report.completed_tasks += 1;
                }
                Err(error) => {
                    let job = self
                        .jobs
                        .get_mut(&transfer_id)
                        .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
                    job.task.fail(error.clone())?;
                    job.last_error = Some(error);
                    self.scheduler.remove_transfer(transfer_id);
                    report.failed_tasks += 1;
                }
            }
            let job = self
                .jobs
                .get(&transfer_id)
                .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
            self.progress
                .observe(&job.task, job.last_error.as_ref(), clock.now_millis())?;
        }
        Ok(())
    }

    pub fn pause(
        &mut self,
        transfer_id: TransferId,
        clock: &impl Clock,
    ) -> Result<(), TransferEngineError> {
        let job = self
            .jobs
            .get_mut(&transfer_id)
            .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
        job.task.transition_to(TransferState::Paused)?;
        for part in job.task.parts().to_vec() {
            if part.state() == PartState::WaitingRetry {
                job.task.transition_part(part.index(), PartState::Paused)?;
            }
        }
        self.scheduler.set_suspended(transfer_id, true);
        self.progress
            .observe(&job.task, job.last_error.as_ref(), clock.now_millis())
    }

    pub fn resume(
        &mut self,
        transfer_id: TransferId,
        clock: &impl Clock,
    ) -> Result<(), TransferEngineError> {
        let job = self
            .jobs
            .get_mut(&transfer_id)
            .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
        job.task.transition_to(TransferState::Running)?;
        self.scheduler.set_suspended(transfer_id, false);
        self.progress
            .observe(&job.task, job.last_error.as_ref(), clock.now_millis())
    }

    pub fn cancel(
        &mut self,
        transfer_id: TransferId,
        clock: &impl Clock,
    ) -> Result<(), TransferEngineError> {
        let job = self
            .jobs
            .get_mut(&transfer_id)
            .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
        job.task.transition_to(TransferState::Cancelled)?;
        for part in job.task.parts().to_vec() {
            if !part.state().is_terminal() {
                job.task
                    .transition_part(part.index(), PartState::Cancelled)?;
            }
        }
        job.last_error = Some(TransferError::Cancelled);
        self.scheduler.remove_transfer(transfer_id);
        self.progress
            .observe(&job.task, job.last_error.as_ref(), clock.now_millis())
    }

    #[must_use]
    pub fn task(&self, transfer_id: TransferId) -> Option<&TransferTask> {
        self.jobs.get(&transfer_id).map(|job| &job.task)
    }

    pub fn drain_events(&mut self) -> Vec<TransferEvent> {
        self.progress.drain()
    }

    /// Remove a completed or cancelled task after its durable/presentation
    /// owner has consumed the final snapshot, keeping long-lived memory bounded.
    pub fn remove_terminal(
        &mut self,
        transfer_id: TransferId,
    ) -> Result<TransferTask, TransferEngineError> {
        let terminal = self
            .jobs
            .get(&transfer_id)
            .map(|job| job.task.state().is_terminal())
            .ok_or(TransferEngineError::UnknownTransfer { transfer_id })?;
        if !terminal {
            return Err(TransferEngineError::InvalidPlan {
                field: "remove nonterminal transfer",
            });
        }
        self.scheduler.remove_transfer(transfer_id);
        self.jobs
            .remove(&transfer_id)
            .map(|job| job.task)
            .ok_or(TransferEngineError::UnknownTransfer { transfer_id })
    }
}

fn fresh_checkpoint(task: &TransferTask, kind: &JobKind) -> TransferCheckpoint {
    TransferCheckpoint {
        transfer_id: task.id(),
        source_identity: match kind {
            JobKind::Upload(spec) => Some(spec.source_identity),
            JobKind::Download(_) => None,
        },
        destination_id: match kind {
            JobKind::Upload(_) => None,
            JobKind::Download(spec) => Some(spec.destination_id),
        },
        parts: task
            .parts()
            .iter()
            .map(|part| PartCheckpoint {
                part_index: part.index(),
                attempts: 0,
                retry_not_before_ms: 0,
                verified: None,
            })
            .collect(),
    }
}

fn validate_checkpoint(
    task: &TransferTask,
    kind: &JobKind,
    checkpoint: &TransferCheckpoint,
) -> Result<(), TransferEngineError> {
    if checkpoint.transfer_id != task.id() || checkpoint.parts.len() != task.parts().len() {
        return Err(TransferEngineError::InvalidPlan {
            field: "checkpoint task layout",
        });
    }
    match kind {
        JobKind::Upload(spec) => {
            if checkpoint.source_identity != Some(spec.source_identity) {
                return Err(TransferError::SourceChanged.into());
            }
            if checkpoint.destination_id.is_some() {
                return Err(TransferEngineError::InvalidPlan {
                    field: "upload checkpoint destination",
                });
            }
        }
        JobKind::Download(spec) => {
            if checkpoint.destination_id != Some(spec.destination_id)
                || checkpoint.source_identity.is_some()
            {
                return Err(TransferEngineError::InvalidPlan {
                    field: "download checkpoint identity",
                });
            }
        }
    }
    for (position, part) in task.parts().iter().enumerate() {
        let entry = checkpoint
            .parts
            .get(position)
            .ok_or(TransferEngineError::InvalidPlan {
                field: "checkpoint part missing",
            })?;
        if entry.part_index != part.index() {
            return Err(TransferEngineError::InvalidPlan {
                field: "checkpoint part index",
            });
        }
        if let Some(verified) = &entry.verified {
            let expected_key = RemotePartKey {
                account_id: kind.account_id(),
                package_id: kind.package_id(),
                part_index: part.index(),
            };
            let remote_matches = verified.remote_object.as_ref().is_some_and(|remote| {
                remote.key == expected_key
                    && remote.encoded_size == part.size_bytes()
                    && remote.digest == verified.digest
            });
            if verified.size_bytes != part.size_bytes()
                || verified.digest != kind.part_digests()[position]
                || entry.retry_not_before_ms != 0
                || !remote_matches
            {
                return Err(TransferEngineError::InvalidPlan {
                    field: "checkpoint verification evidence",
                });
            }
        }
    }
    Ok(())
}

fn hydrate_verified_parts(
    io: &mut impl TransferIo,
    task: &mut TransferTask,
    kind: &JobKind,
    checkpoint: &TransferCheckpoint,
) -> Result<(), TransferEngineError> {
    for part in task.parts().to_vec() {
        let Some(verified) = checkpoint
            .part(part.index())
            .and_then(|entry| entry.verified.as_ref())
        else {
            continue;
        };
        if let JobKind::Download(spec) = kind {
            let bytes =
                io.read_partial(spec.destination_id, part.offset_bytes(), part.size_bytes())?;
            if io.digest(&bytes) != verified.digest {
                return Err(TransferError::HashMismatch.into());
            }
        }
        task.transition_part(part.index(), PartState::Running)?;
        task.set_part_progress(part.index(), part.size_bytes())?;
        task.transition_part(part.index(), PartState::Transferred)?;
        task.transition_part(part.index(), PartState::Verifying)?;
        task.transition_part(part.index(), PartState::Verified)?;
    }
    Ok(())
}

fn prepare_running(
    job: &mut ManagedTransfer,
    part_index: PartIndex,
) -> Result<(), TransferEngineError> {
    match job.task.state() {
        TransferState::Queued | TransferState::WaitingRetry => {
            job.task.transition_to(TransferState::Running)?;
        }
        TransferState::Running => {}
        _ => {
            return Err(TransferEngineError::InvalidPlan {
                field: "scheduled task state",
            });
        }
    }
    let state = job
        .task
        .parts()
        .get(part_index.get() as usize)
        .map(|part| part.state())
        .ok_or(TransferEngineError::InvalidPlan {
            field: "scheduled part index",
        })?;
    match state {
        PartState::Queued | PartState::Paused | PartState::WaitingRetry => {
            job.task.transition_part(part_index, PartState::Running)?;
        }
        PartState::Running => {}
        _ => {
            return Err(TransferEngineError::InvalidPlan {
                field: "scheduled part state",
            });
        }
    }
    job.last_error = None;
    Ok(())
}

fn execute_part(
    job: &mut ManagedTransfer,
    io: &mut impl TransferIo,
    part_index: PartIndex,
    crash_once: &mut Option<CrashPoint>,
) -> Result<(), WorkFailure> {
    let position = part_index.get() as usize;
    let part = job.task.parts().get(position).cloned().ok_or({
        WorkFailure::Engine(TransferEngineError::InvalidPlan {
            field: "worker part index",
        })
    })?;
    let expected_digest = *job.kind.part_digests().get(position).ok_or({
        WorkFailure::Engine(TransferEngineError::InvalidPlan {
            field: "worker part digest",
        })
    })?;
    let previous_attempts = job
        .checkpoint
        .part(part_index)
        .map_or(0, |entry| entry.attempts);
    let attempts = previous_attempts.checked_add(1).ok_or({
        WorkFailure::Engine(TransferEngineError::ArithmeticOverflow {
            field: "part attempts",
        })
    })?;
    let key = RemotePartKey {
        account_id: job.kind.account_id(),
        package_id: job.kind.package_id(),
        part_index,
    };
    let verified = match &job.kind {
        JobKind::Upload(spec) => execute_upload(
            io,
            spec,
            key,
            part.offset_bytes(),
            part.size_bytes(),
            expected_digest,
            attempts,
            crash_once,
        )?,
        JobKind::Download(spec) => execute_download(
            io,
            spec,
            key,
            part.offset_bytes(),
            part.size_bytes(),
            expected_digest,
            attempts,
        )?,
    };
    let mut candidate = job.checkpoint.clone();
    let entry = candidate.part_mut(part_index).ok_or({
        WorkFailure::Engine(TransferEngineError::InvalidPlan {
            field: "checkpoint part update",
        })
    })?;
    entry.attempts = attempts;
    entry.retry_not_before_ms = 0;
    entry.verified = Some(verified);
    io.save_checkpoint(&candidate)
        .map_err(|error| WorkFailure::Transfer { error, attempts })?;
    if take_crash(crash_once, CrashPoint::AfterCheckpointCommit) {
        return Err(WorkFailure::Engine(TransferEngineError::InjectedCrash {
            point: CrashPoint::AfterCheckpointCommit,
        }));
    }
    job.checkpoint = candidate;
    job.task
        .set_part_progress(part_index, part.size_bytes())
        .map_err(|error| WorkFailure::Engine(error.into()))?;
    job.task
        .transition_part(part_index, PartState::Transferred)
        .map_err(|error| WorkFailure::Engine(error.into()))?;
    job.task
        .transition_part(part_index, PartState::Verifying)
        .map_err(|error| WorkFailure::Engine(error.into()))?;
    job.task
        .transition_part(part_index, PartState::Verified)
        .map_err(|error| WorkFailure::Engine(error.into()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn execute_upload(
    io: &mut impl TransferIo,
    spec: &UploadSpec,
    key: RemotePartKey,
    offset: u64,
    size: u64,
    expected_digest: ContentDigest,
    attempts: u32,
    crash_once: &mut Option<CrashPoint>,
) -> Result<VerifiedPart, WorkFailure> {
    if io
        .source_identity(spec.source_id)
        .map_err(|error| WorkFailure::Transfer { error, attempts })?
        != spec.source_identity
    {
        return Err(WorkFailure::Transfer {
            error: TransferError::SourceChanged,
            attempts,
        });
    }
    let discovered = io
        .discover_remote(key)
        .map_err(|error| WorkFailure::Transfer { error, attempts })?;
    let (object, newly_uploaded) = match discovered.as_slice() {
        [] => {
            let bytes = io
                .read_source_range(spec.source_id, offset, size)
                .map_err(|error| WorkFailure::Transfer { error, attempts })?;
            if io
                .source_identity(spec.source_id)
                .map_err(|error| WorkFailure::Transfer { error, attempts })?
                != spec.source_identity
            {
                return Err(WorkFailure::Transfer {
                    error: TransferError::SourceChanged,
                    attempts,
                });
            }
            if u64::try_from(bytes.len()).ok() != Some(size) || io.digest(&bytes) != expected_digest
            {
                return Err(WorkFailure::Transfer {
                    error: TransferError::SourceChanged,
                    attempts,
                });
            }
            match io.upload_remote(key, &bytes, expected_digest) {
                Ok(object) => (object, true),
                Err(UploadError::Definite(error)) => {
                    return Err(WorkFailure::Transfer { error, attempts });
                }
                Err(UploadError::AmbiguousSuccess) => {
                    let reconciled = io
                        .discover_remote(key)
                        .map_err(|error| WorkFailure::Transfer { error, attempts })?;
                    (one_remote(key, &reconciled)?, false)
                }
            }
        }
        [object] => (object.clone(), false),
        _ => return Err(WorkFailure::Engine(remote_conflict(key, discovered.len()))),
    };
    io.verify_remote(&object, size, expected_digest)
        .map_err(|error| WorkFailure::Transfer { error, attempts })?;
    if newly_uploaded && take_crash(crash_once, CrashPoint::AfterRemoteSuccessBeforeCheckpoint) {
        return Err(WorkFailure::Engine(TransferEngineError::InjectedCrash {
            point: CrashPoint::AfterRemoteSuccessBeforeCheckpoint,
        }));
    }
    Ok(VerifiedPart {
        size_bytes: size,
        digest: expected_digest,
        remote_object: Some(object),
    })
}

#[allow(clippy::too_many_arguments)]
fn execute_download(
    io: &mut impl TransferIo,
    spec: &DownloadSpec,
    key: RemotePartKey,
    offset: u64,
    size: u64,
    expected_digest: ContentDigest,
    attempts: u32,
) -> Result<VerifiedPart, WorkFailure> {
    let discovered = io
        .discover_remote(key)
        .map_err(|error| WorkFailure::Transfer { error, attempts })?;
    let object = one_remote(key, &discovered)?;
    io.verify_remote(&object, size, expected_digest)
        .map_err(|error| WorkFailure::Transfer { error, attempts })?;
    let bytes = io
        .download_remote(&object)
        .map_err(|error| WorkFailure::Transfer { error, attempts })?;
    if u64::try_from(bytes.len()).ok() != Some(size) || io.digest(&bytes) != expected_digest {
        return Err(WorkFailure::Transfer {
            error: TransferError::HashMismatch,
            attempts,
        });
    }
    io.write_partial(spec.destination_id, offset, &bytes)
        .map_err(|error| WorkFailure::Transfer { error, attempts })?;
    let written = io
        .read_partial(spec.destination_id, offset, size)
        .map_err(|error| WorkFailure::Transfer { error, attempts })?;
    if io.digest(&written) != expected_digest {
        return Err(WorkFailure::Transfer {
            error: TransferError::HashMismatch,
            attempts,
        });
    }
    Ok(VerifiedPart {
        size_bytes: size,
        digest: expected_digest,
        remote_object: Some(object),
    })
}

fn one_remote(key: RemotePartKey, objects: &[RemoteObject]) -> Result<RemoteObject, WorkFailure> {
    match objects {
        [object] => Ok(object.clone()),
        [] => Err(WorkFailure::Transfer {
            error: TransferError::RemoteMissing,
            attempts: 1,
        }),
        _ => Err(WorkFailure::Engine(remote_conflict(key, objects.len()))),
    }
}

fn remote_conflict(key: RemotePartKey, object_count: usize) -> TransferEngineError {
    TransferEngineError::RemoteConflict { key, object_count }
}

fn retry_checkpoint(
    checkpoint: &TransferCheckpoint,
    part_index: PartIndex,
    attempts: u32,
    not_before_ms: u64,
) -> Result<TransferCheckpoint, TransferEngineError> {
    let mut candidate = checkpoint.clone();
    let part = candidate
        .part_mut(part_index)
        .ok_or(TransferEngineError::InvalidPlan {
            field: "retry checkpoint part",
        })?;
    part.attempts = attempts;
    part.retry_not_before_ms = not_before_ms;
    Ok(candidate)
}

fn finalize_job(job: &mut ManagedTransfer, io: &mut impl TransferIo) -> Result<(), TransferError> {
    match &job.kind {
        JobKind::Upload(spec) => {
            if io.source_identity(spec.source_id)? != spec.source_identity {
                return Err(TransferError::SourceChanged);
            }
            let bytes = io.read_source_range(spec.source_id, 0, spec.source_identity.size_bytes)?;
            if io.source_identity(spec.source_id)? != spec.source_identity
                || io.digest(&bytes) != spec.whole_digest
            {
                return Err(TransferError::SourceChanged);
            }
            Ok(())
        }
        JobKind::Download(spec) => {
            let total = task_total_bytes(&job.task).map_err(|_| TransferError::HashMismatch)?;
            let bytes = io.read_partial(spec.destination_id, 0, total)?;
            if io.digest(&bytes) != spec.whole_digest {
                return Err(TransferError::HashMismatch);
            }
            io.flush_partial(spec.destination_id)?;
            io.atomic_finalize(spec.destination_id)
        }
    }
}

fn task_total_bytes(task: &TransferTask) -> Result<u64, TransferEngineError> {
    task.parts().iter().try_fold(0_u64, |total, part| {
        total
            .checked_add(part.size_bytes())
            .ok_or(TransferEngineError::ArithmeticOverflow {
                field: "task total bytes",
            })
    })
}

fn take_crash(slot: &mut Option<CrashPoint>, point: CrashPoint) -> bool {
    if *slot == Some(point) {
        *slot = None;
        true
    } else {
        false
    }
}

fn job_retry_deadline(job: &ManagedTransfer) -> u64 {
    job.checkpoint
        .parts
        .iter()
        .map(|part| part.retry_not_before_ms)
        .max()
        .unwrap_or_default()
}

fn fail_job_for_engine_error(
    job: Option<&mut ManagedTransfer>,
    part_index: PartIndex,
) -> Result<(), TransferEngineError> {
    let Some(job) = job else {
        return Ok(());
    };
    if job.task.state() == TransferState::Running {
        if job
            .task
            .parts()
            .get(part_index.get() as usize)
            .is_some_and(|part| part.state() == PartState::Running)
        {
            job.task.transition_part(part_index, PartState::Failed)?;
        }
        job.task.fail(TransferError::RemoteMissing)?;
        job.last_error = Some(TransferError::RemoteMissing);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use teleark_core::{
        LogicalFileId, TransferDirection, TransferPriority, TransferState, TransferTask,
    };

    use super::*;
    use crate::{FakeClock, FakeEnvironment, FakeUploadBehavior, SequenceJitter};

    fn config() -> TransferEngineConfig {
        let scheduler = match SchedulerConfig::new(2, 2, 2, 2, 1, 64) {
            Ok(value) => value,
            Err(error) => panic!("test scheduler configuration failed: {error}"),
        };
        let retry = match RetryPolicy::new(3, 100, 1_000, 0) {
            Ok(value) => value,
            Err(error) => panic!("test retry configuration failed: {error}"),
        };
        let progress = match ProgressConfig::new(1, 128, 32) {
            Ok(value) => value,
            Err(error) => panic!("test progress configuration failed: {error}"),
        };
        TransferEngineConfig::new(scheduler, retry, progress)
    }

    fn make_task(
        transfer_id: u64,
        direction: TransferDirection,
        part_sizes: &[u64],
        priority: i16,
    ) -> TransferTask {
        let total = part_sizes.iter().copied().sum();
        match TransferTask::try_new(
            TransferId::new(transfer_id),
            LogicalFileId::new(transfer_id + 100),
            direction,
            total,
            TransferPriority::new(priority),
            part_sizes,
        ) {
            Ok(value) => value,
            Err(error) => panic!("test transfer task failed: {error}"),
        }
    }

    fn remote_key(transfer_id: u64, package_id: u64, part_index: u32) -> RemotePartKey {
        RemotePartKey {
            account_id: AccountId::new(transfer_id as i64),
            package_id: PackageId::new(package_id),
            part_index: PartIndex::new(part_index),
        }
    }

    fn upload_fixture(
        environment: &mut FakeEnvironment,
        transfer_id: u64,
        package_id: u64,
        bytes: &[u8],
        part_sizes: &[u64],
    ) -> (TransferTask, UploadSpec) {
        let source_id = SourceId(transfer_id);
        let identity =
            environment.add_source(source_id, bytes.to_vec(), u128::from(transfer_id), 10);
        let mut offset = 0_usize;
        let mut part_digests = Vec::new();
        for size in part_sizes {
            let size = *size as usize;
            part_digests.push(environment.digest_bytes(&bytes[offset..offset + size]));
            offset += size;
        }
        (
            make_task(transfer_id, TransferDirection::Upload, part_sizes, 0),
            UploadSpec {
                account_id: AccountId::new(transfer_id as i64),
                package_id: PackageId::new(package_id),
                source_id,
                source_identity: identity,
                part_digests,
                whole_digest: environment.digest_bytes(bytes),
            },
        )
    }

    fn download_fixture(
        environment: &mut FakeEnvironment,
        transfer_id: u64,
        package_id: u64,
        destination_id: DestinationId,
        parts: &[&[u8]],
    ) -> (TransferTask, DownloadSpec) {
        let mut part_digests = Vec::new();
        let mut all = Vec::new();
        let mut sizes = Vec::new();
        for (index, bytes) in parts.iter().enumerate() {
            let key = remote_key(transfer_id, package_id, index as u32);
            environment.seed_remote(key, bytes.to_vec());
            part_digests.push(environment.digest_bytes(bytes));
            all.extend_from_slice(bytes);
            sizes.push(bytes.len() as u64);
        }
        (
            make_task(transfer_id, TransferDirection::Download, &sizes, 0),
            DownloadSpec {
                account_id: AccountId::new(transfer_id as i64),
                package_id: PackageId::new(package_id),
                destination_id,
                part_digests,
                whole_digest: environment.digest_bytes(&all),
            },
        )
    }

    #[test]
    fn pause_resume_and_cancel_are_cooperative_safe_boundaries() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let (task, spec) = upload_fixture(&mut environment, 1, 91, b"abcdefghij", &[4, 6]);
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_upload(&mut environment, task, spec, &clock)
                .is_ok()
        );
        assert!(engine.pause(TransferId::new(1), &clock).is_ok());
        assert_eq!(
            engine.task(TransferId::new(1)).map(TransferTask::state),
            Some(TransferState::Paused)
        );
        assert_eq!(
            engine.step(&mut environment, &clock, &mut jitter),
            Ok(StepReport::default())
        );
        assert!(engine.resume(TransferId::new(1), &clock).is_ok());
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert!(engine.pause(TransferId::new(1), &clock).is_ok());
        assert!(engine.resume(TransferId::new(1), &clock).is_ok());
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(
            engine.task(TransferId::new(1)).map(TransferTask::state),
            Some(TransferState::Completed)
        );

        let (task, spec) = upload_fixture(&mut environment, 2, 92, b"cancel", &[6]);
        assert!(
            engine
                .enqueue_upload(&mut environment, task, spec, &clock)
                .is_ok()
        );
        assert!(engine.cancel(TransferId::new(2), &clock).is_ok());
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(
            engine.task(TransferId::new(2)).map(TransferTask::state),
            Some(TransferState::Cancelled)
        );
        assert_eq!(environment.remote_count(remote_key(2, 92, 0)), 0);
    }

    #[test]
    fn flood_wait_uses_injected_clock_and_retry_is_bounded() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let (task, spec) = upload_fixture(&mut environment, 3, 93, b"retry", &[5]);
        let key = remote_key(3, 93, 0);
        environment.push_upload_behavior(key, FakeUploadBehavior::FloodWait { millis: 500 });
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_upload(&mut environment, task, spec, &clock)
                .is_ok()
        );
        let report = engine.step(&mut environment, &clock, &mut jitter);
        assert!(matches!(
            report,
            Ok(StepReport {
                waiting_retries: 1,
                ..
            })
        ));
        assert_eq!(
            engine.task(TransferId::new(3)).map(TransferTask::state),
            Some(TransferState::WaitingRetry)
        );
        clock.set(499);
        assert_eq!(
            engine.step(&mut environment, &clock, &mut jitter),
            Ok(StepReport::default())
        );
        clock.set(500);
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(
            engine.task(TransferId::new(3)).map(TransferTask::state),
            Some(TransferState::Completed)
        );
        assert_eq!(environment.upload_calls(key), 2);

        let (task, spec) = upload_fixture(&mut environment, 4, 94, b"fail", &[4]);
        let key = remote_key(4, 94, 0);
        for _ in 0..3 {
            environment.push_upload_behavior(key, FakeUploadBehavior::NetworkFailure);
        }
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_upload(&mut environment, task, spec, &clock)
                .is_ok()
        );
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        clock.advance(100);
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        clock.advance(200);
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(
            engine.task(TransferId::new(4)).map(TransferTask::state),
            Some(TransferState::Failed)
        );
        assert_eq!(environment.upload_calls(key), 3);
    }

    #[test]
    fn flood_wait_defers_already_admitted_work_for_the_same_account() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let (first_task, mut first_spec) =
            upload_fixture(&mut environment, 14, 104, b"first", &[5]);
        let (second_task, mut second_spec) =
            upload_fixture(&mut environment, 15, 105, b"second", &[6]);
        let account = AccountId::new(77);
        first_spec.account_id = account;
        second_spec.account_id = account;
        let first_key = RemotePartKey {
            account_id: account,
            package_id: PackageId::new(104),
            part_index: PartIndex::new(0),
        };
        let second_key = RemotePartKey {
            account_id: account,
            package_id: PackageId::new(105),
            part_index: PartIndex::new(0),
        };
        environment.push_upload_behavior(first_key, FakeUploadBehavior::FloodWait { millis: 500 });
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_upload(&mut environment, first_task, first_spec, &clock)
                .is_ok()
        );
        assert!(
            engine
                .enqueue_upload(&mut environment, second_task, second_spec, &clock)
                .is_ok()
        );
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(environment.upload_calls(first_key), 1);
        assert_eq!(environment.upload_calls(second_key), 0);
        clock.set(500);
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(environment.upload_calls(second_key), 1);
        assert_eq!(
            engine.task(TransferId::new(14)).map(TransferTask::state),
            Some(TransferState::Completed)
        );
        assert_eq!(
            engine.task(TransferId::new(15)).map(TransferTask::state),
            Some(TransferState::Completed)
        );
    }

    #[test]
    fn restart_skips_verified_parts() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let (task, spec) = upload_fixture(&mut environment, 5, 95, b"abcdefgh", &[4, 4]);
        let mut first = TransferEngine::new(config());
        assert!(
            first
                .enqueue_upload(&mut environment, task, spec.clone(), &clock)
                .is_ok()
        );
        assert!(first.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(environment.upload_calls(remote_key(5, 95, 0)), 1);
        drop(first);

        let restarted_task = make_task(5, TransferDirection::Upload, &[4, 4], 0);
        let mut restarted = TransferEngine::new(config());
        assert!(
            restarted
                .enqueue_upload(&mut environment, restarted_task, spec, &clock)
                .is_ok()
        );
        assert!(
            restarted
                .step(&mut environment, &clock, &mut jitter)
                .is_ok()
        );
        assert_eq!(environment.upload_calls(remote_key(5, 95, 0)), 1);
        assert_eq!(environment.upload_calls(remote_key(5, 95, 1)), 1);
        assert_eq!(
            restarted.task(TransferId::new(5)).map(TransferTask::state),
            Some(TransferState::Completed)
        );
    }

    #[test]
    fn download_restart_revalidates_partial_and_skips_verified_range() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let destination = DestinationId(16);
        let (task, spec) =
            download_fixture(&mut environment, 16, 106, destination, &[b"left", b"right"]);
        let first_key = remote_key(16, 106, 0);
        let second_key = remote_key(16, 106, 1);
        let mut first = TransferEngine::new(config());
        assert!(
            first
                .enqueue_download(&mut environment, task, spec.clone(), &clock)
                .is_ok()
        );
        assert!(first.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(environment.download_calls(first_key), 1);
        drop(first);

        let mut restarted = TransferEngine::new(config());
        assert!(
            restarted
                .enqueue_download(
                    &mut environment,
                    make_task(16, TransferDirection::Download, &[4, 5], 0),
                    spec,
                    &clock,
                )
                .is_ok()
        );
        assert!(
            restarted
                .step(&mut environment, &clock, &mut jitter)
                .is_ok()
        );
        assert_eq!(environment.download_calls(first_key), 1);
        assert_eq!(environment.download_calls(second_key), 1);
        assert_eq!(
            environment.final_bytes(destination),
            Some(&b"leftright"[..])
        );
    }

    #[test]
    fn crash_after_remote_success_reconciles_without_duplicate_part() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let (task, spec) = upload_fixture(&mut environment, 6, 96, b"crash", &[5]);
        let key = remote_key(6, 96, 0);
        let mut first = TransferEngine::new(config());
        assert!(
            first
                .enqueue_upload(&mut environment, task, spec.clone(), &clock)
                .is_ok()
        );
        first.inject_crash_once(CrashPoint::AfterRemoteSuccessBeforeCheckpoint);
        assert_eq!(
            first.step(&mut environment, &clock, &mut jitter),
            Err(TransferEngineError::InjectedCrash {
                point: CrashPoint::AfterRemoteSuccessBeforeCheckpoint
            })
        );
        assert_eq!(environment.remote_count(key), 1);
        assert_eq!(environment.upload_calls(key), 1);
        drop(first);

        let mut restarted = TransferEngine::new(config());
        assert!(
            restarted
                .enqueue_upload(
                    &mut environment,
                    make_task(6, TransferDirection::Upload, &[5], 0),
                    spec,
                    &clock,
                )
                .is_ok()
        );
        assert!(
            restarted
                .step(&mut environment, &clock, &mut jitter)
                .is_ok()
        );
        assert_eq!(environment.remote_count(key), 1);
        assert_eq!(environment.upload_calls(key), 1);
    }

    #[test]
    fn ambiguous_success_is_reconciled_immediately_without_duplicate() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let (task, spec) = upload_fixture(&mut environment, 7, 97, b"maybe", &[5]);
        let key = remote_key(7, 97, 0);
        environment.push_upload_behavior(key, FakeUploadBehavior::AmbiguousSuccess);
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_upload(&mut environment, task, spec, &clock)
                .is_ok()
        );
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(environment.remote_count(key), 1);
        assert_eq!(environment.upload_calls(key), 1);
        assert_eq!(
            engine.task(TransferId::new(7)).map(TransferTask::state),
            Some(TransferState::Completed)
        );
    }

    #[test]
    fn source_change_stops_before_the_next_remote_part() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let (task, spec) = upload_fixture(&mut environment, 8, 98, b"abcdefgh", &[4, 4]);
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_upload(&mut environment, task, spec.clone(), &clock)
                .is_ok()
        );
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        environment.replace_source(spec.source_id, b"abcdWXYZ".to_vec());
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(
            engine.task(TransferId::new(8)).map(TransferTask::state),
            Some(TransferState::Failed)
        );
        assert_eq!(
            engine
                .task(TransferId::new(8))
                .and_then(TransferTask::last_error),
            Some(&TransferError::SourceChanged)
        );
        assert_eq!(environment.remote_count(remote_key(8, 98, 1)), 0);
    }

    #[test]
    fn source_mutation_during_read_never_uploads_changed_bytes() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let (task, spec) = upload_fixture(&mut environment, 9, 99, b"stable", &[6]);
        environment.mutate_source_after_next_read(spec.source_id, b"mutate".to_vec());
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_upload(&mut environment, task, spec, &clock)
                .is_ok()
        );
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(environment.remote_count(remote_key(9, 99, 0)), 0);
        assert_eq!(
            engine.task(TransferId::new(9)).map(TransferTask::state),
            Some(TransferState::Failed)
        );
    }

    #[test]
    fn download_hash_failure_never_exposes_final_destination() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let destination = DestinationId(10);
        let (task, spec) = download_fixture(&mut environment, 10, 100, destination, &[b"good"]);
        let key = remote_key(10, 100, 0);
        environment.corrupt_next_download(key);
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_download(&mut environment, task, spec, &clock)
                .is_ok()
        );
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(
            engine.task(TransferId::new(10)).map(TransferTask::state),
            Some(TransferState::Failed)
        );
        assert!(environment.final_bytes(destination).is_none());
        assert!(environment.partial_bytes(destination).is_some());
    }

    #[test]
    fn whole_file_verification_precedes_flush_and_atomic_finalize() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let destination = DestinationId(11);
        let (task, mut spec) =
            download_fixture(&mut environment, 11, 101, destination, &[b"left", b"right"]);
        spec.whole_digest = ContentDigest([0xff; 32]);
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_download(&mut environment, task, spec, &clock)
                .is_ok()
        );
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(
            engine.task(TransferId::new(11)).map(TransferTask::state),
            Some(TransferState::Failed)
        );
        assert!(environment.final_bytes(destination).is_none());
        assert_eq!(
            environment.partial_bytes(destination),
            Some(&b"leftright"[..])
        );
    }

    #[test]
    fn successful_download_atomically_publishes_exact_bytes() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let destination = DestinationId(12);
        let (task, spec) =
            download_fixture(&mut environment, 12, 102, destination, &[b"one", b"two"]);
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_download(&mut environment, task, spec, &clock)
                .is_ok()
        );
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(environment.final_bytes(destination), Some(&b"onetwo"[..]));
        assert!(environment.partial_bytes(destination).is_none());
        assert_eq!(
            engine.task(TransferId::new(12)).map(TransferTask::state),
            Some(TransferState::Completed)
        );
        assert!(engine.remove_terminal(TransferId::new(12)).is_ok());
        assert!(engine.task(TransferId::new(12)).is_none());
    }

    #[test]
    fn checkpoint_save_failure_reconciles_existing_remote_on_retry() {
        let mut environment = FakeEnvironment::new();
        let clock = FakeClock::new(0);
        let mut jitter = SequenceJitter::default();
        let (task, spec) = upload_fixture(&mut environment, 13, 103, b"commit", &[6]);
        let key = remote_key(13, 103, 0);
        environment.fail_next_checkpoint_save();
        let mut engine = TransferEngine::new(config());
        assert!(
            engine
                .enqueue_upload(&mut environment, task, spec, &clock)
                .is_ok()
        );
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(environment.remote_count(key), 1);
        clock.advance(100);
        assert!(engine.step(&mut environment, &clock, &mut jitter).is_ok());
        assert_eq!(environment.remote_count(key), 1);
        assert_eq!(environment.upload_calls(key), 1);
        assert_eq!(
            engine.task(TransferId::new(13)).map(TransferTask::state),
            Some(TransferState::Completed)
        );
    }

    #[test]
    fn retry_policy_respects_structured_flood_wait_duration() {
        let error = TransferError::FloodWait {
            retry_after: Duration::from_secs(2),
        };
        assert!(error.is_retryable());
    }
}
