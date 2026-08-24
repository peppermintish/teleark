use std::error::Error;
use std::fmt;

use crate::{DomainValidationError, LogicalFileId, PartIndex, TransferError, TransferId};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransferDirection {
    Upload,
    Download,
}

/// Higher values run first; ordering among equal priorities remains a scheduler concern.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TransferPriority(i16);

impl TransferPriority {
    pub const NORMAL: Self = Self(0);

    pub const fn new(value: i16) -> Self {
        Self(value)
    }

    pub const fn get(self) -> i16 {
        self.0
    }
}

/// Authoritative state of one logical-file transfer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransferState {
    Queued,
    Running,
    Paused,
    WaitingRetry,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl TransferState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

/// Authoritative state of one application-level transfer part.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PartState {
    Queued,
    Running,
    Paused,
    WaitingRetry,
    Transferred,
    Verifying,
    Verified,
    Failed,
    Cancelled,
}

impl PartState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Verified | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransferProgress {
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub completed_parts: usize,
    pub total_parts: usize,
}

impl TransferProgress {
    /// `None` represents an indeterminate zero-byte total.
    pub fn fraction(self) -> Option<f64> {
        (self.total_bytes != 0).then_some(self.transferred_bytes as f64 / self.total_bytes as f64)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferPart {
    index: PartIndex,
    offset_bytes: u64,
    size_bytes: u64,
    transferred_bytes: u64,
    state: PartState,
}

impl TransferPart {
    fn new(
        index: PartIndex,
        offset_bytes: u64,
        size_bytes: u64,
    ) -> Result<Self, DomainValidationError> {
        if size_bytes == 0 {
            return Err(DomainValidationError::ZeroSizedPart { index });
        }
        offset_bytes
            .checked_add(size_bytes)
            .ok_or(DomainValidationError::ArithmeticOverflow)?;
        Ok(Self {
            index,
            offset_bytes,
            size_bytes,
            transferred_bytes: 0,
            state: PartState::Queued,
        })
    }

    pub const fn index(&self) -> PartIndex {
        self.index
    }

    pub const fn offset_bytes(&self) -> u64 {
        self.offset_bytes
    }

    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub const fn transferred_bytes(&self) -> u64 {
        self.transferred_bytes
    }

    pub const fn state(&self) -> PartState {
        self.state
    }
}

/// A rejected transfer or part transition.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransferTransitionError {
    InvalidTaskTransition {
        transfer_id: TransferId,
        from: TransferState,
        to: TransferState,
    },
    InvalidPartTransition {
        transfer_id: TransferId,
        part_index: PartIndex,
        from: PartState,
        to: PartState,
    },
    UnknownPart {
        transfer_id: TransferId,
        part_index: PartIndex,
    },
    ProgressRegressed {
        transfer_id: TransferId,
        part_index: PartIndex,
        previous_bytes: u64,
        new_bytes: u64,
    },
    ProgressExceedsPart {
        transfer_id: TransferId,
        part_index: PartIndex,
        size_bytes: u64,
        attempted_bytes: u64,
    },
    PartIsIncomplete {
        transfer_id: TransferId,
        part_index: PartIndex,
        transferred_bytes: u64,
        size_bytes: u64,
    },
    PartsNotReady {
        transfer_id: TransferId,
        target: TransferState,
    },
}

impl fmt::Display for TransferTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTaskTransition {
                transfer_id,
                from,
                to,
            } => write!(
                formatter,
                "transfer {transfer_id} cannot transition from {from:?} to {to:?}"
            ),
            Self::InvalidPartTransition {
                transfer_id,
                part_index,
                from,
                to,
            } => write!(
                formatter,
                "transfer {transfer_id} part {part_index} cannot transition from {from:?} to {to:?}"
            ),
            Self::UnknownPart {
                transfer_id,
                part_index,
            } => write!(formatter, "transfer {transfer_id} has no part {part_index}"),
            Self::ProgressRegressed {
                transfer_id,
                part_index,
                previous_bytes,
                new_bytes,
            } => write!(
                formatter,
                "transfer {transfer_id} part {part_index} progress regressed from {previous_bytes} to {new_bytes} bytes"
            ),
            Self::ProgressExceedsPart {
                transfer_id,
                part_index,
                size_bytes,
                attempted_bytes,
            } => write!(
                formatter,
                "transfer {transfer_id} part {part_index} progress {attempted_bytes} exceeds {size_bytes} bytes"
            ),
            Self::PartIsIncomplete {
                transfer_id,
                part_index,
                transferred_bytes,
                size_bytes,
            } => write!(
                formatter,
                "transfer {transfer_id} part {part_index} has {transferred_bytes} of {size_bytes} bytes"
            ),
            Self::PartsNotReady {
                transfer_id,
                target,
            } => write!(
                formatter,
                "transfer {transfer_id} parts do not satisfy the invariant for {target:?}"
            ),
        }
    }
}

