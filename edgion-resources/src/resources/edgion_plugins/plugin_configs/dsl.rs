//! DSL plugin configuration (schema)
//!
//! This module holds the serializable EdgionDSL plugin configuration. The
//! compiled DSL VM / runtime (`DslPlugin`, bytecode execution) lives gateway-side
//! in the gateway DSL module; this schema type carries no engine dependency so
//! it can live in the `types` leaf layer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::rate_limit_redis::RateLimitAlgorithm;
use crate::duration::parse_duration;
use crate::redis_rate_limit::validate_rate_limit_policy;
use edgion_dsl::bytecode::BuiltinId;

use crate::resources::edgion_plugins::plugin_configs::LinkSysRef;
use crate::resources::edgion_plugins::PluginValidation;

/// A load-time Redis policy selected by `redis.rate_limit(link, policy, key, cost)`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DslRateLimitPolicy {
    pub name: String,
    pub link: String,
    /// Explicit counter-sharing namespace; use distinct values for independent rules.
    pub namespace: String,
    #[serde(default)]
    pub algorithm: RateLimitAlgorithm,
    pub rate: u64,
    pub interval: String,
    /// Aggregate timeout, including the shared NOSCRIPT fallback.
    #[serde(default = "default_rate_limit_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_rate_limit_timeout_ms() -> u64 {
    500
}

impl DslRateLimitPolicy {
    /// Reused by resource validation and Gateway policy construction.
    pub fn validate(&self) -> Result<i64, String> {
        for (field, value) in [("name", &self.name), ("namespace", &self.namespace)] {
            if value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
            {
                return Err(format!(
                    "rateLimitPolicies.{field} must contain 1..128 ASCII letters, digits, or -_.:/"
                ));
            }
        }
        if !(1..=30_000).contains(&self.timeout_ms) {
            return Err("rateLimitPolicies.timeoutMs must be between 1 and 30000".into());
        }
        let window = parse_duration(&self.interval).map_err(|e| format!("rateLimitPolicies.interval: {e}"))?;
        validate_rate_limit_policy(self.rate, window).map_err(|e| format!("rateLimitPolicies: {e}"))
    }
}

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
    DenyWith {
        #[schemars(range(min = 200, max = 599))]
        status: u16,
        body: Option<String>,
    },
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
    /// Script name (for logging and identification), limited to 1..=36 UTF-8 bytes.
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

    /// Maximum cumulative full-body scan passes per execution (default: 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_scan_passes: Option<u8>,

    /// Error handling policy (default: ignore = fail-open)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_policy: Option<DslErrorPolicy>,

    /// Max timeout for http.call in milliseconds (default: 5000, hard ceiling)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_max_timeout_ms: Option<u64>,

    /// Max response body size for http.call in bytes (default: 1048576 = 1 MB)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_max_response_body_bytes: Option<usize>,

    /// Max request body size for http.call in bytes (default: 1048576 = 1 MiB)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_max_request_body_bytes: Option<usize>,

    /// When true, http.call rejects private/loopback IP literals and addresses
    /// resolved for hostnames at dial time. (default: false)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_block_private_ips: Option<bool>,

    /// LinkSys resources accessible to `redis.*` builtins in this script.
    /// Each entry's `name` is the alias used as the first argument to redis.* calls.
    /// Names must be unique within this list; allowed chars: a-z 0-9 - _
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "linkSysRefs")]
    pub linksys_refs: Vec<LinkSysRef>,
    /// Named atomic Redis rate-limit policies, resolved once at plugin construction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rate_limit_policies: Vec<DslRateLimitPolicy>,
}

impl DslConfig {
    /// Maximum UTF-8 byte length accepted for the plugin name.
    pub const MAX_NAME_BYTES: usize = 36;

