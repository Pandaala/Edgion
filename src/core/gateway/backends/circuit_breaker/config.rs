use crate::types::constants::annotations::edgion::CIRCUIT_BREAKER;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakerConfig {
    #[serde(default = "default_consecutive_errors")]
    pub consecutive_errors: u32,
    #[serde(default = "default_ejection_seconds")]
    pub ejection_seconds: u64,
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

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            consecutive_errors: default_consecutive_errors(),
            ejection_seconds: default_ejection_seconds(),
            max_ejection_percent: default_max_ejection_percent(),
        }
    }
}

impl CircuitBreakerConfig {
    #[inline]
    pub fn ejection_duration(&self) -> Duration {
        Duration::from_secs(self.ejection_seconds)
    }

    pub fn validate(&self) -> Option<String> {
        if self.consecutive_errors == 0 {
            return Some("consecutiveErrors must be >= 1".to_string());
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

pub fn parse_circuit_breaker_annotation(meta: &ObjectMeta) -> Option<CircuitBreakerConfig> {
    let annotations = meta.annotations.as_ref()?;
    let yaml_str = annotations.get(CIRCUIT_BREAKER)?;

    match serde_yaml::from_str::<CircuitBreakerConfig>(yaml_str) {
        Ok(config) => {
            if let Some(err) = config.validate() {
                tracing::warn!(
                    resource = %meta.name.as_deref().unwrap_or(""),
                    error = %err,
                    "Invalid circuit-breaker annotation, ignoring"
                );
                return None;
            }
            Some(config)
        }
        Err(e) => {
            tracing::warn!(
                resource = %meta.name.as_deref().unwrap_or(""),
                error = %e,
                "Failed to parse circuit-breaker annotation"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn meta_with_annotation(value: Option<&str>) -> ObjectMeta {
        let mut annotations = BTreeMap::new();
        if let Some(v) = value {
            annotations.insert(CIRCUIT_BREAKER.to_string(), v.to_string());
        }
        ObjectMeta {
            name: Some("test-svc".to_string()),
            annotations: if annotations.is_empty() {
                None
            } else {
                Some(annotations)
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_parse_valid_annotation() {
        let meta = meta_with_annotation(Some("consecutiveErrors: 3\nejectionSeconds: 10\n"));
        let cfg = parse_circuit_breaker_annotation(&meta).unwrap();
        assert_eq!(cfg.consecutive_errors, 3);
        assert_eq!(cfg.ejection_seconds, 10);
        assert_eq!(cfg.max_ejection_percent, 50);
    }

    #[test]
    fn test_parse_defaults() {
        let meta = meta_with_annotation(Some("{}"));
        let cfg = parse_circuit_breaker_annotation(&meta).unwrap();
        assert_eq!(cfg.consecutive_errors, 5);
        assert_eq!(cfg.ejection_seconds, 30);
    }

    #[test]
    fn test_parse_missing_annotation() {
        let meta = meta_with_annotation(None);
        assert!(parse_circuit_breaker_annotation(&meta).is_none());
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let meta = meta_with_annotation(Some("[invalid"));
        assert!(parse_circuit_breaker_annotation(&meta).is_none());
    }

    #[test]
    fn test_validate_zero_errors() {
        let meta = meta_with_annotation(Some("consecutiveErrors: 0\n"));
        assert!(parse_circuit_breaker_annotation(&meta).is_none());
    }

    #[test]
    fn test_validate_zero_ejection() {
        let meta = meta_with_annotation(Some("ejectionSeconds: 0\n"));
        assert!(parse_circuit_breaker_annotation(&meta).is_none());
    }
}
