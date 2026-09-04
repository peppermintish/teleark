//! Frontend-neutral deterministic transfer scheduling and fake-backed workers.
//!
//! This crate owns policy and orchestration around TeleArk Core transfer state
//! machines. Telegram, SQL, GUI, and cryptographic implementations remain
//! behind project-owned ports. The included fake ports are test support, not a
//! production transport, filesystem, or integrity algorithm.

#![forbid(unsafe_code)]

mod adaptive;
mod engine;
mod error;
mod native;
mod pipeline;
mod ports;
mod progress;
mod retry;
mod scheduler;
mod types;

#[cfg(any(test, feature = "test-support"))]
mod fake;

pub use adaptive::{
    AdaptiveControllerConfig, AdaptiveTransferController, ControllerDecision,
    ControllerDecisionOutcome, ControllerDecisionReason, ControllerPhase, DOWNLOAD_PART_SIZE_BYTES,
    LaneTelemetry, MemoryCounters, ParameterBounds, PartCounters, PerformanceSample, QueueCounters,
    SoftLimitPolicy, TransferBottleneck, TransferControlParameters, TransferTelemetrySnapshot,
    TunableParameter,
};
pub use engine::{
    CrashPoint, DownloadSpec, StepReport, TransferEngine, TransferEngineConfig, UploadSpec,
};
pub use error::{ConfigurationError, TransferEngineError};
#[cfg(any(test, feature = "test-support"))]
pub use fake::{FakeClock, FakeEnvironment, FakeUploadBehavior, SequenceJitter};
pub use native::{Blake3Digest, NativeFileSystem};
pub use pipeline::{
    EncryptionPipelineConfig, EncryptionPipelineError, EncryptionPipelineReport, PipelineEvent,
    PipelinePart, PipelineStage, run_encryption_upload_pipeline,
};
pub use ports::{
    CheckpointPort, Clock, DigestPort, FileSystemPort, JitterSource, RemoteTransport, SourcePort,
    TransferIo, UploadError,
};
pub use progress::{ProgressConfig, TransferEvent};
pub use retry::{RetryDecision, RetryPolicy};
pub use scheduler::{
    ScheduledWork, SchedulerConfig, SchedulerSnapshot, TransferScheduler, WorkItem,
};
pub use types::{
    ContentDigest, DestinationId, PartCheckpoint, RemoteObject, RemotePartKey, SourceId,
    SourceIdentity, TransferCheckpoint, VerifiedPart,
};
