//! Bounded, process-owned diagnostic I/O. Recovery checkpoints do not use this sink.
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Write},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

static LOG: OnceLock<Arc<BackgroundLog>> = OnceLock::new();

/// Cumulative diagnostic loss across all transfers in this process, without I/O.
pub fn session_log_dropped_record_count() -> u64 {
    LOG.get()
        .map_or(0, |log| log.omitted_total.load(Ordering::Relaxed))
}

const RECORD_LIMIT: usize = 256;
const BYTE_LIMIT: usize = 1_048_576;
const MAX_RECORD_BYTES: usize = 65_536;

trait Destination: Send + Sync {
    fn append(&self, bytes: &[u8]) -> io::Result<()>;
}

impl Destination for File {
    fn append(&self, bytes: &[u8]) -> io::Result<()> {
        // The file was opened with O_APPEND by the transfer owner. Keeping the
        // handle, rather than reopening a path, cannot recreate a deleted log.
        let mut file = self;
        file.write_all(bytes)
    }
}

struct Record {
    destination: Arc<dyn Destination>,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Pending {
    records: VecDeque<Record>,
    bytes: usize,
    omitted: u64,
    writing: bool,
    closed: bool,
    failures: u64,
}

struct BackgroundLog {
    pending: Mutex<Pending>,
    wake: Condvar,
    owner: Mutex<Option<JoinHandle<()>>>,
    omitted_total: AtomicU64,
}

impl BackgroundLog {
    fn global() -> Arc<Self> {
        Arc::clone(LOG.get_or_init(Self::start))
    }

    fn start() -> Arc<Self> {
        let log = Arc::new(Self {
            pending: Mutex::new(Pending::default()),
            wake: Condvar::new(),
            owner: Mutex::new(None),
            omitted_total: AtomicU64::new(0),
        });
        let worker = Arc::clone(&log);
        match thread::Builder::new()
            .name("teleark-session-logs".into())
            .spawn(move || worker.run())
        {
            Ok(owner) => {
                if let Ok(mut handle) = log.owner.lock() {
                    *handle = Some(owner);
                }
            }
            Err(error) => {
                if let Ok(mut pending) = log.pending.lock() {
                    pending.closed = true;
                }
                tracing::warn!(event = "transfer.logs.unavailable", error_kind = ?error.kind(),
                    "diagnostic log worker unavailable; transfers remain enabled");
            }
        }
        log
    }

    fn omit(&self) {
        self.omitted_total.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut pending) = self.pending.lock() {
            pending.omitted = pending.omitted.saturating_add(1);
        }
    }

    fn submit(&self, record: Record) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        if pending.closed {
            self.omitted_total.fetch_add(1, Ordering::Relaxed);
            pending.omitted = pending.omitted.saturating_add(1);
            return;
        }
        while pending.records.len() >= RECORD_LIMIT
            || pending.bytes.saturating_add(record.bytes.len()) > BYTE_LIMIT
        {
            let Some(oldest) = pending.records.pop_front() else {
                break;
            };
            pending.bytes -= oldest.bytes.len();
            self.omitted_total.fetch_add(1, Ordering::Relaxed);
            pending.omitted = pending.omitted.saturating_add(1);
        }
        pending.bytes += record.bytes.len();
        pending.records.push_back(record);
        drop(pending);
        self.wake.notify_one();
    }

    fn run(&self) {
        loop {
            let (record, omitted) = {
                let Ok(mut pending) = self.pending.lock() else {
                    return;
                };
                while pending.records.is_empty() && !pending.closed {
                    let Ok(next) = self.wake.wait(pending) else {
                        return;
                    };
                    pending = next;
                }
                let Some(record) = pending.records.pop_front() else {
                    return;
                };
                pending.bytes -= record.bytes.len();
                pending.writing = true;
                let omitted = std::mem::take(&mut pending.omitted);
                (record, omitted)
            };
            // No queue lock is held during file I/O. A single writer preserves
            // record ordering across retries that append to the same file.
            let bytes = if omitted == 0 {
                record.bytes
            } else {
                let mut bytes = format!(
                    "{{\"schema\":1,\"event\":\"log_records_omitted\",\"scope\":\"all_transfers\",\"count\":{omitted}}}\n"
                )
                .into_bytes();
                bytes.extend_from_slice(&record.bytes);
                bytes
            };
            let result = record.destination.append(&bytes);
            let mut warning = None;
            if let Ok(mut pending) = self.pending.lock() {
                pending.writing = false;
                if let Err(error) = result {
                    self.omitted_total.fetch_add(1, Ordering::Relaxed);
                    pending.omitted = pending.omitted.saturating_add(omitted.saturating_add(1));
                    pending.failures = pending.failures.saturating_add(1);
                    if pending.failures.is_power_of_two() {
                        warning = Some((error.kind(), pending.failures));
                    }
                }
            }
            if let Some((error_kind, failures)) = warning {
                tracing::warn!(
                    event = "transfer.logs.write_failed",
                    ?error_kind,
                    failures,
                    "diagnostic write failed; transfers remain enabled"
                );
            }
            self.wake.notify_all();
        }
    }

    #[cfg(test)]
    fn close_and_join(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.closed = true;
        }
        self.wake.notify_all();
        if let Some(owner) = self.owner.lock().expect("log owner").take() {
            owner.join().expect("log thread");
        }
    }
}

