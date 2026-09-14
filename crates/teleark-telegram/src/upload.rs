//! Bounded streaming upload: immutable blocks, explicit acknowledgements, no read-back.
use super::*;
use tokio::sync::mpsc;

pub const UPLOAD_PART_BYTES: usize = 512 * 1024;
pub const UPLOAD_RESUME_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;

/// Ciphertext only. The producer runs on a retained blocking owner.
pub struct UploadStream {
    pub total_bytes: u64,
    pub sealed: tokio::sync::oneshot::Receiver<bool>,
    pub blocks: mpsc::Receiver<Vec<u8>>,
    pub recycled: mpsc::Sender<Vec<u8>>,
    pub checkpoint: UploadCheckpoint,
    pub checkpoint_path: PathBuf,
}

/// Versioned temporary Telegram upload identity and acknowledged-part bitmap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadCheckpoint {
    pub file_id: i64,
    pub started_unix_ms: u64,
    pub total_bytes: u64,
    pub acknowledged: Vec<bool>,
}
impl UploadCheckpoint {
    pub fn new(total_bytes: u64, now: u64) -> Result<Self, TelegramError> {
        if total_bytes == 0 || total_bytes > MAX_TRANSFER_OBJECT_BYTES as u64 {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).map_err(|_| TelegramError::new(TelegramErrorKind::Session))?;
        let file_id = i64::from_le_bytes(bytes);
        if file_id == 0 {
            return Err(TelegramError::new(TelegramErrorKind::Session));
        }
        Ok(Self {
            file_id,
            started_unix_ms: now,
            total_bytes,
            acknowledged: vec![false; total_bytes.div_ceil(UPLOAD_PART_BYTES as u64) as usize],
        })
    }
    pub fn resumable(&self, now: u64) -> bool {
        now.checked_sub(self.started_unix_ms)
            .is_some_and(|age| age < UPLOAD_RESUME_WINDOW_MS)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = b"TARKUP01".to_vec();
        bytes.extend_from_slice(&self.file_id.to_le_bytes());
        bytes.extend_from_slice(&self.started_unix_ms.to_le_bytes());
        bytes.extend_from_slice(&self.total_bytes.to_le_bytes());
        bytes.extend(self.acknowledged.iter().map(|&v| u8::from(v)));
        bytes.extend_from_slice(blake3::hash(&bytes).as_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8], total: u64) -> Option<Self> {
        if total == 0 || total > MAX_TRANSFER_OBJECT_BYTES as u64 {
            return None;
        }
        let count = total.div_ceil(UPLOAD_PART_BYTES as u64) as usize;
        if bytes.len() != 64 + count
            || &bytes[..8] != b"TARKUP01"
            || blake3::hash(&bytes[..32 + count]).as_bytes() != &bytes[32 + count..]
        {
            return None;
        }
        let file_id = i64::from_le_bytes(bytes[8..16].try_into().ok()?);
        let started_unix_ms = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
        let total_bytes = u64::from_le_bytes(bytes[24..32].try_into().ok()?);
        if file_id == 0 || total != total_bytes || bytes[32..32 + count].iter().any(|&b| b > 1) {
            return None;
        }
        Some(Self {
            file_id,
            started_unix_ms,
            total_bytes,
            acknowledged: bytes[32..32 + count].iter().map(|&b| b == 1).collect(),
        })
    }
    /// Atomic replacement; call only on a blocking owner.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let temporary = path.with_extension("pending");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&self.encode())?;
        file.sync_all()?;
        std::fs::rename(temporary, path)
    }
}

pub struct StreamUploadOptions {
    pub observer: Option<Arc<dyn ByteTransferObserver>>,
    pub random_id: i64,
    pub tuning: TransferTuning,
}

