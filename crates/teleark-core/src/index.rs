use std::error::Error;
use std::fmt;

use crate::{AccountId, ChatId, DomainValidationError, IndexJobId, MessageId};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IndexCoverage {
    Partial,
    Complete,
}

/// Inclusive Telegram message-ID coverage for one account/chat pair.
///
/// Ranges use ascending IDs even if the transport scanned newest-to-oldest.
/// A checkpoint, when present, must be inside the inclusive range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexRange {
    account_id: AccountId,
    chat_id: ChatId,
    start_message_id: MessageId,
    end_message_id: MessageId,
    coverage: IndexCoverage,
    checkpoint_message_id: Option<MessageId>,
    messages_scanned: u64,
    files_indexed: u64,
}

impl IndexRange {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        account_id: AccountId,
        chat_id: ChatId,
        start_message_id: MessageId,
        end_message_id: MessageId,
        coverage: IndexCoverage,
        checkpoint_message_id: Option<MessageId>,
        messages_scanned: u64,
        files_indexed: u64,
    ) -> Result<Self, DomainValidationError> {
        if start_message_id > end_message_id {
            return Err(DomainValidationError::InvalidIndexRange {
                start_message_id: start_message_id.get(),
                end_message_id: end_message_id.get(),
            });
        }
        if checkpoint_message_id
            .is_some_and(|checkpoint| checkpoint < start_message_id || checkpoint > end_message_id)
        {
            return Err(DomainValidationError::CheckpointOutsideRange {
                checkpoint_message_id: checkpoint_message_id
                    .map(MessageId::get)
                    .unwrap_or_default(),
            });
        }
        Ok(Self {
            account_id,
            chat_id,
            start_message_id,
            end_message_id,
            coverage,
            checkpoint_message_id,
            messages_scanned,
            files_indexed,
        })
    }

    pub const fn account_id(&self) -> AccountId {
        self.account_id
    }

    pub const fn chat_id(&self) -> ChatId {
        self.chat_id
    }

    pub const fn start_message_id(&self) -> MessageId {
        self.start_message_id
    }

    pub const fn end_message_id(&self) -> MessageId {
        self.end_message_id
    }

    pub const fn coverage(&self) -> IndexCoverage {
        self.coverage
    }

    pub const fn checkpoint_message_id(&self) -> Option<MessageId> {
        self.checkpoint_message_id
    }

    pub const fn messages_scanned(&self) -> u64 {
        self.messages_scanned
    }

    pub const fn files_indexed(&self) -> u64 {
        self.files_indexed
    }

    pub fn contains(&self, message_id: MessageId) -> bool {
        (self.start_message_id..=self.end_message_id).contains(&message_id)
    }
}

/// Ordered non-overlapping index coverage, including historical gaps.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexRangeSet {
    ranges: Vec<IndexRange>,
}

impl IndexRangeSet {
    pub fn try_new(mut ranges: Vec<IndexRange>) -> Result<Self, DomainValidationError> {
        ranges.sort_by_key(|range| {
            (
                range.account_id,
                range.chat_id,
                range.start_message_id,
                range.end_message_id,
            )
        });
        validate_no_overlaps(&ranges)?;
        Ok(Self { ranges })
    }

    pub fn ranges(&self) -> &[IndexRange] {
        &self.ranges
    }

    pub fn add(&mut self, range: IndexRange) -> Result<(), DomainValidationError> {
        let mut candidate = self.ranges.clone();
        candidate.push(range);
        *self = Self::try_new(candidate)?;
        Ok(())
    }

    pub fn coverage_at(
        &self,
        account_id: AccountId,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Option<IndexCoverage> {
        self.ranges
            .iter()
            .find(|range| {
                range.account_id == account_id
                    && range.chat_id == chat_id
                    && range.contains(message_id)
            })
            .map(IndexRange::coverage)
    }
}

fn validate_no_overlaps(ranges: &[IndexRange]) -> Result<(), DomainValidationError> {
    for pair in ranges.windows(2) {
        let existing = &pair[0];
        let new = &pair[1];
        let same_scope = existing.account_id == new.account_id && existing.chat_id == new.chat_id;
        if same_scope && new.start_message_id <= existing.end_message_id {
            return Err(DomainValidationError::OverlappingIndexRange {
                existing_start_message_id: existing.start_message_id.get(),
                existing_end_message_id: existing.end_message_id.get(),
                new_start_message_id: new.start_message_id.get(),
                new_end_message_id: new.end_message_id.get(),
            });
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IndexJobState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl IndexJobState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexTransitionError {
    pub index_job_id: IndexJobId,
    pub from: IndexJobState,
    pub to: IndexJobState,
}

impl fmt::Display for IndexTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "index job {} cannot transition from {:?} to {:?}",
            self.index_job_id, self.from, self.to
        )
    }
}

impl Error for IndexTransitionError {}

/// Recoverable indexing job with centralized state transitions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexJob {
    id: IndexJobId,
    account_id: AccountId,
    chat_id: ChatId,
    state: IndexJobState,
}

