//! Passwordless desktop access. Only the retained key owner touches the OS store.
//! Store the authenticated v1 recovery bundle, never a PIN-derived secret.
use super::*;

pub(super) trait DeviceKeyStore: Send + Sync {
    fn read(&self, identity: &str) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError>;
    fn write(&self, identity: &str, bundle: &[u8]) -> Result<(), ApplicationError>;
}

// The record's random ID isolates epochs. The authenticated wrap digest also
// isolates concurrent rotations with the same generation before SQLite CAS.
pub(super) fn identity(record: &VaultMetadataRecord) -> String {
    format!(
        "{}/{}/{}",
        hex_id(&record.vault_id),
        record.recovery_generation,
        blake3::hash(&record.recovery_wrap).to_hex()
    )
}

pub(super) fn validate_stored_bundle(identity: &str, bytes: &[u8]) -> Result<(), ApplicationError> {
    let invalid = || ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable);
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let (key, wrap) = decode_recovery_bundle(text)?;
    let expected = format!(
        "{}/{}/{}",
        hex_id(&wrap.vault_id),
        wrap.recovery_generation,
        blake3::hash(&wrap.encode()).to_hex()
    );
    if expected != identity {
        return Err(invalid());
    }
    let _master = unwrap_master_key_with_recovery(&wrap, &key).map_err(|_| invalid())?;
    Ok(())
}

struct LibraryStore(DesktopLibrary);

impl DeviceKeyStore for LibraryStore {
    fn read(&self, identity: &str) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        self.0
            .credential_read(crate::credential_store::VAULT_NAMESPACE, identity)
    }
    fn write(&self, identity: &str, bundle: &[u8]) -> Result<(), ApplicationError> {
        self.0
            .credential_write(crate::credential_store::VAULT_NAMESPACE, identity, bundle)
    }
}

pub(super) fn library_store(library: DesktopLibrary) -> Arc<dyn DeviceKeyStore> {
    Arc::new(LibraryStore(library))
}

#[cfg(test)]
pub(super) fn platform_store() -> Arc<dyn DeviceKeyStore> {
    Arc::new(MemoryStore::default())
}

#[cfg(test)]
#[derive(Default)]
struct MemoryStore(Mutex<std::collections::BTreeMap<String, Zeroizing<Vec<u8>>>>);

#[cfg(test)]
impl DeviceKeyStore for MemoryStore {
    fn read(&self, identity: &str) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        Ok(self.0.lock().expect("test store").get(identity).cloned())
    }
    fn write(&self, identity: &str, bundle: &[u8]) -> Result<(), ApplicationError> {
        self.0
            .lock()
            .expect("test store")
            .insert(identity.into(), Zeroizing::new(bundle.to_vec()));
        Ok(())
    }
}

impl VaultOwner {
    pub(super) fn check_key_generation(&self) -> Result<(), ApplicationError> {
        self.session
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .current_keys(self.session_generation)
            .map(|_| ())
    }

    pub(super) fn save_device_key(
        &self,
        record: &VaultMetadataRecord,
        bundle: &str,
    ) -> Result<(), ApplicationError> {
        // Authenticate before persisting, including the exact record binding.
        let _ = authenticate_bundle(record, bundle)?;
        let id = identity(record);
        let existing = self.device_keys.read(&id)?;
        if existing.as_deref().map(Vec::as_slice) != Some(bundle.as_bytes()) {
            if existing.as_ref().is_some_and(|bytes| {
                std::str::from_utf8(bytes)
                    .ok()
                    .is_some_and(|text| authenticate_bundle(record, text).is_ok())
            }) {
                return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
            }
            self.device_keys.write(&id, bundle.as_bytes())?;
        }
        let verified = self.device_keys.read(&id)?;
        if verified.as_deref().map(Vec::as_slice) != Some(bundle.as_bytes()) {
            return Err(ApplicationError::new(ApplicationErrorKind::Persistence));
        }
        Ok(())
    }

    fn read_device_key(
        &self,
        record: &VaultMetadataRecord,
    ) -> Result<Zeroizing<String>, ApplicationError> {
        let bytes = self
            .device_keys
            .read(&identity(record))?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
        let _ = authenticate_bundle(record, text)?;
        Ok(Zeroizing::new(text.to_owned()))
    }

