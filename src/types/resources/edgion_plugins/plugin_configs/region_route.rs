//! RegionRoute plugin configuration
//!
//! Routes traffic across multiple regions using consistent hash-range assignment.
//! Supports direct region override, pre-computed hash passthrough, and auto failover.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::common::KeyGet;

/// RegionRoute plugin: multi-region traffic routing with consistent hash-range.
///
/// Traffic routing priority (highest to lowest):
/// 1. `direct_region` — explicit region override from request (optional)
/// 2. `region_key_hash` — pre-computed 0-1000 integer passthrough
/// 3. `region_key` — compute SHA-256 hash of extracted key, map to region range
///
/// When `region_key` is empty, the plugin is a no-op (passes through).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteConfig {
    /// Region definitions: name, hash range [0-1000], and backend endpoint.
    /// Ranges must not overlap. Gaps are allowed (requests in gap pass through).
    pub regions: Vec<RegionDef>,

    /// Optional: directly override the target region.
    ///
    /// Extracts a region name (e.g. "virginia") from the request using KeyGet sources
    /// in order. The first non-empty value that matches a region name is used.
    /// If the matched region is unhealthy, falls through to auto_failover.
    /// If the value is not a valid region name, falls through to hash routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_region: Option<MultiKeyFrom>,

    /// Optional: pre-computed region hash passthrough (0-1000 integer).
    ///
    /// If a valid 0-1000 integer is found in any of the configured sources,
    /// it is used directly to look up the region range table, skipping hash computation.
    /// Useful when a backend has already computed and stored the hash (e.g. in a cookie).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_key_hash: Option<MultiKeyFrom>,

    /// Keys for hash computation, tried in order — first non-empty value wins.
    ///
    /// The selected value is hashed (SHA-256, mod 1001) to produce a 0-1000 integer
    /// which is then mapped to a region via hash_range lookup.
    ///
    /// Empty list = plugin is a no-op (passes through without changing upstream).
    /// Missing value at runtime = pass through.
    #[serde(default)]
    pub region_key: Vec<KeyGet>,

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

/// Region definition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionDef {
    /// Region identifier used in direct_region and failover notify_header.
    pub name: String,

    /// Hash range [low, high], closed interval. Must satisfy: 0 ≤ low ≤ high ≤ 1000.
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
}

/// Multiple key sources: returns the first non-empty value found
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MultiKeyFrom {
    pub key_from: Vec<KeyGet>,
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

impl RegionRouteConfig {
    /// Validate config and cache any error message. Called in preparse.
    pub fn validate(&mut self) {
        if self.regions.is_empty() {
            self.validation_error = Some("regions must not be empty".to_string());
            return;
        }

        for region in &self.regions {
            if region.name.is_empty() {
                self.validation_error = Some("region name must not be empty".to_string());
                return;
            }
            if region.hash_range[0] > region.hash_range[1] {
                self.validation_error = Some(format!(
                    "region '{}': hash_range[0]={} > hash_range[1]={}",
                    region.name, region.hash_range[0], region.hash_range[1]
                ));
                return;
            }
            if region.hash_range[1] > 1000 {
                self.validation_error = Some(format!(
                    "region '{}': hash_range[1]={} exceeds 1000",
                    region.name, region.hash_range[1]
                ));
                return;
            }
            let (host, _) = region.parse_endpoint();
            if host.is_empty() {
                self.validation_error =
                    Some(format!("region '{}': backend_endpoint host is empty", region.name));
                return;
            }
        }

        // Check for overlapping ranges (sort by low, check adjacent)
        let mut sorted: Vec<(u16, u16, &str)> = self
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
                    "hash_range overlap between '{}' (..={}) and '{}' ({}..)",
                    prev_name, prev_hi, cur_name, cur_lo
                ));
                return;
            }
        }

        if let Some(ref f) = self.auto_failover {
            if !(0.0..=1.0).contains(&f.healthy_threshold) {
                self.validation_error =
                    Some("auto_failover.healthy_threshold must be in [0.0, 1.0]".to_string());
                return;
            }
        }
    }

    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }
}
