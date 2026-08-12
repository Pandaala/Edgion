//! PluginSession trait - shared across all filter stages

use crate::ai::{AiAttemptPlan, AiRequestContext};
use crate::ctx::ClientCertInfo;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::log::{EdgionPluginsLog, EdgionPluginsLogToken, PluginLog};
use crate::request_body::{BodyOperationError, BodyScanResult};
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
/// The body gate applies plugin failure policy only to recoverable read
/// failures. Admission refusals and the gateway-wide physical buffer limit are
/// always fail-close because the original body is unavailable for forwarding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyReadError {
    /// Full-body buffering is not implemented on this binary (current default).
    Unimplemented,
    /// The gateway **declined to buffer** this body (protocol or stage
    /// refusal — see the body-gate admission model). This is NOT evidence that
    /// the request had no body, and it is never softened by
    /// `BodyRequirement::on_read_failure`.
    NotBuffered,
    /// The body exceeded the gateway-wide physical buffer limit. During
    /// capture, the partial body is discarded and the error always maps to
    /// HTTP 413. During an atomic mutation, the previous complete version
    /// remains available and the mutating plugin's failure policy applies.
    BufferLimitExceeded { limit: u64 },
    /// The complete body was buffered, but exceeds a plugin's own logical
    /// inspection limit. Because the forwarding copy still exists, the
    /// plugin's configured fail-open / fail-close policy can apply.
    TooLarge { limit: u64 },
    /// The body stream ended prematurely / was aborted.
    Incomplete,
    /// The capture storage backend failed (spill file create/write, or the
    /// capture was poisoned mid-chunk): an infrastructure fault, not a client
    /// abort. Kept distinct from `Incomplete` so operators can alert on it,
    /// and always fail-close — the consumed bytes were not stored, so no
    /// forwarding copy of the body exists.
    StorageFailure,
    /// The buffering read exceeded its time bound. Distinct from `Incomplete`
    /// (an aborted stream): a deadline expiry is a client error and maps to
    /// HTTP 408, not 500. Only produced once the P0 buffering runtime exists.
    Timeout,
}

impl std::fmt::Display for BodyReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyReadError::Unimplemented => write!(f, "body buffering not implemented"),
            BodyReadError::NotBuffered => write!(f, "body not buffered"),
            BodyReadError::BufferLimitExceeded { limit } => {
                write!(f, "body exceeds gateway buffer limit of {limit} bytes")
            }
            BodyReadError::TooLarge { limit } => write!(f, "body exceeds buffer cap of {limit} bytes"),
            BodyReadError::Incomplete => write!(f, "body stream incomplete"),
            BodyReadError::StorageFailure => write!(f, "body capture storage failed"),
            BodyReadError::Timeout => write!(f, "body read timed out"),
        }
    }
}

impl std::error::Error for BodyReadError {}

/// Result of reading a buffered body: the bytes (a cheap `Arc`-backed handle, so
/// cloning is a refcount bump, not a copy), or the reason it is unavailable.
pub type BodyReadResult = Result<Bytes, BodyReadError>;

/// An owned reader over one immutable logical request-body snapshot.
///
/// Each returned chunk is bounded by the body backend. The reader owns every
/// backing reference needed to outlive later body mutations, so consumers may
/// safely stream it across await points without borrowing the plugin session.
#[async_trait]
pub trait RequestBodySnapshotReader: Send {
    /// Exact byte length of this snapshot.
    fn len(&self) -> u64;

    /// Read the next raw byte chunk, or `None` after the complete snapshot.
    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError>;
}

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

    /// Append an EdgionStatus error to the gateway context so it surfaces in the
    /// access log `errors` array. Narrow interface — plugins cannot mutate other
    /// ctx fields; only the error list.
    fn add_error(&mut self, status: EdgionStatus);

    /// Claim the request-wide AI context. The first writer wins; alternate
    /// adapters without a mutable Edgion context fail closed by default.
    fn set_ai_request_context(&mut self, context: AiRequestContext) -> bool {
        drop(context);
        false
    }

    /// Claim the immutable request-wide AI attempt plan. The first writer wins.
    fn set_ai_attempt_plan(&mut self, plan: AiAttemptPlan) -> bool {
        drop(plan);
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
    // Defaults remain `Err(BodyReadError::Unimplemented)` for mocks.
    // `PingoraSessionAdapter` overrides the request methods; response
    // buffering still requires a separate Pingora seam.

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

    /// Read the full upstream response body, buffering it on first call.
    /// `max_bytes` caps the buffer on the first (buffering) call.
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

    /// The dye plan of the currently-running plugin entry, for capture into a
    /// streamed body handler (body-phase outcomes are recorded against the
    /// installing entry's plan).
    fn current_dye_plan(&self) -> Option<Arc<CompiledDyePlan>> {
        None
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
