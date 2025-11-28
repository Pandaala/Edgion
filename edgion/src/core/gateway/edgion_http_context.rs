use std::sync::Arc;
use crate::types::{EdgionErrStatus, HTTPRouteRule};

pub struct EdgionHttpContext {
    pub x_trace_id: Option<String>,
    pub request_id: Option<String>,

    pub auto_gprc: bool,
    
    /// Hostname from the Host header
    pub hostname: String,
    
    /// Error codes collected during request processing
    pub error_codes: Vec<EdgionErrStatus>,

    pub matched_http_route: Option<Arc<HTTPRouteRule>>,
}

impl EdgionHttpContext {
    pub(crate) fn new() -> Self {
        Self {
            x_trace_id: None,
            request_id: None,
            auto_gprc: false,
            hostname: String::new(),
            error_codes: Vec::new(),
            matched_http_route: None,
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionErrStatus) {
        self.error_codes.push(err_code);
    }
}