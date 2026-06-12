//! RequestRedirect filter implementation
//!
//! This filter redirects requests to another location.

use async_trait::async_trait;
use http::StatusCode;
use pingora_http::ResponseHeader;

use crate::core::gateway::plugins::runtime::log::{la, PluginLog};
use crate::core::gateway::plugins::runtime::traits::{PluginSession, RequestFilter};
use edgion_common::utils::net::host_without_port;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::HTTPRequestRedirectFilter;

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
    ///
    /// The original query string is preserved (appended as `?<query>` when
    /// non-empty), matching `URLRewriteFilter::rewrite_path` and the route-level
    /// HTTP→HTTPS redirect; the Gateway API redirect filter has no query modifier,
    /// so the client's query must survive the redirect.
    fn build_location(
        &self,
        original_scheme: &str,
        original_host: Option<&str>,
        original_path: &str,
        original_query: Option<&str>,
        matched_path_len: Option<usize>,
        listener_port: u16,
    ) -> String {
        let scheme = self.config.scheme.as_deref().unwrap_or(original_scheme);
        let hostname = self
            .config
            .hostname
            .as_deref()
            .or_else(|| original_host.map(host_without_port))
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

        let location = format!("{}://{}{}{}", scheme, hostname, port_str, path);

        match original_query {
            Some(q) if !q.is_empty() => format!("{}?{}", location, q),
            _ => location,
        }
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
        let original_query = session.get_query();
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
            original_query.as_deref(),
            matched_path_len,
            listener_port,
        );

        // Determine status code (default: 302 Found)
        let status_code = self.config.status_code.unwrap_or(302) as u16;
        let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::FOUND);

        // Log redirect operation (key point)
        plugin_log.push_fmt("redirect {}", la![status.as_u16().to_string().as_str()]);

        // Build redirect response
        let mut resp = match ResponseHeader::build(status, None) {
            Ok(r) => r,
            Err(_) => {
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        if resp.insert_header("Location", &location).is_err() {
            return PluginRunningResult::ErrTerminateRequest;
        }

        let _ = resp.insert_header("Content-Length", "0");

        // Send response and terminate request
        if session.write_response_header(Box::new(resp), true).await.is_err() {
            return PluginRunningResult::ErrTerminateRequest;
        }

        PluginRunningResult::ErrTerminateRequest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(scheme: Option<&str>, port: Option<i32>) -> RequestRedirectFilter {
        RequestRedirectFilter::new(HTTPRequestRedirectFilter {
            scheme: scheme.map(|s| s.to_string()),
            hostname: None,
            path: None,
            port,
            status_code: None,
        })
    }

    #[test]
    fn build_location_preserves_query_string() {
        let f = filter(Some("https"), None);
        assert_eq!(
            f.build_location(
                "http",
                Some("example.com"),
                "/login",
                Some("returnTo=/dashboard"),
                None,
                80
            ),
            "https://example.com/login?returnTo=/dashboard"
        );
    }

    #[test]
    fn build_location_without_query_has_no_question_mark() {
        let f = filter(Some("https"), None);
        // None query and empty query must both omit the '?'.
        assert_eq!(
            f.build_location("http", Some("example.com"), "/login", None, None, 80),
            "https://example.com/login"
        );
        assert_eq!(
            f.build_location("http", Some("example.com"), "/login", Some(""), None, 80),
            "https://example.com/login"
        );
    }

    #[test]
    fn build_location_appends_query_after_non_default_port() {
        let f = filter(None, Some(8443));
        // Query is appended after the port segment, producing a valid authority + path + query.
        assert_eq!(
            f.build_location("https", Some("example.com"), "/search", Some("q=hello"), None, 443),
            "https://example.com:8443/search?q=hello"
        );
    }
}
