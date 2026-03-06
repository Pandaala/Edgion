//! Gateway API native CORS filter (GEP-1767)
//!
//! Implements the `type: CORS` HTTPRoute filter as defined in Gateway API v1.4.
//! Handles both preflight (OPTIONS) and normal CORS requests.

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

use crate::core::plugins::plugin_runtime::log::PluginLog;
use crate::core::plugins::plugin_runtime::traits::{PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::HTTPCORSFilter;

pub struct GapiCorsFilter {
    config: HTTPCORSFilter,
}

impl GapiCorsFilter {
    pub fn new(config: HTTPCORSFilter) -> Self {
        Self { config }
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
        session.method() == "OPTIONS"
            && session.header_value("origin").is_some()
            && session.header_value("access-control-request-method").is_some()
    }

    fn set_cors_headers(&self, session: &mut dyn PluginSession, origin: &str) {
        let credentials = self.config.allow_credentials.unwrap_or(false);

        // Access-Control-Allow-Origin
        if credentials {
            let _ = session.set_response_header("Access-Control-Allow-Origin", origin);
        } else {
            let has_wildcard = self
                .config
                .allow_origins
                .as_ref()
                .map(|o| o.iter().any(|v| v == "*"))
                .unwrap_or(false);
            if has_wildcard {
                let _ = session.set_response_header("Access-Control-Allow-Origin", "*");
            } else {
                let _ = session.set_response_header("Access-Control-Allow-Origin", origin);
            }
        }

        // Access-Control-Allow-Credentials
        if credentials {
            let _ = session.set_response_header("Access-Control-Allow-Credentials", "true");
        }

        // Access-Control-Allow-Methods
        if let Some(methods) = &self.config.allow_methods {
            if !methods.is_empty() {
                if credentials && methods.iter().any(|m| m == "*") {
                    if let Some(req_method) = session.header_value("access-control-request-method") {
                        let _ = session.set_response_header("Access-Control-Allow-Methods", &req_method);
                    }
                } else {
                    let val = methods.join(", ");
                    let _ = session.set_response_header("Access-Control-Allow-Methods", &val);
                }
            }
        }

        // Access-Control-Allow-Headers
        if let Some(headers) = &self.config.allow_headers {
            if !headers.is_empty() {
                if credentials && headers.iter().any(|h| h == "*") {
                    if let Some(req_headers) = session.header_value("access-control-request-headers") {
                        let _ = session.set_response_header("Access-Control-Allow-Headers", &req_headers);
                    }
                } else {
                    let val = headers.join(", ");
                    let _ = session.set_response_header("Access-Control-Allow-Headers", &val);
                }
            }
        }

        // Access-Control-Expose-Headers
        if let Some(expose) = &self.config.expose_headers {
            if !expose.is_empty() {
                let val = expose.join(", ");
                let _ = session.set_response_header("Access-Control-Expose-Headers", &val);
            }
        }

        // Access-Control-Max-Age (preflight only)
        if self.is_preflight(session) && self.config.max_age > 0 {
            let _ = session.set_response_header("Access-Control-Max-Age", &self.config.max_age.to_string());
        }

        // Vary: Origin
        let _ = session.append_response_header("Vary", "Origin");
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
        let origin = match session.header_value("origin") {
            Some(o) => o,
            None => return PluginRunningResult::GoodNext,
        };

        if !self.is_origin_allowed(&origin) {
            plugin_log.push("CORS: origin rejected; ");
            // Per Gateway API spec: still allow the request through, just don't set CORS headers
            return PluginRunningResult::GoodNext;
        }

        if self.is_preflight(session) {
            self.set_cors_headers(session, &origin);

            if let Ok(resp) = ResponseHeader::build(204, None) {
                let _ = session.write_response_header(Box::new(resp), true).await;
                let _ = session.write_response_body(Some(Bytes::new()), true).await;
            }

            plugin_log.push("CORS: preflight handled; ");
            return PluginRunningResult::ErrTerminateRequest;
        }

        // Normal CORS request: set headers (they will be sent with the proxied response)
        self.set_cors_headers(session, &origin);
        plugin_log.push("CORS: headers set; ");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_origin_match_exact() {
        let filter = GapiCorsFilter::new(HTTPCORSFilter {
            allow_origins: Some(vec!["https://app.example".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        });
        assert!(filter.is_origin_allowed("https://app.example"));
        assert!(!filter.is_origin_allowed("https://evil.com"));
        assert!(!filter.is_origin_allowed("http://app.example"));
    }

    #[test]
    fn test_origin_match_wildcard() {
        let filter = GapiCorsFilter::new(HTTPCORSFilter {
            allow_origins: Some(vec!["*".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        });
        assert!(filter.is_origin_allowed("https://anything.com"));
        assert!(filter.is_origin_allowed("http://localhost:3000"));
    }

    #[test]
    fn test_origin_match_subdomain_wildcard() {
        let filter = GapiCorsFilter::new(HTTPCORSFilter {
            allow_origins: Some(vec!["https://*.example.com".to_string()]),
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        });
        assert!(filter.is_origin_allowed("https://app.example.com"));
        assert!(filter.is_origin_allowed("https://foo.bar.example.com"));
        assert!(!filter.is_origin_allowed("https://example.com"));
        assert!(!filter.is_origin_allowed("https://evil.com"));
    }

    #[test]
    fn test_no_origins_rejects_all() {
        let filter = GapiCorsFilter::new(HTTPCORSFilter {
            allow_origins: None,
            allow_credentials: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: 5,
        });
        assert!(!filter.is_origin_allowed("https://anything.com"));
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
}
