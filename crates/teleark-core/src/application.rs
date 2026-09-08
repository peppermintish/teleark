use std::{error::Error, fmt, path::PathBuf};

use crate::{FileKind, LogicalFile, LogicalFileId, RemoteState, VerificationState};

/// Maximum page size accepted by the frontend-neutral library API.
pub const MAX_LIBRARY_PAGE_SIZE: usize = 500;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LibrarySort {
    ModifiedNewest,
    ModifiedOldest,
    NameAscending,
    NameDescending,
    SizeLargest,
    SizeSmallest,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryFilter {
    pub kinds: Vec<FileKind>,
    pub remote_state: Option<RemoteState>,
    pub verification_state: Option<VerificationState>,
    pub source_account_id: Option<crate::AccountId>,
    pub source_chat_id: Option<crate::ChatId>,
    pub minimum_size_bytes: Option<u64>,
    pub maximum_size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryQuery {
    pub text: String,
    pub filter: LibraryFilter,
    pub sort: LibrarySort,
    pub page_size: usize,
    /// Opaque repository cursor returned by a previous page.
    pub after: Option<String>,
}

impl Default for LibraryQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            filter: LibraryFilter::default(),
            sort: LibrarySort::ModifiedNewest,
            page_size: 100,
            after: None,
        }
    }
}

