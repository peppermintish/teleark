//! Frontend-neutral domain types for TeleArk.
//!
//! This crate deliberately has no GUI, persistence, networking, or localization
//! dependencies. It is shared by every frontend and by infrastructure adapters.

mod collection;
mod error;
mod event;
mod file;
mod ids;
mod index;
mod transfer;

pub use collection::{Collection, CollectionError, CollectionKind, CollectionRule};
pub use error::{DomainValidationError, ErrorDisposition, TransferError};
pub use event::{CoreEvent, VaultState};
pub use file::{
    Blake3Digest, EncryptionState, FileKind, FilePart, LogicalFile, Package, RemoteLocator,
    RemoteObject, RemoteObjectRole, RemoteState, VerificationState,
};
pub use ids::{
    AccountId, ChatId, CollectionId, IndexJobId, LogicalFileId, MessageId, PackageId, PartIndex,
    RemoteObjectId, TransferId,
};
pub use index::{
    IndexCoverage, IndexJob, IndexJobState, IndexRange, IndexRangeSet, IndexTransitionError,
};
pub use transfer::{
    PartState, TransferDirection, TransferPart, TransferPriority, TransferProgress, TransferState,
    TransferTask, TransferTransitionError,
};
