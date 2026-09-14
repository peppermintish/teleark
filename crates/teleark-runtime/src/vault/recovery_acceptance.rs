//! Successful service composition with real crypto/files/SQLite and fake wire replies.
use super::*;
use crate::telegram::test_vault_remote::{GateKind, TestVaultRemote};
use std::time::Duration;
use teleark_storage::{Database, PendingVaultUploadState, VaultJobDirection, VaultJobState};
const PASSWORD: &str = "synthetic recovery test password";
fn open(path: &std::path::Path, remote: &Arc<TestVaultRemote>) -> (DesktopVault, DesktopLibrary) {
    let library = DesktopLibrary::open(path.join("catalog.sqlite")).expect("temporary library");
    let telegram = remote.connect(&path.join("synthetic.session"));
    let vault = DesktopVault::new(telegram, library.clone()).expect("Vault owners");
    (vault, library)
}

#[cfg(unix)]
#[test]
fn metadata_only_changes_during_preparation_and_transport_preserve_authenticated_upload() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    vault.initialize(PASSWORD.into()).expect("keys");
    let source = dir.path().join("metadata.bin");
    let bytes = vec![0x5a; 64 * 1024];
    std::fs::write(&source, &bytes).expect("source");
    let original = std::fs::metadata(&source).expect("metadata");
    let (entered, release) = remote.gate(GateKind::Validation, 0);
    let work = vault
        .submit_upload_files(7, 11, vec![source.clone()])
        .expect("upload");
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("saved queue");
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600))
        .expect("metadata-only update");
    let changed = std::fs::metadata(&source).expect("changed metadata");
    assert_eq!(original.ino(), changed.ino());
    assert_eq!(original.len(), changed.len());
    assert_eq!(
        original.modified().expect("original mtime"),
        changed.modified().expect("current mtime")
    );
    assert_ne!(
        (original.ctime(), original.ctime_nsec()),
        (changed.ctime(), changed.ctime_nsec())
    );
    let (sending, finish) = remote.gate(GateKind::UploadPart, 0);
    release.send(()).expect("release preparation");
    if sending.recv_timeout(Duration::from_secs(30)).is_err() {
        panic!("metadata changes must reach transport: {:?}", work.wait());
    }
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640))
        .expect("metadata changes while upload is blocked");
    finish.send(()).expect("release transport");
    let report = work.wait().expect("batch");
    assert_eq!(report.completed_count, 1);
    let id = vault
        .transfers()
        .iter()
        .find(|row| row.direction == VaultTransferDirection::Upload)
        .expect("upload row")
        .id;
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let saved = db.vault_job(7, id).expect("job").expect("completed job");
    assert_eq!(saved.state, VaultJobState::Completed);
    let package = crate::transfer::package_id_from_bytes(saved.package_id)
        .expect("package")
        .get();
    let downloaded = vault
        .download_file(7, 11, package)
        .expect("authenticated download");
    assert_eq!(std::fs::read(downloaded).expect("plaintext"), bytes);
    assert_eq!(remote.objects(), 2, "exactly one part and manifest");
}

#[test]
fn account_replacement_interrupts_retained_upload_and_preserves_retry() {
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let library = DesktopLibrary::open(dir.path().join("catalog.sqlite")).expect("library");
    let telegram = remote.connect(&dir.path().join("synthetic.session"));
    let lifecycle = telegram.lifecycle();
    let vault = DesktopVault::new(telegram, library.clone()).expect("owners");
    vault.initialize(PASSWORD.into()).expect("keys");
    let source = dir.path().join("account-bound.bin");
    std::fs::write(&source, vec![9; 64 * 1024]).expect("source");
    let (entered, release) = remote.gate(GateKind::UploadPart, 0);
    let work = vault
        .submit_upload_files(7, 11, vec![source])
        .expect("upload");
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("transport");
    let id = vault
        .transfers()
        .iter()
        .find(|row| row.state == VaultTransferState::Running)
        .expect("running")
        .id;
    lifecycle.publish(u64::MAX, Some(8), None);
    release.send(()).expect("release transport");
    let _ = work.wait();
    let db = Database::open(library.database_path.as_ref()).expect("independent database");
    let saved = db.vault_job(7, id).expect("job").expect("saved");
    assert_eq!(saved.state, VaultJobState::Retryable);
    assert!(db.vault_job(8, id).expect("other account").is_none());
    assert!(
        vault
            .transfers()
            .iter()
            .all(|row| row.state != VaultTransferState::Completed)
    );
}

#[test]
fn batch_stop_commits_while_current_transport_is_blocked_and_survives_restart() {
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    let mut preferences = library.preferences().expect("preferences");
    preferences.transfer_tuning.upload_tasks = 1;
    library
        .set_preferences(&preferences)
        .expect("one-task stop-policy fixture");
    vault.initialize(PASSWORD.into()).expect("keys");
    let sources = (0..3)
        .map(|id| {
            let path = dir.path().join(format!("member-{id}.bin"));
            std::fs::write(&path, vec![id as u8; 64 * 1024]).expect("synthetic source");
            path
        })
        .collect::<Vec<_>>();
    let (entered, release) = remote.gate(GateKind::UploadPart, 0);
    let work = vault.submit_upload_files(7, 11, sources).expect("batch");
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("blocked current transport");
    let rows = vault.transfers();
    let current = rows
        .iter()
        .find(|row| row.state == VaultTransferState::Running)
        .expect("current");
    let current_id = current.id;
    let batch = current.batch_id.expect("batch id");
    let ids = rows
        .iter()
        .filter(|row| row.batch_id == Some(batch) && row.id != current_id)
        .map(|row| row.id)
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2);
    let stop = vault
        .submit_stop_upload_batch(7, batch)
        .expect("independent stop admission");
    let (done, stopped) = std::sync::mpsc::sync_channel(1);
    let waiter = std::thread::spawn(move || {
        let _ = done.send(stop.wait());
    });
    let result = stopped.recv_timeout(Duration::from_secs(10));
    if result.is_err() {
        let _ = release.send(());
    }
    result
        .expect("stop acknowledgment must not wait for transport")
        .expect("saved stop");
    waiter.join().expect("control waiter");
    let db = Database::open(library.database_path.as_ref()).expect("independent reader");
    assert_eq!(
        db.vault_job(7, current_id)
            .expect("current job")
            .expect("formal")
            .state,
        VaultJobState::Running
    );
    for id in &ids {
        assert_eq!(
            db.pending_vault_upload(7, *id)
                .expect("pending")
                .expect("saved")
                .state,
            PendingVaultUploadState::Cancelled
        );
    }
    release.send(()).expect("finish current transport");
    let report = work.wait().expect("batch completion");
    assert_eq!(report.completed_count, 1);
    assert_eq!(report.cancelled.len(), 2);
    let objects = remote.objects();
    assert_eq!(objects, 2, "one encrypted part and one manifest only");
    drop(db);
    drop(vault);
    drop(library);
    let (vault, library) = open(dir.path(), &remote);
    vault.restore_upload_history(7).expect("restored history");
    vault.unlock_with_password(PASSWORD.into()).expect("unlock");
    vault
        .submit_resume_queued_transfers(7)
        .expect("startup queue")
        .wait()
        .expect("queue restoration");
    assert_eq!(
        remote.objects(),
        objects,
        "cancelled members publish nothing after restart"
    );
    let db = Database::open(library.database_path.as_ref()).expect("reopened database");
    for id in ids {
        assert_eq!(
            db.pending_vault_upload(7, id)
                .expect("pending")
                .expect("retained")
                .state,
            PendingVaultUploadState::Cancelled
        );
        assert!(db.vault_job(7, id).expect("no formal work").is_none());
    }
}

