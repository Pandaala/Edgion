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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

    /// Ignore processor-authored immediate responses and deactivate ExtProc
    /// for the remainder of the request.
    #[serde(default)]
    pub disable_immediate_response: bool,

    /// Allow a processor to select an explicitly allow-listed reduction of
    /// the configured processing mode from a header verdict.
    #[serde(default)]
    pub allow_mode_override: bool,

    /// Exact reduction-only modes accepted from the processor. Required when
    /// `allowModeOverride` is true and bounded to eight entries.
    #[serde(default)]
    pub allowed_override_modes: Vec<ProcessingMode>,
}

/// gRPC connection settings for the external processing service.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

/// Parsed cold ExtProc target; the connector keeps the authored host spelling.
pub struct ExtProcTarget<'a> {
    pub host: &'a str,
    pub port: u16,
    pub authority: String,
}

impl GrpcServiceConfig {
    /// Parse and validate the effective URI used by both plain and custom TLS channels.
    pub fn parse_target(&self) -> Result<ExtProcTarget<'_>, String> {
        parse_target(&self.target)
    }
}

/// Per-phase processing control, mirroring Envoy's `ProcessingMode`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

    /// Whether to send real upstream response trailers to the processor.
    #[serde(default = "default_header_skip")]
    pub response_trailer_mode: HeaderSendMode,
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
            response_trailer_mode: HeaderSendMode::Skip,
        }
    }
}

impl ProcessingMode {
    /// Canonical representation used for exact override-list membership.
    /// Request headers are already consumed when an override is observed, and
    /// config-only body `SKIP` is wire-equivalent to `NONE`.
    pub fn normalized_override(&self) -> Self {
        Self {
            request_header_mode: HeaderSendMode::Skip,
            response_header_mode: self.response_header_mode,
            request_body_mode: normalize_body(self.request_body_mode),
            response_body_mode: normalize_body(self.response_body_mode),
            response_trailer_mode: self.response_trailer_mode,
        }
    }

    pub fn is_reduction_of(&self, base: &Self) -> bool {
        let requested = self.normalized_override();
        let base = base.normalized_override();
        header_is_reduction(requested.response_header_mode, base.response_header_mode)
            && body_is_reduction(requested.request_body_mode, base.request_body_mode)
            && body_is_reduction(requested.response_body_mode, base.response_body_mode)
            && header_is_reduction(requested.response_trailer_mode, base.response_trailer_mode)
    }

    fn validation_error(&self, prefix: &str) -> Option<String> {
        if !self.request_body_mode.is_supported_request() {
            return Some(format!(
                "{prefix}.requestBodyMode supports NONE, SKIP, BUFFERED, or STREAMED"
            ));
        }
        if !self.response_body_mode.is_supported_response() {
            return Some(format!(
                "{prefix}.responseBodyMode supports NONE, SKIP, or STREAMED; BUFFERED and BUFFERED_PARTIAL would aggregate the upstream response"
            ));
        }
        None
    }
}

fn normalize_body(mode: BodySendMode) -> BodySendMode {
    if matches!(mode, BodySendMode::Skip) {
        BodySendMode::None
    } else {
        mode
    }
}

fn header_is_reduction(requested: HeaderSendMode, base: HeaderSendMode) -> bool {
    requested == base || matches!((base, requested), (HeaderSendMode::Send, HeaderSendMode::Skip))
}

fn body_is_reduction(requested: BodySendMode, base: BodySendMode) -> bool {
    requested == base || matches!(requested, BodySendMode::None)
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
            disable_immediate_response: false,
            allow_mode_override: false,
            allowed_override_modes: Vec::new(),
        }
    }
}

/// Validate a gRPC `host:port` authority. Rejects URLs, paths, empty host/port,
/// and non-numeric / out-of-range ports. Without this, a malformed target only
/// fails later at gateway channel-build time — the plugin is then sealed
/// fail-closed (the route 500s) instead of surfacing a clean validation error on
/// the resource's status at controller ingest. Catching it here gives the
/// operator an actionable message up front.
fn parse_target(target: &str) -> Result<ExtProcTarget<'_>, String> {
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
    let port = match port.parse::<u16>() {
        Ok(p) if crate::gwapi_types::is_valid_port(p as i64) => p,
        _ => {
            return Err(format!(
                "grpcService.target port must be a number in {}..={}",
                crate::gwapi_types::PORT_MIN,
                crate::gwapi_types::PORT_MAX
            ))
        }
    };
    let authority = match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(ip)) => format!("[{ip}]:{port}"),
        Ok(std::net::IpAddr::V4(ip)) => format!("{ip}:{port}"),
        Err(_) => format!("{}:{port}", host.to_ascii_lowercase()),
    };
    // Validate the complete URI: its size bound also includes the scheme bytes.
    // The custom TLS connector uses the same synthetic http URI as plaintext.
    format!("http://{authority}")
        .parse::<http::Uri>()
        .map_err(|_| "grpcService.target cannot form a valid channel URI".to_string())?;
    Ok(ExtProcTarget { host, port, authority })
}

impl PluginValidation for ExtProcConfig {
    fn validation_error(&self) -> Option<String> {
        if let Err(e) = self.grpc_service.parse_target() {
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
        if let Some(error) = self.processing_mode.validation_error("processingMode") {
            return Some(error);
        }
        if !self.allow_mode_override && !self.allowed_override_modes.is_empty() {
            return Some("allowedOverrideModes must be empty when allowModeOverride is false".to_string());
        }
        if self.allow_mode_override && !(1..=8).contains(&self.allowed_override_modes.len()) {
            return Some(
                "allowedOverrideModes must contain 1 through 8 entries when allowModeOverride is true".to_string(),
            );
        }
        let mut normalized = Vec::with_capacity(self.allowed_override_modes.len());
        for (index, mode) in self.allowed_override_modes.iter().enumerate() {
            if let Some(error) = mode.validation_error(&format!("allowedOverrideModes[{index}]")) {
                return Some(error);
            }
            let mode = mode.normalized_override();
            if !mode.is_reduction_of(&self.processing_mode) {
                return Some(format!(
                    "allowedOverrideModes[{index}] must be a reduction of processingMode"
                ));
            }
            if normalized.contains(&mode) {
                return Some(format!(
                    "allowedOverrideModes[{index}] duplicates an earlier normalized mode"
                ));
            }
            normalized.push(mode);
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/ext_proc_test.rs"]
mod tests;
