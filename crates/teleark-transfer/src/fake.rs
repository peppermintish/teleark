use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use teleark_core::{TransferError, TransferId};

use crate::ports::checked_range;
use crate::{
    CheckpointPort, Clock, ContentDigest, DestinationId, DigestPort, FileSystemPort, JitterSource,
    RemoteObject, RemotePartKey, RemoteTransport, SourceId, SourceIdentity, SourcePort,
    TransferCheckpoint, UploadError,
};

/// Deterministic fake upload behavior for one invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FakeUploadBehavior {
    Succeed,
    NetworkFailure,
    FloodWait { millis: u64 },
    AmbiguousSuccess,
}

#[derive(Clone)]
struct FakeSource {
    bytes: Vec<u8>,
    identity: SourceIdentity,
    mutation_after_read: Option<Vec<u8>>,
}

#[derive(Clone)]
struct StoredRemote {
    object: RemoteObject,
    bytes: Vec<u8>,
}

#[derive(Clone, Default)]
struct FakeDestination {
    partial: Option<Vec<u8>>,
    partial_flushed: bool,
    final_bytes: Option<Vec<u8>>,
}

/// In-memory implementation of every transfer I/O port. Its digest is a
/// deterministic test checksum and must never be treated as cryptographic.
#[derive(Default)]
pub struct FakeEnvironment {
    sources: BTreeMap<SourceId, FakeSource>,
    remotes: BTreeMap<RemotePartKey, Vec<StoredRemote>>,
    upload_behaviors: BTreeMap<RemotePartKey, VecDeque<FakeUploadBehavior>>,
    upload_calls: BTreeMap<RemotePartKey, usize>,
    download_calls: BTreeMap<RemotePartKey, usize>,
    corrupt_next_download: BTreeMap<RemotePartKey, bool>,
    checkpoints: BTreeMap<TransferId, TransferCheckpoint>,
    destinations: BTreeMap<DestinationId, FakeDestination>,
    next_remote_id: u64,
    fail_next_checkpoint_save: bool,
}

impl FakeEnvironment {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_remote_id: 1,
            ..Self::default()
        }
    }

    pub fn add_source(
        &mut self,
        source_id: SourceId,
        bytes: Vec<u8>,
        filesystem_id: u128,
        modified_at_units: u64,
    ) -> SourceIdentity {
        let identity = SourceIdentity {
            filesystem_id,
            size_bytes: bytes.len() as u64,
            modified_at_units,
            revision: 1,
        };
        self.sources.insert(
            source_id,
            FakeSource {
                bytes,
                identity,
                mutation_after_read: None,
            },
        );
        identity
    }

    pub fn replace_source(&mut self, source_id: SourceId, bytes: Vec<u8>) {
        if let Some(source) = self.sources.get_mut(&source_id) {
            source.bytes = bytes;
            source.identity.size_bytes = source.bytes.len() as u64;
            source.identity.modified_at_units = source.identity.modified_at_units.saturating_add(1);
            source.identity.revision = source.identity.revision.saturating_add(1);
        }
    }

    pub fn mutate_source_after_next_read(&mut self, source_id: SourceId, bytes: Vec<u8>) {
        if let Some(source) = self.sources.get_mut(&source_id) {
            source.mutation_after_read = Some(bytes);
        }
    }

    #[must_use]
    pub fn digest_bytes(&self, bytes: &[u8]) -> ContentDigest {
        self.digest(bytes)
    }

    pub fn push_upload_behavior(&mut self, key: RemotePartKey, behavior: FakeUploadBehavior) {
        self.upload_behaviors
            .entry(key)
            .or_default()
            .push_back(behavior);
    }

    pub fn seed_remote(&mut self, key: RemotePartKey, bytes: Vec<u8>) -> RemoteObject {
        self.store_remote(key, bytes)
    }

    pub fn corrupt_next_download(&mut self, key: RemotePartKey) {
        self.corrupt_next_download.insert(key, true);
    }

    pub fn fail_next_checkpoint_save(&mut self) {
        self.fail_next_checkpoint_save = true;
    }

    #[must_use]
    pub fn remote_count(&self, key: RemotePartKey) -> usize {
        self.remotes.get(&key).map_or(0, Vec::len)
    }

    #[must_use]
    pub fn upload_calls(&self, key: RemotePartKey) -> usize {
        self.upload_calls.get(&key).copied().unwrap_or_default()
    }

    #[must_use]
    pub fn download_calls(&self, key: RemotePartKey) -> usize {
        self.download_calls.get(&key).copied().unwrap_or_default()
    }

    #[must_use]
    pub fn final_bytes(&self, destination_id: DestinationId) -> Option<&[u8]> {
        self.destinations
            .get(&destination_id)
            .and_then(|destination| destination.final_bytes.as_deref())
    }

    #[must_use]
    pub fn partial_bytes(&self, destination_id: DestinationId) -> Option<&[u8]> {
        self.destinations
            .get(&destination_id)
            .and_then(|destination| destination.partial.as_deref())
    }

    #[must_use]
    pub fn checkpoint(&self, transfer_id: TransferId) -> Option<&TransferCheckpoint> {
        self.checkpoints.get(&transfer_id)
    }

    fn store_remote(&mut self, key: RemotePartKey, bytes: Vec<u8>) -> RemoteObject {
        let object = RemoteObject {
            object_id: self.next_remote_id,
            key,
            plaintext_size: bytes.len() as u64,
            encoded_size: bytes.len() as u64,
            digest: self.digest(&bytes),
        };
        self.next_remote_id = self.next_remote_id.saturating_add(1);
        self.remotes.entry(key).or_default().push(StoredRemote {
            object: object.clone(),
            bytes,
        });
        object
    }
}

