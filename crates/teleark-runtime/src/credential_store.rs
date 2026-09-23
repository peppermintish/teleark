//! A retained, bounded owner serializes credential I/O and backend migration.
//! Keychain calls never run on Storage, the UI, or a network reactor.
use super::*;
use teleark_storage::{
    CREDENTIAL_ITEM_LIMIT, CREDENTIAL_TOTAL_BYTES_LIMIT, CredentialBackend, CredentialItem,
};
use zeroize::Zeroizing;

pub(crate) const VAULT_NAMESPACE: &str = "app.teleark.encryption-key.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeychainStatus {
    pub enabled: bool,
    pub supported: bool,
    pub cleanup_pending: u64,
    pub unavailable_credentials: u64,
}

pub(crate) trait SystemCredentialStore: Send + Sync {
    fn read(
        &self,
        namespace: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError>;
    fn write(&self, namespace: &str, account: &str, bytes: &[u8]) -> Result<(), ApplicationError>;
    fn delete(&self, namespace: &str, account: &str) -> Result<(), ApplicationError>;
}

#[cfg(not(test))]
struct PlatformStore;

#[cfg(all(target_os = "macos", not(test)))]
fn without_keychain_ui<T>(
    operation: impl FnOnce() -> Result<T, ApplicationError>,
) -> Result<T, ApplicationError> {
    use security_framework::os::macos::keychain::SecKeychain;
    // Apple's UI policy is process-global. Serialize every application use of it
    // and preserve a caller's already-disabled policy instead of restoring true.
    static POLICY: Mutex<()> = Mutex::new(());
    let _policy = POLICY.lock().map_err(|_| permission_error())?;
    let _interaction = if SecKeychain::user_interaction_allowed().map_err(|_| permission_error())? {
        Some(SecKeychain::disable_user_interaction().map_err(|_| permission_error())?)
    } else {
        None
    };
    operation()
}

#[cfg(all(target_os = "macos", not(test)))]
impl SystemCredentialStore for PlatformStore {
    fn read(
        &self,
        namespace: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        without_keychain_ui(|| {
            use security_framework::passwords::{PasswordOptions, generic_password};
            match generic_password(PasswordOptions::new_generic_password(namespace, account)) {
                Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
                Err(error) if error.code() == -25300 => Ok(None),
                Err(_) => Err(permission_error()),
            }
        })
    }
    fn write(&self, namespace: &str, account: &str, bytes: &[u8]) -> Result<(), ApplicationError> {
        without_keychain_ui(|| {
            security_framework::passwords::set_generic_password(namespace, account, bytes)
                .map_err(|_| permission_error())
        })
    }
    fn delete(&self, namespace: &str, account: &str) -> Result<(), ApplicationError> {
        without_keychain_ui(|| {
            match security_framework::passwords::delete_generic_password(namespace, account) {
                Ok(()) => Ok(()),
                Err(error) if error.code() == -25300 => Ok(()),
                Err(_) => Err(permission_error()),
            }
        })
    }
}

#[cfg(all(not(target_os = "macos"), not(test)))]
impl SystemCredentialStore for PlatformStore {
    fn read(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        Err(permission_error())
    }
    fn write(&self, _: &str, _: &str, _: &[u8]) -> Result<(), ApplicationError> {
        Err(permission_error())
    }
    fn delete(&self, _: &str, _: &str) -> Result<(), ApplicationError> {
        Err(permission_error())
    }
}

fn permission_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::PermissionDenied)
}
fn conflict_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Conflict)
}
fn persistence_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Persistence)
}

type SyntheticCredentials = std::collections::BTreeMap<(String, String), Zeroizing<Vec<u8>>>;
#[derive(Default)]
pub(crate) struct MemoryStore(Mutex<SyntheticCredentials>);
impl SystemCredentialStore for MemoryStore {
    fn read(
        &self,
        namespace: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        Ok(self
            .0
            .lock()
            .expect("synthetic credential store")
            .get(&(namespace.into(), account.into()))
            .cloned())
    }
    fn write(&self, namespace: &str, account: &str, bytes: &[u8]) -> Result<(), ApplicationError> {
        self.0.lock().expect("synthetic credential store").insert(
            (namespace.into(), account.into()),
            Zeroizing::new(bytes.to_vec()),
        );
        Ok(())
    }
    fn delete(&self, namespace: &str, account: &str) -> Result<(), ApplicationError> {
        self.0
            .lock()
            .expect("synthetic credential store")
            .remove(&(namespace.into(), account.into()));
        Ok(())
    }
}

fn platform_store() -> Arc<dyn SystemCredentialStore> {
    #[cfg(test)]
    {
        synthetic_store()
    }
    #[cfg(not(test))]
    {
        Arc::new(PlatformStore)
    }
}

