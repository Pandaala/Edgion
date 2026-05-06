//! Workqueue - Go controller-runtime style work queue implementation
//!
//! Provides a generic workqueue with:
//! - Deduplication: same key only exists once in queue
//! - Delayed enqueue: cross-resource requeue with coalescing via integrated DelayQueue
//! - Trigger chain: cascade path tracking with cycle detection
//! - Metrics: queue depth, adds
//!
//! Key design decision: We don't track "processing" state. When a key is dequeued,
//! it's simply removed from pending. This allows new enqueue requests during processing
//! to succeed, ensuring updates are not lost (dirty requeue pattern).
//!
//! Note: Exponential backoff retry (`requeue_with_backoff`) is intentionally NOT implemented.
//! Status write failures (e.g. K8s API transiently unavailable) are self-healing: the watcher
//! reconnect triggers a full cache rebuild which re-processes and re-writes all statuses.
//! Accurate identification of transient vs permanent failures is not practical at this layer.

use dashmap::mapref::entry::Entry;
use dashmap::{DashMap, DashSet};
use smallvec::SmallVec;
use std::cmp::Ordering as CmpOrdering;
use std::collections::BinaryHeap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;

// ==================== Trigger Chain ====================

/// Single trigger source in the cascade chain
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TriggerSource {
    /// Resource kind that triggered this enqueue (e.g. "HTTPRoute", "Gateway")
    pub kind: &'static str,
    /// Resource key that triggered this enqueue (e.g. "default/my-route")
    pub key: String,
}

/// Trigger chain tracking cascade path (like X-Forwarded-For for requeue events).
///
/// Records the sequence of resources that caused cascading requeues.
/// Used for cycle detection: if the same (kind, key) pair appears too many times,
/// the cascade is terminated.
#[derive(Debug, Clone, Default)]
pub struct TriggerChain {
    /// Chain of trigger sources, from oldest to newest.
    /// SmallVec avoids heap allocation for typical chains (<=4 hops).
    pub sources: SmallVec<[TriggerSource; 4]>,
}

impl TriggerChain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Extend chain with the current processor's info.
    /// Returns a new chain with the source appended.
    pub fn extend(&self, kind: &'static str, key: &str) -> Self {
        let mut new = self.clone();
        new.sources.push(TriggerSource {
            kind,
            key: key.to_string(),
        });
        new
    }

    /// Count how many times (kind, key) appears in the chain
    pub fn occurrence_count(&self, kind: &str, key: &str) -> usize {
        self.sources.iter().filter(|s| s.kind == kind && s.key == key).count()
    }

    /// Total cascade depth
    pub fn depth(&self) -> usize {
        self.sources.len()
    }

    /// Check if enqueueing target would exceed the cycle limit
    pub fn would_exceed_cycle_limit(&self, target_kind: &str, target_key: &str, max_cycles: usize) -> bool {
        self.occurrence_count(target_kind, target_key) >= max_cycles
    }
}

impl fmt::Display for TriggerChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.sources.iter().enumerate() {
            if i > 0 {
                write!(f, " -> ")?;
            }
            write!(f, "{}/{}", s.kind, s.key)?;
        }
        Ok(())
    }
}

// ==================== Work Item ====================

/// Work item in the queue
#[derive(Debug, Clone)]
pub struct WorkItem {
    /// Resource key: "namespace/name" for namespaced, "name" for cluster-scoped
    pub key: String,
    /// Time when item was enqueued
    pub enqueue_time: Instant,
    /// Trigger chain for cascade tracking
    pub trigger_chain: TriggerChain,
}

impl WorkItem {
    /// Create a new work item (original event, empty chain)
    pub fn new(key: String) -> Self {
        Self {
            key,
            enqueue_time: Instant::now(),
            trigger_chain: TriggerChain::default(),
        }
    }

    /// Create a work item with a trigger chain (cross-resource requeue)
    pub fn with_chain(key: String, chain: TriggerChain) -> Self {
        Self {
            key,
            enqueue_time: Instant::now(),
            trigger_chain: chain,
        }
    }
}

// ==================== Pending entry ====================

/// State stored alongside a pending key.
///
/// `chain` is the cascade history merged across all enqueue paths that converged
/// on this key while it was pending. `enqueue_time` is the time of the first
/// enqueue that caused the entry to be created.
#[derive(Debug, Clone)]
struct PendingEntry {
    enqueue_time: Instant,
    chain: TriggerChain,
}

