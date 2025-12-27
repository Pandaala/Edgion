//! RequestRedirect filter implementation
//!
//! This filter redirects requests to another location.

use async_trait::async_trait;
use http::StatusCode;
use pingora_http::ResponseHeader;

use crate::types::resources::HTTPRequestRedirectFilter;
use crate::types::filters::PluginRunningResult;
use crate::core::plugins::plugin_runtime::filters::{RequestFilter, PluginSession};
use crate::core::plugins::plugin_runtime::log::PluginLog;

pub struct RequestRedirectFilter {
    config: HTTPRequestRedirectFilter,
}

impl RequestRedirectFilter {
    pub fn new(config: HTTPRequestRedirectFilter) -> Self {
        Self { config }
    }

    /// Build redirect Location header value from config and request
    fn build_location(&self, original_host: Option<&str>, original_path: &str) -> String {
        let scheme = self.config.scheme.as_deref().unwrap_or("https");
        let hostname = self.config.hostname.as_deref()
            .or(original_host)
            .unwrap_or("localhost");
        
        let port_str = self.config.port
            .map(|p| format!(":{}", p))
            .unwrap_or_default();
        
        // Handle path modification
        let path = if let Some(path_modifier) = &self.config.path {
            if let Some(replace_full) = &path_modifier.replace_full_path {
                replace_full.clone()
            } else if let Some(replace_prefix) = &path_modifier.replace_prefix_match {
                // TODO: need original matched prefix to do proper replacement
                replace_prefix.clone()
            } else {
                original_path.to_string()
            }
        } else {
            original_path.to_string()
        };

        format!("{}://{}{}{}", scheme, hostname, port_str, path)
    }
}

#[async_trait]
impl RequestFilter for RequestRedirectFilter {
    fn name(&self) -> &str {
        "RequestRedirect"
    }

    async fn run_request(
        &self,
        session: &mut dyn PluginSession,
        _log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Get original request info for building Location
        let original_host = session.header_value("host");
        let original_path = session.header_value(":path").unwrap_or_else(|| "/".to_string());
        
        // Build Location header
        let location = self.build_location(original_host.as_deref(), &original_path);
        
        // Determine status code (default: 302 Found)
        let status_code = self.config.status_code.unwrap_or(302) as u16;
        let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::FOUND);

        // Build redirect response
        let mut resp = match ResponseHeader::build(status, None) {
            Ok(r) => r,
            Err(_) => return PluginRunningResult::ErrTerminateRequest,
        };
        
        let _ = resp.insert_header("Location", &location);
        let _ = resp.insert_header("Content-Length", "0");

        // Send response and terminate request
        let _ = session.write_response_header(Box::new(resp), true).await;
        
        PluginRunningResult::ErrTerminateRequest
    }
}