fn synthetic_store() -> Arc<dyn SystemCredentialStore> {
    static STORE: std::sync::OnceLock<Arc<MemoryStore>> = std::sync::OnceLock::new();
    STORE
        .get_or_init(|| Arc::new(MemoryStore::default()))
        .clone()
}

type Command = Box<dyn FnOnce(&mut CredentialOwner) + Send>;
#[derive(Clone)]
pub(crate) struct CredentialStore(Arc<OwnerHandle>);
struct OwnerHandle {
    sender: mpsc::SyncSender<Command>,
    join: Mutex<Option<JoinHandle<()>>>,
}
struct CredentialOwner {
    storage: StorageWorker,
    system: Arc<dyn SystemCredentialStore>,
}

impl Drop for OwnerHandle {
    fn drop(&mut self) {
        // Dropping the sender drains accepted work. Joining a blocked system
        // store on the UI or network reactor would violate ownership guarantees.
        if let Ok(join) = self.join.get_mut()
            && let Some(join) = join.take()
            && join.is_finished()
        {
            let _ = join.join();
        }
    }
}

impl CredentialStore {
    pub(crate) fn open(storage: StorageWorker) -> Result<Self, ApplicationError> {
        Self::with_system(storage, platform_store())
    }
    fn with_system(
        storage: StorageWorker,
        system: Arc<dyn SystemCredentialStore>,
    ) -> Result<Self, ApplicationError> {
        let (sender, receiver) = mpsc::sync_channel::<Command>(32);
        let join = thread::Builder::new()
            .name("teleark-credentials".into())
            .spawn(move || {
                let mut owner = CredentialOwner { storage, system };
                while let Ok(command) = receiver.recv() {
                    command(&mut owner);
                }
            })
            .map_err(|_| persistence_error())?;
        Ok(Self(Arc::new(OwnerHandle {
            sender,
            join: Mutex::new(Some(join)),
        })))
    }
    fn request<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut CredentialOwner) -> Result<T, ApplicationError> + Send + 'static,
    ) -> Result<T, ApplicationError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.0
            .sender
            .send(Box::new(move |owner| {
                let _ = reply.send(operation(owner));
            }))
            .map_err(|_| persistence_error())?;
        response.recv().map_err(|_| persistence_error())?
    }
    pub(crate) fn read(
        &self,
        namespace: &str,
        identity: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        validate_identity(namespace, identity)?;
        let (namespace, identity) = (namespace.to_owned(), identity.to_owned());
        self.request(move |owner| owner.read(&namespace, &identity))
    }
    pub(crate) fn write(
        &self,
        namespace: &str,
        identity: &str,
        bytes: &[u8],
    ) -> Result<(), ApplicationError> {
        validate_identity(namespace, identity)?;
        validate_payload(bytes)?;
        let (namespace, identity, bytes) = (
            namespace.to_owned(),
            identity.to_owned(),
            Zeroizing::new(bytes.to_vec()),
        );
        self.request(move |owner| owner.write(&namespace, &identity, &bytes))
    }
    pub(crate) fn read_or_import_with(
        &self,
        namespace: &str,
        identity: &str,
        loader: impl FnOnce() -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> + Send + 'static,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        validate_identity(namespace, identity)?;
        let (namespace, identity) = (namespace.to_owned(), identity.to_owned());
        self.request(move |owner| {
            if let Some(existing) = owner.read(&namespace, &identity)? {
                return Ok(Some(existing));
            }
            let Some(bytes) = loader()? else {
                return Ok(None);
            };
            validate_payload(&bytes)?;
            owner.write(&namespace, &identity, &bytes)?;
            Ok(Some(bytes))
        })
    }
    pub(crate) fn delete(&self, namespace: &str, identity: &str) -> Result<(), ApplicationError> {
        validate_identity(namespace, identity)?;
        let (namespace, identity) = (namespace.to_owned(), identity.to_owned());
        self.request(move |owner| {
            owner.cleanup();
            let backend = owner.backend()?;
            owner.save(backend, &namespace, &identity, None)?;
            owner.cleanup();
            Ok(())
        })
    }
    pub(crate) fn set_enabled(
        &self,
        enabled: bool,
        progress: VaultKeyProgress,
    ) -> Result<(), ApplicationError> {
        self.request(move |owner| owner.migrate(enabled, &progress))
    }
}