impl DigestPort for FakeEnvironment {
    fn digest(&self, bytes: &[u8]) -> ContentDigest {
        let mut states = [
            0xcbf2_9ce4_8422_2325_u64,
            0x8422_2325_cbf2_9ce4_u64,
            0x9e37_79b9_7f4a_7c15_u64,
            0xd6e8_feb8_6659_fd93_u64,
        ];
        for (position, byte) in bytes.iter().copied().enumerate() {
            for (lane, state) in states.iter_mut().enumerate() {
                *state ^= u64::from(byte)
                    .wrapping_add(position as u64)
                    .wrapping_add(lane as u64);
                *state = state.wrapping_mul(0x100_0000_01b3);
                *state ^= *state >> 29;
            }
        }
        let mut output = [0_u8; 32];
        for (lane, state) in states.into_iter().enumerate() {
            output[lane * 8..lane * 8 + 8].copy_from_slice(&state.to_be_bytes());
        }
        ContentDigest(output)
    }
}

impl SourcePort for FakeEnvironment {
    fn source_identity(&self, source_id: SourceId) -> Result<SourceIdentity, TransferError> {
        self.sources
            .get(&source_id)
            .map(|source| source.identity)
            .ok_or(TransferError::SourceMissing)
    }

    fn read_source_range(
        &mut self,
        source_id: SourceId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, TransferError> {
        let range = checked_range(offset, length, "source")?;
        let source = self
            .sources
            .get_mut(&source_id)
            .ok_or(TransferError::SourceMissing)?;
        let bytes = source
            .bytes
            .get(range)
            .ok_or(TransferError::SourceChanged)?
            .to_vec();
        if let Some(replacement) = source.mutation_after_read.take() {
            source.bytes = replacement;
            source.identity.size_bytes = source.bytes.len() as u64;
            source.identity.modified_at_units = source.identity.modified_at_units.saturating_add(1);
            source.identity.revision = source.identity.revision.saturating_add(1);
        }
        Ok(bytes)
    }

    fn digest_source(&mut self, source_id: SourceId) -> Result<ContentDigest, TransferError> {
        self.sources
            .get(&source_id)
            .map(|source| self.digest(&source.bytes))
            .ok_or(TransferError::SourceMissing)
    }
}

impl RemoteTransport for FakeEnvironment {
    fn discover_remote(&mut self, key: RemotePartKey) -> Result<Vec<RemoteObject>, TransferError> {
        Ok(self
            .remotes
            .get(&key)
            .map(|objects| objects.iter().map(|stored| stored.object.clone()).collect())
            .unwrap_or_default())
    }

