use bytes::Bytes;
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
///
/// Deliberately NOT `Serialize`/`Deserialize`/`JsonSchema`: this is a
/// process-local plugin return value, never a configuration or wire type. Not
/// deriving them is what lets [`Self::ErrResponse`] carry a clone-cheap
/// [`Bytes`] body instead of an owned `String`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRunningResult {
    /// Just nothing, continue to run other plugins
    Nothing,
    /// Filter running good, and should continue to run other plugins
    GoodNext,
    /// Filter judged that the request should be stopped here, no other plugins should run.
    ///
    /// **Contract** (enforced by code review, not by the type system): only an
    /// inherently dynamic response may use this result, and the plugin MUST
    /// have written the response to the session before returning. Ordinary
    /// rejections, including DSL `terminate()`, use [`Self::ErrResponse`].
    ///
    /// **Runtime fail-safe**: if the plugin returns `ErrTerminateRequest` without
    /// having started a response write, the runtime will
    /// automatically write a 502 Bad Gateway (HTTP) or gRPC INTERNAL (gRPC) and
    /// record [`crate::EdgionErrorCode::PluginTerminateWithoutResponse`] in the access log.
    /// This is a fail-safe — operators should treat the access-log marker as a signal
    /// to fix the offending plugin.
    ///
    /// If a plugin cannot write a response at its current stage (e.g. the sync
    /// `UpstreamResponseFilter` hook where `await` is unavailable), use
    /// [`Self::ErrResponse`] so the runtime owns the response or stage denial.
    /// A naked terminate is a plugin bug, not a supported rejection mechanism.
    ErrTerminateRequest,
    /// Filter returns an error response with custom status and body
    ///
    /// The body is [`Bytes`] rather than `String` so a generation-owned static
    /// body crosses this boundary without a per-request copy: a configured body
    /// prepared once at plugin construction clones as a refcount bump, and a
    /// fixed literal body is built with `Bytes::from_static` and never
    /// allocates. Dynamically built bodies stay a single allocation —
    /// `Bytes::from(String)` and `Bytes::from(Vec<u8>)` are moves. The response
    /// writer already sinks into `Bytes`, so the whole path is copy-free.
    ///
    /// `None` and `Some` of an empty `Bytes` take different write paths — `None`
    /// ends the response on the header write, `Some(empty)` follows it with a
    /// zero-length body write — but both frame as `content-length: 0`, so the
    /// message the client sees is the same. Prefer `None` when there is no body.
    ErrResponse { status: u16, body: Option<Bytes> },
    /// Filter returns a gRPC error (HTTP 200 + grpc-status/grpc-message trailers)
    ///
    /// The message stays a `String`: it is emitted as the `grpc-message`
    /// trailer, which is a header value, not a response body.
    ErrGrpcError { grpc_status: u16, message: Option<String> },
}
