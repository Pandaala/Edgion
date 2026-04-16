//! CenterMetaDataStore — aggregates RegionRoute data across all controllers.
//!
//! Implements [`CenterConfHandler<PluginMetaData>`] and stores per-controller entries
//! keyed by `pm_key` (namespace/name). Provides query APIs for HTTP handlers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::center::watch_cache::CenterConfHandler;
use crate::core::common::metadata_conf_handler::{parse_region_route, ParsedRegionRoute};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::region_route_table::{
    ClusterRegionRouteEntry, ServiceRegionRouteEntry,
};
use crate::types::resources::edgion_plugins::plugin_configs::PluginMetaDataRef;
use crate::types::resources::plugin_metadata::PluginMetaData;

/// View of aggregated ClusterRegionRoute entries from all controllers for a single pm_key.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CenterClusterRouteView {
    pub namespace: String,
    pub name: String,
    /// Map of controller_id → ClusterRegionRouteEntry
    pub controllers: HashMap<String, ClusterRegionRouteEntry>,
}

/// View of aggregated ServiceRegionRoute entries from all controllers for a single pm_key.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CenterServiceRouteView {
    pub namespace: String,
    pub name: String,
    pub cluster_ref: Option<PluginMetaDataRef>,
    /// Map of controller_id → ServiceRegionRouteEntry
    pub controllers: HashMap<String, ServiceRegionRouteEntry>,
}

/// Aggregates RegionRoute PluginMetaData across all controllers.
///
/// Internal structure:
/// - `cluster_routes`: pm_key → { controller_id → ClusterRegionRouteEntry }
/// - `service_routes`: pm_key → { controller_id → ServiceRegionRouteEntry }
pub struct CenterMetaDataStore {
    cluster_routes: RwLock<HashMap<String, HashMap<String, ClusterRegionRouteEntry>>>,
    service_routes: RwLock<HashMap<String, HashMap<String, ServiceRegionRouteEntry>>>,
}

impl CenterMetaDataStore {
    pub fn new() -> Self {
        Self {
            cluster_routes: RwLock::new(HashMap::new()),
            service_routes: RwLock::new(HashMap::new()),
        }
    }

    /// List all ClusterRegionRoute entries aggregated across all controllers.
    pub fn list_cluster_routes(&self) -> Vec<CenterClusterRouteView> {
        self.cluster_routes
            .read()
            .iter()
            .map(|(pm_key, controllers)| {
                let (namespace, name) = split_pm_key(pm_key);
                CenterClusterRouteView {
                    namespace,
                    name,
                    controllers: controllers.clone(),
                }
            })
            .collect()
    }

    /// List all ServiceRegionRoute entries aggregated across all controllers.
    pub fn list_service_routes(&self) -> Vec<CenterServiceRouteView> {
        self.service_routes
            .read()
            .iter()
            .map(|(pm_key, controllers)| {
                let (namespace, name) = split_pm_key(pm_key);
                let cluster_ref = controllers.values().next().and_then(|e| e.cluster_pm_ref.clone());
                CenterServiceRouteView {
                    namespace,
                    name,
                    cluster_ref,
                    controllers: controllers.clone(),
                }
            })
            .collect()
    }

    /// Remove all entries for a given controller from both maps.
    /// If an inner HashMap becomes empty after removal, the outer key is also removed.
    fn remove_all_for_controller(&self, controller_id: &str) {
        {
            let mut cluster = self.cluster_routes.write();
            for inner in cluster.values_mut() {
                inner.remove(controller_id);
            }
            cluster.retain(|_, inner| !inner.is_empty());
        }
        {
            let mut service = self.service_routes.write();
            for inner in service.values_mut() {
                inner.remove(controller_id);
            }
            service.retain(|_, inner| !inner.is_empty());
        }
    }