#[cfg(unix)]
#[test]
fn pending_alias_upload_survives_restart_and_downloads_authenticated_original_bytes() {
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    vault
        .initialize(PASSWORD.into())
        .expect("real key initialization");
    let bytes = vec![0x5a; 2 * 1024 * 1024];
    let source = dir.path().join("canonical.bin");
    std::fs::write(&source, &bytes).expect("source");
    let alias = dir.path().join("Selected name.txt");
    std::os::unix::fs::symlink(&source, &alias).expect("selected alias");
    let (entered, release) = remote.gate(GateKind::Validation, 0);
    let work = vault
        .submit_upload_files(7, 11, vec![alias.clone()])
        .expect("submit real batch");
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("durable queue before validation");
    let id = vault
        .transfers()
        .iter()
        .find(|row| row.direction == VaultTransferDirection::Upload)
        .expect("queued task")
        .id;
    vault
        .submit_transfer_control(7, id, control::VaultUploadControl::Pause)
        .expect("pause request")
        .wait()
        .expect("durable pause");
    release.send(()).expect("release validation");
    let report = work.wait().expect("batch stopped safely");
    assert_eq!(report.paused_count, 1);
    assert_eq!(report.completed_count, 0);
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let saved = db
        .pending_vault_upload(7, id)
        .expect("pending")
        .expect("record");
    assert_eq!(saved.state, PendingVaultUploadState::Paused);
    assert!(db.vault_job(7, id).expect("formal job").is_none());
    assert_eq!(remote.objects(), 0);
    drop(vault);
    drop(library);
    drop(db);

    let (vault, library) = open(dir.path(), &remote);
    vault.restore_upload_history(7).expect("locked history");
    let restored = vault
        .transfers()
        .into_iter()
        .find(|row| row.id == id)
        .expect("same task");
    assert_eq!(restored.file_name, "Selected name.txt");
    assert_eq!(restored.state, VaultTransferState::Paused);
    vault
        .unlock_with_password(PASSWORD.into())
        .expect("real key recovery");
    let file = vault
        .submit_resume_upload(7, id)
        .expect("resume original task")
        .wait()
        .expect("successful complete upload");
    assert_eq!(file.logical_name, "Selected name.txt");
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let job = db.vault_job(7, id).expect("job").expect("formal");
    assert_eq!(job.state, VaultJobState::Completed);
    let completed = vault
        .transfers()
        .into_iter()
        .find(|row| row.id == id)
        .expect("completed projection");
    assert_eq!(completed.state, VaultTransferState::Completed);
    assert_eq!(completed.part_count, 1);
    assert_eq!(completed.completed_parts, 1);

    assert_eq!(
        db.pending_vault_upload(7, id)
            .expect("pending")
            .expect("receipt")
            .state,
        PendingVaultUploadState::Promoted
    );
    let package = crate::transfer::package_id_from_bytes(job.package_id)
        .expect("package")
        .get();
    assert_eq!(
        remote.objects(),
        2,
        "one authenticated part and one manifest"
    );
    let downloaded = vault
        .download_file(7, 11, package)
        .expect("successful authenticated download");
    assert!(downloaded.starts_with(dir.path()));
    assert_eq!(
        downloaded.file_name().and_then(|name| name.to_str()),
        Some("Selected name.txt")
    );
    assert_eq!(std::fs::read(&downloaded).expect("downloaded bytes"), bytes);
    assert_eq!(std::fs::read(&source).expect("unchanged source"), bytes);
    let downloads = db
        .vault_job_ids_by_direction(7, VaultJobDirection::Download, None, 256)
        .expect("download ledger");
    assert_eq!(downloads.len(), 1);
    assert_eq!(
        db.vault_job(7, downloads[0])
            .expect("download")
            .expect("job")
            .state,
        VaultJobState::Completed
    );
    assert_eq!(remote.uploads().len(), 2);
    assert!(!remote.downloads().is_empty());
}

