//! Stable request-diagnostic codes.

/// Normal, noteworthy request handling outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgionInfoCode {
    /// ACME HTTP-01 challenge served locally by the gateway.
    AcmeChallengeServed,

    /// A response-phase plugin replaced the upstream response with a local reply.
    PluginResponseReplaced,
}

impl EdgionInfoCode {
    const fn wire_code(self) -> &'static str {
        match self {
            Self::AcmeChallengeServed => "AcmeChallengeServed",
            Self::PluginResponseReplaced => "PluginResponseReplaced",
        }
    }
}

impl serde::Serialize for EdgionInfoCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.wire_code())
    }
}

/// Error code for Edgion gateway
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgionErrorCode {
    Unknown,

    /// Wasm execution or terminal cleanup failed; guest detail is not exposed.
    WasmExecutionError,

    /// AccessLogEntry self-serialization failed; emitted as a fixed-string JSON
    /// fallback so downstream log parsers still see a parseable record.
    /// Practically unreachable — the entry struct contains only primitives.
    AccessLogSerializeFailed,

    /// An AI provider response could not be translated into the client-visible
    /// OpenAI schema. Provider body detail is intentionally not retained.
    AiResponseTranslationFailed,

    /// Final AI quota reconciliation did not complete. Usage and estimated
    /// cost remain durable in the access log for external replay.
    AiQuotaReconciliationFailed,

    /// The gateway cut an AI streaming response short because the request's
    /// token quota went over limit — a deliberate enforcement action, not an
    /// upstream failure. Names the cause so an operator does not have to
    /// guess "did upstream hang up, or did Edgion end this?" from a bare
    /// truncated body.
    AiQuotaOverLimitTerminated,

    /// The over-limit SSE error event could not be appended before
    /// terminating the stream because an earlier processor had already
    /// exhausted the per-batch response-body emit budget. The stream still
    /// ends (the client sees a bare truncation instead of the diagnostic
    /// event), which is recorded here so the gap is visible outside the body.
    AiQuotaErrorEventDropped,

    /// SSE decoding of an AI response gave up because one unterminated event
    /// exceeded the decoder's byte cap. Parsing stops for the rest of that
    /// stream, so reported token usage falls back to the character estimate.
    /// On its own this costs accounting precision, not correctness — but it
    /// is the precondition for `AiSseDegradedQuotaTerminated`, so it is worth
    /// alerting on even when no stream was cut.
    AiSseDecodeDegraded,
    /// Usage observation could not produce trustworthy response evidence.
    AiUsageObservationUnavailable,

    /// The gateway cut an AI streaming response short because SSE decoding
    /// had degraded while token-quota enforcement was active. A stream whose
    /// framing can no longer be parsed is a stream whose cost can no longer be
    /// bounded, so enforcement fails closed rather than letting the upstream
    /// keep generating billable tokens unmetered. Distinct from
    /// `AiQuotaOverLimitTerminated`: that one means the limit was actually
    /// reached, this one means the limit could no longer be evaluated.
    AiSseDegradedQuotaTerminated,

    /// At least one completed SSE event in an AI response contained invalid
    /// UTF-8 and was dropped from usage parsing. The stream can continue, but
    /// reported usage has lost precision for that event.
    AiSseDecodeMalformed,

    /// The gateway cut an AI streaming response after repeated malformed SSE
    /// events made token-quota enforcement unreliable. This is distinct from
    /// decoder degradation and from a quota that actually went over limit.
    AiSseMalformedQuotaTerminated,

    /// Missing Host header (HTTP 400)
    HostMissing,

    /// X-Forwarded-For header too long (HTTP 400)
    XffHeaderTooLong,

    /// Path contains escaped slash/backslash rejected by path normalization policy (HTTP 400)
    PathEscapedSlashRejected,

    /// Path contains invalid percent-encoding rejected by path normalization policy (HTTP 400)
    PathInvalidPercentEncoding,

    /// Route Not Found
    RouteNotFound,

    /// SNI and Host header mismatch (HTTP 421)
    SniHostMismatch,

    /// The downstream client was gone before the response could be delivered:
    /// either the proxy-ending error itself was a downstream read/write/close
    /// failure (mapped to 499), or writing the final error response failed.
    /// Nginx-convention 499. Combined with multiple `upstreams` entries in the
    /// same access-log record this identifies retry work spent on a caller
    /// that had already disconnected.
    ClientClosed,

    /// Upstream route not matched
    UpstreamNotRouteMatched,

    /// No backend refs found
    UpstreamNotBackendRefs,

    /// Inconsistent weight configuration (some backends have weight, some don't)
    UpstreamInconsistentWeight,

    /// gRPC upstream route not matched
    GrpcUpstreamNotRouteMatched,

    /// EndpointSlice not found for service (Consistent hash store)
    BackendEndpointSliceNotFoundByConsistent,

    /// EndpointSlice not found for service (LeastConnection store)
    BackendEndpointSliceNotFoundByLeastConn,

    /// EndpointSlice not found for service (RoundRobin store)
    BackendEndpointSliceNotFoundByRoundRobin,

    BackendEndpointSliceNotFoundByRoundRobinDefault,

    /// EndpointSlice not found for service (EWMA store)
    BackendEndpointSliceNotFoundByEwma,

    /// A per-backend retry budget (retryConstraint) refused an additional retry.
    RetryBudgetExhausted,

    /// The fail-safe ceiling on transport retries decided inside Pingora
    /// (h2 GOAWAY, RST_STREAM(REFUSED_STREAM), HTTP/1.1-required downgrade,
    /// reused-connection IO errors) refused a further upstream attempt.
    UpstreamTransportRetryCeiling,

    /// Service not found in service store
    BackendServiceNotFound,

    /// Service ClusterIP not configured
    BackendClusterIpNotFound,

    /// Service ExternalName not configured
    BackendExternalNameNotFound,

    /// Port resolution failed
    BackendPortResolutionFailed,

    /// Backend address parsing failed
    BackendAddressParsingFailed,

    /// ServiceImport not yet implemented
    BackendServiceImportNotImplemented,

    /// Localhost backend not allowed for security reasons
    BackendLocalhostNotAllowed,

    /// HTTP/2 required for gRPC
    Http2Required,

    /// gRPC backend refs not found
    GrpcUpstreamNotBackendRefs,

    /// Cross-namespace reference denied (no matching ReferenceGrant)
    RefDenied,

    /// Link-local / cloud-metadata backend rejected (SSRF defense: `169.254.0.0/16`, `fe80::/10`)
    BackendAddrLinkLocalNotAllowed,

    /// DNS resolution failed for ExternalName service hostname
    BackendDnsResolutionFailed,

    /// Backend kind not supported (Gateway API conformance: return 500)
    BackendUnsupportedKind,

    /// Session-persistence sticky target was unhealthy at peer-selection time.
    /// Runtime fell back to normal LB (non-strict) or returned an error (strict mode).
    StickyBackendUnhealthy,

    /// Backend TLS certificate SAN validation failed
    BackendTlsSanValidationFailed,

    /// Backend TLS CA bundle is missing, empty, or unparseable while
    /// `use_system_ca == false`. Fail-closed: the gateway refuses to fall back
    /// to the system trust store when the operator opted out of it.
    BackendTlsCaInvalid,

    /// An applicable BackendTLSPolicy exists but is invalid or the Service
    /// section required to decide applicability cannot be proven. Fail-closed:
    /// the gateway must not construct a plaintext peer.
    BackendTlsPolicyInvalid,

    /// The dynamic `upstreamAuthority` template rendered to an empty label, or
    /// the rendered label failed DNS-label validation. Fail-closed: silently
    /// keeping the static SNI/Host would present the right TLS identity to the
    /// wrong virtual host. See `AuthorityError` in
    /// `edgion-gateway/src/backends/policy/backend_traffic/authority.rs`.
    UpstreamAuthorityRejected,

    /// Plugin returned `ErrTerminateRequest` without committing a response.
    /// Runtime applied a 502 fallback to ensure the client receives a response.
    /// Operators: query the `diagnostics` field for this error code to find plugins violating
    /// the `ErrTerminateRequest` convention; the offending plugin name is the last
    /// entry in the request-stage `stage_logs[].filters[]` array.
    PluginTerminateWithoutResponse,

    /// A Controller-managed upstream credential reached its absolute validity
    /// deadline before final attempt-local header materialization. The Gateway
    /// fails closed even if an older immutable plugin generation is still live.
    CredentialExpired,

    /// A downstream or attempt-local `Connection` field nominated a deferred
    /// credential target, or could not be parsed as UTF-8. The Gateway rejects
    /// the attempt before materializing any credential header.
    CredentialConnectionConflict,

    /// ForwardAuth plugin could not reach the configured auth service
    /// (connection refused, DNS failure, or timeout). Emitted when the
    /// plugin rejects the request (auth is always fail-closed).
    ForwardAuthServiceUnreachable,

    /// End-to-end request timeout already exceeded before peer selection — the
    /// gateway aborts instead of starting a new upstream attempt that would
    /// also fail. Surfaced to the client as 504.
    RequestTimeoutExceeded,

    /// Body buffering refused: the request declared a body but claims the
    /// WebSocket protocol. A refusal is never softened by `on_read_failure`
    /// (design R5), so this is always a hard failure (HTTP 500).
    BodyRefusedWebsocket,

    /// Body buffering refused: gRPC (matched by content-type OR by GRPCRoute).
    /// A streaming gRPC body does not end until the RPC does (HTTP 500).
    BodyRefusedGrpc,

    /// Body buffering refused: the read was issued from the wrong stage — the
    /// request body is already gone (or the response is not bound yet) at the
    /// stage the caller reached from (HTTP 500).
    ///
    /// This is EXPECTED, routine traffic: a body-consuming plugin placed in a
    /// post-request stage emits it on every request by design. Do not alert on
    /// it. The bug signal lives in `BodyRefusedNoSnapshot` instead — the two
    /// were deliberately split so a real code-path defect is not buried under
    /// this variant's volume.
    BodyRefusedStage,

    /// Body read aborted: the stream ended prematurely (HTTP 500).
    BodyReadAborted,

    /// Body read exceeded its time bound. Client-caused, so a client error —
    /// matching nginx `client_body_timeout` and Envoy (HTTP 408).
    BodyReadTimeout,

    /// Captured or streamed request input/output exceeded the physical cap (HTTP 413).
    BodyReadTooLarge,

    /// A streamed request processor cannot inspect request trailers (HTTP 400).
    BodyStreamTrailersUnsupported,

    /// Body buffering is not built on this binary. Fires on every buffered
    /// request until the P0 buffering runtime lands, which is accurate: body
    /// consumers are non-functional today (HTTP 500).
    BodyReadUnimplemented,

    /// Body buffering refused: the pre-plugin `body_declared` snapshot never
    /// ran, so the admission predicate has no trustworthy fact to decide on and
    /// fails closed rather than guessing (HTTP 500).
    ///
    /// Unlike `BodyRefusedStage` this is a BUG, not expected traffic: some code
    /// path reached body-read admission without first calling
    /// `snapshot_body_declared`. It should never appear in the access log —
    /// a non-zero rate is alertable.
    ///
    /// Emitted by the request-side predicate only: the response side does not
    /// consult `body_declared`, so a missing snapshot there surfaces as an
    /// ordinary buffered read rather than this status.
    BodyRefusedNoSnapshot,

    /// Replaying the captured request body to the upstream failed: a
    /// gateway-local fault (spill-file read error, a replay-cursor invariant
    /// violation, or a capture poisoned mid-chunk), never an upstream or
    /// client failure. Without this code the access log shows only the final
    /// 500 with `upstream.err: ["InternalError"]`, which is not actionable;
    /// the matching error-log line (join on `x-trace-id`) carries the precise
    /// cause text. A non-zero rate is alertable: captured bodies are failing
    /// to reach the upstream (HTTP 500).
    BodyReplayFailed,

    /// The request-body capture storage backend failed mid-capture (spill
    /// file create/write, or the capture was poisoned): an infrastructure
    /// fault such as a full disk, not a client abort. Deliberately split from
    /// `BodyReadAborted` (client stream ended prematurely) so storage
    /// incidents stay alertable instead of drowning in client-abort noise
    /// (HTTP 500).
    BodyCaptureStorageFailed,

    /// Process-wide request-body storage admission refused growth or capture
    /// ownership. The gateway fails closed with HTTP 503 under local overload.
    BodyCaptureCapacityExceeded,

    /// A request-body mutation was computed against one logical snapshot but a
    /// different mutation was published before it could be applied. Always a
    /// gateway-side configuration or ordering fault (two plugins mutating the
    /// same body in one request), never client input, so it fails closed
    /// rather than splicing stale byte offsets into a newer body (HTTP 500).
    BodyMutationConflict,

    /// Client-supplied `grpc-timeout` deadline already exceeded before peer
    /// selection. Surfaced to the gRPC client as `grpc-status: 4` (DEADLINE_EXCEEDED)
    /// via the standard 504 → grpc-status mapping.
    GrpcClientDeadlineExceeded,

    /// An upstream attempt was canceled because its read/write timeout fired
    /// with `timeouts.backendRequest` as the operative bound while a retry
    /// policy was configured — the per-try timeout semantics of GEP-1731.
    /// Recorded per canceled-and-retried attempt (the final exhausted
    /// attempt's timeout is not tagged); informational if a later attempt
    /// succeeds, otherwise the request surfaces as 504.
    BackendRequestTimeoutRetry,

    /// A request-phase plugin returned an `ErrResponse` on a gRPC route whose
    /// body is not valid UTF-8, so it could not be carried as `grpc-message`.
    ///
    /// The denial is still enforced with the mapped gRPC status; what is LOST
    /// is the plugin's body, replaced by the status-mapped default message.
    /// `grpc-message` is a header value and cannot hold arbitrary bytes, so
    /// emitting the body verbatim is not an option and a lossy decode would put
    /// U+FFFD sequences in front of the client. A plugin that needs a specific
    /// message should return `ErrGrpcError`, or keep its `ErrResponse` body
    /// UTF-8. Reachable only from a producer that can emit raw bytes (today,
    /// a Wasm guest's `proxy_send_local_response`).
    PluginGrpcMessageBodyNotUtf8,

    /// A response-phase plugin returned `ErrResponse` on a gRPC route, where a
    /// response replacement cannot be expressed: a gRPC error is HTTP 200 plus
    /// trailers, not a status-and-body response.
    ///
    /// The denial is still enforced — it is always accompanied by
    /// [`Self::PluginResponseDenyAborted`], and the exchange ends as
    /// trailers-only with a gRPC code derived from the plugin's HTTP status via
    /// `http_to_grpc_status`. What this marker records is what was LOST in that
    /// translation: the plugin's response body is dropped, and its exact status
    /// survives only as far as that mapping allows. A plugin that needs a
    /// specific gRPC code should return `ErrGrpcError` instead, which is carried
    /// verbatim and does not raise this marker.
    PluginResponseReplaceUnsupportedGrpc,

    /// A response-phase plugin returned `ErrResponse`, but the exchange was no
    /// longer a replaceable HTTP response (a real `101`, an established CONNECT
    /// tunnel, or a gRPC route), so the denial was enforced by aborting the
    /// exchange with the plugin's status instead of stashing a local reply. The
    /// upstream response is never forwarded. The plugin chooses the HTTP status.
    PluginResponseDenyAborted,

    /// Outbound TLS skip-verify was requested, but the target provider does not
    /// expose a skip-verify knob (e.g. etcd-client). Fail-closed rather than
    /// silently ignoring the operator's intent.
    OutboundTlsVerifyUnsupported,

    /// The CA Secret referenced by `caCertificateRefs` has not been resolved by the
    /// Controller yet, or the resolved Secret is missing the `ca.crt` key.
    OutboundTlsCaMissing,

    /// The client-identity Secret referenced by `clientCertificateRef` is missing
    /// `tls.crt` or `tls.key`.
    OutboundTlsClientCertInvalid,

    /// Outbound peer certificate SANs did not match configured subjectAltNames.
    OutboundTlsSanMismatch,

    /// An auth plugin could not write or clear an identity header, or remove a
    /// hidden credential; the request was denied. Usually operator
    /// misconfiguration. This is an access-log diagnostic, not a metric, and a
    /// single occurrence is not an alert condition: a client can trigger it by
    /// sending a hidden query-parameter credential to a gRPC-JSON-transcoded
    /// route, whose query is sealed against removal. Alert on its rate through
    /// log-based alerting.
    AuthHeaderMutationRejected,
}

