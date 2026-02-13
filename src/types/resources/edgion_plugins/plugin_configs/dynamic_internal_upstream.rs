use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::common::KeyGet;

/// DynamicInternalUpstream plugin configuration
///
/// Routes the request to a specific BackendRef selected by request metadata,
/// bypassing weighted round-robin backend selection. The target BackendRef
/// must exist in the route's backend_refs list.
///
/// Unlike DirectEndpoint which targets a specific endpoint IP, DynamicInternalUpstream
/// targets a specific BackendRef (Service), and normal load balancing still
/// applies within that service's endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DynamicInternalUpstreamConfig {
    /// Source to extract the routing key from.
    ///
    /// Typical sources:
    /// - Header: `{ type: header, name: "X-Backend-Target" }`
    /// - Query:  `{ type: query, name: "backend" }`
    /// - Ctx:    `{ type: ctx, name: "target_backend" }`
    /// - Cookie: `{ type: cookie, name: "backend_group" }`
    pub from: KeyGet,

    /// Optional regex to extract the routing key from the raw value.
    ///
    /// Example: if `from` yields "group=premium;region=us",
    /// use regex `group=(\w+)` with group 1 to extract "premium".
    ///
    /// If not set, the raw value is used directly as the routing key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<DynUpstreamExtract>,

    /// Explicit mapping rules from routing key values to backend_ref targets.
    ///
    /// - If provided: the routing key is matched against rules (first match wins)
    /// - If omitted/empty: "direct mode" — the routing key is used directly as
    ///   the backend_ref name to look up in the route's backend_refs
    ///
    /// Direct mode is useful when combined with CtxSet plugin for pre-computation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<DynUpstreamRule>>,

    /// Behavior when the routing key is missing from the request.
    ///
    /// - `fallback` (default): proceed with normal weighted backend selection
    /// - `reject`: return 400 Bad Request
    #[serde(default)]
    pub on_missing: DynUpstreamOnMissing,

    /// Behavior when no rule matches the routing key (rules mode only).
    /// In direct mode, this is not applicable (see on_invalid instead).
    ///
    /// - `fallback` (default): proceed with normal weighted backend selection
    /// - `reject`: return 400 Bad Request
    #[serde(default)]
    pub on_no_match: DynUpstreamOnNoMatch,

    /// Behavior when the resolved backend_ref name is not found
    /// in the route's backend_refs list.
    ///
    /// - `reject` (default): return 403 Forbidden (potential misconfiguration)
    /// - `fallback`: proceed with normal weighted backend selection
    #[serde(default)]
    pub on_invalid: DynUpstreamOnInvalid,

    /// Whether to set `X-Dynamic-Internal-Upstream` request header (sent to upstream)
    /// indicating which backend was targeted. Useful for end-to-end debugging.
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

/// Rule mapping a routing key value to a target backend_ref
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DynUpstreamRule {
    /// Value to match against the routing key (exact match)
    pub value: String,

    /// Target backend_ref identification
    pub backend_ref: DynUpstreamTarget,
}

/// Target backend_ref identification
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DynUpstreamTarget {
    /// Name of the target backend_ref (must match a backend_ref in the route)
    pub name: String,

    /// Optional namespace of the target backend_ref.
    /// If not specified, matches by name only.
    /// If specified, both name and namespace must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// Regex extraction configuration for routing key
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DynUpstreamExtract {
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
pub enum DynUpstreamOnMissing {
    /// Proceed with normal weighted backend selection (default)
    #[default]
    Fallback,
    /// Return 400 Bad Request
    Reject,
}

/// Behavior when no rule matches the routing key
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum DynUpstreamOnNoMatch {
    /// Proceed with normal weighted backend selection (default)
    #[default]
    Fallback,
    /// Return 400 Bad Request
    Reject,
}

/// Behavior when resolved backend_ref is invalid
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum DynUpstreamOnInvalid {
    /// Return 403 Forbidden (default — indicates misconfiguration)
    #[default]
    Reject,
    /// Proceed with normal weighted backend selection
    Fallback,
}

fn default_group() -> usize {
    1
}

impl Default for DynamicInternalUpstreamConfig {
    fn default() -> Self {
        Self {
            from: KeyGet::Header {
                name: "X-Backend-Target".to_string(),
            },
            extract: None,
            rules: None,
            on_missing: DynUpstreamOnMissing::default(),
            on_no_match: DynUpstreamOnNoMatch::default(),
            on_invalid: DynUpstreamOnInvalid::default(),
            debug_header: false,
            validation_error: None,
            compiled_regex: None,
        }
    }
}

impl DynamicInternalUpstreamConfig {
    /// Validate and pre-compile regex at parse time
    pub fn validate(&mut self) {
        if let Some(ref extract) = self.extract {
            match regex::Regex::new(&extract.regex) {
                Ok(re) => self.compiled_regex = Some(re),
                Err(e) => {
                    self.validation_error = Some(format!("Invalid extract regex: {}", e));
                    return;
                }
            }
        }

        // Validate rules if provided
        if let Some(ref rules) = self.rules {
            if rules.is_empty() {
                self.validation_error = Some("Rules list is empty; omit 'rules' for direct mode".to_string());
                return;
            }
            for (i, rule) in rules.iter().enumerate() {
                if rule.value.is_empty() {
                    self.validation_error = Some(format!("Rule[{}]: value cannot be empty", i));
                    return;
                }
                if rule.backend_ref.name.is_empty() {
                    self.validation_error = Some(format!("Rule[{}]: backendRef.name cannot be empty", i));
                    return;
                }
            }
        }
    }

    /// Return validation error if config is invalid.
    /// Called during preparse for status reporting.
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    /// Check if this config uses direct mode (no rules)
    pub fn is_direct_mode(&self) -> bool {
        self.rules.is_none()
    }

    /// Resolve routing key to target backend_ref name and optional namespace.
    ///
    /// In direct mode: routing_key is used directly as backend_ref name.
    /// In rules mode: first matching rule's backend_ref is returned.
    ///
    /// Returns None if no rule matches (rules mode only).
    pub fn resolve_target<'a>(&'a self, routing_key: &'a str) -> Option<(&'a str, Option<&'a str>)> {
        match &self.rules {
            None => {
                // Direct mode: routing key IS the backend_ref name
                Some((routing_key, None))
            }
            Some(rules) => {
                // Rules mode: find first matching rule
                for rule in rules {
                    if rule.value == routing_key {
                        return Some((&rule.backend_ref.name, rule.backend_ref.namespace.as_deref()));
                    }
                }
                None
            }
        }
    }
}
