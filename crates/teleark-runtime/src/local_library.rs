//! Paged, cancellable disk observations for the Library's local projection.
//! These are local copies, not synthetic LogicalFile catalog identities.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use teleark_core::{
    ApplicationError, ApplicationErrorKind, FileKind, LibraryFilter, LibraryQuery, LogicalFileId,
    RemoteState,
};

use crate::{DesktopLibrary, DownloadedFilesCursor, classify_file};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalLibraryKey {
    NativeDownload(u64),
    VaultDownload(u64),
    Imported(LogicalFileId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalLibraryFile {
    pub key: LocalLibraryKey,
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub kind: FileKind,
    pub modified_at_unix_ms: Option<i64>,
    pub account_id: Option<i64>,
    pub chat_id: Option<i64>,
    pub message_id: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Position {
    Downloads(Option<DownloadedFilesCursor>),
    Imports(Option<String>),
}

/// Ephemeral cursor bound to the source account and filters; never persisted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalLibraryCursor {
    account_id: Option<i64>,
    text: String,
    kind: Option<FileKind>,
    position: Position,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalLibraryPage {
    pub files: Vec<LocalLibraryFile>,
    pub next: Option<LocalLibraryCursor>,
}

#[derive(Clone, Default)]
pub struct LocalLibraryCancellation(Arc<AtomicBool>);

impl LocalLibraryCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn check(&self) -> Result<(), ApplicationError> {
        if self.0.load(Ordering::Acquire) {
            Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
        } else {
            Ok(())
        }
    }
}

impl DesktopLibrary {
    /// Reads at most 128 candidates at a time off the UI thread. Empty pages
    /// (deleted files or unmatched filters) are skipped cooperatively until a
    /// visible page is found. Neither observation nor paging initiates a transfer.
    pub fn local_library_page(
        &self,
        account_id: Option<i64>,
        text: &str,
        kind: Option<FileKind>,
        after: Option<LocalLibraryCursor>,
        cancellation: &LocalLibraryCancellation,
    ) -> Result<LocalLibraryPage, ApplicationError> {
        if account_id.is_some_and(|id| id <= 0) {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let text = text.trim().to_lowercase();
        let mut cursor = after.unwrap_or_else(|| LocalLibraryCursor {
            account_id,
            text: text.clone(),
            kind,
            position: if account_id.is_some() {
                Position::Downloads(None)
            } else {
                Position::Imports(None)
            },
        });
        if cursor.account_id != account_id || cursor.text != text || cursor.kind != kind {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        loop {
            cancellation.check()?;
            let mut files = Vec::new();
            let has_more = match cursor.position.clone() {
                Position::Downloads(after) => {
                    let account = account_id.ok_or_else(|| {
                        ApplicationError::new(ApplicationErrorKind::Authorization)
                    })?;
                    let page = self.downloaded_files_page(account, after)?;
                    cursor.position = if page.len() == 128 {
                        Position::Downloads(page.last().map(|file| file.cursor))
                    } else {
                        Position::Imports(None)
                    };
                    for record in page {
                        cancellation.check()?;
                        let name = record
                            .destination
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let key = if record.cursor.kind == 0 {
                            LocalLibraryKey::NativeDownload(record.cursor.id)
                        } else {
                            LocalLibraryKey::VaultDownload(record.cursor.id)
                        };
                        if let Some(file) = observe(
                            LocalLibraryFile {
                                key,
                                name,
                                kind: classify_file(&record.destination),
                                path: record.destination,
                                size_bytes: record.size_bytes,
                                modified_at_unix_ms: None,
                                account_id: Some(record.account_id),
                                chat_id: Some(record.chat_id),
                                message_id: record.message_id,
                            },
                            &text,
                            kind,
                        ) {
                            files.push(file);
                        }
                    }
                    true
                }
                Position::Imports(after) => {
                    let page = self.search(&LibraryQuery {
                        filter: LibraryFilter {
                            remote_state: Some(RemoteState::LocalOnly),
                            ..LibraryFilter::default()
                        },
                        page_size: 128,
                        after,
                        ..LibraryQuery::default()
                    })?;
                    let has_more = page.next.is_some();
                    cursor.position = Position::Imports(page.next);
                    for item in page.items {
                        cancellation.check()?;
                        if let Some(path) = item.local_source_path
                            && let Some(file) = observe(
                                LocalLibraryFile {
                                    key: LocalLibraryKey::Imported(item.file.id),
                                    name: item.file.name,
                                    path,
                                    size_bytes: item.file.size_bytes,
                                    kind: item.file.kind,
                                    modified_at_unix_ms: item.file.modified_at_unix_ms,
                                    account_id: None,
                                    chat_id: None,
                                    message_id: None,
                                },
                                &text,
                                kind,
                            )
                        {
                            files.push(file);
                        }
                    }
                    has_more
                }
            };
            cancellation.check()?;
            if !files.is_empty() || !has_more {
                return Ok(LocalLibraryPage {
                    files,
                    next: has_more.then_some(cursor),
                });
            }
        }
    }
}

fn observe(
    mut file: LocalLibraryFile,
    text: &str,
    kind: Option<FileKind>,
) -> Option<LocalLibraryFile> {
    if kind.is_some_and(|kind| kind != file.kind) || !file.name.to_lowercase().contains(text) {
        return None;
    }
    let metadata = std::fs::metadata(&file.path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    file.size_bytes = metadata.len();
    file.modified_at_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use teleark_storage::{
        NewNativeDownloadTaskRecord, StoredNativeDownloadState, StoredNativeDownloadVerification,
        VaultDownloadRecord,
    };

    fn native(library: &DesktopLibrary, path: &std::path::Path, account: i64, completed: bool) {
        std::fs::write(path, b"x").expect("fixture bytes");
        let mut task = library
            .insert_native_download(NewNativeDownloadTaskRecord {
                account_id: account,
                chat_id: 100,
                message_id: 200,
                file_name: path
                    .file_name()
                    .expect("name")
                    .to_string_lossy()
                    .into_owned(),
                size_bytes: 1,
                destination: path.to_owned(),
                message_sent_at_unix_ms: None,
                caption: None,
                mime_type: None,
                created_at_unix_ms: 1,
            })
            .expect("native history");
        if completed {
            task.state = StoredNativeDownloadState::Completed;
            task.verification = StoredNativeDownloadVerification::SizeChecked;
            task.transferred_bytes = 1;
            task.finished_at_unix_ms = Some(2);
            task.updated_at_unix_ms = 2;
            library
                .save_native_download(task)
                .expect("complete history");
        }
    }

    fn all(
        library: &DesktopLibrary,
        account: Option<i64>,
        text: &str,
        kind: Option<FileKind>,
    ) -> Vec<LocalLibraryFile> {
        let mut cursor = None;
        let mut files = Vec::new();
        loop {
            let page = library
                .local_library_page(
                    account,
                    text,
                    kind,
                    cursor,
                    &LocalLibraryCancellation::default(),
                )
                .expect("local page");
            files.extend(page.files);
            cursor = page.next;
            if cursor.is_none() {
                return files;
            }
        }
    }

    #[test]
    fn local_view_requires_existing_files_and_keeps_account_and_copy_identity() {
        let root = tempfile::tempdir().expect("root");
        let library = DesktopLibrary::open(root.path().join("library.sqlite3")).expect("library");
        let native_path = root.path().join("京都.pdf");
        native(&library, &native_path, 1, true);
        native(&library, &root.path().join("other-account.pdf"), 2, true);
        native(&library, &root.path().join("queued.pdf"), 1, false);
        let removed = root.path().join("removed.pdf");
        native(&library, &removed, 1, true);
        std::fs::remove_file(&removed).expect("external deletion");
        std::fs::write(&native_path, b"changed bytes").expect("external size change");
        let vault_path = root.path().join("restored.zip");
        std::fs::write(&vault_path, b"vault").expect("vault bytes");
        library
            .record_vault_download(VaultDownloadRecord {
                account_id: 1,
                chat_id: 100,
                package_id: "a".repeat(32),
                destination: vault_path,
                size_bytes: 5,
                completed_at_unix_ms: 3,
            })
            .expect("vault inventory");
        let imported = root.path().join("notes.txt");
        std::fs::write(&imported, b"notes").expect("import fixture");
        assert!(
            library
                .import_paths(vec![imported.clone()])
                .into_iter()
                .all(|result| result.is_ok())
        );
        let files = all(&library, Some(1), "", None);
        assert_eq!(files.len(), 3);
        assert!(
            files
                .iter()
                .any(|file| file.name == "京都.pdf" && file.size_bytes == 13)
        );
        assert!(
            files
                .iter()
                .any(|file| matches!(file.key, LocalLibraryKey::NativeDownload(1)))
        );
        assert!(
            files
                .iter()
                .any(|file| matches!(file.key, LocalLibraryKey::VaultDownload(1)))
        );
        assert_eq!(
            all(&library, Some(1), "京都", Some(FileKind::Document)).len(),
            1
        );
        assert!(all(&library, Some(1), "京都", Some(FileKind::Archive)).is_empty());
        assert_eq!(all(&library, None, "", None).len(), 1);
        std::fs::remove_file(&imported).expect("remove imported original");
        assert!(all(&library, None, "", None).is_empty());
    }

    #[test]
    fn local_paging_skips_deleted_pages_and_rejects_changed_filters_and_cancellation() {
        let root = tempfile::tempdir().expect("root");
        let library = DesktopLibrary::open(root.path().join("library.sqlite3")).expect("library");
        native(&library, &root.path().join("oldest.txt"), 1, true);
        for index in 0..128 {
            let path = root.path().join(format!("deleted-{index}.txt"));
            native(&library, &path, 1, true);
            std::fs::remove_file(path).expect("delete newest outputs");
        }
        let token = LocalLibraryCancellation::default();
        let page = library
            .local_library_page(Some(1), "", None, None, &token)
            .expect("skip empty disk page");
        assert_eq!(page.files.len(), 1);
        assert_eq!(page.files[0].name, "oldest.txt");
        let cursor = page.next.expect("imports cursor");
        assert_eq!(
            library
                .local_library_page(Some(2), "", None, Some(cursor.clone()), &token)
                .expect_err("account changed")
                .kind(),
            ApplicationErrorKind::InvalidRequest
        );
        assert_eq!(
            library
                .local_library_page(Some(1), "different", None, Some(cursor), &token)
                .expect_err("filter changed")
                .kind(),
            ApplicationErrorKind::InvalidRequest
        );
        token.cancel();
        assert_eq!(
            library
                .local_library_page(Some(1), "", None, None, &token)
                .expect_err("cancelled scan")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
    }
}
