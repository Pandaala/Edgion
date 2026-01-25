//! Workqueue - Go controller-runtime style work queue implementation
//!
//! Provides a generic workqueue with:
//! - Deduplication: same key only exists once in queue
//! - Processing tracking: prevents concurrent processing of same key
//! - Exponential backoff: retry with increasing delays
//! - Metrics: queue depth, adds, retries, latency

use dashmap::DashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;

/// Work item in the queue
#[derive(Debug, Clone)]
pub struct WorkItem {
    /// Resource key: "namespace/name" for namespaced, "name" for cluster-scoped
    pub key: String,
    /// Number of times this item has been retried
    pub retry_count: u32,
    /// Time when item was enqueued
    pub enqueue_time: Instant,
}

impl WorkItem {
    /// Create a new work item
    pub fn new(key: String) -> Self {
        Self {
            key,
            retry_count: 0,
            enqueue_time: Instant::now(),
        }
    }

    /// Create a work item for retry
    pub fn for_retry(key: String, retry_count: u32) -> Self {
        Self {
            key,
            retry_count,
            enqueue_time: Instant::now(),
        }
    }
}

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
}

impl Default for WorkqueueConfig {
    fn default() -> Self {
        Self {
            capacity: 1000,
            max_retries: 5,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(30),
        }
    }
}

/// Metrics for workqueue monitoring
#[derive(Debug, Default)]
pub struct WorkqueueMetrics {
    /// Total number of items added to queue
    pub adds_total: AtomicU64,
    /// Total number of retries
    pub retries_total: AtomicU64,
    /// Number of items currently in queue (pending)
    pub depth: AtomicU64,
    /// Number of items currently being processed
    pub processing: AtomicU64,
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

