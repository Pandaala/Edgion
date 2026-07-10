//! Wasm plugin configuration (proxy-wasm ABI host).
//!
//! Executes a WebAssembly guest module implementing the proxy-wasm ABI (v0.2
//! subset: request/response header hooks, local responses, logging, basic
//! properties). The gateway compiles and caches the module globally and pools
//! initialized instances per module.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
}

fn default_timeout_ms() -> u64 {
    100
}

fn default_pool_size() -> usize {
    8
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
        }
    }
}

const MAX_TIMEOUT_MS: u64 = 10_000;
const MAX_POOL_SIZE: usize = 64;

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
        if let Some(sum) = &self.sha256 {
            if sum.len() != 64 || !sum.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("sha256 must be 64 hex characters".to_string());
            }
        }
        if let Some(inline) = &self.source.inline {
            use base64::Engine as _;
            if base64::engine::general_purpose::STANDARD.decode(inline).is_err() {
                return Err("source.inline is not valid base64".to_string());
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
