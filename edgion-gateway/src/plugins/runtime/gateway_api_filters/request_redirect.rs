//! RequestRedirect filter implementation
//!
//! This filter redirects requests to another location.

use async_trait::async_trait;
use pingora_http::ResponseHeader;

use crate::plugins::runtime::log::{la, PluginLog};
use crate::plugins::runtime::traits::{PluginSession, PluginSessionHeaderExt, RequestFilter};
use crate::util::net::host_without_port;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::{
    HTTPRequestRedirectFilter, PreparedPathModifier, PreparedRedirectScheme, PreparedRequestRedirect,
    RequestTransformViolation,
};

use super::path_modifier::replace_prefix_match;

fn default_port_for_scheme(scheme: &str) -> u16 {
    match scheme {
        "https" => 443,
        _ => 80,
    }
}

pub struct RequestRedirectFilter {
    config: PreparedRequestRedirect,
}

impl RequestRedirectFilter {
    pub fn try_new(config: &HTTPRequestRedirectFilter) -> Result<Self, RequestTransformViolation> {
        Ok(Self {
            config: config.prepare()?,
        })
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
        matched_path: Option<&str>,
        listener_port: u16,
    ) -> Option<String> {
        let scheme = self
            .config
            .scheme()
            .map(PreparedRedirectScheme::as_str)
            .unwrap_or(original_scheme);
        let hostname = self
            .config
            .hostname()
            .or_else(|| original_host.map(host_without_port))
            .unwrap_or("localhost");

        let final_port: u16 = if let Some(port) = self.config.port() {
            port.get()
        } else if self.config.scheme().is_some() {
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
        let path = match self.config.path() {
            Some(PreparedPathModifier::ReplaceFullPath(replacement)) => replacement.clone(),
            Some(PreparedPathModifier::ReplacePrefixMatch(replacement)) => {
                replace_prefix_match(original_path, matched_path?, replacement)
            }
            None => original_path.to_string(),
        };

        let location = format!("{}://{}{}{}", scheme, hostname, port_str, path);

        match original_query {
            Some(q) if !q.is_empty() => Some(format!("{}?{}", location, q)),
            _ => Some(location),
        }
    }
}

#[async_trait]
impl RequestFilter for RequestRedirectFilter {
    fn name(&self) -> &str {
        "RequestRedirect"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Unknown transport state defaults to HTTPS so redirects never downgrade.
        let original_scheme = match session.downstream_is_tls() {
            Some(false) => "http",
            Some(true) | None => "https",
        };

        let location = {
            let original_host = session.header_str("host");
            let original_path = session.get_path();
            let original_query = session.request_query();
            let ctx = session.ctx();
            let listener_port = ctx.request_info.listener_port;
            let matched_path = ctx
                .route_unit
                .as_ref()
                .and_then(|unit| unit.matched_info.m.path.as_ref())
                .and_then(|path_match| {
                    (path_match.match_type.as_deref() == Some("PathPrefix"))
                        .then_some(path_match.value.as_deref())
                        .flatten()
                });

            self.build_location(
                original_scheme,
                original_host,
                original_path,
                original_query,
                matched_path,
                listener_port,
            )
        };

        let Some(location) = location else {
            plugin_log.push_err("redirect prefix context unavailable");
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        };

        let status = self.config.status_code();

        plugin_log.push_fmt("redirect {}", la![status.as_u16().to_string().as_str()]);

        let Ok(mut resp) = ResponseHeader::build(status, None) else {
            plugin_log.push_err("redirect response build failed");
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        };

        if resp.insert_header("Location", &location).is_err() {
            plugin_log.push_err("redirect location header failed");
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        }

        if resp.insert_header("Content-Length", "0").is_err() {
            plugin_log.push_err("redirect content length header failed");
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        }

        if session.write_response_header(Box::new(resp), true).await.is_err() {
            plugin_log.push_err("redirect response write failed");
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        }

        PluginRunningResult::ErrTerminateRequest
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/gateway_api_filters/request_redirect_test.rs"]
mod tests;
