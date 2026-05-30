use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use std::sync::Arc;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::CorsConfig;

type CorsError = Box<dyn std::error::Error + Send + Sync>;
type CorsResult<T> = Result<T, CorsError>;

/// CORS (Cross-Origin Resource Sharing) plugin
pub struct Cors {
    name: String,
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
    /// subdomain-wildcard, and `allow_origins_by_regex` origins. Mirrors the
    /// `IpRestriction::create` fix (fr-types-01). Construction runs at plugin build
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

    /// Create from a shared `Arc<CorsConfig>` without cloning the underlying
    /// config. Used by the preflight hot path to construct an ad-hoc `Cors`
    /// per request without re-cloning the config.
    pub fn from_arc(config: Arc<CorsConfig>) -> Self {
        Cors {
            name: "Cors".to_string(),
            config,
        }
    }

    /// Check if this is a CORS preflight request
    fn is_preflight_request(&self, session: &mut dyn PluginSession) -> bool {
        // Preflight must be OPTIONS with Origin and Access-Control-Request-Method headers
        if session.method() != "OPTIONS" {
            return false;
        }

        session.header_value("origin").is_some() && session.header_value("access-control-request-method").is_some()
    }

    /// Select the value to emit in the Access-Control-Allow-Origin header for an
    /// already-allowed `origin`. Returns the literal `"*"` only when the configured
    /// `allow_origins` is the standard wildcard `"*"`; in every other case
    /// (force-wildcard `"**"`, single origin, list, or regex match) the request
    /// origin is echoed back. Separated from the allow/deny decision so the value
    /// choice can be unit-tested on its own.
    fn select_acao_value(&self, origin: String) -> String {
        if self.config.allow_origins == "*" {
            "*".to_string()
        } else {
            origin
        }
    }

    /// Get the validated origin (returns Some(value) if allowed, None if rejected).
    /// Orchestrates the allow/deny check (`CorsConfig::is_origin_allowed`) and the
    /// ACAO value selection ([`Cors::select_acao_value`]).
    fn get_validated_origin(&self, session: &mut dyn PluginSession) -> Option<String> {
        let origin = session.header_value("origin")?;

        if !self.config.is_origin_allowed(&origin) {
            return None;
        }

        Some(self.select_acao_value(origin))
    }

