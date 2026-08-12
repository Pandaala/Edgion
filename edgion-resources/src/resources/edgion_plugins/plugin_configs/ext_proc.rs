//! ExtProc plugin configuration.
//!
//! Drives an external processing service over a bidirectional gRPC stream
//! (Envoy `ext_proc` compatible). The gateway sends request/response phases to
//! the processor, which may mutate headers or short-circuit with an immediate
//! response.
//!
//! Request bodies support `BUFFERED` and `STREAMED`. Proxied upstream response
//! bodies support `STREAMED`; response buffering is deliberately rejected.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::TlsConfig;
use crate::resources::edgion_plugins::PluginValidation;

/// ExtProc plugin configuration.
///
/// ## Example
/// ```yaml
/// grpcService:
///   target: "ext-proc.svc.cluster.local:9000"
///   timeoutMs: 200
/// processingMode:
///   requestHeaderMode:  SEND
///   responseHeaderMode: SEND
///   requestBodyMode:    BUFFERED
///   responseBodyMode:   STREAMED
/// failureModeAllow: false      # fail-close when the processor is unreachable
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtProcConfig {
    /// External processing gRPC service connection.
    pub grpc_service: GrpcServiceConfig,

    /// Per-phase SEND/SKIP control.
    #[serde(default)]
    pub processing_mode: ProcessingMode,

    /// When the external processor is unreachable or errors:
    /// - `false` (default): fail-close — short-circuit the request with an error.
    /// - `true`: fail-open — skip processing and continue as if unconfigured.
    #[serde(default)]
    pub failure_mode_allow: bool,
}

/// gRPC connection settings for the external processing service.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrpcServiceConfig {
    /// Target authority in `host:port` form (gRPC endpoint). Required.
    /// Example: "ext-proc.svc.cluster.local:9000"
    pub target: String,

    /// Per-message timeout in milliseconds. Default: 200.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// TLS settings for the gRPC connection (enabled, verify, validation, clientCertificateRef).
    /// Set tls.enabled=true to dial over TLS; all other fields are optional and inherit from
    /// the global outboundTls default when absent.
    #[serde(default)]
    pub tls: TlsConfig,
}

/// Per-phase processing control, mirroring Envoy's `ProcessingMode`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingMode {
    /// Whether to send the request headers to the processor. Default: SEND.
    #[serde(default = "default_header_send")]
    pub request_header_mode: HeaderSendMode,

    /// Whether to send the response headers to the processor. Default: SKIP.
    #[serde(default = "default_header_skip")]
    pub response_header_mode: HeaderSendMode,

    /// Request body handling. `BUFFERED`, `SKIP`, and `NONE` are supported.
    #[serde(default)]
    pub request_body_mode: BodySendMode,

    /// Response body handling. `STREAMED`, `SKIP`, and `NONE` are accepted.
    /// Full or partial buffering is forbidden for proxied upstream responses.
    #[serde(default)]
    pub response_body_mode: BodySendMode,
}

/// Header-phase send control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HeaderSendMode {
    /// Send this phase to the external processor.
    Send,
    /// Skip this phase entirely.
    Skip,
}

/// Body-phase send control. Unsupported stage/mode combinations parse and are
/// then explicitly rejected by validation, surfacing a clear error instead of
/// a silent no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BodySendMode {
    /// Do not send the body (default).
    #[default]
    None,
    /// Skip the body phase (equivalent to NONE for this plugin).
    Skip,
    /// Stream body chunks to the processor.
    Streamed,
    /// Buffer the full body. Supported for requests only.
    Buffered,
    /// Buffer a partial body — NOT supported this release.
    BufferedPartial,
}

impl HeaderSendMode {
    /// Whether this phase should be sent to the processor.
    pub fn is_send(&self) -> bool {
        matches!(self, HeaderSendMode::Send)
    }
}

impl BodySendMode {
    pub fn is_buffered(&self) -> bool {
        matches!(self, BodySendMode::Buffered)
    }

    pub fn is_streamed(&self) -> bool {
        matches!(self, BodySendMode::Streamed)
    }

    fn is_supported_request(&self) -> bool {
        matches!(
            self,
            BodySendMode::None | BodySendMode::Skip | BodySendMode::Buffered | BodySendMode::Streamed
        )
    }

    fn is_supported_response(&self) -> bool {
        matches!(self, BodySendMode::None | BodySendMode::Skip | BodySendMode::Streamed)
    }
}

/// Upper bound for the per-message timeout (60s). The timeout also bounds how
/// long a stalled processor can hold the request path before the fail mode runs.
const MAX_TIMEOUT_MS: u64 = 60_000;

