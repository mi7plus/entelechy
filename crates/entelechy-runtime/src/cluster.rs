//! Cluster execution: a leased work queue and worker pool (PRD 17, 17.4, RK-1).
//!
//! The cluster topology runs a queue plus a worker pool (PRD 17 storage table).
//! This module provides the *coordination model* — leasing with at-least-once
//! redelivery, priority-based load shedding (17.4), and a bounded worker pool with
//! graceful shutdown (RK-1 structured concurrency) — behind the [`WorkQueue`]
//! trait. The in-memory reference queue is fully testable; a Redis/SQS/Postgres
//! backend implements the same trait (as `wasmtime` does for the sandbox).
//!
//! At-least-once delivery is safe because consequential effects carry operation
//! keys (see `entelechy-effects`): a redelivered item that repeats an effect is
//! deduplicated or reconciled rather than double-committed (PRD 7.6/8.5).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Work priority for load shedding (PRD 17.4): reconciliation, revocation,
/// approval and safety-critical control-plane work is served before new
/// optimization work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Priority {
    /// Reconciliation / revocation / approval / safety-critical (served first).
    SafetyCritical,
    /// Other control-plane work.
    Control,
    /// New optimization work (shed first under load).
    Normal,
}

/// A unit of distributed work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkItem {
    /// Stable work id (idempotency handle for at-least-once processing).
    pub id: String,
    /// Scheduling priority (PRD 17.4).
    pub priority: Priority,
    /// Opaque payload (e.g. a run/eval request).
    pub payload: serde_json::Value,
}

/// A leased item: held by a worker until acked or its lease expires.
#[derive(Clone, Debug, PartialEq)]
pub struct LeasedItem {
    /// The work item.
    pub item: WorkItem,
    /// Unix-seconds lease expiry.
    pub lease_expires_at: u64,
}

/// A distributed work queue with visibility leases (PRD 17). Implementations must
/// provide at-least-once delivery: an unacked item whose lease expires is
/// redelivered.
pub trait WorkQueue: Send + Sync {
    /// Enqueue an item.
    fn enqueue(&self, item: WorkItem);
    /// Lease the highest-priority available item for `lease_secs` (load shedding —
    /// PRD 17.4). Reclaims expired leases first.
    fn lease(&self, lease_secs: u64, now: u64) -> Option<LeasedItem>;
    /// Acknowledge successful processing; the item is removed.
    fn ack(&self, id: &str);
    /// Negative-ack: return the item to the available set for retry.
    fn nack(&self, id: &str);
    /// Redeliver any leased items whose lease has expired (at-least-once).
    fn reclaim_expired(&self, now: u64);
    /// Number of items not yet acked (available + leased).
    fn len(&self) -> usize;
}

#[derive(Default)]
struct QueueState {
    available: Vec<WorkItem>,
    leased: HashMap<String, LeasedItem>,
}

/// An in-process reference [`WorkQueue`] (Arc-shareable across worker threads).
#[derive(Default)]
pub struct InMemoryQueue {
    state: Mutex<QueueState>,
}

impl InMemoryQueue {
    /// Create an empty queue.
    pub fn new() -> Self {
        Self::default()
    }
}

impl WorkQueue for InMemoryQueue {
    fn enqueue(&self, item: WorkItem) {
        self.state.lock().unwrap().available.push(item);
    }

    fn lease(&self, lease_secs: u64, now: u64) -> Option<LeasedItem> {
        let mut s = self.state.lock().unwrap();
        reclaim(&mut s, now);
        // Highest priority first (SafetyCritical < Control < Normal by Ord), then
        // FIFO within a priority.
        let idx = s
            .available
            .iter()
            .enumerate()
            .min_by_key(|(i, it)| (it.priority, *i))
            .map(|(i, _)| i)?;
        let item = s.available.remove(idx);
        let leased = LeasedItem {
            item,
            lease_expires_at: now + lease_secs,
        };
        s.leased.insert(leased.item.id.clone(), leased.clone());
        Some(leased)
    }

    fn ack(&self, id: &str) {
        self.state.lock().unwrap().leased.remove(id);
    }

    fn nack(&self, id: &str) {
        let mut s = self.state.lock().unwrap();
        if let Some(leased) = s.leased.remove(id) {
            s.available.push(leased.item);
        }
    }

    fn reclaim_expired(&self, now: u64) {
        reclaim(&mut self.state.lock().unwrap(), now);
    }

