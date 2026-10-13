//! Wasm plugin configuration (proxy-wasm ABI host).
//!
//! Executes a WebAssembly guest module implementing the proxy-wasm ABI (v0.2
//! subset: request/response header hooks, local responses, logging, basic
//! properties). The gateway compiles and caches the module globally and pools
//! initialized instances per module.

use schemars::JsonSchema;

crate::redact::impl_redacted_debug!(WasmFetchSpec);
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::wasm_oci::{OciImageRef, WasmOciSpec};
use crate::resources::common::{SecretObjectReference, TlsConfig};

const MAX_SOURCE_METRICS_LABEL_BYTES: usize = 96;

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
    /// OCI registry source (pull an image whose layer carries the bytecode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oci: Option<WasmOciSpec>,
}

/// Connection settings for fetching `source.url` bytecode from an artifact
/// repository. Reuses the shared `TlsConfig` type (custom CA / mTLS) rather
/// than referencing a LinkSys resource: a one-shot module fetch does not need
/// long-lived connection governance (rate limit / health check).
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
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
            "file={};{};url={};oci={}",
            self.file.as_deref().unwrap_or(""),
            inline_key,
            self.url.as_deref().unwrap_or(""),
            self.oci.as_ref().map(|o| o.image.as_str()).unwrap_or("")
        )
    }

    /// Human-readable source descriptor for metrics labels. URL identities
    /// exclude userinfo, query, fragment, and clear-text path content so
    /// signed URL generations share one bounded, credential-free label.
    /// Unlike `cache_key`, this is safe to expose in observability output.
    pub fn metrics_label(&self) -> String {
        if let Some(f) = &self.file {
            return format!("file:{f}");
        }
        if let Some(u) = &self.url {
            return url_metrics_label(u);
        }
        if let Some(b64) = &self.inline {
            use sha2::{Digest, Sha256};
            let digest = hex::encode(Sha256::digest(b64.as_bytes()));
            return format!("inline:{}", &digest[..8]);
        }
        if let Some(oci) = &self.oci {
            return match OciImageRef::parse(&oci.image) {
                Ok(r) => match &r.digest {
                    Some(d) => {
                        let hex_part = &d["sha256:".len()..];
                        format!("oci:{}/{}@{}", r.registry, r.repository, &hex_part[..8])
                    }
                    None => format!(
                        "oci:{}/{}:{}",
                        r.registry,
                        r.repository,
                        r.tag.as_deref().unwrap_or("latest")
                    ),
                },
                // validate() rejects unparseable refs; keep a readable fallback.
                Err(_) => format!("oci:{}", oci.image),
            };
        }
        "unknown".to_string()
    }
}

fn url_metrics_label(raw: &str) -> String {
    use sha2::Digest as _;

    let Ok(mut parsed) = url::Url::parse(raw) else {
        return "url:invalid".to_string();
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return "url:invalid".to_string();
    }

    // `origin` cannot contain userinfo, path, query, or fragment. The digest
    // retains path-level correlation without exposing path-carried secrets;
    // query and fragment are cleared first so signed URL rotations do not
    // create new metric series.
    let origin = parsed.origin().ascii_serialization();
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    let digest = hex::encode(sha2::Sha256::digest(parsed.as_str().as_bytes()));
    let suffix = format!("#{}", &digest[..8]);
    let prefix = "url:";
    let origin_limit = MAX_SOURCE_METRICS_LABEL_BYTES - prefix.len() - suffix.len();
    let bounded_origin = &origin.as_bytes()[..origin.len().min(origin_limit)];
    // `Origin::ascii_serialization` guarantees ASCII, so the byte boundary is
    // also a UTF-8 character boundary.
    format!(
        "{prefix}{}{suffix}",
        std::str::from_utf8(bounded_origin).unwrap_or("invalid")
    )
}

