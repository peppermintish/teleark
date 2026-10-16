//! Durable summaries on acknowledged owner-thread boundaries; never on progress callbacks.
use super::*;
use teleark_storage::{StoredVaultUploadState as State, VaultUploadRecord};

impl VaultOwner {
    pub(super) fn admit_upload_window(
        &self,
        rows: Vec<VaultTransferSnapshot>,
    ) -> Result<(), ApplicationError> {
        for row in &rows {
            if !self
                .transfers
                .insert_pruning(row.clone(), vault_transfer_evictions)
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
            }
        }
        self.persist_upload_rows(&rows)
    }
    pub(super) fn fail_pending_upload_window(&self, account: i64, kind: ApplicationErrorKind) {
        let ids = self
            .transfers
            .fold(Vec::new(), |mut ids, row| {
                if row.account_id == account
                    && row.direction == VaultTransferDirection::Upload
                    && row.state == VaultTransferState::Queued
                {
                    ids.push(row.id);
                }
                ids
            })
            .unwrap_or_default();
        let mut rows = Vec::new();
        for id in ids {
            self.update_transfer(id, |row| row.state = VaultTransferState::Failed(kind));
            if let Some(row) = self.transfers.get(id) {
                rows.push(row);
            }
        }
        if !rows.is_empty() {
            let _ = self.persist_upload_rows(&rows);
        }
    }
    fn persist_upload_rows(&self, rows: &[VaultTransferSnapshot]) -> Result<(), ApplicationError> {
        let records = rows.iter().map(record).collect();
        let result = self.library.worker.request("save_upload_history", |reply| {
            crate::StorageRequest::SaveVaultUploads { records, reply }
        });
        if result.is_err() {
            for row in rows {
                self.update_transfer(row.id, |row| {
                    row.state = VaultTransferState::Failed(ApplicationErrorKind::Persistence)
                });
            }
        }
        result
    }
    pub(super) fn persist_upload(
        &self,
        snapshot: &VaultTransferSnapshot,
    ) -> Result<(), ApplicationError> {
        self.persist_upload_rows(std::slice::from_ref(snapshot))
    }

    pub(super) fn finish_upload(
        &self,
        id: u64,
        started: Instant,
        package: Option<String>,
    ) -> Result<(), ApplicationError> {
        let mut row = self
            .transfers
            .get(id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let duration = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        row.duration_ms = Some(duration);
        row.average_bytes_per_second = row
            .transferred_bytes
            .saturating_mul(1_000)
            .checked_div(duration.max(1));
        if let Some(package) = package {
            row.package_id = Some(package);
        }
        if row.state == VaultTransferState::Running {
            row.state = VaultTransferState::Completed;
        }
        if let Some(outcome) = super::transfer_terminal_outcome(row.state) {
            super::append_terminal_activity(&mut row, outcome);
        }
        self.persist_upload(&row)?;
        self.update_transfer(id, |current| *current = row);
        Ok(())
    }
    pub(super) fn persist_upload_id(&self, id: u64) -> Result<(), ApplicationError> {
        let snapshot = self
            .transfers
            .get(id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        self.persist_upload(&snapshot)
    }
    pub(super) fn restore_upload_history(&self, account: i64) -> Result<(), ApplicationError> {
        let history = self
            .library
            .worker
            .request("restore_upload_history", |reply| {
                crate::StorageRequest::VaultUploadHistory { account, reply }
            })?;
        let mut rows = history
            .records
            .into_iter()
            .map(snapshot)
            .collect::<Result<Vec<_>, _>>()?;
        let database = teleark_storage::Database::open(self.library.database_path.as_ref())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let retained: std::collections::BTreeSet<_> = rows.iter().map(|row| row.id).collect();
        let formal_ids = database
            .vault_job_history_ids(account, teleark_storage::VaultJobDirection::Upload, 256)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        for id in formal_ids.into_iter().filter(|id| !retained.contains(id)) {
            let job = match database.vault_job(account, id) {
                Ok(Some(job)) => Some(job),
                Ok(None) => continue,
                Err(teleark_storage::StorageError::CorruptData { .. }) => None,
                Err(_) => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
            };
            let mut row = orphan_upload_snapshot(account, id, job.as_ref())?;
            let receipt = match database.pending_vault_upload(account, id) {
                Ok(receipt) => receipt,
                Err(teleark_storage::StorageError::CorruptData { .. }) => None,
                Err(_) => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
            };
            if let Some(receipt) = receipt
                && let Ok(pending) = crate::VaultPendingUploadContext::from_record(&receipt.record)
                && let Some(job) = job.as_ref()
                && let Ok(context) = crate::VaultRecoveryContext::from_record(job)
                && pending.verify_executable(&context).is_ok()
            {
                row.batch_id = Some(receipt.record.batch_id);
            }
            rows.push(row);
        }
        for row in &mut rows {
            let job = match database.vault_job(account, row.id) {
                Ok(job) => job,
                Err(teleark_storage::StorageError::CorruptData { .. }) => None,
                Err(_) => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
            };
            if let Some(job) = job
                && let Ok(context) = crate::VaultRecoveryContext::from_record(&job)
                && matches!(
                    context.direction,
                    crate::VaultRecoveryDirection::Upload { .. }
                )
                && context.chat_id == row.chat_id
                && context.file_name == row.file_name
                && context.size_bytes == row.size_bytes
            {
                row.recovery_state = Some(job.state);
                row.state = match job.state {
                    teleark_storage::VaultJobState::Queued => VaultTransferState::Queued,
                    teleark_storage::VaultJobState::Running => VaultTransferState::Running,
                    teleark_storage::VaultJobState::Pausing => VaultTransferState::Pausing,
                    teleark_storage::VaultJobState::Cancelling => VaultTransferState::Cancelling,
                    teleark_storage::VaultJobState::Paused => VaultTransferState::Paused,
                    teleark_storage::VaultJobState::Cancelled => VaultTransferState::Cancelled,
                    teleark_storage::VaultJobState::Completed => {
                        if row.state != VaultTransferState::Completed {
                            row.average_bytes_per_second = None;
                            row.duration_ms = None;
                        }
                        row.transferred_bytes = context.size_bytes;
                        row.part_count = u32::try_from(
                            context
                                .size_bytes
                                .div_ceil(context.container_plaintext_limit),
                        )
                        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Capacity))?;
                        row.completed_parts = row.part_count;
                        row.package_id = Some(hex_id(&context.package_id));
                        VaultTransferState::Completed
                    }
                    teleark_storage::VaultJobState::Retryable
                    | teleark_storage::VaultJobState::Blocked => {
                        let failure = job.failure_code.as_deref().and_then(parse_error);
                        if failure.is_none() {
                            // A newer failure code is not permission to replay
                            // work whose recovery requirements we cannot explain.
                            row.recovery_state = None;
                        }
                        VaultTransferState::Failed(
                            failure.unwrap_or(ApplicationErrorKind::InvalidRequest),
                        )
                    }
                };
            }
        }
        let _pending_omitted = self.restore_pending_upload_rows(&database, account, &mut rows)?;
        let (downloads, _download_omitted) = self.download_history(&database, account)?;
        rows.extend(downloads);
        rows.sort_by_key(|row| {
            (
                match row.state {
                    VaultTransferState::Queued
                    | VaultTransferState::Running
                    | VaultTransferState::Pausing
                    | VaultTransferState::Cancelling => 0,
                    VaultTransferState::Paused => 1,
                    VaultTransferState::Failed(_) if row.recovery_state.is_some() => 1,
                    _ => 2,
                },
                std::cmp::Reverse(row.queued_at_unix_ms),
            )
        });
        let total = database
            .vault_transfer_history_count(account)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let omitted = total.saturating_sub(rows.len().min(256) as u64);
        rows.truncate(256);
        let logs = self.library.managed_directories()?.logs.join("Transfers");
        for row in &mut rows {
            let path = logs.join(format!(
                "{}-{}.jsonl",
                TransferSessionKind::Vault.file_prefix(),
                row.id
            ));
            if path.is_file() {
                row.session_log_path = Some(path);
            }
        }
        self.transfers.restore_history(rows, omitted, |row| {
            row.account_id == account && !row.restored
        });
        Ok(())
    }
}

