use crate::duration::parse_duration;
use crate::resources::common::OutboundTlsLocal;
use k8s_openapi::api::core::v1::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Health check configuration, configured via the `EdgionBackendTrafficPolicy`
/// resource's `spec.healthCheck` section.
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
    Https,
    Tcp,
    Grpc,
    Grpcs,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveHealthCheckConfig {
    /// Probe type: "http" (default), "https", "tcp", "grpc", or "grpcs".
    #[serde(default = "default_hc_type")]
    pub r#type: HealthCheckType,

    /// HTTP request path for health check.
    /// Default: "/".
    #[serde(default = "default_path")]
    pub path: Option<String>,

    /// Optional health check port override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
    pub port: Option<u16>,

    /// Interval between probes (GEP-2257 duration, e.g. "10s", "1m").
    #[serde(default = "default_interval")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub interval: String,

    /// Probe timeout (GEP-2257 duration, e.g. "3s", "500ms").
    #[serde(default = "default_timeout")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,

    /// gRPC Health Check service name.
    /// Empty string (default) checks the overall server health.
    /// Only used when type is "grpc" or "grpcs".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc_service_name: Option<String>,

    /// Independent TLS configuration, required for HTTPS and GRPCS probes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<OutboundTlsLocal>,

    /// Controller-resolved CA bundles, carried only on the trusted sync channel.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_ca_certificates: Option<Vec<Secret>>,

    /// Controller-resolved mTLS identity, carried only on the trusted sync channel.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_client_certificate: Option<Secret>,

    /// Stable, non-sensitive TLS resolution failure for this generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_tls_error: Option<String>,
}

crate::redact::impl_redacted_debug!(ActiveHealthCheckConfig);

impl ActiveHealthCheckConfig {
    /// Validate TLS independently of policy attachment eligibility. A broken
    /// encrypted checker must continue failing instead of withdrawing the policy.
    pub fn tls_validation_error(&self) -> Option<&'static str> {
        use crate::resources::backend_tls_policy::{
            backend_tls_identity_validation_errors, BackendTLSPolicyValidation,
        };
        use crate::resources::common::is_core_api_group;
        if !matches!(self.r#type, HealthCheckType::Https | HealthCheckType::Grpcs) {
            return None;
        }
        let Some(tls) = &self.tls else {
            return Some("encrypted health checks require tls");
        };
        if tls.verify == Some(false) {
            return Some("encrypted health checks require certificate verification");
        }
        let Some(validation) = &tls.validation else {
            return Some("encrypted health checks require tls.validation.hostname");
        };
        let Some(hostname) = &validation.hostname else {
            return Some("encrypted health checks require tls.validation.hostname");
        };
        let identity = BackendTLSPolicyValidation {
            hostname: hostname.clone(),
            ca_certificate_refs: None,
            well_known_ca_certificates: None,
            subject_alt_names: validation.subject_alt_names.clone(),
        };
        if !backend_tls_identity_validation_errors(&identity).is_empty() {
            return Some("health check TLS hostname or subjectAltNames is invalid");
        }
        // The URL parser also recognizes shortened, octal, hex and integer IP
        // spellings. Reject all of them so endpoint DNS pinning cannot be bypassed.
        if !valid_reference_name(hostname) || !matches!(url::Host::parse(hostname), Ok(url::Host::Domain(_))) {
            return Some("health check TLS hostname must be a DNS name");
        }
        if let Some(refs) = &validation.ca_certificate_refs {
            if refs.is_empty() || validation.well_known_ca_certificates.is_some() {
                return Some("health check CA references must be nonempty and exclusive with system CA");
            }
            for reference in refs {
                if !is_core_api_group(&reference.group)
                    || !matches!(reference.kind.as_str(), "Secret" | "ConfigMap")
                    || !valid_reference_name(&reference.name)
                    || reference
                        .namespace
                        .as_deref()
                        .is_some_and(|ns| !valid_reference_namespace(ns))
                {
                    return Some("health check CA reference is invalid");
                }
            }
        }
        if let Some(reference) = &tls.client_certificate_ref {
            if !is_core_api_group(reference.group.as_deref().unwrap_or_default())
                || reference.kind.as_deref().is_some_and(|kind| kind != "Secret")
                || !valid_reference_name(&reference.name)
                || reference
                    .namespace
                    .as_deref()
                    .is_some_and(|ns| !valid_reference_namespace(ns))
            {
                return Some("health check client certificate reference is invalid");
            }
        }
        None
    }

    /// Both Controller and Gateway enforce the same-namespace reference boundary.
    pub fn tls_namespace_validation_error(&self, policy_namespace: &str) -> Option<&'static str> {
        if !matches!(self.r#type, HealthCheckType::Https | HealthCheckType::Grpcs) {
            return None;
        }
        let tls = self.tls.as_ref()?;
        let cross_namespace = tls
            .validation
            .as_ref()
            .and_then(|v| v.ca_certificate_refs.as_ref())
            .is_some_and(|refs| {
                refs.iter()
                    .any(|reference| reference.namespace.as_deref().is_some_and(|ns| ns != policy_namespace))
            })
            || tls
                .client_certificate_ref
                .as_ref()
                .is_some_and(|reference| reference.namespace.as_deref().is_some_and(|ns| ns != policy_namespace));
        cross_namespace.then_some("health check TLS references must use the policy namespace")
    }
}

