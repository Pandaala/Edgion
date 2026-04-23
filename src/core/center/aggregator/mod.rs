//! In-memory resource key aggregator.
//!
//! Stores per-controller snapshots. Snapshots are replaced on each ListResponse.
//! Offline entries are retained in memory and must be removed explicitly via
//! the Admin DELETE API (see ticket #20).

use crate::core::common::fed_sync::proto::{RegisterRequest, ResourceKey};
use crate::core::common::observe::fed_metrics;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug)]
struct ControllerSnapshot {
    info: RegisterRequest,
    keys: Vec<ResourceKey>,
    last_list_at: Option<Instant>,
    offline_since: Option<Instant>,
}

impl ControllerSnapshot {
    fn new(info: RegisterRequest) -> Self {
        Self {
            info,
            keys: vec![],
            last_list_at: None,
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

    /// Called when controller registers (or reconnects)
    pub fn set_controller_info(&self, controller_id: &str, info: RegisterRequest) {
        let mut map = self.inner.write();
        let snap = map
            .entry(controller_id.to_string())
            .or_insert_with(|| ControllerSnapshot::new(info.clone()));
        snap.info = info;
        snap.offline_since = None; // reconnect clears offline state
        Self::refresh_controller_gauges(&map);
    }

    /// Full-replace snapshot for a controller. Called on each ListResponse.
    pub fn update_snapshot(&self, controller_id: &str, keys: Vec<ResourceKey>) {
        let mut map = self.inner.write();
        if let Some(snap) = map.get_mut(controller_id) {
            snap.keys = keys;
            snap.last_list_at = Some(Instant::now());
        } else {
            // Controller connected but info not yet set (edge case); store anyway
            let mut snap = ControllerSnapshot::new(RegisterRequest {
                controller_id: controller_id.to_string(),
                ..Default::default()
            });
            snap.keys = keys;
            snap.last_list_at = Some(Instant::now());
            map.insert(controller_id.to_string(), snap);
        }
        Self::refresh_key_gauges(&map);
    }

    pub fn mark_offline(&self, controller_id: &str) {
        let mut map = self.inner.write();
        if let Some(snap) = map.get_mut(controller_id) {
            if snap.offline_since.is_none() {
                snap.offline_since = Some(Instant::now());
            }
        }
        // `aggregator_controllers` gauge is scoped to controllers whose
        // snapshots are still online; offline transitions must update it.
        Self::refresh_controller_gauges(&map);
    }

    /// Remove a controller's snapshot entirely. Used by Admin DELETE cascade.
    pub fn remove(&self, controller_id: &str) -> bool {
        let mut map = self.inner.write();
        let removed = map.remove(controller_id).is_some();
        if removed {
            fed_metrics::record_evict_stale(fed_metrics::labels::evict_source::AGGREGATOR);
            Self::refresh_controller_gauges(&map);
            Self::refresh_key_gauges(&map);
        }
        removed
    }

    /// Recompute the `edgion_fed_aggregator_controllers{cluster}` gauge from
    /// the current snapshot map, counting only controllers that are still
    /// online. Caller must hold the write lock.
    fn refresh_controller_gauges(map: &HashMap<String, ControllerSnapshot>) {
        let mut by_cluster: HashMap<String, u64> = HashMap::new();
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

    /// Recompute `edgion_fed_aggregator_keys_total{kind}` from the current
    /// snapshot map (online + offline, because keys are retained until
    /// explicitly removed). Caller must hold the write lock.
    fn refresh_key_gauges(map: &HashMap<String, ControllerSnapshot>) {
        let mut by_kind: HashMap<String, u64> = HashMap::new();
        for snap in map.values() {
            for key in &snap.keys {
                *by_kind.entry(key.kind.clone()).or_default() += 1;
            }
        }
        for (kind, count) in &by_kind {
            fed_metrics::set_aggregator_keys(kind, *count);
        }
    }

    /// All resource keys across online controllers only.
    /// Offline snapshots are retained in memory (for Admin API access)
    /// but excluded from the aggregate view consumed by `GET /api/v1/resources`.
    pub fn list_online_keys(&self) -> Vec<ResourceKey> {
        self.inner
            .read()
            .values()
            .filter(|s| s.offline_since.is_none())
            .flat_map(|s| s.keys.clone())
            .collect()
    }

    /// Online-only variant of `list_keys_by_cluster`.
    pub fn list_online_keys_by_cluster(&self, cluster: &str) -> Vec<ResourceKey> {
        self.inner
            .read()
            .values()
            .filter(|s| s.offline_since.is_none() && s.info.cluster == cluster)
            .flat_map(|s| s.keys.clone())
            .collect()
    }

    /// All resource keys across all controllers (online + offline).
    pub fn list_all_keys(&self) -> Vec<ResourceKey> {
        self.inner.read().values().flat_map(|s| s.keys.clone()).collect()
    }

    /// Resource keys from controllers in a specific cluster.
    pub fn list_keys_by_cluster(&self, cluster: &str) -> Vec<ResourceKey> {
        self.inner
            .read()
            .values()
            .filter(|s| s.info.cluster == cluster)
            .flat_map(|s| s.keys.clone())
            .collect()
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
                last_list_secs_ago: s.last_list_at.map(|t| t.elapsed().as_secs()),
                key_count: s.keys.len(),
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
    /// Seconds elapsed since last successful list (None = never listed)
    pub last_list_secs_ago: Option<u64>,
    pub key_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::common::fed_sync::proto::RegisterRequest;

    fn make_key(kind: &str, name: &str) -> ResourceKey {
        ResourceKey {
            kind: kind.to_string(),
            namespace: "default".to_string(),
            name: name.to_string(),
            resource_version: "1".to_string(),
            labels: Default::default(),
            annotations: Default::default(),
        }
    }

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
    fn test_update_and_query() {
        let agg = ResourceAggregator::new();
        agg.update_snapshot("ctrl-1", vec![make_key("HTTPRoute", "r1")]);
        let all = agg.list_all_keys();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "r1");
    }

    #[test]
    fn test_offline_data_retained_until_manual_remove() {
        let agg = ResourceAggregator::new();
        agg.update_snapshot("ctrl-1", vec![make_key("Gateway", "g1")]);
        agg.mark_offline("ctrl-1");
        // Without scheduler-driven eviction, offline snapshot stays in memory
        // until an explicit Admin DELETE (ticket #20) removes it.
        assert_eq!(agg.list_all_keys().len(), 1);
    }

    #[test]
    fn test_query_by_cluster() {
        let agg = ResourceAggregator::new();
        let info1 = RegisterRequest {
            controller_id: "a/c1".to_string(),
            cluster: "cluster-a".to_string(),
            env: vec![],
            tag: vec![],
            supported_kinds: vec![],
        };
        let info2 = RegisterRequest {
            controller_id: "b/c2".to_string(),
            cluster: "cluster-b".to_string(),
            env: vec![],
            tag: vec![],
            supported_kinds: vec![],
        };
        agg.set_controller_info("a/c1", info1);
        agg.set_controller_info("b/c2", info2);
        agg.update_snapshot("a/c1", vec![make_key("HTTPRoute", "r1")]);
        agg.update_snapshot("b/c2", vec![make_key("HTTPRoute", "r2")]);
        let cluster_a = agg.list_keys_by_cluster("cluster-a");
        assert_eq!(cluster_a.len(), 1);
        assert_eq!(cluster_a[0].name, "r1");
    }

    #[test]
    fn test_remove_drops_snapshot() {
        let agg = ResourceAggregator::new();
        agg.update_snapshot("ctrl-1", vec![make_key("Gateway", "g1")]);
        assert!(agg.remove("ctrl-1"));
        assert_eq!(agg.list_all_keys().len(), 0);
        assert!(!agg.remove("ctrl-1"));
    }

    #[test]
    fn test_list_online_keys_excludes_offline() {
        let agg = ResourceAggregator::new();
        agg.set_controller_info("c-on", mock_register_info("c-on", "cluster-a"));
        agg.update_snapshot("c-on", vec![make_key("Gateway", "g-on")]);
        agg.set_controller_info("c-off", mock_register_info("c-off", "cluster-a"));
        agg.update_snapshot("c-off", vec![make_key("Gateway", "g-off")]);
        agg.mark_offline("c-off");

        let online = agg.list_online_keys();
        assert_eq!(online.len(), 1);
        assert_eq!(online[0].name, "g-on");

        // list_all_keys unchanged (still returns both)
        assert_eq!(agg.list_all_keys().len(), 2);

        let by_cluster = agg.list_online_keys_by_cluster("cluster-a");
        assert_eq!(by_cluster.len(), 1);
    }
}
