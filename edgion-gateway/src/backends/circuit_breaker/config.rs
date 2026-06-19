use edgion_resources::constants::annotations::edgion::{CIRCUIT_BREAKER, OUTLIER_DETECTION};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::Deserialize;

/// Resolved outlier-detection config for a service. The internal module and
/// struct retain the `circuit_breaker` name; the user-facing annotation is
/// `edgion.io/outlier-detection`.
#[derive(Debug, Clone, PartialEq)]
pub struct CircuitBreakerConfig {
    /// Consecutive application-5xx (any >=500 that is NOT 502/503/504) before ejection.
    pub consecutive_errors: u32,
    /// Consecutive gateway failures (502/503/504, connect/proxy errors) before ejection.
    pub consecutive_gateway_errors: u32,
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
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCircuitBreakerConfig {
    #[serde(default = "default_consecutive_errors")]
    consecutive_errors: u32,
    #[serde(default)]
    consecutive_gateway_errors: Option<u32>,
    #[serde(default = "default_ejection_seconds")]
    ejection_seconds: u64,
    #[serde(default)]
    max_ejection_seconds: Option<u64>,
    #[serde(default = "default_max_ejection_percent")]
    max_ejection_percent: u32,
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

impl RawCircuitBreakerConfig {
    /// Resolve field-dependent defaults and clamp invalid combinations.
    fn normalize(self) -> CircuitBreakerConfig {
        // Gateway threshold inherits the (possibly tuned) consecutive_errors so
        // old single-field configs keep their aggressiveness on the hard-failure path.
        let consecutive_gateway_errors = self.consecutive_gateway_errors.unwrap_or(self.consecutive_errors);
        // Default cap = max(300, ejection); explicit cap below base is clamped UP, never rejected.
        let max_ejection_seconds = self
            .max_ejection_seconds
            .unwrap_or_else(|| self.ejection_seconds.max(300))
            .max(self.ejection_seconds);
        CircuitBreakerConfig {
            consecutive_errors: self.consecutive_errors,
            consecutive_gateway_errors,
            ejection_seconds: self.ejection_seconds,
            max_ejection_seconds,
            max_ejection_percent: self.max_ejection_percent,
        }
    }
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        RawCircuitBreakerConfig {
            consecutive_errors: default_consecutive_errors(),
            consecutive_gateway_errors: None,
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
        if self.ejection_seconds == 0 {
            return Some("ejectionSeconds must be >= 1".to_string());
        }
        if self.max_ejection_percent > 100 {
            return Some("maxEjectionPercent must be 0-100".to_string());
        }
        None
    }
}

/// Result of parsing the outlier-detection annotation.
pub struct ParsedOutlier {
    pub config: CircuitBreakerConfig,
    /// True when parsed from the deprecated `edgion.io/circuit-breaker` key.
    pub used_alias: bool,
}

/// Parse outlier-detection config from either the canonical
/// `edgion.io/outlier-detection` key or the deprecated `edgion.io/circuit-breaker`
/// alias. Canonical wins when both are present. Pure function — the deprecation
/// WARN is emitted by the caller (see services conf-handler) to avoid resync spam.
pub fn parse_outlier_detection_annotation(meta: &ObjectMeta) -> Option<ParsedOutlier> {
    let annotations = meta.annotations.as_ref()?;
    let (yaml_str, used_alias) = match annotations.get(OUTLIER_DETECTION) {
        Some(s) => (s, false),
        None => (annotations.get(CIRCUIT_BREAKER)?, true),
    };

    match serde_yaml::from_str::<RawCircuitBreakerConfig>(yaml_str) {
        Ok(raw) => {
            let config = raw.normalize();
            if let Some(err) = config.validate() {
                tracing::warn!(
                    resource = %meta.name.as_deref().unwrap_or(""),
                    error = %err,
                    "Invalid outlier-detection annotation, ignoring"
                );
                return None;
            }
            Some(ParsedOutlier { config, used_alias })
        }
        Err(e) => {
            tracing::warn!(
                resource = %meta.name.as_deref().unwrap_or(""),
                error = %e,
                "Failed to parse outlier-detection annotation"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::constants::annotations::edgion::{CIRCUIT_BREAKER, OUTLIER_DETECTION};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::BTreeMap;

    fn meta_with(key: &str, value: &str) -> ObjectMeta {
        let mut annotations = BTreeMap::new();
        annotations.insert(key.to_string(), value.to_string());
        ObjectMeta {
            name: Some("test-svc".to_string()),
            annotations: Some(annotations),
            ..Default::default()
        }
    }

    #[test]
    fn gateway_errors_defaults_to_consecutive_errors() {
        let m = meta_with(OUTLIER_DETECTION, "consecutiveErrors: 2\n");
        let p = parse_outlier_detection_annotation(&m).unwrap();
        assert_eq!(p.config.consecutive_errors, 2);
        assert_eq!(p.config.consecutive_gateway_errors, 2); // inherits, not literal 5
        assert!(!p.used_alias);
    }

    #[test]
    fn max_ejection_seconds_defaults_to_max_300_ejection() {
        let m = meta_with(OUTLIER_DETECTION, "ejectionSeconds: 600\n");
        let p = parse_outlier_detection_annotation(&m).unwrap();
        assert_eq!(p.config.max_ejection_seconds, 600); // max(300, 600)
    }

    #[test]
    fn max_ejection_seconds_clamps_up_not_rejects() {
        // Explicit max < ejection must be clamped up, NOT cause the whole
        // annotation to be dropped.
        let m = meta_with(OUTLIER_DETECTION, "ejectionSeconds: 60\nmaxEjectionSeconds: 10\n");
        let p = parse_outlier_detection_annotation(&m).unwrap();
        assert_eq!(p.config.max_ejection_seconds, 60);
    }

    #[test]
    fn deprecated_alias_is_parsed_and_flagged() {
        let m = meta_with(CIRCUIT_BREAKER, "consecutiveErrors: 3\n");
        let p = parse_outlier_detection_annotation(&m).unwrap();
        assert_eq!(p.config.consecutive_errors, 3);
        assert!(p.used_alias);
    }

    #[test]
    fn canonical_wins_when_both_present() {
        let mut annotations = BTreeMap::new();
        annotations.insert(OUTLIER_DETECTION.to_string(), "consecutiveErrors: 7\n".to_string());
        annotations.insert(CIRCUIT_BREAKER.to_string(), "consecutiveErrors: 3\n".to_string());
        let m = ObjectMeta {
            annotations: Some(annotations),
            ..Default::default()
        };
        let p = parse_outlier_detection_annotation(&m).unwrap();
        assert_eq!(p.config.consecutive_errors, 7);
        assert!(!p.used_alias);
    }

    #[test]
    fn defaults_all_fields() {
        let m = meta_with(OUTLIER_DETECTION, "{}");
        let p = parse_outlier_detection_annotation(&m).unwrap();
        assert_eq!(p.config.consecutive_errors, 5);
        assert_eq!(p.config.consecutive_gateway_errors, 5);
        assert_eq!(p.config.ejection_seconds, 30);
        assert_eq!(p.config.max_ejection_seconds, 300);
        assert_eq!(p.config.max_ejection_percent, 50);
    }

    #[test]
    fn invalid_yaml_returns_none() {
        let m = meta_with(OUTLIER_DETECTION, "[invalid");
        assert!(parse_outlier_detection_annotation(&m).is_none());
    }

    #[test]
    fn zero_consecutive_errors_rejected() {
        let m = meta_with(OUTLIER_DETECTION, "consecutiveErrors: 0\n");
        assert!(parse_outlier_detection_annotation(&m).is_none());
    }

    #[test]
    fn missing_annotation_returns_none() {
        let m = ObjectMeta {
            annotations: None,
            ..Default::default()
        };
        assert!(parse_outlier_detection_annotation(&m).is_none());
    }
}
