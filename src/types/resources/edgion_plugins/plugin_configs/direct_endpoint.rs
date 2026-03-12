use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::common::KeyGet;

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
    /// 3. The matched backend_ref's port
    ///
    /// If none of the above are available, the plugin returns an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// Behavior when the endpoint value is missing from the request.
    ///
    /// - `fallback` (default): proceed with normal LB selection (no direct routing)
    /// - `reject`: return 400 Bad Request
    #[serde(default)]
    pub on_missing: DirectEndpointOnMissing,

    /// Behavior when the resolved endpoint fails security validation
    /// (not found in any backend_ref's endpoints).
    ///
    /// - `reject` (default): return 403 Forbidden
    /// - `fallback`: ignore and proceed with normal LB
    #[serde(default)]
    pub on_invalid: DirectEndpointOnInvalid,

    /// Whether to inherit TLS configuration from the matched backend_ref's
    /// BackendTLSPolicy when connecting to the direct endpoint.
    ///
    /// Default: true
    #[serde(default = "default_true")]
    pub inherit_tls: bool,

    /// Whether to set `X-Direct-Endpoint` request header (sent to upstream)
    /// indicating which endpoint was targeted. Useful for end-to-end debugging.
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

/// Behavior when endpoint value is missing
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum DirectEndpointOnMissing {
    /// Proceed with normal load balancing (default)
    #[default]
    Fallback,
    /// Return 400 Bad Request
    Reject,
}

/// Behavior when endpoint fails validation
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum DirectEndpointOnInvalid {
    /// Return 403 Forbidden (default)
    #[default]
    Reject,
    /// Proceed with normal load balancing
    Fallback,
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
            on_missing: DirectEndpointOnMissing::default(),
            on_invalid: DirectEndpointOnInvalid::default(),
            inherit_tls: true,
            debug_header: false,
            validation_error: None,
            compiled_regex: None,
        }
    }
}

impl DirectEndpointConfig {
    /// Validate and pre-compile regex at parse time
    pub fn validate(&mut self) {
        if let Some(ref extract) = self.extract {
            match regex::Regex::new(&extract.regex) {
                Ok(re) => self.compiled_regex = Some(re),
                Err(e) => {
                    self.validation_error = Some(format!("Invalid extract regex: {}", e));
                }
            }
        }
    }

    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }
}
