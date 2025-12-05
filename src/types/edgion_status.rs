/// Error code for Edgion gateway
/// Each error code has a fixed numeric code (0Xxx format) and a message string
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgionStatus {

    Unknown = 999,

    /// Missing Host header (HTTP 400)
    HostMissing = 400_01,

    /// Route Not Found
    RouteNotFound = 404_01,

    /// Upstream route not matched
    UpstreamNotRouteMatched = 500_01,

    /// No backend refs found
    UpstreamNotBackendRefs = 500_02,

    /// Inconsistent weight configuration (some backends have weight, some don't)
    UpstreamInconsistentWeight = 500_03,

    /// EndpointSlice not found for service (Consistent hash store)
    BackendEndpointSliceNotFoundByConsistent = 500_05,

    /// EndpointSlice not found for service (LeastConnection store)
    BackendEndpointSliceNotFoundByLeastConn = 500_06,

    /// EndpointSlice not found for service (RoundRobin store)
    BackendEndpointSliceNotFoundByRoundRobin = 500_07,

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



}


