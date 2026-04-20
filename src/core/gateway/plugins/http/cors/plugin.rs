use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::CorsConfig;

type CorsError = Box<dyn std::error::Error + Send + Sync>;
type CorsResult<T> = Result<T, CorsError>;

/// CORS (Cross-Origin Resource Sharing) plugin
pub struct Cors {
    name: String,
    config: CorsConfig,
}

impl Cors {
    /// Create from CorsConfig (for EdgionPlugin system)
    pub fn new(config: &CorsConfig) -> Self {
        let config = config.clone();

        // Log warnings for force wildcard usage
        if config.allow_origins == "**" {
            tracing::warn!(
                component = "cors",
                "Using '**' for allow_origins bypasses security checks"
            );
        }
        if config.allow_methods == "**" {
            tracing::warn!(
                component = "cors",
                "Using '**' for allow_methods bypasses security checks"
            );
        }
        if config.allow_headers == "**" {
            tracing::warn!(
                component = "cors",
                "Using '**' for allow_headers will reflect client headers"
            );
        }

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

    /// Get the validated origin (returns Some(origin) if allowed, None if rejected)
    fn get_validated_origin(&self, session: &mut dyn PluginSession) -> Option<String> {
        let origin = session.header_value("origin")?;

        // Check if origin is allowed
        if !self.config.is_origin_allowed(&origin) {
            return None;
        }

        // Determine what to return based on config
        if self.config.allow_origins == "**" {
            // Force wildcard: return the actual origin
            Some(origin)
        } else if self.config.allow_origins == "*" {
            // Standard wildcard
            Some("*".to_string())
        } else {
            // Specific origin or matched from list/regex
            Some(origin)
        }
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
    async fn test_preflight_request_success() {
        let config = create_cors_config();
        let cors = Cors::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Cors");

        mock_session.expect_method().returning(|| "OPTIONS".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("origin"))
            .returning(|_| Some("https://example.com".to_string()));
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("access-control-request-method"))
            .returning(|_| Some("POST".to_string()));
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("access-control-request-headers"))
            .returning(|_| None);
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("access-control-request-private-network"))
            .returning(|_| None);
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));
        mock_session.expect_append_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = cors.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("cors preflight"));
        assert!(plugin_log.contains("origin match:allow"));
    }

    #[tokio::test]
    async fn test_normal_cors_request() {
        let config = create_cors_config();
        let cors = Cors::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Cors");

        mock_session.expect_method().returning(|| "GET".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("origin"))
            .returning(|_| Some("https://example.com".to_string()));
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("access-control-request-method"))
            .returning(|_| None);
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("access-control-request-headers"))
            .returning(|_| None);
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));
        mock_session.expect_append_response_header().returning(|_, _| Ok(()));

        let result = cors.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("cors actual"));
        assert!(plugin_log.contains("origin match:allow"));
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

    #[tokio::test]
    async fn test_preflight_continue() {
        let mut config = create_cors_config();
        config.preflight_continue = true;
        let cors = Cors::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("Cors");

        mock_session.expect_method().returning(|| "OPTIONS".to_string());
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("origin"))
            .returning(|_| Some("https://example.com".to_string()));
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("access-control-request-method"))
            .returning(|_| Some("POST".to_string()));
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("access-control-request-headers"))
            .returning(|_| None);
        mock_session
            .expect_header_value()
            .with(mockall::predicate::eq("access-control-request-private-network"))
            .returning(|_| None);
        mock_session.expect_set_response_header().returning(|_, _| Ok(()));
        mock_session.expect_append_response_header().returning(|_, _| Ok(()));

        let result = cors.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("cors preflight"));
        assert!(plugin_log.contains("origin match:allow"));
    }
}
