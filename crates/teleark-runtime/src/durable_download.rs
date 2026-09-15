//! Verified, fsynced download extents. Run only on a retained background owner.
use crate::{VaultRecoveryContext, VaultRecoveryDirection};
use teleark_core::TransferError;
use teleark_crypto::ManifestPart;
use teleark_storage::{Database, VaultJobLease, VaultJobState, VaultPartRecord};
use teleark_transfer::{ContentDigest, DestinationId, FileSystemPort, NativeFileSystem};

mod published;
use published::PublishedOutput;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExtentSource {
    Local,
    Remote,
}

pub(crate) struct DurableDownload {
    lease: VaultJobLease,
    context: VaultRecoveryContext,
    context_digest: [u8; 32],
    files: NativeFileSystem,
    published: Option<PublishedOutput>,
    cancellation: crate::TelegramScanCancellation,
}

impl DurableDownload {
    #[cfg(test)]
    pub fn open(database: &Database, lease: VaultJobLease) -> Result<Self, TransferError> {
        Self::open_cancellable(database, lease, crate::TelegramScanCancellation::default())
    }

    pub fn open_cancellable(
        database: &Database,
        lease: VaultJobLease,
        cancellation: crate::TelegramScanCancellation,
    ) -> Result<Self, TransferError> {
        let check = || {
            if cancellation.is_cancelled() {
                Err(TransferError::Cancelled)
            } else {
                Ok(())
            }
        };
        check()?;
        let record = database
            .vault_job(lease.account_id, lease.id)
            .map_err(|_| TransferError::Database)?
            .ok_or(TransferError::ManifestCorrupted)?;
        if record.generation != lease.generation || record.state != VaultJobState::Running {
            return Err(TransferError::Cancelled);
        }
        let context = VaultRecoveryContext::from_record(&record)
            .map_err(|_| TransferError::ManifestCorrupted)?;
        let VaultRecoveryDirection::Download {
            destination,
            whole_plaintext_blake3,
            ..
        } = &context.direction
        else {
            return Err(TransferError::ManifestCorrupted);
        };
        let mut files = NativeFileSystem::new();
        files.register_destination(DestinationId(lease.id), destination)?;
        let mut published = PublishedOutput::open(destination)?;
        if let Some(output) = published.as_mut() {
            output.verify(context.size_bytes, *whole_plaintext_blake3, check)?;
        } else {
            files.prepare_partial(DestinationId(lease.id), context.size_bytes)?;
        }
        Ok(Self {
            lease,
            context,
            context_digest: *blake3::hash(&record.context).as_bytes(),
            files,
            published,
            cancellation,
        })
    }

    /// A receipt authorizes a bounded local recheck, never blind skipping.
    /// Corrupt or lost local bytes are fetched again from the authenticated part.
    #[cfg(test)]
    pub fn restore_part(
        &mut self,
        database: &mut Database,
        part: &ManifestPart,
        fetch: impl FnOnce() -> Result<Vec<u8>, TransferError>,
    ) -> Result<ExtentSource, TransferError> {
        self.restore_part_to(database, part, |out| {
            let bytes = fetch()?;
            out.write_all(&bytes).map_err(|_| TransferError::Database)
        })
    }

