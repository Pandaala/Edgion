//! RegionRoute-related PluginMetaData types
//!
//! ClusterRegionRouteConfig — full cluster topology, route rules, matchMap
//! ServiceRegionRouteConfig — per-service failover overrides with clusterRef
//! RouteByKeyConfMatchMeta — key-to-cluster mapping managed by Center

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::common::KeyGet;
use crate::types::resources::edgion_plugins::plugin_configs::{
    RegionDef, HashCalcConfig, PluginMetaDataRef, RouteRule,
};

/// Cluster RegionRoute configuration within PluginMetaData.
///
/// Contains the full cluster topology, route rules, and matchMap.
/// PluginMetaData namespace = cluster-type dimension, name = cluster identifier.
/// This is the authoritative source managed by operators / Center.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRegionRouteConfig {
    /// Cluster definitions.
    #[serde(default)]
    pub regions: Vec<RegionDef>,

    /// Name of this gateway instance's own cluster.
    /// When routed to own cluster, skips external jump (pass-through).
    /// Empty = feature disabled.
    #[serde(default)]
    pub my_region: String,

    /// Shared key extraction for RouteByKeyConfMatch and RouteByHashRange.
    #[serde(default)]
    pub key_get: Vec<KeyGet>,

    /// Pre-computed hash key sources (tried before computing hash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_key_get: Option<Vec<KeyGet>>,

    /// Hash computation config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_calc: Option<HashCalcConfig>,

    /// Ordered route rules — first match wins.
    #[serde(default)]
    pub route_rules: Vec<RouteRule>,

    /// RouteByKeyConfMatch configuration managed by Center.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_by_key_conf_match: Option<RouteByKeyConfMatchMeta>,
}

/// Service-level RegionRoute configuration within PluginMetaData.
///
/// Contains per-cluster failover state. Created by administrator.
/// The `cluster_ref` links this ServiceRegionRoute PluginMetaData
/// to a ClusterRegionRoute PluginMetaData.
/// Lists all clusters (mirroring ClusterRegionRoute's cluster names);
/// only entries with `failover_to` set have an active failover override.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRegionRouteConfig {
    /// Reference to the ClusterRegionRoute PluginMetaData.
    /// Explicit linkage: namespace/name of the ClusterRegionRoute PluginMetaData
    /// this ServiceRegionRoute PluginMetaData belongs to.
    /// Set by the administrator when creating the ServiceRegionRoute PluginMetaData.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_ref: Option<PluginMetaDataRef>,

    /// Per-cluster failover state. Mirrors ClusterRegionRoute PluginMetaData's cluster names.
    /// Only entries with `failover_to` set have an active override.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<ServiceRegionDef>,
}

/// A cluster entry in ServiceRegionRoute.
/// Mirrors the ClusterRegionRoute PluginMetaData's cluster names.
/// Only `failover_to` is service-specific; if set it overrides the
/// ClusterRegionRoute PluginMetaData's value.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRegionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover_to: Option<String>,
}

/// RouteByKeyConfMatch configuration with bypass support.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteByKeyConfMatchMeta {
    /// Key-to-cluster mapping.
    #[serde(default)]
    pub match_map: HashMap<String, String>,
    /// Keys in this list bypass cluster matchMap override (ServiceRegionRoute PluginMetaData only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bypass_cluster_keys: Vec<String>,
}