#[test]
fn download_paused_during_transport_resumes_same_task_after_restart() {
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    vault.initialize(PASSWORD.into()).expect("initialize");
    let bytes = vec![0xa7; 2 * 1024 * 1024];
    let source = dir.path().join("original.bin");
    std::fs::write(&source, &bytes).expect("source");
    vault.upload_file(7, 11, source.clone()).expect("upload");
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let upload = vault
        .transfers()
        .into_iter()
        .find(|row| row.direction == VaultTransferDirection::Upload)
        .expect("upload row");
    let job = db
        .vault_job(7, upload.id)
        .expect("job")
        .expect("upload ledger");
    let package = crate::transfer::package_id_from_bytes(job.package_id)
        .expect("package")
        .get();
    let (entered, release) = remote.gate(GateKind::DownloadPart, 0);
    let work = vault
        .submit_download_file(7, 11, package)
        .expect("download");
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("part transport entered");
    let id = vault
        .transfers()
        .into_iter()
        .find(|row| row.direction == VaultTransferDirection::Download)
        .expect("download row")
        .id;
    vault
        .submit_transfer_control(7, id, control::VaultUploadControl::Pause)
        .expect("pause")
        .wait()
        .expect("durable pause");
    release.send(()).expect("release transport");
    assert!(work.wait().is_err());
    assert_eq!(
        db.vault_job(7, id)
            .expect("job")
            .expect("download ledger")
            .state,
        VaultJobState::Paused
    );
    drop(vault);
    drop(library);
    drop(db);
    let (vault, library) = open(dir.path(), &remote);
    vault
        .restore_upload_history(7)
        .expect("restore locked history");
    assert_eq!(
        vault
            .transfers()
            .into_iter()
            .find(|row| row.id == id)
            .expect("same task")
            .state,
        VaultTransferState::Paused
    );
    vault.unlock_with_password(PASSWORD.into()).expect("unlock");
    let output = vault
        .submit_resume_download(7, id)
        .expect("resume")
        .wait()
        .expect("authenticated completion");
    assert_eq!(std::fs::read(&output).expect("output"), bytes);
    assert_eq!(std::fs::read(&source).expect("source preserved"), bytes);
    let db = Database::open(library.database_path.as_ref()).expect("database");
    assert_eq!(
        db.vault_job(7, id)
            .expect("job")
            .expect("same ledger")
            .state,
        VaultJobState::Completed
    );
    assert_eq!(
        db.vault_job_ids_by_direction(7, VaultJobDirection::Download, None, 256)
            .expect("download IDs"),
        vec![id]
    );
}

#[test]
fn upload_paused_during_transport_retires_unpublished_ciphertext_after_restart() {
    upload_transport_restart(GateKind::UploadPart, false);
}

#[test]
fn manifest_publication_resumes_after_restart_without_original_source() {
    upload_transport_restart(GateKind::ManifestUpload, true);
}

#[test]
fn published_part_without_acknowledgment_is_reconciled_after_restart() {
    upload_transport_restart(GateKind::PartAcknowledgment, false);
}

#[test]
fn published_manifest_without_acknowledgment_resumes_without_source() {
    upload_transport_restart(GateKind::ManifestAcknowledgment, true);
}

fn upload_transport_restart(gate: GateKind, remove_source: bool) {
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    vault.initialize(PASSWORD.into()).expect("initialize");
    let bytes = vec![0x31; 2 * 1024 * 1024];
    let source = dir.path().join("resume.bin");
    std::fs::write(&source, &bytes).expect("source");
    let (entered, release) = remote.gate(gate, 0);
    let work = vault
        .submit_upload_files(7, 11, vec![source.clone()])
        .expect("batch");
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("transport entered");
    let id = vault
        .transfers()
        .into_iter()
        .find(|row| row.direction == VaultTransferDirection::Upload)
        .expect("upload row")
        .id;
    let interrupted = *remote.uploads().last().expect("attempt before pause");
    let acknowledgment_lost = matches!(
        gate,
        GateKind::PartAcknowledgment | GateKind::ManifestAcknowledgment
    );
    if acknowledgment_lost {
        let db = Database::open(library.database_path.as_ref()).expect("database");
        assert_eq!(remote.objects(), if remove_source { 2 } else { 1 });
        if remove_source {
            let outbox = db
                .vault_manifest_outbox(7, id)
                .expect("outbox")
                .expect("saved envelope");
            assert!(outbox.envelope.is_some());
            assert!(
                outbox.message_id.is_none(),
                "remote publication not yet receipted"
            );
        } else {
            assert!(
                db.vault_parts(7, id, None, 256)
                    .expect("parts")
                    .iter()
                    .all(|part| part.receipt.is_none())
            );
        }
    }

    vault
        .submit_transfer_control(7, id, control::VaultUploadControl::Pause)
        .expect("pause")
        .wait()
        .expect("durable pause");
    release.send(()).expect("release transport");
    let report = work.wait().expect("stopped batch");
    assert_eq!(report.paused_count, 1);
    let db = Database::open(library.database_path.as_ref()).expect("database");
    assert_eq!(
        db.vault_job(7, id).expect("job").expect("ledger").state,
        VaultJobState::Paused
    );
    drop(vault);
    drop(library);
    drop(db);
    if remove_source {
        std::fs::remove_file(&source).expect("remove only synthetic source");
    }
    let (vault, library) = open(dir.path(), &remote);
    vault.restore_upload_history(7).expect("locked history");
    vault.unlock_with_password(PASSWORD.into()).expect("unlock");
    let file = vault
        .submit_resume_upload(7, id)
        .expect("resume")
        .wait()
        .expect("complete upload");
    assert_eq!(file.logical_name, "resume.bin");
    let matching: Vec<_> = remote
        .uploads()
        .into_iter()
        .filter(|(publication, _)| *publication == interrupted.0)
        .collect();
    assert_eq!(
        matching,
        if gate == GateKind::ManifestUpload {
            vec![interrupted, interrupted]
        } else {
            vec![interrupted]
        },
        "containers retire uncommitted ciphertext; a sealed metadata envelope replays exact bytes"
    );
    if !acknowledgment_lost && gate != GateKind::ManifestUpload {
        assert!(
            remote
                .uploads()
                .iter()
                .any(|(publication, digest)| *publication != interrupted.0
                    && *digest != interrupted.1),
            "fresh identity produces fresh ciphertext"
        );
    }
    assert_eq!(remote.objects(), 2, "no duplicate publication");
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let job = db.vault_job(7, id).expect("job").expect("same ledger");
    assert_eq!(job.state, VaultJobState::Completed);
    let package = crate::transfer::package_id_from_bytes(job.package_id)
        .expect("package")
        .get();
    let output = vault
        .download_file(7, 11, package)
        .expect("authenticated download");
    assert_eq!(std::fs::read(output).expect("output"), bytes);
}