    pub fn inc_processing(&self) {
        self.processing.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_processing(&self) {
        self.processing.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn get_depth(&self) -> u64 {
        self.depth.load(Ordering::Relaxed)
    }

    pub fn get_processing(&self) -> u64 {
        self.processing.load(Ordering::Relaxed)
    }

    pub fn get_adds_total(&self) -> u64 {
        self.adds_total.load(Ordering::Relaxed)
    }

    pub fn get_retries_total(&self) -> u64 {
        self.retries_total.load(Ordering::Relaxed)
    }
}

/// Generic workqueue for resource reconciliation
///
/// Each resource type should have its own workqueue instance.
/// This provides isolation between different resource types.
pub struct Workqueue {
    /// Queue name (for logging and metrics)
    name: String,
    /// Sender for enqueueing items
    tx: mpsc::Sender<WorkItem>,
    /// Receiver for dequeueing items (wrapped in Mutex for single consumer)
    rx: Mutex<mpsc::Receiver<WorkItem>>,
    /// Keys currently in the queue (for deduplication)
    pending: DashSet<String>,
    /// Keys currently being processed (to prevent concurrent processing)
    processing: DashSet<String>,
    /// Configuration
    config: WorkqueueConfig,
    /// Metrics
    metrics: Arc<WorkqueueMetrics>,
}

impl Workqueue {
    /// Create a new workqueue with the given name and configuration
    pub fn new(name: &str, config: WorkqueueConfig) -> Self {
        let (tx, rx) = mpsc::channel(config.capacity);
        Self {
            name: name.to_string(),
            tx,
            rx: Mutex::new(rx),
            pending: DashSet::new(),
            processing: DashSet::new(),
            config,
            metrics: Arc::new(WorkqueueMetrics::new()),
        }
    }

    /// Create a new workqueue with default configuration
    pub fn with_defaults(name: &str) -> Self {
        Self::new(name, WorkqueueConfig::default())
    }

    /// Get the queue name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get metrics reference
    pub fn metrics(&self) -> Arc<WorkqueueMetrics> {
        self.metrics.clone()
    }

    /// Enqueue a key for processing
    ///
    /// Returns true if the key was added, false if it was already in the queue (deduplicated)
    pub async fn enqueue(&self, key: String) -> bool {
        // Deduplication: if key is already pending or being processed, skip
        if self.pending.contains(&key) || self.processing.contains(&key) {
            tracing::trace!(
                queue = %self.name,
                key = %key,
                "Key already in queue or processing, skipping"
            );
            return false;
        }

        // Add to pending set first
        self.pending.insert(key.clone());
        self.metrics.inc_depth();

        let item = WorkItem::new(key.clone());

        // Try to send to channel
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
                // Channel closed or full, remove from pending
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

    /// Dequeue an item for processing
    ///
    /// This will block until an item is available.
    /// Returns None if the queue is closed.
    pub async fn dequeue(&self) -> Option<WorkItem> {
        let mut rx = self.rx.lock().await;
        let item = rx.recv().await?;

        // Move from pending to processing
        self.pending.remove(&item.key);
        self.metrics.dec_depth();

        self.processing.insert(item.key.clone());
        self.metrics.inc_processing();

        tracing::debug!(
            queue = %self.name,
            key = %item.key,
            retry_count = item.retry_count,
            "Dequeued item for processing"
        );

        Some(item)
    }

    /// Mark an item as done (successfully processed)
    ///
    /// This removes the key from the processing set.
    pub fn done(&self, key: &str) {
        self.processing.remove(key);
        self.metrics.dec_processing();

        tracing::debug!(
            queue = %self.name,
            key = %key,
            "Item processing done"
        );
    }

    /// Requeue an item with exponential backoff
    ///
    /// This is called when processing fails and needs to be retried.
    /// The item will be re-added to the queue after a delay.
    pub async fn requeue_with_backoff(&self, item: WorkItem) {
        let new_retry_count = item.retry_count + 1;

        // Check if max retries exceeded
        if new_retry_count > self.config.max_retries {
            tracing::warn!(
                queue = %self.name,
                key = %item.key,
                max_retries = self.config.max_retries,
                "Max retries exceeded, giving up"
            );
            self.done(&item.key);
            return;
        }

        // Calculate backoff delay: initial_backoff * 2^retry_count, capped at max_backoff
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

        // Remove from processing (will be re-added to pending after backoff)
        self.processing.remove(&item.key);
        self.metrics.dec_processing();

        // Spawn a task to requeue after backoff
        let tx = self.tx.clone();
        let pending = self.pending.clone();
        let metrics = self.metrics.clone();
        let name = self.name.clone();
        let key = item.key.clone();

        tokio::spawn(async move {
            sleep(backoff).await;

            // Check if already pending (might have been re-added by another event)
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

    /// Get number of items currently being processed
    pub fn processing_count(&self) -> usize {
        self.processing.len()
    }

    /// Check if a key is currently being processed
    pub fn is_processing(&self, key: &str) -> bool {
        self.processing.contains(key)
    }

    /// Check if a key is in the queue (pending or processing)
    pub fn contains(&self, key: &str) -> bool {
        self.pending.contains(key) || self.processing.contains(key)
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

        // Enqueue
        assert!(queue.enqueue("ns/name".to_string()).await);
        assert_eq!(queue.len(), 1);

        // Dequeue
        let item = queue.dequeue().await.unwrap();
        assert_eq!(item.key, "ns/name");
        assert_eq!(item.retry_count, 0);
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.processing_count(), 1);

        // Done
        queue.done(&item.key);
        assert_eq!(queue.processing_count(), 0);
    }

    #[tokio::test]
    async fn test_deduplication() {
        let queue = Workqueue::with_defaults("test");

        // First enqueue succeeds
        assert!(queue.enqueue("ns/name".to_string()).await);

        // Second enqueue is deduplicated
        assert!(!queue.enqueue("ns/name".to_string()).await);

        assert_eq!(queue.len(), 1);
    }

    #[tokio::test]
    async fn test_deduplication_while_processing() {
        let queue = Workqueue::with_defaults("test");

        // Enqueue and dequeue
        queue.enqueue("ns/name".to_string()).await;
        let _item = queue.dequeue().await.unwrap();

        // Try to enqueue same key while processing - should be deduplicated
        assert!(!queue.enqueue("ns/name".to_string()).await);
    }

    #[tokio::test]
    async fn test_requeue_after_done() {
        let queue = Workqueue::with_defaults("test");

        // Enqueue, dequeue, done
        queue.enqueue("ns/name".to_string()).await;
        let item = queue.dequeue().await.unwrap();
        queue.done(&item.key);

        // Now we can enqueue again
        assert!(queue.enqueue("ns/name".to_string()).await);
    }

    #[tokio::test]
    async fn test_requeue_with_backoff() {
        let config = WorkqueueConfig {
            capacity: 10,
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(100),
        };
        let queue = Workqueue::new("test", config);

        // Enqueue and dequeue
        queue.enqueue("ns/name".to_string()).await;
        let item = queue.dequeue().await.unwrap();
        assert_eq!(item.retry_count, 0);

        // Requeue with backoff
        queue.requeue_with_backoff(item).await;

        // Wait for backoff to complete
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Should be back in queue
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
        };
        let queue = Workqueue::new("test", config);

        // Enqueue
        queue.enqueue("ns/name".to_string()).await;

        // Dequeue and fail multiple times
        for i in 0..=2 {
            let item = queue.dequeue().await.unwrap();
            assert_eq!(item.retry_count, i);
            queue.requeue_with_backoff(item).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // After max retries, item should be removed (done called)
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(queue.processing_count(), 0);
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
        assert_eq!(queue.metrics().get_processing(), 1);

        queue.done(&item.key);
        assert_eq!(queue.metrics().get_processing(), 0);
    }
}
