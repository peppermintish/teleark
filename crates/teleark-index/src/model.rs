use std::{fmt, num::NonZeroUsize};

use teleark_core::{
    AccountId, ChatId, FileKind, IndexCoverage, IndexJobId, IndexJobState, MessageId,
};

use crate::{ExecutionFailure, ValidationError};

pub const CONTENT_POLICY_VERSION: u32 = 1;
pub const DEFAULT_BATCH_SIZE: usize = 1_000;
pub const MAX_BATCH_SIZE: usize = 4_096;
pub const MAX_CURSOR_BYTES: usize = 16 * 1024;
pub const MAX_REMOTE_KEY_BYTES: usize = 16 * 1024;
pub const MAX_PAGE_METADATA_BYTES: usize = 16 * 1024 * 1024;

const MAX_TEXT_BYTES: usize = 1024 * 1024;
const MAX_FILE_NAME_BYTES: usize = 16 * 1024;
const MAX_MIME_TYPE_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatchSize(NonZeroUsize);

impl BatchSize {
    pub fn try_new(value: usize) -> Result<Self, ValidationError> {
        let Some(value) = NonZeroUsize::new(value) else {
            return Err(ValidationError::InvalidBatchSize {
                requested: 0,
                maximum: MAX_BATCH_SIZE,
            });
        };
        if value.get() > MAX_BATCH_SIZE {
            return Err(ValidationError::InvalidBatchSize {
                requested: value.get(),
                maximum: MAX_BATCH_SIZE,
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for BatchSize {
    fn default() -> Self {
        Self(NonZeroUsize::new(DEFAULT_BATCH_SIZE).unwrap_or(NonZeroUsize::MIN))
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct HistoryCursor(Vec<u8>);

impl HistoryCursor {
    pub fn try_new(bytes: Vec<u8>) -> Result<Self, ValidationError> {
        if bytes.is_empty() {
            return Err(ValidationError::EmptyCursor);
        }
        if bytes.len() > MAX_CURSOR_BYTES {
            return Err(ValidationError::CursorTooLarge {
                bytes: bytes.len(),
                maximum: MAX_CURSOR_BYTES,
            });
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for HistoryCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("HistoryCursor")
            .field(&format_args!("<{} bytes>", self.0.len()))
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RemoteMediaKey(Vec<u8>);

impl RemoteMediaKey {
    pub fn try_new(bytes: Vec<u8>) -> Result<Self, ValidationError> {
        if bytes.is_empty() {
            return Err(ValidationError::EmptyRemoteMediaKey);
        }
        if bytes.len() > MAX_REMOTE_KEY_BYTES {
            return Err(ValidationError::RemoteMediaKeyTooLarge {
                bytes: bytes.len(),
                maximum: MAX_REMOTE_KEY_BYTES,
            });
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for RemoteMediaKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RemoteMediaKey")
            .field(&format_args!("<{} bytes>", self.0.len()))
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceKey {
    pub account_id: AccountId,
    pub chat_id: ChatId,
    pub message_id: MessageId,
}

impl SourceKey {
    pub const fn new(account_id: AccountId, chat_id: ChatId, message_id: MessageId) -> Self {
        Self {
            account_id,
            chat_id,
            message_id,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryMedia {
    pub remote_key: RemoteMediaKey,
    pub file_name: String,
    pub size_bytes: u64,
    pub kind: FileKind,
    pub mime_type: Option<String>,
    pub caption: Option<String>,
}

impl HistoryMedia {
    pub fn new(
        remote_key: RemoteMediaKey,
        file_name: impl Into<String>,
        size_bytes: u64,
        kind: FileKind,
    ) -> Self {
        Self {
            remote_key,
            file_name: file_name.into(),
            size_bytes,
            kind,
            mime_type: None,
            caption: None,
        }
    }

    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    pub fn with_caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = Some(caption.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoryContent {
    Empty,
    PlainText(String),
    Media(HistoryMedia),
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryRecord {
    pub source: SourceKey,
    pub revision: u64,
    pub modified_at_unix_ms: i64,
    pub content: HistoryContent,
}

impl HistoryRecord {
    pub const fn new(
        source: SourceKey,
        revision: u64,
        modified_at_unix_ms: i64,
        content: HistoryContent,
    ) -> Self {
        Self {
            source,
            revision,
            modified_at_unix_ms,
            content,
        }
    }

    pub(crate) fn metadata_bytes(&self) -> usize {
        match &self.content {
            HistoryContent::Empty | HistoryContent::Deleted => 0,
            HistoryContent::PlainText(text) => text.len(),
            HistoryContent::Media(media) => {
                media.remote_key.as_bytes().len()
                    + media.file_name.len()
                    + media.mime_type.as_ref().map_or(0, String::len)
                    + media.caption.as_ref().map_or(0, String::len)
            }
        }
    }

    pub(crate) fn has_valid_field_lengths(&self) -> bool {
        match &self.content {
            HistoryContent::Empty | HistoryContent::Deleted => true,
            HistoryContent::PlainText(text) => text.len() <= MAX_TEXT_BYTES,
            HistoryContent::Media(media) => {
                media.file_name.len() <= MAX_FILE_NAME_BYTES
                    && media
                        .mime_type
                        .as_ref()
                        .is_none_or(|value| value.len() <= MAX_MIME_TYPE_BYTES)
                    && media
                        .caption
                        .as_ref()
                        .is_none_or(|value| value.len() <= MAX_TEXT_BYTES)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentPolicy {
    include_media: bool,
    include_plain_text: bool,
    minimum_media_size_bytes: u64,
}

impl ContentPolicy {
    pub const fn files_only() -> Self {
        Self {
            include_media: true,
            include_plain_text: false,
            minimum_media_size_bytes: 0,
        }
    }

    pub const fn with_plain_text(mut self, include: bool) -> Self {
        self.include_plain_text = include;
        self
    }

    pub const fn with_media(mut self, include: bool) -> Self {
        self.include_media = include;
        self
    }

    pub const fn with_minimum_media_size_bytes(mut self, minimum: u64) -> Self {
        self.minimum_media_size_bytes = minimum;
        self
    }

    pub const fn includes_media(self) -> bool {
        self.include_media
    }

    pub const fn includes_plain_text(self) -> bool {
        self.include_plain_text
    }

    pub const fn minimum_media_size_bytes(self) -> u64 {
        self.minimum_media_size_bytes
    }

    pub fn fingerprint(self) -> String {
        format!(
            "v{CONTENT_POLICY_VERSION}:media={}:plain_text={}:minimum_media_size={}",
            u8::from(self.include_media),
            u8::from(self.include_plain_text),
            self.minimum_media_size_bytes
        )
    }

    pub(crate) const fn accepts(&self, content: &HistoryContent) -> bool {
        match content {
            HistoryContent::Empty => false,
            HistoryContent::PlainText(text) => self.include_plain_text && !text.is_empty(),
            HistoryContent::Media(media) => {
                self.include_media && media.size_bytes >= self.minimum_media_size_bytes
            }
            HistoryContent::Deleted => true,
        }
    }
}

impl Default for ContentPolicy {
    fn default() -> Self {
        Self::files_only()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MessageRange {
    pub start_message_id: MessageId,
    pub end_message_id: MessageId,
}

impl MessageRange {
    pub fn try_new(
        start_message_id: MessageId,
        end_message_id: MessageId,
    ) -> Result<Self, ValidationError> {
        if start_message_id > end_message_id {
            return Err(ValidationError::InvalidRange {
                start_message_id: start_message_id.get(),
                end_message_id: end_message_id.get(),
            });
        }
        Ok(Self {
            start_message_id,
            end_message_id,
        })
    }

    pub const fn contains(self, message_id: MessageId) -> bool {
        message_id.get() >= self.start_message_id.get()
            && message_id.get() <= self.end_message_id.get()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexRequest {
    job_id: IndexJobId,
    account_id: AccountId,
    chat_id: ChatId,
    ranges: Vec<MessageRange>,
    policy: ContentPolicy,
}

impl IndexRequest {
    pub fn try_new(
        job_id: IndexJobId,
        account_id: AccountId,
        chat_id: ChatId,
        mut ranges: Vec<MessageRange>,
        policy: ContentPolicy,
    ) -> Result<Self, ValidationError> {
        if job_id.get() == 0 {
            return Err(ValidationError::ZeroJobId);
        }
        if ranges.is_empty() {
            return Err(ValidationError::EmptyRanges);
        }
        if !policy.includes_media() && !policy.includes_plain_text() {
            return Err(ValidationError::NoContentSelected);
        }
        ranges.sort_unstable();
        for pair in ranges.windows(2) {
            let first = pair[0];
            let second = pair[1];
            if second.start_message_id <= first.end_message_id {
                return Err(ValidationError::OverlappingRanges {
                    first_start_message_id: first.start_message_id.get(),
                    first_end_message_id: first.end_message_id.get(),
                    second_start_message_id: second.start_message_id.get(),
                    second_end_message_id: second.end_message_id.get(),
                });
            }
        }
        Ok(Self {
            job_id,
            account_id,
            chat_id,
            ranges,
            policy,
        })
    }

    pub const fn job_id(&self) -> IndexJobId {
        self.job_id
    }

    pub const fn account_id(&self) -> AccountId {
        self.account_id
    }

    pub const fn chat_id(&self) -> ChatId {
        self.chat_id
    }

    pub fn ranges(&self) -> &[MessageRange] {
        &self.ranges
    }

    pub const fn policy(&self) -> ContentPolicy {
        self.policy
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryPageRequest {
    pub account_id: AccountId,
    pub chat_id: ChatId,
    pub range: MessageRange,
    pub cursor: Option<HistoryCursor>,
    pub limit: BatchSize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryPage {
    records: Vec<HistoryRecord>,
    next_cursor: Option<HistoryCursor>,
    exhausted: bool,
}

impl HistoryPage {
    pub fn new(
        records: Vec<HistoryRecord>,
        next_cursor: Option<HistoryCursor>,
        exhausted: bool,
    ) -> Self {
        Self {
            records,
            next_cursor,
            exhausted,
        }
    }

    pub fn into_parts(self) -> (Vec<HistoryRecord>, Option<HistoryCursor>, bool) {
        (self.records, self.next_cursor, self.exhausted)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexedContent {
    PlainText(String),
    Media(HistoryMedia),
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedRecord {
    pub source: SourceKey,
    pub revision: u64,
    pub modified_at_unix_ms: i64,
    pub content: IndexedContent,
}

impl IndexedRecord {
    pub(crate) fn from_history(record: HistoryRecord) -> Option<Self> {
        let content = match record.content {
            HistoryContent::Empty => return None,
            HistoryContent::PlainText(text) => IndexedContent::PlainText(text),
            HistoryContent::Media(media) => IndexedContent::Media(media),
            HistoryContent::Deleted => IndexedContent::Deleted,
        };
        Some(Self {
            source: record.source,
            revision: record.revision,
            modified_at_unix_ms: record.modified_at_unix_ms,
            content,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexStatistics {
    pub messages_scanned: u64,
    pub records_written: u64,
    pub files_indexed: u64,
    pub plain_text_indexed: u64,
    pub deletions_applied: u64,
    pub bytes_indexed: u64,
    pub batches_committed: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexCheckpoint {
    next_range_index: usize,
    cursor: Option<HistoryCursor>,
}

impl IndexCheckpoint {
    pub const fn initial() -> Self {
        Self {
            next_range_index: 0,
            cursor: None,
        }
    }

    pub fn try_new(
        next_range_index: usize,
        cursor: Option<HistoryCursor>,
        range_count: usize,
    ) -> Result<Self, ValidationError> {
        if next_range_index > range_count {
            return Err(ValidationError::CheckpointOutsideRequest {
                next_range_index,
                range_count,
            });
        }
        if next_range_index == range_count && cursor.is_some() {
            return Err(ValidationError::CursorAfterFinalRange);
        }
        Ok(Self {
            next_range_index,
            cursor,
        })
    }

    pub const fn next_range_index(&self) -> usize {
        self.next_range_index
    }

    pub fn cursor(&self) -> Option<&HistoryCursor> {
        self.cursor.as_ref()
    }
}

impl Default for IndexCheckpoint {
    fn default() -> Self {
        Self::initial()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexJobRecord {
    request: IndexRequest,
    state: IndexJobState,
    checkpoint: IndexCheckpoint,
    statistics: IndexStatistics,
    revision: u64,
    last_failure: Option<ExecutionFailure>,
}

impl IndexJobRecord {
    pub fn new(request: IndexRequest) -> Self {
        Self {
            request,
            state: IndexJobState::Queued,
            checkpoint: IndexCheckpoint::initial(),
            statistics: IndexStatistics::default(),
            revision: 0,
            last_failure: None,
        }
    }

    pub fn try_restore(
        request: IndexRequest,
        state: IndexJobState,
        checkpoint: IndexCheckpoint,
        statistics: IndexStatistics,
        revision: u64,
        last_failure: Option<ExecutionFailure>,
    ) -> Result<Self, ValidationError> {
        IndexCheckpoint::try_new(
            checkpoint.next_range_index,
            checkpoint.cursor.clone(),
            request.ranges.len(),
        )?;
        if state == IndexJobState::Completed && checkpoint.next_range_index != request.ranges.len()
        {
            return Err(ValidationError::CompletedBeforeFinalRange);
        }
        if (state == IndexJobState::Failed) != last_failure.is_some() {
            return Err(ValidationError::FailureDoesNotMatchState);
        }
        Ok(Self {
            request,
            state,
            checkpoint,
            statistics,
            revision,
            last_failure,
        })
    }

    pub const fn id(&self) -> IndexJobId {
        self.request.job_id
    }

    pub const fn state(&self) -> IndexJobState {
        self.state
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn request(&self) -> &IndexRequest {
        &self.request
    }

    pub const fn checkpoint(&self) -> &IndexCheckpoint {
        &self.checkpoint
    }

    pub const fn statistics(&self) -> &IndexStatistics {
        &self.statistics
    }

    pub const fn last_failure(&self) -> Option<&ExecutionFailure> {
        self.last_failure.as_ref()
    }

    pub(crate) fn with_state(
        &self,
        state: IndexJobState,
        revision: u64,
        last_failure: Option<ExecutionFailure>,
    ) -> Self {
        Self {
            request: self.request.clone(),
            state,
            checkpoint: self.checkpoint.clone(),
            statistics: self.statistics.clone(),
            revision,
            last_failure,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BatchId {
    pub job_id: IndexJobId,
    pub sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexBatch {
    id: BatchId,
    request: IndexRequest,
    expected_job_revision: u64,
    expected_checkpoint: IndexCheckpoint,
    next_checkpoint: IndexCheckpoint,
    range_index: usize,
    range: MessageRange,
    coverage: IndexCoverage,
    records: Vec<IndexedRecord>,
    messages_examined: u64,
}

impl IndexBatch {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: BatchId,
        request: IndexRequest,
        expected_job_revision: u64,
        expected_checkpoint: IndexCheckpoint,
        next_checkpoint: IndexCheckpoint,
        range_index: usize,
        range: MessageRange,
        coverage: IndexCoverage,
        records: Vec<IndexedRecord>,
        messages_examined: u64,
    ) -> Self {
        Self {
            id,
            request,
            expected_job_revision,
            expected_checkpoint,
            next_checkpoint,
            range_index,
            range,
            coverage,
            records,
            messages_examined,
        }
    }

    pub const fn id(&self) -> BatchId {
        self.id
    }

    pub const fn request(&self) -> &IndexRequest {
        &self.request
    }

    pub const fn expected_job_revision(&self) -> u64 {
        self.expected_job_revision
    }

    pub const fn expected_checkpoint(&self) -> &IndexCheckpoint {
        &self.expected_checkpoint
    }

    pub const fn next_checkpoint(&self) -> &IndexCheckpoint {
        &self.next_checkpoint
    }

    pub const fn range_index(&self) -> usize {
        self.range_index
    }

    pub const fn range(&self) -> MessageRange {
        self.range
    }

    pub const fn coverage(&self) -> IndexCoverage {
        self.coverage
    }

    pub fn records(&self) -> &[IndexedRecord] {
        &self.records
    }

    pub const fn messages_examined(&self) -> u64 {
        self.messages_examined
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RangeEvidence {
    pub job_id: IndexJobId,
    pub range_index: usize,
    pub range: MessageRange,
    pub coverage: IndexCoverage,
    pub cursor: Option<HistoryCursor>,
    pub statistics: IndexStatistics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchCommitDisposition {
    Applied,
    AlreadyCommitted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchCommitReceipt {
    pub disposition: BatchCommitDisposition,
    pub job: IndexJobRecord,
    pub range: RangeEvidence,
}

impl BatchCommitReceipt {
    pub const fn new(
        disposition: BatchCommitDisposition,
        job: IndexJobRecord,
        range: RangeEvidence,
    ) -> Self {
        Self {
            disposition,
            job,
            range,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgressSnapshot {
    pub job_id: IndexJobId,
    pub state: IndexJobState,
    pub checkpoint: IndexCheckpoint,
    pub statistics: IndexStatistics,
    pub completed_ranges: usize,
    pub total_ranges: usize,
    pub current_range: Option<MessageRange>,
    pub last_range_evidence: Option<RangeEvidence>,
    pub last_failure: Option<ExecutionFailure>,
}

impl ProgressSnapshot {
    pub(crate) fn from_job(job: &IndexJobRecord, evidence: Option<RangeEvidence>) -> Self {
        let completed_ranges = job.checkpoint.next_range_index;
        let current_range = job
            .request
            .ranges
            .get(job.checkpoint.next_range_index)
            .copied();
        Self {
            job_id: job.id(),
            state: job.state,
            checkpoint: job.checkpoint.clone(),
            statistics: job.statistics.clone(),
            completed_ranges,
            total_ranges: job.request.ranges.len(),
            current_range,
            last_range_evidence: evidence,
            last_failure: job.last_failure.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    Completed(ProgressSnapshot),
    Paused(ProgressSnapshot),
    Cancelled(ProgressSnapshot),
}
