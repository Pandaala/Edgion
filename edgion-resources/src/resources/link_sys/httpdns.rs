//! HTTP DNS resolver configuration for LinkSys.
//!
//! Resolves a domain by issuing an HTTP(S) request to an HTTP DNS endpoint and
//! parsing the response into an IP list. This is the only LinkSys-backed
//! resolver variant; standard DNS is configured inline (a server list) and
//! system DNS is the no-config fallback.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::TlsConfig;

/// Connection-level settings for an HTTP DNS resolver request.
///
/// Deliberately lean: an HTTP DNS resolver targets a `urlTemplate` (not a
/// Kubernetes Service), so — unlike the webhook `ExternalHttpConnection` — it
/// has NO required `target`/`WebhookTarget`. Settings cover outbound TLS,
/// request timeout, and the explicit local-service exception.
//
// NOTE: no `PartialEq`/`Eq` — `TlsConfig` derives neither, so structural
// equality is unavailable here (and unnecessary). Do not add them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HttpDnsConnection {
    /// Allow this HTTP DNS endpoint to use loopback addresses. Default false.
    /// Link-local and unspecified addresses remain blocked; private addresses
    /// remain allowed. Applies to both literal and resolved endpoint addresses.
    #[serde(default)]
    pub allow_loopback: bool,
    /// Outbound TLS configuration (custom CA, mTLS, SAN checks) for the HTTPS
    /// request to the HTTP DNS endpoint. Absent = system CA bundle, no client
    /// certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,
    /// Request timeout in milliseconds. Absent = the fetcher's built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 120000))]
    pub timeout_ms: Option<u64>,
    /// Connect timeout in milliseconds. Absent = effective total timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 120000))]
    pub connect_timeout_ms: Option<u64>,
    /// Idle connections retained per endpoint host. Default 8; zero disables pooling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0, max = 256))]
    pub pool_max_idle_per_host: Option<usize>,
    /// Idle connection lifetime in milliseconds. Default 90000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 600000))]
    pub pool_idle_timeout_ms: Option<u64>,
    /// Maximum response bytes; cannot exceed the 65536-byte physical cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 65536))]
    pub max_response_bytes: Option<usize>,
}

impl HttpDnsConnection {
    pub const DEFAULT_TIMEOUT_MS: u64 = 3000;
    pub const DEFAULT_POOL_MAX_IDLE_PER_HOST: usize = 8;
    pub const DEFAULT_POOL_IDLE_TIMEOUT_MS: u64 = 90000;
    pub const MAX_RESPONSE_BYTES: usize = 65536;

    /// Validate both admission and runtime construction through the same bounds.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let total = self.timeout_ms.unwrap_or(Self::DEFAULT_TIMEOUT_MS);
        let connect = self.connect_timeout_ms.unwrap_or(total);
        if !(1..=120000).contains(&total) {
            errors.push("HTTP DNS timeoutMs must be between 1 and 120000".into());
        }
        if !(1..=120000).contains(&connect) || connect > total {
            errors.push("HTTP DNS connectTimeoutMs must be between 1 and timeoutMs (at most 120000)".into());
        }
        if self
            .pool_max_idle_per_host
            .unwrap_or(Self::DEFAULT_POOL_MAX_IDLE_PER_HOST)
            > 256
        {
            errors.push("HTTP DNS poolMaxIdlePerHost must be between 0 and 256".into());
        }
        if !(1..=600000).contains(&self.pool_idle_timeout_ms.unwrap_or(Self::DEFAULT_POOL_IDLE_TIMEOUT_MS)) {
            errors.push("HTTP DNS poolIdleTimeoutMs must be between 1 and 600000".into());
        }
        if !(1..=Self::MAX_RESPONSE_BYTES).contains(&self.max_response_bytes.unwrap_or(Self::MAX_RESPONSE_BYTES)) {
            errors.push("HTTP DNS maxResponseBytes must be between 1 and 65536".into());
        }
        errors
    }
}

/// Built-in vendor presets. Each preset implies a known URL template and
/// response format so operators do not have to spell them out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HttpDnsPreset {
    Aliyun,
    Tencent,
}