fn valid_reference_namespace(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
}

fn valid_reference_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 253 && value.split('.').all(valid_reference_namespace)
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
            grpc_service_name: None,
            tls: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            resolved_tls_error: None,
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

        if matches!(active.r#type, HealthCheckType::Http | HealthCheckType::Https) {
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
    fn test_deserialize_grpc_config() {
        let yaml = r#"
active:
  type: grpc
  port: 9090
  interval: 10s
  timeout: 3s
  healthyThreshold: 2
  unhealthyThreshold: 3
  grpcServiceName: "my.package.MyService"
"#;

        let cfg: ServiceHealthCheck = serde_yaml::from_str(yaml).expect("deserialize grpc config");
        assert!(cfg.get_validation_error().is_none());
        let active = cfg.active.expect("active config");
        assert_eq!(active.r#type, HealthCheckType::Grpc);
        assert_eq!(active.port, Some(9090));
        assert_eq!(active.grpc_service_name.as_deref(), Some("my.package.MyService"));
    }

    #[test]
    fn test_grpc_config_without_service_name() {
        let yaml = r#"
active:
  type: grpc
  port: 9090
"#;

        let cfg: ServiceHealthCheck = serde_yaml::from_str(yaml).expect("deserialize grpc config");
        assert!(cfg.get_validation_error().is_none());
        let active = cfg.active.expect("active config");
        assert_eq!(active.r#type, HealthCheckType::Grpc);
        assert!(active.grpc_service_name.is_none());
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
                grpc_service_name: None,
                tls: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                resolved_tls_error: None,
            }),
        };

        let yaml = serde_yaml::to_string(&original).expect("serialize");
        let decoded: ServiceHealthCheck = serde_yaml::from_str(&yaml).expect("deserialize");
        assert_eq!(decoded.active, original.active);
    }
    #[test]
    fn encrypted_tls_validation_is_independent_of_attachment() {
        for protocol in ["https", "grpcs"] {
            let mut active: ActiveHealthCheckConfig = serde_json::from_value(serde_json::json!({
                "type": protocol, "tls": {"validation": {"hostname": "health.example.com"}}
            }))
            .unwrap();
            assert!(active.tls_validation_error().is_none());
            assert!(active.tls_namespace_validation_error("ns").is_none());
            for hostname in [
                "127.0.0.1",
                "127.1",
                "2130706433",
                "0x7f000001",
                "0177.0.0.1",
                "*.example.com",
                "bad host",
            ] {
                active.tls.as_mut().unwrap().validation.as_mut().unwrap().hostname = Some(hostname.into());
                assert!(active.tls_validation_error().is_some(), "hostname: {hostname}");
                assert!(ServiceHealthCheck {
                    active: Some(active.clone())
                }
                .get_validation_error()
                .is_none());
            }
            active.tls = None;
            assert!(active.tls_validation_error().is_some());
        }
        // Existing plaintext protocols remain literal, even with an irrelevant TLS block.
        for protocol in ["http", "tcp", "grpc"] {
            let active: ActiveHealthCheckConfig =
                serde_json::from_value(serde_json::json!({"type": protocol, "tls": {"verify": false}})).unwrap();
            assert!(active.tls_validation_error().is_none());
        }
    }

    #[test]
    fn encrypted_tls_rejects_invalid_trust_reference_and_san_shapes() {
        for tls in [
            serde_json::json!({"verify": false, "validation": {"hostname": "health.example.com"}}),
            serde_json::json!({"validation": {"hostname": "health.example.com", "caCertificateRefs": []}}),
            serde_json::json!({"validation": {"hostname": "health.example.com", "caCertificateRefs": [{"kind": "Secret", "name": "ca"}], "wellKnownCACertificates": "System"}}),
            serde_json::json!({"validation": {"hostname": "health.example.com", "caCertificateRefs": [{"group": "foreign.io", "kind": "Secret", "name": "ca"}]}}),
            serde_json::json!({"validation": {"hostname": "health.example.com", "subjectAltNames": [{"type": "Hostname", "uri": "spiffe://example/health"}]}}),
            serde_json::json!({"validation": {"hostname": "health.example.com"}, "clientCertificateRef": {"kind": "ConfigMap", "name": "identity"}}),
        ] {
            let active: ActiveHealthCheckConfig =
                serde_json::from_value(serde_json::json!({"type": "https", "tls": tls})).unwrap();
            assert!(active.tls_validation_error().is_some());
        }
        let active: ActiveHealthCheckConfig = serde_json::from_value(serde_json::json!({"type": "https", "tls": {
            "validation": {"hostname": "health.example.com", "caCertificateRefs": [{"kind": "ConfigMap", "name": "ca", "namespace": "other"}]}
        }})).unwrap();
        assert!(active.tls_validation_error().is_none());
        assert!(active.tls_namespace_validation_error("ns").is_some());
        assert!(active.tls_namespace_validation_error("other").is_none());
    }

    #[test]
    fn encrypted_tls_material_is_redacted_and_not_operator_schema() {
        let secret: Secret = serde_json::from_value(serde_json::json!({"data": {"tls.key": "c2VjcmV0"}})).unwrap();
        let active = ActiveHealthCheckConfig {
            resolved_ca_certificates: Some(vec![secret.clone()]),
            resolved_client_certificate: Some(secret),
            resolved_tls_error: Some("health check CA reference is unresolved".into()),
            ..Default::default()
        };
        let schema = serde_json::to_value(schemars::schema_for!(ActiveHealthCheckConfig)).unwrap();
        for key in [
            "resolvedCaCertificates",
            "resolvedClientCertificate",
            "resolvedTlsError",
        ] {
            assert!(schema["properties"].get(key).is_none());
        }
        assert!(schema["properties"].get("tls").is_some());
        let sync = serde_json::to_value(&active).unwrap();
        assert_eq!(sync["resolvedClientCertificate"]["data"]["tls.key"], "c2VjcmV0");
        let decoded: ActiveHealthCheckConfig = serde_json::from_value(sync).unwrap();
        assert_eq!(decoded, active);
        let admin = {
            let _guard = crate::redact::AdminRedactGuard::new();
            serde_json::to_value(&active).unwrap()
        };
        assert_eq!(admin["resolvedClientCertificate"], "[redacted]");
        assert_eq!(admin["resolvedCaCertificates"], "[redacted]");
        assert!(!format!("{active:?}").contains("c2VjcmV0"));
        let mut changed = active.clone();
        changed.resolved_client_certificate = None;
        assert_ne!(changed, active, "identity rotation must affect generation equality");
    }
}
