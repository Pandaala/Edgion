/// Error code for Edgion gateway
/// Each error code has a fixed numeric code (0Xxx format) and a message string
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgionErrCode {
    /// Missing Host header (HTTP 400)
    /// Error code: 0x40001
    HostMissing = 0x40001,
    
    /// Route not found (HTTP 404)
    /// Error code: 0x40401
    RouteNotFound = 0x40401,
}

impl EdgionErrCode {
    /// Get the numeric code as u32
    pub fn code(&self) -> u32 {
        *self as u32
    }

    /// Get the error message string
    pub fn message(&self) -> &'static str {
        match self {
            EdgionErrCode::HostMissing => "Missing Host header",
            EdgionErrCode::RouteNotFound => "Route not found",
        }
    }

    /// Get HTTP status code associated with this error
    pub fn http_status(&self) -> u16 {
        match self {
            EdgionErrCode::HostMissing => 400,
            EdgionErrCode::RouteNotFound => 404,
        }
    }
}


