use crate::ai::terminal::{
    AiClientStreamDialect, FinalResponseEligibility, ResponseProcessorSlot, ResponseTerminalState,
};
use crate::ai::{AiAttemptPlan, AiRequestContext, EdgionBackendSnapshot, GenAiSummary};
use crate::backends::{SelectedHttpBackend, ServiceBackendKind};
use crate::lb::edgion_backend::EdgionBackend;
use crate::observe::logs::LogBuffer;
use crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseProcessor;
use crate::plugins::runtime::PluginRuntime;
use crate::plugins::{EdgionPluginsLog, StageLogs};
use crate::request_body::RequestBodyState;
use crate::routes::grpc::{CompiledGrpcBackend, GrpcRouteRuleUnit};
use crate::routes::HttpRouteRuleUnit;
use crate::runtime::GatewayInfo;
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::http_route::SessionPersistence;
use edgion_resources::resources::lb_policy::ParsedLBPolicy;
use edgion_resources::uri_utils;
use edgion_resources::{EdgionStatus, HTTPBackendRef, HTTPRouteMatch};
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_http::ResponseHeader;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const PLUGIN_ENTRY_NAME_CAPACITY: usize = crate::plugins::runtime::log::NAME_CAPACITY;
const PLUGIN_ENTRY_ALIAS_CAPACITY: usize = 32;

/// Allocation-free identity of the request-stage plugin entry currently being
/// invoked. The runtime owns this slot; body observers may only snapshot it at
/// registration time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CurrentPluginEntryIdentity {
    name: [u8; PLUGIN_ENTRY_NAME_CAPACITY],
    name_len: u8,
    alias: [u8; PLUGIN_ENTRY_ALIAS_CAPACITY],
    alias_len: u8,
    has_alias: bool,
}

impl CurrentPluginEntryIdentity {
    pub(crate) fn new(name: &str, alias: Option<&str>) -> Option<Self> {
        if name.is_empty() || name.len() > PLUGIN_ENTRY_NAME_CAPACITY {
            return None;
        }
        let alias = alias.unwrap_or_default();
        if alias.len() > PLUGIN_ENTRY_ALIAS_CAPACITY {
            return None;
        }
        let mut identity = Self {
            name: [0; PLUGIN_ENTRY_NAME_CAPACITY],
            name_len: name.len() as u8,
            alias: [0; PLUGIN_ENTRY_ALIAS_CAPACITY],
            alias_len: alias.len() as u8,
            has_alias: !alias.is_empty(),
        };
        identity.name[..name.len()].copy_from_slice(name.as_bytes());
        identity.alias[..alias.len()].copy_from_slice(alias.as_bytes());
        Some(identity)
    }

    pub(crate) fn name(&self) -> &str {
        // Both arrays are populated exclusively from validated UTF-8 `str`
        // slices and read back at their original lengths.
        std::str::from_utf8(&self.name[..usize::from(self.name_len)]).expect("plugin identity must remain UTF-8")
    }

    pub(crate) fn alias(&self) -> Option<&str> {
        self.has_alias.then(|| {
            std::str::from_utf8(&self.alias[..usize::from(self.alias_len)])
                .expect("plugin alias identity must remain UTF-8")
        })
    }
}

#[cfg(test)]
mod plugin_entry_identity_tests {
    use super::CurrentPluginEntryIdentity;

    #[test]
    fn preserves_bounded_name_and_alias() {
        let identity = CurrentPluginEntryIdentity::new("A2aGateway", Some("edge.a2a-1")).unwrap();
        assert_eq!(identity.name(), "A2aGateway");
        assert_eq!(identity.alias(), Some("edge.a2a-1"));
        assert_eq!(
            CurrentPluginEntryIdentity::new("A2aGateway", None).unwrap().alias(),
            None
        );
    }

    #[test]
    fn rejects_empty_or_overlong_identity_parts() {
        assert!(CurrentPluginEntryIdentity::new("", None).is_none());
        assert!(CurrentPluginEntryIdentity::new(&"x".repeat(37), None).is_none());
        assert!(CurrentPluginEntryIdentity::new("A2aGateway", Some(&"x".repeat(33))).is_none());
    }
}

/// Operator-defined log labels, resolved once per resource and shared by
/// reference across `Rsc` clones (so the per-request path never re-allocates).
///
/// Serializes as a flat JSON object, e.g. `{"env":"prod","tenant":"internal"}`.
/// Backed by a `BTreeMap` so the output ordering is deterministic across reloads
/// and route-table rebuilds. The resolution logic and guardrails live in
/// `edgion_resources::resources::common::resolve_log_labels` (the single source
/// of truth, applied at resource-parse time); this newtype only wraps the
/// resolved map for the data-plane log path.
#[derive(Debug, Clone)]
pub struct LogLabels(Arc<BTreeMap<String, String>>);

impl LogLabels {
    /// Wrap a map resolved at parse time (e.g. a `pre_parse` cache field) into a
    /// `LogLabels`. Cheap: shares the `Arc`, performs no scanning.
    pub fn from_resolved(map: Arc<BTreeMap<String, String>>) -> Self {
        LogLabels(map)
    }
}

impl Serialize for LogLabels {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Delegate to the inner map; serde's `rc` feature is not enabled, so the
        // `Arc` wrapper must be unwrapped manually here.
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LogLabels {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<String, String>::deserialize(deserializer)?;
        Ok(LogLabels(Arc::new(map)))
    }
}

/// Resolve operator-defined log labels from a resource's `metadata`.
///
/// Thin gateway-side wrapper over
/// [`edgion_resources::resources::common::resolve_log_labels`] (the single
/// implementation of the prefix scan + guardrails). Called from TLSRoute
/// build only (`routes::tls::route_unit`) — the `edgion.io/log-label.<suffix>`
/// carrier is narrowed to TLS resources, whose handshake layer has no plugin
/// chain and no `logging()` stage to hook an `AccessLogExtern` declaration
/// onto. HTTPRoute/GRPCRoute build no longer calls this: their log projection
/// goes through `AccessLogExtern`'s `from: routeLabel` / `from:
/// routeAnnotation` instead. EdgionTls caches its resolved map in
/// `pre_parse`, so it wraps the cached `Arc` via [`LogLabels::from_resolved`]
/// rather than calling this function.
pub fn extract_log_labels(meta: &kube::api::ObjectMeta) -> Option<LogLabels> {
    edgion_resources::resources::common::resolve_log_labels(meta).map(LogLabels)
}

/// A route's raw `metadata.labels` or `metadata.annotations`, resolved once at
/// route build and `Arc`-shared onto every match unit.
///
/// Distinct from [`LogLabels`], which is the *projected* log-label set. This is
/// the raw metadata, needed to answer `AccessLogExtern`'s `from: routeLabel` /
/// `from: routeAnnotation` lookups without touching `ObjectMeta` per request.
pub type RouteMeta = Option<Arc<BTreeMap<String, String>>>;

/// Clone a route's labels and annotations into `Arc`s at build time.
/// Returns `(labels, annotations)`; `None` when the map is absent or empty, so
/// the common case stores nothing.
pub fn extract_route_meta(meta: &kube::api::ObjectMeta) -> (RouteMeta, RouteMeta) {
    let take = |m: Option<&BTreeMap<String, String>>| -> RouteMeta {
        match m {
            Some(map) if !map.is_empty() => Some(Arc::new(map.clone())),
            _ => None,
        }
    };
    (take(meta.labels.as_ref()), take(meta.annotations.as_ref()))
}

/// Unified resource identifier for access log.
/// Provides consistent `kind`/`ns`/`name` across all resource types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rsc {
    pub kind: String,
    pub ns: String,
    pub name: String,

    /// Operator-defined log labels for this resource (see [`extract_log_labels`]).
    /// Omitted from the serialized log entry when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<LogLabels>,
}

impl Rsc {
    /// Build a resource identifier with no labels (the common case for backend /
    /// internally-derived resources that carry no operator log labels).
    pub fn new(kind: impl Into<String>, ns: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            ns: ns.into(),
            name: name.into(),
            labels: None,
        }
    }

    /// Attach resolved operator log labels, consuming and returning `self`.
    #[cfg(test)]
    pub fn with_labels(mut self, labels: Option<LogLabels>) -> Self {
        self.labels = labels;
        self
    }
}

#[derive(Clone, Serialize)]
pub struct MatchInfo {
    /// Resource identifier (kind="HTTPRoute")
    pub rsc: Rsc,

    /// Rule id in HTTPROUTE
    pub rule_id: usize,
    /// Match id at rule id,
    pub match_id: usize,

    /// sync_version from gRPC sync (0 = not set)
    #[serde(skip_serializing_if = "is_zero")]
    pub sv: u64,

    /// match item
    #[serde(skip)]
    pub m: HTTPRouteMatch,

    /// Raw route labels for `AccessLogExtern` `from: routeLabel`. In-process
    /// only — never part of the serialized entry; only declared keys reach `ext`.
    #[serde(skip)]
    pub labels_meta: RouteMeta,

    /// Raw route annotations for `AccessLogExtern` `from: routeAnnotation`.
    #[serde(skip)]
    pub annotations_meta: RouteMeta,
}

pub fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl MatchInfo {
    /// HTTPRoute no longer carries operator log labels (see
    /// [`extract_log_labels`]'s doc) — `rsc.labels` is always `None` here.
    /// `AccessLogExtern`'s `from: routeLabel` / `from: routeAnnotation` reads
    /// `labels_meta` / `annotations_meta` instead (attached via
    /// [`MatchInfo::with_route_meta`]).
    pub fn new(ns: String, name: String, rule_id: usize, match_id: usize, m: HTTPRouteMatch, sv: u64) -> Self {
        Self {
            rsc: Rsc {
                kind: "HTTPRoute".to_string(),
                ns,
                name,
                labels: None,
            },
            m,
            rule_id,
            match_id,
            sv,
            labels_meta: None,
            annotations_meta: None,
        }
    }

    /// Attach the route's raw metadata maps, resolved once at route build.
    pub fn with_route_meta(mut self, labels: RouteMeta, annotations: RouteMeta) -> Self {
        self.labels_meta = labels;
        self.annotations_meta = annotations;
        self
    }
}

impl fmt::Display for MatchInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{} (rule:{}, match:{})",
            self.rsc.ns, self.rsc.name, self.rule_id, self.match_id
        )
    }
}

/// Client certificate information extracted from TLS connection
#[derive(Debug, Clone, Serialize)]
pub struct ClientCertInfo {
    /// Certificate subject DN (Distinguished Name)
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub subject: String,
    /// Subject Alternative Names (SANs) from certificate
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "edgion_resources::sensitive::masked_vec::serialize"
    )]
    pub sans: Vec<String>,
    /// Common Name (CN) extracted from subject
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    pub cn: Option<String>,
    /// Certificate fingerprint (SHA256 hex) — not PII
    pub fingerprint: String,
}

/// TLS connection identifier stored in SslDigestExtension
#[derive(Debug, Clone)]
pub struct TlsConnId(pub u64);

#[derive(Debug, Clone, Serialize)]
pub struct MatchedInfo {
    pub rsc: Rsc,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// sync_version from gRPC sync (0 = not set)
    #[serde(skip_serializing_if = "is_zero")]
    pub sv: u64,
}

/// TLS handshake metadata stored in SslDigestExtension.
///
/// Created once per TLS connection in handshake_complete_callback and shared by
/// requests on that connection through Pingora's digest extension.
///
/// For the HTTP path, consumed in pg_request_filter.rs.
/// For the TLS proxy path, consumed in EdgionTls::process_new.
#[derive(Debug, Clone, Serialize)]
pub struct TlsConnMeta {
    pub ts: i64,
    #[serde(skip_serializing)]
    pub start_at: Instant,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handshake_complete_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Connection id for tls.log <-> access.log correlation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_id: Option<String>,
    /// SNI from handshake (if present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<LogBuffer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<MatchedInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err_log: Option<String>,
    /// mTLS client cert info (only when annotation gate is enabled).
    #[serde(skip_serializing)]
    pub client_cert_info: Option<ClientCertInfo>,
    /// Whether mTLS is configured for this connection.
    #[serde(skip_serializing_if = "is_false")]
    pub is_mtls: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// A hostname that has been normalized (port-stripped, FQDN-dot-removed, lowercased).
///
/// Construct via `NormalizedHostname::new()` which applies the normalization once.
/// All downstream consumers can rely on the invariant — no further lowercasing needed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NormalizedHostname(String);

impl NormalizedHostname {
    /// Normalize and wrap a raw hostname.
    /// Applies: trim, strip port (IPv4/IPv6), remove trailing FQDN dot, lowercase.
    pub fn new(raw: &str) -> Self {
        let h = raw.trim();

        let host = if h.starts_with('[') {
            match h.find(']') {
                Some(end) => &h[1..end],
                None => h,
            }
        } else {
            match h.rfind(':') {
                // A registered name or IPv4 authority has at most one colon.
                // Bare IPv6 has multiple colons and no syntactically distinct
                // port suffix, so it must remain intact.
                Some(pos) if h.matches(':').count() == 1 && h[pos + 1..].bytes().all(|b| b.is_ascii_digit()) => {
                    &h[..pos]
                }
                _ => h,
            }
        };

        let host = host.strip_suffix('.').unwrap_or(host);
        Self(host.to_ascii_lowercase())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::ops::Deref for NormalizedHostname {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for NormalizedHostname {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NormalizedHostname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod normalized_hostname_tests {
    use super::NormalizedHostname;

    #[test]
    fn normalization_strips_only_unambiguous_ports() {
        assert_eq!(NormalizedHostname::new("Example.COM:8443").as_str(), "example.com");
        assert_eq!(NormalizedHostname::new("127.0.0.1:8080").as_str(), "127.0.0.1");
        assert_eq!(NormalizedHostname::new("[2001:db8::1]:443").as_str(), "2001:db8::1");
        assert_eq!(NormalizedHostname::new("2001:db8::1").as_str(), "2001:db8::1");
    }
}

/// Request information extracted from the incoming request
#[derive(Debug, Clone, Default, Serialize)]
pub struct RequestInfo {
    /// TCP client IP address (direct connection, immutable)
    #[serde(
        rename = "client-addr",
        serialize_with = "edgion_resources::sensitive::masked::serialize"
    )]
    pub client_addr: String,
    /// TCP client port (direct connection, 0 if unknown)
    #[serde(rename = "client-port")]
    pub client_port: u16,
    /// Real client address (extracted from headers if behind trusted proxy)
    #[serde(
        rename = "remote-addr",
        serialize_with = "edgion_resources::sensitive::masked::serialize"
    )]
    pub remote_addr: String,
    /// Trace ID from x-trace-id header
    #[serde(rename = "x-trace-id")]
    pub x_trace_id: Option<String>,
    /// Normalized hostname from the Host header (lowercased, port-stripped).
    /// Construct via `NormalizedHostname::new()` — never apply `to_lowercase()` again.
    #[serde(rename = "host")]
    pub hostname: NormalizedHostname,
    /// Snapshot of req_header().uri.path() at request entry. Frozen for the request lifetime.
    /// Source for `request_path()` / `request_uri()` views.
    /// Not serialized into access_log directly; surfaced via the optional
    /// `request_uri` field on `AccessLogEntry` when `include_raw_uri` is on.
    #[serde(skip)]
    pub entry_raw_path: String,

