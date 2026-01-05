//! Global preflight request handler
//! Intercepts and handles CORS preflight requests before plugin execution

use bytes::Bytes;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::sync::Arc;

use crate::core::plugins::edgion_plugins::cors::Cors;
use crate::core::plugins::plugin_runtime::log::PluginLog;
use crate::core::plugins::plugin_runtime::session_adapter::PingoraSessionAdapter;
use crate::core::plugins::plugin_runtime::traits::request_filter::RequestFilter;
use crate::types::resources::edgion_gateway_config::{PreflightMode, PreflightPolicy};
use crate::types::resources::CorsConfig;
use crate::types::EdgionHttpContext;

pub struct PreflightHandler {
    policy: Option<PreflightPolicy>,
}

impl PreflightHandler {
    pub fn new(policy: Option<PreflightPolicy>) -> Self {
        Self { policy }
    }

    /// Check if this is a preflight request based on configured mode
    pub fn is_preflight(&self, session: &Session) -> bool {
        let method = session.req_header().method.as_str();
        if method != "OPTIONS" {
            return false;
        }

        let mode = self
            .policy
            .as_ref()
            .map(|p| &p.mode)
            .unwrap_or(&PreflightMode::CorsStandard);

        match mode {
            PreflightMode::AllOptions => true,
            PreflightMode::CorsStandard => {
                // CORS standard: must have Origin + Access-Control-Request-Method
                self.has_header(session, "origin") && self.has_header(session, "access-control-request-method")
            }
        }
    }

    /// Handle preflight request with CORS configuration
    ///
    /// Returns: Ok(true) if handled and request should be terminated
    pub async fn handle_preflight(
        &self,
        session: &mut Session,
        ctx: &mut EdgionHttpContext,
        cors_config: Option<&Arc<CorsConfig>>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // If CORS config exists, apply CORS headers and return success
        if let Some(cors_config) = cors_config {
            // Create CORS plugin instance to handle preflight
            let cors_plugin = Cors::new(cors_config);
            let mut plugin_log = PluginLog::new("PreflightHandler");

            // Wrap pingora session in adapter
            let mut adapter = PingoraSessionAdapter::new(session, ctx);

            // Run CORS plugin logic (this will set headers and may terminate request)
            let _result = cors_plugin.run_request(&mut adapter, &mut plugin_log).await;

            // CORS plugin handles preflight, request is terminated
            tracing::debug!("Preflight handled by CORS plugin");
            return Ok(true);
        }

        // No CORS config: return simple empty response
        let status = self.policy.as_ref().map(|p| p.status_code).unwrap_or(204);

        tracing::debug!(status = status, "Preflight handled without CORS config");

        let resp = ResponseHeader::build(status, None)?;
        session.write_response_header(Box::new(resp), true).await?;
        session.write_response_body(Some(Bytes::new()), true).await?;

        Ok(true) // Terminate request
    }

    fn has_header(&self, session: &Session, name: &str) -> bool {
        session.req_header().headers.get(name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pingora_http::RequestHeader;

    #[test]
    fn test_preflight_detection_cors_standard() {
        let handler = PreflightHandler::new(Some(PreflightPolicy {
            mode: PreflightMode::CorsStandard,
            status_code: 204,
        }));

        // Create a mock session with OPTIONS + Origin + Access-Control-Request-Method
        let mut req_header = RequestHeader::build("OPTIONS", b"/", None).unwrap();
        req_header.insert_header("Origin", "https://example.com").unwrap();
        req_header
            .insert_header("Access-Control-Request-Method", "POST")
            .unwrap();

        // TODO: Need to create a proper Session for testing
        // For now, this test is a placeholder
    }

    #[test]
    fn test_preflight_mode_all_options() {
        let handler = PreflightHandler::new(Some(PreflightPolicy {
            mode: PreflightMode::AllOptions,
            status_code: 204,
        }));

        // All OPTIONS requests should be detected as preflight
        // Test logic would go here once we have proper Session mocking
    }
}