impl IndexJob {
    pub const fn new(id: IndexJobId, account_id: AccountId, chat_id: ChatId) -> Self {
        Self {
            id,
            account_id,
            chat_id,
            state: IndexJobState::Queued,
        }
    }

    pub const fn id(&self) -> IndexJobId {
        self.id
    }

    pub const fn account_id(&self) -> AccountId {
        self.account_id
    }

    pub const fn chat_id(&self) -> ChatId {
        self.chat_id
    }

    pub const fn state(&self) -> IndexJobState {
        self.state
    }

    pub fn transition_to(&mut self, next: IndexJobState) -> Result<(), IndexTransitionError> {
        if !index_transition_allowed(self.state, next) {
            return Err(IndexTransitionError {
                index_job_id: self.id,
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        Ok(())
    }
}

const fn index_transition_allowed(from: IndexJobState, to: IndexJobState) -> bool {
    use IndexJobState::*;
    matches!(
        (from, to),
        (Queued, Running | Paused | Cancelled)
            | (Running, Paused | Completed | Failed | Cancelled)
            | (Paused, Running | Cancelled)
            | (Failed, Queued | Cancelled)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: i64, end: i64, coverage: IndexCoverage) -> IndexRange {
        IndexRange::try_new(
            AccountId::new(1),
            ChatId::new(2),
            MessageId::new(start),
            MessageId::new(end),
            coverage,
            None,
            0,
            0,
        )
        .unwrap()
    }

    #[test]
    fn non_contiguous_historical_coverage_is_preserved() {
        let ranges = IndexRangeSet::try_new(vec![
            range(300, 399, IndexCoverage::Partial),
            range(100, 199, IndexCoverage::Complete),
        ])
        .unwrap();

        assert_eq!(ranges.ranges()[0].start_message_id(), MessageId::new(100));
        assert_eq!(
            ranges.coverage_at(AccountId::new(1), ChatId::new(2), MessageId::new(150)),
            Some(IndexCoverage::Complete)
        );
        assert_eq!(
            ranges.coverage_at(AccountId::new(1), ChatId::new(2), MessageId::new(250)),
            None
        );
    }

    #[test]
    fn overlapping_coverage_is_rejected_with_structured_bounds() {
        let error = IndexRangeSet::try_new(vec![
            range(100, 200, IndexCoverage::Complete),
            range(200, 300, IndexCoverage::Partial),
        ])
        .unwrap_err();

        assert_eq!(
            error,
            DomainValidationError::OverlappingIndexRange {
                existing_start_message_id: 100,
                existing_end_message_id: 200,
                new_start_message_id: 200,
                new_end_message_id: 300,
            }
        );
    }

    #[test]
    fn checkpoint_must_be_inside_its_range() {
        let error = IndexRange::try_new(
            AccountId::new(1),
            ChatId::new(2),
            MessageId::new(100),
            MessageId::new(200),
            IndexCoverage::Partial,
            Some(MessageId::new(99)),
            0,
            0,
        )
        .unwrap_err();

        assert_eq!(
            error,
            DomainValidationError::CheckpointOutsideRange {
                checkpoint_message_id: 99,
            }
        );
    }

    #[test]
    fn completed_index_job_cannot_restart() {
        let mut job = IndexJob::new(IndexJobId::new(8), AccountId::new(1), ChatId::new(2));
        job.transition_to(IndexJobState::Running).unwrap();
        job.transition_to(IndexJobState::Completed).unwrap();

        assert_eq!(
            job.transition_to(IndexJobState::Running),
            Err(IndexTransitionError {
                index_job_id: IndexJobId::new(8),
                from: IndexJobState::Completed,
                to: IndexJobState::Running,
            })
        );
    }
}