    /// Snapshot of req_header().uri.query() at request entry. Frozen for the request lifetime.
    /// Source for `request_uri()` view.
    /// Not serialized into access_log directly; surfaced via the optional
    /// `request_uri` field on `AccessLogEntry` when `include_raw_uri` is on.
    #[serde(skip)]
    pub entry_raw_query: Option<String>,

    /// Cache of normalized current path. Populated at entry from entry_raw_path,
    /// refreshed by session_adapter::set_upstream_uri after rewrite.
    /// Source for `path()` / `uri()` views.
    /// Serializes as JSON field "path" for backward-compatible access_log output.
    #[serde(rename = "path", serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub normalized_current_path: String,
    /// Response status code (e.g., 200, 400, 404, 500)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Original X-Forwarded-For header value (before appending client IP)
    #[serde(
        rename = "x-forwarded-for",
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    pub x_forwarded_for: Option<String>,
    /// SNI (Server Name Indication) from TLS handshake
    /// Only present for HTTPS connections; HTTP connections will have None
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    pub sni: Option<String>,
    /// Auto-discovered protocol (e.g., "grpc", "grpc-web", "websocket")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discover_protocol: Option<String>,
    /// Whether the request is a gRPC or gRPC-Web request
    #[serde(skip)]
    pub is_grpc_request: bool,
    /// Whether the client declared a request body, snapshotted in
    /// `build_request_metadata` **before any plugin runs**.
    ///
    /// `None` means the snapshot never ran, which is a bug — the body-gate
    /// predicate treats it as a refusal (fail-close) rather than guessing.
    /// This is deliberately `Option<bool>` and not `bool`: `RequestInfo`
    /// derives `Default`, and a defaulted `false` would read as "no body,
    /// run the plugin without one".
    #[serde(skip)]
    pub body_declared: Option<bool>,
    /// Whether the request is currently within the request-processing stage,
    /// where the client body (if any) has not yet been proxied upstream.
    ///
    /// Set `true` in `snapshot_body_declared` at the same entry point as
    /// `body_declared`. Stays `true` through `request_filter`'s route/gateway/
    /// global request-stage plugins *and* the later backendRef-scoped request
    /// plugin chains (`pg_upstream_peer.rs`'s HTTP path,
    /// `routes/grpc/integration.rs`'s gRPC path) that run after `request_filter`
    /// returns but before an upstream connect is attempted -- a read at either
    /// of those call sites is serviceable because Pingora has not started
    /// pumping the downstream body upstream yet. It is flipped back to
    /// `false` at the tail of `upstream_request_filter`
    /// (`pg_upstream_request_filter.rs`), which runs only after a successful
    /// upstream connect and right before body proxying begins -- the true
    /// moment after which `read_request_body` must be refused, never
    /// answered with a misleading zero-length `Ok`.
    ///
    /// Defaults to `false` (the safe/refuse direction): a `RequestInfo` built
    /// without going through the real request-entry snapshot (a bare
    /// `RequestInfo::default()` in a test, or a future code path that never
    /// reaches `snapshot_body_declared`) reads as "past the request stage"
    /// rather than silently defaulting to "still the request stage" and
    /// permitting a body read it should not.
    #[serde(skip)]
    pub in_request_body_stage: bool,
    /// Whether the DOWNSTREAM request method was `HEAD`, snapshotted in
    /// `snapshot_body_declared` at the same entry point as `body_declared`,
    /// **before any plugin runs**.
    ///
    /// `read_response_body` needs this fact (RFC 9110 9.3.2: a HEAD response
    /// carries `Content-Length` but no body) to decide the bodyless
    /// short-circuit, and ANDs it with the live request method — the silent
    /// `PassThrough` disposition requires both to agree. Neither is sufficient
    /// alone, because `set_upstream_method` (reachable from `proxy_rewrite` and
    /// from DSL's `req.set_method`, which is not stage-gated) rewrites the live
    /// header in place and each direction breaks one of them:
    ///
    /// - Live header alone: a rewrite TO `HEAD` makes `read_response_body`
    ///   report "no body" for a response whose body already arrived.
    /// - This snapshot alone: a downstream `HEAD` rewritten to a body-carrying
    ///   method makes pingora write the real body downstream (its
    ///   `init_body_writer` keys on the same live header this rewrite mutated),
    ///   while the snapshot still says HEAD.
    ///
    /// Both directions land a body-inspecting plugin on `Ok(Bytes::new())`,
    /// whose contract is "the response truthfully declared no body" — silently
    /// downgrading "inspect the body" into "inspect nothing and believe it was
    /// empty". ANDing lets the mutable fact only NARROW `PassThrough`, never
    /// widen it; `request_admission` applies the same discipline in the mirror
    /// direction (a client-controlled fact may only widen a refusal).
    ///
    /// Defaults to `false` (the safe direction): a `RequestInfo` that never
    /// reached the snapshot reads as "not HEAD", which routes to `Buffer` — an
    /// attempted read that can fail loudly — rather than to the silent
    /// `PassThrough`. A genuine HEAD response has no body to buffer either way.
    #[serde(skip)]
    pub method_is_head: bool,
    /// gRPC service (parsed from path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_service: Option<String>,
    /// gRPC method (parsed from path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_method: Option<String>,
    /// Client certificate information (for mTLS connections)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert_info: Option<ClientCertInfo>,
    /// TLS connection id for correlating tls.log and access.log
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_id: Option<String>,
    /// Listener name that matched the SNI during TLS handshake (Gateway TLS only).
    /// None for HTTP connections, EdgionTls connections, or when TLS metadata is unavailable.
    /// Used for 421 Misdirected Request detection.
    #[serde(skip)]
    pub tls_matched_listener: Option<String>,
    /// Gateway key (ns/name) from TLS handshake matching (Gateway TLS only).
    /// Used together with tls_matched_listener for 421 detection.
    #[serde(skip)]
    pub tls_matched_gateway_key: Option<String>,
    /// Listener port that received this request (set at context creation, always available)
    #[serde(rename = "listener-port")]
    pub listener_port: u16,
    /// gRPC status code sent in the trailers-only response (e.g. 12=UNIMPLEMENTED, 14=UNAVAILABLE).
    /// None for non-gRPC requests or when no gRPC error was generated locally.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_status: Option<u16>,
    /// Whether an HTTP/gRPC response has already been written to downstream.
    /// Used as a guard to prevent double-write across error response paths.
    #[serde(skip)]
    pub response_committed: bool,

    /// Parsed `grpc-timeout` deadline (`start_time + value`). Populated by
    /// `pg_request_filter::build_request_metadata` when the request carries
    /// a syntactically valid header. `None` when no header, malformed
    /// header, or non-gRPC request — callers fall back to route-level
    /// `request_timeout` semantics.
    #[serde(skip)]
    pub grpc_client_deadline: Option<Instant>,
}

/// Backend TLS connection information
#[derive(Debug, Clone, Serialize)]
pub struct BackendTlsInfo {
    /// SNI (Server Name Indication) sent to backend
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    /// TLS handshake success
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handshake_ok: Option<bool>,
    /// TLS protocol version (e.g., "TLSv1.3")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Cipher suite used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cipher: Option<String>,
}

/// Upstream connection information for a single connection attempt
#[derive(Debug, Serialize)]
pub struct UpstreamInfo {
    /// Upstream IP address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Upstream port
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// HTTP status code for this upstream
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Connect time in milliseconds (from start_time to connection established)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ct: Option<u64>,
    /// Header time in milliseconds (from start_time to receiving response header)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ht: Option<u64>,
    /// Body time in milliseconds (from start_time to receiving first body chunk)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bt: Option<u64>,
    /// Elapsed time in milliseconds (total time for this upstream attempt)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub et: Option<u64>,
    /// Write pending time in milliseconds (HTTP/1.x only, upstream backpressure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wpt: Option<u64>,
    /// Upstream response body size in bytes (payload only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body_size: Option<usize>,
    /// Start time - when this upstream attempt started (for calculating ct, ht, and bt)
    #[serde(skip)]
    pub start_time: Instant,
    /// Error messages collected during upstream attempts
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub err: Vec<String>,
    /// Canonical backend address used by the LB selection algorithm, before
    /// any target-port rewrite. Runtime-state accounting must use this key
    /// so that selection reads and request-completion writes are consistent.
    #[serde(skip)]
    pub lb_backend_addr: Option<SocketAddr>,
    /// Live `EdgionBackend` shared with the `ServiceSelectorCell` snapshot.
    /// Set by `build_peer_from_lb_selection`; read by `connected_to_upstream`
    /// to call `inc_conn()` and by other attempt hooks for endpoint metadata.
    /// Sticky selection restores the same live Arc; bypass and non-LB paths
    /// leave this field as `None`.
    #[serde(skip)]
    pub lb_edgion_backend: Option<Arc<EdgionBackend>>,
    /// Service key (`"namespace/name"`) that owns this backend, needed by
    /// service-scoped LB runtime state.
    #[serde(skip)]
    pub service_key: Option<String>,
    /// LB policy used for this upstream selection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lb_policy: Option<ParsedLBPolicy>,
    /// Backend TLS connection information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<BackendTlsInfo>,
    /// Attempt-owned LB commit guard. `Some` only when this attempt reached
    /// `connected_to_upstream` AND its policy needs per-attempt accounting
    /// (LeastConn / EWMA). Consumed by `finalize_attempt`:
    ///   - `pg_upstream_peer::begin_upstream_attempt` on retry → FinalizeMode::Discard
    ///   - `pg_logging::lb_runtime_cleanup` at request end    → FinalizeMode::Settle
    /// The guard owns the live backend Arc required for cancellation-safe Drop;
    /// `lb_edgion_backend` remains the selected-endpoint metadata used by other
    /// attempt hooks.
    #[serde(skip)]
    pub lb_committed: Option<crate::lb::LbCommitGuard>,
}

impl UpstreamInfo {
    #[inline]
    pub fn set_response_body_size(&mut self, size: usize) {
        if self.response_body_size.is_some_and(|previous| previous != size) {
            self.err.push("response_body_size already set".to_string());
        }
        self.response_body_size = Some(size);
    }
}

#[cfg(test)]
mod upstream_info_tests {
    use super::*;

    fn upstream_info() -> UpstreamInfo {
        UpstreamInfo {
            ip: None,
            port: None,
            status: None,
            ct: None,
            ht: None,
            bt: None,
            et: None,
            wpt: None,
            response_body_size: None,
            start_time: Instant::now(),
            err: Vec::new(),
            lb_backend_addr: None,
            lb_edgion_backend: None,
            service_key: None,
            lb_policy: None,
            tls: None,
            lb_committed: None,
        }
    }

    #[test]
    fn repeated_response_body_size_is_idempotent_when_unchanged() {
        let mut upstream = upstream_info();

        upstream.set_response_body_size(17);
        upstream.set_response_body_size(17);

        assert_eq!(upstream.response_body_size, Some(17));
        assert!(upstream.err.is_empty());
    }

    #[test]
    fn changed_response_body_size_keeps_double_set_diagnostic() {
        let mut upstream = upstream_info();

        upstream.set_response_body_size(17);
        upstream.set_response_body_size(23);

        assert_eq!(upstream.response_body_size, Some(23));
        assert_eq!(upstream.err, ["response_body_size already set"]);
    }
}

/// Bypass-path tag set when a plugin staged a non-default backend selection.
/// `None` on `BackendContext.source` means the default K8s resource-selected path
/// (the common case — emitting nothing in access log keeps the JSON quiet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendSource {
    /// DirectEndpoint bypass (plugin-staged exact IP).
    DirectEndpoint,
    /// ExternalJump bypass (plugin-staged external domain, DNS-resolved at runtime).
    ExternalJump,
    /// Typed EdgionBackend selected by an AiProxy attempt plan.
    AiBackend,
}

