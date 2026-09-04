use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::thread;

use crate::ConfigurationError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncryptionPipelineConfig {
    pub encryption_worker_count: usize,
    pub plaintext_queue_depth: usize,
    pub encrypted_queue_depth: usize,
}

impl EncryptionPipelineConfig {
    pub fn new(
        encryption_worker_count: usize,
        plaintext_queue_depth: usize,
        encrypted_queue_depth: usize,
    ) -> Result<Self, ConfigurationError> {
        if encryption_worker_count == 0 || plaintext_queue_depth == 0 || encrypted_queue_depth == 0
        {
            return Err(ConfigurationError::InvalidPipelinePolicy {
                field: "pipeline concurrency",
            });
        }
        Ok(Self {
            encryption_worker_count,
            plaintext_queue_depth,
            encrypted_queue_depth,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PipelinePart {
    pub part_index: u32,
    pub plaintext_offset: u64,
    pub plaintext_length: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineStage {
    Read,
    Encrypt,
    EncryptedQueue,
    Upload,
    Confirmed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PipelineEvent {
    pub sequence: u64,
    pub part_index: u32,
    pub stage: PipelineStage,
    pub plaintext_queue_length: usize,
    pub encrypted_queue_length: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptionPipelineReport {
    pub completed_parts: usize,
    pub maximum_plaintext_queue_length: usize,
    pub maximum_encrypted_queue_length: usize,
    pub events: Vec<PipelineEvent>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum EncryptionPipelineError<PipelineError> {
    Configuration(ConfigurationError),
    Read {
        part_index: u32,
        source: PipelineError,
    },
    Encrypt {
        part_index: u32,
        source: PipelineError,
    },
    Upload {
        part_index: u32,
        source: PipelineError,
    },
    WorkerStopped,
}

struct PlaintextWork {
    descriptor: PipelinePart,
    bytes: Vec<u8>,
}

enum ReaderMessage {
    Part(PlaintextWork),
}

enum EncryptedMessage<EncryptedPart, PipelineError> {
    Part {
        part_index: u32,
        encrypted: EncryptedPart,
    },
    ReadFailed {
        part_index: u32,
        source: PipelineError,
    },
    EncryptFailed {
        part_index: u32,
        source: PipelineError,
    },
}

/// Runs a bounded reader → encryption-worker-pool → encrypted queue → upload
/// pipeline. Uploads execute on the calling thread while encryption continues
/// on scoped workers, so secrets and borrowed crypto state do not need a
/// process-global owner.
pub fn run_encryption_upload_pipeline<
    EncryptedPart,
    PipelineError,
    ReadPart,
    EncryptPart,
    UploadPart,
>(
    config: EncryptionPipelineConfig,
    parts: &[PipelinePart],
    read_part: ReadPart,
    encrypt_part: EncryptPart,
    mut upload_part: UploadPart,
) -> Result<EncryptionPipelineReport, EncryptionPipelineError<PipelineError>>
where
    EncryptedPart: Send,
    PipelineError: Send,
    ReadPart: Fn(PipelinePart) -> Result<Vec<u8>, PipelineError> + Send + Sync,
    EncryptPart: Fn(PipelinePart, Vec<u8>) -> Result<EncryptedPart, PipelineError> + Send + Sync,
    UploadPart: FnMut(u32, EncryptedPart) -> Result<(), PipelineError>,
{
    if parts.iter().any(|part| part.plaintext_length == 0) {
        return Err(EncryptionPipelineError::Configuration(
            ConfigurationError::InvalidPipelinePolicy {
                field: "zero-length pipeline part",
            },
        ));
    }
    if parts.is_empty() {
        return Ok(EncryptionPipelineReport {
            completed_parts: 0,
            maximum_plaintext_queue_length: 0,
            maximum_encrypted_queue_length: 0,
            events: Vec::new(),
        });
    }

    let cancellation = Arc::new(AtomicBool::new(false));
    let plaintext_queue_length = Arc::new(AtomicUsize::new(0));
    let encrypted_queue_length = Arc::new(AtomicUsize::new(0));
    let maximum_plaintext_queue_length = Arc::new(AtomicUsize::new(0));
    let maximum_encrypted_queue_length = Arc::new(AtomicUsize::new(0));
    let (plaintext_sender, plaintext_receiver) = mpsc::sync_channel(config.plaintext_queue_depth);
    let shared_plaintext_receiver = Arc::new(Mutex::new(plaintext_receiver));
    let (encrypted_sender, encrypted_receiver) = mpsc::sync_channel(config.encrypted_queue_depth);

    let mut completed_parts = 0_usize;
    let mut events = Vec::with_capacity(parts.len().saturating_mul(5));
    let mut first_error = None;

    thread::scope(|scope| {
        let reader_cancellation = Arc::clone(&cancellation);
        let reader_queue_length = Arc::clone(&plaintext_queue_length);
        let reader_queue_maximum = Arc::clone(&maximum_plaintext_queue_length);
        let encrypted_sender_for_reader = encrypted_sender.clone();
        scope.spawn(move || {
            for descriptor in parts.iter().copied() {
                if reader_cancellation.load(Ordering::Acquire) {
                    break;
                }
                match read_part(descriptor) {
                    Ok(bytes) => {
                        if bytes.len() as u64 != descriptor.plaintext_length {
                            reader_cancellation.store(true, Ordering::Release);
                            break;
                        }
                        let queue_length = reader_queue_length.fetch_add(1, Ordering::AcqRel) + 1;
                        reader_queue_maximum.fetch_max(queue_length, Ordering::AcqRel);
                        if plaintext_sender
                            .send(ReaderMessage::Part(PlaintextWork { descriptor, bytes }))
                            .is_err()
                        {
                            reader_queue_length.fetch_sub(1, Ordering::AcqRel);
                            break;
                        }
                    }
                    Err(source) => {
                        reader_cancellation.store(true, Ordering::Release);
                        let _ = encrypted_sender_for_reader.send(EncryptedMessage::ReadFailed {
                            part_index: descriptor.part_index,
                            source,
                        });
                        break;
                    }
                }
            }
        });

        for _worker_index in 0..config.encryption_worker_count {
            let worker_cancellation = Arc::clone(&cancellation);
            let worker_receiver = Arc::clone(&shared_plaintext_receiver);
            let worker_sender = encrypted_sender.clone();
            let worker_plaintext_length = Arc::clone(&plaintext_queue_length);
            let worker_encrypted_length = Arc::clone(&encrypted_queue_length);
            let worker_encrypted_maximum = Arc::clone(&maximum_encrypted_queue_length);
            let encrypt_part = &encrypt_part;
            scope.spawn(move || {
                loop {
                    let received = worker_receiver
                        .lock()
                        .ok()
                        .and_then(|receiver| receiver.recv().ok());
                    let Some(message) = received else {
                        break;
                    };
                    match message {
                        ReaderMessage::Part(work) => {
                            worker_plaintext_length.fetch_sub(1, Ordering::AcqRel);
                            if worker_cancellation.load(Ordering::Acquire) {
                                continue;
                            }
                            match encrypt_part(work.descriptor, work.bytes) {
                                Ok(encrypted) => {
                                    let queue_length =
                                        worker_encrypted_length.fetch_add(1, Ordering::AcqRel) + 1;
                                    worker_encrypted_maximum
                                        .fetch_max(queue_length, Ordering::AcqRel);
                                    if worker_sender
                                        .send(EncryptedMessage::Part {
                                            part_index: work.descriptor.part_index,
                                            encrypted,
                                        })
                                        .is_err()
                                    {
                                        worker_encrypted_length.fetch_sub(1, Ordering::AcqRel);
                                        break;
                                    }
                                }
                                Err(source) => {
                                    worker_cancellation.store(true, Ordering::Release);
                                    let _ = worker_sender.send(EncryptedMessage::EncryptFailed {
                                        part_index: work.descriptor.part_index,
                                        source,
                                    });
                                }
                            }
                        }
                    }
                }
            });
        }
        drop(encrypted_sender);

        while let Ok(message) = encrypted_receiver.recv() {
            match message {
                EncryptedMessage::Part {
                    part_index,
                    encrypted,
                } => {
                    encrypted_queue_length.fetch_sub(1, Ordering::AcqRel);
                    let sequence = events.len() as u64 + 1;
                    events.push(PipelineEvent {
                        sequence,
                        part_index,
                        stage: PipelineStage::EncryptedQueue,
                        plaintext_queue_length: plaintext_queue_length.load(Ordering::Acquire),
                        encrypted_queue_length: encrypted_queue_length.load(Ordering::Acquire),
                    });
                    if first_error.is_some() {
                        continue;
                    }
                    if let Err(source) = upload_part(part_index, encrypted) {
                        cancellation.store(true, Ordering::Release);
                        first_error = Some(EncryptionPipelineError::Upload { part_index, source });
                        continue;
                    }
                    completed_parts = completed_parts.saturating_add(1);
                    let sequence = events.len() as u64 + 1;
                    events.push(PipelineEvent {
                        sequence,
                        part_index,
                        stage: PipelineStage::Confirmed,
                        plaintext_queue_length: plaintext_queue_length.load(Ordering::Acquire),
                        encrypted_queue_length: encrypted_queue_length.load(Ordering::Acquire),
                    });
                }
                EncryptedMessage::ReadFailed { part_index, source } => {
                    cancellation.store(true, Ordering::Release);
                    if first_error.is_none() {
                        first_error = Some(EncryptionPipelineError::Read { part_index, source });
                    }
                }
                EncryptedMessage::EncryptFailed { part_index, source } => {
                    cancellation.store(true, Ordering::Release);
                    if first_error.is_none() {
                        first_error = Some(EncryptionPipelineError::Encrypt { part_index, source });
                    }
                }
            }
        }
    });

    if let Some(error) = first_error {
        return Err(error);
    }
    if completed_parts != parts.len() {
        return Err(EncryptionPipelineError::WorkerStopped);
    }
    Ok(EncryptionPipelineReport {
        completed_parts,
        maximum_plaintext_queue_length: maximum_plaintext_queue_length.load(Ordering::Acquire),
        maximum_encrypted_queue_length: maximum_encrypted_queue_length.load(Ordering::Acquire),
        events,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Condvar, Mutex};

    use super::*;

    fn config() -> EncryptionPipelineConfig {
        EncryptionPipelineConfig::new(2, 2, 2).expect("valid pipeline")
    }

    #[test]
    fn encryption_overlaps_upload_with_bounded_queues() {
        let second_part_encrypted = Arc::new((Mutex::new(false), Condvar::new()));
        let signal_from_encrypt = Arc::clone(&second_part_encrypted);
        let signal_for_upload = Arc::clone(&second_part_encrypted);
        let parts = [
            PipelinePart {
                part_index: 0,
                plaintext_offset: 0,
                plaintext_length: 4,
            },
            PipelinePart {
                part_index: 1,
                plaintext_offset: 4,
                plaintext_length: 4,
            },
        ];
        let report = run_encryption_upload_pipeline(
            config(),
            &parts,
            |_| Ok::<_, &'static str>(vec![7; 4]),
            move |descriptor, bytes| {
                if descriptor.part_index == 1
                    && let Ok(mut ready) = signal_from_encrypt.0.lock()
                {
                    *ready = true;
                    signal_from_encrypt.1.notify_all();
                }
                Ok::<_, &'static str>(bytes)
            },
            move |part_index, _| {
                if part_index == 0 {
                    let mut ready = signal_for_upload.0.lock().map_err(|_| "poisoned")?;
                    while !*ready {
                        ready = signal_for_upload.1.wait(ready).map_err(|_| "poisoned")?;
                    }
                }
                Ok::<_, &'static str>(())
            },
        )
        .expect("pipeline succeeds");
        assert_eq!(report.completed_parts, 2);
        assert!(report.maximum_plaintext_queue_length <= 2);
        assert!(report.maximum_encrypted_queue_length <= 2);
    }

    #[test]
    fn upload_failure_cancels_without_deadlocking_workers() {
        let parts = (0..8)
            .map(|part_index| PipelinePart {
                part_index,
                plaintext_offset: u64::from(part_index) * 4,
                plaintext_length: 4,
            })
            .collect::<Vec<_>>();
        let error = run_encryption_upload_pipeline(
            config(),
            &parts,
            |_| Ok::<_, &'static str>(vec![1; 4]),
            |_, bytes| Ok::<_, &'static str>(bytes),
            |part_index, _| {
                if part_index == 0 {
                    Err("network")
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("upload must fail");
        assert!(matches!(
            error,
            EncryptionPipelineError::Upload {
                source: "network",
                ..
            }
        ));
    }
}
