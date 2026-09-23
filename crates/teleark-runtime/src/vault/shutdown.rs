//! Cooperative exit: durable pause intent precedes cancellation and writer settlement.
use super::*;
use std::time::Duration;
use teleark_storage::VaultJobState;

#[derive(Default)]
pub(super) struct ExitPause {
    requested: AtomicBool,
    cancellation: Mutex<crate::TelegramScanCancellation>,
}
impl ExitPause {
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
    pub fn cancellation(&self) -> crate::TelegramScanCancellation {
        self.cancellation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn begin(&self) {
        self.requested.store(true, Ordering::Release);
        // Interrupt manifest discovery, which has no durable transfer yet.
        self.cancellation().cancel();
    }
    fn reset(&self) {
        *self.cancellation.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
        self.requested.store(false, Ordering::Release);
    }
}
impl DesktopVault {
    /// Background-only. Fence admission, pause saved queues (including non-visible rows),
    /// then await retained transfer owners closing their journals. Never waits for a
    /// complete upload/download. A timeout/error keeps the application alive for retry.
    pub fn pause_for_shutdown(&self) -> Result<(), ApplicationError> {
        let pending = {
            let mut session = self
                .inner
                .session
                .lock()
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
            session.closing = true;
            session.pending_transfers.clone()
        };
        self.inner.upload_controls.exit_pause.begin();
        let account = self.inner.lifecycle.snapshot().1;
        let deadline = Instant::now() + Duration::from_secs(30);
        if let Some(account) = account {
            self.submit(|reply| VaultCommand::PauseForShutdown { account, reply })?
                .wait()?;
        }
        pending.wait_until_empty(deadline)?;
        // The last owner may have admitted a saved selection during the first pass.
        if let Some(account) = account {
            self.submit(|reply| VaultCommand::PauseForShutdown { account, reply })?
                .wait()?;
        }
        Ok(())
    }