#[test]
fn another_device_reuses_authenticated_published_containers_and_rejects_wrong_source() {
    use std::io::Write;
    let dir = tempfile::tempdir().expect("first device");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    let bundle = vault.initialize(PASSWORD.into()).expect("initialize");
    let source = dir.path().join("multipart.bin");
    let length = 2 * 1024 * 1024;
    let chunk = vec![0x69; 1024 * 1024];
    let mut original = std::fs::File::create(&source).expect("source");
    let mut expected = blake3::Hasher::new();
    for _ in 0..length / chunk.len() as u64 {
        original.write_all(&chunk).expect("source chunk");
        expected.update(&chunk);
    }
    original.sync_all().expect("source synced");
    drop(original);
    let (recovery, wrap) = decode_recovery_bundle(&bundle).expect("recovery bundle");
    let master = teleark_crypto::unwrap_master_key_with_recovery(&wrap, &recovery)
        .expect("synthetic master");
    let mut fs = NativeFileSystem::new();
    fs.register_source(SourceId(99), &source)
        .expect("source identity");
    let package = package_bytes(123);
    let context = crate::VaultRecoveryContext {
        account_id: 7,
        task_id: 99,
        chat_id: 11,
        package_id: package,
        vault_id: wrap.vault_id,
        master_key_generation: 1,
        file_key_wrap: teleark_crypto::wrap_file_key(
            &master,
            &teleark_crypto::FileKey::from_bytes([7; 32]),
            &wrap.vault_id,
            &package,
            1,
            1,
            &mut AeadUsageRegistry::new(),
        )
        .expect("wrapped file key"),
        file_name: "multipart.bin".into(),
        created_at_unix_ms: now_unix_ms().expect("clock") as u64,
        size_bytes: length,
        container_plaintext_limit: 1024 * 1024,
        direction: crate::VaultRecoveryDirection::Upload {
            source: std::fs::canonicalize(&source).expect("path"),
            identity: fs.source_identity(SourceId(99)).expect("identity"),
            source_blake3: *expected.finalize().as_bytes(),
        },
    };
    Database::open(library.database_path.as_ref())
        .expect("database")
        .admit_vault_job(&context.admission_record().expect("context"))
        .expect("admit versioned test geometry");
    let (entered, release) = remote.gate(GateKind::UploadPart, 1);
    let work = vault.submit_resume_upload(7, 99).expect("start");
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("second container active");
    let pending = remote
        .summaries()
        .into_iter()
        .filter(|file| file.caption == remote_upload::CAPTION)
        .max_by_key(|file| file.message_id)
        .expect("remote recovery before completion");
    let first_publication = remote.uploads()[0];
    vault
        .submit_transfer_control(7, 99, control::VaultUploadControl::Pause)
        .expect("pause")
        .wait()
        .expect("durable intent");
    release.send(()).expect("finish network callback");
    assert_eq!(
        work.wait().expect_err("paused").kind(),
        ApplicationErrorKind::Cancelled
    );
    drop(vault);
    drop(library);

    let second = tempfile::tempdir().expect("independent device");
    let (vault, library) = open(second.path(), &remote);
    vault
        .restore_with_recovery(bundle, PASSWORD.into())
        .expect("same recovery key");
    let scan = vault
        .scan_managed_files(7, 11, crate::TelegramScanCancellation::new())
        .expect("discover incomplete upload");
    assert_eq!(scan.files.len(), 1);
    assert_eq!(scan.files[0].health, crate::VaultFileHealth::PendingUpload);
    assert_eq!(scan.files[0].part_message_ids.len(), 1);
    let wrong = second.path().join("wrong.bin");
    std::fs::write(&wrong, vec![0; length as usize]).expect("different source");
    assert_eq!(
        vault
            .submit_resume_remote_upload(7, 11, pending.message_id, wrong)
            .expect("admit verification")
            .wait()
            .expect_err("content mismatch")
            .kind(),
        ApplicationErrorKind::SourceChanged
    );
    assert_eq!(
        remote.objects(),
        1,
        "wrong source publishes no payload or completion"
    );
    let local = second.path().join("same-file.bin");
    std::fs::copy(&source, &local).expect("same source on another device");
    let resumed = vault
        .submit_resume_remote_upload(7, 11, pending.message_id, local)
        .expect("resume remote")
        .wait()
        .expect("complete on new device");
    assert_eq!(resumed.logical_name, "multipart.bin");
    assert_eq!(
        remote
            .uploads()
            .iter()
            .filter(|attempt| **attempt == first_publication)
            .count(),
        1,
        "published ciphertext was verified and reused"
    );
    assert_eq!(
        remote.objects(),
        3,
        "two containers and one completed manifest"
    );
    let output = vault
        .download_file(7, 11, 123)
        .expect("stream authenticated plaintext");
    assert_eq!(
        std::fs::read(output).expect("output"),
        std::fs::read(source).expect("source")
    );
    let scan = vault
        .scan_managed_files(7, 11, crate::TelegramScanCancellation::new())
        .expect("completed discovery");
    assert_eq!(scan.files.len(), 1);
    assert_ne!(scan.files[0].health, crate::VaultFileHealth::PendingUpload);
    let spool = library.database_path.with_extension("upload-spool");
    for account in std::fs::read_dir(spool).expect("tiny metadata") {
        for task in std::fs::read_dir(account.expect("account").path()).expect("tasks") {
            for entry in std::fs::read_dir(task.expect("task").path()).expect("metadata") {
                assert_ne!(
                    entry
                        .expect("record")
                        .path()
                        .extension()
                        .and_then(|value| value.to_str()),
                    Some("ciphertext")
                );
            }
        }
    }
}

