use std::collections::{BTreeMap, VecDeque};

use teleark_core::{TransferError, TransferId, TransferProgress, TransferState, TransferTask};

use crate::{ConfigurationError, TransferEngineError};

/// Bounded progress coalescing policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProgressConfig {
    min_interval_ms: u64,
    event_capacity: usize,
    max_tracked_transfers: usize,
}

impl ProgressConfig {
    pub fn new(
        min_interval_ms: u64,
        event_capacity: usize,
        max_tracked_transfers: usize,
    ) -> Result<Self, ConfigurationError> {
        if min_interval_ms == 0 {
            return Err(ConfigurationError::InvalidProgressPolicy {
                field: "min_interval_ms",
            });
        }
        if event_capacity == 0 {
            return Err(ConfigurationError::InvalidProgressPolicy {
                field: "event_capacity",
            });
        }
        if max_tracked_transfers == 0 {
            return Err(ConfigurationError::InvalidProgressPolicy {
                field: "max_tracked_transfers",
            });
        }
        Ok(Self {
            min_interval_ms,
            event_capacity,
            max_tracked_transfers,
        })
    }
}

/// Coalesced frontend-neutral task update.
#[derive(Clone, Debug, PartialEq)]
pub struct TransferEvent {
    pub transfer_id: TransferId,
    pub state: TransferState,
    pub progress: TransferProgress,
    pub error: Option<TransferError>,
    pub observed_at_ms: u64,
    pub significant: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct LastEmission {
    state: TransferState,
    progress: TransferProgress,
    error: Option<TransferError>,
    at_ms: u64,
}

pub(crate) struct ProgressEmitter {
    config: ProgressConfig,
    last: BTreeMap<TransferId, LastEmission>,
    events: VecDeque<TransferEvent>,
}

impl ProgressEmitter {
    pub(crate) fn new(config: ProgressConfig) -> Self {
        Self {
            config,
            last: BTreeMap::new(),
            events: VecDeque::with_capacity(config.event_capacity),
        }
    }

    pub(crate) fn observe(
        &mut self,
        task: &TransferTask,
        error: Option<&TransferError>,
        now_ms: u64,
    ) -> Result<(), TransferEngineError> {
        let transfer_id = task.id();
        let state = task.state();
        let progress = task.progress();
        let error = error.cloned();
        let previous = self.last.get(&transfer_id);
        let significant = previous.is_none_or(|old| old.state != state || old.error != error);
        let changed = previous.is_none_or(|old| old.progress != progress);
        let due = previous
            .is_none_or(|old| now_ms.saturating_sub(old.at_ms) >= self.config.min_interval_ms);
        if !significant && (!changed || !due) {
            return Ok(());
        }
        if previous.is_none() && self.last.len() >= self.config.max_tracked_transfers {
            return Err(TransferEngineError::EventBackpressure {
                capacity: self.config.max_tracked_transfers,
            });
        }
        let event = TransferEvent {
            transfer_id,
            state,
            progress,
            error: error.clone(),
            observed_at_ms: now_ms,
            significant,
        };
        self.push(event)?;
        self.last.insert(
            transfer_id,
            LastEmission {
                state,
                progress,
                error,
                at_ms: now_ms,
            },
        );
        if state.is_terminal() {
            self.last.remove(&transfer_id);
        }
        Ok(())
    }

    pub(crate) fn drain(&mut self) -> Vec<TransferEvent> {
        self.events.drain(..).collect()
    }

    fn push(&mut self, event: TransferEvent) -> Result<(), TransferEngineError> {
        if self.events.len() < self.config.event_capacity {
            self.events.push_back(event);
            return Ok(());
        }
        if !event.significant {
            if let Some(existing) =
                self.events.iter_mut().rev().find(|existing| {
                    !existing.significant && existing.transfer_id == event.transfer_id
                })
            {
                *existing = event;
            }
            return Ok(());
        }
        if let Some(position) = self.events.iter().position(|queued| !queued.significant) {
            self.events.remove(position);
            self.events.push_back(event);
            return Ok(());
        }
        Err(TransferEngineError::EventBackpressure {
            capacity: self.config.event_capacity,
        })
    }
}

#[cfg(test)]
mod tests {
    use teleark_core::{
        LogicalFileId, PartIndex, PartState, TransferDirection, TransferPriority, TransferState,
    };

    use super::*;

    fn task() -> TransferTask {
        match TransferTask::try_new(
            TransferId::new(1),
            LogicalFileId::new(2),
            TransferDirection::Upload,
            10,
            TransferPriority::NORMAL,
            &[10],
        ) {
            Ok(value) => value,
            Err(error) => panic!("test transfer failed: {error}"),
        }
    }

    #[test]
    fn progress_is_throttled_but_state_changes_are_immediate() {
        let config = match ProgressConfig::new(100, 8, 4) {
            Ok(value) => value,
            Err(error) => panic!("test progress config failed: {error}"),
        };
        let mut emitter = ProgressEmitter::new(config);
        let mut task = task();
        assert!(emitter.observe(&task, None, 0).is_ok());
        assert!(task.transition_to(TransferState::Running).is_ok());
        assert!(
            task.transition_part(PartIndex::new(0), PartState::Running)
                .is_ok()
        );
        assert!(task.set_part_progress(PartIndex::new(0), 1).is_ok());
        assert!(emitter.observe(&task, None, 1).is_ok());
        assert!(task.set_part_progress(PartIndex::new(0), 2).is_ok());
        assert!(emitter.observe(&task, None, 2).is_ok());
        assert!(task.set_part_progress(PartIndex::new(0), 3).is_ok());
        assert!(emitter.observe(&task, None, 101).is_ok());
        let events = emitter.drain();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].state, TransferState::Queued);
        assert_eq!(events[1].state, TransferState::Running);
        assert_eq!(events[2].progress.transferred_bytes, 3);
    }

    #[test]
    fn redundant_progress_is_dropped_under_bounded_pressure() {
        let config = match ProgressConfig::new(1, 1, 4) {
            Ok(value) => value,
            Err(error) => panic!("test progress config failed: {error}"),
        };
        let mut emitter = ProgressEmitter::new(config);
        let mut task = task();
        assert!(emitter.observe(&task, None, 0).is_ok());
        let _ = emitter.drain();
        assert!(task.transition_to(TransferState::Running).is_ok());
        assert!(
            task.transition_part(PartIndex::new(0), PartState::Running)
                .is_ok()
        );
        assert!(task.set_part_progress(PartIndex::new(0), 1).is_ok());
        assert!(emitter.observe(&task, None, 1).is_ok());
        let _ = emitter.drain();
        assert!(task.set_part_progress(PartIndex::new(0), 2).is_ok());
        assert!(emitter.observe(&task, None, 2).is_ok());
        assert!(task.set_part_progress(PartIndex::new(0), 3).is_ok());
        assert!(emitter.observe(&task, None, 3).is_ok());
        let events = emitter.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].progress.transferred_bytes, 3);
    }
}
