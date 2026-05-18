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

    /// DNS resolution failed for ExternalName service hostname
    BackendDnsResolutionFailed = 503_15,

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

    /// Client-supplied `grpc-timeout` deadline already exceeded before peer
    /// selection. Surfaced to the gRPC client as `grpc-status: 4` (DEADLINE_EXCEEDED)
    /// via the standard 504 → grpc-status mapping.
    GrpcClientDeadlineExceeded = 504_02,
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
            EdgionStatus::AcmeChallengeServed => Some(200),
            EdgionStatus::HostMissing => Some(400),
            EdgionStatus::XffHeaderTooLong => Some(400),
            EdgionStatus::PathEscapedSlashRejected => Some(400),
            EdgionStatus::PathInvalidPercentEncoding => Some(400),
            EdgionStatus::DuplicateHostHeader => Some(400),
            EdgionStatus::RouteNotFound => Some(404),
            EdgionStatus::SniHostMismatch => Some(421),
            EdgionStatus::ClientCertInvalid => Some(403),
            EdgionStatus::UpstreamNotRouteMatched => Some(500),
            EdgionStatus::UpstreamNotBackendRefs => Some(500),
            EdgionStatus::UpstreamInconsistentWeight => Some(500),
            EdgionStatus::GrpcUpstreamNotRouteMatched => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByConsistent => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByLeastConn => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault => Some(500),
            EdgionStatus::BackendEndpointSliceNotFoundByEwma => Some(500),
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
            EdgionStatus::PluginTerminateWithoutResponse => Some(502),
            EdgionStatus::ForwardAuthServiceUnreachable => Some(503),
            EdgionStatus::RequestTimeoutExceeded => Some(504),
            EdgionStatus::GrpcClientDeadlineExceeded => Some(504),
        }
    }

    #[test]
    fn http_status_matches_for_all_variants() {
        let all: &[EdgionStatus] = &[
            EdgionStatus::Unknown,
            EdgionStatus::UnknownGRPCERR,
            EdgionStatus::AccessLogSerializeFailed,
            EdgionStatus::AcmeChallengeServed,
            EdgionStatus::HostMissing,
            EdgionStatus::XffHeaderTooLong,
            EdgionStatus::PathEscapedSlashRejected,
            EdgionStatus::PathInvalidPercentEncoding,
            EdgionStatus::DuplicateHostHeader,
            EdgionStatus::RouteNotFound,
            EdgionStatus::SniHostMismatch,
            EdgionStatus::ClientCertInvalid,
            EdgionStatus::UpstreamNotRouteMatched,
            EdgionStatus::UpstreamNotBackendRefs,
            EdgionStatus::UpstreamInconsistentWeight,
            EdgionStatus::GrpcUpstreamNotRouteMatched,
            EdgionStatus::BackendEndpointSliceNotFoundByConsistent,
            EdgionStatus::BackendEndpointSliceNotFoundByLeastConn,
            EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin,
            EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault,
            EdgionStatus::BackendEndpointSliceNotFoundByEwma,
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
            EdgionStatus::PluginTerminateWithoutResponse,
            EdgionStatus::ForwardAuthServiceUnreachable,
            EdgionStatus::RequestTimeoutExceeded,
            EdgionStatus::GrpcClientDeadlineExceeded,
        ];
        for &s in all {
            assert_eq!(s.http_status(), expected_http(s), "http_status mismatch for {s:?}");
        }
    }
}
