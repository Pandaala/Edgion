//! Etcd client configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::common::SecretReference;

/// Etcd client configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdClientConfig {
    /// etcd server endpoints (e.g., "http://127.0.0.1:2379", "https://etcd.example.com:2379")
    pub endpoints: Vec<String>,

    /// Authentication configuration (username/password)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<EtcdAuth>,

    /// TLS/SSL configuration for secure connections
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<EtcdTls>,

    /// Timeout configuration (dial, request, keep-alive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<EtcdTimeout>,

    /// HTTP/2 keep-alive configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<EtcdKeepAlive>,

    /// Namespace prefix for all keys (etcd v3 feature)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Auto-sync cluster members interval (in seconds, 0 to disable)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_sync_interval: Option<u64>,

    /// Maximum message size for gRPC calls (in bytes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_send_size: Option<usize>,

    /// Maximum message size for gRPC responses (in bytes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_recv_size: Option<usize>,

    /// User agent string for client identification
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,

    /// Reject connections to old cluster versions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_old_cluster: Option<bool>,

    /// Observability configuration (logging, metrics)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<EtcdObservability>,
}

/// Etcd authentication configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdAuth {
    /// Username for etcd authentication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Password for etcd authentication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Secret reference for credentials
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretReference>,
}

/// Etcd timeout configuration (in milliseconds)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdTimeout {
    /// Dial timeout in milliseconds (connection establishment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial: Option<u64>,

    /// Request timeout in milliseconds (per-request)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<u64>,

    /// Keep-alive timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<u64>,
}

/// Etcd HTTP/2 keep-alive configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdKeepAlive {
    /// Keep-alive time interval in seconds (time between pings)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,

    /// Keep-alive timeout in seconds (max time waiting for ping ack)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// Permit keep-alive pings without active streams
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permit_without_stream: Option<bool>,
}

/// Etcd TLS configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdTls {
    /// Enable TLS/SSL
    #[serde(default)]
    pub enabled: bool,

    /// TLS certificates configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certs: Option<EtcdTlsCerts>,

    /// Skip certificate verification (insecure, for testing only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure_skip_verify: Option<bool>,
}

/// Etcd TLS certificates configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdTlsCerts {
    /// CA certificate (PEM format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<String>,

    /// Client certificate (PEM format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,

    /// Client private key (PEM format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,

    /// Secret reference for certificates
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretReference>,
}

/// Etcd observability configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdObservability {
    /// Metrics configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<EtcdMetrics>,

    /// Logging configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<EtcdLogging>,
}

/// Etcd metrics configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdMetrics {
    /// Enable metrics collection
    #[serde(default)]
    pub enabled: bool,

    /// Metrics labels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, String>>,
}

/// Etcd logging configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdLogging {
    /// Enable logging
    #[serde(default)]
    pub enabled: bool,

    /// Log level (e.g., "debug", "info", "warn", "error")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,

    /// Log slow operations (threshold in milliseconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_log_threshold: Option<u64>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{LinkSys, LinkSysSpec, SystemType};

    #[test]
    fn test_deserialize_etcd_simple() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-simple
  namespace: test
spec:
  type: etcd
  etcd:
    endpoints:
      - "http://etcd-server:2379"
"#;

        let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
        assert_eq!(link_sys.metadata.name.as_deref(), Some("etcd-simple"));
        assert_eq!(link_sys.metadata.namespace.as_deref(), Some("test"));
        assert_eq!(link_sys.spec.sys_type, SystemType::Etcd);
        
        let etcd_config = link_sys.spec.etcd.expect("Etcd config should be present");
        assert_eq!(etcd_config.endpoints.len(), 1);
        assert_eq!(etcd_config.endpoints[0], "http://etcd-server:2379");
        assert!(etcd_config.auth.is_none());
        assert!(etcd_config.tls.is_none());
    }

    #[test]
    fn test_deserialize_etcd_with_auth() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-with-auth
  namespace: test
spec:
  type: etcd
  etcd:
    endpoints:
      - "https://etcd.example.com:2379"
    auth:
      username: "test-user"
      password: "test-password"
"#;

        let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
        let etcd_config = link_sys.spec.etcd.expect("Etcd config should be present");
        
        let auth = etcd_config.auth.expect("Auth should be present");
        assert_eq!(auth.username.as_deref(), Some("test-user"));
        assert_eq!(auth.password.as_deref(), Some("test-password"));
    }

    #[test]
    fn test_deserialize_etcd_with_tls() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-with-tls
  namespace: test
spec:
  type: etcd
  etcd:
    endpoints:
      - "https://etcd.example.com:2379"
    tls:
      enabled: true
      insecureSkipVerify: false
      certs:
        caCert: "CA_CERT_CONTENT"
        clientCert: "CLIENT_CERT_CONTENT"
        clientKey: "CLIENT_KEY_CONTENT"
"#;

        let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
        let etcd_config = link_sys.spec.etcd.expect("Etcd config should be present");
        
        let tls = etcd_config.tls.expect("TLS should be present");
        assert!(tls.enabled);
        assert_eq!(tls.insecure_skip_verify, Some(false));
        
        let certs = tls.certs.expect("Certs should be present");
        assert_eq!(certs.ca_cert.as_deref(), Some("CA_CERT_CONTENT"));
        assert_eq!(certs.client_cert.as_deref(), Some("CLIENT_CERT_CONTENT"));
        assert_eq!(certs.client_key.as_deref(), Some("CLIENT_KEY_CONTENT"));
    }

    #[test]
    fn test_deserialize_etcd_with_timeout() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-with-timeout
  namespace: test
