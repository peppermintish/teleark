//! Cheap list identities and filters. Formatting belongs to visible rows only.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Default)]
pub(crate) struct TransferProjectionCache {
    native: Option<std::sync::Weak<[Arc<ChannelDownloadSnapshot>]>>,
    vault: Option<std::sync::Weak<[Arc<VaultTransferSnapshot>]>>,
    account: Option<i64>,
    items: Arc<Vec<TransferItem>>,
}

impl TransferProjectionCache {
    pub(super) fn get(
        &self,
        app: &TeleArkApp,
        account: Option<i64>,
    ) -> Option<Arc<Vec<TransferItem>>> {
        (self.account == account
            && self
                .native
                .as_ref()?
                .upgrade()
                .is_some_and(|items| Arc::ptr_eq(&items, &app.native_transfer_view.items))
            && self
                .vault
                .as_ref()?
                .upgrade()
                .is_some_and(|items| Arc::ptr_eq(&items, &app.vault_transfer_view.items)))
        .then(|| self.items.clone())
    }

    pub(super) fn set(
        &mut self,
        app: &TeleArkApp,
        account: Option<i64>,
        items: Arc<Vec<TransferItem>>,
    ) {
        self.native = Some(Arc::downgrade(&app.native_transfer_view.items));
        self.vault = Some(Arc::downgrade(&app.vault_transfer_view.items));
        self.account = account;
        self.items = items;
    }
}

