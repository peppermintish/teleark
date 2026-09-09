//! Durable summaries on acknowledged owner-thread boundaries; never on progress callbacks.
use super::*;
use teleark_storage::{StoredVaultUploadState as State, VaultUploadRecord};

impl VaultOwner {
    pub(super) fn admit_upload_window(
        &self,
        rows: Vec<VaultTransferSnapshot>,
    ) -> Result<(), ApplicationError> {
        for row in &rows {
            self.transfers
                .insert_pruning(row.clone(), vault_transfer_evictions);
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
        self.transfers
            .restore_history(rows, history.omitted, |row| {
                matches!(
                    row.state,
                    VaultTransferState::Queued | VaultTransferState::Running
                ) || row.direction == VaultTransferDirection::Download
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
        telemetry: transfer_controller(true, 0, teleark_transfer::SoftLimitPolicy::Respect)?
            .snapshot(),
        state,
    })
}

// Independent v1 locale-neutral error codes. No Debug/prose matching in the durable codec.
fn error_code(kind: ApplicationErrorKind) -> &'static str {
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
fn parse_error(code: &str) -> Option<ApplicationErrorKind> {
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
            telemetry: transfer_controller(true, 1, teleark_transfer::SoftLimitPolicy::Respect)
                .expect("controller")
                .snapshot(),
            state,
        }
    }
    fn owner(temp: &Path) -> Result<VaultOwner, ApplicationError> {
        Ok(VaultOwner {
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
        })
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
