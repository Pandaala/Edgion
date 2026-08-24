use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Filter configuration
/// Each stage uses a fixed array order as priority
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PluginConf {
    pub enable: bool,
    pub name: String,
    pub config: serde_json::Value,
}

/// Filter running stage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum PluginRunningStage {
    /// request_filter (async)
    Request,
    /// upstream_response_filter (sync)
    UpstreamResponseFilter,
    /// response_filter (async)
    UpstreamResponse,
}

/// Filter running result
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PluginRunningResult {
    /// Just nothing, continue to run other plugins
    Nothing,
    /// Filter running good, and should continue to run other plugins
    GoodNext,
    /// Filter judged that the request should be stopped here, no other plugins should run.
    ///
    /// **Convention** (enforced by code review, not by the type system): any plugin
    /// that returns `ErrTerminateRequest` SHOULD have already written the response
    /// (header + body) to the session before returning.
    ///
    /// **Runtime fail-safe**: if the plugin returns `ErrTerminateRequest` without
    /// having started a response write, the runtime will
    /// automatically write a 502 Bad Gateway (HTTP) or gRPC INTERNAL (gRPC) and
    /// record [`EdgionErrorCode::PluginTerminateWithoutResponse`] in the access log.
    /// This is a fail-safe — operators should treat the access-log marker as a signal
    /// to fix the offending plugin.
    ///
    /// If a plugin cannot write a response at its current stage (e.g. the sync
    /// `UpstreamResponseFilter` hook where `await` is unavailable), prefer
    /// [`ErrResponse`] so the runtime can emit the status/body downstream, or
    /// document explicitly why a naked terminate is acceptable in that path.
    ErrTerminateRequest,
    /// Filter returns an error response with custom status and body
    ErrResponse {
        status: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<String>,
    },
    /// Filter returns a gRPC error (HTTP 200 + grpc-status/grpc-message trailers)
    ErrGrpcError {
        grpc_status: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}
