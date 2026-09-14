//! Bounded, key-free projection of saved encrypted download jobs.
use super::*;
use teleark_storage::{Database, VaultJobDirection, VaultJobRecord, VaultJobState};

impl VaultOwner {
    pub(super) fn download_history(
        &self,
        database: &Database,
        account: i64,
    ) -> Result<(Vec<VaultTransferSnapshot>, u64), ApplicationError> {
        let ids = database
            .vault_job_history_ids(account, VaultJobDirection::Download, 256)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let count = database
            .vault_job_count_by_direction(account, VaultJobDirection::Download)
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Persistence))?;
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            let record = match database.vault_job(account, id) {
                Ok(Some(record)) => Some(record),
                Ok(None) => continue,
                Err(teleark_storage::StorageError::CorruptData { .. }) => None,
                Err(_) => return Err(ApplicationError::new(ApplicationErrorKind::Persistence)),
            };
            rows.push(download_snapshot(account, id, record.as_ref())?);
        }
        let omitted = count.saturating_sub(rows.len() as u64);
        Ok((rows, omitted))
    }
}

fn download_snapshot(
    account: i64,
    id: u64,
    record: Option<&VaultJobRecord>,
) -> Result<VaultTransferSnapshot, ApplicationError> {
    let context = record.and_then(|record| crate::VaultRecoveryContext::from_record(record).ok());
    let valid = context.as_ref().filter(|context| {
        matches!(
            context.direction,
            crate::VaultRecoveryDirection::Download { .. }
        )
    });
    let saved = record.filter(|_| valid.is_some());
    let state = saved.map_or(
        VaultTransferState::Failed(ApplicationErrorKind::InvalidRequest),
        |record| match record.state {
            VaultJobState::Queued => VaultTransferState::Queued,
            VaultJobState::Running => VaultTransferState::Running,
            VaultJobState::Pausing => VaultTransferState::Pausing,
            VaultJobState::Paused => VaultTransferState::Paused,
            VaultJobState::Cancelling => VaultTransferState::Cancelling,
            VaultJobState::Cancelled => VaultTransferState::Cancelled,
            VaultJobState::Completed => VaultTransferState::Completed,
            VaultJobState::Retryable | VaultJobState::Blocked => VaultTransferState::Failed(
                record
                    .failure_code
                    .as_deref()
                    .and_then(upload_history::parse_error)
                    .unwrap_or(ApplicationErrorKind::Persistence),
            ),
        },
    );
    let size = valid.map_or(0, |context| context.size_bytes);
    Ok(VaultTransferSnapshot {
        id,
        account_id: account,
        chat_id: record.map_or(0, |record| record.chat_id),
        recovery_state: saved.map(|record| record.state),
        restored: true,
        upload_activity: None,
        batch_id: None,
        queued_at_unix_ms: record.map_or(0, |record| record.created_at_unix_ms),
        direction: VaultTransferDirection::Download,
        file_name: valid.map_or_else(String::new, |context| context.file_name.clone()),
        package_id: record.map(|record| hex_id(&record.package_id)),
        size_bytes: size,
        transferred_bytes: if state == VaultTransferState::Completed {
            size
        } else {
            0
        },
        completed_parts: 0,
        part_count: 0,
        started_at_unix_ms: record.map_or(0, |record| record.updated_at_unix_ms),
        duration_ms: None,
        average_bytes_per_second: None,
        destination: valid.and_then(|context| match &context.direction {
            crate::VaultRecoveryDirection::Download { destination, .. } => {
                Some(destination.clone())
            }
            _ => None,
        }),
        session_log_path: None,
        telemetry: transfer_controller(false, 0, teleark_telegram::TransferTuning::default())?
            .snapshot(),
        state,
    })
}