#[cfg(test)]
thread_local! { static MATERIALIZED_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn reset_materialized_rows() {
    MATERIALIZED_ROWS.set(0);
}
#[cfg(test)]
pub(super) fn materialized_rows() -> usize {
    MATERIALIZED_ROWS.get()
}

#[derive(Clone)]
pub(super) enum TransferItem {
    Preview(Arc<TransferRow>),
    Native(Arc<ChannelDownloadSnapshot>, bool),
    NativeBatch(u64, Arc<[Arc<ChannelDownloadSnapshot>]>),
    Vault(Arc<VaultTransferSnapshot>, bool),
    VaultBatch(
        u64,
        Arc<VaultTransferSnapshot>,
        Arc<[Arc<VaultTransferSnapshot>]>,
    ),
}

impl TransferItem {
    pub(super) fn row(&self, app: &TeleArkApp) -> TransferRow {
        #[cfg(test)]
        MATERIALIZED_ROWS.set(MATERIALIZED_ROWS.get() + 1);
        let mut row = match self {
            Self::Preview(row) => (**row).clone(),
            Self::Native(row, child) => app.transfer_row_from_snapshot(row, *child),
            Self::NativeBatch(id, rows) => {
                app.transfer_row_from_batch(*id, &rows.iter().map(Arc::as_ref).collect::<Vec<_>>())
            }
            Self::Vault(row, child) => {
                let mut row = app.transfer_row_from_vault_snapshot(row);
                row.batch_child = *child;
                row
            }
            Self::VaultBatch(id, first, rows) => app
                .transfer_row_from_vault_batch(
                    *id,
                    &rows.iter().map(Arc::as_ref).collect::<Vec<_>>(),
                )
                .unwrap_or_else(|| app.transfer_row_from_vault_snapshot(first)),
        };
        if app.vault_locked && (row.vault_transfer_id.is_some() || row.vault_batch_id.is_some()) {
            row.name = app.tr("vault-locked-file");
            row.caption = None;
            row.destination = app.tr("vault-locked-detail");
            if let Some(batch) = row.batch_summary.as_mut() {
                batch.file_names.clear();
            }
        }
        row
    }

    pub(super) fn key(&self, index: usize) -> u64 {
        match self {
            Self::Preview(row) => transfer_selection_key(row, index),
            Self::Native(row, _) => row.id,
            Self::NativeBatch(id, _) => 0x4000_0000_0000_0000 | id,
            Self::Vault(row, _) => 0x2000_0000_0000_0000 | row.id,
            Self::VaultBatch(id, _, _) => vault_batch_key(*id),
        }
    }

    pub(super) fn state(&self) -> TransferState {
        match self {
            Self::Preview(row) => row.state,
            Self::Native(row, _) => transfer_state(row.state),
            Self::NativeBatch(_, rows) => {
                aggregate_download_states(rows.iter().map(|row| row.state))
            }
            Self::Vault(row, _) => vault_transfer_state(row),
            Self::VaultBatch(_, _, rows) => aggregate_transfer_states(
                &rows
                    .iter()
                    .map(|row| vault_transfer_state(row))
                    .collect::<Vec<_>>(),
            ),
        }
    }

    pub(super) fn direction(&self) -> TransferDirection {
        match self {
            Self::Preview(row) => row.direction,
            Self::Native(..) | Self::NativeBatch(..) => TransferDirection::Download,
            Self::Vault(row, _) => vault_direction(row.direction),
            Self::VaultBatch(_, first, rows) => {
                vault_direction(rows.first().unwrap_or(first).direction)
            }
        }
    }

    pub(super) fn child(&self) -> bool {
        match self {
            Self::Preview(row) => row.batch_child,
            Self::Native(_, child) | Self::Vault(_, child) => *child,
            _ => false,
        }
    }

    fn native_id(&self) -> Option<u64> {
        match self {
            Self::Native(row, _) => Some(row.id),
            Self::Preview(row) => row.runtime_task_id,
            _ => None,
        }
    }

    fn native_batch(&self) -> Option<u64> {
        match self {
            Self::Native(row, _) => row.batch_id,
            Self::NativeBatch(id, _) => Some(*id),
            Self::Preview(row) => row.runtime_batch_id,
            _ => None,
        }
    }

    fn vault_batch(&self) -> Option<u64> {
        match self {
            Self::Vault(row, _) => row.batch_id,
            Self::VaultBatch(id, _, _) => Some(*id),
            Self::Preview(row) => row.vault_batch_id,
            _ => None,
        }
    }

    pub(super) fn activity_detail(&self, app: &TeleArkApp) -> Option<SharedString> {
        let active = |row: &VaultTransferSnapshot| {
            matches!(
                row.state,
                VaultTransferState::Queued | VaultTransferState::Running
            )
        };
        match self {
            Self::Preview(row) => row.activity_detail.clone(),
            Self::Vault(row, _) if active(row) => row
                .upload_activity
                .as_ref()
                .map(|activity| app.upload_activity_detail(activity)),
            Self::VaultBatch(_, _, rows) => rows
                .iter()
                .find(|row| row.state == VaultTransferState::Running)
                .or_else(|| {
                    rows.iter().find(|row| {
                        row.state == VaultTransferState::Queued && row.upload_activity.is_some()
                    })
                })
                .and_then(|row| row.upload_activity.as_ref())
                .map(|activity| app.upload_activity_detail(activity)),
            _ => None,
        }
    }

    fn matches(&self, app: &TeleArkApp, terms: &SearchTerms) -> bool {
        let query = terms.query;
        if query.is_empty() {
            return true;
        }
        if app.vault_locked && matches!(self, Self::Vault(..) | Self::VaultBatch(..)) {
            return terms.vault_source
                || app.tr("vault-locked-file").to_lowercase().contains(query);
        }
        let source_matches = |chat| {
            terms.sources.get(&chat).copied().unwrap_or_else(|| {
                app.telegram_source_name(chat)
                    .to_lowercase()
                    .contains(query)
            })
        };
        match self {
            Self::Native(row, _) => {
                row.file_name.to_lowercase().contains(query)
                    || source_matches(row.chat_id)
                    || row
                        .destination
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(query)
            }
            Self::Vault(row, _) => {
                row.file_name.to_lowercase().contains(query)
                    || terms.vault_source
                    || match row.direction {
                        VaultTransferDirection::Upload => terms.vault_target,
                        VaultTransferDirection::Download => {
                            row.destination.as_ref().map_or(terms.unavailable, |path| {
                                path.to_string_lossy().to_lowercase().contains(query)
                            })
                        }
                    }
            }
            _ => {
                let row = self.row(app);
                row.name.to_lowercase().contains(query)
                    || row.source.to_lowercase().contains(query)
                    || row.destination.to_lowercase().contains(query)
            }
        }
    }
}

pub(super) fn vault_transfer_state(snapshot: &VaultTransferSnapshot) -> TransferState {
    match snapshot.state {
        VaultTransferState::Queued => TransferState::Waiting,
        VaultTransferState::Cancelled => TransferState::Cancelled,
        VaultTransferState::Running => match snapshot.direction {
            VaultTransferDirection::Upload => TransferState::Uploading,
            VaultTransferDirection::Download => TransferState::Downloading,
        },
        VaultTransferState::Completed => TransferState::Completed,
        VaultTransferState::Failed(_) | VaultTransferState::Interrupted => TransferState::Failed,
    }
}

fn vault_direction(direction: VaultTransferDirection) -> TransferDirection {
    match direction {
        VaultTransferDirection::Upload => TransferDirection::Upload,
        VaultTransferDirection::Download => TransferDirection::Download,
    }
}

struct SearchTerms<'a> {
    query: &'a str,
    sources: BTreeMap<i64, bool>,
    vault_source: bool,
    vault_target: bool,
    unavailable: bool,
}

