//! Global preflight request handler
//! Intercepts and handles CORS preflight requests before plugin execution

use bytes::Bytes;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::sync::Arc;

use edgion_resources::resources::edgion_gateway_config::{PreflightMode, PreflightPolicy};
use edgion_resources::resources::CorsConfig;

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
        ctx: &mut crate::EdgionHttpContext,
        cors_config: Option<&Arc<CorsConfig>>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // A route-level CORS plugin owns both local preflight responses and
        // pass-through response headers. Let the normal plugin pipeline run it
        // exactly once so queued operations such as `Vary: Origin` are not
        // duplicated.
        if cors_config.is_some() {
            return Ok(false);
        }

        // No CORS config: return simple empty response
        let status = self.policy.as_ref().map(|p| p.status_code).unwrap_or(204);
        if !ctx.claim_final_response(status) {
            return Ok(true);
        }

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
    use crate::ctx::PendingHeaderOp;
    use crate::plugins::http::cors::Cors;
    use crate::plugins::runtime::log::PluginLog;
    use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
    use crate::plugins::runtime::traits::request_filter::RequestFilter;
    use crate::EdgionHttpContext;
    use edgion_resources::filters::PluginRunningResult;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    async fn session_from_request(raw_request: &[u8]) -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(raw_request)
            .await
            .expect("write request into duplex pair");
        drop(client);
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("client must not have closed before sending the request");
        session
    }

    fn cors_config(preflight_continue: bool) -> Arc<CorsConfig> {
        let mut config = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            preflight_continue,
            ..CorsConfig::default()
        };
        config.validate_and_init().expect("CORS test config is valid");
        Arc::new(config)
    }

    #[tokio::test]
    async fn test_preflight_detection_cors_standard() {
        let handler = PreflightHandler::new(Some(PreflightPolicy {
            mode: PreflightMode::CorsStandard,
            status_code: 204,
        }));
        let session = session_from_request(
            b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://example.com\r\nAccess-Control-Request-Method: POST\r\n\r\n",
        )
        .await;

        assert!(handler.is_preflight(&session));
    }

    #[tokio::test]
    async fn test_preflight_continue_runs_route_cors_once() {
        let handler = PreflightHandler::new(Some(PreflightPolicy {
            mode: PreflightMode::CorsStandard,
            status_code: 204,
        }));
        let mut session = session_from_request(
            b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://example.com\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        )
        .await;
        let mut ctx = EdgionHttpContext::default();
        let cors_config = cors_config(true);

        assert!(handler.is_preflight(&session));
        let response_sent = handler
            .handle_preflight(&mut session, &mut ctx, Some(&cors_config))
            .await
            .expect("preflight handling should succeed");

        assert!(!response_sent);

        let cors = Cors::from_arc(Arc::clone(&cors_config));
        let result = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            let mut plugin_log = PluginLog::new("Cors");
            cors.run_request(&mut adapter, &mut plugin_log).await
        };

        assert_eq!(result, PluginRunningResult::GoodNext);
        let vary_origin_count = ctx
            .pending_response_header_ops
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    PendingHeaderOp::Append { name, value }
                        if name.eq_ignore_ascii_case("vary") && value.eq_ignore_ascii_case("origin")
                )
            })
            .count();
        assert_eq!(vary_origin_count, 1);
    }

    #[tokio::test]
    async fn test_preflight_mode_all_options_without_origin_passes_through() {
        let handler = PreflightHandler::new(Some(PreflightPolicy {
            mode: PreflightMode::AllOptions,
            status_code: 204,
        }));
        let mut session = session_from_request(b"OPTIONS / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();
        let cors_config = cors_config(false);

        assert!(handler.is_preflight(&session));
        let response_sent = handler
            .handle_preflight(&mut session, &mut ctx, Some(&cors_config))
            .await
            .expect("preflight handling should succeed");

        assert!(!response_sent);
    }
}
