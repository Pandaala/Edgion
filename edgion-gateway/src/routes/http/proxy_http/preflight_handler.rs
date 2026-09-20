//! Global preflight request handler
//! Intercepts and handles CORS preflight requests before plugin execution

use bytes::Bytes;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::sync::Arc;

use edgion_resources::resources::edgion_gateway_config::{PreflightMode, PreflightPolicy};
use edgion_resources::resources::CorsConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedPreflightMode {
    CorsStandard,
    AllOptions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedPreflightPolicy {
    pub(crate) mode: ResolvedPreflightMode,
    pub(crate) status_code: u16,
}

impl ResolvedPreflightPolicy {
    pub(crate) fn resolve(policy: Option<&PreflightPolicy>) -> Self {
        let Some(policy) = policy else {
            return Self::default();
        };
        Self {
            mode: match policy.mode {
                PreflightMode::CorsStandard => ResolvedPreflightMode::CorsStandard,
                PreflightMode::AllOptions => ResolvedPreflightMode::AllOptions,
            },
            status_code: policy.status_code,
        }
    }
}

impl Default for ResolvedPreflightPolicy {
    fn default() -> Self {
        Self {
            mode: ResolvedPreflightMode::CorsStandard,
            status_code: 204,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PreflightHandler;

impl PreflightHandler {
    /// Check if this is a preflight request based on configured mode
    pub(crate) fn is_preflight(&self, session: &Session, policy: ResolvedPreflightPolicy) -> bool {
        let method = session.req_header().method.as_str();
        if method != "OPTIONS" {
            return false;
        }

        match policy.mode {
            ResolvedPreflightMode::AllOptions => true,
            ResolvedPreflightMode::CorsStandard => {
                // CORS standard: must have Origin + Access-Control-Request-Method
                self.has_header(session, "origin") && self.has_header(session, "access-control-request-method")
            }
        }
    }

    /// Handle preflight request with CORS configuration
    ///
    /// Returns: Ok(true) if handled and request should be terminated
    pub(crate) async fn handle_preflight(
        &self,
        session: &mut Session,
        ctx: &mut crate::EdgionHttpContext,
        policy: ResolvedPreflightPolicy,
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
        let status = policy.status_code;
        if !edgion_resources::http_status::is_valid_final_http_status(status) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid preflight final status: {status}"),
            )
            .into());
        }
        let mut resp = ResponseHeader::build(status, None)?;
        ctx.request_info.trace_id.reconcile_response(&mut resp);
        if !ctx.claim_final_response(status) {
            return Ok(true);
        }

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
        session_pair_from_request(raw_request).await.0
    }

    async fn session_pair_from_request(raw_request: &[u8]) -> (Session, tokio::io::DuplexStream) {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(raw_request)
            .await
            .expect("write request into duplex pair");
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("client must not have closed before sending the request");
        (session, client)
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

    fn policy(mode: ResolvedPreflightMode, status_code: u16) -> ResolvedPreflightPolicy {
        ResolvedPreflightPolicy { mode, status_code }
    }

    #[tokio::test]
    async fn test_preflight_detection_cors_standard() {
        let handler = PreflightHandler;
        let policy = policy(ResolvedPreflightMode::CorsStandard, 204);
        let session = session_from_request(
            b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://example.com\r\nAccess-Control-Request-Method: POST\r\n\r\n",
        )
        .await;

        assert!(handler.is_preflight(&session, policy));
    }

    #[tokio::test]
    async fn test_preflight_continue_runs_route_cors_once() {
        let handler = PreflightHandler;
        let policy = policy(ResolvedPreflightMode::CorsStandard, 204);
        let mut session = session_from_request(
            b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://example.com\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        )
        .await;
        let mut ctx = EdgionHttpContext::default();
        let cors_config = cors_config(true);

        assert!(handler.is_preflight(&session, policy));
        let response_sent = handler
            .handle_preflight(&mut session, &mut ctx, policy, Some(&cors_config))
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
            .pending_response_header_ops()
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    PendingHeaderOp::Append { name, value }
                        if name.eq_ignore_ascii_case("vary") && value.as_bytes().eq_ignore_ascii_case(b"origin")
                )
            })
            .count();
        assert_eq!(vary_origin_count, 1);
    }

    #[tokio::test]
    async fn test_preflight_mode_all_options_without_origin_passes_through() {
        let handler = PreflightHandler;
        let policy = policy(ResolvedPreflightMode::AllOptions, 204);
        let mut session = session_from_request(b"OPTIONS / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();
        let cors_config = cors_config(false);

        assert!(handler.is_preflight(&session, policy));
        let response_sent = handler
            .handle_preflight(&mut session, &mut ctx, policy, Some(&cors_config))
            .await
            .expect("preflight handling should succeed");

        assert!(!response_sent);
    }

    #[tokio::test]
    async fn invalid_preflight_status_does_not_claim_response_ownership() {
        let handler = PreflightHandler;
        let policy = policy(ResolvedPreflightMode::AllOptions, 700);
        let mut session = session_from_request(b"OPTIONS / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();

        assert!(handler
            .handle_preflight(&mut session, &mut ctx, policy, None)
            .await
            .is_err());
        assert!(!ctx.response_write_started());
        assert_eq!(ctx.final_downstream_status(), None);
    }

    #[tokio::test]
    async fn no_cors_fallback_uses_pinned_status() {
        let handler = PreflightHandler;
        let policy = policy(ResolvedPreflightMode::AllOptions, 299);
        let (mut session, _client) = session_pair_from_request(b"OPTIONS / HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();

        assert!(handler.is_preflight(&session, policy));
        assert!(handler
            .handle_preflight(&mut session, &mut ctx, policy, None)
            .await
            .expect("preflight fallback should succeed"));
        assert!(ctx.response_write_started());
        assert_eq!(ctx.final_downstream_status(), Some(299));
    }
}
