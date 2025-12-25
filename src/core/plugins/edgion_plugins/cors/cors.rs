use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

use crate::core::plugins::{Plugin, PluginSession, PluginLog};
use crate::types::filters::{PluginConf, PluginRunningResult, PluginRunningStage};
use crate::types::resources::edgion_plugins::CorsConfig;

type CorsError = Box<dyn std::error::Error + Send + Sync>;
type CorsResult<T> = Result<T, CorsError>;

/// CORS (Cross-Origin Resource Sharing) plugin
pub struct Cors {
    name: String,
    config: CorsConfig,
    stages: Vec<PluginRunningStage>,
}

impl Cors {
    /// Create from CorsConfig (for EdgionPlugin system)
    pub fn new(config: &CorsConfig) -> Self {
        let config = config.clone();

        // Log warnings for force wildcard usage
        if config.allow_origins == "**" {
            println!("CORS: Using '**' for allow_origins bypasses security checks!");
        }
        if config.allow_methods == "**" {
            println!("CORS: Using '**' for allow_methods bypasses security checks!");
        }
        if config.allow_headers == "**" {
            println!("CORS: Using '**' for allow_headers will reflect client headers!");
        }

        Cors {
            name: "Cors".to_string(),
            config,
            stages: vec![PluginRunningStage::Request],
        }
    }

    /// Check if this is a CORS preflight request
    fn is_preflight_request(&self, session: &mut dyn PluginSession) -> bool {
        // Preflight must be OPTIONS with Origin and Access-Control-Request-Method headers
        if session.method() != "OPTIONS" {
            return false;
        }

        session.header_value("origin").is_some()
            && session.header_value("access-control-request-method").is_some()
    }

