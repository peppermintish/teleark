//! A completed transfer stays historical; missing outputs get a new batch.
use super::*;

impl DesktopTransfers {
    /// Background-only: recheck completed outputs and queue only confirmed missing
    /// files. Preserve source batch boundaries, history and every existing file.
    /// Cancellation is checked between preparation steps and before admission.
    pub fn redownload_missing_completed(
        &self,
        ids: &[u64],
        cancelled: &AtomicBool,
    ) -> Result<Vec<u64>, ApplicationError> {
        self.redownload_missing_with_probe(ids, cancelled, &crate::local_file_presence)
    }

    pub(super) fn redownload_missing_with_probe(
        &self,
        ids: &[u64],
        cancelled: &AtomicBool,
        probe: &impl Fn(&Path, u64) -> crate::LocalFilePresence,
    ) -> Result<Vec<u64>, ApplicationError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        validate_batch_size(ids.len())?;
        let check_cancelled = || {
            if cancelled.load(Ordering::Acquire) {
                Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
            } else {
                Ok(())
            }
        };
        let mut groups = BTreeMap::<(i64, Option<u64>), Vec<ChannelDownloadRequest>>::new();
        // Validate the entire selection before admitting any replacement work.
        for id in ids.iter().copied().collect::<BTreeSet<_>>() {
            check_cancelled()?;
            let snapshot = match self.inner.snapshots.get(id) {
                Some(snapshot) => snapshot,
                None => self
                    .inner
                    .library
                    .native_download(id)?
                    .map(snapshot_from_record)
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::NotFound))?,
            };
            self.require_active_account(snapshot.account_id)?;
            if snapshot.state != ChannelDownloadState::Completed {
                return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
            }
            groups
                .entry((snapshot.chat_id, snapshot.batch_id))
                .or_default()
                .push(ChannelDownloadRequest {
                    account_id: snapshot.account_id.expect("authorized account"),
                    chat_id: snapshot.chat_id,
                    message_id: snapshot.message_id,
                    message_sent_at_unix_ms: snapshot.message_sent_at_unix_ms,
                    file_name: snapshot.file_name,
                    caption: snapshot.caption,
                    mime_type: snapshot.mime_type,
                    size_bytes: snapshot.size_bytes,
                    destination: snapshot.destination,
                });
        }
        let mut batches = Vec::new();
        for sources in groups.into_values() {
            let mut destinations = Vec::new();
            let result = (|| {
                let mut requests = Vec::new();
                for mut request in sources {
                    check_cancelled()?;
                    self.require_active_account(Some(request.account_id))?;
                    let presence = probe(&request.destination, request.size_bytes);
                    check_cancelled()?;
                    self.require_active_account(Some(request.account_id))?;
                    if presence != crate::LocalFilePresence::Missing {
                        continue;
                    }
                    let destination = self
                        .inner
                        .library
                        .next_download_destination(&request.file_name)?;
                    destinations.push(destination.clone());
                    request.destination = destination;
                    requests.push(request);
                }
                check_cancelled()?;
                if requests.is_empty() {
                    return Ok(None);
                }
                self.enqueue_channel_download_batch(requests).map(Some)
            })();
            if result.is_err() {
                // Admission may fail before persistence or after some workers start.
                // Release only our unclaimed empty markers, never a durable task's.
                for destination in destinations {
                    if !self
                        .inner
                        .library
                        .worker
                        .native_download_destination_in_use(destination.clone())?
                    {
                        reservation::release_cancelled(&destination)?;
                    }
                }
            }
            if let Some(batch) = result? {
                batches.push(batch);
            }
        }
        Ok(batches)
    }
}
