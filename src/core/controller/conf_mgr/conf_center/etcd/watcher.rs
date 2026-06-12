//! EtcdWatcher — init-phase full-list + runtime-phase watch stream.
//!
//! Dispatches `EtcdEvent`s to per-kind broadcast channels, mirroring
//! `FileSystemWatcher`'s Init/InitApply/InitDone/Apply/Delete lifecycle.
//!
//! ## Compaction handling
//!
//! When etcd compacts past our last revision, we cannot resume watching.
//! Instead of re-sending Init/InitApply/InitDone (which would confuse
//! already-initialized resource controllers), we perform a **resync**:
//!
//! - Full list → send `Apply` for every current key (idempotent upsert)
//! - Compare with previously known keys → send `Delete` for removed keys
//!
//! This is analogous to Kubernetes informer's "relist" on 410 Gone.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use etcd_client::EventType;
use parking_lot::RwLock as SyncRwLock;
use tokio::sync::{mpsc, RwLock};

use super::event::EtcdEvent;
use crate::core::controller::conf_mgr::sync_runtime::ShutdownSignal;
use crate::core::common::link::providers::etcd::EtcdLinkClient;

/// Sender type for per-kind event channels.
pub type KindEventSender = mpsc::Sender<EtcdEvent>;
/// Receiver type for per-kind event channels.
pub type KindEventReceiver = mpsc::Receiver<EtcdEvent>;

/// EtcdWatcher — watches the etcd `{prefix}/conf/` key space.
///
/// - **Init phase**: `get_prefix_with_revision` to list all existing keys, then
///   dispatches `Init → InitApply × N → InitDone` per kind.
/// - **Runtime phase**: `watch_prefix_from_revision` for real-time changes,
///   dispatches `Apply` / `Delete` events.
/// - **Resync** (on compaction): full list → `Apply` for every current key +
///   `Delete` for keys that no longer exist.
pub struct EtcdWatcher {
    client: Arc<EtcdLinkClient>,
    /// The conf-data prefix, e.g. "/edgion/conf/"
    conf_prefix: String,
    /// Per-kind broadcast senders
    senders: Arc<RwLock<HashMap<&'static str, KindEventSender>>>,
    /// Known keys per kind — maintained across init and runtime for resync diff.
    known_keys: Arc<SyncRwLock<HashMap<&'static str, HashSet<String>>>>,
}

impl EtcdWatcher {
    pub fn new(client: Arc<EtcdLinkClient>, key_prefix: &str) -> Self {
        Self {
            client,
            conf_prefix: format!("{}/conf/", key_prefix),
            senders: Arc::new(RwLock::new(HashMap::new())),
            known_keys: Arc::new(SyncRwLock::new(HashMap::new())),
        }
    }

    /// Register a kind and get its event receiver.
    /// Must be called before `run()`. Each kind may only be subscribed once.
    pub async fn subscribe(&self, kind: &'static str) -> KindEventReceiver {
        let mut senders = self.senders.write().await;
        let (tx, rx) = mpsc::channel(256);
        senders.insert(kind, tx);
        rx
    }

    /// Run the watcher: init phase → runtime phase.
    pub async fn run(&self, mut shutdown_signal: ShutdownSignal) -> Result<()> {
        tracing::info!(
            component = "etcd_watcher",
            prefix = %self.conf_prefix,
            "Starting EtcdWatcher"
        );

        // Phase 1: full list (first time — sends Init/InitApply/InitDone)
        let revision = self.init_phase().await?;

        // Phase 2: watch stream
        self.runtime_phase(revision, &mut shutdown_signal).await?;

        tracing::info!(component = "etcd_watcher", "EtcdWatcher stopped");
        Ok(())
    }

    // ==================== Init Phase ====================

