//! Instance-owned fake wire replies for complete Vault service tests. Release
//! builds contain neither this module nor the request interception field. Vault
//! workers, ciphertext, source checks, TelegramObjectStore mapping and SQL stay real.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GateKind {
    StorageDiscovery,
    Validation,
    PendingMetadataUpload,
    UploadPart,
    DownloadPart,
    ManifestDownload,
    ManifestUpload,
    PartAcknowledgment,
    ManifestAcknowledgment,
}
struct Gate {
    kind: GateKind,
    skip: usize,
    entered: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
}
#[derive(Clone)]
struct Object {
    summary: TelegramFileSummary,
    bytes: Vec<u8>,
}
#[derive(Default)]
struct State {
    objects: BTreeMap<i64, Object>,
    publications: BTreeMap<i64, i64>,
    uploads: Vec<(i64, [u8; 32])>,
    downloads: Vec<i64>,
    validations: Vec<bool>,
    storage_created: bool,
    fail_once: Option<GateKind>,
}
struct ConcurrentGate {
    kind: GateKind,
    remaining: usize,
    entered: mpsc::SyncSender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}
pub(crate) struct TestVaultRemote {
    concurrent_gate: Mutex<Option<ConcurrentGate>>,
    state: Mutex<State>,
    gate: Mutex<Option<Gate>>,
}
impl TestVaultRemote {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            concurrent_gate: Mutex::new(None),
            state: Mutex::new(State::default()),
            gate: Mutex::new(None),
        })
    }
    pub(crate) fn connect(self: &Arc<Self>, path: &Path) -> DesktopTelegram {
        let mut telegram = DesktopTelegram::open_direct(path).expect("synthetic Telegram owner");
        telegram.test_vault_remote = Some(self.clone());
        telegram.lifecycle().publish(1, Some(7), None);
        telegram
    }
    pub(crate) fn gate(
        &self,
        kind: GateKind,
        skip: usize,
    ) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::channel();
        let previous = self.gate.lock().expect("gate").replace(Gate {
            kind,
            skip,
            entered,
            release: wait,
        });
        assert!(previous.is_none(), "one bounded test gate");
        (ready, release)
    }
    pub(crate) fn concurrent_upload_gate(
        &self,
        count: usize,
    ) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        self.concurrent_gate(GateKind::UploadPart, count)
    }
    pub(crate) fn concurrent_pending_metadata_gate(
        &self,
        count: usize,
    ) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        self.concurrent_gate(GateKind::PendingMetadataUpload, count)
    }
    fn concurrent_gate(
        &self,
        kind: GateKind,
        count: usize,
    ) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        assert!(count > 0, "concurrent gate needs an entrant");
        let (entered, ready) = mpsc::sync_channel(count);
        let (release, wait) = mpsc::channel();
        let previous = self
            .concurrent_gate
            .lock()
            .expect("gate")
            .replace(ConcurrentGate {
                kind,
                remaining: count,
                entered,
                release: Arc::new(Mutex::new(wait)),
            });
        assert!(
            previous.is_none_or(|gate| gate.remaining == 0),
            "one active concurrent gate"
        );
        (ready, release)
    }
    fn cross_gate(&self, kind: GateKind) -> Result<(), ApplicationError> {
        let concurrent = {
            let mut gate = self.concurrent_gate.lock().expect("gate");
            gate.as_mut()
                .filter(|gate| gate.kind == kind && gate.remaining > 0)
                .map(|gate| {
                    gate.remaining -= 1;
                    (gate.entered.clone(), gate.release.clone())
                })
        };
        if let Some((entered, release)) = concurrent {
            entered.send(()).expect("test observer");
            release
                .lock()
                .expect("gate release")
                .recv_timeout(Duration::from_secs(30))
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))?;
        }
        let gate = {
            let mut slot = self.gate.lock().expect("gate");
            if let Some(gate) = slot.as_mut()
                && gate.kind == kind
            {
                if gate.skip > 0 {
                    gate.skip -= 1;
                    None
                } else {
                    slot.take()
                }
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            gate.entered
                .send(())
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))?;
            gate.release
                .recv_timeout(Duration::from_secs(30))
                .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))?;
        }
        Ok(())
    }
    fn scope(
        account: i64,
        chat: i64,
        cancel: Option<&TelegramScanCancellation>,
    ) -> Result<(), ApplicationError> {
        if account != 7 || chat != 11 {
            return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
        }
        if cancel.is_some_and(TelegramScanCancellation::is_cancelled) {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        Ok(())
    }
    pub(crate) fn objects(&self) -> usize {
        self.state
            .lock()
            .expect("state")
            .objects
            .values()
            .filter(|object| object.summary.caption != crate::vault::remote_upload::CAPTION)
            .count()
    }
    pub(crate) fn downloads(&self) -> Vec<i64> {
        self.state.lock().expect("state").downloads.clone()
    }
    pub(crate) fn summaries(&self) -> Vec<TelegramFileSummary> {
        self.state
            .lock()
            .expect("state")
            .objects
            .values()
            .map(|object| object.summary.clone())
            .collect()
    }
    pub(crate) fn uploads(&self) -> Vec<(i64, [u8; 32])> {
        self.state.lock().expect("state").uploads.clone()
    }

    pub(crate) fn fail_once(&self, kind: GateKind) {
        assert!(
            self.state
                .lock()
                .expect("state")
                .fail_once
                .replace(kind)
                .is_none()
        );
    }
    fn fail_if_armed(&self, kind: GateKind) -> Result<(), ApplicationError> {
        let mut state = self.state.lock().expect("state");
        if state.fail_once == Some(kind) {
            state.fail_once = None;
            Err(ApplicationError::new(ApplicationErrorKind::Network))
        } else {
            Ok(())
        }
    }

    pub(super) fn handle(&self, request: TelegramRequest) {
        match request {
            TelegramRequest::DiscoverStorage {
                account_id,
                preferred,
                create,
                progress,
                reply,
            } => {
                let result = (|| {
                    if account_id != 7 {
                        return Err(ApplicationError::new(ApplicationErrorKind::Authorization));
                    }
                    if let Some(progress) = &progress {
                        progress.phase(crate::StorageSetupPhase::ReadingDialogs);
                    }
                    self.cross_gate(GateKind::StorageDiscovery)?;
                    let mut state = self.state.lock().expect("state");
                    let created = !state.storage_created && create.is_some();
                    if created {
                        if let Some(progress) = &progress {
                            progress.phase(crate::StorageSetupPhase::CreatingChannel);
                        }
                        state.storage_created = true;
                    }
                    let status = if state.storage_created && preferred.is_none_or(|id| id == 11) {
                        crate::StorageChannelStatus::Ready(TelegramChatSummary {
                            id: 11,
                            name: "Synthetic private storage".into(),
                            username: None,
                            kind: TelegramChatKind::Channel,
                            sync_pts: None,
                        })
                    } else {
                        crate::StorageChannelStatus::Missing
                    };
                    Ok((status, created))
                })();
                let _ = reply.send(result);
            }
            TelegramRequest::ValidateStorage {
                account_id,
                chat_id,
                discover,
                reply,
            } => {
                let result = (|| {
                    Self::scope(account_id, chat_id, None)?;
                    {
                        let mut state = self.state.lock().expect("state");
                        assert!(state.validations.len() < 128);
                        state.validations.push(discover);
                    }
                    self.cross_gate(GateKind::Validation)?;
                    Ok(TelegramChatSummary {
                        id: 11,
                        name: "Synthetic private storage".into(),
                        username: None,
                        kind: TelegramChatKind::Channel,
                        sync_pts: None,
                    })
                })();
                let _ = reply.send(result);
            }
            TelegramRequest::SearchFiles {
                account_id,
                chat_id,
                caption,
                limit,
                cancellation,
                reply,
            } => {
                let result = Self::scope(account_id, chat_id, cancellation.as_ref()).map(|()| {
                    self.state
                        .lock()
                        .expect("state")
                        .objects
                        .values()
                        .filter(|object| object.summary.caption == caption)
                        .take(limit)
                        .map(|object| object.summary.clone())
                        .collect()
                });
                let _ = reply.send(result);
            }
            TelegramRequest::UploadStream {
                account_id,
                chat_id,
                file_name,
                caption,
                mut stream,
                options,
                cancellation,
                reply,
            } => {
                let mut bytes = Vec::new();
                while let Some(block) = stream.blocks.blocking_recv() {
                    bytes.extend_from_slice(&block);
                    let _ = stream.recycled.try_send(block);
                }
                if stream.sealed.blocking_recv() != Ok(true) {
                    let _ = reply.send(Err(ApplicationError::new(
                        ApplicationErrorKind::SourceChanged,
                    )));
                    return;
                }
                self.handle(TelegramRequest::UploadBytes {
                    account_id,
                    chat_id,
                    file_name,
                    caption,
                    bytes,
                    publication_random_id: Some(options.random_id),
                    observer: options.observer,
                    cancellation,
                    reply,
                });
            }
            TelegramRequest::UploadBytes {
                account_id,
                chat_id,
                file_name,
                caption,
                bytes,
                publication_random_id,
                observer,
                cancellation,
                reply,
            } => {
                let result = (|| {
                    Self::scope(account_id, chat_id, cancellation.as_ref())?;
                    let pending_metadata = caption == crate::vault::remote_upload::CAPTION;
                    if publication_random_id.is_none() {
                        assert!(
                            pending_metadata,
                            "durable payloads need a publication identity"
                        );
                        // The production unkeyed send creates a fresh Telegram message.
                        // Only caller-reserved IDs participate in retry deduplication.
                    }
                    if let Some(random) = publication_random_id {
                        assert_ne!(random, 0);
                    }
                    assert!(bytes.len() <= 64 * 1024 * 1024, "bounded encrypted object");
                    if !pending_metadata {
                        let random = publication_random_id
                            .expect("durable payloads have a publication identity");
                        let mut state = self.state.lock().expect("state");
                        assert!(state.uploads.len() < 64);
                        state
                            .uploads
                            .push((random, *blake3::hash(&bytes).as_bytes()));
                    }
                    // A real byte callback at a controlled simulated wire boundary;
                    // these bytes are not yet a committed message or local receipt.
                    if let Some(observer) = &observer {
                        observer.observe(teleark_telegram::ByteTransferEvent::Uploading {
                            bytes: bytes.len() as u64 / 2,
                            total: bytes.len() as u64,
                        });
                    }
                    if pending_metadata {
                        // Wait before taking `state`; tests must never hold the remote
                        // mutex while coordinating concurrent publications.
                        self.cross_gate(GateKind::PendingMetadataUpload)?;
                    } else {
                        self.cross_gate(if caption == crate::transfer::MANIFEST_CAPTION {
                            GateKind::ManifestUpload
                        } else {
                            GateKind::UploadPart
                        })?;
                        Self::scope(account_id, chat_id, cancellation.as_ref())?;
                        self.fail_if_armed(if caption == crate::transfer::MANIFEST_CAPTION {
                            GateKind::ManifestUpload
                        } else {
                            GateKind::UploadPart
                        })?;
                    }
                    let mut state = self.state.lock().expect("state");
                    if let Some(random) = publication_random_id
                        && let Some(id) = state.publications.get(&random).copied()
                    {
                        let object = state.objects.get(&id).expect("stable publication");
                        assert_eq!(object.bytes, bytes);
                        assert_eq!(object.summary.file_name, file_name);
                        assert_eq!(object.summary.caption, caption);
                        return Ok(id);
                    }
                    assert!(state.objects.len() < 256);
                    assert!(
                        state
                            .objects
                            .values()
                            .map(|object| object.bytes.len())
                            .sum::<usize>()
                            + bytes.len()
                            <= 128 * 1024 * 1024
                    );
                    let acknowledgment = if caption == crate::transfer::MANIFEST_CAPTION {
                        GateKind::ManifestAcknowledgment
                    } else {
                        GateKind::PartAcknowledgment
                    };
                    let id = state.objects.len() as i64 + 1;
                    let size = bytes.len() as u64;
                    state.objects.insert(
                        id,
                        Object {
                            summary: TelegramFileSummary {
                                message_id: id,
                                sent_at_unix_ms: 100,
                                modified_at_unix_ms: 100,
                                file_name,
                                caption,
                                mime_type: None,
                                size_bytes: size,
                            },
                            bytes,
                        },
                    );
                    if let Some(random) = publication_random_id {
                        state.publications.insert(random, id);
                    }
                    drop(state);
                    // The remote object exists, but no successful reply or
                    // verified local receipt has reached the Vault owner yet.
                    if !pending_metadata {
                        self.cross_gate(acknowledgment)?;
                        Self::scope(account_id, chat_id, cancellation.as_ref())?;
                        self.fail_if_armed(acknowledgment)?;
                    }
                    if let Some(observer) = &observer {
                        observer.observe(teleark_telegram::ByteTransferEvent::Uploading {
                            bytes: size,
                            total: size,
                        });
                    }
                    Ok(id)
                })();
                let _ = reply.send(result);
            }
            TelegramRequest::DownloadStream {
                account_id,
                chat_id,
                message_id,
                expected,
                blocks,
                observer,
                cancellation,
                abort,
                reply,
            } => {
                let result = (|| {
                    Self::scope(account_id, chat_id, cancellation.as_ref())?;
                    let object = {
                        let mut state = self.state.lock().expect("state");
                        state.downloads.push(message_id);
                        state.objects.get(&message_id).cloned().ok_or_else(|| {
                            ApplicationError::new(ApplicationErrorKind::SourceMissing)
                        })?
                    };
                    assert_eq!(object.bytes.len() as u64, expected);
                    if let Some(observer) = &observer {
                        observer.observe(teleark_telegram::ByteTransferEvent::Downloading {
                            bytes: expected / 2,
                            total: expected,
                        });
                    }
                    self.cross_gate(GateKind::DownloadPart)?;
                    self.fail_if_armed(GateKind::DownloadPart)?;
                    Self::scope(account_id, chat_id, cancellation.as_ref())?;
                    for block in object.bytes.chunks(teleark_telegram::UPLOAD_PART_BYTES) {
                        if abort.is_cancelled() {
                            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
                        }
                        blocks
                            .blocking_send(block.to_vec())
                            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Cancelled))?;
                    }
                    if let Some(observer) = &observer {
                        observer.observe(teleark_telegram::ByteTransferEvent::Downloading {
                            bytes: expected,
                            total: expected,
                        });
                    }
                    Ok(())
                })();
                let _ = reply.send(result);
            }
            TelegramRequest::DownloadBytes {
                account_id,
                chat_id,
                message_id,
                cancellation,
                observer,
                reply,
            } => {
                let result = (|| {
                    Self::scope(account_id, chat_id, cancellation.as_ref())?;
                    let object = {
                        let mut state = self.state.lock().expect("state");
                        assert!(state.downloads.len() < 128);
                        state.downloads.push(message_id);
                        state.objects.get(&message_id).cloned().ok_or_else(|| {
                            ApplicationError::new(ApplicationErrorKind::SourceMissing)
                        })?
                    };
                    if let Some(observer) = &observer {
                        observer.observe(teleark_telegram::ByteTransferEvent::Downloading {
                            bytes: object.bytes.len() as u64 / 2,
                            total: object.bytes.len() as u64,
                        });
                    }
                    if object.summary.caption != crate::transfer::MANIFEST_CAPTION {
                        self.cross_gate(GateKind::DownloadPart)?;
                        self.fail_if_armed(GateKind::DownloadPart)?;
                    } else {
                        self.cross_gate(GateKind::ManifestDownload)?;
                        self.fail_if_armed(GateKind::ManifestDownload)?;
                    }
                    Self::scope(account_id, chat_id, cancellation.as_ref())?;
                    if let Some(observer) = &observer {
                        observer.observe(teleark_telegram::ByteTransferEvent::Downloading {
                            bytes: object.bytes.len() as u64,
                            total: object.bytes.len() as u64,
                        });
                    }
                    Ok(object.bytes)
                })();
                let _ = reply.send(result);
            }
            _ => panic!("unsupported request at synthetic Vault wire boundary"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish_pending_metadata(
        remote: Arc<TestVaultRemote>,
        file_name: &'static str,
        bytes: Vec<u8>,
    ) -> std::thread::JoinHandle<i64> {
        std::thread::spawn(move || {
            let (reply, response) = mpsc::sync_channel(1);
            remote.handle(TelegramRequest::UploadBytes {
                account_id: 7,
                chat_id: 11,
                file_name: file_name.into(),
                caption: crate::vault::remote_upload::CAPTION.into(),
                bytes,
                publication_random_id: None,
                observer: None,
                cancellation: None,
                reply,
            });
            response
                .recv_timeout(Duration::from_secs(10))
                .expect("pending metadata reply")
                .expect("pending metadata publication")
        })
    }

    #[test]
    fn concurrent_unkeyed_metadata_publications_remain_distinct() {
        let remote = TestVaultRemote::new();
        let (entered, release) = remote.concurrent_pending_metadata_gate(2);
        let first = publish_pending_metadata(remote.clone(), "first.tarku", vec![1]);
        let second = publish_pending_metadata(remote.clone(), "second.tarku", vec![2]);

        for _ in 0..2 {
            entered
                .recv_timeout(Duration::from_secs(10))
                .expect("both publications overlap before insertion");
        }
        for _ in 0..2 {
            release.send(()).expect("release publication");
        }

        let first = first.join().expect("first publisher");
        let second = second.join().expect("second publisher");
        assert_ne!(first, second);
        let state = remote.state.lock().expect("state");
        assert_eq!(state.objects.len(), 2);
        assert!(state.publications.is_empty());
        assert_eq!(state.objects.get(&first).expect("first object").bytes, [1]);
        assert_eq!(
            state.objects.get(&second).expect("second object").bytes,
            [2]
        );
    }
}
