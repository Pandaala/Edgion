//! Gateway API native CORS filter (GEP-1767)
//!
//! Implements the `type: CORS` HTTPRoute filter as defined in Gateway API v1.5.
//! Handles both preflight (OPTIONS) and normal CORS requests.

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use smallvec::SmallVec;

use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, PluginSessionHeaderExt, RequestFilter, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::HTTPCORSFilter;

type CorsHeaderPairs = SmallVec<[(&'static str, String); 6]>;

#[derive(Clone, Debug)]
pub struct GapiCorsFilter {
    config: HTTPCORSFilter,
}

impl GapiCorsFilter {
    /// Validate origins and the supported credentialed exposure policy before activation.
    pub fn try_new(config: HTTPCORSFilter) -> Result<Self, String> {
        if let Err(errors) = config.validate() {
            return Err(format!("CORS filter validation failed: {}", errors.join("; ")));
        }
        Ok(Self { config })
    }

    fn is_origin_allowed(&self, origin: &str) -> bool {
        let origins = match &self.config.allow_origins {
            Some(o) if !o.is_empty() => o,
            _ => return false,
        };

        for pattern in origins {
            if pattern == "*" {
                return true;
            }
            if self.origin_matches_pattern(origin, pattern) {
                return true;
            }
        }
        false
    }

    /// Match origin against a pattern that may contain a wildcard `*` in the host.
    /// Per Gateway API spec, `*` is a greedy left-match within the host portion.
    /// E.g. `https://*.example.com` matches `https://foo.bar.example.com`.
    fn origin_matches_pattern(&self, origin: &str, pattern: &str) -> bool {
        if origin == pattern {
            return true;
        }

        if !pattern.contains('*') {
            return false;
        }

        let Some((p_scheme_host, p_port)) = split_origin_port(pattern) else {
            return false;
        };
        let Some((o_scheme_host, o_port)) = split_origin_port(origin) else {
            return false;
        };

        if p_port != o_port {
            return false;
        }

        if let Some(suffix) = p_scheme_host.strip_prefix('*') {
            o_scheme_host.ends_with(suffix)
        } else if let Some(pos) = p_scheme_host.find('*') {
            let prefix = &p_scheme_host[..pos];
            let suffix = &p_scheme_host[pos + 1..];
            o_scheme_host.starts_with(prefix) && o_scheme_host.ends_with(suffix)
        } else {
            false
        }
    }

    fn is_preflight(&self, session: &mut dyn PluginSession) -> bool {
        session.get_method() == "OPTIONS"
            && session.header_str("origin").is_some()
            && session.header_str("access-control-request-method").is_some()
    }

    /// Compute CORS response headers as key-value pairs.
    /// This allows callers to apply headers to either a ResponseHeader (preflight)
    /// or via session queue (simple requests).
    fn build_cors_headers(&self, session: &mut dyn PluginSession, origin: &str, is_preflight: bool) -> CorsHeaderPairs {
        let mut headers = CorsHeaderPairs::new();
        let credentials = self.config.allow_credentials.unwrap_or(false);

        // Access-Control-Allow-Origin
        let acao = if credentials {
            origin.to_string()
        } else {
            let has_wildcard = self
                .config
                .allow_origins
                .as_ref()
                .map(|o| o.iter().any(|v| v == "*"))
                .unwrap_or(false);
            if has_wildcard {
                // When request carries credentials (Cookie/Authorization), browsers reject
                // "Access-Control-Allow-Origin: *". Echo the origin instead.
                let request_has_credentials =
                    session.header_str("cookie").is_some() || session.header_str("authorization").is_some();
                if request_has_credentials {
                    origin.to_string()
                } else {
                    "*".to_string()
                }
            } else {
                origin.to_string()
            }
        };
        headers.push(("Access-Control-Allow-Origin", acao));

        // Access-Control-Allow-Credentials
        if credentials {
            headers.push(("Access-Control-Allow-Credentials", "true".to_string()));
        }

        // Access-Control-Allow-Methods
        if let Some(methods) = &self.config.allow_methods {
            if !methods.is_empty() {
                if credentials && methods.iter().any(|m| m == "*") {
                    // Echo back the requested method (only if the header is present)
                    if let Some(req_method) = session.header_value("access-control-request-method") {
                        headers.push(("Access-Control-Allow-Methods", req_method));
                    }
                } else {
                    headers.push(("Access-Control-Allow-Methods", methods.join(", ")));
                }
            }
        }

        // Access-Control-Allow-Headers
        if let Some(allow_headers) = &self.config.allow_headers {
            if !allow_headers.is_empty() {
                if credentials && allow_headers.iter().any(|h| h == "*") {
                    // Echo back the requested headers (only if the header is present)
                    if let Some(req_headers) = session.header_value("access-control-request-headers") {
                        headers.push(("Access-Control-Allow-Headers", req_headers));
                    }
                } else {
                    headers.push(("Access-Control-Allow-Headers", allow_headers.join(", ")));
                }
            }
        }

        // Access-Control-Expose-Headers
        if let Some(expose) = &self.config.expose_headers {
            if !expose.is_empty() {
                headers.push(("Access-Control-Expose-Headers", expose.join(", ")));
            }
        }

        // Access-Control-Max-Age (preflight only)
        if is_preflight {
            headers.push(("Access-Control-Max-Age", self.config.max_age.to_string()));
        }

        headers
    }

    /// Build a 204 preflight response with CORS headers set directly on the ResponseHeader.
    /// Preflight terminates at the request stage and never reaches the upstream-response
    /// stage, so headers must be written onto a locally-built ResponseHeader here.
    fn build_preflight_response(&self, session: &mut dyn PluginSession, origin: &str) -> Option<Box<ResponseHeader>> {
        let headers = self.build_cors_headers(session, origin, true);
        let mut resp = ResponseHeader::build(204, Some(headers.len() + 1)).ok()?;
        for (name, value) in &headers {
            let _ = resp.insert_header(*name, value.as_str());
        }
        let _ = resp.append_header("Vary", "Origin");
        Some(Box::new(resp))
    }
}

/// Split an origin like `https://example.com:8443` into (scheme_host, port_suffix).
/// Returns ("https://example.com", ":8443") or ("https://example.com", "").
fn split_origin_port(origin: &str) -> Option<(&str, &str)> {
    let after_scheme = origin.find("://").map(|i| i + 3)?;
    let host_part = &origin[after_scheme..];
    if let Some(colon) = host_part.rfind(':') {
        let port_str = &host_part[colon + 1..];
        if port_str.chars().all(|c| c.is_ascii_digit()) {
            let split_at = after_scheme + colon;
            return Some((&origin[..split_at], &origin[split_at..]));
        }
    }
    Some((origin, ""))
}

#[async_trait]
impl RequestFilter for GapiCorsFilter {
    fn name(&self) -> &str {
        "GapiCorsFilter"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let Some(origin) = session.header_value("origin") else {
            return PluginRunningResult::GoodNext;
        };

        if !self.is_origin_allowed(&origin) {
            plugin_log.push("origin match:deny");
            // For preflight with non-matching origin, return 204 directly
            // to avoid forwarding a useless OPTIONS request to the backend.
            if self.is_preflight(session) {
                if let Ok(resp) = ResponseHeader::build(204, None) {
                    let _ = session.write_response_header(Box::new(resp), true).await;
                    let _ = session.write_response_body(Some(Bytes::new()), true).await;
                }
                plugin_log.push("cors preflight deny");
                return PluginRunningResult::ErrTerminateRequest;
            }
            // Simple request: allow through without CORS headers
            return PluginRunningResult::GoodNext;
        }

        if self.is_preflight(session) {
            // Build preflight response with CORS headers set directly on ResponseHeader.
            // Preflight terminates the request, so the upstream-response stage never
            // runs for this request — headers must be written here, not deferred.
            if let Some(resp) = self.build_preflight_response(session, &origin) {
                let _ = session.write_response_header(resp, true).await;
                let _ = session.write_response_body(Some(Bytes::new()), true).await;
            }

            plugin_log.push("cors preflight");
            return PluginRunningResult::ErrTerminateRequest;
        }

        // Normal CORS request: defer header writing to the upstream-response stage,
        // where the real ResponseHeader is available and append/set semantics apply directly.
        plugin_log.push("cors actual");
        PluginRunningResult::GoodNext
    }
}

impl UpstreamResponseFilter for GapiCorsFilter {
    fn name(&self) -> &str {
        "GapiCorsFilter"
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        let Some(origin) = session.header_value("origin") else {
            return PluginRunningResult::GoodNext;
        };

        if !self.is_origin_allowed(&origin) {
            return PluginRunningResult::GoodNext;
        }

        let headers = self.build_cors_headers(session, &origin, false);
        for (name, value) in &headers {
            let _ = session.set_response_header(name, value);
        }
        let _ = session.append_response_header("Vary", "Origin");
        plugin_log.push("cors response headers applied");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    #[test]
    fn credentialed_expose_headers_validation_and_wire_values() {
        for credentials in [None, Some(false), Some(true)] {
            for names in [vec![], vec!["*"], vec!["X-Request-Id"], vec!["*", "X-Request-Id"]] {
                let config = HTTPCORSFilter {
                    allow_origins: Some(vec!["https://app.example".into()]),
                    allow_credentials: credentials,
                    allow_methods: None,
                    allow_headers: None,
                    expose_headers: Some(names.iter().map(|name| name.to_string()).collect()),
                    max_age: 5,
                };
                let result = GapiCorsFilter::try_new(config);
                if credentials == Some(true) && names.contains(&"*") {
                    assert!(result.unwrap_err().contains("exposeHeaders"));
                    continue;
                }
                let filter = result.unwrap();
                for preflight in [false, true] {
                    let mut session = MockPluginSession::new();
                    let headers = filter.build_cors_headers(&mut session, "https://app.example", preflight);
                    let exposed = headers
                        .iter()
                        .find(|(name, _)| *name == "Access-Control-Expose-Headers");
                    assert_eq!(
                        exposed.map(|(_, value)| value.clone()),
                        (!names.is_empty()).then(|| names.join(", "))
                    );
                }
            }
        }
    }

    #[test]
    fn max_age_default_is_emitted_and_explicit_zero_is_rejected() {
        let config: HTTPCORSFilter = serde_json::from_value(serde_json::json!({
            "allowOrigins": ["https://app.example"]
        }))
        .expect("default maxAge CORS fixture must deserialize");
        let filter = GapiCorsFilter::try_new(config).expect("default maxAge must be valid");
        let mut session = MockPluginSession::new();
        let headers = filter.build_cors_headers(&mut session, "https://app.example", true);
        assert!(headers
            .iter()
            .any(|(name, value)| *name == "Access-Control-Max-Age" && value == "5"));

        let zero: HTTPCORSFilter = serde_json::from_value(serde_json::json!({"maxAge": 0}))
            .expect("zero maxAge CORS fixture must deserialize");
        assert!(GapiCorsFilter::try_new(zero)
            .unwrap_err()
            .contains("maxAge must be between 1"));
    }

    #[test]
    fn test_origin_match_exact() {
        let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
            allow_origins: Some(vec!["https://app.example".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        })
        .expect("test CORS configuration must be valid");
        assert!(filter.is_origin_allowed("https://app.example"));
        assert!(!filter.is_origin_allowed("https://evil.com"));
        assert!(!filter.is_origin_allowed("http://app.example"));
    }

    #[test]
    fn test_origin_match_wildcard() {
        let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
            allow_origins: Some(vec!["*".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        })
        .expect("test CORS configuration must be valid");
        assert!(filter.is_origin_allowed("https://anything.com"));
        assert!(filter.is_origin_allowed("http://localhost:3000"));
    }

    #[test]
    fn test_origin_match_subdomain_wildcard() {
        let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
            allow_origins: Some(vec!["https://*.example.com".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        })
        .expect("test CORS configuration must be valid");
        assert!(filter.is_origin_allowed("https://app.example.com"));
        assert!(filter.is_origin_allowed("https://foo.bar.example.com"));
        assert!(!filter.is_origin_allowed("https://example.com"));
        assert!(!filter.is_origin_allowed("https://evil.com"));
    }

    #[test]
    fn test_no_origins_rejects_all() {
        let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
            allow_origins: None,
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        })
        .expect("test CORS configuration must be valid");
        assert!(!filter.is_origin_allowed("https://anything.com"));
    }

    #[test]
    fn maximum_header_set_stays_inline_and_preserves_order() {
        let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
            allow_origins: Some(vec!["https://app.example".to_string()]),
            allow_credentials: Some(true),
            allow_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
            allow_headers: Some(vec!["Content-Type".to_string(), "X-Trace-Id".to_string()]),
            expose_headers: Some(vec!["X-Request-Id".to_string()]),
            max_age: 3600,
        })
        .expect("maximum-header CORS test configuration must be valid");
        let mut session = MockPluginSession::new();

        let headers = filter.build_cors_headers(&mut session, "https://app.example", true);

        assert_eq!(headers.len(), 6);
        assert!(!headers.spilled());
        assert_eq!(
            headers.as_slice(),
            [
                ("Access-Control-Allow-Origin", "https://app.example".to_string()),
                ("Access-Control-Allow-Credentials", "true".to_string()),
                ("Access-Control-Allow-Methods", "GET, POST".to_string()),
                ("Access-Control-Allow-Headers", "Content-Type, X-Trace-Id".to_string()),
                ("Access-Control-Expose-Headers", "X-Request-Id".to_string()),
                ("Access-Control-Max-Age", "3600".to_string()),
            ]
            .as_slice()
        );
    }

    #[test]
    fn test_split_origin_port() {
        let (host, port) = split_origin_port("https://example.com:8443").unwrap();
        assert_eq!(host, "https://example.com");
        assert_eq!(port, ":8443");

        let (host, port) = split_origin_port("https://example.com").unwrap();
        assert_eq!(host, "https://example.com");
        assert_eq!(port, "");

        let (host, port) = split_origin_port("http://localhost:3000").unwrap();
        assert_eq!(host, "http://localhost");
        assert_eq!(port, ":3000");
    }

    #[test]
    fn test_v15_origin_validation_accepts_valid() {
        let config = HTTPCORSFilter {
            allow_origins: Some(vec![
                "https://example.com".to_string(),
                "https://example.com:8443".to_string(),
                "http://localhost:3000".to_string(),
                "https://*.example.com".to_string(),
            ]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        };
        assert!(GapiCorsFilter::try_new(config).is_ok());

        let wildcard: HTTPCORSFilter = serde_json::from_value(serde_json::json!({"allowOrigins": ["*"]}))
            .expect("wildcard CORS fixture must deserialize");
        assert!(GapiCorsFilter::try_new(wildcard).is_ok());
    }

    #[test]
    fn test_v15_origin_validation_rejects_path() {
        let config = HTTPCORSFilter {
            allow_origins: Some(vec!["https://example.com/path".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        };
        let err = GapiCorsFilter::try_new(config).unwrap_err();
        assert!(err.contains("path component"), "got: {}", err);
    }

    #[test]
    fn test_v15_origin_validation_rejects_trailing_slash() {
        let config = HTTPCORSFilter {
            allow_origins: Some(vec!["https://example.com/".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        };
        assert!(GapiCorsFilter::try_new(config).is_err());
    }

    #[test]
    fn test_v15_origin_validation_rejects_query() {
        let config = HTTPCORSFilter {
            allow_origins: Some(vec!["https://example.com?q=1".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        };
        let err = GapiCorsFilter::try_new(config).unwrap_err();
        assert!(err.contains("query string"), "got: {}", err);
    }

    #[test]
    fn test_v15_origin_validation_rejects_fragment() {
        let config = HTTPCORSFilter {
            allow_origins: Some(vec!["https://example.com#sec".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        };
        let err = GapiCorsFilter::try_new(config).unwrap_err();
        assert!(err.contains("fragment"), "got: {}", err);
    }

    #[test]
    fn test_v15_origin_validation_rejects_no_scheme() {
        let config = HTTPCORSFilter {
            allow_origins: Some(vec!["example.com".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        };
        let err = GapiCorsFilter::try_new(config).unwrap_err();
        assert!(err.contains("scheme"), "got: {}", err);
    }

    #[test]
    fn test_v15_origin_validation_rejects_bad_wildcard() {
        let config = HTTPCORSFilter {
            allow_origins: Some(vec!["https://exam*ple.com".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        };
        let err = GapiCorsFilter::try_new(config).unwrap_err();
        assert!(err.contains("wildcard"), "got: {}", err);
    }
}
