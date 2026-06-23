//! DSL plugin configuration (schema)
//!
//! This module holds the serializable EdgionDSL plugin configuration. The
//! compiled DSL VM / runtime (`DslPlugin`, bytecode execution) lives gateway-side
//! in the gateway DSL module; this schema type carries no engine dependency so
//! it can live in the `types` leaf layer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::plugin_configs::LinkSysRef;
use crate::resources::edgion_plugins::PluginValidation;

/// DSL error handling policy — user-configurable per plugin.
///
/// Pure serializable schema; the gateway VM (`apply_error_policy`) maps these
/// variants onto runtime `PluginRunningResult`s.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum DslErrorPolicy {
    /// Ignore error, continue to next plugin (GoodNext) — fail-open
    #[serde(alias = "ignore")]
    #[default]
    Ignore,
    /// Return error response to client (default 500)
    #[serde(alias = "deny")]
    Deny,
    /// Return a specific status code + body
    #[serde(alias = "denyWith")]
    DenyWith { status: u16, body: Option<String> },
}

/// DSL plugin configuration — stored in EdgionPlugin enum
///
/// Users can provide inline `source` code in YAML, which is automatically
/// compiled to bytecode during preparse. Pre-compiled `bytecode` is also
/// supported for production deployments.
///
/// YAML example:
/// ```yaml
/// type: Dsl
/// config:
///   name: "ip-filter"
///   source: |
///     let ip = req.header("X-Real-IP")
///     if ip == nil {
///       return deny(403, "Missing IP")
///     }
///   maxSteps: 10000
///   errorPolicy: deny
/// ```
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DslConfig {
    /// Script name (for logging and identification)
    pub name: String,

    /// EdgionDSL source code — inline script written by the user.
    /// Automatically compiled to bytecode during preparse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Pre-compiled bytecode (base64-encoded JSON, populated by controller
    /// or via external tooling). Takes priority over `source` if both present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytecode: Option<String>,

    /// Max VM instructions per execution (default: 10000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,

    /// Max loop iterations per loop (default: 100)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_loop_iterations: Option<u32>,

    /// Max builtin API calls per execution (default: 500)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_count: Option<u32>,

    /// Max value stack depth (default: 128)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_stack_depth: Option<usize>,

    /// Max string length from concatenation (default: 8192)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_string_len: Option<usize>,

    /// Max number of elements in a List value (default: 1024)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_list_len: Option<usize>,

    /// Max number of entries in a Map value (default: 512)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_map_size: Option<usize>,

    /// Error handling policy (default: ignore = fail-open)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_policy: Option<DslErrorPolicy>,

    /// Max timeout for http.call in milliseconds (default: 5000, hard ceiling)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_max_timeout_ms: Option<u64>,

    /// Max response body size for http.call in bytes (default: 1048576 = 1 MB)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_max_response_body_bytes: Option<usize>,

    /// When true, http.call rejects URLs whose host is a private/loopback IP literal
    /// (covers 127.x, 10.x, 172.16-31.x, 192.168.x, 169.254.x, ::1, fc00::/7, fe80::/10).
    /// Hostname-based SSRF still requires network-level egress controls. (default: false)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_block_private_ips: Option<bool>,

    /// LinkSys resources accessible to `redis.*` builtins in this script.
    /// Each entry's `name` is the alias used as the first argument to redis.* calls.
    /// Names must be unique within this list; allowed chars: a-z 0-9 - _
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "linkSysRefs")]
    pub linksys_refs: Vec<LinkSysRef>,
}

impl DslConfig {
    /// Validate linkSysRefs: names must be unique and contain only [a-z0-9_-].
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for r in &self.linksys_refs {
            if r.name.is_empty() {
                return Err("linkSysRefs: name must not be empty".to_string());
            }
            if !r
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                return Err(format!("linkSysRefs: invalid name '{}' (allowed: a-z 0-9 - _)", r.name));
            }
            if !seen.insert(r.name.as_str()) {
                return Err(format!("linkSysRefs: duplicate name '{}'", r.name));
            }
        }
        Ok(())
    }
}

impl PluginValidation for DslConfig {
    /// Schema-level (light) validation. Field-range and linkSysRefs checks only.
    ///
    /// The heavy data-plane validation (compiling `source` / deserializing
    /// `bytecode` / resolving linksys_refs against redis links) requires the
    /// gateway DSL VM and therefore runs gateway-side during plugin-runtime
    /// construction (`PluginRuntime::add_from_*` invokes `DslPlugin::validate_config`,
    /// which fails closed via `seal_fail_closed`). Keeping the VM out of this
    /// schema layer is what lets `types` stay a leaf with no engine dependency.
    fn validation_error(&self) -> Option<String> {
        if self.name.is_empty() {
            return Some("dsl plugin name is required".to_string());
        }
        if self.source.is_none() && self.bytecode.is_none() {
            return Some("either source or bytecode must be provided".to_string());
        }
        if let Some(steps) = self.max_steps {
            if steps == 0 || steps > 1_000_000 {
                return Some("maxSteps must be between 1 and 1000000".to_string());
            }
        }
        if let Some(loop_iter) = self.max_loop_iterations {
            if loop_iter == 0 || loop_iter > 100_000 {
                return Some("maxLoopIterations must be between 1 and 100000".to_string());
            }
        }
        if let Some(calls) = self.max_call_count {
            if calls == 0 || calls > 100_000 {
                return Some("maxCallCount must be between 1 and 100000".to_string());
            }
        }
        if let Some(depth) = self.max_stack_depth {
            if depth == 0 || depth > 1024 {
                return Some("maxStackDepth must be between 1 and 1024".to_string());
            }
        }
        if let Some(len) = self.max_string_len {
            if len == 0 || len > 1_048_576 {
                return Some("maxStringLen must be between 1 and 1048576".to_string());
            }
        }
        if let Some(DslErrorPolicy::DenyWith { status, .. }) = &self.error_policy {
            if *status < 100 || *status > 599 {
                return Some("errorPolicy denyWith status must be between 100 and 599".to_string());
            }
        }
        // NOTE: linkSysRefs validation (`self.validate()`) is intentionally NOT
        // run here. The gateway heavy check (`DslPlugin::validate_config`, called
        // from `PluginRuntime::add_from_*`) compiles `source`/`bytecode` first and
        // then runs `config.validate()` — preserving the original error-priority
        // ordering (compile error wins over a linkSysRefs error on a dual-fault
        // config). Running it here would flip that priority.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_passes_light_checks_for_valid_config() {
        let config = DslConfig {
            name: "ok".to_string(),
            source: Some("let x = 1".to_string()),
            bytecode: None,
            ..Default::default()
        };
        // Light checks only — name set + source present + valid ranges.
        assert_eq!(config.validation_error(), None);
    }

    #[test]
    fn validation_error_requires_name() {
        let config = DslConfig {
            name: String::new(),
            source: Some("let x = 1".to_string()),
            ..Default::default()
        };
        assert!(config.validation_error().is_some());
    }

    #[test]
    fn validation_error_requires_source_or_bytecode() {
        let config = DslConfig {
            name: "x".to_string(),
            source: None,
            bytecode: None,
            ..Default::default()
        };
        assert!(config.validation_error().is_some());
    }

    #[test]
    fn validation_error_rejects_out_of_range_deny_status() {
        let config = DslConfig {
            name: "x".to_string(),
            source: Some("let x = 1".to_string()),
            error_policy: Some(DslErrorPolicy::DenyWith { status: 99, body: None }),
            ..Default::default()
        };
        assert!(config.validation_error().is_some());
    }
}
