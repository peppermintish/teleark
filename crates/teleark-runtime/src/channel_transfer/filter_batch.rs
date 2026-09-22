//! Filter-driven admission reads bounded cache pages off the frontend thread.
use super::*;
use crate::{StorageRequest, classify_file};
use teleark_core::{AccountId, ChatId, FileKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelBatchFilter {
    pub account_id: i64,
    pub chat_id: i64,
    /// Inclusive, frozen when the user starts the batch.
    pub earliest_unix_ms: Option<i64>,
    pub latest_unix_ms: i64,
    /// Empty means every file kind.
    pub kinds: Vec<FileKind>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelBatchPreparationPhase {
    Discovering,
    PreparingFolder,
    Queuing,
    Completed,
    Cancelled,
    Failed(ApplicationErrorKind),
}

impl ChannelBatchPreparationPhase {
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::Discovering | Self::PreparingFolder | Self::Queuing
        )
    }
}

#[derive(Clone, Debug)]
pub struct ChannelBatchPreparationSnapshot {
    pub phase: ChannelBatchPreparationPhase,
    pub examined: u64,
    pub matched: usize,
    pub started_at: Instant,
    pub phase_started_at: Instant,
    pub last_activity_at: Instant,
    /// At most five transitions, including terminal outcome; no sample history.
    pub events: Vec<(ChannelBatchPreparationPhase, Instant)>,
}

#[derive(Clone)]
pub struct ChannelBatchPreparation(Arc<Mutex<ChannelBatchPreparationSnapshot>>);

impl Default for ChannelBatchPreparation {
    fn default() -> Self {
        let now = Instant::now();
        Self(Arc::new(Mutex::new(ChannelBatchPreparationSnapshot {
            phase: ChannelBatchPreparationPhase::Discovering,
            examined: 0,
            matched: 0,
            started_at: now,
            phase_started_at: now,
            last_activity_at: now,
            events: vec![(ChannelBatchPreparationPhase::Discovering, now)],
        })))
    }
}

impl ChannelBatchPreparation {
    pub fn snapshot(&self) -> Option<ChannelBatchPreparationSnapshot> {
        self.0.lock().ok().map(|state| state.clone())
    }

    /// Once durable admission starts, task Stop owns cancellation instead.
    pub fn cancel(&self) {
        if let Ok(mut state) = self.0.lock()
            && matches!(
                state.phase,
                ChannelBatchPreparationPhase::Discovering
                    | ChannelBatchPreparationPhase::PreparingFolder
            )
        {
            transition(&mut state, ChannelBatchPreparationPhase::Cancelled);
        }
    }

    fn update(
        &self,
        phase: ChannelBatchPreparationPhase,
        examined: u64,
        matched: usize,
    ) -> Result<(), ApplicationError> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        if state.phase == ChannelBatchPreparationPhase::Cancelled {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        if state.phase != phase {
            transition(&mut state, phase);
        }
        state.examined = examined;
        state.matched = matched;
        state.last_activity_at = Instant::now();
        Ok(())
    }

    fn check(&self) -> Result<(), ApplicationError> {
        if self
            .0
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?
            .phase
            == ChannelBatchPreparationPhase::Cancelled
        {
            Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
        } else {
            Ok(())
        }
    }
}