    /// Insert or update a single PluginMetaData entry for the given controller.
    fn upsert_pm(&self, controller_id: &str, pm_key: &str, pm: &PluginMetaData) {
        match parse_region_route(pm) {
            ParsedRegionRoute::Cluster { entry, .. } => {
                self.cluster_routes
                    .write()
                    .entry(pm_key.to_string())
                    .or_default()
                    .insert(controller_id.to_string(), entry);
            }
            ParsedRegionRoute::Service { entry, .. } => {
                self.service_routes
                    .write()
                    .entry(pm_key.to_string())
                    .or_default()
                    .insert(controller_id.to_string(), entry);
            }
            ParsedRegionRoute::Other => {}
        }
    }

    /// Remove a single pm_key entry for the given controller from both maps.
    /// Cannot know the type at delete time, so tries both.
    fn remove_pm(&self, controller_id: &str, pm_key: &str) {
        {
            let mut cluster = self.cluster_routes.write();
            if let Some(inner) = cluster.get_mut(pm_key) {
                inner.remove(controller_id);
                if inner.is_empty() {
                    cluster.remove(pm_key);
                }
            }
        }
        {
            let mut service = self.service_routes.write();
            if let Some(inner) = service.get_mut(pm_key) {
                inner.remove(controller_id);
                if inner.is_empty() {
                    service.remove(pm_key);
                }
            }
        }
    }
}

impl Default for CenterMetaDataStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CenterConfHandler<PluginMetaData> for CenterMetaDataStore {
    fn full_set(&self, controller_id: &str, data: &HashMap<String, Arc<PluginMetaData>>) {
        self.remove_all_for_controller(controller_id);
        for (pm_key, pm) in data {
            self.upsert_pm(controller_id, pm_key, pm);
        }
    }

    fn partial_update(
        &self,
        controller_id: &str,
        add: HashMap<String, Arc<PluginMetaData>>,
        update: HashMap<String, Arc<PluginMetaData>>,
        remove: HashSet<String>,
    ) {
        for (pm_key, pm) in add.iter().chain(update.iter()) {
            self.upsert_pm(controller_id, pm_key, pm);
        }
        for pm_key in &remove {
            self.remove_pm(controller_id, pm_key);
        }
    }

    fn controller_offline(&self, _controller_id: &str) {
        // Keep data — offline controller's config remains queryable
    }

    fn controller_removed(&self, controller_id: &str) {
        self.remove_all_for_controller(controller_id);
    }
}