    fn validate_name(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("dsl plugin name is required".to_string());
        }
        if self.name.len() > Self::MAX_NAME_BYTES {
            return Err(format!(
                "dsl plugin name must be at most {} UTF-8 bytes",
                Self::MAX_NAME_BYTES
            ));
        }
        Ok(())
    }

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
        if self.rate_limit_policies.len() > 64 {
            return Err("rateLimitPolicies supports at most 64 policies".into());
        }
        let mut policies = std::collections::HashSet::new();
        for policy in &self.rate_limit_policies {
            policy.validate()?;
            if !seen.contains(policy.link.as_str()) {
                return Err(format!(
                    "rateLimitPolicies link '{}' is not in linkSysRefs",
                    policy.link
                ));
            }
            if !policies.insert(policy.name.as_str()) {
                return Err(format!("rateLimitPolicies duplicate name '{}'", policy.name));
            }
        }
        Ok(())
    }

    /// Full static validation — the exact set of failures the gateway's
    /// `DslPlugin` constructor would hit, minus VM init (`Vm::new`), which is
    /// the one failure that cannot be predicted statically and is covered by
    /// the data-plane seal backstop.
    ///
    /// The name is checked first because it must fit the runtime log identity.
    /// For valid names, check order mirrors the original gateway
    /// `validate_config`: resolve the script (compile `source` /
    /// deserialize+validate `bytecode`) first, then the linkSysRefs format
    /// check, then the redis link-name scan — so a dual-fault config reports
    /// the compile error. Returns the validated `CompiledScript` so data-plane
    /// callers can reuse it without paying for a second compile/deserialize
    /// cycle.
    pub fn compile_and_validate(&self) -> Result<edgion_dsl::bytecode::CompiledScript, String> {
        self.validate_name()?;
        let val_limits = edgion_dsl::validator::ValidationLimits {
            max_map_size: self.max_map_size.unwrap_or(512),
            ..Default::default()
        };
        let script = edgion_dsl::resolve_script(self.source.as_deref(), self.bytecode.as_deref(), &val_limits)?;
        if script.references_builtin(BuiltinId::RespBody) {
            return Err(
                "resp.body is not supported because proxied upstream response bodies remain streaming-only".to_string(),
            );
        }
        self.validate()?;
        let valid_names: std::collections::HashSet<&str> = self.linksys_refs.iter().map(|r| r.name.as_str()).collect();
        edgion_dsl::check_redis_link_names(&script, &valid_names)?;
        Ok(script)
    }
}

#[cfg(test)]
mod rate_limit_tests {
    use super::*;

    fn config_value() -> serde_json::Value {
        serde_json::json!({
            "name": "limited", "source": "redis.rate_limit(\"cache\", \"requests\", \"key\", 1)",
            "linkSysRefs": [{"name": "cache", "namespace": "default"}],
            "rateLimitPolicies": [{"name": "requests", "link": "cache", "namespace": "api/requests",
                "rate": 100, "interval": "1s", "algorithm": "FixedWindow"}]
        })
    }

    #[test]
    fn redis_rate_limit_local_config_roundtrip_and_boundaries() {
        let config: DslConfig = serde_json::from_value(config_value()).unwrap();
        assert_eq!(config.rate_limit_policies[0].timeout_ms, 500);
        let script = config.compile_and_validate().unwrap();
        let mut transported: DslConfig = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        transported.source = None;
        transported.bytecode = Some(script.serialize_base64().unwrap());
        assert!(transported.compile_and_validate().is_ok());
        for (field, value) in [
            ("rate", serde_json::json!(0)),
            ("rate", serde_json::json!(1_000_001)),
            ("interval", serde_json::json!("0s")),
            ("interval", serde_json::json!("2147483648ms")),
            ("timeoutMs", serde_json::json!(0)),
            ("timeoutMs", serde_json::json!(30001)),
            ("namespace", serde_json::json!("")),
            ("namespace", serde_json::json!("{hash-tag}")),
            ("link", serde_json::json!("missing")),
            ("name", serde_json::json!("")),
        ] {
            let mut input = config_value();
            input["rateLimitPolicies"][0][field] = value;
            let invalid: DslConfig = serde_json::from_value(input).unwrap();
            assert!(invalid.compile_and_validate().is_err(), "accepted invalid {field}");
        }
        let mut duplicate = config.clone();
        duplicate
            .rate_limit_policies
            .push(config.rate_limit_policies[0].clone());
        assert!(duplicate.validate().unwrap_err().contains("duplicate"));
        let mut unknown_field = config_value();
        unknown_field["rateLimitPolicies"][0]["timoutMs"] = serde_json::json!(5);
        assert!(serde_json::from_value::<DslConfig>(unknown_field).is_err());
    }
}