spec:
  type: etcd
  etcd:
    endpoints:
      - "http://etcd:2379"
    timeout:
      dial: 5000
      request: 10000
      keepAlive: 30000
"#;

        let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
        let etcd_config = link_sys.spec.etcd.expect("Etcd config should be present");
        
        let timeout = etcd_config.timeout.expect("Timeout should be present");
        assert_eq!(timeout.dial, Some(5000));
        assert_eq!(timeout.request, Some(10000));
        assert_eq!(timeout.keep_alive, Some(30000));
    }

    #[test]
    fn test_deserialize_etcd_with_keepalive() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-with-keepalive
  namespace: test
spec:
  type: etcd
  etcd:
    endpoints:
      - "http://etcd:2379"
    keepAlive:
      time: 30
      timeout: 10
      permitWithoutStream: true
"#;

        let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
        let etcd_config = link_sys.spec.etcd.expect("Etcd config should be present");
        
        let keep_alive = etcd_config.keep_alive.expect("KeepAlive should be present");
        assert_eq!(keep_alive.time, Some(30));
        assert_eq!(keep_alive.timeout, Some(10));
        assert_eq!(keep_alive.permit_without_stream, Some(true));
    }

    #[test]
    fn test_deserialize_etcd_full_config() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-full
  namespace: test
spec:
  type: etcd
  etcd:
    endpoints:
      - "https://etcd-1:2379"
      - "https://etcd-2:2379"
    auth:
      username: "app-user"
      password: "secret"
    tls:
      enabled: true
      insecureSkipVerify: false
    timeout:
      dial: 5000
      request: 10000
    keepAlive:
      time: 30
      timeout: 10
    namespace: "/my-app/"
    autoSyncInterval: 300
    maxCallSendSize: 2097152
    maxCallRecvSize: 4194304
    userAgent: "edgion-gateway/v1.0.0"
    rejectOldCluster: true
    observability:
      logging:
        enabled: true
        level: "info"
        slowLogThreshold: 1000
      metrics:
        enabled: true
        labels:
          environment: "production"
"#;

        let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
        let etcd_config = link_sys.spec.etcd.expect("Etcd config should be present");
        
        assert_eq!(etcd_config.endpoints.len(), 2);
        assert_eq!(etcd_config.namespace.as_deref(), Some("/my-app/"));
        assert_eq!(etcd_config.auto_sync_interval, Some(300));
        assert_eq!(etcd_config.max_call_send_size, Some(2097152));
        assert_eq!(etcd_config.max_call_recv_size, Some(4194304));
        assert_eq!(etcd_config.user_agent.as_deref(), Some("edgion-gateway/v1.0.0"));
        assert_eq!(etcd_config.reject_old_cluster, Some(true));
        
        let observability = etcd_config.observability.expect("Observability should be present");
        let logging = observability.logging.expect("Logging should be present");
        assert!(logging.enabled);
        assert_eq!(logging.level.as_deref(), Some("info"));
        assert_eq!(logging.slow_log_threshold, Some(1000));
        
        let metrics = observability.metrics.expect("Metrics should be present");
        assert!(metrics.enabled);
        let labels = metrics.labels.expect("Labels should be present");
        assert_eq!(labels.get("environment").map(|s| s.as_str()), Some("production"));
    }

    #[test]
    fn test_serialize_etcd_config() {
        let link_sys = LinkSys {
            metadata: kube::api::ObjectMeta {
                name: Some("test-etcd".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                sys_type: SystemType::Etcd,
                redis: None,
                etcd: Some(EtcdClientConfig {
                    endpoints: vec!["http://etcd:2379".to_string()],
                    auth: Some(EtcdAuth {
                        username: Some("user".to_string()),
                        password: Some("pass".to_string()),
                        secret_ref: None,
                    }),
                    tls: None,
                    timeout: Some(EtcdTimeout {
                        dial: Some(5000),
                        request: Some(10000),
                        keep_alive: None,
                    }),
                    keep_alive: None,
                    namespace: Some("/app/".to_string()),
                    auto_sync_interval: Some(300),
                    max_call_send_size: None,
                    max_call_recv_size: None,
                    user_agent: None,
                    reject_old_cluster: None,
                    observability: None,
                }),
            },
        };

        let yaml = serde_yaml::to_string(&link_sys).expect("Failed to serialize");
        assert!(yaml.contains("type: etcd"));
        assert!(yaml.contains("endpoints:"));
        assert!(yaml.contains("http://etcd:2379"));
        assert!(yaml.contains("username: user"));
        assert!(yaml.contains("namespace: /app/"));
        assert!(yaml.contains("autoSyncInterval: 300"));
    }

    #[test]
    fn test_deserialize_etcd_with_secret_ref() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-secret
  namespace: test
spec:
  type: etcd
  etcd:
    endpoints:
      - "https://etcd:2379"
    auth:
      secretRef:
        name: etcd-creds
        namespace: test
        usernameKey: username
        passwordKey: password
    tls:
      enabled: true
      certs:
        secretRef:
          name: etcd-tls
          namespace: test
"#;

        let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
        let etcd_config = link_sys.spec.etcd.expect("Etcd config should be present");
        
        let auth = etcd_config.auth.expect("Auth should be present");
        let secret_ref = auth.secret_ref.expect("Secret ref should be present");
        assert_eq!(secret_ref.name, "etcd-creds");
        assert_eq!(secret_ref.namespace.as_deref(), Some("test"));
        
        let tls = etcd_config.tls.expect("TLS should be present");
        let tls_certs = tls.certs.expect("TLS certs should be present");
        let tls_secret_ref = tls_certs.secret_ref.expect("TLS secret ref should be present");
        assert_eq!(tls_secret_ref.name, "etcd-tls");
    }
}

