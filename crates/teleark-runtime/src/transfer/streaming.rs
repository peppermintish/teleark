//! Bounded in-memory encryption and tiny recovery metadata, with read-only legacy spool replay.
use super::*;
use crate::VaultPartRecovery;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use teleark_telegram::{TransferTuning, UPLOAD_PART_BYTES, UploadCheckpoint, UploadStream};

fn map_spool_error(error: std::io::Error) -> TransferError {
    match error.kind() {
        std::io::ErrorKind::StorageFull => TransferError::DiskFull,
        std::io::ErrorKind::PermissionDenied => TransferError::PermissionDenied,
        _ => TransferError::Database,
    }
}
pub(crate) fn spool_prefix(root: &Path, identity: &[u8]) -> PathBuf {
    root.join(blake3::hash(identity).to_hex().as_str())
}
fn read_seal(
    prefix: &Path,
    identity: &[u8],
    header: &PartHeader,
) -> Option<teleark_crypto::EncryptedPartSummary> {
    let mut file = File::open(prefix.with_extension("seal")).ok()?;
    if file.metadata().ok()?.len() != 192 {
        return None;
    }
    let mut bytes = [0u8; 192];
    file.read_exact(&mut bytes).ok()?;
    if bytes.len() != 192 || bytes[..96] != header.encode() {
        return None;
    }
    let mut hash = blake3::Hasher::new();
    hash.update(identity);
    hash.update(&bytes[..160]);
    if hash.finalize().as_bytes() != &bytes[160..] {
        return None;
    }
    Some(teleark_crypto::EncryptedPartSummary {
        header: header.clone(),
        plaintext_blake3: bytes[96..128].try_into().ok()?,
        encoded_blake3: bytes[128..160].try_into().ok()?,
        encoded_length: header.expected_encoded_length().ok()?,
    })
}
pub(crate) fn reusable_spool(
    root: &Path,
    identity: &[u8],
    reservation: &VaultPartRecovery,
    receipted: bool,
) -> bool {
    let prefix = spool_prefix(root, identity);
    let Some(seal) = read_seal(&prefix, identity, &reservation.header) else {
        return false;
    };
    if seal.plaintext_blake3 != reservation.plaintext_blake3 {
        return false;
    }
    if receipted {
        return true;
    }
    let Ok(mut file) = File::open(prefix.with_extension("ciphertext")) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if metadata.len() != seal.encoded_length {
        return false;
    }
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; UPLOAD_PART_BYTES];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buffer[..n]);
            }
            Err(_) => return false,
        }
    }
    hasher.finalize().as_bytes() == &seal.encoded_blake3
}
fn load_checkpoint(
    path: &Path,
    total: u64,
    now: u64,
    sealed: bool,
) -> Result<UploadCheckpoint, TransferError> {
    match File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::with_capacity(32_768);
            file.take(32_768)
                .read_to_end(&mut bytes)
                .map_err(map_spool_error)?;
            // Newer checkpoint codecs remain intact; do not downgrade them.
            if bytes.starts_with(b"TARKUP") && bytes.get(..8) != Some(b"TARKUP01") {
                return Err(TransferError::ManifestCorrupted);
            }
            if let Some(checkpoint) = UploadCheckpoint::decode(&bytes, total) {
                if sealed && checkpoint.resumable(now) {
                    return Ok(checkpoint);
                }
            } else {
                std::fs::rename(path, path.with_extension("upload-corrupt"))
                    .map_err(map_spool_error)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(TransferError::Database),
    }
    UploadCheckpoint::new(total, now).map_err(|_| TransferError::KeyUnavailable)
}

