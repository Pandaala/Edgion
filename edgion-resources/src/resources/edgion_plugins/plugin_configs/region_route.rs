//! RegionRoute plugin configuration
//!
//! Routes traffic across multiple regions using ordered route rules.
//! Supports direct region override, key-to-region conf match, consistent hash-range routing,
//! and pre-computed hash passthrough.
//!
//! `RegionRouteConfig` in EdgionPlugins holds ALL routing logic plus the default region table.
//! An optional `override_ref` points to a `RegionRouteOverride` EdgionConfigData which can
//! whole-replace the `regions` at request time (fail-to-base on any overlay error).

use crate::gwapi_types::is_valid_header_token;
use crate::gwapi_types::HEADER_NAME_MAX_LEN;
use crate::response_header_policy::validate_response_header_mutation;
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{KeyGet, KeySet};
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_config_data::RegionTableValidation;

/// RegionRoute plugin: flat base configuration.
///
/// Holds ALL routing logic and the default region table.
/// An optional `override_ref` points to a `RegionRouteOverride` EdgionConfigData that can
/// whole-replace the `regions` at request time (fail-to-base on any overlay error).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteConfig {
    /// Name of this gateway instance's own region.
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

    /// RouteByKeyConfMatch configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_by_key_conf_match: Option<RouteByKeyConfMatchMeta>,

    /// Default region table (safe default). Applied directly when no overlay is set.
    #[serde(default)]
    pub regions: Vec<RegionDef>,

    /// Optional overlay reference. When set, the gateway looks up this EdgionConfigData at
    /// request time and applies a `RegionRouteOverride` on top of the base `regions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_ref: Option<EdgionConfigDataRef>,
}

/// RouteByKeyConfMatch configuration with bypass support.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteByKeyConfMatchMeta {
    /// Key-to-region mapping.
    #[serde(default)]
    pub match_map: HashMap<String, String>,
    /// Keys in this list bypass the region matchMap override.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bypass_cluster_keys: Vec<String>,
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

    /// Failover target: if set, traffic routed to this region is redirected
    /// to the named region.
    /// Must be a valid region name in the same regions list. Cannot be self.
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
    /// Must be at least 2. Valid hash values are in the range [0, modulo-1].
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
    /// Accepts DNS names, IPv4, and bracketed IPv6, with an optional port in
    /// 1..=65535. IPv6 brackets are removed for the canonical DNS/IP resolver.
    /// DNS spelling (including a terminal root dot) is preserved. No DNS or
    /// address-safety decision is made here; dial-time checks remain mandatory.
    pub fn parse_endpoint(&self) -> Result<(&str, u16), &'static str> {
        let default_port = if self.tls { 443 } else { 80 };
        let endpoint = self.backend_endpoint.as_str();
        let (host, port) = if let Some(rest) = endpoint.strip_prefix('[') {
            let (host, suffix) = rest.split_once(']').ok_or("IPv6 endpoint needs closing bracket")?;
            host.parse::<Ipv6Addr>().map_err(|_| "invalid IPv6 endpoint")?;
            let port = if suffix.is_empty() {
                None
            } else {
                Some(suffix.strip_prefix(':').ok_or("unexpected text after IPv6 bracket")?)
            };
            (host, port)
        } else {
            let (host, port) = endpoint.split_once(':').map_or((endpoint, None), |(h, p)| (h, Some(p)));
            let dns_name = host.strip_suffix('.').unwrap_or(host);
            if dns_name.is_empty()
                || dns_name.len() > 253
                || !dns_name.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label.as_bytes()[0].is_ascii_alphanumeric()
                        && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                        && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            {
                return Err("endpoint must contain a DNS name, IPv4, or bracketed IPv6");
            }
            // Numeric dotted addresses must be canonical IPv4, not ambiguous
            // abbreviated, octal, or out-of-range DNS-like spellings.
            if dns_name.bytes().all(|b| b.is_ascii_digit() || b == b'.') && dns_name.parse::<Ipv4Addr>().is_err() {
                return Err("invalid IPv4 endpoint");
            }
            (host, port)
        };
        let port = match port {
            None => default_port,
            Some(raw) => {
                if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("endpoint port must be an integer in 1..=65535");
                }
                raw.parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or("endpoint port must be an integer in 1..=65535")?
            }
        };
        Ok((host, port))
    }
}

impl RegionRouteConfig {
    /// Hash domain used by both computed and precomputed hash routing.
    pub fn effective_modulo(&self) -> u16 {
        self.hash_calc.as_ref().map_or(default_modulo(), |calc| calc.modulo)
    }

    /// Check a non-empty candidate table against this consumer's hash domain.
    pub fn validate_effective_regions(&self, regions: &[RegionDef]) -> Result<(), String> {
        RegionTableValidation::new(regions)?.validate_modulo(self.effective_modulo())
    }

    /// Validate config. Called in preparse.
    pub fn validate(&mut self) {
        // Validation is read-only and exposed through `PluginValidation`.
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for RegionRouteConfig {
    fn validation_error(&self) -> Option<String> {
        for target in self.hash_calc.iter().flat_map(|calc| &calc.write_back) {
            if let KeySet::ResponseHeader { name } = target {
                if name.len() > HEADER_NAME_MAX_LEN as usize || !is_valid_header_token(name) {
                    return Some(
                        "hashCalc.writeBack response header names must be HTTP tokens of at most 256 bytes".to_string(),
                    );
                }
                if let Err(error) = validate_response_header_mutation(name) {
                    return Some(error.to_string());
                }
            }
        }
        if self.hash_calc.as_ref().is_some_and(|calc| calc.modulo < 2) {
            return Some("hashCalc.modulo must be at least 2".to_string());
        }

        if self.regions.is_empty() && self.route_rules.is_empty() {
            return None;
        }
        self.validate_effective_regions(&self.regions).err()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/region_route_test/validation_trait_tests.rs"]
mod validation_trait_tests;