    /// Full-list all existing keys, dispatch Init/InitApply/InitDone per kind.
    /// Called once at startup. Returns the cluster revision.
    async fn init_phase(&self) -> Result<i64> {
        tracing::info!(
            component = "etcd_watcher",
            prefix = %self.conf_prefix,
            "Starting init phase"
        );

        let (kvs, revision) = self.client.get_prefix_with_revision(&self.conf_prefix).await?;

        tracing::info!(
            component = "etcd_watcher",
            count = kvs.len(),
            revision = revision,
            "Init phase: fetched all keys"
        );

        // Group by kind and record known keys
        let mut by_kind: HashMap<&str, Vec<(String, String)>> = HashMap::new();
        let mut new_known: HashMap<&'static str, HashSet<String>> = HashMap::new();

        for (key, value, _rev) in &kvs {
            if let Some(kind) = self.parse_kind_from_key(key) {
                if let Ok(content) = std::str::from_utf8(value) {
                    by_kind
                        .entry(kind)
                        .or_default()
                        .push((key.clone(), content.to_string()));
                    new_known.entry(kind).or_default().insert(key.clone());
                }
            }
        }

        // Clone the snapshot so we don't hold the read guard across `tx.send().await`.
        // Holding it would block any future dynamic `subscribe()` (which takes the write lock).
        let senders = self.senders.read().await.clone();

        // For each registered kind: Init → InitApply × N → InitDone
        for (kind, tx) in senders.iter() {
            let _ = tx.send(EtcdEvent::Init).await;

            if let Some(items) = by_kind.get(*kind) {
                for (key, content) in items {
                    let _ = tx
                        .send(EtcdEvent::InitApply {
                            key: key.clone(),
                            content: content.clone(),
                        })
                        .await;
                }
            }

            let _ = tx.send(EtcdEvent::InitDone).await;
        }

        // Store known keys for future resync diff
        *self.known_keys.write() = new_known;

        tracing::info!(
            component = "etcd_watcher",
            kinds = by_kind.len(),
            revision = revision,
            "Init phase complete"
        );

        Ok(revision)
    }

    // ==================== Resync Phase (compaction recovery) ====================

    /// Re-list all keys after compaction. Unlike init_phase, this sends
    /// `Apply` (not InitApply) for current keys, and `Delete` for keys
    /// that existed before but are now gone.
    async fn resync_phase(&self) -> Result<i64> {
        tracing::info!(
            component = "etcd_watcher",
            "Starting resync phase (compaction recovery)"
        );

        let (kvs, revision) = self.client.get_prefix_with_revision(&self.conf_prefix).await?;

        // Build current key set grouped by kind
        let mut current_by_kind: HashMap<&'static str, HashMap<String, String>> = HashMap::new();
        for (key, value, _rev) in &kvs {
            if let Some(kind) = self.parse_kind_from_key(key) {
                if let Ok(content) = std::str::from_utf8(value) {
                    current_by_kind
                        .entry(kind)
                        .or_default()
                        .insert(key.clone(), content.to_string());
                }
            }
        }

        // Clone the snapshot so we don't hold the read guard across `tx.send().await`
        // (same rationale as init_phase / runtime_phase).
        let senders = self.senders.read().await.clone();
        // Take ownership of old keys — they'll be replaced by new_known below anyway.
        let old_known = std::mem::take(&mut *self.known_keys.write());
        let mut new_known: HashMap<&'static str, HashSet<String>> = HashMap::new();

        for (kind, tx) in senders.iter() {
            let current = current_by_kind.get(*kind);
            let old_keys = old_known.get(*kind);

            // Send Apply for all current keys (idempotent upsert)
            if let Some(items) = current {
                for (key, content) in items {
                    let _ = tx
                        .send(EtcdEvent::Apply {
                            key: key.clone(),
                            content: content.clone(),
                        })
                        .await;
                    new_known.entry(kind).or_default().insert(key.clone());
                }
            }

            // Send Delete for keys that existed before but are now gone
            if let Some(old) = old_keys {
                let current_keys: HashSet<&String> = current.map(|m| m.keys().collect()).unwrap_or_default();
                for old_key in old {
                    if !current_keys.contains(old_key) {
                        let _ = tx.send(EtcdEvent::Delete { key: old_key.clone() }).await;
                    }
                }
            }
        }

        *self.known_keys.write() = new_known;

        tracing::info!(
            component = "etcd_watcher",
            count = kvs.len(),
            revision = revision,
            "Resync phase complete"
        );

        Ok(revision)
    }

    // ==================== Runtime Phase ====================