    /// Compute all CORS + timing + private-network headers as key-value pairs.
    /// This allows callers to apply headers to either a ResponseHeader (preflight)
    /// or via session queue (simple requests / preflight_continue).
    fn build_cors_header_pairs(
        &self,
        session: &mut dyn PluginSession,
        allowed_origin: &str,
        is_preflight: bool,
    ) -> Vec<(&'static str, String)> {
        let mut headers = Vec::new();

        // Access-Control-Allow-Origin (required)
        headers.push(("Access-Control-Allow-Origin", allowed_origin.to_string()));

        // Access-Control-Allow-Methods
        headers.push(("Access-Control-Allow-Methods", self.config.get_allow_methods()));

        // Access-Control-Allow-Headers
        let requested_headers = session.header_value("access-control-request-headers");
        headers.push((
            "Access-Control-Allow-Headers",
            self.config.get_allow_headers(requested_headers.as_deref()),
        ));

        // Access-Control-Expose-Headers
        let expose = self.config.get_expose_headers();
        if !expose.is_empty() {
            headers.push(("Access-Control-Expose-Headers", expose.to_string()));
        }

        // Access-Control-Allow-Credentials
        if self.config.allow_credentials {
            headers.push(("Access-Control-Allow-Credentials", "true".to_string()));
        }

        // Access-Control-Max-Age (preflight only)
        if is_preflight {
            if let Some(max_age) = self.config.max_age {
                headers.push(("Access-Control-Max-Age", max_age.to_string()));
            }
        }

        // Timing-Allow-Origin
        if let Some(origin) = session.header_value("origin") {
            if self.config.is_timing_origin_allowed(&origin) {
                let timing_value = if let Some(ref timing_origins) = self.config.timing_allow_origins {
                    if timing_origins == "**" {
                        origin.clone()
                    } else if timing_origins == "*" {
                        "*".to_string()
                    } else {
                        origin.clone()
                    }
                } else {
                    origin.clone()
                };
                headers.push(("Timing-Allow-Origin", timing_value));
            }
        }

        // Private Network Access
        if self.config.allow_private_network {
            if let Some(req_pn) = session.header_value("access-control-request-private-network") {
                if req_pn == "true" {
                    headers.push(("Access-Control-Allow-Private-Network", "true".to_string()));
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
            resp.insert_header(*name, value.as_str())
                .map_err(|e| Box::new(e) as CorsError)?;
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

    /// Handle normal CORS request (non-preflight)
    fn handle_normal_request(&self, session: &mut dyn PluginSession, allowed_origin: &str) -> CorsResult<()> {
        self.set_cors_headers_via_session(session, allowed_origin, false)
    }
}

#[async_trait]
impl RequestFilter for Cors {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Check if there's an Origin header (CORS indicator)
        if session.header_value("origin").is_none() {
            // No Origin header means same-origin request, skip CORS
            return PluginRunningResult::GoodNext;
        }

        // Validate origin
        let allowed_origin = match self.get_validated_origin(session) {
            Some(origin) => origin,
            None => {
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
            }
        };

        // Check if this is a preflight request
        if self.is_preflight_request(session) {
            plugin_log.push("cors preflight");
            if let Err(_e) = self.handle_preflight(session, &allowed_origin).await {
                plugin_log.push("origin match:deny");
                return PluginRunningResult::ErrTerminateRequest;
            }

            plugin_log.push("origin match:allow");

            // If preflight_continue is true, continue to upstream
            if self.config.preflight_continue {
                return PluginRunningResult::GoodNext;
            }

            // Preflight is complete, terminate request (don't proxy to upstream)
            return PluginRunningResult::ErrTerminateRequest;
        }

        // Normal CORS request - set response headers
        plugin_log.push("cors actual");
        if let Err(_e) = self.handle_normal_request(session, &allowed_origin) {
            plugin_log.push("origin match:deny");
        } else {
            plugin_log.push("origin match:allow");
        }

        // Continue processing
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;

    fn create_cors_config() -> CorsConfig {
        CorsConfig {
            allow_origins: "https://example.com".to_string(),
            allow_origins_by_regex: None,
            allow_methods: "GET,POST,PUT,DELETE".to_string(),
            allow_headers: "Content-Type,Authorization".to_string(),
            expose_headers: "".to_string(),
            max_age: Some(3600),
            allow_credentials: true,
            allow_private_network: false,
            preflight_continue: false,
            timing_allow_origins: None,
            timing_allow_origins_by_regex: None,
            origins_cache: None,
            compiled_origins_regex: None,
            compiled_timing_regex: None,
        }
    }

    #[tokio::test]
    async fn test_origin_not_allowed() {
        let config = create_cors_config();
        let cors = Cors::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Cors");

        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("origin"))
            .returning(|_| Some("https://evil.com".to_string()));

        let result = cors.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("origin match:deny"));
    }

    #[test]
    fn test_select_acao_value_standard_wildcard_returns_star() {
        let mut config = create_cors_config();
        config.allow_origins = "*".to_string();
        let cors = Cors::new(&config);

        assert_eq!(cors.select_acao_value("https://any.example.com".to_string()), "*");
    }

    #[test]
    fn test_select_acao_value_force_wildcard_echoes_origin() {
        let mut config = create_cors_config();
        config.allow_origins = "**".to_string();
        let cors = Cors::new(&config);

        assert_eq!(
            cors.select_acao_value("https://any.example.com".to_string()),
            "https://any.example.com"
        );
    }

    #[test]
    fn test_select_acao_value_specific_origin_echoes_origin() {
        let config = create_cors_config();
        let cors = Cors::new(&config);

        assert_eq!(
            cors.select_acao_value("https://example.com".to_string()),
            "https://example.com"
        );
    }

    /// Run a simple (non-preflight) GET through `Cors` with the given origin and
    /// return whether the plugin logged an allow decision. Mocks the response-header
    /// sinks so the allowed path can complete. Mirrors the deserialization shape:
    /// the caller must leave `compiled_origins_regex` / `origins_cache` as `None` so
    /// the test exercises `Cors::new`'s `validate_and_init` call, not a pre-built config.
    async fn origin_allowed_via_new(config: &CorsConfig, origin: &'static str) -> bool {
        let cors = Cors::new(config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Cors");

        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session.expect_header_value().returning(move |name| {
            if name == "origin" {
                Some(origin.to_string())
            } else {
                None
            }
        });
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));
        mock_session.expect_append_response_header().returning(|_, _| Ok(()));

        let _ = cors.run_request(&mut mock_session, &mut plugin_log).await;
        plugin_log.contains("origin match:allow")
    }

    // Regression for fr-types-02: `Cors::new` must call `validate_and_init` so the
    // `#[serde(skip)]` matchers are compiled. Each config below is built the way
    // deserialization yields it (compiled fields `None`); a config routed through
    // `CorsConfig::new` would pre-build the matchers and mask the bug.

    #[tokio::test]
    async fn test_new_compiles_regex_origin_allowed() {
        let mut config = create_cors_config();
        config.allow_credentials = false; // regex + credentials is rejected by validate_and_init
        config.allow_origins = "https://exact.example.com".to_string();
        config.allow_origins_by_regex = Some(vec![r"https://.*\.regex\.example\.com".to_string()]);

        assert!(
            origin_allowed_via_new(&config, "https://api.regex.example.com").await,
            "regex-matched origin should be allowed after Cors::new compiles allow_origins_by_regex"
        );
    }

    #[tokio::test]
    async fn test_new_compiles_comma_separated_list() {
        let mut config = create_cors_config();
        config.allow_credentials = false;
        config.allow_origins = "https://a.example.com,https://b.example.com".to_string();

        assert!(
            origin_allowed_via_new(&config, "https://b.example.com").await,
            "second origin in a comma-separated list should be allowed via origins_cache"
        );
    }

    #[tokio::test]
    async fn test_new_compiles_subdomain_wildcard() {
        let mut config = create_cors_config();
        config.allow_credentials = false;
        config.allow_origins = "*.example.com".to_string();

        assert!(
            origin_allowed_via_new(&config, "https://tenant.example.com").await,
            "subdomain-wildcard origin should be allowed via the compiled wildcard regex"
        );
    }

    #[tokio::test]
    async fn test_no_origin_header_skip() {
        let config = create_cors_config();
        let cors = Cors::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Cors");

        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("origin"))
            .returning(|_| None);

        let result = cors.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        // No origin header, should skip CORS processing (no logs)
    }
}
