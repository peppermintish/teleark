//! Bounded independent RPC lanes. Only the control lane can replace session state.
use std::{collections::VecDeque, future::Future};
use teleark_telegram::ScanCancellation;
use tokio::{sync::mpsc, task::JoinSet};

const PENDING_CAPACITY: usize = 32;
const READ_CAPACITY: usize = 8;
const TRANSFER_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Lane {
    Probe,
    Read,
    ManagedRead,
    Transfer,
    Control,
    Barrier,
    Shutdown,
}

pub(super) async fn run<S, R, F, Fut>(
    mut receiver: mpsc::Receiver<R>,
    mut state: S,
    snapshot: impl Fn(&S) -> S,
    classify: impl Fn(&R) -> Lane,
    execute: F,
    reject: impl Fn(R),
) where
    S: Send + 'static,
    R: Send + 'static,
    F: Fn(S, R, ScanCancellation) -> Fut,
    Fut: Future<Output = S> + Send + 'static,
{
    let mut view = snapshot(&state);
    let mut available = Some(state);
    let mut controls = JoinSet::new();
    let mut probes = JoinSet::new();
    let mut reads = JoinSet::new();
    let mut managed_reads = JoinSet::new();
    let mut transfers = JoinSet::new();
    let mut pending = VecDeque::new();
    let mut control_cancel = ScanCancellation::default();
    let mut barrier_active = false;
    loop {
        // A saturated transfer lane cannot consume the slots reserved for reads.
        let mut index = 0;
        while index < pending.len() {
            let (lane, _) = &pending[index];
            let ready = match lane {
                Lane::Probe => !barrier_active && probes.is_empty(),
                Lane::Read => !barrier_active && reads.len() < READ_CAPACITY,
                Lane::ManagedRead => !barrier_active && managed_reads.is_empty(),
                Lane::Transfer => !barrier_active && transfers.len() < TRANSFER_CAPACITY,
                Lane::Control | Lane::Barrier => available.is_some(),
                Lane::Shutdown => unreachable!("shutdown is handled on receipt"),
            };
            if !ready {
                index += 1;
                continue;
            }
            let (lane, request) = pending.remove(index).expect("pending index exists");
            match lane {
                Lane::Probe => {
                    probes.spawn(execute(
                        snapshot(&view),
                        request,
                        ScanCancellation::default(),
                    ));
                }
                Lane::Read => {
                    reads.spawn(execute(
                        snapshot(&view),
                        request,
                        ScanCancellation::default(),
                    ));
                }
                Lane::ManagedRead => {
                    managed_reads.spawn(execute(
                        snapshot(&view),
                        request,
                        ScanCancellation::default(),
                    ));
                }
                Lane::Transfer => {
                    transfers.spawn(execute(
                        snapshot(&view),
                        request,
                        ScanCancellation::default(),
                    ));
                }
                Lane::Control | Lane::Barrier => {
                    control_cancel = ScanCancellation::default();
                    controls.spawn(execute(
                        available.take().expect("control state is available"),
                        request,
                        control_cancel.clone(),
                    ));
                }
                Lane::Shutdown => unreachable!(),
            }
        }
        tokio::select! {
            biased;
            result = controls.join_next(), if !controls.is_empty() => {
                let Some(Ok(updated)) = result else { break };
                state = updated;
                view = snapshot(&state);
                available = Some(state);
                barrier_active = pending.iter().any(|(lane, _)| *lane == Lane::Barrier);
            }
            _ = probes.join_next(), if !probes.is_empty() => {}
            _ = reads.join_next(), if !reads.is_empty() => {}
            _ = managed_reads.join_next(), if !managed_reads.is_empty() => {}
            _ = transfers.join_next(), if !transfers.is_empty() => {}
            request = receiver.recv() => {
                let Some(request) = request else { break };
                let lane = classify(&request);
                if lane == Lane::Shutdown { break; }
                if lane == Lane::Barrier {
                    // Finish cancellation before publishing a replacement session. In-flight
                    // callers lose their reply sender; no old result can publish afterwards.
                    barrier_active = true;
                    control_cancel.cancel();
                    probes.abort_all();
                    reads.abort_all();
                    managed_reads.abort_all();
                    transfers.abort_all();
                    while probes.join_next().await.is_some() {}
                    while reads.join_next().await.is_some() {}
                    while managed_reads.join_next().await.is_some() {}
                    while transfers.join_next().await.is_some() {}
                    pending.clear();
                }
                // A separate bounded admission slot keeps ordinary read
                // saturation from refusing managed-channel synchronization.
                if (lane == Lane::ManagedRead && pending.iter().filter(|(lane, _)| *lane == Lane::ManagedRead).count() >= 1)
                    || (lane != Lane::ManagedRead && pending.iter().filter(|(lane, _)| *lane != Lane::ManagedRead).count() >= PENDING_CAPACITY) {
                    reject(request);
                } else {
                    pending.push_back((lane, request));
                }
            }
        }
    }
    control_cancel.cancel();
    probes.abort_all();
    reads.abort_all();
    managed_reads.abort_all();
    transfers.abort_all();
    while probes.join_next().await.is_some() {}
    while reads.join_next().await.is_some() {}
    while managed_reads.join_next().await.is_some() {}
    while transfers.join_next().await.is_some() {}
    while controls.join_next().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::{Notify, oneshot};

    #[tokio::test]
    async fn managed_reads_bypass_saturated_normal_reads_and_are_fenced_by_account_barriers() {
        let (tx, task) = start(Arc::default());
        let block = Arc::new(Notify::new());
        let mut held = Vec::new();
        for _ in 0..READ_CAPACITY {
            let (entered, reply) = send(&tx, Lane::Read, Some(block.clone())).await;
            entered.await.expect("normal lane blocked");
            held.push(reply);
        }
        let (_, result) = send(&tx, Lane::ManagedRead, None).await;
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), result)
                .await
                .expect("managed lane does not wait for normal RPCs")
                .expect("managed reply"),
            1
        );
        let (entered, managed) = send(&tx, Lane::ManagedRead, Some(block)).await;
        entered.await.expect("managed read blocked");
        let (_, barrier) = send(&tx, Lane::Barrier, None).await;
        barrier.await.expect("account barrier");
        assert!(
            managed.await.is_err(),
            "old managed callbacks cannot survive an account barrier"
        );
        for reply in held {
            assert!(reply.await.is_err());
        }
        drop(tx);
        task.await.expect("dispatcher stopped");
    }

    struct Request {
        lane: Lane,
        block: Option<Arc<Notify>>,
        entered: oneshot::Sender<()>,
        reply: oneshot::Sender<usize>,
    }

    async fn send(
        tx: &mpsc::Sender<Request>,
        lane: Lane,
        block: Option<Arc<Notify>>,
    ) -> (oneshot::Receiver<()>, oneshot::Receiver<usize>) {
        let (entered, started) = oneshot::channel();
        let (reply, result) = oneshot::channel();
        tx.send(Request {
            lane,
            block,
            entered,
            reply,
        })
        .await
        .expect("controlled dispatcher fixture");
        (started, result)
    }

    fn start(rejected: Arc<AtomicUsize>) -> (mpsc::Sender<Request>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(32);
        let task = tokio::spawn(run(
            rx,
            1_usize,
            |s| *s,
            |r: &Request| r.lane,
            |mut state, request, cancellation| async move {
                let _ = request.entered.send(());
                // This mutation models an ambiguous remote-create guard that must survive
                // cancellation; it belongs to the retained control state, not a read copy.
                if matches!(request.lane, Lane::Control | Lane::Barrier) {
                    state += 1;
                }
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {},
                    _ = async { if let Some(block) = request.block { block.notified().await; } } => {
                        let _ = request.reply.send(state);
                    }
                }
                state
            },
            move |_| {
                rejected.fetch_add(1, Ordering::SeqCst);
            },
        ));
        (tx, task)
    }

    #[tokio::test]
    async fn blocked_transfers_and_metadata_leave_reads_available() {
        let (tx, task) = start(Arc::default());
        let block = Arc::new(Notify::new());
        let mut held = Vec::new();
        for _ in 0..TRANSFER_CAPACITY {
            let (entered, reply) = send(&tx, Lane::Transfer, Some(block.clone())).await;
            entered.await.expect("controlled dispatcher fixture");
            held.push(reply);
        }
        let (entered, control) = send(&tx, Lane::Control, Some(block)).await;
        entered.await.expect("controlled dispatcher fixture");
        let (_, read) = send(&tx, Lane::Read, None).await;
        assert_eq!(read.await.expect("controlled dispatcher fixture"), 1);
        let (_, barrier) = send(&tx, Lane::Barrier, None).await;
        // Cancelled control preserves its state mutation; the barrier adds one more.
        assert_eq!(barrier.await.expect("controlled dispatcher fixture"), 3);
        assert!(control.await.is_err());
        for reply in held {
            assert!(reply.await.is_err());
        }
        let (_, read) = send(&tx, Lane::Read, None).await;
        assert_eq!(read.await.expect("controlled dispatcher fixture"), 3);
        drop(tx);
        task.await.expect("controlled dispatcher fixture");
    }

    #[tokio::test]
    async fn proxy_probe_has_a_reserved_slot_when_reads_and_transfers_are_blocked() {
        let (tx, task) = start(Arc::default());
        let block = Arc::new(Notify::new());
        let mut held = Vec::new();
        for lane in std::iter::repeat_n(Lane::Read, READ_CAPACITY)
            .chain(std::iter::repeat_n(Lane::Transfer, TRANSFER_CAPACITY))
        {
            let (entered, reply) = send(&tx, lane, Some(block.clone())).await;
            entered.await.expect("blocked operation admitted");
            held.push(reply);
        }
        let (_, result) = send(&tx, Lane::Probe, None).await;
        assert_eq!(result.await.expect("probe bypasses busy service lanes"), 1);
        let (entered, probe) = send(&tx, Lane::Probe, Some(block)).await;
        entered.await.expect("second probe admitted");
        let (_, barrier) = send(&tx, Lane::Barrier, None).await;
        barrier.await.expect("barrier completed");
        assert!(probe.await.is_err(), "barrier cancels a retained probe too");
        for reply in held {
            assert!(reply.await.is_err());
        }
        drop(tx);
        task.await.expect("dispatcher stopped");
    }

    #[tokio::test]
    async fn saturation_is_bounded_and_a_barrier_can_always_enter() {
        let rejected = Arc::new(AtomicUsize::new(0));
        let (tx, task) = start(rejected.clone());
        let block = Arc::new(Notify::new());
        let mut replies = Vec::new();
        for _ in 0..READ_CAPACITY {
            let (entered, reply) = send(&tx, Lane::Read, Some(block.clone())).await;
            entered.await.expect("controlled dispatcher fixture");
            replies.push(reply);
        }
        for _ in 0..PENDING_CAPACITY + 1 {
            let (_, reply) = send(&tx, Lane::Read, Some(block.clone())).await;
            replies.push(reply);
        }
        assert!(
            replies
                .pop()
                .expect("controlled dispatcher fixture")
                .await
                .is_err()
        );
        assert_eq!(rejected.load(Ordering::SeqCst), 1);
        let (entered, barrier) = send(&tx, Lane::Barrier, Some(block.clone())).await;
        entered.await.expect("controlled dispatcher fixture");
        for reply in replies {
            assert!(reply.await.is_err());
        }
        let (mut entered, read) = send(&tx, Lane::Read, None).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            entered.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        block.notify_one();
        assert_eq!(barrier.await.expect("controlled dispatcher fixture"), 2);
        assert_eq!(read.await.expect("controlled dispatcher fixture"), 2);
        drop(tx);
        task.await.expect("controlled dispatcher fixture");
    }

    #[tokio::test]
    async fn owner_closure_drains_every_active_lane() {
        let (tx, task) = start(Arc::default());
        let block = Arc::new(Notify::new());
        let mut replies = Vec::new();
        for lane in [Lane::Read, Lane::Transfer, Lane::Control] {
            let (entered, reply) = send(&tx, lane, Some(block.clone())).await;
            entered.await.expect("controlled dispatcher fixture");
            replies.push(reply);
        }
        drop(tx);
        task.await.expect("controlled dispatcher fixture");
        for reply in replies {
            assert!(reply.await.is_err());
        }
    }
}