    pub fn restore_part_to(
        &mut self,
        database: &mut Database,
        part: &ManifestPart,
        fetch: impl FnOnce(&mut dyn std::io::Write) -> Result<(), TransferError>,
    ) -> Result<ExtentSource, TransferError> {
        let identity = self.identity(part)?;
        let reservation = VaultPartRecord {
            part_index: part.part_index,
            identity,
            receipt: None,
        };
        if !database
            .reserve_vault_part(self.lease, &reservation)
            .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        if let Some(output) = self.published.as_mut() {
            // The complete final file was verified when reopening. Verify this
            // range again before acknowledging progress without remote work.
            let digest = range_digest(
                part,
                |offset, length| output.read_range(offset, length),
                &self.cancellation,
            )?;
            return if digest == part.plaintext_blake3 {
                Ok(ExtentSource::Local)
            } else {
                Err(TransferError::HashMismatch)
            };
        }
        let receipt = receipt(&reservation.identity);
        let saved = database
            .vault_parts(
                self.lease.account_id,
                self.lease.id,
                part.part_index.checked_sub(1),
                1,
            )
            .map_err(|_| TransferError::Database)?
            .into_iter()
            .find(|saved| saved.part_index == part.part_index)
            .ok_or(TransferError::Database)?;
        if let Some(saved_receipt) = saved.receipt {
            if saved_receipt != receipt {
                return Err(TransferError::ManifestCorrupted);
            }
            let digest = range_digest(
                part,
                |offset, length| {
                    self.files
                        .read_partial(DestinationId(self.lease.id), offset, length)
                },
                &self.cancellation,
            )?;
            if digest == part.plaintext_blake3 {
                return Ok(ExtentSource::Local);
            }
        }
        let mut writer = ExtentWriter {
            files: &mut self.files,
            id: DestinationId(self.lease.id),
            offset: part.plaintext_offset,
            remaining: part.plaintext_length,
            hash: blake3::Hasher::new(),
            error: None,
            cancellation: &self.cancellation,
        };
        let fetched = fetch(&mut writer);
        if let Some(error) = writer.error {
            return Err(error);
        }
        fetched?;
        if writer.remaining != 0 || writer.hash.finalize().as_bytes() != &part.plaintext_blake3 {
            return Err(TransferError::HashMismatch);
        }
        if self.cancellation.is_cancelled()
            || !database
                .reserve_vault_part(self.lease, &reservation)
                .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        self.files.flush_partial(DestinationId(self.lease.id))?;
        if !database
            .confirm_vault_part(self.lease, part.part_index, &receipt)
            .map_err(|_| TransferError::Database)?
        {
            return Err(TransferError::Cancelled);
        }
        Ok(ExtentSource::Remote)
    }

    pub fn finalize(&mut self, database: &Database) -> Result<(), TransferError> {
        let record = database
            .vault_job(self.lease.account_id, self.lease.id)
            .map_err(|_| TransferError::Database)?
            .ok_or(TransferError::ManifestCorrupted)?;
        if record.generation != self.lease.generation || record.state != VaultJobState::Running {
            return Err(TransferError::Cancelled);
        }
        let VaultRecoveryDirection::Download {
            whole_plaintext_blake3,
            ..
        } = self.context.direction
        else {
            return Err(TransferError::ManifestCorrupted);
        };
        let check = || {
            if self.cancellation.is_cancelled() {
                Err(TransferError::Cancelled)
            } else {
                Ok(())
            }
        };
        check()?;
        if let Some(output) = self.published.as_mut() {
            return output.verify(self.context.size_bytes, whole_plaintext_blake3, check);
        }
        if self
            .files
            .digest_partial_cancellable(DestinationId(self.lease.id), check)?
            != ContentDigest(whole_plaintext_blake3)
        {
            return Err(TransferError::HashMismatch);
        }
        self.files.flush_partial(DestinationId(self.lease.id))?;
        check()?;
        self.files.atomic_finalize(DestinationId(self.lease.id))
    }

    fn identity(&self, part: &ManifestPart) -> Result<Vec<u8>, TransferError> {
        if part.remote_locator.account_id != self.context.account_id
            || part.remote_locator.chat_id != self.context.chat_id
            || part
                .plaintext_offset
                .checked_add(part.plaintext_length)
                .is_none_or(|end| end > self.context.size_bytes)
        {
            return Err(TransferError::ManifestCorrupted);
        }
        // TARKDE01: codec u32, context digest, index u32, offset/length u64,
        // plaintext hash, encoded hash, remote message i64, instance id.
        let mut identity = Vec::with_capacity(152);
        identity.extend_from_slice(b"TARKDE01");
        identity.extend_from_slice(&1u32.to_le_bytes());
        identity.extend_from_slice(&self.context_digest);
        identity.extend_from_slice(&part.part_index.to_le_bytes());
        identity.extend_from_slice(&part.plaintext_offset.to_le_bytes());
        identity.extend_from_slice(&part.plaintext_length.to_le_bytes());
        identity.extend_from_slice(&part.plaintext_blake3);
        identity.extend_from_slice(&part.encoded_ciphertext_blake3);
        identity.extend_from_slice(&part.remote_locator.message_id.to_le_bytes());
        identity.extend_from_slice(&part.part_instance_id);
        Ok(identity)
    }
}

