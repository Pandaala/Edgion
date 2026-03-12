use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::common::KeyGet;

/// DynamicExternalUpstream plugin configuration
///
/// Routes the request to an external domain based on a routing key
/// extracted from request metadata and mapped via a preconfigured domainMap.
///
/// Unlike DirectEndpoint (targets endpoint IP) and DynamicInternalUpstream (targets BackendRef),
/// DynamicExternalUpstream targets external domains that may be outside the K8s cluster.
/// Only domains in the domainMap can be targeted (whitelist model).
///
/// Typical scenarios:
/// - Multi-region routing: route traffic to different regional API gateways
/// - CDN origin: direct traffic to different CDN origins based on request attributes
/// - Cross-cluster routing: forward requests to other K8s cluster Ingress/Gateway domains
/// - Third-party API proxy: route to different third-party services by business logic
/// - Blue-green/canary across clusters: switch traffic between cluster domains
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DynamicExternalUpstreamConfig {
    /// Source to extract the routing key from.
    ///
    /// Typical sources:
    /// - Header: `{ type: header, name: "X-Target-Region" }`
    /// - Query:  `{ type: query, name: "region" }`
    /// - Ctx:    `{ type: ctx, name: "computed_target" }`
    /// - Cookie: `{ type: cookie, name: "region_preference" }`
    pub from: KeyGet,

    /// Optional regex to extract the routing key from the raw value.
    ///
    /// If not set, the raw value is used directly as the routing key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<ExtUpstreamExtract>,

    /// Domain mapping: routing key → target domain configuration.
    ///
    /// Only domains explicitly listed here can be jumped to (whitelist model).
    /// The routing key extracted from the request is used as the HashMap key.
    pub domain_map: HashMap<String, DomainTarget>,

    /// Behavior when the routing key is missing from the request.
    ///
    /// - `skip` (default): proceed with normal backend selection (no jump)
    /// - `reject`: return 400 Bad Request
    #[serde(default)]
    pub on_missing: ExtUpstreamOnMissing,

    /// Behavior when the routing key doesn't match any entry in domainMap.
    ///
    /// - `skip` (default): proceed with normal backend selection (no jump)
    /// - `reject`: return 400 Bad Request
    #[serde(default)]
    pub on_no_match: ExtUpstreamOnNoMatch,

    /// Whether to set `X-Dynamic-External-Upstream` request header (sent to upstream)
    /// indicating which domain was targeted. Useful for end-to-end debugging.
    ///
    /// Default: false
    #[serde(default)]
    pub debug_header: bool,

    // === Validation cache ===
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,

    // === Compiled regex cache ===
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<regex::Regex>,
}

/// Target domain configuration for external upstream routing
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DomainTarget {
    /// Target domain name (e.g., "api-us.example.com")
    ///
    /// Must be a valid hostname. IP addresses are also accepted
    /// but using DirectEndpoint is recommended for IP targets.
    pub domain: String,

    /// Target port.
    ///
    /// Default: 443 if tls=true, 80 if tls=false
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// Whether to use TLS for the connection.
    ///
    /// Default: true
    #[serde(default = "default_true")]
    pub tls: bool,

    /// Override the Host header sent to the external domain.
    ///
    /// - If set: the Host header is changed to this value
    /// - If not set: the original request Host header is preserved
    ///
    /// Common patterns:
    /// - Keep original: useful when the external domain is a proxy/CDN
    ///   that routes by the original host
    /// - Set to target domain: useful when the external domain expects
    ///   its own hostname in the Host header
    /// - Set to custom value: for advanced routing scenarios
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_host: Option<String>,

    /// Override the TLS SNI (Server Name Indication).
    ///
    /// - If not set (default): uses the target `domain` as SNI
    ///   (correct behavior for standard TLS connections)
    /// - If set: uses this custom SNI value
    ///
    /// Only relevant when `tls: true`.
    /// Override is rarely needed; the main use case is when the external
    /// server uses a different certificate hostname than the connection domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
}

/// Regex extraction configuration for routing key
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtUpstreamExtract {
    /// Regex pattern to apply to the raw value
    pub regex: String,

    /// Capture group index (0 = full match, 1 = first group, etc.)
    /// Default: 1
    #[serde(default = "default_group")]
    pub group: usize,
}

/// Behavior when routing key value is missing
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum ExtUpstreamOnMissing {
    /// Proceed with normal backend selection (default)
    #[default]
    Skip,
    /// Return 400 Bad Request
    Reject,
}

/// Behavior when no domain matches the routing key
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum ExtUpstreamOnNoMatch {
    /// Proceed with normal backend selection (default)
    #[default]
    Skip,
    /// Return 400 Bad Request
    Reject,
}

fn default_true() -> bool {
    true
}
fn default_group() -> usize {
    1
}

impl DomainTarget {
    /// Get effective port (respecting TLS default)
    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or(if self.tls { 443 } else { 80 })
    }

    /// Get effective SNI (defaults to domain)
    pub fn effective_sni(&self) -> &str {
        self.sni.as_deref().unwrap_or(&self.domain)
    }
}

impl Default for DynamicExternalUpstreamConfig {
    fn default() -> Self {
        Self {
            from: KeyGet::Header {
                name: "X-Target-Region".to_string(),
            },
            extract: None,
            domain_map: HashMap::new(),
            on_missing: ExtUpstreamOnMissing::default(),
            on_no_match: ExtUpstreamOnNoMatch::default(),
            debug_header: false,
            validation_error: None,
            compiled_regex: None,
        }
    }
}

impl DynamicExternalUpstreamConfig {
    /// Validate configuration and pre-compile regex at parse time
    pub fn validate(&mut self) {
        // Validate regex
        if let Some(ref extract) = self.extract {
            match regex::Regex::new(&extract.regex) {
                Ok(re) => self.compiled_regex = Some(re),
                Err(e) => {
                    self.validation_error = Some(format!("Invalid extract regex: {}", e));
                    return;
                }
            }
        }

        // Validate domainMap
        if self.domain_map.is_empty() {
            self.validation_error = Some("domainMap cannot be empty".to_string());
            return;
        }

        for (key, target) in &self.domain_map {
            if target.domain.is_empty() {
                self.validation_error = Some(format!("domainMap['{}']: domain cannot be empty", key));
                return;
            }
            if let Some(port) = target.port {
                if port == 0 {
                    self.validation_error = Some(format!("domainMap['{}']: port cannot be 0", key));
                    return;
                }
            }
            // Validate domain format (basic check, no spaces, no slashes)
            if target.domain.contains(' ') || target.domain.contains('/') {
                self.validation_error = Some(format!("domainMap['{}']: invalid domain '{}'", key, target.domain));
                return;
            }
        }
    }

    /// Return validation error if config is invalid.
    /// Called during preparse for status reporting.
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    /// Look up target domain by routing key
    pub fn lookup_domain(&self, routing_key: &str) -> Option<&DomainTarget> {
        self.domain_map.get(routing_key)
    }
}
