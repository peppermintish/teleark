//! Shared directional payload budgets. No mutex is held while a timer or network
//! request waits. A FIFO permit orders admission; cancellation releases it.
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::{Semaphore, watch},
    time::Instant,
};

const MAX_BURST: usize = 128 * 1024;
const READER_SLICE: usize = 16 * 1024;
const EVENT_LIMIT: usize = 16;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TransferSpeedLimits {
    /// Payload bytes per second; zero means unlimited.
    pub upload: u64,
    pub download: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BandwidthEventKind {
    LimitChanged,
    Waiting,
    Resumed,
}
#[derive(Clone, Copy, Debug)]
pub struct BandwidthEvent {
    pub sequence: u64,
    pub kind: BandwidthEventKind,
    pub at: Instant,
}
#[derive(Clone, Debug)]
pub struct BandwidthSnapshot {
    pub bytes_per_second: u64,
    pub waiting: usize,
    pub waiting_millis: u64,
    pub last_activity_millis: Option<u64>,
    pub revision: u64,
    pub events: Vec<BandwidthEvent>,
    pub omitted_events: u64,
}

struct State {
    rate: u64,
    credit: f64,
    updated: Instant,
    waiting: usize,
    waiting_since: Option<Instant>,
    last_activity: Option<Instant>,
    revision: u64,
    sequence: u64,
    events: VecDeque<BandwidthEvent>,
}
impl State {
    fn event(&mut self, kind: BandwidthEventKind) {
        self.sequence = self.sequence.saturating_add(1);
        if self.events.len() == EVENT_LIMIT {
            self.events.pop_front();
        }
        self.events.push_back(BandwidthEvent {
            sequence: self.sequence,
            kind,
            at: Instant::now(),
        });
    }
}
struct Inner {
    state: Mutex<State>,
    turn: Semaphore,
    changed: watch::Sender<u64>,
}
#[derive(Clone)]
pub struct BandwidthBudget(Arc<Inner>);
impl Default for BandwidthBudget {
    fn default() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State {
                rate: 0,
                credit: 0.0,
                updated: Instant::now(),
                waiting: 0,
                waiting_since: None,
                last_activity: None,
                revision: 0,
                sequence: 0,
                events: VecDeque::new(),
            }),
            turn: Semaphore::new(1),
            changed: watch::channel(0).0,
        }))
    }
}
impl BandwidthBudget {
    pub fn set_limit(&self, rate: u64) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.rate == rate {
            return;
        }
        state.rate = rate;
        // Do not carry unlimited or previous-rate credit into a new policy.
        state.credit = 0.0;
        state.updated = Instant::now();
        state.revision = state.revision.saturating_add(1);
        state.event(BandwidthEventKind::LimitChanged);
        self.0.changed.send_replace(state.revision);
    }
    pub fn snapshot(&self) -> BandwidthSnapshot {
        let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let age = |at: Instant| at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        BandwidthSnapshot {
            bytes_per_second: state.rate,
            waiting: state.waiting,
            waiting_millis: state.waiting_since.map_or(0, age),
            last_activity_millis: state.last_activity.map(age),
            revision: state.revision,
            events: state.events.iter().copied().collect(),
            omitted_events: state.sequence.saturating_sub(state.events.len() as u64),
        }
    }
    fn rate(&self) -> u64 {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).rate
    }
    /// A power-of-two protocol chunk, reducing low-rate bursts without changing
    /// unlimited throughput or the persistent logical-part/bitmap format.
    pub(crate) fn download_chunk_size(&self) -> usize {
        let rate = self.rate();
        if rate == 0 {
            return MAX_BURST;
        }
        let target = (rate / 4).clamp(4096, MAX_BURST as u64) as usize;
        1 << (usize::BITS - 1 - target.leading_zeros())
    }
    pub async fn acquire(&self, mut bytes: usize) {
        while bytes > 0 {
            let count = bytes.min(MAX_BURST);
            self.acquire_chunk(count).await;
            bytes -= count;
        }
    }
    async fn acquire_chunk(&self, bytes: usize) {
        if self.rate() == 0 || bytes == 0 {
            return;
        }
        let mut changes = self.0.changed.subscribe();
        let mut waiting = None;
        // This private semaphore is never closed. Its permit is admission
        // ownership, not a state lock; all policy changes remain independent.
        let _turn = match self.0.turn.try_acquire() {
            Ok(turn) => turn,
            Err(_) => {
                waiting = Some(Waiting::new(self.clone()));
                let Ok(turn) = self.0.turn.acquire().await else {
                    return;
                };
                turn
            }
        };
        loop {
            let delay = {
                let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.rate == 0 {
                    None
                } else {
                    let now = Instant::now();
                    state.credit = (state.credit
                        + now.duration_since(state.updated).as_secs_f64() * state.rate as f64)
                        .min(MAX_BURST as f64);
                    state.updated = now;
                    if state.credit >= bytes as f64 {
                        state.credit -= bytes as f64;
                        state.last_activity = Some(now);
                        state.revision = state.revision.saturating_add(1);
                        None
                    } else {
                        Some(Duration::from_secs_f64(
                            ((bytes as f64 - state.credit) / state.rate as f64).max(0.000_001),
                        ))
                    }
                }
            };
            let Some(delay) = delay else {
                break;
            };
            waiting.get_or_insert_with(|| Waiting::new(self.clone()));
            tokio::select! { _ = tokio::time::sleep(delay) => {}, _ = changes.changed() => {} }
        }
        drop(waiting);
    }
}
struct Waiting(BandwidthBudget);
impl Waiting {
    fn new(budget: BandwidthBudget) -> Self {
        {
            let mut state = budget.0.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.waiting == 0 {
                state.waiting_since = Some(Instant::now());
                state.event(BandwidthEventKind::Waiting);
            }
            state.waiting += 1;
            state.revision = state.revision.saturating_add(1);
        }
        Self(budget)
    }
}
impl Drop for Waiting {
    fn drop(&mut self) {
        let mut state = self.0.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.waiting = state.waiting.saturating_sub(1);
        if state.waiting == 0 {
            state.waiting_since = None;
            state.event(BandwidthEventKind::Resumed);
        }
        state.revision = state.revision.saturating_add(1);
    }
}
#[derive(Clone, Default)]
pub struct TransferBandwidth {
    pub upload: BandwidthBudget,
    pub download: BandwidthBudget,
}
impl TransferBandwidth {
    pub fn set_limits(&self, limits: TransferSpeedLimits) {
        self.upload.set_limit(limits.upload);
        self.download.set_limit(limits.download);
    }
}

