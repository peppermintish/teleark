use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use teleark_core::{
    AccountId, LogicalFileId, PartIndex, TransferDirection, TransferId, TransferPriority,
};

use crate::{ConfigurationError, TransferEngineError};

/// Validated scheduler and queue bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerConfig {
    max_active: usize,
    max_uploads: usize,
    max_downloads: usize,
    max_per_account: usize,
    max_per_file: usize,
    max_queued_parts: usize,
}

impl SchedulerConfig {
    pub fn new(
        max_active: usize,
        max_uploads: usize,
        max_downloads: usize,
        max_per_account: usize,
        max_per_file: usize,
        max_queued_parts: usize,
    ) -> Result<Self, ConfigurationError> {
        for (field, value) in [
            ("max_active", max_active),
            ("max_uploads", max_uploads),
            ("max_downloads", max_downloads),
            ("max_per_account", max_per_account),
            ("max_per_file", max_per_file),
            ("max_queued_parts", max_queued_parts),
        ] {
            if value == 0 {
                return Err(ConfigurationError::ZeroLimit { field });
            }
        }
        for (field, limit) in [
            ("max_uploads", max_uploads),
            ("max_downloads", max_downloads),
            ("max_per_account", max_per_account),
            ("max_per_file", max_per_file),
        ] {
            if limit > max_active {
                return Err(ConfigurationError::LimitExceedsGlobal {
                    field,
                    limit,
                    global: max_active,
                });
            }
        }
        Ok(Self {
            max_active,
            max_uploads,
            max_downloads,
            max_per_account,
            max_per_file,
            max_queued_parts,
        })
    }

    #[must_use]
    pub const fn max_active(self) -> usize {
        self.max_active
    }

    #[must_use]
    pub const fn max_per_file(self) -> usize {
        self.max_per_file
    }
}

/// One application-part scheduling request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkItem {
    pub transfer_id: TransferId,
    pub part_index: PartIndex,
    pub logical_file_id: LogicalFileId,
    pub account_id: AccountId,
    pub direction: TransferDirection,
    pub priority: TransferPriority,
    pub not_before_ms: u64,
}

impl WorkItem {
    const fn key(self) -> (TransferId, PartIndex) {
        (self.transfer_id, self.part_index)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct QueuedWork {
    item: WorkItem,
    sequence: u64,
}

/// Work holding every applicable scheduler permit until explicitly released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduledWork {
    pub item: WorkItem,
}

/// Read-only scheduler counts for diagnostics and deterministic tests.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SchedulerSnapshot {
    pub queued: usize,
    pub active: usize,
    pub active_uploads: usize,
    pub active_downloads: usize,
    pub active_by_account: BTreeMap<AccountId, usize>,
    pub active_by_file: BTreeMap<LogicalFileId, usize>,
}

/// Deterministic priority/FIFO scheduler with simultaneous permit checks.
pub struct TransferScheduler {
    config: SchedulerConfig,
    queue: Vec<QueuedWork>,
    active: BTreeMap<(TransferId, PartIndex), WorkItem>,
    blocked_accounts_until: BTreeMap<AccountId, u64>,
    suspended_transfers: BTreeSet<TransferId>,
    next_sequence: u64,
}

impl TransferScheduler {
    #[must_use]
    pub fn new(config: SchedulerConfig) -> Self {
        Self {
            config,
            queue: Vec::new(),
            active: BTreeMap::new(),
            blocked_accounts_until: BTreeMap::new(),
            suspended_transfers: BTreeSet::new(),
            next_sequence: 0,
        }
    }

    pub fn enqueue(&mut self, item: WorkItem) -> Result<(), TransferEngineError> {
        self.enqueue_batch(&[item])
    }

    /// Applies controller-selected limits without interrupting already active
    /// work. A lower bound takes effect for the next dispatch; active permits
    /// drain naturally and are never revoked in the middle of an RPC.
    pub fn reconfigure(&mut self, config: SchedulerConfig) -> Result<(), TransferEngineError> {
        if self.queue.len() > config.max_queued_parts {
            return Err(TransferEngineError::QueueFull {
                limit: config.max_queued_parts,
            });
        }
        self.config = config;
        Ok(())
    }