// ==================== Delay Queue internals ====================

/// Item waiting in the delay heap
struct DelayedItem {
    key: String,
    chain: TriggerChain,
    ready_at: Instant,
}

impl PartialEq for DelayedItem {
    fn eq(&self, other: &Self) -> bool {
        self.ready_at == other.ready_at
    }
}

impl Eq for DelayedItem {}

impl PartialOrd for DelayedItem {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for DelayedItem {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Reverse ordering: earliest ready_at has highest priority (min-heap via BinaryHeap)
        other.ready_at.cmp(&self.ready_at)
    }
}

// ==================== Configuration ====================

/// Configuration for workqueue
#[derive(Debug, Clone)]
pub struct WorkqueueConfig {
    /// Queue capacity (bounded channel size)
    pub capacity: usize,
    /// Default delay for cross-resource requeue coalescing
    pub default_requeue_delay: Duration,
    /// Maximum times a (kind, key) pair may appear in a trigger chain before the cascade is stopped
    pub max_trigger_cycles: usize,
    /// Maximum total depth of a trigger chain (safety net)
    pub max_trigger_depth: usize,
}

impl Default for WorkqueueConfig {
    fn default() -> Self {
        Self {
            capacity: 5000,
            default_requeue_delay: Duration::from_millis(100),
            max_trigger_cycles: 5,
            max_trigger_depth: 20,
        }
    }
}

// ==================== Metrics ====================

/// Metrics for workqueue monitoring
#[derive(Debug, Default)]
pub struct WorkqueueMetrics {
    /// Total number of items added to queue
    pub adds_total: AtomicU64,
    /// Number of items currently in queue (pending)
    pub depth: AtomicU64,
    /// Total number of delayed items scheduled
    pub delayed_total: AtomicU64,
}

