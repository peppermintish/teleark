//! Verified, fsynced download extents. Run only on a retained background owner.
use crate::{VaultRecoveryContext, VaultRecoveryDirection};
use teleark_core::TransferError;
use teleark_crypto::ManifestPart;
use teleark_storage::{Database, VaultJobLease, VaultJobState, VaultPartRecord};
use teleark_transfer::{ContentDigest, DestinationId, FileSystemPort, NativeFileSystem};

const REMOTE_READ_RETRIES: u8 = 2;

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
        mut fetch: impl FnMut() -> Result<Vec<u8>, TransferError>,
    ) -> Result<ExtentSource, TransferError> {
        self.restore_part_to(database, part, |out| {
            let bytes = fetch()?;
            out.write_all(&bytes).map_err(|_| TransferError::Database)
        })
    }

    #[cfg(test)]
    pub fn restore_part_to(
        &mut self,
        database: &mut Database,
        part: &ManifestPart,
        fetch: impl FnMut(&mut dyn std::io::Write) -> Result<(), TransferError>,
    ) -> Result<ExtentSource, TransferError> {
        self.restore_part_to_with_retry(database, part, fetch, |_, _| {})
    }

    pub fn restore_part_to_with_retry(
        &mut self,
        database: &mut Database,
        part: &ManifestPart,
        mut fetch: impl FnMut(&mut dyn std::io::Write) -> Result<(), TransferError>,
        mut retry_scheduled: impl FnMut(u8, std::time::Duration),
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
            let digest = match range_digest(
                part,
                |offset, length| {
                    self.files
                        .read_partial(DestinationId(self.lease.id), offset, length)
                },
                &self.cancellation,
            ) {
                Ok(digest) => Some(digest),
                // A receipt proves the intended extent identity, not the
                // current bytes in the local cache. Short/corrupt local ranges
                // are cache misses; preserve genuine I/O and permission errors.
                Err(TransferError::HashMismatch) => None,
                Err(error) => return Err(error),
            };
            if digest == Some(part.plaintext_blake3) {
                return Ok(ExtentSource::Local);
            }
        }
        let mut retries = 0_u8;
        loop {
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
            if let Some(error) = writer.error.take() {
                return Err(error);
            }
            let complete = writer.remaining == 0
                && writer.hash.finalize().as_bytes() == &part.plaintext_blake3;
            drop(writer);
            match fetched {
                Ok(()) if complete => break,
                Ok(()) => return Err(TransferError::HashMismatch),
                Err(TransferError::Network) if retries < REMOTE_READ_RETRIES => {
                    retries += 1;
                    let delay = remote_read_retry_delay(retries);
                    retry_scheduled(retries, delay);
                    wait_for_retry(delay, &self.cancellation)?;
                }
                Err(error) => return Err(error),
            }
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

fn remote_read_retry_delay(retry: u8) -> std::time::Duration {
    // Small bounded backoff for transient remote reads; durable control and
    // cancellation remain responsive during the wait.
    std::time::Duration::from_millis(100 * u64::from(retry))
}

fn wait_for_retry(
    delay: std::time::Duration,
    cancellation: &crate::TelegramScanCancellation,
) -> Result<(), TransferError> {
    let started = std::time::Instant::now();
    loop {
        if cancellation.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        let remaining = delay.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(std::time::Duration::from_millis(10)));
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
        fixture_with_data(path, b"abcdefgh", 4)
    }

    fn fixture_with_data(
        path: &std::path::Path,
        bytes: &[u8],
        part_size: usize,
    ) -> (Database, VaultJobLease, Vec<ManifestPart>) {
        fixture_with_expected_digest(path, bytes, part_size, *blake3::hash(bytes).as_bytes())
    }

    fn fixture_with_expected_digest(
        path: &std::path::Path,
        bytes: &[u8],
        part_size: usize,
        whole_plaintext_blake3: [u8; 32],
    ) -> (Database, VaultJobLease, Vec<ManifestPart>) {
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
            size_bytes: bytes.len() as u64,
            direction: VaultRecoveryDirection::Download {
                destination: path.with_file_name("restored.bin"),
                manifest_message_id: 10,
                manifest_blake3: [5; 32],
                whole_plaintext_blake3,
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
        let parts = bytes
            .chunks(part_size)
            .enumerate()
            .map(|(index, plaintext)| ManifestPart {
                part_index: index as u32,
                part_instance_id: [index as u8 + 1; 16],
                plaintext_offset: (index * part_size) as u64,
                plaintext_length: plaintext.len() as u64,
                encoded_length: plaintext.len() as u64 + 128,
                frame_count: plaintext.len().div_ceil(512 * 1024) as u32,
                plaintext_blake3: *blake3::hash(plaintext).as_bytes(),
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
            .collect();
        (db, lease, parts)
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

    #[test]
    fn restart_recovers_nonzero_truncated_partial_and_refetches_short_receipt_range() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("jobs.sqlite");
        let (mut db, mut lease, parts) = fixture(&path);
        let mut worker = DurableDownload::open(&db, lease).expect("open");
        worker
            .restore_part(&mut db, &parts[0], || Ok(b"abcd".to_vec()))
            .expect("saved receipt extent");
        assert_eq!(
            worker.restore_part_to(&mut db, &parts[1], |output| {
                output
                    .write_all(b"ef")
                    .map_err(|_| TransferError::Database)?;
                Err(TransferError::HashMismatch)
            }),
            Err(TransferError::HashMismatch),
            "a failed extent remains unreceipted"
        );
        assert!(
            db.vault_parts(7, 9, Some(0), 1)
                .expect("second receipt query")[0]
                .receipt
                .is_none()
        );
        drop(worker);
        drop(db);

        let partial = dir.path().join("restored.bin.partial");
        std::fs::write(&partial, b"ab").expect("truncate to a nonzero prefix");
        let metadata = std::fs::metadata(&partial).expect("truncated partial");
        assert_eq!(metadata.len(), 2);

        let mut db = Database::open(&path).expect("restart database");
        db.recover_vault_jobs(7, 102).expect("recover job");
        lease.generation = 2;
        assert!(
            db.transition_vault_job(
                lease,
                VaultJobState::Queued,
                VaultJobTransition::Start,
                103,
                None,
            )
            .expect("resume job")
        );
        lease.generation = 3;
        let mut worker = DurableDownload::open(&db, lease).expect("open truncated partial");
        assert_eq!(
            std::fs::metadata(&partial)
                .expect("partial stays truncated")
                .len(),
            2,
            "opening must not hide a truncated receipt range by extending it with zeros"
        );

        let mut refetched_receipted_extent = false;
        assert_eq!(
            worker
                .restore_part(&mut db, &parts[0], || {
                    refetched_receipted_extent = true;
                    Ok(b"abcd".to_vec())
                })
                .expect("short local range is a cache miss"),
            ExtentSource::Remote
        );
        assert!(refetched_receipted_extent);
        let mut fetched_missing_extent = false;
        assert_eq!(
            worker
                .restore_part(&mut db, &parts[1], || {
                    fetched_missing_extent = true;
                    Ok(b"efgh".to_vec())
                })
                .expect("resume missing extent"),
            ExtentSource::Remote
        );
        assert!(fetched_missing_extent);
        assert!(!dir.path().join("restored.bin").exists());
        assert!(partial.exists());

        worker
            .finalize(&db)
            .expect("verify whole file before publish");
        assert_eq!(
            std::fs::read(dir.path().join("restored.bin")).expect("published file"),
            b"abcdefgh"
        );
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

    #[test]
    fn truncated_extents_restart_at_the_part_start_and_only_confirm_full_verified_bytes() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("jobs.sqlite");
        let part_size = 512 * 1024;
        let bytes = (0..part_size * 3)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let (mut db, lease, parts) = fixture_with_data(&path, &bytes, part_size);
        let mut worker = DurableDownload::open(&db, lease).expect("open");
        let cuts = [0, part_size / 2, part_size - 1];

        for (part, cut) in parts.iter().zip(cuts) {
            let start = part.plaintext_offset as usize;
            let end = start + part.plaintext_length as usize;
            let plaintext = &bytes[start..end];
            let mut attempts = 0;
            let mut scheduled = Vec::new();
            worker
                .restore_part_to_with_retry(
                    &mut db,
                    part,
                    |output| {
                        attempts += 1;
                        if attempts == 1 {
                            output
                                .write_all(&plaintext[..cut])
                                .map_err(|_| TransferError::Database)?;
                            return Err(TransferError::Network);
                        }

                        let independent = Database::open(&path).expect("independent database");
                        assert!(
                            independent
                                .vault_parts(7, 9, part.part_index.checked_sub(1), 1)
                                .expect("reserved extent")
                                .into_iter()
                                .find(|saved| saved.part_index == part.part_index)
                                .expect("reservation")
                                .receipt
                                .is_none(),
                            "a truncated attempt must not create a receipt"
                        );
                        if cut > 0 {
                            let partial = std::fs::read(dir.path().join("restored.bin.partial"))
                                .expect("partial bytes remain available for retry");
                            assert_eq!(&partial[start..start + cut], &plaintext[..cut]);
                        }
                        output
                            .write_all(plaintext)
                            .map_err(|_| TransferError::Database)?;
                        Ok(())
                    },
                    |attempt, delay| scheduled.push((attempt, delay)),
                )
                .expect("bounded retry restores complete extent");
            assert_eq!(attempts, 2);
            assert_eq!(scheduled, [(1, std::time::Duration::from_millis(100))]);
            assert!(!dir.path().join("restored.bin").exists());
            let saved = db
                .vault_parts(7, 9, part.part_index.checked_sub(1), 1)
                .expect("part receipt")
                .into_iter()
                .find(|saved| saved.part_index == part.part_index)
                .expect("reservation");
            assert!(saved.receipt.is_some());
        }

        worker.finalize(&db).expect("verified publish");
        assert_eq!(
            std::fs::read(dir.path().join("restored.bin")).expect("published bytes"),
            bytes
        );
    }

    #[test]
    fn retries_are_bounded_and_integrity_failures_are_terminal_without_receipts() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("jobs.sqlite");
        let bytes = b"abcdefgh";
        let (mut db, lease, parts) = fixture_with_data(&path, bytes, 4);
        let mut worker = DurableDownload::open(&db, lease).expect("open");
        let mut attempts = 0;
        let mut retries = Vec::new();
        assert_eq!(
            worker.restore_part_to_with_retry(
                &mut db,
                &parts[0],
                |output| {
                    attempts += 1;
                    output
                        .write_all(b"ab")
                        .map_err(|_| TransferError::Database)?;
                    Err(TransferError::Network)
                },
                |attempt, _| retries.push(attempt),
            ),
            Err(TransferError::Network)
        );
        assert_eq!(attempts, 3, "initial request plus two bounded retries");
        assert_eq!(retries, [1, 2]);
        assert!(dir.path().join("restored.bin.partial").exists());
        assert!(!dir.path().join("restored.bin").exists());
        assert!(
            db.vault_parts(7, 9, None, 1).expect("reservation")[0]
                .receipt
                .is_none()
        );

        drop(worker);
        let dir = tempfile::tempdir().expect("integrity fixture");
        let path = dir.path().join("jobs.sqlite");
        let (mut db, lease, parts) = fixture_with_data(&path, bytes, 4);
        let mut worker = DurableDownload::open(&db, lease).expect("open");
        for terminal in [
            TransferError::HashMismatch,
            TransferError::AuthenticationFailed,
        ] {
            let mut attempts = 0;
            let mut retries = 0;
            assert_eq!(
                worker.restore_part_to_with_retry(
                    &mut db,
                    &parts[0],
                    |_| {
                        attempts += 1;
                        Err(terminal.clone())
                    },
                    |_, _| retries += 1,
                ),
                Err(terminal)
            );
            assert_eq!(attempts, 1);
            assert_eq!(retries, 0);
            assert!(
                db.vault_parts(7, 9, None, 1).expect("reservation")[0]
                    .receipt
                    .is_none()
            );
        }
        assert!(!dir.path().join("restored.bin").exists());
    }

    #[test]
    fn final_whole_file_integrity_failure_never_publishes_the_local_destination() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("jobs.sqlite");
        let bytes = b"abcdefgh";
        let (mut db, lease, parts) = fixture_with_expected_digest(&path, bytes, 4, [0x55; 32]);
        let mut worker = DurableDownload::open(&db, lease).expect("open");
        for (part, part_bytes) in parts.iter().zip([b"abcd", b"efgh"]) {
            worker
                .restore_part(&mut db, part, || Ok(part_bytes.to_vec()))
                .expect("extent-level authenticated bytes");
        }
        assert_eq!(worker.finalize(&db), Err(TransferError::HashMismatch));
        assert!(dir.path().join("restored.bin.partial").exists());
        assert!(!dir.path().join("restored.bin").exists());
        assert!(
            db.vault_parts(7, 9, None, 2)
                .expect("receipts")
                .iter()
                .all(|part| part.receipt.is_some())
        );
    }
}