fn validate_identity(namespace: &str, identity: &str) -> Result<(), ApplicationError> {
    if namespace.is_empty() || namespace.len() > 128 || identity.is_empty() || identity.len() > 512
    {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok(())
}
fn validate_payload(bytes: &[u8]) -> Result<(), ApplicationError> {
    if bytes.is_empty() || bytes.len() > 65536 {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok(())
}

impl CredentialOwner {
    fn cleanup_count(&self) -> Result<u64, ApplicationError> {
        self.storage.request("credential_cleanup_count", |reply| {
            StorageRequest::CredentialCleanupCount { reply }
        })
    }
    fn retire_unpublished(&self, references: Vec<teleark_storage::CredentialCleanup>) {
        for item in references {
            if self
                .storage
                .request("enqueue_credential_cleanup", |reply| {
                    StorageRequest::EnqueueCredentialCleanup { item, reply }
                })
                .is_err()
            {
                tracing::warn!(
                    event = "credential.cleanup.persistence_failed",
                    "unpublished credential cleanup could not be persisted"
                );
            }
        }
        self.cleanup();
    }
    fn cleanup(&self) {
        let Ok(items) = self.storage.request("credential_cleanup", |reply| {
            StorageRequest::CredentialCleanup { reply }
        }) else {
            return;
        };
        for item in items {
            if self
                .system
                .delete(&item.namespace, &item.keychain_account)
                .is_err()
            {
                tracing::warn!(
                    event = "credential.cleanup.pending",
                    "credential cleanup requires system access"
                );
                break;
            }
            if self
                .storage
                .request("complete_credential_cleanup", |reply| {
                    StorageRequest::CompleteCredentialCleanup { item, reply }
                })
                .is_err()
            {
                break;
            }
        }
    }
    fn backend(&self) -> Result<CredentialBackend, ApplicationError> {
        self.storage.request("credential_backend", |reply| {
            StorageRequest::CredentialBackend { reply }
        })
    }
    fn item(
        &self,
        namespace: &str,
        identity: &str,
    ) -> Result<Option<CredentialItem>, ApplicationError> {
        self.storage
            .request("credential_item", |reply| StorageRequest::CredentialItem {
                namespace: namespace.into(),
                identity: identity.into(),
                reply,
            })
    }
    fn save(
        &self,
        expected: CredentialBackend,
        namespace: &str,
        identity: &str,
        item: Option<CredentialItem>,
    ) -> Result<(), ApplicationError> {
        self.storage.request("save_credential_item", |reply| {
            StorageRequest::SaveCredentialItem {
                expected,
                namespace: namespace.into(),
                identity: identity.into(),
                item,
                reply,
            }
        })
    }
    fn read(
        &mut self,
        namespace: &str,
        identity: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        let backend = self.backend()?;
        if let Some(item) = self.item(namespace, identity)? {
            let bytes = if backend.keychain_enabled {
                let stored = self.system.read(namespace, &item.keychain_account)?;
                if stored.is_none() && namespace == VAULT_NAMESPACE {
                    return Ok(None);
                }
                stored
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))
            } else {
                if item.payload.is_none() && namespace == VAULT_NAMESPACE {
                    return Ok(None);
                }
                item.payload
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))
            }?;
            if namespace == VAULT_NAMESPACE && validate_payload(&bytes).is_err() {
                return Ok(None);
            }
            validate_payload(&bytes)?;
            return Ok(Some(bytes));
        }
        // v0.4.14 Vault entries keep their original service/account identity.
        if backend.keychain_enabled && namespace == VAULT_NAMESPACE {
            let bytes = self.system.read(namespace, identity)?;
            if let Some(bytes) = &bytes {
                if validate_payload(bytes).is_err() {
                    return Ok(None);
                }
                self.save(
                    backend,
                    namespace,
                    identity,
                    Some(CredentialItem {
                        namespace: namespace.into(),
                        identity: identity.into(),
                        keychain_account: identity.into(),
                        payload: None,
                    }),
                )?;
            }
            return Ok(bytes);
        }
        Ok(None)
    }
    fn account(namespace: &str, identity: &str) -> Result<String, ApplicationError> {
        if namespace == VAULT_NAMESPACE {
            return Ok(identity.into());
        }
        // Public references must not become offline password-hash verifiers.
        use teleark_crypto::RandomSource;
        let mut random = [0u8; 32];
        teleark_crypto::OsRandom
            .fill_bytes(&mut random)
            .map_err(|_| persistence_error())?;
        Ok(format!(
            "v1/{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ))
    }
    fn verified_system_write(
        &self,
        namespace: &str,
        account: &str,
        bytes: &[u8],
    ) -> Result<(), ApplicationError> {
        if namespace == VAULT_NAMESPACE {
            crate::vault::validate_device_credential(account, bytes)?;
        }
        if let Some(existing) = self.system.read(namespace, account)? {
            if existing.as_slice() != bytes {
                if namespace != VAULT_NAMESPACE
                    || crate::vault::validate_device_credential(account, &existing).is_ok()
                {
                    return Err(conflict_error());
                }
                // Explicit recovery import authenticated the replacement above.
                // An invalid old value is repairable; a valid different value is not.
                self.system.write(namespace, account, bytes)?;
            }
        } else {
            self.system.write(namespace, account, bytes)?;
        }
        if self
            .system
            .read(namespace, account)?
            .as_deref()
            .map(Vec::as_slice)
            != Some(bytes)
        {
            return Err(persistence_error());
        }
        Ok(())
    }
    fn write(
        &mut self,
        namespace: &str,
        identity: &str,
        bytes: &[u8],
    ) -> Result<(), ApplicationError> {
        self.cleanup();
        let backend = self.backend()?;
        if self.cleanup_count()? >= CREDENTIAL_ITEM_LIMIT as u64 {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        let account = Self::account(namespace, identity)?;
        let unpublished = if backend.keychain_enabled && namespace != VAULT_NAMESPACE {
            vec![teleark_storage::CredentialCleanup {
                namespace: namespace.into(),
                keychain_account: account.clone(),
            }]
        } else {
            Vec::new()
        };
        let result = (|| {
            if backend.keychain_enabled {
                self.verified_system_write(namespace, &account, bytes)?;
            }
            self.save(
                backend,
                namespace,
                identity,
                Some(CredentialItem {
                    namespace: namespace.into(),
                    identity: identity.into(),
                    keychain_account: account,
                    payload: (!backend.keychain_enabled).then(|| Zeroizing::new(bytes.to_vec())),
                }),
            )
        })();
        if let Err(error) = result {
            self.retire_unpublished(unpublished);
            return Err(error);
        }
        self.cleanup();
        Ok(())
    }
    fn migrate(
        &mut self,
        enabled: bool,
        progress: &VaultKeyProgress,
    ) -> Result<(), ApplicationError> {
        progress.phase(VaultKeyPhase::Loading)?;
        if enabled && !cfg!(target_os = "macos") {
            return Err(permission_error());
        }
        self.cleanup();
        // Register every readable legacy epoch before the snapshot. Missing old
        // password-only epochs stay intact and continue to accept recovery import.
        let mut backend = self.backend()?;
        if backend.keychain_enabled {
            let records = self.storage.request("credential_vault_epochs", |reply| {
                StorageRequest::CredentialVaultEpochs { reply }
            })?;
            for record in records {
                progress.check_cancelled()?;
                let id = crate::vault::device_key_identity(&record);
                let _ = self.read(VAULT_NAMESPACE, &id)?;
                progress.activity()?;
            }
            backend = self.backend()?;
        }
        if backend.keychain_enabled == enabled {
            self.cleanup();
            return Ok(());
        }
        let mut items = self.storage.request("credential_items", |reply| {
            StorageRequest::CredentialItems { reply }
        })?;
        if items.len() > CREDENTIAL_ITEM_LIMIT {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        if enabled
            && self.cleanup_count()?.saturating_add(items.len() as u64)
                > CREDENTIAL_ITEM_LIMIT as u64
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        progress.phase(VaultKeyPhase::Securing)?;
        let mut unpublished = Vec::new();
        let result = (|| {
            let mut total = 0usize;
            for item in &mut items {
                progress.check_cancelled()?;
                if enabled {
                    let bytes = item.payload.take().ok_or_else(|| {
                        ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable)
                    })?;
                    if item.namespace == VAULT_NAMESPACE {
                        crate::vault::validate_device_credential(&item.identity, &bytes)?;
                    }
                    // A failed prior deletion may still target the old reference.
                    // Re-enabling uses a fresh immutable generic account so cleanup
                    // from another process cannot delete the newly active value.
                    item.keychain_account = Self::account(&item.namespace, &item.identity)?;
                    if item.namespace != VAULT_NAMESPACE {
                        unpublished.push(teleark_storage::CredentialCleanup {
                            namespace: item.namespace.clone(),
                            keychain_account: item.keychain_account.clone(),
                        });
                    }
                    self.verified_system_write(&item.namespace, &item.keychain_account, &bytes)?;
                } else {
                    let bytes = self
                        .system
                        .read(&item.namespace, &item.keychain_account)?
                        .ok_or_else(|| {
                            ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable)
                        })?;
                    validate_payload(&bytes)?;
                    if item.namespace == VAULT_NAMESPACE {
                        crate::vault::validate_device_credential(&item.identity, &bytes)?;
                    }
                    total = total.saturating_add(bytes.len());
                    if total > CREDENTIAL_TOTAL_BYTES_LIMIT {
                        return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
                    }
                    item.payload = Some(bytes);
                }
                progress.activity()?;
            }
            progress.phase(VaultKeyPhase::Saving)?;
            self.storage.request("replace_credential_backend", |reply| {
                StorageRequest::ReplaceCredentialBackend {
                    expected: backend,
                    enabled,
                    items,
                    reply,
                }
            })
        })();
        if let Err(error) = result {
            self.retire_unpublished(unpublished);
            return Err(error);
        }
        self.cleanup();
        Ok(())
    }
}

impl DesktopLibrary {
    /// Opens isolated fixture storage without ever accessing the system Keychain.
    /// Synthetic secrets survive reopening within this process only.
    pub fn open_synthetic(path: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        let mut library = Self::open(path)?;
        library.credential_store =
            CredentialStore::with_system(library.worker.clone(), synthetic_store())?;
        Ok(library)
    }
    pub fn keychain_status(&self) -> Result<KeychainStatus, ApplicationError> {
        let backend = self.worker.request("credential_backend", |reply| {
            StorageRequest::CredentialBackend { reply }
        })?;
        Ok(KeychainStatus {
            enabled: backend.keychain_enabled,
            supported: cfg!(target_os = "macos"),
            cleanup_pending: self.worker.request("credential_cleanup_count", |reply| {
                StorageRequest::CredentialCleanupCount { reply }
            })?,
            unavailable_credentials: self
                .worker
                .request("unavailable_credential_count", |reply| {
                    StorageRequest::UnavailableCredentialCount { reply }
                })?,
        })
    }
    /// Background-only. Publish the supplied progress before calling this method.
    /// Verified copies and the preference are committed in one SQLite transaction.
    pub fn set_keychain_enabled(
        &self,
        enabled: bool,
        progress: VaultKeyProgress,
    ) -> Result<(), ApplicationError> {
        let result = (|| {
            progress.phase(VaultKeyPhase::Loading)?;
            if enabled && !cfg!(target_os = "macos") {
                return Err(permission_error());
            }
            self.migrate_legacy_credentials()?;
            self.credential_store.set_enabled(enabled, progress.clone())
        })();
        progress.finish(result.as_ref().err().map(ApplicationError::kind));
        result
    }
    pub(crate) fn credential_read(
        &self,
        namespace: &str,
        identity: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        self.credential_store.read(namespace, identity)
    }
    pub(crate) fn credential_write(
        &self,
        namespace: &str,
        identity: &str,
        bytes: &[u8],
    ) -> Result<(), ApplicationError> {
        self.credential_store.write(namespace, identity, bytes)
    }
    pub(crate) fn credential_read_or_import_with(
        &self,
        namespace: &str,
        identity: &str,
        loader: impl FnOnce() -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> + Send + 'static,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
        self.credential_store
            .read_or_import_with(namespace, identity, loader)
    }
    pub(crate) fn credential_delete(
        &self,
        namespace: &str,
        identity: &str,
    ) -> Result<(), ApplicationError> {
        self.credential_store.delete(namespace, identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use std::sync::atomic::{AtomicBool, Ordering};
    #[cfg(target_os = "macos")]
    use std::time::Duration;
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    const SERVICE: &str = "app.teleark.synthetic.v1";

    fn library(
        path: &Path,
        system: Arc<dyn SystemCredentialStore>,
        enabled: bool,
    ) -> Result<DesktopLibrary, ApplicationError> {
        let mut db = Database::open(path).map_err(map_storage_error)?;
        db.credential_backend(enabled).map_err(map_storage_error)?;
        drop(db);
        let mut library = DesktopLibrary::open(path)?;
        library.credential_store = CredentialStore::with_system(library.worker.clone(), system)?;
        Ok(library)
    }

    #[test]
    fn sqlite_credentials_survive_restart_and_import_never_overwrites_a_new_value() -> TestResult {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("synthetic.sqlite");
        let first = library(&path, Arc::new(MemoryStore::default()), false)?;
        first.credential_write(SERVICE, "default", b"new synthetic value")?;
        let result = first
            .credential_read_or_import_with(SERVICE, "default", || {
                Ok(Some(Zeroizing::new(b"old synthetic value".to_vec())))
            })?
            .expect("existing value");
        assert_eq!(result.as_slice(), b"new synthetic value");
        drop(first);
        let reopened = library(&path, Arc::new(MemoryStore::default()), false)?;
        assert_eq!(
            reopened
                .credential_read(SERVICE, "default")?
                .as_deref()
                .map(Vec::as_slice),
            Some(b"new synthetic value".as_slice())
        );
        reopened.credential_delete(SERVICE, "default")?;
        assert!(reopened.credential_read(SERVICE, "default")?.is_none());
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn verified_backend_switch_preserves_values_both_directions_and_restarts() -> TestResult {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("synthetic.sqlite");
        let system = Arc::new(MemoryStore::default());
        let first = library(&path, system.clone(), false)?;
        first.credential_write(SERVICE, "default", b"synthetic secret")?;
        first.set_keychain_enabled(true, VaultKeyProgress::new())?;
        assert!(first.keychain_status()?.enabled);
        let db = Database::open(&path)?;
        assert!(
            db.credential_items()?
                .iter()
                .all(|item| item.payload.is_none())
        );
        drop(db);
        drop(first);
        let reopened = library(&path, system, false)?;
        assert!(reopened.keychain_status()?.enabled);
        assert_eq!(
            reopened
                .credential_read(SERVICE, "default")?
                .as_deref()
                .map(Vec::as_slice),
            Some(b"synthetic secret".as_slice())
        );
        reopened.set_keychain_enabled(false, VaultKeyProgress::new())?;
        drop(reopened);
        let local_only = library(&path, Arc::new(MemoryStore::default()), false)?;
        assert!(!local_only.keychain_status()?.enabled);
        assert_eq!(
            local_only
                .credential_read(SERVICE, "default")?
                .as_deref()
                .map(Vec::as_slice),
            Some(b"synthetic secret".as_slice())
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    struct FailingStore {
        base: MemoryStore,
        reject: bool,
        lose_writes: bool,
    }
    #[cfg(target_os = "macos")]
    impl SystemCredentialStore for FailingStore {
        fn read(
            &self,
            namespace: &str,
            account: &str,
        ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
            self.base.read(namespace, account)
        }
        fn write(
            &self,
            namespace: &str,
            account: &str,
            bytes: &[u8],
        ) -> Result<(), ApplicationError> {
            if self.reject {
                return Err(permission_error());
            }
            if self.lose_writes {
                return Ok(());
            }
            self.base.write(namespace, account, bytes)
        }
        fn delete(&self, namespace: &str, account: &str) -> Result<(), ApplicationError> {
            self.base.delete(namespace, account)
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn denied_or_unverified_migration_preserves_source_and_preference() -> TestResult {
        for lose_writes in [false, true] {
            let temp = tempfile::tempdir()?;
            let first = library(
                &temp.path().join("synthetic.sqlite"),
                Arc::new(FailingStore {
                    base: MemoryStore::default(),
                    reject: !lose_writes,
                    lose_writes,
                }),
                false,
            )?;
            first.credential_write(SERVICE, "default", b"synthetic secret")?;
            let progress = VaultKeyProgress::new();
            assert!(first.set_keychain_enabled(true, progress.clone()).is_err());
            assert!(progress.snapshot().finished);
            assert!(progress.snapshot().error.is_some());
            assert!(!first.keychain_status()?.enabled);
            assert_eq!(
                first
                    .credential_read(SERVICE, "default")?
                    .as_deref()
                    .map(Vec::as_slice),
                Some(b"synthetic secret".as_slice())
            );
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    struct BlockedStore {
        base: MemoryStore,
        entered: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        blocked: AtomicBool,
    }
    #[cfg(target_os = "macos")]
    impl SystemCredentialStore for BlockedStore {
        fn read(
            &self,
            namespace: &str,
            account: &str,
        ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
            if self.blocked.swap(false, Ordering::AcqRel) {
                self.entered.send(()).expect("signal synthetic store");
                self.release
                    .lock()
                    .expect("synthetic release")
                    .recv_timeout(Duration::from_secs(30))
                    .expect("bounded release");
            }
            self.base.read(namespace, account)
        }
        fn write(
            &self,
            namespace: &str,
            account: &str,
            bytes: &[u8],
        ) -> Result<(), ApplicationError> {
            self.base.write(namespace, account, bytes)
        }
        fn delete(&self, namespace: &str, account: &str) -> Result<(), ApplicationError> {
            self.base.delete(namespace, account)
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn blocked_keychain_keeps_storage_responsive_and_cancel_prevents_commit() -> TestResult {
        let temp = tempfile::tempdir()?;
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::channel();
        let first = library(
            &temp.path().join("synthetic.sqlite"),
            Arc::new(BlockedStore {
                base: MemoryStore::default(),
                entered,
                release: Mutex::new(wait),
                blocked: AtomicBool::new(true),
            }),
            false,
        )?;
        first.credential_write(SERVICE, "default", b"synthetic secret")?;
        let progress = VaultKeyProgress::new();
        let worker_library = first.clone();
        let worker_progress = progress.clone();
        let operation =
            thread::spawn(move || worker_library.set_keychain_enabled(true, worker_progress));
        ready.recv_timeout(Duration::from_secs(30))?;
        assert_eq!(progress.snapshot().phase, VaultKeyPhase::Securing);
        assert!(!progress.snapshot().finished);
        assert!(!first.keychain_status()?.enabled);
        first.preferences()?;
        progress.cancel();
        release.send(())?;
        assert_eq!(
            operation
                .join()
                .expect("synthetic operation")
                .expect_err("cancelled migration")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        assert!(!first.keychain_status()?.enabled);
        assert_eq!(
            first
                .credential_read(SERVICE, "default")?
                .as_deref()
                .map(Vec::as_slice),
            Some(b"synthetic secret".as_slice())
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn missing_registered_keychain_item_fails_closed() -> TestResult {
        let temp = tempfile::tempdir()?;
        let system = Arc::new(MemoryStore::default());
        let first = library(&temp.path().join("synthetic.sqlite"), system.clone(), true)?;
        first.credential_write(SERVICE, "default", b"synthetic secret")?;
        system.0.lock().expect("synthetic store").clear();
        assert_eq!(
            first
                .credential_read(SERVICE, "default")
                .expect_err("missing protected credential")
                .kind(),
            ApplicationErrorKind::VaultKeyUnavailable
        );
        Ok(())
    }

    #[test]
    fn lazy_import_never_reads_a_legacy_secret_when_current_value_exists() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first = library(
            &temp.path().join("synthetic.sqlite"),
            Arc::new(MemoryStore::default()),
            false,
        )?;
        first.credential_write(SERVICE, "default", b"current synthetic secret")?;
        let current = first.credential_read_or_import_with(SERVICE, "default", || {
            panic!("legacy loader must not run")
        })?;
        assert_eq!(
            current.as_deref().map(Vec::as_slice),
            Some(b"current synthetic secret".as_slice())
        );
        first.credential_delete(SERVICE, "default")?;
        assert!(
            first
                .credential_read_or_import_with(SERVICE, "default", || Ok(None))?
                .is_none()
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    struct DeleteFailingStore {
        base: MemoryStore,
        reject: AtomicBool,
    }
    #[cfg(target_os = "macos")]
    impl SystemCredentialStore for DeleteFailingStore {
        fn read(
            &self,
            namespace: &str,
            account: &str,
        ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
            self.base.read(namespace, account)
        }
        fn write(
            &self,
            namespace: &str,
            account: &str,
            bytes: &[u8],
        ) -> Result<(), ApplicationError> {
            self.base.write(namespace, account, bytes)
        }
        fn delete(&self, namespace: &str, account: &str) -> Result<(), ApplicationError> {
            if self.reject.load(Ordering::Acquire) {
                Err(permission_error())
            } else {
                self.base.delete(namespace, account)
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cleanup_failure_is_durable_and_retry_cannot_delete_reenabled_credentials() -> TestResult {
        let temp = tempfile::tempdir()?;
        let system = Arc::new(DeleteFailingStore {
            base: MemoryStore::default(),
            reject: AtomicBool::new(true),
        });
        let first = library(&temp.path().join("synthetic.sqlite"), system.clone(), true)?;
        first.credential_write(SERVICE, "default", b"old synthetic secret")?;
        first.credential_write(SERVICE, "default", b"current synthetic secret")?;
        assert_eq!(first.keychain_status()?.cleanup_pending, 1);
        first.set_keychain_enabled(false, VaultKeyProgress::new())?;
        assert_eq!(first.keychain_status()?.cleanup_pending, 2);
        first.set_keychain_enabled(true, VaultKeyProgress::new())?;
        assert_eq!(first.keychain_status()?.cleanup_pending, 2);
        system.reject.store(false, Ordering::Release);
        first.set_keychain_enabled(true, VaultKeyProgress::new())?;
        assert_eq!(first.keychain_status()?.cleanup_pending, 0);
        assert_eq!(system.base.0.lock().expect("synthetic store").len(), 1);
        assert_eq!(
            first
                .credential_read(SERVICE, "default")?
                .as_deref()
                .map(Vec::as_slice),
            Some(b"current synthetic secret".as_slice())
        );
        first.credential_delete(SERVICE, "default")?;
        assert!(system.base.0.lock().expect("synthetic store").is_empty());
        Ok(())
    }

    #[cfg(target_os = "macos")]
    struct PartialMigrationStore {
        base: MemoryStore,
        writes: std::sync::atomic::AtomicUsize,
    }
    #[cfg(target_os = "macos")]
    impl SystemCredentialStore for PartialMigrationStore {
        fn read(
            &self,
            namespace: &str,
            account: &str,
        ) -> Result<Option<Zeroizing<Vec<u8>>>, ApplicationError> {
            self.base.read(namespace, account)
        }
        fn write(
            &self,
            namespace: &str,
            account: &str,
            bytes: &[u8],
        ) -> Result<(), ApplicationError> {
            if self.writes.fetch_add(1, Ordering::AcqRel) == 1 {
                return Err(permission_error());
            }
            self.base.write(namespace, account, bytes)
        }
        fn delete(&self, namespace: &str, account: &str) -> Result<(), ApplicationError> {
            self.base.delete(namespace, account)
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn partial_failed_migration_retires_unpublished_generic_copies_before_retry() -> TestResult {
        let temp = tempfile::tempdir()?;
        let system = Arc::new(PartialMigrationStore {
            base: MemoryStore::default(),
            writes: std::sync::atomic::AtomicUsize::new(0),
        });
        let first = library(&temp.path().join("synthetic.sqlite"), system.clone(), false)?;
        first.credential_write(SERVICE, "a", b"first synthetic secret")?;
        first.credential_write(SERVICE, "b", b"second synthetic secret")?;
        assert!(
            first
                .set_keychain_enabled(true, VaultKeyProgress::new())
                .is_err()
        );
        assert!(!first.keychain_status()?.enabled);
        assert_eq!(first.keychain_status()?.cleanup_pending, 0);
        assert!(system.base.0.lock().expect("synthetic store").is_empty());
        assert_eq!(
            first
                .credential_read(SERVICE, "a")?
                .as_deref()
                .map(Vec::as_slice),
            Some(b"first synthetic secret".as_slice())
        );
        first.set_keychain_enabled(true, VaultKeyProgress::new())?;
        assert_eq!(system.base.0.lock().expect("synthetic store").len(), 2);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn competing_database_revision_retires_only_unpublished_copy() -> TestResult {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("synthetic.sqlite");
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::channel();
        let system = Arc::new(BlockedStore {
            base: MemoryStore::default(),
            entered,
            release: Mutex::new(wait),
            blocked: AtomicBool::new(false),
        });
        let first = library(&path, system.clone(), true)?;
        first.credential_write(SERVICE, "a", b"first synthetic secret")?;
        system.blocked.store(true, Ordering::Release);
        let worker_library = first.clone();
        let operation = thread::spawn(move || {
            worker_library.credential_write(SERVICE, "a", b"competing synthetic secret")
        });
        ready.recv_timeout(Duration::from_secs(30))?;
        let mut db = Database::open(&path)?;
        let before = db.credential_backend(true)?;
        let retained = db.credential_items()?;
        assert!(db.replace_credential_backend(before, true, &retained)?);
        release.send(())?;
        assert_eq!(
            operation
                .join()
                .expect("synthetic operation")
                .expect_err("stale revision")
                .kind(),
            ApplicationErrorKind::Conflict
        );
        assert_eq!(system.base.0.lock().expect("synthetic store").len(), 1);
        assert_eq!(
            first
                .credential_read(SERVICE, "a")?
                .as_deref()
                .map(Vec::as_slice),
            Some(b"first synthetic secret".as_slice())
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn authenticated_recovery_import_repairs_missing_and_corrupt_registered_keychain_values()
    -> TestResult {
        let temp = tempfile::tempdir()?;
        let system = Arc::new(MemoryStore::default());
        let first = library(&temp.path().join("synthetic.sqlite"), system.clone(), true)?;
        let telegram = DesktopTelegram::open_direct(temp.path().join("synthetic.session"))?;
        let vault = DesktopVault::new(telegram, first.clone())?;
        vault.submit_prepare_key(VaultKeyProgress::new())?.wait()?;
        vault
            .submit_select_channel_key(1, 2, VaultKeyProgress::new())?
            .wait()?;
        let bundle = vault
            .submit_recovery_export(VaultKeyProgress::new())?
            .wait()?
            .to_string();
        let original = first.worker.vault_metadata()?.expect("vault epoch");
        let identity = crate::vault::device_key_identity(&original);
        for corrupt in [
            None,
            Some(Vec::new()),
            Some(vec![b'x'; 65_537]),
            Some(b"corrupt synthetic bundle".to_vec()),
        ] {
            if let Some(bytes) = corrupt {
                system.write(VAULT_NAMESPACE, &identity, &bytes)?;
            } else {
                system.0.lock().expect("synthetic store").clear();
            }
            vault.lock()?;
            assert!(
                vault
                    .submit_prepare_key(VaultKeyProgress::new())?
                    .wait()
                    .is_err()
            );
            assert!(vault.status().locked);
            vault
                .submit_recovery_import(bundle.clone(), VaultKeyProgress::new())?
                .wait()?;
            vault
                .submit_select_channel_key(1, 2, VaultKeyProgress::new())?
                .wait()?;
            assert!(!vault.status().locked);
            assert_eq!(first.worker.vault_metadata()?, Some(original.clone()));
            assert_eq!(
                vault
                    .submit_recovery_export(VaultKeyProgress::new())?
                    .wait()?
                    .to_string(),
                bundle
            );
        }
        Ok(())
    }
}
