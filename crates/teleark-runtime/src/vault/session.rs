//! Session admission is independent from retained operation key leases.
use super::*;
use std::sync::atomic::AtomicUsize;

#[derive(Clone, Default)]
pub(super) struct KeyLease {
    pub revision: u64,
    pub record: Option<VaultMetadataRecord>,
    pub active: Option<Arc<VaultMasterKey>>,
    pub historical: Option<([u8; 16], Arc<VaultMasterKey>)>,
}

pub(super) struct VaultSession {
    generation: u64,
    keys: KeyLease,
    pub status: VaultStatus,
    pub closing: bool,
    pub pending_transfers: Arc<AtomicUsize>,
}

pub(super) struct VaultEnvelope {
    pub command: VaultCommand,
    pub generation: u64,
    pub keys: KeyLease,
    _admission: Option<WorkPermit>,
}

impl VaultSession {
    pub(super) fn scan_revision(&self) -> (u64, u64) {
        (self.generation, self.keys.revision)
    }

    pub fn new(record: Option<VaultMetadataRecord>) -> Self {
        Self {
            generation: 0,
            closing: false,
            pending_transfers: Arc::new(AtomicUsize::new(0)),
            status: status_for(record.as_ref(), true),
            keys: KeyLease {
                record,
                ..KeyLease::default()
            },
        }
    }

    pub(super) fn current_keys(&self, generation: u64) -> Result<KeyLease, ApplicationError> {
        if generation != self.generation || self.closing {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        Ok(self.keys.clone())
    }

    pub fn lock(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.keys.revision = self.keys.revision.wrapping_add(1);
        self.keys.active = None;
        self.keys.historical = None;
        self.status.locked = true;
        self.status.active_key_locked = true;
        self.status.historical_key_unlocked = false;
        self.generation
    }

    pub fn admit(&self, command: VaultCommand) -> Result<VaultEnvelope, ApplicationError> {
        if self.closing && (command.is_transfer() || command.is_scan()) {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let needs_active = (command.is_transfer()
            && !matches!(
                command,
                VaultCommand::Download { .. }
                    | VaultCommand::ResumeDownload { .. }
                    | VaultCommand::ResumeQueuedTransfers { .. }
            ))
            || matches!(
                command,
                VaultCommand::Upload { .. }
                    | VaultCommand::UploadBatch { .. }
                    | VaultCommand::ChangePassword { .. }
                    | VaultCommand::RotateRecovery { .. }
            );
        let needs_any = command.is_scan()
            || matches!(
                command,
                VaultCommand::Download { .. }
                    | VaultCommand::ResumeDownload { .. }
                    | VaultCommand::ResumeQueuedTransfers { .. }
            );
        if (needs_active && self.keys.active.is_none())
            || (needs_any && self.keys.active.is_none() && self.keys.historical.is_none())
        {
            return Err(ApplicationError::new(
                ApplicationErrorKind::VaultKeyUnavailable,
            ));
        }
        let admission = command.is_transfer().then(|| {
            self.pending_transfers.fetch_add(1, Ordering::AcqRel);
            WorkPermit(self.pending_transfers.clone())
        });
        Ok(VaultEnvelope {
            command,
            generation: self.generation,
            keys: self.keys.clone(),
            _admission: admission,
        })
    }

    pub fn publish(
        &mut self,
        generation: u64,
        record: Option<VaultMetadataRecord>,
        active: Option<Arc<VaultMasterKey>>,
        historical: Option<([u8; 16], Arc<VaultMasterKey>)>,
    ) {
        self.keys.record = record;
        // A late password/KDF completion cannot undo an explicit lock.
        if generation == self.generation {
            self.keys.revision = self.keys.revision.wrapping_add(1);
            self.keys.active = active;
            self.keys.historical = historical;
        }
        self.status = status_for(self.keys.record.as_ref(), self.keys.active.is_none());
        self.status.historical_key_unlocked = self.keys.historical.is_some();
        self.status.locked = self.keys.active.is_none() && self.keys.historical.is_none();
    }
}

impl VaultEnvelope {
    pub fn invalidate(generation: u64) -> Self {
        Self {
            command: VaultCommand::Invalidate,
            generation,
            keys: KeyLease::default(),
            _admission: None,
        }
    }
    pub fn shutdown() -> Self {
        Self {
            command: VaultCommand::Shutdown,
            generation: 0,
            keys: KeyLease::default(),
            _admission: None,
        }
    }
}

impl VaultCommand {
    pub fn is_transfer(&self) -> bool {
        #[cfg(test)]
        if matches!(
            self,
            Self::TestTransfer { .. } | Self::TestControlledUpload { .. }
        ) {
            return true;
        }
        matches!(
            self,
            Self::Upload { .. }
                | Self::ResumeUpload { .. }
                | Self::ResumeQueuedUploads { .. }
                | Self::ResumeQueuedTransfers { .. }
                | Self::UploadBatch { .. }
                | Self::Download { .. }
                | Self::ResumeDownload { .. }
        )
    }

    pub fn is_scan(&self) -> bool {
        #[cfg(test)]
        if matches!(self, Self::TestScan { .. }) {
            return true;
        }
        matches!(self, Self::Scan { .. })
    }
    pub fn is_key_operation(&self) -> bool {
        !self.is_transfer() && !self.is_scan()
    }
}

struct WorkPermit(Arc<AtomicUsize>);
impl Drop for WorkPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn download() -> VaultCommand {
        VaultCommand::Download {
            account_id: 7,
            chat_id: 9,
            package_id: 1,
            reply: mpsc::sync_channel(1).0,
        }
    }

    #[test]
    fn lock_preserves_a_blocked_health_worker_and_its_cancellation_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.db"))?;
        let telegram = DesktopTelegram::open_direct(temp.path().join("test.session"))?;
        let vault = DesktopVault::new(telegram, library)?;
        vault.initialize("synthetic scan password".into())?;
        let cancellation = crate::TelegramScanCancellation::new();
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::channel();
        let job = vault.submit(|reply| VaultCommand::TestScan {
            entered,
            release: wait,
            cancellation: cancellation.clone(),
            reply,
        })?;
        ready.recv_timeout(Duration::from_secs(3))?;
        vault.lock()?;
        assert!(!cancellation.is_cancelled());
        assert!(vault.status().locked);
        assert!(
            vault
                .submit_managed_scan(
                    7,
                    9,
                    ManagedScanMode::CheckHealth,
                    crate::TelegramScanCancellation::new(),
                    None
                )
                .is_err()
        );
        release.send(())?;
        job.wait()?;
        assert!(!cancellation.is_cancelled());
        assert!(vault.status().locked);
        Ok(())
    }

