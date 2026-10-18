//! PluginSession trait - shared across all filter stages

use super::ai_session::AiSessionCapability;
use super::waf_session::WafSessionCapability;
use crate::ctx::ClientCertInfo;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::log::{EdgionPluginsLogToken, PluginLog};
use crate::request_body::{
    BodyOperationError, BodyReadError, BodyReadResult, BodyScanResult, RequestBodyChunkConsumer,
    RequestBodySnapshotReader,
};
use crate::{EdgionHttpContext, MirrorState, PendingHeaderOp, SetJumpOutcome, UpstreamJumpTarget};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::common::{KeyGet, KeySet};
pub use edgion_resources::resources::edgion_plugins::DyeOutcome;
use edgion_resources::EdgionErrorCode;
use pingora_http::ResponseHeader;
use std::sync::Arc;

/// Optional cache nomination never changes ordinary request handling.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResponseCacheIntentAdmission {
    Admitted,
    AlreadyAdmitted,
    Bypassed,
    #[default]
    Unsupported,
}

pub type PluginSessionError = Box<dyn std::error::Error + Send + Sync>;
pub type PluginSessionResult<T> = Result<T, PluginSessionError>;

pub(crate) fn empty_response_header_map() -> &'static http::HeaderMap {
    static EMPTY: std::sync::OnceLock<http::HeaderMap> = std::sync::OnceLock::new();
    EMPTY.get_or_init(http::HeaderMap::new)
}

#[inline]
const fn safe_default_downstream_response_trailers_representable() -> bool {
    false
}

/// Result of a canonical policy-bounded native capture. A partial source remains
/// owned by native first-attempt replay and is never a complete request cache.
pub struct RequestBodyInspectionCapture {
    pub captured_len: u64,
    pub complete: bool,
    pub storage: Option<Arc<crate::request_body::BodyStorageView>>,
}

/// Which matched route a `matched_route_meta` lookup targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteProto {
    Http,
    Grpc,
}

#[cfg_attr(test, mockall::automock)]
#[cfg_attr(test, allow(clippy::disallowed_types))]
#[async_trait]
pub trait PluginSession: Send + Sync {
    /// Transfer a Wasm execution to the request's typed lifecycle owner.
    /// Unsupported adapters reject the offer; they must not silently lose it.
    fn install_wasm_binding(&mut self, binding: Arc<crate::plugins::http::wasm::request::WasmRequestBinding>) -> bool {
        drop(binding);
        false
    }

    fn wasm_binding(&self, identity: &Arc<()>) -> Option<Arc<crate::plugins::http::wasm::request::WasmRequestBinding>> {
        self.ctx().wasm.as_deref().and_then(|state| state.get(identity))
    }
    /// Monotone failure signal for the outer response transport owner.
    fn mark_ip_body_denied(&mut self);
    /// Evaluate only an actual-owner cold ACL plan against this dispatch's captured view.
    fn ip_acl_decision(
        &self,
        acl: &crate::plugins::common::ip_acl::CompiledIpAcl,
        ip: Option<std::net::IpAddr>,
    ) -> edgion_resources::resources::edgion_plugins::AccessDecision;
    /// Capture once for this dispatch; scalar readers never expose resource loans.
    fn ensure_config_data_view(&mut self) {}
    fn ip_list_availability(
        &self,
        _key: &crate::plugins::runtime::conditions::ip_refs::IpListReadKey,
    ) -> crate::plugins::runtime::conditions::ip_refs::IpListAvailability {
        crate::plugins::runtime::conditions::ip_refs::IpListAvailability::Unavailable
    }
    fn ip_list_contains(
        &self,
        _key: &crate::plugins::runtime::conditions::ip_refs::IpListReadKey,
        _ip: std::net::IpAddr,
    ) -> Option<bool> {
        None
    }
    fn config_data_profile_index(
        &self,
        _key: &str,
        _default: &str,
        _names: &std::collections::HashMap<String, usize>,
    ) -> crate::plugins::runtime::conditions::ip_refs::ProfileSelection {
        crate::plugins::runtime::conditions::ip_refs::ProfileSelection::Unavailable
    }

