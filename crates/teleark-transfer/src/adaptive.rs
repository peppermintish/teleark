use std::collections::{BTreeMap, VecDeque};

use crate::ConfigurationError;

/// One mebibyte application-level download part. A transport adapter may need
/// more than one protocol RPC to fill it when its hard per-request limit is
/// smaller.
pub const DOWNLOAD_PART_SIZE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerPhase {
    Ramp,
    Probe,
    Stable,
    Recover,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftLimitPolicy {
    Respect,
    AdaptiveOverride,
    Ignore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferBottleneck {
    Unknown,
    EncryptionCpu,
    TelegramOrNetwork,
    Disk,
    Memory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TunableParameter {
    TransferConnections,
    InflightRpcsPerConnection,
    ActiveFiles,
    InflightPartsPerFile,
    EncryptionWorkers,
    EncryptedQueueDepth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerDecisionOutcome {
    Probe,
    Keep,
    Confirm,
    Platform,
    Rollback,
    Recover,
    RespectSoftLimit,
    OverrideSoftLimit,
    IgnoreSoftLimit,
    PauseLane,
    ResumeLane,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerDecisionReason {
    InitialRamp,
    InflightBelowBdpTarget,
    ThroughputImproved,
    ThroughputNeedsConfirmation,
    ThroughputGainBelowThreshold,
    ThroughputRegressed,
    EncryptionStarvedNetwork,
    NetworkBackpressuredEncryption,
    SmallFileQueueNeedsSlots,
    LargeFilePipelineNeedsParts,
    MemoryBudgetPressure,
    DiskLimited,
    PartRetryRequired,
    FloodWaitRequired,
    FloodWaitExpired,
    TelegramSoftLimitConflict,
    AllParametersAtPlatform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferControlParameters {
    pub transfer_connection_count: u16,
    pub inflight_rpcs_per_connection: u16,
    pub active_file_count: u16,
    pub inflight_parts_per_file: u16,
    pub encryption_worker_count: u16,
    pub encrypted_part_queue_depth: u16,
}

impl TransferControlParameters {
    #[must_use]
    pub const fn conservative_upload() -> Self {
        Self {
            transfer_connection_count: 1,
            inflight_rpcs_per_connection: 4,
            active_file_count: 1,
            inflight_parts_per_file: 4,
            encryption_worker_count: 1,
            encrypted_part_queue_depth: 2,
        }
    }

    #[must_use]
    pub const fn conservative_download() -> Self {
        Self {
            encryption_worker_count: 0,
            encrypted_part_queue_depth: 0,
            ..Self::conservative_upload()
        }
    }

    fn value(self, parameter: TunableParameter) -> u16 {
        match parameter {
            TunableParameter::TransferConnections => self.transfer_connection_count,
            TunableParameter::InflightRpcsPerConnection => self.inflight_rpcs_per_connection,
            TunableParameter::ActiveFiles => self.active_file_count,
            TunableParameter::InflightPartsPerFile => self.inflight_parts_per_file,
            TunableParameter::EncryptionWorkers => self.encryption_worker_count,
            TunableParameter::EncryptedQueueDepth => self.encrypted_part_queue_depth,
        }
    }

    fn set_value(&mut self, parameter: TunableParameter, value: u16) {
        match parameter {
            TunableParameter::TransferConnections => self.transfer_connection_count = value,
            TunableParameter::InflightRpcsPerConnection => {
                self.inflight_rpcs_per_connection = value;
            }
            TunableParameter::ActiveFiles => self.active_file_count = value,
            TunableParameter::InflightPartsPerFile => self.inflight_parts_per_file = value,
            TunableParameter::EncryptionWorkers => self.encryption_worker_count = value,
            TunableParameter::EncryptedQueueDepth => self.encrypted_part_queue_depth = value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParameterBounds {
    pub minimum: u16,
    pub maximum: u16,
    pub probe_step: u16,
}

impl ParameterBounds {
    pub fn new(minimum: u16, maximum: u16, probe_step: u16) -> Result<Self, ConfigurationError> {
        if probe_step == 0 || minimum > maximum {
            return Err(ConfigurationError::InvalidAdaptivePolicy {
                field: "parameter bounds",
            });
        }
        Ok(Self {
            minimum,
            maximum,
            probe_step,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdaptiveControllerConfig {
    pub transfer_connections: ParameterBounds,
    pub inflight_rpcs_per_connection: ParameterBounds,
    pub active_files: ParameterBounds,
    pub inflight_parts_per_file: ParameterBounds,
    pub encryption_workers: ParameterBounds,
    pub encrypted_queue_depth: ParameterBounds,
    pub keep_gain_basis_points: u16,
    pub confirm_gain_basis_points: u16,
    pub probe_settle_millis: u64,
    pub target_bdp_multiplier_milli: u16,
    pub memory_budget_bytes: u64,
    pub decision_history_capacity: usize,
    pub soft_limit_policy: SoftLimitPolicy,
    pub telegram_soft_active_file_limit: Option<u16>,
}

impl AdaptiveControllerConfig {
    pub fn maximum_throughput(
        memory_budget_bytes: u64,
        available_parallelism: u16,
    ) -> Result<Self, ConfigurationError> {
        if memory_budget_bytes < DOWNLOAD_PART_SIZE_BYTES || available_parallelism == 0 {
            return Err(ConfigurationError::InvalidAdaptivePolicy {
                field: "maximum throughput resources",
            });
        }
        Ok(Self {
            transfer_connections: ParameterBounds::new(1, 16, 1)?,
            inflight_rpcs_per_connection: ParameterBounds::new(1, 32, 4)?,
            active_files: ParameterBounds::new(1, 32, 1)?,
            inflight_parts_per_file: ParameterBounds::new(1, 64, 4)?,
            encryption_workers: ParameterBounds::new(1, available_parallelism, 1)?,
            encrypted_queue_depth: ParameterBounds::new(1, 64, 2)?,
            keep_gain_basis_points: 300,
            confirm_gain_basis_points: 100,
            probe_settle_millis: 1_000,
            target_bdp_multiplier_milli: 1_750,
            memory_budget_bytes,
            decision_history_capacity: 2_048,
            soft_limit_policy: SoftLimitPolicy::AdaptiveOverride,
            telegram_soft_active_file_limit: Some(2),
        })
    }

    pub fn validate(self) -> Result<Self, ConfigurationError> {
        if self.confirm_gain_basis_points > self.keep_gain_basis_points
            || self.keep_gain_basis_points > 10_000
            || self.probe_settle_millis == 0
            || !(1_500..=2_000).contains(&self.target_bdp_multiplier_milli)
            || self.memory_budget_bytes < DOWNLOAD_PART_SIZE_BYTES
            || self.decision_history_capacity == 0
        {
            return Err(ConfigurationError::InvalidAdaptivePolicy {
                field: "adaptive controller policy",
            });
        }
        Ok(self)
    }

    fn bounds(self, parameter: TunableParameter) -> ParameterBounds {
        match parameter {
            TunableParameter::TransferConnections => self.transfer_connections,
            TunableParameter::InflightRpcsPerConnection => self.inflight_rpcs_per_connection,
            TunableParameter::ActiveFiles => self.active_files,
            TunableParameter::InflightPartsPerFile => self.inflight_parts_per_file,
            TunableParameter::EncryptionWorkers => self.encryption_workers,
            TunableParameter::EncryptedQueueDepth => self.encrypted_queue_depth,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PartCounters {
    pub total_parts: u64,
    pub completed_parts: u64,
    pub inflight_parts: u64,
    pub retry_parts: u64,
    pub failed_parts: u64,
    pub missing_parts: u64,
    pub completed_parts_per_second_milli: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueCounters {
    pub small_files_queued: u64,
    pub small_files_active: u64,
    pub small_files_paused: u64,
    pub small_files_completed: u64,
    pub small_queue_weight: u16,
    pub large_files_queued: u64,
    pub large_files_active: u64,
    pub large_files_paused: u64,
    pub large_files_completed: u64,
    pub large_queue_weight: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryCounters {
    pub plaintext_buffer_bytes: u64,
    pub encrypted_buffer_bytes: u64,
    pub network_inflight_bytes: u64,
    pub writer_queue_bytes: u64,
}

impl MemoryCounters {
    #[must_use]
    pub fn total_bytes(self) -> u64 {
        self.plaintext_buffer_bytes
            .saturating_add(self.encrypted_buffer_bytes)
            .saturating_add(self.network_inflight_bytes)
            .saturating_add(self.writer_queue_bytes)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PerformanceSample {
    pub observed_at_millis: u64,
    pub goodput_bytes_per_second: u64,
    pub encryption_bytes_per_second: u64,
    pub disk_bytes_per_second: u64,
    pub round_trip_time_p95_millis: u64,
    pub inflight_bytes: u64,
    pub cpu_utilization_basis_points: u16,
    pub network_waiting_for_encryption_millis: u64,
    pub encryption_waiting_for_network_millis: u64,
    pub encrypted_queue_length: u16,
    pub active_small_files: u16,
    pub active_large_files: u16,
    pub flood_wait_seconds: Option<u32>,
    pub affected_lane: Option<u16>,
    pub parts: PartCounters,
    pub queues: QueueCounters,
    pub memory: MemoryCounters,
}

impl PerformanceSample {
    #[must_use]
    pub fn estimated_bdp_bytes(self) -> u64 {
        self.goodput_bytes_per_second
            .saturating_mul(self.round_trip_time_p95_millis)
            .saturating_div(1_000)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerDecision {
    pub sequence: u64,
    pub observed_at_millis: u64,
    pub phase: ControllerPhase,
    pub parameter: Option<TunableParameter>,
    pub before: TransferControlParameters,
    pub after: TransferControlParameters,
    pub baseline_goodput_bytes_per_second: u64,
    pub observed_goodput_bytes_per_second: u64,
    pub goodput_change_basis_points: i32,
    pub outcome: ControllerDecisionOutcome,
    pub reason: ControllerDecisionReason,
    pub affected_lane: Option<u16>,
    pub flood_wait_seconds: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LaneTelemetry {
    pub data_center_id: i32,
    pub lane_id: u16,
    pub inflight_rpc_count: u16,
    pub throughput_bytes_per_second: u64,
    pub round_trip_time_p95_millis: u64,
    pub error_count: u64,
    pub request_count: u64,
    pub paused_until_millis: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferTelemetrySnapshot {
    pub phase: ControllerPhase,
    pub parameters: TransferControlParameters,
    pub goodput_bytes_per_second: u64,
    pub encryption_bytes_per_second: u64,
    pub disk_bytes_per_second: u64,
    pub round_trip_time_p95_millis: u64,
    pub estimated_bdp_bytes: u64,
    pub inflight_bytes: u64,
    pub target_inflight_bytes: u64,
    pub cpu_utilization_basis_points: u16,
    pub encrypted_queue_length: u16,
    pub network_waiting_for_encryption_millis: u64,
    pub encryption_waiting_for_network_millis: u64,
    pub bottleneck: TransferBottleneck,
    pub parts: PartCounters,
    pub queues: QueueCounters,
    pub memory: MemoryCounters,
    pub memory_budget_bytes: u64,
    pub lanes: Vec<LaneTelemetry>,
    pub decisions: Vec<ControllerDecision>,
}

#[derive(Clone, Copy, Debug)]
struct PendingProbe {
    parameter: TunableParameter,
    before: TransferControlParameters,
    baseline_goodput_bytes_per_second: u64,
    confirmation_count: u8,
    started_at_millis: u64,
}

/// Per-DC goodput controller. It treats RTT as diagnostic evidence and never
/// reduces concurrency solely because RTT rose.
pub struct AdaptiveTransferController {
    config: AdaptiveControllerConfig,
    upload: bool,
    phase: ControllerPhase,
    parameters: TransferControlParameters,
    baseline_goodput_bytes_per_second: Option<u64>,
    pending_probe: Option<PendingProbe>,
    probe_cursor: usize,
    decision_sequence: u64,
    soft_limit_decision_recorded: bool,
    decisions: VecDeque<ControllerDecision>,
    lanes: BTreeMap<u16, LaneTelemetry>,
    latest_sample: PerformanceSample,
}

impl AdaptiveTransferController {
    pub fn new(config: AdaptiveControllerConfig, upload: bool) -> Result<Self, ConfigurationError> {
        let parameters = if upload {
            TransferControlParameters::conservative_upload()
        } else {
            TransferControlParameters::conservative_download()
        };
        Self::with_initial_parameters(config, upload, parameters)
    }

    pub fn with_initial_parameters(
        config: AdaptiveControllerConfig,
        upload: bool,
        mut parameters: TransferControlParameters,
    ) -> Result<Self, ConfigurationError> {
        let config = config.validate()?;
        clamp_parameters(&mut parameters, config);
        if !upload {
            parameters.encryption_worker_count = 0;
            parameters.encrypted_part_queue_depth = 0;
        }
        Ok(Self {
            config,
            upload,
            phase: ControllerPhase::Ramp,
            parameters,
            baseline_goodput_bytes_per_second: None,
            pending_probe: None,
            probe_cursor: 0,
            decision_sequence: 0,
            soft_limit_decision_recorded: false,
            decisions: VecDeque::with_capacity(config.decision_history_capacity),
            lanes: BTreeMap::new(),
            latest_sample: PerformanceSample::default(),
        })
    }

    #[must_use]
    pub const fn parameters(&self) -> TransferControlParameters {
        self.parameters
    }

    pub fn observe(&mut self, sample: PerformanceSample) -> ControllerDecision {
        self.latest_sample = sample;
        if let Some(wait_seconds) = sample.flood_wait_seconds {
            self.phase = ControllerPhase::Recover;
            if let Some(lane_id) = sample.affected_lane {
                let deadline = sample
                    .observed_at_millis
                    .saturating_add(u64::from(wait_seconds).saturating_mul(1_000));
                self.lanes.entry(lane_id).or_default().paused_until_millis = Some(deadline);
            }
            return self.record_decision(
                None,
                self.parameters,
                self.parameters,
                self.baseline_goodput_bytes_per_second.unwrap_or_default(),
                sample.goodput_bytes_per_second,
                ControllerDecisionOutcome::PauseLane,
                ControllerDecisionReason::FloodWaitRequired,
                sample.affected_lane,
                Some(wait_seconds),
            );
        }

        if let Some(lane_id) = self.resume_expired_lane(sample.observed_at_millis) {
            self.phase = ControllerPhase::Recover;
            return self.record_decision(
                None,
                self.parameters,
                self.parameters,
                self.baseline_goodput_bytes_per_second.unwrap_or_default(),
                sample.goodput_bytes_per_second,
                ControllerDecisionOutcome::ResumeLane,
                ControllerDecisionReason::FloodWaitExpired,
                Some(lane_id),
                None,
            );
        }

        if sample.memory.total_bytes() > self.config.memory_budget_bytes {
            self.phase = ControllerPhase::Recover;
            let before = self.parameters;
            let parameter = if self.upload && self.parameters.encryption_worker_count > 1 {
                TunableParameter::EncryptionWorkers
            } else {
                TunableParameter::InflightPartsPerFile
            };
            self.decrease(parameter);
            self.pending_probe = None;
            return self.record_decision(
                Some(parameter),
                before,
                self.parameters,
                self.baseline_goodput_bytes_per_second.unwrap_or_default(),
                sample.goodput_bytes_per_second,
                ControllerDecisionOutcome::Recover,
                ControllerDecisionReason::MemoryBudgetPressure,
                None,
                None,
            );
        }

        if self.upload
            && u32::from(sample.encrypted_queue_length).saturating_mul(100)
                >= u32::from(self.parameters.encrypted_part_queue_depth).saturating_mul(80)
            && self.parameters.encryption_worker_count > self.config.encryption_workers.minimum
        {
            self.phase = ControllerPhase::Recover;
            let before = self.parameters;
            self.decrease(TunableParameter::EncryptionWorkers);
            self.pending_probe = None;
            return self.record_decision(
                Some(TunableParameter::EncryptionWorkers),
                before,
                self.parameters,
                sample.goodput_bytes_per_second,
                sample.goodput_bytes_per_second,
                ControllerDecisionOutcome::Recover,
                ControllerDecisionReason::NetworkBackpressuredEncryption,
                None,
                None,
            );
        }

        if let Some(pending) = self.pending_probe {
            return self.evaluate_probe(pending, sample);
        }

        if self.baseline_goodput_bytes_per_second.is_none() {
            self.baseline_goodput_bytes_per_second = Some(sample.goodput_bytes_per_second);
        }

        if sample.active_small_files > sample.active_large_files
            && self.active_file_probe_crosses_soft_limit()
            && !self.soft_limit_decision_recorded
        {
            self.soft_limit_decision_recorded = true;
            return match self.config.soft_limit_policy {
                SoftLimitPolicy::Respect => {
                    self.phase = ControllerPhase::Stable;
                    self.record_decision(
                        Some(TunableParameter::ActiveFiles),
                        self.parameters,
                        self.parameters,
                        sample.goodput_bytes_per_second,
                        sample.goodput_bytes_per_second,
                        ControllerDecisionOutcome::RespectSoftLimit,
                        ControllerDecisionReason::TelegramSoftLimitConflict,
                        None,
                        None,
                    )
                }
                SoftLimitPolicy::AdaptiveOverride | SoftLimitPolicy::Ignore => self.start_probe(
                    TunableParameter::ActiveFiles,
                    ControllerDecisionReason::TelegramSoftLimitConflict,
                    sample,
                ),
            };
        }

        if let Some((parameter, reason)) = self.next_probe(sample) {
            return self.start_probe(parameter, reason, sample);
        }

        self.phase = ControllerPhase::Stable;
        self.record_decision(
            None,
            self.parameters,
            self.parameters,
            self.baseline_goodput_bytes_per_second.unwrap_or_default(),
            sample.goodput_bytes_per_second,
            ControllerDecisionOutcome::Platform,
            ControllerDecisionReason::AllParametersAtPlatform,
            None,
            None,
        )
    }

    pub fn recover_from_part_retry(&mut self, sample: PerformanceSample) -> ControllerDecision {
        self.latest_sample = sample;
        self.phase = ControllerPhase::Recover;
        let before = self.parameters;
        self.decrease(TunableParameter::InflightPartsPerFile);
        self.pending_probe = None;
        self.baseline_goodput_bytes_per_second = Some(sample.goodput_bytes_per_second);
        self.record_decision(
            Some(TunableParameter::InflightPartsPerFile),
            before,
            self.parameters,
            sample.goodput_bytes_per_second,
            sample.goodput_bytes_per_second,
            ControllerDecisionOutcome::Recover,
            ControllerDecisionReason::PartRetryRequired,
            sample.affected_lane,
            sample.flood_wait_seconds,
        )
    }

    pub fn update_lane(&mut self, telemetry: LaneTelemetry) {
        self.lanes.insert(telemetry.lane_id, telemetry);
    }

    #[must_use]
    pub fn snapshot(&self) -> TransferTelemetrySnapshot {
        let estimated_bdp_bytes = self.latest_sample.estimated_bdp_bytes();
        TransferTelemetrySnapshot {
            phase: self.phase,
            parameters: self.parameters,
            goodput_bytes_per_second: self.latest_sample.goodput_bytes_per_second,
            encryption_bytes_per_second: self.latest_sample.encryption_bytes_per_second,
            disk_bytes_per_second: self.latest_sample.disk_bytes_per_second,
            round_trip_time_p95_millis: self.latest_sample.round_trip_time_p95_millis,
            estimated_bdp_bytes,
            inflight_bytes: self.latest_sample.inflight_bytes,
            target_inflight_bytes: estimated_bdp_bytes
                .saturating_mul(u64::from(self.config.target_bdp_multiplier_milli))
                .saturating_div(1_000),
            cpu_utilization_basis_points: self.latest_sample.cpu_utilization_basis_points,
            encrypted_queue_length: self.latest_sample.encrypted_queue_length,
            network_waiting_for_encryption_millis: self
                .latest_sample
                .network_waiting_for_encryption_millis,
            encryption_waiting_for_network_millis: self
                .latest_sample
                .encryption_waiting_for_network_millis,
            bottleneck: classify_bottleneck(self.latest_sample, self.config.memory_budget_bytes),
            parts: self.latest_sample.parts,
            queues: self.latest_sample.queues,
            memory: self.latest_sample.memory,
            memory_budget_bytes: self.config.memory_budget_bytes,
            lanes: self.lanes.values().copied().collect(),
            decisions: self.decisions.iter().copied().collect(),
        }
    }

    fn start_probe(
        &mut self,
        parameter: TunableParameter,
        reason: ControllerDecisionReason,
        sample: PerformanceSample,
    ) -> ControllerDecision {
        let before = self.parameters;
        self.increase(parameter);
        if before == self.parameters {
            return self.record_decision(
                Some(parameter),
                before,
                before,
                sample.goodput_bytes_per_second,
                sample.goodput_bytes_per_second,
                ControllerDecisionOutcome::Platform,
                ControllerDecisionReason::AllParametersAtPlatform,
                None,
                None,
            );
        }
        self.phase = if self.baseline_goodput_bytes_per_second == Some(0) {
            ControllerPhase::Ramp
        } else {
            ControllerPhase::Probe
        };
        self.pending_probe = Some(PendingProbe {
            parameter,
            before,
            baseline_goodput_bytes_per_second: sample.goodput_bytes_per_second,
            confirmation_count: 0,
            started_at_millis: sample.observed_at_millis,
        });
        let crosses_soft_limit = parameter == TunableParameter::ActiveFiles
            && self
                .config
                .telegram_soft_active_file_limit
                .is_some_and(|limit| before.active_file_count >= limit);
        let outcome = if crosses_soft_limit {
            match self.config.soft_limit_policy {
                SoftLimitPolicy::Respect => ControllerDecisionOutcome::RespectSoftLimit,
                SoftLimitPolicy::AdaptiveOverride => ControllerDecisionOutcome::OverrideSoftLimit,
                SoftLimitPolicy::Ignore => ControllerDecisionOutcome::IgnoreSoftLimit,
            }
        } else {
            ControllerDecisionOutcome::Probe
        };
        self.record_decision(
            Some(parameter),
            before,
            self.parameters,
            sample.goodput_bytes_per_second,
            sample.goodput_bytes_per_second,
            outcome,
            reason,
            None,
            None,
        )
    }

    fn evaluate_probe(
        &mut self,
        pending: PendingProbe,
        sample: PerformanceSample,
    ) -> ControllerDecision {
        if sample.observed_at_millis
            < pending
                .started_at_millis
                .saturating_add(self.config.probe_settle_millis)
        {
            self.phase = ControllerPhase::Probe;
            return self.record_decision(
                Some(pending.parameter),
                pending.before,
                self.parameters,
                pending.baseline_goodput_bytes_per_second,
                sample.goodput_bytes_per_second,
                ControllerDecisionOutcome::Confirm,
                ControllerDecisionReason::ThroughputNeedsConfirmation,
                None,
                None,
            );
        }
        let change = percentage_change_basis_points(
            pending.baseline_goodput_bytes_per_second,
            sample.goodput_bytes_per_second,
        );
        if change >= i32::from(self.config.keep_gain_basis_points) {
            self.pending_probe = None;
            self.baseline_goodput_bytes_per_second = Some(sample.goodput_bytes_per_second);
            self.phase = ControllerPhase::Probe;
            return self.record_decision(
                Some(pending.parameter),
                pending.before,
                self.parameters,
                pending.baseline_goodput_bytes_per_second,
                sample.goodput_bytes_per_second,
                ControllerDecisionOutcome::Keep,
                ControllerDecisionReason::ThroughputImproved,
                None,
                None,
            );
        }
        if change >= i32::from(self.config.confirm_gain_basis_points)
            && pending.confirmation_count == 0
        {
            self.pending_probe = Some(PendingProbe {
                confirmation_count: 1,
                ..pending
            });
            return self.record_decision(
                Some(pending.parameter),
                pending.before,
                self.parameters,
                pending.baseline_goodput_bytes_per_second,
                sample.goodput_bytes_per_second,
                ControllerDecisionOutcome::Confirm,
                ControllerDecisionReason::ThroughputNeedsConfirmation,
                None,
                None,
            );
        }

        self.parameters = pending.before;
        self.pending_probe = None;
        self.phase = if change < 0 {
            ControllerPhase::Recover
        } else {
            ControllerPhase::Stable
        };
        self.record_decision(
            Some(pending.parameter),
            self.parameters_with_probe_value(pending),
            pending.before,
            pending.baseline_goodput_bytes_per_second,
            sample.goodput_bytes_per_second,
            if change < 0 {
                ControllerDecisionOutcome::Rollback
            } else {
                ControllerDecisionOutcome::Platform
            },
            if change < 0 {
                ControllerDecisionReason::ThroughputRegressed
            } else {
                ControllerDecisionReason::ThroughputGainBelowThreshold
            },
            None,
            None,
        )
    }

    fn next_probe(
        &mut self,
        sample: PerformanceSample,
    ) -> Option<(TunableParameter, ControllerDecisionReason)> {
        if self.upload
            && sample.network_waiting_for_encryption_millis
                > sample
                    .encryption_waiting_for_network_millis
                    .saturating_mul(2)
            && self.can_increase(TunableParameter::EncryptionWorkers)
        {
            return Some((
                TunableParameter::EncryptionWorkers,
                ControllerDecisionReason::EncryptionStarvedNetwork,
            ));
        }
        if sample.active_small_files > sample.active_large_files
            && self.soft_limit_allows_probe()
            && self.can_increase(TunableParameter::ActiveFiles)
        {
            return Some((
                TunableParameter::ActiveFiles,
                ControllerDecisionReason::SmallFileQueueNeedsSlots,
            ));
        }
        if sample.active_large_files > 0
            && self.can_increase(TunableParameter::InflightPartsPerFile)
        {
            return Some((
                TunableParameter::InflightPartsPerFile,
                ControllerDecisionReason::LargeFilePipelineNeedsParts,
            ));
        }
        let target_inflight = sample
            .estimated_bdp_bytes()
            .saturating_mul(u64::from(self.config.target_bdp_multiplier_milli))
            .saturating_div(1_000);
        let reason = if sample.inflight_bytes < target_inflight {
            ControllerDecisionReason::InflightBelowBdpTarget
        } else {
            ControllerDecisionReason::InitialRamp
        };
        let order = [
            TunableParameter::InflightRpcsPerConnection,
            TunableParameter::TransferConnections,
            TunableParameter::InflightPartsPerFile,
            TunableParameter::ActiveFiles,
            TunableParameter::EncryptedQueueDepth,
            TunableParameter::EncryptionWorkers,
        ];
        for _ in 0..order.len() {
            let parameter = order[self.probe_cursor % order.len()];
            self.probe_cursor = self.probe_cursor.saturating_add(1);
            if (!self.upload
                && matches!(
                    parameter,
                    TunableParameter::EncryptionWorkers | TunableParameter::EncryptedQueueDepth
                ))
                || (parameter == TunableParameter::ActiveFiles && !self.soft_limit_allows_probe())
            {
                continue;
            }
            if self.can_increase(parameter) {
                return Some((parameter, reason));
            }
        }
        None
    }

    fn soft_limit_allows_probe(&self) -> bool {
        let Some(limit) = self.config.telegram_soft_active_file_limit else {
            return true;
        };
        self.parameters.active_file_count < limit
            || !matches!(self.config.soft_limit_policy, SoftLimitPolicy::Respect)
    }

    fn active_file_probe_crosses_soft_limit(&self) -> bool {
        self.config
            .telegram_soft_active_file_limit
            .is_some_and(|limit| {
                self.parameters.active_file_count >= limit
                    && self.can_increase(TunableParameter::ActiveFiles)
            })
    }

    fn can_increase(&self, parameter: TunableParameter) -> bool {
        self.parameters.value(parameter) < self.config.bounds(parameter).maximum
    }

    fn increase(&mut self, parameter: TunableParameter) {
        let bounds = self.config.bounds(parameter);
        let current = self.parameters.value(parameter);
        self.parameters.set_value(
            parameter,
            current
                .saturating_add(bounds.probe_step)
                .min(bounds.maximum),
        );
    }

    fn decrease(&mut self, parameter: TunableParameter) {
        let bounds = self.config.bounds(parameter);
        let current = self.parameters.value(parameter);
        self.parameters.set_value(
            parameter,
            current
                .saturating_sub(bounds.probe_step)
                .max(bounds.minimum),
        );
    }

    fn parameters_with_probe_value(&self, pending: PendingProbe) -> TransferControlParameters {
        let mut probed = pending.before;
        let bounds = self.config.bounds(pending.parameter);
        probed.set_value(
            pending.parameter,
            pending
                .before
                .value(pending.parameter)
                .saturating_add(bounds.probe_step)
                .min(bounds.maximum),
        );
        probed
    }

    fn resume_expired_lane(&mut self, observed_at_millis: u64) -> Option<u16> {
        for lane in self.lanes.values_mut() {
            if lane
                .paused_until_millis
                .is_some_and(|deadline| deadline <= observed_at_millis)
            {
                lane.paused_until_millis = None;
                return Some(lane.lane_id);
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn record_decision(
        &mut self,
        parameter: Option<TunableParameter>,
        before: TransferControlParameters,
        after: TransferControlParameters,
        baseline_goodput_bytes_per_second: u64,
        observed_goodput_bytes_per_second: u64,
        outcome: ControllerDecisionOutcome,
        reason: ControllerDecisionReason,
        affected_lane: Option<u16>,
        flood_wait_seconds: Option<u32>,
    ) -> ControllerDecision {
        self.decision_sequence = self.decision_sequence.saturating_add(1);
        let decision = ControllerDecision {
            sequence: self.decision_sequence,
            observed_at_millis: self.latest_sample.observed_at_millis,
            phase: self.phase,
            parameter,
            before,
            after,
            baseline_goodput_bytes_per_second,
            observed_goodput_bytes_per_second,
            goodput_change_basis_points: percentage_change_basis_points(
                baseline_goodput_bytes_per_second,
                observed_goodput_bytes_per_second,
            ),
            outcome,
            reason,
            affected_lane,
            flood_wait_seconds,
        };
        if self.decisions.len() >= self.config.decision_history_capacity {
            self.decisions.pop_front();
        }
        self.decisions.push_back(decision);
        decision
    }
}

fn clamp_parameters(parameters: &mut TransferControlParameters, config: AdaptiveControllerConfig) {
    for parameter in [
        TunableParameter::TransferConnections,
        TunableParameter::InflightRpcsPerConnection,
        TunableParameter::ActiveFiles,
        TunableParameter::InflightPartsPerFile,
        TunableParameter::EncryptionWorkers,
        TunableParameter::EncryptedQueueDepth,
    ] {
        if !matches!(
            parameter,
            TunableParameter::EncryptionWorkers | TunableParameter::EncryptedQueueDepth
        ) || parameters.encryption_worker_count != 0
        {
            let bounds = config.bounds(parameter);
            parameters.set_value(
                parameter,
                parameters
                    .value(parameter)
                    .clamp(bounds.minimum, bounds.maximum),
            );
        }
    }
}

fn percentage_change_basis_points(baseline: u64, observed: u64) -> i32 {
    if baseline == 0 {
        return if observed == 0 { 0 } else { 10_000 };
    }
    let difference = i128::from(observed) - i128::from(baseline);
    let basis_points = difference
        .saturating_mul(10_000)
        .checked_div(i128::from(baseline))
        .unwrap_or_default();
    i32::try_from(basis_points).unwrap_or_else(|_| {
        if basis_points.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }
    })
}

fn classify_bottleneck(sample: PerformanceSample, memory_budget_bytes: u64) -> TransferBottleneck {
    if sample.memory.total_bytes() >= memory_budget_bytes {
        TransferBottleneck::Memory
    } else if sample.disk_bytes_per_second != 0
        && sample.disk_bytes_per_second < sample.goodput_bytes_per_second
    {
        TransferBottleneck::Disk
    } else if sample.network_waiting_for_encryption_millis
        > sample
            .encryption_waiting_for_network_millis
            .saturating_mul(2)
    {
        TransferBottleneck::EncryptionCpu
    } else if sample.encryption_waiting_for_network_millis
        > sample
            .network_waiting_for_encryption_millis
            .saturating_mul(2)
    {
        TransferBottleneck::TelegramOrNetwork
    } else {
        TransferBottleneck::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(upload: bool) -> AdaptiveTransferController {
        let config = AdaptiveControllerConfig::maximum_throughput(512 * 1024 * 1024, 8)
            .expect("valid adaptive configuration");
        AdaptiveTransferController::new(config, upload).expect("valid controller")
    }

    fn sample(observed_at_millis: u64, goodput: u64) -> PerformanceSample {
        PerformanceSample {
            observed_at_millis,
            goodput_bytes_per_second: goodput,
            round_trip_time_p95_millis: 100,
            inflight_bytes: 1024 * 1024,
            ..PerformanceSample::default()
        }
    }

    #[test]
    fn a_three_percent_probe_is_kept() {
        let mut controller = controller(false);
        let probe = controller.observe(sample(0, 100_000_000));
        assert_eq!(probe.outcome, ControllerDecisionOutcome::Probe);
        let result = controller.observe(sample(1_000, 103_000_000));
        assert_eq!(result.outcome, ControllerDecisionOutcome::Keep);
        assert_eq!(result.goodput_change_basis_points, 300);
        assert_eq!(controller.parameters(), probe.after);
    }

    #[test]
    fn a_probe_waits_for_its_settle_window_before_evaluation() {
        let mut controller = controller(false);
        let probe = controller.observe(sample(0, 100_000_000));
        let early = controller.observe(sample(999, 150_000_000));
        assert_eq!(early.outcome, ControllerDecisionOutcome::Confirm);
        assert_eq!(controller.parameters(), probe.after);
        let settled = controller.observe(sample(1_000, 103_000_000));
        assert_eq!(settled.outcome, ControllerDecisionOutcome::Keep);
    }

    #[test]
    fn a_part_retry_immediately_reduces_per_file_inflight() {
        let mut config = AdaptiveControllerConfig::maximum_throughput(64 * 1024 * 1024, 4)
            .expect("valid config");
        config.transfer_connections = ParameterBounds::new(1, 1, 1).expect("fixed connections");
        config.inflight_rpcs_per_connection = ParameterBounds::new(4, 4, 1).expect("fixed RPCs");
        config.active_files = ParameterBounds::new(1, 1, 1).expect("fixed active files");
        let mut controller = AdaptiveTransferController::new(config, false).expect("controller");
        let mut initial_sample = sample(0, 100_000_000);
        initial_sample.active_large_files = 1;
        let probe = controller.observe(initial_sample);
        assert!(probe.after.inflight_parts_per_file > probe.before.inflight_parts_per_file);
        let decision = controller.recover_from_part_retry(sample(500, 80_000_000));
        assert_eq!(decision.outcome, ControllerDecisionOutcome::Recover);
        assert_eq!(decision.reason, ControllerDecisionReason::PartRetryRequired);
        assert_eq!(controller.parameters(), probe.before);
    }

    #[test]
    fn a_sub_one_percent_probe_rolls_back_to_the_previous_parameters() {
        let mut controller = controller(false);
        let probe = controller.observe(sample(0, 100_000_000));
        let result = controller.observe(sample(1_000, 100_600_000));
        assert_eq!(result.outcome, ControllerDecisionOutcome::Platform);
        assert_eq!(controller.parameters(), probe.before);
    }

    #[test]
    fn rtt_growth_alone_does_not_rollback_a_good_probe() {
        let mut controller = controller(false);
        let probe = controller.observe(sample(0, 100_000_000));
        let mut improved = sample(1_000, 110_000_000);
        improved.round_trip_time_p95_millis = 400;
        let result = controller.observe(improved);
        assert_eq!(result.outcome, ControllerDecisionOutcome::Keep);
        assert_eq!(controller.parameters(), probe.after);
    }

    #[test]
    fn flood_wait_pauses_only_the_affected_lane() {
        let mut controller = controller(false);
        controller.update_lane(LaneTelemetry {
            data_center_id: 4,
            lane_id: 2,
            ..LaneTelemetry::default()
        });
        let mut flood = sample(5_000, 80_000_000);
        flood.flood_wait_seconds = Some(17);
        flood.affected_lane = Some(2);
        let decision = controller.observe(flood);
        assert_eq!(decision.outcome, ControllerDecisionOutcome::PauseLane);
        assert_eq!(decision.flood_wait_seconds, Some(17));
        assert_eq!(
            controller.snapshot().lanes[0].paused_until_millis,
            Some(22_000)
        );

        let resumed = controller.observe(sample(22_000, 80_000_000));
        assert_eq!(resumed.outcome, ControllerDecisionOutcome::ResumeLane);
        assert_eq!(resumed.affected_lane, Some(2));
        assert_eq!(controller.snapshot().lanes[0].paused_until_millis, None);
    }

    #[test]
    fn respect_policy_records_the_soft_limit_conflict_without_crossing_it() {
        let mut config = AdaptiveControllerConfig::maximum_throughput(512 * 1024 * 1024, 8)
            .expect("valid adaptive configuration");
        config.soft_limit_policy = SoftLimitPolicy::Respect;
        let initial = TransferControlParameters {
            active_file_count: 2,
            ..TransferControlParameters::conservative_download()
        };
        let mut controller =
            AdaptiveTransferController::with_initial_parameters(config, false, initial)
                .expect("valid controller");
        let mut small_files = sample(1_000, 80_000_000);
        small_files.active_small_files = 8;

        let decision = controller.observe(small_files);

        assert_eq!(
            decision.outcome,
            ControllerDecisionOutcome::RespectSoftLimit
        );
        assert_eq!(decision.before.active_file_count, 2);
        assert_eq!(decision.after.active_file_count, 2);
    }

    #[test]
    fn adaptive_policy_records_an_explicit_soft_limit_override_probe() {
        let config = AdaptiveControllerConfig::maximum_throughput(512 * 1024 * 1024, 8)
            .expect("valid adaptive configuration");
        let initial = TransferControlParameters {
            active_file_count: 2,
            ..TransferControlParameters::conservative_download()
        };
        let mut controller =
            AdaptiveTransferController::with_initial_parameters(config, false, initial)
                .expect("valid controller");
        let mut small_files = sample(1_000, 80_000_000);
        small_files.active_small_files = 8;

        let decision = controller.observe(small_files);

        assert_eq!(
            decision.outcome,
            ControllerDecisionOutcome::OverrideSoftLimit
        );
        assert_eq!(decision.before.active_file_count, 2);
        assert_eq!(decision.after.active_file_count, 3);
        assert_eq!(
            decision.reason,
            ControllerDecisionReason::TelegramSoftLimitConflict
        );
    }

    #[test]
    fn memory_pressure_reduces_upload_parallelism() {
        let mut controller = controller(true);
        let mut pressure = sample(1_000, 50_000_000);
        pressure.memory.plaintext_buffer_bytes = 600 * 1024 * 1024;
        let before = controller.parameters();
        let decision = controller.observe(pressure);
        assert_eq!(decision.outcome, ControllerDecisionOutcome::Recover);
        assert_eq!(
            decision.reason,
            ControllerDecisionReason::MemoryBudgetPressure
        );
        assert!(
            controller.parameters().inflight_parts_per_file < before.inflight_parts_per_file
                || controller.parameters().encryption_worker_count < before.encryption_worker_count
        );
    }

    #[test]
    fn encrypted_queue_backpressure_classifies_network_limit() {
        let mut controller = controller(true);
        let mut network_limited = sample(1_000, 50_000_000);
        network_limited.encryption_waiting_for_network_millis = 500;
        network_limited.network_waiting_for_encryption_millis = 10;
        controller.observe(network_limited);
        assert_eq!(
            controller.snapshot().bottleneck,
            TransferBottleneck::TelegramOrNetwork
        );
    }

    #[test]
    fn history_is_bounded() {
        let mut config = AdaptiveControllerConfig::maximum_throughput(64 * 1024 * 1024, 2)
            .expect("valid config");
        config.decision_history_capacity = 3;
        let mut controller =
            AdaptiveTransferController::new(config, false).expect("valid controller");
        for index in 0..10 {
            controller.observe(sample(index, 10_000_000));
        }
        assert_eq!(controller.snapshot().decisions.len(), 3);
    }
}