    fn prepare_managed_key(&mut self, progress: &VaultKeyProgress) -> Result<(), ApplicationError> {
        if self.master_key.is_some() {
            return Ok(());
        }
        if let Some(record) = &self.record {
            progress.phase(VaultKeyPhase::Loading)?;
            let bundle = self.read_device_key(record)?;
            progress.check_cancelled()?;
            self.check_key_generation()?;
            self.unlock_recovery(&bundle)
        } else {
            // Preserve the v1 record codec with a discarded random wrapping
            // password. Neither file keys nor recovery keys depend on this value.
            let password = random_wrapping_password()?;
            let _bundle =
                Zeroizing::new(self.create_key_epoch_persisted(&password, progress, true)?);
            Ok(())
        }
    }

    fn import_unknown_epoch(
        &self,
        bundle: &str,
        progress: &VaultKeyProgress,
    ) -> Result<VaultMetadataRecord, ApplicationError> {
        let (recovery_key, recovery_wrap) = decode_recovery_bundle(bundle)?;
        let master = unwrap_master_key_with_recovery(&recovery_wrap, &recovery_key)
            .map_err(map_crypto_error)?;
        let password_text = random_wrapping_password()?;
        let password =
            Password::new(password_text.as_bytes().to_vec()).map_err(map_crypto_error)?;
        progress.phase(VaultKeyPhase::WrappingPassword)?;
        let password_wrap = wrap_master_key_with_password(
            &master,
            &password,
            recovery_wrap.vault_id,
            1,
            &mut OsRandom,
            &mut AeadUsageRegistry::new(),
        )
        .map_err(map_crypto_error)?;
        let now = now_unix_ms()?;
        let record = VaultMetadataRecord {
            vault_id: recovery_wrap.vault_id,
            password_wrap: password_wrap.encode().map_err(map_crypto_error)?,
            recovery_wrap: recovery_wrap.encode(),
            password_generation: 1,
            recovery_generation: recovery_wrap.recovery_generation,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        progress.phase(VaultKeyPhase::Securing)?;
        self.save_device_key(&record, bundle)?;
        self.check_key_generation()?;
        progress.phase(VaultKeyPhase::Saving)?;
        self.library
            .worker
            .request("import_vault_key_epoch", |reply| {
                crate::StorageRequest::ImportVaultKeyEpoch {
                    record: record.clone(),
                    reply,
                }
            })?;
        Ok(record)
    }

    pub(super) fn manage_key(
        &mut self,
        action: ManagedKeyAction,
        progress: &VaultKeyProgress,
    ) -> Result<String, ApplicationError> {
        // Multiple admitted operations may precede the first load. Read the
        // current lease only on this serialized owner, after validating logout.
        let keys = self
            .session
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .current_keys(self.session_generation)?;
        self.record = self.library.worker.vault_metadata()?;
        if let Some(record) = &self.record {
            validate_record(record)?;
        }
        self.master_key = if self.record.as_ref().map(|r| r.vault_id)
            == keys.record.as_ref().map(|r| r.vault_id)
        {
            keys.active
        } else {
            None
        };
        self.historical_key = keys.historical;
        if self.master_key.is_some() && self.record != keys.record {
            self.refresh_status();
        }
        progress.phase(VaultKeyPhase::Loading)?;
        match action {
            ManagedKeyAction::Prepare => {
                self.prepare_managed_key(progress)?;
                Ok(String::new())
            }
            ManagedKeyAction::NewEpoch => {
                let password = random_wrapping_password()?;
                self.create_key_epoch_persisted(&password, progress, true)
            }
            ManagedKeyAction::Import(bundle) => {
                if self.record.is_none() {
                    progress.phase(VaultKeyPhase::Securing)?;
                    let password = random_wrapping_password()?;
                    self.restore_recovery_persisted(&bundle, &password, Some(progress))?;
                } else {
                    let (_, wrap) = decode_recovery_bundle(&bundle)?;
                    let record = match self.library.worker.vault_key_epoch(wrap.vault_id)? {
                        Some(record) => record,
                        None => self.import_unknown_epoch(&bundle, progress)?,
                    };
                    progress.phase(VaultKeyPhase::Securing)?;
                    self.save_device_key(&record, &bundle)?;
                    self.check_key_generation()?;
                    progress.phase(VaultKeyPhase::Saving)?;
                    self.unlock_recovery(&bundle)?;
                }
                Ok(String::new())
            }
            ManagedKeyAction::Export => {
                self.prepare_managed_key(progress)?;
                let record = self
                    .record
                    .as_ref()
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
                let bundle = self.read_device_key(record)?;
                progress.check_cancelled()?;
                self.check_key_generation()?;
                Ok(bundle.to_string())
            }
            ManagedKeyAction::RotateRecovery => {
                self.prepare_managed_key(progress)?;
                progress.phase(VaultKeyPhase::Securing)?;
                self.rotate_recovery_persisted(Some(progress))
            }
        }
    }
}

fn authenticate_bundle(
    record: &VaultMetadataRecord,
    bundle: &str,
) -> Result<VaultMasterKey, ApplicationError> {
    validate_record(record)?;
    let (key, wrap) = decode_recovery_bundle(bundle)?;
    if wrap.encode() != record.recovery_wrap {
        return Err(ApplicationError::new(
            ApplicationErrorKind::VaultKeyUnavailable,
        ));
    }
    unwrap_master_key_with_recovery(&wrap, &key)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))
}

