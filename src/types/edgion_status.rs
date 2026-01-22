//! Edgion status/error codes

#![allow(clippy::inconsistent_digit_grouping)]

/// Error code for Edgion gateway
/// Each error code has a fixed numeric code (0Xxx format) and a message string
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum EdgionStatus {
    Unknown = 999,

    UnknownGRPCERR = 999_01,

    /// Missing Host header (HTTP 400)
    HostMissing = 400_01,

    /// X-Forwarded-For header too long (HTTP 400)
    XffHeaderTooLong = 400_02,

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

    /// Endpoints not found for service (Consistent hash store)
    BackendEndpointNotFoundByConsistent = 500_10,

    /// Endpoints not found for service (LeastConnection store)
    BackendEndpointNotFoundByLeastConn = 500_11,

    /// Endpoints not found for service (RoundRobin store)
    BackendEndpointNotFoundByRoundRobin = 500_12,

    /// Endpoints not found for service (RoundRobin default)
    BackendEndpointNotFoundByRoundRobinDefault = 500_13,

    /// Endpoints not found for service (EWMA store)
    BackendEndpointNotFoundByEwma = 500_14,

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
}