impl Error for TransferTransitionError {}

/// Logical-file-level transfer state with validated mutation methods.
///
/// State fields are private so all changes pass through the authoritative
/// transition tables below. A task can enter `Verifying` only after every part
/// is transferred and can enter `Completed` only after every part is verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferTask {
    id: TransferId,
    logical_file_id: LogicalFileId,
    direction: TransferDirection,
    priority: TransferPriority,
    total_bytes: u64,
    state: TransferState,
    parts: Vec<TransferPart>,
    last_error: Option<TransferError>,
}

impl TransferTask {
    pub fn try_new(
        id: TransferId,
        logical_file_id: LogicalFileId,
        direction: TransferDirection,
        total_bytes: u64,
        priority: TransferPriority,
        part_sizes_bytes: &[u64],
    ) -> Result<Self, DomainValidationError> {
        let mut parts = Vec::with_capacity(part_sizes_bytes.len());
        let mut offset_bytes = 0_u64;
        for (position, size_bytes) in part_sizes_bytes.iter().copied().enumerate() {
            let index = PartIndex::new(
                u32::try_from(position).map_err(|_| DomainValidationError::ArithmeticOverflow)?,
            );
            parts.push(TransferPart::new(index, offset_bytes, size_bytes)?);
            offset_bytes = offset_bytes
                .checked_add(size_bytes)
                .ok_or(DomainValidationError::ArithmeticOverflow)?;
        }
        if offset_bytes != total_bytes {
            return Err(DomainValidationError::PartTotalMismatch {
                expected_bytes: total_bytes,
                actual_bytes: offset_bytes,
            });
        }

        Ok(Self {
            id,
            logical_file_id,
            direction,
            priority,
            total_bytes,
            state: TransferState::Queued,
            parts,
            last_error: None,
        })
    }

    pub const fn id(&self) -> TransferId {
        self.id
    }

    pub const fn logical_file_id(&self) -> LogicalFileId {
        self.logical_file_id
    }

    pub const fn direction(&self) -> TransferDirection {
        self.direction
    }

    pub const fn priority(&self) -> TransferPriority {
        self.priority
    }

    pub const fn state(&self) -> TransferState {
        self.state
    }

    pub fn parts(&self) -> &[TransferPart] {
        &self.parts
    }

    pub fn last_error(&self) -> Option<&TransferError> {
        self.last_error.as_ref()
    }

    pub fn progress(&self) -> TransferProgress {
        TransferProgress {
            transferred_bytes: self.parts.iter().map(|part| part.transferred_bytes).sum(),
            total_bytes: self.total_bytes,
            completed_parts: self
                .parts
                .iter()
                .filter(|part| part.state == PartState::Verified)
                .count(),
            total_parts: self.parts.len(),
        }
    }

