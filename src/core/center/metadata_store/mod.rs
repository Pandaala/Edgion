//! CenterMetaDataStore — aggregates RegionRoute data across all controllers.
//!
//! Implements [`CenterConfHandler<PluginMetaData>`] and stores per-controller entries
//! keyed by `pm_key` (namespace/name). Provides query APIs for HTTP handlers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::center::watch_cache::CenterConfHandler;
use crate::core::common::metadata_conf_handler::{parse_region_route, ParsedRegionRoute};
use edgion_resources::resources::edgion_plugins::plugin_configs::PluginMetaDataRef;
use edgion_resources::resources::plugin_metadata::PluginMetaData;
use edgion_resources::resources::plugin_metadata::{ClusterRegionRouteEntry, ServiceRegionRouteEntry};
use edgion_resources::resources::plugin_metadata::{GlobalConnectionIpRestrictionData, MetaDataEntry, ProfileRules};

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

/// Per-Controller view of a GlobalConnectionIpRestriction PM aggregated at Center.
/// (Naming disambiguation: distinct from `GlobalConnectionIpRestrictionEntry` in
/// edgion_stream_plugins — that one is the stream-plugin ref carrier.)
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerPmEntry {
    pub pm_namespace: String,
    pub pm_name: String,
    pub enable: bool,
    pub active_profile: String,
    pub profiles: HashMap<String, ProfileRules>,
    pub description: Option<String>,
    /// sha256 over canonical JSON of the PM data; used for consistency/freshness display.
    pub content_hash: String,
    /// Unix ms of last modification on the Controller (Center-assigned on observation).
    pub last_modified: i64,
}

/// Aggregated view of one GlobalConnectionIpRestriction PM across all Controllers.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CenterGlobalIpRestrictionEntryView {
    pub namespace: String,
    pub name: String,
    /// Map of controller_id → ControllerPmEntry
    pub controllers: HashMap<String, ControllerPmEntry>,
}

/// Aggregates RegionRoute PluginMetaData across all controllers.
///
/// Internal structure:
/// - `cluster_routes`: pm_key → { controller_id → ClusterRegionRouteEntry }
/// - `service_routes`: pm_key → { controller_id → ServiceRegionRouteEntry }
/// - `global_ip_restrictions`: pm_key → { controller_id → ControllerPmEntry }
pub struct CenterMetaDataStore {
    cluster_routes: RwLock<HashMap<String, HashMap<String, ClusterRegionRouteEntry>>>,
    service_routes: RwLock<HashMap<String, HashMap<String, ServiceRegionRouteEntry>>>,
    // pm_key ("ns/name") → { controller_id → entry }
    global_ip_restrictions: RwLock<HashMap<String, HashMap<String, ControllerPmEntry>>>,
}