fn default_timeout_ms() -> u64 {
    200
}

fn default_header_send() -> HeaderSendMode {
    HeaderSendMode::Send
}

fn default_header_skip() -> HeaderSendMode {
    HeaderSendMode::Skip
}

impl Default for ProcessingMode {
    fn default() -> Self {
        Self {
            request_header_mode: HeaderSendMode::Send,
            response_header_mode: HeaderSendMode::Skip,
            request_body_mode: BodySendMode::None,
            response_body_mode: BodySendMode::None,
        }
    }
}

impl Default for ExtProcConfig {
    fn default() -> Self {
        Self {
            grpc_service: GrpcServiceConfig {
                target: String::new(),
                timeout_ms: default_timeout_ms(),
                tls: TlsConfig::default(),
            },
            processing_mode: ProcessingMode::default(),
            failure_mode_allow: false,
        }
    }
}

/// Validate a gRPC `host:port` authority. Rejects URLs, paths, empty host/port,
/// and non-numeric / out-of-range ports. Without this, a malformed target only
/// fails later at gateway channel-build time — the plugin is then sealed
/// fail-closed (the route 500s) instead of surfacing a clean validation error on
/// the resource's status at controller ingest. Catching it here gives the
/// operator an actionable message up front.
fn validate_target(target: &str) -> Result<(), String> {
    if target.is_empty() {
        return Err("grpcService.target is required".to_string());
    }
    if target.contains("://") {
        return Err("grpcService.target must be a host:port authority, not a URL".to_string());
    }
    if target.contains('/') || target.contains('?') || target.contains('#') {
        return Err("grpcService.target must be a bare host:port authority (no path/query/fragment)".to_string());
    }

    // Split host and port, honoring IPv6 bracket literals (e.g. `[::1]:9000`).
    let (host, port) = if let Some(rest) = target.strip_prefix('[') {
        let Some((h, after)) = rest.split_once(']') else {
            return Err("grpcService.target has an unclosed IPv6 bracket".to_string());
        };
        let Some(port) = after.strip_prefix(':') else {
            return Err("grpcService.target must include a port (host:port)".to_string());
        };
        (h, port)
    } else {
        let Some((h, p)) = target.rsplit_once(':') else {
            return Err("grpcService.target must include a port (host:port)".to_string());
        };
        (h, p)
    };

    if host.is_empty() {
        return Err("grpcService.target host must not be empty".to_string());
    }
    match port.parse::<u16>() {
        Ok(p) if crate::gwapi_types::is_valid_port(p as i64) => Ok(()),
        _ => Err(format!(
            "grpcService.target port must be a number in {}..={}",
            crate::gwapi_types::PORT_MIN,
            crate::gwapi_types::PORT_MAX
        )),
    }
}

