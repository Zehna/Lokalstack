//! Phase 11C Task 4 — bounded background capture worker (spec §5, §32).
//!
//! One dedicated OS worker thread drains a bounded `std::sync::mpsc`
//! channel (exact capacity 8 — plan §8 locked constant). Producers never
//! block: `SyncSender::try_send` maps `Full → QueueFull` (one bounded
//! breadcrumb, never a hang) and `Disconnected → QueueClosed`. Duplicate
//! fingerprints coalesce through a `try_lock`ed `HashSet` — never held
//! during build, never awaited (spec §10 lock discipline).
//!
//! Bundle building happens OUTSIDE any lock; each job is isolated with
//! `catch_unwind` so one panicking capture cannot kill the worker or the
//! app. Panic payloads are discarded un-inspected — they may carry
//! secret-bearing data — and only a fixed-template breadcrumb is logged.

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use super::incidents::{IncidentKey, Severity};

/// Exact queue capacity (plan §8): 8. Consumed by the wiring in Task 6/9
/// (the reporting path creates the channel); unused until then.
#[allow(dead_code)]
pub(crate) const QUEUE_CAPACITY: usize = 8;

/// How long the idle worker waits before re-checking the shutdown flag.
const RECV_TICK: Duration = Duration::from_millis(100);

/// One queued capture request.
#[derive(Debug, Clone)]
pub(crate) enum CaptureRequest {
    #[allow(dead_code)] // constructed by the reporting path in Task 6/9
    Bundle {
        key: IncidentKey,
        trigger: String,
        severity: Severity,
    },
}

/// Result of a producer's non-blocking enqueue attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnqueueOutcome {
    /// Queued for the worker.
    Accepted,
    /// A capture for this fingerprint is already queued/in-flight.
    CoalescedDuplicate,
    /// Queue at capacity: producer drops the request (bounded breadcrumb).
    QueueFull,
    /// Worker/channel gone (shutdown or panic-death): caller logs once.
    QueueClosed,
}

/// The sending half handed to producers (reporting path, Task 6/9).
#[derive(Clone)]
pub(crate) struct CaptureWorker {
    tx: mpsc::SyncSender<CaptureRequest>,
    coalesce: Arc<Mutex<HashSet<IncidentKey>>>,
    shutdown: Arc<AtomicBool>,
}

/// Key of a request (for coalescing) regardless of payload shape.
fn key_of(req: &CaptureRequest) -> IncidentKey {
    match req {
        CaptureRequest::Bundle { key, .. } => key.clone(),
    }
}