fn transition(state: &mut ChannelBatchPreparationSnapshot, phase: ChannelBatchPreparationPhase) {
    let now = Instant::now();
    state.phase = phase;
    state.phase_started_at = now;
    state.last_activity_at = now;
    if state.events.len() < 5 {
        state.events.push((phase, now));
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilteredChannelBatch {
    pub batch_id: u64,
    pub count: usize,
    pub directory: PathBuf,
}

impl DesktopTransfers {
    /// Downloads all matching *indexed* source files, regardless of browser
    /// selection or its visible page. Read in chunks of 256; retain at most
    /// 5,000 matches. An oversized result fails before creating any directory
    /// or task. Call off the GUI thread and retain its handle to completion.
    pub fn enqueue_filtered_channel_batch(
        &self,
        filter: ChannelBatchFilter,
        progress: &ChannelBatchPreparation,
    ) -> Result<Option<FilteredChannelBatch>, ApplicationError> {
        let result = self.prepare_filtered_channel_batch(filter, progress);
        if let Ok(mut state) = progress.0.lock() {
            let phase = match &result {
                Ok(_) => ChannelBatchPreparationPhase::Completed,
                Err(error) if error.kind() == ApplicationErrorKind::Cancelled => {
                    ChannelBatchPreparationPhase::Cancelled
                }
                Err(error) => ChannelBatchPreparationPhase::Failed(error.kind()),
            };
            if state.phase != phase {
                transition(&mut state, phase);
            }
        }
        result
    }

    fn prepare_filtered_channel_batch(
        &self,
        filter: ChannelBatchFilter,
        progress: &ChannelBatchPreparation,
    ) -> Result<Option<FilteredChannelBatch>, ApplicationError> {
        if filter
            .earliest_unix_ms
            .is_some_and(|from| from > filter.latest_unix_ms)
        {
            return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
        }
        let (files, examined) = collect_candidates(&filter, progress, |before| {
            self.require_active_account(Some(filter.account_id))?;
            self.inner
                .library
                .worker
                .request("channel_batch_candidates", |reply| {
                    StorageRequest::ChannelBatchCandidates {
                        account: AccountId::new(filter.account_id),
                        chat: ChatId::new(filter.chat_id),
                        before,
                        reply,
                    }
                })
        })?;
        if files.is_empty() {
            return Ok(None);
        }
        progress.update(
            ChannelBatchPreparationPhase::PreparingFolder,
            examined,
            files.len(),
        )?;
        self.require_active_account(Some(filter.account_id))?;
        let directories = self.inner.library.managed_directories()?;
        progress.check()?;
        let directory = create_batch_directory(&directories.downloads)?;
        let mut claimed = Vec::new();
        let mut admission_started = false;
        let result = (|| {
            let mut reserved = BTreeSet::new();
            let mut requests = Vec::with_capacity(files.len());
            for file in files {
                progress.check()?;
                let file_name = safe_name(&file.file_name, file.message_id.get());
                let destination = available_download_destination_with_claim(
                    &directory,
                    &file_name,
                    |path| {
                        Ok(destination_paths(path)?
                            .iter()
                            .any(|path| reserved.contains(path)))
                    },
                    claim_destination,
                )?;
                claimed.push(destination.clone());
                reserved.extend(destination_paths(&destination)?);
                requests.push(ChannelDownloadRequest {
                    account_id: filter.account_id,
                    chat_id: filter.chat_id,
                    message_id: file.message_id.get(),
                    message_sent_at_unix_ms: Some(file.sent_at_unix_ms),
                    file_name,
                    caption: file.caption,
                    mime_type: file.mime_type,
                    size_bytes: file.size_bytes,
                    destination,
                });
            }
            let count = requests.len();
            // Serializes cancellation against the start of durable admission.
            progress.update(ChannelBatchPreparationPhase::Queuing, examined, count)?;
            admission_started = true;
            let batch_id = self.enqueue_channel_download_batch(requests)?;
            Ok(Some(FilteredChannelBatch {
                batch_id,
                count,
                directory: directory.clone(),
            }))
        })();
        if result.is_err() && !admission_started {
            for destination in claimed {
                let _ = super::reservation::release_cancelled(&destination);
            }
            // An admission error may follow a successful durable commit. Keep
            // those folders/markers available to recovery and queued owners.
            // Pre-admission cleanup only ever removes our empty folder.
            let _ = std::fs::remove_dir(&directory);
        }
        result
    }
}

fn collect_candidates(
    filter: &ChannelBatchFilter,
    progress: &ChannelBatchPreparation,
    mut read: impl FnMut(
        i64,
    )
        -> Result<Vec<teleark_storage::CachedTelegramFileRecord>, ApplicationError>,
) -> Result<(Vec<teleark_storage::CachedTelegramFileRecord>, u64), ApplicationError> {
    let mut before = i64::MAX;
    let mut examined = 0_u64;
    let mut files = Vec::new();
    loop {
        progress.check()?;
        let page = read(before)?;
        progress.check()?;
        if page.is_empty() {
            break;
        }
        before = page.last().expect("nonempty page").message_id.get();
        examined = examined.saturating_add(page.len() as u64);
        for file in page {
            if file.sent_at_unix_ms <= filter.latest_unix_ms
                && filter
                    .earliest_unix_ms
                    .is_none_or(|from| file.sent_at_unix_ms >= from)
                && (filter.kinds.is_empty()
                    || filter
                        .kinds
                        .contains(&classify_file(Path::new(&file.file_name))))
            {
                if files.len() == CHANNEL_DOWNLOAD_BATCH_CAPACITY {
                    return Err(ApplicationError::new(ApplicationErrorKind::Capacity));
                }
                files.push(file);
            }
        }
        progress.update(
            ChannelBatchPreparationPhase::Discovering,
            examined,
            files.len(),
        )?;
    }
    Ok((files, examined))
}

fn safe_name(name: &str, message_id: i64) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("telegram-document-{message_id}"))
}

fn destination_paths(destination: &Path) -> Result<[PathBuf; 4], ApplicationError> {
    let [partial, map] = teleark_telegram::native_download_artifact_paths(destination)
        .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
    let mut reservation = destination.as_os_str().to_os_string();
    reservation.push(".partial");
    Ok([
        destination.to_owned(),
        PathBuf::from(reservation),
        partial,
        map,
    ])
}