    /// Keep already-paused work paused when an exit is abandoned after an error.
    pub fn abandon_shutdown(&self) {
        self.inner.upload_controls.exit_pause.reset();
        self.inner
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closing = false;
    }
}
impl VaultOwner {
    pub(super) fn pause_saved_transfers(&self, account: i64) -> Result<(), ApplicationError> {
        let persistence = |_| ApplicationError::new(ApplicationErrorKind::Persistence);
        let mut db = teleark_storage::Database::open(self.library.database_path.as_ref())
            .map_err(persistence)?;
        // Existing indexed keyset queries bound memory independently of queue size.
        for state in [
            VaultJobState::Queued,
            VaultJobState::Running,
            VaultJobState::Pausing,
        ] {
            let mut after = 0;
            loop {
                let ids = db
                    .vault_job_ids(account, state, after, 128)
                    .map_err(persistence)?;
                if ids.is_empty() {
                    break;
                }
                for id in ids {
                    after = id;
                    if let Err(error) = self.control_upload(account, id, VaultUploadControl::Pause)
                    {
                        let current = db.vault_job(account, id).map_err(persistence)?;
                        // Completion or another control can win the CAS. Persistence failures
                        // still stop exit; no error-string matching or destructive fallback.
                        if error.kind() != ApplicationErrorKind::Conflict
                            || current.is_some_and(|r| {
                                matches!(r.state, VaultJobState::Queued | VaultJobState::Running)
                            })
                        {
                            return Err(error);
                        }
                    }
                }
            }
        }
        let mut after = 0;
        loop {
            let rows = db
                .queued_pending_vault_uploads(account, after, 128)
                .map_err(persistence)?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                after = row.id;
                self.control_pending_upload(&mut db, account, row.id, VaultUploadControl::Pause)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teleark_storage::{
        Database, VaultJobDirection, VaultJobLease, VaultJobRecord, VaultJobTransition,
    };

    #[test]
    fn exit_pauses_hidden_queues_before_waking_locked_workers_and_preserves_restart_intent()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("catalog.db");
        let library = DesktopLibrary::open(&path)?;
        let telegram = DesktopTelegram::open_direct(dir.path().join("synthetic.session"))?;
        telegram.lifecycle().publish(1, Some(7), None);
        let vault = DesktopVault::new(telegram, library)?;
        vault.inner.session.lock().expect("session").publish(
            0,
            None,
            Some(Arc::new(VaultMasterKey::from_bytes([7; 32]))),
            None,
        );
        let mut db = Database::open(&path)?;
        for id in 1..=260 {
            db.admit_vault_job(&VaultJobRecord {
                account_id: 7,
                id,
                chat_id: 11,
                direction: if id % 2 == 0 {
                    VaultJobDirection::Download
                } else {
                    VaultJobDirection::Upload
                },
                package_id: [1; 16],
                context_version: 1,
                context: vec![1],
                state: VaultJobState::Queued,
                generation: 0,
                created_at_unix_ms: 100,
                updated_at_unix_ms: 100,
                failure_code: None,
            })?;
        }
        let pending = crate::VaultPendingUploadContext {
            account_id: 7,
            task_id: 1000,
            chat_id: 11,
            batch_id: 42,
            created_at_unix_ms: 100,
            vault_id: [7; 16],
            master_key_generation: 1,
            file_name: "synthetic.bin".into(),
            source: dir.path().join("synthetic.bin"),
            identity: teleark_transfer::SourceIdentity {
                filesystem_id: 0,
                size_bytes: 4,
                modified_at_units: 0,
                revision: 0,
            },
        }
        .admission_record()
        .expect("synthetic pending context");
        db.admit_pending_vault_upload_selection([Ok(pending.clone())])?;
        let mut lease = VaultJobLease {
            account_id: 7,
            id: 1,
            generation: 0,
        };
        db.transition_vault_job(
            lease,
            VaultJobState::Queued,
            VaultJobTransition::Start,
            101,
            None,
        )?;
        lease.generation = 1;
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::channel();
        let work = vault.submit(|reply| VaultCommand::TestControlledUpload {
            lease,
            entered,
            release: wait,
            reply,
        })?;
        let cancellation = ready.recv_timeout(Duration::from_secs(5))?;
        vault.lock()?;
        assert!(
            vault.inner.transfers.all().expect("projection").is_empty(),
            "queue is not materialized in the UI"
        );
        let exiting = vault.clone();
        let (finished, done) = mpsc::channel();
        let shutdown = thread::spawn(move || {
            let _ = finished.send(exiting.pause_for_shutdown());
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cancellation.is_cancelled() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert_eq!(
            db.vault_job(7, 1)?.expect("intent").state,
            VaultJobState::Pausing
        );
        assert!(
            done.try_recv().is_err(),
            "exit waits for the retained writer, not transfer completion"
        );
        release.send(())?;
        done.recv_timeout(Duration::from_secs(5))??;
        shutdown.join().expect("shutdown owner");
        work.wait()?;
        assert!(
            vault.submit_resume_download(7, 2).is_err(),
            "new admission is fenced"
        );
        db.recover_all_vault_jobs(200)?;
        for id in 1..=260 {
            assert_eq!(
                db.vault_job(7, id)?.expect("saved").state,
                VaultJobState::Paused
            );
        }
        let saved = db.pending_vault_upload(7, 1000)?.expect("saved selection");
        assert_eq!(
            saved.state,
            teleark_storage::PendingVaultUploadState::Paused
        );
        assert_eq!(saved.record, pending);
        vault.abandon_shutdown();
        assert!(!vault.inner.session.lock().expect("session").closing);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn checkpoint_failure_is_reported_and_retry_keeps_original_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("catalog.db");
        let library = DesktopLibrary::open(&path)?;
        let telegram = DesktopTelegram::open_direct(dir.path().join("synthetic.session"))?;
        telegram.lifecycle().publish(1, Some(7), None);
        let vault = DesktopVault::new(telegram, library)?;
        let backup = dir.path().join("preserved.db");
        std::fs::rename(&path, &backup)?;
        std::fs::create_dir(&path)?;
        assert_eq!(
            vault
                .pause_for_shutdown()
                .expect_err("cannot open directory")
                .kind(),
            ApplicationErrorKind::Persistence
        );
        vault.abandon_shutdown();
        std::fs::remove_dir(&path)?;
        std::fs::rename(&backup, &path)?;
        vault.pause_for_shutdown()?;
        Ok(())
    }
}
