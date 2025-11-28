use std::sync::Arc;
use crate::types::{EdgionErrCode, HTTPRouteRule};

pub struct EdgionHttpContext {
    pub x_trace_id: Option<String>,
    pub request_id: Option<String>,

    pub auto_gprc: bool,
    
    /// Error codes collected during request processing
    pub error_codes: Vec<EdgionErrCode>,

    pub matched_http_route: Option<Arc<HTTPRouteRule>>,
}

impl EdgionHttpContext {
    pub(crate) fn new() -> Self {
        Self {
            x_trace_id: None,
            request_id: None,
            auto_gprc: false,
            error_codes: Vec::new(),
            matched_http_route: None,
        }
    }

    /// Add an error code to the context
    pub fn add_error(&mut self, err_code: EdgionErrCode) {
        self.error_codes.push(err_code);
    }
}