#[test]
fn multipart_download_restart_reuses_verified_first_extent() {
    use std::io::{Read, Write};
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    let bundle = vault.initialize(PASSWORD.into()).expect("initialize");
    let source = dir.path().join("multipart.bin");
    let length = 2 * 1024 * 1024;
    let chunk = vec![0x69; 1024 * 1024];
    let mut original = std::fs::File::create(&source).expect("source");
    let mut expected = blake3::Hasher::new();
    for _ in 0..length / chunk.len() as u64 {
        original.write_all(&chunk).expect("source chunk");
        expected.update(&chunk);
    }
    original.sync_all().expect("source synced");
    drop(original);
    let (recovery, wrap) = decode_recovery_bundle(&bundle).expect("recovery bundle");
    let master = teleark_crypto::unwrap_master_key_with_recovery(&wrap, &recovery)
        .expect("synthetic master");
    let mut fs = NativeFileSystem::new();
    fs.register_source(SourceId(99), &source)
        .expect("source identity");
    let package = package_bytes(123);
    let context = crate::VaultRecoveryContext {
        account_id: 7,
        task_id: 99,
        chat_id: 11,
        package_id: package,
        vault_id: wrap.vault_id,
        master_key_generation: 1,
        file_key_wrap: teleark_crypto::wrap_file_key(
            &master,
            &teleark_crypto::FileKey::from_bytes([7; 32]),
            &wrap.vault_id,
            &package,
            1,
            1,
            &mut AeadUsageRegistry::new(),
        )
        .expect("wrapped file key"),
        file_name: "multipart.bin".into(),
        created_at_unix_ms: now_unix_ms().expect("clock") as u64,
        size_bytes: length,
        container_plaintext_limit: 1024 * 1024,
        direction: crate::VaultRecoveryDirection::Upload {
            source: std::fs::canonicalize(&source).expect("path"),
            identity: fs.source_identity(SourceId(99)).expect("identity"),
            source_blake3: *expected.finalize().as_bytes(),
        },
    };
    Database::open(library.database_path.as_ref())
        .expect("database")
        .admit_vault_job(&context.admission_record().expect("context"))
        .expect("admit versioned test geometry");
    vault
        .submit_resume_upload(7, 99)
        .expect("upload")
        .wait()
        .expect("multipart upload");
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let upload = vault
        .transfers()
        .into_iter()
        .find(|row| row.direction == VaultTransferDirection::Upload)
        .expect("upload");
    let job = db.vault_job(7, upload.id).expect("job").expect("ledger");
    let package = crate::transfer::package_id_from_bytes(job.package_id)
        .expect("package")
        .get();
    assert_eq!(remote.objects(), 3, "two real-size parts and manifest");
    let before = remote.downloads().len();
    let (entered, release) = remote.gate(GateKind::DownloadPart, 1);
    let work = vault
        .submit_download_file(7, 11, package)
        .expect("download");
    entered
        .recv_timeout(Duration::from_secs(60))
        .expect("second part entered");
    let id = vault
        .transfers()
        .into_iter()
        .find(|row| row.direction == VaultTransferDirection::Download)
        .expect("download")
        .id;
    let parts = db.vault_parts(7, id, None, 256).expect("parts");
    assert_eq!(
        parts.iter().filter(|part| part.receipt.is_some()).count(),
        1,
        "first extent durable before second download"
    );
    let requests = remote.downloads();
    let first_message = requests[requests.len() - 2];
    let second_message = requests[requests.len() - 1];
    assert!(requests.len() >= before + 2);
    assert_ne!(first_message, second_message);
    assert_eq!(
        requests[before..]
            .iter()
            .filter(|id| **id == first_message)
            .count(),
        1,
        "first part is downloaded once, not discovered and downloaded again"
    );

    vault
        .submit_transfer_control(7, id, control::VaultUploadControl::Pause)
        .expect("pause")
        .wait()
        .expect("saved pause");
    release.send(()).expect("release");
    assert!(work.wait().is_err());
    assert_eq!(
        db.vault_job(7, id).expect("job").expect("ledger").state,
        VaultJobState::Paused
    );
    let resumed_from = remote.downloads().len();
    drop(vault);
    drop(library);
    drop(db);
    let (vault, library) = open(dir.path(), &remote);
    vault.restore_upload_history(7).expect("history");
    vault.unlock_with_password(PASSWORD.into()).expect("unlock");
    let output = vault
        .submit_resume_download(7, id)
        .expect("resume")
        .wait()
        .expect("completion");
    let requests = remote.downloads();
    assert!(
        !requests[resumed_from..].contains(&first_message),
        "verified first extent reused locally"
    );
    assert!(
        requests[resumed_from..].contains(&second_message),
        "interrupted second extent fetched"
    );
    assert_eq!(
        requests[resumed_from..]
            .iter()
            .filter(|id| **id == second_message)
            .count(),
        1,
        "resumed part is downloaded once"
    );
    let mut output = std::fs::File::open(output).expect("output");
    assert_eq!(output.metadata().expect("metadata").len(), length);
    let mut actual = blake3::Hasher::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let count = output.read(&mut buffer).expect("read");
        if count == 0 {
            break;
        }
        actual.update(&buffer[..count]);
    }
    assert_eq!(actual.finalize(), expected.finalize());
    let db = Database::open(library.database_path.as_ref()).expect("database");
    assert_eq!(
        db.vault_job(7, id).expect("job").expect("ledger").state,
        VaultJobState::Completed
    );
    assert_eq!(
        db.vault_parts(7, id, None, 256)
            .expect("parts")
            .iter()
            .filter(|part| part.receipt.is_some())
            .count(),
        2
    );
}

#[test]
fn network_failed_upload_retries_same_task_after_restart() {
    network_failure_restart(GateKind::UploadPart);
}

