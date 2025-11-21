//! EdgionGatewayConfig CRD definition
//!
//! This model defines the EdgionGatewayConfig custom resource, which is used
//! as parametersRef in GatewayClass to provide gateway-wide configuration.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// EdgionGatewayConfig is the configuration for a GatewayClass
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "edgion.com",
    version = "v1alpha1",
    kind = "EdgionGatewayConfig",
    plural = "Edgiongatewayclassconfigs",
    shortname = "edgwcfg",
    namespaced = false,
    status = "EdgionGatewayConfigStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EdgionGatewayConfigSpec {
    /// Default configuration for all listeners in gateways using this class
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_defaults: Option<ListenerDefaults>,

    /// Default load balancing configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_balancing: Option<LoadBalancing>,

    /// Access log configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_log: Option<AccessLog>,

    /// Security policies
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<Security>,

    /// Resource and performance limits
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,

    /// Observability configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<Observability>,
}

// ============================================
// Listener Defaults
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerDefaults {
    /// Default TLS termination mode
    #[serde(default = "default_tls_mode")]
    pub default_tls_mode: TLSMode,

    /// Whether to allow insecure (HTTP) connections
    #[serde(default)]
    pub allow_insecure: bool,

    /// SNI matching behavior
    #[serde(default = "default_sni_matching_policy")]
    pub sni_matching_policy: SNIMatchingPolicy,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
pub enum TLSMode {
    Terminate,
    Passthrough,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
pub enum SNIMatchingPolicy {
    Strict,
    Loose,
}

fn default_tls_mode() -> TLSMode {
    TLSMode::Terminate
}

fn default_sni_matching_policy() -> SNIMatchingPolicy {
    SNIMatchingPolicy::Strict
}

// ============================================
// Load Balancing
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancing {
    /// Default load balancing algorithm
    #[serde(default = "default_lb_policy")]
    pub default_lb_policy: LBPolicy,

    /// Default health check configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<HealthCheck>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum LBPolicy {
    RoundRobin,
    LeastConnections,
    Random,
    IpHash,
    WeightedRoundRobin,
}

fn default_lb_policy() -> LBPolicy {
    LBPolicy::RoundRobin
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheck {
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Health check interval (e.g., '30s', '1m')
    #[serde(default = "default_health_check_interval")]
    pub interval: String,

    /// Health check timeout
    #[serde(default = "default_health_check_timeout")]
    pub timeout: String,

    /// Number of failures before marking unhealthy
    #[serde(default = "default_unhealthy_threshold")]
    pub unhealthy_threshold: u32,

    /// Number of successes before marking healthy
    #[serde(default = "default_healthy_threshold")]
    pub healthy_threshold: u32,
}

fn default_health_check_interval() -> String {
    "30s".to_string()
}

fn default_health_check_timeout() -> String {
    "5s".to_string()
}

fn default_unhealthy_threshold() -> u32 {
    3
}

fn default_healthy_threshold() -> u32 {
    2
}

// ============================================
// Access Log
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessLog {
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Log format
    #[serde(default = "default_log_format")]
    pub format: LogFormat,

    /// Path to log file
    #[serde(default = "default_log_path")]
    pub log_path: String,

    /// Sampling rate (0.0-1.0)
    #[serde(default = "default_sampling_rate")]
    pub sampling_rate: f64,

    /// Headers to include in logs
    #[serde(default = "default_include_headers")]
    pub include_headers: Vec<String>,

    /// Paths to exclude from logging (e.g., health checks)
    #[serde(default = "default_exclude_paths")]
    pub exclude_paths: Vec<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Json,
    Text,
    Custom,
}

fn default_log_format() -> LogFormat {
    LogFormat::Json
}

fn default_log_path() -> String {
    "/var/log/Edgion/access.log".to_string()
}

fn default_sampling_rate() -> f64 {
    1.0
}

fn default_include_headers() -> Vec<String> {
    vec!["User-Agent".to_string(), "Referer".to_string()]
}

fn default_exclude_paths() -> Vec<String> {
    vec!["/health".to_string(), "/readiness".to_string()]
}

// ============================================
// Security
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Security {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TLSConfig>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TLSConfig {
    /// Minimum TLS version
    #[serde(default = "default_min_tls_version")]
    pub min_tls_version: String,

    /// Maximum TLS version
    #[serde(default = "default_max_tls_version")]
    pub max_tls_version: String,

    /// Allowed TLS cipher suites
    #[serde(default = "default_allowed_ciphers")]
    pub allowed_ciphers: Vec<String>,
}

fn default_min_tls_version() -> String {
    "1.2".to_string()
}

fn default_max_tls_version() -> String {
    "1.3".to_string()
}

fn default_allowed_ciphers() -> Vec<String> {
    vec![
        "TLS_AES_128_GCM_SHA256".to_string(),
        "TLS_AES_256_GCM_SHA384".to_string(),
        "TLS_CHACHA20_POLY1305_SHA256".to_string(),
        "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256".to_string(),
        "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256".to_string(),
        "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384".to_string(),
        "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384".to_string(),
    ]
}

// ============================================
// Limits
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// Maximum concurrent connections per instance
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,

    /// Maximum requests per connection (HTTP/1.1)
    #[serde(default = "default_max_requests_per_connection")]
    pub max_requests_per_connection: u32,

    /// Connection timeout
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout: String,

    /// Request timeout
    #[serde(default = "default_request_timeout")]
    pub request_timeout: String,

    /// Idle connection timeout
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: String,

    /// Maximum request header size
    #[serde(default = "default_max_request_header_size")]
    pub max_request_header_size: String,

    /// Maximum request body size
    #[serde(default = "default_max_request_body_size")]
    pub max_request_body_size: String,

    /// Read buffer size
    #[serde(default = "default_read_buffer_size")]
    pub read_buffer_size: String,

    /// Write buffer size
    #[serde(default = "default_write_buffer_size")]
    pub write_buffer_size: String,
}

fn default_max_connections() -> u32 {
    10000
}

fn default_max_requests_per_connection() -> u32 {
    1000
}

fn default_connection_timeout() -> String {
    "60s".to_string()
}

fn default_request_timeout() -> String {
    "30s".to_string()
}

fn default_idle_timeout() -> String {
    "300s".to_string()
}

fn default_max_request_header_size() -> String {
    "8KB".to_string()
}

fn default_max_request_body_size() -> String {
    "10MB".to_string()
}

fn default_read_buffer_size() -> String {
    "4KB".to_string()
}

fn default_write_buffer_size() -> String {
    "4KB".to_string()
}

fn default_true() -> bool {
    true
}

// ============================================
// Observability
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Observability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracing: Option<Tracing>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<Logging>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Metrics endpoint path
    #[serde(default = "default_metrics_endpoint")]
    pub endpoint: String,

    /// Metrics server port
    #[serde(default = "default_metrics_port")]
    pub port: u16,

    /// Metrics format
    #[serde(default = "default_metrics_format")]
    pub format: MetricsFormat,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MetricsFormat {
    Prometheus,
    Openmetrics,
}

fn default_metrics_endpoint() -> String {
    "/metrics".to_string()
}

fn default_metrics_port() -> u16 {
    9090
}

fn default_metrics_format() -> MetricsFormat {
    MetricsFormat::Prometheus
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tracing {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_tracing_provider")]
    pub provider: TracingProvider,

    /// Tracing collector endpoint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,

    /// Trace sampling rate (0.0-1.0)
    #[serde(default = "default_tracing_sampling_rate")]
    pub sampling_rate: f64,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TracingProvider {
    Jaeger,
    Zipkin,
    Otlp,
}

fn default_tracing_provider() -> TracingProvider {
    TracingProvider::Otlp
}

fn default_tracing_sampling_rate() -> f64 {
    0.1
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Logging {
    #[serde(default = "default_logging_level")]
    pub level: LogLevel,

    #[serde(default = "default_logging_format")]
    pub format: LoggingFormat,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LoggingFormat {
    Json,
    Text,
}

fn default_logging_level() -> LogLevel {
    LogLevel::Info
}

fn default_logging_format() -> LoggingFormat {
    LoggingFormat::Json
}

// ============================================
// Status
// ============================================

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EdgionGatewayConfigStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<StatusCondition>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusCondition {
    #[serde(rename = "type")]
    pub condition_type: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_gateway_config() {
        let yaml = r#"
apiVersion: example.com/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: test-config
spec:
  listenerDefaults:
    defaultTLSMode: Terminate
    allowInsecure: false
  loadBalancing:
    defaultLBPolicy: round-robin
  accessLog:
    enabled: true
    format: json
"#;
        let config: EdgionGatewayConfig = serde_yaml::from_str(yaml).expect("Failed to parse YAML");
        assert_eq!(config.metadata.name, Some("test-config".to_string()));

        let spec = config.spec;
        assert!(spec.listener_defaults.is_some());
        assert!(spec.load_balancing.is_some());
        assert!(spec.access_log.is_some());
    }

    #[test]
    fn test_default_values() {
        let defaults = ListenerDefaults {
            default_tls_mode: default_tls_mode(),
            allow_insecure: false,
            sni_matching_policy: default_sni_matching_policy(),
        };

        assert_eq!(defaults.default_tls_mode, TLSMode::Terminate);
        assert_eq!(defaults.sni_matching_policy, SNIMatchingPolicy::Strict);
    }
}
