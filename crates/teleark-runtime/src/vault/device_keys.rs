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
    fn latest_channel_proof(
        &self,
        account: i64,
        chat: i64,
    ) -> Result<Option<crate::TelegramFileSummary>, ApplicationError> {
        let mut proofs = self.telegram.search_files_exact_caption(
            account,
            chat,
            CHANNEL_KEY_PROOF_CAPTION,
            16,
            None,
        )?;
        proofs.sort_by_key(|file| std::cmp::Reverse(file.message_id));
        Ok(proofs.into_iter().next())
    }

    fn publish_channel_proof(
        &self,
        account: i64,
        chat: i64,
        progress: &VaultKeyProgress,
    ) -> Result<(), ApplicationError> {
        progress.phase(VaultKeyPhase::CheckingChannel)?;
        let record = self
            .record
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
        let master = self
            .master_key
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
        let expected =
            teleark_crypto::seal_channel_key_proof(master, record.vault_id, account, chat);
        if let Some(file) = self.latest_channel_proof(account, chat)? {
            let bytes = self
                .telegram
                .download_bytes(account, chat, file.message_id, None)?;
            if teleark_crypto::open_channel_key_proof(
                &bytes,
                master,
                record.vault_id,
                account,
                chat,
            ) {
                return Ok(());
            }
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        progress.check_cancelled()?;
        let digest = blake3::hash(&expected);
        let mut random = [0; 8];
        random.copy_from_slice(&digest.as_bytes()[..8]);
        let publication_random_id = (i64::from_be_bytes(random) & i64::MAX).max(1);
        let message_id = self.telegram.upload_bytes_observed(
            account,
            chat,
            "channel-key-proof.v1.takp".into(),
            CHANNEL_KEY_PROOF_CAPTION.into(),
            expected,
            crate::telegram::UploadByteOptions {
                publication_random_id: Some(publication_random_id),
                ..Default::default()
            },
        )?;
        let bytes = self
            .telegram
            .download_bytes(account, chat, message_id, None)?;
        if !teleark_crypto::open_channel_key_proof(&bytes, master, record.vault_id, account, chat) {
            return Err(ApplicationError::new(
                ApplicationErrorKind::VaultKeyUnavailable,
            ));
        }
        progress.activity()?;
        Ok(())
    }

    pub(super) fn select_channel_key(
        &mut self,
        account: i64,
        chat: i64,
        initialize_empty: Option<VaultChannelSetupScope>,
        progress: &VaultKeyProgress,
    ) -> Result<VaultKeySelection, ApplicationError> {
        progress.phase(VaultKeyPhase::Loading)?;
        let records = self
            .library
            .worker
            .request("credential_vault_epochs", |reply| {
                crate::StorageRequest::CredentialVaultEpochs { reply }
            })?;
        let preferred = self.library.worker.vault_metadata()?;
        let mut available = Vec::new();
        let mut stored_material_found = false;
        for record in records {
            progress.check_cancelled()?;
            // The selected credential backend is authoritative. A missing or
            // unreadable reference is not a usable key.
            if let Some(bytes) = self.device_keys.read(&identity(&record))? {
                stored_material_found = true;
                if let Ok(text) = std::str::from_utf8(&bytes)
                    && let Ok(master) = authenticate_bundle(&record, text)
                {
                    available.push((record, Arc::new(master)));
                }
            }
            progress.activity()?;
        }
        let pending_key = if initialize_empty.is_some() {
            self.library.pending_channel_key(account)?
        } else {
            None
        };
        let bound_channel = self.library.storage_channel_id(account)? == Some(chat);
        let pending_for_channel =
            bound_channel && pending_key.is_some_and(|marker| marker.channel_id == chat);
        let proof = if bound_channel {
            progress.phase(VaultKeyPhase::CheckingChannel)?;
            self.latest_channel_proof(account, chat)?
        } else {
            None
        };
        if pending_for_channel {
            progress.phase(VaultKeyPhase::CheckingChannel)?;
            let manifests = self.library.cached_manifest_candidates(account, chat)?;
            let pending = self
                .library
                .cached_pending_upload_candidates(account, chat)?;
            // The initial managed sync seeds manifests, but a concurrent
            // upload can publish after that read. Recheck both locators just
            // before initializing an apparently empty channel.
            let remote_has_managed = if manifests.is_empty() && pending.is_empty() {
                !self
                    .telegram
                    .search_files_exact_caption(
                        account,
                        chat,
                        crate::transfer::MANIFEST_CAPTION,
                        1,
                        None,
                    )?
                    .is_empty()
                    || !self
                        .telegram
                        .search_files_exact_caption(account, chat, remote_upload::CAPTION, 1, None)?
                        .is_empty()
            } else {
                true
            };
            if !remote_has_managed && proof.is_none() {
                // The initial managed catalog has been read. A lost key must
                // never cause a replacement while recoverable remote objects
                // are present; an actually empty channel can be initialized.
                let scope = initialize_empty.expect("pending setup has a captured scope");
                self.check_channel_setup_scope(scope, chat)?;
                let target = pending_key.and_then(|marker| marker.target_vault_id);
                let target_record_persisted = target
                    .map(|id| self.library.worker.vault_key_epoch(id))
                    .transpose()?
                    .flatten()
                    .is_some();
                let mut proof_published = false;
                if target_record_persisted {
                    if let Some(index) = available
                        .iter()
                        .position(|(item, _)| Some(item.vault_id) == target)
                    {
                        let (record, master) = available.swap_remove(index);
                        self.record = Some(record);
                        self.master_key = Some(master);
                        self.publish_channel_proof(account, chat, progress)?;
                        proof_published = true;
                    }
                    // A persisted key whose credential is missing cannot be
                    // replaced just because the channel has no files yet.
                } else if preferred.as_ref().map(|record| record.vault_id)
                    == pending_key.and_then(|marker| marker.previous_vault_id)
                {
                    let password = random_wrapping_password()?;
                    let _secret = Zeroizing::new(self.create_key_epoch_persisted(
                        &password,
                        progress,
                        true,
                        Some((scope, chat)),
                    )?);
                    self.publish_channel_proof(account, chat, progress)?;
                    proof_published = true;
                }
                if proof_published {
                    self.library.clear_pending_channel_key(account, chat)?;
                    self.check_key_generation()?;
                    self.historical_key = None;
                    self.catalog.clear();
                    let mut session = self
                        .session
                        .lock()
                        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
                    session.current_keys(self.session_generation)?;
                    session.publish(
                        self.session_generation,
                        self.record.clone(),
                        self.master_key.clone(),
                        None,
                    );
                    session.status.key_selection = Some(VaultKeySelection::Ready);
                    return Ok(VaultKeySelection::Ready);
                }
            }
        }
        let mut choice = if available.is_empty() {
            None
        } else {
            progress.phase(VaultKeyPhase::CheckingChannel)?;
            let mut candidates = self.library.cached_manifest_candidates(account, chat)?;
            candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.file.message_id.get()));
            candidates.truncate(MAX_MANIFEST_SCAN);
            let proof_choice = if let Some(proof) = proof.as_ref() {
                progress.check_cancelled()?;
                let bytes = self
                    .telegram
                    .download_bytes(account, chat, proof.message_id, None)?;
                available.iter().position(|(record, master)| {
                    teleark_crypto::open_channel_key_proof(
                        &bytes,
                        master,
                        record.vault_id,
                        account,
                        chat,
                    )
                })
            } else {
                None
            };
            if proof.is_some() {
                // The newest authenticated proof selects the channel's upload
                // key. A file-local edit, corruption or historical epoch must
                // never revoke it or hide every unrelated file.
                proof_choice
            } else if candidates.is_empty() && !bound_channel {
                preferred
                    .as_ref()
                    .and_then(|record| {
                        available
                            .iter()
                            .position(|(item, _)| item.vault_id == record.vault_id)
                    })
                    .or(Some(0))
            } else if candidates.is_empty() {
                None
            } else {
                let mut store = TelegramObjectStore::new(self.telegram.clone(), account, chat);
                let mut selected = None;
                for candidate in candidates {
                    progress.check_cancelled()?;
                    let object = catalog::byte_object(&candidate)?;
                    let bytes = match store.download(object.object_id).map_err(map_transfer_error) {
                        Ok(bytes) => bytes,
                        Err(error)
                            if matches!(
                                error.kind(),
                                ApplicationErrorKind::SourceMissing
                                    | ApplicationErrorKind::NotFound
                            ) =>
                        {
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    if bytes.len() as u64 != object.encoded_size {
                        continue;
                    }
                    let id = match teleark_crypto::manifest_vault_id_hint(
                        &bytes,
                        teleark_crypto::ManifestLimits::default(),
                    ) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };
                    let index = available
                        .iter()
                        .position(|(record, _)| record.vault_id == id);
                    let Some(index) = index else { continue };
                    let Ok(manifest) = teleark_crypto::open_manifest(
                        &bytes,
                        &available[index].1,
                        teleark_crypto::ManifestLimits::default(),
                    ) else {
                        continue;
                    };
                    if object.name
                        != teleark_crypto::remote_manifest_name(&manifest.public_header.package_id)
                        || manifest.metadata.parts.iter().any(|part| {
                            (part.remote_locator.account_id, part.remote_locator.chat_id)
                                != (account, chat)
                        })
                    {
                        continue;
                    }
                    selected = Some(index);
                    progress.activity()?;
                    // Legacy channels without a proof select the newest
                    // authenticated envelope; older entries keep their own
                    // health/key requirement instead of vetoing the channel.
                    break;
                }
                selected
            }
        };
        if choice.is_none() && proof.is_none() && !available.is_empty() {
            // Older channels may have no proof and only one now-damaged remote
            // manifest. Authenticate the retained envelope for this exact
            // account/channel before choosing its key; never guess a local key.
            let mut before = i64::MAX;
            for _ in 0..MAX_MANIFEST_SCAN {
                progress.check_cancelled()?;
                let records = self
                    .library
                    .worker
                    .request("vault_inventory_page", |reply| {
                        crate::StorageRequest::VaultInventoryPage {
                            account,
                            chat,
                            before,
                            reply,
                        }
                    })?;
                let Some(record) = records.into_iter().next() else {
                    break;
                };
                before = record.manifest_message_id;
                let Some(index) = available
                    .iter()
                    .position(|(key, _)| key.vault_id == record.vault_id)
                else {
                    continue;
                };
                let Ok(manifest) = teleark_crypto::open_manifest(
                    &record.sealed_manifest,
                    &available[index].1,
                    teleark_crypto::ManifestLimits::default(),
                ) else {
                    continue;
                };
                if record.remote_name
                    == teleark_crypto::remote_manifest_name(&manifest.public_header.package_id)
                    && manifest.public_header.vault_id == record.vault_id
                    && manifest.metadata.parts.iter().all(|part| {
                        (part.remote_locator.account_id, part.remote_locator.chat_id)
                            == (account, chat)
                    })
                {
                    choice = Some(index);
                    break;
                }
            }
        }
        self.check_key_generation()?;
        if let Some(scope) = initialize_empty {
            self.check_channel_setup_scope(scope, chat)?;
        }
        progress.check_cancelled()?;
        let outcome = if !stored_material_found {
            VaultKeySelection::NoKeys
        } else if let Some(index) = choice {
            let (record, master) = available.swap_remove(index);
            self.record = Some(record);
            self.master_key = Some(master);
            self.historical_key = None;
            VaultKeySelection::Ready
        } else {
            self.record = preferred.clone();
            self.master_key = None;
            self.historical_key = None;
            VaultKeySelection::Undecryptable
        };
        if outcome == VaultKeySelection::NoKeys {
            self.record = preferred;
            self.master_key = None;
            self.historical_key = None;
        }
        self.catalog.clear();
        let mut session = self
            .session
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        session.current_keys(self.session_generation)?;
        session.publish(
            self.session_generation,
            self.record.clone(),
            self.master_key.clone(),
            None,
        );
        session.status.key_selection = Some(outcome);
        if outcome == VaultKeySelection::Ready
            && pending_key.is_some_and(|marker| marker.channel_id == chat)
        {
            // If key persistence succeeded before marker cleanup (for example,
            // after a process crash), selecting that advanced epoch retires
            // the stale marker without generating another key.
            drop(session);
            self.library.clear_pending_channel_key(account, chat)?;
        }
        Ok(outcome)
    }

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
        // A damaged or mismatched stored bundle cannot stop a later key from
        // authenticating this channel. Device-store access errors still fail.
        let _ = authenticate_bundle(record, text)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
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
                Zeroizing::new(self.create_key_epoch_persisted(&password, progress, true, None)?);
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
        let session = self
            .session
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let keys = session.current_keys(self.session_generation)?;
        if matches!(action, ManagedKeyAction::Export)
            && session.status.key_selection != Some(VaultKeySelection::Ready)
        {
            return Err(ApplicationError::new(
                ApplicationErrorKind::VaultKeyUnavailable,
            ));
        }
        drop(session);
        self.record = if matches!(action, ManagedKeyAction::Export) {
            keys.record.clone()
        } else {
            self.library.worker.vault_metadata()?
        };
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
                self.create_key_epoch_persisted(&password, progress, true, None)
            }
            ManagedKeyAction::NewChannel { scope, chat_id } => {
                self.check_channel_setup_scope(scope, chat_id)?;
                let password = random_wrapping_password()?;
                let bundle = self.create_key_epoch_persisted(
                    &password,
                    progress,
                    true,
                    Some((scope, chat_id)),
                )?;
                self.check_channel_setup_scope(scope, chat_id)?;
                self.publish_channel_proof(scope.account_id, chat_id, progress)?;
                Ok(bundle)
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
                if self.master_key.is_none() {
                    return Err(ApplicationError::new(
                        ApplicationErrorKind::VaultKeyUnavailable,
                    ));
                }
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
            .map(|_| ())?;
        if vault.status().key_selection != Some(VaultKeySelection::Ready) {
            vault
                .submit_select_channel_key(1, 2, VaultKeyProgress::new())?
                .wait()?;
        }
        Ok(())
    }

    fn export(vault: &DesktopVault) -> Result<String, ApplicationError> {
        vault
            .submit_recovery_export(VaultKeyProgress::new())?
            .wait()
            .map(|secret| secret.to_string())
    }

    #[test]
    fn channel_selection_reports_missing_keys_and_selects_only_a_readable_epoch() -> TestResult {
        let temp = tempfile::tempdir()?;
        let store = platform_store();
        let (vault, _library) = open(temp.path(), store)?;
        let select = || {
            vault
                .submit_select_channel_key(1, 2, VaultKeyProgress::new())?
                .wait()
        };
        assert_eq!(select()?, VaultKeySelection::NoKeys);
        assert_eq!(
            vault.status().key_selection,
            Some(VaultKeySelection::NoKeys)
        );
        prepare(&vault)?;
        assert_eq!(select()?, VaultKeySelection::Ready);
        assert_eq!(vault.status().key_selection, Some(VaultKeySelection::Ready));
        assert!(!export(&vault)?.is_empty());
        Ok(())
    }

    #[test]
    fn empty_channel_recovers_after_failed_key_write_on_restart() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        let store = Arc::new(FailingStore {
            base: MemoryStore::default(),
            reject: AtomicBool::new(true),
            lose_writes: false,
        });
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault =
            DesktopVault::with_device_keys(telegram.clone(), library.clone(), store.clone())?;
        let scope = vault.channel_setup_scope(7)?;
        let first_progress = crate::StorageSetupProgress::new();
        let first = telegram.ensure_storage_channel_with_key_observed(
            &library,
            &vault,
            scope,
            "Synthetic private storage".into(),
            "Synthetic description".into(),
            first_progress.clone(),
        );
        assert_eq!(
            first.expect_err("device store rejects key").kind(),
            ApplicationErrorKind::PermissionDenied
        );
        assert_eq!(
            first_progress.snapshot().error,
            Some(ApplicationErrorKind::PermissionDenied)
        );
        assert_eq!(library.storage_channel_id(7)?, Some(11));
        assert!(library.worker.vault_metadata()?.is_none());
        assert!(library.pending_channel_key(7)?.is_some());
        drop(vault);

        store.reject.store(false, Ordering::Release);
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let restarted = DesktopVault::with_device_keys(telegram.clone(), library.clone(), store)?;
        let scope = restarted.channel_setup_scope(7)?;
        let rediscovered = telegram.ensure_storage_channel_with_key_observed(
            &library,
            &restarted,
            scope,
            "Synthetic private storage".into(),
            "Synthetic description".into(),
            crate::StorageSetupProgress::new(),
        )?;
        assert!(!rediscovered.created);
        assert_eq!(
            restarted
                .submit_select_or_initialize_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        let record = library.worker.vault_metadata()?.expect("new key persisted");
        assert_eq!(restarted.status().active_vault_id, Some(record.vault_id));
        assert_eq!(
            restarted
                .submit_select_or_initialize_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        assert_eq!(library.worker.vault_metadata()?, Some(record));
        assert!(library.pending_channel_key(7)?.is_none());
        Ok(())
    }

    #[test]
    fn new_channel_setup_persists_a_fresh_key_before_completion() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault = DesktopVault::with_device_keys(
            telegram.clone(),
            library.clone(),
            Arc::new(MemoryStore::default()),
        )?;
        vault
            .submit_new_managed_key(VaultKeyProgress::new())?
            .wait()?;
        let previous = library.worker.vault_metadata()?.expect("older key");
        let scope = vault.channel_setup_scope(7)?;
        let progress = crate::StorageSetupProgress::new();
        let managed = telegram.ensure_storage_channel_with_key_observed(
            &library,
            &vault,
            scope,
            "Synthetic private storage".into(),
            "Synthetic description".into(),
            progress.clone(),
        )?;
        assert!(managed.created);
        assert_eq!(managed.channel.id, 11);
        let current = library.worker.vault_metadata()?.expect("channel key");
        assert_ne!(current.vault_id, previous.vault_id);
        let snapshot = progress.snapshot();
        assert!(snapshot.finished);
        assert!(snapshot.error.is_none());
        let phases: Vec<_> = snapshot.timeline.iter().map(|(phase, _)| *phase).collect();
        assert!(phases.contains(&crate::StorageSetupPhase::CreatingKey));
        assert_eq!(phases.last(), Some(&crate::StorageSetupPhase::Completed));
        assert_eq!(library.storage_channel_id(7)?, Some(11));
        assert!(library.pending_channel_key(7)?.is_none());
        // Simulate a crash after metadata persistence but before marker cleanup.
        library.save_pending_channel_key(7, 11, Some(previous.vault_id))?;
        assert_eq!(
            vault
                .submit_select_or_initialize_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        assert_eq!(library.worker.vault_metadata()?, Some(current));
        assert!(library.pending_channel_key(7)?.is_none());
        Ok(())
    }

    #[test]
    fn empty_channel_proof_selects_its_key_after_restart_and_import() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        let store = Arc::new(MemoryStore::default());
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault =
            DesktopVault::with_device_keys(telegram.clone(), library.clone(), store.clone())?;
        let scope = vault.channel_setup_scope(7)?;
        telegram.ensure_storage_channel_with_key_observed(
            &library,
            &vault,
            scope,
            "Synthetic private storage".into(),
            "Synthetic description".into(),
            crate::StorageSetupProgress::new(),
        )?;
        assert_eq!(remote.proof_bytes().len(), 1);
        assert_eq!(remote.proof_bytes()[0].len(), 72);
        assert_eq!(
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        let channel_record = library.worker.vault_metadata()?.expect("channel key");
        let bundle = export(&vault)?;
        drop(vault);

        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let restarted =
            DesktopVault::with_device_keys(telegram.clone(), library.clone(), store.clone())?;
        let scope = restarted.channel_setup_scope(7)?;
        let discovered = telegram.ensure_storage_channel_with_key_observed(
            &library,
            &restarted,
            scope,
            "Synthetic private storage".into(),
            "Synthetic description".into(),
            crate::StorageSetupProgress::new(),
        )?;
        assert!(!discovered.created);
        assert_eq!(remote.proof_bytes().len(), 1);
        restarted
            .submit_new_managed_key(VaultKeyProgress::new())?
            .wait()?;
        assert_eq!(
            restarted
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        assert_eq!(
            restarted.status().active_vault_id,
            Some(channel_record.vault_id)
        );
        assert_eq!(export(&restarted)?, bundle);

        let other = tempfile::tempdir()?;
        let imported_library = DesktopLibrary::open(other.path().join("catalog.sqlite"))?;
        imported_library.save_storage_channel_id(7, 11)?;
        let imported = DesktopVault::with_device_keys(
            remote.connect(&other.path().join("synthetic.session")),
            imported_library,
            Arc::new(MemoryStore::default()),
        )?;
        imported
            .submit_recovery_import(bundle, VaultKeyProgress::new())?
            .wait()?;
        assert_eq!(
            imported
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        assert_eq!(
            imported.status().active_vault_id,
            Some(channel_record.vault_id)
        );
        Ok(())
    }

    #[test]
    fn legacy_single_file_corruption_retains_authenticated_inventory_and_output() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        library.save_telegram_sources(
            &crate::TelegramAccount {
                id: 7,
                display_name: "Synthetic".into(),
                username: None,
            },
            &[crate::TelegramChatSummary {
                id: 11,
                name: "Legacy".into(),
                username: None,
                kind: crate::TelegramChatKind::Channel,
                sync_pts: None,
            }],
        )?;
        let vault = DesktopVault::with_device_keys(
            telegram,
            library.clone(),
            Arc::new(MemoryStore::default()),
        )?;
        vault.submit_prepare_key(VaultKeyProgress::new())?.wait()?;
        let source = temp.path().join("original.txt");
        std::fs::write(&source, b"synthetic legacy content")?;
        let file = vault.upload_file(7, 11, source)?;
        library.cache_telegram_files(7, 11, &remote.summaries())?;
        assert!(remote.proof_bytes().is_empty());
        let output = vault.download_file(7, 11, file.package_numeric_id)?;
        remote.corrupt_manifest(file.manifest_message_id);
        assert_eq!(
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        let scan =
            vault.scan_cached_managed_files(7, 11, crate::TelegramScanCancellation::new())?;
        assert_eq!(scan.files.len(), 1);
        assert_eq!(scan.files[0].logical_name, "original.txt");
        assert_eq!(
            scan.files[0].health,
            crate::VaultFileHealth::InvalidManifest
        );
        remote.remove_manifest(file.manifest_message_id);
        assert_eq!(
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        assert_eq!(
            vault
                .scan_cached_managed_files(7, 11, crate::TelegramScanCancellation::new())?
                .files
                .len(),
            1
        );
        assert_eq!(std::fs::read(output)?, b"synthetic legacy content");
        library.save_storage_channel_id(7, 12)?;
        assert_eq!(
            vault
                .submit_select_channel_key(7, 12, VaultKeyProgress::new())?
                .wait()
                .expect_err("foreign channel denied")
                .kind(),
            ApplicationErrorKind::Authorization,
            "another channel cannot use retained evidence"
        );
        Ok(())
    }

    #[test]
    fn large_channel_file_survives_sync_restart_and_file_local_corruption() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        let keys = Arc::new(MemoryStore::default());
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        library.save_telegram_sources(
            &crate::TelegramAccount {
                id: 7,
                display_name: "Synthetic".into(),
                username: None,
            },
            &[crate::TelegramChatSummary {
                id: 11,
                name: "Synthetic storage".into(),
                username: None,
                kind: crate::TelegramChatKind::Channel,
                sync_pts: None,
            }],
        )?;
        let vault =
            DesktopVault::with_device_keys(telegram.clone(), library.clone(), keys.clone())?;
        telegram.ensure_storage_channel_with_key_observed(
            &library,
            &vault,
            vault.channel_setup_scope(7)?,
            "Synthetic storage".into(),
            "Synthetic description".into(),
            crate::StorageSetupProgress::new(),
        )?;
        let source = temp.path().join("boundary.pdf");
        remote.fail_once(crate::telegram::test_vault_remote::GateKind::ProofDownload);
        let progress = VaultKeyProgress::new();
        assert_eq!(
            vault.synchronize_channel_key(7, 11, progress.clone(), None)?,
            VaultKeySelection::Ready
        );
        assert!(
            progress
                .snapshot()
                .timeline
                .iter()
                .any(|(phase, _)| *phase == VaultKeyPhase::Waiting)
        );
        let plaintext: Vec<_> = (0..11 * 1024 * 1024 + 17)
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&source, &plaintext)?;
        let uploaded = vault.upload_file(7, 11, source)?;
        library.cache_telegram_files(7, 11, &remote.summaries())?;
        let select = || {
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()
        };
        assert_eq!(select()?, VaultKeySelection::Ready);
        let scan =
            vault.scan_cached_managed_files(7, 11, crate::TelegramScanCancellation::new())?;
        assert!(
            scan.files
                .iter()
                .any(|file| file.package_numeric_id == uploaded.package_numeric_id)
        );
        let output = vault.download_file(7, 11, uploaded.package_numeric_id)?;
        assert_eq!(std::fs::read(&output)?, plaintext);

        // Backoff never holds the key owner. An explicit lock stops the old
        // attempt instead of letting it silently reopen the session later.
        remote.fail_once(crate::telegram::test_vault_remote::GateKind::ProofDownload);
        let (waiting, ready) = mpsc::sync_channel(1);
        let observer = crate::ManagedScanObserver::new(
            11,
            Arc::new(move |state, changed| {
                if changed && state.phase == crate::ChannelSyncPhase::Waiting {
                    waiting.send(()).expect("waiting observer");
                }
            }),
        );
        let work = vault.clone();
        let blocked = std::thread::spawn(move || {
            work.synchronize_channel_key(7, 11, VaultKeyProgress::new(), Some(observer))
        });
        ready.recv_timeout(std::time::Duration::from_secs(10))?;
        vault.lock()?;
        assert_eq!(
            blocked
                .join()
                .expect("retained background task")
                .expect_err("old retry fenced")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        assert!(vault.status().locked);
        assert_eq!(select()?, VaultKeySelection::Ready);

        // A successful wire response for an old Telegram connection cannot
        // publish its key after account replacement, even before GUI cleanup.
        let (entered, release) = remote.gate(
            crate::telegram::test_vault_remote::GateKind::ProofDownload,
            0,
        );
        let work = vault.clone();
        let stale = std::thread::spawn(move || {
            work.synchronize_channel_key(7, 11, VaultKeyProgress::new(), None)
        });
        entered.recv_timeout(std::time::Duration::from_secs(10))?;
        telegram.lifecycle().publish(2, Some(8), None);
        release.send(())?;
        assert_eq!(
            stale
                .join()
                .expect("retained proof task")
                .expect_err("old account fenced")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        assert!(vault.status().locked);
        telegram.lifecycle().publish(3, Some(7), None);
        assert_eq!(select()?, VaultKeySelection::Ready);

        // A second file's damaged envelope must not revoke the channel key or
        // remove the original uploaded/downloaded file from the catalog.
        let second = temp.path().join("damaged.txt");
        std::fs::write(&second, b"synthetic second file")?;
        let damaged = vault.upload_file(7, 11, second)?;
        remote.corrupt_manifest(damaged.manifest_message_id);
        library.cache_telegram_files(7, 11, &remote.summaries())?;
        assert_eq!(select()?, VaultKeySelection::Ready);
        let scan =
            vault.scan_cached_managed_files(7, 11, crate::TelegramScanCancellation::new())?;
        assert_eq!(scan.files.len(), 2);
        assert!(
            scan.files
                .iter()
                .any(|file| file.package_numeric_id == damaged.package_numeric_id
                    && file.health == crate::VaultFileHealth::InvalidManifest)
        );
        assert!(scan.files.iter().any(|file| file.package_numeric_id
            == uploaded.package_numeric_id
            && file.health == crate::VaultFileHealth::Present));
        drop(vault);
        let restarted = DesktopVault::with_device_keys(
            remote.connect(&temp.path().join("synthetic.session")),
            library,
            keys,
        )?;
        assert_eq!(
            restarted
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        let scan =
            restarted.scan_cached_managed_files(7, 11, crate::TelegramScanCancellation::new())?;
        assert_eq!(scan.files.len(), 2);
        let restored = restarted.download_file(7, 11, uploaded.package_numeric_id)?;
        assert_eq!(std::fs::read(restored)?, plaintext);
        assert_eq!(
            std::fs::read(&output)?,
            plaintext,
            "sync and restart retain completed outputs"
        );
        // Deletion after a cached catalog read is file-local as well. The last
        // authenticated envelope remains available to explain its health.
        remote.remove_manifest(uploaded.manifest_message_id);
        assert_eq!(
            restarted
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        let scan =
            restarted.scan_cached_managed_files(7, 11, crate::TelegramScanCancellation::new())?;
        assert_eq!(scan.files.len(), 2);
        assert!(scan.files.iter().any(|file| file.package_numeric_id
            == uploaded.package_numeric_id
            && file.health == crate::VaultFileHealth::MissingManifest));
        assert_eq!(std::fs::read(output)?, plaintext);
        Ok(())
    }

    #[test]
    fn latest_damaged_proof_or_missing_credential_never_selects_an_old_key() -> TestResult {
        use crate::telegram::test_vault_remote::{GateKind, TestVaultRemote};
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        let store = Arc::new(MemoryStore::default());
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault =
            DesktopVault::with_device_keys(telegram.clone(), library.clone(), store.clone())?;
        telegram.ensure_storage_channel_with_key_observed(
            &library,
            &vault,
            vault.channel_setup_scope(7)?,
            "Synthetic private storage".into(),
            "Synthetic description".into(),
            crate::StorageSetupProgress::new(),
        )?;
        let record = library.worker.vault_metadata()?.expect("channel key");
        remote.fail_once(GateKind::ProofDownload);
        assert_eq!(
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()
                .expect_err("remote proof failure")
                .kind(),
            ApplicationErrorKind::Network
        );
        assert_eq!(library.worker.vault_metadata()?, Some(record.clone()));
        let mut damaged = remote.proof_bytes().pop().expect("proof");
        damaged[71] ^= 1;
        remote.insert_proof(damaged);
        assert_eq!(
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Undecryptable
        );
        assert_eq!(library.worker.vault_metadata()?, Some(record.clone()));
        store.0.lock().expect("store").remove(&identity(&record));
        assert_eq!(
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::NoKeys
        );
        Ok(())
    }

    #[test]
    fn interrupted_proof_publication_resumes_exact_persisted_key() -> TestResult {
        use crate::telegram::test_vault_remote::{GateKind, TestVaultRemote};
        for failure in [GateKind::ProofUpload, GateKind::ProofAcknowledgment] {
            let temp = tempfile::tempdir()?;
            let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
            let remote = TestVaultRemote::new();
            remote.fail_once(failure);
            let store = Arc::new(MemoryStore::default());
            let telegram = remote.connect(&temp.path().join("synthetic.session"));
            let vault =
                DesktopVault::with_device_keys(telegram.clone(), library.clone(), store.clone())?;
            let result = telegram.ensure_storage_channel_with_key_observed(
                &library,
                &vault,
                vault.channel_setup_scope(7)?,
                "Synthetic private storage".into(),
                "Synthetic description".into(),
                crate::StorageSetupProgress::new(),
            );
            assert_eq!(
                result.expect_err("proof publication fails").kind(),
                ApplicationErrorKind::Network
            );
            let record = library.worker.vault_metadata()?.expect("persisted key");
            assert_eq!(
                library
                    .pending_channel_key(7)?
                    .and_then(|marker| marker.target_vault_id),
                Some(record.vault_id)
            );
            vault
                .submit_new_managed_key(VaultKeyProgress::new())?
                .wait()?;
            assert_ne!(
                library
                    .worker
                    .vault_metadata()?
                    .expect("unrelated key")
                    .vault_id,
                record.vault_id
            );
            drop(vault);
            let telegram = remote.connect(&temp.path().join("synthetic.session"));
            let restarted =
                DesktopVault::with_device_keys(telegram.clone(), library.clone(), store)?;
            telegram.ensure_storage_channel_with_key_observed(
                &library,
                &restarted,
                restarted.channel_setup_scope(7)?,
                "Synthetic private storage".into(),
                "Synthetic description".into(),
                crate::StorageSetupProgress::new(),
            )?;
            assert_eq!(restarted.status().active_vault_id, Some(record.vault_id));
            assert_eq!(remote.proof_bytes().len(), 1);
            assert!(library.pending_channel_key(7)?.is_none());
            assert_eq!(
                restarted.status().key_selection,
                Some(VaultKeySelection::Ready)
            );
        }
        Ok(())
    }

    #[test]
    fn rediscovered_verified_empty_channel_creates_new_key_and_proof() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        remote.mark_storage_created();
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault = DesktopVault::with_device_keys(
            telegram.clone(),
            library.clone(),
            Arc::new(MemoryStore::default()),
        )?;
        vault
            .submit_new_managed_key(VaultKeyProgress::new())?
            .wait()?;
        let former = library.worker.vault_metadata()?.expect("former key");
        let discovered = telegram.ensure_storage_channel_with_key_observed(
            &library,
            &vault,
            vault.channel_setup_scope(7)?,
            "Synthetic private storage".into(),
            "Synthetic description".into(),
            crate::StorageSetupProgress::new(),
        )?;
        assert!(!discovered.created);
        let current = library.worker.vault_metadata()?.expect("current key");
        assert_ne!(current.vault_id, former.vault_id);
        assert_eq!(remote.proof_bytes().len(), 1);
        assert_eq!(
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        assert_eq!(vault.status().active_vault_id, Some(current.vault_id));
        Ok(())
    }

    #[test]
    fn account_switch_during_channel_discovery_cannot_admit_old_key_setup() -> TestResult {
        use crate::telegram::test_vault_remote::{GateKind, TestVaultRemote};
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        let (entered, release) = remote.gate(GateKind::StorageDiscovery, 0);
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault = DesktopVault::with_device_keys(
            telegram.clone(),
            library.clone(),
            Arc::new(MemoryStore::default()),
        )?;
        let scope = vault.channel_setup_scope(7)?;
        let progress = crate::StorageSetupProgress::new();
        let worker = {
            let library = library.clone();
            let vault = vault.clone();
            let telegram = telegram.clone();
            let progress = progress.clone();
            std::thread::spawn(move || {
                telegram.ensure_storage_channel_with_key_observed(
                    &library,
                    &vault,
                    scope,
                    "Synthetic private storage".into(),
                    "Synthetic description".into(),
                    progress,
                )
            })
        };
        entered.recv_timeout(Duration::from_secs(30))?;
        vault.lock()?;
        telegram.lifecycle().publish(2, Some(8), None);
        release.send(())?;
        let error = worker
            .join()
            .expect("setup worker")
            .expect_err("stale setup");
        assert_eq!(error.kind(), ApplicationErrorKind::Cancelled);
        assert_eq!(
            progress.snapshot().error,
            Some(ApplicationErrorKind::Cancelled)
        );
        assert!(library.worker.vault_metadata()?.is_none());
        assert!(vault.status().active_vault_id.is_none());
        Ok(())
    }

    #[test]
    fn managed_manifest_prevents_automatic_key_replacement() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        library.save_storage_channel_id(7, 11)?;
        library.save_pending_channel_key(7, 11, None)?;
        library.save_telegram_sources(
            &crate::TelegramAccount {
                id: 7,
                display_name: "Fixture".into(),
                username: None,
            },
            &[crate::TelegramChatSummary {
                id: 11,
                name: "Fixture".into(),
                username: None,
                kind: crate::TelegramChatKind::Channel,
                sync_pts: None,
            }],
        )?;
        library.cache_telegram_files(
            7,
            11,
            &[crate::TelegramFileSummary {
                message_id: 42,
                sent_at_unix_ms: 1_000,
                modified_at_unix_ms: 1_000,
                file_name: "42.v1.manifest.tam".into(),
                caption: crate::transfer::MANIFEST_CAPTION.into(),
                mime_type: None,
                size_bytes: 42,
            }],
        )?;
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault = DesktopVault::with_device_keys(
            telegram,
            library.clone(),
            Arc::new(MemoryStore::default()),
        )?;
        assert_eq!(
            vault
                .submit_select_or_initialize_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::NoKeys
        );
        assert!(library.worker.vault_metadata()?.is_none());
        Ok(())
    }

    #[test]
    fn uncached_remote_pending_upload_prevents_automatic_key_replacement() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        library.save_storage_channel_id(7, 11)?;
        library.save_pending_channel_key(7, 11, None)?;
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        telegram.upload_bytes(7, 11, "pending.tarku", remote_upload::CAPTION, vec![1])?;
        let vault = DesktopVault::with_device_keys(
            telegram,
            library.clone(),
            Arc::new(MemoryStore::default()),
        )?;
        assert_eq!(
            vault
                .submit_select_or_initialize_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::NoKeys
        );
        assert!(library.worker.vault_metadata()?.is_none());
        Ok(())
    }

    #[test]
    fn discovered_empty_channel_without_local_pending_marker_is_not_rekeyed() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        library.save_storage_channel_id(7, 11)?;
        let remote = TestVaultRemote::new();
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault = DesktopVault::with_device_keys(
            telegram,
            library.clone(),
            Arc::new(MemoryStore::default()),
        )?;
        assert_eq!(
            vault
                .submit_select_or_initialize_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::NoKeys
        );
        assert!(library.worker.vault_metadata()?.is_none());
        Ok(())
    }

    #[test]
    fn damaged_former_key_does_not_block_selection_of_current_key() -> TestResult {
        let temp = tempfile::tempdir()?;
        let store = Arc::new(MemoryStore::default());
        let (vault, library) = open(temp.path(), store.clone())?;
        prepare(&vault)?;
        let former = library.worker.vault_metadata()?.expect("former key");
        vault
            .submit_new_managed_key(VaultKeyProgress::new())?
            .wait()?;
        let current = library.worker.vault_metadata()?.expect("current key");
        assert_ne!(former.vault_id, current.vault_id);
        store.0.lock().expect("store").insert(
            identity(&former),
            Zeroizing::new(b"damaged bundle".to_vec()),
        );
        assert_eq!(
            vault
                .submit_select_channel_key(1, 2, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        assert_eq!(vault.status().active_vault_id, Some(current.vault_id));
        store.0.lock().expect("store").remove(&identity(&current));
        assert_eq!(
            vault
                .submit_select_channel_key(1, 2, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Undecryptable,
            "a damaged stored key exists, even though it cannot be used"
        );
        Ok(())
    }

    #[test]
    fn channel_manifest_selects_matching_former_key_and_rejects_missing_match() -> TestResult {
        use crate::telegram::test_vault_remote::TestVaultRemote;
        let temp = tempfile::tempdir()?;
        let store = Arc::new(MemoryStore::default());
        let library = DesktopLibrary::open(temp.path().join("catalog.sqlite"))?;
        let remote = TestVaultRemote::new();
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault = DesktopVault::with_device_keys(telegram, library.clone(), store.clone())?;
        prepare(&vault)?;
        let former = library.worker.vault_metadata()?.expect("former key");
        let source = temp.path().join("file.bin");
        std::fs::write(&source, b"single-key channel fixture")?;
        vault.upload_file(7, 11, source)?;
        library.save_telegram_sources(
            &crate::TelegramAccount {
                id: 7,
                display_name: "Fixture".into(),
                username: None,
            },
            &[crate::TelegramChatSummary {
                id: 11,
                name: "Fixture".into(),
                username: None,
                kind: crate::TelegramChatKind::Channel,
                sync_pts: None,
            }],
        )?;
        library.cache_telegram_files(7, 11, &remote.summaries())?;
        assert!(
            !library.cached_manifest_candidates(7, 11)?.is_empty(),
            "uploaded manifest must be visible in the channel catalog"
        );
        vault
            .submit_new_managed_key(VaultKeyProgress::new())?
            .wait()?;
        assert_ne!(
            library.worker.vault_metadata()?.expect("new key").vault_id,
            former.vault_id
        );
        assert_eq!(
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Ready
        );
        assert_eq!(vault.status().active_vault_id, Some(former.vault_id));
        store.0.lock().expect("store").remove(&identity(&former));
        assert_eq!(
            vault
                .submit_select_channel_key(7, 11, VaultKeyProgress::new())?
                .wait()?,
            VaultKeySelection::Undecryptable
        );
        assert!(vault.status().active_key_locked);
        Ok(())
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
        vault
            .submit_select_channel_key(1, 2, VaultKeyProgress::new())?
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
        vault
            .submit_select_channel_key(1, 2, VaultKeyProgress::new())?
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
        vault
            .submit_select_channel_key(1, 2, VaultKeyProgress::new())?
            .wait()?;
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