#[test]
fn resumed_upload_rejects_changed_bytes_even_when_size_and_mtime_are_preserved() {
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    vault.initialize(PASSWORD.into()).expect("keys");
    let source = dir.path().join("changed.bin");
    std::fs::write(&source, b"original").expect("source");
    remote.fail_once(GateKind::UploadPart);
    assert!(vault.upload_file(7, 11, source.clone()).is_err());
    let id = vault
        .transfers()
        .iter()
        .find(|row| row.direction == VaultTransferDirection::Upload)
        .expect("failed row")
        .id;
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let saved = db.vault_job(7, id).expect("job").expect("saved");
    let parts = db.vault_parts(7, id, None, 256).expect("reservations");
    assert_eq!(parts.len(), 1);
    let modified = std::fs::metadata(&source)
        .expect("metadata")
        .modified()
        .expect("mtime");
    std::fs::write(&source, b"replaced").expect("same-length change");
    std::fs::File::options()
        .write(true)
        .open(&source)
        .expect("source handle")
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .expect("restore mtime");
    let result = vault
        .submit_resume_upload(7, id)
        .expect("resume request")
        .wait();
    assert_eq!(
        result.expect_err("changed bytes rejected").kind(),
        ApplicationErrorKind::SourceChanged
    );
    assert_eq!(remote.objects(), 0, "changed bytes must not reach Telegram");
    let failed = db.vault_job(7, id).expect("job").expect("retained");
    assert_eq!(failed.state, VaultJobState::Blocked);
    assert_eq!(
        failed.context, saved.context,
        "retain original source/key context"
    );
    assert_eq!(
        db.vault_parts(7, id, None, 256)
            .expect("original reservations"),
        parts
    );
}

#[test]
fn network_failed_download_retries_same_task_after_restart() {
    network_failure_restart(GateKind::DownloadPart);
}

#[test]
fn lost_part_reply_retries_without_duplicate_publication() {
    network_failure_restart(GateKind::PartAcknowledgment);
}

#[test]
fn lost_manifest_reply_retries_without_source_or_duplicate_publication() {
    network_failure_restart(GateKind::ManifestAcknowledgment);
}

fn network_failure_restart(failure: GateKind) {
    let upload_failure = failure != GateKind::DownloadPart;
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    vault.initialize(PASSWORD.into()).expect("initialize");
    let bytes = vec![0x27; 2 * 1024 * 1024];
    let source = dir.path().join("retry.bin");
    std::fs::write(&source, &bytes).expect("source");
    let direction = if upload_failure {
        remote.fail_once(failure);
        assert!(vault.upload_file(7, 11, source.clone()).is_err());
        VaultTransferDirection::Upload
    } else {
        vault.upload_file(7, 11, source.clone()).expect("upload");
        let db = Database::open(library.database_path.as_ref()).expect("database");
        let id = vault
            .transfers()
            .into_iter()
            .find(|row| row.direction == VaultTransferDirection::Upload)
            .expect("upload")
            .id;
        let job = db.vault_job(7, id).expect("job").expect("ledger");
        let package = crate::transfer::package_id_from_bytes(job.package_id)
            .expect("package")
            .get();
        remote.fail_once(GateKind::DownloadPart);
        assert!(vault.download_file(7, 11, package).is_err());
        VaultTransferDirection::Download
    };
    let id = vault
        .transfers()
        .into_iter()
        .find(|row| row.direction == direction)
        .expect("failed task")
        .id;
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let failed = db.vault_job(7, id).expect("job").expect("ledger");
    assert_eq!(failed.state, VaultJobState::Retryable);
    if matches!(
        failure,
        GateKind::PartAcknowledgment | GateKind::ManifestAcknowledgment
    ) {
        assert_eq!(
            remote.objects(),
            if failure == GateKind::ManifestAcknowledgment {
                2
            } else {
                1
            }
        );
    }
    if failure == GateKind::ManifestAcknowledgment {
        std::fs::remove_file(&source).expect("remove synthetic source after saved manifest");
    }
    #[cfg(unix)]
    if matches!(failure, GateKind::UploadPart | GateKind::PartAcknowledgment) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600))
            .expect("metadata-only change before restart with persisted reservations");
    }

    drop(vault);
    drop(library);
    drop(db);
    let (vault, library) = open(dir.path(), &remote);
    vault.restore_upload_history(7).expect("history");
    assert_eq!(
        vault
            .transfers()
            .into_iter()
            .find(|row| row.id == id)
            .expect("restored task")
            .recovery_state,
        Some(VaultJobState::Retryable)
    );
    vault.unlock_with_password(PASSWORD.into()).expect("unlock");
    let output = if upload_failure {
        vault
            .submit_resume_upload(7, id)
            .expect("retry")
            .wait()
            .expect("uploaded");
        let package = crate::transfer::package_id_from_bytes(failed.package_id)
            .expect("package")
            .get();
        vault.download_file(7, 11, package).expect("download")
    } else {
        vault
            .submit_resume_download(7, id)
            .expect("retry")
            .wait()
            .expect("downloaded")
    };
    assert_eq!(std::fs::read(output).expect("output"), bytes);
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let completed = db.vault_job(7, id).expect("job").expect("same ledger");
    assert_eq!(completed.state, VaultJobState::Completed);
    assert!(completed.generation > failed.generation);
    assert_eq!(completed.context, failed.context);
    assert_eq!(remote.objects(), 2);
    if matches!(
        failure,
        GateKind::PartAcknowledgment | GateKind::ManifestAcknowledgment
    ) {
        assert_eq!(
            remote.uploads().len(),
            2,
            "lost reply causes reconciliation, not another send"
        );
    }
}