fn random_wrapping_password() -> Result<Zeroizing<String>, ApplicationError> {
    // Same CSPRNG as file and recovery keys; no user input or application PIN.
    let mut bytes = Zeroizing::new([0; 32]);
    OsRandom
        .fill_bytes(bytes.as_mut())
        .map_err(map_crypto_error)?;
    Ok(Zeroizing::new(
        bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn open(
        path: &Path,
        store: Arc<dyn DeviceKeyStore>,
    ) -> Result<(DesktopVault, DesktopLibrary), ApplicationError> {
        let library = DesktopLibrary::open(path.join("catalog.sqlite"))?;
        let telegram = DesktopTelegram::open_direct(path.join("synthetic.session"))?;
        let vault = DesktopVault::with_device_keys(telegram, library.clone(), store)?;
        Ok((vault, library))
    }

    fn prepare(vault: &DesktopVault) -> Result<(), ApplicationError> {
        vault
            .submit_prepare_key(VaultKeyProgress::new())?
            .wait()
            .map(|_| ())
    }

    fn export(vault: &DesktopVault) -> Result<String, ApplicationError> {
        vault
            .submit_recovery_export(VaultKeyProgress::new())?
            .wait()
            .map(|secret| secret.to_string())
    }

    #[test]
    fn copied_keychain_references_become_local_missing_material_and_import_repairs_them()
    -> TestResult {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("synthetic.sqlite");
        let library = DesktopLibrary::open_synthetic(&path)?;
        let telegram = DesktopTelegram::open_direct(temp.path().join("synthetic.session"))?;
        let vault = DesktopVault::new(telegram, library.clone())?;
        prepare(&vault)?;
        let record = library.worker.vault_metadata()?.expect("vault metadata");
        let bundle = export(&vault)?;
        let mut db = teleark_storage::Database::open(&path)?;
        let backend = db.credential_backend(true)?;
        let mut items = db.credential_items()?;
        for item in &mut items {
            item.payload = None;
        }
        assert!(db.replace_credential_backend(backend, true, &items)?);
        let converted = db.credential_backend_for_platform(false)?;
        assert!(!converted.keychain_enabled);
        assert_eq!(db.unavailable_credential_count()?, 1);
        assert_eq!(db.vault_metadata()?, Some(record.clone()));
        drop(db);
        vault.lock()?;
        assert_eq!(
            prepare(&vault)
                .expect_err("foreign OS key unavailable")
                .kind(),
            ApplicationErrorKind::VaultKeyUnavailable
        );
        assert!(!library.keychain_status()?.enabled);
        assert_eq!(library.keychain_status()?.unavailable_credentials, 1);
        vault
            .submit_recovery_import(bundle.clone(), VaultKeyProgress::new())?
            .wait()?;
        assert_eq!(library.keychain_status()?.unavailable_credentials, 0);
        assert_eq!(export(&vault)?, bundle);
        assert_eq!(library.worker.vault_metadata()?, Some(record));
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn selected_backend_migrates_legacy_and_historical_authenticated_keys() -> TestResult {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("synthetic.sqlite");
        let library = DesktopLibrary::open_synthetic(&path)?;
        let telegram = DesktopTelegram::open_direct(temp.path().join("synthetic.session"))?;
        let vault = DesktopVault::new(telegram, library.clone())?;
        prepare(&vault)?;
        let old = library.worker.vault_metadata()?.expect("first epoch");
        let old_bundle = export(&vault)?;
        vault
            .submit_new_managed_key(VaultKeyProgress::new())?
            .wait()?;
        let active_bundle = export(&vault)?;

        // Simulate a pre-v23 key, which exists in Keychain without a registry row.
        let mut db = teleark_storage::Database::open(&path)?;
        let backend = db.credential_backend(true)?;
        assert!(db.save_credential_item(
            backend,
            crate::credential_store::VAULT_NAMESPACE,
            &identity(&old),
            None
        )?);
        drop(db);
        library.set_keychain_enabled(false, VaultKeyProgress::new())?;
        vault.lock()?;
        prepare(&vault)?;
        assert_eq!(export(&vault)?, active_bundle);
        assert_eq!(
            library
                .credential_read(crate::credential_store::VAULT_NAMESPACE, &identity(&old))?
                .as_deref()
                .map(Vec::as_slice),
            Some(old_bundle.as_bytes())
        );

        let rotated = vault
            .submit_recovery_rotation(VaultKeyProgress::new())?
            .wait()?;
        library.set_keychain_enabled(true, VaultKeyProgress::new())?;
        vault.lock()?;
        prepare(&vault)?;
        assert_eq!(export(&vault)?, rotated.to_string());
        let db = teleark_storage::Database::open(&path)?;
        assert!(
            db.credential_items()?
                .iter()
                .all(|item| item.payload.is_none())
        );
        Ok(())
    }

    #[test]
    fn automatic_keys_survive_restart_and_pin_changes_without_rewrapping_files() -> TestResult {
        let temp = tempfile::tempdir()?;
        let store = Arc::new(MemoryStore::default());
        let (vault, library) = open(temp.path(), store.clone())?;
        assert!(library.app_pin()?.is_none());
        let progress = VaultKeyProgress::new();
        let first = vault.submit_prepare_key(progress.clone())?;
        let second = vault.submit_prepare_key(VaultKeyProgress::new())?;
        assert!(
            first.wait()?.is_empty(),
            "setup does not disclose a recovery secret"
        );
        second.wait()?;
        let record = library.worker.vault_metadata()?.expect("automatic record");
        let revision = vault
            .inner
            .session
            .lock()
            .expect("session")
            .current_keys(0)?
            .revision;
        let bundle = export(&vault)?;
        assert_eq!(
            vault
                .inner
                .session
                .lock()
                .expect("session")
                .current_keys(0)?
                .revision,
            revision,
            "viewing a recovery bundle must not invalidate file projections"
        );
        assert_eq!(store.0.lock().expect("store").len(), 1);
        assert!(progress.snapshot().finished);
        assert_eq!(progress.snapshot().timeline.len(), 8);
        let master = authenticate_bundle(&record, &bundle)?;
        let wrapped = teleark_crypto::wrap_file_key(
            &master,
            &teleark_crypto::FileKey::from_bytes([8; 32]),
            &record.vault_id,
            &[3; 16],
            1,
            1,
            &mut AeadUsageRegistry::new(),
        )?;
        let pin = crate::AppPinRecord::create("123456".into())?;
        library.replace_app_pin(None, Some(pin.clone()))?;
        let replacement = crate::AppPinRecord::create("654321".into())?;
        library.replace_app_pin(Some(pin), Some(replacement.clone()))?;
        library.replace_app_pin(Some(replacement), None)?;
        assert_eq!(library.worker.vault_metadata()?, Some(record.clone()));
        vault.lock()?;
        drop(vault);
        let (reopened, _) = open(temp.path(), store)?;
        prepare(&reopened)?;
        assert!(!reopened.status().locked);
        assert_eq!(export(&reopened)?, bundle);
        let session = reopened.inner.session.lock().expect("session");
        let keys = session.current_keys(0)?;
        teleark_crypto::unwrap_file_key(
            keys.active.as_ref().expect("automatic key"),
            &wrapped,
            &record.vault_id,
            &[3; 16],
            1,
        )?;
        Ok(())
    }

    #[test]
    fn missing_or_corrupt_store_never_replaces_existing_keys_and_import_repairs_access()
    -> TestResult {
        let temp = tempfile::tempdir()?;
        let store = Arc::new(MemoryStore::default());
        let (vault, library) = open(temp.path(), store.clone())?;
        prepare(&vault)?;
        let bundle = export(&vault)?;
        let original = library.worker.vault_metadata()?.expect("record");
        vault.lock()?;
        store.0.lock().expect("store").clear();
        assert_eq!(
            prepare(&vault).expect_err("missing").kind(),
            ApplicationErrorKind::VaultKeyUnavailable
        );
        assert!(vault.status().locked);
        assert_eq!(library.worker.vault_metadata()?, Some(original.clone()));
        vault
            .submit_recovery_import(bundle, VaultKeyProgress::new())?
            .wait()?;
        assert!(!vault.status().locked);
        vault.lock()?;
        store.write(&identity(&original), b"corrupt fixture")?;
        assert!(prepare(&vault).is_err());
        assert!(vault.status().locked);
        assert_eq!(library.worker.vault_metadata()?, Some(original));
        Ok(())
    }

    struct FailingStore {
        base: MemoryStore,
        reject: AtomicBool,
        lose_writes: bool,
    }
    impl DeviceKeyStore for FailingStore {
        fn read(&self, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
            self.base.read(id)
        }
        fn write(&self, id: &str, bundle: &[u8]) -> Result<(), ApplicationError> {
            if self.reject.load(Ordering::Acquire) {
                return Err(ApplicationError::new(
                    ApplicationErrorKind::PermissionDenied,
                ));
            }
            if self.lose_writes {
                Ok(())
            } else {
                self.base.write(id, bundle)
            }
        }
    }

    #[test]
    fn denied_or_unverified_store_write_never_commits_metadata() -> TestResult {
        for lose_writes in [false, true] {
            let temp = tempfile::tempdir()?;
            let store = Arc::new(FailingStore {
                base: MemoryStore::default(),
                reject: AtomicBool::new(!lose_writes),
                lose_writes,
            });
            let (vault, library) = open(temp.path(), store)?;
            let progress = VaultKeyProgress::new();
            assert!(vault.submit_prepare_key(progress.clone())?.wait().is_err());
            assert!(progress.snapshot().finished);
            assert!(progress.snapshot().error.is_some());
            assert!(library.worker.vault_metadata()?.is_none());
            assert!(vault.status().locked);
        }
        Ok(())
    }

    #[test]
    fn failed_rotation_retains_old_store_binding_and_successful_rotation_preserves_master()
    -> TestResult {
        // Rotation may be requested before automatic setup has completed.
        let fresh_temp = tempfile::tempdir()?;
        let (fresh, _) = open(fresh_temp.path(), Arc::new(MemoryStore::default()))?;
        let combined_progress = VaultKeyProgress::new();
        fresh
            .submit_recovery_rotation(combined_progress.clone())?
            .wait()?;
        let combined = combined_progress.snapshot();
        assert_eq!(
            combined.timeline.last().map(|(phase, _)| *phase),
            Some(VaultKeyPhase::Completed)
        );
        assert!(combined.timeline.len() <= 16);
        let temp = tempfile::tempdir()?;
        let store = Arc::new(FailingStore {
            base: MemoryStore::default(),
            reject: AtomicBool::new(false),
            lose_writes: false,
        });
        let (vault, library) = open(temp.path(), store.clone())?;
        prepare(&vault)?;
        let old = library.worker.vault_metadata()?.expect("old");
        let bundle = export(&vault)?;
        store.reject.store(true, Ordering::Release);
        assert!(
            vault
                .submit_recovery_rotation(VaultKeyProgress::new())?
                .wait()
                .is_err()
        );
        assert_eq!(library.worker.vault_metadata()?, Some(old.clone()));
        vault.lock()?;
        prepare(&vault)?;
        assert_eq!(export(&vault)?, bundle);
        store.reject.store(false, Ordering::Release);
        let next_bundle = vault
            .submit_recovery_rotation(VaultKeyProgress::new())?
            .wait()?;
        let next = library.worker.vault_metadata()?.expect("next");
        assert_eq!(next.vault_id, old.vault_id);
        assert_eq!(next.recovery_generation, old.recovery_generation + 1);
        assert_ne!(identity(&old), identity(&next));
        let master = authenticate_bundle(&old, &bundle)?;
        let wrapped = teleark_crypto::wrap_file_key(
            &master,
            &teleark_crypto::FileKey::from_bytes([8; 32]),
            &old.vault_id,
            &[3; 16],
            1,
            1,
            &mut AeadUsageRegistry::new(),
        )?;
        let restored = authenticate_bundle(&next, &next_bundle)?;
        teleark_crypto::unwrap_file_key(&restored, &wrapped, &old.vault_id, &[3; 16], 1)?;
        vault.lock()?;
        prepare(&vault)?;
        assert_eq!(export(&vault)?, next_bundle.to_string());
        Ok(())
    }

    struct BlockedStore {
        base: Arc<MemoryStore>,
        entered: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        block: AtomicBool,
        block_reads: bool,
    }
    impl BlockedStore {
        fn wait(&self, reading: bool) {
            if self.block_reads == reading && self.block.swap(false, Ordering::AcqRel) {
                self.entered.send(()).expect("entered");
                self.release
                    .lock()
                    .expect("release")
                    .recv_timeout(Duration::from_secs(30))
                    .expect("bounded test release");
            }
        }
    }
    impl DeviceKeyStore for BlockedStore {
        fn read(&self, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
            self.wait(true);
            self.base.read(id)
        }
        fn write(&self, id: &str, bundle: &[u8]) -> Result<(), ApplicationError> {
            self.base.write(id, bundle)?;
            self.wait(false);
            Ok(())
        }
    }

    #[test]
    fn cancel_or_logout_during_keychain_read_never_reopens_the_session() -> TestResult {
        for logout in [false, true] {
            let temp = tempfile::tempdir()?;
            let base = Arc::new(MemoryStore::default());
            let (first, library) = open(temp.path(), base.clone())?;
            prepare(&first)?;
            let original = library.worker.vault_metadata()?;
            drop(first);
            let (entered, ready) = mpsc::sync_channel(1);
            let (release, wait) = mpsc::channel();
            let store = Arc::new(BlockedStore {
                base,
                entered,
                release: Mutex::new(wait),
                block: AtomicBool::new(true),
                block_reads: true,
            });
            let (vault, _) = open(temp.path(), store)?;
            let progress = VaultKeyProgress::new();
            let job = vault.submit_prepare_key(progress.clone())?;
            ready.recv_timeout(Duration::from_secs(30))?;
            assert_eq!(progress.snapshot().phase, VaultKeyPhase::Loading);
            if logout {
                vault.lock()?;
            } else {
                progress.cancel();
            }
            release.send(())?;
            assert_eq!(
                job.wait().expect_err("cancelled read").kind(),
                ApplicationErrorKind::Cancelled
            );
            assert!(vault.status().locked);
            assert_eq!(library.worker.vault_metadata()?, original);
            prepare(&vault)?;
            assert!(!vault.status().locked);
        }
        Ok(())
    }

    #[test]
    fn blocked_store_reports_phase_and_cancellation_or_logout_prevents_late_commit() -> TestResult {
        for logout in [false, true] {
            let temp = tempfile::tempdir()?;
            let (entered, ready) = mpsc::sync_channel(1);
            let (release, wait) = mpsc::channel();
            let base = Arc::new(MemoryStore::default());
            let store = Arc::new(BlockedStore {
                base,
                entered,
                release: Mutex::new(wait),
                block: AtomicBool::new(true),
                block_reads: false,
            });
            let (vault, library) = open(temp.path(), store)?;
            let progress = VaultKeyProgress::new();
            let job = vault.submit_prepare_key(progress.clone())?;
            ready.recv_timeout(Duration::from_secs(30))?;
            assert_eq!(progress.snapshot().phase, VaultKeyPhase::Securing);
            assert!(!progress.snapshot().finished);
            assert!(
                library.worker.vault_metadata()?.is_none(),
                "storage owner remains responsive before commit"
            );
            // A blocked key store cannot block status, preferences, or logout.
            library.preferences()?;
            if logout {
                vault.lock()?;
            } else {
                progress.cancel();
            }
            release.send(())?;
            assert_eq!(
                job.wait().expect_err("cancelled").kind(),
                ApplicationErrorKind::Cancelled
            );
            assert!(vault.status().locked);
            assert!(library.worker.vault_metadata()?.is_none());
            prepare(&vault)?;
            assert!(!vault.status().locked);
        }
        Ok(())
    }

    #[test]
    fn competing_database_commit_keeps_winning_device_key_and_orphan_is_not_authoritative()
    -> TestResult {
        let temp = tempfile::tempdir()?;
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::channel();
        let base = Arc::new(MemoryStore::default());
        let blocked = Arc::new(BlockedStore {
            base: base.clone(),
            entered,
            release: Mutex::new(wait),
            block: AtomicBool::new(true),
            block_reads: false,
        });
        let (first, library) = open(temp.path(), blocked)?;
        let job = first.submit_prepare_key(VaultKeyProgress::new())?;
        ready.recv_timeout(Duration::from_secs(30))?;
        let (winner, _) = open(temp.path(), base.clone())?;
        prepare(&winner)?;
        let record = library.worker.vault_metadata()?.expect("winner");
        let bundle = export(&winner)?;
        release.send(())?;
        assert!(job.wait().is_err());
        assert_eq!(library.worker.vault_metadata()?, Some(record));
        drop(first);
        drop(winner);
        let (restarted, _) = open(temp.path(), base)?;
        prepare(&restarted)?;
        assert_eq!(export(&restarted)?, bundle);
        Ok(())
    }

    #[test]
    fn passwordless_disaster_restore_and_historical_import_preserve_the_active_epoch() -> TestResult
    {
        let temp = tempfile::tempdir()?;
        let (vault, library) = open(temp.path(), Arc::new(MemoryStore::default()))?;
        prepare(&vault)?;
        let old_bundle = export(&vault)?;
        let old = library.worker.vault_metadata()?.expect("old");
        vault
            .submit_new_managed_key(VaultKeyProgress::new())?
            .wait()?;
        let next = library.worker.vault_metadata()?.expect("next");
        assert_ne!(old.vault_id, next.vault_id);
        vault
            .submit_recovery_import(old_bundle.clone(), VaultKeyProgress::new())?
            .wait()?;
        assert!(vault.status().historical_key_unlocked);
        assert!(!vault.status().active_key_locked);
        assert_eq!(library.worker.vault_metadata()?, Some(next));
        let independent_temp = tempfile::tempdir()?;
        let (independent, independent_library) =
            open(independent_temp.path(), Arc::new(MemoryStore::default()))?;
        prepare(&independent)?;
        let independent_active = independent_library.worker.vault_metadata()?;
        independent
            .submit_recovery_import(old_bundle.clone(), VaultKeyProgress::new())?
            .wait()?;
        assert!(independent.status().historical_key_unlocked);
        assert_eq!(
            independent_library.worker.vault_metadata()?,
            independent_active
        );
        assert_eq!(
            independent_library
                .worker
                .vault_key_epoch(old.vault_id)?
                .expect("imported")
                .recovery_wrap,
            old.recovery_wrap
        );
        let restored_temp = tempfile::tempdir()?;
        let store = Arc::new(MemoryStore::default());
        let (restored, recovered_library) = open(restored_temp.path(), store.clone())?;
        restored
            .submit_recovery_import(old_bundle.clone(), VaultKeyProgress::new())?
            .wait()?;
        assert_eq!(
            recovered_library
                .worker
                .vault_metadata()?
                .expect("restored")
                .vault_id,
            old.vault_id
        );
        assert!(recovered_library.app_pin()?.is_none());
        drop(restored);
        let (reopened, _) = open(restored_temp.path(), store)?;
        prepare(&reopened)?;
        assert_eq!(export(&reopened)?, old_bundle);
        Ok(())
    }
}
