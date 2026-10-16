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
    presentation: Option<std::rc::Rc<Presentation>>,
    batch_retries: Option<BatchRetries>,
}

struct BatchRetries {
    native: std::sync::Weak<[Arc<ChannelDownloadSnapshot>]>,
    local_revision: u64,
    account: Option<i64>,
    ids: BTreeMap<u64, Arc<Vec<u64>>>,
}

impl TransferProjectionCache {
    pub(super) fn batch_retry_ids(&mut self, app: &TeleArkApp, batch: u64) -> Arc<Vec<u64>> {
        let account = app.telegram_account.as_ref().map(|account| account.id);
        let valid = self.batch_retries.as_ref().is_some_and(|cached| {
            cached.account == account
                && cached.local_revision == app.local_downloads.revision()
                && cached
                    .native
                    .upgrade()
                    .is_some_and(|items| Arc::ptr_eq(&items, &app.native_transfer_view.items))
        });
        if !valid {
            let mut ids = BTreeMap::<u64, Arc<Vec<u64>>>::new();
            for row in app.native_transfer_view.items.iter() {
                if let Some(batch) = row.batch_id
                    && row.account_id == account
                    && app.native_action_supported(TransferAction::Retry, row)
                {
                    Arc::make_mut(ids.entry(batch).or_default()).push(row.id);
                }
            }
            self.batch_retries = Some(BatchRetries {
                native: Arc::downgrade(&app.native_transfer_view.items),
                local_revision: app.local_downloads.revision(),
                account,
                ids,
            });
        }
        self.batch_retries
            .as_ref()
            .and_then(|cached| cached.ids.get(&batch))
            .cloned()
            .unwrap_or_default()
    }

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

pub(super) struct Presentation {
    source: Arc<Vec<TransferItem>>,
    local_revision: u64,
    selection: &'static str,
    query: String,
    expanded: BTreeSet<u64>,
    selected: BTreeSet<u64>,
    pub stats: [usize; 5],
    pub activity_row: Option<TransferItem>,
    pub rows: Arc<Vec<TransferItem>>,
    pub keys: Arc<Vec<u64>>,
    pub key_indices: BTreeMap<u64, usize>,
    pub selected_count: usize,
    pub actions: BTreeMap<usize, Arc<Vec<u64>>>,
    pub vault_actions: BTreeMap<usize, Arc<Vec<u64>>>,
    pub scroll_applied: std::cell::Cell<bool>,
}
impl TransferProjectionCache {
    pub(super) fn presentation(
        &mut self,
        app: &TeleArkApp,
        source: Arc<Vec<TransferItem>>,
        query: &str,
    ) -> std::rc::Rc<Presentation> {
        if let Some(cached) = &self.presentation
            && Arc::ptr_eq(&cached.source, &source)
            && cached.local_revision == app.local_downloads.revision()
            && cached.selection == app.nav_selection
            && cached.query == query
            && cached.expanded == app.expanded_transfer_batches
            && cached.selected == app.selected_transfer_keys
        {
            return cached.clone();
        }
        let mut stats = [0; 5];
        for item in source.iter().filter(|item| !item.child()) {
            let index = match item.state() {
                TransferState::Uploading => 0,
                TransferState::Downloading => 1,
                TransferState::Waiting | TransferState::Paused => 2,
                TransferState::Completed => 3,
                TransferState::Failed | TransferState::Cancelled => 4,
            };
            stats[index] += 1;
        }
        let activity_row = source
            .iter()
            .find(|item| {
                !item.child()
                    && item.direction() == TransferDirection::Upload
                    && item.activity_detail(app).is_some()
            })
            .cloned();
        let rows = Arc::new(visible_items(
            &source,
            app,
            app.nav_selection,
            query,
            &app.expanded_transfer_batches,
        ));
        let keys = Arc::new(
            rows.iter()
                .enumerate()
                .map(|(i, row)| row.key(i))
                .collect::<Vec<_>>(),
        );
        let key_indices = keys.iter().enumerate().map(|(i, key)| (*key, i)).collect();
        let selected_count = rows
            .iter()
            .enumerate()
            .filter(|(index, item)| item.has_selection(*index, &app.selected_transfer_keys))
            .count();
        let snapshots = &app.native_transfer_view.items;
        let batches = snapshots
            .iter()
            .map(|row| (row.id, row.batch_id))
            .collect::<Vec<_>>();
        let scoped = scope_ids(&rows, &app.selected_transfer_keys, &batches);
        let actions = [
            TransferAction::Resume,
            TransferAction::Pause,
            TransferAction::Retry,
            TransferAction::Cancel,
            TransferAction::Delete,
        ]
        .into_iter()
        .map(|action| {
            let ids = snapshots
                .iter()
                .filter(|row| scoped.contains(&row.id) && app.native_action_supported(action, row))
                .map(|row| row.id)
                .collect();
            (action as usize, Arc::new(ids))
        })
        .collect();
        let vault_snapshots = &app.vault_transfer_view.items;
        let vault_batches = vault_snapshots
            .iter()
            .map(|row| (row.id, row.batch_id))
            .collect::<Vec<_>>();
        let vault_scoped = scope_vault_ids(&rows, &app.selected_transfer_keys, &vault_batches);
        let vault_actions = [
            TransferAction::Resume,
            TransferAction::Pause,
            TransferAction::Retry,
            TransferAction::Cancel,
            TransferAction::Delete,
        ]
        .into_iter()
        .map(|action| {
            let ids = vault_snapshots
                .iter()
                .filter(|row| vault_scoped.contains(&row.id) && vault_action_supported(action, row))
                .map(|row| row.id)
                .collect();
            (action as usize, Arc::new(ids))
        })
        .collect();
        let result = std::rc::Rc::new(Presentation {
            source,
            local_revision: app.local_downloads.revision(),
            selection: app.nav_selection,
            query: query.into(),
            expanded: app.expanded_transfer_batches.clone(),
            selected: app.selected_transfer_keys.clone(),
            stats,
            activity_row,
            rows,
            keys,
            key_indices,
            selected_count,
            actions,
            vault_actions,
            scroll_applied: std::cell::Cell::new(false),
        });
        self.presentation = Some(result.clone());
        result
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
    fn has_selection(&self, index: usize, selected: &BTreeSet<u64>) -> bool {
        selected.contains(&self.key(index))
            || match self {
                Self::NativeBatch(_, rows) => rows.iter().any(|row| selected.contains(&row.id)),
                Self::VaultBatch(_, _, rows) => rows
                    .iter()
                    .any(|row| selected.contains(&vault_transfer_selection_key(row.id))),
                _ => false,
            }
    }
    pub(super) fn row(&self, app: &TeleArkApp) -> TransferRow {
        #[cfg(test)]
        MATERIALIZED_ROWS.set(MATERIALIZED_ROWS.get() + 1);
        match self {
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
        }
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
            Self::Native(row, _) => transfer_state(native_display_state(row)),
            Self::NativeBatch(_, rows) => {
                aggregate_download_states(rows.iter().map(|row| native_display_state(row)))
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

    pub(super) fn parent_key(&self) -> Option<u64> {
        self.native_batch()
            .or_else(|| self.vault_batch().map(vault_batch_key))
    }

    pub(super) fn batch_count(&self) -> usize {
        match self {
            Self::NativeBatch(_, rows) => rows.len(),
            Self::VaultBatch(_, _, rows) => rows.len(),
            Self::Preview(row) => row.batch_summary.as_ref().map_or(0, |batch| batch.total),
            _ => 0,
        }
    }

    fn native_id(&self) -> Option<u64> {
        match self {
            Self::Native(row, _) => Some(row.id),
            Self::Preview(row) => row.runtime_task_id,
            _ => None,
        }
    }

    fn vault_id(&self) -> Option<u64> {
        match self {
            Self::Vault(row, _) => Some(row.id),
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
            Self::Native(row, _) => row
                .cleanup
                .map(|cleanup| app.native_cleanup_detail(cleanup)),
            Self::NativeBatch(_, rows) => rows
                .iter()
                .find_map(|row| row.cleanup)
                .map(|cleanup| app.native_cleanup_detail(cleanup)),
            Self::Vault(row, _) if active(row) => app
                .vault_transfer_view
                .updates
                .get(&row.id)
                .unwrap_or(row)
                .upload_activity
                .as_ref()
                .map(|activity| app.upload_activity_detail(activity, row.direction)),
            Self::VaultBatch(_, _, rows) => rows
                .iter()
                .map(|row| app.vault_transfer_view.updates.get(&row.id).unwrap_or(row))
                .find(|row| row.state == VaultTransferState::Running)
                .or_else(|| {
                    rows.iter().find(|row| {
                        row.state == VaultTransferState::Queued && row.upload_activity.is_some()
                    })
                })
                .and_then(|row| {
                    row.upload_activity
                        .as_ref()
                        .map(|activity| app.upload_activity_detail(activity, row.direction))
                }),
            _ => None,
        }
    }

    fn matches(&self, app: &TeleArkApp, terms: &SearchTerms) -> bool {
        let query = terms.query;
        if query.is_empty() {
            return true;
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
        VaultTransferState::Queued
        | VaultTransferState::Pausing
        | VaultTransferState::Cancelling => TransferState::Waiting,
        VaultTransferState::Paused => TransferState::Paused,
        VaultTransferState::Cancelled => TransferState::Cancelled,
        VaultTransferState::Running => match snapshot.direction {
            VaultTransferDirection::Upload => TransferState::Uploading,
            VaultTransferDirection::Download => TransferState::Downloading,
        },
        VaultTransferState::Completed => TransferState::Completed,
        VaultTransferState::Failed(_) | VaultTransferState::Interrupted => TransferState::Failed,
    }
}

pub(super) fn vault_action_supported(
    action: TransferAction,
    snapshot: &VaultTransferSnapshot,
) -> bool {
    use teleark_runtime::VaultRecoveryState as Recovery;
    match action {
        TransferAction::Pause => matches!(
            snapshot.recovery_state,
            Some(Recovery::Queued | Recovery::Running)
        ),
        TransferAction::Resume => snapshot.recovery_state == Some(Recovery::Paused),
        TransferAction::Retry => snapshot.recovery_state == Some(Recovery::Retryable),
        TransferAction::Cancel => matches!(
            snapshot.recovery_state,
            Some(
                Recovery::Queued
                    | Recovery::Running
                    | Recovery::Pausing
                    | Recovery::Paused
                    | Recovery::Retryable
                    | Recovery::Blocked
            )
        ),
        TransferAction::Delete => matches!(
            snapshot.state,
            VaultTransferState::Completed
                | VaultTransferState::Failed(_)
                | VaultTransferState::Cancelled
                | VaultTransferState::Interrupted
        ),
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
    let mut batches: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (id, batch) in snapshots {
        if let Some(batch) = batch {
            batches.entry(*batch).or_default().push(*id);
        }
    }
    let has_selection = items
        .iter()
        .enumerate()
        .any(|(index, item)| item.has_selection(index, selected));
    let mut ids = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let include = !has_selection || selected.contains(&item.key(index));
        if let Some(id) = item.native_id()
            && include
        {
            ids.insert(id);
        } else if !item.child()
            && let Some(batch) = item.native_batch()
            && let Some(members) = batches.get(&batch)
        {
            ids.extend(members.iter().filter(|id| include || selected.contains(id)));
        }
    }
    ids
}

pub(super) fn scope_vault_ids(
    items: &[TransferItem],
    selected: &BTreeSet<u64>,
    snapshots: &[(u64, Option<u64>)],
) -> BTreeSet<u64> {
    let mut batches: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (id, batch) in snapshots {
        if let Some(batch) = batch {
            batches.entry(*batch).or_default().push(*id);
        }
    }
    let has_selection = items
        .iter()
        .enumerate()
        .any(|(index, item)| item.has_selection(index, selected));
    let mut ids = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let include = !has_selection || selected.contains(&item.key(index));
        if let Some(id) = item.vault_id()
            && include
        {
            ids.insert(id);
        } else if !item.child()
            && let Some(batch) = item.vault_batch()
            && let Some(members) = batches.get(&batch)
        {
            ids.extend(
                members
                    .iter()
                    .filter(|id| include || selected.contains(&vault_transfer_selection_key(**id))),
            );
        }
    }
    ids
}

/// A group checkbox and its children share one selection, including collapsed
/// members and members in the auxiliary window. Keys survive state reordering.
pub(super) fn set_selection(
    items: &[TransferItem],
    selected: &mut BTreeSet<u64>,
    keys: &[u64],
    checked: bool,
) {
    let mut groups = BTreeMap::<u64, Vec<u64>>::new();
    for (index, item) in items.iter().enumerate() {
        if item.child()
            && let Some(parent) = item.parent_key()
        {
            let group_key = if item.native_batch().is_some() {
                0x4000_0000_0000_0000 | parent
            } else {
                parent
            };
            groups.entry(group_key).or_default().push(item.key(index));
        }
    }
    for key in keys {
        for key in std::iter::once(key).chain(groups.get(key).into_iter().flatten()) {
            if checked {
                selected.insert(*key);
            } else {
                selected.remove(key);
            }
        }
    }
    for (group, members) in groups {
        if members.iter().all(|key| selected.contains(key)) {
            selected.insert(group);
        } else {
            selected.remove(&group);
        }
    }
}
