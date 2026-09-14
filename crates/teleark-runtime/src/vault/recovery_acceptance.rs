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
fn upload_paused_during_transport_reuses_publication_and_ciphertext_after_restart() {
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
        if acknowledgment_lost {
            vec![interrupted]
        } else {
            vec![interrupted, interrupted]
        },
        "published bytes are reconciled without resending; unsent bytes retain identity"
    );
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
fn multipart_download_restart_reuses_verified_first_extent() {
    use std::io::{Read, Write};
    let dir = tempfile::tempdir().expect("directory");
    let remote = TestVaultRemote::new();
    let (vault, library) = open(dir.path(), &remote);
    vault.initialize(PASSWORD.into()).expect("initialize");
    let source = dir.path().join("multipart.bin");
    let length = crate::transfer::encrypted_part_plaintext_limit() + 1024 * 1024;
    let chunk = vec![0x69; 1024 * 1024];
    let mut original = std::fs::File::create(&source).expect("source");
    let mut expected = blake3::Hasher::new();
    for _ in 0..length / chunk.len() as u64 {
        original.write_all(&chunk).expect("source chunk");
        expected.update(&chunk);
    }
    original.sync_all().expect("source synced");
    drop(original);
    vault.upload_file(7, 11, source).expect("multipart upload");
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