    fn upload_remote(
        &mut self,
        key: RemotePartKey,
        bytes: &[u8],
        digest: ContentDigest,
    ) -> Result<RemoteObject, UploadError> {
        *self.upload_calls.entry(key).or_default() += 1;
        let behavior = self
            .upload_behaviors
            .get_mut(&key)
            .and_then(VecDeque::pop_front)
            .unwrap_or(FakeUploadBehavior::Succeed);
        match behavior {
            FakeUploadBehavior::NetworkFailure => {
                return Err(UploadError::Definite(TransferError::Network));
            }
            FakeUploadBehavior::FloodWait { millis } => {
                return Err(UploadError::Definite(TransferError::FloodWait {
                    retry_after: Duration::from_millis(millis),
                }));
            }
            FakeUploadBehavior::Succeed | FakeUploadBehavior::AmbiguousSuccess => {}
        }
        if self.digest(bytes) != digest {
            return Err(UploadError::Definite(TransferError::HashMismatch));
        }
        let object = self.store_remote(key, bytes.to_vec());
        if behavior == FakeUploadBehavior::AmbiguousSuccess {
            Err(UploadError::AmbiguousSuccess)
        } else {
            Ok(object)
        }
    }

    fn verify_remote(
        &mut self,
        object: &RemoteObject,
        expected_size: u64,
        expected_digest: ContentDigest,
    ) -> Result<(), TransferError> {
        let stored = self
            .remotes
            .get(&object.key)
            .and_then(|objects| {
                objects
                    .iter()
                    .find(|stored| stored.object.object_id == object.object_id)
            })
            .ok_or(TransferError::RemoteMissing)?;
        if stored.object.plaintext_size != expected_size
            || stored.object.digest != expected_digest
            || stored.bytes.len() as u64 != expected_size
            || self.digest(&stored.bytes) != expected_digest
        {
            return Err(TransferError::HashMismatch);
        }
        Ok(())
    }

    fn download_remote(&mut self, object: &RemoteObject) -> Result<Vec<u8>, TransferError> {
        *self.download_calls.entry(object.key).or_default() += 1;
        let mut bytes = self
            .remotes
            .get(&object.key)
            .and_then(|objects| {
                objects
                    .iter()
                    .find(|stored| stored.object.object_id == object.object_id)
            })
            .map(|stored| stored.bytes.clone())
            .ok_or(TransferError::RemoteMissing)?;
        if self
            .corrupt_next_download
            .remove(&object.key)
            .unwrap_or_default()
            && let Some(first) = bytes.first_mut()
        {
            *first ^= 1;
        }
        Ok(bytes)
    }
}

impl CheckpointPort for FakeEnvironment {
    fn load_checkpoint(
        &mut self,
        transfer_id: TransferId,
    ) -> Result<Option<TransferCheckpoint>, TransferError> {
        Ok(self.checkpoints.get(&transfer_id).cloned())
    }

    fn save_checkpoint(&mut self, checkpoint: &TransferCheckpoint) -> Result<(), TransferError> {
        if self.fail_next_checkpoint_save {
            self.fail_next_checkpoint_save = false;
            return Err(TransferError::Database);
        }
        self.checkpoints
            .insert(checkpoint.transfer_id, checkpoint.clone());
        Ok(())
    }
}

impl FileSystemPort for FakeEnvironment {
    fn prepare_partial(
        &mut self,
        destination_id: DestinationId,
        total_bytes: u64,
    ) -> Result<(), TransferError> {
        let length = usize::try_from(total_bytes).map_err(|_| TransferError::DiskFull)?;
        let destination = self.destinations.entry(destination_id).or_default();
        if destination.final_bytes.is_some() {
            return Err(TransferError::PermissionDenied);
        }
        match &destination.partial {
            Some(existing) if existing.len() != length => return Err(TransferError::HashMismatch),
            Some(_) => {}
            None => destination.partial = Some(vec![0_u8; length]),
        }
        Ok(())
    }

