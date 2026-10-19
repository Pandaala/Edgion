use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::sync::Arc;

use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::CorsConfig;

type CorsError = Box<dyn std::error::Error + Send + Sync>;
type CorsResult<T> = Result<T, CorsError>;
/// Response-header pairs built for one request. Values are `Cow` so the parts
/// that are fixed by configuration (allow-methods, allow-headers, expose-headers,
/// the literal flag values) borrow instead of allocating; only the genuinely
/// request-dependent parts own their bytes.
type CorsHeaderPairs<'a> = SmallVec<[(&'static str, Cow<'a, str>); 8]>;

/// CORS (Cross-Origin Resource Sharing) plugin
pub struct Cors {
    config: Arc<CorsConfig>,
}

impl Cors {
    /// Create from a `CorsConfig` reference. Wraps the config in an `Arc` —
    /// callers that already hold an initialized `Arc<CorsConfig>` should use
    /// [`Cors::from_arc`] instead to avoid a deep clone.
    ///
    /// `origins_cache` / `compiled_origins_regex` / `compiled_timing_regex` carry
    /// `#[serde(skip)]`, so they are `None` after gRPC/YAML deserialization. Compile
    /// them here via `validate_and_init`; without this `is_origin_allowed` skips the
    /// cache and regex branches, silently breaking comma-separated, `*.domain`
    /// subdomain-wildcard, and `allow_origins_by_regex` origins. Construction runs at plugin build
    /// (config-apply / hot-reload), off the request path, so `tracing` is permitted
    /// here. On failure keep the uncompiled (deny-safe) config: regex/cache origins
    /// are skipped rather than wrongly allowed. In practice this path is unreachable
    /// for an operator config because `validation_error()` (run before construction)
    /// already rejects the same security/syntax cases.
    pub fn new(config: &CorsConfig) -> Self {
        let mut config = config.clone();
        if let Err(e) = config.validate_and_init() {
            tracing::warn!(
                error = %e,
                label = "HTTP plugin",
                "Failed to initialize CorsConfig; regex/cache origins will be ignored (deny-safe)",
            );
        }
        Self::from_arc(Arc::new(config))
    }

    /// Create from a shared, already-initialized `Arc<CorsConfig>` without
    /// cloning the underlying config.
    pub fn from_arc(config: Arc<CorsConfig>) -> Self {
        Cors { config }
    }

    /// Check if this is a CORS preflight request
    fn is_preflight_request(&self, session: &mut dyn PluginSession) -> bool {
        if session.get_method() != "OPTIONS" {
            return false;
        }

        session.header_str("origin").is_some() && session.header_str("access-control-request-method").is_some()
    }