/// Backend context containing service info and upstream attempt history.
///
/// `rsc` and `service_key` are `None` for the bypass paths (DirectEndpoint / ExternalJump),
/// which don't correspond to a K8s-selected backend resource. `source` is `Some` only on
/// those bypass paths; `None` on the default K8s resource path. `upstreams` is always
/// populated as attempts happen.
#[derive(Debug, Serialize)]
pub struct BackendContext {
    /// Bypass-path tag — `None` on the default K8s resource path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSource>,
    /// Resource identifier (kind="Service") — `None` on bypass paths.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsc: Option<Rsc>,
    /// LB store index key `"namespace/name"` — `None` on bypass paths (no LB).
    #[serde(skip)]
    pub service_key: Option<String>,
    /// Routing path (ClusterIP / Endpoint / EPS / ExternalName).
    /// `None` for ExternalJump (DNS-resolved external domain — none of the K8s
    /// service-routing modes apply). `Some` for the default path and DirectEndpoint.
    #[serde(skip)]
    pub service_kind: Option<ServiceBackendKind>,
    /// List of upstream connection attempts (for retry tracking and access log)
    pub upstreams: Vec<UpstreamInfo>,
    /// Current upstream index (for status updates)
    #[serde(skip)]
    pub current_upstream_id: Option<usize>,
    /// Position of the selected backend in `route_unit.rule.backend_refs` (HTTP)
    /// or the gRPC equivalent. `None` on the ExternalJump path (DNS-resolved
    /// external domain — no K8s backend ref) and when the route unit or its
    /// backend_refs list is absent during context initialization. `Some(idx)`
    /// on the normal HTTP/gRPC path, DirectEndpoint, and InternalJump. The
    /// hot metric path falls back to the dynamic lookup when this is `None`.
    #[serde(skip)]
    pub backend_ref_idx: Option<usize>,
}

/// Session persistence state for the current request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPersistenceState {
    /// No session persistence configured or token matched healthy backend (no action needed)
    Inactive,
    /// First request or stale token — inject new token in response
    InjectNew,
    /// Fallback happened — inject updated token in response
    InjectUpdate,
}

/// Stored internal jump target info (lightweight, survives retries via Clone)
///
/// Unlike DirectEndpointPreset which specifies an exact endpoint address,
/// InternalJumpPreset specifies a BackendRef by name. The actual endpoint
/// selection happens via normal LB in get_peer().
///
/// This preset is consumed by select_http_backend() to find the matching
/// BackendRef instead of using weighted round-robin selection.
#[derive(Debug, Clone)]
pub struct InternalJumpPreset {
    /// Name of the target backend_ref (must match a backend_ref in the route)
    pub backend_ref_name: String,
    /// Optional namespace of the target backend_ref
    /// If None, matches by name only
    pub backend_ref_namespace: Option<String>,
}

/// Stored external jump target info (lightweight, survives retries via Clone)
///
/// Unlike DirectEndpointPreset (exact IP) or InternalJumpPreset (BackendRef name),
/// ExternalJumpPreset stores a domain name that requires DNS resolution
/// before creating the HttpPeer.
///
/// DNS resolution happens in upstream_peer_http() phase (async context).
/// On retry, DNS resolution is redone — the domain may resolve to a different
/// IP, providing natural DNS-level failover.
#[derive(Debug, Clone)]
pub struct ExternalJumpPreset {
    /// Target domain name (e.g., "api-us.example.com")
    pub domain: String,
    /// Target port
    pub port: u16,
    /// Whether to use TLS
    pub use_tls: bool,
    /// TLS SNI (Server Name Indication)
    pub sni: String,
    /// True when this jump represents gateway-to-gateway forwarding (e.g.,
    /// `region_route` cross-region traffic). The upstream gateway will run its
    /// own path normalization, so the path should be forwarded raw to avoid
    /// double-normalization conflicts.
    ///
    /// Set to `true` only by plugins that route traffic to another Edgion-like
    /// gateway. Plugins that target a regular external server (e.g.,
    /// `dynamic_external_upstream`) should set `false` so the global
    /// `forward_normalized_path` / per-route annotation policy applies.
    ///
    /// Default `false`: a future plugin that forgets to set this explicitly
    /// will not silently degrade to raw-forwarding behavior.
    pub preserve_raw_path: bool,
    /// Optional per-read upstream timeout supplied by the jump owner.
    ///
    /// When present, this replaces the normal route/global backendRequest
    /// bound before request and gRPC deadline clamping. `None` preserves the
    /// legacy external-jump timeout behavior.
    pub upstream_read_timeout_override: Option<Duration>,
}

/// Stored direct endpoint info (lightweight, survives retries via Clone)
///
/// Unlike Box<HttpPeer> which would be consumed on first use,
/// DirectEndpointPreset persists across retries so upstream_peer can
/// rebuild the same HttpPeer on each attempt.
#[derive(Debug, Clone)]
pub struct DirectEndpointPreset {
    /// Target socket address (ip:port)
    pub addr: std::net::SocketAddr,
    /// Whether to use TLS
    pub use_tls: bool,
    /// SNI for TLS connections
    pub sni: String,
    /// Index of the matched backend_ref in route_unit.rule.backend_refs
    /// Used to pre-select the correct backend_ref for logging and metrics
    pub backend_ref_idx: usize,
}

/// Mutually-exclusive upstream-jump slot — exactly one variant lives per request.
///
/// - `Direct`: bypass LB, peer built from a static IP. Carries `backend_ref`
///   (boxed to keep the enum's stack footprint small — `HTTPBackendRef` is the
///   heaviest variant by far) so the setter can co-write `selected_backend`
///   (peer-construction reads service metadata from `selected_backend`; see
///   `try_direct_endpoint_peer`).
/// - `Internal`: LB-input modifier. Dispatch falls through to `select_http_backend`,
///   which uses the carried preset to look up a named `BackendRef` before LB.
/// - `External`: bypass LB, peer built from a domain (DNS-resolved at runtime).
#[derive(Debug, Clone)]
pub enum UpstreamJumpTarget {
    Direct {
        preset: DirectEndpointPreset,
        backend_ref: Box<HTTPBackendRef>,
    },
    Internal(InternalJumpPreset),
    External(ExternalJumpPreset),
}

/// Outcome of `EdgionHttpContext::try_set_jump` / `PluginSession::set_upstream_jump`.
///
/// First-write-wins: once the slot is set, subsequent calls return `Rejected` and
/// the context is left untouched. Plugin call sites switch on this to log the
/// rejection on their own `PluginLog` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetJumpOutcome {
    Set,
    Rejected,
}

/// Snapshot of mirror config for request runtime.
/// Note: max_concurrent is managed by the Semaphore in RequestMirrorPlugin and
/// is NOT included here — the spawned task does not need it.
#[derive(Debug, Clone)]
pub struct MirrorConfig {
    pub connect_timeout: Duration,
    pub write_timeout: Duration,
    pub max_buffered_chunks: usize,
    pub mirror_log: bool,
}

/// Why a streaming mirror was cancelled before its body completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirrorAbandonReason {
    BodyAbandoned = 1,
    ChannelFull = 2,
}

/// Cancellation guard for an in-flight streaming mirror request.
///
/// Dropping an armed guard fails closed as `BodyAbandoned`. A real downstream
/// EOF must explicitly call [`Self::complete`] to disarm it. This makes future
/// early-return paths cancel the mirror instead of turning a delivered prefix
/// into a clean HTTP EOF.
pub struct MirrorCancellation {
    cancel_tx: Option<oneshot::Sender<()>>,
    reason: Arc<AtomicU8>,
}

impl MirrorCancellation {
    pub(crate) fn new() -> (Self, oneshot::Receiver<()>) {
        let (cancel_tx, cancel_rx) = oneshot::channel();
        (
            Self {
                cancel_tx: Some(cancel_tx),
                reason: Arc::new(AtomicU8::new(0)),
            },
            cancel_rx,
        )
    }

    pub(crate) fn reason_handle(&self) -> Arc<AtomicU8> {
        self.reason.clone()
    }

    pub(crate) fn cancel(mut self, reason: MirrorAbandonReason) {
        self.reason.store(reason as u8, Ordering::Release);
        if let Some(cancel_tx) = self.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
    }

    pub(crate) fn complete(mut self) {
        drop(self.cancel_tx.take());
    }

    pub(crate) fn load_reason(reason: &AtomicU8) -> Option<MirrorAbandonReason> {
        match reason.load(Ordering::Acquire) {
            1 => Some(MirrorAbandonReason::BodyAbandoned),
            2 => Some(MirrorAbandonReason::ChannelFull),
            _ => None,
        }
    }
}

impl Drop for MirrorCancellation {
    fn drop(&mut self) {
        if let Some(cancel_tx) = self.cancel_tx.take() {
            self.reason
                .store(MirrorAbandonReason::BodyAbandoned as u8, Ordering::Release);
            let _ = cancel_tx.send(());
        }
    }
}

/// Mirror stream state kept in request context.
pub enum MirrorState {
    /// Mirror request task is active and waiting for body chunks from request_body_filter.
    Streaming {
        /// Cancels the in-flight mirror request without ending its streamed
        /// body as a clean HTTP EOF. Keep this field first: Rust drops enum
        /// fields in declaration order, so implicit state destruction must
        /// signal cancellation before `body_tx` can yield a normal EOF.
        cancellation: MirrorCancellation,
        body_tx: mpsc::Sender<Result<Bytes, std::io::Error>>,
        writer_handle: JoinHandle<()>,
        /// Maximum milliseconds to wait for channel space before abandoning the mirror.
        /// 0 = immediate abandon (zero impact on main request latency, default).
        /// > 0 = brief back-pressure window (adds at most this many ms to body processing).
        channel_full_timeout_ms: u64,
    },
    /// Mirror has been disabled for this request (timeout/error/full buffer/closed channel).
    Abandoned,
}

/// A response-header mutation queued by a plugin in a stage where the real
/// `ResponseHeader` is not yet available (typically `request_filter`).
///
/// Applied non-destructively onto the forwarded header at the upstream-response
/// stage (`upstream_response_filter` via `apply_pending_response_header_ops`),
/// and drained destructively (`drain_pending_response_header_ops`) when building
/// a response-phase local reply (`build_response_local_reply`) and at
/// request-stage termination points that build their own response (`ErrResponse`,
/// gRPC errors).
#[derive(Debug, Clone)]
pub enum PendingHeaderOp {
    Set { name: String, value: String },
    Append { name: String, value: String },
    Remove { name: String },
}

impl PendingHeaderOp {
    /// Apply this queued operation to a real ResponseHeader.
    ///
    /// `insert_header` / `append_header` errors (invalid header value bytes,
    /// e.g. control chars) are intentionally swallowed: dropping a malformed
    /// queued op silently is preferable to failing the entire response, and
    /// matches the behavior of the pre-tagged-queue implementation.
    pub fn apply(self, response_header: &mut ResponseHeader) {
        match self {
            PendingHeaderOp::Set { name, value } => {
                let _ = response_header.insert_header(name, value);
            }
            PendingHeaderOp::Append { name, value } => {
                let _ = response_header.append_header(name, value);
            }
            PendingHeaderOp::Remove { name } => {
                response_header.remove_header(&name);
            }
        }
    }
}

/// Which upper bound won the read/write-timeout fold in
/// `configure_peer_timeouts` for the current upstream attempt.
/// Consumed by `error_while_proxy` to decide whether a timeout is a
/// per-try timeout (retryable, GEP-1731) or a deadline expiry (final).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerTimeoutSource {
    /// Route `timeouts.backendRequest`, or the global default fallback.
    BackendRequest,
    /// Route `timeouts.request` remaining budget.
    RequestDeadline,
    /// Client-supplied `grpc-timeout` remaining budget.
    GrpcDeadline,
}

/// A response-phase plugin denial that the response stage could not deliver on
/// its own, carried to the abort path.
///
/// A denial the response stage cannot express is NOT permission to forward the
/// upstream response. The runners answer such a plugin result by calling
/// `set_terminate`, which only writes `ctx.plugin_running_result` — and every
/// reader of that field is request-stage, so on its own it is a silent ALLOW.
/// Setting this instead makes the response-stage entry point abort the exchange,
/// and the abort is raised as `ErrorType::HTTPStatus`, the same mechanism the
/// retry-budget denial in `upstream_response_filter` already uses: the upstream
/// response is never forwarded, the upstream connection is torn down rather than
/// reused, and `fail_to_proxy` delivers the denial downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseStageDenial {
    /// `ErrResponse` on an exchange that is no longer a replaceable HTTP
    /// response — a real `101`, an established CONNECT tunnel, or a gRPC route
    /// (see `apply_response_local_reply` / `build_response_local_reply`).
    /// `fail_to_proxy`'s ordinary status handling writes it.
    Http(u16),

    /// `ErrGrpcError`, which names a gRPC status directly. The sync response
    /// runner cannot await, so it cannot write the trailers itself, and routing
    /// the code through an HTTP status would flatten it through a lossy mapping.
    /// `fail_to_proxy` writes these trailers verbatim.
    Grpc { status: u16, message: Option<String> },
}