    fn len(&self) -> usize {
        let s = self.state.lock().unwrap();
        s.available.len() + s.leased.len()
    }
}

fn reclaim(s: &mut QueueState, now: u64) {
    let expired: Vec<String> = s
        .leased
        .iter()
        .filter(|(_, l)| l.lease_expires_at <= now)
        .map(|(id, _)| id.clone())
        .collect();
    for id in expired {
        if let Some(l) = s.leased.remove(&id) {
            s.available.push(l.item);
        }
    }
}

/// Current wall-clock time in unix seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A bounded pool of worker threads draining a [`WorkQueue`] (PRD 17, RK-1).
pub struct WorkerPool {
    handles: Vec<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl WorkerPool {
    /// Spawn `workers` threads that lease items and run `handler`. A handler
    /// returning `true` acks the item; `false` nacks it for retry. Workers exit on
    /// [`WorkerPool::shutdown`] (structured shutdown — RK-1).
    pub fn spawn<Q, H>(queue: Arc<Q>, workers: usize, lease_secs: u64, handler: Arc<H>) -> Self
    where
        Q: WorkQueue + 'static,
        H: Fn(&WorkItem) -> bool + Send + Sync + 'static,
    {
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let q = Arc::clone(&queue);
            let h = Arc::clone(&handler);
            let stop = Arc::clone(&shutdown);
            handles.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match q.lease(lease_secs, now_secs()) {
                        Some(leased) => {
                            let ok = h(&leased.item);
                            if ok {
                                q.ack(&leased.item.id);
                            } else {
                                q.nack(&leased.item.id);
                            }
                        }
                        None => std::thread::sleep(Duration::from_millis(2)),
                    }
                }
            }));
        }
        Self { handles, shutdown }
    }

    /// Signal shutdown and join all workers (RK-1 structured concurrency).
    pub fn shutdown(self) {
        self.shutdown.store(true, Ordering::Relaxed);
        for h in self.handles {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, priority: Priority) -> WorkItem {
        WorkItem {
            id: id.into(),
            priority,
            payload: serde_json::Value::Null,
        }
    }

    #[test]
    fn load_shedding_serves_safety_critical_first() {
        let q = InMemoryQueue::new();
        q.enqueue(item("opt", Priority::Normal));
        q.enqueue(item("reconcile", Priority::SafetyCritical));
        q.enqueue(item("ctrl", Priority::Control));
        // Highest priority leased first (PRD 17.4).
        assert_eq!(q.lease(30, 0).unwrap().item.id, "reconcile");
        assert_eq!(q.lease(30, 0).unwrap().item.id, "ctrl");
        assert_eq!(q.lease(30, 0).unwrap().item.id, "opt");
    }

    #[test]
    fn expired_lease_is_redelivered_at_least_once() {
        let q = InMemoryQueue::new();
        q.enqueue(item("w", Priority::Normal));
        let leased = q.lease(10, 100).unwrap();
        assert_eq!(leased.lease_expires_at, 110);
        // Not acked; before expiry nothing else is available.
        assert!(q.lease(10, 105).is_none());
        // After expiry it is redelivered.
        assert_eq!(q.lease(10, 200).unwrap().item.id, "w");
    }

    #[test]
    fn ack_removes_and_nack_retries() {
        let q = InMemoryQueue::new();
        q.enqueue(item("a", Priority::Normal));
        let l = q.lease(30, 0).unwrap();
        q.ack(&l.item.id);
        assert_eq!(q.len(), 0);

        q.enqueue(item("b", Priority::Normal));
        let l = q.lease(30, 0).unwrap();
        q.nack(&l.item.id);
        assert_eq!(q.lease(30, 0).unwrap().item.id, "b");
    }

    #[test]
    fn worker_pool_drains_the_queue() {
        let q = Arc::new(InMemoryQueue::new());
        for i in 0..50 {
            q.enqueue(item(&format!("job-{i}"), Priority::Normal));
        }
        let processed = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&processed);
        let handler = Arc::new(move |it: &WorkItem| {
            sink.lock().unwrap().push(it.id.clone());
            true
        });
        let pool = WorkerPool::spawn(Arc::clone(&q), 4, 30, handler);

        // Wait until drained or timeout.
        let start = std::time::Instant::now();
        while q.len() > 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        pool.shutdown();

        assert_eq!(q.len(), 0);
        assert_eq!(processed.lock().unwrap().len(), 50);
    }
}
