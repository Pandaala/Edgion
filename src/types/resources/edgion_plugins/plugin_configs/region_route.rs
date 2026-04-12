//! RegionRoute plugin configuration
//!
//! Routes traffic across multiple regions using ordered route rules.
//! Supports direct region override, key-to-region conf match, consistent hash-range routing,
//! pre-computed hash passthrough, and auto failover.
//!
//! `base_info` groups management metadata (service_group, cluster) with region definitions
//! so that Center can aggregate region-route entries across controllers by
//! `{service_group}/{cluster}/{namespace}`.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::common::{KeyGet, KeySet};

/// RegionRoute plugin: multi-region traffic routing with ordered route rules.
///
/// Traffic routing follows `route_rules` in order — first matching rule wins.
/// All rules miss → pass through.
///
/// When `route_rules` is empty, the plugin is a no-op (passes through).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteConfig {
    /// Base information: service group, cluster, region definitions, and local region.
    ///
    /// Groups management metadata with the region topology so that Center can
    /// aggregate region-route entries across controllers by `{service_group}/{cluster}/{namespace}`.
    #[serde(default)]
    pub base_info: RegionRouteBaseInfo,

    /// Shared key extraction for RouteByKeyConfMatch and RouteByHashRange.
    /// Tried in order — first non-empty value wins.
    #[serde(default)]
    pub key_get: Vec<KeyGet>,

    /// Pre-computed hash key sources for RouteByHashRange.
    /// Tried before computing — if a valid integer in [0, modulo) is found, it is used directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_key_get: Option<Vec<KeyGet>>,

    /// Hash computation config.
    ///
    /// If absent, RouteByHashRange skips hash computation and acts as passthrough-only
    /// (still reads from hashKeyGet if configured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_calc: Option<HashCalcConfig>,

    /// Ordered route rules — first match wins, all miss → pass through.
    ///
    /// Empty list = plugin is a no-op (always passes through).
    #[serde(default)]
    pub route_rules: Vec<RouteRule>,

    /// Optional: auto failover configuration.
    ///
    /// When enabled, a background task probes each region's DNS health.
    /// If the target region is unhealthy, traffic is redirected to the healthiest other region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_failover: Option<AutoFailoverConfig>,

    // === Validation cache (runtime only) ===
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

/// Base information for grouping and management.
///
/// Contains the service group name, cluster identifier, region definitions,
/// and the local region name. These fields enable Center to aggregate
/// region-route configurations across multiple controllers.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteBaseInfo {
    /// Service group name for grouping related RegionRoute plugins.
    ///
    /// Multiple plugins with the same `service_group` form a logical group
    /// managed together. Must be a valid K8s DNS-1123 label
    /// (lowercase alphanumeric and hyphens, max 63 chars, must start/end with alphanumeric).
    pub service_group: String,

    /// Cluster identifier (e.g., "us-prod", "eu-staging").
    ///
    /// Used together with `service_group` and the resource namespace to form
    /// the composite management key `{service_group}/{cluster}/{namespace}`.
    #[serde(default)]
    pub cluster: String,

    /// Region definitions: name, hash range [low, high], and backend endpoint.
    /// Ranges must not overlap. Gaps are allowed (requests in gap pass through).
    pub regions: Vec<RegionDef>,

    /// Optional: name of this gateway instance's own region.
    /// Must match a region name in `regions`. When routing determines this region
    /// as the target, the plugin passes through without ExternalJumpPreset
    /// (traffic is already in the right region — no cross-region jump needed).
    /// Empty string = feature disabled.
    #[serde(default)]
    pub my_region: String,
}

/// Region definition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionDef {
    /// Region identifier used in RouteByDirectRegion and failover notify_header.
    pub name: String,

    /// Hash range [low, high], closed interval. Must satisfy: 0 ≤ low ≤ high < modulo.
    /// Requests whose computed hash falls within [low, high] are routed to this region.
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

    /// Static failover target: if set, traffic routed to this region is redirected
    /// to the named region before the health check.
    /// Must be a valid region name in the same regions list. Cannot be self.
    /// No chaining — only one level of redirect is followed at runtime.
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

/// Auto failover configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AutoFailoverConfig {
    /// Enable auto failover. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Failover is triggered when the DNS probe returns no IPs (ratio = 0.0).
    /// Threshold is compared against the probe result: 1.0 = IPs found, 0.0 = DNS empty/failed.
    /// Default: 0.5 (any DNS failure triggers failover).
    #[serde(default = "default_half")]
    pub healthy_threshold: f32,

    /// Background DNS probe interval in milliseconds. Default: 5000.
    #[serde(default = "default_5000")]
    pub probe_interval_ms: u64,

    /// Optional response header to write the failover region name to (for debugging).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_header: Option<String>,

    /// Behavior when all regions are unhealthy (DNS probe returns nothing).
    #[serde(default)]
    pub on_all_regions_down: OnAllDown,
}