    /// Borrowed access to the live request header map.
    ///
    /// This is the zero-copy read seam: every borrowed accessor in
    /// [`PluginSessionHeaderExt`](super::header_ext::PluginSessionHeaderExt)
    /// is derived from it, so the mock surface grows by this one method no
    /// matter how rich that extension trait becomes.
    ///
    /// A bare reference keeps mock expectations backed by owned header maps.
    /// Nested optional borrows use explicitly named self lifetimes, as in
    /// `request_query`, `get_request_method`, and `client_cert_info`; their
    /// mock expectations require static fixtures. Borrowed header views remain
    /// in the extension trait; richer borrowed views need feasibility checks.
    ///
    /// Request-header writes go straight into the live header map
    /// (`set_request_header` / `append_request_header` / `remove_request_header`),
    /// so this observes exactly the same state as [`Self::header_value`].
    /// Deferred credential headers are applied to the *upstream* request only
    /// and are invisible to both.
    fn request_header_map(&self) -> &http::HeaderMap;

    /// Borrow the live typed request URI, including native HTTP/2 authority.
    /// This is the same URI owned by the transport; no parsing or normalization.
    /// Unlike get_request_uri(), it is not the frozen origin-form entry string.
    fn request_uri(&self) -> &http::Uri;

    /// Get a request header value by name, as an owned `String`.
    ///
    /// Prefer the borrowed accessors on
    /// [`PluginSessionHeaderExt`](super::header_ext::PluginSessionHeaderExt)
    /// unless the value must outlive an immutable borrow of the session -
    /// i.e. it is moved across an `.await`, handed to `spawn_blocking`, or
    /// held while the session is used mutably.
    fn header_value(&self, name: &str) -> Option<String>;

    /// Get all request headers as key-value pairs.
    ///
    /// Returns a Vec of (name, value) tuples. Multi-value headers are
    /// represented as separate entries with the same name.
    /// Used by plugins that need to forward/inspect all headers (e.g., ForwardAuth).
    fn request_headers(&self) -> Vec<(String, String)>;

    /// Get all request headers without requiring UTF-8 values.
    ///
    /// The default preserves compatibility with alternate adapters. The
    /// Pingora adapter overrides it so security inspection never drops an
    /// invalid-byte header value.
    fn request_headers_bytes(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.request_headers()
            .into_iter()
            .map(|(name, value)| (name.into_bytes(), value.into_bytes()))
            .collect()
    }

    /// Get every value for one request header without combining duplicates.
    ///
    /// `Some(empty)` means the header is absent. `None` means at least one
    /// matching value is not valid UTF-8 and must not be silently ignored.
    /// Adapters with raw header access should override this method so invalid
    /// values remain distinguishable from missing values.
    fn request_header_values(&self, name: &str) -> Option<Vec<String>> {
        Some(
            self.request_headers()
                .into_iter()
                .filter(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
                .map(|(_, value)| value)
                .collect(),
        )
    }

    // ========== Condition evaluation methods (read-only) ==========

    /// Borrow a decoded value from the lazily initialized frozen entry query.
    /// The borrow ends with this session read; the live rewritten URI is not the source.
    fn get_query_param_ref<'a>(&'a self, name: &str) -> Option<&'a str>;

    /// Get an owned decoded query parameter value from the frozen entry query.
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

