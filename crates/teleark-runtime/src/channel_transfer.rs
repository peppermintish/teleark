use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
};

use teleark_core::{ApplicationError, ApplicationErrorKind};

use crate::DesktopTelegram;

const CHANNEL_DOWNLOAD_QUEUE_CAPACITY: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelDownloadRequest {
    pub chat_id: i64,
    pub message_id: i64,
    pub file_name: String,
    pub size_bytes: u64,
    pub destination: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelDownloadState {
    Queued,
    Running,
    Completed,
    Failed(ApplicationErrorKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelDownloadSnapshot {
    pub id: u64,
    pub chat_id: i64,
    pub message_id: i64,
    pub file_name: String,
    pub size_bytes: u64,
    pub destination: PathBuf,
    pub state: ChannelDownloadState,
}

#[derive(Clone)]
pub struct DesktopTransfers {
    inner: Arc<TransferWorkerInner>,
}

struct TransferWorkerInner {
    sender: Mutex<Option<mpsc::SyncSender<TransferCommand>>>,
    snapshots: Arc<Mutex<Vec<ChannelDownloadSnapshot>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

enum TransferCommand {
    Download(ChannelDownloadSnapshot),
}

trait ChannelDownloadBackend: Send + Sync + 'static {
    fn download(
        &self,
        chat_id: i64,
        message_id: i64,
        destination: &Path,
    ) -> Result<(), ApplicationError>;
}

impl ChannelDownloadBackend for DesktopTelegram {
    fn download(
        &self,
        chat_id: i64,
        message_id: i64,
        destination: &Path,
    ) -> Result<(), ApplicationError> {
        self.download_file(chat_id, message_id, destination)
    }
}

impl DesktopTransfers {
    pub fn new(telegram: DesktopTelegram) -> Result<Self, ApplicationError> {
        Self::with_backend(Arc::new(telegram))
    }

    fn with_backend(backend: Arc<dyn ChannelDownloadBackend>) -> Result<Self, ApplicationError> {
        let (sender, receiver) = mpsc::sync_channel(CHANNEL_DOWNLOAD_QUEUE_CAPACITY);
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let worker_snapshots = Arc::clone(&snapshots);
        let join = thread::Builder::new()
            .name("teleark-channel-downloads".to_owned())
            .spawn(move || transfer_loop(receiver, backend, worker_snapshots))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Network))?;
        Ok(Self {
            inner: Arc::new(TransferWorkerInner {
                sender: Mutex::new(Some(sender)),
                snapshots,
                join: Mutex::new(Some(join)),
            }),
        })
    }

    pub fn enqueue_channel_download(
        &self,
        request: ChannelDownloadRequest,
    ) -> Result<u64, ApplicationError> {
        validate_request(&request)?;
        let mut snapshots = self
            .inner
            .snapshots
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        if snapshots.iter().any(|snapshot| {
            snapshot.destination == request.destination
                && matches!(
                    snapshot.state,
                    ChannelDownloadState::Queued | ChannelDownloadState::Running
                )
        }) {
            return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
        }
        let id = snapshots
            .last()
            .map_or(1, |snapshot| snapshot.id.saturating_add(1));
        if id == u64::MAX {
            return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
        }
        let snapshot = ChannelDownloadSnapshot {
            id,
            chat_id: request.chat_id,
            message_id: request.message_id,
            file_name: request.file_name,
            size_bytes: request.size_bytes,
            destination: request.destination,
            state: ChannelDownloadState::Queued,
        };
        snapshots.push(snapshot.clone());
        drop(snapshots);

        let send_result = self
            .inner
            .sender
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Cancelled))?
            .try_send(TransferCommand::Download(snapshot));
        if let Err(error) = send_result {
            if let Ok(mut snapshots) = self.inner.snapshots.lock() {
                snapshots.retain(|snapshot| snapshot.id != id);
            }
            let kind = match error {
                mpsc::TrySendError::Full(_) => ApplicationErrorKind::Capacity,
                mpsc::TrySendError::Disconnected(_) => ApplicationErrorKind::Cancelled,
            };
            return Err(ApplicationError::new(kind));
        }
        Ok(id)
    }

    pub fn snapshots(&self) -> Result<Vec<ChannelDownloadSnapshot>, ApplicationError> {
        self.inner
            .snapshots
            .lock()
            .map(|snapshots| snapshots.clone())
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))
    }
}

impl Drop for TransferWorkerInner {
    fn drop(&mut self) {
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
        if let Ok(mut join) = self.join.lock()
            && let Some(join) = join.take()
        {
            let _ = join.join();
        }
    }
}

fn validate_request(request: &ChannelDownloadRequest) -> Result<(), ApplicationError> {
    if request.chat_id <= 0
        || request.message_id <= 0
        || request.file_name.trim().is_empty()
        || request.destination.as_os_str().is_empty()
        || request.destination.file_name().is_none()
    {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok(())
}

fn transfer_loop(
    receiver: mpsc::Receiver<TransferCommand>,
    backend: Arc<dyn ChannelDownloadBackend>,
    snapshots: Arc<Mutex<Vec<ChannelDownloadSnapshot>>>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            TransferCommand::Download(snapshot) => {
                update_state(&snapshots, snapshot.id, ChannelDownloadState::Running);
                let state = match backend.download(
                    snapshot.chat_id,
                    snapshot.message_id,
                    &snapshot.destination,
                ) {
                    Ok(()) => ChannelDownloadState::Completed,
                    Err(error) => ChannelDownloadState::Failed(error.kind()),
                };
                update_state(&snapshots, snapshot.id, state);
            }
        }
    }
}