impl TelegramConnection {
    pub async fn upload_stream_document(
        &self,
        chat: &TelegramChat,
        mut stream: UploadStream,
        name: &str,
        caption: &str,
        options: StreamUploadOptions,
    ) -> Result<SentDocument, TelegramError> {
        if !options.tuning.validate()
            || options.random_id == 0
            || name.is_empty()
            || caption.is_empty()
            || stream.total_bytes != stream.checkpoint.total_bytes
        {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        let count = stream.total_bytes.div_ceil(UPLOAD_PART_BYTES as u64) as usize;
        if count == 0
            || stream.total_bytes > MAX_TRANSFER_OBJECT_BYTES as u64
            || stream.checkpoint.file_id == 0
            || stream.checkpoint.acknowledged.len() != count
        {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let clients = self
            .transfer_clients(true, options.tuning.upload_connections)
            .await;
        let file_id = stream.checkpoint.file_id;
        let attempts = options.tuning.upload_attempts;
        save_parts(
            &mut stream,
            options.tuning.upload_parts,
            options.observer.as_ref(),
            |index, bytes| {
                let client = clients[index % clients.len()].clone();
                let budget = self.bandwidth.upload.clone();
                let flood_gate = self.upload_flood_gate.clone();

                let observer = options.observer.clone();
                async move {
                    let request = tl::functions::upload::SaveBigFilePart {
                        file_id,
                        file_part: index as i32,
                        file_total_parts: count as i32,
                        bytes,
                    };
                    for attempt in 1..=attempts {
                        flood_gate.wait().await;
                        budget.acquire(request.bytes.len()).await;
                        if let Some(observer) = &observer {
                            observer.observe(ByteTransferEvent::PartStarted {
                                index: index as u32,
                                attempt,
                            });
                        }
                        let result =
                            tokio::time::timeout(Duration::from_secs(60), client.invoke(&request))
                                .await;
                        let error = match result {
                            Ok(Ok(true)) => return Ok((index, request.bytes)),
                            Ok(Ok(false)) | Err(_) => {
                                TelegramError::new(TelegramErrorKind::Network)
                            }
                            Ok(Err(error)) => map_invocation(error),
                        };
                        if let Some(delay) = error.retry_after() {
                            flood_gate.extend(delay);
                        }
                        if attempt == attempts
                            || !matches!(
                                error.kind(),
                                TelegramErrorKind::Network
                                    | TelegramErrorKind::Server
                                    | TelegramErrorKind::FloodWait
                            )
                        {
                            return Err(error);
                        }
                        let delay = error
                            .retry_after()
                            .unwrap_or(Duration::from_millis(250 * (1u64 << (attempt - 1))));
                        if let Some(observer) = &observer {
                            observer.observe(ByteTransferEvent::PartRetry {
                                index: index as u32,
                                attempt,
                                wait_millis: delay.as_millis().min(u64::MAX as u128) as u64,
                            });
                        }
                        tokio::time::sleep(delay).await;
                    }
                    Err(TelegramError::new(TelegramErrorKind::Network))
                }
            },
        )
        .await?;
        // The producer must close only after source validation and durable spool sealing.
        if stream.blocks.recv().await.is_some() {
            return Err(TelegramError::new(TelegramErrorKind::SourceMissing));
        }
        if stream.sealed.await != Ok(true) {
            return Err(TelegramError::new(TelegramErrorKind::SourceMissing));
        }
        if let Some(observer) = &options.observer {
            observer.observe(ByteTransferEvent::SendingMessage);
        }
        let request = publication::document_request(
            chat.peer_ref.into(),
            tl::types::InputFileBig {
                id: stream.checkpoint.file_id,
                parts: count as i32,
                name: name.into(),
            }
            .into(),
            name,
            caption,
            options.random_id,
        );
        let updates = match self.sync_client.invoke(&request).await {
            Ok(updates) => updates,
            Err(error) if missing_temporary_parts(&error) => {
                // Persist a new temporary identity before requesting a bounded
                // replay from the Runtime's immutable ciphertext spool.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| TelegramError::new(TelegramErrorKind::Session))?
                    .as_millis()
                    .min(u64::MAX as u128) as u64;
                let replacement = UploadCheckpoint::new(stream.total_bytes, now)?;
                let path = stream.checkpoint_path.clone();
                tokio::task::spawn_blocking(move || replacement.save(&path))
                    .await
                    .map_err(|_| TelegramError::new(TelegramErrorKind::Session))?
                    .map_err(map_io)?;
                return Err(TelegramError::new(TelegramErrorKind::Network));
            }
            Err(error) => return Err(map_invocation(error)),
        };
        let message_id = publication::sent_message_id(updates, options.random_id)
            .filter(|id| *id > 0)
            .ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))?;
        Ok(SentDocument {
            message_id: i64::from(message_id),
        })
    }
}

fn missing_temporary_parts(error: &InvocationError) -> bool {
    matches!(error,InvocationError::Rpc(rpc) if rpc.code==400 &&
        (matches!(rpc.name.as_str(),"FILE_PART_MISSING"|"FILE_PARTS_INVALID"|"FILE_ID_INVALID") ||
            rpc.name.starts_with("FILE_PART_") && rpc.name.ends_with("_MISSING")))
}