#[test]
fn cancelled_upload_and_download_remain_terminal_after_restart() {
    for upload in [true, false] {
        let dir = tempfile::tempdir().expect("directory");
        let remote = TestVaultRemote::new();
        let (vault, library) = open(dir.path(), &remote);
        vault.initialize(PASSWORD.into()).expect("initialize");
        let source = dir.path().join("cancel.bin");
        let bytes = vec![0x42; 2 * 1024 * 1024];
        std::fs::write(&source, &bytes).expect("source");
        let mut package = None;
        if !upload {
            vault.upload_file(7, 11, source.clone()).expect("upload");
            let db = Database::open(library.database_path.as_ref()).expect("database");
            let id = vault
                .transfers()
                .into_iter()
                .find(|row| row.direction == VaultTransferDirection::Upload)
                .expect("upload")
                .id;
            package = Some(
                crate::transfer::package_id_from_bytes(
                    db.vault_job(7, id)
                        .expect("job")
                        .expect("ledger")
                        .package_id,
                )
                .expect("package")
                .get(),
            );
        }
        let (entered, release) = remote.gate(
            if upload {
                GateKind::UploadPart
            } else {
                GateKind::DownloadPart
            },
            0,
        );
        let upload_work = if upload {
            Some(
                vault
                    .submit_upload_files(7, 11, vec![source.clone()])
                    .expect("upload"),
            )
        } else {
            None
        };
        let download_work = package.map(|package| {
            vault
                .submit_download_file(7, 11, package)
                .expect("download")
        });
        entered
            .recv_timeout(Duration::from_secs(30))
            .expect("transport entered");
        let direction = if upload {
            VaultTransferDirection::Upload
        } else {
            VaultTransferDirection::Download
        };
        let id = vault
            .transfers()
            .into_iter()
            .find(|row| row.direction == direction)
            .expect("task")
            .id;
        vault
            .submit_transfer_control(7, id, control::VaultUploadControl::Cancel)
            .expect("cancel")
            .wait()
            .expect("durable intent");
        release.send(()).expect("release");
        if let Some(work) = upload_work {
            assert_eq!(work.wait().expect("stopped batch").cancelled.len(), 1);
        }
        if let Some(work) = download_work {
            assert!(work.wait().is_err());
        }
        let db = Database::open(library.database_path.as_ref()).expect("database");
        assert_eq!(
            db.vault_job(7, id).expect("job").expect("ledger").state,
            VaultJobState::Cancelled
        );
        let requests = (remote.uploads().len(), remote.downloads().len());
        drop(vault);
        drop(library);
        drop(db);
        let (vault, library) = open(dir.path(), &remote);
        vault.restore_upload_history(7).expect("history");
        assert_eq!(
            vault
                .transfers()
                .into_iter()
                .find(|row| row.id == id)
                .expect("restored")
                .state,
            VaultTransferState::Cancelled
        );
        vault.unlock_with_password(PASSWORD.into()).expect("unlock");
        let error = if upload {
            vault
                .submit_resume_upload(7, id)
                .expect("request")
                .wait()
                .expect_err("cancel is terminal")
        } else {
            vault
                .submit_resume_download(7, id)
                .expect("request")
                .wait()
                .expect_err("cancel is terminal")
        };
        assert_eq!(error.kind(), ApplicationErrorKind::Conflict);
        assert_eq!(
            (remote.uploads().len(), remote.downloads().len()),
            requests,
            "cancelled task performs no remote work on restart or resume"
        );
        assert_eq!(std::fs::read(source).expect("preserved source"), bytes);
        let db = Database::open(library.database_path.as_ref()).expect("database");
        assert_eq!(
            db.vault_job(7, id).expect("job").expect("ledger").state,
            VaultJobState::Cancelled
        );
    }
}

#[test]
fn default_three_files_are_work_conserving_and_fresh_parts_have_no_readback() {
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, _library) = open(dir.path(), &remote);
    vault.initialize(PASSWORD.into()).expect("keys");
    let sources = (0..4)
        .map(|index| {
            let path = dir.path().join(format!("parallel-{index}.bin"));
            std::fs::write(&path, vec![index as u8; 128 * 1024]).expect("source");
            path
        })
        .collect();
    let (entered, release) = remote.concurrent_upload_gate(4);
    let work = vault.submit_upload_files(7, 11, sources).expect("batch");
    for _ in 0..3 {
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("three files enter transport before a completion");
    }
    assert!(entered.try_recv().is_err(), "fourth file stays queued");
    release.send(()).expect("one finishes");
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("fourth fills the free slot while two remain blocked");
    for _ in 0..3 {
        release.send(()).expect("finish");
    }
    assert_eq!(work.wait().expect("batch").completed_count, 4);
    let summaries = remote.summaries();
    assert!(
        remote.downloads().iter().all(|id| summaries
            .iter()
            .find(|row| row.message_id == *id)
            .is_some_and(|row| row.caption == crate::transfer::MANIFEST_CAPTION)),
        "fresh content parts must never be read back"
    );
}

#[test]
fn lost_ciphertext_burns_identity_before_new_encryption_after_restart() {
    upload_transport_restart_with_spool_loss(false, false);
}

#[test]
fn unfinished_v1_upload_automatically_restarts_as_aligned_v2() {
    upload_transport_restart_with_spool_loss(true, false);
}

#[test]
fn unsealed_manifest_reservation_retires_file_key_before_any_new_encryption() {
    upload_transport_restart_with_spool_loss(false, true);
}