    #[test]
    fn locking_revokes_new_admission_without_revoking_or_retaining_operation_keys() {
        let mut session = VaultSession::new(None);
        let key = Arc::new(VaultMasterKey::from_bytes([7; 32]));
        let weak = Arc::downgrade(&key);
        session.publish(0, None, Some(key), None);
        let first = session.admit(download()).expect("first admitted");
        let second = session.admit(download()).expect("queued admitted");
        assert_eq!(session.pending_transfers.load(Ordering::Acquire), 2);
        session.lock();
        assert!(session.status.locked);
        assert!(session.keys.active.is_none());
        assert!(session.admit(download()).is_err());
        assert!(first.keys.active.is_some());
        drop(first);
        assert!(weak.upgrade().is_some());
        drop(second);
        assert!(weak.upgrade().is_none());
        assert_eq!(session.pending_transfers.load(Ordering::Acquire), 0);
    }

    #[test]
    fn stale_unlock_completion_cannot_reopen_session_and_historical_keys_cannot_upload() {
        let mut session = VaultSession::new(None);
        let generation = session.lock();
        session.publish(
            0,
            None,
            Some(Arc::new(VaultMasterKey::from_bytes([7; 32]))),
            None,
        );
        assert!(session.status.locked);
        assert!(session.admit(download()).is_err());
        session.publish(
            generation,
            None,
            None,
            Some(([2; 16], Arc::new(VaultMasterKey::from_bytes([8; 32])))),
        );
        assert!(
            session
                .admit(VaultCommand::ResumeQueuedTransfers {
                    account_id: 7,
                    reply: mpsc::sync_channel(1).0,
                })
                .is_ok(),
            "mixed recovery can use historical download keys"
        );
        assert!(!session.status.locked);
        assert!(session.status.active_key_locked);
        assert!(session.admit(download()).is_ok());
        assert!(
            session
                .admit(VaultCommand::ResumeDownload {
                    account_id: 7,
                    task_id: 9,
                    reply: mpsc::sync_channel(1).0,
                })
                .is_ok(),
            "historical keys may admit download recovery"
        );

        assert!(
            session
                .admit(VaultCommand::ResumeUpload {
                    account_id: 7,
                    task_id: 9,
                    reply: mpsc::sync_channel(1).0
                })
                .is_err()
        );

        assert!(
            session
                .admit(VaultCommand::ResumeQueuedUploads {
                    account_id: 7,
                    reply: mpsc::sync_channel(1).0
                })
                .is_err()
        );
        assert!(
            session
                .admit(VaultCommand::UploadBatch {
                    progress: VaultUploadSelectionProgress::new(1),
                    account_id: 7,
                    chat_id: 9,
                    sources: vec![],
                    reply: mpsc::sync_channel(1).0
                })
                .is_err()
        );
    }

