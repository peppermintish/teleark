//! Actual pending-source recovery, before a file has an encryption checkpoint.
use super::*;
use teleark_storage::{
    Database, PendingVaultUploadSnapshot, PendingVaultUploadState as S, VaultJobLease,
    VaultJobState, VaultJobTransition as T,
};

impl VaultOwner {
    pub(super) fn restore_pending_upload_rows(
        &self,
        db: &Database,
        account: i64,
        rows: &mut Vec<VaultTransferSnapshot>,
    ) -> Result<u64, ApplicationError> {
        let (ids, count) = db
            .pending_vault_upload_history_ids(account, 256)
            .map_err(persistence)?;
        let mut added = 0;
        for id in ids {
            let Some(saved) = db.pending_vault_upload(account, id).map_err(persistence)? else {
                continue;
            };
            if saved.state == S::Promoted {
                continue;
            }
            let row = match pending_snapshot(&saved) {
                Ok(row) => row,
                Err(_) => VaultTransferSnapshot {
                    recovery_state: None,
                    restored: true,
                    upload_activity: None,
                    id,
                    account_id: account,
                    chat_id: saved.record.chat_id,
                    batch_id: Some(saved.record.batch_id),
                    queued_at_unix_ms: saved.record.created_at_unix_ms,
                    direction: VaultTransferDirection::Upload,
                    file_name: String::new(),
                    package_id: None,
                    size_bytes: 0,
                    transferred_bytes: 0,
                    completed_parts: 0,
                    part_count: 0,
                    started_at_unix_ms: 0,
                    duration_ms: None,
                    average_bytes_per_second: None,
                    destination: None,
                    session_log_path: None,
                    telemetry: transfer_controller(
                        true,
                        0,
                        teleark_telegram::TransferTuning::default(),
                    )?
                    .snapshot(),
                    state: VaultTransferState::Failed(ApplicationErrorKind::InvalidRequest),
                },
            };
            if let Some(existing) = rows
                .iter_mut()
                .find(|row| row.id == id && row.account_id == account)
            {
                *existing = row;
            } else {
                rows.push(row);
            }
            added += 1;
        }
        Ok(count.saturating_sub(added))
    }

