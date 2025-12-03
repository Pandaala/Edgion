use crate::types::{EdgionStatus, HTTPBackendRef, HTTPRouteMatch};

#[derive(Clone)]
pub struct MatchInfo {
    /// route namespace
    pub rns: String,
    /// route name
    pub rn: String,
    /// match item
    pub m: HTTPRouteMatch,
}

impl MatchInfo {
    pub fn new(rns: String, rn: String, m: HTTPRouteMatch) -> Self {
        Self { rns, rn, m }
    }
}

/// Request information extracted from the incoming request
#[derive(Debug, Clone, Default)]
pub struct RequestInfo {
    /// Hostname from the Host header
    pub hostname: String,
    /// Request path from URI
    pub path: String,
}

pub struct EdgionHttpContext {
    pub x_trace_id: Option<String>,
    pub request_id: Option<String>,

    pub auto_gprc: bool,
    
    /// Request information (hostname, path)
    pub request_info: RequestInfo,
    
    /// Error codes collected during request processing
    pub error_codes: Vec<EdgionStatus>,

    /// Matched route info (namespace, name, match item)
    pub matched_info: Option<MatchInfo>,
    
    /// Selected backend from load balancing
    pub selected_backend: Option<HTTPBackendRef>,
}

impl EdgionHttpContext {
    pub fn new() -> Self {
        Self {
            x_trace_id: None,
            request_id: None,
            auto_gprc: false,
            request_info: RequestInfo::default(),
            error_codes: Vec::with_capacity(5),
            matched_info: None,
            selected_backend: None,
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionStatus) {
        self.error_codes.push(err_code);
    }
}