    /// Performs the sole authoritative task-state transition.
    pub fn transition_to(&mut self, next: TransferState) -> Result<(), TransferTransitionError> {
        if !task_transition_allowed(self.state, next) {
            return Err(TransferTransitionError::InvalidTaskTransition {
                transfer_id: self.id,
                from: self.state,
                to: next,
            });
        }

        match next {
            TransferState::Verifying if !self.parts_ready_for_verification() => {
                return Err(TransferTransitionError::PartsNotReady {
                    transfer_id: self.id,
                    target: next,
                });
            }
            TransferState::Completed
                if !self.parts.iter().all(|part| {
                    part.state == PartState::Verified && part.transferred_bytes == part.size_bytes
                }) =>
            {
                return Err(TransferTransitionError::PartsNotReady {
                    transfer_id: self.id,
                    target: next,
                });
            }
            _ => {}
        }

        self.state = next;
        if next != TransferState::Failed {
            self.last_error = None;
        }
        Ok(())
    }

    /// Records a structured failure and transitions to `Failed`.
    pub fn fail(&mut self, error: TransferError) -> Result<(), TransferTransitionError> {
        self.transition_to(TransferState::Failed)?;
        self.last_error = Some(error);
        Ok(())
    }

    /// Updates monotonically increasing byte progress for one part.
    pub fn set_part_progress(
        &mut self,
        index: PartIndex,
        transferred_bytes: u64,
    ) -> Result<(), TransferTransitionError> {
        let transfer_id = self.id;
        let part = self.part_mut(index)?;
        if transferred_bytes < part.transferred_bytes {
            return Err(TransferTransitionError::ProgressRegressed {
                transfer_id,
                part_index: index,
                previous_bytes: part.transferred_bytes,
                new_bytes: transferred_bytes,
            });
        }
        if transferred_bytes > part.size_bytes {
            return Err(TransferTransitionError::ProgressExceedsPart {
                transfer_id,
                part_index: index,
                size_bytes: part.size_bytes,
                attempted_bytes: transferred_bytes,
            });
        }
        part.transferred_bytes = transferred_bytes;
        Ok(())
    }

    /// Performs the sole authoritative application-part state transition.
    pub fn transition_part(
        &mut self,
        index: PartIndex,
        next: PartState,
    ) -> Result<(), TransferTransitionError> {
        let transfer_id = self.id;
        let part = self.part_mut(index)?;
        if !part_transition_allowed(part.state, next) {
            return Err(TransferTransitionError::InvalidPartTransition {
                transfer_id,
                part_index: index,
                from: part.state,
                to: next,
            });
        }
        if next == PartState::Transferred && part.transferred_bytes != part.size_bytes {
            return Err(TransferTransitionError::PartIsIncomplete {
                transfer_id,
                part_index: index,
                transferred_bytes: part.transferred_bytes,
                size_bytes: part.size_bytes,
            });
        }
        part.state = next;
        Ok(())
    }

    fn part_mut(&mut self, index: PartIndex) -> Result<&mut TransferPart, TransferTransitionError> {
        self.parts
            .get_mut(index.get() as usize)
            .filter(|part| part.index == index)
            .ok_or(TransferTransitionError::UnknownPart {
                transfer_id: self.id,
                part_index: index,
            })
    }

    fn parts_ready_for_verification(&self) -> bool {
        self.parts.iter().all(|part| {
            matches!(
                part.state,
                PartState::Transferred | PartState::Verifying | PartState::Verified
            ) && part.transferred_bytes == part.size_bytes
        })
    }
}

const fn task_transition_allowed(from: TransferState, to: TransferState) -> bool {
    use TransferState::*;
    matches!(
        (from, to),
        (Queued, Running | Paused | Cancelled)
            | (
                Running,
                Paused | WaitingRetry | Verifying | Failed | Cancelled
            )
            | (Paused, Running | Cancelled)
            | (WaitingRetry, Running | Paused | Failed | Cancelled)
            | (Verifying, Completed | Failed | Cancelled)
            | (Failed, Queued | Cancelled)
    )
}