impl CenterMetaDataStore {
    pub fn new() -> Self {
        Self {
            cluster_routes: RwLock::new(HashMap::new()),
            service_routes: RwLock::new(HashMap::new()),
            global_ip_restrictions: RwLock::new(HashMap::new()),
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

    /// List all GlobalConnectionIpRestriction PMs aggregated across Controllers.
    /// Returns one view per `pm_key`, with the outer key already split into
    /// `namespace`/`name` so callers don't pay for an extra `pm_key` clone.
    pub fn list_global_ip_restrictions(&self) -> Vec<CenterGlobalIpRestrictionEntryView> {
        self.global_ip_restrictions
            .read()
            .iter()
            .map(|(pm_key, controllers)| {
                let (namespace, name) = split_pm_key(pm_key);
                CenterGlobalIpRestrictionEntryView {
                    namespace,
                    name,
                    controllers: controllers.clone(),
                }
            })
            .collect()
    }

    /// Get a single PM's entries across Controllers.
    pub fn get_global_ip_restriction(&self, pm_key: &str) -> Option<HashMap<String, ControllerPmEntry>> {
        self.global_ip_restrictions.read().get(pm_key).cloned()
    }

    /// Remove all entries for a given controller from both maps.
    /// If an inner HashMap becomes empty after removal, the outer key is also removed.
    fn remove_all_for_controller(&self, controller_id: &str) {
        {
            let mut cluster = self.cluster_routes.write();
            cluster.retain(|_, inner| {
                inner.remove(controller_id);
                !inner.is_empty()
            });
        }
        {
            let mut service = self.service_routes.write();
            service.retain(|_, inner| {
                inner.remove(controller_id);
                !inner.is_empty()
            });
        }
        {
            let mut gir = self.global_ip_restrictions.write();
            gir.retain(|_, inner| {
                inner.remove(controller_id);
                !inner.is_empty()
            });
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
        // Classify all PMs first (no locks needed)
        let mut new_cluster: HashMap<String, ClusterRegionRouteEntry> = HashMap::new();
        let mut new_service: HashMap<String, ServiceRegionRouteEntry> = HashMap::new();
        let mut new_gir: HashMap<String, ControllerPmEntry> = HashMap::new();

        for (pm_key, pm) in data {
            match parse_region_route(pm) {
                ParsedRegionRoute::Cluster { entry, .. } => {
                    new_cluster.insert(pm_key.clone(), entry);
                }
                ParsedRegionRoute::Service { entry, .. } => {
                    new_service.insert(pm_key.clone(), entry);
                }
                ParsedRegionRoute::Other => {}
            }
            if let MetaDataEntry::GlobalConnectionIpRestriction(ref gir_data) = pm.spec.metadata {
                let ns = pm.metadata.namespace.clone().unwrap_or_default();
                let name = pm.metadata.name.clone().unwrap_or_default();
                let entry = ControllerPmEntry {
                    pm_namespace: ns,
                    pm_name: name,
                    enable: gir_data.enable,
                    active_profile: gir_data.active_profile.clone(),
                    profiles: gir_data.profiles.clone(),
                    description: gir_data.description.clone(),
                    content_hash: canonical_hash(gir_data),
                    last_modified: chrono::Utc::now().timestamp_millis(),
                };
                new_gir.insert(pm_key.clone(), entry);
            }
        }

        // Single lock acquisition for cluster_routes
        {
            let mut cluster = self.cluster_routes.write();
            cluster.retain(|_, controllers| {
                controllers.remove(controller_id);
                !controllers.is_empty()
            });
            for (pm_key, entry) in new_cluster {
                cluster
                    .entry(pm_key)
                    .or_default()
                    .insert(controller_id.to_string(), entry);
            }
        }

        // Single lock acquisition for service_routes
        {
            let mut service = self.service_routes.write();
            service.retain(|_, controllers| {
                controllers.remove(controller_id);
                !controllers.is_empty()
            });
            for (pm_key, entry) in new_service {
                service
                    .entry(pm_key)
                    .or_default()
                    .insert(controller_id.to_string(), entry);
            }
        }

        // Single lock acquisition for global_ip_restrictions
        {
            let mut gir = self.global_ip_restrictions.write();
            gir.retain(|_, controllers| {
                controllers.remove(controller_id);
                !controllers.is_empty()
            });
            for (pm_key, entry) in new_gir {
                gir.entry(pm_key).or_default().insert(controller_id.to_string(), entry);
            }
        }
    }

    fn partial_update(
        &self,
        controller_id: &str,
        add: HashMap<String, Arc<PluginMetaData>>,
        update: HashMap<String, Arc<PluginMetaData>>,
        remove: HashSet<String>,
    ) {
        // Classify add+update (no locks needed)
        let mut cluster_upserts: HashMap<String, ClusterRegionRouteEntry> = HashMap::new();
        let mut service_upserts: HashMap<String, ServiceRegionRouteEntry> = HashMap::new();
        let mut gir_upserts: HashMap<String, ControllerPmEntry> = HashMap::new();

        for (pm_key, pm) in add.iter().chain(update.iter()) {
            match parse_region_route(pm) {
                ParsedRegionRoute::Cluster { entry, .. } => {
                    cluster_upserts.insert(pm_key.clone(), entry);
                }
                ParsedRegionRoute::Service { entry, .. } => {
                    service_upserts.insert(pm_key.clone(), entry);
                }
                ParsedRegionRoute::Other => {}
            }
            if let MetaDataEntry::GlobalConnectionIpRestriction(ref gir_data) = pm.spec.metadata {
                let ns = pm.metadata.namespace.clone().unwrap_or_default();
                let name = pm.metadata.name.clone().unwrap_or_default();
                let entry = ControllerPmEntry {
                    pm_namespace: ns,
                    pm_name: name,
                    enable: gir_data.enable,
                    active_profile: gir_data.active_profile.clone(),
                    profiles: gir_data.profiles.clone(),
                    description: gir_data.description.clone(),
                    content_hash: canonical_hash(gir_data),
                    last_modified: chrono::Utc::now().timestamp_millis(),
                };
                gir_upserts.insert(pm_key.clone(), entry);
            }
        }

        // Apply cluster changes in one lock
        {
            let mut cluster = self.cluster_routes.write();
            for pm_key in &remove {
                if let Some(controllers) = cluster.get_mut(pm_key.as_str()) {
                    controllers.remove(controller_id);
                    if controllers.is_empty() {
                        cluster.remove(pm_key.as_str());
                    }
                }
            }
            for (pm_key, entry) in cluster_upserts {
                cluster
                    .entry(pm_key)
                    .or_default()
                    .insert(controller_id.to_string(), entry);
            }
        }

        // Apply service changes in one lock
        {
            let mut service = self.service_routes.write();
            for pm_key in &remove {
                if let Some(controllers) = service.get_mut(pm_key.as_str()) {
                    controllers.remove(controller_id);
                    if controllers.is_empty() {
                        service.remove(pm_key.as_str());
                    }
                }
            }
            for (pm_key, entry) in service_upserts {
                service
                    .entry(pm_key)
                    .or_default()
                    .insert(controller_id.to_string(), entry);
            }
        }

        // Apply global_ip_restrictions changes in one lock
        {
            let mut gir = self.global_ip_restrictions.write();
            for pm_key in &remove {
                if let Some(controllers) = gir.get_mut(pm_key.as_str()) {
                    controllers.remove(controller_id);
                    if controllers.is_empty() {
                        gir.remove(pm_key.as_str());
                    }
                }
            }
            for (pm_key, entry) in gir_upserts {
                gir.entry(pm_key).or_default().insert(controller_id.to_string(), entry);
            }
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

/// Compute a deterministic SHA-256 hex digest over the canonical JSON of a
/// [`GlobalConnectionIpRestrictionData`].  Profile map keys are sorted before
/// serialization so that insertion order cannot affect the output.
///
/// The hash is used for UI freshness/consistency display only — crypto strength
/// is not required, but **determinism is**.
fn canonical_hash(data: &GlobalConnectionIpRestrictionData) -> String {
    use sha2::{Digest, Sha256};

    // Sort profile entries for a deterministic iteration order.
    let mut profile_pairs: Vec<(&String, &ProfileRules)> = data.profiles.iter().collect();
    profile_pairs.sort_by(|a, b| a.0.cmp(b.0));

    // serde_json::json! serializes ProfileRules via its Serialize impl which
    // already skips `allow_matcher` / `deny_matcher` (#[serde(skip)]), so the
    // hash input never contains runtime-only fields.
    let canon = serde_json::json!({
        "enable": data.enable,
        "activeProfile": data.active_profile,
        "description": data.description,
        "profiles": profile_pairs
            .iter()
            .map(|(k, v)| (k, v))
            .collect::<Vec<_>>(),
    });

    let mut hasher = Sha256::new();
    hasher.update(canon.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::plugin_metadata::{
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

    // ── GlobalConnectionIpRestriction helpers ──────────────────────────────────

    fn make_gir_pm(namespace: &str, name: &str) -> PluginMetaData {
        use edgion_resources::resources::edgion_plugins::DefaultAction;
        use edgion_resources::resources::plugin_metadata::{IpGroup, ProfileRules};

        let mut profiles = HashMap::new();
        profiles.insert(
            "default".to_string(),
            ProfileRules {
                allow: Some(vec![IpGroup {
                    name: "internal".to_string(),
                    description: None,
                    cidrs: vec!["10.0.0.0/8".to_string()],
                }]),
                deny: None,
                default_action: DefaultAction::Deny,
                allow_matcher: None,
                deny_matcher: None,
            },
        );
        let spec = PluginMetaDataSpec {
            metadata: MetaDataEntry::GlobalConnectionIpRestriction(GlobalConnectionIpRestrictionData {
                enable: true,
                active_profile: "default".to_string(),
                profiles,
                description: None,
            }),
        };
        let mut pm = PluginMetaData::new(name, spec);
        pm.metadata.namespace = Some(namespace.to_string());
        pm
    }

    #[test]
    fn full_set_populates_global_ip_restrictions() {
        let store = CenterMetaDataStore::new();
        let pm = make_gir_pm("prod", "global-ip");
        let data = make_full_set(vec![("prod/global-ip", pm)]);
        store.full_set("ctrl-1", &data);

        let result = store.list_global_ip_restrictions();
        assert_eq!(result.len(), 1);
        let view = result
            .iter()
            .find(|v| v.namespace == "prod" && v.name == "global-ip")
            .expect("should have pm_key");
        assert!(view.controllers.contains_key("ctrl-1"));
        let entry = &view.controllers["ctrl-1"];
        assert_eq!(entry.pm_namespace, "prod");
        assert_eq!(entry.pm_name, "global-ip");
        assert!(entry.enable);
        assert_eq!(entry.active_profile, "default");
        assert!(!entry.content_hash.is_empty());
    }

    #[test]
    fn full_set_replaces_gir_for_same_controller() {
        let store = CenterMetaDataStore::new();

        let pm1 = make_gir_pm("prod", "gir-old");
        let data1 = make_full_set(vec![("prod/gir-old", pm1)]);
        store.full_set("ctrl-1", &data1);
        assert_eq!(store.list_global_ip_restrictions().len(), 1);

        let pm2 = make_gir_pm("prod", "gir-new");
        let data2 = make_full_set(vec![("prod/gir-new", pm2)]);
        store.full_set("ctrl-1", &data2);

        let result = store.list_global_ip_restrictions();
        assert_eq!(result.len(), 1, "old entry should be gone");
        assert!(result.iter().any(|v| v.namespace == "prod" && v.name == "gir-new"));
        assert!(!result.iter().any(|v| v.namespace == "prod" && v.name == "gir-old"));
    }

    #[test]
    fn multi_controller_gir_aggregation() {
        let store = CenterMetaDataStore::new();

        let pm1 = make_gir_pm("prod", "shared-gir");
        let data1 = make_full_set(vec![("prod/shared-gir", pm1)]);
        store.full_set("ctrl-1", &data1);

        let pm2 = make_gir_pm("prod", "shared-gir");
        let data2 = make_full_set(vec![("prod/shared-gir", pm2)]);
        store.full_set("ctrl-2", &data2);

        let result = store.list_global_ip_restrictions();
        assert_eq!(result.len(), 1, "same pm_key → one outer entry");
        let view = result
            .iter()
            .find(|v| v.namespace == "prod" && v.name == "shared-gir")
            .expect("should have pm_key");
        assert_eq!(view.controllers.len(), 2, "two controllers");
        assert!(view.controllers.contains_key("ctrl-1"));
        assert!(view.controllers.contains_key("ctrl-2"));
    }

    #[test]
    fn partial_update_upserts_and_removes_gir() {
        let store = CenterMetaDataStore::new();

        // Seed via full_set
        let pm = make_gir_pm("prod", "gir-a");
        let data = make_full_set(vec![("prod/gir-a", pm)]);
        store.full_set("ctrl-1", &data);
        assert_eq!(store.list_global_ip_restrictions().len(), 1);

        // Add a second GIR via partial_update
        let pm_b = make_gir_pm("prod", "gir-b");
        let add = make_full_set(vec![("prod/gir-b", pm_b)]);
        store.partial_update("ctrl-1", add, HashMap::new(), HashSet::new());
        assert_eq!(store.list_global_ip_restrictions().len(), 2);

        // Remove the first via partial_update
        let remove: HashSet<String> = ["prod/gir-a".to_string()].into_iter().collect();
        store.partial_update("ctrl-1", HashMap::new(), HashMap::new(), remove);
        let result = store.list_global_ip_restrictions();
        assert_eq!(result.len(), 1);
        assert!(result.iter().any(|v| v.namespace == "prod" && v.name == "gir-b"));
    }

    #[test]
    fn canonical_hash_stable_across_profile_insert_order() {
        use edgion_resources::resources::edgion_plugins::DefaultAction;
        use edgion_resources::resources::plugin_metadata::{IpGroup, ProfileRules};

        fn mk_profile() -> ProfileRules {
            ProfileRules {
                allow: Some(vec![IpGroup {
                    name: "g".into(),
                    description: None,
                    cidrs: vec!["10.0.0.0/8".into()],
                }]),
                deny: None,
                default_action: DefaultAction::Deny,
                allow_matcher: None,
                deny_matcher: None,
            }
        }

        let d1 = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "a".to_string(),
            profiles: {
                let mut m = HashMap::new();
                m.insert("a".to_string(), mk_profile());
                m.insert("b".to_string(), mk_profile());
                m
            },
            description: None,
        };
        let d2 = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "a".to_string(),
            profiles: {
                let mut m = HashMap::new();
                m.insert("b".to_string(), mk_profile()); // reverse order
                m.insert("a".to_string(), mk_profile());
                m
            },
            description: None,
        };
        assert_eq!(canonical_hash(&d1), canonical_hash(&d2));
    }
}
