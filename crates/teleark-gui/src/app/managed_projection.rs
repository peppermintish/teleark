use super::ChannelBatchPeriod;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use teleark_core::FileKind;

use teleark_runtime::ManagedVaultFile;

/// Retaining the source makes all subsequent edits copy-on-write and invalidates
/// this projection without scanning unchanged catalog rows during rendering.
#[derive(Default)]
pub(crate) struct ManagedProjection {
    source: Arc<Vec<ManagedVaultFile>>,
    query: String,
    period: ChannelBatchPeriod,
    kinds: HashSet<FileKind>,
    rows: Arc<Vec<usize>>,
    by_message: HashMap<i64, usize>,
    downloadable_count: usize,
}

impl ManagedProjection {
    #[cfg(test)]
    pub(crate) fn rows(
        &mut self,
        source: &Arc<Vec<ManagedVaultFile>>,
        query: &str,
    ) -> Arc<Vec<usize>> {
        self.filtered_rows(source, query, ChannelBatchPeriod::AnyTime, &HashSet::new())
    }

    pub(crate) fn filtered_rows(
        &mut self,
        source: &Arc<Vec<ManagedVaultFile>>,
        query: &str,
        period: ChannelBatchPeriod,
        kinds: &HashSet<FileKind>,
    ) -> Arc<Vec<usize>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_millis().min(i64::MAX as u128) as i64);
        self.filtered_rows_at(source, query, period, kinds, now)
    }

    pub(crate) fn downloadable_count(&self) -> usize {
        self.downloadable_count
    }

    pub(super) fn filtered_rows_at(
        &mut self,
        source: &Arc<Vec<ManagedVaultFile>>,
        query: &str,
        period: ChannelBatchPeriod,
        kinds: &HashSet<FileKind>,
        now: i64,
    ) -> Arc<Vec<usize>> {
        if !Arc::ptr_eq(source, &self.source)
            || query != self.query
            || period != self.period
            || kinds != &self.kinds
        {
            self.source = source.clone();
            self.query = query.into();
            self.period = period;
            self.kinds = kinds.clone();
            let age = match period {
                ChannelBatchPeriod::AnyTime => None,
                ChannelBatchPeriod::Past24Hours => Some(86_400_000),
                ChannelBatchPeriod::Past7Days => Some(7 * 86_400_000),
                ChannelBatchPeriod::Past30Days => Some(30 * 86_400_000),
            };
            let earliest = age.map(|age| now.saturating_sub(age));
            self.by_message.clear();
            self.downloadable_count = 0;
            self.rows = Arc::new(
                source
                    .iter()
                    .enumerate()
                    .filter_map(|(index, file)| {
                        if (query.is_empty() || file.logical_name.to_lowercase().contains(query))
                            && earliest.is_none_or(|earliest| file.created_at_unix_ms >= earliest)
                            && (kinds.is_empty() || kinds.contains(&file.media_kind))
                        {
                            self.by_message.insert(file.manifest_message_id, index);
                            self.downloadable_count +=
                                usize::from(super::managed_downloadable(file));
                            Some(index)
                        } else {
                            None
                        }
                    })
                    .collect(),
            );
        }
        self.rows.clone()
    }

    pub(crate) fn selected(&self, message: Option<i64>) -> Option<&ManagedVaultFile> {
        self.by_message
            .get(&message?)
            .and_then(|index| self.source.get(*index))
    }
}