fn record(s: &VaultTransferSnapshot) -> VaultUploadRecord {
    let (state, failure_code) = match s.state {
        VaultTransferState::Queued => (State::Queued, None),
        VaultTransferState::Running => (State::Running, None),
        VaultTransferState::Completed => (State::Completed, None),
        VaultTransferState::Cancelled => (State::Cancelled, None),
        VaultTransferState::Pausing | VaultTransferState::Paused => (State::Interrupted, None),
        VaultTransferState::Cancelling => (State::Cancelled, None),
        VaultTransferState::Interrupted => (State::Interrupted, None),
        VaultTransferState::Failed(kind) => (State::Failed, Some(error_code(kind).to_owned())),
    };
    VaultUploadRecord {
        account_id: s.account_id,
        id: s.id,
        chat_id: s.chat_id,
        batch_id: s.batch_id,
        queued_at_unix_ms: s.queued_at_unix_ms,
        file_name: s.file_name.clone(),
        package_id: s.package_id.clone(),
        size_bytes: s.size_bytes,
        transferred_bytes: s.transferred_bytes.min(s.size_bytes),
        completed_parts: s.completed_parts,
        part_count: s.part_count,
        started_at_unix_ms: s.started_at_unix_ms,
        duration_ms: s.duration_ms,
        average_bytes_per_second: s.average_bytes_per_second,
        state,
        failure_code,
    }
}
fn orphan_upload_snapshot(
    account: i64,
    id: u64,
    record: Option<&teleark_storage::VaultJobRecord>,
) -> Result<VaultTransferSnapshot, ApplicationError> {
    let context = record
        .and_then(|record| crate::VaultRecoveryContext::from_record(record).ok())
        .filter(|context| {
            matches!(
                context.direction,
                crate::VaultRecoveryDirection::Upload { .. }
            )
        });
    Ok(VaultTransferSnapshot {
        id,
        account_id: account,
        chat_id: record.map_or(0, |record| record.chat_id),
        recovery_state: None,
        restored: true,
        upload_activity: None,
        batch_id: None,
        queued_at_unix_ms: record.map_or(0, |record| record.created_at_unix_ms),
        direction: VaultTransferDirection::Upload,
        file_name: context
            .as_ref()
            .map_or_else(String::new, |context| context.file_name.clone()),
        package_id: record.map(|record| hex_id(&record.package_id)),
        size_bytes: context.as_ref().map_or(0, |context| context.size_bytes),
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
        server_status: None,
        state: VaultTransferState::Failed(ApplicationErrorKind::InvalidRequest),
    })
}