    /// Borrow the current upstream-bound query so bounded parsers can check its
    /// size before copying. Unlike get_query_param this observes trusted rewrites.
    #[allow(clippy::needless_lifetimes)]
    fn request_query<'a>(&'a self) -> Option<&'a str>;

    /// Borrow the current HTTP method, including trusted plugin rewrites.
    fn get_method(&self) -> &str;

    /// Borrow the frozen entry method. None means entry capture never ran;
    /// callers must select an explicit missing-metadata policy, never a live fallback.
    #[allow(clippy::needless_lifetimes)]
    fn get_request_method<'a>(&'a self) -> Option<&'a str>;

    /// Get the wire HTTP version used to build SecLang REQUEST_PROTOCOL.
    fn get_http_protocol(&self) -> String {
        "HTTP/1.1".to_string()
    }

    /// Get the bounded request identity used for SecLang UNIQUE_ID.
    fn get_request_id(&self) -> String {
        String::new()
    }

    /// Begin a resource-local admission frame. Unsupported adapters remain unavailable.
    fn begin_rate_limit_local_frame(
        &mut self,
        _count: usize,
    ) -> Option<crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalFrame> {
        None
    }

    /// Restore the enclosing resource's admission frame.
    fn end_rate_limit_local_frame(
        &mut self,
        _frame: Option<crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalFrame>,
    ) {
    }

    /// Read a compiled producer slot without exposing it as a writable ctx variable.
    fn rate_limit_local_outcome(
        &self,
        _slot: usize,
    ) -> crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalOutcome {
        crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalOutcome::NotEvaluated
    }

    fn set_rate_limit_local_outcome(
        &mut self,
        _slot: usize,
        _outcome: crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalOutcome,
    ) -> bool {
        false
    }

    /// Borrow the current context value for this session read.
    /// Later plugin writes remain visible on subsequent reads.
    fn get_ctx_var_ref<'s>(&'s self, key: &str) -> Option<&'s str>;

    /// Get an owned context variable by key (set by plugins like KeySet).
    fn get_ctx_var(&self, key: &str) -> Option<String>;

    /// Set a context variable (for plugins to pass data downstream)
    fn set_ctx_var(&mut self, key: &str, value: &str) -> PluginSessionResult<()>;

    /// Remove a context variable
    fn remove_ctx_var(&mut self, key: &str) -> PluginSessionResult<()>;

    /// Get a path parameter captured from the matched route.
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

    /// Set an ordinary response header (deferred until a response exists).
    /// All response mutation methods reject the shared protected-field policy;
    /// body/transport owners maintain framing directly on their own response.
    fn set_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Replace response field lines, preserving raw bytes and sensitivity through deferred replies.
    fn set_response_header_value(&mut self, name: &str, value: http::HeaderValue) -> PluginSessionResult<()>;

    /// Append a value to an existing response header (e.g., Vary: Origin)
    fn append_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Append a response field line without converting its typed value to text.
    fn append_response_header_value(&mut self, name: &str, value: http::HeaderValue) -> PluginSessionResult<()>;

    /// Check the live response by name, independently of value encoding.
    /// Returns false before a response is bound; pending operations are not a live response.
    fn has_response_header(&self, name: &str) -> bool;

    /// Remove an ordinary response header, subject to the same policy as writes.
    fn remove_response_header(&mut self, name: &str) -> PluginSessionResult<()>;

    /// Get a response header value by name (for ResponseRewrite)
    fn get_response_header(&self, name: &str) -> Option<String>;

    /// Borrowed access to the live response header map.
    ///
    /// Before a response is bound this returns an empty map. Consumers that
    /// need to distinguish that stage must use [`Self::get_response_status`].
    /// Prefer this over the owned bulk accessors when values do not need to
    /// outlive the current synchronous plugin callback.
    fn response_header_map(&self) -> &http::HeaderMap {
        empty_response_header_map()
    }

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

    /// Get response headers without requiring UTF-8 values.
    fn response_headers_bytes(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.response_headers()
            .into_iter()
            .map(|(name, value)| (name.into_bytes(), value.into_bytes()))
            .collect()
    }

    /// Get the response status code (available in upstream_response_filter stage)
    /// Returns None if response has not been received yet (request stage)
    fn get_response_status(&self) -> Option<u16>;

    /// Set the response status code (for ResponseRewrite)
    fn set_response_status(&mut self, status: u16) -> PluginSessionResult<()>;

    /// Set a request header (for upstream)
    fn set_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Replace every field line while preserving typed value metadata, including sensitivity.
    /// Credential and identity producers must mark the value sensitive before insertion.
    fn set_request_header_value(&mut self, name: &str, value: http::HeaderValue) -> PluginSessionResult<()>;

    /// Append a value to an existing request header (e.g., X-Forwarded-For)
    fn append_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Append a request field line, preserving raw bytes and sensitivity.
    fn append_request_header_value(&mut self, name: &str, value: http::HeaderValue) -> PluginSessionResult<()>;

    /// Remove a request header (e.g., hide credentials)
    fn remove_request_header(&mut self, name: &str) -> PluginSessionResult<()>;

    /// Whether `name` is the configured trusted-proxy client identity header.
    /// Credential injection must never overwrite this request security input.
    fn is_remote_ip_header(&self, _name: &str) -> bool {
        false
    }

    /// Register a prevalidated credential header for final attempt-local
    /// upstream materialization. Adapters without request context fail closed.
    fn defer_credential_header(
        &mut self,
        _name: http::HeaderName,
        _value: Arc<http::HeaderValue>,
        _valid_until: Option<std::time::Instant>,
    ) -> PluginSessionResult<()> {
        Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "deferred credential headers are unavailable",
        )))
    }

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

    /// Set the upstream Host and replace an existing URI authority (for proxy rewrite).
    /// HTTP/2 uses the URI authority for `:authority` even when Host is present.
    fn set_upstream_host(&mut self, host: &str) -> PluginSessionResult<()>;

    /// Set the upstream HTTP method (for proxy rewrite)
    fn set_upstream_method(&mut self, method: &str) -> PluginSessionResult<()>;

    async fn write_response_body(&mut self, body: Option<Bytes>, end_of_stream: bool) -> PluginSessionResult<()>;

    async fn shutdown(&mut self);

    /// Physical downstream transport peer IP, before PROXY/XFF rewriting.
    fn direct_peer_ip(&self) -> &str;

    /// PROXY-effective downstream peer before HTTP RealIp/XFF processing.
    fn downstream_peer_ip(&self) -> &str;

    /// Layer-resolved client IP.
    fn remote_ip(&self) -> &str;

    /// Set the layer-resolved client IP.
    ///
    /// Used by RealIp plugin to update the extracted real client IP.
    fn set_remote_ip(&mut self, addr: &str) -> PluginSessionResult<()>;

    /// Apply one compiled RequestId entry to the canonical request state.
    // Mockall requires a named lifetime for a reference nested in Option.
    #[allow(clippy::needless_lifetimes)]
    fn apply_request_ids<'a>(
        &mut self,
        default: Option<&'a edgion_resources::plugin_configs::DefaultIdConfig>,
        rules: &[crate::request_trace_id::CompiledRequestIdRule],
    ) -> bool;

    /// Get mTLS client certificate info extracted during handshake.
    ///
    /// Returns None when:
    /// - request is plain HTTP (no TLS)
    /// - connection is TLS without client cert
    /// - EdgionTls did not enable `edgion.io/expose-client-cert`
    ///
    /// Gateway frontendValidation retains its existing certificate extraction.
    /// The borrow ends before mutable session operations; clone explicitly when
    /// a consumer needs to retain the complete record across those operations.
    // Mockall requires a named self lifetime for a reference nested in Option.
    #[allow(clippy::needless_lifetimes)]
    fn client_cert_info<'a>(&'a self) -> Option<&'a ClientCertInfo> {
        None
    }

    /// Whether the downstream connection terminated TLS on this gateway.
    ///
    /// Read from the real connection state, not from a request header. `None`
    /// means the state is unavailable; a security-relevant consumer must treat
    /// that as "unknown" and fall back to its own safe default rather than
    /// assuming plaintext.
    fn downstream_is_tls(&self) -> Option<bool> {
        None
    }

    /// Get the discovered protocol type (e.g., "grpc", "grpc-web", "websocket", "http").
    /// Used by DSL `req.protocol`.
    fn discover_protocol(&self) -> String {
        "http".to_string()
    }

    /// Whether the client declared a request body, read from the pre-plugin
    /// snapshot (`ctx.request_info.body_declared`). `None` means the snapshot
    /// never ran. Consumers needing "does this request have a body" must use
    /// this rather than re-deriving it from headers, which plugins can mutate.
    fn declares_body(&self) -> Option<bool> {
        None
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

    /// Nominate an immutable policy only from an actually executed request filter.
    fn nominate_response_cache(
        &mut self,
        _policy: &Arc<crate::plugins::http::response_cache::ResponseCachePolicy>,
    ) -> ResponseCacheIntentAdmission {
        ResponseCacheIntentAdmission::Unsupported
    }

    /// Borrow the selected-WAF integration capability for one operation.
    /// The adapter retains ownership; callers must release this borrow before
    /// using another session operation or awaiting body capture.
    fn waf(&mut self) -> &mut dyn WafSessionCapability;

    /// Borrow AI integration operations without creating exchange state.
    fn ai(&mut self) -> &mut dyn AiSessionCapability;

    /// Capture a soft window plus one proof byte, then feed one bounded reader.
    #[allow(unused_parens)]
    async fn capture_request_body_inspection<'consumer>(
        &mut self,
        _window: u64,
        _consumer: &'consumer mut (dyn RequestBodyChunkConsumer + 'consumer),
    ) -> Result<RequestBodyInspectionCapture, BodyReadError> {
        Err(BodyReadError::Unimplemented)
    }

    /// Capture a prefix separately from the complete-body API. The boolean
    /// proves that the captured bytes represent the complete logical body.
    async fn read_request_body_prefix(&mut self, _limit: usize) -> Result<(bytes::Bytes, bool), BodyReadError> {
        Err(BodyReadError::Unimplemented)
    }

    /// Append an EdgionErrorCode error to the gateway context so it surfaces in the
    /// access-log diagnostics array. Narrow interface — plugins cannot mutate other
    /// ctx fields; only the error list.
    fn record_error(&mut self, status: EdgionErrorCode);

    /// Only a rule-scoped HTTPRoute REST request may claim this conversion.
    fn grpc_transcoding_allowed(&self) -> bool {
        false
    }
    /// Install the sealed RPC plan and its response processor atomically.
    fn install_grpc_transcoding(
        &mut self,
        _plan: crate::plugins::http::grpc_json_transcoder::GrpcTranscodingPlan,
    ) -> bool {
        false
    }

    /// Install a bounded response-stream observer. Alternate adapters that do
    /// not run the proxy response-body pipeline drop it by default.
    fn install_stream_processor(
        &mut self,
        processor: Box<dyn crate::plugins::runtime::traits::StreamingResponseProcessor>,
    ) {
        drop(processor);
    }

    /// Claim the request's first irreversible response terminal for the active
    /// processor instance. Alternate adapters cannot claim by default.
    fn try_claim_response_terminal(&mut self, _cause: crate::response::terminal::ResponseTerminalCause) -> bool {
        false
    }

    /// Record whether the claimed terminal produced a client-visible event.
    fn mark_response_terminal_emission(&mut self, _emission: crate::response::terminal::TerminalEmission) {}

    /// Install one request-scoped attached ExtProc owner. Duplicate identity is
    /// a configuration/runtime error and never overwrites the first owner.
    fn install_attached_ext_proc(
        &mut self,
        _identity: String,
        _owner: std::sync::Arc<crate::plugins::http::ext_proc::AttachedExtProc>,
        _reserve_framing: bool,
    ) -> bool {
        false
    }

    fn attached_ext_proc(
        &self,
        _identity: &str,
    ) -> Option<std::sync::Arc<crate::plugins::http::ext_proc::AttachedExtProc>> {
        None
    }

    fn install_attached_response_processor(
        &mut self,
        processor: Box<dyn crate::plugins::runtime::traits::StreamingResponseProcessor>,
    ) -> bool {
        drop(processor);
        false
    }

    /// Install an internal request-owned response-trailer companion.
    ///
    /// The caller is an ordinary compiled response plugin, so installation
    /// order is already the canonical route/backend/Gateway/global response
    /// order. This does not create an operator-visible plugin stage.
    fn install_upstream_response_trailer(
        &mut self,
        processor: Box<dyn crate::plugins::runtime::traits::UpstreamResponseTrailer>,
    ) -> bool {
        drop(processor);
        false
    }

    /// Number of installed response-trailer companions for this request.
    fn upstream_response_trailer_count(&self) -> usize {
        0
    }

    /// Whether the selected downstream transport and committed/planned
    /// response framing can represent an upstream response trailer block.
    ///
    /// This is transport capability, not evidence that trailers exist. The
    /// safe default is false so adapters without a real downstream session
    /// cannot accidentally claim that a trailer mutation will reach the wire.
    fn downstream_response_trailers_representable(&self) -> bool {
        safe_default_downstream_response_trailers_representable()
    }

    /// Whether an output trailer field name may be introduced on the selected
    /// downstream transport. HTTP/1.1 requires advance declaration in the
    /// committed `Trailer` response header; native trailer transports do not.
    /// The safe default rejects introduction for adapters without transport
    /// state. Existing trailer names remain independently mutable.
    fn downstream_response_trailer_name_allowed(&self, _name: &http::HeaderName) -> bool {
        false
    }

    /// EOS bit from the final upstream response Header task, if this adapter
    /// participates in the canonical proxy response lifecycle.
    fn upstream_response_headers_end_stream(&self) -> Option<bool> {
        None
    }

    /// Claim the request-wide JSON Schema validation slot.
    ///
    /// Returns `true` only for the first validator that actually runs in the
    /// effective global/Gateway/Route/BackendRef request chain. The safe
    /// default fails closed for adapters that do not carry an
    /// [`EdgionHttpContext`].
    fn claim_json_schema_validation(&mut self) -> bool {
        false
    }

    /// Look up the resolved target `PluginRuntime` for an `ExtensionRef` at the
    /// given index (assigned at route build time by
    /// `PluginRuntime::from_httproute_filters_for_rule` /
    /// `from_grpcroute_filters_for_rule`). Returns `None` if the ref was
    /// dangling at resolve time, or if `index` is out of range (defensive).
    ///
    /// Default impl returns `None` — used by adapters that don't carry an
    /// `EdgionHttpContext` (e.g. some test mocks). Production path is
    /// `PingoraSessionAdapter`, which reads the request-pinned route snapshot.
    fn resolved_route_ext_ref(&self, _index: usize) -> Option<std::sync::Arc<crate::plugins::runtime::PluginRuntime>> {
        None
    }

    /// Resolve a backend-scope ExtensionRef once per logical request. Both a
    /// present runtime and a missing result are pinned across retries.
    fn pinned_backend_ext_ref(&mut self, _key: &str) -> Option<std::sync::Arc<crate::plugins::runtime::PluginRuntime>> {
        None
    }

    /// Start a new EdgionPluginsLog and return a token for safe log pushing.
    ///
    /// This creates an empty EdgionPluginsLog entry in the pending list and returns
    /// a token containing its index. Use the token with
    /// `push_to_edgion_plugins_log` before the pending list is drained at the end
    /// of the plugin stage.
    fn start_edgion_plugins_log(&mut self, name: String) -> EdgionPluginsLogToken;

    /// Push a PluginLog to the EdgionPluginsLog identified by the token.
    ///
    /// If the token's index is not present in the pending list, the log is ignored.
    fn push_to_edgion_plugins_log(&mut self, token: &EdgionPluginsLogToken, log: PluginLog);

    // ========== KeyGet/KeySet unified accessor methods ==========

    /// Read one optional string from the request using a KeyGet accessor.
    ///
    /// Synchronous and I/O-free. Used by RateLimitLocal, Conditions, and other plugins.
    ///
    /// # Returns
    /// * `Some(String)` - The extracted value
    /// * `None` - If the value doesn't exist or is empty (for RemoteIp)
    fn key_get(&self, key: &KeyGet) -> Option<String>;

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
    /// Publish through the canonical slot with fatal conflict handling.
    /// Unsupported adapters fail closed rather than weaken the ownership contract.
    fn set_strict_upstream_jump(&mut self, _target: UpstreamJumpTarget) -> SetJumpOutcome {
        SetJumpOutcome::Rejected
    }

    /// Install request mirror state used by the request-body-filter tee.
    /// Returns `false` after the request-scoped body policy has frozen.
    fn set_mirror_state(&mut self, state: MirrorState) -> bool;

    // ========== Shared request-body capture ==========
    //
    // Eligible request-stage plugins share one bounded, versioned capture of the
    // proxied request body. The first read may drain the downstream stream into
    // that request-scoped storage; later reads observe its current logical version.
    // Async + `&mut self` permits capture and mutation. Returned `Bytes` values are
    // immutable handles, so no borrow of the session is retained.
    //
    // The main upstream response is permanently streaming and has no full-body
    // cache. Response-body processing operates only on the current bounded chunk.
    // Defaults remain `Err(BodyReadError::Unimplemented)` for adapters that do not
    // provide the selected operation.

    /// Read the full request body, buffering it on first call so it is available
    /// during the request phase, before the request is proxied upstream.
    /// `max_bytes` caps the buffer on the first (buffering) call (`None` = gateway
    /// default cap); later calls return the already-buffered bytes.
    async fn read_request_body(&mut self, _max_bytes: Option<u64>) -> BodyReadResult {
        Err(BodyReadError::Unimplemented)
    }

    /// Ensure the request body is completely captured without requiring a
    /// contiguous materialization, and return its final length.
    ///
    /// The default preserves compatibility for test sessions and alternate
    /// adapters by using the full-body accessor. The Pingora adapter overrides
    /// this method so a disk-backed body remains on disk.
    async fn ensure_request_body(&mut self, max_bytes: Option<u64>) -> Result<u64, BodyReadError> {
        let body = self.read_request_body(max_bytes).await?;
        u64::try_from(body.len()).map_err(|_| BodyReadError::Incomplete)
    }

    /// Capture before upstream dispatch and deliver admitted transport chunks,
    /// or bounded chunks from the current immutable cached snapshot. Success
    /// proves real EOF and exact byte parity. Defaults do not materialize input.
    #[cfg_attr(test, allow(unused_parens))]
    async fn capture_request_body_with_consumer<'consumer>(
        &mut self,
        _max_bytes: Option<u64>,
        _consumer: &'consumer mut (dyn RequestBodyChunkConsumer + 'consumer),
    ) -> Result<u64, BodyReadError> {
        Err(BodyReadError::Unimplemented)
    }

    /// Defensive compatibility seam for attempting to read a complete upstream
    /// response body.
    ///
    /// This method must never aggregate the main upstream response. The production
    /// adapter returns an empty body only when the live and pre-plugin transport
    /// facts unanimously prove that the response is bodyless; every other case
    /// fails loudly. New response processing must use the streaming chunk path.
    async fn read_response_body(&mut self, _max_bytes: Option<u64>) -> BodyReadResult {
        Err(BodyReadError::Unimplemented)
    }

    /// Replace a byte range in the buffered request body. Unchanged regions
    /// retain their `Bytes` backing allocation.
    async fn replace_request_body_slice(
        &mut self,
        _range: std::ops::Range<usize>,
        _replacement: Bytes,
    ) -> BodyReadResult {
        Err(BodyReadError::Unimplemented)
    }

    /// Apply several disjoint byte-range replacements as one atomic mutation.
    ///
    /// `expected_generation` binds the edits to the snapshot their offsets were
    /// computed against; a mutation published in between fails with
    /// [`BodyReadError::GenerationConflict`] rather than splicing stale offsets
    /// into a different body. Returns the new generation.
    async fn replace_request_body_spans(
        &mut self,
        _expected_generation: u64,
        _edits: &[(std::ops::Range<u64>, Bytes)],
    ) -> Result<u64, BodyReadError> {
        Err(BodyReadError::Unimplemented)
    }

    /// Replace the complete buffered request body used for every upstream
    /// attempt.
    async fn overwrite_request_body(&mut self, _body: Bytes) -> Result<(), BodyReadError> {
        Err(BodyReadError::Unimplemented)
    }

    /// Compute SHA-256 without requiring a contiguous materialization of a
    /// disk-backed buffered request body.
    async fn request_body_sha256(&mut self) -> Result<[u8; 32], BodyReadError> {
        use sha2::Digest;
        let body = self.read_request_body(None).await?;
        Ok(sha2::Sha256::digest(body).into())
    }

    /// Resolve the opaque request Body against an existing complete cache.
    ///
    /// This is metadata-only: it must never initiate capture or materialize
    /// the cached bytes. A transport-proven bodyless request is recorded as an
    /// empty stable source so subsequent operations share the same semantics.
    fn request_body_available_len(&mut self, _known_bodyless: bool) -> Result<u64, BodyReadError> {
        Err(BodyReadError::Unimplemented)
    }

    /// Capture an owned reader over the current logical request-body snapshot.
    ///
    /// This is metadata-only and never starts transport capture or materializes
    /// disk-backed bytes. `known_bodyless` may be true only when the transport
    /// snapshot proved that the request has no body.
    async fn request_body_snapshot_reader(
        &mut self,
        _known_bodyless: bool,
    ) -> Result<Box<dyn RequestBodySnapshotReader>, BodyReadError> {
        Err(BodyReadError::Unimplemented)
    }

    async fn request_body_clear(&mut self) -> Result<(), BodyOperationError> {
        Err(BodyReadError::Unimplemented.into())
    }

    async fn request_body_append(&mut self, _bytes: Bytes) -> Result<(), BodyOperationError> {
        Err(BodyReadError::Unimplemented.into())
    }

    async fn request_body_prepend(&mut self, _bytes: Bytes) -> Result<(), BodyOperationError> {
        Err(BodyReadError::Unimplemented.into())
    }

    async fn request_body_remove(&mut self, _range: std::ops::Range<u64>) -> Result<(), BodyOperationError> {
        Err(BodyReadError::Unimplemented.into())
    }

    async fn request_body_replace_literal(
        &mut self,
        _from: Bytes,
        _to: Bytes,
        _max_scan_bytes: u64,
    ) -> Result<BodyScanResult<()>, BodyOperationError> {
        Err(BodyReadError::Unimplemented.into())
    }

    // ========== Entry-level dye suite (traffic tagging) ==========

    /// Latched transport fact: whether the downstream request headers ended
    /// the stream (`Some(true)` = the transport guarantees no body will
    /// arrive; `Some(false)` = a body may follow). `None` when the session
    /// type cannot supply the fact (custom-connector, subrequest, mocks) —
    /// per-chunk streamed body handlers are unavailable there and callers
    /// must fail per their configured failure policy.
    ///
    /// Prefer this over `declares_body()` for body-inspection gating: the
    /// latter infers from `Content-Length`, which an H2 client can contradict
    /// (declare `CL: 0`, still send DATA frames).
    fn request_headers_end_stream(&mut self) -> Option<bool> {
        None
    }

    /// Canonical request classification frozen by routing/media-type handling.
    /// Plugins use this fact instead of re-parsing mutable headers.
    fn is_grpc_request(&self) -> bool {
        false
    }

    /// Queue an operation for the fresh header of a plugin-authored local
    /// reply. Unlike the ordinary response-header methods, this must not
    /// mutate a currently bound upstream response that will be discarded.
    fn queue_local_reply_header_op(&mut self, op: PendingHeaderOp) -> PluginSessionResult<()>;

    /// The dye plan of the currently-running plugin entry, for capture into a
    /// streamed body handler (body-phase outcomes are recorded against the
    /// installing entry's plan).
    fn current_dye_plan(&self) -> Option<Arc<CompiledDyePlan>> {
        None
    }

    /// Replace the host-owned current request-plugin identity and return the
    /// previous value. Runtime dispatchers pair this with
    /// `restore_plugin_entry_identity`, including nested ExtensionRef calls.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn replace_plugin_entry_identity(
        &mut self,
        _identity: Option<crate::ctx::CurrentPluginEntryIdentity>,
    ) -> Option<crate::ctx::CurrentPluginEntryIdentity> {
        None
    }

    /// Restore the exact identity returned by
    /// `replace_plugin_entry_identity`.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn restore_plugin_entry_identity(&mut self, _previous: Option<crate::ctx::CurrentPluginEntryIdentity>) {}

    /// Install a bounded, synchronous and read-only request-body observer.
    /// A false return only disables observation; callers must continue the
    /// request under the seam's fixed failure-open contract.
    #[allow(private_interfaces)]
    fn install_request_body_observer(
        &mut self,
        factory: crate::request_body::observer::RequestBodyObserverFactory,
        _limits: crate::request_body::observer::ObserverLimits,
    ) -> bool {
        let _ = edgion_common::startup::catch_redacted_panic(|| drop(factory));
        false
    }

    /// Install a per-chunk streamed request-body handler for this request.
    /// Returns `false` when the capability is unavailable (the handler is
    /// dropped, nothing was installed) — the caller must then apply its
    /// failure policy. Installing any handler makes the request
    /// non-replayable and switches upstream framing to the streamed
    /// disposition for the rest of the request.
    fn install_request_body_stream_handler(
        &mut self,
        handler: Box<dyn crate::plugins::runtime::traits::request_body_stream::RequestBodyStreamHandler>,
    ) -> bool {
        drop(handler);
        false
    }

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

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/traits/session_test/response_trailer_capability_tests.rs"]
mod response_trailer_capability_tests;
