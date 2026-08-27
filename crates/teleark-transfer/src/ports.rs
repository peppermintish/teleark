use teleark_core::{TransferError, TransferId, TransferTask};

use crate::{
    ContentDigest, DestinationId, RemoteObject, RemotePartKey, SourceId, SourceIdentity,
    TransferCheckpoint,
};

/// Explicit monotonic clock used for retry and progress policy.
pub trait Clock {
    fn now_millis(&self) -> u64;
}

/// Injected bounded jitter source. Implementations return a value in
/// `0..=maximum_millis`; the retry policy defensively clamps it.
pub trait JitterSource {
    fn jitter_millis(&mut self, maximum_millis: u64) -> u64;
}

/// Integrity implementation selected by the application. The fake supplied by
/// test support is deliberately not a production cryptographic hash.
pub trait DigestPort {
    fn digest(&self, bytes: &[u8]) -> ContentDigest;
}

/// Bounded local source access.
pub trait SourcePort {
    fn source_identity(&self, source_id: SourceId) -> Result<SourceIdentity, TransferError>;

    fn read_source_range(
        &mut self,
        source_id: SourceId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, TransferError>;

    fn digest_source(&mut self, source_id: SourceId) -> Result<ContentDigest, TransferError>;
}

/// Result used when a remote write may have succeeded despite a lost response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UploadError {
    Definite(TransferError),
    AmbiguousSuccess,
}

/// Project-owned remote part transport. No Telegram or `grammers` type crosses
/// this boundary.
pub trait RemoteTransport {
    fn discover_remote(&mut self, key: RemotePartKey) -> Result<Vec<RemoteObject>, TransferError>;

    fn upload_remote(
        &mut self,
        key: RemotePartKey,
        bytes: &[u8],
        digest: ContentDigest,
    ) -> Result<RemoteObject, UploadError>;

    fn verify_remote(
        &mut self,
        object: &RemoteObject,
        expected_size: u64,
        expected_digest: ContentDigest,
    ) -> Result<(), TransferError>;

    fn download_remote(&mut self, object: &RemoteObject) -> Result<Vec<u8>, TransferError>;

    /// Publish/verify any package-level recovery object after every part and
    /// the source whole-file digest have been verified.
    fn finalize_upload(
        &mut self,
        _account_id: teleark_core::AccountId,
        _package_id: teleark_core::PackageId,
        _whole_digest: ContentDigest,
    ) -> Result<(), TransferError> {
        Ok(())
    }
}

/// Durable checkpoint replacement port.
pub trait CheckpointPort {
    fn load_checkpoint(
        &mut self,
        transfer_id: TransferId,
    ) -> Result<Option<TransferCheckpoint>, TransferError>;

    fn save_checkpoint(&mut self, checkpoint: &TransferCheckpoint) -> Result<(), TransferError>;

    /// Persist the Core task/part state projection after lifecycle transitions
    /// that do not alter immutable checkpoint evidence.
    fn save_task_state(&mut self, _task: &TransferTask) -> Result<(), TransferError> {
        Ok(())
    }
}

/// Safe positional `.partial` output and atomic-finalization boundary.
pub trait FileSystemPort {
    fn prepare_partial(
        &mut self,
        destination_id: DestinationId,
        total_bytes: u64,
    ) -> Result<(), TransferError>;

    fn write_partial(
        &mut self,
        destination_id: DestinationId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), TransferError>;

    fn read_partial(
        &mut self,
        destination_id: DestinationId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, TransferError>;

    fn digest_partial(
        &mut self,
        destination_id: DestinationId,
    ) -> Result<ContentDigest, TransferError>;

    fn flush_partial(&mut self, destination_id: DestinationId) -> Result<(), TransferError>;

    fn atomic_finalize(&mut self, destination_id: DestinationId) -> Result<(), TransferError>;

    fn final_exists(&self, destination_id: DestinationId) -> bool;
}

/// Convenience bound for an environment implementing all narrow I/O ports.
pub trait TransferIo:
    DigestPort + SourcePort + RemoteTransport + CheckpointPort + FileSystemPort
{
}

impl<T> TransferIo for T where
    T: DigestPort + SourcePort + RemoteTransport + CheckpointPort + FileSystemPort
{
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn checked_range(
    offset: u64,
    length: u64,
    context: &'static str,
) -> Result<std::ops::Range<usize>, TransferError> {
    let end = offset
        .checked_add(length)
        .ok_or(TransferError::HashMismatch)?;
    let start = usize::try_from(offset).map_err(|_| TransferError::HashMismatch)?;
    let end = usize::try_from(end).map_err(|_| TransferError::HashMismatch)?;
    if start > end {
        return Err(match context {
            "source" => TransferError::SourceChanged,
            _ => TransferError::HashMismatch,
        });
    }
    Ok(start..end)
}