fn save_seal(
    prefix: &Path,
    identity: &[u8],
    summary: &teleark_crypto::EncryptedPartSummary,
) -> Result<(), TransferError> {
    let mut bytes = summary.header.encode().to_vec();
    bytes.extend_from_slice(&summary.plaintext_blake3);
    bytes.extend_from_slice(&summary.encoded_blake3);
    let mut hash = blake3::Hasher::new();
    hash.update(identity);
    hash.update(&bytes);
    bytes.extend_from_slice(hash.finalize().as_bytes());
    let mut file = private_file(&prefix.with_extension("seal-pending"), false)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(map_spool_error)?;
    std::fs::rename(
        prefix.with_extension("seal-pending"),
        prefix.with_extension("seal"),
    )
    .map_err(map_spool_error)
}
pub(super) fn private_file(path: &Path, exclusive: bool) -> Result<File, TransferError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(map_spool_error)
}
struct PipeWriter {
    pending: Vec<u8>,
    sender: tokio::sync::mpsc::Sender<Vec<u8>>,
    recycled: tokio::sync::mpsc::Receiver<Vec<u8>>,
}
impl PipeWriter {
    fn emit(&mut self) -> std::io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut next = self
            .recycled
            .try_recv()
            .unwrap_or_else(|_| Vec::with_capacity(UPLOAD_PART_BYTES));
        next.clear();
        let bytes = std::mem::replace(&mut self.pending, next);
        self.sender
            .blocking_send(bytes)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))
    }
}
impl Write for PipeWriter {
    fn write(&mut self, mut bytes: &[u8]) -> std::io::Result<usize> {
        let length = bytes.len();
        while !bytes.is_empty() {
            let count = bytes.len().min(UPLOAD_PART_BYTES - self.pending.len());
            self.pending.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
            if self.pending.len() == UPLOAD_PART_BYTES {
                self.emit()?;
            }
        }
        Ok(length)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.emit()
    }
}
impl<S: ReservedPublicationStore> EncryptedRemoteTransport<S> {
    pub(crate) fn reconcile_summary_receipt(
        &mut self,
        key: RemotePartKey,
        reservation: &VaultPartRecovery,
        identity: &[u8],
        root: &Path,
    ) -> Result<Option<RemoteObject>, TransferError> {
        let Some(summary) = read_seal(&spool_prefix(root, identity), identity, &reservation.header)
        else {
            return Ok(None);
        };
        if summary.plaintext_blake3 != reservation.plaintext_blake3 {
            return Err(TransferError::HashMismatch);
        }
        let candidates = self
            .store
            .search_exact_caption(&self.caption(key), MAX_RECONCILIATION_RESULTS)?;
        if candidates.len() >= MAX_RECONCILIATION_RESULTS {
            return Err(TransferError::RemoteMissing);
        }
        for object in candidates {
            if object.name != self.name(key) || object.encoded_size != summary.encoded_length {
                continue;
            }
            let mut reader = self
                .store
                .download_reader(object.object_id, object.encoded_size)?;
            let mut hash = blake3::Hasher::new();
            let mut buffer = vec![0; UPLOAD_PART_BYTES];
            loop {
                if self
                    .cancellation
                    .as_ref()
                    .is_some_and(crate::TelegramScanCancellation::is_cancelled)
                {
                    return Err(TransferError::Cancelled);
                }
                let n = reader
                    .read(&mut buffer)
                    .map_err(|_| reader.error().unwrap_or(TransferError::Network))?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            if hash.finalize().as_bytes() == &summary.encoded_blake3 {
                return self
                    .accept_uploaded_part(self.summary_part(key, &summary), object)
                    .map(Some);
            }
        }
        Ok(None)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn stream_reserved_source(
        &mut self,
        key: RemotePartKey,
        reservation: &VaultPartRecovery,
        identity: &[u8],
        source: &Path,
        root: &Path,
        receipt: Option<u64>,
        tuning: TransferTuning,
    ) -> Result<RemoteObject, TransferError> {
        self.stream_reserved_source_once(
            key,
            reservation,
            identity,
            source,
            root,
            receipt,
            tuning,
            true,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn stream_reserved_source_once(
        &mut self,
        key: RemotePartKey,
        reservation: &VaultPartRecovery,
        identity: &[u8],
        source: &Path,
        root: &Path,
        receipt: Option<u64>,
        tuning: TransferTuning,
        allow_restart: bool,
    ) -> Result<RemoteObject, TransferError> {
        let plan = self.restore_reserved_plan(key, reservation)?;
        let prefix = spool_prefix(root, identity);
        let saved = read_seal(&prefix, identity, &reservation.header);
        // Older receipts can be recovered once through the authenticated reader,
        // without invoking encryption with their saved identity.
        if let Some(object_id) = receipt {
            if let Some(summary) = saved {
                let part = self.summary_part(key, &summary);
                return self.accept_uploaded_part(
                    part,
                    RemoteByteObject {
                        object_id,
                        name: self.name(key),
                        encoded_size: summary.encoded_length,
                    },
                );
            }
            let mut reader = self
                .store
                .download_reader(object_id, plan.expected_encoded_size)?;
            let decoded = decrypt_part_cancellable(
                &mut reader,
                &mut std::io::sink(),
                &self.file_key,
                self.limits,
                || {
                    self.cancellation
                        .as_ref()
                        .is_some_and(crate::TelegramScanCancellation::is_cancelled)
                },
            );
            if let Some(error) = reader.error() {
                return Err(error);
            }
            let decoded = decoded.map_err(map_crypto_error)?;
            if decoded.header != reservation.header
                || decoded.plaintext_blake3 != reservation.plaintext_blake3
            {
                return Err(TransferError::HashMismatch);
            }
            let summary = teleark_crypto::EncryptedPartSummary {
                header: decoded.header,
                plaintext_blake3: decoded.plaintext_blake3,
                encoded_blake3: decoded.encoded_blake3,
                encoded_length: decoded.encoded_length,
            };
            save_seal(&prefix, identity, &summary)?;
            return self.accept_uploaded_part(
                self.summary_part(key, &summary),
                RemoteByteObject {
                    object_id,
                    name: self.name(key),
                    encoded_size: summary.encoded_length,
                },
            );
        }
        // Only ambiguous publication recovery performs a bounded read-back.
        // Fresh uploads avoid readback; imported receipts are authenticated above.
        if let Some(summary) = &saved {
            let candidates = self
                .store
                .search_exact_caption(&self.caption(key), MAX_RECONCILIATION_RESULTS)?;
            if candidates.len() >= MAX_RECONCILIATION_RESULTS {
                return Err(TransferError::RemoteMissing);
            }
            for object in candidates {
                if object.name == self.name(key) && object.encoded_size == summary.encoded_length {
                    let mut reader = self
                        .store
                        .download_reader(object.object_id, object.encoded_size)?;
                    let mut hash = blake3::Hasher::new();
                    let mut buffer = vec![0; UPLOAD_PART_BYTES];
                    loop {
                        let count = reader
                            .read(&mut buffer)
                            .map_err(|_| reader.error().unwrap_or(TransferError::Network))?;
                        if count == 0 {
                            break;
                        }
                        hash.update(&buffer[..count]);
                    }
                    if hash.finalize().as_bytes() == &summary.encoded_blake3 {
                        return self.accept_uploaded_part(self.summary_part(key, summary), object);
                    }
                }
            }
        }
        if saved.is_some() && !prefix.with_extension("ciphertext").exists() {
            // Only a new durable identity may re-encrypt after in-memory blocks are gone.
            return Err(TransferError::Network);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TransferError::Database)?
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let checkpoint_path = prefix.with_extension("upload");
        let checkpoint = load_checkpoint(
            &checkpoint_path,
            plan.expected_encoded_size,
            now,
            saved.is_some(),
        )?;
        checkpoint.save(&checkpoint_path).map_err(map_spool_error)?;
        let original_file_id = checkpoint.file_id;
        let replay_checkpoint_path = checkpoint_path.clone();
        let replay_source = source.to_owned();
        let replay_identity = identity.to_vec();
        let (sender, blocks) = tokio::sync::mpsc::channel(usize::from(tuning.upload_queue));
        let (recycled, recycle_receiver) =
            tokio::sync::mpsc::channel(usize::from(tuning.upload_parts + tuning.upload_queue + 2));
        let (seal_sender, sealed) = tokio::sync::oneshot::channel();
        let stream = UploadStream {
            total_bytes: plan.expected_encoded_size,
            blocks,
            recycled,
            checkpoint,
            checkpoint_path,
            sealed,
        };
        let context = self.encryption_context();
        let source = source.to_owned();
        let identity = identity.to_owned();
        let prefix_worker = prefix.clone();
        let producer = std::thread::Builder::new()
            .name("teleark-upload-crypto".into())
            .spawn(move || {
                let result = (|| {
                    if let Some(summary) =
                        saved.filter(|_| prefix_worker.with_extension("ciphertext").exists())
                    {
                        let mut file = File::open(prefix_worker.with_extension("ciphertext"))
                            .map_err(|_| TransferError::SourceMissing)?;
                        let mut recycled = recycle_receiver;
                        for index in 0..summary.encoded_length.div_ceil(UPLOAD_PART_BYTES as u64) {
                            if context
                                .cancellation
                                .as_ref()
                                .is_some_and(crate::TelegramScanCancellation::is_cancelled)
                            {
                                return Err(TransferError::Cancelled);
                            }
                            let count = (summary.encoded_length - index * UPLOAD_PART_BYTES as u64)
                                .min(UPLOAD_PART_BYTES as u64)
                                as usize;
                            let mut bytes = recycled
                                .try_recv()
                                .unwrap_or_else(|_| Vec::with_capacity(UPLOAD_PART_BYTES));
                            bytes.resize(count, 0);
                            file.read_exact(&mut bytes)
                                .map_err(|_| TransferError::SourceMissing)?;
                            sender
                                .blocking_send(bytes)
                                .map_err(|_| TransferError::Cancelled)?;
                        }
                        Ok(summary)
                    } else {
                        let mut file =
                            File::open(&source).map_err(|_| TransferError::SourceMissing)?;
                        file.seek(SeekFrom::Start(plan.header.plaintext_offset))
                            .map_err(|_| TransferError::SourceChanged)?;
                        let mut writer = PipeWriter {
                            pending: Vec::with_capacity(UPLOAD_PART_BYTES),
                            sender,
                            recycled: recycle_receiver,
                        };
                        let summary = encrypt_part_cancellable(
                            &mut file.take(plan.header.plaintext_length),
                            &mut writer,
                            &plan.header,
                            &context.file_key,
                            context.limits,
                            &mut AeadUsageRegistry::new(),
                            || {
                                context
                                    .cancellation
                                    .as_ref()
                                    .is_some_and(crate::TelegramScanCancellation::is_cancelled)
                            },
                        )
                        .map_err(map_crypto_error)?;
                        if Some(ContentDigest(summary.plaintext_blake3)) != plan.expected_digest {
                            return Err(TransferError::SourceChanged);
                        }
                        writer.flush().map_err(map_spool_error)?;
                        save_seal(&prefix_worker, &identity, &summary)?;
                        Ok(summary)
                    }
                })();
                let _ = seal_sender.send(result.is_ok());
                result
            })
            .map_err(map_spool_error)?;
        // This is a retained background transfer owner, never the UI/network reactor.
        let result = self.store.upload_stream_reserved(
            &self.name(key),
            &self.caption(key),
            stream,
            std::num::NonZeroI64::new(reservation.publication_random_id)
                .ok_or(TransferError::ManifestCorrupted)?,
            tuning,
        );
        let summary = producer.join().map_err(|_| TransferError::Database)?;
        if let Err(error) = &summary
            && !matches!(error, TransferError::Cancelled)
        {
            return Err(error.clone());
        }
        if result.is_err()
            && allow_restart
            && prefix.with_extension("ciphertext").exists()
            && summary.is_ok()
            && std::fs::read(&replay_checkpoint_path)
                .ok()
                .and_then(|bytes| {
                    UploadCheckpoint::decode(
                        &bytes,
                        reservation.header.expected_encoded_length().ok()?,
                    )
                })
                .is_some_and(|checkpoint| checkpoint.file_id != original_file_id)
        {
            return self.stream_reserved_source_once(
                key,
                reservation,
                &replay_identity,
                &replay_source,
                root,
                None,
                tuning,
                false,
            );
        }
        let object = result.map_err(|error| match error {
            UploadError::Definite(error) => error,
            UploadError::AmbiguousSuccess => TransferError::Network,
        })?;
        let summary = summary?;
        self.accept_uploaded_part(self.summary_part(key, &summary), object)
    }
    fn summary_part(
        &self,
        key: RemotePartKey,
        summary: &teleark_crypto::EncryptedPartSummary,
    ) -> ManifestPart {
        ManifestPart {
            part_index: key.part_index.get(),
            part_instance_id: summary.header.part_instance_id.0,
            plaintext_offset: summary.header.plaintext_offset,
            plaintext_length: summary.header.plaintext_length,
            encoded_length: summary.encoded_length,
            frame_count: summary.header.frame_count,
            plaintext_blake3: summary.plaintext_blake3,
            encoded_ciphertext_blake3: summary.encoded_blake3,
            remote_locator: RemoteLocator {
                account_id: key.account_id.get(),
                chat_id: self.chat_id,
                message_id: 0,
                remote_name: self.name(key),
                locator_version: 1,
                locator_extension: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StreamingProbe {
        prefix: PathBuf,
        interrupt: bool,
        blocks_received: usize,
    }

    impl RemoteObjectStore for StreamingProbe {
        fn search_exact_caption(
            &mut self,
            _: &str,
            _: usize,
        ) -> Result<Vec<RemoteByteObject>, TransferError> {
            panic!("fresh streaming upload must not search remote content");
        }

        fn upload(
            &mut self,
            _: &str,
            _: &str,
            _: Vec<u8>,
        ) -> Result<RemoteByteObject, UploadError> {
            panic!("fresh streaming upload must not buffer a whole container");
        }

        fn download(&mut self, _: u64) -> Result<Vec<u8>, TransferError> {
            panic!("fresh streaming upload must not read back remote content");
        }
    }

    impl ReservedPublicationStore for StreamingProbe {
        fn upload_reserved(
            &mut self,
            _: &str,
            _: &str,
            _: Vec<u8>,
            _: std::num::NonZeroI64,
        ) -> Result<RemoteByteObject, UploadError> {
            panic!("the reserved publication must use the streaming path");
        }

        fn upload_stream_reserved(
            &mut self,
            name: &str,
            _: &str,
            mut stream: UploadStream,
            _: std::num::NonZeroI64,
            _tuning: TransferTuning,
        ) -> Result<RemoteByteObject, UploadError> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("test receiver");
            let first = runtime.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(30), stream.blocks.recv())
                    .await
                    .expect("the first block must arrive without completing encryption")
                    .expect("first encrypted block")
            });
            assert_eq!(first.len(), UPLOAD_PART_BYTES);
            // Keep the consumer stopped after its first receive. The bounded
            // queue allows only ready blocks plus the producer's pending write,
            // regardless of how long the producer runs before these assertions.
            assert!(
                !self.prefix.with_extension("ciphertext").exists(),
                "ciphertext stays in memory"
            );
            assert!(
                !self.prefix.with_extension("seal").exists(),
                "backpressure bounds preparation"
            );
            self.blocks_received = 1;
            if self.interrupt {
                return Err(UploadError::Definite(TransferError::Network));
            }
            let mut received = first.len() as u64;
            let _ = stream.recycled.try_send(first);
            while let Some(block) = stream.blocks.blocking_recv() {
                let expected = (stream.total_bytes - received).min(UPLOAD_PART_BYTES as u64);
                assert_eq!(block.len() as u64, expected);
                received += block.len() as u64;
                self.blocks_received += 1;
                let _ = stream.recycled.try_send(block);
            }
            assert_eq!(received, stream.total_bytes);
            assert_eq!(stream.sealed.blocking_recv(), Ok(true));
            Ok(RemoteByteObject {
                object_id: 1,
                name: name.into(),
                encoded_size: received,
            })
        }
    }

    #[test]
    fn first_512_kib_block_reaches_upload_before_60_mib_container_is_encrypted() {
        for interrupt in [false, true] {
            let dir = tempfile::tempdir().expect("directory");
            let source = dir.path().join("source");
            let mut file = File::create(&source).expect("source");
            let block = vec![0x5a; UPLOAD_PART_BYTES];
            let mut digest = blake3::Hasher::new();
            let total = 60 * 1024 * 1024;
            for _ in 0..total / UPLOAD_PART_BYTES as u64 {
                file.write_all(&block).expect("synthetic content");
                digest.update(&block);
            }
            drop(file);
            let key = RemotePartKey {
                account_id: AccountId::new(9),
                package_id: PackageId::new(11),
                part_index: PartIndex::new(0),
            };
            let mut transport = EncryptedRemoteTransport::new(
                StreamingProbe {
                    prefix: PathBuf::new(),
                    interrupt,
                    blocks_received: 0,
                },
                key.account_id,
                99,
                key.package_id,
                FileKey::from_bytes([7; 32]),
                total,
                vec![total],
            )
            .expect("transport");
            let reservation = transport
                .reserve_part_identity(
                    key,
                    ContentDigest(*digest.finalize().as_bytes()),
                    std::num::NonZeroI64::new(42).expect("publication id"),
                )
                .expect("reservation");
            let identity = reservation.encode().expect("recovery identity");
            transport.store.prefix = spool_prefix(dir.path(), &identity);
            let result = transport.stream_reserved_source(
                key,
                &reservation,
                &identity,
                &source,
                dir.path(),
                None,
                TransferTuning::default(),
            );
            if interrupt {
                assert!(result.is_err(), "a dropped consumer must stop its producer");
                assert_eq!(transport.store.blocks_received, 1);
            } else {
                let object = result.expect("streamed publication");
                assert_eq!(
                    object.encoded_size,
                    reservation
                        .header
                        .expected_encoded_length()
                        .expect("encoded size")
                );
                let expected_blocks = (reservation
                    .header
                    .expected_encoded_length()
                    .expect("encoded size") as usize)
                    .div_ceil(UPLOAD_PART_BYTES);
                assert_eq!(transport.store.blocks_received, expected_blocks);
            }
        }
    }

    #[test]
    fn temporary_checkpoints_preserve_newer_bytes_and_recover_corruption_without_nonce_work() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("part.upload");
        let checkpoint = UploadCheckpoint::new(512 * 1024, 100).expect("identity");
        checkpoint.save(&path).expect("save");
        assert_eq!(
            load_checkpoint(&path, 512 * 1024, 101, true)
                .expect("resume")
                .file_id,
            checkpoint.file_id
        );
        assert_ne!(
            load_checkpoint(
                &path,
                512 * 1024,
                100 + teleark_telegram::UPLOAD_RESUME_WINDOW_MS,
                true
            )
            .expect("expired")
            .file_id,
            checkpoint.file_id
        );
        assert_ne!(
            load_checkpoint(&path, 512 * 1024, 101, false)
                .expect("unsealed")
                .file_id,
            checkpoint.file_id
        );
        let mut newer = checkpoint.encode();
        newer[7] = b'2';
        std::fs::write(&path, &newer).expect("future");
        assert!(load_checkpoint(&path, 512 * 1024, 101, true).is_err());
        assert_eq!(std::fs::read(&path).expect("preserved"), newer);
        let mut corrupt = checkpoint.encode();
        corrupt[32] ^= 1;
        std::fs::write(&path, &corrupt).expect("corrupt");
        assert_ne!(
            load_checkpoint(&path, 512 * 1024, 101, true)
                .expect("recover")
                .file_id,
            checkpoint.file_id
        );
        assert_eq!(
            std::fs::read(path.with_extension("upload-corrupt")).expect("original"),
            corrupt
        );
    }
}
