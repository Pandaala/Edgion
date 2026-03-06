//! RequestRedirect filter implementation
//!
//! This filter redirects requests to another location.

use async_trait::async_trait;
use http::StatusCode;
use pingora_http::ResponseHeader;

use crate::core::gateway::plugins::runtime::log::PluginLog;
use crate::core::gateway::plugins::runtime::traits::{PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::HTTPRequestRedirectFilter;

fn default_port_for_scheme(scheme: &str) -> u16 {
    match scheme {
        "https" => 443,
        _ => 80,
    }
}

pub struct RequestRedirectFilter {
    config: HTTPRequestRedirectFilter,
}

impl RequestRedirectFilter {
    pub fn new(config: HTTPRequestRedirectFilter) -> Self {
        Self { config }
    }

    /// Build redirect Location header value from config and request.
    ///
    /// Port omission follows RFC 7230 / Gateway API semantics:
    /// - HTTP  scheme default port is 80  → omit from Location
    /// - HTTPS scheme default port is 443 → omit from Location
    /// - Any other port is always included
    fn build_location(
        &self,
        original_scheme: &str,
        original_host: Option<&str>,
        original_path: &str,
        matched_path_len: Option<usize>,
        listener_port: u16,
    ) -> String {
        let scheme = self.config.scheme.as_deref().unwrap_or(original_scheme);
        let hostname = self.config.hostname.as_deref()
            .or_else(|| original_host.map(|h| h.split(':').next().unwrap_or(h)))
            .unwrap_or("localhost");

        let final_port: u16 = if let Some(p) = self.config.port {
            p as u16
        } else if self.config.scheme.is_some() {
            // Scheme changed without explicit port → use new scheme's default
            default_port_for_scheme(scheme)
        } else {
            // No scheme change, no port override → preserve listener port
            listener_port
        };

        let port_str = if final_port == default_port_for_scheme(scheme) {
            String::new()
        } else {
            format!(":{}", final_port)
        };

        // Handle path modification
        let path = if let Some(path_modifier) = &self.config.path {
            if let Some(replace_full) = &path_modifier.replace_full_path {
                replace_full.clone()
            } else if let Some(replace_prefix) = &path_modifier.replace_prefix_match {
                if let Some(len) = matched_path_len {
                    if len <= original_path.len() {
                        let suffix = &original_path[len..];
                        format!("{}{}", replace_prefix, suffix)
                    } else {
                        replace_prefix.clone()
                    }
                } else {
                    replace_prefix.clone()
                }
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

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Determine the original request scheme from TLS context
        let original_scheme = if session.ctx().request_info.sni.is_some() {
            "https"
        } else {
            "http"
        };

        // Get original request info for building Location
        let original_host = session.header_value("host");
        let original_path = session.get_path().to_string();
        let listener_port = session.ctx().request_info.listener_port;

        // Extract matched path length from route info
        let matched_path_len = session
            .ctx()
            .route_unit
            .as_ref()
            .and_then(|unit| unit.matched_info.m.path.as_ref())
            .and_then(|path_match| {
                if path_match.match_type.as_deref() == Some("PathPrefix") {
                    path_match.value.as_ref().map(|v| v.len())
                } else {
                    None
                }
            });

        // Build Location header
        let location = self.build_location(
            original_scheme,
            original_host.as_deref(),
            &original_path,
            matched_path_len,
            listener_port,
        );

        // Determine status code (default: 302 Found)
        let status_code = self.config.status_code.unwrap_or(302) as u16;
        let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::FOUND);

        // Log redirect operation (key point)
        plugin_log.push(&format!("Redirect to {} [{}]; ", location, status.as_u16()));

        // Build redirect response
        let mut resp = match ResponseHeader::build(status, None) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to build redirect response: {:?}", e);
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        if let Err(e) = resp.insert_header("Location", &location) {
            tracing::error!("Failed to insert Location header: {:?}", e);
            return PluginRunningResult::ErrTerminateRequest;
        }

        let _ = resp.insert_header("Content-Length", "0");

        // Send response and terminate request
        if let Err(e) = session.write_response_header(Box::new(resp), true).await {
            tracing::error!("Failed to write redirect response: {:?}", e);
            return PluginRunningResult::ErrTerminateRequest;
        }

        PluginRunningResult::ErrTerminateRequest
    }
}
