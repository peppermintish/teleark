use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use crate::{
    AccountId, ChatId, CollectionId, DomainValidationError, EncryptionState, FileKind, LogicalFile,
    LogicalFileId, VerificationState,
};

/// A locale-neutral smart-collection predicate over logical-file metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CollectionRule {
    source: Option<(AccountId, ChatId)>,
    file_kind: Option<FileKind>,
    extension: Option<String>,
    minimum_size_bytes: Option<u64>,
    maximum_size_bytes: Option<u64>,
    encrypted: Option<bool>,
    verified: Option<bool>,
    multipart: Option<bool>,
}

impl CollectionRule {
    pub const fn new() -> Self {
        Self {
            source: None,
            file_kind: None,
            extension: None,
            minimum_size_bytes: None,
            maximum_size_bytes: None,
            encrypted: None,
            verified: None,
            multipart: None,
        }
    }

    pub const fn with_source(mut self, account_id: AccountId, chat_id: ChatId) -> Self {
        self.source = Some((account_id, chat_id));
        self
    }

    pub const fn with_file_kind(mut self, file_kind: FileKind) -> Self {
        self.file_kind = Some(file_kind);
        self
    }

    pub fn with_extension(mut self, extension: impl AsRef<str>) -> Self {
        let normalized = extension
            .as_ref()
            .trim()
            .trim_start_matches('.')
            .to_lowercase();
        self.extension = (!normalized.is_empty()).then_some(normalized);
        self
    }

    pub fn with_size_range(
        mut self,
        minimum_size_bytes: Option<u64>,
        maximum_size_bytes: Option<u64>,
    ) -> Result<Self, DomainValidationError> {
        if let (Some(minimum_bytes), Some(maximum_bytes)) = (minimum_size_bytes, maximum_size_bytes)
            && minimum_bytes > maximum_bytes
        {
            return Err(DomainValidationError::InvalidCollectionSizeBounds {
                minimum_bytes,
                maximum_bytes,
            });
        }
        self.minimum_size_bytes = minimum_size_bytes;
        self.maximum_size_bytes = maximum_size_bytes;
        Ok(self)
    }

    pub const fn with_encrypted(mut self, encrypted: bool) -> Self {
        self.encrypted = Some(encrypted);
        self
    }

    pub const fn with_verified(mut self, verified: bool) -> Self {
        self.verified = Some(verified);
        self
    }

    pub const fn with_multipart(mut self, multipart: bool) -> Self {
        self.multipart = Some(multipart);
        self
    }