/// Classification of the upstream response as a streaming protocol. Set once
/// per upstream attempt in `pg_upstream_response_filter` from the normalized
/// Content-Type; reassigned on retry (a later attempt may answer with a
/// different media type). Explicit media types only — absence of
/// Content-Length or chunked framing never implies streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseStreamKind {
    /// Not a recognized streaming media type (the default).
    #[default]
    None,
    /// `text/event-stream` (Server-Sent Events).
    Sse,
}

pub struct EdgionHttpContext {
    /// Request start time for latency calculation
    pub start_time: Instant,

    /// Gateway information (copied from EdgionHttp for easy access)
    pub gateway_info: GatewayInfo,

    /// Request information (hostname, path, x-trace-id)
    pub request_info: RequestInfo,

    /// Informational statuses collected during request processing
    pub info: Vec<EdgionStatus>,

    /// Error codes collected during request processing
    pub edgion_status: Vec<EdgionStatus>,

    /// Matched HTTP route unit containing full route information
    pub route_unit: Option<Arc<HttpRouteRuleUnit>>,

    /// Selected HTTP backend from load balancing (schema + compiled plugin runtime)
    pub selected_backend: Option<SelectedHttpBackend>,

    /// Matched gRPC route unit (for gRPC routes)
    pub grpc_route_unit: Option<Arc<GrpcRouteRuleUnit>>,

    /// Selected gRPC backend (schema + compiled plugin runtime, for gRPC routes)
    pub selected_grpc_backend: Option<CompiledGrpcBackend>,

    /// Whether this request is handled by GRPCRoute (not just gRPC protocol)
    /// Used to determine backend peer selection and plugin execution
    pub is_grpc_route_matched: bool,

    /// Backend context containing service info and upstream attempts
    pub backend_context: Option<BackendContext>,

    /// Stage execution logs (grouped by stage)
    pub stage_logs: Vec<StageLogs>,

    /// Pending EdgionPlugins logs (collected during current stage, merged at stage end)
    pub pending_edgion_plugins_logs: Vec<EdgionPluginsLog>,

    /// Plugin running result
    pub plugin_running_result: PluginRunningResult,

    /// Host-owned identity of the request-stage plugin entry currently being
    /// invoked. Nested ExtensionRef execution replaces and restores this slot.
    pub(crate) current_plugin_entry: Option<CurrentPluginEntryIdentity>,

    /// First-writer latch for an A2A Gateway activated by the matched rule.
    pub(crate) a2a_gateway_activated: bool,

    /// Unified backend generation pinned by A2A protocol classification.
    pub(crate) edgion_backend_snapshot: Option<EdgionBackendSnapshot>,

    /// Number of connection attempts to backends
    pub try_cnt: u32,

    /// Signal set by upstream_response_filter when upstream status code matches retry.codes.
    /// Consumed by error_while_proxy to trigger retry via e.set_retry(true).
    /// Reset to false after each retry attempt.
    pub retry_on_status: bool,

    /// Source of the read/write timeout applied to the current upstream
    /// attempt's peer. Overwritten by `configure_peer_timeouts` on every
    /// attempt; `None` when no read/write bound was applied.
    pub peer_timeout_source: Option<PeerTimeoutSource>,

    /// Time when first upstream connection was initiated
    /// Set only once on first connection attempt
    pub upstream_start_time: Option<Instant>,

    /// Hash key used for consistent hashing (for test metrics)
    pub hash_key: Option<String>,

    /// Context variables map for plugin communication
    /// Plugins can set values (e.g., KeySet plugin) and conditions can read them
    pub ctx_map: HashMap<String, String>,

    /// Composed upstream authority for this request, when the target Service
    /// carries an `upstreamAuthority` policy. Written in `upstream_peer` (which
    /// also sets it as the peer SNI) and read in `upstream_request_filter`, which
    /// is the only place the upstream `Host` header can be written.
    pub upstream_authority: Option<String>,

    /// Lazily extracted path parameters from route pattern (e.g., "/api/:uid/profile")
    /// - None: not yet extracted
    /// - Some(HashMap): already extracted (may be empty if no params or no match)
    pub path_params: Option<HashMap<String, String>>,

    /// Lazily-built per-request decoded query parameter map.
    /// Initialized on first access via `cached_query()`. Shared between
    /// route deep-match and `PluginSession::get_query_param` so the same
    /// query string is parsed (and url-decoded) at most once per request.
    /// `std::sync::OnceLock` is used (not `OnceCell`) because Pingora requires
    /// the per-request CTX type to be `Send + Sync`; per-request access is
    /// still single-threaded so `&self` init-once is sufficient.
    /// Field is named `_cell` to disambiguate from the `cached_query()` method
    /// and prevent accidental in-crate access that would bypass the helper's
    /// entry-snapshot semantics.
    pub(crate) cached_query_cell: std::sync::OnceLock<HashMap<String, String>>,

    /// Unified upstream-jump slot. Set by request-stage plugins via
    /// `PluginSession::set_upstream_jump`. First-write-wins; subsequent setter
    /// calls are rejected (see `SetJumpOutcome`). Replaces the legacy
    /// `direct_endpoint` / `internal_jump` / `external_jump` triplet.
    pub jump: Option<UpstreamJumpTarget>,

    /// True when `req_header.uri` is already in its upstream-forwarded final form;
    /// further writes are forbidden.
    ///
    /// Set in exactly two locations (matching the §9 Rule 2 lint allowlist):
    ///   1. `session_adapter::set_upstream_uri` — set after rewrite-class plugin writes
    ///   2. `pg_upstream_peer::upstream_peer_http` single-point decision block —
    ///      set after the default-normalized vs raw decision is finalized
    ///
    /// Set at most once per request lifecycle. Covers:
    ///   - rewrite plugin "final form" semantics
    ///   - single-point decision idempotency (retries don't re-decide)
    ///   - mirror / fan-out consumers see consistent `req_header.uri`
    pub uri_finalized: bool,

    /// Response-header mutations queued by plugins running before the real
    /// `ResponseHeader` is available (typically request stage). Drained onto
    /// the real `ResponseHeader` at four sites:
    ///   1. `upstream_response_filter` (Phase 6) — applied NON-DESTRUCTIVELY via
    ///      `apply_pending_response_header_ops` before any response-stage plugin
    ///      runs, so they can further mutate the already-merged headers. The queue
    ///      is left intact so a subsequent response-phase local reply can re-apply
    ///      the ops onto its fresh header (filter-04c).
    ///   2. `ErrResponse` request-stage termination handler (HTTP route) —
    ///      applied after runtime defaults (`content-type: text/plain`,
    ///      `content-length`) so plugin Set ops override defaults. The gRPC
    ///      branch dispatches to `send_grpc_error_direct` instead and lets
    ///      that site handle the drain; on that branch the user-provided
    ///      `body` is forwarded as the `grpc-message` trailer (with the
    ///      `http_to_grpc_status` default message as fallback when `body`
    ///      is `None`).
    ///   3. `send_grpc_error_direct` — applied before re-asserting
    ///      `content-type: application/grpc` so the gRPC contract value
    ///      cannot be overwritten by a queued Set.
    ///   4. `send_grpc_trailers_only` — same single-frame Trailers-Only pattern
    ///      as (3), reached when a plugin calls `write_response_header` directly
    ///      with a non-200 status on a gRPC route.
    ///
    /// Each entry carries an explicit op (Set/Append/Remove) so semantics
    /// are preserved across the deferral — without this tag, an `Append`
    /// would silently degrade to `Set` at drain time.
    pub pending_response_header_ops: Vec<PendingHeaderOp>,

    /// Dye plan of the plugin entry currently being invoked (installed by the
    /// stage loops before each invocation, overwritten per invocation).
    /// Arc so the adapter lifts it out before iterating (render mutates ctx).
    pub current_dye_plan: Option<Arc<crate::plugins::runtime::CompiledDyePlan>>,
    /// Request-direction dye names already handled this request (client value
    /// stripped once; gateway-rendered values are never re-stripped).
    pub dye_handled_request_names: Vec<String>,

    /// Per-request mirror state for RequestMirror plugin.
    pub mirror_state: Option<MirrorState>,

    /// Session persistence state (set during request phase).
    /// When InjectNew or InjectUpdate, upstream_response_filter will inject a token.
    pub sp_state: SessionPersistenceState,

    /// Session persistence: resolved target endpoint from affinity map lookup.
    /// When Some, try_get_peer will route directly to this addr instead of running LB.
    pub sp_target_addr: Option<std::net::SocketAddr>,

    /// Session persistence: the sticky target resolved from the affinity map,
    /// retained across attempts so a strict-mode retry can re-pin to the same
    /// backend (`sp_target_addr` is consumed by `take()` on the first attempt).
    /// Plain field, no serde attribute (`EdgionHttpContext` has no serde derive).
    pub sp_sticky_addr: Option<std::net::SocketAddr>,
    /// Optional typed scope used by the canonical affinity token/map.
    pub sp_token_scope: Option<String>,

    /// Direct evidence that an earlier attempt consumed the SP sticky target
    /// (`sp_target_addr.take()` returned `Some`). Drives the cross-attempt token
    /// refresh in `upstream_peer_http`. Plain field, no serde attribute
    /// (`EdgionHttpContext` has no serde derive).
    pub sp_sticky_consumed: bool,

    /// Per-Gateway operator override for loopback upstream addresses.
    /// Link-local / metadata ranges are never allowed by this switch.
    pub allow_loopback_upstream: bool,

    /// Resolved GatewayClass request-body policy.
    /// Parsed when the listener is built; the request hot path only copies and
    /// reads these numeric values.
    pub request_body_config: crate::request_body::ResolvedRequestBodyConfig,

    /// Full request-body cache and mutation framing state. Its read-only view
    /// stays available through the final upstream-response header plugin chain
    /// and is then released before streaming the response.
    pub request_body: RequestBodyState,

    /// Streamed request-body handlers installed by request-stage plugins
    /// (e.g. ExtProc `STREAMED`). Driven per transport body event by
    /// `request_body_filter_action`. Empty for the overwhelming majority of
    /// requests — the emptiness test is the per-chunk fast-path gate.
    ///
    /// `SyncWrapper` grants only `&mut` access, which makes the `!Sync`
    /// handler internals (a live gRPC stream) safe inside the Sync-required
    /// context; all driving is `&mut` anyway.
    ///
    /// NOT the monotonic source of truth: the hook temporarily `take`s this
    /// vector across `.await` points, and the downstream future can be
    /// dropped mid-hook (the proxy's upstream-error `select!` arm), losing
    /// the taken handlers. Retry/disposition predicates key on
    /// [`Self::request_body_stream_active`] instead. Accepted cost of that
    /// drop: the lost handlers' pending access-log entries die with them, so
    /// a request killed by an upstream error mid-chunk has no
    /// `request_body_filters` stage in its access log.
    pub request_body_stream:
        Vec<sync_wrapper::SyncWrapper<Box<dyn crate::plugins::runtime::traits::RequestBodyStreamHandler>>>,

    /// Monotonic latch: set when the first streamed body handler is installed
    /// and NEVER cleared, surviving even a downstream future dropped mid-hook
    /// (which can empty `request_body_stream`). `request_retry_allowed`,
    /// `upstream_request_body_disposition`, and the retry `buffer_ok` gates
    /// derive from this latch — the contract requires the retry predicate to
    /// be monotonic, and a lost handler must fail the request rather than
    /// re-arm retries that would forward the remaining body uninspected.
    pub request_body_stream_active: bool,

    /// Optional fixed-capacity, read-only request-body observer registry.
    /// Allocated only when an internal request-stage plugin successfully
    /// registers; it never owns or replays body bytes.
    pub(crate) request_body_observers: Option<Box<crate::request_body::observer::RequestBodyObservers>>,

    /// First-write-wins request-chain guard for JSON Schema validation.
    ///
    /// A request may compose plugins from global, Gateway, Route, and BackendRef
    /// sources even though each individual EdgionPlugins resource permits only
    /// one validator. The first validator that actually runs claims this slot;
    /// a second validator fails closed before materializing or parsing JSON.
    pub(crate) json_schema_validation_claimed: bool,

    /// Snapshot of resolved global plugin runtimes for this request, taken at
    /// `request_filter` entry. Each entry is one EdgionPlugins resource;
    /// per-stage bucketing is handled by `PluginRuntime`'s internal stage Vecs.
    /// `None` until resolve runs; `Some` with empty Vec if config has no
    /// `global_plugins_ref` or all refs are dangling.
    ///
    /// Populated by `pg_request_filter` (early stage), read by:
    ///   - Step-2 global iteration in `pg_request_filter`
    ///   - global iteration in `pg_upstream_response_filter` (sync)
    ///   - global iteration in `pg_upstream_response_body_filter` (per-chunk hot path)
    ///   - global iteration in `pg_response_filter` (async)
    ///
    /// `Arc<Vec<...>>` lets call sites `Arc::clone` once to release the borrow on
    /// `ctx` before iterating, avoiding `&ctx` / `&mut ctx` conflicts when running
    /// plugins.
    pub resolved_global_plugins: Option<Arc<Vec<Arc<PluginRuntime>>>>,