fn snapshot(r: VaultUploadRecord) -> Result<VaultTransferSnapshot, ApplicationError> {
    let state = match r.state {
        State::Queued | State::Running => VaultTransferState::Interrupted,
        State::Interrupted => VaultTransferState::Interrupted,
        State::Cancelled => VaultTransferState::Cancelled,
        State::Completed => VaultTransferState::Completed,
        State::Failed => VaultTransferState::Failed(
            r.failure_code
                .as_deref()
                .and_then(parse_error)
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?,
        ),
    };
    Ok(VaultTransferSnapshot {
        recovery_state: None,
        restored: true,
        upload_activity: None,
        id: r.id,
        account_id: r.account_id,
        chat_id: r.chat_id,
        batch_id: r.batch_id,
        queued_at_unix_ms: r.queued_at_unix_ms,
        direction: VaultTransferDirection::Upload,
        file_name: r.file_name,
        package_id: r.package_id,
        size_bytes: r.size_bytes,
        transferred_bytes: r.transferred_bytes,
        completed_parts: r.completed_parts,
        part_count: r.part_count,
        started_at_unix_ms: r.started_at_unix_ms,
        duration_ms: r.duration_ms,
        average_bytes_per_second: r.average_bytes_per_second,
        destination: None,
        session_log_path: None,
        telemetry: transfer_controller(true, 0, teleark_telegram::TransferTuning::default())?
            .snapshot(),
        server_status: None,
        state,
    })
}