    /// Select the value to emit in the Access-Control-Allow-Origin header for an
    /// already-allowed `origin`. Returns the literal `"*"` only when the configured
    /// `allow_origins` is the standard wildcard `"*"`; in every other case
    /// (force-wildcard `"**"`, single origin, list, or regex match) the request
    /// origin is echoed back. Separated from the allow/deny decision so the value
    /// choice can be unit-tested on its own.
    fn select_acao_value(&self, origin: &str) -> Cow<'static, str> {
        if self.config.allow_origins == "*" {
            Cow::Borrowed("*")
        } else {
            Cow::Owned(origin.to_owned())
        }
    }

    /// Get the validated origin (returns Some(value) if allowed, None if rejected).
    /// Orchestrates the allow/deny check (`CorsConfig::is_origin_allowed`) and the
    /// ACAO value selection ([`Cors::select_acao_value`]).
    fn get_validated_origin(&self, session: &mut dyn PluginSession) -> Option<Cow<'static, str>> {
        let origin = session.header_str("origin")?;

        if !self.config.is_origin_allowed(origin) {
            return None;
        }

        Some(self.select_acao_value(origin))
    }

    /// Compute all CORS + timing + private-network headers as key-value pairs.
    /// This allows callers to apply headers to either a ResponseHeader (preflight)
    /// or via session queue (simple requests / preflight_continue).
    fn build_cors_header_pairs<'a>(
        &'a self,
        session: &mut dyn PluginSession,
        allowed_origin: &'a str,
        is_preflight: bool,
    ) -> CorsHeaderPairs<'a> {
        let mut headers = CorsHeaderPairs::new();

        headers.push(("Access-Control-Allow-Origin", Cow::Borrowed(allowed_origin)));

        // Access-Control-Allow-Methods / -Headers (preflight only). Per WHATWG Fetch
        // 3.2.2/3.2.3 the CORS-preflight fetch algorithm is the only reader of these
        // two; the actual-request response is inspected solely for -Allow-Origin,
        // -Allow-Credentials and -Expose-Headers. Emitting them on an actual response
        // costs bytes on every cross-origin response and, with the `**` shorthands,
        // renders a content-free `Access-Control-Allow-Headers: *` plus the fully
        // expanded method list.
        if is_preflight {
            headers.push((
                "Access-Control-Allow-Methods",
                Cow::Borrowed(self.config.get_allow_methods()),
            ));

            headers.push((
                "Access-Control-Allow-Headers",
                self.config
                    .get_allow_headers(|| session.header_value("access-control-request-headers")),
            ));
        }

        let expose = self.config.get_expose_headers();
        if !expose.is_empty() {
            headers.push(("Access-Control-Expose-Headers", Cow::Borrowed(expose)));
        }

        if self.config.allow_credentials {
            headers.push(("Access-Control-Allow-Credentials", Cow::Borrowed("true")));
        }

        // Access-Control-Max-Age (preflight only)
        if is_preflight {
            if let Some(max_age) = self.config.max_age {
                headers.push(("Access-Control-Max-Age", Cow::Owned(max_age.to_string())));
            }
        }

        if let Some(origin) = session.header_str("origin") {
            if self.config.is_timing_origin_allowed(origin) {
                let timing_value = if let Some(ref timing_origins) = self.config.timing_allow_origins {
                    if timing_origins == "**" {
                        Cow::Owned(origin.to_owned())
                    } else if timing_origins == "*" {
                        Cow::Borrowed("*")
                    } else {
                        Cow::Owned(origin.to_owned())
                    }
                } else {
                    Cow::Owned(origin.to_owned())
                };
                headers.push(("Timing-Allow-Origin", timing_value));
            }
        }

        // Private Network Access
        if self.config.allow_private_network {
            if let Some(req_pn) = session.header_str("access-control-request-private-network") {
                if req_pn == "true" {
                    headers.push(("Access-Control-Allow-Private-Network", Cow::Borrowed("true")));
                }
            }
        }

        headers
    }

    /// Apply CORS headers via session queue (for simple requests and preflight_continue).
    /// Headers are consumed during the upstream response phase.
    fn set_cors_headers_via_session(
        &self,
        session: &mut dyn PluginSession,
        allowed_origin: &str,
        is_preflight: bool,
    ) -> CorsResult<()> {
        let headers = self.build_cors_header_pairs(session, allowed_origin, is_preflight);
        for (name, value) in &headers {
            session.set_response_header(name, value)?;
        }
        if self.config.should_add_vary_header() {
            session.append_response_header("Vary", "Origin")?;
        }
        Ok(())
    }

    /// Build a preflight response with CORS headers set directly on the ResponseHeader.
    /// This avoids the session queue which is only consumed during the upstream response phase.
    fn build_preflight_response(
        &self,
        session: &mut dyn PluginSession,
        allowed_origin: &str,
    ) -> CorsResult<Box<ResponseHeader>> {
        let headers = self.build_cors_header_pairs(session, allowed_origin, true);
        let mut resp = ResponseHeader::build(204, Some(headers.len() + 1)).map_err(|e| Box::new(e) as CorsError)?;
        for (name, value) in &headers {
            let value: &str = value;
            resp.insert_header(*name, value).map_err(|e| Box::new(e) as CorsError)?;
        }
        if self.config.should_add_vary_header() {
            resp.append_header("Vary", "Origin")
                .map_err(|e| Box::new(e) as CorsError)?;
        }
        Ok(Box::new(resp))
    }

    /// Handle preflight request
    async fn handle_preflight(&self, session: &mut dyn PluginSession, allowed_origin: &str) -> CorsResult<()> {
        // If preflight_continue is true, queue headers for upstream response phase
        if self.config.preflight_continue {
            self.set_cors_headers_via_session(session, allowed_origin, true)?;
            return Ok(());
        }

        // Build preflight response with CORS headers set directly on ResponseHeader.
        // Cannot use session queue because terminated requests skip the upstream response phase.
        let resp = self.build_preflight_response(session, allowed_origin)?;
        session.write_response_header(resp, true).await?;
        session.write_response_body(Some(Bytes::new()), true).await?;

        Ok(())
    }

    fn handle_normal_request(&self, session: &mut dyn PluginSession, allowed_origin: &str) -> CorsResult<()> {
        self.set_cors_headers_via_session(session, allowed_origin, false)
    }
}

#[async_trait]
impl RequestFilter for Cors {
    fn name(&self) -> &str {
        "Cors"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if session.header_str("origin").is_none() {
            // No Origin header means same-origin request, skip CORS
            return PluginRunningResult::GoodNext;
        }

        let Some(allowed_origin) = self.get_validated_origin(session) else {
            plugin_log.push("origin match:deny");
            // For preflight with non-matching origin, return 204 directly
            // to avoid forwarding a useless OPTIONS request to the backend.
            if self.is_preflight_request(session) {
                plugin_log.push("cors preflight");
                if let Ok(resp) = ResponseHeader::build(204, None) {
                    let _ = session.write_response_header(Box::new(resp), true).await;
                    let _ = session.write_response_body(Some(Bytes::new()), true).await;
                }
                return PluginRunningResult::ErrTerminateRequest;
            }
            // Simple request: allow through without CORS headers (browser will block)
            return PluginRunningResult::GoodNext;
        };

        if self.is_preflight_request(session) {
            plugin_log.push("cors preflight");
            if let Err(_e) = self.handle_preflight(session, &allowed_origin).await {
                plugin_log.push("origin match:deny");
                return PluginRunningResult::ErrTerminateRequest;
            }

            plugin_log.push("origin match:allow");

            if self.config.preflight_continue {
                return PluginRunningResult::GoodNext;
            }

            // Preflight is complete, terminate request (don't proxy to upstream)
            return PluginRunningResult::ErrTerminateRequest;
        }

        plugin_log.push("cors actual");
        if let Err(_e) = self.handle_normal_request(session, &allowed_origin) {
            plugin_log.push("origin match:deny");
        } else {
            plugin_log.push("origin match:allow");
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/cors/plugin_test.rs"]
mod tests;
