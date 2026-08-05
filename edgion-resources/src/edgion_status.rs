//! Edgion status/error codes

#![allow(clippy::inconsistent_digit_grouping)]

/// Error code for Edgion gateway
/// Each error code has a fixed numeric code (0Xxx format) and a message string
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum EdgionStatus {
    Unknown = 999,

    UnknownGRPCERR = 999_01,

    /// AccessLogEntry self-serialization failed; emitted as a fixed-string JSON
    /// fallback so downstream log parsers still see a parseable record.
    /// Practically unreachable — the entry struct contains only primitives.
    AccessLogSerializeFailed = 999_02,

    /// An AI provider response could not be translated into the client-visible
    /// OpenAI schema. Provider body detail is intentionally not retained.
    AiResponseTranslationFailed = 999_06,

    /// Final AI quota reconciliation did not complete. Usage and estimated
    /// cost remain durable in the access log for external replay.
    AiQuotaReconciliationFailed = 999_11,

    /// The gateway cut an AI streaming response short because the request's
    /// token quota went over limit — a deliberate enforcement action, not an
    /// upstream failure. Names the cause so an operator does not have to
    /// guess "did upstream hang up, or did Edgion end this?" from a bare
    /// truncated body.
    AiQuotaOverLimitTerminated = 999_12,

    /// The over-limit SSE error event could not be appended before
    /// terminating the stream because an earlier processor had already
    /// exhausted the per-batch response-body emit budget. The stream still
    /// ends (the client sees a bare truncation instead of the diagnostic
    /// event), which is recorded here so the gap is visible outside the body.
    AiQuotaErrorEventDropped = 999_13,

    /// ACME HTTP-01 challenge served locally by the gateway (HTTP 200)
    AcmeChallengeServed = 200_01,

    /// Missing Host header (HTTP 400)
    HostMissing = 400_01,

    /// X-Forwarded-For header too long (HTTP 400)
    XffHeaderTooLong = 400_02,

    /// Path contains escaped slash/backslash rejected by path normalization policy (HTTP 400)
    PathEscapedSlashRejected = 400_03,

    /// Path contains invalid percent-encoding rejected by path normalization policy (HTTP 400)
    PathInvalidPercentEncoding = 400_04,

    /// HTTP/1.1 request contains more than one Host header field line (HTTP 400).
    /// RFC 9110 §7.2 violation; rejected when `SecurityProtectConfig.reject_duplicate_host`
    /// is true (default). Host smuggling defense.
    DuplicateHostHeader = 400_05,

    /// Route Not Found
    RouteNotFound = 404_01,

    /// SNI and Host header mismatch (HTTP 421)
    SniHostMismatch = 421_01,

    /// Client certificate validation failed (HTTP 403)
    ClientCertInvalid = 403_01,

    /// The downstream client was gone before the response could be delivered:
    /// either the proxy-ending error itself was a downstream read/write/close
    /// failure (mapped to 499), or writing the final error response failed.
    /// Nginx-convention 499. Combined with multiple `upstreams` entries in the
    /// same access-log record this identifies retry work spent on a caller
    /// that had already disconnected.
    ClientClosed = 499_01,

    /// Upstream route not matched
    UpstreamNotRouteMatched = 500_01,

    /// No backend refs found
    UpstreamNotBackendRefs = 500_02,

    /// Inconsistent weight configuration (some backends have weight, some don't)
    UpstreamInconsistentWeight = 500_03,

    /// gRPC upstream route not matched
    GrpcUpstreamNotRouteMatched = 500_04,

    /// EndpointSlice not found for service (Consistent hash store)
    BackendEndpointSliceNotFoundByConsistent = 500_05,

    /// EndpointSlice not found for service (LeastConnection store)
    BackendEndpointSliceNotFoundByLeastConn = 500_06,

    /// EndpointSlice not found for service (RoundRobin store)
    BackendEndpointSliceNotFoundByRoundRobin = 500_07,

    BackendEndpointSliceNotFoundByRoundRobinDefault = 500_08,

    /// EndpointSlice not found for service (EWMA store)
    BackendEndpointSliceNotFoundByEwma = 500_09,

    /// A per-backend retry budget (retryConstraint) refused an additional retry.
    RetryBudgetExhausted = 503_01,

    /// Service not found in service store
    BackendServiceNotFound = 503_02,

    /// Service ClusterIP not configured
    BackendClusterIpNotFound = 503_03,

    /// Service ExternalName not configured
    BackendExternalNameNotFound = 503_04,

    /// Port resolution failed
    BackendPortResolutionFailed = 503_05,

    /// Backend address parsing failed
    BackendAddressParsingFailed = 503_06,

    /// LoadBalancer backend selection failed
    BackendLoadBalancerSelectionFailed = 503_07,

    /// ServiceImport not yet implemented
    BackendServiceImportNotImplemented = 503_08,

    /// Localhost backend not allowed for security reasons
    BackendLocalhostNotAllowed = 503_09,

    /// HTTP/2 required for gRPC
    Http2Required = 503_10,

    /// gRPC backend refs not found
    GrpcUpstreamNotBackendRefs = 503_11,

    /// Cross-namespace reference denied (no matching ReferenceGrant)
    RefDenied = 503_12,

    /// Link-local / cloud-metadata backend rejected (SSRF defense: `169.254.0.0/16`, `fe80::/10`)
    BackendAddrLinkLocalNotAllowed = 503_13,

    /// DNS resolution failed for ExternalName service hostname
    BackendDnsResolutionFailed = 503_15,

    /// Backend kind not supported (Gateway API conformance: return 500)
    BackendUnsupportedKind = 500_15,

    /// Session-persistence sticky target was unhealthy at peer-selection time.
    /// Runtime fell back to normal LB (non-strict) or returned an error (strict mode).
    StickyBackendUnhealthy = 50016,

    /// Backend TLS certificate SAN validation failed
    BackendTlsSanValidationFailed = 502_01,

    /// Backend TLS CA bundle is missing, empty, or unparseable while
    /// `use_system_ca == false`. Fail-closed: the gateway refuses to fall back
    /// to the system trust store when the operator opted out of it.
    BackendTlsCaInvalid = 502_03,

    /// The dynamic `upstreamAuthority` template rendered to an empty label, or
    /// the rendered label failed DNS-label validation. Fail-closed: silently
    /// keeping the static SNI/Host would present the right TLS identity to the
    /// wrong virtual host. See `AuthorityError` in
    /// `edgion-gateway/src/backends/policy/backend_traffic/authority.rs`.
    UpstreamAuthorityRejected = 502_04,

    /// Plugin returned `ErrTerminateRequest` without committing a response.
    /// Runtime applied a 502 fallback to ensure the client receives a response.
    /// Operators: grep the `errors` field in access logs to find plugins violating
    /// the `ErrTerminateRequest` convention; the offending plugin name is the last
    /// entry in the request-stage `stage_logs[].filters[]` array.
    PluginTerminateWithoutResponse = 502_02,

    /// ForwardAuth plugin could not reach the configured auth service
    /// (connection refused, DNS failure, or timeout). Emitted when the
    /// plugin rejects the request (auth is always fail-closed).
    ForwardAuthServiceUnreachable = 503_14,

    /// End-to-end request timeout already exceeded before peer selection — the
    /// gateway aborts instead of starting a new upstream attempt that would
    /// also fail. Surfaced to the client as 504.
    RequestTimeoutExceeded = 504_01,

    /// Body buffering refused: the request declared a body but claims the
    /// WebSocket protocol. A refusal is never softened by `on_read_failure`
    /// (design R5), so this is always a hard failure (HTTP 500).
    ///
    /// This group starts at `500_17`, not `500_16`: `StickyBackendUnhealthy`
    /// above is spelled `50016` without the digit separator, so `500_16` is
    /// already taken and reusing it is an E0081 duplicate-discriminant error.
    /// When adding a variant here, grep for both spellings of the value.
    BodyRefusedWebsocket = 500_17,

    /// Body buffering refused: gRPC (matched by content-type OR by GRPCRoute).
    /// A streaming gRPC body does not end until the RPC does (HTTP 500).
    BodyRefusedGrpc = 500_18,

    /// Body buffering refused: the read was issued from the wrong stage — the
    /// request body is already gone (or the response is not bound yet) at the
    /// stage the caller reached from (HTTP 500).
    ///
    /// This is EXPECTED, routine traffic: a body-consuming plugin placed in a
    /// post-request stage emits it on every request by design. Do not alert on
    /// it. The bug signal lives in `BodyRefusedNoSnapshot` instead — the two
    /// were deliberately split so a real code-path defect is not buried under
    /// this variant's volume.
    BodyRefusedStage = 500_19,

    /// Body read aborted: the stream ended prematurely (HTTP 500).
    BodyReadAborted = 500_20,

    /// Body read exceeded its time bound. Client-caused, so a client error —
    /// matching nginx `client_body_timeout` and Envoy (HTTP 408).
    BodyReadTimeout = 408_01,

    /// Body exceeded the configured buffer cap (HTTP 413).
    BodyReadTooLarge = 413_01,

    /// Body buffering is not built on this binary. Fires on every buffered
    /// request until the P0 buffering runtime lands, which is accurate: body
    /// consumers are non-functional today (HTTP 500).
    BodyReadUnimplemented = 500_21,

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
    BodyRefusedNoSnapshot = 500_22,

    /// Replaying the captured request body to the upstream failed: a
    /// gateway-local fault (spill-file read error, a replay-cursor invariant
    /// violation, or a capture poisoned mid-chunk), never an upstream or
    /// client failure. Without this code the access log shows only the final
    /// 500 with `upstream.err: ["InternalError"]`, which is not actionable;
    /// the matching error-log line (join on `x-trace-id`) carries the precise
    /// cause text. A non-zero rate is alertable: captured bodies are failing
    /// to reach the upstream (HTTP 500).
    BodyReplayFailed = 500_23,

    /// The request-body capture storage backend failed mid-capture (spill
    /// file create/write, or the capture was poisoned): an infrastructure
    /// fault such as a full disk, not a client abort. Deliberately split from
    /// `BodyReadAborted` (client stream ended prematurely) so storage
    /// incidents stay alertable instead of drowning in client-abort noise
    /// (HTTP 500).
    BodyCaptureStorageFailed = 500_24,

    /// Client-supplied `grpc-timeout` deadline already exceeded before peer
    /// selection. Surfaced to the gRPC client as `grpc-status: 4` (DEADLINE_EXCEEDED)
    /// via the standard 504 → grpc-status mapping.
    GrpcClientDeadlineExceeded = 504_02,

    /// An upstream attempt was canceled because its read/write timeout fired
    /// with `timeouts.backendRequest` as the operative bound while a retry
    /// policy was configured — the per-try timeout semantics of GEP-1731.
    /// Recorded per canceled-and-retried attempt (the final exhausted
    /// attempt's timeout is not tagged); informational if a later attempt
    /// succeeds, otherwise the request surfaces as 504.
    BackendRequestTimeoutRetry = 504_03,

    /// A response-phase plugin replaced the upstream response with a local reply
    /// (status + headers + body). Informational: this is the normal, intended
    /// outcome of the `sendLocalReply` equivalent. No fixed HTTP status (the
    /// replacement status is whatever the plugin chose), so `999` family.
    PluginResponseReplaced = 999_03,

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
    PluginResponseReplaceUnsupportedGrpc = 999_04,

    /// A response-phase plugin returned `ErrResponse`, but the exchange was no
    /// longer a replaceable HTTP response (a real `101`, an established CONNECT
    /// tunnel, or a gRPC route), so the denial was enforced by aborting the
    /// exchange with the plugin's status instead of stashing a local reply. The
    /// upstream response is never forwarded. No fixed HTTP status (the plugin
    /// chose it), so `999` family.
    PluginResponseDenyAborted = 999_10,

    /// Outbound TLS skip-verify was requested, but the target provider does not
    /// expose a skip-verify knob (e.g. etcd-client). Fail-closed rather than
    /// silently ignoring the operator's intent.
    OutboundTlsVerifyUnsupported = 999_05,

    /// The CA Secret referenced by `caCertificateRefs` has not been resolved by the
    /// Controller yet, or the resolved Secret is missing the `ca.crt` key.
    OutboundTlsCaMissing = 999_07,

    /// The client-identity Secret referenced by `clientCertificateRef` is missing
    /// `tls.crt` or `tls.key`.
    OutboundTlsClientCertInvalid = 999_08,

    /// Outbound peer certificate SANs did not match configured subjectAltNames.
    OutboundTlsSanMismatch = 999_09,
}

