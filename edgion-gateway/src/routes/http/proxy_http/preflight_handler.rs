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
#[path = "../../../../tests/unit/routes/http/proxy_http/preflight_handler_test.rs"]
mod tests;
