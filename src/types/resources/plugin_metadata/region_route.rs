//! RegionRoute-related PluginMetaData types
//!
//! GlobalRegionRouteConfig — full region topology, route rules, matchMap
//! ServiceRegionRouteConfig — per-service failover overrides with globalRef
//! RouteByKeyConfMatchMeta — key-to-region mapping managed by Center

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::common::KeyGet;
use crate::types::resources::edgion_plugins::plugin_configs::{
    HashCalcConfig, PluginMetaDataRef, RegionDef, RouteRule,
};

/// Global RegionRoute configuration within PluginMetaData.
///
/// Contains the full region topology, route rules, and matchMap.
/// PluginMetaData namespace = cluster-type dimension, name = cluster identifier.
/// This is the authoritative source managed by operators / Center.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalRegionRouteConfig {
    /// Region definitions.
    #[serde(default)]
    pub regions: Vec<RegionDef>,

    /// Name of this gateway instance's own region.
    /// When routed to own region, skips external jump (pass-through).
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
/// Contains per-region failover state. Created by administrator.
/// The `global_ref` links this ServiceRegionRoute PluginMetaData
/// to a GlobalRegionRoute PluginMetaData.
/// Lists all regions (mirroring GlobalRegionRoute's region names);
/// only entries with `failover_to` set have an active failover override.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRegionRouteConfig {
    /// Reference to the GlobalRegionRoute PluginMetaData.
    /// Explicit linkage: namespace/name of the GlobalRegionRoute PluginMetaData
    /// this ServiceRegionRoute PluginMetaData belongs to.
    /// Set by the administrator when creating the ServiceRegionRoute PluginMetaData.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_ref: Option<PluginMetaDataRef>,

    /// Per-region failover state. Mirrors GlobalRegionRoute PluginMetaData's region names.
    /// Only entries with `failover_to` set have an active override.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<ServiceRegionDef>,
}

/// A region entry in ServiceRegionRoute.
/// Mirrors the GlobalRegionRoute PluginMetaData's region names.
/// Only `failover_to` is service-specific; if set it overrides the
/// GlobalRegionRoute PluginMetaData's value.
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
    /// Key-to-region mapping.
    #[serde(default)]
    pub match_map: HashMap<String, String>,
    /// Keys in this list bypass global matchMap override (ServiceRegionRoute PluginMetaData only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bypass_global_keys: Vec<String>,
}
