use teleark_core::{AccountId, PackageId, PartIndex, TransferId};

/// Fixed-size integrity result supplied by an injected digest implementation.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContentDigest(pub [u8; 32]);

/// Project-owned local source handle.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceId(pub u64);

/// Project-owned destination handle. It is not a user-controlled path string.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DestinationId(pub u64);

/// Source identity captured before upload and checked at safe boundaries.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceIdentity {
    pub filesystem_id: u128,
    pub size_bytes: u64,
    pub modified_at_units: u64,
    pub revision: u64,
}

/// Stable idempotency key for one application part in fake/project-owned
/// remote transport APIs.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RemotePartKey {
    pub account_id: AccountId,
    pub package_id: PackageId,
    pub part_index: PartIndex,
}

/// Verified remote metadata. Payload bytes remain behind the transport port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteObject {
    pub object_id: u64,
    pub key: RemotePartKey,
    pub encoded_size: u64,
    pub digest: ContentDigest,
}

/// Evidence that one application part is safe to skip after restart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedPart {
    pub size_bytes: u64,
    pub digest: ContentDigest,
    pub remote_object: Option<RemoteObject>,
}

/// Durable per-part checkpoint owned by the transfer subsystem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartCheckpoint {
    pub part_index: PartIndex,
    pub attempts: u32,
    pub retry_not_before_ms: u64,
    pub verified: Option<VerifiedPart>,
}

/// Durable task checkpoint. Implementations persist replacement atomically.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferCheckpoint {
    pub transfer_id: TransferId,
    pub source_identity: Option<SourceIdentity>,
    pub destination_id: Option<DestinationId>,
    pub parts: Vec<PartCheckpoint>,
}

impl TransferCheckpoint {
    pub(crate) fn part(&self, index: PartIndex) -> Option<&PartCheckpoint> {
        self.parts
            .get(index.get() as usize)
            .filter(|part| part.part_index == index)
    }

    pub(crate) fn part_mut(&mut self, index: PartIndex) -> Option<&mut PartCheckpoint> {
        self.parts
            .get_mut(index.get() as usize)
            .filter(|part| part.part_index == index)
    }
}