async fn save_parts<F>(
    stream: &mut UploadStream,
    parallel: u16,
    observer: Option<&Arc<dyn ByteTransferObserver>>,
    upload: impl Fn(usize, Vec<u8>) -> F,
) -> Result<(), TelegramError>
where
    F: std::future::Future<Output = Result<(usize, Vec<u8>), TelegramError>> + Send + 'static,
{
    let count = stream.total_bytes.div_ceil(UPLOAD_PART_BYTES as u64) as usize;
    let mut inflight: JoinSet<Result<(usize, Vec<u8>), TelegramError>> = JoinSet::new();
    let mut received = 0usize;
    let mut acknowledged_bytes = stream
        .checkpoint
        .acknowledged
        .iter()
        .enumerate()
        .filter(|(_, done)| **done)
        .map(|(index, _)| {
            (stream.total_bytes - index as u64 * UPLOAD_PART_BYTES as u64)
                .min(UPLOAD_PART_BYTES as u64)
        })
        .sum::<u64>();
    let mut dirty = 0;
    loop {
        if received == count && inflight.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            result = inflight.join_next(), if !inflight.is_empty() => {
                let (index,bytes) = result.ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))?
                    .map_err(|_| TelegramError::new(TelegramErrorKind::Network))??;
                stream.checkpoint.acknowledged[index] = true;
                acknowledged_bytes += bytes.len() as u64;
                if let Some(observer) = observer { observer.observe(ByteTransferEvent::Uploading {bytes:acknowledged_bytes,total:stream.total_bytes}); }
                let _ = stream.recycled.try_send(bytes);
                dirty += 1;
                if dirty >= 8 || acknowledged_bytes == stream.total_bytes {
                    let checkpoint = stream.checkpoint.clone(); let path = stream.checkpoint_path.clone();
                    tokio::task::spawn_blocking(move || checkpoint.save(&path)).await
                        .map_err(|_| TelegramError::new(TelegramErrorKind::Session))?.map_err(map_io)?;
                    dirty = 0;
                }
            }
            bytes = stream.blocks.recv(), if received < count && inflight.len() < usize::from(parallel) => {
                let bytes = bytes.ok_or_else(|| TelegramError::new(TelegramErrorKind::SourceMissing))?;
                let index = received; received += 1;
                let expected = (stream.total_bytes - index as u64 * UPLOAD_PART_BYTES as u64).min(UPLOAD_PART_BYTES as u64) as usize;
                if bytes.len() != expected { return Err(TelegramError::new(TelegramErrorKind::SourceMissing)); }
                if stream.checkpoint.acknowledged[index] { let _ = stream.recycled.try_send(bytes); continue; }
                inflight.spawn(upload(index,bytes));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn ten_parts_run_concurrently_and_checkpoint_skips_confirmed_parts() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for acknowledged in [0, 3] {
            let total = 20usize;
            let directory = tempfile::tempdir().expect("spool");
            let (sender, blocks) = mpsc::channel(2);
            let (recycled, _) = mpsc::channel(12);
            let (_, sealed) = tokio::sync::oneshot::channel();
            let mut checkpoint =
                UploadCheckpoint::new((total * UPLOAD_PART_BYTES) as u64, 100).expect("checkpoint");
            for index in 0..acknowledged {
                checkpoint.acknowledged[index] = true;
            }
            let mut stream = UploadStream {
                total_bytes: checkpoint.total_bytes,
                blocks,
                recycled,
                sealed,
                checkpoint,
                checkpoint_path: directory.path().join("upload"),
            };
            let producer = tokio::spawn(async move {
                for _ in 0..total {
                    sender
                        .send(vec![9; UPLOAD_PART_BYTES])
                        .await
                        .expect("consumer");
                }
            });
            let active = Arc::new(AtomicUsize::new(0));
            let maximum = Arc::new(AtomicUsize::new(0));
            let calls = Arc::new(AtomicUsize::new(0));
            let start = Arc::new(tokio::sync::Barrier::new(10));
            save_parts(&mut stream, 10, None, |index, bytes| {
                assert!(index >= acknowledged);
                let active = active.clone();
                let maximum = maximum.clone();
                let calls = calls.clone();
                let start = start.clone();
                async move {
                    let ordinal = calls.fetch_add(1, Ordering::SeqCst);
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    if ordinal < 10 {
                        tokio::time::timeout(Duration::from_secs(2), start.wait())
                            .await
                            .expect("all ten RPCs must enter before any finishes");
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok((index, bytes))
                }
            })
            .await
            .expect("acknowledged");
            producer.await.expect("producer");
            assert_eq!(maximum.load(Ordering::SeqCst), 10);
            assert_eq!(calls.load(Ordering::SeqCst), total - acknowledged);
            let saved = UploadCheckpoint::decode(
                &std::fs::read(&stream.checkpoint_path).expect("saved"),
                stream.total_bytes,
            )
            .expect("valid");
            assert!(saved.acknowledged.iter().all(|done| *done));
        }
    }

    #[test]
    fn checkpoint_window_codec_and_corruption() {
        let mut checkpoint =
            UploadCheckpoint::new(3 * UPLOAD_PART_BYTES as u64, 100).expect("identity");
        checkpoint.acknowledged[1] = true;
        assert!(checkpoint.resumable(100 + UPLOAD_RESUME_WINDOW_MS - 1));
        assert!(!checkpoint.resumable(100 + UPLOAD_RESUME_WINDOW_MS));
        assert!(!checkpoint.resumable(99));
        let encoded = checkpoint.encode();
        assert_eq!(
            UploadCheckpoint::decode(&encoded, checkpoint.total_bytes),
            Some(checkpoint.clone())
        );
        for index in 0..encoded.len() {
            let mut corrupt = encoded.clone();
            corrupt[index] ^= 1;
            assert!(UploadCheckpoint::decode(&corrupt, checkpoint.total_bytes).is_none());
        }
    }
}
