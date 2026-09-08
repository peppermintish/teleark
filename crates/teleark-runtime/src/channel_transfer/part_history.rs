//! Bounded part replay with incremental counters; appending never rescans the replay.
use super::{ChannelDownloadPartEvent, DownloadPartState, PartCounters};
use std::collections::{BTreeMap, VecDeque};

const CAPACITY: usize = 8_192;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartEventHistory {
    events: VecDeque<ChannelDownloadPartEvent>,
    latest: BTreeMap<u64, (u64, DownloadPartState)>,
    counts: [u64; 4],
    observed: u64,
}

impl PartEventHistory {
    pub fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = &ChannelDownloadPartEvent> + ExactSizeIterator {
        self.events.iter()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn omitted(&self) -> u64 {
        self.observed.saturating_sub(self.events.len() as u64)
    }

    pub(super) fn push(&mut self, event: ChannelDownloadPartEvent) {
        if self.events.len() == CAPACITY {
            let oldest_sequence = self.observed - self.events.len() as u64;
            if let Some(old) = self.events.pop_front()
                && self
                    .latest
                    .get(&old.part_index)
                    .is_some_and(|(sequence, _)| *sequence == oldest_sequence)
                && let Some((_, state)) = self.latest.remove(&old.part_index)
            {
                self.counts[state_index(state)] -= 1;
            }
        }
        if let Some((_, old)) = self
            .latest
            .insert(event.part_index, (self.observed, event.state))
        {
            self.counts[state_index(old)] -= 1;
        }
        self.counts[state_index(event.state)] += 1;
        self.observed = self.observed.saturating_add(1);
        self.events.push_back(event);
    }

    pub(super) fn retain_recent(&mut self, limit: usize) {
        while self.events.len() > limit {
            let sequence = self.observed.saturating_sub(self.events.len() as u64);
            if let Some(old) = self.events.pop_front()
                && self
                    .latest
                    .get(&old.part_index)
                    .is_some_and(|(latest, _)| *latest == sequence)
                && let Some((_, state)) = self.latest.remove(&old.part_index)
            {
                self.counts[state_index(state)] -= 1;
            }
        }
        self.events.shrink_to_fit();
    }

    pub(super) fn counters(
        &self,
        total_parts: u64,
        completed_parts: u64,
        rate: u64,
    ) -> PartCounters {
        PartCounters {
            total_parts,
            completed_parts,
            inflight_parts: self.counts[state_index(DownloadPartState::Inflight)],
            retry_parts: self.counts[state_index(DownloadPartState::Retry)],
            failed_parts: self.counts[state_index(DownloadPartState::Failed)],
            missing_parts: total_parts.saturating_sub(completed_parts),
            completed_parts_per_second_milli: rate,
        }
    }
}

impl IntoIterator for PartEventHistory {
    type Item = ChannelDownloadPartEvent;
    type IntoIter = std::collections::vec_deque::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.into_iter()
    }
}

fn state_index(state: DownloadPartState) -> usize {
    match state {
        DownloadPartState::Inflight => 0,
        DownloadPartState::Completed => 1,
        DownloadPartState::Retry => 2,
        DownloadPartState::Failed => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(index: u64, state: DownloadPartState) -> ChannelDownloadPartEvent {
        ChannelDownloadPartEvent {
            part_index: index,
            offset_bytes: index * 1_048_576,
            length_bytes: 1_048_576,
            state,
            attempt: 1,
            elapsed_millis: 1,
        }
    }

    fn legacy_counts(events: impl Iterator<Item = ChannelDownloadPartEvent>) -> [u64; 4] {
        let mut latest = BTreeMap::new();
        for event in events {
            latest.insert(event.part_index, event.state);
        }
        let mut counts = [0; 4];
        for state in latest.into_values() {
            counts[state_index(state)] += 1;
        }
        counts
    }

    #[test]
    fn eviction_preserves_newer_states_and_reports_retention() {
        let mut history = PartEventHistory::default();
        for sequence in 0..CAPACITY as u64 * 3 {
            let states = [
                DownloadPartState::Inflight,
                DownloadPartState::Retry,
                DownloadPartState::Completed,
                DownloadPartState::Failed,
            ];
            history.push(event(sequence % 9_001, states[sequence as usize % 4]));
            if sequence % 997 == 0 {
                assert_eq!(history.counts, legacy_counts(history.iter().cloned()));
            }
        }
        assert_eq!(history.counts, legacy_counts(history.iter().cloned()));
        assert_eq!(history.len(), CAPACITY);
        assert_eq!(history.omitted(), CAPACITY as u64 * 2);
        assert!(history.latest.len() <= CAPACITY);
        // Total completion comes from verified byte progress, not the truncated replay.
        assert_eq!(
            history.counters(30_000, 20_000, 1_500).completed_parts,
            20_000
        );
        assert_eq!(
            history.counters(30_000, 20_000, 1_500).missing_parts,
            10_000
        );
    }

    #[test]
    fn compacted_part_history_can_resume_without_corrupting_incremental_counts() {
        let mut history = PartEventHistory::default();
        for index in 0..9000 {
            history.push(event(index % 129, DownloadPartState::Inflight));
        }
        history.retain_recent(20);
        assert_eq!(history.len(), 20);
        assert_eq!(history.omitted(), 8980);
        assert_eq!(history.counts, legacy_counts(history.iter().copied()));
        for index in 0..1000 {
            history.push(event(index % 131, DownloadPartState::Completed));
        }
        assert_eq!(history.counts, legacy_counts(history.iter().copied()));
        assert_eq!(history.omitted(), 8980);
        assert_eq!(history.counters(10000, 7500, 1).completed_parts, 7500);
    }

    #[test]
    #[ignore = "manual before/after performance measurement"]
    fn perf_native_part_history() {
        use std::{hint::black_box, time::Instant};
        let mut history = PartEventHistory::default();
        for index in 0..CAPACITY as u64 {
            history.push(event(index, DownloadPartState::Completed));
        }
        let mut legacy: Vec<_> = history.iter().cloned().collect();
        let updates: Vec<_> = (0..1_000)
            .map(|i| event(i + CAPACITY as u64, DownloadPartState::Inflight))
            .collect();
        let started = Instant::now();
        for event in &updates {
            legacy.remove(0);
            legacy.push(*event);
            black_box(legacy_counts(legacy.iter().cloned()));
            black_box(legacy.clone());
        }
        let before = started.elapsed();
        let started = Instant::now();
        for event in updates {
            history.push(event);
            black_box(history.counters(20_000, 8_192, 0));
        }
        let after = started.elapsed();
        assert_eq!(history.counts, legacy_counts(legacy.into_iter()));
        println!(
            "native_part_history capacity={CAPACITY} updates=1000 old_us={} new_us={}",
            before.as_micros(),
            after.as_micros()
        );
    }
}