fn upload_transport_restart_with_spool_loss(legacy: bool, unsealed: bool) {
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    vault.initialize(PASSWORD.into()).expect("keys");
    let source = dir.path().join("lost-spool.bin");
    let bytes = vec![33u8; 1024 * 1024];
    std::fs::write(&source, &bytes).expect("source");
    let (entered, release) = remote.gate(GateKind::UploadPart, 0);
    let work = vault
        .submit_upload_files(7, 11, vec![source])
        .expect("batch");
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("transport");
    let id = vault
        .transfers()
        .iter()
        .find(|row| row.direction == VaultTransferDirection::Upload)
        .expect("row")
        .id;
    vault
        .submit_transfer_control(7, id, control::VaultUploadControl::Pause)
        .expect("pause")
        .wait()
        .expect("saved pause");
    release.send(()).expect("release");
    assert_eq!(work.wait().expect("batch").paused_count, 1);
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let before = db.vault_parts(7, id, None, 1).expect("parts").remove(0);
    let original = crate::VaultPartRecovery::decode(&before.identity).expect("identity");
    let root = library
        .database_path
        .with_extension("upload-spool")
        .join("7")
        .join(id.to_string());
    let prefix = crate::transfer::streaming::spool_prefix(&root, &before.identity);
    std::fs::write(
        prefix.with_extension("ciphertext"),
        b"interrupted or corrupted ciphertext",
    )
    .expect("simulate loss");
    drop(vault);
    drop(library);
    drop(db);
    if legacy || unsealed {
        // Persist the genuine historical v1 header geometry, without creating
        // ciphertext. The new process must detect and upgrade this reservation.
        let mut db = Database::open(dir.path().join("catalog.sqlite")).expect("database");
        let record = db.vault_job(7, id).expect("job").expect("saved");
        let mut lease = teleark_storage::VaultJobLease {
            account_id: 7,
            id,
            generation: record.generation,
        };
        for (state, transition) in [
            (
                VaultJobState::Paused,
                teleark_storage::VaultJobTransition::Resume,
            ),
            (
                VaultJobState::Queued,
                teleark_storage::VaultJobTransition::Start,
            ),
        ] {
            assert!(
                db.transition_vault_job(lease, state, transition, 1, None)
                    .expect("transition")
            );
        }
        lease.generation += 1;
        if unsealed {
            assert!(
                db.reserve_vault_manifest(
                    lease,
                    &teleark_storage::VaultManifestOutbox {
                        codec_version: 1,
                        commitment: [77; 32],
                        random_id: 817,
                        envelope: None,
                        message_id: None,
                    }
                )
                .expect("unsealed commitment")
            );
        }
        let mut historical = original.clone();
        let header = &original.header;
        historical.header = teleark_crypto::PartHeader::new(
            header.package_id,
            header.part_instance_id,
            header.part_index,
            header.part_count,
            header.logical_file_size,
            header.plaintext_offset,
            header.plaintext_length,
            8 * 1024 * 1024,
            teleark_crypto::PartLimits::default(),
        )
        .expect("v1 header");
        if legacy {
            assert!(
                db.replace_unpublished_vault_part(
                    lease,
                    before.part_index,
                    &before.identity,
                    &historical.encode().expect("codec"),
                )
                .expect("old reservation")
            );
        }
        for (state, transition) in [
            (
                VaultJobState::Running,
                teleark_storage::VaultJobTransition::RequestPause,
            ),
            (
                VaultJobState::Pausing,
                teleark_storage::VaultJobTransition::AcknowledgePause,
            ),
        ] {
            assert!(
                db.transition_vault_job(lease, state, transition, 1, None)
                    .expect("transition")
            );
        }
    }
    let (vault, library) = open(dir.path(), &remote);
    vault.unlock_with_password(PASSWORD.into()).expect("unlock");
    let file = vault
        .submit_resume_upload(7, id)
        .expect("resume")
        .wait()
        .expect("completed");
    let db = Database::open(library.database_path.as_ref()).expect("database");
    let after = db.vault_parts(7, id, None, 1).expect("parts").remove(0);
    let replaced = crate::VaultPartRecovery::decode(&after.identity).expect("new identity");
    assert_eq!(replaced.header.format_major, 2);
    if legacy || unsealed {
        assert_ne!(replaced.header.package_id, original.header.package_id);
        let row = vault
            .transfers()
            .into_iter()
            .find(|row| row.id == id)
            .expect("row");
        assert!(
            row.upload_activity
                .as_ref()
                .expect("activity")
                .events
                .iter()
                .any(|event| event.phase
                    == if unsealed {
                        VaultUploadPhase::RestartingUnsealed
                    } else {
                        VaultUploadPhase::UpgradingUpload
                    })
        );
    } else {
        assert_eq!(replaced.header.package_id, original.header.package_id);
        assert_eq!(
            u128::from_be_bytes(original.header.part_instance_id.0).checked_add(1),
            Some(u128::from_be_bytes(replaced.header.part_instance_id.0))
        );
    }
    assert_ne!(
        original.publication_random_id,
        replaced.publication_random_id
    );
    assert!(after.receipt.is_some());
    assert!(!prefix.with_extension("ciphertext").exists());
    assert_eq!(remote.objects(), 2);
    assert_eq!(file.logical_name, "lost-spool.bin");
    let package = crate::transfer::package_id_from_bytes(
        db.vault_job(7, id).expect("job").expect("saved").package_id,
    )
    .expect("package")
    .get();
    let output = vault.download_file(7, 11, package).expect("download");
    assert_eq!(std::fs::read(output).expect("plaintext"), bytes);
}

#[test]
fn restart_recovers_three_upload_files_concurrently() {
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    vault.initialize(PASSWORD.into()).expect("keys");
    let sources = (0..3)
        .map(|index| {
            let path = dir.path().join(format!("resume-{index}.bin"));
            std::fs::write(&path, vec![index as u8; 1024]).expect("source");
            path
        })
        .collect();
    let (entered, release) = remote.concurrent_upload_gate(3);
    let work = vault.submit_upload_files(7, 11, sources).expect("batch");
    for _ in 0..3 {
        entered
            .recv_timeout(Duration::from_secs(20))
            .expect("parallel upload");
    }
    let ids = vault
        .transfers()
        .iter()
        .filter(|row| row.direction == VaultTransferDirection::Upload)
        .map(|row| row.id)
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 3);
    for &id in &ids {
        vault
            .submit_transfer_control(7, id, control::VaultUploadControl::Pause)
            .expect("pause")
            .wait()
            .expect("saved");
    }
    for _ in 0..3 {
        release.send(()).expect("release");
    }
    assert_eq!(work.wait().expect("batch").paused_count, 3);
    drop(vault);
    drop(library);
    let mut db = Database::open(dir.path().join("catalog.sqlite")).expect("database");
    for &id in &ids {
        let record = db.vault_job(7, id).expect("job").expect("context");
        assert!(
            db.transition_vault_job(
                teleark_storage::VaultJobLease {
                    account_id: 7,
                    id,
                    generation: record.generation
                },
                VaultJobState::Paused,
                teleark_storage::VaultJobTransition::Resume,
                1,
                None
            )
            .expect("queued at interruption")
        );
    }
    drop(db);
    let (vault, _) = open(dir.path(), &remote);
    vault.unlock_with_password(PASSWORD.into()).expect("unlock");
    let (entered, release) = remote.concurrent_upload_gate(3);
    let work = vault
        .submit_resume_queued_transfers(7)
        .expect("resume queue");
    for _ in 0..3 {
        entered
            .recv_timeout(Duration::from_secs(20))
            .expect("three restored files enter before any completion");
    }
    for _ in 0..3 {
        release.send(()).expect("finish");
    }
    let report = work.wait().expect("recovered");
    assert_eq!(report.resumed, 3);
    assert_eq!(report.failed, 0);
}