    fn write_partial(
        &mut self,
        destination_id: DestinationId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), TransferError> {
        let range = checked_range(offset, bytes.len() as u64, "destination")?;
        let partial = self
            .destinations
            .get_mut(&destination_id)
            .and_then(|destination| destination.partial.as_mut())
            .ok_or(TransferError::PermissionDenied)?;
        let target = partial.get_mut(range).ok_or(TransferError::DiskFull)?;
        target.copy_from_slice(bytes);
        Ok(())
    }

    fn read_partial(
        &mut self,
        destination_id: DestinationId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, TransferError> {
        let range = checked_range(offset, length, "destination")?;
        self.destinations
            .get(&destination_id)
            .and_then(|destination| destination.partial.as_ref())
            .and_then(|partial| partial.get(range))
            .map(ToOwned::to_owned)
            .ok_or(TransferError::HashMismatch)
    }

    fn digest_partial(
        &mut self,
        destination_id: DestinationId,
    ) -> Result<ContentDigest, TransferError> {
        self.destinations
            .get(&destination_id)
            .and_then(|destination| destination.partial.as_ref())
            .map(|bytes| self.digest(bytes))
            .ok_or(TransferError::HashMismatch)
    }

    fn flush_partial(&mut self, destination_id: DestinationId) -> Result<(), TransferError> {
        let destination = self
            .destinations
            .get_mut(&destination_id)
            .ok_or(TransferError::PermissionDenied)?;
        if destination.partial.is_none() {
            return Err(TransferError::PermissionDenied);
        }
        destination.partial_flushed = true;
        Ok(())
    }

    fn atomic_finalize(&mut self, destination_id: DestinationId) -> Result<(), TransferError> {
        let destination = self
            .destinations
            .get_mut(&destination_id)
            .ok_or(TransferError::PermissionDenied)?;
        if !destination.partial_flushed || destination.final_bytes.is_some() {
            return Err(TransferError::PermissionDenied);
        }
        destination.final_bytes = destination.partial.take();
        Ok(())
    }

    fn final_exists(&self, destination_id: DestinationId) -> bool {
        self.destinations
            .get(&destination_id)
            .is_some_and(|destination| destination.final_bytes.is_some())
    }
}

/// Manually advanced clock; it never sleeps.
#[derive(Default)]
pub struct FakeClock {
    now_ms: Cell<u64>,
}

impl FakeClock {
    #[must_use]
    pub const fn new(now_ms: u64) -> Self {
        Self {
            now_ms: Cell::new(now_ms),
        }
    }

    pub fn set(&self, now_ms: u64) {
        self.now_ms.set(now_ms);
    }

    pub fn advance(&self, millis: u64) {
        self.now_ms.set(self.now_ms.get().saturating_add(millis));
    }
}

impl Clock for FakeClock {
    fn now_millis(&self) -> u64 {
        self.now_ms.get()
    }
}

/// Deterministic finite jitter sequence; exhausted sequences return zero.
#[derive(Default)]
pub struct SequenceJitter {
    values: VecDeque<u64>,
}

impl SequenceJitter {
    #[must_use]
    pub fn new(values: impl IntoIterator<Item = u64>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }
}

impl JitterSource for SequenceJitter {
    fn jitter_millis(&mut self, maximum_millis: u64) -> u64 {
        self.values
            .pop_front()
            .unwrap_or_default()
            .min(maximum_millis)
    }
}