// Independent v1 locale-neutral error codes. No Debug/prose matching in the durable codec.
pub(super) fn error_code(kind: ApplicationErrorKind) -> &'static str {
    match kind {
        ApplicationErrorKind::InvalidRequest => "invalid_request",
        ApplicationErrorKind::NotFound => "not_found",
        ApplicationErrorKind::Conflict => "conflict",
        ApplicationErrorKind::Persistence => "persistence",
        ApplicationErrorKind::SourceMissing => "source_missing",
        ApplicationErrorKind::SourceChanged => "source_changed",
        ApplicationErrorKind::PermissionDenied => "permission_denied",
        ApplicationErrorKind::SourcePermissionDenied => "source_permission_denied",
        ApplicationErrorKind::StorageAccessDenied => "storage_access_denied",
        ApplicationErrorKind::StorageConfigurationUnsafe => "storage_configuration_unsafe",
        ApplicationErrorKind::StorageIdentityDamaged => "storage_identity_damaged",
        ApplicationErrorKind::StorageIdentityUnsupported => "storage_identity_unsupported",
        ApplicationErrorKind::VaultKeyUnavailable => "vault_key_unavailable",
        ApplicationErrorKind::Capacity => "capacity",
        ApplicationErrorKind::Authorization => "authorization",
        ApplicationErrorKind::Network => "network",
        ApplicationErrorKind::Server => "server",
        ApplicationErrorKind::Cancelled => "cancelled",
        _ => "persistence",
    }
}
pub(super) fn parse_error(code: &str) -> Option<ApplicationErrorKind> {
    [
        ApplicationErrorKind::InvalidRequest,
        ApplicationErrorKind::NotFound,
        ApplicationErrorKind::Conflict,
        ApplicationErrorKind::Persistence,
        ApplicationErrorKind::SourceMissing,
        ApplicationErrorKind::SourceChanged,
        ApplicationErrorKind::PermissionDenied,
        ApplicationErrorKind::SourcePermissionDenied,
        ApplicationErrorKind::StorageAccessDenied,
        ApplicationErrorKind::StorageConfigurationUnsafe,
        ApplicationErrorKind::StorageIdentityDamaged,
        ApplicationErrorKind::StorageIdentityUnsupported,
        ApplicationErrorKind::VaultKeyUnavailable,
        ApplicationErrorKind::Capacity,
        ApplicationErrorKind::Authorization,
        ApplicationErrorKind::Network,
        ApplicationErrorKind::Server,
        ApplicationErrorKind::Cancelled,
    ]
    .into_iter()
    .find(|kind| error_code(*kind) == code)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(id: u64, state: VaultTransferState) -> VaultTransferSnapshot {
        VaultTransferSnapshot {
            recovery_state: None,
            restored: false,
            upload_activity: Some(VaultUploadActivity::new(VaultUploadPhase::Persisting)),
            id,
            account_id: 7,
            chat_id: 90,
            batch_id: Some(50),
            queued_at_unix_ms: 100,
            direction: VaultTransferDirection::Upload,
            file_name: format!("東京 – {id}.bin"),
            package_id: None,
            size_bytes: 128,
            transferred_bytes: 128,
            completed_parts: 2,
            part_count: 2,
            started_at_unix_ms: 110,
            duration_ms: None,
            average_bytes_per_second: None,
            destination: None,
            session_log_path: None,
            telemetry: transfer_controller(true, 1, teleark_telegram::TransferTuning::default())
                .expect("controller")
                .snapshot(),
            server_status: None,
            state,
        }
    }
    fn owner(temp: &Path) -> Result<VaultOwner, ApplicationError> {
        Ok(VaultOwner {
            device_keys: device_keys::platform_store(),
            catalog: catalog::ManifestCache::default(),
            catalog_key_revision: 0,
            library: DesktopLibrary::open(temp.join("history.sqlite3"))?,
            telegram: DesktopTelegram::open_direct(temp.join("test.session"))?,
            record: None,
            master_key: None,
            historical_key: None,
            health_worker: None,
            session: Arc::new(Mutex::new(VaultSession::new(None))),
            session_generation: 0,
            transfers: Arc::new(TransferSnapshots::new(Vec::new())?),
            active_upload_batch: Arc::new(Mutex::new(None)),
            upload_controls: UploadControls::default(),
            #[cfg(test)]
            test_container_part_limit: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }
    #[test]
    fn valid_queued_recovery_overrides_interrupted_legacy_history_while_locked()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let owner = owner(temp.path())?;
        let row = fixture(1, VaultTransferState::Running);
        owner.persist_upload(&row)?;
        let source = temp.path().join("synthetic.bin");
        std::fs::write(&source, [9; 128])?;
        let context = crate::VaultRecoveryContext {
            container_plaintext_limit: crate::encrypted_part_plaintext_limit(),
            account_id: 7,
            task_id: 1,
            chat_id: 90,
            package_id: [1; 16],
            vault_id: [2; 16],
            master_key_generation: 1,
            file_key_wrap: teleark_crypto::wrap_file_key(
                &VaultMasterKey::from_bytes([3; 32]),
                &teleark_crypto::FileKey::from_bytes([4; 32]),
                &[2; 16],
                &[1; 16],
                1,
                1,
                &mut AeadUsageRegistry::new(),
            )?,
            file_name: row.file_name.clone(),
            created_at_unix_ms: 100,
            size_bytes: 128,
            direction: crate::VaultRecoveryDirection::Upload {
                source,
                identity: teleark_transfer::SourceIdentity {
                    filesystem_id: 1,
                    size_bytes: 128,
                    modified_at_units: 1,
                    revision: 1,
                },
                source_blake3: *blake3::hash(&[9; 128]).as_bytes(),
            },
        };
        let mut db = teleark_storage::Database::open(owner.library.database_path.as_ref())?;
        assert!(db.admit_vault_job(&context.admission_record().expect("valid recovery context"))?);
        owner.restore_upload_history(7)?;
        let restored = owner.transfers.get(1).expect("restored task");
        assert_eq!(restored.state, VaultTransferState::Queued);
        assert_eq!(
            restored.recovery_state,
            Some(teleark_storage::VaultJobState::Queued)
        );
        assert_eq!(restored.batch_id, Some(50));
        assert!(restored.restored);
        assert!(owner.master_key.is_none(), "history does not unlock keys");
        for (transition, expected) in [
            (
                teleark_storage::VaultJobTransition::Start,
                VaultTransferState::Running,
            ),
            (
                teleark_storage::VaultJobTransition::RequestPause,
                VaultTransferState::Pausing,
            ),
            (
                teleark_storage::VaultJobTransition::RequestCancel,
                VaultTransferState::Cancelling,
            ),
        ] {
            let saved = db.vault_job(7, 1)?.expect("authoritative job");
            assert!(db.transition_vault_job(
                teleark_storage::VaultJobLease {
                    account_id: 7,
                    id: 1,
                    generation: saved.generation
                },
                saved.state,
                transition,
                200,
                None,
            )?);
            owner.restore_upload_history(7)?;
            assert_eq!(
                owner.transfers.get(1).expect("updated phase").state,
                expected
            );
        }
        for (id, terminal, failure, expected) in [
            (
                2,
                teleark_storage::VaultJobTransition::Complete,
                None,
                VaultTransferState::Completed,
            ),
            (
                3,
                teleark_storage::VaultJobTransition::FailRetryable,
                Some("network"),
                VaultTransferState::Failed(ApplicationErrorKind::Network),
            ),
            (
                4,
                teleark_storage::VaultJobTransition::FailBlocked,
                Some("source_missing"),
                VaultTransferState::Failed(ApplicationErrorKind::SourceMissing),
            ),
            (
                5,
                teleark_storage::VaultJobTransition::FailRetryable,
                Some("future_problem"),
                VaultTransferState::Failed(ApplicationErrorKind::InvalidRequest),
            ),
        ] {
            let mut legacy = fixture(id, VaultTransferState::Running);
            legacy.transferred_bytes = 0;
            legacy.completed_parts = 0;
            legacy.part_count = 0;
            legacy.average_bytes_per_second = Some(777);
            legacy.duration_ms = Some(88);
            owner.persist_upload(&legacy)?;
            let mut saved_context = context.clone();
            saved_context.task_id = id;
            saved_context.file_name = legacy.file_name;
            assert!(db.admit_vault_job(&saved_context.admission_record().expect("context"))?);
            assert!(db.transition_vault_job(
                teleark_storage::VaultJobLease {
                    account_id: 7,
                    id,
                    generation: 0
                },
                teleark_storage::VaultJobState::Queued,
                teleark_storage::VaultJobTransition::Start,
                201,
                None
            )?);
            assert!(db.transition_vault_job(
                teleark_storage::VaultJobLease {
                    account_id: 7,
                    id,
                    generation: 1
                },
                teleark_storage::VaultJobState::Running,
                terminal,
                202,
                failure
            )?);
            owner.restore_upload_history(7)?;
            let restored = owner.transfers.get(id).expect("terminal history");
            assert_eq!(restored.state, expected);
            if id == 2 {
                assert_eq!(restored.transferred_bytes, 128);
                assert_eq!((restored.completed_parts, restored.part_count), (1, 1));
                assert_eq!(restored.package_id, Some(hex_id(&context.package_id)));
                assert_eq!(restored.average_bytes_per_second, None);
                assert_eq!(restored.duration_ms, None);
            }
            if id == 5 {
                assert!(restored.recovery_state.is_none());
            }
        }
        for id in 6..=8 {
            let mut missing = context.clone();
            missing.task_id = id;
            let mut record = missing.admission_record().expect("context");
            if id == 8 {
                record.context = vec![1];
            }
            assert!(db.admit_vault_job(&record)?);
            if id == 7 {
                assert!(db.transition_vault_job(
                    teleark_storage::VaultJobLease {
                        account_id: 7,
                        id,
                        generation: 0
                    },
                    teleark_storage::VaultJobState::Queued,
                    teleark_storage::VaultJobTransition::Start,
                    201,
                    None
                )?);
                assert!(db.transition_vault_job(
                    teleark_storage::VaultJobLease {
                        account_id: 7,
                        id,
                        generation: 1
                    },
                    teleark_storage::VaultJobState::Running,
                    teleark_storage::VaultJobTransition::Complete,
                    202,
                    None
                )?);
            }
        }
        owner.restore_upload_history(7)?;
        let queued = owner
            .transfers
            .get(6)
            .expect("queued ledger without summary is visible");
        assert_eq!(queued.state, VaultTransferState::Queued);
        assert_eq!(queued.file_name, context.file_name);
        let completed = owner
            .transfers
            .get(7)
            .expect("completed ledger without summary is visible");
        assert_eq!(completed.state, VaultTransferState::Completed);
        assert_eq!(completed.transferred_bytes, 128);
        let damaged = owner
            .transfers
            .get(8)
            .expect("damaged ledger remains visible");
        assert_eq!(
            damaged.state,
            VaultTransferState::Failed(ApplicationErrorKind::InvalidRequest)
        );
        assert!(damaged.recovery_state.is_none());
        Ok(())
    }

    #[test]
    fn restart_restores_batch_results_while_locked_and_repeated_reads_keep_live_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        {
            let owner = owner(temp.path())?;
            owner.push_transfer(fixture(1, VaultTransferState::Running))?;
            owner.finish_upload(1, Instant::now(), Some("0102030405060708".into()))?;
            owner.push_transfer(fixture(2, VaultTransferState::Queued))?;
            owner.push_transfer(fixture(
                3,
                VaultTransferState::Failed(ApplicationErrorKind::SourceChanged),
            ))?;
        }
        for _ in 0..2 {
            let library = DesktopLibrary::open(temp.path().join("history.sqlite3"))?;
            let telegram = DesktopTelegram::open_direct(temp.path().join("new.session"))?;
            let vault = DesktopVault::new(telegram, library)?;
            assert!(vault.status().locked);
            vault.restore_upload_history(7)?;
            let rows = vault.transfers();
            assert_eq!(rows.len(), 3);
            assert!(
                rows.iter()
                    .all(|r| r.restored && r.batch_id == Some(50) && r.account_id == 7)
            );
            assert_eq!(rows[0].state, VaultTransferState::Completed);
            assert_eq!(rows[0].package_id.as_deref(), Some("0102030405060708"));
            assert_eq!(rows[0].transferred_bytes, 128);
            assert_eq!(rows[1].state, VaultTransferState::Interrupted);
            assert_eq!(
                rows[2].state,
                VaultTransferState::Failed(ApplicationErrorKind::SourceChanged)
            );
            assert!(!vault.has_active_transfers());
            vault.restore_upload_history(8)?;
            assert!(
                vault.transfers().is_empty(),
                "account switch must not leave another account's history in this cache"
            );
            vault.restore_upload_history(7)?;
            // A delayed restore may not roll back a live observation to an older database sample.
            vault.inner.transfers.update(2, |r| {
                r.state = VaultTransferState::Running;
                r.restored = false;
                r.transferred_bytes = 96;
            });
            vault.restore_upload_history(7)?;
            assert_eq!(
                vault
                    .inner
                    .transfers
                    .get(2)
                    .expect("live")
                    .transferred_bytes,
                96
            );
            assert_eq!(
                vault.inner.transfers.get(2).expect("live").state,
                VaultTransferState::Running
            );
        }
        Ok(())
    }
    #[test]
    fn blocked_or_failed_completion_cannot_publish_success_or_block_observers()
    -> Result<(), Box<dyn std::error::Error>> {
        for success in [true, false] {
            let temp = tempfile::tempdir()?;
            let mut owner = owner(temp.path())?;
            let rows = owner.transfers.clone();
            rows.extend([
                fixture(1, VaultTransferState::Running),
                fixture(2, VaultTransferState::Running),
            ]);
            let (sender, receiver) = mpsc::sync_channel(1);
            let (entered, waiting) = mpsc::sync_channel(1);
            let (release, released) = mpsc::sync_channel(1);
            let storage = thread::spawn(move || {
                let crate::StorageRequest::SaveVaultUploads { records, reply } =
                    receiver.recv().expect("save")
                else {
                    panic!("save expected")
                };
                assert_eq!(records[0].state, State::Completed);
                entered.send(()).expect("entered");
                released.recv().expect("release");
                let _ = reply.send(if success {
                    Ok(())
                } else {
                    Err(ApplicationError::new(ApplicationErrorKind::Persistence))
                });
            });
            owner.library.worker = crate::StorageWorker {
                inner: Arc::new(crate::WorkerInner {
                    sender,
                    join: Mutex::new(None),
                }),
            };
            let transfer = thread::spawn(move || {
                owner.finish_upload(1, Instant::now(), Some("0102030405060708".into()))
            });
            waiting.recv_timeout(std::time::Duration::from_secs(2))?;
            assert_eq!(
                rows.get(1).expect("pending commit").state,
                VaultTransferState::Running
            );
            let (observed, observation) = mpsc::sync_channel(1);
            let observer_rows = rows.clone();
            let callback = thread::spawn(move || {
                VaultUploadObserver::new(observer_rows, 2).phase(VaultUploadPhase::Verifying);
                observed.send(()).expect("callback");
            });
            observation.recv_timeout(std::time::Duration::from_secs(2))?;
            assert_eq!(
                rows.get(2)
                    .expect("unrelated")
                    .upload_activity
                    .expect("activity")
                    .phase,
                VaultUploadPhase::Verifying
            );
            release.send(())?;
            let result = transfer.join().expect("owner");
            assert_eq!(result.is_ok(), success);
            assert_eq!(
                rows.get(1).expect("terminal").state,
                if success {
                    VaultTransferState::Completed
                } else {
                    VaultTransferState::Failed(ApplicationErrorKind::Persistence)
                }
            );
            callback.join().expect("callback");
            storage.join().expect("storage");
        }
        Ok(())
    }
    #[test]
    fn durable_error_codes_are_exact_and_unknown_codes_are_rejected() {
        for kind in [
            ApplicationErrorKind::SourcePermissionDenied,
            ApplicationErrorKind::StorageIdentityUnsupported,
            ApplicationErrorKind::Server,
            ApplicationErrorKind::VaultKeyUnavailable,
        ] {
            assert_eq!(
                snapshot(record(&fixture(1, VaultTransferState::Failed(kind))))
                    .expect("codec")
                    .state,
                VaultTransferState::Failed(kind)
            );
        }
        let mut stored = record(&fixture(
            1,
            VaultTransferState::Failed(ApplicationErrorKind::Network),
        ));
        stored.failure_code = Some("future_error".into());
        assert!(snapshot(stored).is_err());
    }
}
