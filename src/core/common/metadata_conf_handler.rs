//! Shared PluginMetaData → RegionRoute parsing logic.
//!
//! Used by both the Controller's PluginMetadataHandler and Center's CenterMetaDataStore
//! to parse raw PluginMetaData resources into typed cluster/service route entries.

use edgion_resources::resources::edgion_plugins::plugin_configs::PluginMetaDataRef;
use edgion_resources::resources::plugin_metadata::{ClusterRegionRouteEntry, ServiceRegionRouteEntry};
use edgion_resources::resources::plugin_metadata::{MetaDataEntry, PluginMetaData};

/// The result of parsing a PluginMetaData resource for RegionRoute purposes.
pub enum ParsedRegionRoute {
    /// A ClusterRegionRoute entry with its pm_key and fully constructed table entry.
    Cluster {
        pm_key: String,
        entry: ClusterRegionRouteEntry,
    },
    /// A ServiceRegionRoute entry with its pm_key, optional cluster reference, and table entry.
    Service {
        pm_key: String,
        cluster_ref: Option<PluginMetaDataRef>,
        entry: ServiceRegionRouteEntry,
    },
    /// Not a RegionRoute type — caller should ignore or handle separately.
    Other,
}

/// Parse a PluginMetaData into its RegionRoute type.
///
/// Returns `ParsedRegionRoute::Cluster` or `ParsedRegionRoute::Service` for the corresponding
/// MetaDataEntry variants, and `ParsedRegionRoute::Other` for all other types.
pub fn parse_region_route(pm: &PluginMetaData) -> ParsedRegionRoute {
    let ns = pm.metadata.namespace.as_deref().unwrap_or("default");
    let name = pm.metadata.name.as_deref().unwrap_or("");
    let pm_key = format!("{}/{}", ns, name);

    match &pm.spec.metadata {
        MetaDataEntry::ClusterRegionRoute(cfg) => {
            let entry = ClusterRegionRouteEntry {
                pm_namespace: ns.to_string(),
                pm_name: name.to_string(),
                my_region: cfg.my_region.clone(),
                regions: cfg.regions.clone(),
                key_get: cfg.key_get.clone(),
                hash_key_get: cfg.hash_key_get.clone(),
                hash_calc: cfg.hash_calc.clone(),
                route_rules: cfg.route_rules.clone(),
                route_by_key_conf_match: cfg.route_by_key_conf_match.clone(),
            };
            ParsedRegionRoute::Cluster { pm_key, entry }
        }
        MetaDataEntry::ServiceRegionRoute(cfg) => {
            let entry = ServiceRegionRouteEntry {
                pm_namespace: ns.to_string(),
                pm_name: name.to_string(),
                cluster_pm_ref: cfg.cluster_ref.clone(),
                regions: cfg.regions.clone(),
                ref_plugins: vec![], // Center doesn't track ref_plugins
            };
            ParsedRegionRoute::Service {
                pm_key,
                cluster_ref: cfg.cluster_ref.clone(),
                entry,
            }
        }
        _ => ParsedRegionRoute::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::plugin_metadata::{
        ClusterRegionRouteConfig, KeyGroup, KeyListData, KeyMatchMode, MetaDataEntry, MetaDataItem, PluginMetaDataSpec,
        ServiceRegionRouteConfig,
    };

    fn build_plugin_metadata(namespace: &str, name: &str, spec: PluginMetaDataSpec) -> PluginMetaData {
        let mut pm = PluginMetaData::new(name, spec);
        pm.metadata.namespace = Some(namespace.to_string());
        pm
    }

    #[test]
    fn parse_cluster_region_route() {
        let pm = build_plugin_metadata(
            "prod",
            "global-rr",
            PluginMetaDataSpec {
                metadata: MetaDataEntry::ClusterRegionRoute(ClusterRegionRouteConfig {
                    my_region: "us-west".to_string(),
                    regions: vec![],
                    key_get: vec![],
                    hash_key_get: None,
                    hash_calc: None,
                    route_rules: vec![],
                    route_by_key_conf_match: None,
                }),
            },
        );

        match parse_region_route(&pm) {
            ParsedRegionRoute::Cluster { pm_key, entry } => {
                assert_eq!(pm_key, "prod/global-rr");
                assert_eq!(entry.my_region, "us-west");
                assert_eq!(entry.pm_namespace, "prod");
                assert_eq!(entry.pm_name, "global-rr");
            }
            _ => panic!("expected Cluster variant"),
        }
    }

    #[test]
    fn parse_service_region_route() {
        let pm = build_plugin_metadata(
            "default",
            "svc-rr",
            PluginMetaDataSpec {
                metadata: MetaDataEntry::ServiceRegionRoute(ServiceRegionRouteConfig {
                    cluster_ref: Some(PluginMetaDataRef {
                        name: "global-rr".to_string(),
                        namespace: Some("prod".to_string()),
                    }),
                    regions: vec![],
                }),
            },
        );

        match parse_region_route(&pm) {
            ParsedRegionRoute::Service {
                pm_key,
                cluster_ref,
                entry,
            } => {
                assert_eq!(pm_key, "default/svc-rr");
                let cr = cluster_ref.expect("cluster_ref should be Some");
                assert_eq!(cr.name, "global-rr");
                assert_eq!(cr.namespace.as_deref(), Some("prod"));
                assert_eq!(entry.pm_namespace, "default");
                assert_eq!(entry.pm_name, "svc-rr");
                assert!(entry.ref_plugins.is_empty());
            }
            _ => panic!("expected Service variant"),
        }
    }

    #[test]
    fn parse_other_type_returns_other() {
        let pm = build_plugin_metadata(
            "default",
            "my-keys",
            PluginMetaDataSpec {
                metadata: MetaDataEntry::KeyList(KeyListData {
                    match_mode: KeyMatchMode::Exact,
                    items: vec![KeyGroup {
                        name: "default".to_string(),
                        description: None,
                        items: vec![MetaDataItem {
                            key: "test-key".to_string(),
                            code: None,
                            priority: None,
                            id: None,
                            behavior: None,
                        }],
                    }],
                }),
            },
        );

        assert!(matches!(parse_region_route(&pm), ParsedRegionRoute::Other));
    }
}
