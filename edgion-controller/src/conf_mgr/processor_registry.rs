//! ProcessorRegistry - Global registry for all ResourceProcessors
//!
//! Provides unified access to all processors via:
//! - Dynamic access by kind name: `get("HTTPRoute")`
//! - Cross-resource requeue: `requeue("Gateway", "default/my-gateway")`
//! - WatchObj collection for ConfigSyncServer

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use super::sync_runtime::resource_processor::ProcessorObj;
use super::sync_runtime::workqueue::TriggerChain;
use crate::conf_sync::conf_server::WatchObj;

/// Registry that manages all ResourceProcessor instances
///
/// This provides a central place to:
/// 1. Register processors during startup
/// 2. Access processors by kind name
/// 3. Collect WatchObjs for ConfigSyncServer
/// 4. Enable cross-resource requeue
pub struct ProcessorRegistry {
    /// All processors indexed by kind name
    processors: RwLock<HashMap<&'static str, Arc<dyn ProcessorObj>>>,

    /// Notified whenever any processor transitions to ready or registration completes.
    /// Used by `wait_kinds_ready` and `wait_registry_ready` to avoid busy-polling.
    pub(crate) ready_notify: Arc<Notify>,

    /// Set to `true` after all processors have been registered (Phase 1 + Phase 2).
    /// Prevents the lifecycle from reading `all_kinds()` before registration is complete.
    registration_complete: AtomicBool,
}