impl EdgionStatus {
    /// HTTP status implied by the discriminant, if any.
    ///
    /// Convention: discriminant is encoded as `<http>_<sub>` where `<http>` is
    /// the 3-digit HTTP status (e.g. `400_01` → 400). The `999` family is a
    /// catch-all for codes with no HTTP semantics and returns `None`.
    #[inline]
    pub fn http_status(self) -> Option<u16> {
        let code = self as u32;
        let prefix = if code < 1_000 { code } else { code / 100 };
        if (200..=599).contains(&prefix) {
            Some(prefix as u16)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exhaustive map from variant to its expected HTTP status. The match is
    /// exhaustive on purpose: adding a new variant fails to compile until an
    /// explicit decision is recorded here, which guards the discriminant
    /// encoding convention against silent drift.
    fn expected_http(s: EdgionStatus) -> Option<u16> {
        match s {
            EdgionStatus::Unknown => None,
            EdgionStatus::UnknownGRPCERR => None,
            EdgionStatus::AccessLogSerializeFailed => None,
            EdgionStatus::AiResponseTranslationFailed => None,
            EdgionStatus::AiQuotaReconciliationFailed => None,
            EdgionStatus::AiQuotaOverLimitTerminated => None,
            EdgionStatus::AiQuotaErrorEventDropped => None,
            EdgionStatus::AcmeChallengeServed => Some(200),
            EdgionStatus::HostMissing => Some(400),
            EdgionStatus::XffHeaderTooLong => Some(400),
            EdgionStatus::PathEscapedSlashRejected => Some(400),
            EdgionStatus::PathInvalidPercentEncoding => Some(400),
            EdgionStatus::DuplicateHostHeader => Some(400),
            EdgionStatus::RouteNotFound => Some(404),
            EdgionStatus::SniHostMismatch => Some(421),
            EdgionStatus::ClientCertInvalid => Some(403),
            EdgionStatus::ClientClosed => Some(499),
            EdgionStatus::UpstreamNotRouteMatched => Some(500),
            EdgionStatus::UpstreamNotBackendRefs => Some(500),
            EdgionStatus::UpstreamInconsistentWeight => Some(500),
            EdgionStatus::GrpcUpstreamNotRouteMatched => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByConsistent => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByLeastConn => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByEwma => Some(500),
            EdgionStatus::RetryBudgetExhausted => Some(503),
            EdgionStatus::BackendServiceNotFound => Some(503),
            EdgionStatus::BackendClusterIpNotFound => Some(503),
            EdgionStatus::BackendExternalNameNotFound => Some(503),
            EdgionStatus::BackendPortResolutionFailed => Some(503),
            EdgionStatus::BackendAddressParsingFailed => Some(503),
            EdgionStatus::BackendLoadBalancerSelectionFailed => Some(503),
            EdgionStatus::BackendServiceImportNotImplemented => Some(503),
            EdgionStatus::BackendLocalhostNotAllowed => Some(503),
            EdgionStatus::Http2Required => Some(503),
            EdgionStatus::GrpcUpstreamNotBackendRefs => Some(503),
            EdgionStatus::RefDenied => Some(503),
            EdgionStatus::BackendAddrLinkLocalNotAllowed => Some(503),
            EdgionStatus::BackendDnsResolutionFailed => Some(503),
            EdgionStatus::BackendUnsupportedKind => Some(500),
            EdgionStatus::StickyBackendUnhealthy => Some(500),
            EdgionStatus::BackendTlsSanValidationFailed => Some(502),
            EdgionStatus::BackendTlsCaInvalid => Some(502),
            EdgionStatus::UpstreamAuthorityRejected => Some(502),
            EdgionStatus::PluginTerminateWithoutResponse => Some(502),
            EdgionStatus::ForwardAuthServiceUnreachable => Some(503),
            EdgionStatus::RequestTimeoutExceeded => Some(504),
            EdgionStatus::GrpcClientDeadlineExceeded => Some(504),
            EdgionStatus::BackendRequestTimeoutRetry => Some(504),
            EdgionStatus::BodyRefusedWebsocket => Some(500),
            EdgionStatus::BodyRefusedGrpc => Some(500),
            EdgionStatus::BodyRefusedStage => Some(500),
            EdgionStatus::BodyRefusedNoSnapshot => Some(500),
            EdgionStatus::BodyReadAborted => Some(500),
            EdgionStatus::BodyReadTimeout => Some(408),
            EdgionStatus::BodyReadTooLarge => Some(413),
            EdgionStatus::BodyReadUnimplemented => Some(500),
            EdgionStatus::BodyReplayFailed => Some(500),
            EdgionStatus::BodyCaptureStorageFailed => Some(500),
            EdgionStatus::PluginResponseReplaced => None,
            EdgionStatus::PluginResponseReplaceUnsupportedGrpc => None,
            EdgionStatus::PluginResponseDenyAborted => None,
            EdgionStatus::OutboundTlsVerifyUnsupported => None,
            EdgionStatus::OutboundTlsCaMissing => None,
            EdgionStatus::OutboundTlsClientCertInvalid => None,
            EdgionStatus::OutboundTlsSanMismatch => None,
        }
    }

    #[test]
    fn outbound_tls_san_mismatch_renders_stable_code() {
        let status = EdgionStatus::OutboundTlsSanMismatch;
        assert_eq!(status as u32, 999_09);
        assert_eq!(
            status.http_status(),
            None,
            "OutboundTlsSanMismatch should have no HTTP status"
        );
    }

    #[test]
    fn http_status_matches_for_all_variants() {
        let all: &[EdgionStatus] = &[
            EdgionStatus::Unknown,
            EdgionStatus::UnknownGRPCERR,
            EdgionStatus::AccessLogSerializeFailed,
            EdgionStatus::AiResponseTranslationFailed,
            EdgionStatus::AiQuotaReconciliationFailed,
            EdgionStatus::AiQuotaOverLimitTerminated,
            EdgionStatus::AiQuotaErrorEventDropped,
            EdgionStatus::AcmeChallengeServed,
            EdgionStatus::HostMissing,
            EdgionStatus::XffHeaderTooLong,
            EdgionStatus::PathEscapedSlashRejected,
            EdgionStatus::PathInvalidPercentEncoding,
            EdgionStatus::DuplicateHostHeader,
            EdgionStatus::RouteNotFound,
            EdgionStatus::SniHostMismatch,
            EdgionStatus::ClientCertInvalid,
            EdgionStatus::ClientClosed,
            EdgionStatus::UpstreamNotRouteMatched,
            EdgionStatus::UpstreamNotBackendRefs,
            EdgionStatus::UpstreamInconsistentWeight,
            EdgionStatus::GrpcUpstreamNotRouteMatched,
            EdgionStatus::BackendEndpointSliceNotFoundByConsistent,
            EdgionStatus::BackendEndpointSliceNotFoundByLeastConn,
            EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin,
            EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault,
            EdgionStatus::BackendEndpointSliceNotFoundByEwma,
            EdgionStatus::RetryBudgetExhausted,
            EdgionStatus::BackendServiceNotFound,
            EdgionStatus::BackendClusterIpNotFound,
            EdgionStatus::BackendExternalNameNotFound,
            EdgionStatus::BackendPortResolutionFailed,
            EdgionStatus::BackendAddressParsingFailed,
            EdgionStatus::BackendLoadBalancerSelectionFailed,
            EdgionStatus::BackendServiceImportNotImplemented,
            EdgionStatus::BackendLocalhostNotAllowed,
            EdgionStatus::Http2Required,
            EdgionStatus::GrpcUpstreamNotBackendRefs,
            EdgionStatus::RefDenied,
            EdgionStatus::BackendAddrLinkLocalNotAllowed,
            EdgionStatus::BackendDnsResolutionFailed,
            EdgionStatus::BackendUnsupportedKind,
            EdgionStatus::StickyBackendUnhealthy,
            EdgionStatus::BackendTlsSanValidationFailed,
            EdgionStatus::BackendTlsCaInvalid,
            EdgionStatus::UpstreamAuthorityRejected,
            EdgionStatus::PluginTerminateWithoutResponse,
            EdgionStatus::ForwardAuthServiceUnreachable,
            EdgionStatus::RequestTimeoutExceeded,
            EdgionStatus::GrpcClientDeadlineExceeded,
            EdgionStatus::BackendRequestTimeoutRetry,
            EdgionStatus::BodyRefusedWebsocket,
            EdgionStatus::BodyRefusedGrpc,
            EdgionStatus::BodyRefusedStage,
            EdgionStatus::BodyRefusedNoSnapshot,
            EdgionStatus::BodyReadAborted,
            EdgionStatus::BodyReadTimeout,
            EdgionStatus::BodyReadTooLarge,
            EdgionStatus::BodyReadUnimplemented,
            EdgionStatus::BodyReplayFailed,
            EdgionStatus::BodyCaptureStorageFailed,
            EdgionStatus::PluginResponseReplaced,
            EdgionStatus::PluginResponseReplaceUnsupportedGrpc,
            EdgionStatus::PluginResponseDenyAborted,
            EdgionStatus::OutboundTlsVerifyUnsupported,
            EdgionStatus::OutboundTlsCaMissing,
            EdgionStatus::OutboundTlsClientCertInvalid,
            EdgionStatus::OutboundTlsSanMismatch,
        ];
        for &s in all {
            assert_eq!(s.http_status(), expected_http(s), "http_status mismatch for {s:?}");
        }
    }

    #[test]
    fn body_status_codes_map_to_their_http_status() {
        assert_eq!(EdgionStatus::BodyRefusedWebsocket.http_status(), Some(500));
        assert_eq!(EdgionStatus::BodyRefusedGrpc.http_status(), Some(500));
        assert_eq!(EdgionStatus::BodyRefusedStage.http_status(), Some(500));
        assert_eq!(EdgionStatus::BodyRefusedNoSnapshot.http_status(), Some(500));
        assert_eq!(EdgionStatus::BodyReadAborted.http_status(), Some(500));
        assert_eq!(EdgionStatus::BodyReadUnimplemented.http_status(), Some(500));
        assert_eq!(EdgionStatus::BodyReadTimeout.http_status(), Some(408));
        assert_eq!(EdgionStatus::BodyReadTooLarge.http_status(), Some(413));
        assert_eq!(EdgionStatus::BodyReplayFailed.http_status(), Some(500));
        assert_eq!(EdgionStatus::BodyCaptureStorageFailed.http_status(), Some(500));
    }
}