/// Paces reads before bytes reach the transport. Only one bounded admission
/// future is retained; dropping the stream cancels any pending admission.
pub(crate) struct LimitedReader<R> {
    reader: R,
    remaining: usize,
    budget: BandwidthBudget,
    admission: Option<Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
    admitted: usize,
}
impl<R> LimitedReader<R> {
    pub(crate) fn new(reader: R, size: usize, budget: BandwidthBudget) -> Self {
        Self {
            reader,
            remaining: size,
            budget,
            admission: None,
            admitted: 0,
        }
    }
}
impl<R: AsyncRead + Unpin> AsyncRead for LimitedReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 || this.remaining == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.admitted == 0 {
            this.admitted = buf.remaining().min(this.remaining).min(READER_SLICE);
            let budget = this.budget.clone();
            let count = this.admitted;
            this.admission = Some(Box::pin(async move { budget.acquire(count).await }));
        }
        if let Some(admission) = this.admission.as_mut() {
            if admission.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            this.admission = None;
        }
        let length = this.admitted.min(buf.remaining());
        let mut limited = ReadBuf::new(buf.initialize_unfilled_to(length));
        match Pin::new(&mut this.reader).poll_read(cx, &mut limited) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                let read = limited.filled().len();
                buf.advance(read);
                this.admitted = this.admitted.saturating_sub(read);
                this.remaining = if read == 0 {
                    0
                } else {
                    this.remaining.saturating_sub(read)
                };
                Poll::Ready(result)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    #[tokio::test(start_paused = true)]
    async fn aggregate_budget_paces_all_workers_and_directions_are_independent() {
        let shared = TransferBandwidth::default();
        shared.set_limits(TransferSpeedLimits {
            upload: 1024,
            download: 2048,
        });
        let start = Instant::now();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let budget = shared.upload.clone();
            tasks.spawn(async move { budget.acquire(1024).await });
        }
        shared.download.acquire(2048).await;
        assert_eq!(start.elapsed(), Duration::from_secs(1));
        while let Some(result) = tasks.join_next().await {
            result.expect("worker");
        }
        assert_eq!(start.elapsed(), Duration::from_secs(4));
        assert_eq!(shared.upload.snapshot().waiting, 0);
    }
    #[tokio::test(start_paused = true)]
    async fn live_unlimited_change_wakes_waiters_and_abort_releases_admission() {
        let budget = BandwidthBudget::default();
        budget.set_limit(1);
        let copy = budget.clone();
        let task = tokio::spawn(async move { copy.acquire(4096).await });
        tokio::task::yield_now().await;
        assert_eq!(budget.snapshot().waiting, 1);
        task.abort();
        let _ = task.await;
        assert_eq!(budget.snapshot().waiting, 0);
        let copy = budget.clone();
        let task = tokio::spawn(async move { copy.acquire(4096).await });
        tokio::task::yield_now().await;
        budget.set_limit(0);
        task.await.expect("unlimited wake");
        assert_eq!(budget.snapshot().waiting, 0);
    }
    #[tokio::test(start_paused = true)]
    async fn limited_reader_preserves_bytes_and_waits_before_delivery() {
        let budget = BandwidthBudget::default();
        budget.set_limit(4096);
        let original = vec![42; 16 * 1024];
        let mut reader = LimitedReader::new(original.as_slice(), original.len(), budget.clone());
        let start = Instant::now();
        let mut received = Vec::new();
        reader.read_to_end(&mut received).await.expect("read");
        assert_eq!(received, original);
        assert!(start.elapsed() >= Duration::from_secs(4));
        for limit in 1..100 {
            budget.set_limit(limit);
        }
        let snapshot = budget.snapshot();
        assert_eq!(snapshot.events.len(), EVENT_LIMIT);
        assert!(snapshot.omitted_events > 0);
    }
    #[tokio::test(start_paused = true)]
    async fn active_rate_changes_recompute_wait_without_old_credit() {
        let budget = BandwidthBudget::default();
        budget.set_limit(1);
        let copy = budget.clone();
        let task = tokio::spawn(async move { copy.acquire(4096).await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        budget.set_limit(4096);
        let changed = Instant::now();
        task.await.expect("increase wakes timer");
        assert!(changed.elapsed() >= Duration::from_secs(1));
        assert!(changed.elapsed() < Duration::from_secs(2));

        let copy = budget.clone();
        let task = tokio::spawn(async move { copy.acquire(4096).await });
        tokio::task::yield_now().await;
        budget.set_limit(1024);
        let changed = Instant::now();
        task.await.expect("decrease recomputes budget");
        assert!(changed.elapsed() >= Duration::from_secs(4));
        assert!(changed.elapsed() < Duration::from_secs(5));
    }
    #[tokio::test(start_paused = true)]
    async fn cancelling_fifo_waiter_preserves_other_waiters_and_timeline() {
        let budget = BandwidthBudget::default();
        budget.set_limit(1);
        let mut tasks = Vec::new();
        for _ in 0..3 {
            let copy = budget.clone();
            tasks.push(tokio::spawn(async move { copy.acquire(4096).await }));
        }
        tokio::task::yield_now().await;
        assert_eq!(budget.snapshot().waiting, 3);
        tasks.remove(1).abort();
        tokio::task::yield_now().await;
        assert_eq!(budget.snapshot().waiting, 2);
        // Synchronous policy/snapshot operations remain available while admission is blocked.
        budget.set_limit(0);
        for task in tasks {
            task.await.expect("remaining waiter");
        }
        let snapshot = budget.snapshot();
        assert_eq!(snapshot.waiting, 0);
        assert_eq!(
            snapshot.events.last().expect("terminal wait event").kind,
            BandwidthEventKind::Resumed
        );
    }
    #[tokio::test(start_paused = true)]
    async fn short_source_reads_reuse_admission_without_charging_bytes_twice() {
        struct ShortReader(usize);
        impl AsyncRead for ShortReader {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                let count = self.0.min(3).min(buf.remaining());
                buf.put_slice(&[7; 3][..count]);
                self.0 -= count;
                Poll::Ready(Ok(()))
            }
        }
        let budget = BandwidthBudget::default();
        budget.set_limit(1024);
        let mut reader = LimitedReader::new(ShortReader(1024), 1024, budget);
        let start = Instant::now();
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .expect("read short source");
        assert_eq!(bytes, vec![7; 1024]);
        assert!(start.elapsed() >= Duration::from_secs(1));
        assert!(start.elapsed() < Duration::from_millis(1100));
    }
    #[tokio::test(start_paused = true)]
    async fn unlimited_has_no_wait_and_idle_credit_cannot_exceed_burst_bound() {
        let budget = BandwidthBudget::default();
        let start = Instant::now();
        budget.acquire(8 * MAX_BURST).await;
        assert_eq!(start.elapsed(), Duration::ZERO);
        assert_eq!(budget.snapshot().revision, 0);
        budget.set_limit(1024);
        tokio::time::advance(Duration::from_secs(10000)).await;
        let start = Instant::now();
        budget.acquire(MAX_BURST).await;
        assert_eq!(start.elapsed(), Duration::ZERO);
        assert!(
            !budget
                .snapshot()
                .events
                .iter()
                .any(|event| event.kind == BandwidthEventKind::Waiting),
            "available credit must not invent a bandwidth wait"
        );
        budget.acquire(1024).await;
        assert_eq!(start.elapsed(), Duration::from_secs(1));
    }
    #[test]
    fn protocol_chunks_preserve_alignment_and_logical_part_offsets() {
        let budget = BandwidthBudget::default();
        for rate in [0, 1, 1024, 16384, 1_000_000, u64::MAX] {
            budget.set_limit(rate);
            let chunk = budget.download_chunk_size();
            assert!((4096..=MAX_BURST).contains(&chunk));
            assert!(chunk.is_power_of_two());
            assert_eq!(chunk % 4096, 0);
            assert_eq!((64 * 1024 * 1024) % chunk, 0);
        }
    }
}