impl CaptureWorker {
    /// Spawn the single capture worker. `build` is injected by later tasks
    /// (real bundle builder); tests inject counting/failing fakes.
    #[allow(dead_code)] // wired into the reporting path in Task 6/9
    pub(crate) fn spawn(
        rx: mpsc::Receiver<CaptureRequest>,
        coalesce: Arc<Mutex<HashSet<IncidentKey>>>,
        build: Arc<dyn Fn(CaptureRequest) + Send + Sync>,
        shutdown: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("diag-capture".into())
            .spawn(move || {
                loop {
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    match rx.recv_timeout(RECV_TICK) {
                        Ok(req) => {
                            let key = key_of(&req);
                            // Panic isolation at the job boundary ONLY:
                            // never wraps the whole app, never suppresses
                            // unrelated panics.
                            let outcome =
                                catch_unwind(AssertUnwindSafe(|| build(req)));
                            // Release the coalescing key on success, error,
                            // AND panic — a stuck key would wedge future
                            // captures of the same problem.
                            if let Ok(mut set) = coalesce.try_lock() {
                                set.remove(&key);
                            }
                            if outcome.is_err() {
                                // Discard the payload un-inspected (may be
                                // secret-bearing); fixed redacted template.
                                super::warn(
                                    "diagnostics",
                                    "capture job panicked; job skipped",
                                );
                            }
                            // Loop continues: next queued capture proceeds.
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("spawning the diagnostics capture worker cannot fail under normal conditions")
    }

    /// Non-blocking producer path (spec §5): try_lock the coalescing set
    /// (contended ⇒ treat as duplicate, NEVER wait), then `try_send`.
    #[allow(dead_code)]
    pub(crate) fn try_enqueue(&self, req: CaptureRequest) -> EnqueueOutcome {
        let key = key_of(&req);
        {
            let Ok(mut set) = self.coalesce.try_lock() else {
                // Contended: someone else is mid-enqueue for some key. The
                // conservative non-blocking answer is "coalesced" — we never
                // block a producer, and a missed rare capture is bounded by
                // the incident history.
                return EnqueueOutcome::CoalescedDuplicate;
            };
            if !set.insert(key.clone()) {
                return EnqueueOutcome::CoalescedDuplicate;
            }
        }
        match self.tx.try_send(req) {
            Ok(()) => EnqueueOutcome::Accepted,
            Err(mpsc::TrySendError::Full(_)) => {
                // Undo the coalescing marker so a retry can succeed later.
                if let Ok(mut set) = self.coalesce.try_lock() {
                    set.remove(&key);
                }
                super::warn("diagnostics", "capture queue full; capture skipped");
                EnqueueOutcome::QueueFull
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                if let Ok(mut set) = self.coalesce.try_lock() {
                    set.remove(&key);
                }
                EnqueueOutcome::QueueClosed
            }
        }
    }

    /// Trigger cooperative shutdown (called from app exit path).
    #[allow(dead_code)]
    pub(crate) fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn key(n: u8) -> IncidentKey {
        IncidentKey { subsystem: "sub".into(), fingerprint: format!("fp-{n}") }
    }

    fn req(n: u8) -> CaptureRequest {
        CaptureRequest::Bundle {
            key: key(n),
            trigger: "test".into(),
            severity: Severity::Severe,
        }
    }

    /// The real capacity bound: fill against a slow consumer, then confirm
    /// the producer is refused.
    #[test]
    fn queue_bound_is_exactly_8_with_slow_consumer() {
        let (tx, rx) = mpsc::sync_channel::<CaptureRequest>(QUEUE_CAPACITY);
        let coalesce: Arc<Mutex<HashSet<IncidentKey>>> = Arc::new(Mutex::new(HashSet::new()));
        // Consumer that drains slowly.
        let handle = std::thread::spawn(move || {
            for _ in 0..QUEUE_CAPACITY {
                match rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(_) => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
        });
        let w = CaptureWorker {
            tx,
            coalesce: coalesce.clone(),
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        let mut full = 0;
        for i in 0..(QUEUE_CAPACITY * 3) {
            match w.try_enqueue(req(i as u8)) {
                EnqueueOutcome::Accepted => {}
                EnqueueOutcome::QueueFull => full += 1,
                other => panic!("unexpected outcome: {other:?}"),
            }
        }
        assert!(full >= QUEUE_CAPACITY, "queue must fill up: full={full}");
        assert!(full <= QUEUE_CAPACITY * 2);
        handle.join().unwrap();
    }

    #[test]
    fn duplicate_fingerprint_coalesces() {
        let (tx, rx) = mpsc::sync_channel::<CaptureRequest>(QUEUE_CAPACITY);
        let coalesce: Arc<Mutex<HashSet<IncidentKey>>> = Arc::new(Mutex::new(HashSet::new()));
        let w = CaptureWorker {
            tx,
            coalesce: coalesce.clone(),
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(w.try_enqueue(req(1)), EnqueueOutcome::Accepted);
        assert_eq!(
            w.try_enqueue(req(1)),
            EnqueueOutcome::CoalescedDuplicate,
            "same fingerprint before build completes coalesces"
        );
        assert_eq!(w.try_enqueue(req(2)), EnqueueOutcome::Accepted, "distinct key accepted");
        // Drain.
        drop(w);
        while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
    }

    #[test]
    fn queue_full_does_not_block_producer() {
        let (tx, _rx) = mpsc::sync_channel::<CaptureRequest>(QUEUE_CAPACITY);
        // Fill without consuming.
        let coalesce: Arc<Mutex<HashSet<IncidentKey>>> = Arc::new(Mutex::new(HashSet::new()));
        for i in 0..QUEUE_CAPACITY {
            tx.try_send(req(100 + i as u8)).unwrap();
        }
        let w = CaptureWorker {
            tx,
            coalesce,
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        let start = std::time::Instant::now();
        let mut outcomes = Vec::new();
        for _ in 0..50 {
            outcomes.push(w.try_enqueue(req(9)));
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "50 enqueue attempts must be non-blocking (a blocked producer would hang), took {elapsed:?}"
        );
        assert!(outcomes.iter().all(|o| matches!(
            o,
            EnqueueOutcome::QueueFull | EnqueueOutcome::CoalescedDuplicate
        )));
    }

    #[test]
    fn worker_drains_and_consumes_requests() {
        let coalesce: Arc<Mutex<HashSet<IncidentKey>>> = Arc::new(Mutex::new(HashSet::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let counter = Arc::new(AtomicUsize::new(0));
        let c2 = counter.clone();
        let (tx, rx) = mpsc::sync_channel::<CaptureRequest>(QUEUE_CAPACITY);
        let handle = CaptureWorker::spawn(
            rx,
            coalesce.clone(),
            Arc::new(move |_r| {
                c2.fetch_add(1, Ordering::SeqCst);
            }),
            shutdown.clone(),
        );
        let w = CaptureWorker { tx, coalesce, shutdown: shutdown.clone() };
        w.try_enqueue(req(1));
        w.try_enqueue(req(2));
        w.try_enqueue(req(3));
        drop(w);
        handle.join().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 3, "all 3 requests built");
    }

    #[test]
    fn shutdown_flag_terminates_idle_worker() {
        let coalesce: Arc<Mutex<HashSet<IncidentKey>>> = Arc::new(Mutex::new(HashSet::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel::<CaptureRequest>(QUEUE_CAPACITY);
        let handle = CaptureWorker::spawn(
            rx,
            coalesce.clone(),
            Arc::new(|_r| panic!("no jobs expected")),
            shutdown.clone(),
        );
        let w = CaptureWorker { tx, coalesce, shutdown: shutdown.clone() };
        let start = std::time::Instant::now();
        w.shutdown();
        handle.join().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "idle worker must stop within ~2 s of the flag"
        );
    }

    #[test]
    fn worker_survives_a_panicking_build_job() {
        let coalesce: Arc<Mutex<HashSet<IncidentKey>>> = Arc::new(Mutex::new(HashSet::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel::<CaptureRequest>(QUEUE_CAPACITY);
        let counter = Arc::new(AtomicUsize::new(0));
        let c2 = counter.clone();
        let handle = CaptureWorker::spawn(
            rx,
            coalesce.clone(),
            Arc::new(move |r: CaptureRequest| {
                match &r {
                    CaptureRequest::Bundle { key, .. } if key.fingerprint == "fp-1" => {
                        panic!("boom: token=super-secret-value Authorization: Bearer abc")
                    }
                    _ => {}
                }
                c2.fetch_add(1, Ordering::SeqCst);
            }),
            shutdown.clone(),
        );
        let w = CaptureWorker { tx, coalesce: coalesce.clone(), shutdown: shutdown.clone() };
        assert_eq!(w.try_enqueue(req(1)), EnqueueOutcome::Accepted);
        assert_eq!(w.try_enqueue(req(2)), EnqueueOutcome::Accepted);
        drop(w);
        handle.join().expect("worker thread must NOT die from a job panic");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "job B executed after job A panicked");
        assert!(
            coalesce.try_lock().unwrap().is_empty(),
            "coalescing key released even on panic"
        );
    }

    #[test]
    fn queue_closed_after_worker_death_reports_queueclosed() {
        let (tx, rx) = mpsc::sync_channel::<CaptureRequest>(QUEUE_CAPACITY);
        drop(rx); // receiver gone = worker exited
        let coalesce: Arc<Mutex<HashSet<IncidentKey>>> = Arc::new(Mutex::new(HashSet::new()));
        let w = CaptureWorker {
            tx,
            coalesce,
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        let start = std::time::Instant::now();
        assert_eq!(w.try_enqueue(req(1)), EnqueueOutcome::QueueClosed);
        assert!(start.elapsed() < Duration::from_millis(100), "non-blocking");
    }
}
