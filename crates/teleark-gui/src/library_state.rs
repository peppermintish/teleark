use std::path::PathBuf;

use teleark_core::{
    ApplicationErrorKind, EncryptionState, FileKind, LibraryItem, LibraryPage, LibraryStatistics,
    LogicalFileId, PackageId, RemoteState, VerificationState,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LibraryView {
    #[default]
    Local,
    Remote,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LibraryRowId {
    Catalog(LogicalFileId),
    Local(teleark_runtime::LocalLibraryKey),
}

impl LibraryRowId {
    pub(crate) fn element_id(&self, prefix: &str) -> gpui_kit::SharedString {
        let (kind, id) = match self {
            Self::Catalog(id) => ("catalog", id.get()),
            Self::Local(teleark_runtime::LocalLibraryKey::Imported(id)) => ("import", id.get()),
            Self::Local(teleark_runtime::LocalLibraryKey::NativeDownload(id)) => ("native", *id),
            Self::Local(teleark_runtime::LocalLibraryKey::VaultDownload(id)) => ("vault", *id),
        };
        format!("{prefix}-{kind}-{id}").into()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LibraryPageCursor {
    Remote(String),
    Local(teleark_runtime::LocalLibraryCursor),
}

/// Presentation data derived from frontend-neutral Core and Runtime DTOs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LibraryRowView {
    pub(crate) id: LibraryRowId,
    pub(crate) name: String,
    pub(crate) size_bytes: u64,
    pub(crate) kind: FileKind,
    pub(crate) source_name: Option<String>,
    pub(crate) local_source_path: Option<PathBuf>,
    pub(crate) source_chat_id: Option<i64>,
    pub(crate) source_account_id: Option<i64>,
    pub(crate) source_message_id: Option<i64>,
    pub(crate) modified_at_unix_ms: Option<i64>,
    pub(crate) remote_state: RemoteState,
    pub(crate) encryption_state: EncryptionState,
    pub(crate) verification_state: VerificationState,
    pub(crate) package_id: Option<PackageId>,
    pub(crate) part_count: u32,
}

impl LibraryRowView {
    pub(crate) fn download_source(
        &self,
        account: Option<i64>,
    ) -> Result<(i64, i64, i64), &'static str> {
        if self.package_id.is_some() || self.encryption_state != EncryptionState::Unencrypted {
            return Err("library-action-managed-source");
        }
        let (Some(owner), Some(chat), Some(message)) = (
            self.source_account_id,
            self.source_chat_id,
            self.source_message_id,
        ) else {
            return Err("library-action-source-unavailable");
        };
        if account != Some(owner) {
            return Err("library-action-account-required");
        }
        Ok((owner, chat, message))
    }
}

impl From<LibraryItem> for LibraryRowView {
    fn from(item: LibraryItem) -> Self {
        let file = item.file;
        Self {
            id: LibraryRowId::Catalog(file.id),
            name: file.name,
            size_bytes: file.size_bytes,
            kind: file.kind,
            source_name: item.source_name,
            local_source_path: item.local_source_path,
            source_chat_id: file.source_chat_id.map(|id| id.get()),
            source_account_id: file.source_account_id.map(|id| id.get()),
            source_message_id: item.source_message_id.map(|id| id.get()),
            modified_at_unix_ms: file.modified_at_unix_ms,
            remote_state: file.remote_state,
            encryption_state: file.encryption_state,
            verification_state: file.verification_state,
            package_id: file.package_id,
            part_count: item.part_count,
        }
    }
}

impl From<teleark_runtime::LocalLibraryFile> for LibraryRowView {
    fn from(file: teleark_runtime::LocalLibraryFile) -> Self {
        Self {
            id: LibraryRowId::Local(file.key),
            name: file.name,
            size_bytes: file.size_bytes,
            kind: file.kind,
            source_name: None,
            local_source_path: Some(file.path),
            source_chat_id: file.chat_id,
            source_account_id: file.account_id,
            source_message_id: file.message_id,
            modified_at_unix_ms: file.modified_at_unix_ms,
            remote_state: RemoteState::LocalOnly,
            encryption_state: EncryptionState::Unencrypted,
            verification_state: VerificationState::Unverified,
            package_id: None,
            part_count: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LibrarySnapshot {
    pub(crate) rows: Vec<LibraryRowView>,
    pub(crate) total_matching: u64,
    pub(crate) next_cursor: Option<LibraryPageCursor>,
    pub(crate) statistics: LibraryStatistics,
}

impl LibrarySnapshot {
    pub(crate) fn from_core(page: LibraryPage, statistics: LibraryStatistics) -> Self {
        Self {
            rows: page.items.into_iter().map(LibraryRowView::from).collect(),
            total_matching: page.total_matching,
            next_cursor: page.next.map(LibraryPageCursor::Remote),
            statistics,
        }
    }

    pub(crate) fn from_local(page: teleark_runtime::LocalLibraryPage) -> Self {
        let mut paths = std::collections::BTreeSet::new();
        let rows = page
            .files
            .into_iter()
            .filter(|file| paths.insert(file.path.clone()))
            .map(LibraryRowView::from)
            .collect::<Vec<_>>();
        Self {
            total_matching: rows.len() as u64,
            statistics: LibraryStatistics::default(),
            rows,
            next_cursor: page.next.map(LibraryPageCursor::Local),
        }
    }

    pub(crate) fn append_snapshot(&mut self, page: Self) {
        for row in page.rows {
            if !self.rows.iter().any(|existing| {
                existing.id == row.id
                    || (matches!(&row.id, LibraryRowId::Local(_))
                        && row.local_source_path.is_some()
                        && existing.local_source_path == row.local_source_path)
            }) {
                self.rows.push(row);
            }
        }
        self.total_matching = if matches!(
            self.rows.first().map(|row| &row.id),
            Some(LibraryRowId::Local(_))
        ) {
            self.rows.len() as u64
        } else {
            page.total_matching
        };
        self.next_cursor = page.next_cursor;
        self.statistics = page.statistics;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LibraryContent {
    Loading,
    Ready(LibrarySnapshot),
    Empty(LibrarySnapshot),
    Failed(ApplicationErrorKind),
}

impl LibraryContent {
    pub(crate) fn from_snapshot(snapshot: LibrarySnapshot) -> Self {
        if snapshot.rows.is_empty() {
            Self::Empty(snapshot)
        } else {
            Self::Ready(snapshot)
        }
    }

    pub(crate) fn snapshot(&self) -> Option<&LibrarySnapshot> {
        match self {
            Self::Ready(snapshot) | Self::Empty(snapshot) => Some(snapshot),
            Self::Loading | Self::Failed(_) => None,
        }
    }

    pub(crate) fn rows(&self) -> &[LibraryRowView] {
        self.snapshot().map_or(&[], |snapshot| &snapshot.rows)
    }

    pub(crate) fn selected(&self, index: usize) -> Option<&LibraryRowView> {
        let rows = self.rows();
        rows.get(index.min(rows.len().saturating_sub(1)))
    }

    pub(crate) fn snapshot_mut(&mut self) -> Option<&mut LibrarySnapshot> {
        match self {
            Self::Ready(snapshot) | Self::Empty(snapshot) => Some(snapshot),
            Self::Loading | Self::Failed(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImportActivity {
    Idle,
    Picking,
    Importing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImportFeedback {
    Succeeded {
        imported: u64,
    },
    PartiallySucceeded {
        imported: u64,
        failed: u64,
    },
    Failed {
        failed: u64,
        reason: ApplicationErrorKind,
    },
    NoFilesSelected,
    PickerFailed,
}

impl ImportFeedback {
    pub(crate) fn from_results(
        results: &[Result<teleark_core::LogicalFile, teleark_core::ApplicationError>],
    ) -> Option<Self> {
        if results.is_empty() {
            return None;
        }

        let imported =
            u64::try_from(results.iter().filter(|result| result.is_ok()).count()).ok()?;
        let failed = u64::try_from(results.len()).ok()?.saturating_sub(imported);
        if failed == 0 {
            return Some(Self::Succeeded { imported });
        }
        if imported != 0 {
            return Some(Self::PartiallySucceeded { imported, failed });
        }

        let reason = results
            .iter()
            .find_map(|result| result.as_ref().err().map(|error| error.kind()))
            .unwrap_or(ApplicationErrorKind::Persistence);
        Some(Self::Failed { failed, reason })
    }
}

#[cfg(test)]
mod tests {
    use teleark_core::{
        ApplicationError, FileKind, LibraryItem, LibraryPage, LibraryStatistics, LogicalFile,
        LogicalFileId,
    };

    use super::*;

    fn item(id: u64, name: &str) -> LibraryItem {
        LibraryItem {
            file: LogicalFile::new(LogicalFileId::new(id), name, 42, FileKind::Document)
                .expect("valid file"),
            source_name: Some("Source name".to_owned()),
            source_message_id: Some(teleark_core::MessageId::new(77)),
            local_source_path: Some("/tmp/source.pdf".into()),
            part_count: 3,
        }
    }

    #[test]
    fn local_pages_deduplicate_paths_without_colliding_with_catalog_or_other_download_ids() {
        use teleark_runtime::{LocalLibraryFile, LocalLibraryKey, LocalLibraryPage};
        let file = |key, path: &str| LocalLibraryFile {
            key,
            path: path.into(),
            name: "file.txt".into(),
            size_bytes: 3,
            kind: FileKind::Document,
            modified_at_unix_ms: None,
            account_id: Some(1),
            chat_id: Some(100),
            message_id: Some(200),
        };
        let snapshot = LibrarySnapshot::from_local(LocalLibraryPage {
            files: vec![
                file(LocalLibraryKey::NativeDownload(1), "/fixture/native.txt"),
                file(LocalLibraryKey::VaultDownload(1), "/fixture/vault.txt"),
                file(
                    LocalLibraryKey::Imported(LogicalFileId::new(1)),
                    "/fixture/native.txt",
                ),
            ],
            next: None,
        });
        assert_eq!(snapshot.rows.len(), 2);
        assert_eq!(snapshot.total_matching, 2);
        let keys = snapshot
            .rows
            .iter()
            .map(|row| row.id.element_id("row"))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(keys.len(), 2);
        assert!(!keys.contains(&LibraryRowId::Catalog(LogicalFileId::new(1)).element_id("row")));
        assert!(
            snapshot
                .rows
                .iter()
                .all(|row| row.remote_state == RemoteState::LocalOnly
                    && row.verification_state == VerificationState::Unverified)
        );
    }

    #[test]
    fn indexed_download_keeps_source_identity_and_rejects_unsafe_sources() {
        let mut row = LibraryRowView::from(item(7, "résumé.pdf"));
        row.local_source_path = None;
        row.source_account_id = Some(42);
        row.source_chat_id = Some(-1007);
        row.part_count = 1;
        assert_eq!(row.download_source(Some(42)), Ok((42, -1007, 77)));
        assert_eq!(
            row.download_source(Some(43)),
            Err("library-action-account-required")
        );
        assert_eq!(
            row.download_source(None),
            Err("library-action-account-required")
        );
        row.source_message_id = None;
        assert_eq!(
            row.download_source(Some(42)),
            Err("library-action-source-unavailable")
        );
        row.source_message_id = Some(77);
        row.encryption_state = EncryptionState::Encrypted;
        assert_eq!(
            row.download_source(Some(42)),
            Err("library-action-managed-source")
        );
        row.encryption_state = EncryptionState::Unencrypted;
        row.package_id = Some(PackageId::new(1));
        assert_eq!(
            row.download_source(Some(42)),
            Err("library-action-managed-source")
        );
    }

    #[test]
    fn core_page_maps_to_stable_presentation_rows() {
        let snapshot = LibrarySnapshot::from_core(
            LibraryPage {
                items: vec![item(7, "résumé.pdf")],
                next: Some("opaque".to_owned()),
                total_matching: 99,
            },
            LibraryStatistics {
                logical_file_count: 101,
                logical_bytes: 4_242,
                local_file_count: 100,
                remote_file_count: 1,
                active_transfer_count: 0,
            },
        );

        assert_eq!(
            snapshot.rows[0].id,
            LibraryRowId::Catalog(LogicalFileId::new(7))
        );
        assert_eq!(snapshot.rows[0].name, "résumé.pdf");
        assert_eq!(snapshot.rows[0].source_name.as_deref(), Some("Source name"));
        assert_eq!(snapshot.rows[0].source_message_id, Some(77));
        assert_eq!(
            snapshot.rows[0].local_source_path.as_deref(),
            Some(std::path::Path::new("/tmp/source.pdf"))
        );
        assert_eq!(snapshot.rows[0].part_count, 3);
        assert_eq!(snapshot.total_matching, 99);
        assert_eq!(
            snapshot.next_cursor,
            Some(LibraryPageCursor::Remote("opaque".into()))
        );
        assert_eq!(snapshot.statistics.logical_file_count, 101);
    }

    #[test]
    fn empty_and_selected_states_are_deterministic() {
        let empty = LibraryContent::from_snapshot(LibrarySnapshot::from_core(
            LibraryPage::default(),
            LibraryStatistics::default(),
        ));
        assert!(matches!(empty, LibraryContent::Empty(_)));
        assert!(empty.selected(0).is_none());

        let ready = LibraryContent::from_snapshot(LibrarySnapshot::from_core(
            LibraryPage {
                items: vec![item(1, "one"), item(2, "two")],
                total_matching: 2,
                ..LibraryPage::default()
            },
            LibraryStatistics::default(),
        ));
        assert_eq!(
            ready.selected(999).map(|row| row.name.as_str()),
            Some("two")
        );
    }

    #[test]
    fn additional_pages_append_rows_and_replace_cursor_metadata() {
        let mut content = LibraryContent::from_snapshot(LibrarySnapshot::from_core(
            LibraryPage {
                items: vec![item(1, "one")],
                next: Some("page-two".to_owned()),
                total_matching: 3,
            },
            LibraryStatistics::default(),
        ));

        content
            .snapshot_mut()
            .expect("ready snapshot")
            .append_snapshot(LibrarySnapshot::from_core(
                LibraryPage {
                    items: vec![item(2, "two"), item(3, "three")],
                    next: None,
                    total_matching: 3,
                },
                LibraryStatistics::default(),
            ));

        let snapshot = content.snapshot().expect("appended snapshot");
        assert_eq!(snapshot.rows.len(), 3);
        assert_eq!(snapshot.rows[2].name, "three");
        assert_eq!(snapshot.next_cursor, None);
        assert_eq!(snapshot.total_matching, 3);
    }

    #[test]
    fn import_results_distinguish_success_partial_and_failure() {
        let ok = Ok(
            LogicalFile::new(LogicalFileId::new(1), "one.pdf", 1, FileKind::Document)
                .expect("valid file"),
        );
        let error = Err(ApplicationError::new(ApplicationErrorKind::SourceMissing));

        assert_eq!(
            ImportFeedback::from_results(std::slice::from_ref(&ok)),
            Some(ImportFeedback::Succeeded { imported: 1 })
        );
        assert_eq!(
            ImportFeedback::from_results(&[ok, error.clone()]),
            Some(ImportFeedback::PartiallySucceeded {
                imported: 1,
                failed: 1,
            })
        );
        assert_eq!(
            ImportFeedback::from_results(&[error]),
            Some(ImportFeedback::Failed {
                failed: 1,
                reason: ApplicationErrorKind::SourceMissing,
            })
        );
    }
}
