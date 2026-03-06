//! Workqueue - Go controller-runtime style work queue implementation
//!
//! Provides a generic workqueue with:
//! - Deduplication: same key only exists once in queue
//! - Exponential backoff: retry with increasing delays
//! - Delayed enqueue: cross-resource requeue with coalescing via integrated DelayQueue
//! - Trigger chain: cascade path tracking with cycle detection
//! - Metrics: queue depth, adds, retries
//!
//! Key design decision: We don't track "processing" state. When a key is dequeued,
//! it's simply removed from pending. This allows new enqueue requests during processing
//! to succeed, ensuring updates are not lost (dirty requeue pattern).

use dashmap::DashSet;
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
    /// Number of times this item has been retried
    pub retry_count: u32,
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
            retry_count: 0,
            enqueue_time: Instant::now(),
            trigger_chain: TriggerChain::default(),
        }
    }

    /// Create a work item for retry (preserves chain from original)
    pub fn for_retry(key: String, retry_count: u32) -> Self {
        Self {
            key,
            retry_count,
            enqueue_time: Instant::now(),
            trigger_chain: TriggerChain::default(),
        }
    }

    /// Create a work item with a trigger chain (cross-resource requeue)
    pub fn with_chain(key: String, chain: TriggerChain) -> Self {
        Self {
            key,
            retry_count: 0,
            enqueue_time: Instant::now(),
            trigger_chain: chain,
        }
    }
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
    /// Maximum number of retries before giving up
    pub max_retries: u32,
    /// Initial backoff duration for retries
    pub initial_backoff: Duration,
    /// Maximum backoff duration
    pub max_backoff: Duration,
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
            capacity: 1000,
            max_retries: 5,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(30),
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
    /// Total number of retries
    pub retries_total: AtomicU64,
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

    pub fn inc_retries(&self) {
        self.retries_total.fetch_add(1, Ordering::Relaxed);
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

    pub fn get_retries_total(&self) -> u64 {
        self.retries_total.load(Ordering::Relaxed)
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
    /// Sender for enqueueing ready items
    tx: mpsc::Sender<WorkItem>,
    /// Receiver for dequeueing items (wrapped in Mutex for single consumer)
    rx: Mutex<mpsc::Receiver<WorkItem>>,
    /// Keys currently in the ready queue (for deduplication).
    /// Wrapped in Arc so delay_loop and requeue_with_backoff share the same set.
    pending: Arc<DashSet<String>>,
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
        let pending = Arc::new(DashSet::new());
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
        ready_tx: mpsc::Sender<WorkItem>,
        pending: Arc<DashSet<String>>,
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

                                    if pending.contains(&item.key) {
                                        tracing::trace!(
                                            queue = %name,
                                            key = %item.key,
                                            "Delayed item ready but key already pending, skipping"
                                        );
                                        continue;
                                    }

                                    pending.insert(item.key.clone());
                                    metrics.inc_depth();
                                    metrics.inc_adds();

                                    let work_item = WorkItem::with_chain(item.key.clone(), item.chain);
                                    if ready_tx.send(work_item).await.is_err() {
                                        pending.remove(&item.key);
                                        metrics.dec_depth();
                                        tracing::error!(
                                            queue = %name,
                                            key = %item.key,
                                            "Failed to move delayed item to ready queue"
                                        );
                                    }
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
                                        if item.ready_at <= now {
                                            scheduled.remove(&item.key);
                                            if !pending.contains(&item.key) {
                                                pending.insert(item.key.clone());
                                                metrics.inc_depth();
                                                metrics.inc_adds();
                                                let work_item = WorkItem::with_chain(item.key.clone(), item.chain);
                                                let _ = ready_tx.send(work_item).await;
                                            }
                                        } else {
                                            scheduled.remove(&item.key);
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
        if self.pending.contains(&key) {
            tracing::trace!(
                queue = %self.name,
                key = %key,
                "Key already in queue, skipping"
            );
            return false;
        }

        self.pending.insert(key.clone());
        self.metrics.inc_depth();

        let item = WorkItem::new(key.clone());

        match self.tx.send(item).await {
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
    /// `delay` elapses. Deduplication checks both `pending` (ready queue) and
    /// `scheduled` (delay queue) to avoid redundant work.
    ///
    /// Returns true if the key was scheduled, false if skipped due to dedup.
    pub async fn enqueue_after(&self, key: String, delay: Duration, chain: TriggerChain) -> bool {
        if self.pending.contains(&key) || self.scheduled.contains(&key) {
            tracing::trace!(
                queue = %self.name,
                key = %key,
                "Key already pending or scheduled, skipping enqueue_after"
            );
            return false;
        }

        self.scheduled.insert(key.clone());
        self.metrics.inc_delayed();

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
        let item = rx.recv().await?;

        self.pending.remove(&item.key);
        self.metrics.dec_depth();

        tracing::debug!(
            queue = %self.name,
            key = %item.key,
            retry_count = item.retry_count,
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

    /// Requeue an item with exponential backoff.
    ///
    /// This is called when processing fails and needs to be retried.
    /// The item will be re-added to the queue after a delay.
    ///
    /// This mechanism is independent from the delay subsystem (`enqueue_after`)
    /// which is used for cross-resource requeue coalescing. Keeping them separate
    /// avoids edge cases where a long retry backoff blocks a short cross-resource delay.
    pub async fn requeue_with_backoff(&self, item: WorkItem) {
        let new_retry_count = item.retry_count + 1;

        if new_retry_count > self.config.max_retries {
            tracing::warn!(
                queue = %self.name,
                key = %item.key,
                max_retries = self.config.max_retries,
                "Max retries exceeded, giving up"
            );
            return;
        }

        let backoff = self
            .config
            .initial_backoff
            .saturating_mul(2u32.saturating_pow(item.retry_count));
        let backoff = backoff.min(self.config.max_backoff);

        tracing::info!(
            queue = %self.name,
            key = %item.key,
            retry_count = new_retry_count,
            backoff_ms = backoff.as_millis(),
            "Scheduling retry with backoff"
        );

        let tx = self.tx.clone();
        let pending = self.pending.clone();
        let metrics = self.metrics.clone();
        let name = self.name.clone();
        let key = item.key.clone();

        tokio::spawn(async move {
            sleep(backoff).await;

            if pending.contains(&key) {
                tracing::trace!(
                    queue = %name,
                    key = %key,
                    "Key already pending after backoff, skipping requeue"
                );
                return;
            }

            pending.insert(key.clone());
            metrics.inc_depth();
            metrics.inc_retries();

            let retry_item = WorkItem::for_retry(key.clone(), new_retry_count);
            if tx.send(retry_item).await.is_err() {
                pending.remove(&key);
                metrics.dec_depth();
                tracing::error!(
                    queue = %name,
                    key = %key,
                    "Failed to requeue item after backoff"
                );
            }
        });
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
        self.pending.contains(key)
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
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_enqueue_dequeue() {
        let queue = Workqueue::with_defaults("test");

        assert!(queue.enqueue("ns/name".to_string()).await);
        assert_eq!(queue.len(), 1);

        let item = queue.dequeue().await.unwrap();
        assert_eq!(item.key, "ns/name");
        assert_eq!(item.retry_count, 0);
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
    async fn test_requeue_with_backoff() {
        let config = WorkqueueConfig {
            capacity: 10,
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(100),
            ..Default::default()
        };
        let queue = Workqueue::new("test", config);

        queue.enqueue("ns/name".to_string()).await;
        let item = queue.dequeue().await.unwrap();
        assert_eq!(item.retry_count, 0);

        queue.requeue_with_backoff(item).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let result = timeout(Duration::from_millis(100), queue.dequeue()).await;
        assert!(result.is_ok());
        let item = result.unwrap().unwrap();
        assert_eq!(item.key, "ns/name");
        assert_eq!(item.retry_count, 1);
    }

    #[tokio::test]
    async fn test_max_retries() {
        let config = WorkqueueConfig {
            capacity: 10,
            max_retries: 2,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
            ..Default::default()
        };
        let queue = Workqueue::new("test", config);

        queue.enqueue("ns/name".to_string()).await;

        for i in 0..=2 {
            let item = queue.dequeue().await.unwrap();
            assert_eq!(item.retry_count, i);
            queue.requeue_with_backoff(item).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(queue.len(), 0);
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
}