// Let the filesystem arbitrate basename equivalence (including case and Unicode
// normalization) before any worker can open a private partial. The standard
// native completion/cancellation owner retires these empty admission markers.
fn claim_destination(destination: &Path) -> Result<bool, ApplicationError> {
    let mut marker = destination.as_os_str().to_os_string();
    marker.push(".partial");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(PathBuf::from(marker)) {
        Ok(file) => file
            .sync_all()
            .map(|()| true)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(ApplicationError::new(
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                ApplicationErrorKind::PermissionDenied
            } else {
                ApplicationErrorKind::Persistence
            },
        )),
    }
}

fn create_batch_directory(root: &Path) -> Result<PathBuf, ApplicationError> {
    let timestamp = unix_time_millis()?;
    for suffix in 0..10_000 {
        let directory = root.join(format!("batch-{timestamp}-{suffix}"));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ApplicationError::new(
                    if error.kind() == std::io::ErrorKind::PermissionDenied {
                        ApplicationErrorKind::PermissionDenied
                    } else {
                        ApplicationErrorKind::Persistence
                    },
                ));
            }
        }
    }
    Err(ApplicationError::new(ApplicationErrorKind::Capacity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_discovery_has_feedback_and_cancels_before_any_admission() {
        let progress = ChannelBatchPreparation::default();
        let worker_progress = progress.clone();
        let (entered, entering) = mpsc::sync_channel(1);
        let (release, releasing) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            collect_candidates(
                &ChannelBatchFilter {
                    account_id: 1,
                    chat_id: 2,
                    earliest_unix_ms: None,
                    latest_unix_ms: 10,
                    kinds: vec![],
                },
                &worker_progress,
                |_| {
                    entered.send(()).expect("entered");
                    releasing.recv().expect("release");
                    Ok(vec![])
                },
            )
        });
        entering
            .recv_timeout(Duration::from_secs(2))
            .expect("blocked read");
        assert_eq!(
            progress.snapshot().expect("visible").phase,
            ChannelBatchPreparationPhase::Discovering
        );
        progress.cancel();
        assert_eq!(
            progress.snapshot().expect("cancel feedback").phase,
            ChannelBatchPreparationPhase::Cancelled
        );
        release.send(()).expect("release");
        assert_eq!(
            worker
                .join()
                .expect("retained owner")
                .expect_err("cancelled")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        assert_eq!(progress.snapshot().expect("timeline").events.len(), 2);
    }

    #[test]
    fn cancel_during_folder_preparation_cannot_cross_admission() {
        let progress = ChannelBatchPreparation::default();
        progress
            .update(ChannelBatchPreparationPhase::PreparingFolder, 500, 12)
            .expect("phase");
        progress.cancel();
        assert_eq!(
            progress
                .update(ChannelBatchPreparationPhase::Queuing, 500, 12)
                .expect_err("cancel barrier")
                .kind(),
            ApplicationErrorKind::Cancelled
        );
        let admitted = ChannelBatchPreparation::default();
        admitted
            .update(ChannelBatchPreparationPhase::Queuing, 500, 12)
            .expect("admission");
        admitted.cancel();
        assert_eq!(
            admitted.snapshot().expect("state").phase,
            ChannelBatchPreparationPhase::Queuing,
            "durably admitted work uses Stop"
        );
    }

    #[test]
    fn reserved_destinations_cannot_collide_with_other_members_artifacts() {
        let root = tempfile::tempdir().expect("folder");
        let mut reserved = BTreeSet::new();
        for name in [
            "report",
            ".report.teleark-partial",
            ".report.teleark-partial.map",
            "report.partial",
        ] {
            let path =
                available_download_destination_with_reservations(root.path(), name, |candidate| {
                    Ok(destination_paths(candidate)?
                        .iter()
                        .any(|path| reserved.contains(path)))
                })
                .expect("safe destination");
            let paths = destination_paths(&path).expect("artifact footprint");
            assert!(paths.iter().all(|path| !reserved.contains(path)));
            reserved.extend(paths);
        }
        assert_eq!(reserved.len(), 16);
    }

    #[test]
    fn unique_private_folders_preserve_user_files_and_confine_basenames() {
        let directory = tempfile::tempdir().expect("folder");
        let existing = directory.path().join("existing.bin");
        std::fs::write(&existing, b"user bytes").expect("user file");
        let first = create_batch_directory(directory.path()).expect("first unique folder");
        let second = create_batch_directory(directory.path()).expect("second unique folder");
        assert_ne!(first, second);
        assert_eq!(std::fs::read(existing).expect("original"), b"user bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(first)
                    .expect("private folder")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert_eq!(safe_name("../nested/report.zip", 7), "report.zip");
        assert_eq!(safe_name("..", 7), "telegram-document-7");
    }
}
