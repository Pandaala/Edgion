use crate::types::{EdgionErrStatus, HTTPBackendRef, MatchInfo};

pub struct EdgionHttpContext {
    pub x_trace_id: Option<String>,
    pub request_id: Option<String>,

    pub auto_gprc: bool,
    
    /// Hostname from the Host header
    pub hostname: String,
    
    /// Error codes collected during request processing
    pub error_codes: Vec<EdgionErrStatus>,

    /// Matched route info (namespace, name, match item)
    pub matched_info: Option<MatchInfo>,
    
    /// Selected backend from load balancing
    pub selected_backend: Option<HTTPBackendRef>,
}

impl EdgionHttpContext {
    pub(crate) fn new() -> Self {
        Self {
            x_trace_id: None,
            request_id: None,
            auto_gprc: false,
            hostname: String::new(),
            error_codes: Vec::with_capacity(5),
            matched_info: None,
            selected_backend: None,
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionErrStatus) {
        self.error_codes.push(err_code);
    }
}