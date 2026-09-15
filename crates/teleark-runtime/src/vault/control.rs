//! Independent durable controls for busy encrypted-transfer owners.
use super::*;
use std::collections::BTreeMap;
use teleark_storage::{Database, VaultJobLease, VaultJobState, VaultJobTransition};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultUploadControl {
    Pause,
    Cancel,
}

type UploadControlEntries = BTreeMap<(i64, u64), (u64, crate::TelegramScanCancellation)>;

#[derive(Clone, Default)]
pub(super) struct UploadControls {
    entries: Arc<Mutex<UploadControlEntries>>,
    pub(super) exit_pause: Arc<super::shutdown::ExitPause>,
}

pub(super) struct UploadRegistration {
    owner: UploadControls,
    lease: VaultJobLease,
    pub cancellation: crate::TelegramScanCancellation,
    lifecycle: Option<crate::telegram::lifecycle::Subscription>,
}
impl UploadControls {
    pub fn register(&self, lease: VaultJobLease) -> Result<UploadRegistration, ApplicationError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        if entries.contains_key(&(lease.account_id, lease.id)) {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        if entries.len() >= VAULT_QUEUE_CAPACITY {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        let cancellation = crate::TelegramScanCancellation::default();
        entries.insert(
            (lease.account_id, lease.id),
            (lease.generation, cancellation.clone()),
        );
        Ok(UploadRegistration {
            owner: self.clone(),
            lease,
            cancellation,
            lifecycle: None,
        })
    }
    fn cancel(&self, lease: VaultJobLease) -> Result<bool, ApplicationError> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let cancellation = entries
            .get(&(lease.account_id, lease.id))
            .filter(|(generation, _)| *generation == lease.generation)
            .map(|(_, token)| token.clone());
        drop(entries);
        if let Some(cancellation) = cancellation {
            cancellation.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn contains(&self, account: i64, id: u64) -> Result<bool, ApplicationError> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .contains_key(&(account, id)))
    }

    fn signal_or_acknowledge(
        &self,
        database: &mut Database,
        lease: VaultJobLease,
    ) -> Result<Option<VaultJobState>, ApplicationError> {
        if self.cancel(lease)? {
            return Ok(None);
        }
        // An absent registration means no publication owner remains. A worker
        // registering later must validate this lease before publishing a part.
        // Only acknowledge pending control states; never complete an orphan.
        let record = database
            .vault_job(lease.account_id, lease.id)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        if record.generation != lease.generation
            || !matches!(
                record.state,
                VaultJobState::Pausing | VaultJobState::Cancelling
            )
        {
            return Ok(None);
        }
        finish_job(database, lease, Some(ApplicationErrorKind::Cancelled)).map(Some)
    }
}
impl Drop for UploadRegistration {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.owner.entries.lock() {
            let key = (self.lease.account_id, self.lease.id);
            if entries
                .get(&key)
                .is_some_and(|(generation, _)| *generation == self.lease.generation)
            {
                entries.remove(&key);
            }
        }
    }
}

