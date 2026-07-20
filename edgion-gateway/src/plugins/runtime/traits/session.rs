//! PluginSession trait - shared across all filter stages

use crate::ctx::ClientCertInfo;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::log::{EdgionPluginsLog, EdgionPluginsLogToken, PluginLog};
use crate::{EdgionHttpContext, MirrorState, SetJumpOutcome, UpstreamJumpTarget};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::common::{KeyGet, KeySet};
pub use edgion_resources::resources::edgion_plugins::DyeOutcome;
use edgion_resources::EdgionStatus;
use pingora_http::ResponseHeader;
use std::sync::Arc;

pub type PluginSessionError = Box<dyn std::error::Error + Send + Sync>;
pub type PluginSessionResult<T> = Result<T, PluginSessionError>;

/// Why a body could not be read.
///
/// Pingora streams bodies chunk-by-chunk and does not buffer the full body, so
/// the buffering runtime (P0 `01-body-processing-runtime.md`) is not yet built;
/// [`PluginSession::read_request_body`] / `read_response_body` therefore default
/// to [`BodyReadError::Unimplemented`]. The body gate maps any variant to the
/// plugin's configured fail-open / fail-close policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyReadError {
    /// Full-body buffering is not implemented on this binary (current default).
    Unimplemented,
    /// Buffering was not requested for this request, so no bytes are retained.
    NotBuffered,
    /// The body exceeded the configured buffer cap and was not retained.
    /// The body gate maps this to HTTP 413 (client error), unlike other
    /// fail-close reasons which map to 500.
    TooLarge { limit: u64 },
    /// The body stream ended prematurely / was aborted.
    Incomplete,
}

impl std::fmt::Display for BodyReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyReadError::Unimplemented => write!(f, "body buffering not implemented"),
            BodyReadError::NotBuffered => write!(f, "body not buffered"),
            BodyReadError::TooLarge { limit } => write!(f, "body exceeds buffer cap of {limit} bytes"),
            BodyReadError::Incomplete => write!(f, "body stream incomplete"),
        }
    }
}

impl std::error::Error for BodyReadError {}

/// Result of reading a buffered body: the bytes (a cheap `Arc`-backed handle, so
/// cloning is a refcount bump, not a copy), or the reason it is unavailable.
pub type BodyReadResult = Result<Bytes, BodyReadError>;

