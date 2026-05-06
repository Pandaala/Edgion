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

    /// Route Not Found
    RouteNotFound = 404_01,

    /// SNI and Host header mismatch (HTTP 421)
    SniHostMismatch = 421_01,

    /// Client certificate validation failed (HTTP 403)
    ClientCertInvalid = 403_01,

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

    /// Backend kind not supported (Gateway API conformance: return 500)
    BackendUnsupportedKind = 500_15,

    /// Session-persistence sticky target was unhealthy at peer-selection time.
    /// Runtime fell back to normal LB (non-strict) or returned an error (strict mode).
    StickyBackendUnhealthy = 50016,

    /// Backend TLS certificate SAN validation failed
    BackendTlsSanValidationFailed = 502_01,

    /// Plugin returned `ErrTerminateRequest` without committing a response.
    /// Runtime applied a 502 fallback to ensure the client receives a response.
    /// Operators: grep the `errors` field in access logs to find plugins violating
    /// the `ErrTerminateRequest` convention; the offending plugin name is the last
    /// entry in the request-stage `stage_logs[].filters[]` array.
    PluginTerminateWithoutResponse = 502_02,

    /// ForwardAuth plugin could not reach the configured auth service
    /// (connection refused, DNS failure, or timeout). Emitted both when the
    /// plugin rejects the request and when `allow_degradation` lets it pass.
    ForwardAuthServiceUnreachable = 503_14,

    /// End-to-end request timeout already exceeded before peer selection — the
    /// gateway aborts instead of starting a new upstream attempt that would
    /// also fail. Surfaced to the client as 504.
    RequestTimeoutExceeded = 504_01,
}