    /// Get the validated origin (returns Some(origin) if allowed, None if rejected)
    fn get_validated_origin(&self, session: &mut dyn PluginSession) -> Option<String> {
        let origin = session.header_value("origin")?;

        // Check if origin is allowed
        if !self.config.is_origin_allowed(&origin) {
            println!("CORS: Origin '{}' not allowed", origin);
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

    /// Set CORS headers on the response
    fn set_cors_headers(
        &self,
        session: &mut dyn PluginSession,
        allowed_origin: &str,
    ) -> CorsResult<()> {
        // Access-Control-Allow-Origin (required)
        session.set_response_header("Access-Control-Allow-Origin", allowed_origin)?;

        // Access-Control-Allow-Methods
        let methods = self.config.get_allow_methods();
        session.set_response_header("Access-Control-Allow-Methods", &methods)?;

        // Access-Control-Allow-Headers
        let requested_headers = session.header_value("access-control-request-headers");
        let headers = self.config.get_allow_headers(requested_headers.as_deref());
        session.set_response_header("Access-Control-Allow-Headers", &headers)?;

        // Access-Control-Expose-Headers
        let expose = self.config.get_expose_headers();
        if !expose.is_empty() {
            session.set_response_header("Access-Control-Expose-Headers", expose)?;
        }

        // Access-Control-Allow-Credentials
        if self.config.allow_credentials {
            session.set_response_header("Access-Control-Allow-Credentials", "true")?;
        }

        // Access-Control-Max-Age (for preflight, only if configured)
        if self.is_preflight_request(session) {
            if let Some(max_age) = self.config.max_age {
                session.set_response_header(
                    "Access-Control-Max-Age",
                    &max_age.to_string(),
                )?;
            }
        }

        // Vary: Origin (append if not using wildcard)
        if self.config.should_add_vary_header() {
            session.append_response_header("Vary", "Origin")?;
        }

        Ok(())
    }

    /// Set Timing-Allow-Origin header if configured
    fn set_timing_headers(
        &self,
        session: &mut dyn PluginSession,
        origin: &str,
    ) -> CorsResult<()> {
        if self.config.is_timing_origin_allowed(origin) {
            // Determine what value to set
            let timing_value = if let Some(ref timing_origins) = self.config.timing_allow_origins {
                if timing_origins == "**" {
                    origin.to_string()
                } else if timing_origins == "*" {
                    "*".to_string()
                } else if timing_origins == origin {
                    origin.to_string()
                } else {
                    // Matched via regex, return the origin
                    origin.to_string()
                }
            } else {
                // Only regex configured
                origin.to_string()
            };

            session.set_response_header("Timing-Allow-Origin", &timing_value)?;
        }

        Ok(())
    }

    /// Handle preflight request
    async fn handle_preflight(
        &self,
        session: &mut dyn PluginSession,
        allowed_origin: &str,
    ) -> CorsResult<()> {
        // Set all CORS headers
        self.set_cors_headers(session, allowed_origin)?;

        // Check timing headers
        if let Some(origin) = session.header_value("origin") {
            self.set_timing_headers(session, &origin)?;
        }

        // Check for Private Network Access request
        if self.config.allow_private_network {
            if let Some(req_private_network) = session.header_value("access-control-request-private-network") {
                if req_private_network == "true" {
                    session.set_response_header("Access-Control-Allow-Private-Network", "true")?;
                }
            }
        }

        // If preflight_continue is true, don't send response here
        if self.config.preflight_continue {
            // Let the request continue to upstream
            return Ok(());
        }

        // Return 200 OK
        let resp = ResponseHeader::build(200, None)?;
        session.write_response_header(Box::new(resp), true).await?;

        // Send empty body
        session.write_response_body(Some(Bytes::new()), true).await?;

        Ok(())
    }

    /// Handle normal CORS request (non-preflight)
    fn handle_normal_request(
        &self,
        session: &mut dyn PluginSession,
        allowed_origin: &str,
    ) -> CorsResult<()> {
        // Set CORS headers
        self.set_cors_headers(session, allowed_origin)?;

        // Check timing headers
        if let Some(origin) = session.header_value("origin") {
            self.set_timing_headers(session, &origin)?;
        }

        Ok(())
    }
}

#[async_trait]
impl Plugin for Cors {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_async(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Only process in Request stage
        if stage != PluginRunningStage::Request {
            return PluginRunningResult::Nothing;
        }

        // Check if there's an Origin header (CORS indicator)
        let origin_header = match session.header_value("origin") {
            Some(origin) => origin,
            None => {
                // No Origin header means same-origin request, skip CORS
                return PluginRunningResult::GoodNext;
            }
        };

        // Validate origin
        let allowed_origin = match self.get_validated_origin(session) {
            Some(origin) => origin,
            None => {
                // Origin not allowed, don't set CORS headers (browser will block)
                plugin_log.add_plugin_log(&format!("Rejecting request from origin '{}'; ", origin_header));
                return PluginRunningResult::GoodNext;
            }
        };

        // Check if this is a preflight request
        if self.is_preflight_request(session) {
            plugin_log.add_plugin_log(&format!("Handling preflight request from '{}'; ", origin_header));

            if let Err(e) = self.handle_preflight(session, &allowed_origin).await {
                plugin_log.add_plugin_log(&format!("Failed to handle preflight: {}; ", e));
                return PluginRunningResult::ErrTerminateRequest;
            }

            // If preflight_continue is true, continue to upstream
            if self.config.preflight_continue {
                plugin_log.add_plugin_log("Forwarding preflight request to upstream; ");
                return PluginRunningResult::GoodNext;
            }

            // Preflight is complete, terminate request (don't proxy to upstream)
            return PluginRunningResult::ErrTerminateRequest;
        }

        // Normal CORS request
        plugin_log.add_plugin_log(&format!("Handling normal request from '{}'; ", origin_header));

        if let Err(e) = self.handle_normal_request(session, &allowed_origin) {
            plugin_log.add_plugin_log(&format!("Failed to set headers: {}; ", e));
        }

        // Continue processing
        PluginRunningResult::GoodNext
    }

    fn get_stages(&self) -> Vec<PluginRunningStage> {
        self.stages.clone()
    }

    fn check_schema(&self, _conf: &PluginConf) {
        // Schema validation is done in CorsConfig::new
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::MockPluginSession;

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
        mock_session
            .expect_set_response_header()
            .returning(|_, _| Ok(()));
        mock_session
            .expect_append_response_header()
            .returning(|_, _| Ok(()));
        mock_session
            .expect_write_response_header()
            .returning(|_, _| Ok(()));
        mock_session
            .expect_write_response_body()
            .returning(|_, _| Ok(()));

        let result = cors.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.log.as_ref().unwrap().contains("preflight"));
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
        mock_session
            .expect_set_response_header()
            .returning(|_, _| Ok(()));
        mock_session
            .expect_append_response_header()
            .returning(|_, _| Ok(()));

        let result = cors.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.log.as_ref().unwrap().contains("normal request"));
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

        let result = cors.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.log.as_ref().unwrap().contains("Rejecting"));
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

        let result = cors.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.log.is_none());
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
        mock_session
            .expect_set_response_header()
            .returning(|_, _| Ok(()));
        mock_session
            .expect_append_response_header()
            .returning(|_, _| Ok(()));

        let result = cors.run_async(PluginRunningStage::Request, &mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.log.as_ref().unwrap().contains("Forwarding preflight"));
    }
}