    /// Snapshot of resolved Gateway-level plugin runtimes for this request (from the
    /// `edgion.io/edgion-plugins` annotation on the matched Gateway). Runs after
    /// GatewayClass-wide global plugins and before route-level plugins. `None` until
    /// resolve runs; `Some` with empty Vec when the Gateway has no annotation or all
    /// refs dangle.
    ///
    /// Populated by `pg_request_filter` Step 2b; read by response stages (inner→outer
    /// order: route-level → Gateway-level → GatewayClass-global).
    pub resolved_gateway_plugins: Option<Arc<Vec<Arc<PluginRuntime>>>>,

    /// Resolved targets of every ExtensionRef in the matched route's filter list,
    /// indexed by `ExtensionRefFilter.index` (assigned at route build time by
    /// `PluginRuntime::from_httproute_filters_for_rule` /
    /// `from_grpcroute_filters_for_rule`).
    ///
    /// Slot value `None` = dangling ref (target not in plugin store at resolve
    /// time); the slot is preserved so the index continues to align with
    /// `ExtensionRefFilter.index`.
    /// Outer `None` = resolve has not run yet.
    ///
    /// Populated by `pg_request_filter`, read by `ExtensionRefFilter::prepare_execution`
    /// via `PluginSession::resolved_route_ext_ref(index)`.
    pub resolved_route_ext_refs: Option<Arc<Vec<Option<Arc<PluginRuntime>>>>>,

    /// Bounded AI request metadata and shared atomic usage counters. Set once
    /// by the first effective AiProxy entry; contains no prompt or credential.
    pub ai: Option<AiRequestContext>,

    /// Immutable canonical request and candidate snapshot for AI forwarding.
    /// The canonical body is never rendered by Debug or access logging.
    pub ai_attempt_plan: Option<AiAttemptPlan>,

    /// Request-body generation observed by `AiGuard` when it settled its
    /// decision, and the identities of guard entries that already decided.
    ///
    /// `AiProxy` compares the generation against the current one after reading
    /// the body: a difference means something mutated the body between the
    /// guard and the proxy, so the guard's mask is not what the provider would
    /// receive. This is the exact, scope-independent ordering guarantee;
    /// plugin ordering alone cannot provide it.
    pub ai_guard_body_generation: Option<u64>,
    /// One-shot latch so a re-entered guard (the backendRef chain re-runs on
    /// every upstream attempt) never selects, evaluates, or masks twice.
    pub ai_guard_decided: Vec<String>,

    /// Bounded active-attempt and pending Redis settlement state. Async work is
    /// drained only at the next-attempt and universal logging seams.
    pub ai_attempt_runtime: crate::ai::attempt_runtime::AiAttemptRuntimeState,

    /// Attempt-owned aggregate permit for a non-AI logical backend.
    pub logical_backend_cb_permit: Option<crate::ai::circuit_breaker::LogicalBackendCircuitBreakerPermit>,

    /// Request-captured canonical DNS and dial-time egress classifier for a
    /// routed external backend. Retries share its bounded resolver memo.
    pub(crate) routed_external_resolver: Option<Arc<crate::plugins::http::common::outbound::egress::EgressResolver>>,

    /// One-shot guard for an external A2A target's transport base path. The
    /// same request context re-enters peer selection on retry.
    pub(crate) a2a_path_prefix_applied: bool,

    /// Request-captured typed destination policy for the selected AI backend.
    pub ai_effective_policy: Option<Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy>>,

    /// Pristine post-rule request used to rebuild later AI attempts.
    pub ai_request_baseline: Option<pingora_http::RequestHeader>,

    /// Replaceable provider-aware response processor for the active AI attempt.
    pub ai_stream_processor: Option<Box<dyn StreamingResponseProcessor>>,

    /// Client-facing AI stream dialect selected at canonical AI ingress.
    pub(crate) ai_client_stream_dialect: Option<AiClientStreamDialect>,

    /// Final method/status/media eligibility captured after the response onion.
    pub(crate) final_response_eligibility: Option<FinalResponseEligibility>,

    /// Stable processor instance currently executing in the response driver.
    pub(crate) active_response_processor_slot: Option<ResponseProcessorSlot>,

    /// First-wins request-local terminal and close ownership.
    pub(crate) response_terminal_state: ResponseTerminalState,

    /// Immutable GenAI usage summary committed once during final logging.
    /// `Some` is also the idempotence sentinel for metric emission.
    pub genai_summary: Option<GenAiSummary>,

    /// Streaming-response processors installed for this request by request-stage
    /// plugins (e.g. AiProxy's usage extractor). Run in installation order from
    /// `pg_upstream_response_body_filter`, before the read-only body-filter
    /// chains. Not operator-facing; not a plugin stage.
    pub stream_processors: Vec<Box<dyn StreamingResponseProcessor>>,

    /// Whether one request-stage semantic response guard claimed the canonical
    /// AI source. The first matching guard wins for the request lifetime.
    pub semantic_response_guard_claimed: bool,

    /// Same-callback handoff from the canonical AI source to the dedicated
    /// semantic guard. It is never a cross-request or detached queue.
    pub canonical_record_batch: Option<crate::ai::canonical_sse::CanonicalRecordBatch>,

    /// Dedicated canonical semantic guard. It runs after the AI source and
    /// before every ordinary writable response processor.
    pub semantic_response_processor: Option<Box<dyn StreamingResponseProcessor>>,

    /// Request-scoped ExtProc transactions keyed by the compiled request-entry
    /// identity. The request filter and its internal response companion share
    /// these owners; targets are never used as transaction identity.
    pub attached_ext_proc:
        std::collections::HashMap<String, std::sync::Arc<crate::plugins::http::ext_proc::AttachedExtProc>>,

    /// The single request-scoped semantic Guardrail owner and its compiled
    /// request-entry identity. Multiple matched entries are rejected.
    pub attached_guardrail: Option<(
        String,
        std::sync::Arc<crate::plugins::http::guardrail::AttachedGuardrail>,
    )>,

    /// The request-scoped A2A response observer and its request-entry log
    /// identity. It only sees the final downstream-visible body chunks; the
    /// observer never owns or changes response bytes.
    pub attached_a2a_observation: Option<(String, crate::plugins::http::a2a_gateway::AttachedA2aObservation)>,

    /// Early framing reservations for attached processors that may mutate the
    /// final response body before their concrete processor is installed.
    pub attached_response_framing_reservations: usize,

    /// Becomes true at the first response body callback. Late processor
    /// installation is rejected to keep response composition deterministic.
    pub response_body_processing_started: bool,

    /// Response-phase local-reply body, set by a response-phase plugin that
    /// returned `ErrResponse { status, body }` (Phase 6 sync or Phase 7 async)
    /// and consumed by `upstream_response_body_filter` (Phase 8). `Some` until
    /// the body filter injects it on the first chunk; the replacement response
    /// header (status + framing) was already rewritten in place during the
    /// plugin phase.
    pub local_reply_body: Option<bytes::Bytes>,

    /// Stashed response-phase local-reply header. `Some` means a
    /// reply is pending; `response_filter` writes it downstream and aborts the
    /// pump. The gateway Server header is applied at write time.
    pub local_reply_header: Option<Box<ResponseHeader>>,

    /// Set true once `response_filter` has written the local reply downstream.
    /// Read by error_while_proxy / fail_to_proxy / suppress_error_log to treat
    /// the sentinel abort as an intentional reply, not a proxy failure.
    pub local_reply_written: bool,

    /// A response-phase plugin denial the response stage could not deliver
    /// itself. See [`ResponseStageDenial`].
    pub response_denial: Option<ResponseStageDenial>,

    /// See [`ResponseStreamKind`]. Consumed by the compression exemption and by
    /// streaming processors/plugins that must know how to interpret chunks.
    pub response_stream_kind: ResponseStreamKind,

    /// Circuit-breaker admission permit for the chosen endpoint, acquired at
    /// `upstream_peer` (pre-connect) when the target Service carries a
    /// `circuitBreaker` concurrency cap. Holding it on the ctx ties the permit's
    /// lifetime to the request: it releases exactly once when the ctx drops at
    /// request end, covering every terminal path (success, upstream error,
    /// client disconnect, cancellation). `None` when no cap applies or admission
    /// was not attempted (bypass paths, no policy).
    pub cb_permit: Option<crate::backends::circuit_breaker::CbPermit>,

    /// Geo for the direct peer address (`request_info.client_addr`).
    ///
    /// `OnceLock` rather than `RefCell`: `extract_rule_values` and other geo
    /// consumers take `&self`, and this context must stay `Send + Sync`
    /// (Pingora requires the per-request CTX type to be `Send + Sync`; see the
    /// `cached_query_cell` comment above for the same reasoning). Set-once is
    /// exactly the semantics needed — a resolved lookup, including a
    /// `NotFound`, is never re-queried.
    pub direct_geo: std::sync::OnceLock<crate::geoip::GeoLookup>,

    /// Geo for the real client address (`request_info.remote_addr`).
    ///
    /// Filled lazily by its first consumer, never eagerly at request entry:
    /// the `real_ip` plugin can overwrite `remote_addr` after the
    /// listener-level extractor runs, and an eager fill would cache the
    /// pre-plugin address.
    pub client_geo: std::sync::OnceLock<crate::geoip::GeoLookup>,
}

/// Parse a `RequestInfo` address field as an `IpAddr`, tolerating either a
/// bare IP or a `SocketAddr` (`ip:port`). Mirrors
/// `real_ip_extractor::extract_ip_from_socket_addr`'s try-`SocketAddr`-then-
/// `IpAddr` fallback, but returns `None` on failure instead of defaulting to
/// `0.0.0.0` — an unparseable geo input must become `GeoLookup::NotFound`,
/// never a lookup against a bogus address.
fn parse_ip_str(addr: &str) -> Option<std::net::IpAddr> {
    addr.parse::<std::net::SocketAddr>()
        .map(|sa| sa.ip())
        .or_else(|_| addr.parse::<std::net::IpAddr>())
        .ok()
}

impl Default for EdgionHttpContext {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgionHttpContext {
    pub async fn replace_request_body_slice(
        &mut self,
        range: std::ops::Range<usize>,
        replacement: Bytes,
    ) -> crate::plugins::runtime::traits::session::BodyReadResult {
        self.request_body.replace_slice(range, replacement).await
    }

    pub fn overwrite_request_body(
        &mut self,
        body: Bytes,
    ) -> Result<(), crate::plugins::runtime::traits::session::BodyReadError> {
        self.request_body.overwrite(body)
    }

    /// Claim the one JSON Schema validation slot for this request.
    ///
    /// Returns `true` only to the first caller.
    pub(crate) fn claim_json_schema_validation(&mut self) -> bool {
        if self.json_schema_validation_claimed {
            false
        } else {
            self.json_schema_validation_claimed = true;
            true
        }
    }

    pub(crate) fn try_set_ai_request_context(&mut self, context: AiRequestContext) -> bool {
        if self.ai.is_some() {
            false
        } else {
            self.ai = Some(context);
            self.ai_client_stream_dialect = Some(AiClientStreamDialect::OpenAiCompatibleSse);
            true
        }
    }

    /// Replace attempt-local provider/usage state while retaining the one
    /// request-wide quota handle across AI failover attempts.
    pub(crate) fn replace_ai_attempt_context(&mut self, mut context: AiRequestContext) {
        self.ai_client_stream_dialect = Some(AiClientStreamDialect::OpenAiCompatibleSse);
        if let Some(mut previous) = self.ai.take() {
            if context.quota.is_none() {
                context.quota = previous.quota.take();
            }
            if context.quota_status.is_none() {
                context.quota_status = previous.quota_status;
            }
        }
        self.ai = Some(context);
    }

    pub(crate) fn try_set_ai_attempt_plan(&mut self, plan: AiAttemptPlan) -> bool {
        if self.ai_attempt_plan.is_some() {
            false
        } else {
            self.ai_attempt_plan = Some(plan);
            true
        }
    }

    /// Drain queued response-header ops onto the given real `ResponseHeader`,
    /// preserving each op's original semantics (Set/Append/Remove). Called
    /// from drain points: upstream-response stage and request-stage termination
    /// paths that build their own response (ErrResponse, gRPC errors).
    pub fn drain_pending_response_header_ops(&mut self, response_header: &mut ResponseHeader) {
        if self.pending_response_header_ops.is_empty() {
            return;
        }
        let ops = std::mem::take(&mut self.pending_response_header_ops);
        for op in ops {
            op.apply(response_header);
        }
    }

