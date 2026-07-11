//! Wasm plugin configuration (proxy-wasm ABI host).
//!
//! Executes a WebAssembly guest module implementing the proxy-wasm ABI (v0.2
//! subset: request/response header hooks, local responses, logging, basic
//! properties). The gateway compiles and caches the module globally and pools
//! initialized instances per module.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{SecretObjectReference, TlsConfig};

/// Where the Wasm bytecode comes from. Exactly one field must be set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WasmSourceSpec {
    /// Local filesystem path to a `.wasm` file (resolved against the gateway
    /// work directory when relative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Base64-encoded Wasm bytecode carried inline in the resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline: Option<String>,
    /// HTTP(S) URL to fetch the bytecode from (requires `sha256`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Connection settings for the `url` fetch (TLS, auth, timeout, retry).
    /// Only meaningful when `url` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch: Option<WasmFetchSpec>,
}

/// Connection settings for fetching `source.url` bytecode from an artifact
/// repository. Reuses the shared `TlsConfig` type (custom CA / mTLS) rather
/// than referencing a LinkSys resource: a one-shot module fetch does not need
/// long-lived connection governance (rate limit / health check).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WasmFetchSpec {
    /// TLS settings for the fetch (custom CA bundle and/or client identity).
    /// CA / client-cert Secret refs are resolved by the controller.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,

    /// Secret whose `key` holds the full `Authorization` header value sent
    /// with the fetch (e.g. `Bearer <token>` or `Basic <b64>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_header_secret_ref: Option<WasmAuthHeaderSecretRef>,

    /// Controller-resolved `Authorization` header value. Never enters the
    /// CRD schema; redacted in admin output; shipped to the gateway over
    /// conf_sync.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_auth_header: Option<String>,

    /// Per-attempt fetch timeout in milliseconds.
    #[serde(default = "default_fetch_timeout_ms")]
    pub timeout_ms: u64,

    /// Retry policy. Fixed semantics: transport errors and 5xx responses are
    /// retried with exponential backoff; 4xx responses are never retried
    /// (an auth failure will not heal by retrying).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<WasmFetchRetry>,
}

/// Retry policy for the url fetch.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WasmFetchRetry {
    /// Total attempts including the first one (1 = no retry).
    #[serde(default = "default_fetch_attempts")]
    pub attempts: u32,
    /// Base backoff in milliseconds, doubled per retry.
    #[serde(default = "default_fetch_backoff_ms")]
    pub backoff_ms: u64,
}

impl Default for WasmFetchRetry {
    fn default() -> Self {
        WasmFetchRetry {
            attempts: default_fetch_attempts(),
            backoff_ms: default_fetch_backoff_ms(),
        }
    }
}

/// Reference to the Secret carrying the fetch `Authorization` header value.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WasmAuthHeaderSecretRef {
    /// The Secret (cross-namespace refs require a ReferenceGrant).
    #[serde(flatten)]
    pub secret: SecretObjectReference,
    /// Key within the Secret `data` map holding the header value.
    #[serde(default = "default_auth_header_key")]
    pub key: String,
}

fn default_fetch_timeout_ms() -> u64 {
    10_000
}
fn default_fetch_attempts() -> u32 {
    3
}
fn default_fetch_backoff_ms() -> u64 {
    500
}
fn default_auth_header_key() -> String {
    "authorization".to_string()
}

impl WasmSourceSpec {
    /// Stable cache key for the module cache (does not include timeouts etc.).
    ///
    /// Inline sources are keyed by a SHA-256 hash of the base64 string
    /// bytes, not by length: two different inline modules of equal base64
    /// length would otherwise collide in the module cache, causing one
    /// plugin to execute the other's module.
    pub fn cache_key(&self) -> String {
        let inline_key = match &self.inline {
            Some(b64) => {
                use sha2::{Digest, Sha256};
                format!("inline_sha256={}", hex::encode(Sha256::digest(b64.as_bytes())))
            }
            None => "inline_sha256=".to_string(),
        };
        format!(
            "file={};{};url={}",
            self.file.as_deref().unwrap_or(""),
            inline_key,
            self.url.as_deref().unwrap_or("")
        )
    }