impl VaultOwner {
    /// Bind the retained operation to the connection/account revision without
    /// doing I/O in lifecycle callbacks. A replacement also fences local hashing.
    pub(super) fn register_transfer(
        &self,
        lease: VaultJobLease,
    ) -> Result<UploadRegistration, ApplicationError> {
        if self.upload_controls.exit_pause.is_requested() {
            self.control_upload(lease.account_id, lease.id, VaultUploadControl::Pause)?;
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        let lifecycle = self.telegram.lifecycle();
        let (revision, account, _) = lifecycle.snapshot();
        if account != Some(lease.account_id) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let mut registration = self.upload_controls.register(lease)?;
        let token = registration.cancellation.clone();
        let observed = lifecycle.clone();
        registration.lifecycle = Some(lifecycle.subscribe(Arc::new(move || {
            let (current, account, _) = observed.snapshot();
            if current != revision || account != Some(lease.account_id) {
                token.cancel();
            }
        }))?);
        let (current, account, _) = lifecycle.snapshot();
        if current != revision || account != Some(lease.account_id) {
            registration.cancellation.cancel();
        }
        Ok(registration)
    }

    pub(super) fn stop_saved_upload_batch(
        &self,
        account: i64,
        batch: u64,
    ) -> Result<(), ApplicationError> {
        if self.telegram.lifecycle().snapshot().1 != Some(account) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let persistence = |_| ApplicationError::new(ApplicationErrorKind::Persistence);
        let mut db = Database::open(self.library.database_path.as_ref()).map_err(persistence)?;
        let registered = self
            .upload_controls
            .entries
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .keys()
            .filter(|(owner, _)| *owner == account)
            .map(|(_, id)| *id)
            .collect::<Vec<_>>();
        let mut preparing = None;
        for id in registered {
            if let Some(saved) = db.pending_vault_upload(account, id).map_err(persistence)?
                && saved.record.batch_id == batch
                && saved.state == teleark_storage::PendingVaultUploadState::Queued
                && preparing.replace(id).is_some()
            {
                return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
            }
        }
        db.cancel_queued_vault_upload_batch(account, batch, preparing, now_unix_ms()?)
            .map_err(persistence)?;
        // The committed queue state is the restart fence. The live selection
        // token only accelerates its existing between-files scheduling policy.
        if let Ok(active) = self.active_upload_batch.lock()
            && let Some((owner, id, cancel)) = active.as_ref()
            && *owner == account
            && *id == batch
        {
            cancel.store(true, Ordering::Release);
        }
        let ids = self
            .transfers
            .fold(Vec::new(), |mut ids, row| {
                if row.account_id == account
                    && row.batch_id == Some(batch)
                    && row.recovery_state.is_some()
                {
                    ids.push(row.id);
                }
                ids
            })
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        for id in ids {
            let cancelled = db
                .pending_vault_upload(account, id)
                .map_err(persistence)?
                .is_some_and(|saved| {
                    saved.state == teleark_storage::PendingVaultUploadState::Cancelled
                })
                || db
                    .vault_job(account, id)
                    .map_err(persistence)?
                    .is_some_and(|saved| saved.state == VaultJobState::Cancelled);
            if cancelled {
                self.update_transfer(id, |row| {
                    row.state = VaultTransferState::Cancelled;
                    row.recovery_state = Some(VaultJobState::Cancelled);
                    row.upload_activity = None;
                });
                self.persist_upload_id(id)?;
            }
        }
        Ok(())
    }

    pub(super) fn control_pending_upload(
        &self,
        db: &mut Database,
        account: i64,
        id: u64,
        action: VaultUploadControl,
    ) -> Result<(), ApplicationError> {
        use teleark_storage::PendingVaultUploadState as S;
        let saved = db
            .pending_vault_upload(account, id)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        if saved.state == S::Promoted {
            if db
                .vault_job(account, id)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                .is_some()
            {
                return self.control_upload(account, id, action);
            }
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        crate::VaultPendingUploadContext::from_record(&saved.record)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
        let (transition, target) = match (action, saved.state) {
            (VaultUploadControl::Pause, S::Queued) => (
                Some(VaultJobTransition::RequestPause),
                VaultTransferState::Paused,
            ),
            (VaultUploadControl::Pause, S::Paused) => (None, VaultTransferState::Paused),
            (VaultUploadControl::Cancel, S::Queued | S::Paused | S::Retryable | S::Blocked) => (
                Some(VaultJobTransition::RequestCancel),
                VaultTransferState::Cancelled,
            ),
            (VaultUploadControl::Cancel, S::Cancelled) => (None, VaultTransferState::Cancelled),
            _ => return Err(ApplicationError::new(ApplicationErrorKind::Conflict)),
        };
        let lease = VaultJobLease {
            account_id: account,
            id,
            generation: saved.generation,
        };
        if let Some(transition) = transition {
            if !db
                .transition_pending_vault_upload(lease, saved.state, transition, None)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            {
                if db
                    .vault_job(account, id)
                    .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
                    .is_some()
                {
                    return self.control_upload(account, id, action);
                }
                return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
            }
            self.update_transfer(id, |row| {
                row.state = target;
                row.recovery_state = Some(if target == VaultTransferState::Paused {
                    VaultJobState::Paused
                } else {
                    VaultJobState::Cancelled
                });
                row.upload_activity = None;
            });
            self.upload_controls.cancel(lease)?;
        }
        if self.transfers.get(id).is_some() {
            self.persist_upload_id(id)?;
        }
        Ok(())
    }

    pub(super) fn control_upload(
        &self,
        account: i64,
        id: u64,
        action: VaultUploadControl,
    ) -> Result<(), ApplicationError> {
        if self.telegram.lifecycle().snapshot().1 != Some(account) {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        let mut db = Database::open(self.library.database_path.as_ref())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let Some(record) = db
            .vault_job(account, id)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
        else {
            return self.control_pending_upload(&mut db, account, id, action);
        };
        let lease = VaultJobLease {
            account_id: account,
            id,
            generation: record.generation,
        };
        let (transition, target) = match (action, record.state) {
            (VaultUploadControl::Pause, VaultJobState::Running) => (
                Some(VaultJobTransition::RequestPause),
                VaultTransferState::Pausing,
            ),
            (VaultUploadControl::Pause, VaultJobState::Queued) => (
                Some(VaultJobTransition::RequestPause),
                VaultTransferState::Paused,
            ),
            (VaultUploadControl::Pause, VaultJobState::Pausing) => {
                (None, VaultTransferState::Pausing)
            }
            (VaultUploadControl::Pause, VaultJobState::Paused) => {
                (None, VaultTransferState::Paused)
            }
            (VaultUploadControl::Cancel, VaultJobState::Running | VaultJobState::Pausing) => (
                Some(VaultJobTransition::RequestCancel),
                VaultTransferState::Cancelling,
            ),
            (
                VaultUploadControl::Cancel,
                VaultJobState::Queued
                | VaultJobState::Paused
                | VaultJobState::Retryable
                | VaultJobState::Blocked,
            ) => (
                Some(VaultJobTransition::RequestCancel),
                VaultTransferState::Cancelled,
            ),
            (VaultUploadControl::Cancel, VaultJobState::Cancelling) => {
                (None, VaultTransferState::Cancelling)
            }
            (VaultUploadControl::Cancel, VaultJobState::Cancelled) => {
                (None, VaultTransferState::Cancelled)
            }
            _ => return Err(ApplicationError::new(ApplicationErrorKind::Conflict)),
        };
        if transition.is_none() {
            if let Some(state) = self.upload_controls.signal_or_acknowledge(&mut db, lease)? {
                self.update_transfer(id, |row| {
                    row.recovery_state = Some(state);
                    row.state = if state == VaultJobState::Paused {
                        VaultTransferState::Paused
                    } else {
                        VaultTransferState::Cancelled
                    };
                    row.upload_activity = None;
                });
                if self
                    .transfers
                    .get(id)
                    .is_some_and(|row| row.direction == VaultTransferDirection::Upload)
                {
                    self.persist_upload_id(id)?;
                }
            }
            return Ok(());
        }
        if let Some(transition) = transition
            && !db
                .transition_vault_job(lease, record.state, transition, now_unix_ms()?, None)
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
        {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        // Publish intent before waking the sender, so its acknowledgment cannot
        // be overwritten by a late intent update from this control owner.
        let durable_state = match target {
            VaultTransferState::Pausing => VaultJobState::Pausing,
            VaultTransferState::Paused => VaultJobState::Paused,
            VaultTransferState::Cancelling => VaultJobState::Cancelling,
            _ => VaultJobState::Cancelled,
        };
        self.update_transfer(id, |row| {
            row.recovery_state = Some(durable_state);
            row.state = target;
            row.upload_activity = None;
        });
        if let Some(state) = self.upload_controls.signal_or_acknowledge(&mut db, lease)? {
            self.update_transfer(id, |row| {
                row.recovery_state = Some(state);
                row.state = if state == VaultJobState::Paused {
                    VaultTransferState::Paused
                } else {
                    VaultTransferState::Cancelled
                };
            });
        }
        if self
            .transfers
            .get(id)
            .is_some_and(|row| row.direction == VaultTransferDirection::Upload)
        {
            self.persist_upload_id(id)?;
        }
        Ok(())
    }
}

pub(super) fn finish_job(
    database: &mut Database,
    lease: VaultJobLease,
    failure: Option<ApplicationErrorKind>,
) -> Result<VaultJobState, ApplicationError> {
    for _ in 0..4 {
        let record = database
            .vault_job(lease.account_id, lease.id)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?;
        if record.generation != lease.generation {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        let (transition, target, code) = match record.state {
            VaultJobState::Pausing => (
                VaultJobTransition::AcknowledgePause,
                VaultJobState::Paused,
                None,
            ),
            VaultJobState::Cancelling => (
                VaultJobTransition::AcknowledgeCancel,
                VaultJobState::Cancelled,
                None,
            ),
            VaultJobState::Running => {
                if let Some(kind) = failure {
                    if retryable_upload_failure(kind) {
                        (
                            VaultJobTransition::FailRetryable,
                            VaultJobState::Retryable,
                            Some(upload_history::error_code(kind)),
                        )
                    } else {
                        (
                            VaultJobTransition::FailBlocked,
                            VaultJobState::Blocked,
                            Some(upload_history::error_code(kind)),
                        )
                    }
                } else {
                    (VaultJobTransition::Complete, VaultJobState::Completed, None)
                }
            }
            VaultJobState::Paused | VaultJobState::Cancelled => return Ok(record.state),
            _ => return Err(ApplicationError::new(ApplicationErrorKind::Conflict)),
        };
        if database
            .transition_vault_job(lease, record.state, transition, now_unix_ms()?, code)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
        {
            return Ok(target);
        }
    }
    Err(ApplicationError::new(ApplicationErrorKind::Conflict))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn controls_acknowledge_orphans_but_preserve_newer_and_live_owners()
    -> Result<(), Box<dyn std::error::Error>> {
        for (intent, pending, stopped) in [
            (
                VaultJobTransition::RequestPause,
                VaultJobState::Pausing,
                VaultJobState::Paused,
            ),
            (
                VaultJobTransition::RequestCancel,
                VaultJobState::Cancelling,
                VaultJobState::Cancelled,
            ),
        ] {
            let dir = tempfile::tempdir()?;
            let mut db = Database::open(dir.path().join("catalog.db"))?;
            db.admit_vault_job(&teleark_storage::VaultJobRecord {
                account_id: 7,
                id: 1,
                chat_id: 11,
                direction: teleark_storage::VaultJobDirection::Upload,
                package_id: [1; 16],
                context_version: 1,
                context: vec![1],
                state: VaultJobState::Queued,
                generation: 0,
                created_at_unix_ms: 100,
                updated_at_unix_ms: 100,
                failure_code: None,
            })?;
            let old = VaultJobLease {
                account_id: 7,
                id: 1,
                generation: 0,
            };
            assert!(db.transition_vault_job(
                old,
                VaultJobState::Queued,
                VaultJobTransition::Start,
                101,
                None
            )?);
            let lease = VaultJobLease {
                generation: 1,
                ..old
            };
            let controls = UploadControls::default();
            assert_eq!(controls.signal_or_acknowledge(&mut db, lease)?, None);
            assert_eq!(
                db.vault_job(7, 1)?.expect("running").state,
                VaultJobState::Running
            );
            let registration = controls.register(lease)?;
            assert!(db.transition_vault_job(lease, VaultJobState::Running, intent, 102, None)?);
            assert_eq!(controls.signal_or_acknowledge(&mut db, old)?, None);
            assert!(!registration.cancellation.is_cancelled());
            assert_eq!(controls.signal_or_acknowledge(&mut db, lease)?, None);
            assert!(registration.cancellation.is_cancelled());
            assert_eq!(
                db.vault_job(7, 1)?.expect("waiting for owner").state,
                pending
            );
            drop(registration);
            assert_eq!(
                controls.signal_or_acknowledge(&mut db, lease)?,
                Some(stopped)
            );
            assert_eq!(db.vault_job(7, 1)?.expect("stopped").state, stopped);
            assert_eq!(controls.signal_or_acknowledge(&mut db, lease)?, None);
        }
        Ok(())
    }

    #[test]
    fn control_queue_persists_intent_and_wakes_a_blocked_locked_upload()
    -> Result<(), Box<dyn std::error::Error>> {
        for (action, pending, stopped, direction) in [
            (
                VaultUploadControl::Pause,
                VaultJobState::Pausing,
                VaultJobState::Paused,
            ),
            (
                VaultUploadControl::Cancel,
                VaultJobState::Cancelling,
                VaultJobState::Cancelled,
            ),
        ]
        .into_iter()
        .flat_map(|(action, pending, stopped)| {
            [
                teleark_storage::VaultJobDirection::Upload,
                teleark_storage::VaultJobDirection::Download,
            ]
            .map(move |direction| (action, pending, stopped, direction))
        }) {
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
            db.admit_vault_job(&teleark_storage::VaultJobRecord {
                account_id: 7,
                id: 1,
                chat_id: 11,
                direction,
                package_id: [1; 16],
                context_version: 1,
                context: vec![1],
                state: VaultJobState::Queued,
                generation: 0,
                created_at_unix_ms: 100,
                updated_at_unix_ms: 100,
                failure_code: None,
            })?;
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
            let command = vault.submit_transfer_control(7, 1, action)?;
            let (finished, done) = mpsc::channel();
            let control = thread::spawn(move || {
                let _ = finished.send(command.wait());
            });
            let result = done.recv_timeout(Duration::from_secs(5));
            let saved = db.vault_job(7, 1)?.expect("pending intent");
            let signalled = cancellation.is_cancelled();
            let _ = release.send(());
            control.join().expect("control owner waiter");
            result??;
            assert_eq!(saved.state, pending);
            assert!(
                signalled,
                "intent wakes transport before the busy owner finishes"
            );
            work.wait()?;
            assert_eq!(
                db.vault_job(7, 1)?.expect("acknowledged intent").state,
                stopped
            );
            assert!(vault.inner.session.lock().expect("session").status.locked);
        }
        Ok(())
    }
}