/// `flush` submits one complete JSONL record; it never waits for the disk.
/// Diagnostic loss is disclosed by an aggregate gap in the next successful log
/// write. Transfer states and resumable bytes have independent durable owners.
pub(crate) struct SessionLogWriter {
    sink: Arc<BackgroundLog>,
    destination: Option<Arc<dyn Destination>>,
    record: Vec<u8>,
    oversized: bool,
}

/// Cloneable bounded diagnostic ingress; it never performs filesystem work.
#[derive(Clone)]
pub(crate) struct SessionLogSink {
    sink: Arc<BackgroundLog>,
    destination: Option<Arc<dyn Destination>>,
}
impl SessionLogSink {
    pub(crate) fn record(&self, record: String) {
        if let Some(destination) = &self.destination
            && record.len() <= MAX_RECORD_BYTES
        {
            self.sink.submit(Record {
                destination: destination.clone(),
                bytes: record.into_bytes(),
            });
            return;
        }
        self.sink.omit();
    }
}
impl SessionLogWriter {
    pub(crate) fn event_sink(&self) -> SessionLogSink {
        SessionLogSink {
            sink: self.sink.clone(),
            destination: self.destination.clone(),
        }
    }

    pub(crate) fn new(file: Option<File>) -> Self {
        Self {
            sink: BackgroundLog::global(),
            destination: file.map(|file| Arc::new(file) as Arc<dyn Destination>),
            record: Vec::new(),
            oversized: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_until_idle(&self) {
        let pending = self.sink.pending.lock().expect("log queue");
        let (_pending, timeout) = self
            .sink
            .wake
            .wait_timeout_while(pending, std::time::Duration::from_secs(5), |pending| {
                pending.writing || !pending.records.is_empty()
            })
            .expect("log wait");
        assert!(!timeout.timed_out(), "log worker did not drain");
    }
}

impl Write for SessionLogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.oversized {
            if self.record.len().saturating_add(bytes.len()) > MAX_RECORD_BYTES {
                self.record.clear();
                self.oversized = true;
            } else if self.destination.is_some() {
                self.record.extend_from_slice(bytes);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.oversized || self.destination.is_none() {
            self.sink.omit();
        } else if !self.record.is_empty()
            && let Some(destination) = self.destination.as_ref()
        {
            self.sink.submit(Record {
                destination: Arc::clone(destination),
                bytes: std::mem::take(&mut self.record),
            });
        }
        self.record.clear();
        self.oversized = false;
        Ok(())
    }
}

impl Drop for SessionLogWriter {
    fn drop(&mut self) {
        if self.oversized || !self.record.is_empty() {
            let _ = self.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[derive(Default)]
    struct Memory(Mutex<Vec<u8>>);

    impl Destination for Memory {
        fn append(&self, bytes: &[u8]) -> io::Result<()> {
            self.0.lock().expect("memory log").extend_from_slice(bytes);
            Ok(())
        }
    }

    struct Blocked {
        started: Mutex<Option<mpsc::SyncSender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
        memory: Arc<Memory>,
    }

    impl Destination for Blocked {
        fn append(&self, bytes: &[u8]) -> io::Result<()> {
            if let Some(started) = self.started.lock().expect("started").take() {
                started.send(()).expect("started receiver");
                self.release
                    .lock()
                    .expect("release")
                    .recv()
                    .expect("release sender");
            }
            self.memory.append(bytes)
        }
    }

    fn blocked(log: &BackgroundLog) -> (Arc<dyn Destination>, mpsc::SyncSender<()>, Arc<Memory>) {
        let (started, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let memory = Arc::new(Memory::default());
        let destination: Arc<dyn Destination> = Arc::new(Blocked {
            started: Mutex::new(Some(started)),
            release: Mutex::new(wait),
            memory: Arc::clone(&memory),
        });
        log.submit(Record {
            destination: Arc::clone(&destination),
            bytes: b"{\"event\":\"started\"}\n".to_vec(),
        });
        ready
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("writer entered controlled wait");
        (destination, release, memory)
    }

    fn records(memory: &Memory) -> Vec<serde_json::Value> {
        let bytes = memory.0.lock().expect("memory log");
        std::str::from_utf8(&bytes)
            .expect("UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("complete JSON record"))
            .collect()
    }

    #[test]
    fn blocked_disk_never_blocks_submission_and_record_loss_is_disclosed() {
        let log = BackgroundLog::start();
        let (destination, release, memory) = blocked(&log);
        let (finished, completion) = mpsc::sync_channel(1);
        let sink = Arc::clone(&log);
        let producer = thread::spawn(move || {
            let mut writer = SessionLogWriter {
                sink,
                destination: Some(destination),
                record: Vec::new(),
                oversized: false,
            };
            for sequence in 0..1000 {
                writeln!(writer, "{{\"sequence\":{sequence}}}").expect("stage");
                writer.flush().expect("submit");
            }
            writeln!(writer, "{{\"event\":\"session_finished\"}}").expect("stage terminal");
            writer.flush().expect("submit terminal");
            drop(writer);
            finished.send(()).expect("completion receiver");
        });
        let result = completion.recv_timeout(std::time::Duration::from_secs(2));
        let (queued, bytes, omitted) = {
            let pending = log.pending.lock().expect("queue");
            (pending.records.len(), pending.bytes, pending.omitted)
        };
        release.send(()).expect("release disk");
        producer.join().expect("producer");
        log.close_and_join();
        result.expect("enqueue/flush/drop cannot wait for blocked disk");
        assert_eq!(queued, RECORD_LIMIT);
        assert!(bytes <= BYTE_LIMIT);
        assert_eq!(omitted, 1001 - RECORD_LIMIT as u64);
        let records = records(&memory);
        assert_eq!(records[1]["event"], "log_records_omitted");
        assert_eq!(records[1]["scope"], "all_transfers");
        assert_eq!(records[1]["count"], omitted);
        assert_eq!(
            records.last().expect("terminal")["event"],
            "session_finished"
        );
        assert_eq!(records.len(), RECORD_LIMIT + 2);
    }

    #[test]
    fn queue_bounds_bytes_and_oversized_records_are_never_partially_written() {
        let log = BackgroundLog::start();
        let (destination, release, memory) = blocked(&log);
        let mut writer = SessionLogWriter {
            sink: Arc::clone(&log),
            destination: Some(destination),
            record: Vec::new(),
            oversized: false,
        };
        for _ in 0..20 {
            writeln!(
                writer,
                "{{\"value\":\"{}\"}}",
                "x".repeat(MAX_RECORD_BYTES - 13)
            )
            .expect("stage");
            writer.flush().expect("submit");
        }
        writer
            .write_all(&vec![b'x'; MAX_RECORD_BYTES + 1])
            .expect("oversized input");
        writer.flush().expect("discard complete oversized record");
        assert!(writer.record.is_empty());
        let (bytes, count) = {
            let pending = log.pending.lock().expect("queue");
            (pending.bytes, pending.records.len())
        };
        assert!(bytes <= BYTE_LIMIT);
        assert!(count < 20);
        drop(writer);
        release.send(()).expect("release disk");
        log.close_and_join();
        let records = records(&memory);
        assert!(
            records
                .iter()
                .any(|record| record["event"] == "log_records_omitted")
        );
        assert_eq!(records.len(), count + 2);
    }

    #[test]
    fn failed_destination_does_not_stop_other_logs() {
        struct Failed;
        impl Destination for Failed {
            fn append(&self, _: &[u8]) -> io::Result<()> {
                Err(io::ErrorKind::StorageFull.into())
            }
        }
        let log = BackgroundLog::start();
        let memory = Arc::new(Memory::default());
        log.submit(Record {
            destination: Arc::new(Failed),
            bytes: b"{\"failed\":true}\n".to_vec(),
        });
        log.submit(Record {
            destination: memory.clone(),
            bytes: b"{\"event\":\"session_finished\"}\n".to_vec(),
        });
        log.close_and_join();
        let records = records(&memory);
        assert_eq!(records[0]["count"], 1);
        assert_eq!(records[1]["event"], "session_finished");
    }
}