    /// Human-readable, bounded-cardinality descriptor for metrics labels:
    /// the file path, the url, or `inline:<sha8>`. Unlike `cache_key` this is
    /// meant to be read by an operator on a dashboard.
    pub fn metrics_label(&self) -> String {
        if let Some(f) = &self.file {
            return format!("file:{f}");
        }
        if let Some(u) = &self.url {
            return format!("url:{u}");
        }
        if let Some(b64) = &self.inline {
            use sha2::{Digest, Sha256};
            let digest = hex::encode(Sha256::digest(b64.as_bytes()));
            return format!("inline:{}", &digest[..8]);
        }
        "unknown".to_string()
    }
}

/// Wasm plugin configuration.
///
/// ## Usage Example:
/// ```yaml
/// type: Wasm
/// config:
///   source:
///     file: ./plugins/header_demo.wasm
///   pluginConfig: '{"add_header":"x-wasm-added"}'
///   failOpen: false
///   timeoutMs: 100
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WasmConfig {
    /// Bytecode source (exactly one of file / inline / url).
    pub source: WasmSourceSpec,

    /// Hex-encoded SHA-256 of the bytecode. Optional for file/inline,
    /// REQUIRED for url sources. Verified before compilation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

    /// Opaque configuration passed to the guest `on_configure` hook
    /// (readable via the PluginConfiguration buffer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_config: Option<String>,

    /// Opaque configuration passed to the guest `on_vm_start` hook
    /// (readable via the VmConfiguration buffer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_config: Option<String>,

    /// On load failure / trap / timeout: `false` (default) returns 500,
    /// `true` continues the plugin chain.
    #[serde(default)]
    pub fail_open: bool,

    /// Per-hook wall-clock budget in milliseconds (epoch interruption).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// Max pooled instances kept per module (per gateway process).
    #[serde(default = "default_pool_size")]
    pub instance_pool_size: usize,

    /// Origins (`scheme://host[:port]`) the guest may target with
    /// `proxy_dispatch_http_call`. Empty (default) disables callouts: a
    /// dispatch attempt fails with a host error the guest can observe.
    /// Entries are exact-origin matches after normalization (scheme
    /// lowercased, default port filled in). Request phase only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub callout_allowlist: Vec<String>,

    /// Upper bound applied to each callout's guest-supplied timeout, in
    /// milliseconds. Also the effective timeout when the guest passes 0.
    #[serde(default = "default_callout_timeout_ms")]
    pub callout_timeout_ms: u64,
}

/// Normalize a callout origin to `scheme://host:port` canonical form.
///
/// Accepts only `http`/`https`, a host, an optional port, and nothing else —
/// no path (beyond a bare `/`), query, fragment, or credentials. Shared by
/// admission validation (here) and the gateway's dispatch-time allowlist
/// check so the two can never disagree.
pub fn normalize_callout_origin(raw: &str) -> Result<String, String> {
    let parsed = url::Url::parse(raw).map_err(|e| format!("'{raw}' is not a valid URL: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("'{raw}': only http/https origins are allowed"));
    }
    if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!("'{raw}': origin only — no path, query, or fragment"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("'{raw}': credentials are not allowed in an origin"));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("'{raw}': origin must include a host"))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| format!("'{raw}': origin must include a port"))?;
    Ok(format!("{scheme}://{host}:{port}"))
}

fn default_timeout_ms() -> u64 {
    100
}

fn default_pool_size() -> usize {
    8
}

fn default_callout_timeout_ms() -> u64 {
    5_000
}

impl Default for WasmConfig {
    fn default() -> Self {
        WasmConfig {
            source: WasmSourceSpec::default(),
            sha256: None,
            plugin_config: None,
            vm_config: None,
            fail_open: false,
            timeout_ms: default_timeout_ms(),
            instance_pool_size: default_pool_size(),
            callout_allowlist: Vec::new(),
            callout_timeout_ms: default_callout_timeout_ms(),
        }
    }
}

