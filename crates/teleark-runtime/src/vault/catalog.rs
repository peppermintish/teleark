//! Bounded derived metadata keyed by the unified catalog's per-message version.
//! No keys or decrypted manifest payloads survive a scan.
use super::*;
use std::collections::BTreeMap;
use teleark_storage::CachedManifestCandidate;

const CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct ManifestCache {
    scope: Option<(i64, i64)>,
    entries: BTreeMap<i64, Entry>,
    receipts: std::collections::VecDeque<((i64, i64), u64, crate::transfer::RemoteByteObject)>,
}
struct Entry {
    source: CachedManifestCandidate,
    file: Option<ManagedVaultFile>,
}
impl Entry {
    fn bytes(&self) -> usize {
        let source = &self.source.file;
        256 + source.file_name.len()
            + source.caption.as_ref().map_or(0, String::len)
            + source.mime_type.as_ref().map_or(0, String::len)
            + self.file.as_ref().map_or(0, |file| {
                file.part_message_ids.len() * std::mem::size_of::<i64>()
                    + file.logical_name.len()
                    + file.package_id.len()
                    + file.relative_path.as_ref().map_or(0, String::len)
                    + file.mime_type.as_ref().map_or(0, String::len)
                    + file
                        .related_remote_names
                        .iter()
                        .map(String::len)
                        .sum::<usize>()
            })
    }
}
impl ManifestCache {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    pub(super) fn remember_receipt(
        &mut self,
        scope: (i64, i64),
        package: u64,
        object: crate::transfer::RemoteByteObject,
    ) {
        self.receipts
            .retain(|(s, p, _)| (*s, *p) != (scope, package));
        self.receipts.push_back((scope, package, object));
        while self.receipts.len() > 128 {
            self.receipts.pop_front();
        }
    }
    pub(super) fn locator_for(
        &self,
        scope: (i64, i64),
        package: PackageId,
    ) -> Option<crate::transfer::RemoteByteObject> {
        if let Some((_, _, object)) = self
            .receipts
            .iter()
            .find(|(s, p, _)| *s == scope && *p == package.get())
        {
            return Some(object.clone());
        }
        (self.scope == Some(scope))
            .then(|| {
                self.entries.values().find_map(|entry| {
                    entry
                        .file
                        .as_ref()
                        .filter(|file| file.package_numeric_id == package.get())
                        .and_then(|_| byte_object(&entry.source).ok())
                })
            })
            .flatten()
    }
    #[cfg(test)]
    pub(super) fn project(
        &mut self,
        scope: (i64, i64),
        candidates: Vec<CachedManifestCandidate>,
        cancellation: &crate::TelegramScanCancellation,
        read: impl FnMut(&CachedManifestCandidate) -> Result<Option<ManagedVaultFile>, ApplicationError>,
    ) -> Result<ManagedVaultScan, ApplicationError> {
        self.project_observed(scope, candidates, cancellation, read, None)
    }
    pub(super) fn project_observed(
        &mut self,
        scope: (i64, i64),
        candidates: Vec<CachedManifestCandidate>,
        cancellation: &crate::TelegramScanCancellation,
        mut read: impl FnMut(
            &CachedManifestCandidate,
        ) -> Result<Option<ManagedVaultFile>, ApplicationError>,
        observer: Option<&crate::ManagedScanObserver>,
    ) -> Result<ManagedVaultScan, ApplicationError> {
        if self.scope != Some(scope) {
            self.entries.clear();
            self.receipts.retain(|(s, _, _)| *s == scope);
            self.scope = Some(scope);
        }
        let retained: std::collections::BTreeSet<_> = candidates
            .iter()
            .take(MAX_MANIFEST_SCAN)
            .map(|c| c.file.message_id.get())
            .collect();
        self.entries.retain(|id, _| retained.contains(id));
        if let Some(observer) = observer {
            observer.total(candidates.len().min(MAX_MANIFEST_SCAN));
        }
        let mut report = ManagedVaultScan {
            catalog_limited: candidates.len() >= MAX_MANIFEST_SCAN,
            ..Default::default()
        };
        let mut bytes: usize = self.entries.values().map(Entry::bytes).sum();
        for candidate in candidates.into_iter().take(MAX_MANIFEST_SCAN) {
            if cancellation.is_cancelled() {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            let id = candidate.file.message_id.get();
            if self
                .entries
                .get(&id)
                .is_some_and(|entry| entry.source != candidate)
                && let Some(old) = self.entries.remove(&id)
            {
                bytes = bytes.saturating_sub(old.bytes());
            }
            let cached = self.entries.contains_key(&id);
            let result = match self.entries.get(&id) {
                Some(entry) => entry.file.clone(),
                None => {
                    // The loader distinguishes authenticated-content rejection from
                    // retryable transport/access failures using typed errors.
                    let result = read(&candidate)?;
                    let entry = Entry {
                        source: candidate,
                        file: result.clone(),
                    };
                    let cost = entry.bytes();
                    while bytes.saturating_add(cost) > CACHE_BYTES && !self.entries.is_empty() {
                        if let Some((_, old)) = self.entries.pop_first() {
                            bytes = bytes.saturating_sub(old.bytes());
                        }
                    }
                    if cost <= CACHE_BYTES {
                        bytes += cost;
                        self.entries.insert(id, entry);
                    }
                    result
                }
            };
            if let Some(observer) = observer {
                observer.advance(cached, result.is_none());
            }
            match result {
                Some(file) => report.files.push(file),
                None => report.rejected_manifests += 1,
            }
        }
        report.files.sort_by(|a, b| {
            b.created_at_unix_ms
                .cmp(&a.created_at_unix_ms)
                .then_with(|| a.logical_name.cmp(&b.logical_name))
        });
        Ok(report)
    }
}

pub(super) fn byte_object(
    candidate: &CachedManifestCandidate,
) -> Result<crate::transfer::RemoteByteObject, ApplicationError> {
    Ok(crate::transfer::RemoteByteObject {
        object_id: u64::try_from(candidate.file.message_id.get())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?,
        name: candidate.file.file_name.clone(),
        encoded_size: candidate.file.size_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(id: i64, revision: i64) -> CachedManifestCandidate {
        CachedManifestCandidate {
            revision,
            file: teleark_storage::CachedTelegramFileRecord {
                message_id: teleark_core::MessageId::new(id),
                file_name: format!("{id}.tarkm"),
                caption: Some(crate::transfer::MANIFEST_CAPTION.into()),
                mime_type: None,
                size_bytes: 80,
                sent_at_unix_ms: 1_000,
                modified_at_unix_ms: 1_000,
            },
        }
    }
    fn file(id: i64) -> ManagedVaultFile {
        ManagedVaultFile {
            vault_id: None,
            health: crate::VaultFileHealth::Unchecked,
            part_message_ids: Vec::new(),
            package_numeric_id: id as u64,
            package_id: id.to_string(),
            logical_name: format!("{id}.pdf"),
            relative_path: None,
            mime_type: None,
            media_kind: FileKind::Document,
            size_bytes: 42,
            encoded_size_bytes: 80,
            part_count: 1,
            created_at_unix_ms: 1_000,
            manifest_message_id: id,
            related_remote_names: vec![],
        }
    }
    #[test]
    fn only_changed_manifests_are_loaded_and_deleted_items_leave_the_projection() {
        let mut cache = ManifestCache::default();
        let cancel = crate::TelegramScanCancellation::new();
        let mut reads = vec![];
        let mut read = |candidate: &CachedManifestCandidate| {
            reads.push(candidate.file.message_id.get());
            Ok(Some(file(candidate.file.message_id.get())))
        };
        let first = cache
            .project(
                (1, 2),
                vec![candidate(10, 1), candidate(11, 1)],
                &cancel,
                &mut read,
            )
            .expect("initial authentication");
        let repeated = cache
            .project(
                (1, 2),
                vec![candidate(10, 1), candidate(11, 1)],
                &cancel,
                &mut read,
            )
            .expect("cached scan");
        assert_eq!(first, repeated);
        let edited = cache
            .project(
                (1, 2),
                vec![candidate(10, 2), candidate(11, 1), candidate(12, 2)],
                &cancel,
                &mut read,
            )
            .expect("edit and upload");
        assert_eq!(edited.files.len(), 3);
        let deleted = cache
            .project((1, 2), vec![candidate(11, 1)], &cancel, &mut read)
            .expect("deleted manifests");
        assert_eq!(deleted.files, vec![file(11)]);
        assert_eq!(reads, vec![10, 11, 10, 12]);
        assert!(cache.locator_for((1, 3), PackageId::new(11)).is_none());
        assert!(cache.locator_for((1, 2), PackageId::new(10)).is_none());
        assert_eq!(
            cache
                .locator_for((1, 2), PackageId::new(11))
                .expect("single manifest download")
                .object_id,
            11
        );
    }
    #[test]
    fn rejection_versions_account_switch_lock_and_transient_failures_do_not_poison_cache() {
        let mut cache = ManifestCache::default();
        let cancel = crate::TelegramScanCancellation::new();
        assert_eq!(
            cache
                .project((1, 2), vec![candidate(10, 1)], &cancel, |_| Ok(None))
                .expect("damaged manifest")
                .rejected_manifests,
            1
        );
        cache
            .project((1, 2), vec![candidate(10, 1)], &cancel, |_| {
                panic!("unchanged rejection should be cached")
            })
            .expect("cached rejection");
        assert!(
            cache
                .project((1, 2), vec![candidate(10, 2)], &cancel, |_| Err(
                    ApplicationError::new(ApplicationErrorKind::Network)
                ))
                .is_err()
        );
        assert_eq!(
            cache
                .project((1, 2), vec![candidate(10, 2)], &cancel, |_| Ok(Some(file(
                    10
                ))))
                .expect("network retry")
                .files
                .len(),
            1
        );
        let mut calls = 0;
        cache
            .project((9, 2), vec![candidate(10, 2)], &cancel, |_| {
                calls += 1;
                Ok(Some(file(10)))
            })
            .expect("new account");
        assert_eq!(calls, 1);
        cache.clear();
        cache
            .project((9, 2), vec![candidate(10, 2)], &cancel, |_| {
                calls += 1;
                Ok(Some(file(10)))
            })
            .expect("after unlock");
        assert_eq!(calls, 2);
        cancel.cancel();
        assert_eq!(
            cache
                .project((9, 2), vec![candidate(11, 1)], &cancel, |_| panic!(
                    "cancel before crypto"
                ))
                .expect_err("cancelled")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
    }
    #[test]
    fn cache_and_catalog_limits_are_explicit() {
        let mut cache = ManifestCache::default();
        let cancel = crate::TelegramScanCancellation::new();
        let candidates = (1..=1_001).map(|id| candidate(id, 1)).collect();
        let report = cache
            .project((1, 2), candidates, &cancel, |c| {
                Ok(Some(file(c.file.message_id.get())))
            })
            .expect("bounded catalog");
        assert_eq!(report.files.len(), 1_000);
        assert!(report.catalog_limited);
        assert!(cache.entries.values().map(Entry::bytes).sum::<usize>() <= CACHE_BYTES);
        let mut huge = file(10);
        huge.logical_name = "x".repeat(CACHE_BYTES);
        cache
            .project((1, 2), vec![candidate(10, 2)], &cancel, |_| {
                Ok(Some(huge.clone()))
            })
            .expect("one uncached oversize projection");
        assert!(cache.entries.is_empty());
    }
}