/// Wasm plugin configuration.
///
/// ## Usage Example:
/// ```yaml
/// type: Wasm
/// config:
///   source:
///     file: ./plugins/header_demo.wasm
///   pluginConfig:
///     json:
///       add_header: x-wasm-added
///   failOpen: false
///   timeoutMs: 100
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WasmConfig {
    /// Bytecode source (exactly one of file / inline / url / oci).
    pub source: WasmSourceSpec,

    /// Hex-encoded SHA-256 of the bytecode. Optional for file/inline,
    /// REQUIRED for url sources. Verified before compilation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

    /// Opaque bytes passed to the guest `on_configure` hook (readable via the
    /// PluginConfiguration buffer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_config: Option<WasmRawConfig>,

    /// Opaque bytes passed to the guest `on_vm_start` hook (readable via the
    /// VmConfiguration buffer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_config: Option<WasmRawConfig>,

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

/// One strict representation of opaque bytes passed to a Proxy-Wasm guest.
///
/// Exactly one object shape is accepted:
///
/// - `{ string: "..." }` passes the string's UTF-8 bytes.
/// - `{ base64: "..." }` decodes canonical padded RFC 4648 Standard base64.
/// - `{ json: ... }` accepts any JSON value and emits compact UTF-8 JSON with
///   object keys sorted recursively. Original whitespace, object-key order,
///   and number spelling are not preserved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum WasmRawConfig {
    String(WasmRawString),
    Json(WasmRawJson),
    Base64(WasmRawBase64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WasmRawString {
    pub string: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WasmRawJson {
    pub json: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WasmRawBase64 {
    pub base64: String,
}

/// Hard cap for each decoded guest configuration field.
pub const MAX_WASM_RAW_CONFIG_BYTES: usize = 64 * 1024;
const MAX_WASM_RAW_BASE64_BYTES: usize = MAX_WASM_RAW_CONFIG_BYTES.div_ceil(3) * 4;

impl WasmRawConfig {
    pub fn string(value: impl Into<String>) -> Self {
        Self::String(WasmRawString { string: value.into() })
    }

    pub fn base64(value: impl Into<String>) -> Self {
        Self::Base64(WasmRawBase64 { base64: value.into() })
    }

    /// Decode the exact bytes that the Gateway passes to the guest.
    ///
    /// The Gateway calls this independently even after Controller validation;
    /// callers must handle the error rather than assuming conf_sync is trusted.
    pub fn guest_bytes(&self) -> Result<Vec<u8>, String> {
        let bytes = match self {
            Self::String(value) => {
                if value.string.len() > MAX_WASM_RAW_CONFIG_BYTES {
                    return Err(raw_config_size_error());
                }
                value.string.as_bytes().to_vec()
            }
            Self::Base64(value) => {
                use base64::Engine as _;

                if value.base64.len() > MAX_WASM_RAW_BASE64_BYTES {
                    return Err(raw_config_size_error());
                }
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(value.base64.as_bytes())
                    .map_err(|_| "must be canonical padded Standard base64".to_string())?;
                if decoded.len() > MAX_WASM_RAW_CONFIG_BYTES {
                    return Err(raw_config_size_error());
                }
                if base64::engine::general_purpose::STANDARD.encode(&decoded) != value.base64 {
                    return Err("must be canonical padded Standard base64".to_string());
                }
                decoded
            }
            Self::Json(value) => canonical_json_bytes(&value.json)?,
        };
        Ok(bytes)
    }
}

fn raw_config_size_error() -> String {
    format!(
        "decoded value exceeds the {}KiB limit",
        MAX_WASM_RAW_CONFIG_BYTES / 1024
    )
}

struct CappedJsonWriter {
    bytes: Vec<u8>,
}

impl std::io::Write for CappedJsonWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(buf.len()) > MAX_WASM_RAW_CONFIG_BYTES {
            return Err(std::io::Error::other("canonical JSON exceeds the guest config limit"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, String> {
    let mut writer = CappedJsonWriter {
        bytes: Vec::with_capacity(256),
    };
    write_canonical_json(&mut writer, value).map_err(|_| raw_config_size_error())?;
    Ok(writer.bytes)
}

fn write_canonical_json(writer: &mut CappedJsonWriter, value: &Value) -> std::io::Result<()> {
    use std::io::Write as _;

    match value {
        Value::Object(object) => {
            writer.write_all(b"{")?;
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (index, (key, child)) in entries.into_iter().enumerate() {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, key).map_err(std::io::Error::other)?;
                writer.write_all(b":")?;
                write_canonical_json(writer, child)?;
            }
            writer.write_all(b"}")
        }
        Value::Array(array) => {
            writer.write_all(b"[")?;
            for (index, child) in array.iter().enumerate() {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                write_canonical_json(writer, child)?;
            }
            writer.write_all(b"]")
        }
        _ => serde_json::to_writer(writer, value).map_err(std::io::Error::other),
    }
}

/// Normalize a callout origin to `scheme://host:port` canonical form.
///
/// Accepts only `http`/`https`, a host, an optional port, and nothing else —
/// no path (beyond a bare `/`), query, fragment, or credentials. Shared by
/// admission validation (here) and the gateway's dispatch-time allowlist
/// check so the two can never disagree. Rejection diagnostics contain only fixed
/// categories because invalid input may contain credentials or tokens.
pub fn normalize_callout_origin(raw: &str) -> Result<String, String> {
    normalize_callout_origin_inner(raw).map_err(|error| {
        let message = match error {
            CalloutOriginError::InvalidUrl => "origin is not a valid URL",
            CalloutOriginError::Scheme => "only http/https origins are allowed",
            CalloutOriginError::Components => "origin only: no path, query, or fragment",
            CalloutOriginError::Credentials => "credentials are not allowed in an origin",
            CalloutOriginError::Host => "origin must include a host",
            CalloutOriginError::Port => "origin must include a port",
        };
        message.to_string()
    })
}

/// Runtime variant of [`normalize_callout_origin`] that deliberately avoids
/// allocating an error diagnostic.
pub fn normalize_callout_origin_quiet(raw: &str) -> Option<String> {
    normalize_callout_origin_inner(raw).ok()
}

enum CalloutOriginError {
    InvalidUrl,
    Scheme,
    Components,
    Credentials,
    Host,
    Port,
}

fn normalize_callout_origin_inner(raw: &str) -> Result<String, CalloutOriginError> {
    let parsed = url::Url::parse(raw).map_err(|_| CalloutOriginError::InvalidUrl)?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(CalloutOriginError::Scheme);
    }
    if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(CalloutOriginError::Components);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CalloutOriginError::Credentials);
    }
    let host = parsed.host_str().ok_or(CalloutOriginError::Host)?;
    let port = parsed.port_or_known_default().ok_or(CalloutOriginError::Port)?;
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
            self.source.oci.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if set != 1 {
            return Err("source: exactly one of file/inline/url/oci must be set".to_string());
        }
        if let Some(url) = &self.source.url {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("source.url must use http or https".to_string());
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
        if let Some(oci) = &self.source.oci {
            OciImageRef::parse(&oci.image).map_err(|e| format!("source.oci.image: {e}"))?;
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
        if let Some(config) = &self.plugin_config {
            config.guest_bytes().map_err(|error| format!("pluginConfig {error}"))?;
        }
        if let Some(config) = &self.vm_config {
            config.guest_bytes().map_err(|error| format!("vmConfig {error}"))?;
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
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/wasm_test.rs"]
mod tests;
