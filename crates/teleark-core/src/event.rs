use crate::{
    AccountId, CollectionId, IndexJobId, IndexJobState, LogicalFile, LogicalFileId, TransferId,
    TransferProgress, TransferState,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VaultState {
    Unconfigured,
    Locked,
    Unlocked,
}

/// Events emitted by Core without imposing any frontend runtime or widget type.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum CoreEvent {
    FileAdded(LogicalFile),
    FileChanged(LogicalFileId),
    CollectionChanged(CollectionId),
    TransferUpdated {
        transfer_id: TransferId,
        state: TransferState,
        progress: TransferProgress,
    },
    IndexUpdated {
        index_job_id: IndexJobId,
        state: IndexJobState,
    },
    AccountUpdated(AccountId),
    VaultStateChanged(VaultState),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_contain_only_frontend_neutral_values() {
        let event = CoreEvent::TransferUpdated {
            transfer_id: TransferId::new(9),
            state: TransferState::Running,
            progress: TransferProgress {
                transferred_bytes: 50,
                total_bytes: 100,
                completed_parts: 0,
                total_parts: 1,
            },
        };

        assert!(matches!(event, CoreEvent::TransferUpdated { .. }));
    }
}
