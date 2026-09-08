//! RequestRedirect filter implementation
//!
//! This filter redirects requests to another location.

use async_trait::async_trait;
use http::StatusCode;
use pingora_http::ResponseHeader;

use crate::plugins::runtime::log::{la, PluginLog};
use crate::plugins::runtime::traits::{PluginSession, PluginSessionHeaderExt, RequestFilter};
use crate::util::net::host_without_port;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::HTTPRequestRedirectFilter;

use super::path_modifier::replace_prefix_match;

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
        matched_path: Option<&str>,
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
                replace_prefix_match(original_path, matched_path.unwrap_or(original_path), replace_prefix)
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
        // Unknown transport state defaults to HTTPS so redirects never downgrade.
        let original_scheme = match session.downstream_is_tls() {
            Some(false) => "http",
            Some(true) | None => "https",
        };

        let location = {
            let original_host = session.header_str("host");
            let original_path = session.get_path();
            let original_query = session.get_query();
            let ctx = session.ctx();
            let listener_port = ctx.request_info.listener_port;
            let matched_path = ctx
                .route_unit
                .as_ref()
                .and_then(|unit| unit.matched_info.m.path.as_ref())
                .and_then(|path_match| {
                    (path_match.match_type.as_deref() == Some("PathPrefix"))
                        .then(|| path_match.value.as_deref())
                        .flatten()
                });

            self.build_location(
                original_scheme,
                original_host,
                original_path,
                original_query.as_deref(),
                matched_path,
                listener_port,
            )
        };

        // Determine status code (default: 302 Found)
        let status_code = self.config.status_code.unwrap_or(302) as u16;
        let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::FOUND);

        plugin_log.push_fmt("redirect {}", la![status.as_u16().to_string().as_str()]);

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

        if session.write_response_header(Box::new(resp), true).await.is_err() {
            return PluginRunningResult::ErrTerminateRequest;
        }

        PluginRunningResult::ErrTerminateRequest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::EdgionHttpContext;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use edgion_resources::resources::{HTTPPathModifier, HTTPPathModifierType};

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

    #[test]
    fn build_location_replaces_parameterized_prefix_by_path_elements() {
        let f = RequestRedirectFilter::new(HTTPRequestRedirectFilter {
            scheme: Some("https".to_string()),
            hostname: None,
            path: Some(HTTPPathModifier {
                modifier_type: HTTPPathModifierType::ReplacePrefixMatch,
                replace_full_path: None,
                replace_prefix_match: Some("/v2".to_string()),
            }),
            port: None,
            status_code: None,
        });

        assert_eq!(
            f.build_location(
                "http",
                Some("example.com"),
                "/users/alice/orders",
                Some("view=full"),
                Some("/users/:id"),
                80,
            ),
            "https://example.com/v2/orders?view=full"
        );
    }

    async fn assert_emitted_redirect(
        downstream_is_tls: Option<bool>,
        configured_scheme: Option<&str>,
        listener_port: u16,
        status_code: Option<i32>,
        expected_status: u16,
        expected_location: &str,
    ) {
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("host", "example.com")]);
        session.expect_downstream_is_tls().return_const(downstream_is_tls);
        session.expect_get_path().return_const("/search".to_string());
        session
            .expect_get_query()
            .return_const(Some("q=hello%20world&next=%2Fhome".to_string()));

        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.listener_port = listener_port;
        session.expect_ctx().return_const(ctx);

        let expected_location = expected_location.to_string();
        session
            .expect_write_response_header()
            .times(1)
            .returning(move |response, end_stream| {
                assert_eq!(response.status.as_u16(), expected_status);
                assert!(end_stream);
                assert_eq!(
                    response.headers.get("Location").and_then(|value| value.to_str().ok()),
                    Some(expected_location.as_str())
                );
                assert_eq!(
                    response
                        .headers
                        .get("Content-Length")
                        .and_then(|value| value.to_str().ok()),
                    Some("0")
                );
                Ok(())
            });

        let f = RequestRedirectFilter::new(HTTPRequestRedirectFilter {
            scheme: configured_scheme.map(str::to_string),
            hostname: None,
            path: None,
            port: None,
            status_code,
        });
        let mut log = PluginLog::new("test");
        assert_eq!(
            f.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        );
    }

    #[tokio::test]
    async fn run_request_emits_exact_location_status_and_end_stream() {
        assert_emitted_redirect(
            Some(true),
            None,
            443,
            Some(308),
            308,
            "https://example.com/search?q=hello%20world&next=%2Fhome",
        )
        .await;
    }

    #[tokio::test]
    async fn omitted_scheme_uses_transport_tls_state_and_https_safe_unknown() {
        assert_emitted_redirect(
            Some(false),
            None,
            80,
            None,
            302,
            "http://example.com/search?q=hello%20world&next=%2Fhome",
        )
        .await;
        // The default context has no SNI, proving TLS does not depend on SNI presence.
        assert_emitted_redirect(
            Some(true),
            None,
            443,
            None,
            302,
            "https://example.com/search?q=hello%20world&next=%2Fhome",
        )
        .await;
        assert_emitted_redirect(
            None,
            None,
            443,
            None,
            302,
            "https://example.com/search?q=hello%20world&next=%2Fhome",
        )
        .await;
    }

    #[tokio::test]
    async fn explicit_scheme_takes_precedence_over_transport_tls_state() {
        assert_emitted_redirect(
            Some(true),
            Some("http"),
            443,
            None,
            302,
            "http://example.com/search?q=hello%20world&next=%2Fhome",
        )
        .await;
    }
}