/// Behavior when every region fails its health probe
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnAllDown {
    /// Pass through (fail-open): let the request proceed; the underlying connection may fail.
    #[default]
    Pass,
    /// Return HTTP 503 Service Unavailable immediately.
    Reject,
}

// ── Default helpers ──────────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}
fn default_half() -> f32 {
    0.5
}
fn default_5000() -> u64 {
    5000
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

// ── RegionRouteConfig validation ─────────────────────────────────────────────

/// Check whether `s` is a valid K8s DNS-1123 label:
/// - 1..=63 chars
/// - lowercase alphanumeric and hyphens only
/// - must start and end with an alphanumeric character
fn is_dns1123_label(s: &str) -> bool {
    if s.is_empty() || s.len() > 63 {
        return false;
    }
    let bytes = s.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return false;
    }
    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl RegionRouteConfig {
    /// Validate config and cache any error message. Called in preparse.
    pub fn validate(&mut self) {
        // Validate baseInfo.serviceGroup (required, DNS-1123 label)
        if self.base_info.service_group.is_empty() {
            self.validation_error = Some("baseInfo.serviceGroup must not be empty".to_string());
            return;
        }
        if !is_dns1123_label(&self.base_info.service_group) {
            self.validation_error = Some(format!(
                "baseInfo.serviceGroup '{}' is not a valid DNS-1123 label",
                self.base_info.service_group
            ));
            return;
        }

        if self.base_info.regions.is_empty() {
            self.validation_error = Some("baseInfo.regions must not be empty".to_string());
            return;
        }

        // Determine effective modulo (for hashRange validation)
        let modulo = self.hash_calc.as_ref().map(|c| c.modulo).unwrap_or(1000);

        // Validate hashCalc.modulo (modulo already computed above)
        if modulo < 2 {
            self.validation_error = Some("hashCalc.modulo must be >= 2".to_string());
            return;
        }

        for region in &self.base_info.regions {
            if region.name.is_empty() {
                self.validation_error = Some("region name must not be empty".to_string());
                return;
            }
            if region.hash_range[0] > region.hash_range[1] {
                self.validation_error = Some(format!(
                    "region '{}': hashRange[0]={} > hashRange[1]={}",
                    region.name, region.hash_range[0], region.hash_range[1]
                ));
                return;
            }
            if region.hash_range[1] >= modulo {
                self.validation_error = Some(format!(
                    "region '{}': hashRange[1]={} must be < modulo={}",
                    region.name, region.hash_range[1], modulo
                ));
                return;
            }
            let (host, _) = region.parse_endpoint();
            if host.is_empty() {
                self.validation_error = Some(format!("region '{}': backend_endpoint host is empty", region.name));
                return;
            }
        }

        // Note: myRegion and failoverTo reference validity is NOT validated here as hard errors.
        // Invalid references are silently degraded at runtime (my_region_idx = None,
        // region_redirect[i] = Invalid) so the plugin continues to work for all other traffic.
        // Invalid failoverTo is surfaced per-request via pluginLog: "err:failoverTo invalid; ".

        // Check for overlapping ranges (sort by low, check adjacent)
        let mut sorted: Vec<(u16, u16, &str)> = self
            .base_info
            .regions
            .iter()
            .map(|r| (r.hash_range[0], r.hash_range[1], r.name.as_str()))
            .collect();
        sorted.sort_by_key(|(lo, _, _)| *lo);
        for window in sorted.windows(2) {
            let (_, prev_hi, prev_name) = window[0];
            let (cur_lo, _, cur_name) = window[1];
            if cur_lo <= prev_hi {
                self.validation_error = Some(format!(
                    "hashRange overlap between '{}' (..={}) and '{}' ({}..)",
                    prev_name, prev_hi, cur_name, cur_lo
                ));
                return;
            }
        }

        // Validate route rules
        for (i, rule) in self.route_rules.iter().enumerate() {
            match rule {
                RouteRule::RouteByDirectRegion { key_from } => {
                    if key_from.is_empty() {
                        self.validation_error = Some(format!(
                            "routeRules[{}] RouteByDirectRegion: keyFrom must not be empty",
                            i
                        ));
                        return;
                    }
                }
                RouteRule::RouteByKeyConfMatch { match_map } => {
                    if match_map.is_empty() {
                        self.validation_error = Some(format!(
                            "routeRules[{}] RouteByKeyConfMatch: matchMap must not be empty",
                            i
                        ));
                        return;
                    }
                }
                RouteRule::RouteByHashRange => {
                    // No per-rule config to validate
                }
            }
        }

        if let Some(ref f) = self.auto_failover {
            if !(0.0..=1.0).contains(&f.healthy_threshold) {
                self.validation_error = Some("auto_failover.healthy_threshold must be in [0.0, 1.0]".to_string());
            }
        }
    }

    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }
}
