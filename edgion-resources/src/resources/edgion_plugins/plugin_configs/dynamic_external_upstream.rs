use std::collections::HashMap;

use http::uri::Authority;
use http::HeaderValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::KeyGet;
use crate::resources::edgion_plugins::PluginValidation;

use super::upstream_fallback::{default_upstream_fallback, UpstreamFallback};

const EMPTY_SNI: &str = "sni cannot be empty";
const INVALID_SNI: &str = "sni contains invalid characters";
const EMPTY_OVERRIDE_HOST: &str = "overrideHost cannot be empty";
const OVERRIDE_HOST_HEADER: &str = "overrideHost is not a valid HTTP header value";
const OVERRIDE_HOST_AUTHORITY: &str = "overrideHost is not a valid Host authority";

/// Shared domain/SNI character predicate. Internal names and IP forms that
/// already pass `domain` validation must also pass an explicit `sni`.
fn is_allowed_external_name_char(c: char) -> bool {
    !c.is_control() && !matches!(c, ' ' | '/' | '\\' | '@' | '#' | '?' | '[' | ']')
}

fn is_valid_host_authority(value: &str) -> bool {
    // HTTP Host is `uri-host [ ":" port ]`. URI Authority also allows userinfo.
    if value.is_empty() || value.contains('@') {
        return false;
    }
    let Ok(authority) = Authority::try_from(value) else {
        return false;
    };
    let host = authority.host();
    if host.is_empty() {
        return false;
    }
    if value == host {
        return true;
    }
    let Some(port) = value.get(host.len()..).and_then(|rest| rest.strip_prefix(':')) else {
        return false;
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    port.parse::<i64>().ok().is_some_and(crate::gwapi_types::is_valid_port)
}

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
    /// - `fallback` (default): proceed with normal backend selection (no jump)
    /// - `reject`: return 400 Bad Request
    #[serde(default = "default_upstream_fallback")]
    pub on_missing: UpstreamFallback,

    /// Behavior when the routing key doesn't match any entry in domainMap.
    ///
    /// - `fallback` (default): proceed with normal backend selection (no jump)
    /// - `reject`: return 400 Bad Request
    #[serde(default = "default_upstream_fallback")]
    pub on_no_match: UpstreamFallback,

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
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
    pub port: Option<u16>,

    /// Whether to use TLS for the connection.
    ///
    /// Default: true
    #[serde(default = "default_true")]
    pub tls: bool,

    /// Override the Host header sent to the external domain.
    ///
    /// - If set: must be nonempty, a valid HTTP header value, and a valid
    ///   Host authority; the Host header is changed to this value
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
    /// - If set: must be nonempty and pass the same character rules as
    ///   `domain`; uses this custom SNI value
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
            on_missing: default_upstream_fallback(),
            on_no_match: default_upstream_fallback(),
            compiled_regex: None,
        }
    }
}

impl DynamicExternalUpstreamConfig {
    /// Stateless field validation: mirrors every error branch in validate() without mutating self.
    fn check_fields(&self) -> Option<String> {
        // Validate regex
        if let Some(ref extract) = self.extract {
            if let Err(e) = regex::Regex::new(&extract.regex) {
                return Some(format!("Invalid extract regex: {}", e));
            }
        }

        // Validate domainMap
        if self.domain_map.is_empty() {
            return Some("domainMap cannot be empty".to_string());
        }

        for (key, target) in &self.domain_map {
            if target.domain.is_empty() {
                return Some(format!("domainMap['{}']: domain cannot be empty", key));
            }
            if let Some(port) = target.port {
                if port == 0 {
                    return Some(format!("domainMap['{}']: port cannot be 0", key));
                }
            }
            // Validate domain: reject control chars, null bytes, and chars invalid in DNS/SNI/Host
            if let Some(c) = target.domain.chars().find(|c| !is_allowed_external_name_char(*c)) {
                return Some(format!(
                    "domainMap['{}']: domain contains invalid character {:?}",
                    key, c
                ));
            }
            if let Some(sni) = target.sni.as_deref() {
                if sni.is_empty() {
                    return Some(EMPTY_SNI.to_string());
                }
                if !sni.chars().all(is_allowed_external_name_char) {
                    return Some(INVALID_SNI.to_string());
                }
            }
            if let Some(host) = target.override_host.as_deref() {
                if host.is_empty() {
                    return Some(EMPTY_OVERRIDE_HOST.to_string());
                }
                if HeaderValue::try_from(host).is_err() {
                    return Some(OVERRIDE_HOST_HEADER.to_string());
                }
                if !is_valid_host_authority(host) {
                    return Some(OVERRIDE_HOST_AUTHORITY.to_string());
                }
            }
        }

        None
    }

    /// Pre-compile regex at parse time when fields are valid.
    /// Field-level errors are surfaced via the `PluginValidation` trait.
    pub fn validate(&mut self) {
        if self.check_fields().is_some() {
            return;
        }
        if let Some(ref extract) = self.extract {
            if let Ok(re) = regex::Regex::new(&extract.regex) {
                self.compiled_regex = Some(re);
            }
        }
    }

    /// Look up target domain by routing key
    pub fn lookup_domain(&self, routing_key: &str) -> Option<&DomainTarget> {
        self.domain_map.get(routing_key)
    }
}

impl PluginValidation for DynamicExternalUpstreamConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod validation_trait_tests {
    use super::*;
    use crate::resources::edgion_plugins::PluginValidation;

    #[test]
    fn validation_error_returns_field_error_without_validate_call() {
        // Default config has an empty domain_map, which is the first field error
        // (after regex check passes since extract is None).
        let config = DynamicExternalUpstreamConfig::default();
        let result = config.validation_error();
        assert_eq!(result, Some("domainMap cannot be empty".to_string()));
    }

