use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

/// Ephemeral bounded activity. `Uploading` is legacy transport-read progress;
/// `PartAcknowledged` reports server-confirmed ciphertext, not message publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteTransferEvent {
    WaitingForUpload,
    UploadPlan {
        parts: u32,
        total: u64,
    },
    UploadQueue {
        queued: u16,
        active: u16,
    },
    PartAcknowledged {
        index: u32,
        bytes: u64,
        total: u64,
    },
    SavingCheckpoint,
    CheckpointSaved,
    WaitingForSeal,
    Uploading {
        bytes: u64,
        total: u64,
    },
    PartStarted {
        index: u32,
        attempt: u16,
    },
    PartRetry {
        index: u32,
        attempt: u16,
        wait_millis: u64,
    },
    ServerThrottled {
        code: i32,
        wait_seconds: u32,
    },
    SendingMessage,
    Downloading {
        bytes: u64,
        total: u64,
    },
}

/// Called inline with bounded byte counters only. Implementations must not
/// block on I/O or retain buffers, filenames, captions or credentials.
pub trait ByteTransferObserver: Send + Sync {
    fn observe(&self, event: ByteTransferEvent);
    fn transfer_tuning(&self) -> crate::TransferTuning {
        crate::TransferTuning::default()
    }
}

pub(crate) struct UploadReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    observer: Option<&'a dyn ByteTransferObserver>,
}

impl<'a> UploadReader<'a> {
    pub(crate) fn new(bytes: &'a [u8], observer: Option<&'a dyn ByteTransferObserver>) -> Self {
        Self {
            bytes,
            offset: 0,
            observer,
        }
    }
}

impl AsyncRead for UploadReader<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let count = buf
            .remaining()
            .min(self.bytes.len().saturating_sub(self.offset));
        buf.put_slice(&self.bytes[self.offset..self.offset + count]);
        self.offset += count;
        if count != 0
            && let Some(observer) = self.observer
        {
            observer.observe(ByteTransferEvent::Uploading {
                bytes: self.offset as u64,
                total: self.bytes.len() as u64,
            });
        }
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<ByteTransferEvent>>);
    impl ByteTransferObserver for Recorder {
        fn observe(&self, event: ByteTransferEvent) {
            self.0.lock().expect("test snapshot lock").push(event);
        }
    }

    #[tokio::test]
    async fn reports_partial_reads_without_claiming_remote_completion() {
        let recorder = Recorder::default();
        let mut reader = UploadReader::new(b"synthetic", Some(&recorder));
        let mut first = [0; 3];
        reader
            .read_exact(&mut first)
            .await
            .expect("read synthetic bytes");
        assert_eq!(&first, b"syn");
        assert_eq!(
            *recorder.0.lock().expect("test snapshot lock"),
            [ByteTransferEvent::Uploading { bytes: 3, total: 9 }]
        );
        let mut rest = Vec::new();
        reader
            .read_to_end(&mut rest)
            .await
            .expect("read synthetic bytes");
        assert_eq!(rest, b"thetic");
        assert_eq!(
            recorder.0.lock().expect("test snapshot lock").last(),
            Some(&ByteTransferEvent::Uploading { bytes: 9, total: 9 })
        );
        assert!(
            !recorder
                .0
                .lock()
                .expect("test snapshot lock")
                .contains(&ByteTransferEvent::SendingMessage)
        );
    }
}