    #[test]
    fn blocked_transfer_does_not_block_lock_unlock_or_preserved_queued_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let library = DesktopLibrary::open(temp.path().join("catalog.db"))?;
        let telegram = DesktopTelegram::open_direct(temp.path().join("test.session"))?;
        let vault = DesktopVault::new(telegram, library)?;
        vault.initialize("synthetic session password".into())?;
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::channel();
        let first = vault.clone();
        let work = thread::spawn(move || {
            first.request(|reply| VaultCommand::TestTransfer {
                entered,
                release: wait,
                reply,
            })
        });
        let original_key = ready.recv_timeout(Duration::from_secs(3))?;
        let (entered, queued_ready) = mpsc::sync_channel(1);
        let (release_queued, wait) = mpsc::channel();
        let (reply, queued_done) = mpsc::sync_channel(1);
        let queued =
            vault
                .inner
                .session
                .lock()
                .expect("session")
                .admit(VaultCommand::TestTransfer {
                    entered,
                    release: wait,
                    reply,
                })?;
        vault
            .inner
            .transfer_sender
            .lock()
            .expect("sender")
            .as_ref()
            .expect("open")
            .send(queued)
            .map_err(|_| "queue closed")?;
        let (locked, lock_done) = mpsc::sync_channel(1);
        let locker = vault.clone();
        let lock_work = thread::spawn(move || {
            locked.send(locker.lock()).expect("lock reply");
        });
        lock_done.recv_timeout(Duration::from_secs(3))??;
        lock_work.join().expect("lock thread");
        assert!(vault.status().locked);
        assert!(vault.has_active_transfers());
        assert!(original_key.upgrade().is_some());
        assert_eq!(
            vault
                .upload_files(7, 9, vec![PathBuf::from("synthetic-not-opened")])
                .expect_err("locked upload")
                .kind(),
            ApplicationErrorKind::VaultKeyUnavailable
        );
        assert_eq!(
            vault
                .download_file(7, 9, 1)
                .expect_err("locked download")
                .kind(),
            ApplicationErrorKind::VaultKeyUnavailable
        );
        let (unlocked, unlock_done) = mpsc::sync_channel(1);
        let unlocker = vault.clone();
        let unlock_work = thread::spawn(move || {
            unlocked
                .send(unlocker.unlock_with_password("synthetic session password".into()))
                .expect("unlock reply");
        });
        unlock_done.recv_timeout(Duration::from_secs(5))??;
        unlock_work.join().expect("unlock thread");
        assert!(!vault.status().locked);
        {
            let session = vault.inner.session.lock().expect("session");
            assert!(!Arc::ptr_eq(
                &original_key.upgrade().expect("task key"),
                session.keys.active.as_ref().expect("new session key")
            ));
        }
        vault.lock()?;
        release.send(())?;
        work.join().expect("transfer thread")?;
        let queued_key = queued_ready.recv_timeout(Duration::from_secs(3))?;
        assert!(queued_key.upgrade().is_some());
        assert!(vault.status().locked);
        release_queued.send(())?;
        queued_done.recv_timeout(Duration::from_secs(3))??;
        Ok(())
    }
}