impl LibraryQuery {
    pub fn validate(&self) -> Result<(), ApplicationError> {
        if self.page_size == 0 || self.page_size > MAX_LIBRARY_PAGE_SIZE {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        if self
            .filter
            .minimum_size_bytes
            .zip(self.filter.maximum_size_bytes)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        Ok(())
    }
}

/// One file row returned to any frontend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryItem {
    pub file: LogicalFile,
    pub source_name: Option<String>,
    /// Unambiguous native Telegram document identity, scoped by `file`'s account/chat.
    /// Indexed presence is not evidence that this application uploaded the file.
    pub source_message_id: Option<crate::MessageId>,
    /// Original local source, when this catalog item was imported from disk.
    ///
    /// Frontends may offer platform-native open and reveal actions for this
    /// path, but must never translate or reinterpret it.
    pub local_source_path: Option<PathBuf>,
    pub part_count: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryPage {
    pub items: Vec<LibraryItem>,
    pub next: Option<String>,
    pub total_matching: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LibraryStatistics {
    pub logical_file_count: u64,
    pub logical_bytes: u64,
    pub local_file_count: u64,
    pub remote_file_count: u64,
    pub active_transfer_count: u64,
}

/// Metadata supplied when a user imports a local source into the library.
///
/// The source path stays local and is never localized or inferred from a
/// Telegram name. The repository allocates the stable logical-file ID in the
/// same transaction that persists this record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportLocalFile {
    pub source_path: PathBuf,
    pub name: String,
    pub size_bytes: u64,
    pub kind: FileKind,
    pub modified_at_unix_ms: Option<i64>,
}

impl ImportLocalFile {
    pub fn validate(&self) -> Result<(), ApplicationError> {
        if self.source_path.as_os_str().is_empty() || self.name.trim().is_empty() {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ApplicationErrorKind {
    InvalidRequest,
    NotFound,
    Conflict,
    Persistence,
    SourceMissing,
    SourceChanged,
    PermissionDenied,
    /// A local plaintext upload source cannot be read.
    SourcePermissionDenied,
    /// The fixed storage channel cannot be reached by this account.
    StorageAccessDenied,
    /// The fixed channel no longer satisfies private owner-only storage policy.
    StorageConfigurationUnsafe,
    /// Management records need repair; file data may still be usable.
    StorageIdentityDamaged,
    /// A newer channel identity must not be overwritten by an older app.
    StorageIdentityUnsupported,
    /// The package belongs to a key epoch that is currently locked/unavailable.
    VaultKeyUnavailable,
    Capacity,
    Authorization,
    Network,
    /// Telegram accepted the request but its server failed to process it.
    Server,
    Cancelled,
}

/// Stable locale-neutral failure from an application service or port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationError {
    kind: ApplicationErrorKind,
}

impl ApplicationError {
    pub const fn new(kind: ApplicationErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(&self) -> ApplicationErrorKind {
        self.kind
    }
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TeleArk application failure: {:?}", self.kind)
    }
}

impl Error for ApplicationError {}

/// Persistence port used by the Core application service.
///
/// Implementations own transactions and durable representation details. The
/// API intentionally exposes neither SQL nor adapter-specific records.
pub trait LibraryRepository: Send + Sync {
    fn search(&self, query: &LibraryQuery) -> Result<LibraryPage, ApplicationError>;

    fn statistics(&self) -> Result<LibraryStatistics, ApplicationError>;

    fn import_local_file(&self, command: &ImportLocalFile)
    -> Result<LogicalFile, ApplicationError>;

    fn file(&self, id: LogicalFileId) -> Result<Option<LogicalFile>, ApplicationError>;

    fn delete_file(&self, id: LogicalFileId) -> Result<bool, ApplicationError>;
}

/// Core-owned, frontend-neutral library use cases.
pub struct LibraryService<R> {
    repository: R,
}

impl<R> LibraryService<R>
where
    R: LibraryRepository,
{
    pub const fn new(repository: R) -> Self {
        Self { repository }
    }

    pub fn search_files(&self, query: &LibraryQuery) -> Result<LibraryPage, ApplicationError> {
        query.validate()?;
        self.repository.search(query)
    }

    pub fn library_statistics(&self) -> Result<LibraryStatistics, ApplicationError> {
        self.repository.statistics()
    }

    pub fn import_local_file(
        &self,
        command: &ImportLocalFile,
    ) -> Result<LogicalFile, ApplicationError> {
        command.validate()?;
        self.repository.import_local_file(command)
    }

    pub fn file(&self, id: LogicalFileId) -> Result<Option<LogicalFile>, ApplicationError> {
        self.repository.file(id)
    }

    pub fn delete_file(&self, id: LogicalFileId) -> Result<bool, ApplicationError> {
        self.repository.delete_file(id)
    }

    pub fn into_inner(self) -> R {
        self.repository
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeRepository {
        imported: Mutex<Vec<ImportLocalFile>>,
    }

    impl LibraryRepository for FakeRepository {
        fn search(&self, _query: &LibraryQuery) -> Result<LibraryPage, ApplicationError> {
            Ok(LibraryPage::default())
        }

        fn statistics(&self) -> Result<LibraryStatistics, ApplicationError> {
            Ok(LibraryStatistics::default())
        }

        fn import_local_file(
            &self,
            command: &ImportLocalFile,
        ) -> Result<LogicalFile, ApplicationError> {
            self.imported
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .push(command.clone());
            LogicalFile::new(
                LogicalFileId::new(1),
                command.name.clone(),
                command.size_bytes,
                command.kind,
            )
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))
        }

        fn file(&self, _id: LogicalFileId) -> Result<Option<LogicalFile>, ApplicationError> {
            Ok(None)
        }

        fn delete_file(&self, _id: LogicalFileId) -> Result<bool, ApplicationError> {
            Ok(false)
        }
    }

    #[test]
    fn invalid_query_never_reaches_the_repository() {
        let service = LibraryService::new(FakeRepository::default());
        let query = LibraryQuery {
            page_size: MAX_LIBRARY_PAGE_SIZE + 1,
            ..LibraryQuery::default()
        };
        let error = service
            .search_files(&query)
            .expect_err("oversized page must be rejected");
        assert_eq!(error.kind(), ApplicationErrorKind::InvalidRequest);
    }

    #[test]
    fn local_import_is_validated_and_delegated() {
        let service = LibraryService::new(FakeRepository::default());
        let command = ImportLocalFile {
            source_path: PathBuf::from("/tmp/example.pdf"),
            name: "example.pdf".to_owned(),
            size_bytes: 42,
            kind: FileKind::Document,
            modified_at_unix_ms: Some(7),
        };
        let file = service.import_local_file(&command).expect("valid import");
        assert_eq!(file.name, "example.pdf");
        assert_eq!(service.repository.imported.lock().expect("lock").len(), 1);
    }

    #[test]
    fn empty_source_path_is_rejected() {
        let service = LibraryService::new(FakeRepository::default());
        let command = ImportLocalFile {
            source_path: PathBuf::new(),
            name: "example.pdf".to_owned(),
            size_bytes: 42,
            kind: FileKind::Document,
            modified_at_unix_ms: None,
        };
        let error = service
            .import_local_file(&command)
            .expect_err("empty source path must fail");
        assert_eq!(error.kind(), ApplicationErrorKind::InvalidRequest);
    }
}