    /// Apply queued response-header ops onto `response_header` WITHOUT consuming
    /// the queue (each op is cloned). This is the single application point on the
    /// normal forward path, called at the upstream-response stage
    /// (`upstream_response_filter`). The queue is deliberately left intact so that
    /// if a response-phase plugin subsequently replaces the response with a local
    /// reply, `build_response_local_reply` can re-apply the same ops onto the
    /// fresh reply header via `drain_pending_response_header_ops` (filter-04c).
    ///
    /// Placed before the response-stage sync plugin runners so those plugins still
    /// observe the merged headers (the semantic the former in-runner pre-drain
    /// provided).
    pub fn apply_pending_response_header_ops(&self, response_header: &mut ResponseHeader) {
        for op in &self.pending_response_header_ops {
            op.clone().apply(response_header);
        }
    }

    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            gateway_info: GatewayInfo::default(),
            request_info: RequestInfo::default(),
            info: Vec::with_capacity(3),
            edgion_status: Vec::with_capacity(5),
            route_unit: None,
            selected_backend: None,
            grpc_route_unit: None,
            selected_grpc_backend: None,
            is_grpc_route_matched: false,
            backend_context: None,
            stage_logs: Vec::with_capacity(3),
            pending_edgion_plugins_logs: Vec::new(),
            plugin_running_result: PluginRunningResult::Nothing,
            current_plugin_entry: None,
            a2a_gateway_activated: false,
            edgion_backend_snapshot: None,
            try_cnt: 0,
            retry_on_status: false,
            peer_timeout_source: None,
            upstream_start_time: None,
            hash_key: None,
            ctx_map: HashMap::new(),
            upstream_authority: None,
            path_params: None,
            cached_query_cell: std::sync::OnceLock::new(),
            jump: None,
            uri_finalized: false,
            pending_response_header_ops: Vec::new(),
            current_dye_plan: None,
            dye_handled_request_names: Vec::new(),
            mirror_state: None,
            sp_state: SessionPersistenceState::Inactive,
            sp_target_addr: None,
            sp_sticky_addr: None,
            sp_token_scope: None,
            sp_sticky_consumed: false,
            allow_loopback_upstream: false,
            request_body_config: crate::request_body::ResolvedRequestBodyConfig::default(),
            request_body: RequestBodyState::default(),
            request_body_stream: Vec::new(),
            request_body_stream_active: false,
            request_body_observers: None,
            json_schema_validation_claimed: false,
            resolved_global_plugins: None,
            resolved_gateway_plugins: None,
            resolved_route_ext_refs: None,
            ai: None,
            ai_attempt_plan: None,
            ai_guard_body_generation: None,
            ai_guard_decided: Vec::new(),
            ai_attempt_runtime: crate::ai::attempt_runtime::AiAttemptRuntimeState::default(),
            logical_backend_cb_permit: None,
            routed_external_resolver: None,
            a2a_path_prefix_applied: false,
            ai_effective_policy: None,
            ai_request_baseline: None,
            ai_stream_processor: None,
            ai_client_stream_dialect: None,
            final_response_eligibility: None,
            active_response_processor_slot: None,
            response_terminal_state: ResponseTerminalState::Open,
            genai_summary: None,
            stream_processors: Vec::new(),
            semantic_response_guard_claimed: false,
            canonical_record_batch: None,
            semantic_response_processor: None,
            attached_ext_proc: std::collections::HashMap::new(),
            attached_guardrail: None,
            attached_a2a_observation: None,
            attached_response_framing_reservations: 0,
            response_body_processing_started: false,
            local_reply_body: None,
            local_reply_header: None,
            local_reply_written: false,
            response_denial: None,
            response_stream_kind: ResponseStreamKind::None,
            cb_permit: None,
            direct_geo: std::sync::OnceLock::new(),
            client_geo: std::sync::OnceLock::new(),
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionStatus) {
        self.edgion_status.push(err_code);
    }

    /// Add an informational status to the context
    pub fn add_info(&mut self, info_code: EdgionStatus) {
        self.info.push(info_code);
    }

    /// Get (and lazily build) the per-request decoded query parameter map.
    ///
    /// Callers should pass `self.request_info.entry_raw_query.as_deref()` —
    /// the entry-time frozen query — so the cache is order-independent: any
    /// reader (route matcher or plugin) sees the same client-sent query
    /// regardless of whether an earlier plugin called `set_upstream_uri` and
    /// rewrote the live URI.
    ///
    /// `raw_query` is consulted only on first access. Subsequent calls return
    /// the already-cached map regardless of the argument.
    ///
    /// Decoding follows `edgion_resources::uri_utils::parse_query_string` —
    /// `%XX` percent-decoded and `+` translated to space, matching what the
    /// route matcher sees in `queryParams`.
    pub fn cached_query(&self, raw_query: Option<&str>) -> &HashMap<String, String> {
        self.cached_query_cell
            .get_or_init(|| raw_query.map(uri_utils::parse_query_string).unwrap_or_default())
    }

    /// Geo for the real client address (`request_info.remote_addr`). Resolves
    /// on first call, then caches. Never queries eagerly — the `real_ip`
    /// plugin can rewrite `remote_addr` after the listener-level extractor
    /// runs, so filling this at request entry would cache the wrong address.
    pub fn client_geo(&self) -> &crate::geoip::GeoLookup {
        self.client_geo.get_or_init(|| {
            self.resolve_geo(
                &self.request_info.remote_addr,
                &self.direct_geo,
                &self.request_info.client_addr,
            )
        })
    }

    /// Geo for the direct peer address (`request_info.client_addr`). Resolves
    /// on first call, then caches.
    pub fn direct_geo(&self) -> &crate::geoip::GeoLookup {
        self.direct_geo.get_or_init(|| {
            self.resolve_geo(
                &self.request_info.client_addr,
                &self.client_geo,
                &self.request_info.remote_addr,
            )
        })
    }

    /// Resolve `addr`, reusing `other_slot` when the two addresses normalize
    /// equal. Most requests have no `X-Forwarded-For`, so `client_addr` and
    /// `remote_addr` are usually the same host — and comparing the raw
    /// strings would miss that, because `::ffff:203.0.113.5` and
    /// `203.0.113.5` are the same host spelled two ways. Mirrors
    /// `real_ip_extractor`'s handling of `client_addr`: try `SocketAddr`
    /// first (a peer address may carry a port), then fall back to a bare
    /// `IpAddr`. An address that fails to parse yields `NotFound`, not a
    /// panic.
    fn resolve_geo(
        &self,
        addr: &str,
        other_slot: &std::sync::OnceLock<crate::geoip::GeoLookup>,
        other_addr: &str,
    ) -> crate::geoip::GeoLookup {
        use crate::geoip::info::normalize_addr;
        use crate::geoip::{GeoIpProvider, GeoLookup};

        let Some(ip) = parse_ip_str(addr) else {
            return GeoLookup::NotFound;
        };
        let ip = normalize_addr(ip);

        if let Some(cached) = other_slot.get() {
            if parse_ip_str(other_addr).map(normalize_addr) == Some(ip) {
                return cached.clone();
            }
        }
        GeoIpProvider::global().lookup(ip)
    }

    /// Resolve the route-level `request_timeout`, preferring HTTP `route_unit`
    /// over `grpc_route_unit`. Returns `None` when neither side resolves to a
    /// concrete duration (either side may have `parsed_timeouts = None`, or have
    /// `request_timeout` set to `Unset`/`Disabled`, both of which yield `None`
    /// from `as_duration()`).
    ///
    /// The fallback is applied at the resolved `Duration` level — not at the
    /// `parsed_timeouts` level — so that an HTTP route with `parsed_timeouts`
    /// present but `request_timeout` not configured correctly falls through to
    /// the gRPC side.
    #[inline]
    pub fn request_timeout(&self) -> Option<Duration> {
        self.route_unit
            .as_ref()
            .and_then(|u| u.rule.parsed_timeouts.as_ref())
            .and_then(|t| t.request_timeout.as_duration())
            .or_else(|| {
                self.grpc_route_unit
                    .as_ref()
                    .and_then(|u| u.rule.parsed_timeouts.as_ref())
                    .and_then(|t| t.request_timeout.as_duration())
            })
    }

    /// True when a route-level `request_timeout` is set and `start_time`
    /// has already exceeded it. Returns `false` when no timeout is configured.
    #[inline]
    pub fn request_timeout_exceeded(&self) -> bool {
        self.request_timeout()
            .is_some_and(|t| Instant::now().saturating_duration_since(self.start_time) >= t)
    }

    /// Remaining wall-clock time before the client-supplied `grpc-timeout`
    /// deadline expires. Returns `None` when no `grpc-timeout` header was
    /// parsed, and `Some(Duration::ZERO)` when the deadline has already
    /// passed.
    #[inline]
    pub fn grpc_remaining(&self) -> Option<Duration> {
        self.request_info
            .grpc_client_deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }

    /// True when a `grpc-timeout` deadline was parsed for this request and
    /// `Instant::now()` is already past it.
    #[inline]
    pub fn grpc_deadline_exceeded(&self) -> bool {
        self.grpc_remaining().is_some_and(|r| r.is_zero())
    }

    /// Resolve the retry `attempts` cap with field-level fallback between
    /// HTTP and gRPC route units.
    #[inline]
    pub fn retry_attempts(&self) -> Option<u32> {
        self.route_unit
            .as_ref()
            .and_then(|u| u.rule.parsed_retry.as_ref())
            .and_then(|r| r.attempts)
            .or_else(|| {
                self.grpc_route_unit
                    .as_ref()
                    .and_then(|u| u.rule.parsed_retry.as_ref())
                    .and_then(|r| r.attempts)
            })
    }

    /// Resolve the retry `backoff` base duration with field-level fallback
    /// between HTTP and gRPC route units.
    #[inline]
    pub fn retry_backoff(&self) -> Option<Duration> {
        self.route_unit
            .as_ref()
            .and_then(|u| u.rule.parsed_retry.as_ref())
            .and_then(|r| r.backoff)
            .or_else(|| {
                self.grpc_route_unit
                    .as_ref()
                    .and_then(|u| u.rule.parsed_retry.as_ref())
                    .and_then(|r| r.backoff)
            })
    }

    /// Resolve the effective `sessionPersistence` config with field-level
    /// fallback between HTTP and gRPC route units (mirrors `retry_attempts`).
    /// Only one route unit is set per request; the order pins the tie-break
    /// for consistency with the retry helpers.
    #[inline]
    pub fn effective_session_persistence(&self) -> Option<&SessionPersistence> {
        pick_effective_sp(
            self.route_unit
                .as_ref()
                .and_then(|u| u.rule.session_persistence.as_ref()),
            self.grpc_route_unit
                .as_ref()
                .and_then(|u| u.rule.session_persistence.as_ref()),
        )
    }

    /// True when the effective `sessionPersistence` declares `strict: true`.
    #[inline]
    pub fn effective_sp_strict(&self) -> bool {
        self.effective_session_persistence()
            .and_then(|sp| sp.strict)
            .unwrap_or(false)
    }

    /// True when the matched route opted into retrying non-idempotent methods
    /// on the reused-connection retry path via the
    /// `edgion.io/retry-non-idempotent: "true"` annotation. Defaults to `false`
    /// (the safe default: a reused keepalive reset is not retried for
    /// non-idempotent methods, to avoid a silent duplicate mutation).
    #[inline]
    pub fn retry_allow_non_idempotent(&self) -> bool {
        self.route_unit
            .as_ref()
            .and_then(|u| u.rule.parsed_allow_non_idempotent_retry)
            .or_else(|| {
                self.grpc_route_unit
                    .as_ref()
                    .and_then(|u| u.rule.parsed_allow_non_idempotent_retry)
            })
            .unwrap_or(false)
    }

    /// True when the matched rule (HTTP or gRPC route unit) carries a retry
    /// policy — i.e. `rules[].retry` was present with at least one recognized field.
    #[inline]
    pub fn retry_configured(&self) -> bool {
        self.route_unit.as_ref().is_some_and(|u| u.rule.parsed_retry.is_some())
            || self
                .grpc_route_unit
                .as_ref()
                .is_some_and(|u| u.rule.parsed_retry.is_some())
    }

    /// Initialize backend context for the resource-selected path (HTTP/gRPC backend ref).
    /// `source` stays `None` — this is the default path, no bypass tag needed.
    /// Bypass paths (DirectEndpoint / ExternalJump) construct `BackendContext` directly
    /// at the call site (in `try_direct_endpoint_peer` / `try_external_jump_peer`)
    /// and set their own `Some(source)` tag.
    pub fn init_backend_context(
        &mut self,
        rsc: Rsc,
        service_key: String,
        service_kind: ServiceBackendKind,
        backend_ref_idx: Option<usize>,
    ) {
        self.backend_context = Some(BackendContext {
            source: None,
            rsc: Some(rsc),
            service_key: Some(service_key),
            service_kind: Some(service_kind),
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx,
        });
    }

    /// Push a new upstream connection attempt with address info
    pub fn push_upstream(&mut self, ip: Option<String>, port: Option<u16>) {
        if let Some(ref mut bc) = self.backend_context {
            bc.upstreams.push(UpstreamInfo {
                ip,
                port,
                status: None,
                ct: None,
                ht: None,
                bt: None,
                et: None,
                wpt: None,
                response_body_size: None,
                start_time: std::time::Instant::now(),
                err: Vec::new(),
                lb_backend_addr: None,
                lb_edgion_backend: None,
                service_key: None,
                lb_policy: None,
                tls: None,
                lb_committed: None,
            });
            bc.current_upstream_id = Some(bc.upstreams.len() - 1);
        }
    }

    /// Get mutable reference to current upstream
    pub fn get_current_upstream_mut(&mut self) -> Option<&mut UpstreamInfo> {
        self.backend_context
            .as_mut()
            .and_then(|bc| bc.current_upstream_id.and_then(|id| bc.upstreams.get_mut(id)))
    }

    /// Get reference to current upstream
    pub fn get_current_upstream(&self) -> Option<&UpstreamInfo> {
        self.backend_context
            .as_ref()
            .and_then(|bc| bc.current_upstream_id.and_then(|id| bc.upstreams.get(id)))
    }

    // ========== Context variable methods ==========

    /// Get a context variable by key
    pub fn get_ctx_var(&self, key: &str) -> Option<&str> {
        self.ctx_map.get(key).map(|s| s.as_str())
    }