    pub fn matches(&self, file: &LogicalFile) -> bool {
        if self.source.is_some_and(|(account_id, chat_id)| {
            file.source_account_id != Some(account_id) || file.source_chat_id != Some(chat_id)
        }) {
            return false;
        }
        if self.file_kind.is_some_and(|kind| file.kind != kind) {
            return false;
        }
        if self.extension.as_ref().is_some_and(|expected| {
            !file
                .extension()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
        }) {
            return false;
        }
        if self
            .minimum_size_bytes
            .is_some_and(|minimum| file.size_bytes < minimum)
        {
            return false;
        }
        if self
            .maximum_size_bytes
            .is_some_and(|maximum| file.size_bytes > maximum)
        {
            return false;
        }
        if self.encrypted.is_some_and(|expected| {
            let actual = file.encryption_state != EncryptionState::Unencrypted;
            actual != expected
        }) {
            return false;
        }
        if self.verified.is_some_and(|expected| {
            let actual = file.verification_state == VerificationState::Verified;
            actual != expected
        }) {
            return false;
        }
        if self
            .multipart
            .is_some_and(|expected| file.package_id.is_some() != expected)
        {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CollectionKind {
    Manual {
        logical_file_ids: BTreeSet<LogicalFileId>,
    },
    Smart {
        rule: CollectionRule,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Collection {
    id: CollectionId,
    name: String,
    kind: CollectionKind,
}

impl Collection {
    pub fn new_manual(
        id: CollectionId,
        name: impl Into<String>,
    ) -> Result<Self, DomainValidationError> {
        Self::with_kind(
            id,
            name,
            CollectionKind::Manual {
                logical_file_ids: BTreeSet::new(),
            },
        )
    }

    pub fn new_smart(
        id: CollectionId,
        name: impl Into<String>,
        rule: CollectionRule,
    ) -> Result<Self, DomainValidationError> {
        Self::with_kind(id, name, CollectionKind::Smart { rule })
    }

    fn with_kind(
        id: CollectionId,
        name: impl Into<String>,
        kind: CollectionKind,
    ) -> Result<Self, DomainValidationError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(DomainValidationError::EmptyCollectionName);
        }
        Ok(Self { id, name, kind })
    }

    pub const fn id(&self) -> CollectionId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn kind(&self) -> &CollectionKind {
        &self.kind
    }

    pub fn rename(&mut self, name: impl Into<String>) -> Result<(), DomainValidationError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(DomainValidationError::EmptyCollectionName);
        }
        self.name = name;
        Ok(())
    }

    /// Adds only a logical-file identity. Multipart pieces cannot be members.
    pub fn add_file(&mut self, logical_file_id: LogicalFileId) -> Result<bool, CollectionError> {
        match &mut self.kind {
            CollectionKind::Manual { logical_file_ids } => {
                Ok(logical_file_ids.insert(logical_file_id))
            }
            CollectionKind::Smart { .. } => Err(CollectionError::SmartMembershipDerived),
        }
    }

    pub fn remove_file(&mut self, logical_file_id: LogicalFileId) -> Result<bool, CollectionError> {
        match &mut self.kind {
            CollectionKind::Manual { logical_file_ids } => {
                Ok(logical_file_ids.remove(&logical_file_id))
            }
            CollectionKind::Smart { .. } => Err(CollectionError::SmartMembershipDerived),
        }
    }

    pub fn contains(&self, file: &LogicalFile) -> bool {
        match &self.kind {
            CollectionKind::Manual { logical_file_ids } => logical_file_ids.contains(&file.id),
            CollectionKind::Smart { rule } => rule.matches(file),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionError {
    SmartMembershipDerived,
}

impl fmt::Display for CollectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SmartMembershipDerived => {
                formatter.write_str("smart collection membership is derived from its rule")
            }
        }
    }
}

impl Error for CollectionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileKind, LogicalFile, LogicalFileId};

    fn file() -> LogicalFile {
        let mut file = LogicalFile::new(
            LogicalFileId::new(5),
            "TeleArk.Release.DMG",
            500,
            FileKind::DiskImage,
        )
        .unwrap();
        file.source_account_id = Some(AccountId::new(1));
        file.source_chat_id = Some(ChatId::new(2));
        file.verification_state = VerificationState::Verified;
        file
    }

    #[test]
    fn manual_collection_contains_logical_files_only() {
        let logical_file = file();
        let mut collection = Collection::new_manual(CollectionId::new(1), "Releases").unwrap();

        assert!(collection.add_file(logical_file.id).unwrap());
        assert!(collection.contains(&logical_file));
        assert!(!collection.add_file(logical_file.id).unwrap());
    }

    #[test]
    fn smart_collection_matches_normalized_facets() {
        let rule = CollectionRule::new()
            .with_source(AccountId::new(1), ChatId::new(2))
            .with_file_kind(FileKind::DiskImage)
            .with_extension(".dmg")
            .with_size_range(Some(100), Some(1_000))
            .unwrap()
            .with_verified(true);
        let collection = Collection::new_smart(CollectionId::new(2), "Mac", rule).unwrap();

        assert!(collection.contains(&file()));
    }

    #[test]
    fn smart_collection_rejects_manual_membership_mutation() {
        let mut collection =
            Collection::new_smart(CollectionId::new(2), "Dynamic", CollectionRule::new()).unwrap();

        assert_eq!(
            collection.add_file(LogicalFileId::new(3)),
            Err(CollectionError::SmartMembershipDerived)
        );
    }
}
