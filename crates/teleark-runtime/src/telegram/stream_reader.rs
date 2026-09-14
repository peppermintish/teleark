//! Blocking reader used only by retained crypto owners, never by UI/reactor callbacks.
use super::*;
use std::io::{Cursor, Read};

pub(super) struct ByteReader {
    blocks: tokio::sync::mpsc::Receiver<Vec<u8>>,
    completion: mpsc::Receiver<Result<(), ApplicationError>>,
    current: Cursor<Vec<u8>>,
    abort: TelegramScanCancellation,
    telegram: DesktopTelegram,
    generation: Option<u64>,
    finished: bool,
    error: Option<teleark_core::TransferError>,
    _test_worker: Option<JoinHandle<()>>,
}
impl ByteReader {
    pub(super) fn new(
        blocks: tokio::sync::mpsc::Receiver<Vec<u8>>,
        completion: mpsc::Receiver<Result<(), ApplicationError>>,
        abort: TelegramScanCancellation,
        telegram: DesktopTelegram,
        generation: Option<u64>,
        test_worker: Option<JoinHandle<()>>,
    ) -> Self {
        Self {
            blocks,
            completion,
            current: Cursor::new(Vec::new()),
            abort,
            telegram,
            generation,
            finished: false,
            error: None,
            _test_worker: test_worker,
        }
    }
}
impl Read for ByteReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            if self.generation.is_some_and(|generation| {
                self.telegram.inner.monitor.snapshot().generation != generation
            }) {
                self.error = Some(teleark_core::TransferError::Cancelled);
            }
            if self.error.is_some() {
                return Err(std::io::Error::other("remote stream interrupted"));
            }
            let count = self.current.read(output)?;
            if count > 0 {
                return Ok(count);
            }
            if self.finished {
                return Ok(0);
            }
            if let Some(bytes) = self.blocks.blocking_recv() {
                if bytes.is_empty() || bytes.len() > teleark_telegram::UPLOAD_PART_BYTES {
                    self.error = Some(teleark_core::TransferError::HashMismatch);
                } else {
                    self.current = Cursor::new(bytes);
                }
            } else {
                self.finished = true;
                self.error = match self.completion.recv() {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(crate::transfer::map_application_error(error)),
                    Err(_) => Some(teleark_core::TransferError::Network),
                };
                if self.generation.is_some_and(|generation| {
                    self.telegram.inner.monitor.snapshot().generation != generation
                }) {
                    self.error = Some(teleark_core::TransferError::Cancelled);
                }
            }
        }
    }
}
impl crate::transfer::RemoteReader for ByteReader {
    fn error(&self) -> Option<teleark_core::TransferError> {
        self.error.clone()
    }
}
impl Drop for ByteReader {
    fn drop(&mut self) {
        // Dropping an early-failed crypto reader wakes the network request immediately.
        self.blocks.close();
        self.abort.cancel();
    }
}
