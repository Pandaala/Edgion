use crate::core::common::utils::duration::parse_duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Health check configuration parsed from `edgion.io/health-check` annotation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceHealthCheck {
    /// Active health check configuration (periodic probing).
    #[serde(default)]
    pub active: Option<ActiveHealthCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthCheckType {
    Http,
    Tcp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveHealthCheckConfig {
    /// Probe type: "http" (default) or "tcp".
    #[serde(default = "default_hc_type")]
    pub r#type: HealthCheckType,

    /// HTTP request path for health check.
    /// Default: "/".
    #[serde(default = "default_path")]
    pub path: Option<String>,

    /// Optional health check port override.
    #[serde(default)]
    pub port: Option<u16>,

    /// Interval between probes (e.g. "10s", "1m").
    #[serde(default = "default_interval")]
    pub interval: String,

    /// Probe timeout (e.g. "3s", "500ms").
    #[serde(default = "default_timeout")]
    pub timeout: String,

    /// Consecutive successes before marking healthy.
    #[serde(default = "default_healthy_threshold")]
    pub healthy_threshold: u32,

    /// Consecutive failures before marking unhealthy.
    #[serde(default = "default_unhealthy_threshold")]
    pub unhealthy_threshold: u32,

    /// Expected healthy HTTP status codes.
    #[serde(default = "default_expected_statuses")]
    pub expected_statuses: Vec<u16>,

    /// Optional Host header override for HTTP checks.
    #[serde(default)]
    pub host: Option<String>,
}

fn default_hc_type() -> HealthCheckType {
    HealthCheckType::Http
}
fn default_path() -> Option<String> {
    Some("/".to_string())
}
fn default_interval() -> String {
    "10s".to_string()
}
fn default_timeout() -> String {
    "3s".to_string()
}
fn default_healthy_threshold() -> u32 {
    2
}
fn default_unhealthy_threshold() -> u32 {
    3
}
fn default_expected_statuses() -> Vec<u16> {
    vec![200]
}

impl Default for ActiveHealthCheckConfig {
    fn default() -> Self {
        Self {
            r#type: default_hc_type(),
            path: default_path(),
            port: None,
            interval: default_interval(),
            timeout: default_timeout(),
            healthy_threshold: default_healthy_threshold(),
            unhealthy_threshold: default_unhealthy_threshold(),
            expected_statuses: default_expected_statuses(),
            host: None,
        }
    }
}

impl ServiceHealthCheck {
    pub fn get_validation_error(&self) -> Option<&str> {
        let Some(active) = &self.active else {
            return None;
        };

        if active.healthy_threshold == 0 {
            return Some("healthyThreshold must be >= 1");
        }
        if active.unhealthy_threshold == 0 {
            return Some("unhealthyThreshold must be >= 1");
        }

        match parse_duration(&active.interval) {
            Ok(d) if d > Duration::ZERO => {}
            _ => return Some("interval must be a valid non-zero duration"),
        }
        match parse_duration(&active.timeout) {
            Ok(d) if d > Duration::ZERO => {}
            _ => return Some("timeout must be a valid non-zero duration"),
        }

        if matches!(active.r#type, HealthCheckType::Http) {
            if active.path.as_deref().is_some_and(|p| p.is_empty()) {
                return Some("path must not be empty");
            }
            if active.expected_statuses.is_empty() {
                return Some("expectedStatuses must not be empty for http health check");
            }
            if active
                .expected_statuses
                .iter()
                .any(|status| !(100..=599).contains(status))
            {
                return Some("expectedStatuses must be valid HTTP status codes");
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_minimal_config() {
        let yaml = r#"
active:
  type: http
  path: /healthz
"#;

        let cfg: ServiceHealthCheck = serde_yaml::from_str(yaml).expect("deserialize minimal config");
        let active = cfg.active.expect("active config");
        assert_eq!(active.r#type, HealthCheckType::Http);
        assert_eq!(active.path.as_deref(), Some("/healthz"));
        assert_eq!(active.interval, "10s");
        assert_eq!(active.timeout, "3s");
        assert_eq!(active.healthy_threshold, 2);
        assert_eq!(active.unhealthy_threshold, 3);
        assert_eq!(active.expected_statuses, vec![200]);
    }

    #[test]
    fn test_deserialize_full_config() {
        let yaml = r#"
active:
  type: tcp
  path: /ignored
  port: 9090
  interval: 5s
  timeout: 1s
  healthyThreshold: 3
  unhealthyThreshold: 5
  expectedStatuses: [200, 204]
  host: backend.internal
"#;

        let cfg: ServiceHealthCheck = serde_yaml::from_str(yaml).expect("deserialize full config");
        let active = cfg.active.expect("active config");
        assert_eq!(active.r#type, HealthCheckType::Tcp);
        assert_eq!(active.port, Some(9090));
        assert_eq!(active.interval, "5s");
        assert_eq!(active.timeout, "1s");
        assert_eq!(active.healthy_threshold, 3);
        assert_eq!(active.unhealthy_threshold, 5);
        assert_eq!(active.expected_statuses, vec![200, 204]);
        assert_eq!(active.host.as_deref(), Some("backend.internal"));
    }

    #[test]
    fn test_default_values() {
        let cfg = ActiveHealthCheckConfig::default();
        assert_eq!(cfg.r#type, HealthCheckType::Http);
        assert_eq!(cfg.path.as_deref(), Some("/"));
        assert_eq!(cfg.interval, "10s");
        assert_eq!(cfg.timeout, "3s");
        assert_eq!(cfg.healthy_threshold, 2);
        assert_eq!(cfg.unhealthy_threshold, 3);
        assert_eq!(cfg.expected_statuses, vec![200]);
    }

    #[test]
    fn test_validation_rejects_invalid_config() {
        let yaml = r#"
active:
  type: http
  path: ""
  interval: 0s
  timeout: 3s
  healthyThreshold: 0
"#;

        let cfg: ServiceHealthCheck = serde_yaml::from_str(yaml).expect("deserialize invalid config");
        assert!(cfg.get_validation_error().is_some());
    }

    #[test]
    fn test_serde_roundtrip() {
        let original = ServiceHealthCheck {
            active: Some(ActiveHealthCheckConfig {
                r#type: HealthCheckType::Http,
                path: Some("/livez".to_string()),
                port: Some(8080),
                interval: "7s".to_string(),
                timeout: "2s".to_string(),
                healthy_threshold: 2,
                unhealthy_threshold: 3,
                expected_statuses: vec![200, 204],
                host: Some("svc.local".to_string()),
            }),
        };

        let yaml = serde_yaml::to_string(&original).expect("serialize");
        let decoded: ServiceHealthCheck = serde_yaml::from_str(&yaml).expect("deserialize");
        assert_eq!(decoded.active, original.active);
    }
}