    pub(super) fn resume_pending_upload(
        &mut self,
        account: i64,
        id: u64,
        automatic: bool,
    ) -> Result<ManagedVaultFile, ApplicationError> {
        let (revision, current, _) = self.telegram.lifecycle().snapshot();
        if current != Some(account) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        if self.master_key.is_none() {
            return Err(ApplicationError::new(
                ApplicationErrorKind::VaultKeyUnavailable,
            ));
        }
        let mut db = Database::open(self.library.database_path.as_ref()).map_err(persistence)?;
        let mut saved = db
            .pending_vault_upload(account, id)
            .map_err(persistence)?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        if saved.state == S::Promoted {
            if db.vault_job(account, id).map_err(persistence)?.is_some() {
                return self.resume_upload(account, id);
            }
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let context = crate::VaultPendingUploadContext::from_record(&saved.record)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        if automatic && saved.state != S::Queued {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        let action = match saved.state {
            S::Queued => None,
            S::Paused => Some(T::Resume),
            S::Retryable => Some(T::Retry),
            _ => return Err(ApplicationError::new(ApplicationErrorKind::Conflict)),
        };
        if let Some(action) = action {
            if !db
                .transition_pending_vault_upload(
                    VaultJobLease {
                        account_id: account,
                        id,
                        generation: saved.generation,
                    },
                    saved.state,
                    action,
                    None,
                )
                .map_err(persistence)?
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            saved.generation += 1;
            saved.state = S::Queued;
            saved.failure_code = None;
        }
        let mut row = pending_snapshot(&saved)?;
        row.restored = false;
        row.upload_activity = Some(VaultUploadActivity::new(VaultUploadPhase::CheckingStorage));
        if self.transfers.get(id).is_some() {
            self.update_transfer(id, |existing| *existing = row);
        } else {
            self.push_transfer(row)?;
        }
        let plan = QueuedUpload {
            id,
            batch_id: context.batch_id,
            queued_at: context.created_at_unix_ms as i64,
            generation: saved.generation,
            source: VaultUploadSource {
                path: context.source.clone(),
                file_name: context.file_name.clone(),
                size_bytes: context.identity.size_bytes,
                modified: UNIX_EPOCH.checked_add(std::time::Duration::from_nanos(
                    context.identity.modified_at_units,
                )),
            },
            pending: context,
        };
        let result = (|| {
            let (current_revision, current, _) = self.telegram.lifecycle().snapshot();
            if revision != current_revision || current != Some(account) {
                return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
            }
            if self.record.as_ref().map(|record| record.vault_id) != Some(plan.pending.vault_id)
                || plan.pending.master_key_generation != 1
            {
                return Err(ApplicationError::new(
                    ApplicationErrorKind::VaultKeyUnavailable,
                ));
            }
            // Restart is a new selection boundary: recheck complete uniqueness,
            // then the actual upload checks current target/source before hashing.
            let registration = self.upload_controls.register(VaultJobLease {
                account_id: account,
                id,
                generation: plan.generation,
            })?;
            let current = db
                .pending_vault_upload(account, id)
                .map_err(persistence)?
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
            if current.generation != plan.generation || current.state != S::Queued {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            self.telegram
                .validate_storage_channel(account, plan.pending.chat_id)?;
            drop(registration);
            self.upload(
                account,
                plan.pending.chat_id,
                &plan.source.path,
                Some(&plan),
                None,
            )
        })();
        if let Err(error) = &result {
            self.settle_pending_upload(&plan, error.kind())?;
            if let Some(current) = db.pending_vault_upload(account, id).map_err(persistence)?
                && current.state != S::Promoted
            {
                let row = pending_snapshot(&current)?;
                self.update_transfer(id, |existing| *existing = row);
                self.persist_upload_id(id)?;
            }
        }
        result
    }
}

fn persistence(_: teleark_storage::StorageError) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Persistence)
}

pub(super) fn pending_snapshot(
    saved: &PendingVaultUploadSnapshot,
) -> Result<VaultTransferSnapshot, ApplicationError> {
    let context = crate::VaultPendingUploadContext::from_record(&saved.record)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    let (state, recovery) = match saved.state {
        S::Queued => (VaultTransferState::Queued, VaultJobState::Queued),
        S::Paused => (VaultTransferState::Paused, VaultJobState::Paused),
        S::Cancelled => (VaultTransferState::Cancelled, VaultJobState::Cancelled),
        S::Retryable | S::Blocked => (
            VaultTransferState::Failed(
                saved
                    .failure_code
                    .as_deref()
                    .and_then(upload_history::parse_error)
                    .unwrap_or(ApplicationErrorKind::InvalidRequest),
            ),
            if saved.state == S::Retryable {
                VaultJobState::Retryable
            } else {
                VaultJobState::Blocked
            },
        ),
        S::Promoted => return Err(ApplicationError::new(ApplicationErrorKind::Conflict)),
    };
    Ok(VaultTransferSnapshot {
        recovery_state: Some(recovery),
        restored: true,
        upload_activity: None,
        id: saved.record.id,
        account_id: saved.record.account_id,
        chat_id: saved.record.chat_id,
        batch_id: Some(saved.record.batch_id),
        queued_at_unix_ms: saved.record.created_at_unix_ms,
        direction: VaultTransferDirection::Upload,
        file_name: context.file_name,
        package_id: None,
        size_bytes: context.identity.size_bytes,
        transferred_bytes: 0,
        completed_parts: 0,
        part_count: 0,
        started_at_unix_ms: 0,
        duration_ms: None,
        average_bytes_per_second: None,
        destination: None,
        session_log_path: None,
        telemetry: transfer_controller(true, 0, teleark_telegram::TransferTuning::default())?
            .snapshot(),
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_batch_stop_persists_without_live_batch_and_preserves_current_preparation() {
        let dir = tempfile::tempdir().expect("directory");
        let owner = owner(dir.path());
        let mut db = Database::open(owner.library.database_path.as_ref()).expect("database");
        let records = (1..=3)
            .map(|id| pending(dir.path(), id).admission_record().expect("record"))
            .collect::<Vec<_>>();
        db.admit_pending_vault_uploads(&records).expect("admission");
        for id in 1..=3 {
            let saved = db
                .pending_vault_upload(7, id)
                .expect("read")
                .expect("saved");
            owner
                .push_transfer(pending_snapshot(&saved).expect("row"))
                .expect("projection");
        }
        let formal = teleark_storage::VaultJobRecord {
            account_id: 7,
            id: 3,
            chat_id: 11,
            direction: teleark_storage::VaultJobDirection::Upload,
            package_id: [1; 16],
            context_version: 1,
            context: vec![1, 2, 3],
            state: VaultJobState::Queued,
            generation: 0,
            created_at_unix_ms: 100,
            updated_at_unix_ms: 100,
            failure_code: None,
        };
        // The control path only needs ledger identity/state, never decryption.
        assert!(
            db.promote_pending_vault_upload(&records[2], 0, &formal)
                .expect("promote queued member")
        );
        let preparing = owner
            .upload_controls
            .register(VaultJobLease {
                account_id: 7,
                id: 1,
                generation: 0,
            })
            .expect("preparation");
        assert!(owner.active_upload_batch.lock().expect("batch").is_none());
        assert_eq!(
            owner
                .stop_saved_upload_batch(8, 13)
                .expect_err("account fence")
                .kind(),
            ApplicationErrorKind::Authorization
        );
        owner
            .stop_saved_upload_batch(7, 13)
            .expect("stop restored batch");
        assert!(!preparing.cancellation.is_cancelled());
        assert_eq!(
            owner.transfers.get(1).expect("current").state,
            VaultTransferState::Queued
        );
        for id in [2, 3] {
            assert_eq!(
                owner.transfers.get(id).expect("visible cancellation").state,
                VaultTransferState::Cancelled
            );
        }
        drop(preparing);
        drop(owner);
        drop(db);
        let restored = self::owner(dir.path());
        let db = Database::open(restored.library.database_path.as_ref()).expect("reopen");
        assert_eq!(
            db.vault_job(7, 3)
                .expect("formal ledger")
                .expect("formal member")
                .state,
            VaultJobState::Cancelled
        );
        assert_eq!(
            db.pending_vault_upload(7, 2)
                .expect("read")
                .expect("durable")
                .state,
            S::Cancelled
        );
        restored
            .stop_saved_upload_batch(7, 13)
            .expect("no current owner after restart");
        assert_eq!(
            db.pending_vault_upload(7, 1)
                .expect("read")
                .expect("queued before stop")
                .state,
            S::Cancelled
        );
    }

    fn owner(path: &std::path::Path) -> VaultOwner {
        let library = DesktopLibrary::open(path.join("catalog.sqlite")).expect("library");
        let telegram =
            DesktopTelegram::open_direct(path.join("synthetic.session")).expect("telegram");
        telegram.lifecycle().publish(1, Some(7), None);
        VaultOwner {
            catalog: catalog::ManifestCache::default(),
            catalog_key_revision: 0,
            telegram,
            library,
            record: Some(VaultMetadataRecord {
                vault_id: [3; 16],
                password_wrap: vec![],
                recovery_wrap: vec![],
                password_generation: 1,
                recovery_generation: 1,
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
            }),
            master_key: Some(Arc::new(VaultMasterKey::from_bytes([7; 32]))),
            historical_key: None,
            health_worker: None,
            session: Arc::new(Mutex::new(VaultSession::new(None))),
            session_generation: 0,
            transfers: Arc::new(TransferSnapshots::new(Vec::new()).expect("snapshots")),
            active_upload_batch: Arc::new(Mutex::new(None)),
            upload_controls: UploadControls::default(),
        }
    }
    fn pending(path: &std::path::Path, id: u64) -> crate::VaultPendingUploadContext {
        let source = path.join("source.bin");
        if !source.exists() {
            std::fs::write(&source, b"abcdefgh").expect("source");
        }
        let mut context = crate::VaultPendingUploadContext {
            account_id: 7,
            task_id: id,
            chat_id: 11,
            batch_id: 13,
            created_at_unix_ms: 100,
            vault_id: [3; 16],
            master_key_generation: 1,
            file_name: format!("source-{id}.bin"),
            source: source.clone(),
            identity: teleark_transfer::SourceIdentity {
                filesystem_id: 0,
                size_bytes: 8,
                modified_at_units: 0,
                revision: 0,
            },
        };
        context.inspect_source(&source).expect("metadata only");
        context
    }

    #[test]
    fn locked_pending_controls_persist_intent_and_signal_preflight_without_formal_job() {
        for action in [
            control::VaultUploadControl::Pause,
            control::VaultUploadControl::Cancel,
        ] {
            let dir = tempfile::tempdir().expect("directory");
            let mut owner = owner(dir.path());
            owner.master_key = None;
            let mut db = Database::open(owner.library.database_path.as_ref()).expect("database");
            let context = pending(dir.path(), 1);
            db.admit_pending_vault_uploads(&[context.admission_record().expect("context")])
                .expect("admit");
            let snapshot = db
                .pending_vault_upload(7, 1)
                .expect("read")
                .expect("queued");
            owner
                .push_transfer(pending_snapshot(&snapshot).expect("projection"))
                .expect("visible");
            let registration = owner
                .upload_controls
                .register(VaultJobLease {
                    account_id: 7,
                    id: 1,
                    generation: 0,
                })
                .expect("preflight owner");
            owner
                .control_upload(7, 1, action)
                .expect("independent durable control");
            let saved = db
                .pending_vault_upload(7, 1)
                .expect("saved")
                .expect("record");
            assert_eq!(
                saved.state,
                if action == control::VaultUploadControl::Pause {
                    S::Paused
                } else {
                    S::Cancelled
                }
            );
            assert_eq!(saved.generation, 1);
            assert!(registration.cancellation.is_cancelled());
            assert!(db.vault_job(7, 1).expect("formal").is_none());
            assert_eq!(
                owner.transfers.get(1).expect("visible stop").recovery_state,
                Some(if action == control::VaultUploadControl::Pause {
                    VaultJobState::Paused
                } else {
                    VaultJobState::Cancelled
                })
            );
            owner
                .control_upload(7, 1, action)
                .expect("idempotent control");
            assert_eq!(
                db.pending_vault_upload(7, 1)
                    .expect("read")
                    .expect("record")
                    .generation,
                1
            );
        }
    }

    #[test]
    fn automatic_pending_resume_preserves_pause_and_manual_resume_retains_failed_context() {
        let dir = tempfile::tempdir().expect("directory");
        let mut owner = owner(dir.path());
        let mut db = Database::open(owner.library.database_path.as_ref()).expect("database");
        let context = pending(dir.path(), 1);
        db.admit_pending_vault_uploads(&[context.admission_record().expect("context")])
            .expect("admit");
        db.transition_pending_vault_upload(
            VaultJobLease {
                account_id: 7,
                id: 1,
                generation: 0,
            },
            S::Queued,
            T::RequestPause,
            None,
        )
        .expect("pause");
        let error = owner
            .resume_pending_upload(7, 1, true)
            .expect_err("startup cannot undo pause");
        assert_eq!(error.kind(), ApplicationErrorKind::Cancelled);
        assert_eq!(
            db.pending_vault_upload(7, 1)
                .expect("read")
                .expect("paused")
                .generation,
            1
        );
        let error = owner
            .resume_upload(7, 1)
            .expect_err("synthetic transport has no authorized account");
        assert_eq!(error.kind(), ApplicationErrorKind::Authorization);
        let saved = db
            .pending_vault_upload(7, 1)
            .expect("read")
            .expect("retryable");
        assert_eq!(
            saved.record,
            context.admission_record().expect("original scope")
        );
        assert_eq!(saved.state, S::Retryable);
        assert_eq!(saved.generation, 3);
        assert_eq!(
            owner
                .transfers
                .get(1)
                .expect("visible guidance")
                .recovery_state,
            Some(VaultJobState::Retryable)
        );
        assert_eq!(
            std::fs::read(context.source).expect("source preserved"),
            b"abcdefgh"
        );
        assert!(db.vault_job(7, 1).expect("formal").is_none());
    }

    #[test]
    fn locked_history_includes_unmaterialized_pending_work_and_deduplicates_legacy_rows() {
        let dir = tempfile::tempdir().expect("directory");
        let mut owner = owner(dir.path());
        let mut db = Database::open(owner.library.database_path.as_ref()).expect("database");
        let mut records = Vec::new();
        for id in 1..=300 {
            records.push(pending(dir.path(), id).admission_record().expect("record"));
        }
        for chunk in records.chunks(128) {
            db.admit_pending_vault_uploads(chunk).expect("all windows");
        }
        for id in 2..=300 {
            db.transition_pending_vault_upload(
                VaultJobLease {
                    account_id: 7,
                    id,
                    generation: 0,
                },
                S::Queued,
                T::RequestCancel,
                None,
            )
            .expect("cancel");
        }
        let duplicate = pending_snapshot(
            &db.pending_vault_upload(7, 300)
                .expect("read")
                .expect("last"),
        )
        .expect("snapshot");
        owner
            .persist_upload(&duplicate)
            .expect("compatibility history");
        assert_eq!(
            db.vault_transfer_history_count(7).expect("union count"),
            300
        );
        owner.master_key = None;
        owner.restore_upload_history(7).expect("locked hydration");
        let view = owner.transfers.view().expect("view");
        assert_eq!(view.items.len(), 256);
        assert_eq!(view.omitted_items, 44);
        assert_eq!(
            owner
                .transfers
                .get(1)
                .expect("active work survives terminal history")
                .recovery_state,
            Some(VaultJobState::Queued)
        );
        assert_eq!(
            owner
                .transfers
                .get(300)
                .expect("not materialized before restart")
                .recovery_state,
            Some(VaultJobState::Cancelled)
        );
        assert_eq!(view.items.iter().filter(|row| row.id == 300).count(), 1);
    }
    #[test]
    fn control_hands_off_to_formal_ledger_when_promotion_wins_pending_lookup() {
        let dir = tempfile::tempdir().expect("directory");
        let owner = owner(dir.path());
        let mut db = Database::open(owner.library.database_path.as_ref()).expect("database");
        let pending = pending(dir.path(), 1);
        let record = pending.admission_record().expect("pending");
        db.admit_pending_vault_uploads(std::slice::from_ref(&record))
            .expect("admit");
        owner
            .push_transfer(
                pending_snapshot(
                    &db.pending_vault_upload(7, 1)
                        .expect("read")
                        .expect("record"),
                )
                .expect("projection"),
            )
            .expect("visible");
        let package = [1; 16];
        let context = crate::VaultRecoveryContext {
            account_id: 7,
            task_id: 1,
            chat_id: 11,
            package_id: package,
            vault_id: [3; 16],
            master_key_generation: 1,
            file_key_wrap: teleark_crypto::wrap_file_key(
                &VaultMasterKey::from_bytes([7; 32]),
                &teleark_crypto::FileKey::from_bytes([4; 32]),
                &[3; 16],
                &package,
                1,
                1,
                &mut AeadUsageRegistry::new(),
            )
            .expect("wrap"),
            file_name: pending.file_name.clone(),
            created_at_unix_ms: 100,
            size_bytes: 8,
            direction: crate::VaultRecoveryDirection::Upload {
                source: pending.source.clone(),
                identity: pending.identity,
                source_blake3: *blake3::hash(b"abcdefgh").as_bytes(),
            },
        };
        pending.verify_executable(&context).expect("bound source");
        assert!(
            db.promote_pending_vault_upload(
                &record,
                0,
                &context.admission_record().expect("formal")
            )
            .expect("promoted")
        );
        // Simulate promotion immediately after control's first formal lookup.
        owner
            .control_pending_upload(&mut db, 7, 1, control::VaultUploadControl::Pause)
            .expect("handoff");
        assert_eq!(
            db.vault_job(7, 1).expect("formal").expect("job").state,
            VaultJobState::Paused
        );
        assert_eq!(
            db.pending_vault_upload(7, 1)
                .expect("pending")
                .expect("receipt")
                .state,
            S::Promoted
        );
        assert_eq!(
            owner.transfers.get(1).expect("projection").state,
            VaultTransferState::Paused
        );
    }
    #[test]
    fn startup_pending_dispatch_is_account_scoped_and_does_not_resume_paused_tasks() {
        let dir = tempfile::tempdir().expect("directory");
        let mut owner = owner(dir.path());
        let mut db = Database::open(owner.library.database_path.as_ref()).expect("database");
        let first = pending(dir.path(), 1);
        let paused = pending(dir.path(), 2);
        let mut foreign = pending(dir.path(), 3);
        foreign.account_id = 8;
        db.admit_pending_vault_uploads(&[
            first.admission_record().expect("first"),
            paused.admission_record().expect("paused"),
            foreign.admission_record().expect("foreign"),
        ])
        .expect("admit");
        db.transition_pending_vault_upload(
            VaultJobLease {
                account_id: 7,
                id: 2,
                generation: 0,
            },
            S::Queued,
            T::RequestPause,
            None,
        )
        .expect("pause");
        let report = owner.resume_queued_jobs(7, true).expect("startup dispatch");
        assert_eq!(report.resumed, 0);
        assert_eq!(report.failed, 1);
        assert_eq!(
            db.pending_vault_upload(7, 1)
                .expect("read")
                .expect("attempted")
                .state,
            S::Retryable
        );
        assert_eq!(
            db.pending_vault_upload(7, 2)
                .expect("read")
                .expect("preserved pause")
                .state,
            S::Paused
        );
        assert_eq!(
            db.pending_vault_upload(8, 3)
                .expect("read")
                .expect("foreign unchanged")
                .generation,
            0
        );
    }
}
