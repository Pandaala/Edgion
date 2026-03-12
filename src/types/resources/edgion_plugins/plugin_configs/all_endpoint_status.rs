use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Maximum allowed per-endpoint timeout: 5 seconds
pub const MAX_TIMEOUT_MS: u64 = 5000;
/// Maximum allowed wall-clock timeout for the entire fan-out: 10 seconds
pub const MAX_WALL_TIMEOUT_MS: u64 = 10_000;
/// Maximum allowed endpoints to query (hard ceiling)
pub const MAX_ENDPOINTS_LIMIT: usize = 50;
/// Maximum allowed response body size per endpoint: 16KB
pub const MAX_BODY_SIZE_LIMIT: usize = 16 * 1024;
/// Maximum concurrent plugin executions across the entire process
pub const MAX_GLOBAL_CONCURRENCY: usize = 3;

/// AllEndpointStatus plugin configuration.
///
/// Queries all backend endpoints for the current route and returns
/// an aggregated JSON response with each endpoint's status, latency,
/// and response body. Designed for health checks and deployment verification.
///
/// Security ceilings are enforced at runtime via `min()` clamping —
/// user-configured values that exceed hard limits are silently capped.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AllEndpointStatusConfig {
    /// Per-endpoint request timeout in milliseconds.
    /// Default: 2000 (2 seconds). Maximum: 5000 (5 seconds).
    /// Enforced ceiling for security — prevents slow backends from blocking.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// Wall-clock timeout for the entire fan-out operation in milliseconds.
    /// Default: 10000 (10 seconds). Maximum: 10000 (10 seconds).
    /// Even if individual endpoints haven't timed out, the whole operation
    /// will be cancelled when this deadline is reached. Remaining endpoints
    /// are reported as "wall timeout exceeded".
    #[serde(default = "default_wall_timeout_ms")]
    pub wall_timeout_ms: u64,

    /// Maximum number of endpoints to query.
    /// Default: 20. Maximum: 50.
    /// Enforced ceiling for security — prevents large fan-out attacks.
    /// Most services have fewer than 20 pods; raise only when needed.
    #[serde(default = "default_max_endpoints")]
    pub max_endpoints: usize,

    /// Maximum response body size per endpoint in bytes.
    /// Default: 16384 (16KB). Maximum: 16384 (16KB).
    /// Body is read via streaming — only the first max_body_size bytes are
    /// fetched, the rest is discarded immediately (no full-body buffering).
    /// Enforced ceiling for security — prevents memory exhaustion.
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,

    /// Maximum number of concurrent requests to endpoints.
    /// Default: 10. Controls the fan-out parallelism.
    #[serde(default = "default_concurrency_limit")]
    pub concurrency_limit: usize,

    /// Whether to include response headers in the aggregated result.
    /// Default: false. Set to true for detailed debugging.
    #[serde(default)]
    pub include_response_headers: bool,

    /// Override the HTTP method for endpoint requests.
    /// Default: None (uses the original request method).
    /// Common override: "GET" for health checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method_override: Option<String>,

    /// Override the request path for endpoint requests.
    /// Default: None (uses the original request path + query).
    /// Common override: "/health" or "/healthz".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_override: Option<String>,

    // === Validation cache ===
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

fn default_timeout_ms() -> u64 {
    2000
}
fn default_wall_timeout_ms() -> u64 {
    10_000
}
fn default_max_endpoints() -> usize {
    20
}
fn default_max_body_size() -> usize {
    16 * 1024
}
fn default_concurrency_limit() -> usize {
    10
}

impl Default for AllEndpointStatusConfig {
    fn default() -> Self {
        Self {
            timeout_ms: default_timeout_ms(),
            wall_timeout_ms: default_wall_timeout_ms(),
            max_endpoints: default_max_endpoints(),
            max_body_size: default_max_body_size(),
            concurrency_limit: default_concurrency_limit(),
            include_response_headers: false,
            method_override: None,
            path_override: None,
            validation_error: None,
        }
    }
}

impl AllEndpointStatusConfig {
    /// Return validation error if config is invalid.
    /// Called during preparse for status reporting.
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    /// Clamp timeout_ms to security ceiling.
    pub fn effective_timeout_ms(&self) -> u64 {
        self.timeout_ms.min(MAX_TIMEOUT_MS)
    }

    /// Clamp wall_timeout_ms to security ceiling.
    pub fn effective_wall_timeout_ms(&self) -> u64 {
        self.wall_timeout_ms.min(MAX_WALL_TIMEOUT_MS)
    }

    /// Clamp max_endpoints using three-layer min: plugin config, global TOML, hard cap.
    pub fn effective_max_endpoints(&self, global_max: usize) -> usize {
        self.max_endpoints.min(global_max).min(MAX_ENDPOINTS_LIMIT)
    }

    /// Clamp max_body_size to security ceiling.
    pub fn effective_max_body_size(&self) -> usize {
        self.max_body_size.min(MAX_BODY_SIZE_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let config = AllEndpointStatusConfig::default();
        assert_eq!(config.timeout_ms, 2000);
        assert_eq!(config.wall_timeout_ms, 10_000);
        assert_eq!(config.max_endpoints, 20);
        assert_eq!(config.max_body_size, 16 * 1024);
        assert_eq!(config.concurrency_limit, 10);
        assert!(!config.include_response_headers);
        assert!(config.method_override.is_none());
        assert!(config.path_override.is_none());
    }

    #[test]
    fn test_timeout_clamped_to_5s() {
        let config = AllEndpointStatusConfig {
            timeout_ms: 10000,
            ..Default::default()
        };
        assert_eq!(config.effective_timeout_ms(), 5000);
    }

    #[test]
    fn test_wall_timeout_clamped_to_10s() {
        let config = AllEndpointStatusConfig {
            wall_timeout_ms: 30000,
            ..Default::default()
        };
        assert_eq!(config.effective_wall_timeout_ms(), 10000);
    }

    #[test]
    fn test_max_endpoints_clamped_to_hard_cap() {
        let config = AllEndpointStatusConfig {
            max_endpoints: 500,
            ..Default::default()
        };
        // global_max = 100, hard cap = 50 → min(500, 100, 50) = 50
        assert_eq!(config.effective_max_endpoints(100), 50);
    }

    #[test]
    fn test_max_endpoints_clamped_to_global() {
        let config = AllEndpointStatusConfig {
            max_endpoints: 40,
            ..Default::default()
        };
        // global_max = 20, hard cap = 50 → min(40, 20, 50) = 20
        assert_eq!(config.effective_max_endpoints(20), 20);
    }

    #[test]
    fn test_max_endpoints_plugin_value_lowest() {
        let config = AllEndpointStatusConfig {
            max_endpoints: 15,
            ..Default::default()
        };
        // global_max = 30, hard cap = 50 → min(15, 30, 50) = 15
        assert_eq!(config.effective_max_endpoints(30), 15);
    }

    #[test]
    fn test_max_body_size_clamped() {
        let config = AllEndpointStatusConfig {
            max_body_size: 1024 * 1024,
            ..Default::default()
        }; // 1MB
        assert_eq!(config.effective_max_body_size(), 16 * 1024);
    }
}
