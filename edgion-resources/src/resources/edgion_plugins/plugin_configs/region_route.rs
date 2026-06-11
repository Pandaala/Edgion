//! RegionRoute plugin configuration
//!
//! Routes traffic across multiple regions using ordered route rules.
//! Supports direct region override, key-to-region conf match, consistent hash-range routing,
//! and pre-computed hash passthrough.
//!
//! EP only holds a `serviceRegionRouteRef` — all routing config lives in
//! PluginMetaData (ClusterRegionRoute / ServiceRegionRoute).

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{KeyGet, KeySet};

/// RegionRoute plugin: multi-region traffic routing.
///
/// EP only holds `serviceRegionRouteRef`. All routing config lives in
/// PluginMetaData (ClusterRegionRoute / ServiceRegionRoute).
///
/// Gateway resolves the PluginMetaData chain at plugin-build time via the global
/// `PluginMetaDataGatewayStore`:
///   `EP.serviceRegionRouteRef → ServiceRegionRoute PluginMetaData
///     → ServiceRegionRoute PluginMetaData.clusterRef → ClusterRegionRoute PluginMetaData`
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteConfig {
    /// Reference to a ServiceRegionRoute PluginMetaData by name.
    /// Namespace is inherited from the parent EdgionPlugins.
    /// The ServiceRegionRoute PluginMetaData must be pre-created by the administrator;
    /// it in turn references the ClusterRegionRoute PluginMetaData via its clusterRef.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_region_route_ref: Option<String>,
}

/// Cluster definition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionDef {
    /// Cluster identifier used in RouteByDirectRegion and failover notify_header.
    pub name: String,

    /// Hash range [low, high], closed interval. Must satisfy: 0 ≤ low ≤ high < modulo.
    /// Requests whose computed hash falls within [low, high] are routed to this cluster.
    pub hash_range: [u16; 2],

    /// Target backend endpoint: "host" or "host:port".
    ///
    /// Only the upstream IP changes (via DNS resolution of this host).
    /// The original request's SNI (from Host header) and all request headers are preserved.
    /// Port defaults to 443 when tls=true, 80 when tls=false.
    pub backend_endpoint: String,

    /// Whether the upstream connection uses TLS. Default: true.
    #[serde(default = "default_true")]
    pub tls: bool,

    /// Failover target: if set, traffic routed to this cluster is redirected
    /// to the named cluster. Set by Center (ClusterRegionRoute PluginMetaData) or
    /// overridden by ServiceRegionRoute PluginMetaData.
    /// Must be a valid cluster name in the same clusters list. Cannot be self.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover_to: Option<String>,
}

/// Hash computation configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HashCalcConfig {
    /// Hash algorithm. Currently only SHA-256 is supported.
    #[serde(default)]
    pub algorithm: HashAlgorithm,

    /// Hash value domain [0, modulo-1]. Default: 1000 (slots 0..=999).
    ///
    /// Valid hash values are in the range [0, modulo-1].
    /// All region hashRange values must satisfy hashRange[1] < modulo.
    #[serde(default = "default_modulo")]
    pub modulo: u16,

    /// Write computed hash to these targets (empty = no write-back).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write_back: Vec<KeySet>,
}

/// Hash algorithm for key hashing
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum HashAlgorithm {
    /// CRC32: crc32(key) % modulo. Fast, hardware-accelerated on x86/ARM.
    #[default]
    Crc32,
    /// SHA-256: sha256(key)[0:8] as big-endian u64, mod modulo.
    Sha256,
}

/// A route rule — evaluated in order, first match wins
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum RouteRule {
    /// Route by extracting a region name directly from the request.
    ///
    /// Extracts values from `key_from` in order; the first non-empty value that matches
    /// a region name is used. If the value does not match any region, falls through to
    /// the next rule.
    RouteByDirectRegion {
        /// Key sources to extract the region name from.
        #[serde(rename = "keyFrom")]
        key_from: Vec<KeyGet>,
    },

    /// Route by matching the extracted key against a fixed map of key → region name.
    ///
    /// Uses the shared `keyGet` field to extract the key. If the extracted value is found
    /// in `match_map`, routes to the mapped region. Otherwise falls through to the next rule.
    RouteByKeyConfMatch {
        /// Map of key value → region name.
        #[serde(rename = "matchMap")]
        match_map: HashMap<String, String>,
    },

    /// Route by consistent hash range.
    ///
    /// First tries to read a pre-computed hash from `hashKeyGet`. If valid (in [0, modulo)),
    /// uses it directly. Otherwise computes the hash via `hashCalc`.
    ///
    /// This rule is **terminal**: if neither passthrough nor computation yields a result
    /// (e.g. no hashCalc configured and no hashKeyGet value), returns GoodNext immediately
    /// without evaluating further rules.
    RouteByHashRange,
}

/// Reference to a PluginMetaData resource by name (and optional namespace).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginMetaDataRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

// ── Default helpers ──────────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}
fn default_modulo() -> u16 {
    1000
}

// ── RegionDef helpers ────────────────────────────────────────────────────────

impl RegionDef {
    /// Parse `backend_endpoint` into `(host, port)`.
    ///
    /// Handles plain "host", "host:port". IPv6 addresses in brackets (e.g. "[::1]:8080")
    /// are not a typical use case here but handled gracefully.
    pub fn parse_endpoint(&self) -> (&str, u16) {
        let default_port = if self.tls { 443 } else { 80 };
        if let Some(pos) = self.backend_endpoint.rfind(':') {
            let host = &self.backend_endpoint[..pos];
            if let Ok(port) = self.backend_endpoint[pos + 1..].parse::<u16>() {
                return (host, port);
            }
        }
        (&self.backend_endpoint, default_port)
    }
}

impl RegionRouteConfig {
    /// Validate config. Called in preparse.
    ///
    /// This is a no-op for RegionRoute: the EP holds only `serviceRegionRouteRef` and
    /// all routing config lives in PluginMetaData. PluginMetaData existence is checked
    /// by the Controller's `preparse()` (reported via status conditions) and by the
    /// Gateway at plugin-build time via `resolve_config()`.
    pub fn validate(&mut self) {
        // No local validation needed — PluginMetaData chain resolution handles all checks.
    }

    /// Stateless field-level validation.
    /// Returns None — region_route validation is handled by
    /// PluginMetaData chain resolution at preparse time.
    fn check_fields(&self) -> Option<String> {
        None
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for RegionRouteConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod validation_trait_tests {
    use super::*;
    use crate::resources::edgion_plugins::PluginValidation;

    #[test]
    fn validation_error_returns_none_for_default_config() {
        // region_route relies on PluginMetaData chain resolution for all checks;
        // the trait method must return None and must not panic.
        let config = RegionRouteConfig::default();
        assert_eq!(config.validation_error(), None);
    }
}