/// Which matched route a `matched_route_meta` lookup targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteProto {
    Http,
    Grpc,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait PluginSession: Send + Sync {
    /// Get a request header value by name
    fn header_value(&self, name: &str) -> Option<String>;

    /// Get all request headers as key-value pairs.
    ///
    /// Returns a Vec of (name, value) tuples. Multi-value headers are
    /// represented as separate entries with the same name.
    /// Used by plugins that need to forward/inspect all headers (e.g., ForwardAuth).
    fn request_headers(&self) -> Vec<(String, String)>;

    // ========== Condition evaluation methods (read-only) ==========

    /// Get a query parameter value by name
    fn get_query_param(&self, name: &str) -> Option<String>;

    /// Get a cookie value by name
    fn get_cookie(&self, name: &str) -> Option<String>;

    /// Get the request path (without query string)
    fn get_path(&self) -> &str;

    /// Get the entry-time raw path (frozen, immune to rewrites).
    /// Use for audit, security investigation, special raw-form deny rules.
    fn get_request_path(&self) -> &str;

    /// Get the current normalized full URI (current path + current query).
    fn get_uri(&self) -> String;

    /// Get the entry-time raw full URI (entry path + entry query, frozen).
    fn get_request_uri(&self) -> String;

    /// Get the request query string (without leading '?')
    fn get_query(&self) -> Option<String>;

    /// Get the HTTP method as &str (zero-copy)
    fn get_method(&self) -> &str;

    /// Get a context variable by key (set by plugins like KeySet)
    fn get_ctx_var(&self, key: &str) -> Option<String>;

    /// Set a context variable (for plugins to pass data downstream)
    fn set_ctx_var(&mut self, key: &str, value: &str) -> PluginSessionResult<()>;

    /// Remove a context variable
    fn remove_ctx_var(&mut self, key: &str) -> PluginSessionResult<()>;

    /// Get a path parameter by name (lazy extraction from route pattern)
    ///
    /// For route pattern "/api/:uid/profile" and request path "/api/123/profile",
    /// `get_path_param("uid")` returns `Some("123")`.
    ///
    /// This uses lazy extraction - parameters are only parsed on first call
    /// and then cached for subsequent calls.
    fn get_path_param(&mut self, name: &str) -> Option<String>;

    /// Read identity/metadata of the matched route for template variables
    /// `${matchedHttpRoute:<selector>}` / `${matchedGrpcRoute:<selector>}`.
    /// Selectors: `name`, `namespace`, `label.<key>`, `annotation.<key>`.
    /// Returns `None` when no route matched, the route handle is absent, or the
    /// selector/key is unknown.
    fn matched_route_meta(&self, proto: RouteProto, selector: &str) -> Option<String>;

    async fn write_response_header(
        &mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> PluginSessionResult<()>;

    /// Set a response header (will be added when write_response_header is called)
    fn set_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Append a value to an existing response header (e.g., Vary: Origin)
    fn append_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Remove a response header (for ResponseHeaderModifier)
    fn remove_response_header(&mut self, name: &str) -> PluginSessionResult<()>;

    /// Get a response header value by name (for ResponseRewrite)
    fn get_response_header(&self, name: &str) -> Option<String>;

    /// Get all response header names (for DSL resp.header_names())
    fn response_header_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// Get all response header (name, value) pairs, including duplicates.
    ///
    /// Unlike `response_header_names()` + `get_response_header()` (which yields
    /// each name once and only its first value), this preserves multi-valued
    /// headers such as `Set-Cookie`. Default returns empty.
    fn response_headers(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Get the response status code (available in upstream_response_filter stage)
    /// Returns None if response has not been received yet (request stage)
    fn get_response_status(&self) -> Option<u16>;

    /// Set the response status code (for ResponseRewrite)
    fn set_response_status(&mut self, status: u16) -> PluginSessionResult<()>;

    /// Set a request header (for upstream)
    fn set_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Append a value to an existing request header (e.g., X-Forwarded-For)
    fn append_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Remove a request header (e.g., hide credentials)
    fn remove_request_header(&mut self, name: &str) -> PluginSessionResult<()>;

    /// Replace the request query string (the part after `?`) while leaving the
    /// request path untouched. A non-empty `query` (no leading `?`) becomes the
    /// new query; an empty string removes the query entirely.
    ///
    /// Unlike `set_upstream_uri`, this does NOT mark the URI as finalized — the
    /// raw-vs-normalized path-forwarding decision is still made later at the
    /// upstream-peer stage. Used by credential-hiding to strip a query
    /// parameter without overriding that path decision.
    fn set_request_query(&mut self, query: &str) -> PluginSessionResult<()>;

    /// Set the upstream URI (for proxy rewrite)
    fn set_upstream_uri(&mut self, uri: &str) -> PluginSessionResult<()>;

    /// Set the upstream host (for proxy rewrite)
    fn set_upstream_host(&mut self, host: &str) -> PluginSessionResult<()>;

    /// Set the upstream HTTP method (for proxy rewrite)
    fn set_upstream_method(&mut self, method: &str) -> PluginSessionResult<()>;

    async fn write_response_body(&mut self, body: Option<Bytes>, end_of_stream: bool) -> PluginSessionResult<()>;

    async fn shutdown(&mut self);

    /// Get client IP address (TCP direct connection, without port)
    fn client_addr(&self) -> &str;

    /// Get remote address (real client IP, extracted from proxy headers)
    fn remote_addr(&self) -> &str;

    /// Set remote address (real client IP)
    ///
    /// Used by RealIp plugin to update the extracted real client IP.
    fn set_remote_addr(&mut self, addr: &str) -> PluginSessionResult<()>;

    /// Overwrite the request's trace id (`ctx.request_info.x_trace_id`).
    ///
    /// Used by the TraceContext plugin to align `x-trace-id` (access log +
    /// response header) with the W3C `trace_id`.
    fn set_trace_id(&mut self, value: &str) -> PluginSessionResult<()>;

    /// Get mTLS client certificate info extracted during handshake.
    ///
    /// Returns None when:
    /// - request is plain HTTP (no TLS)
    /// - connection is TLS without client cert
    /// - EdgionTls did not enable `edgion.io/expose-client-cert`
    fn client_cert_info(&self) -> Option<ClientCertInfo> {
        None
    }

    /// Get the discovered protocol type (e.g., "grpc", "grpc-web", "websocket", "http").
    /// Used by DSL `req.protocol`.
    fn discover_protocol(&self) -> String {
        "http".to_string()
    }

    /// Get the gRPC service name parsed from the request path.
    /// Returns None for non-gRPC requests.
    fn grpc_service(&self) -> Option<String> {
        None
    }

    /// Get the gRPC method name parsed from the request path.
    /// Returns None for non-gRPC requests.
    fn grpc_method(&self) -> Option<String> {
        None
    }

    /// Get reference to EdgionHttpContext (for access log generation)
    fn ctx(&self) -> &EdgionHttpContext;

    /// Append an EdgionStatus error to the gateway context so it surfaces in the
    /// access log `errors` array. Narrow interface — plugins cannot mutate other
    /// ctx fields; only the error list.
    fn add_error(&mut self, status: EdgionStatus);

    /// Look up the resolved target `PluginRuntime` for an `ExtensionRef` at the
    /// given index (assigned at route build time by
    /// `PluginRuntime::from_httproute_filters_for_rule` /
    /// `from_grpcroute_filters_for_rule`). Returns `None` if the ref was
    /// dangling at resolve time, or if `index` is out of range (defensive).
    ///
    /// Default impl returns `None` — used by adapters that don't carry an
    /// `EdgionHttpContext` (e.g. some test mocks). Production path is
    /// `PingoraSessionAdapter`, which reads `ctx.resolved_route_ext_refs`.
    fn resolved_route_ext_ref(&self, _index: usize) -> Option<std::sync::Arc<crate::plugins::runtime::PluginRuntime>> {
        None
    }

    /// Push EdgionPlugins execution log to pending list (for ExtensionRef)
    fn push_edgion_plugins_log(&mut self, log: EdgionPluginsLog);

    /// Start a new EdgionPluginsLog and return a token for safe log pushing.
    ///
    /// This creates an empty EdgionPluginsLog entry in the pending list and returns
    /// a token that can be used with `push_to_edgion_plugins_log` to append logs.
    /// The token records the current depth to prevent misuse in nested scopes.
    fn start_edgion_plugins_log(&mut self, name: String) -> EdgionPluginsLogToken;

    /// Push a PluginLog to the EdgionPluginsLog identified by the token.
    ///
    /// # Panics
    /// In debug builds, panics if the current depth doesn't match the token's depth,
    /// indicating the token is being used in the wrong scope.
    fn push_to_edgion_plugins_log(&mut self, token: &EdgionPluginsLogToken, log: PluginLog);

    // ========== KeyGet/KeySet unified accessor methods ==========

    /// Get a value using KeyGet accessor
    ///
    /// This is the unified way to extract values from the session context.
    /// Used by RateLimit, Conditions, and other plugins.
    ///
    /// This method is async to support remote key sources (e.g., Webhook).
    /// For local key sources (Header, Cookie, Query, etc.), the async overhead
    /// is negligible (nanosecond-level state machine creation + 1 poll(Ready)).
    ///
    /// # Returns
    /// * `Some(String)` - The extracted value
    /// * `None` - If the value doesn't exist or is empty (for ClientIp)
    async fn key_get(&self, key: &KeyGet) -> Option<String>;

    /// Synchronous version of key_get for local key sources only.
    ///
    /// Used by sync filter contexts (UpstreamResponseFilter, UpstreamResponseBodyFilter)
    /// where async is not available. Only supports local variants (Header, Cookie, Query,
    /// Path, Method, Ctx, ClientIp, ClientIpAndPath). Returns None for Webhook variants.
    fn key_get_local(&self, key: &KeyGet) -> Option<String>;

    /// Set a value using KeySet accessor
    ///
    /// This is the unified way to write values to the session context.
    /// When `value` is `None`, the key is removed/cleared.
    ///
    /// # Arguments
    /// * `key` - The KeySet specifying where to write
    /// * `value` - The value to set, or None to remove
    fn key_set(&mut self, key: &KeySet, value: Option<String>) -> PluginSessionResult<()>;

    /// Set the unified upstream-jump slot. First-write-wins (see `SetJumpOutcome`).
    /// Plugins should match on the returned outcome to log the rejection on
    /// their own `PluginLog` line:
    ///   `Set` -> existing per-plugin status string
    ///   `Rejected` -> `log.push("jump locked, rejected")`
    fn set_upstream_jump(&mut self, target: UpstreamJumpTarget) -> SetJumpOutcome;

    /// Set request mirror state used by request_body_filter tee.
    fn set_mirror_state(&mut self, state: MirrorState);

    // ========== Full-body buffering (OpenResty-style, lazy) ==========
    //
    // Single lazy accessor per direction (mirrors `ngx.req.read_body` +
    // `ngx.req.get_body_data` folded into one call): the first call force-buffers
    // the body (capped by `max_bytes`) and every call returns the buffered bytes.
    // Async + `&mut self` because buffering drains the downstream stream and
    // populates ctx. The returned `Bytes` is an immutable `Arc`-backed handle, so
    // returning it by value is a refcount bump, not a copy — no borrow of the
    // session is held, which keeps the method mockable and easy to compose.
    //
    // Both default to `Err(BodyReadError::Unimplemented)`: Pingora does not buffer
    // bodies yet. The real `PingoraSessionAdapter` overrides land with the P0
    // buffering runtime.

    /// Read the full request body, buffering it on first call so it is available
    /// during the request phase, before the request is proxied upstream.
    /// `max_bytes` caps the buffer on the first (buffering) call (`None` = gateway
    /// default cap); later calls return the already-buffered bytes.
    async fn read_request_body(&mut self, _max_bytes: Option<u64>) -> BodyReadResult {
        Err(BodyReadError::Unimplemented)
    }

    /// Read the full upstream response body, buffering it on first call.
    /// `max_bytes` caps the buffer on the first (buffering) call.
    async fn read_response_body(&mut self, _max_bytes: Option<u64>) -> BodyReadResult {
        Err(BodyReadError::Unimplemented)
    }

    // ========== Entry-level dye suite (traffic tagging) ==========

    /// Install the dye plan of the plugin entry about to be invoked. Every
    /// call site that invokes a plugin's stage method MUST call this
    /// immediately before that invocation (mirroring the four
    /// `PluginRuntime::run_*_plugins` stage loops) — including indirect
    /// invocation through an `ExtensionRef` bundle, which iterates a nested
    /// `PluginRuntime`'s filters one by one rather than going through those
    /// loops. Default no-op for sessions that don't participate in dye
    /// rendering (e.g. `MockPluginSession`, tests).
    fn install_dye_plan(&mut self, _plan: Option<Arc<CompiledDyePlan>>) {}

    /// Once-per-name anti-spoof strip: remove the client-supplied value of
    /// each request-direction rule name the currently-installed plan
    /// declares, the first time any entry in the request declares that name.
    /// Called immediately after `install_dye_plan`, before the plugin's kill
    /// switch / gating / conditions — request-stage call sites only. Default
    /// no-op.
    fn strip_request_dye_once(&mut self) {}

    /// Record a plugin outcome for entry-level dye rendering. Success passes
    /// the resolved selection value; failures pass the cause (lowercase
    /// kebab, plugin-authored). No-op unless the current entry configured a
    /// matching dye rule. Call BEFORE writing any self-authored response.
    fn record_outcome(&mut self, outcome: DyeOutcome, value: &str) {
        let _ = (outcome, value);
    }
}