impl ProcessorRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            processors: RwLock::new(HashMap::new()),
            ready_notify: Arc::new(Notify::new()),
            registration_complete: AtomicBool::new(false),
        }
    }

    /// Notify waiters that a processor became ready.
    ///
    /// Called by `ResourceProcessor::set_ready()` immediately after the cache
    /// is marked ready. Wakes all tasks waiting in `wait_kinds_ready`.
    pub fn notify_ready(&self) {
        self.ready_notify.notify_waiters();
    }

    /// Mark that all processors have been registered (Phase 1 + Phase 2 complete).
    ///
    /// Called by `spawn_all_controllers` after both phases finish spawning.
    /// Wakes any task waiting in `wait_registry_ready`.
    pub fn mark_registration_complete(&self) {
        self.registration_complete.store(true, Ordering::Release);
        self.ready_notify.notify_waiters();
    }

    /// Check whether processor registration is complete.
    pub fn is_registration_complete(&self) -> bool {
        self.registration_complete.load(Ordering::Acquire)
    }

    /// Register a processor
    ///
    /// Called during ResourceProcessor initialization
    pub fn register(&self, processor: Arc<dyn ProcessorObj>) {
        let kind = processor.kind();
        let mut map = self.processors.write();

        tracing::info!(component = "processor_registry", kind = kind, "Registering processor");

        map.insert(kind, processor);
    }

    /// Get a processor by kind name (dynamic)
    pub fn get(&self, kind: &str) -> Option<Arc<dyn ProcessorObj>> {
        self.processors.read().get(kind).cloned()
    }

    /// Get all registered kind names
    pub fn all_kinds(&self) -> Vec<&'static str> {
        self.processors.read().keys().copied().collect()
    }

    /// Get count of registered processors
    pub fn count(&self) -> usize {
        self.processors.read().len()
    }

    /// Check if all registered processors are ready
    ///
    /// Returns `false` if registry is empty (must have at least one processor)
    pub fn is_all_ready(&self) -> bool {
        let processors = self.processors.read();
        !processors.is_empty() && processors.values().all(|p| p.is_ready())
    }

    /// Get list of kinds that are not ready
    pub fn not_ready_kinds(&self) -> Vec<&'static str> {
        self.processors
            .read()
            .iter()
            .filter(|(_, p)| !p.is_ready())
            .map(|(k, _)| *k)
            .collect()
    }

    /// Walk all processors. If any has a dirty stats flag, take a snapshot of
    /// every processor's `store_size` and return a per-kind map + total.
    ///
    /// Returns `None` when no processor is dirty (nothing changed since last
    /// call), so the caller can skip emitting a redundant StatsReport.
    ///
    /// All dirty flags are cleared as a side effect.
    pub fn collect_stats_if_dirty(&self) -> Option<(HashMap<String, u32>, u32)> {
        let processors = self.processors.read();
        // First pass: determine if any processor is dirty, clearing flags as we go.
        let mut any_dirty = false;
        for p in processors.values() {
            if p.take_stats_dirty() {
                any_dirty = true;
            }
        }
        if !any_dirty {
            return None;
        }
        // Second pass: snapshot per-kind store sizes for the report.
        let mut per_kind: HashMap<String, u32> = HashMap::with_capacity(processors.len());
        let mut total: u32 = 0;
        for (kind, p) in processors.iter() {
            let size = p.store_size() as u32;
            per_kind.insert((*kind).to_string(), size);
            total = total.saturating_add(size);
        }
        Some((per_kind, total))
    }

    /// Snapshot all processor sizes regardless of dirty flags and consume the
    /// current dirty state, just like an emitted incremental report.
    ///
    /// Federation uses this once for every newly established session so a
    /// restarted Center receives an initial resource count even when the
    /// Controller's resources have not changed since the previous session.
    pub fn collect_stats_snapshot(&self) -> (HashMap<String, u32>, u32) {
        let processors = self.processors.read();
        let mut per_kind: HashMap<String, u32> = HashMap::with_capacity(processors.len());
        let mut total: u32 = 0;
        for (kind, processor) in processors.iter() {
            let _ = processor.take_stats_dirty();
            let size = processor.store_size() as u32;
            per_kind.insert((*kind).to_string(), size);
            total = total.saturating_add(size);
        }
        (per_kind, total)
    }

    /// Get all WatchObjs for ConfigSyncServer registration
    ///
    /// Returns a HashMap that can be passed to ConfigSyncServer::register_all()
    /// Filters out resources in no_sync_kinds (configurable, default: ReferenceGrant, Secret)
    ///
    /// # Arguments
    /// * `no_sync_kinds` - List of resource kind names that should not be synced to Gateway
    pub fn all_watch_objs(&self, no_sync_kinds: &[&str]) -> HashMap<String, Arc<dyn WatchObj>> {
        self.processors
            .read()
            .iter()
            .filter(|(k, _)| !no_sync_kinds.contains(k))
            .map(|(k, v)| (k.to_string(), v.as_watch_obj()))
            .collect()
    }

    /// Cross-resource requeue (immediate, no trigger chain).
    ///
    /// Enqueues a key to another resource's workqueue immediately.
    /// Used for init-time revalidation and scenarios without cascade context.
    ///
    /// For cascade-aware requeue with delay coalescing, use `requeue_with_chain`.
    pub async fn requeue(&self, kind: &str, key: String) {
        if let Some(processor) = self.get(kind) {
            processor.requeue(key.clone()).await;
            tracing::debug!(
                component = "processor_registry",
                target_kind = kind,
                key = %key,
                "Cross-resource requeue triggered"
            );
        } else {
            tracing::warn!(
                component = "processor_registry",
                target_kind = kind,
                key = %key,
                "Requeue failed: processor not found"
            );
        }
    }

    /// Cross-resource requeue with trigger chain (delayed, coalesced).
    ///
    /// Enqueues a key through the delay subsystem for coalescing.
    /// The trigger chain is propagated for cascade cycle detection.
    ///
    /// Called by `HandlerContext::requeue` after cycle detection passes.
    pub async fn requeue_with_chain(&self, kind: &str, key: String, chain: TriggerChain) {
        if let Some(processor) = self.get(kind) {
            processor.requeue_with_chain(key.clone(), chain).await;
            tracing::debug!(
                component = "processor_registry",
                target_kind = kind,
                key = %key,
                "Cross-resource requeue with chain triggered"
            );
        } else {
            tracing::warn!(
                component = "processor_registry",
                target_kind = kind,
                key = %key,
                "Requeue with chain failed: processor not found"
            );
        }
    }

    /// Wait for a specific set of resource kinds to become ready.
    ///
    /// Used by phased init to gate Phase 2 resource startup until Phase 1
    /// foundation resources (Gateway, Secret, etc.) are fully loaded.
    ///
    /// Wakes immediately when any processor calls `notify_ready()`. A 1-second
    /// fallback timeout guards against missed notifications (e.g. a processor
    /// that became ready between the condition check and the wait call).
    ///
    /// Returns `true` if all specified kinds became ready within the timeout,
    /// `false` if the timeout was reached (Phase 2 will still start as a fallback).
    pub async fn wait_kinds_ready(&self, kinds: &[&str], timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        // Clone the Notify Arc so we can move it into each notified() call
        // without holding a borrow on self.
        let notify = self.ready_notify.clone();

        loop {
            let pending: Vec<&str> = kinds
                .iter()
                .filter(|k| self.get(k).map(|p| !p.is_ready()).unwrap_or(true))
                .copied()
                .collect();

            if pending.is_empty() {
                tracing::info!(
                    component = "processor_registry",
                    elapsed_ms = std::time::Instant::now().saturating_duration_since(start).as_millis(),
                    kinds = ?kinds,
                    "All specified resource kinds are ready"
                );
                return true;
            }

            let elapsed = std::time::Instant::now().saturating_duration_since(start);
            if elapsed >= timeout {
                tracing::warn!(
                    component = "processor_registry",
                    elapsed_ms = elapsed.as_millis(),
                    not_ready = ?pending,
                    "Timeout waiting for specified resource kinds, proceeding anyway"
                );
                return false;
            }

            // Wait for the next ready notification, or at most 1 s as a safety net
            // against the race where notify_waiters() fires between the condition
            // check above and the notified() future being registered.
            let remaining = timeout - elapsed;
            let _ = tokio::time::timeout(remaining.min(Duration::from_secs(1)), notify.notified()).await;
        }
    }

    /// Requeue all resources across all kinds.
    /// Used on leader transition to trigger full status reconciliation.
    pub async fn requeue_all(&self) {
        let processors: Vec<_> = self.processors.read().iter().map(|(k, v)| (*k, v.clone())).collect();
        for (kind, processor) in processors {
            let count = processor.requeue_all_keys().await;
            tracing::info!(
                component = "processor_registry",
                kind = kind,
                count = count,
                "Requeued all resources for leader status reconciliation"
            );
        }
    }

    /// Clear only the processors map.
    /// Used by `ControllerRuntime::reset()` during leader-change or controller restart.
    /// Callers will re-register all processors after reset, so the entire map is cleared
    /// rather than merely marking processors as not-ready.
    pub fn clear_processors(&self) {
        tracing::info!(component = "processor_registry", "Clearing processors map");
        self.processors.write().clear();
        self.registration_complete.store(false, Ordering::Release);
    }
}

impl Default for ProcessorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/conf_mgr/processor_registry_test.rs"]
mod tests;