/// Split a pm_key of the form "namespace/name" into (namespace, name).
/// If there is no '/', returns ("", pm_key).
fn split_pm_key(pm_key: &str) -> (String, String) {
    match pm_key.split_once('/') {
        Some((ns, name)) => (ns.to_string(), name.to_string()),
        None => (String::new(), pm_key.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::plugin_metadata::{
        ClusterRegionRouteConfig, MetaDataEntry, PluginMetaDataSpec, ServiceRegionRouteConfig,
    };

    fn make_cluster_pm(namespace: &str, name: &str) -> PluginMetaData {
        let spec = PluginMetaDataSpec {
            metadata: MetaDataEntry::ClusterRegionRoute(ClusterRegionRouteConfig {
                my_region: "us-west".to_string(),
                regions: vec![],
                key_get: vec![],
                hash_key_get: None,
                hash_calc: None,
                route_rules: vec![],
                route_by_key_conf_match: None,
            }),
        };
        let mut pm = PluginMetaData::new(name, spec);
        pm.metadata.namespace = Some(namespace.to_string());
        pm
    }

    fn make_service_pm(namespace: &str, name: &str) -> PluginMetaData {
        let spec = PluginMetaDataSpec {
            metadata: MetaDataEntry::ServiceRegionRoute(ServiceRegionRouteConfig {
                cluster_ref: None,
                regions: vec![],
            }),
        };
        let mut pm = PluginMetaData::new(name, spec);
        pm.metadata.namespace = Some(namespace.to_string());
        pm
    }

    fn make_full_set(items: Vec<(&str, PluginMetaData)>) -> HashMap<String, Arc<PluginMetaData>> {
        items
            .into_iter()
            .map(|(key, pm)| (key.to_string(), Arc::new(pm)))
            .collect()
    }

    #[test]
    fn full_set_populates_cluster_routes() {
        let store = CenterMetaDataStore::new();
        let pm = make_cluster_pm("prod", "global-rr");
        let data = make_full_set(vec![("prod/global-rr", pm)]);
        store.full_set("ctrl-1", &data);

        let routes = store.list_cluster_routes();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].namespace, "prod");
        assert_eq!(routes[0].name, "global-rr");
        assert!(routes[0].controllers.contains_key("ctrl-1"));
    }

    #[test]
    fn full_set_replaces_previous_data() {
        let store = CenterMetaDataStore::new();

        // First full_set with one key
        let pm1 = make_cluster_pm("prod", "rr-old");
        let data1 = make_full_set(vec![("prod/rr-old", pm1)]);
        store.full_set("ctrl-1", &data1);
        assert_eq!(store.list_cluster_routes().len(), 1);

        // Second full_set for same controller with a different key
        let pm2 = make_cluster_pm("prod", "rr-new");
        let data2 = make_full_set(vec![("prod/rr-new", pm2)]);
        store.full_set("ctrl-1", &data2);

        let routes = store.list_cluster_routes();
        assert_eq!(routes.len(), 1, "old entry should be gone");
        assert_eq!(routes[0].name, "rr-new");
    }

    #[test]
    fn multi_controller_aggregation() {
        let store = CenterMetaDataStore::new();

        let pm1 = make_cluster_pm("prod", "shared-rr");
        let data1 = make_full_set(vec![("prod/shared-rr", pm1)]);
        store.full_set("ctrl-1", &data1);

        let pm2 = make_cluster_pm("prod", "shared-rr");
        let data2 = make_full_set(vec![("prod/shared-rr", pm2)]);
        store.full_set("ctrl-2", &data2);

        let routes = store.list_cluster_routes();
        assert_eq!(routes.len(), 1, "same pm_key should be one entry");
        assert_eq!(routes[0].controllers.len(), 2, "should have 2 controllers");
        assert!(routes[0].controllers.contains_key("ctrl-1"));
        assert!(routes[0].controllers.contains_key("ctrl-2"));
    }

    #[test]
    fn controller_removed_cleans_up() {
        let store = CenterMetaDataStore::new();

        let pm = make_cluster_pm("prod", "global-rr");
        let data = make_full_set(vec![("prod/global-rr", pm)]);
        store.full_set("ctrl-1", &data);
        assert_eq!(store.list_cluster_routes().len(), 1);

        store.controller_removed("ctrl-1");

        let routes = store.list_cluster_routes();
        assert!(routes.is_empty(), "all routes should be removed");
        // Verify inner maps are also cleaned (no empty outer keys)
        assert!(
            store.cluster_routes.read().is_empty(),
            "outer map should be empty after cleanup"
        );
    }

    #[test]
    fn partial_update_remove_cleans_both_maps() {
        let store = CenterMetaDataStore::new();

        // Add one cluster route and one service route for ctrl-1
        let cluster_pm = make_cluster_pm("prod", "c-rr");
        let service_pm = make_service_pm("prod", "s-rr");
        let data = make_full_set(vec![("prod/c-rr", cluster_pm), ("prod/s-rr", service_pm)]);
        store.full_set("ctrl-1", &data);
        assert_eq!(store.list_cluster_routes().len(), 1);
        assert_eq!(store.list_service_routes().len(), 1);

        // partial_update remove — we don't know which map each key belongs to
        let remove: HashSet<String> = ["prod/c-rr", "prod/s-rr"].iter().map(|s| s.to_string()).collect();
        store.partial_update("ctrl-1", HashMap::new(), HashMap::new(), remove);

        assert!(
            store.list_cluster_routes().is_empty(),
            "cluster route should be removed"
        );
        assert!(
            store.list_service_routes().is_empty(),
            "service route should be removed"
        );
    }
}
