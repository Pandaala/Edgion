use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::KeyGet;

use super::upstream_fallback::{default_upstream_fallback, default_upstream_reject, UpstreamFallback};

/// DirectEndpoint plugin configuration
///
/// Routes the request to a specific endpoint IP extracted from request metadata,
/// bypassing normal load balancing. The endpoint must be a valid backend address
/// belonging to one of the route's backend_refs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DirectEndpointConfig {
    /// Source to extract the target endpoint value from.
    ///
    /// Typical sources:
    /// - Header: `{ type: header, name: "X-Target-Endpoint" }`
    /// - Query:  `{ type: query, name: "target_endpoint" }`
    /// - Ctx:    `{ type: ctx, name: "resolved_endpoint" }`
    /// - Cookie: `{ type: cookie, name: "pinned_endpoint" }`
    pub from: KeyGet,

    /// Optional regex to extract the endpoint IP from the raw value.
    ///
    /// Example: if `from` yields "pod-abc_10.0.1.5:8080_zone-a",
    /// use regex `(\d+\.\d+\.\d+\.\d+)` with group 1 to extract "10.0.1.5".
    ///
    /// If not set, the raw value is used directly as the endpoint address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<EndpointExtract>,

    /// Port to use when connecting to the direct endpoint.
    ///
    /// Priority:
    /// 1. Port extracted from the value (if value contains "ip:port" format)
    /// 2. This `port` field
    /// 3. The serving port mapped from the matched backend_ref's Service port
    ///
    /// The route must declare a valid Service port even when an explicit endpoint
    /// port is supplied. Explicit ports must belong to that Service section;
    /// an omitted port requires a unique serving port for the selected IP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
    pub port: Option<u16>,

    /// Behavior when the endpoint value is missing from the request.
    ///
    /// - `fallback` (default): proceed with normal LB selection (no direct routing)
    /// - `reject`: return 400 Bad Request
    #[serde(default = "default_upstream_fallback")]
    pub on_missing: UpstreamFallback,

    /// Behavior when the resolved endpoint fails security validation
    /// (not found in any backend_ref's endpoints).
    ///
    /// - `reject` (default): return 403 Forbidden
    /// - `fallback`: ignore and proceed with normal LB
    #[serde(default = "default_upstream_reject")]
    pub on_invalid: UpstreamFallback,

    /// Whether to inherit the effective BackendTLSPolicy for the proven Service
    /// port section. Invalid policies reject the override; absent policies use HTTP.
    ///
    /// Default: true
    #[serde(default = "default_true")]
    pub inherit_tls: bool,

    // === Compiled regex cache ===
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<regex::Regex>,
}

/// Regex extraction configuration for endpoint value
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndpointExtract {
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

impl Default for DirectEndpointConfig {
    fn default() -> Self {
        Self {
            from: KeyGet::Header {
                name: "X-Target-Endpoint".to_string(),
            },
            extract: None,
            port: None,
            on_missing: default_upstream_fallback(),
            on_invalid: default_upstream_reject(),
            inherit_tls: true,
            compiled_regex: None,
        }
    }
}

impl DirectEndpointConfig {
    /// Pre-compile regex at parse time. Compilation errors are surfaced via the
    /// `PluginValidation` trait's `validation_error()` method instead of being
    /// cached on the config.
    pub fn validate(&mut self) {
        if let Some(ref extract) = self.extract {
            if let Ok(re) = regex::Regex::new(&extract.regex) {
                self.compiled_regex = Some(re);
            }
        }
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for DirectEndpointConfig {
    fn validation_error(&self) -> Option<String> {
        if let Some(ref extract) = self.extract {
            if let Err(e) = regex::Regex::new(&extract.regex) {
                return Some(format!("Invalid extract regex: {}", e));
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/direct_endpoint_test/validation_trait_tests.rs"]
mod validation_trait_tests;
