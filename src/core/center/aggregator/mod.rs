//! In-memory resource key aggregator.
//!
//! Stores per-controller snapshots. Snapshots are replaced on each ListResponse.
//! Offline entries are retained in memory and must be removed explicitly via
//! the Admin DELETE API (see ticket #20).

use crate::core::common::fed_sync::proto::{RegisterRequest, ResourceKey};
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
    }

    pub fn mark_offline(&self, controller_id: &str) {
        if let Some(snap) = self.inner.write().get_mut(controller_id) {
            if snap.offline_since.is_none() {
                snap.offline_since = Some(Instant::now());
            }
        }
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
}