impl PluginValidation for DslConfig {
    /// Full preparse validation: light field/range checks first, then the deep
    /// static checks (compile `source` / deserialize+validate `bytecode`,
    /// linkSysRefs format, redis link-name scan) via
    /// [`DslConfig::compile_and_validate`] and the shared `edgion-dsl` crate.
    /// Control plane and data plane therefore report identical DSL errors. VM
    /// init (`Vm::new`) is the one remaining gateway-only failure — it cannot
    /// be predicted statically and is covered by the data-plane seal backstop.
    fn validation_error(&self) -> Option<String> {
        if let Err(error) = self.validate_name() {
            return Some(error);
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
        if let Some(passes) = self.max_body_scan_passes {
            if !(1..=16).contains(&passes) {
                return Some("maxBodyScanPasses must be between 1 and 16".to_string());
            }
        }
        if let Some(bytes) = self.http_max_request_body_bytes {
            if !(1..=33_554_432).contains(&bytes) {
                return Some("httpMaxRequestBodyBytes must be between 1 and 33554432".to_string());
            }
        }
        if let Some(DslErrorPolicy::DenyWith { status, .. }) = &self.error_policy {
            if !crate::http_status::is_valid_final_http_status(*status) {
                return Some("errorPolicy denyWith status must be between 200 and 599".to_string());
            }
        }
        // Deep static checks last. `compile_and_validate` preserves the
        // gateway error priority (compile error wins over a linkSysRefs error
        // on a dual-fault config), so running it after the light checks keeps
        // the combined ordering identical to the gateway's.
        self.compile_and_validate().err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_passes_for_valid_config() {
        let config = DslConfig {
            name: "ok".to_string(),
            source: Some("let x = 1".to_string()),
            bytecode: None,
            ..Default::default()
        };
        // Full pipeline — the trivial script compiles, so no error.
        assert_eq!(config.validation_error(), None);
    }

    #[test]
    fn validation_error_requires_name() {
        let config = DslConfig {
            name: String::new(),
            source: Some("let x = 1".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.validation_error(),
            Some("dsl plugin name is required".to_string())
        );
        assert_eq!(
            config.compile_and_validate().unwrap_err(),
            "dsl plugin name is required"
        );
    }

    #[test]
    fn plugin_name_uses_utf8_byte_boundaries() {
        for name in ["a".repeat(36), "é".repeat(18)] {
            let config = DslConfig {
                name,
                source: Some("let x = 1".to_string()),
                ..Default::default()
            };
            assert_eq!(config.validation_error(), None);
            assert!(config.compile_and_validate().is_ok());
        }

        for name in ["a".repeat(37), "é".repeat(19)] {
            let config = DslConfig {
                name,
                source: Some("let x = 1".to_string()),
                ..Default::default()
            };
            let expected = Some("dsl plugin name must be at most 36 UTF-8 bytes".to_string());
            assert_eq!(config.validation_error(), expected);
            assert_eq!(
                config.compile_and_validate().unwrap_err(),
                "dsl plugin name must be at most 36 UTF-8 bytes"
            );
        }
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
        for status in [99, 103, 600] {
            let config = DslConfig {
                name: "x".to_string(),
                source: Some("let x = 1".to_string()),
                error_policy: Some(DslErrorPolicy::DenyWith { status, body: None }),
                ..Default::default()
            };
            assert!(config.validation_error().is_some(), "{status}");
        }
    }

    #[test]
    fn validation_error_rejects_out_of_range_body_scan_passes() {
        let config = DslConfig {
            name: "scan-budget".to_string(),
            source: Some("return next".to_string()),
            max_body_scan_passes: Some(0),
            ..Default::default()
        };
        assert_eq!(
            config.validation_error(),
            Some("maxBodyScanPasses must be between 1 and 16".to_string())
        );

        let config = DslConfig {
            max_body_scan_passes: Some(17),
            ..config
        };
        assert_eq!(
            config.validation_error(),
            Some("maxBodyScanPasses must be between 1 and 16".to_string())
        );
    }

    #[test]
    fn validation_error_checks_http_request_body_limit_boundaries() {
        for bytes in [1, 33_554_432] {
            let config = DslConfig {
                name: "request-body-limit".to_string(),
                source: Some("return next()".to_string()),
                http_max_request_body_bytes: Some(bytes),
                ..Default::default()
            };
            assert_eq!(config.validation_error(), None);
        }

        for bytes in [0, 33_554_433] {
            let config = DslConfig {
                name: "request-body-limit".to_string(),
                source: Some("return next()".to_string()),
                http_max_request_body_bytes: Some(bytes),
                ..Default::default()
            };
            assert_eq!(
                config.validation_error(),
                Some("httpMaxRequestBodyBytes must be between 1 and 33554432".to_string())
            );
        }
    }

    #[test]
    fn validation_error_rejects_uncompilable_source() {
        let config = DslConfig {
            name: "bad".to_string(),
            source: Some("this is not valid dsl @@@".to_string()),
            ..Default::default()
        };
        let err = config.validation_error().expect("uncompilable source must fail");
        assert!(err.contains("DSL compile errors"), "got: {err}");
    }

    #[test]
    fn validation_error_rejects_unknown_redis_link() {
        let config = DslConfig {
            name: "unknown-link".to_string(),
            source: Some("let r = redis.get(\"mylink\", \"mykey\")".to_string()),
            ..Default::default()
        };
        let err = config
            .validation_error()
            .expect("redis call referencing an undeclared link must fail");
        assert!(err.contains("linkSysRefs"), "got: {err}");
    }

    #[test]
    fn validation_error_rejects_streaming_upstream_response_body_read() {
        let config = DslConfig {
            name: "response-body".to_string(),
            source: Some("let body = resp.body".to_string()),
            ..Default::default()
        };
        let err = config
            .validation_error()
            .expect("proxied upstream response bodies must remain streaming-only");
        assert!(err.contains("streaming-only"), "got: {err}");
    }

    #[test]
    fn compile_and_validate_compile_error_wins_on_dual_fault() {
        use crate::resources::edgion_plugins::plugin_configs::LinkSysRef;
        // Both faults present: uncompilable source AND an invalid linkSysRefs
        // name. The compile error must win (original gateway error priority).
        let config = DslConfig {
            name: "dual".to_string(),
            source: Some("this is not valid dsl @@@".to_string()),
            linksys_refs: vec![LinkSysRef {
                namespace: "default".to_string(),
                name: "BAD NAME".to_string(),
            }],
            ..Default::default()
        };
        let err = config.compile_and_validate().expect_err("dual fault must fail");
        assert!(err.contains("DSL compile errors"), "compile error must win, got: {err}");
    }

    #[test]
    fn compile_and_validate_reports_link_format_error_when_script_compiles() {
        use crate::resources::edgion_plugins::plugin_configs::LinkSysRef;
        let config = DslConfig {
            name: "bad-link-name".to_string(),
            source: Some("let a = 1".to_string()),
            linksys_refs: vec![LinkSysRef {
                namespace: "default".to_string(),
                name: "BAD NAME".to_string(),
            }],
            ..Default::default()
        };
        let err = config.compile_and_validate().expect_err("invalid link name must fail");
        assert!(err.contains("invalid name"), "got: {err}");
    }
}
