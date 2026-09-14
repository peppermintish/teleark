//! Ciphertext spool and bounded producer. No encryption identity is replayed.
use super::*;
use crate::VaultPartRecovery;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use teleark_telegram::{TransferTuning, UPLOAD_PART_BYTES, UploadCheckpoint, UploadStream};

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
        .map_err(|_| TransferError::Database)?;
    std::fs::rename(
        prefix.with_extension("seal-pending"),
        prefix.with_extension("seal"),
    )
    .map_err(|_| TransferError::Database)
}
fn private_file(path: &Path, exclusive: bool) -> Result<File, TransferError> {
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
    options.open(path).map_err(|_| TransferError::Database)
}
struct PipeWriter {
    file: File,
    pending: Vec<u8>,
    sender: tokio::sync::mpsc::Sender<Vec<u8>>,
    recycled: tokio::sync::mpsc::Receiver<Vec<u8>>,
}
impl PipeWriter {
    fn emit(&mut self) -> std::io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.file.write_all(&self.pending)?;
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
        self.emit()?;
        self.file.flush()
    }
}
impl<S: ReservedPublicationStore> EncryptedRemoteTransport<S> {
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
            let bytes = self.store.download(object_id)?;
            let object = RemoteByteObject {
                object_id,
                name: self.name(key),
                encoded_size: bytes.len() as u64,
            };
            let (_, decoded) = self.decode(key, &object, &bytes)?;
            let part = self
                .manifest_parts
                .get(&key.part_index.get())
                .ok_or(TransferError::ManifestCorrupted)?;
            if part.part_instance_id != reservation.header.part_instance_id.0
                || decoded.digest.0 != reservation.plaintext_blake3
            {
                return Err(TransferError::HashMismatch);
            }
            return Ok(decoded);
        }
        // Only ambiguous publication recovery performs a bounded read-back.
        // Fresh uploads and saved receipts never read remote ciphertext.
        if let Some(summary) = &saved {
            let candidates = self
                .store
                .search_exact_caption(&self.caption(key), MAX_RECONCILIATION_RESULTS)?;
            if candidates.len() >= MAX_RECONCILIATION_RESULTS {
                return Err(TransferError::RemoteMissing);
            }
            for object in candidates {
                if object.name == self.name(key) && object.encoded_size == summary.encoded_length {
                    let bytes = self.store.download(object.object_id)?;
                    if blake3::hash(&bytes).as_bytes() == &summary.encoded_blake3 {
                        return self.accept_uploaded_part(self.summary_part(key, summary), object);
                    }
                }
            }
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TransferError::Database)?
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let checkpoint_path = prefix.with_extension("upload");
        let checkpoint = std::fs::read(&checkpoint_path)
            .ok()
            .and_then(|bytes| UploadCheckpoint::decode(&bytes, plan.expected_encoded_size))
            .filter(|checkpoint| saved.is_some() && checkpoint.resumable(now))
            .map(Ok)
            .unwrap_or_else(|| UploadCheckpoint::new(plan.expected_encoded_size, now))
            .map_err(|_| TransferError::Database)?;
        checkpoint
            .save(&checkpoint_path)
            .map_err(|_| TransferError::Database)?;
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
                    if let Some(summary) = saved {
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
                            file: private_file(&prefix_worker.with_extension("ciphertext"), true)?,
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
                        writer
                            .flush()
                            .and_then(|()| writer.file.sync_all())
                            .map_err(|_| TransferError::Database)?;
                        save_seal(&prefix_worker, &identity, &summary)?;
                        Ok(summary)
                    }
                })();
                let _ = seal_sender.send(result.is_ok());
                result
            })
            .map_err(|_| TransferError::Database)?;
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
        if result.is_err()
            && allow_restart
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
