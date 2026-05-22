//! PluginSession trait - shared across all filter stages

use crate::core::gateway::plugins::runtime::log::{EdgionPluginsLog, EdgionPluginsLogToken, PluginLog};
use crate::types::common::{KeyGet, KeySet};
use crate::types::ctx::ClientCertInfo;
use crate::types::EdgionStatus;
use crate::types::{EdgionHttpContext, MirrorState, SetJumpOutcome, UpstreamJumpTarget};
use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

pub type PluginSessionError = Box<dyn std::error::Error + Send + Sync>;
pub type PluginSessionResult<T> = Result<T, PluginSessionError>;

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

    /// Get the HTTP method (returns owned String for compatibility)
    fn method(&self) -> String;

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

    /// Get the HTTP method as &str (more efficient than method())
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

    async fn write_response_header(
        &mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> PluginSessionResult<()>;

    fn write_response_header_boxed<'a>(
        &'a mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PluginSessionResult<()>> + Send + 'a>>;

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
    fn resolved_route_ext_ref(
        &self,
        _index: usize,
    ) -> Option<std::sync::Arc<crate::core::gateway::plugins::runtime::PluginRuntime>> {
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
}
