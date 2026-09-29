//! Ordered, bounded ciphertext delivery; no filesystem or cryptography on the reactor.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

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
        let count = expected.div_ceil(DOWNLOAD_PART_SIZE_BYTES);
        let window = u64::from(tuning.download_parts);
        let mut next = 0;
        let mut emitted = 0;
        let receipts = Arc::new(StreamReceipts {
            state: Mutex::new((0, BTreeMap::new())),
            observer,
            expected,
        });
        let attempts = Arc::new(Mutex::new(DownloadStreamAttempts::default()));
        let publication = Arc::new(Mutex::new(()));
        let mut ready = BTreeMap::new();
        let mut inflight = JoinSet::new();
        while emitted < count {
            // Outstanding requests plus reordered results never exceed one window.
            while next < count && next < emitted + window {
                let index = next;
                next += 1;
                let (offset, length) = stream_download_part_range(index, expected)
                    .ok_or_else(|| TelegramError::new(TelegramErrorKind::LimitExceeded))?;
                let client = clients[index as usize % clients.len()].clone();
                let document = file.document.clone();
                let gate = self.download_flood_gate.clone();
                let bandwidth = self.bandwidth.download.clone();
                let receipts = receipts.clone();
                let attempts = attempts.clone();
                let publication = publication.clone();
                let activity_observer = receipts.observer.clone();
                let activity_sink = activity_observer.as_ref().map(|observer| {
                    let observer = observer.clone();
                    let attempts = attempts.clone();
                    let publication = publication.clone();
                    Arc::new(move |update| {
                        publish_attempt_event(&attempts, &publication, &observer, update);
                    }) as DownloadChunkActivitySink
                });
                inflight.spawn(async move {
                    let mut attempt = 1;
                    while attempt <= u32::from(tuning.download_attempts) {
                        let result = download_logical_part(
                            client.clone(),
                            document.clone(),
                            index,
                            offset,
                            length,
                            gate.clone(),
                            bandwidth.clone(),
                            Some(receipts.clone()),
                            activity_sink.clone(),
                            attempt as u16,
                        )
                        .await;
                        match result {
                            Ok(part) => {
                                publish_attempt_update(
                                    &activity_sink,
                                    DownloadChunkActivityUpdate::Finished {
                                        index,
                                        now: Instant::now(),
                                    },
                                );
                                return Ok(part);
                            }
                            Err(error) => {
                                let is_flood = error.kind() == TelegramErrorKind::FloodWait;
                                if is_flood && let Some(delay) = error.retry_after() {
                                    receipts.retrying(index);
                                    gate.extend_with_status(
                                        delay,
                                        error.server_code().unwrap_or(420),
                                        error.server_message().unwrap_or("FLOOD_WAIT"),
                                    );
                                    if let Some(status) = gate.status() {
                                        let gate_wait = gate.remaining();
                                        publish_attempt_update(
                                            &activity_sink,
                                            DownloadChunkActivityUpdate::FloodWaitRetryScheduled {
                                                index,
                                                attempt: attempt as u16,
                                                delay: gate_wait,
                                                code: status.code,
                                                wait_until_unix_ms: status.wait_until_unix_ms,
                                                now: Instant::now(),
                                            },
                                        );
                                    } else {
                                        publish_attempt_update(
                                            &activity_sink,
                                            DownloadChunkActivityUpdate::RetryScheduled {
                                                index,
                                                attempt: attempt as u16,
                                                delay,
                                                now: Instant::now(),
                                            },
                                        );
                                    }
                                    tokio::time::sleep(delay).await;
                                    continue;
                                }
                                let Some(delay) = download_part_retry_delay(
                                    &error,
                                    attempt,
                                    u32::from(tuning.download_attempts),
                                ) else {
                                    publish_attempt_update(
                                        &activity_sink,
                                        DownloadChunkActivityUpdate::Finished {
                                            index,
                                            now: Instant::now(),
                                        },
                                    );
                                    return Err(error);
                                };
                                receipts.retrying(index);
                                attempt += 1;
                                publish_attempt_update(
                                    &activity_sink,
                                    DownloadChunkActivityUpdate::RetryScheduled {
                                        index,
                                        attempt: attempt as u16,
                                        delay,
                                        now: Instant::now(),
                                    },
                                );
                                tokio::time::sleep(delay).await;
                            }
                        }
                    }
                    publish_attempt_update(
                        &activity_sink,
                        DownloadChunkActivityUpdate::Finished {
                            index,
                            now: Instant::now(),
                        },
                    );
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

#[derive(Default)]
struct DownloadStreamAttempts {
    active: u16,
    active_indices: BTreeSet<u64>,
    waiting: BTreeMap<u64, Instant>,
}

impl DownloadStreamAttempts {
    fn started(&mut self, index: u64, attempt: u16, now: Instant) -> ByteTransferEvent {
        self.waiting.remove(&index);
        if self.active_indices.insert(index) {
            self.active = self.active.saturating_add(1);
        }
        let (waiting_chunks, next_wait_millis) = self.wait_summary(now);
        ByteTransferEvent::DownloadChunkStarted {
            index,
            attempt,
            active_chunks: self.active,
            waiting_chunks,
            next_wait_millis,
        }
    }

    fn retry_scheduled(
        &mut self,
        index: u64,
        attempt: u16,
        delay: Duration,
        now: Instant,
    ) -> ByteTransferEvent {
        if self.active_indices.remove(&index) {
            self.active = self.active.saturating_sub(1);
        }
        self.waiting
            .insert(index, now.checked_add(delay).unwrap_or(now));
        let (waiting_chunks, next_wait_millis) = self.wait_summary(now);
        ByteTransferEvent::DownloadChunkRetry {
            index,
            attempt,
            wait_millis: delay.as_millis().min(u64::MAX as u128) as u64,
            active_chunks: self.active,
            waiting_chunks,
            next_wait_millis,
        }
    }

    fn waiting_for_gate(
        &mut self,
        index: u64,
        attempt: u16,
        delay: Duration,
        server_code: Option<i32>,
        server_wait_until_unix_ms: Option<i64>,
        now: Instant,
    ) -> ByteTransferEvent {
        if self.active_indices.remove(&index) {
            self.active = self.active.saturating_sub(1);
        }
        self.waiting
            .insert(index, now.checked_add(delay).unwrap_or(now));
        let (waiting_chunks, next_wait_millis) = self.wait_summary(now);
        ByteTransferEvent::DownloadChunkWaiting {
            index,
            attempt,
            wait_millis: delay.as_millis().min(u64::MAX as u128) as u64,
            server_code,
            server_wait_until_unix_ms,
            active_chunks: self.active,
            waiting_chunks,
            next_wait_millis,
        }
    }

    fn finished(&mut self, index: u64, now: Instant) -> ByteTransferEvent {
        if self.active_indices.remove(&index) {
            self.active = self.active.saturating_sub(1);
        }
        self.waiting.remove(&index);
        let (waiting_chunks, next_wait_millis) = self.wait_summary(now);
        ByteTransferEvent::DownloadChunkFinished {
            index,
            active_chunks: self.active,
            waiting_chunks,
            next_wait_millis,
        }
    }

    fn wait_summary(&self, now: Instant) -> (u16, u64) {
        let waiting = u16::try_from(self.waiting.len()).unwrap_or(u16::MAX);
        let next_wait_millis = self
            .waiting
            .values()
            .map(|deadline| deadline.saturating_duration_since(now).as_millis())
            .min()
            .unwrap_or_default()
            .min(u128::from(u64::MAX)) as u64;
        (waiting, next_wait_millis)
    }
}

fn publish_attempt_event(
    attempts: &Arc<Mutex<DownloadStreamAttempts>>,
    publication: &Arc<Mutex<()>>,
    observer: &Arc<dyn ByteTransferObserver>,
    update: DownloadChunkActivityUpdate,
) {
    // Keep the bounded state transition and its callback in one order so a slow
    // observer cannot receive stale active/waiting counts after a newer update.
    let _publication = publication
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut state = attempts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let DownloadChunkActivityUpdate::FloodWaitRetryScheduled {
        index,
        attempt,
        delay,
        code,
        wait_until_unix_ms,
        now,
    } = update
    {
        let retry_event = state.retry_scheduled(index, attempt, delay, now);
        drop(state);
        observer.observe(ByteTransferEvent::DownloadServerThrottled {
            code,
            wait_until_unix_ms,
        });
        observer.observe(retry_event);
        return;
    }
    let event = match update {
        DownloadChunkActivityUpdate::Started {
            index,
            attempt,
            now,
        } => state.started(index, attempt, now),
        DownloadChunkActivityUpdate::RetryScheduled {
            index,
            attempt,
            delay,
            now,
        } => state.retry_scheduled(index, attempt, delay, now),
        DownloadChunkActivityUpdate::FloodWaitRetryScheduled { .. } => {
            unreachable!("FloodWait update was handled before the state match")
        }
        DownloadChunkActivityUpdate::WaitingForGate {
            index,
            attempt,
            delay,
            server_code,
            server_wait_until_unix_ms,
            now,
        } => state.waiting_for_gate(
            index,
            attempt,
            delay,
            server_code,
            server_wait_until_unix_ms,
            now,
        ),
        DownloadChunkActivityUpdate::Finished { index, now } => state.finished(index, now),
    };
    drop(state);
    observer.observe(event);
}

fn publish_attempt_update(
    activity: &Option<DownloadChunkActivitySink>,
    update: DownloadChunkActivityUpdate,
) {
    if let Some(activity) = activity {
        activity(update);
    }
}

fn stream_download_part_range(index: u64, expected: u64) -> Option<(u64, u64)> {
    let offset = index.checked_mul(DOWNLOAD_PART_SIZE_BYTES)?;
    let remaining = expected.checked_sub(offset)?;
    Some((offset, remaining.min(DOWNLOAD_PART_SIZE_BYTES)))
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

impl StreamReceipts {
    fn retrying(&self, part: u64) {
        if let Ok(mut state) = self.state.lock() {
            // Keep the lifetime network byte total but start the per-attempt
            // high-water mark over so retransmitted bytes are measured again.
            state.1.remove(&part);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver, SyncSender};

    #[test]
    fn download_chunk_activity_tracks_retry_and_shared_gate_waits() {
        let start = Instant::now();
        let mut state = DownloadStreamAttempts::default();

        let started = state.started(17, 1, start);
        assert!(matches!(
            started,
            ByteTransferEvent::DownloadChunkStarted {
                index: 17,
                active_chunks: 1,
                waiting_chunks: 0,
                ..
            }
        ));
        let retry = state.retry_scheduled(17, 2, Duration::from_millis(300), start);
        assert!(matches!(
            retry,
            ByteTransferEvent::DownloadChunkRetry {
                index: 17,
                attempt: 2,
                active_chunks: 0,
                waiting_chunks: 1,
                next_wait_millis: 300,
                ..
            }
        ));
        let restarted = state.started(17, 2, start + Duration::from_millis(300));
        assert!(matches!(
            restarted,
            ByteTransferEvent::DownloadChunkStarted {
                active_chunks: 1,
                waiting_chunks: 0,
                ..
            }
        ));
        let gated =
            state.waiting_for_gate(17, 2, Duration::from_secs(2), Some(420), Some(4_200), start);
        assert!(matches!(
            gated,
            ByteTransferEvent::DownloadChunkWaiting {
                index: 17,
                server_code: Some(420),
                server_wait_until_unix_ms: Some(4_200),
                active_chunks: 0,
                waiting_chunks: 1,
                next_wait_millis: 2000,
                ..
            }
        ));
        let resumed = state.started(17, 2, start + Duration::from_secs(2));
        assert!(matches!(
            resumed,
            ByteTransferEvent::DownloadChunkStarted {
                active_chunks: 1,
                waiting_chunks: 0,
                ..
            }
        ));
        let finished = state.finished(17, start + Duration::from_secs(3));
        assert!(matches!(
            finished,
            ByteTransferEvent::DownloadChunkFinished {
                active_chunks: 0,
                waiting_chunks: 0,
                ..
            }
        ));
    }

    struct BlockingObserver {
        first_entered: SyncSender<()>,
        release_first: Mutex<Receiver<()>>,
        second_observed: mpsc::Sender<()>,
        events: Mutex<Vec<ByteTransferEvent>>,
    }

    impl ByteTransferObserver for BlockingObserver {
        fn observe(&self, event: ByteTransferEvent) {
            match event {
                ByteTransferEvent::DownloadChunkStarted { index: 0, .. } => {
                    let _ = self.first_entered.send(());
                    let _ = self.release_first.lock().expect("release receiver").recv();
                }
                ByteTransferEvent::DownloadServerThrottled { .. } => {
                    let _ = self.second_observed.send(());
                }
                _ => {}
            }
            self.events.lock().expect("event list").push(event);
        }
    }

    #[test]
    fn concurrent_chunk_updates_publish_in_counter_order() {
        let attempts = Arc::new(Mutex::new(DownloadStreamAttempts::default()));
        let publication = Arc::new(Mutex::new(()));
        let (first_entered_tx, first_entered_rx) = mpsc::sync_channel(0);
        let (release_first_tx, release_first_rx) = mpsc::sync_channel(0);
        let (second_attempt_tx, second_attempt_rx) = mpsc::sync_channel(0);
        let (second_observed_tx, second_observed_rx) = mpsc::channel();
        let recorder = Arc::new(BlockingObserver {
            first_entered: first_entered_tx,
            release_first: Mutex::new(release_first_rx),
            second_observed: second_observed_tx,
            events: Mutex::new(Vec::new()),
        });
        let observer: Arc<dyn ByteTransferObserver> = recorder.clone();

        let first_attempts = attempts.clone();
        let first_publication = publication.clone();
        let first_observer = observer.clone();
        let first = std::thread::spawn(move || {
            publish_attempt_event(
                &first_attempts,
                &first_publication,
                &first_observer,
                DownloadChunkActivityUpdate::Started {
                    index: 0,
                    attempt: 1,
                    now: Instant::now(),
                },
            );
        });
        first_entered_rx.recv().expect("first callback started");

        let second_attempts = attempts.clone();
        let second_publication = publication.clone();
        let second_observer = observer.clone();
        let second = std::thread::spawn(move || {
            second_attempt_tx.send(()).expect("second thread ready");
            publish_attempt_event(
                &second_attempts,
                &second_publication,
                &second_observer,
                DownloadChunkActivityUpdate::FloodWaitRetryScheduled {
                    index: 0,
                    attempt: 1,
                    delay: Duration::from_millis(100),
                    code: 420,
                    wait_until_unix_ms: 4_200,
                    now: Instant::now(),
                },
            );
            publish_attempt_event(
                &second_attempts,
                &second_publication,
                &second_observer,
                DownloadChunkActivityUpdate::Started {
                    index: 1,
                    attempt: 1,
                    now: Instant::now(),
                },
            );
        });
        second_attempt_rx.recv().expect("second publisher started");
        let delivered_before_first_finished = second_observed_rx
            .recv_timeout(Duration::from_millis(100))
            .is_ok();
        release_first_tx.send(()).expect("release first callback");
        first.join().expect("first publication");
        second.join().expect("second publication");
        assert!(
            !delivered_before_first_finished,
            "a newer counter snapshot cannot overtake a blocked earlier callback"
        );

        let events = recorder.events.lock().expect("ordered events");
        assert_eq!(events.len(), 4);
        assert!(matches!(
            events[0],
            ByteTransferEvent::DownloadChunkStarted {
                index: 0,
                active_chunks: 1,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            ByteTransferEvent::DownloadServerThrottled {
                code: 420,
                wait_until_unix_ms: 4_200,
            }
        ));
        assert!(matches!(
            events[2],
            ByteTransferEvent::DownloadChunkRetry {
                active_chunks: 0,
                waiting_chunks: 1,
                ..
            }
        ));
        assert!(matches!(
            events[3],
            ByteTransferEvent::DownloadChunkStarted {
                index: 1,
                active_chunks: 1,
                waiting_chunks: 1,
                ..
            }
        ));
    }

    #[test]
    fn retried_stream_part_counts_retransmitted_bytes_in_measured_total() {
        #[derive(Default)]
        struct Recorder(Mutex<Vec<ByteTransferEvent>>);

        impl ByteTransferObserver for Recorder {
            fn observe(&self, event: ByteTransferEvent) {
                self.0.lock().expect("events").push(event);
            }
        }

        let observer = Arc::new(Recorder::default());
        let receipts = StreamReceipts {
            state: Mutex::new((0, BTreeMap::new())),
            observer: Some(observer.clone()),
            expected: 128,
        };
        receipts.acknowledged(17, 4);
        receipts.retrying(17);
        receipts.acknowledged(17, 2);
        let totals = observer
            .0
            .lock()
            .expect("events")
            .iter()
            .filter_map(|event| match event {
                ByteTransferEvent::Downloading { bytes, .. } => Some(*bytes),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(totals, [4, 6], "the second attempt contributes again");
    }

    #[test]
    fn encrypted_download_stream_uses_one_mib_inflight_parts() {
        assert_eq!(DOWNLOAD_PART_SIZE_BYTES, 1024 * 1024);

        let expected = DOWNLOAD_PART_SIZE_BYTES * 2 + 17;
        assert_eq!(expected.div_ceil(DOWNLOAD_PART_SIZE_BYTES), 3);
        assert_eq!(
            stream_download_part_range(0, expected),
            Some((0, DOWNLOAD_PART_SIZE_BYTES))
        );
        assert_eq!(
            stream_download_part_range(1, expected),
            Some((DOWNLOAD_PART_SIZE_BYTES, DOWNLOAD_PART_SIZE_BYTES))
        );
        assert_eq!(
            stream_download_part_range(2, expected),
            Some((DOWNLOAD_PART_SIZE_BYTES * 2, 17))
        );
        assert_eq!(stream_download_part_range(3, expected), None);
    }
}
