/// Error code for Edgion gateway
/// Each error code has a fixed numeric code (0Xxx format) and a message string
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgionErrStatus {
    /// Missing Host header (HTTP 400)
    HostMissing = 0x40001,

    /// Route Not Found
    RouteNotFound = 0x40401,

    UpstreamNotRouteMatched = 0x50001,

    UpstreamNotRouteMatched = 0x50001,

}