const MAX_TIMEOUT_MS: u64 = 10_000;
const MAX_POOL_SIZE: usize = 64;
/// Hard cap for `source.inline` decoded bytecode. Inline is a dev/test
/// convenience only: the bytes ride the CR through etcd (~1.5MiB object cap,
/// minus base64 overhead) and every conf_sync carries them in full. Anything
/// larger must use a `file` or `url` source.
pub const MAX_INLINE_DECODED_BYTES: usize = 1024 * 1024;
/// Above this decoded size the gateway logs a WARN at load time: still
/// accepted, but the operator should move to a `file`/`url` source.
pub const INLINE_WARN_DECODED_BYTES: usize = 512 * 1024;

impl WasmConfig {
    pub fn validate(&self) -> Result<(), String> {
        let set = [
            self.source.file.is_some(),
            self.source.inline.is_some(),
            self.source.url.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if set != 1 {
            return Err("source: exactly one of file/inline/url must be set".to_string());
        }
        if let Some(url) = &self.source.url {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!("source.url must be http(s), got '{}'", url));
            }
            if self.sha256.is_none() {
                return Err("sha256 is required when source.url is used".to_string());
            }
        }
        if let Some(fetch) = &self.source.fetch {
            if self.source.url.is_none() {
                return Err("source.fetch is only valid together with source.url".to_string());
            }
            if fetch.timeout_ms == 0 || fetch.timeout_ms > 60_000 {
                return Err(format!(
                    "source.fetch.timeoutMs must be in 1..=60000, got {}",
                    fetch.timeout_ms
                ));
            }
            if let Some(retry) = &fetch.retry {
                if retry.attempts == 0 || retry.attempts > 10 {
                    return Err(format!(
                        "source.fetch.retry.attempts must be in 1..=10, got {}",
                        retry.attempts
                    ));
                }
                if retry.backoff_ms > 30_000 {
                    return Err(format!(
                        "source.fetch.retry.backoffMs must be <= 30000, got {}",
                        retry.backoff_ms
                    ));
                }
            }
        }
        if let Some(sum) = &self.sha256 {
            if sum.len() != 64 || !sum.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("sha256 must be 64 hex characters".to_string());
            }
        }
        if let Some(inline) = &self.source.inline {
            use base64::Engine as _;
            match base64::engine::general_purpose::STANDARD.decode(inline) {
                Err(_) => return Err("source.inline is not valid base64".to_string()),
                Ok(decoded) if decoded.len() > MAX_INLINE_DECODED_BYTES => {
                    return Err(format!(
                        "source.inline decodes to {} bytes, exceeding the {}KiB inline cap; \
                         inline is a dev/test convenience — use a file or url source for larger modules",
                        decoded.len(),
                        MAX_INLINE_DECODED_BYTES / 1024
                    ));
                }
                Ok(_) => {}
            }
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_TIMEOUT_MS {
            return Err(format!(
                "timeoutMs must be in 1..={}, got {}",
                MAX_TIMEOUT_MS, self.timeout_ms
            ));
        }
        if self.instance_pool_size == 0 || self.instance_pool_size > MAX_POOL_SIZE {
            return Err(format!(
                "instancePoolSize must be in 1..={}, got {}",
                MAX_POOL_SIZE, self.instance_pool_size
            ));
        }
        for entry in &self.callout_allowlist {
            normalize_callout_origin(entry).map_err(|e| format!("calloutAllowlist: {e}"))?;
        }
        if self.callout_timeout_ms == 0 || self.callout_timeout_ms > MAX_TIMEOUT_MS {
            return Err(format!(
                "calloutTimeoutMs must be in 1..={}, got {}",
                MAX_TIMEOUT_MS, self.callout_timeout_ms
            ));
        }
        Ok(())
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for WasmConfig {
    fn validation_error(&self) -> Option<String> {
        self.validate().err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_config() -> WasmConfig {
        WasmConfig {
            source: WasmSourceSpec {
                file: Some("./demo.wasm".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn validation_error_returns_field_error_without_validate_call() {
        // No source set at all → invalid.
        let config = WasmConfig::default();
        assert!(config.validation_error().is_some());
    }

    #[test]
    fn valid_file_source_passes() {
        assert!(file_config().validate().is_ok());
    }

    #[test]
    fn two_sources_rejected() {
        let mut c = file_config();
        c.source.inline = Some("AA==".to_string());
        assert!(c.validate().unwrap_err().contains("exactly one"));
    }

    #[test]
    fn url_requires_sha256() {
        let mut c = WasmConfig::default();
        c.source.url = Some("https://example.com/m.wasm".to_string());
        assert!(c.validate().unwrap_err().contains("sha256"));
        c.sha256 = Some("a".repeat(64));
        assert!(c.validate().is_ok());
    }

    #[test]
    fn bad_sha256_rejected() {
        let mut c = file_config();
        c.sha256 = Some("xyz".to_string());
        assert!(c.validate().is_err());
    }

    #[test]
    fn bad_inline_base64_rejected() {
        let mut c = WasmConfig::default();
        c.source.inline = Some("!!not-base64!!".to_string());
        assert!(c.validate().unwrap_err().contains("base64"));
    }

    #[test]
    fn fetch_requires_url_source() {
        let mut c = file_config();
        c.source.fetch = Some(WasmFetchSpec::default());
        assert!(c.validate().unwrap_err().contains("source.url"));

        let mut c = WasmConfig::default();
        c.source.url = Some("https://artifacts.internal/m.wasm".to_string());
        c.sha256 = Some("a".repeat(64));
        c.source.fetch = Some(WasmFetchSpec {
            timeout_ms: default_fetch_timeout_ms(),
            ..Default::default()
        });
        assert!(c.validate().is_ok());
    }

    #[test]
    fn fetch_bounds_validated() {
        let mut c = WasmConfig::default();
        c.source.url = Some("https://artifacts.internal/m.wasm".to_string());
        c.sha256 = Some("a".repeat(64));
        let mut fetch = WasmFetchSpec {
            timeout_ms: 0,
            ..Default::default()
        };
        c.source.fetch = Some(fetch.clone());
        assert!(c.validate().unwrap_err().contains("timeoutMs"));

        fetch.timeout_ms = 5000;
        fetch.retry = Some(WasmFetchRetry {
            attempts: 0,
            backoff_ms: 100,
        });
        c.source.fetch = Some(fetch.clone());
        assert!(c.validate().unwrap_err().contains("attempts"));

        fetch.retry = Some(WasmFetchRetry {
            attempts: 3,
            backoff_ms: 60_000,
        });
        c.source.fetch = Some(fetch);
        assert!(c.validate().unwrap_err().contains("backoffMs"));
    }

    #[test]
    fn fetch_serde_camel_case_and_resolved_field_not_deserializable_from_crd_path() {
        let json = r#"{
            "source": {
                "url": "https://artifacts.internal/m.wasm",
                "fetch": {
                    "authHeaderSecretRef": {"name": "repo-token", "key": "authorization"},
                    "timeoutMs": 3000,
                    "retry": {"attempts": 2, "backoffMs": 250}
                }
            },
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }"#;
        let c: WasmConfig = serde_json::from_str(json).unwrap();
        let fetch = c.source.fetch.as_ref().unwrap();
        assert_eq!(fetch.timeout_ms, 3000);
        assert_eq!(fetch.retry.as_ref().unwrap().attempts, 2);
        let auth = fetch.auth_header_secret_ref.as_ref().unwrap();
        assert_eq!(auth.secret.name, "repo-token");
        assert_eq!(auth.key, "authorization");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn callout_origin_normalization() {
        assert_eq!(
            normalize_callout_origin("https://auth.example.com").unwrap(),
            "https://auth.example.com:443"
        );
        assert_eq!(normalize_callout_origin("http://svc:8080/").unwrap(), "http://svc:8080");
        assert!(normalize_callout_origin("https://a.example.com/v1").is_err());
        assert!(normalize_callout_origin("ftp://a.example.com").is_err());
        assert!(normalize_callout_origin("https://user:pw@a.example.com").is_err());
        assert!(normalize_callout_origin("https://a.example.com?q=1").is_err());
    }

    #[test]
    fn callout_config_validated() {
        let mut c = file_config();
        c.callout_allowlist = vec!["https://auth.example.com".to_string()];
        assert!(c.validate().is_ok());
        c.callout_allowlist = vec!["https://a.example.com/deny-path".to_string()];
        assert!(c.validate().unwrap_err().contains("calloutAllowlist"));
        c.callout_allowlist = vec![];
        c.callout_timeout_ms = 0;
        assert!(c.validate().unwrap_err().contains("calloutTimeoutMs"));
    }

    #[test]
    fn resolved_auth_header_redacted_under_admin_guard() {
        let fetch = WasmFetchSpec {
            resolved_auth_header: Some("Bearer real-token".to_string()),
            timeout_ms: default_fetch_timeout_ms(),
            ..Default::default()
        };
        // conf_sync path (no guard): real value ships to the gateway.
        let plain = serde_json::to_value(&fetch).unwrap();
        assert_eq!(plain["resolvedAuthHeader"], "Bearer real-token");
        // Admin path (guard active): value collapses to "[redacted]".
        let _guard = crate::redact::AdminRedactGuard::new();
        let redacted = serde_json::to_value(&fetch).unwrap();
        assert_eq!(redacted["resolvedAuthHeader"], "[redacted]");
    }

    #[test]
    fn fetch_does_not_change_cache_key() {
        let mut a = WasmSourceSpec {
            url: Some("https://artifacts.internal/m.wasm".to_string()),
            ..Default::default()
        };
        let key_plain = a.cache_key();
        a.fetch = Some(WasmFetchSpec::default());
        assert_eq!(
            a.cache_key(),
            key_plain,
            "fetch settings must not affect module identity"
        );
    }

    #[test]
    fn oversized_inline_rejected_with_guidance() {
        use base64::Engine as _;
        let mut c = WasmConfig::default();
        let bytes = vec![0u8; MAX_INLINE_DECODED_BYTES + 1];
        c.source.inline = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
        let err = c.validate().unwrap_err();
        assert!(err.contains("inline cap"), "got: {err}");
        assert!(err.contains("file or url"), "got: {err}");
        // At the cap exactly it still passes.
        let bytes = vec![0u8; MAX_INLINE_DECODED_BYTES];
        c.source.inline = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
        assert!(c.validate().is_ok());
    }

    #[test]
    fn timeout_and_pool_bounds() {
        let mut c = file_config();
        c.timeout_ms = 0;
        assert!(c.validate().is_err());
        c.timeout_ms = 10_001;
        assert!(c.validate().is_err());
        c.timeout_ms = 100;
        c.instance_pool_size = 0;
        assert!(c.validate().is_err());
        c.instance_pool_size = 65;
        assert!(c.validate().is_err());
    }

    #[test]
    fn cache_key_distinguishes_equal_length_inline_sources() {
        // Two different inline base64 strings of equal length must not
        // collide in the module cache key (previously keyed by length only).
        let a = WasmSourceSpec {
            inline: Some("AAAAAAAAAAAAAAAAAAAA".to_string()),
            ..Default::default()
        };
        let b = WasmSourceSpec {
            inline: Some("BBBBBBBBBBBBBBBBBBBB".to_string()),
            ..Default::default()
        };
        assert_eq!(a.inline.as_deref().unwrap().len(), b.inline.as_deref().unwrap().len());
        assert_ne!(
            a.cache_key(),
            b.cache_key(),
            "different inline content must not collide"
        );
    }

    #[test]
    fn cache_key_stable_for_identical_inline_source() {
        let a = WasmSourceSpec {
            inline: Some("AAAAAAAAAAAAAAAAAAAA".to_string()),
            ..Default::default()
        };
        let b = WasmSourceSpec {
            inline: Some("AAAAAAAAAAAAAAAAAAAA".to_string()),
            ..Default::default()
        };
        assert_eq!(
            a.cache_key(),
            b.cache_key(),
            "identical inline content must key identically"
        );
    }

    #[test]
    fn serde_camel_case_roundtrip() {
        let json = r#"{
            "source": { "file": "./demo.wasm" },
            "pluginConfig": "{}",
            "failOpen": true,
            "timeoutMs": 50,
            "instancePoolSize": 4
        }"#;
        let c: WasmConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.source.file.as_deref(), Some("./demo.wasm"));
        assert!(c.fail_open);
        assert_eq!(c.timeout_ms, 50);
        assert_eq!(c.instance_pool_size, 4);
    }
}