    /// Set a context variable (for plugins like KeySet)
    pub fn set_ctx_var(&mut self, key: String, value: String) {
        self.ctx_map.insert(key, value);
    }

    /// Remove a context variable
    pub fn remove_ctx_var(&mut self, key: &str) -> Option<String> {
        self.ctx_map.remove(key)
    }

    // ===== URI/path canonical accessors =====
    // See docs/superpowers/specs/2026-04-26-uri-normalization-design.md §3.2

    /// Current normalized path. Reflects post-rewrite state (cache refreshed by
    /// session_adapter::set_upstream_uri after each rewrite). Default view for
    /// routing matchers, ACL plugins, access_log.
    pub fn path(&self) -> &str {
        &self.request_info.normalized_current_path
    }

    /// Entry-time raw path. Frozen for the request lifetime.
    /// Use for audit, security investigation, special raw-form deny rules.
    pub fn request_path(&self) -> &str {
        &self.request_info.entry_raw_path
    }

    /// Current query string (passthrough; never normalized).
    pub fn query<'a>(&self, req_header: &'a pingora_http::RequestHeader) -> Option<&'a str> {
        req_header.uri.query()
    }

    /// Current normalized full URI (current path + current query).
    pub fn uri(&self, req_header: &pingora_http::RequestHeader) -> String {
        compose_uri(self.path(), self.query(req_header))
    }

    /// Entry-time raw full URI (entry raw path + entry raw query). Frozen.
    pub fn request_uri(&self) -> String {
        compose_uri(
            &self.request_info.entry_raw_path,
            self.request_info.entry_raw_query.as_deref(),
        )
    }

    /// Refresh `normalized_current_path` from current `req_header.uri.path()`.
    /// Called by session_adapter::set_upstream_uri after rewrite, or by entry filter.
    /// Business code should NOT call this directly.
    pub fn refresh_normalized_path(
        &mut self,
        req_header: &pingora_http::RequestHeader,
        cfg: &edgion_resources::path_normalization::PathNormalizationConfig,
    ) {
        let raw = req_header.uri.path();
        self.request_info.normalized_current_path = match edgion_resources::path_normalization::normalize_path(raw, cfg)
        {
            Some(s) => s,
            None => raw.to_string(),
        };
    }

    /// First-write-wins setter for the unified upstream-jump slot. Returns
    /// `Set` on the first successful write, `Rejected` on every subsequent
    /// call. On `Direct` success, co-writes `selected_backend` so the
    /// peer-construction path can derive service metadata; on `Rejected`,
    /// `selected_backend` is NOT touched (see Risk #1 in the spec).
    pub fn try_set_jump(&mut self, target: UpstreamJumpTarget) -> SetJumpOutcome {
        if self.jump.is_some() {
            return SetJumpOutcome::Rejected;
        }
        if let UpstreamJumpTarget::Direct { preset, backend_ref } = &target {
            // Plugin-injected direct endpoint: no backend-level plugin chain
            // (matches today, where the synthetic backend's runtime is empty).
            let route_namespace = backend_ref.namespace.as_deref().unwrap_or_default();
            self.selected_backend = Some(SelectedHttpBackend::Direct(
                crate::routes::http::match_unit::CompiledBackend::compile(
                    backend_ref.as_ref().clone(),
                    route_namespace,
                    preset.backend_ref_idx,
                    Arc::new(PluginRuntime::new()),
                ),
            ));
        }
        self.jump = Some(target);
        SetJumpOutcome::Set
    }
}

/// Pure precedence step for `effective_session_persistence`: the HTTP rule's
/// config wins over the gRPC rule's. Split out so the precedence is
/// unit-testable without constructing route units.
#[inline]
pub(crate) fn pick_effective_sp<'a>(
    http_sp: Option<&'a SessionPersistence>,
    grpc_sp: Option<&'a SessionPersistence>,
) -> Option<&'a SessionPersistence> {
    http_sp.or(grpc_sp)
}