    /// Atomically add a batch or reject it without partially changing the queue.
    pub fn enqueue_batch(&mut self, items: &[WorkItem]) -> Result<(), TransferEngineError> {
        let new_length = self.queue.len().checked_add(items.len()).ok_or(
            TransferEngineError::ArithmeticOverflow {
                field: "scheduler queue length",
            },
        )?;
        if new_length > self.config.max_queued_parts {
            return Err(TransferEngineError::QueueFull {
                limit: self.config.max_queued_parts,
            });
        }
        let mut batch_keys = BTreeSet::new();
        for item in items {
            let key = item.key();
            if !batch_keys.insert(key)
                || self.active.contains_key(&key)
                || self.queue.iter().any(|queued| queued.item.key() == key)
            {
                return Err(TransferEngineError::DuplicateWork {
                    transfer_id: item.transfer_id,
                    part_index: item.part_index,
                });
            }
        }
        let final_sequence = self.next_sequence.checked_add(items.len() as u64).ok_or(
            TransferEngineError::ArithmeticOverflow {
                field: "scheduler FIFO sequence",
            },
        )?;
        for item in items {
            self.queue.push(QueuedWork {
                item: *item,
                sequence: self.next_sequence,
            });
            self.next_sequence += 1;
        }
        debug_assert_eq!(self.next_sequence, final_sequence);
        Ok(())
    }

    /// Select as many eligible parts as all configured limits allow.
    pub fn dispatch_ready(&mut self, now_ms: u64) -> Vec<ScheduledWork> {
        self.blocked_accounts_until
            .retain(|_, deadline| *deadline > now_ms);
        let mut selected = Vec::new();
        while self.active.len() < self.config.max_active {
            let candidate = self
                .queue
                .iter()
                .enumerate()
                .filter(|(_, queued)| self.eligible(queued.item, now_ms))
                .min_by(|(_, left), (_, right)| queue_order(left, right))
                .map(|(index, _)| index);
            let Some(index) = candidate else {
                break;
            };
            let queued = self.queue.remove(index);
            self.active.insert(queued.item.key(), queued.item);
            selected.push(ScheduledWork { item: queued.item });
        }
        selected
    }

    pub fn release(&mut self, work: ScheduledWork) -> Result<(), TransferEngineError> {
        if self.active.remove(&work.item.key()).is_some() {
            Ok(())
        } else {
            Err(TransferEngineError::DuplicateWork {
                transfer_id: work.item.transfer_id,
                part_index: work.item.part_index,
            })
        }
    }

    pub fn requeue(
        &mut self,
        mut work: ScheduledWork,
        not_before_ms: u64,
    ) -> Result<(), TransferEngineError> {
        self.release(work)?;
        work.item.not_before_ms = not_before_ms;
        self.enqueue(work.item)
    }

    pub fn block_account_until(&mut self, account_id: AccountId, deadline_ms: u64) {
        self.blocked_accounts_until
            .entry(account_id)
            .and_modify(|existing| *existing = (*existing).max(deadline_ms))
            .or_insert(deadline_ms);
    }

    pub(crate) fn account_blocked_until(&self, account_id: AccountId, now_ms: u64) -> Option<u64> {
        self.blocked_accounts_until
            .get(&account_id)
            .copied()
            .filter(|deadline| *deadline > now_ms)
    }

    pub fn set_suspended(&mut self, transfer_id: TransferId, suspended: bool) {
        if suspended {
            self.suspended_transfers.insert(transfer_id);
        } else {
            self.suspended_transfers.remove(&transfer_id);
        }
    }

    pub fn remove_transfer(&mut self, transfer_id: TransferId) {
        self.queue
            .retain(|queued| queued.item.transfer_id != transfer_id);
        self.suspended_transfers.remove(&transfer_id);
    }