    /// Watch for real-time changes starting from `start_revision + 1`.
    /// Reconnects on error; resyncs on compaction.
    async fn runtime_phase(&self, start_revision: i64, shutdown_signal: &mut ShutdownSignal) -> Result<()> {
        let mut revision = start_revision;
        let mut reconnect_backoff = Duration::from_secs(1);
        const MAX_BACKOFF: Duration = Duration::from_secs(30);

        // Snapshot senders once — they are registered before run() and never change.
        let senders = self.senders.read().await.clone();

        loop {
            tracing::info!(
                component = "etcd_watcher",
                revision = revision + 1,
                "Starting watch stream"
            );

            match self
                .client
                .watch_prefix_from_revision(&self.conf_prefix, revision + 1)
                .await
            {
                Ok(mut stream) => {
                    // Stream established — reset reconnect backoff.
                    reconnect_backoff = Duration::from_secs(1);
                    loop {
                        tokio::select! {
                            _ = shutdown_signal.wait() => {
                                tracing::info!(component = "etcd_watcher", "Shutdown signal received");
                                return Ok(());
                            }
                            msg = stream.message() => {
                                match msg {
                                    Ok(Some(resp)) => {
                                        // Check for compaction
                                        if resp.compact_revision() > 0 {
                                            tracing::warn!(
                                                component = "etcd_watcher",
                                                compact_revision = resp.compact_revision(),
                                                "Revision compacted, performing resync"
                                            );
                                            // Resync must be resilient: a transient list failure
                                            // here used to terminate the entire controller. Retry
                                            // with exponential backoff until success or shutdown.
                                            let mut resync_backoff = Duration::from_secs(1);
                                            let new_revision = loop {
                                                match self.resync_phase().await {
                                                    Ok(rev) => break rev,
                                                    Err(e) => {
                                                        tracing::error!(
                                                            component = "etcd_watcher",
                                                            error = %e,
                                                            backoff_secs = resync_backoff.as_secs(),
                                                            "Resync failed, retrying after backoff"
                                                        );
                                                        tokio::select! {
                                                            _ = shutdown_signal.wait() => return Ok(()),
                                                            _ = tokio::time::sleep(resync_backoff) => {}
                                                        }
                                                        resync_backoff =
                                                            std::cmp::min(resync_backoff * 2, MAX_BACKOFF);
                                                    }
                                                }
                                            };
                                            revision = new_revision;
                                            break; // Restart watch from new revision
                                        }

                                        // Process events and maintain known_keys
                                        for event in resp.events() {
                                            if let Some(kv) = event.kv() {
                                                let key = kv.key_str().unwrap_or("").to_string();
                                                revision = kv.mod_revision();

                                                if let Some(kind) = self.parse_kind_from_key(&key) {
                                                    if let Some(tx) = senders.get(kind) {
                                                        match event.event_type() {
                                                            EventType::Put => {
                                                                let content = match std::str::from_utf8(kv.value()) {
                                                                    Ok(s) => s.to_string(),
                                                                    Err(e) => {
                                                                        tracing::warn!(
                                                                            component = "etcd_watcher",
                                                                            key = %key,
                                                                            error = %e,
                                                                            "Skipping non-UTF8 value"
                                                                        );
                                                                        continue;
                                                                    }
                                                                };
                                                                let _ = tx.send(EtcdEvent::Apply {
                                                                    key: key.clone(),
                                                                    content,
                                                                }).await;
                                                                self.known_keys.write()
                                                                    .entry(kind)
                                                                    .or_default()
                                                                    .insert(key);
                                                            }
                                                            EventType::Delete => {
                                                                let _ = tx.send(EtcdEvent::Delete {
                                                                    key: key.clone(),
                                                                }).await;
                                                                if let Some(set) = self.known_keys.write().get_mut(kind) {
                                                                    set.remove(&key);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Ok(None) => {
                                        tracing::warn!(component = "etcd_watcher", "Watch stream ended");
                                        break; // Reconnect
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            component = "etcd_watcher",
                                            error = %e,
                                            "Watch stream error"
                                        );
                                        break; // Reconnect
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(
                        component = "etcd_watcher",
                        error = %e,
                        "Failed to create watch stream"
                    );
                }
            }

            // Exponential backoff before reconnect; reset to 1s once a stream connects.
            tokio::select! {
                _ = shutdown_signal.wait() => return Ok(()),
                _ = tokio::time::sleep(reconnect_backoff) => {}
            }
            reconnect_backoff = std::cmp::min(reconnect_backoff * 2, MAX_BACKOFF);
        }
    }

    // ==================== Helpers ====================

    /// Extract the kind from an etcd key like "/edgion/conf/HTTPRoute/default/my-route".
    /// Returns the kind as a `&'static str` via `ResourceKind` mapping.
    fn parse_kind_from_key(&self, key: &str) -> Option<&'static str> {
        let stripped = key.strip_prefix(&self.conf_prefix)?;
        let kind_str = stripped.split('/').next()?;
        let rk = edgion_resources::ResourceKind::from_kind_name(kind_str)?;
        Some(rk.as_str())
    }
}