fn compose_uri(path: &str, query: Option<&str>) -> String {
    match query {
        Some(q) if !q.is_empty() => format!("{}?{}", path, q),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod log_label_tests {
    use super::*;
    use edgion_resources::constants::annotations::edgion::LOG_LABEL_PREFIX;
    use kube::api::ObjectMeta;

    fn json_of(labels: &Option<LogLabels>) -> String {
        let rsc = Rsc::new("HTTPRoute", "ns", "name").with_labels(labels.clone());
        serde_json::to_string(&rsc).unwrap()
    }

    // Resolution semantics (prefix strip, precedence, guardrails, cap) are
    // covered by `edgion_resources::resources::common::log_labels`. These tests
    // pin the gateway-side serialization shape and the delegating wrapper.

    #[test]
    fn extract_serializes_sorted_object_with_prefix_stripped() {
        let meta = ObjectMeta {
            annotations: Some(
                [(format!("{LOG_LABEL_PREFIX}tenant"), "internal".to_string())]
                    .into_iter()
                    .collect(),
            ),
            labels: Some(
                [(format!("{LOG_LABEL_PREFIX}env"), "prod".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };
        let out = extract_log_labels(&meta).expect("labels");
        assert_eq!(
            json_of(&Some(out)),
            r#"{"kind":"HTTPRoute","ns":"ns","name":"name","labels":{"env":"prod","tenant":"internal"}}"#
        );
    }

    #[test]
    fn extract_returns_none_without_prefixed_entries() {
        let meta = ObjectMeta {
            annotations: Some([("unrelated".to_string(), "x".to_string())].into_iter().collect()),
            ..Default::default()
        };
        assert!(extract_log_labels(&meta).is_none());
    }

    #[test]
    fn from_resolved_wraps_cached_arc_without_rescanning() {
        let map = Arc::new([("env".to_string(), "prod".to_string())].into_iter().collect());
        let out = LogLabels::from_resolved(Arc::clone(&map));
        assert_eq!(
            json_of(&Some(out)),
            r#"{"kind":"HTTPRoute","ns":"ns","name":"name","labels":{"env":"prod"}}"#
        );
    }

    #[test]
    fn labels_field_omitted_when_absent() {
        assert_eq!(json_of(&None), r#"{"kind":"HTTPRoute","ns":"ns","name":"name"}"#);
    }
}

#[cfg(test)]
mod route_meta_tests {
    use super::*;

    #[test]
    fn route_meta_is_resolved_once_and_shared() {
        use std::collections::BTreeMap;

        let meta = kube::api::ObjectMeta {
            labels: Some(BTreeMap::from([("app".to_string(), "web".to_string())])),
            annotations: Some(BTreeMap::from([("owner".to_string(), "platform".to_string())])),
            ..Default::default()
        };

        let (labels, annotations) = extract_route_meta(&meta);
        assert_eq!(labels.as_ref().unwrap().get("app").unwrap(), "web");
        assert_eq!(annotations.as_ref().unwrap().get("owner").unwrap(), "platform");
    }

    #[test]
    fn route_meta_is_none_when_absent() {
        let (labels, annotations) = extract_route_meta(&kube::api::ObjectMeta::default());
        assert!(labels.is_none());
        assert!(annotations.is_none());
    }

    #[test]
    fn route_meta_is_not_serialized_into_the_log_entry() {
        // The maps exist to answer `from: routeLabel` / `from: routeAnnotation`
        // lookups. They must never leak into `match_info` wholesale — only
        // declared keys reach `ext`. Populate both maps via `with_route_meta`
        // with distinctive values so the assertion actually exercises
        // `#[serde(skip)]` on both `labels_meta` and `annotations_meta`
        // (a prior version of this test left both maps `None` via `MatchInfo::new`
        // alone, which caught removing `#[serde(skip)]` from `annotations_meta` —
        // that field's name contains "annotations" — but not from `labels_meta`,
        // since `"labels_meta":null` doesn't contain "annotations").
        // `HTTPRouteMatch` has no `Default` impl, so build it explicitly with
        // every field empty/None (all optional matchers).
        use std::collections::BTreeMap;

        let match_item = HTTPRouteMatch {
            path: None,
            headers: None,
            query_params: None,
            method: None,
        };
        let labels = Arc::new(BTreeMap::from([(
            "canary-label-marker".to_string(),
            "canary-label-value".to_string(),
        )]));
        let annotations = Arc::new(BTreeMap::from([(
            "canary-annotation-marker".to_string(),
            "canary-annotation-value".to_string(),
        )]));
        let info = MatchInfo::new("default".to_string(), "r".to_string(), 0, 0, match_item, 0)
            .with_route_meta(Some(labels), Some(annotations));
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("labels_meta"), "got: {json}");
        assert!(!json.contains("annotations_meta"), "got: {json}");
        assert!(!json.contains("canary-label-marker"), "got: {json}");
        assert!(!json.contains("canary-label-value"), "got: {json}");
        assert!(!json.contains("canary-annotation-marker"), "got: {json}");
        assert!(!json.contains("canary-annotation-value"), "got: {json}");
    }
}

#[cfg(test)]
mod sensitive_tests {
    use super::*;
    use edgion_resources::sensitive::SENSITIVE_MODE;
    use parking_lot::Mutex;
    use std::sync::atomic::Ordering;

    static MODE_LOCK: Mutex<()> = Mutex::new(());

    struct ModeGuard<'a> {
        prev: u8,
        // Held purely for RAII: serializes tests that mutate SENSITIVE_MODE.
        _lock: parking_lot::MutexGuard<'a, ()>,
    }
    impl Drop for ModeGuard<'_> {
        fn drop(&mut self) {
            SENSITIVE_MODE.store(self.prev, Ordering::SeqCst);
        }
    }
    fn set_mode(mode: u8) -> ModeGuard<'static> {
        let guard = MODE_LOCK.lock();
        let prev = SENSITIVE_MODE.swap(mode, Ordering::SeqCst);
        ModeGuard { prev, _lock: guard }
    }

    #[test]
    fn retry_configured_false_without_route_units() {
        let ctx = EdgionHttpContext::new();
        assert!(!ctx.retry_configured());
        assert!(ctx.peer_timeout_source.is_none());
    }

    #[test]
    fn test_client_cert_info_wrap_mode() {
        let _g = set_mode(2);
        let cert = ClientCertInfo {
            subject: "CN=alice@example.com".to_string(),
            sans: vec!["alice@example.com".to_string()],
            cn: Some("alice@example.com".to_string()),
            fingerprint: "abc123".to_string(),
        };
        let json = serde_json::to_string(&cert).unwrap();
        assert!(
            json.contains("<E>CN=alice@example.com</E>"),
            "subject not wrapped: {}",
            json
        );
        assert!(json.contains("<E>alice@example.com</E>"), "sans not wrapped: {}", json);
        assert!(json.contains("abc123"), "fingerprint should not be wrapped: {}", json);
    }

    #[test]
    fn test_client_cert_info_off_mode() {
        let _g = set_mode(0);
        let cert = ClientCertInfo {
            subject: "CN=alice@example.com".to_string(),
            sans: vec![],
            cn: None,
            fingerprint: "abc123".to_string(),
        };
        let json = serde_json::to_string(&cert).unwrap();
        assert!(
            json.contains("CN=alice@example.com"),
            "subject should be plain in off mode: {}",
            json
        );
    }

    #[test]
    fn test_request_info_wrap_mode() {
        let _g = set_mode(2);
        let ri = RequestInfo {
            client_addr: "127.0.0.1".to_string(),
            remote_addr: "10.0.0.1".to_string(),
            entry_raw_path: "/api/users/alice".to_string(),
            normalized_current_path: "/api/users/alice".to_string(),
            x_forwarded_for: Some("203.0.113.5, 10.0.0.1".to_string()),
            sni: Some("example.com".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&ri).unwrap();
        assert!(json.contains("<E>127.0.0.1</E>"), "client_addr: {}", json);
        assert!(json.contains("<E>10.0.0.1</E>"), "remote_addr: {}", json);
        assert!(json.contains("<E>/api/users/alice</E>"), "path: {}", json);
        assert!(json.contains("<E>203.0.113.5, 10.0.0.1</E>"), "xff: {}", json);
        assert!(json.contains("<E>example.com</E>"), "sni: {}", json);
    }

    #[test]
    fn test_request_info_replace_mode() {
        let _g = set_mode(1);
        let ri = RequestInfo {
            client_addr: "127.0.0.1".to_string(),
            remote_addr: "10.0.0.1".to_string(),
            entry_raw_path: "/api/users/alice".to_string(),
            normalized_current_path: "/api/users/alice".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&ri).unwrap();
        assert!(!json.contains("127.0.0.1"), "client_addr leaked: {}", json);
        assert!(!json.contains("10.0.0.1"), "remote_addr leaked: {}", json);
        assert!(!json.contains("/api/users/alice"), "path leaked: {}", json);
        assert!(json.contains("***"), "replace_with not found: {}", json);
    }

    #[test]
    fn body_declared_defaults_to_none_not_false() {
        // `RequestInfo` derives `Default`. A plain `bool` would default `false`,
        // which the predicate would read as "no body — run the plugin". `None`
        // means "the snapshot never ran" and fails closed instead.
        let info = RequestInfo::default();
        assert_eq!(info.body_declared, None);
    }
}

#[cfg(test)]
mod request_body_cap_tests {
    use super::*;
    use crate::plugins::runtime::traits::session::BodyReadError;
    use crate::request_body::BodyStorage;
    use pingora_core::protocols::http::RequestBodyBuffer;

    async fn context_with_body(body: &'static [u8], cap: u64) -> EdgionHttpContext {
        let (mut storage, view) = BodyStorage::new(128 * 1024, cap);
        storage.write(&Bytes::from_static(body)).await.unwrap();
        storage.finish().await.unwrap();
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = cap;
        ctx.request_body.install(view);
        ctx
    }

    #[tokio::test]
    async fn overwrite_enforces_physical_cap_atomically() {
        let mut exact = context_with_body(b"old", 4).await;
        exact.overwrite_request_body(Bytes::from_static(b"1234")).unwrap();
        assert_eq!(
            exact.request_body.read_all().await.unwrap().unwrap(),
            Bytes::from_static(b"1234")
        );

        let mut over = context_with_body(b"old", 4).await;
        assert_eq!(
            over.overwrite_request_body(Bytes::from_static(b"12345")),
            Err(BodyReadError::BufferLimitExceeded { limit: 4 })
        );
        assert_eq!(
            over.request_body.read_all().await.unwrap().unwrap(),
            Bytes::from_static(b"old"),
            "a rejected mutation must leave the previous version visible"
        );
    }

    #[tokio::test]
    async fn slice_replacement_enforces_physical_cap_atomically() {
        let mut ctx = context_with_body(b"abcd", 5).await;
        assert_eq!(
            ctx.replace_request_body_slice(1..3, Bytes::from_static(b"WXYZ")).await,
            Err(BodyReadError::BufferLimitExceeded { limit: 5 })
        );
        assert_eq!(
            ctx.request_body.read_all().await.unwrap().unwrap(),
            Bytes::from_static(b"abcd")
        );
    }
}

#[cfg(test)]
mod path_view_tests {
    use super::*;
    use pingora_http::RequestHeader;

    fn make_ctx(entry_path: &str, entry_query: Option<&str>) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.entry_raw_path = entry_path.to_string();
        ctx.request_info.entry_raw_query = entry_query.map(str::to_string);
        let path_cfg = edgion_resources::path_normalization::PathNormalizationConfig::default();
        ctx.request_info.normalized_current_path =
            match edgion_resources::path_normalization::normalize_path(entry_path, &path_cfg) {
                Some(s) => s,
                None => entry_path.to_string(),
            };
        ctx
    }

    #[test]
    fn path_returns_normalized_current() {
        let ctx = make_ctx("/%61dmin", None);
        assert_eq!(ctx.path(), "/admin");
    }

    #[test]
    fn request_path_returns_entry_raw() {
        let ctx = make_ctx("/%61dmin", None);
        assert_eq!(ctx.request_path(), "/%61dmin");
    }

    #[test]
    fn request_uri_composes_entry() {
        let ctx = make_ctx("/%61dmin", Some("x=1"));
        assert_eq!(ctx.request_uri(), "/%61dmin?x=1");
    }

    #[test]
    fn refresh_after_rewrite() {
        let mut ctx = make_ctx("/api/old", None);
        let req = RequestHeader::build("GET", b"/api/new", None).unwrap();
        let path_cfg = edgion_resources::path_normalization::PathNormalizationConfig::default();
        ctx.refresh_normalized_path(&req, &path_cfg);
        assert_eq!(ctx.path(), "/api/new");
        assert_eq!(ctx.request_path(), "/api/old");
    }
}

#[cfg(test)]
mod jump_tests {
    use super::*;

    fn fresh_ctx() -> EdgionHttpContext {
        EdgionHttpContext::new()
    }

    fn sample_internal() -> InternalJumpPreset {
        InternalJumpPreset {
            backend_ref_name: "svc-a".to_string(),
            backend_ref_namespace: None,
        }
    }

    fn sample_external() -> ExternalJumpPreset {
        ExternalJumpPreset {
            domain: "api.example.com".to_string(),
            port: 443,
            use_tls: true,
            sni: "api.example.com".to_string(),
            preserve_raw_path: false,
            upstream_read_timeout_override: None,
        }
    }

    fn sample_direct() -> (DirectEndpointPreset, Box<HTTPBackendRef>) {
        let preset = DirectEndpointPreset {
            addr: "10.0.0.1:8080".parse().unwrap(),
            use_tls: false,
            sni: String::new(),
            backend_ref_idx: 0,
        };
        let backend_ref = HTTPBackendRef {
            name: "direct-svc".to_string(),
            namespace: None,
            port: Some(8080),
            weight: Some(1),
            group: None,
            kind: None,
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        };
        (preset, Box::new(backend_ref))
    }

    #[test]
    fn first_internal_write_returns_set() {
        let mut ctx = fresh_ctx();
        let outcome = ctx.try_set_jump(UpstreamJumpTarget::Internal(sample_internal()));
        assert_eq!(outcome, SetJumpOutcome::Set);
        assert!(matches!(ctx.jump, Some(UpstreamJumpTarget::Internal(_))));
        assert!(
            ctx.selected_backend.is_none(),
            "Internal must NOT co-write selected_backend"
        );
    }

    #[test]
    fn first_external_write_returns_set() {
        let mut ctx = fresh_ctx();
        let outcome = ctx.try_set_jump(UpstreamJumpTarget::External(sample_external()));
        assert_eq!(outcome, SetJumpOutcome::Set);
        assert!(matches!(ctx.jump, Some(UpstreamJumpTarget::External(_))));
        assert!(
            ctx.selected_backend.is_none(),
            "External must NOT co-write selected_backend"
        );
    }

    #[test]
    fn first_direct_write_co_writes_selected_backend() {
        let mut ctx = fresh_ctx();
        let (preset, backend_ref) = sample_direct();
        let outcome = ctx.try_set_jump(UpstreamJumpTarget::Direct {
            preset,
            backend_ref: backend_ref.clone(),
        });
        assert_eq!(outcome, SetJumpOutcome::Set);
        assert!(matches!(ctx.jump, Some(UpstreamJumpTarget::Direct { .. })));
        assert_eq!(
            ctx.selected_backend.as_ref().map(|b| b.name.as_str()),
            Some("direct-svc"),
            "Direct success must co-write selected_backend"
        );
    }

    #[test]
    fn second_write_returns_rejected_and_does_not_mutate_ctx() {
        let mut ctx = fresh_ctx();
        let first = ctx.try_set_jump(UpstreamJumpTarget::External(sample_external()));
        assert_eq!(first, SetJumpOutcome::Set);
        let selected_before_name = ctx.selected_backend.as_ref().map(|b| b.name.clone());

        let (preset, backend_ref) = sample_direct();
        let second = ctx.try_set_jump(UpstreamJumpTarget::Direct { preset, backend_ref });
        assert_eq!(second, SetJumpOutcome::Rejected);

        assert!(matches!(ctx.jump, Some(UpstreamJumpTarget::External(_))));
        assert_eq!(
            ctx.selected_backend.as_ref().map(|b| b.name.as_str()),
            selected_before_name.as_deref(),
            "selected_backend must not change on rejected second write"
        );
    }

    #[test]
    fn second_direct_write_after_internal_does_not_leak_backend() {
        let mut ctx = fresh_ctx();
        ctx.try_set_jump(UpstreamJumpTarget::Internal(sample_internal()));
        assert!(ctx.selected_backend.is_none());

        let (preset, backend_ref) = sample_direct();
        let outcome = ctx.try_set_jump(UpstreamJumpTarget::Direct { preset, backend_ref });
        assert_eq!(outcome, SetJumpOutcome::Rejected);
        assert!(
            ctx.selected_backend.is_none(),
            "Rejected Direct must NOT touch selected_backend"
        );
    }
}

#[cfg(test)]
mod cached_query_tests {
    use super::*;

    #[test]
    fn cached_query_initializes_lazily_and_caches() {
        let ctx = EdgionHttpContext::new();
        // First access triggers parse. The space in `hello world` is percent-
        // encoded as `%20`, so the decoded value exercises url-decode.
        let first = ctx.cached_query(Some("name=hello%20world&n=1")) as *const _;
        // Second access with a *different* raw_query must still return the
        // already-cached HashMap — init-once semantics, not "re-parse".
        let second = ctx.cached_query(Some("name=changed&n=2")) as *const _;
        assert_eq!(first, second, "cached_query must be init-once per request");

        // Verify decoded values (route-matcher-compatible).
        let map = ctx.cached_query(Some("ignored=value"));
        assert_eq!(map.get("name"), Some(&"hello world".to_string()));
        assert_eq!(map.get("n"), Some(&"1".to_string()));
    }

    #[test]
    fn cached_query_handles_none_raw() {
        let ctx = EdgionHttpContext::new();
        let map = ctx.cached_query(None);
        assert!(map.is_empty(), "None raw_query yields empty cache");
        // Subsequent call with Some(...) must NOT replace the cached empty map.
        let map2 = ctx.cached_query(Some("a=1"));
        assert!(map2.is_empty(), "OnceLock already initialized to empty");
    }
}

#[cfg(test)]
mod geo_context_tests {
    use super::*;
    use crate::geoip::{GeoIpProvider, GeoLookup, GEO_TEST_LOCK};

    #[test]
    fn geo_slots_start_unresolved_and_resolve_once() {
        // Serialize against the process-wide GeoIpProvider singleton: other
        // modules' tests in this same test binary load the fixture database
        // into it, so without the lock + clear this test would flake
        // depending on run order.
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.client_addr = "203.0.113.7".to_string();
        ctx.request_info.remote_addr = "203.0.113.7".to_string();

        // No database loaded in this test process.
        assert_eq!(*ctx.client_geo(), GeoLookup::Unavailable);
        // Second call returns the cached value, not a second lookup.
        assert_eq!(*ctx.client_geo(), GeoLookup::Unavailable);
    }

    #[test]
    fn equal_addresses_share_one_lookup() {
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.client_addr = "::ffff:203.0.113.7".to_string();
        ctx.request_info.remote_addr = "203.0.113.7".to_string();
        // The mapped and plain forms normalize equal, so the second slot must
        // reuse the first result rather than resolving independently.
        let a = ctx.direct_geo().clone();
        let b = ctx.client_geo().clone();
        assert_eq!(a, b);
    }

    #[test]
    fn unparseable_address_reports_not_found_not_panic() {
        let _g = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.remote_addr = "not-an-ip".to_string();
        assert_eq!(*ctx.client_geo(), GeoLookup::NotFound);
    }
}

#[cfg(test)]
mod local_reply_tests {
    use super::*;

    #[test]
    fn local_reply_defaults_are_inactive() {
        let ctx = EdgionHttpContext::default();
        assert!(ctx.local_reply_header.is_none());
        assert!(ctx.local_reply_body.is_none());
        assert!(!ctx.local_reply_written);
    }
}

#[cfg(test)]
mod json_schema_validation_claim_tests {
    use super::*;

    #[test]
    fn request_context_allows_exactly_one_json_schema_validator() {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.claim_json_schema_validation());
        assert!(!ctx.claim_json_schema_validation());
    }
}

#[cfg(test)]
mod ai_context_claim_tests {
    use std::sync::Arc;

    use super::*;
    use crate::ai::{AiAttemptPlan, AiPlanError, AiProviderLabel, AiRequestContext, AiUsageShared};

    fn context(model: &str) -> AiRequestContext {
        AiRequestContext {
            provider: AiProviderLabel::OpenAi,
            model: model.to_string(),
            pricing: None,
            requested_streaming: false,
            usage: Arc::new(AiUsageShared::default()),
            quota: None,
            quota_status: None,
        }
    }

    #[test]
    fn first_ai_context_writer_wins() {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.try_set_ai_request_context(context("model-a")));
        assert!(!ctx.try_set_ai_request_context(context("model-b")));
        assert_eq!(ctx.ai.as_ref().map(|ai| ai.model.as_str()), Some("model-a"));
    }

    #[test]
    fn first_ai_attempt_plan_writer_wins() {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.try_set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend)));
        assert!(!ctx.try_set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidRouteConfiguration,)));
        assert!(matches!(
            ctx.ai_attempt_plan,
            Some(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend))
        ));
    }
}

#[cfg(test)]
mod sp_config_tests {
    use super::pick_effective_sp;
    use edgion_resources::resources::http_route::SessionPersistence;

    fn sp(name: &str) -> SessionPersistence {
        SessionPersistence {
            session_name: Some(name.to_string()),
            absolute_timeout: None,
            idle_timeout: None,
            persistence_type: None,
            cookie_config: None,
            strict: None,
        }
    }

    #[test]
    fn http_only_uses_http() {
        let http = sp("http-sp");
        let picked = pick_effective_sp(Some(&http), None).expect("http-only must resolve");
        assert_eq!(picked.session_name.as_deref(), Some("http-sp"));
    }

    #[test]
    fn grpc_only_falls_back_to_grpc() {
        let grpc = sp("grpc-sp");
        let picked = pick_effective_sp(None, Some(&grpc)).expect("grpc-only must resolve");
        assert_eq!(picked.session_name.as_deref(), Some("grpc-sp"));
    }

    #[test]
    fn neither_yields_none() {
        assert!(pick_effective_sp(None, None).is_none());
    }

    #[test]
    fn http_wins_over_grpc() {
        // Only one route unit is ever set per request; this pins the tie-break
        // order for consistency with retry_attempts()/retry_backoff().
        let http = sp("http-sp");
        let grpc = sp("grpc-sp");
        let picked = pick_effective_sp(Some(&http), Some(&grpc)).expect("must resolve");
        assert_eq!(picked.session_name.as_deref(), Some("http-sp"));
    }
}
