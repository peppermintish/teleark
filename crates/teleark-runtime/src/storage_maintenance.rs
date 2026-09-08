//! Bounded presentation state shared by the maintenance owner and frontend.
use crate::{StorageMaintenancePhase, TelegramScanCancellation, map_storage_error};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};
use teleark_core::{ApplicationError, ApplicationErrorKind};
use teleark_storage::{Database, SettingRecord};

#[derive(Clone, Debug)]
pub struct StorageMaintenanceSnapshot {
    pub phase: StorageMaintenancePhase,
    pub started: Instant,
    pub phase_since: Instant,
    pub last_activity: Instant,
    pub timeline: VecDeque<(StorageMaintenancePhase, u64)>,
    pub omitted: u64,
    pub finished: bool,
    pub error: Option<ApplicationErrorKind>,
}
#[derive(Clone)]
pub struct StorageMaintenance {
    inner: Arc<Mutex<StorageMaintenanceSnapshot>>,
    pub(crate) cancellation: TelegramScanCancellation,
}
impl Default for StorageMaintenance {
    fn default() -> Self {
        Self::new()
    }
}
impl StorageMaintenance {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            inner: Arc::new(Mutex::new(StorageMaintenanceSnapshot {
                phase: StorageMaintenancePhase::Checking,
                started: now,
                phase_since: now,
                last_activity: now,
                timeline: VecDeque::from([(StorageMaintenancePhase::Checking, 0)]),
                omitted: 0,
                finished: false,
                error: None,
            })),
            cancellation: TelegramScanCancellation::new(),
        }
    }
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub fn snapshot(&self) -> StorageMaintenanceSnapshot {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub(crate) fn phase(&self, phase: StorageMaintenancePhase) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.finished {
            return;
        }
        let now = Instant::now();
        state.last_activity = now;
        if state.phase != phase {
            state.phase = phase;
            state.phase_since = now;
            let millis = state
                .started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            if state.timeline.len() == 32 {
                state.timeline.pop_front();
                state.omitted += 1;
            }
            state.timeline.push_back((phase, millis));
        }
    }
    pub(crate) fn finish(&self, error: Option<ApplicationErrorKind>) {
        if error.is_none() {
            self.phase(StorageMaintenancePhase::Completed);
        }
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.error = error;
        state.finished = true;
        state.last_activity = Instant::now();
    }
}

pub(crate) fn repair_token(
    database: &mut Database,
    account: i64,
    chat: i64,
    proposed: i64,
    clear: bool,
) -> Result<i64, ApplicationError> {
    if account <= 0 || chat <= 0 || proposed == 0 {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    if crate::storage_channel::load_binding(database, account)? != Some(chat) {
        return Err(ApplicationError::new(
            ApplicationErrorKind::StorageAccessDenied,
        ));
    }
    let key = format!("storage-repair.v1.{account}.{chat}");
    if clear {
        database.delete_setting(&key).map_err(map_storage_error)?;
        return Ok(proposed);
    }
    if let Some(record) = database.setting(&key).map_err(map_storage_error)? {
        return record
            .value
            .parse::<i64>()
            .ok()
            .filter(|id| *id != 0)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence));
    }
    database
        .set_setting(&SettingRecord {
            key,
            value: proposed.to_string(),
            updated_at_unix_ms: 0,
        })
        .map_err(map_storage_error)?;
    Ok(proposed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_reuses_send_token_and_never_changes_binding() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("db");
        {
            let mut db = Database::open(&path).expect("database");
            crate::storage_channel::save_binding(&mut db, 1, 2).expect("bind");
            assert_eq!(repair_token(&mut db, 1, 2, 3, false).expect("token"), 3);
        }
        let mut db = Database::open(&path).expect("reopen");
        assert_eq!(
            repair_token(&mut db, 1, 2, 4, false).expect("same token"),
            3
        );
        assert!(repair_token(&mut db, 1, 9, 4, false).is_err());
        repair_token(&mut db, 1, 2, 3, true).expect("complete");
        assert_eq!(
            repair_token(&mut db, 1, 2, 4, false).expect("new repair"),
            4
        );
    }
    #[test]
    fn timeline_retains_terminal_state_and_discloses_omissions() {
        let progress = StorageMaintenance::new();
        for _ in 0..40 {
            progress.phase(StorageMaintenancePhase::Repairing);
            progress.phase(StorageMaintenancePhase::Verifying);
        }
        progress.finish(Some(ApplicationErrorKind::Network));
        progress.phase(StorageMaintenancePhase::Completed);
        let state = progress.snapshot();
        assert!(state.finished);
        assert!(state.omitted > 0);
        assert_eq!(state.timeline.len(), 32);
        assert_eq!(state.error, Some(ApplicationErrorKind::Network));
        assert_eq!(state.phase, StorageMaintenancePhase::Verifying);
    }
}