    #[test]
    fn validation_error_detects_invalid_regex() {
        let config = DynamicExternalUpstreamConfig {
            extract: Some(ExtUpstreamExtract {
                regex: "[invalid".to_string(),
                group: 1,
            }),
            ..Default::default()
        };
        let result = config.validation_error();
        assert!(
            result.as_deref().unwrap_or("").starts_with("Invalid extract regex:"),
            "expected regex error, got: {:?}",
            result
        );
    }

    #[test]
    fn validation_error_none_for_valid_config() {
        let mut config = DynamicExternalUpstreamConfig::default();
        config.domain_map.insert(
            "us".to_string(),
            DomainTarget {
                domain: "api-us.example.com".to_string(),
                port: Some(443),
                tls: true,
                override_host: None,
                sni: None,
            },
        );
        assert_eq!(config.validation_error(), None);
    }

    #[test]
    fn validation_error_detects_empty_domain() {
        let mut config = DynamicExternalUpstreamConfig::default();
        config.domain_map.insert(
            "us".to_string(),
            DomainTarget {
                domain: "".to_string(),
                port: None,
                tls: true,
                override_host: None,
                sni: None,
            },
        );
        let result = config.validation_error();
        assert_eq!(result, Some("domainMap['us']: domain cannot be empty".to_string()));
    }

    #[test]
    fn validation_error_detects_zero_port() {
        let mut config = DynamicExternalUpstreamConfig::default();
        config.domain_map.insert(
            "eu".to_string(),
            DomainTarget {
                domain: "api.example.com".to_string(),
                port: Some(0),
                tls: true,
                override_host: None,
                sni: None,
            },
        );
        let result = config.validation_error();
        assert_eq!(result, Some("domainMap['eu']: port cannot be 0".to_string()));
    }

    #[test]
    fn validation_error_detects_invalid_domain_char() {
        let mut config = DynamicExternalUpstreamConfig::default();
        config.domain_map.insert(
            "bad".to_string(),
            DomainTarget {
                domain: "api example.com".to_string(), // space is invalid
                port: None,
                tls: true,
                override_host: None,
                sni: None,
            },
        );
        let result = config.validation_error();
        assert_eq!(
            result,
            Some("domainMap['bad']: domain contains invalid character ' '".to_string())
        );
    }

    fn target_with(domain: &str) -> DomainTarget {
        DomainTarget {
            domain: domain.to_string(),
            port: None,
            tls: true,
            override_host: None,
            sni: None,
        }
    }

    fn config_with(target: DomainTarget) -> DynamicExternalUpstreamConfig {
        let mut config = DynamicExternalUpstreamConfig::default();
        config.domain_map.insert("us".to_string(), target);
        config
    }

    #[test]
    fn validation_error_none_when_sni_and_override_host_are_omitted() {
        assert_eq!(config_with(target_with("api-us.example.com")).validation_error(), None);
    }

    #[test]
    fn validation_error_none_for_explicit_valid_sni_and_supported_name_forms() {
        for sni in ["api-b.example.com", "localhost", "127.0.0.1", "us-west.internal"] {
            let mut target = target_with("api-us.example.com");
            target.sni = Some(sni.to_string());
            assert_eq!(
                config_with(target).validation_error(),
                None,
                "supported sni form must remain valid: {sni}"
            );
        }
    }

    #[test]
    fn validation_error_rejects_explicit_empty_sni() {
        let mut target = target_with("api-us.example.com");
        target.sni = Some(String::new());
        assert_eq!(config_with(target).validation_error(), Some(EMPTY_SNI.to_string()));
    }

    #[test]
    fn validation_error_rejects_sni_with_invalid_characters() {
        let mut target = target_with("api-us.example.com");
        target.sni = Some("api example.com".to_string());
        assert_eq!(config_with(target).validation_error(), Some(INVALID_SNI.to_string()));
    }

    #[test]
    fn validation_error_none_for_explicit_valid_override_host() {
        for host in [
            "localhost",
            "us-west.internal",
            "api.example.com:8443",
            "[::1]",
            "[::1]:443",
        ] {
            let mut target = target_with("api-us.example.com");
            target.override_host = Some(host.to_string());
            assert_eq!(
                config_with(target).validation_error(),
                None,
                "supported overrideHost form must remain valid: {host}"
            );
        }
    }

    #[test]
    fn validation_error_rejects_explicit_empty_override_host() {
        let mut target = target_with("api-us.example.com");
        target.override_host = Some(String::new());
        assert_eq!(
            config_with(target).validation_error(),
            Some(EMPTY_OVERRIDE_HOST.to_string())
        );
    }

    #[test]
    fn validation_error_rejects_override_host_header_value() {
        let mut target = target_with("api-us.example.com");
        target.override_host = Some("api.example.com\r\nX-Injected: 1".to_string());
        assert_eq!(
            config_with(target).validation_error(),
            Some(OVERRIDE_HOST_HEADER.to_string())
        );
    }

    #[test]
    fn validation_error_rejects_override_host_authority() {
        for host in [
            "not a host",
            "user@example.com",
            "example.com:abc",
            "example.com:0",
            "example.com:+80",
        ] {
            let mut target = target_with("api-us.example.com");
            target.override_host = Some(host.to_string());
            assert_eq!(
                config_with(target).validation_error(),
                Some(OVERRIDE_HOST_AUTHORITY.to_string()),
                "invalid Host authority must be rejected: {host}"
            );
        }
    }
}