impl EdgionErrorCode {
    const fn wire_code(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::WasmExecutionError => "WasmExecutionError",
            Self::AccessLogSerializeFailed => "AccessLogSerializeFailed",
            Self::AiResponseTranslationFailed => "AiResponseTranslationFailed",
            Self::AiQuotaReconciliationFailed => "AiQuotaReconciliationFailed",
            Self::AiQuotaOverLimitTerminated => "AiQuotaOverLimitTerminated",
            Self::AiQuotaErrorEventDropped => "AiQuotaErrorEventDropped",
            Self::AiSseDecodeDegraded => "AiSseDecodeDegraded",
            Self::AiUsageObservationUnavailable => "AiUsageObservationUnavailable",
            Self::AiSseDegradedQuotaTerminated => "AiSseDegradedQuotaTerminated",
            Self::AiSseDecodeMalformed => "AiSseDecodeMalformed",
            Self::AiSseMalformedQuotaTerminated => "AiSseMalformedQuotaTerminated",
            Self::HostMissing => "HostMissing",
            Self::XffHeaderTooLong => "XffHeaderTooLong",
            Self::PathEscapedSlashRejected => "PathEscapedSlashRejected",
            Self::PathInvalidPercentEncoding => "PathInvalidPercentEncoding",
            Self::RouteNotFound => "RouteNotFound",
            Self::SniHostMismatch => "SniHostMismatch",
            Self::ClientClosed => "ClientClosed",
            Self::UpstreamNotRouteMatched => "UpstreamNotRouteMatched",
            Self::UpstreamNotBackendRefs => "UpstreamNotBackendRefs",
            Self::UpstreamInconsistentWeight => "UpstreamInconsistentWeight",
            Self::GrpcUpstreamNotRouteMatched => "GrpcUpstreamNotRouteMatched",
            Self::BackendEndpointSliceNotFoundByConsistent => "BackendEndpointSliceNotFoundByConsistent",
            Self::BackendEndpointSliceNotFoundByLeastConn => "BackendEndpointSliceNotFoundByLeastConn",
            Self::BackendEndpointSliceNotFoundByRoundRobin => "BackendEndpointSliceNotFoundByRoundRobin",
            Self::BackendEndpointSliceNotFoundByRoundRobinDefault => "BackendEndpointSliceNotFoundByRoundRobinDefault",
            Self::BackendEndpointSliceNotFoundByEwma => "BackendEndpointSliceNotFoundByEwma",
            Self::RetryBudgetExhausted => "RetryBudgetExhausted",
            Self::UpstreamTransportRetryCeiling => "UpstreamTransportRetryCeiling",
            Self::BackendServiceNotFound => "BackendServiceNotFound",
            Self::BackendClusterIpNotFound => "BackendClusterIpNotFound",
            Self::BackendExternalNameNotFound => "BackendExternalNameNotFound",
            Self::BackendPortResolutionFailed => "BackendPortResolutionFailed",
            Self::BackendAddressParsingFailed => "BackendAddressParsingFailed",
            Self::BackendServiceImportNotImplemented => "BackendServiceImportNotImplemented",
            Self::BackendLocalhostNotAllowed => "BackendLocalhostNotAllowed",
            Self::Http2Required => "Http2Required",
            Self::GrpcUpstreamNotBackendRefs => "GrpcUpstreamNotBackendRefs",
            Self::RefDenied => "RefDenied",
            Self::BackendAddrLinkLocalNotAllowed => "BackendAddrLinkLocalNotAllowed",
            Self::BackendDnsResolutionFailed => "BackendDnsResolutionFailed",
            Self::BackendUnsupportedKind => "BackendUnsupportedKind",
            Self::StickyBackendUnhealthy => "StickyBackendUnhealthy",
            Self::BackendTlsSanValidationFailed => "BackendTlsSanValidationFailed",
            Self::BackendTlsCaInvalid => "BackendTlsCaInvalid",
            Self::BackendTlsPolicyInvalid => "BackendTlsPolicyInvalid",
            Self::UpstreamAuthorityRejected => "UpstreamAuthorityRejected",
            Self::PluginTerminateWithoutResponse => "PluginTerminateWithoutResponse",
            Self::CredentialExpired => "CredentialExpired",
            Self::CredentialConnectionConflict => "CredentialConnectionConflict",
            Self::ForwardAuthServiceUnreachable => "ForwardAuthServiceUnreachable",
            Self::RequestTimeoutExceeded => "RequestTimeoutExceeded",
            Self::BodyRefusedWebsocket => "BodyRefusedWebsocket",
            Self::BodyRefusedGrpc => "BodyRefusedGrpc",
            Self::BodyRefusedStage => "BodyRefusedStage",
            Self::BodyReadAborted => "BodyReadAborted",
            Self::BodyReadTimeout => "BodyReadTimeout",
            Self::BodyReadTooLarge => "BodyReadTooLarge",
            Self::BodyStreamTrailersUnsupported => "BodyStreamTrailersUnsupported",
            Self::BodyReadUnimplemented => "BodyReadUnimplemented",
            Self::BodyRefusedNoSnapshot => "BodyRefusedNoSnapshot",
            Self::BodyReplayFailed => "BodyReplayFailed",
            Self::BodyCaptureStorageFailed => "BodyCaptureStorageFailed",
            Self::BodyCaptureCapacityExceeded => "BodyCaptureCapacityExceeded",
            Self::BodyMutationConflict => "BodyMutationConflict",
            Self::GrpcClientDeadlineExceeded => "GrpcClientDeadlineExceeded",
            Self::BackendRequestTimeoutRetry => "BackendRequestTimeoutRetry",
            Self::PluginGrpcMessageBodyNotUtf8 => "PluginGrpcMessageBodyNotUtf8",
            Self::PluginResponseReplaceUnsupportedGrpc => "PluginResponseReplaceUnsupportedGrpc",
            Self::PluginResponseDenyAborted => "PluginResponseDenyAborted",
            Self::OutboundTlsVerifyUnsupported => "OutboundTlsVerifyUnsupported",
            Self::OutboundTlsCaMissing => "OutboundTlsCaMissing",
            Self::OutboundTlsClientCertInvalid => "OutboundTlsClientCertInvalid",
            Self::OutboundTlsSanMismatch => "OutboundTlsSanMismatch",
            Self::AuthHeaderMutationRejected => "AuthHeaderMutationRejected",
        }
    }
}

impl serde::Serialize for EdgionErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.wire_code())
    }
}

#[cfg(test)]
#[path = "../tests/unit/diagnostic_test.rs"]
mod tests;