impl HttpDnsPreset {
    /// The vendor's built-in best-effort HTTP DNS URL template. `{domain}` is
    /// substituted at request time.
    ///
    /// This is only a fallback used when no explicit `urlTemplate` is given. The
    /// vendors' public resolving endpoints are plain HTTP (their public IPs do
    /// not serve a trusted certificate for a bare-IP TLS handshake), and Aliyun
    /// additionally requires a per-account path segment a preset cannot know.
    /// The recommended production pattern is therefore `preset: <vendor>` (to
    /// reuse the response FORMAT) *plus* an explicit `urlTemplate` carrying the
    /// correct scheme / account / signing — the urlTemplate wins over this
    /// default (see `edgion_common::dns::httpdns::render_url`).
    pub fn url_template(&self) -> &'static str {
        match self {
            // Aliyun public HTTPDNS (account-less form; real accounts use /<id>/d).
            HttpDnsPreset::Aliyun => "http://203.107.1.1/d?host={domain}",
            // Tencent DNSPod (HTTPDNS) public plaintext resolving host. A custom
            // urlTemplate for this preset must NOT add `&ttl=1`: that makes the
            // response `ip;ip,<ttl>` and the ';'-delimited parser drops the last
            // (ttl-suffixed) address.
            HttpDnsPreset::Tencent => "http://119.29.29.29/d?dn={domain}",
        }
    }

    /// The response body format this vendor returns. Used by the resolver core
    /// in place of the user-supplied `response` when a preset is set.
    pub fn response_format(&self) -> HttpDnsResponseFormat {
        match self {
            HttpDnsPreset::Aliyun => HttpDnsResponseFormat {
                kind: HttpDnsBodyKind::Json,
                ip_path: Some("ips".to_string()),
                delimiter: None,
                ttl_path: Some("ttl".to_string()),
            },
            HttpDnsPreset::Tencent => HttpDnsResponseFormat {
                kind: HttpDnsBodyKind::Delimited,
                ip_path: None,
                delimiter: Some(";".to_string()),
                ttl_path: None,
            },
        }
    }
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
// NOTE: no `PartialEq`/`Eq` — `HttpDnsConnection` embeds `TlsConfig`, which
// derives neither, so structural equality is unavailable. Do not add them.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HttpDnsResolverConfig {
    /// Optional vendor preset. Selects the response FORMAT (and a best-effort
    /// default URL); may be combined with an explicit `url_template`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<HttpDnsPreset>,
    /// Request URL template; `{domain}` is substituted. Wins over a preset's
    /// built-in URL, so `preset` + `url_template` = the preset's response format
    /// with a custom (account-scoped / correct-scheme) endpoint. Required only
    /// when no preset is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_template: Option<String>,
    /// Response parsing rules. Ignored when a preset is set (the preset's format
    /// wins); used only for a preset-less generic `url_template` config.
    #[serde(default)]
    pub response: HttpDnsResponseFormat,
    /// Failure fallback behavior.
    #[serde(default)]
    pub fallback: HttpDnsFallback,
    /// Connection settings (outbound TLS, request timeout).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<HttpDnsConnection>,
}

impl HttpDnsResolverConfig {
    /// Validate the configuration; empty result means OK.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if let Some(connection) = &self.connection {
            errors.extend(connection.validate());
        }
        // A urlTemplate, whenever present, must carry the `{domain}` placeholder —
        // including alongside a preset (a preset + explicit urlTemplate reuses the
        // preset's response format with a custom URL). With no urlTemplate, a
        // preset supplies the built-in default; without either there is no URL.
        let tmpl = self.url_template.as_deref().unwrap_or("");
        if !tmpl.is_empty() {
            if !tmpl.contains("{domain}") {
                errors.push("HTTP DNS urlTemplate must contain the {domain} placeholder".to_string());
            }
        } else if self.preset.is_none() {
            errors.push("HTTP DNS requires urlTemplate when no preset is set".to_string());
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
#[path = "../../../tests/unit/resources/link_sys/httpdns_test.rs"]
mod tests;
