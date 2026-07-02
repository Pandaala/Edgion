//! HTTP DNS resolver configuration for LinkSys.
//!
//! Resolves a domain by issuing an HTTP(S) request to an HTTP DNS endpoint and
//! parsing the response into an IP list. This is the only LinkSys-backed
//! resolver variant; standard DNS is configured inline (a server list) and
//! system DNS is the no-config fallback.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::external_http::ExternalHttpConnection;

/// Built-in vendor presets. Each preset implies a known URL template and
/// response format so operators do not have to spell them out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HttpDnsPreset {
    Aliyun,
    Tencent,
}

/// Shape of the HTTP DNS response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum HttpDnsBodyKind {
    #[default]
    Json,
    Delimited,
}

/// How to extract IPs (and optional TTL) from the response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct HttpDnsResponseFormat {
    /// Body kind: `json` (extract via `ipPath`) or `delimited` (split by `delimiter`).
    #[serde(default)]
    pub kind: HttpDnsBodyKind,
    /// For `json`: dotted path to the IP array/string (e.g. "ips" or "data.ips").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_path: Option<String>,
    /// For `delimited`: the separator between IPs (default ",").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
    /// Optional dotted JSON path to a TTL value (parsed for future use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_path: Option<String>,
}

/// What to do when the HTTP DNS request/parse fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HttpDnsFallback {
    /// Fall back to the OS/system resolver.
    #[default]
    System,
    /// Fall back to a standard DNS server list.
    Dns { servers: Vec<String> },
    /// No fallback — surface the failure.
    None,
}

/// HTTP DNS resolver configuration.
//
// NOTE: no `PartialEq`/`Eq` — `ExternalHttpConnection` derives neither, and it
// transitively contains an `f64` (PassiveBackoff.multiplier) so `Eq` is
// structurally impossible. Do not add them.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HttpDnsResolverConfig {
    /// Optional vendor preset (syntactic sugar over urlTemplate + response).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<HttpDnsPreset>,
    /// Request URL template; `{domain}` is substituted. Required when no preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_template: Option<String>,
    /// Response parsing rules (ignored when a preset is set).
    #[serde(default)]
    pub response: HttpDnsResponseFormat,
    /// Failure fallback behavior.
    #[serde(default)]
    pub fallback: HttpDnsFallback,
    /// Connection settings (TLS, timeout) — reuses the shared external-HTTP type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<ExternalHttpConnection>,
}

impl HttpDnsResolverConfig {
    /// Validate the configuration; empty result means OK.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.preset.is_none() {
            if self.url_template.as_deref().unwrap_or("").is_empty() {
                errors.push("HTTP DNS requires urlTemplate when no preset is set".to_string());
            } else if !self.url_template.as_deref().unwrap_or("").contains("{domain}") {
                errors.push("HTTP DNS urlTemplate must contain the {domain} placeholder".to_string());
            }
        }
        if let HttpDnsFallback::Dns { servers } = &self.fallback {
            if servers.is_empty() {
                errors.push("HTTP DNS dns fallback requires a non-empty servers list".to_string());
            }
            for server in servers {
                if !is_valid_dns_server(server) {
                    errors.push(format!(
                        "HTTP DNS dns fallback server '{server}' is not a valid `ip` or `ip:port`"
                    ));
                }
            }
        }
        errors
    }
}

/// Validate a `ip` or `ip:port` DNS server string. Also accepts a bracketed
/// IPv6 literal (with or without a port), matching what the runtime resolver
/// (`edgion_common::dns::standard`) parses.
fn is_valid_dns_server(s: &str) -> bool {
    if s.parse::<std::net::SocketAddr>().is_ok() {
        return true;
    }
    let stripped = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(s);
    stripped.parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_requires_url_template() {
        let cfg = HttpDnsResolverConfig {
            preset: None,
            url_template: None,
            response: HttpDnsResponseFormat::default(),
            fallback: HttpDnsFallback::System,
            connection: None,
        };
        let errs = cfg.validate();
        assert!(
            errs.iter().any(|e| e.contains("urlTemplate")),
            "generic (no preset) must require urlTemplate, got {errs:?}"
        );
    }

    #[test]
    fn preset_is_valid_without_url_template() {
        let cfg = HttpDnsResolverConfig {
            preset: Some(HttpDnsPreset::Aliyun),
            url_template: None,
            response: HttpDnsResponseFormat::default(),
            fallback: HttpDnsFallback::System,
            connection: None,
        };
        assert!(cfg.validate().is_empty(), "preset config must validate clean");
    }

    #[test]
    fn dns_fallback_requires_servers() {
        let cfg = HttpDnsResolverConfig {
            preset: Some(HttpDnsPreset::Aliyun),
            url_template: None,
            response: HttpDnsResponseFormat::default(),
            fallback: HttpDnsFallback::Dns { servers: vec![] },
            connection: None,
        };
        assert!(cfg.validate().iter().any(|e| e.contains("fallback")));
    }

    #[test]
    fn dns_fallback_rejects_invalid_server_string() {
        let cfg = HttpDnsResolverConfig {
            preset: Some(HttpDnsPreset::Aliyun),
            url_template: None,
            response: HttpDnsResponseFormat::default(),
            fallback: HttpDnsFallback::Dns {
                servers: vec!["not-an-ip".to_string()],
            },
            connection: None,
        };
        let errs = cfg.validate();
        assert!(
            errs.iter().any(|e| e.contains("not-an-ip")),
            "expected an error naming the invalid server, got {errs:?}"
        );
    }

    #[test]
    fn dns_fallback_accepts_valid_server_strings() {
        let cfg = HttpDnsResolverConfig {
            preset: Some(HttpDnsPreset::Aliyun),
            url_template: None,
            response: HttpDnsResponseFormat::default(),
            fallback: HttpDnsFallback::Dns {
                servers: vec!["1.1.1.1".to_string(), "8.8.8.8:5353".to_string(), "[::1]".to_string()],
            },
            connection: None,
        };
        assert!(cfg.validate().is_empty(), "valid servers must validate clean");
    }
}
