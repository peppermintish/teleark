//! Automatic manifest synchronization. Backoff never occupies a Vault worker.
use super::*;
use std::time::{Duration, Instant};

impl DesktopVault {
    /// Retained background key validation. Backoff releases the key owner;
    /// locking, backend changes and account replacement fence every retry.
    pub fn synchronize_channel_key(
        &self,
        account: i64,
        chat: i64,
        progress: VaultKeyProgress,
        observer: Option<crate::ManagedScanObserver>,
    ) -> Result<VaultKeySelection, ApplicationError> {
        let result =
            self.synchronize_channel_key_inner(account, chat, &progress, observer.as_ref());
        progress.finish(result.as_ref().err().map(ApplicationError::kind));
        if let Some(observer) = observer {
            observer.finish(result.as_ref().err().map(ApplicationError::kind));
        }
        result
    }

    fn synchronize_channel_key_inner(
        &self,
        account: i64,
        chat: i64,
        progress: &VaultKeyProgress,
        observer: Option<&crate::ManagedScanObserver>,
    ) -> Result<VaultKeySelection, ApplicationError> {
        if chat <= 0 {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let scope = self.channel_setup_scope(account)?;
        let mut revision = scope.session_revision;
        let current =
            |expected| {
                progress.check_cancelled().is_ok()
                    && {
                        let (generation, active, _) = self.inner.lifecycle.snapshot();
                        generation == scope.telegram_revision && active == Some(account)
                    }
                    && self.inner.session.lock().is_ok_and(|session| {
                        !session.closing && session.scan_revision() == expected
                    })
            };
        let mut attempt = 0_u32;
        loop {
            if !current(revision) {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            if let Some(observer) = observer {
                observer.restart();
                observer.phase(crate::ChannelSyncPhase::ManifestVerifying);
            }
            let job =
                self.submit_in_session(Some(revision), |reply| VaultCommand::SelectChannelKey {
                    account,
                    chat,
                    initialize_empty: Some(scope),
                    finish_progress: false,
                    progress: progress.clone(),
                    reply,
                });
            // Admission revokes new scan leases once. Any other key mutation
            // during a failed attempt invalidates this exact retry generation.
            revision.1 = revision.1.wrapping_add(1);
            let result = match job {
                Ok(job) => job.wait(),
                Err(error) => Err(error),
            };
            match result {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let Some(delay) = retry_delay(attempt, &error) else {
                        return Err(error);
                    };
                    progress.waiting(error.kind(), delay)?;
                    if let Some(observer) = observer {
                        observer.retry(&error, delay);
                    }
                    let start = Instant::now();
                    while current(revision) && start.elapsed() < delay {
                        std::thread::sleep(
                            delay
                                .saturating_sub(start.elapsed())
                                .min(Duration::from_millis(50)),
                        );
                    }
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }

    /// Run on a retained background task. Each attempt has its own admitted key
    /// lease; a lock, key change or account replacement prevents further attempts.
    /// Successful authenticated entries remain cached across transient failures.
    pub fn synchronize_managed_files(
        &self,
        account_id: i64,
        chat_id: i64,
        cancellation: crate::TelegramScanCancellation,
        observer: Option<crate::ManagedScanObserver>,
    ) -> Result<ManagedVaultScan, ApplicationError> {
        let observer = observer.unwrap_or_else(|| crate::ManagedScanObserver::silent(chat_id));
        let revision = self
            .inner
            .session
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .scan_revision();
        let current = || {
            !cancellation.is_cancelled()
                && self
                    .inner
                    .lifecycle
                    .snapshot()
                    .1
                    .is_none_or(|account| account == account_id)
                && self
                    .inner
                    .session
                    .lock()
                    .is_ok_and(|session| session.scan_revision() == revision)
        };
        run(
            &observer,
            current,
            || {
                self.submit_in_session(Some(revision), |reply| VaultCommand::Scan {
                    account_id,
                    chat_id,
                    verify_health: false,
                    cancellation: cancellation.clone(),
                    observer: Some(observer.clone()),
                    reply,
                })?
                .wait()
            },
            |delay| {
                // This is a service retry deadline, never a UI refresh timer.
                // The scan, key, transfer and control owners remain available.
                let start = Instant::now();
                while current() && start.elapsed() < delay {
                    std::thread::sleep(
                        delay
                            .saturating_sub(start.elapsed())
                            .min(Duration::from_millis(50)),
                    );
                }
            },
        )
    }
}

fn retry_delay(attempt: u32, error: &ApplicationError) -> Option<Duration> {
    if !matches!(
        error.kind(),
        ApplicationErrorKind::Network
            | ApplicationErrorKind::Server
            | ApplicationErrorKind::Conflict
    ) {
        return None;
    }
    Some(
        error
            .retry_after()
            .unwrap_or_else(|| Duration::from_secs((2_u64 << attempt.min(5)).min(60))),
    )
}

fn run<T>(
    observer: &crate::ManagedScanObserver,
    current: impl Fn() -> bool,
    mut scan: impl FnMut() -> Result<T, ApplicationError>,
    mut wait: impl FnMut(Duration),
) -> Result<T, ApplicationError> {
    let mut attempt = 0_u32;
    let result = loop {
        if !current() {
            break Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        observer.restart();
        match scan() {
            Ok(value) => {
                break if current() {
                    Ok(value)
                } else {
                    Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
                };
            }
            Err(error) => {
                let Some(delay) = retry_delay(attempt, &error) else {
                    break Err(error);
                };
                observer.retry(&error, delay);
                wait(delay);
                attempt = attempt.saturating_add(1);
            }
        }
    };
    observer.finish(result.as_ref().err().map(ApplicationError::kind));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn unlocking_then_syncing_recovers_a_real_encrypted_manifest_after_wire_failure() {
        use crate::telegram::test_vault_remote::{GateKind, TestVaultRemote};
        let temp = tempfile::tempdir().expect("synthetic fixture");
        let library =
            DesktopLibrary::open(temp.path().join("catalog.sqlite")).expect("synthetic fixture");
        let remote = TestVaultRemote::new();
        let telegram = remote.connect(&temp.path().join("synthetic.session"));
        let vault = DesktopVault::new(telegram, library.clone()).expect("synthetic fixture");
        vault
            .initialize("synthetic sync password".into())
            .expect("synthetic fixture");
        let source = temp.path().join("example.txt");
        std::fs::write(&source, b"synthetic sync fixture").expect("synthetic fixture");
        let uploaded = vault
            .submit_upload_files(7, 11, vec![source])
            .expect("synthetic fixture")
            .wait()
            .expect("synthetic fixture");
        assert_eq!(uploaded.completed.len(), 1);
        library
            .save_telegram_sources(
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
            )
            .expect("synthetic fixture");
        library
            .cache_telegram_files(7, 11, &remote.summaries())
            .expect("synthetic fixture");
        vault.lock().expect("test state");
        remote.fail_once(GateKind::ManifestDownload);
        let downloads = remote.downloads().len();
        vault
            .unlock_with_password("synthetic sync password".into())
            .expect("synthetic fixture");
        assert_eq!(
            remote.downloads().len(),
            downloads,
            "unlock is local even with a pending network failure"
        );
        let (waiting, ready) = mpsc::sync_channel(1);
        let observer = crate::ManagedScanObserver::new(
            11,
            Arc::new(move |state, transitioned| {
                if transitioned && state.phase == crate::ChannelSyncPhase::Waiting {
                    waiting.send(()).expect("synthetic fixture");
                }
            }),
        );
        let work = vault.clone();
        let job = thread::spawn(move || {
            work.synchronize_managed_files(
                7,
                11,
                crate::TelegramScanCancellation::new(),
                Some(observer),
            )
        });
        ready
            .recv_timeout(Duration::from_secs(10))
            .expect("synthetic fixture");
        assert!(!vault.status().locked, "sync failure must not undo unlock");
        // An independent scan can run during backoff. It consumes no key operation
        // and proves the retry deadline does not occupy the scan worker.
        let scan = vault
            .scan_cached_managed_files(7, 12, crate::TelegramScanCancellation::new())
            .expect("synthetic fixture");
        assert!(scan.files.is_empty());
        let result = job
            .join()
            .expect("synthetic fixture")
            .expect("synthetic fixture");
        assert_eq!(result.files[0].logical_name, "example.txt");
        assert_eq!(
            remote.downloads().len() - downloads,
            3,
            "failed manifest retries and the pending envelope is authenticated once"
        );
    }

    #[test]
    fn stale_session_cannot_admit_a_retry_after_lock_and_unlock() {
        let temp = tempfile::tempdir().expect("synthetic fixture");
        let library =
            DesktopLibrary::open(temp.path().join("catalog.sqlite")).expect("synthetic fixture");
        let telegram = DesktopTelegram::open_direct(temp.path().join("synthetic.session"))
            .expect("synthetic fixture");
        let vault = DesktopVault::new(telegram, library).expect("synthetic fixture");
        vault
            .initialize("synthetic sync password".into())
            .expect("synthetic fixture");
        let revision = vault
            .inner
            .session
            .lock()
            .expect("test state")
            .scan_revision();
        vault.lock().expect("test state");
        vault
            .unlock_with_password("synthetic sync password".into())
            .expect("synthetic fixture");
        let result = vault.submit_in_session(Some(revision), |reply| VaultCommand::Scan {
            account_id: 7,
            chat_id: 11,
            verify_health: false,
            cancellation: crate::TelegramScanCancellation::new(),
            observer: None,
            reply,
        });
        assert!(matches!(result, Err(error) if error.kind() == ApplicationErrorKind::Cancelled));
    }

    #[test]
    fn retries_without_new_events_and_keeps_server_deadline_and_feedback() {
        let states = Arc::new(Mutex::new(Vec::new()));
        let output = states.clone();
        let observer = crate::ManagedScanObserver::new(
            11,
            Arc::new(move |state, _| output.lock().expect("test state").push(state)),
        );
        let calls = Cell::new(0);
        let mut delays = Vec::new();
        let value = run(
            &observer,
            || true,
            || {
                let attempt = calls.get();
                calls.set(attempt + 1);
                assert_eq!(
                    states
                        .lock()
                        .expect("test state")
                        .last()
                        .expect("published status")
                        .phase,
                    crate::ChannelSyncPhase::ManifestQueued
                );
                match attempt {
                    0 => Err(ApplicationError::new(ApplicationErrorKind::Network)),
                    1 => Err(ApplicationError::new(ApplicationErrorKind::Network)
                        .with_retry_after(Duration::from_secs(120))),
                    _ => Ok(42),
                }
            },
            |delay| {
                let state = states
                    .lock()
                    .expect("test state")
                    .last()
                    .expect("published status")
                    .clone();
                assert!(state.active());
                assert!(state.retry_after.is_some());
                assert_eq!(state.failure, Some(ApplicationErrorKind::Network));
                delays.push(delay);
            },
        )
        .expect("synthetic fixture");
        assert_eq!(value, 42);
        assert_eq!(calls.get(), 3);
        assert_eq!(delays, [Duration::from_secs(2), Duration::from_secs(120)]);
        let states = states.lock().expect("test state");
        assert_eq!(
            states.last().expect("published status").phase,
            crate::ChannelSyncPhase::ManifestCompleted
        );
        assert!(states.last().expect("published status").failure.is_none());
        assert!(
            states
                .last()
                .expect("published status")
                .retry_after
                .is_none()
        );
    }

    #[test]
    fn cancellation_or_changed_session_stops_retry_and_permanent_errors_do_not_loop() {
        let observer = crate::ManagedScanObserver::silent(11);
        let current = Cell::new(true);
        let calls = Cell::new(0);
        let result: Result<(), _> = run(
            &observer,
            || current.get(),
            || {
                calls.set(calls.get() + 1);
                Err(ApplicationError::new(ApplicationErrorKind::Network))
            },
            |_| current.set(false),
        );
        assert_eq!(
            result.expect_err("expected rejection").kind(),
            ApplicationErrorKind::Cancelled
        );
        assert_eq!(calls.get(), 1);
        current.set(true);
        let late = run(
            &observer,
            || current.get(),
            || {
                current.set(false);
                Ok(42)
            },
            |_| panic!("no retry"),
        );
        assert_eq!(
            late.expect_err("expected rejection").kind(),
            ApplicationErrorKind::Cancelled
        );
        for kind in [
            ApplicationErrorKind::Authorization,
            ApplicationErrorKind::Persistence,
            ApplicationErrorKind::Cancelled,
            ApplicationErrorKind::StorageAccessDenied,
            ApplicationErrorKind::VaultKeyUnavailable,
            ApplicationErrorKind::SourceMissing,
        ] {
            let result: Result<(), _> = run(
                &observer,
                || true,
                || Err(ApplicationError::new(kind)),
                |_| panic!("permanent error retried"),
            );
            assert_eq!(result.expect_err("expected rejection").kind(), kind);
        }
        assert_eq!(
            retry_delay(
                u32::MAX,
                &ApplicationError::new(ApplicationErrorKind::Network)
            ),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn server_wait_survives_object_adapter_round_trip() {
        let delay = Duration::from_secs(123);
        let remote = crate::transfer::map_application_error(
            ApplicationError::new(ApplicationErrorKind::Network).with_retry_after(delay),
        );
        let error = map_transfer_error(remote);
        assert_eq!(error.retry_after(), Some(delay));
        assert_eq!(retry_delay(0, &error), Some(delay));
        let blocked = map_transfer_error(crate::transfer::map_application_error(
            ApplicationError::new(ApplicationErrorKind::StorageAccessDenied),
        ));
        assert_eq!(blocked.kind(), ApplicationErrorKind::PermissionDenied);
        assert_eq!(retry_delay(0, &blocked), None);
        let reconnect = map_transfer_error(crate::transfer::map_application_error(
            ApplicationError::new(ApplicationErrorKind::Conflict),
        ));
        assert_eq!(retry_delay(0, &reconnect), Some(Duration::from_secs(2)));
    }
}