impl WorkqueueMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inc_adds(&self) {
        self.adds_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_depth(&self) {
        self.depth.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_depth(&self) {
        self.depth.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn get_depth(&self) -> u64 {
        self.depth.load(Ordering::Relaxed)
    }

    pub fn get_adds_total(&self) -> u64 {
        self.adds_total.load(Ordering::Relaxed)
    }

    pub fn inc_delayed(&self) {
        self.delayed_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_delayed_total(&self) -> u64 {
        self.delayed_total.load(Ordering::Relaxed)
    }
}

// ==================== Workqueue ====================

/// Generic workqueue for resource reconciliation
///
/// Each resource type should have its own workqueue instance.
/// This provides isolation between different resource types.
///
/// Design: We only track pending keys (not processing). When dequeue happens,
/// the key is removed from pending immediately. This allows new enqueue requests
/// during processing to succeed and be queued, ensuring dirty updates are not lost.
///
/// The integrated delay subsystem supports `enqueue_after` for cross-resource requeue
/// coalescing. Delayed items are held in a background task's priority queue and moved
/// to the ready queue when their delay expires.
///
/// **Requirement**: Must be constructed within a tokio runtime context (spawns a
/// background task for the delay loop).
pub struct Workqueue {
    /// Queue name (for logging and metrics)
    name: String,
    /// Sender for ready key notifications. Only the key travels through the
    /// channel; the associated `PendingEntry` (enqueue time + trigger chain)
    /// lives in `pending` so that converging enqueue paths can merge their
    /// chains rather than racing through the channel as separate items.
    tx: mpsc::Sender<String>,
    /// Receiver for dequeueing keys (wrapped in Mutex for single consumer)
    rx: Mutex<mpsc::Receiver<String>>,
    /// Keys currently in the ready queue with their merged trigger chain
    /// (for deduplication and cascade-history preservation). Wrapped in Arc
    /// so the delay loop shares the same map.
    pending: Arc<DashMap<String, PendingEntry>>,
    /// Keys currently scheduled in the delay queue (dedup at schedule time).
    /// Wrapped in Arc so delay_loop shares the same set.
    scheduled: Arc<DashSet<String>>,
    /// Sender for delayed items
    delay_tx: mpsc::Sender<DelayedItem>,
    /// Configuration
    config: WorkqueueConfig,
    /// Metrics
    metrics: Arc<WorkqueueMetrics>,
}

impl Workqueue {
    /// Create a new workqueue with the given name and configuration.
    ///
    /// Spawns a background tokio task for the delay loop.
    /// **Must be called within a tokio runtime context.**
    pub fn new(name: &str, config: WorkqueueConfig) -> Self {
        let (tx, rx) = mpsc::channel(config.capacity);
        let (delay_tx, delay_rx) = mpsc::channel(config.capacity);
        let pending = Arc::new(DashMap::new());
        let scheduled = Arc::new(DashSet::new());
        let metrics = Arc::new(WorkqueueMetrics::new());

        Self::spawn_delay_loop(
            name.to_string(),
            delay_rx,
            tx.clone(),
            pending.clone(),
            scheduled.clone(),
            metrics.clone(),
        );

        Self {
            name: name.to_string(),
            tx,
            rx: Mutex::new(rx),
            pending,
            scheduled,
            delay_tx,
            config,
            metrics,
        }
    }

    /// Create a new workqueue with default configuration
    pub fn with_defaults(name: &str) -> Self {
        Self::new(name, WorkqueueConfig::default())
    }

    /// Background task that manages the delay heap.
    ///
    /// Receives delayed items via `delay_rx`, holds them in a min-heap sorted by
    /// `ready_at`, and moves them to the ready queue (`ready_tx`) when their time comes.
    fn spawn_delay_loop(
        name: String,
        mut delay_rx: mpsc::Receiver<DelayedItem>,
        ready_tx: mpsc::Sender<String>,
        pending: Arc<DashMap<String, PendingEntry>>,
        scheduled: Arc<DashSet<String>>,
        metrics: Arc<WorkqueueMetrics>,
    ) {
        tokio::spawn(async move {
            let mut heap: BinaryHeap<DelayedItem> = BinaryHeap::new();

            loop {
                if heap.is_empty() {
                    match delay_rx.recv().await {
                        Some(item) => heap.push(item),
                        None => break, // channel closed, exit
                    }
                } else {
                    let next_ready = heap.peek().unwrap().ready_at;
                    let sleep_dur = next_ready.saturating_duration_since(Instant::now());

                    tokio::select! {
                        _ = sleep(sleep_dur) => {
                            let now = Instant::now();
                            while let Some(top) = heap.peek() {
                                if top.ready_at <= now {
                                    let item = heap.pop().unwrap();
                                    scheduled.remove(&item.key);

                                    promote_delayed_item(&name, item, &pending, &ready_tx, &metrics).await;
                                } else {
                                    break;
                                }
                            }
                        }
                        recv_result = delay_rx.recv() => {
                            match recv_result {
                                Some(item) => heap.push(item),
                                None => {
                                    // Channel closed. Drain remaining items.
                                    let now = Instant::now();
                                    while let Some(item) = heap.pop() {
                                        scheduled.remove(&item.key);
                                        if item.ready_at <= now {
                                            promote_delayed_item(&name, item, &pending, &ready_tx, &metrics).await;
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            tracing::debug!(queue = %name, "Delay loop exited");
        });
    }

    /// Get the queue name
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Promote a delayed item to the ready queue.
///
/// On collision (key already pending from a concurrent immediate enqueue) the
/// trigger chain is **merged** into the existing pending entry rather than
/// dropped. The deeper of the two chains wins. This preserves cascade history
/// for cycle detection — see `c3a-008`. No new mpsc message is sent on
/// collision because the existing pending entry already has one in flight.
async fn promote_delayed_item(
    name: &str,
    item: DelayedItem,
    pending: &DashMap<String, PendingEntry>,
    ready_tx: &mpsc::Sender<String>,
    metrics: &WorkqueueMetrics,
) {
    let needs_send = match pending.entry(item.key.clone()) {
        Entry::Vacant(v) => {
            v.insert(PendingEntry {
                enqueue_time: Instant::now(),
                chain: item.chain,
            });
            metrics.inc_depth();
            metrics.inc_adds();
            true
        }
        Entry::Occupied(mut o) => {
            if item.chain.depth() > o.get().chain.depth() {
                let prev_depth = o.get().chain.depth();
                o.get_mut().chain = item.chain;
                tracing::trace!(
                    queue = %name,
                    key = %o.key(),
                    prev_depth = prev_depth,
                    new_depth = o.get().chain.depth(),
                    "Delayed item collided with pending; merged deeper chain"
                );
            } else {
                tracing::trace!(
                    queue = %name,
                    key = %o.key(),
                    "Delayed item collided with pending; existing chain is at least as deep"
                );
            }
            false
        }
    };

    if needs_send && ready_tx.send(item.key.clone()).await.is_err() {
        pending.remove(&item.key);
        metrics.dec_depth();
        tracing::error!(
            queue = %name,
            key = %item.key,
            "Failed to move delayed item to ready queue"
        );
    }
}

impl Workqueue {
    /// Get metrics reference
    pub fn metrics(&self) -> Arc<WorkqueueMetrics> {
        self.metrics.clone()
    }

    /// Enqueue a key for immediate processing.
    ///
    /// Returns true if the key was added, false if it was already in the queue (deduplicated).
    ///
    /// Note: We only check pending, not processing. This allows enqueueing during processing,
    /// which enables dirty requeue - if an update arrives while processing, it will be queued
    /// and processed again after the current processing completes.
    pub async fn enqueue(&self, key: String) -> bool {
        let was_inserted = match self.pending.entry(key.clone()) {
            Entry::Vacant(v) => {
                v.insert(PendingEntry {
                    enqueue_time: Instant::now(),
                    chain: TriggerChain::default(),
                });
                true
            }
            Entry::Occupied(_) => false,
        };

        if !was_inserted {
            tracing::trace!(
                queue = %self.name,
                key = %key,
                "Key already in queue, skipping"
            );
            return false;
        }

        self.metrics.inc_depth();
        let depth = self.metrics.get_depth();
        let capacity = self.config.capacity as u64;
        if capacity > 0 && depth * 100 >= capacity * 80 && depth % 500 == 0 {
            tracing::warn!(
                queue = %self.name,
                depth = depth,
                capacity = capacity,
                "Workqueue depth is above 80% of capacity"
            );
        }

        match self.tx.send(key.clone()).await {
            Ok(()) => {
                self.metrics.inc_adds();
                tracing::debug!(
                    queue = %self.name,
                    key = %key,
                    depth = self.metrics.get_depth(),
                    "Enqueued item"
                );
                true
            }
            Err(e) => {
                self.pending.remove(&key);
                self.metrics.dec_depth();
                tracing::error!(
                    queue = %self.name,
                    key = %key,
                    error = %e,
                    "Failed to enqueue item"
                );
                false
            }
        }
    }

    /// Enqueue a key with a delay (for cross-resource requeue coalescing).
    ///
    /// The key is held in a background delay heap and moved to the ready queue after
    /// `delay` elapses. Deduplication is via `scheduled` at schedule time; if the
    /// key is already in `pending` when the delay fires, the promotion is silently
    /// dropped by the delay loop.
    ///
    /// Returns true if the key was scheduled, false if skipped due to dedup.
    pub async fn enqueue_after(&self, key: String, delay: Duration, chain: TriggerChain) -> bool {
        if !self.scheduled.insert(key.clone()) {
            tracing::trace!(
                queue = %self.name,
                key = %key,
                "Key already scheduled, skipping enqueue_after"
            );
            return false;
        }
        self.metrics.inc_delayed();
        let outstanding = (self.pending.len() + self.scheduled.len()) as u64;
        let capacity = self.config.capacity as u64;
        if capacity > 0 && outstanding * 100 >= capacity * 80 && outstanding % 500 == 0 {
            tracing::warn!(
                queue = %self.name,
                pending = self.pending.len(),
                scheduled = self.scheduled.len(),
                capacity = capacity,
                "Workqueue pending+scheduled load is above 80% of capacity"
            );
        }

        let delayed = DelayedItem {
            key: key.clone(),
            chain,
            ready_at: Instant::now() + delay,
        };

        match self.delay_tx.send(delayed).await {
            Ok(()) => {
                tracing::debug!(
                    queue = %self.name,
                    key = %key,
                    delay_ms = delay.as_millis(),
                    "Scheduled delayed enqueue"
                );
                true
            }
            Err(_) => {
                self.scheduled.remove(&key);
                tracing::error!(
                    queue = %self.name,
                    key = %key,
                    "Failed to schedule delayed enqueue (delay channel closed)"
                );
                false
            }
        }
    }

    /// Dequeue an item for processing.
    ///
    /// This will block until an item is available.
    /// Returns None if the queue is closed.
    ///
    /// Note: The key is removed from pending immediately, allowing new enqueue
    /// requests for the same key during processing.
    pub async fn dequeue(&self) -> Option<WorkItem> {
        let mut rx = self.rx.lock().await;
        let key = rx.recv().await?;

        let (enqueue_time, trigger_chain) = match self.pending.remove(&key) {
            Some((_, entry)) => {
                self.metrics.dec_depth();
                (entry.enqueue_time, entry.chain)
            }
            None => {
                // Defensive: pending entry should exist for any key in the channel,
                // but if it's missing (e.g. send-after-remove race on shutdown),
                // fall back to an empty WorkItem rather than dropping the event.
                tracing::warn!(
                    queue = %self.name,
                    key = %key,
                    "Dequeued key without pending entry; using empty chain"
                );
                (Instant::now(), TriggerChain::default())
            }
        };

        let item = WorkItem {
            key: key.clone(),
            enqueue_time,
            trigger_chain,
        };

        tracing::debug!(
            queue = %self.name,
            key = %key,
            chain_depth = item.trigger_chain.depth(),
            "Dequeued item for processing"
        );

        Some(item)
    }

    /// Mark an item as done (successfully processed).
    ///
    /// This is now a no-op since we don't track processing state,
    /// but kept for API compatibility and logging.
    pub fn done(&self, key: &str) {
        tracing::debug!(
            queue = %self.name,
            key = %key,
            "Item processing done"
        );
    }

    /// Get current queue depth (items waiting to be processed)
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Check if a key is in the ready queue (pending)
    pub fn contains(&self, key: &str) -> bool {
        self.pending.contains_key(key)
    }

    /// Check if a key is in the delay queue (scheduled)
    pub fn is_scheduled(&self, key: &str) -> bool {
        self.scheduled.contains(key)
    }

    /// Get the configuration
    pub fn config(&self) -> &WorkqueueConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_enqueue_dequeue() {
        let queue = Workqueue::with_defaults("test");

        assert!(queue.enqueue("ns/name".to_string()).await);
        assert_eq!(queue.len(), 1);

        let item = queue.dequeue().await.unwrap();
        assert_eq!(item.key, "ns/name");
        assert!(item.trigger_chain.sources.is_empty());
        assert_eq!(queue.len(), 0);

        queue.done(&item.key);
    }

    #[tokio::test]
    async fn test_deduplication() {
        let queue = Workqueue::with_defaults("test");

        assert!(queue.enqueue("ns/name".to_string()).await);
        assert!(!queue.enqueue("ns/name".to_string()).await);
        assert_eq!(queue.len(), 1);
    }

    #[tokio::test]
    async fn test_enqueue_during_processing() {
        let queue = Workqueue::with_defaults("test");

        queue.enqueue("ns/name".to_string()).await;
        let _item = queue.dequeue().await.unwrap();

        assert!(queue.enqueue("ns/name".to_string()).await);
        assert_eq!(queue.len(), 1);
    }

    #[tokio::test]
    async fn test_requeue_after_done() {
        let queue = Workqueue::with_defaults("test");

        queue.enqueue("ns/name".to_string()).await;
        let item = queue.dequeue().await.unwrap();
        queue.done(&item.key);

        assert!(queue.enqueue("ns/name".to_string()).await);
    }

    #[tokio::test]
    async fn test_metrics() {
        let queue = Workqueue::with_defaults("test");

        assert_eq!(queue.metrics().get_adds_total(), 0);
        assert_eq!(queue.metrics().get_depth(), 0);

        queue.enqueue("ns/name1".to_string()).await;
        queue.enqueue("ns/name2".to_string()).await;

        assert_eq!(queue.metrics().get_adds_total(), 2);
        assert_eq!(queue.metrics().get_depth(), 2);

        let item = queue.dequeue().await.unwrap();
        assert_eq!(queue.metrics().get_depth(), 1);

        queue.done(&item.key);
        assert_eq!(queue.metrics().get_depth(), 1);
    }

    #[tokio::test]
    async fn test_dirty_requeue_pattern() {
        let queue = Workqueue::with_defaults("test");

        queue.enqueue("ns/resource".to_string()).await;

        let item = queue.dequeue().await.unwrap();
        assert_eq!(item.key, "ns/resource");

        assert!(queue.enqueue("ns/resource".to_string()).await);

        queue.done(&item.key);

        assert_eq!(queue.len(), 1);

        let item2 = queue.dequeue().await.unwrap();
        assert_eq!(item2.key, "ns/resource");
        queue.done(&item2.key);

        assert_eq!(queue.len(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_enqueue_dedup() {
        let queue = Arc::new(Workqueue::with_defaults("test"));
        let num_tasks = 20;
        let mut handles = vec![];

        for _ in 0..num_tasks {
            let q = queue.clone();
            let handle = tokio::spawn(async move { q.enqueue("ns/hotkey".to_string()).await });
            handles.push(handle);
        }

        let mut success_count = 0usize;
        for handle in handles {
            if handle.await.unwrap() {
                success_count += 1;
            }
        }
        assert_eq!(success_count, 1, "Exactly one enqueue should win the race");

        assert_eq!(
            queue.len(),
            1,
            "Queue should contain exactly one item after concurrent enqueues"
        );
        assert_eq!(queue.metrics().get_depth(), 1, "Metrics depth should be 1");
        assert_eq!(queue.metrics().get_adds_total(), 1, "Only 1 add should be counted");

        let item = queue.dequeue().await.unwrap();
        assert_eq!(item.key, "ns/hotkey");
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.metrics().get_depth(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_enqueue_after_and_enqueue() {
        let queue = Arc::new(Workqueue::with_defaults("test"));
        let queue_clone1 = queue.clone();
        let queue_clone2 = queue.clone();

        // Concurrently call enqueue_after (with 100ms delay) and enqueue (immediate)
        // on the same key. This tests that:
        // 1. Both calls succeed in queueing the key (once in scheduled, once in pending)
        // 2. When we dequeue the immediate item, the pending is clear
        // 3. When the delay fires, it will add the key to pending again (since pending is empty)
        let enqueue_after_handle = tokio::spawn(async move {
            queue_clone1
                .enqueue_after("ns/key".to_string(), Duration::from_millis(100), TriggerChain::new())
                .await
        });

        let enqueue_handle = tokio::spawn(async move { queue_clone2.enqueue("ns/key".to_string()).await });

        let scheduled = enqueue_after_handle.await.unwrap();
        let enqueued = enqueue_handle.await.unwrap();

        // Both succeed independently — enqueue writes to pending, enqueue_after writes to scheduled.
        // Since they target independent DashSets, both operations always succeed.
        assert!(enqueued, "immediate enqueue should succeed");
        assert!(scheduled, "enqueue_after should succeed");

        // Dequeue the immediate item from the successful enqueue path
        let item = queue.dequeue().await.expect("Should have an item");
        assert_eq!(item.key, "ns/key");

        // Now pending is empty. Wait for the delay to fire.
        sleep(Duration::from_millis(150)).await;

        // The delay loop will attempt to promote the scheduled item.
        // Since pending is now empty (we dequeued it above), the pending.insert succeeds
        // and the delayed item is promoted to the ready queue.
        let item2 = tokio::time::timeout(Duration::from_millis(500), queue.dequeue())
            .await
            .expect("Timeout waiting for delayed item")
            .expect("Queue should not be closed");
        assert_eq!(item2.key, "ns/key", "Delayed item should be the same key");

        // Now all should be processed
        assert_eq!(queue.len(), 0, "Pending queue should be empty after draining");
    }

    #[tokio::test]
    async fn test_delay_fire_collision_preserves_chain() {
        // Reproduces c3a-008: when enqueue_after's delay fires and the key is already
        // pending from a concurrent immediate enqueue, the trigger chain must be
        // merged into the pending entry, not silently dropped. Otherwise the cycle
        // detector loses cascade history and `max_trigger_cycles` cannot fire.
        let queue = Workqueue::with_defaults("test");
        let chain = TriggerChain::new().extend("HTTPRoute", "default/route-x");
        assert_eq!(chain.depth(), 1);

        // Schedule with delay (50ms) carrying a chain of depth 1.
        queue
            .enqueue_after("ns/key".to_string(), Duration::from_millis(50), chain.clone())
            .await;

        // Immediate enqueue lands in pending with empty chain. The delayed item is
        // still parked in the heap; when its 50ms timer fires it will hit the
        // pending-collision branch.
        queue.enqueue("ns/key".to_string()).await;

        // Wait for the delay to fire while the immediate item is still pending
        // (we deliberately do NOT dequeue between the two enqueues).
        sleep(Duration::from_millis(120)).await;

        // Dequeue: the surviving WorkItem must carry the merged chain (depth 1),
        // not the empty chain that would result if the collision dropped the chain.
        let item = queue.dequeue().await.expect("Should have an item");
        assert_eq!(item.key, "ns/key");
        assert_eq!(
            item.trigger_chain.depth(),
            1,
            "Chain must be preserved when delay-fire collides with pending"
        );
        assert_eq!(item.trigger_chain.sources[0].kind, "HTTPRoute");
        assert_eq!(item.trigger_chain.sources[0].key, "default/route-x");

        assert_eq!(queue.len(), 0, "No more items after dequeue");
    }
}
