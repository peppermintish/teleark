//! Coalesced wakes plus bounded per-channel deltas. Rendering never clones the whole catalog.
use super::*;

const DELTA_CAPACITY: usize = 128;
const DELTA_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ChannelDelta {
    pub chat_id: i64,
    pub revision: i64,
    pub upserted: Vec<TelegramFileSummary>,
    pub removed: Vec<i64>,
    pub history_exhausted: bool,
    pub managed_files_changed: bool,
}

impl ChannelDelta {
    fn estimated_bytes(&self) -> usize {
        64 + 8 * self.removed.len()
            + self
                .upserted
                .iter()
                .map(|f| {
                    64 + f.file_name.len()
                        + f.caption.len()
                        + f.mime_type.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
    }
}

pub struct ChannelChanges {
    pub revision: i64,
    pub reset_required: bool,
    pub deltas: Vec<Arc<ChannelDelta>>,
}

pub struct ChannelSyncSubscription {
    pub(super) receiver: tokio::sync::watch::Receiver<()>,
}

impl ChannelSyncSubscription {
    /// Sleeps until an actual published change; no timer or database query.
    pub async fn changed(&mut self) -> bool {
        self.receiver.changed().await.is_ok()
    }
}

#[derive(Default)]
pub(super) struct DeltaJournal {
    latest: BTreeMap<i64, i64>,
    exhausted: BTreeMap<i64, bool>,
    lost_through: BTreeMap<i64, i64>,
    deltas: VecDeque<Arc<ChannelDelta>>,
    bytes: usize,
}

impl DeltaJournal {
    pub(super) fn append(&mut self, delta: ChannelDelta) {
        self.latest.insert(delta.chat_id, delta.revision);
        let previous = self
            .exhausted
            .insert(delta.chat_id, delta.history_exhausted);
        if delta.upserted.is_empty()
            && delta.removed.is_empty()
            && !delta.managed_files_changed
            && previous == Some(delta.history_exhausted)
        {
            return;
        }
        self.bytes = self.bytes.saturating_add(delta.estimated_bytes());
        self.deltas.push_back(Arc::new(delta));
        while self.deltas.len() > DELTA_CAPACITY || self.bytes > DELTA_BYTES {
            if let Some(old) = self.deltas.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.estimated_bytes());
                self.lost_through.insert(old.chat_id, old.revision);
            }
        }
    }

    pub(super) fn changes(&self, chat: i64, since: i64) -> ChannelChanges {
        ChannelChanges {
            revision: self.latest.get(&chat).copied().unwrap_or(since).max(since),
            reset_required: self
                .lost_through
                .get(&chat)
                .is_some_and(|lost| *lost > since),
            deltas: self
                .deltas
                .iter()
                .filter(|delta| delta.chat_id == chat && delta.revision > since)
                .cloned()
                .collect(),
        }
    }

    pub(super) fn retain_sources(&mut self, sources: &BTreeMap<i64, Job>) {
        self.latest.retain(|chat, _| sources.contains_key(chat));
        self.exhausted.retain(|chat, _| sources.contains_key(chat));
        self.lost_through
            .retain(|chat, _| sources.contains_key(chat));
        self.deltas
            .retain(|delta| sources.contains_key(&delta.chat_id));
        self.bytes = self
            .deltas
            .iter()
            .map(|delta| delta.estimated_bytes())
            .sum();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn delta(chat: i64, revision: i64) -> ChannelDelta {
        ChannelDelta {
            chat_id: chat,
            revision,
            upserted: vec![],
            removed: vec![revision],
            history_exhausted: false,
            managed_files_changed: false,
        }
    }
    #[test]
    fn retention_miss_is_scoped_and_empty_updates_do_not_evict_useful_deltas() {
        let mut journal = DeltaJournal::default();
        journal.append(delta(2, 1));
        for revision in 2..500 {
            let mut blank = delta(2, revision);
            blank.removed.clear();
            journal.append(blank);
        }
        assert_eq!(journal.changes(2, 0).deltas.len(), 1);
        assert!(!journal.changes(2, 0).reset_required);
        assert_eq!(journal.changes(2, 1).revision, 499);
        assert!(journal.changes(2, 1).deltas.is_empty());
        for revision in 1..=128 {
            journal.append(delta(3, revision));
        }
        assert!(journal.changes(2, 0).reset_required);
        assert!(!journal.changes(2, 1).reset_required);
        assert!(!journal.changes(3, 0).reset_required);
        assert_eq!(journal.changes(3, 0).deltas.len(), 128);
        assert!(journal.bytes <= DELTA_BYTES);
    }
    #[test]
    fn oversize_delta_falls_back_to_a_coherent_baseline() {
        let mut journal = DeltaJournal::default();
        let mut large = delta(2, 1);
        large.removed = vec![1; DELTA_BYTES / 8];
        journal.append(large);
        assert!(journal.deltas.is_empty());
        assert!(journal.changes(2, 0).reset_required);
        assert!(!journal.changes(3, 0).reset_required);
    }
}