fn range_digest(
    part: &ManifestPart,
    mut read: impl FnMut(u64, u64) -> Result<Vec<u8>, TransferError>,
    cancel: &crate::TelegramScanCancellation,
) -> Result<[u8; 32], TransferError> {
    let mut hash = blake3::Hasher::new();
    let mut offset = part.plaintext_offset;
    let mut remaining = part.plaintext_length;
    while remaining > 0 {
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        let length = remaining.min(1024 * 1024);
        let bytes = read(offset, length)?;
        if bytes.len() as u64 != length {
            return Err(TransferError::HashMismatch);
        }
        hash.update(&bytes);
        offset += length;
        remaining -= length;
    }
    Ok(*hash.finalize().as_bytes())
}
struct ExtentWriter<'a> {
    files: &'a mut NativeFileSystem,
    id: DestinationId,
    offset: u64,
    remaining: u64,
    hash: blake3::Hasher,
    error: Option<TransferError>,
    cancellation: &'a crate::TelegramScanCancellation,
}
impl std::io::Write for ExtentWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let result = if self.cancellation.is_cancelled() {
            Err(TransferError::Cancelled)
        } else if bytes.len() as u64 > self.remaining {
            Err(TransferError::HashMismatch)
        } else {
            self.files.write_partial(self.id, self.offset, bytes)
        };
        if let Err(error) = result {
            self.error = Some(error);
            return Err(std::io::Error::other("container destination unavailable"));
        }
        self.hash.update(bytes);
        self.offset += bytes.len() as u64;
        self.remaining -= bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn receipt(identity: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(40);
    bytes.extend_from_slice(b"TARKDR01");
    bytes.extend_from_slice(blake3::hash(identity).as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use teleark_crypto::{
        AeadUsageRegistry, FileKey, RemoteLocator, VaultMasterKey, wrap_file_key,
    };
    use teleark_storage::VaultJobTransition;

    fn fixture(path: &std::path::Path) -> (Database, VaultJobLease, Vec<ManifestPart>) {
        let mut db = Database::open(path).expect("database");
        let context = VaultRecoveryContext {
            container_plaintext_limit: crate::encrypted_part_plaintext_limit(),
            account_id: 7,
            task_id: 9,
            chat_id: 11,
            package_id: [1; 16],
            vault_id: [2; 16],
            master_key_generation: 1,
            file_key_wrap: wrap_file_key(
                &VaultMasterKey::from_bytes([3; 32]),
                &FileKey::from_bytes([4; 32]),
                &[2; 16],
                &[1; 16],
                1,
                1,
                &mut AeadUsageRegistry::new(),
            )
            .expect("wrapped key"),
            file_name: "restored.bin".into(),
            created_at_unix_ms: 100,
            size_bytes: 8,
            direction: VaultRecoveryDirection::Download {
                destination: path.with_file_name("restored.bin"),
                manifest_message_id: 10,
                manifest_blake3: [5; 32],
                whole_plaintext_blake3: *blake3::hash(b"abcdefgh").as_bytes(),
            },
        };
        db.admit_vault_job(&context.admission_record().expect("context"))
            .expect("admit");
        let mut lease = VaultJobLease {
            account_id: 7,
            id: 9,
            generation: 0,
        };
        assert!(
            db.transition_vault_job(
                lease,
                VaultJobState::Queued,
                VaultJobTransition::Start,
                101,
                None
            )
            .expect("start")
        );
        lease.generation = 1;
        (db, lease, fixture_parts())
    }

    fn fixture_parts() -> Vec<ManifestPart> {
        [b"abcd", b"efgh"]
            .into_iter()
            .enumerate()
            .map(|(index, bytes)| ManifestPart {
                part_index: index as u32,
                part_instance_id: [index as u8 + 1; 16],
                plaintext_offset: index as u64 * 4,
                plaintext_length: 4,
                encoded_length: 132,
                frame_count: 1,
                plaintext_blake3: *blake3::hash(bytes).as_bytes(),
                encoded_ciphertext_blake3: [index as u8; 32],
                remote_locator: RemoteLocator {
                    account_id: 7,
                    chat_id: 11,
                    message_id: index as i64 + 20,
                    remote_name: format!("part-{index}"),
                    locator_version: 1,
                    locator_extension: None,
                },
            })
            .collect()
    }

    #[test]
    fn abrupt_exit_preserves_fsynced_extent_and_published_output() {
        const ENV: &str = "TELEARK_SYNTHETIC_DOWNLOAD_CRASH";
        if let Some(path) = std::env::var_os(ENV) {
            let path = std::path::PathBuf::from(path);
            let (mut db, lease, parts) = fixture(&path);
            let mut worker = DurableDownload::open(&db, lease).expect("child open");
            worker
                .restore_part(&mut db, &parts[0], || Ok(b"abcd".to_vec()))
                .expect("child first extent");
            if path.file_stem().expect("stem") == "published" {
                worker
                    .restore_part(&mut db, &parts[1], || Ok(b"efgh".to_vec()))
                    .expect("second");
                worker
                    .finalize(&db)
                    .expect("publish before terminal checkpoint");
            }
            // Bypass Rust destructors and graceful pause/owner shutdown.
            std::process::exit(73);
        }
        for published in [false, true] {
            let dir = tempfile::tempdir().expect("directory");
            let path = dir.path().join(if published {
                "published.sqlite"
            } else {
                "partial.sqlite"
            });
            let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", "durable_download::tests::abrupt_exit_preserves_fsynced_extent_and_published_output"])
                .env(ENV, &path).status().expect("child");
            assert_eq!(status.code(), Some(73));
            let mut db = Database::open(&path).expect("reopen abruptly closed database");
            db.recover_vault_jobs(7, 102).expect("recover");
            let mut lease = VaultJobLease {
                account_id: 7,
                id: 9,
                generation: 2,
            };
            assert!(
                db.transition_vault_job(
                    lease,
                    VaultJobState::Queued,
                    VaultJobTransition::Start,
                    103,
                    None
                )
                .expect("restart")
            );
            lease.generation = 3;
            let mut worker = DurableDownload::open(&db, lease).expect("reopen output");
            let parts = fixture_parts();
            assert_eq!(
                worker
                    .restore_part(&mut db, &parts[0], || panic!(
                        "durable extent must be reused"
                    ))
                    .expect("first"),
                ExtentSource::Local
            );
            let source = worker
                .restore_part(&mut db, &parts[1], || {
                    assert!(!published, "published output must need no transport");
                    Ok(b"efgh".to_vec())
                })
                .expect("second");
            assert_eq!(
                source,
                if published {
                    ExtentSource::Local
                } else {
                    ExtentSource::Remote
                }
            );
            worker.finalize(&db).expect("finalize");
            assert_eq!(
                std::fs::read(dir.path().join("restored.bin")).expect("output"),
                b"abcdefgh"
            );
        }
    }

    #[test]
    fn cancellation_before_final_hash_preserves_partial_for_restart() {
        let dir = tempfile::tempdir().expect("directory");
        let (mut db, lease, parts) = fixture(&dir.path().join("jobs.sqlite"));
        let cancellation = crate::TelegramScanCancellation::default();
        let mut worker =
            DurableDownload::open_cancellable(&db, lease, cancellation.clone()).expect("open");
        for (part, bytes) in parts.iter().zip([b"abcd", b"efgh"]) {
            worker
                .restore_part(&mut db, part, || Ok(bytes.to_vec()))
                .expect("part");
        }
        cancellation.cancel();
        assert_eq!(worker.finalize(&db), Err(TransferError::Cancelled));
        assert!(dir.path().join("restored.bin.partial").exists());
        assert!(!dir.path().join("restored.bin").exists());
    }

    #[test]
    fn finalization_fits_the_default_worker_stack() {
        let result = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let dir = tempfile::tempdir().expect("directory");
                let path = dir.path().join("jobs.sqlite");
                let (mut db, lease, parts) = fixture(&path);
                let mut worker = DurableDownload::open(&db, lease).expect("open");
                for (part, bytes) in parts.iter().zip([b"abcd", b"efgh"]) {
                    worker
                        .restore_part(&mut db, part, || Ok(bytes.to_vec()))
                        .expect("part");
                }
                worker.finalize(&db)
            })
            .expect("spawn")
            .join()
            .expect("worker thread");
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn reopen_rechecks_fsynced_extents_and_fetches_only_missing_or_corrupt_data() {
        for corrupt in [false, true] {
            let dir = tempfile::tempdir().expect("directory");
            let path = dir.path().join("jobs.sqlite");
            let (mut db, mut lease, parts) = fixture(&path);
            let mut worker = DurableDownload::open(&db, lease).expect("open");
            worker
                .restore_part(&mut db, &parts[0], || Ok(b"abcd".to_vec()))
                .expect("first extent");
            assert_eq!(
                worker.restore_part(&mut db, &parts[1], || Err(TransferError::Network)),
                Err(TransferError::Network)
            );
            assert!(dir.path().join("restored.bin.partial").exists());
            drop(worker);
            drop(db);
            if corrupt {
                std::fs::write(dir.path().join("restored.bin.partial"), b"xxxx\0\0\0\0")
                    .expect("corrupt local bytes");
            }
            let mut db = Database::open(&path).expect("reopen");
            db.recover_vault_jobs(7, 102).expect("restart");
            lease.generation = 2;
            assert!(
                db.transition_vault_job(
                    lease,
                    VaultJobState::Queued,
                    VaultJobTransition::Start,
                    103,
                    None
                )
                .expect("resume")
            );
            lease.generation = 3;
            let mut worker = DurableDownload::open(&db, lease).expect("reopen partial");
            let mut fetched = false;
            worker
                .restore_part(&mut db, &parts[0], || {
                    fetched = true;
                    Ok(b"abcd".to_vec())
                })
                .expect("verify first");
            assert_eq!(
                fetched, corrupt,
                "valid local receipt never requires another remote read"
            );
            worker
                .restore_part(&mut db, &parts[1], || Ok(b"efgh".to_vec()))
                .expect("remaining extent");
            worker.finalize(&db).expect("verified publish");
            assert_eq!(
                std::fs::read(dir.path().join("restored.bin")).expect("final"),
                b"abcdefgh"
            );
            drop(worker);
            let mut finalization =
                DurableDownload::open(&db, lease).expect("reopen after final rename");
            finalization
                .restore_part(&mut db, &parts[0], || {
                    panic!("complete final bytes need no remote read")
                })
                .expect("verified final extent");
            finalization.finalize(&db).expect("idempotent finalization");
            std::fs::write(dir.path().join("restored.bin"), b"foreign!")
                .expect("external replacement");
            assert!(matches!(
                DurableDownload::open(&db, lease),
                Err(TransferError::HashMismatch)
            ));
            assert_eq!(
                std::fs::read(dir.path().join("restored.bin")).expect("preserved replacement"),
                b"foreign!"
            );
            assert_eq!(
                db.vault_job(7, 9).expect("job").expect("present").state,
                VaultJobState::Running,
                "inventory owner must acknowledge completion separately"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn restart_rejects_symlink_output_even_when_target_has_expected_bytes() {
        let dir = tempfile::tempdir().expect("directory");
        let (db, lease, _) = fixture(&dir.path().join("jobs.sqlite"));
        let foreign = dir.path().join("foreign.bin");
        std::fs::write(&foreign, b"abcdefgh").expect("foreign file");
        let output = dir.path().join("restored.bin");
        std::os::unix::fs::symlink(&foreign, &output).expect("symlink");
        assert!(matches!(
            DurableDownload::open(&db, lease),
            Err(TransferError::PermissionDenied)
        ));
        assert_eq!(std::fs::read(&foreign).expect("preserved"), b"abcdefgh");
        assert!(!dir.path().join("restored.bin.partial").exists());
    }

    #[cfg(unix)]
    #[test]
    fn resumed_final_file_cannot_be_replaced_between_verification_and_completion() {
        for symlink in [false, true] {
            let dir = tempfile::tempdir().expect("directory");
            let (mut db, lease, parts) = fixture(&dir.path().join("jobs.sqlite"));
            let output = dir.path().join("restored.bin");
            std::fs::write(&output, b"abcdefgh").expect("published output");
            let mut worker = DurableDownload::open(&db, lease).expect("reopen final");
            std::fs::rename(&output, dir.path().join("original.bin")).expect("move original");
            if symlink {
                std::os::unix::fs::symlink(dir.path().join("original.bin"), &output)
                    .expect("replacement link");
            } else {
                std::fs::write(&output, b"abcdefgh").expect("same-byte foreign replacement");
            }
            assert_eq!(
                worker.restore_part(&mut db, &parts[0], || panic!("no remote read")),
                Err(TransferError::PermissionDenied)
            );
            assert_eq!(worker.finalize(&db), Err(TransferError::PermissionDenied));
            assert_eq!(
                std::fs::read(&output).expect("preserved output"),
                b"abcdefgh"
            );
            assert_eq!(
                db.vault_job(7, 9).expect("read").expect("job").state,
                VaultJobState::Running
            );
        }
    }

    #[test]
    fn stop_during_fetch_prevents_write_and_false_receipt() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("jobs.sqlite");
        let (mut db, lease, parts) = fixture(&path);
        let cancellation = crate::TelegramScanCancellation::new();
        let mut worker =
            DurableDownload::open_cancellable(&db, lease, cancellation.clone()).expect("open");
        assert_eq!(
            worker.restore_part(&mut db, &parts[0], || {
                let mut control = Database::open(&path).expect("independent control");
                assert!(
                    control
                        .transition_vault_job(
                            lease,
                            VaultJobState::Running,
                            VaultJobTransition::RequestPause,
                            102,
                            None
                        )
                        .expect("pause")
                );
                cancellation.cancel();
                Ok(b"abcd".to_vec())
            }),
            Err(TransferError::Cancelled)
        );
        assert!(
            db.vault_parts(7, 9, None, 1).expect("parts")[0]
                .receipt
                .is_none()
        );
        assert_eq!(
            std::fs::read(dir.path().join("restored.bin.partial")).expect("untouched"),
            [0; 8]
        );
        assert_eq!(worker.finalize(&db), Err(TransferError::Cancelled));
    }

    #[test]
    fn unverified_remote_bytes_never_receive_a_local_receipt() {
        let dir = tempfile::tempdir().expect("directory");
        let (mut db, lease, parts) = fixture(&dir.path().join("jobs.sqlite"));
        let mut worker = DurableDownload::open(&db, lease).expect("open");
        assert_eq!(
            worker.restore_part(&mut db, &parts[0], || Ok(b"fake".to_vec())),
            Err(TransferError::HashMismatch)
        );
        assert!(
            db.vault_parts(7, 9, None, 1).expect("parts")[0]
                .receipt
                .is_none()
        );
        assert_eq!(worker.finalize(&db), Err(TransferError::HashMismatch));
        assert!(!dir.path().join("restored.bin").exists());
    }
}