fn update_state(
    snapshots: &Mutex<Vec<ChannelDownloadSnapshot>>,
    id: u64,
    state: ChannelDownloadState,
) {
    if let Ok(mut snapshots) = snapshots.lock()
        && let Some(snapshot) = snapshots.iter_mut().find(|snapshot| snapshot.id == id)
    {
        snapshot.state = state;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{Condvar, Mutex as StdMutex},
        time::{Duration, Instant},
    };

    use super::*;

    struct FakeBackend {
        outcome: Result<(), ApplicationErrorKind>,
        calls: StdMutex<Vec<(i64, i64, PathBuf)>>,
    }

    impl ChannelDownloadBackend for FakeBackend {
        fn download(
            &self,
            chat_id: i64,
            message_id: i64,
            destination: &Path,
        ) -> Result<(), ApplicationError> {
            self.calls.lock().expect("fake call lock").push((
                chat_id,
                message_id,
                destination.to_owned(),
            ));
            match self.outcome {
                Ok(()) => {
                    fs::write(destination, b"telegram bytes")
                        .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
                    Ok(())
                }
                Err(kind) => Err(ApplicationError::new(kind)),
            }
        }
    }

    fn request(destination: PathBuf) -> ChannelDownloadRequest {
        ChannelDownloadRequest {
            chat_id: 100,
            message_id: 200,
            file_name: "archive.zip".to_owned(),
            size_bytes: 14,
            destination,
        }
    }

    fn wait_for_terminal(transfers: &DesktopTransfers, id: u64) -> ChannelDownloadSnapshot {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snapshot = transfers
                .snapshots()
                .expect("snapshots")
                .into_iter()
                .find(|snapshot| snapshot.id == id)
                .expect("queued snapshot");
            if matches!(
                snapshot.state,
                ChannelDownloadState::Completed | ChannelDownloadState::Failed(_)
            ) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "download worker timed out");
            thread::yield_now();
        }
    }

    #[test]
    fn real_worker_runs_backend_and_publishes_completed_snapshot() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let destination = directory.path().join("archive.zip");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = DesktopTransfers::with_backend(backend.clone()).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("enqueue");
        let snapshot = wait_for_terminal(&transfers, id);
        assert_eq!(snapshot.state, ChannelDownloadState::Completed);
        assert_eq!(
            fs::read(destination).expect("download result"),
            b"telegram bytes"
        );
        assert_eq!(backend.calls.lock().expect("calls").len(), 1);
    }

    #[test]
    fn backend_failures_remain_structured_in_the_snapshot() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let backend = Arc::new(FakeBackend {
            outcome: Err(ApplicationErrorKind::Network),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = DesktopTransfers::with_backend(backend).expect("worker");
        let id = transfers
            .enqueue_channel_download(request(directory.path().join("failed.zip")))
            .expect("enqueue");
        assert_eq!(
            wait_for_terminal(&transfers, id).state,
            ChannelDownloadState::Failed(ApplicationErrorKind::Network)
        );
    }

    #[test]
    fn downloads_from_different_channels_keep_their_source_identity() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let backend = Arc::new(FakeBackend {
            outcome: Ok(()),
            calls: StdMutex::new(Vec::new()),
        });
        let transfers = DesktopTransfers::with_backend(backend.clone()).expect("worker");
        let first = transfers
            .enqueue_channel_download(request(directory.path().join("first.zip")))
            .expect("first enqueue");
        let second = transfers
            .enqueue_channel_download(ChannelDownloadRequest {
                chat_id: 101,
                message_id: 201,
                file_name: "second.pdf".to_owned(),
                size_bytes: 14,
                destination: directory.path().join("second.pdf"),
            })
            .expect("second enqueue");
        assert_eq!(wait_for_terminal(&transfers, first).chat_id, 100);
        let second_snapshot = wait_for_terminal(&transfers, second);
        assert_eq!(second_snapshot.chat_id, 101);
        assert_eq!(second_snapshot.message_id, 201);
        let calls = backend.calls.lock().expect("calls");
        assert_eq!((calls[0].0, calls[0].1), (100, 200));
        assert_eq!((calls[1].0, calls[1].1), (101, 201));
    }

    #[test]
    fn invalid_and_duplicate_active_destinations_are_rejected() {
        struct BlockingBackend {
            gate: Arc<(StdMutex<bool>, Condvar)>,
        }
        impl ChannelDownloadBackend for BlockingBackend {
            fn download(
                &self,
                _chat_id: i64,
                _message_id: i64,
                _destination: &Path,
            ) -> Result<(), ApplicationError> {
                let (lock, condition) = &*self.gate;
                let mut released = lock.lock().expect("gate lock");
                while !*released {
                    released = condition.wait(released).expect("gate wait");
                }
                Ok(())
            }
        }

        let gate = Arc::new((StdMutex::new(false), Condvar::new()));
        let transfers = DesktopTransfers::with_backend(Arc::new(BlockingBackend {
            gate: Arc::clone(&gate),
        }))
        .expect("worker");
        let directory = tempfile::tempdir().expect("temporary directory");
        let destination = directory.path().join("same.zip");
        transfers
            .enqueue_channel_download(request(destination.clone()))
            .expect("first enqueue");
        let error = transfers
            .enqueue_channel_download(request(destination))
            .expect_err("duplicate active destination must fail");
        assert_eq!(error.kind(), ApplicationErrorKind::Conflict);
        let invalid = ChannelDownloadRequest {
            chat_id: 0,
            ..request(directory.path().join("invalid.zip"))
        };
        assert_eq!(
            transfers
                .enqueue_channel_download(invalid)
                .expect_err("invalid chat must fail")
                .kind(),
            ApplicationErrorKind::InvalidRequest
        );
        let (lock, condition) = &*gate;
        *lock.lock().expect("gate lock") = true;
        condition.notify_all();
    }
}