const fn part_transition_allowed(from: PartState, to: PartState) -> bool {
    use PartState::*;
    matches!(
        (from, to),
        (Queued, Running | Paused | Cancelled)
            | (
                Running,
                Paused | WaitingRetry | Transferred | Failed | Cancelled
            )
            | (Paused, Running | Cancelled)
            | (WaitingRetry, Running | Paused | Failed | Cancelled)
            | (Transferred, Verifying | Cancelled)
            | (Verifying, Verified | Failed | Cancelled)
            | (Failed, Queued | Cancelled)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> TransferTask {
        TransferTask::try_new(
            TransferId::new(4),
            LogicalFileId::new(9),
            TransferDirection::Upload,
            30,
            TransferPriority::NORMAL,
            &[10, 20],
        )
        .unwrap()
    }

    fn transfer_and_verify_all_parts(task: &mut TransferTask) {
        task.transition_to(TransferState::Running).unwrap();
        for (index, size) in [(0, 10), (1, 20)] {
            let index = PartIndex::new(index);
            task.transition_part(index, PartState::Running).unwrap();
            task.set_part_progress(index, size).unwrap();
            task.transition_part(index, PartState::Transferred).unwrap();
        }
        task.transition_to(TransferState::Verifying).unwrap();
        for index in [0, 1] {
            let index = PartIndex::new(index);
            task.transition_part(index, PartState::Verifying).unwrap();
            task.transition_part(index, PartState::Verified).unwrap();
        }
    }

    #[test]
    fn valid_sequence_can_pause_resume_verify_and_complete() {
        let mut task = task();
        task.transition_to(TransferState::Running).unwrap();
        task.transition_to(TransferState::Paused).unwrap();
        task.transition_to(TransferState::Running).unwrap();

        for (index, size) in [(0, 10), (1, 20)] {
            let index = PartIndex::new(index);
            task.transition_part(index, PartState::Running).unwrap();
            task.set_part_progress(index, size).unwrap();
            task.transition_part(index, PartState::Transferred).unwrap();
        }
        task.transition_to(TransferState::Verifying).unwrap();
        for index in [0, 1] {
            let index = PartIndex::new(index);
            task.transition_part(index, PartState::Verifying).unwrap();
            task.transition_part(index, PartState::Verified).unwrap();
        }
        task.transition_to(TransferState::Completed).unwrap();

        assert_eq!(task.state(), TransferState::Completed);
        assert_eq!(task.progress().fraction(), Some(1.0));
        assert_eq!(task.progress().completed_parts, 2);
    }

    #[test]
    fn completed_transfer_cannot_restart() {
        let mut task = task();
        transfer_and_verify_all_parts(&mut task);
        task.transition_to(TransferState::Completed).unwrap();

        assert_eq!(
            task.transition_to(TransferState::Running),
            Err(TransferTransitionError::InvalidTaskTransition {
                transfer_id: TransferId::new(4),
                from: TransferState::Completed,
                to: TransferState::Running,
            })
        );
    }

    #[test]
    fn task_cannot_verify_before_all_parts_are_transferred() {
        let mut task = task();
        task.transition_to(TransferState::Running).unwrap();

        assert_eq!(
            task.transition_to(TransferState::Verifying),
            Err(TransferTransitionError::PartsNotReady {
                transfer_id: TransferId::new(4),
                target: TransferState::Verifying,
            })
        );
        assert_eq!(task.state(), TransferState::Running);
    }

    #[test]
    fn part_progress_is_monotonic_and_bounded() {
        let mut task = task();
        let index = PartIndex::new(0);
        task.set_part_progress(index, 8).unwrap();

        assert!(matches!(
            task.set_part_progress(index, 7),
            Err(TransferTransitionError::ProgressRegressed { .. })
        ));
        assert!(matches!(
            task.set_part_progress(index, 11),
            Err(TransferTransitionError::ProgressExceedsPart { .. })
        ));
        assert_eq!(task.parts()[0].transferred_bytes(), 8);
    }

    #[test]
    fn task_layout_must_equal_logical_file_size() {
        let error = TransferTask::try_new(
            TransferId::new(4),
            LogicalFileId::new(9),
            TransferDirection::Download,
            30,
            TransferPriority::NORMAL,
            &[10, 19],
        )
        .unwrap_err();

        assert_eq!(
            error,
            DomainValidationError::PartTotalMismatch {
                expected_bytes: 30,
                actual_bytes: 29,
            }
        );
    }
}
