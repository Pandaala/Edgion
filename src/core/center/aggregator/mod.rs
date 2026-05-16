//! In-memory controller-info aggregator.
//!
//! Tracks per-controller registration and online/offline state.
//! Snapshots are updated on registration and marked-offline on disconnect.
//! Offline entries are retained in memory and must be removed explicitly via
//! the Admin DELETE API (see ticket #20).

use crate::core::common::fed_sync::proto::RegisterRequest;
use crate::core::common::observe::fed_metrics;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug)]
struct ControllerSnapshot {
    info: RegisterRequest,
    offline_since: Option<std::time::Instant>,
}

impl ControllerSnapshot {
    fn new(info: RegisterRequest) -> Self {
        Self {
            info,
            offline_since: None,
        }
    }
}

#[derive(Clone)]
pub struct ResourceAggregator {
    inner: Arc<RwLock<HashMap<String, ControllerSnapshot>>>,
}

impl ResourceAggregator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Called when controller registers (or reconnects).
    pub fn set_controller_info(&self, controller_id: &str, info: RegisterRequest) {
        let snapshot = {
            let mut map = self.inner.write();
            let snap = map
                .entry(controller_id.to_string())
                .or_insert_with(|| ControllerSnapshot::new(info.clone()));
            snap.info = info;
            snap.offline_since = None;
            Self::compute_gauge_snapshot(&map)
        };
        Self::emit_gauges(&snapshot);
    }

    pub fn mark_offline(&self, controller_id: &str) {
        let snapshot = {
            let mut map = self.inner.write();
            if let Some(snap) = map.get_mut(controller_id) {
                if snap.offline_since.is_none() {
                    snap.offline_since = Some(std::time::Instant::now());
                }
            }
            Self::compute_gauge_snapshot(&map)
        };
        Self::emit_gauges(&snapshot);
    }

    /// Remove a controller's snapshot entirely. Used by Admin DELETE cascade.
    pub fn remove(&self, controller_id: &str) -> bool {
        let outcome = {
            let mut map = self.inner.write();
            // Capture the cluster before removal so we can zero the gauge if it disappears.
            let removed_cluster = map.get(controller_id).map(|s| {
                if s.info.cluster.is_empty() {
                    "unknown".to_string()
                } else {
                    s.info.cluster.clone()
                }
            });
            if map.remove(controller_id).is_none() {
                None
            } else {
                let snapshot = Self::compute_gauge_snapshot(&map);
                // Pre-seeding cannot see a fully-removed entry; emit zero explicitly when
                // the last controller for this cluster is gone.
                let zero_cluster = removed_cluster.and_then(|cluster| {
                    let still_present = map.values().any(|s| {
                        let c = if s.info.cluster.is_empty() {
                            "unknown"
                        } else {
                            s.info.cluster.as_str()
                        };
                        c == cluster
                    });
                    if still_present { None } else { Some(cluster) }
                });
                Some((snapshot, zero_cluster))
            }
        };
        let Some((snapshot, zero_cluster)) = outcome else {
            return false;
        };
        fed_metrics::record_evict_stale(fed_metrics::labels::evict_source::AGGREGATOR);
        Self::emit_gauges(&snapshot);
        if let Some(cluster) = zero_cluster {
            fed_metrics::set_aggregator_controllers(&cluster, 0);
        }
        true
    }

    /// Compute the per-cluster online-controller counts. Pure function — must
    /// be called inside the lock so the snapshot is consistent with the map
    /// state at that instant. The returned map is then emitted by
    /// [`Self::emit_gauges`] outside the lock.
    fn compute_gauge_snapshot(map: &HashMap<String, ControllerSnapshot>) -> HashMap<String, u64> {
        let mut by_cluster: HashMap<String, u64> = HashMap::new();
        // Pre-seed all known clusters with 0 so disappeared clusters are zeroed out.
        for snap in map.values() {
            let cluster = if snap.info.cluster.is_empty() {
                "unknown"
            } else {
                &snap.info.cluster
            };
            by_cluster.entry(cluster.to_string()).or_insert(0);
        }
        for snap in map.values().filter(|s| s.offline_since.is_none()) {
            let cluster = if snap.info.cluster.is_empty() {
                "unknown".to_string()
            } else {
                snap.info.cluster.clone()
            };
            *by_cluster.entry(cluster).or_default() += 1;
        }
        by_cluster
    }

    /// Emit the gauge snapshot to the `metrics` backend. Must be called
    /// OUTSIDE the write lock so a future metrics backend with non-trivial
    /// emit cost cannot stall registration / disconnect handling.
    fn emit_gauges(snapshot: &HashMap<String, u64>) {
        for (cluster, count) in snapshot {
            fed_metrics::set_aggregator_controllers(cluster, *count);
        }
    }

    /// Summary of all known controllers (for Admin API).
    pub fn controller_summaries(&self) -> Vec<ControllerSummary> {
        self.inner
            .read()
            .values()
            .map(|s| ControllerSummary {
                controller_id: s.info.controller_id.clone(),
                cluster: s.info.cluster.clone(),
                env: s.info.env.clone(),
                tag: s.info.tag.clone(),
                online: s.offline_since.is_none(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ControllerSummary {
    pub controller_id: String,
    pub cluster: String,
    pub env: Vec<String>,
    pub tag: Vec<String>,
    pub online: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::common::fed_sync::proto::RegisterRequest;

    fn mock_register_info(cid: &str, cluster: &str) -> RegisterRequest {
        RegisterRequest {
            controller_id: cid.to_string(),
            cluster: cluster.to_string(),
            env: vec![],
            tag: vec![],
            supported_kinds: vec![],
        }
    }

    #[test]
    fn test_set_and_mark_offline() {
        let agg = ResourceAggregator::new();
        agg.set_controller_info("ctrl-1", mock_register_info("ctrl-1", "cluster-a"));
        let summaries = agg.controller_summaries();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].online);
        agg.mark_offline("ctrl-1");
        let summaries = agg.controller_summaries();
        assert!(!summaries[0].online);
    }

    #[test]
    fn test_remove_drops_snapshot() {
        let agg = ResourceAggregator::new();
        agg.set_controller_info("ctrl-1", mock_register_info("ctrl-1", "cluster-a"));
        assert!(agg.remove("ctrl-1"));
        assert_eq!(agg.controller_summaries().len(), 0);
        assert!(!agg.remove("ctrl-1"));
    }

    #[test]
    fn test_reconnect_clears_offline() {
        let agg = ResourceAggregator::new();
        agg.set_controller_info("ctrl-1", mock_register_info("ctrl-1", "cluster-a"));
        agg.mark_offline("ctrl-1");
        assert!(!agg.controller_summaries()[0].online);
        agg.set_controller_info("ctrl-1", mock_register_info("ctrl-1", "cluster-a"));
        assert!(agg.controller_summaries()[0].online);
    }
}