impl PluginValidation for ExtProcConfig {
    fn validation_error(&self) -> Option<String> {
        if let Err(e) = validate_target(&self.grpc_service.target) {
            return Some(e);
        }
        if self.grpc_service.timeout_ms == 0 {
            return Some("grpcService.timeoutMs must be greater than 0".to_string());
        }
        // Upper bound: the timeout also caps how long a stalled processor holds
        // the request path before the fail mode kicks in.
        if self.grpc_service.timeout_ms > MAX_TIMEOUT_MS {
            return Some(format!("grpcService.timeoutMs must not exceed {MAX_TIMEOUT_MS}"));
        }
        // Reject unsupported body modes loudly so a user never configures a
        // silently ignored mode.
        if !self.processing_mode.request_body_mode.is_supported_request() {
            return Some("processingMode.requestBodyMode supports NONE, SKIP, BUFFERED, or STREAMED".to_string());
        }
        if !self.processing_mode.response_body_mode.is_supported_response() {
            return Some(
                "processingMode.responseBodyMode supports NONE, SKIP, or STREAMED; \
                 BUFFERED and BUFFERED_PARTIAL would aggregate the upstream response"
                    .to_string(),
            );
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_base() -> ExtProcConfig {
        ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target: "ext-proc:9000".to_string(),
                timeout_ms: 200,
                tls: TlsConfig::default(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn validation_error_returns_field_error_without_validate_call() {
        // Default config has an empty target — must be rejected statelessly.
        let config = ExtProcConfig::default();
        assert!(config.validation_error().is_some());
    }

    #[test]
    fn rejects_empty_target() {
        let config = ExtProcConfig::default();
        assert_eq!(
            config.validation_error().as_deref(),
            Some("grpcService.target is required")
        );
    }

    #[test]
    fn rejects_url_target() {
        let mut config = valid_base();
        config.grpc_service.target = "http://ext-proc:9000".to_string();
        assert!(config.validation_error().unwrap().contains("not a URL"));
    }

    #[test]
    fn rejects_target_without_port() {
        let mut config = valid_base();
        config.grpc_service.target = "ext-proc".to_string();
        assert!(config.validation_error().unwrap().contains("host:port"));
    }

    #[test]
    fn rejects_malformed_targets() {
        for bad in [
            ":9000",
            "host:",
            "host:9000/path",
            "host:abc",
            "host:0",
            "host:70000",
            "[::1",
        ] {
            let mut config = valid_base();
            config.grpc_service.target = bad.to_string();
            assert!(
                config.validation_error().is_some(),
                "expected target {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn accepts_ipv6_target() {
        let mut config = valid_base();
        config.grpc_service.target = "[::1]:9000".to_string();
        assert!(config.validation_error().is_none(), "IPv6 host:port should be valid");
    }

    #[test]
    fn rejects_zero_timeout() {
        let mut config = valid_base();
        config.grpc_service.timeout_ms = 0;
        assert!(config.validation_error().unwrap().contains("timeoutMs"));
    }

    #[test]
    fn rejects_timeout_over_ceiling() {
        let mut config = valid_base();
        config.grpc_service.timeout_ms = MAX_TIMEOUT_MS + 1;
        assert!(config.validation_error().unwrap().contains("timeoutMs"));
        // At the ceiling is allowed.
        config.grpc_service.timeout_ms = MAX_TIMEOUT_MS;
        assert!(config.validation_error().is_none());
    }

    #[test]
    fn accepts_streamed_request_body() {
        let mut config = valid_base();
        config.processing_mode.request_body_mode = BodySendMode::Streamed;
        assert!(config.validation_error().is_none());
    }

    #[test]
    fn accepts_streamed_response_body() {
        let mut config = valid_base();
        config.processing_mode.response_body_mode = BodySendMode::Streamed;
        assert!(config.validation_error().is_none());
    }

    #[test]
    fn accepts_buffered_request_body() {
        let mut config = valid_base();
        config.processing_mode.request_body_mode = BodySendMode::Buffered;
        assert!(config.validation_error().is_none());
    }

    #[test]
    fn rejects_buffered_response_body() {
        let mut config = valid_base();
        config.processing_mode.response_body_mode = BodySendMode::Buffered;
        let err = config.validation_error().unwrap();
        assert!(err.contains("responseBodyMode"), "got: {err}");
    }

    #[test]
    fn rejects_buffered_partial_response_body() {
        let mut config = valid_base();
        config.processing_mode.response_body_mode = BodySendMode::BufferedPartial;
        let err = config.validation_error().unwrap();
        assert!(err.contains("responseBodyMode"), "got: {err}");
    }

    #[test]
    fn accepts_valid_header_only_config() {
        let mut config = valid_base();
        config.processing_mode.response_header_mode = HeaderSendMode::Send;
        assert!(config.validation_error().is_none());
    }

    #[test]
    fn deserializes_defaults() {
        let json = r#"{"grpcService":{"target":"ext-proc:9000"}}"#;
        let config: ExtProcConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.grpc_service.timeout_ms, 200);
        assert!(!config.grpc_service.tls.enabled);
        assert_eq!(config.processing_mode.request_header_mode, HeaderSendMode::Send);
        assert_eq!(config.processing_mode.response_header_mode, HeaderSendMode::Skip);
        assert!(!config.failure_mode_allow);
    }

    #[test]
    fn extproc_grpc_accepts_verify_and_validation() {
        let c: GrpcServiceConfig = serde_json::from_value(serde_json::json!({
            "target":"ep:9000","tls":{"enabled":true,"verify":false,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]}}
        }))
        .unwrap();
        assert!(c.tls.enabled);
        assert_eq!(c.tls.outbound.verify, Some(false));
    }

    #[test]
    fn deserializes_screaming_snake_modes() {
        let json = r#"{
            "grpcService":{"target":"ext-proc:9000","timeoutMs":150,"tls":{"enabled":true}},
            "processingMode":{
                "requestHeaderMode":"SEND",
                "responseHeaderMode":"SEND",
                "requestBodyMode":"SKIP",
                "responseBodyMode":"NONE"
            },
            "failureModeAllow":true
        }"#;
        let config: ExtProcConfig = serde_json::from_str(json).unwrap();
        assert!(config.grpc_service.tls.enabled);
        assert_eq!(config.grpc_service.timeout_ms, 150);
        assert_eq!(config.processing_mode.response_header_mode, HeaderSendMode::Send);
        assert!(config.failure_mode_allow);
        assert!(config.validation_error().is_none());
    }
}
