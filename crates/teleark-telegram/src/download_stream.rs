//! Ordered, bounded ciphertext delivery; no filesystem or cryptography on the reactor.
use super::*;
use std::{collections::BTreeMap, sync::Mutex};

impl TelegramConnection {
    pub async fn download_stream_observed(
        &self,
        file: &TelegramFile,
        expected: u64,
        output: tokio::sync::mpsc::Sender<Vec<u8>>,
        observer: Option<Arc<dyn ByteTransferObserver>>,
    ) -> Result<(), TelegramError> {
        if expected == 0 || expected > MAX_STREAM_OBJECT_BYTES || file.size_bytes != expected {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let tuning = observer.as_deref().map_or_else(
            TransferTuning::default,
            ByteTransferObserver::transfer_tuning,
        );
        if !tuning.validate() {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        let clients = self
            .transfer_clients(false, tuning.download_connections)
            .await;
        let count = expected.div_ceil(UPLOAD_PART_BYTES as u64);
        let window = u64::from(tuning.download_parts);
        let mut next = 0;
        let mut emitted = 0;
        let receipts = Arc::new(StreamReceipts {
            state: Mutex::new((0, BTreeMap::new())),
            observer,
            expected,
        });
        let mut ready = BTreeMap::new();
        let mut inflight = JoinSet::new();
        while emitted < count {
            // Outstanding requests plus reordered results never exceed one window.
            while next < count && next < emitted + window {
                let index = next;
                next += 1;
                let offset = index * UPLOAD_PART_BYTES as u64;
                let length = (expected - offset).min(UPLOAD_PART_BYTES as u64);
                let client = clients[index as usize % clients.len()].clone();
                let document = file.document.clone();
                let gate = self.download_flood_gate.clone();
                let bandwidth = self.bandwidth.download.clone();
                let receipts = receipts.clone();
                inflight.spawn(async move {
                    for attempt in 1..=u32::from(tuning.download_attempts) {
                        match download_logical_part(
                            client.clone(),
                            document.clone(),
                            index,
                            offset,
                            length,
                            gate.clone(),
                            bandwidth.clone(),
                            Some(receipts.clone()),
                        )
                        .await
                        {
                            Ok(part) => return Ok(part),
                            Err(error) => {
                                let Some(delay) = download_part_retry_delay(
                                    &error,
                                    attempt,
                                    u32::from(tuning.download_attempts),
                                ) else {
                                    return Err(error);
                                };
                                tokio::time::sleep(delay).await;
                            }
                        }
                    }
                    Err(TelegramError::new(TelegramErrorKind::Network))
                });
            }
            let part = inflight
                .join_next()
                .await
                .ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))?
                .map_err(|_| TelegramError::new(TelegramErrorKind::Network))??;
            receipts.completed(part.part_index);
            ready.insert(part.part_index, part.bytes);
            while let Some(bytes) = ready.remove(&emitted) {
                output
                    .send(bytes)
                    .await
                    .map_err(|_| TelegramError::new(TelegramErrorKind::Cancelled))?;
                emitted += 1;
            }
        }
        Ok(())
    }
}

struct StreamReceipts {
    state: Mutex<(u64, BTreeMap<u64, u64>)>,
    observer: Option<Arc<dyn ByteTransferObserver>>,
    expected: u64,
}
impl DownloadReceiptObserver for StreamReceipts {
    fn acknowledged(&self, part: u64, bytes: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let previous = state.1.entry(part).or_default();
        let delta = bytes.saturating_sub(*previous);
        *previous = (*previous).max(bytes);
        state.0 = state.0.saturating_add(delta);
        if delta > 0
            && let Some(observer) = &self.observer
        {
            // Keep cumulative callback ordering across concurrent RPC completions.
            observer.observe(ByteTransferEvent::Downloading {
                bytes: state.0,
                total: self.expected,
            });
        }
    }
    fn completed(&self, part: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.1.remove(&part);
        }
    }
}
