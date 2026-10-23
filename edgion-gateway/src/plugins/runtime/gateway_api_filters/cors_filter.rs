//! Gateway API native CORS filter (GEP-1767)
//!
//! Implements the `type: CORS` HTTPRoute filter as defined in Gateway API v1.5.
//! Handles both preflight (OPTIONS) and normal CORS requests.

use std::borrow::Cow;

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use smallvec::SmallVec;

use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, PluginSessionHeaderExt, RequestFilter, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::HTTPCORSFilter;

type CorsHeaderPairs<'a> = SmallVec<[(&'static str, Cow<'a, str>); 6]>;

#[derive(Clone, Debug)]
enum PreparedCorsHeader {
    Omit,
    Fixed(String),
    Reflect,
}

impl PreparedCorsHeader {
    fn new(values: Option<Vec<String>>, credentials: bool) -> Self {
        match values {
            Some(values) if !values.is_empty() => {
                if credentials && values.iter().any(|value| value == "*") {
                    Self::Reflect
                } else {
                    Self::Fixed(values.join(", "))
                }
            }
            _ => Self::Omit,
        }
    }

    fn resolve<'a>(&'a self, session: &dyn PluginSession, request_header: &str) -> Option<Cow<'a, str>> {
        match self {
            Self::Omit => None,
            Self::Fixed(value) => Some(Cow::Borrowed(value)),
            Self::Reflect => session.header_value(request_header).map(Cow::Owned),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GapiCorsFilter {
    allow_origins: Vec<String>,
    credentials: bool,
    has_wildcard_origin: bool,
    allow_methods: PreparedCorsHeader,
    allow_headers: PreparedCorsHeader,
    expose_headers: Option<String>,
    max_age: String,
}

impl GapiCorsFilter {
    /// Validate origins and the supported credentialed exposure policy before activation.
    pub fn try_new(config: HTTPCORSFilter) -> Result<Self, String> {
        if let Err(errors) = config.validate() {
            return Err(format!("CORS filter validation failed: {}", errors.join("; ")));
        }
        let credentials = config.allow_credentials.unwrap_or(false);
        let allow_origins = config.allow_origins.unwrap_or_default();
        let has_wildcard_origin = allow_origins.iter().any(|origin| origin == "*");
        Ok(Self {
            allow_origins,
            credentials,
            has_wildcard_origin,
            allow_methods: PreparedCorsHeader::new(config.allow_methods, credentials),
            allow_headers: PreparedCorsHeader::new(config.allow_headers, credentials),
            expose_headers: config
                .expose_headers
                .filter(|values| !values.is_empty())
                .map(|values| values.join(", ")),
            max_age: config.max_age.to_string(),
        })
    }

    fn is_origin_allowed(&self, origin: &str) -> bool {
        for pattern in &self.allow_origins {
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

    fn is_preflight(&self, session: &dyn PluginSession) -> bool {
        session.get_method() == "OPTIONS"
            && session.header_str("origin").is_some()
            && session.header_str("access-control-request-method").is_some()
    }

    /// Compute CORS response headers as key-value pairs.
    /// This allows callers to apply headers to either a ResponseHeader (preflight)
    /// or via session queue (simple requests).
    fn build_cors_headers<'a>(
        &'a self,
        session: &dyn PluginSession,
        origin: &'a str,
        is_preflight: bool,
    ) -> CorsHeaderPairs<'a> {
        let mut headers = CorsHeaderPairs::new();
        // Cookie/Authorization presence remains a live request decision.
        let acao = if self.credentials
            || !self.has_wildcard_origin
            || session.header_str("cookie").is_some()
            || session.header_str("authorization").is_some()
        {
            origin
        } else {
            "*"
        };
        headers.push(("Access-Control-Allow-Origin", Cow::Borrowed(acao)));
        if self.credentials {
            headers.push(("Access-Control-Allow-Credentials", Cow::Borrowed("true")));
        }
        if let Some(value) = self.allow_methods.resolve(session, "access-control-request-method") {
            headers.push(("Access-Control-Allow-Methods", value));
        }
        if let Some(value) = self.allow_headers.resolve(session, "access-control-request-headers") {
            headers.push(("Access-Control-Allow-Headers", value));
        }
        if let Some(value) = &self.expose_headers {
            headers.push(("Access-Control-Expose-Headers", Cow::Borrowed(value)));
        }
        if is_preflight {
            headers.push(("Access-Control-Max-Age", Cow::Borrowed(&self.max_age)));
        }
        headers
    }

    /// Build a 204 preflight response with CORS headers set directly on the ResponseHeader.
    /// Preflight terminates at the request stage and never reaches the upstream-response
    /// stage, so headers must be written onto a locally-built ResponseHeader here.
    fn build_preflight_response(&self, session: &dyn PluginSession, origin: &str) -> Option<Box<ResponseHeader>> {
        let headers = self.build_cors_headers(session, origin, true);
        let mut resp = ResponseHeader::build(204, Some(headers.len() + 1)).ok()?;
        for (name, value) in headers {
            let _ = match value {
                Cow::Borrowed(value) => resp.insert_header(name, value),
                Cow::Owned(value) => resp.insert_header(name, value),
            };
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
        let Some(origin) = session.header_str("origin") else {
            return PluginRunningResult::GoodNext;
        };

        if !self.is_origin_allowed(origin) {
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
            if let Some(resp) = self.build_preflight_response(session, origin) {
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
#[path = "../../../../tests/unit/plugins/runtime/gateway_api_filters/cors_filter_test.rs"]
mod tests;
