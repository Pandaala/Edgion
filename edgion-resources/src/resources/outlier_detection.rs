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
#[path = "../../tests/unit/resources/outlier_detection_test.rs"]
mod tests;
