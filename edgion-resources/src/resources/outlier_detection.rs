//! Outlier-detection configuration.
//!
//! `OutlierDetectionConfig` is the deserialize/CRD-spec shape: fields whose
//! defaults depend on sibling values are `Option` so `normalize` can apply
//! them; `#[serde(default)]` cannot express cross-field defaults.
//! `OutlierDetectionResolved` is the fully-resolved shape used by the gateway
//! data plane, produced by `OutlierDetectionConfig::normalize`.

use crate::duration::parse_duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Resolved outlier-detection config for a service. The user-facing
/// configuration surface is the `EdgionBackendTrafficPolicy` resource's
/// `spec.outlierDetection` section.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlierDetectionResolved {
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
    /// Base ejection duration (GEP-2257, e.g. "30s"; multiplied by the endpoint's ejection count).
    #[serde(default = "default_ejection_time")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub ejection_time: String,
    /// Upper bound on the backoff-scaled ejection duration (GEP-2257, default max("5m", ejectionTime)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub max_ejection_time: Option<String>,
    #[serde(default = "default_max_ejection_percent")]
    pub max_ejection_percent: u32,
}

fn default_consecutive_errors() -> u32 {
    5
}
fn default_ejection_time() -> String {
    "30s".to_string()
}
fn default_max_ejection_percent() -> u32 {
    50
}

impl OutlierDetectionConfig {
    /// Validate the raw config. Checks the GEP-2257 duration fields first —
    /// `normalize` falls back to defaults on unparseable strings, so parse
    /// errors must be surfaced here — then delegates the numeric checks to
    /// the normalized shape.
    pub fn get_validation_error(&self) -> Option<String> {
        match parse_duration(&self.ejection_time) {
            // Ejection is tracked at whole-second granularity internally, so
            // sub-second durations (e.g. "500ms") are rejected, not truncated to 0.
            Ok(d) if d.as_secs() >= 1 => {}
            _ => return Some("ejectionTime must be a valid duration of at least 1s".to_string()),
        }
        if let Some(max) = &self.max_ejection_time {
            if parse_duration(max).is_err() {
                return Some("maxEjectionTime must be a valid GEP-2257 duration".to_string());
            }
        }
        self.clone().normalize().validate()
    }

    /// Resolve field-dependent defaults and clamp invalid combinations.
    pub fn normalize(self) -> OutlierDetectionResolved {
        // Gateway threshold inherits the (possibly tuned) consecutive_errors so
        // old single-field configs keep their aggressiveness on the hard-failure path.
        let consecutive_gateway_errors = self.consecutive_gateway_errors.unwrap_or(self.consecutive_errors);
        // Local-origin threshold inherits the (already-resolved) gateway threshold when
        // unset, so a config that predates this field keeps counting connect/proxy errors
        // exactly as it did when they shared the gateway bucket — behaviour is unchanged.
        let consecutive_local_origin_failures = self
            .consecutive_local_origin_failures
            .unwrap_or(consecutive_gateway_errors);
        // Duration strings are validated by `get_validation_error`; if an
        // unvalidated string leaks through (lenient admission), fall back to
        // the documented defaults rather than panicking.
        let ejection_seconds = parse_duration(&self.ejection_time).map(|d| d.as_secs()).unwrap_or(30);
        // Default cap = max("5m", ejection); explicit cap below base is clamped UP, never rejected.
        let max_ejection_seconds = self
            .max_ejection_time
            .as_deref()
            .and_then(|s| parse_duration(s).ok())
            .map(|d| d.as_secs())
            .unwrap_or_else(|| ejection_seconds.max(300))
            .max(ejection_seconds);
        OutlierDetectionResolved {
            consecutive_errors: self.consecutive_errors,
            consecutive_gateway_errors,
            consecutive_local_origin_failures,
            ejection_seconds,
            max_ejection_seconds,
            max_ejection_percent: self.max_ejection_percent,
        }
    }
}

impl Default for OutlierDetectionResolved {
    fn default() -> Self {
        OutlierDetectionConfig {
            consecutive_errors: default_consecutive_errors(),
            consecutive_gateway_errors: None,
            consecutive_local_origin_failures: None,
            ejection_time: default_ejection_time(),
            max_ejection_time: None,
            max_ejection_percent: default_max_ejection_percent(),
        }
        .normalize()
    }
}

impl OutlierDetectionResolved {
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
            return Some("ejectionTime must be at least 1s".to_string());
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
        ejection_time: &str,
        max_ejection_time: Option<&str>,
    ) -> OutlierDetectionConfig {
        OutlierDetectionConfig {
            consecutive_errors,
            consecutive_gateway_errors,
            consecutive_local_origin_failures,
            ejection_time: ejection_time.to_string(),
            max_ejection_time: max_ejection_time.map(str::to_string),
            max_ejection_percent: default_max_ejection_percent(),
        }
    }

    #[test]
    fn gateway_errors_defaults_to_consecutive_errors() {
        let config = config_with(2, None, None, &default_ejection_time(), None).normalize();
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
            &default_ejection_time(),
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
            &default_ejection_time(),
            None,
        )
        .normalize();
        assert_eq!(config.consecutive_gateway_errors, 8);
        assert_eq!(config.consecutive_local_origin_failures, 2);
    }

    #[test]
    fn zero_local_origin_failures_rejected() {
        let config = OutlierDetectionResolved {
            consecutive_local_origin_failures: 0,
            ..OutlierDetectionResolved::default()
        };
        assert!(config.validate().is_some());
    }

    #[test]
    fn max_ejection_time_defaults_to_max_5m_ejection() {
        let config = config_with(default_consecutive_errors(), None, None, "10m", None).normalize();
        assert_eq!(config.max_ejection_seconds, 600); // max(300, 600)
    }

    #[test]
    fn max_ejection_time_clamps_up_not_rejects() {
        // Explicit max < ejection must be clamped up, NOT cause validation to fail.
        let config = config_with(default_consecutive_errors(), None, None, "1m", Some("10s")).normalize();
        assert_eq!(config.max_ejection_seconds, 60);
    }

    #[test]
    fn invalid_ejection_time_rejected_by_validation() {
        // Bare integers (the pre-GEP-2257 form) and sub-second durations are
        // rejected by get_validation_error, not silently defaulted.
        let config = config_with(default_consecutive_errors(), None, None, "30", None);
        assert!(config.get_validation_error().is_some());
        let config = config_with(default_consecutive_errors(), None, None, "500ms", None);
        assert!(config.get_validation_error().is_some());
        let config = config_with(default_consecutive_errors(), None, None, "30s", Some("bogus"));
        assert!(config.get_validation_error().is_some());
        let config = config_with(default_consecutive_errors(), None, None, "30s", Some("5m"));
        assert!(config.get_validation_error().is_none());
    }

    #[test]
    fn invalid_ejection_time_normalize_falls_back_to_defaults() {
        // Lenient admission can leak an unvalidated string into normalize;
        // it must fall back to the documented defaults, not panic.
        let config = config_with(default_consecutive_errors(), None, None, "bogus", None).normalize();
        assert_eq!(config.ejection_seconds, 30);
        assert_eq!(config.max_ejection_seconds, 300);
    }

    #[test]
    fn defaults_all_fields() {
        let config = OutlierDetectionResolved::default();
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
        let config = OutlierDetectionResolved {
            consecutive_errors: 0,
            ..OutlierDetectionResolved::default()
        };
        assert!(config.validate().is_some());
    }
}
