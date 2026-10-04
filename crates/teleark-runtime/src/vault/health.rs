//! File-local health and encrypted inventory. All work runs on the Vault owner.
use super::*;
use crate::{StorageRequest, VaultFileHealth};
use teleark_crypto::{ManifestLimits, manifest_vault_id_hint, open_manifest};
use teleark_storage::VaultInventoryRecord;

pub(super) fn open_with_keys(
    bytes: &[u8],
    active: Option<([u8; 16], &VaultMasterKey)>,
    historical: Option<&([u8; 16], Arc<VaultMasterKey>)>,
) -> Result<teleark_crypto::OpenedManifest, ApplicationError> {
    let id = manifest_vault_id_hint(bytes, ManifestLimits::default()).map_err(map_crypto_error)?;
    let key = active
        .filter(|(key_id, _)| *key_id == id)
        .map(|(_, key)| key)
        .or_else(|| {
            historical
                .filter(|(key_id, _)| *key_id == id)
                .map(|(_, key)| key.as_ref())
        })
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
    open_manifest(bytes, key, ManifestLimits::default()).map_err(map_crypto_error)
}

pub(super) fn load(
    store: &mut TelegramObjectStore,
    library: &DesktopLibrary,
    scope: (i64, i64),
    object: crate::RemoteByteObject,
    active: Option<([u8; 16], &VaultMasterKey)>,
    historical: Option<&([u8; 16], Arc<VaultMasterKey>)>,
    observer: &crate::ManagedScanObserver,
) -> Result<Option<ManagedVaultFile>, ApplicationError> {
    observer.phase(crate::ChannelSyncPhase::ManifestReceiving);
    let bytes = match store.download(object.object_id).map_err(map_transfer_error) {
        Ok(bytes) => bytes,
        Err(error)
            if matches!(
                error.kind(),
                ApplicationErrorKind::SourceMissing | ApplicationErrorKind::NotFound
            ) =>
        {
            // A document can disappear between the committed catalog read and
            // this fetch. Preserve its authenticated inventory and continue
            // projecting the other files instead of failing the entire scan.
            observer.phase(crate::ChannelSyncPhase::Persisting);
            library
                .worker
                .request("save_vault_message_health", |reply| {
                    StorageRequest::SaveVaultMessageHealth {
                        account: scope.0,
                        chat: scope.1,
                        messages: vec![(object.object_id as i64, false)],
                        observed_at: now_unix_ms().unwrap_or(0),
                        reply,
                    }
                })?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    observer.phase(crate::ChannelSyncPhase::ManifestVerifying);
    if bytes.len() as u64 != object.encoded_size {
        mark_invalid(library, scope, object.object_id as i64, true)?;
        return Ok(None);
    }
    let manifest = match open_with_keys(&bytes, active, historical) {
        Ok(manifest) => manifest,
        Err(error) if error.kind() == ApplicationErrorKind::VaultKeyUnavailable => {
            let mut file = unavailable(&object);
            file.vault_id = manifest_vault_id_hint(&bytes, ManifestLimits::default()).ok();
            return Ok(Some(file));
        }
        Err(_) => {
            mark_invalid(library, scope, object.object_id as i64, true)?;
            return Ok(None);
        }
    };
    if object.name != teleark_crypto::remote_manifest_name(&manifest.public_header.package_id)
        || manifest
            .metadata
            .parts
            .iter()
            .any(|part| (part.remote_locator.account_id, part.remote_locator.chat_id) != scope)
    {
        mark_invalid(library, scope, object.object_id as i64, true)?;
        return Ok(None);
    }
    let record = VaultInventoryRecord {
        account_id: scope.0,
        chat_id: scope.1,
        manifest_message_id: i64::try_from(object.object_id)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?,
        remote_name: object.name.clone(),
        vault_id: manifest.public_header.vault_id,
        sealed_manifest: bytes,
        observed_at_unix_ms: now_unix_ms()?,
        manifest_invalid: false,
    };
    observer.phase(crate::ChannelSyncPhase::Persisting);
    library.worker.request("save_vault_inventory", |reply| {
        StorageRequest::SaveVaultInventory { record, reply }
    })?;
    mark_invalid(library, scope, object.object_id as i64, false)?;
    let mut file = managed_file_from_recovered(&crate::RecoveredManifest { object, manifest })?;
    file.health = check(
        library,
        scope,
        file.manifest_message_id,
        &file.part_message_ids,
    )?;
    Ok(Some(file))
}

fn unavailable(object: &crate::RemoteByteObject) -> ManagedVaultFile {
    ManagedVaultFile {
        vault_id: None,
        health: VaultFileHealth::KeyUnavailable,
        part_message_ids: Vec::new(),
        package_numeric_id: object.object_id,
        package_id: String::new(),
        logical_name: object.name.clone(),
        relative_path: None,
        mime_type: None,
        media_kind: FileKind::Other,
        size_bytes: 0,
        encoded_size_bytes: object.encoded_size,
        part_count: 0,
        created_at_unix_ms: 0,
        manifest_message_id: object.object_id as i64,
        related_remote_names: vec![object.name.clone()],
    }
}

pub(super) fn check(
    library: &DesktopLibrary,
    scope: (i64, i64),
    manifest: i64,
    parts: &[i64],
) -> Result<VaultFileHealth, ApplicationError> {
    let mut unknown = false;
    let mut missing = false;
    let ids = std::iter::once(manifest)
        .chain(parts.iter().copied())
        .collect::<Vec<_>>();
    for (batch, chunk) in ids.chunks(256).enumerate() {
        let results = library.worker.request("vault_message_health", |reply| {
            StorageRequest::VaultMessageHealth {
                account: scope.0,
                chat: scope.1,
                messages: chunk.to_vec(),
                reply,
            }
        })?;
        for (index, health) in results.into_iter().enumerate() {
            if health == VaultFileHealth::MissingParts {
                if batch == 0 && index == 0 {
                    return Ok(VaultFileHealth::MissingManifest);
                }
                missing = true;
            }
            unknown |= health == VaultFileHealth::Unchecked;
        }
    }
    Ok(if missing {
        VaultFileHealth::MissingParts
    } else if unknown {
        VaultFileHealth::Unchecked
    } else {
        VaultFileHealth::Present
    })
}

pub(super) fn retain_missing(
    report: &mut ManagedVaultScan,
    library: &DesktopLibrary,
    scope: (i64, i64),
    active: Option<([u8; 16], &VaultMasterKey)>,
    historical: Option<&([u8; 16], Arc<VaultMasterKey>)>,
    cancellation: &crate::TelegramScanCancellation,
) -> Result<(), ApplicationError> {
    let known: std::collections::BTreeSet<_> =
        report.files.iter().map(|f| f.manifest_message_id).collect();
    let mut bytes: usize = report
        .files
        .iter()
        .map(ManagedVaultFile::estimated_bytes)
        .sum();
    let mut before = i64::MAX;
    // One envelope per storage reply bounds plaintext/ciphertext working memory.
    for _ in 0..MAX_MANIFEST_SCAN {
        if cancellation.is_cancelled() {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        let records = library.worker.request("vault_inventory_page", |reply| {
            StorageRequest::VaultInventoryPage {
                account: scope.0,
                chat: scope.1,
                before,
                reply,
            }
        })?;
        let Some(record) = records.into_iter().next() else {
            return Ok(());
        };
        before = record.manifest_message_id;
        if known.contains(&before) {
            continue;
        }
        if report.files.len() >= MAX_MANIFEST_SCAN {
            report.catalog_limited = true;
            return Ok(());
        }
        let object = crate::RemoteByteObject {
            object_id: before as u64,
            name: record.remote_name,
            encoded_size: record.sealed_manifest.len() as u64,
        };
        let mut file = match open_with_keys(&record.sealed_manifest, active, historical) {
            Ok(manifest) => {
                managed_file_from_recovered(&crate::RecoveredManifest { object, manifest })?
            }
            Err(error) if error.kind() == ApplicationErrorKind::VaultKeyUnavailable => {
                unavailable(&object)
            }
            Err(_) => continue,
        };
        if record.manifest_invalid && file.health != VaultFileHealth::KeyUnavailable {
            file.health = VaultFileHealth::InvalidManifest;
        }
        if !matches!(
            file.health,
            VaultFileHealth::KeyUnavailable | VaultFileHealth::InvalidManifest
        ) {
            file.health = check(library, scope, before, &file.part_message_ids)?;
        }
        let cost = file.estimated_bytes();
        if bytes.saturating_add(cost) > 16 * 1024 * 1024 {
            report.catalog_limited = true;
            continue;
        }
        bytes += cost;
        report.files.push(file);
    }
    report.catalog_limited = true;
    Ok(())
}

pub(super) fn recover_target(
    store: &mut TelegramObjectStore,
    scope: (i64, i64),
    package: PackageId,
    expected: Option<(i64, [u8; 32])>,
    active: Option<([u8; 16], &VaultMasterKey)>,
    historical: Option<&([u8; 16], Arc<VaultMasterKey>)>,
) -> Result<(crate::RecoveredManifest, [u8; 32]), ApplicationError> {
    let name = teleark_crypto::remote_manifest_name(&package_bytes(package.get()));
    let (object, bytes) = if let Some((message_id, digest)) = expected {
        let id = u64::try_from(message_id)
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        let bytes = store.download(id).map_err(map_transfer_error)?;
        if blake3::hash(&bytes).as_bytes() != &digest {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
        }
        (
            crate::RemoteByteObject {
                object_id: id,
                name: name.clone(),
                encoded_size: bytes.len() as u64,
            },
            bytes,
        )
    } else {
        let object = store
            .search_exact_caption(crate::transfer::MANIFEST_CAPTION, MAX_MANIFEST_SCAN)
            .map_err(map_transfer_error)?
            .into_iter()
            .find(|object| object.name == name)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        let bytes = store
            .download(object.object_id)
            .map_err(map_transfer_error)?;
        if bytes.len() as u64 != object.encoded_size {
            return Err(ApplicationError::new(ApplicationErrorKind::SourceChanged));
        }
        (object, bytes)
    };
    let manifest = open_with_keys(&bytes, active, historical)?;
    if teleark_crypto::remote_manifest_name(&manifest.public_header.package_id) != name
        || manifest
            .metadata
            .parts
            .iter()
            .any(|part| (part.remote_locator.account_id, part.remote_locator.chat_id) != scope)
    {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok((
        crate::RecoveredManifest { object, manifest },
        *blake3::hash(&bytes).as_bytes(),
    ))
}

fn aggregate_presence(manifest: bool, missing: bool, unknown: bool) -> VaultFileHealth {
    if !manifest {
        VaultFileHealth::MissingManifest
    } else if missing {
        VaultFileHealth::MissingParts
    } else if unknown {
        VaultFileHealth::Unchecked
    } else {
        VaultFileHealth::Present
    }
}

pub(super) fn verify(
    report: &mut ManagedVaultScan,
    telegram: &DesktopTelegram,
    library: &DesktopLibrary,
    scope: (i64, i64),
    cancellation: &crate::TelegramScanCancellation,
    observer: &crate::ManagedScanObserver,
) -> Result<(), ApplicationError> {
    verify_with(report, library, scope, cancellation, observer, |ids| {
        let page = telegram
            .sync_channel(
                scope.0,
                scope.1,
                crate::channel_sync::ChannelRead::Verify(ids.to_vec()),
                cancellation.clone(),
                true,
            )
            .map_err(|error| ApplicationError::new(error.kind))?;
        let present: std::collections::BTreeSet<_> =
            page.files.iter().map(|file| file.message_id).collect();
        Ok(ids.iter().map(|id| present.contains(id)).collect())
    })
}

fn verify_with(
    report: &mut ManagedVaultScan,
    library: &DesktopLibrary,
    scope: (i64, i64),
    cancellation: &crate::TelegramScanCancellation,
    observer: &crate::ManagedScanObserver,
    mut probe: impl FnMut(&[i64]) -> Result<Vec<bool>, ApplicationError>,
) -> Result<(), ApplicationError> {
    observer.phase(crate::ChannelSyncPhase::Verifying);
    for file in &mut report.files {
        if cancellation.is_cancelled() {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        if matches!(
            file.health,
            VaultFileHealth::KeyUnavailable | VaultFileHealth::InvalidManifest
        ) {
            continue;
        }
        let ids = std::iter::once(file.manifest_message_id)
            .chain(file.part_message_ids.iter().copied())
            .collect::<Vec<_>>();
        let mut missing_manifest = false;
        let mut missing_part = false;
        for chunk in ids.chunks(100) {
            observer.phase(crate::ChannelSyncPhase::Verifying);
            let exists = probe(chunk)?;
            if exists.len() != chunk.len() {
                return Err(ApplicationError::new(ApplicationErrorKind::Network));
            }
            if cancellation.is_cancelled() {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            let observations: Vec<_> = chunk.iter().copied().zip(exists).collect();
            for (id, exists) in &observations {
                if !exists {
                    if *id == file.manifest_message_id {
                        missing_manifest = true;
                    } else {
                        missing_part = true;
                    }
                }
            }
            observer.phase(crate::ChannelSyncPhase::Persisting);
            library
                .worker
                .request("save_vault_message_health", |reply| {
                    StorageRequest::SaveVaultMessageHealth {
                        account: scope.0,
                        chat: scope.1,
                        messages: observations,
                        observed_at: now_unix_ms().unwrap_or(0),
                        reply,
                    }
                })?;
        }
        file.health = aggregate_presence(!missing_manifest, missing_part, false);
    }
    Ok(())
}

pub(super) fn full_check(
    telegram: &DesktopTelegram,
    library: &DesktopLibrary,
    scope: (i64, i64),
    active: Option<([u8; 16], &VaultMasterKey)>,
    historical: Option<&([u8; 16], Arc<VaultMasterKey>)>,
    cancellation: &crate::TelegramScanCancellation,
    observer: &crate::ManagedScanObserver,
) -> Result<ManagedVaultScan, ApplicationError> {
    let run = (random_transfer_id()? & i64::MAX as u64).max(1) as i64;
    let mut check_run = CheckRun {
        telegram,
        library,
        scope,
        cancellation,
        observer,
        run,
        projected_bytes: 0,
    };
    let mut report = ManagedVaultScan {
        health_checked_files: Some(0),
        ..Default::default()
    };
    let mut store = TelegramObjectStore::new(telegram.clone(), scope.0, scope.1)
        .with_cancellation(cancellation.clone());
    let mut before = None;
    loop {
        if cancellation.is_cancelled() {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        observer.phase(crate::ChannelSyncPhase::ManifestReading);
        let page = telegram.scan_file_page_cancellable(
            scope.0,
            scope.1,
            before,
            100,
            cancellation.clone(),
        )?;
        for source in page
            .files
            .into_iter()
            .filter(|file| file.caption == crate::transfer::MANIFEST_CAPTION)
        {
            let object = crate::RemoteByteObject {
                object_id: source.message_id as u64,
                name: source.file_name,
                encoded_size: source.size_bytes,
            };
            if let Some(file) = load(
                &mut store, library, scope, object, active, historical, observer,
            )? {
                check_run.process(file, &mut report)?;
            } else {
                report.rejected_manifests = report.rejected_manifests.saturating_add(1);
                observer.advance(false, true);
            }
        }
        if page.exhausted {
            break;
        }
        let next = page
            .next_before_message_id
            .filter(|id| *id > 0 && before.is_none_or(|previous| *id < previous))
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Network))?;
        before = Some(next);
    }
    // A remote scan cannot enumerate deleted messages. Walk preserved encrypted
    // envelopes by keyset, skipping records already completed by this run.
    let mut before = i64::MAX;
    loop {
        if cancellation.is_cancelled() {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        observer.phase(crate::ChannelSyncPhase::ReadingLocal);
        let Some(record) = library
            .worker
            .request("unchecked_vault_inventory", |reply| {
                StorageRequest::UncheckedVaultInventory {
                    account: scope.0,
                    chat: scope.1,
                    before,
                    run,
                    reply,
                }
            })?
        else {
            break;
        };
        before = record.manifest_message_id;
        let object = crate::RemoteByteObject {
            object_id: before as u64,
            name: record.remote_name,
            encoded_size: record.sealed_manifest.len() as u64,
        };
        let mut file = match open_with_keys(&record.sealed_manifest, active, historical) {
            Ok(manifest) => {
                managed_file_from_recovered(&crate::RecoveredManifest { object, manifest })?
            }
            Err(error) if error.kind() == ApplicationErrorKind::VaultKeyUnavailable => {
                let mut file = unavailable(&object);
                file.vault_id = Some(record.vault_id);
                file
            }
            Err(_) => {
                report.rejected_manifests = report.rejected_manifests.saturating_add(1);
                continue;
            }
        };
        if record.manifest_invalid && file.health != VaultFileHealth::KeyUnavailable {
            file.health = VaultFileHealth::InvalidManifest;
        }
        check_run.process(file, &mut report)?;
    }
    Ok(report)
}

struct CheckRun<'a> {
    telegram: &'a DesktopTelegram,
    library: &'a DesktopLibrary,
    scope: (i64, i64),
    cancellation: &'a crate::TelegramScanCancellation,
    observer: &'a crate::ManagedScanObserver,
    run: i64,
    projected_bytes: usize,
}
impl CheckRun<'_> {
    fn process(
        &mut self,
        file: ManagedVaultFile,
        report: &mut ManagedVaultScan,
    ) -> Result<(), ApplicationError> {
        let manifest = file.manifest_message_id;
        let can_check = !matches!(
            file.health,
            VaultFileHealth::KeyUnavailable | VaultFileHealth::InvalidManifest
        );
        let mut single = ManagedVaultScan {
            files: vec![file],
            ..Default::default()
        };
        verify(
            &mut single,
            self.telegram,
            self.library,
            self.scope,
            self.cancellation,
            self.observer,
        )?;
        self.library
            .worker
            .request("mark_vault_health_scan", |reply| {
                StorageRequest::MarkVaultHealthScan {
                    account: self.scope.0,
                    chat: self.scope.1,
                    manifest,
                    run: self.run,
                    reply,
                }
            })?;
        if can_check {
            *report.health_checked_files.get_or_insert(0) += 1;
        }
        self.observer.advance(false, false);
        let cost: usize = single
            .files
            .iter()
            .map(ManagedVaultFile::estimated_bytes)
            .sum();
        if report.files.len() < MAX_MANIFEST_SCAN
            && self.projected_bytes.saturating_add(cost) <= 16 * 1024 * 1024
        {
            self.projected_bytes += cost;
            report.files.extend(single.files);
        } else {
            report.catalog_limited = true;
        }
        Ok(())
    }
}

pub(super) struct HealthWorker {
    cancellation: crate::TelegramScanCancellation,
    join: Option<JoinHandle<()>>,
}
impl HealthWorker {
    pub(super) fn spawn(
        cancellation: crate::TelegramScanCancellation,
        run: impl FnOnce() + Send + 'static,
    ) -> Result<Self, ApplicationError> {
        let join = thread::Builder::new()
            .name("teleark-file-health".into())
            .spawn(run)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        Ok(Self {
            cancellation,
            join: Some(join),
        })
    }
    pub(super) fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub(super) fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }
}
impl Drop for HealthWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(join) = self.join.take() {
            // Retain the thread until cancellation releases its network request;
            // neither the GUI nor the Vault owner waits for this join.
            let _ = thread::Builder::new()
                .name("teleark-health-reaper".into())
                .spawn(move || {
                    let _ = join.join();
                });
        }
    }
}

fn mark_invalid(
    library: &DesktopLibrary,
    scope: (i64, i64),
    manifest: i64,
    invalid: bool,
) -> Result<(), ApplicationError> {
    library
        .worker
        .request("set_vault_manifest_invalid", |reply| {
            StorageRequest::SetVaultManifestInvalid {
                account: scope.0,
                chat: scope.1,
                manifest,
                invalid,
                reply,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(manifest: i64, parts: Vec<i64>) -> ManagedVaultFile {
        ManagedVaultFile {
            vault_id: Some([1; 16]),
            health: VaultFileHealth::Unchecked,
            part_message_ids: parts,
            package_numeric_id: manifest as u64,
            package_id: manifest.to_string(),
            logical_name: "synthetic.bin".into(),
            relative_path: None,
            mime_type: None,
            media_kind: FileKind::Other,
            size_bytes: 100,
            encoded_size_bytes: 200,
            part_count: 2,
            created_at_unix_ms: 1,
            manifest_message_id: manifest,
            related_remote_names: Vec::new(),
        }
    }
    #[test]
    fn missing_part_and_manifest_are_isolated_and_checks_use_bounded_batches()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("health.db"))?;
        let cancellation = crate::TelegramScanCancellation::new();
        let observer = crate::ManagedScanObserver::silent(2);
        let mut report = ManagedVaultScan {
            files: vec![
                file(10, (11..=250).collect()),
                file(300, vec![301, 302]),
                file(400, vec![401]),
            ],
            ..Default::default()
        };
        let mut batches = Vec::new();
        verify_with(
            &mut report,
            &library,
            (1, 2),
            &cancellation,
            &observer,
            |ids| {
                batches.push(ids.len());
                Ok(ids.iter().map(|id| *id != 101 && *id != 400).collect())
            },
        )?;
        assert_eq!(batches, vec![100, 100, 41, 3, 2]);
        assert_eq!(
            report
                .files
                .iter()
                .map(|file| file.health)
                .collect::<Vec<_>>(),
            vec![
                VaultFileHealth::MissingParts,
                VaultFileHealth::Present,
                VaultFileHealth::MissingManifest
            ]
        );
        assert_eq!(
            check(&library, (1, 2), 10, &[101])?,
            VaultFileHealth::MissingParts
        );
        assert_eq!(
            check(&library, (9, 2), 10, &[101])?,
            VaultFileHealth::Unchecked
        );
        Ok(())
    }
    #[test]
    fn failed_or_cancelled_probe_preserves_completed_checks_without_inventing_missing_parts()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("health.db"))?;
        let cancellation = crate::TelegramScanCancellation::new();
        let observer = crate::ManagedScanObserver::silent(2);
        let mut report = ManagedVaultScan {
            files: vec![file(10, vec![11]), file(20, vec![21])],
            ..Default::default()
        };
        let error = verify_with(
            &mut report,
            &library,
            (1, 2),
            &cancellation,
            &observer,
            |ids| {
                if ids[0] == 20 {
                    return Err(ApplicationError::new(ApplicationErrorKind::Network));
                }
                Ok(vec![true; ids.len()])
            },
        )
        .expect_err("network failure");
        assert_eq!(error.kind(), ApplicationErrorKind::Network);
        assert_eq!(report.files[0].health, VaultFileHealth::Present);
        assert_eq!(report.files[1].health, VaultFileHealth::Unchecked);
        let mut cancelled = ManagedVaultScan {
            files: vec![file(30, vec![31])],
            ..Default::default()
        };
        assert_eq!(
            verify_with(
                &mut cancelled,
                &library,
                (1, 2),
                &cancellation,
                &observer,
                |ids| {
                    cancellation.cancel();
                    Ok(vec![false; ids.len()])
                }
            )
            .expect_err("cancelled")
            .kind(),
            ApplicationErrorKind::Cancelled
        );
        assert_eq!(
            check(&library, (1, 2), 30, &[31])?,
            VaultFileHealth::Unchecked
        );
        Ok(())
    }
    #[test]
    fn key_routing_uses_only_the_matching_epoch_and_still_authenticates_the_envelope() {
        let fixture =
            include_str!("../../../teleark-crypto/tests/vectors/manifest_v1/unicode_multipart.txt");
        let hex = fixture
            .lines()
            .find_map(|line| line.strip_prefix("encoded_hex="))
            .expect("fixture");
        let bytes: Vec<_> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("fixture hex"))
            .collect();
        let active = VaultMasterKey::from_bytes([0x55; 32]);
        let old = ([0x44; 16], Arc::new(VaultMasterKey::from_bytes([0x11; 32])));
        assert!(open_with_keys(&bytes, Some(([0x55; 16], &active)), Some(&old)).is_ok());
        assert_eq!(
            open_with_keys(&bytes, Some(([0x55; 16], &active)), None)
                .expect_err("old key unavailable")
                .kind(),
            ApplicationErrorKind::VaultKeyUnavailable
        );
        assert!(open_with_keys(&bytes, Some(([0x44; 16], &active)), None).is_err());
        let mut damaged = bytes;
        *damaged.last_mut().expect("tag") ^= 1;
        assert!(open_with_keys(&damaged, None, Some(&old)).is_err());
    }
}
