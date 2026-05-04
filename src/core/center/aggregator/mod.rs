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
        let mut map = self.inner.write();
        let snap = map
            .entry(controller_id.to_string())
            .or_insert_with(|| ControllerSnapshot::new(info.clone()));
        snap.info = info;
        snap.offline_since = None;
        Self::refresh_controller_gauges(&map);
    }

    pub fn mark_offline(&self, controller_id: &str) {
        let mut map = self.inner.write();
        if let Some(snap) = map.get_mut(controller_id) {
            if snap.offline_since.is_none() {
                snap.offline_since = Some(std::time::Instant::now());
            }
        }
        Self::refresh_controller_gauges(&map);
    }

    /// Remove a controller's snapshot entirely. Used by Admin DELETE cascade.
    pub fn remove(&self, controller_id: &str) -> bool {
        let mut map = self.inner.write();
        // Capture the cluster before removal so we can zero the gauge if it disappears.
        let removed_cluster = map.get(controller_id).map(|s| {
            if s.info.cluster.is_empty() {
                "unknown".to_string()
            } else {
                s.info.cluster.clone()
            }
        });
        let removed = map.remove(controller_id).is_some();
        if removed {
            fed_metrics::record_evict_stale(fed_metrics::labels::evict_source::AGGREGATOR);
            Self::refresh_controller_gauges(&map);
            // Pre-seeding cannot see a fully-removed entry; emit zero explicitly when the
            // last controller for this cluster is gone.
            if let Some(cluster) = removed_cluster {
                let still_present = map.values().any(|s| {
                    let c = if s.info.cluster.is_empty() {
                        "unknown"
                    } else {
                        s.info.cluster.as_str()
                    };
                    c == cluster
                });
                if !still_present {
                    fed_metrics::set_aggregator_controllers(&cluster, 0);
                }
            }
        }
        removed
    }

    /// Recompute the `edgion_fed_aggregator_controllers{cluster}` gauge.
    /// Caller must hold the write lock.
    fn refresh_controller_gauges(map: &HashMap<String, ControllerSnapshot>) {
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
        for (cluster, count) in &by_cluster {
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
