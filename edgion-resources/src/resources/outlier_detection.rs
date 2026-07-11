//! Outlier-detection (circuit-breaker) configuration.
//!
//! `OutlierDetectionConfig` is the deserialize/CRD-spec shape: fields whose
//! defaults depend on sibling values are `Option` so `normalize` can apply
//! them; `#[serde(default)]` cannot express cross-field defaults.
//! `CircuitBreakerConfig` is the fully-resolved shape used by the gateway
//! data plane, produced by `OutlierDetectionConfig::normalize`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Resolved outlier-detection config for a service. The internal module and
/// struct retain the `circuit_breaker` name; the user-facing configuration
/// surface is the `EdgionBackendTrafficPolicy` resource's
/// `spec.outlierDetection` section.
#[derive(Debug, Clone, PartialEq)]
pub struct CircuitBreakerConfig {
    /// Consecutive application-5xx (any >=500 that is NOT 502/503/504) before ejection.
    pub consecutive_errors: u32,
    /// Consecutive upstream-origin gateway failures (backend returned 502/503/504) before ejection.
    pub consecutive_gateway_errors: u32,
    /// Consecutive local-origin failures (connect/TLS-handshake/transport-reset/proxy
    /// errors — the data-plane never got a usable HTTP response) before ejection.
    pub consecutive_local_origin_failures: u32,
    /// Base ejection duration in seconds (multiplied by the endpoint's ejection count).
    pub ejection_seconds: u64,
    /// Upper bound on the backoff-scaled ejection duration in seconds.
    pub max_ejection_seconds: u64,
    /// Max percent of a service's endpoints that may be ejected at once (0 disables the cap).
    pub max_ejection_percent: u32,
}

/// Deserialize shape. Fields whose defaults depend on sibling values are
/// `Option` so `normalize` can apply them; `#[serde(default)]` cannot
/// express cross-field defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutlierDetectionConfig {
    #[serde(default = "default_consecutive_errors")]
    pub consecutive_errors: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consecutive_gateway_errors: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consecutive_local_origin_failures: Option<u32>,
    #[serde(default = "default_ejection_seconds")]
    pub ejection_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ejection_seconds: Option<u64>,
    #[serde(default = "default_max_ejection_percent")]
    pub max_ejection_percent: u32,
}

fn default_consecutive_errors() -> u32 {
    5
}
fn default_ejection_seconds() -> u64 {
    30
}
fn default_max_ejection_percent() -> u32 {
    50
}

impl OutlierDetectionConfig {
    /// Resolve field-dependent defaults and clamp invalid combinations.
    pub fn normalize(self) -> CircuitBreakerConfig {
        // Gateway threshold inherits the (possibly tuned) consecutive_errors so
        // old single-field configs keep their aggressiveness on the hard-failure path.
        let consecutive_gateway_errors = self.consecutive_gateway_errors.unwrap_or(self.consecutive_errors);
        // Local-origin threshold inherits the (already-resolved) gateway threshold when
        // unset, so a config that predates this field keeps counting connect/proxy errors
        // exactly as it did when they shared the gateway bucket — behaviour is unchanged.
        let consecutive_local_origin_failures = self
            .consecutive_local_origin_failures
            .unwrap_or(consecutive_gateway_errors);
        // Default cap = max(300, ejection); explicit cap below base is clamped UP, never rejected.
        let max_ejection_seconds = self
            .max_ejection_seconds
            .unwrap_or_else(|| self.ejection_seconds.max(300))
            .max(self.ejection_seconds);
        CircuitBreakerConfig {
            consecutive_errors: self.consecutive_errors,
            consecutive_gateway_errors,
            consecutive_local_origin_failures,
            ejection_seconds: self.ejection_seconds,
            max_ejection_seconds,
            max_ejection_percent: self.max_ejection_percent,
        }
    }
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        OutlierDetectionConfig {
            consecutive_errors: default_consecutive_errors(),
            consecutive_gateway_errors: None,
            consecutive_local_origin_failures: None,
            ejection_seconds: default_ejection_seconds(),
            max_ejection_seconds: None,
            max_ejection_percent: default_max_ejection_percent(),
        }
        .normalize()
    }
}