    pub fn set_priority(&mut self, transfer_id: TransferId, priority: TransferPriority) {
        for queued in &mut self.queue {
            if queued.item.transfer_id == transfer_id {
                queued.item.priority = priority;
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> SchedulerSnapshot {
        let mut snapshot = SchedulerSnapshot {
            queued: self.queue.len(),
            active: self.active.len(),
            ..SchedulerSnapshot::default()
        };
        for item in self.active.values() {
            match item.direction {
                TransferDirection::Upload => snapshot.active_uploads += 1,
                TransferDirection::Download => snapshot.active_downloads += 1,
            }
            *snapshot
                .active_by_account
                .entry(item.account_id)
                .or_default() += 1;
            *snapshot
                .active_by_file
                .entry(item.logical_file_id)
                .or_default() += 1;
        }
        snapshot
    }

    fn eligible(&self, item: WorkItem, now_ms: u64) -> bool {
        if item.not_before_ms > now_ms
            || self.suspended_transfers.contains(&item.transfer_id)
            || self
                .blocked_accounts_until
                .get(&item.account_id)
                .is_some_and(|deadline| *deadline > now_ms)
        {
            return false;
        }
        let snapshot = self.snapshot();
        let direction_available = match item.direction {
            TransferDirection::Upload => snapshot.active_uploads < self.config.max_uploads,
            TransferDirection::Download => snapshot.active_downloads < self.config.max_downloads,
        };
        direction_available
            && snapshot
                .active_by_account
                .get(&item.account_id)
                .copied()
                .unwrap_or_default()
                < self.config.max_per_account
            && snapshot
                .active_by_file
                .get(&item.logical_file_id)
                .copied()
                .unwrap_or_default()
                < self.config.max_per_file
    }
}

fn queue_order(left: &QueuedWork, right: &QueuedWork) -> Ordering {
    right
        .item
        .priority
        .cmp(&left.item.priority)
        .then_with(|| left.sequence.cmp(&right.sequence))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SchedulerConfig {
        match SchedulerConfig::new(4, 2, 3, 2, 1, 32) {
            Ok(value) => value,
            Err(error) => panic!("test scheduler config failed: {error}"),
        }
    }

    fn item(
        transfer: u64,
        part: u32,
        file: u64,
        account: i64,
        direction: TransferDirection,
        priority: i16,
    ) -> WorkItem {
        WorkItem {
            transfer_id: TransferId::new(transfer),
            part_index: PartIndex::new(part),
            logical_file_id: LogicalFileId::new(file),
            account_id: AccountId::new(account),
            direction,
            priority: TransferPriority::new(priority),
            not_before_ms: 0,
        }
    }

    #[test]
    fn all_concurrency_limits_are_enforced_together() {
        let mut scheduler = TransferScheduler::new(config());
        let items = [
            item(1, 0, 1, 1, TransferDirection::Upload, 0),
            item(1, 1, 1, 1, TransferDirection::Upload, 0),
            item(2, 0, 2, 1, TransferDirection::Upload, 0),
            item(3, 0, 3, 2, TransferDirection::Upload, 0),
            item(4, 0, 4, 2, TransferDirection::Download, 0),
            item(5, 0, 5, 3, TransferDirection::Download, 0),
        ];
        assert!(scheduler.enqueue_batch(&items).is_ok());
        let active = scheduler.dispatch_ready(0);
        let snapshot = scheduler.snapshot();
        assert_eq!(active.len(), 4);
        assert_eq!(snapshot.active, 4);
        assert!(snapshot.active_uploads <= 2);
        assert!(snapshot.active_downloads <= 3);
        assert!(snapshot.active_by_account.values().all(|count| *count <= 2));
        assert!(snapshot.active_by_file.values().all(|count| *count <= 1));
    }

    #[test]
    fn priority_then_fifo_is_deterministic() {
        let mut scheduler = TransferScheduler::new(match SchedulerConfig::new(1, 1, 1, 1, 1, 8) {
            Ok(value) => value,
            Err(error) => panic!("test scheduler config failed: {error}"),
        });
        assert!(
            scheduler
                .enqueue(item(1, 0, 1, 1, TransferDirection::Upload, -1))
                .is_ok()
        );
        assert!(
            scheduler
                .enqueue(item(2, 0, 2, 2, TransferDirection::Upload, 5))
                .is_ok()
        );
        assert!(
            scheduler
                .enqueue(item(3, 0, 3, 3, TransferDirection::Upload, 5))
                .is_ok()
        );

        let first = scheduler.dispatch_ready(0)[0];
        assert_eq!(first.item.transfer_id, TransferId::new(2));
        assert!(scheduler.release(first).is_ok());
        let second = scheduler.dispatch_ready(0)[0];
        assert_eq!(second.item.transfer_id, TransferId::new(3));
        assert!(scheduler.release(second).is_ok());
        let third = scheduler.dispatch_ready(0)[0];
        assert_eq!(third.item.transfer_id, TransferId::new(1));
    }

    #[test]
    fn account_flood_deadline_and_task_suspension_are_scoped() {
        let mut scheduler = TransferScheduler::new(config());
        assert!(
            scheduler
                .enqueue(item(1, 0, 1, 1, TransferDirection::Upload, 9))
                .is_ok()
        );
        assert!(
            scheduler
                .enqueue(item(2, 0, 2, 2, TransferDirection::Upload, 0))
                .is_ok()
        );
        scheduler.block_account_until(AccountId::new(1), 50);
        scheduler.set_suspended(TransferId::new(2), true);
        assert!(scheduler.dispatch_ready(49).is_empty());
        let work = scheduler.dispatch_ready(50);
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].item.transfer_id, TransferId::new(1));
    }
}
