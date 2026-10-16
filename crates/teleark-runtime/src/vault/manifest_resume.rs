//! Complete a persisted manifest without reopening the original plaintext file.
use super::*;
use teleark_storage::{Database, VaultJobLease, VaultJobRecord, VaultJobState, VaultJobTransition};

impl VaultOwner {
    pub(super) fn resume_manifest_upload(
        &mut self,
        database: &mut Database,
        record: &VaultJobRecord,
        context: &crate::VaultRecoveryContext,
    ) -> Result<ManagedVaultFile, ApplicationError> {
        let account = context.account_id;
        let chat = context.chat_id;
        let id = context.task_id;
        self.telegram.validate_storage_channel(account, chat)?;
        let master = self
            .master_key
            .clone()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::VaultKeyUnavailable))?;
        let started = Instant::now();
        let now = now_unix_ms()?;
        let mut log = TransferSessionLog::create(&self.library, TransferSessionKind::Vault, id)?;
        let telemetry =
            transfer_controller(true, 0, teleark_telegram::TransferTuning::default())?.snapshot();
        log.append_started(
            true,
            now,
            context.size_bytes,
            u32::try_from(
                encrypted_part_sizes(context.size_bytes)
                    .map_err(map_transfer_error)?
                    .len(),
            )
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?,
            &telemetry,
        )?;
        let mut lease = VaultJobLease {
            account_id: account,
            id,
            generation: record.generation,
        };
        if !database
            .transition_vault_job(
                lease,
                VaultJobState::Queued,
                VaultJobTransition::Start,
                now,
                None,
            )
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        lease.generation = lease
            .generation
            .checked_add(1)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Capacity))?;
        self.update_transfer(id, |row| {
            row.state = VaultTransferState::Running;
            row.recovery_state = Some(VaultJobState::Running);
            row.session_log_path = Some(log.path.clone());
        });
        let mut result = (|| {
            let registration = self.register_transfer(lease)?;
            let observer = Arc::new(VaultUploadObserver::new(self.transfers.clone(), id));
            observer.phase(VaultUploadPhase::Publishing);
            let store = TelegramObjectStore::new(self.telegram.clone(), account, chat)
                .with_observer(observer.clone())
                .with_cancellation(registration.cancellation.clone());
            let mut worker = crate::DurableUploadParts::open(database, lease, store, &master)
                .map_err(map_transfer_error)?
                .with_cancellation(registration.cancellation.clone());
            let object = worker
                .resume_saved_manifest(database, &master)
                .map_err(map_transfer_error)?
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
            observer.phase(VaultUploadPhase::Persisting);
            let envelope = worker
                .into_transport()
                .take_published_manifest_envelope()
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            let manifest = teleark_crypto::open_manifest(
                &envelope,
                &master,
                teleark_crypto::ManifestLimits::default(),
            )
            .map_err(map_crypto_error)?;
            let inventory = teleark_storage::VaultInventoryRecord {
                account_id: account,
                chat_id: chat,
                manifest_message_id: object.object_id as i64,
                remote_name: object.name.clone(),
                vault_id: manifest.public_header.vault_id,
                sealed_manifest: envelope,
                observed_at_unix_ms: now_unix_ms()?,
                manifest_invalid: false,
            };
            self.library
                .worker
                .request("save_resumed_upload_manifest", |reply| {
                    crate::StorageRequest::SaveVaultInventory {
                        record: inventory,
                        reply,
                    }
                })?;
            // The manifest receipt was verified. Part availability is not
            // re-probed here, so retain the default unknown health assessment.
            let file = managed_file_from_recovered(&crate::RecoveredManifest {
                object: object.clone(),
                manifest,
            })?;
            self.catalog.remember_receipt(
                (account, chat),
                package_id_from_bytes(context.package_id)
                    .map_err(map_transfer_error)?
                    .get(),
                object,
            );
            Ok::<_, ApplicationError>(file)
        })();
        let terminal = control::finish_job(
            database,
            lease,
            result.as_ref().err().map(ApplicationError::kind),
        );
        match &terminal {
            Ok(VaultJobState::Paused | VaultJobState::Cancelled) => {
                result = Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            Err(error) => result = Err(error.clone()),
            _ => {}
        }
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.update_transfer(id, |row| {
            row.recovery_state = terminal.as_ref().ok().copied();
            row.state = match terminal {
                Ok(VaultJobState::Completed) => VaultTransferState::Completed,
                Ok(VaultJobState::Paused) => VaultTransferState::Paused,
                Ok(VaultJobState::Cancelled) => VaultTransferState::Cancelled,
                _ => VaultTransferState::Failed(
                    result
                        .as_ref()
                        .err()
                        .map_or(ApplicationErrorKind::Persistence, ApplicationError::kind),
                ),
            };
            row.duration_ms = Some(elapsed);
            row.average_bytes_per_second = None;
            if let Ok(file) = &result {
                row.package_id = Some(file.package_id.clone());
                row.transferred_bytes = row.size_bytes;
                row.completed_parts = row.part_count;
            }
        });
        let _ = log.append_finished(
            elapsed,
            result.as_ref().err().map(ApplicationError::kind),
            &telemetry,
        );
        self.persist_upload_id(id)?;
        result
    }
}