pub(super) fn visible_items(
    items: &[TransferItem],
    app: &TeleArkApp,
    selection: &str,
    query: &str,
    expanded: &BTreeSet<u64>,
) -> Vec<TransferItem> {
    let reveal =
        !query.is_empty() || matches!(selection, "nav-waiting" | "nav-completed" | "nav-failed");
    let terms = SearchTerms {
        query,
        sources: if query.is_empty() {
            BTreeMap::new()
        } else {
            app.telegram_chats
                .iter()
                .filter(|chat| !chat.name.is_empty())
                .map(|chat| (chat.id, chat.name.to_lowercase().contains(query)))
                .collect()
        },
        vault_source: !query.is_empty()
            && app
                .tr("storage-channel-title")
                .to_lowercase()
                .contains(query),
        vault_target: !query.is_empty()
            && app
                .tr("transfer-vault-storage-channel")
                .to_lowercase()
                .contains(query),
        unavailable: !query.is_empty()
            && app
                .tr("transfer-value-unavailable")
                .to_lowercase()
                .contains(query),
    };
    items
        .iter()
        .filter(|item| {
            (!item.child()
                || reveal
                || item.native_batch().is_some_and(|id| expanded.contains(&id))
                || item
                    .vault_batch()
                    .is_some_and(|id| expanded.contains(&vault_batch_key(id))))
                && transfer_matches_nav(selection, item.state(), item.direction())
                && item.matches(app, &terms)
        })
        .cloned()
        .collect()
}

pub(super) fn scope_ids(
    items: &[TransferItem],
    selected: &BTreeSet<u64>,
    snapshots: &[(u64, Option<u64>)],
) -> BTreeSet<u64> {
    let has_selection = items
        .iter()
        .enumerate()
        .any(|(index, item)| selected.contains(&item.key(index)));
    let mut batches: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (id, batch) in snapshots {
        if let Some(batch) = batch {
            batches.entry(*batch).or_default().push(*id);
        }
    }
    let mut ids = BTreeSet::new();
    for (_, item) in items
        .iter()
        .enumerate()
        .filter(|(index, item)| !has_selection || selected.contains(&item.key(*index)))
    {
        if let Some(id) = item.native_id() {
            ids.insert(id);
        } else if let Some(batch) = item.native_batch()
            && let Some(members) = batches.get(&batch)
        {
            ids.extend(members);
        }
    }
    ids
}
