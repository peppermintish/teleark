//! Instance-owned fake wire replies for complete Vault service tests. Release
//! builds contain neither this module nor the request interception field. Vault
//! workers, ciphertext, source checks, TelegramObjectStore mapping and SQL stay real.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GateKind {
    Validation,
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
    fail_once: Option<GateKind>,
}
pub(crate) struct TestVaultRemote {
    state: Mutex<State>,
    gate: Mutex<Option<Gate>>,
}
impl TestVaultRemote {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
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
    fn cross_gate(&self, kind: GateKind) -> Result<(), ApplicationError> {
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
        self.state.lock().expect("state").objects.len()
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
                    let random = publication_random_id
                        .expect("durable runtime must supply publication identity");
                    assert_ne!(random, 0);
                    assert!(bytes.len() <= 64 * 1024 * 1024, "bounded encrypted object");
                    {
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
                    let mut state = self.state.lock().expect("state");
                    if let Some(id) = state.publications.get(&random).copied() {
                        let object = state.objects.get(&id).expect("stable publication");
                        assert_eq!(object.bytes, bytes);
                        assert_eq!(object.summary.file_name, file_name);
                        assert_eq!(object.summary.caption, caption);
                        return Ok(id);
                    }
                    assert!(state.objects.len() < 32);
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
                    state.publications.insert(random, id);
                    drop(state);
                    // The remote object exists, but no successful reply or
                    // verified local receipt has reached the Vault owner yet.
                    self.cross_gate(acknowledgment)?;
                    Self::scope(account_id, chat_id, cancellation.as_ref())?;
                    self.fail_if_armed(acknowledgment)?;
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