impl CircuitBreakerConfig {
    pub fn validate(&self) -> Option<String> {
        if self.consecutive_errors == 0 {
            return Some("consecutiveErrors must be >= 1".to_string());
        }
        if self.consecutive_gateway_errors == 0 {
            return Some("consecutiveGatewayErrors must be >= 1".to_string());
        }
        if self.consecutive_local_origin_failures == 0 {
            return Some("consecutiveLocalOriginFailures must be >= 1".to_string());
        }
        if self.ejection_seconds == 0 {
            return Some("ejectionSeconds must be >= 1".to_string());
        }
        if self.max_ejection_percent > 100 {
            return Some("maxEjectionPercent must be 0-100".to_string());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(
        consecutive_errors: u32,
        consecutive_gateway_errors: Option<u32>,
        consecutive_local_origin_failures: Option<u32>,
        ejection_seconds: u64,
        max_ejection_seconds: Option<u64>,
    ) -> OutlierDetectionConfig {
        OutlierDetectionConfig {
            consecutive_errors,
            consecutive_gateway_errors,
            consecutive_local_origin_failures,
            ejection_seconds,
            max_ejection_seconds,
            max_ejection_percent: default_max_ejection_percent(),
        }
    }

    #[test]
    fn gateway_errors_defaults_to_consecutive_errors() {
        let config = config_with(2, None, None, default_ejection_seconds(), None).normalize();
        assert_eq!(config.consecutive_errors, 2);
        assert_eq!(config.consecutive_gateway_errors, 2); // inherits, not literal 5
        assert_eq!(config.consecutive_local_origin_failures, 2); // inherits gateway -> errors
    }

    #[test]
    fn local_origin_failures_defaults_to_gateway_errors() {
        // Only the gateway threshold is set; local-origin inherits it (back-compat:
        // connect/proxy errors keep counting at the same threshold they used when they
        // shared the gateway bucket).
        let config = config_with(
            default_consecutive_errors(),
            Some(4),
            None,
            default_ejection_seconds(),
            None,
        )
        .normalize();
        assert_eq!(config.consecutive_gateway_errors, 4);
        assert_eq!(config.consecutive_local_origin_failures, 4);
    }

    #[test]
    fn local_origin_failures_explicit_overrides_inheritance() {
        let config = config_with(
            default_consecutive_errors(),
            Some(8),
            Some(2),
            default_ejection_seconds(),
            None,
        )
        .normalize();
        assert_eq!(config.consecutive_gateway_errors, 8);
        assert_eq!(config.consecutive_local_origin_failures, 2);
    }

    #[test]
    fn zero_local_origin_failures_rejected() {
        let config = CircuitBreakerConfig {
            consecutive_local_origin_failures: 0,
            ..CircuitBreakerConfig::default()
        };
        assert!(config.validate().is_some());
    }

    #[test]
    fn max_ejection_seconds_defaults_to_max_300_ejection() {
        let config = config_with(default_consecutive_errors(), None, None, 600, None).normalize();
        assert_eq!(config.max_ejection_seconds, 600); // max(300, 600)
    }

    #[test]
    fn max_ejection_seconds_clamps_up_not_rejects() {
        // Explicit max < ejection must be clamped up, NOT cause validation to fail.
        let config = config_with(default_consecutive_errors(), None, None, 60, Some(10)).normalize();
        assert_eq!(config.max_ejection_seconds, 60);
    }

    #[test]
    fn defaults_all_fields() {
        let config = CircuitBreakerConfig::default();
        assert_eq!(config.consecutive_errors, 5);
        assert_eq!(config.consecutive_gateway_errors, 5);
        assert_eq!(config.consecutive_local_origin_failures, 5);
        assert_eq!(config.ejection_seconds, 30);
        assert_eq!(config.max_ejection_seconds, 300);
        assert_eq!(config.max_ejection_percent, 50);
        assert!(config.validate().is_none());
    }

    #[test]
    fn zero_consecutive_errors_rejected() {
        let config = CircuitBreakerConfig {
            consecutive_errors: 0,
            ..CircuitBreakerConfig::default()
        };
        assert!(config.validate().is_some());
    }
}
