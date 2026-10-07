//! Single request entry and internal response companion for Guardrail.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use super::attached::{AttachedGuardrail, GuardrailSummary};
use super::response::{response_head_hold_limits, GuardrailResponseProcessor};
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter, UpstreamResponse};
use crate::routes::http::proxy_http::content_type::{is_sse_content_type, response_is_identity_encoded};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::common::MethodSpec;
use edgion_resources::resources::edgion_plugins::GuardrailConfig;

const PLUGIN_NAME: &str = "Guardrail";

#[derive(Clone)]
pub struct Guardrail {
    identity: String,
    client: Arc<ManagedOutboundClient>,
    permits: Arc<Semaphore>,
    window_bytes: usize,
    overlap_bytes: usize,
    max_request_bytes: usize,
    max_replacement_bytes: usize,
    fail_open: bool,
    head_limits: Option<pingora_proxy::ResponseHeadHoldLimits>,
}

impl Guardrail {
    fn build(config: &GuardrailConfig, identity: String) -> Result<Self, String> {
        if let Some(error) = config.validation_error() {
            return Err(error);
        }
        let mut request = config.request.clone();
        // The operator cannot configure the method; the compiled internal
        // request is always the non-idempotent decision POST.
        request.method = Some(MethodSpec {
            template: "POST".to_string(),
        });
        let client = ManagedOutboundClient::from_parts(
            &config.conn,
            &request,
            config.resolved_secrets.as_ref(),
            "Guardrail decision service",
            false,
        )?;
        let head_limits = config
            .hold_first_window
            .then(|| response_head_hold_limits(std::time::Duration::from_millis(config.head_hold_timeout_ms)));
        Ok(Self {
            identity,
            client: Arc::new(client),
            permits: Arc::new(Semaphore::new(config.max_concurrent)),
            window_bytes: config.window_bytes,
            overlap_bytes: config.overlap_bytes,
            max_request_bytes: config.max_request_bytes,
            max_replacement_bytes: config.max_replacement_bytes,
            fail_open: config.fail_open,
            head_limits,
        })
    }

    pub fn create_attached(
        config: &GuardrailConfig,
        identity: String,
    ) -> Option<(Box<dyn RequestFilter>, Box<dyn UpstreamResponse>)> {
        Self::build(config, identity).ok().map(|plugin| {
            let companion = plugin.clone();
            (
                Box::new(plugin) as Box<dyn RequestFilter>,
                Box::new(companion) as Box<dyn UpstreamResponse>,
            )
        })
    }

    fn precommit_failure(
        &self,
        session: &mut dyn PluginSession,
        owner: &AttachedGuardrail,
        reason: &'static str,
        _log: &mut PluginLog,
    ) -> PluginRunningResult {
        if self.fail_open {
            session.disable_semantic_guard();
            owner.publish(GuardrailSummary {
                outcome: "fail-open",
                reason,
                ..GuardrailSummary::default()
            });
            PluginRunningResult::GoodNext
        } else {
            owner.publish(GuardrailSummary {
                outcome: "fail-close",
                reason,
                ..GuardrailSummary::default()
            });
            PluginRunningResult::ErrResponse {
                status: 503,
                body: None,
            }
        }
    }
}

#[async_trait]
impl RequestFilter for Guardrail {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if session.ctx().has_responses_plan() {
            log.push_err("responses-guard-unsupported");
            return crate::ai::openai_error_response(session, crate::ai::AiProxyError::UnsupportedParameter);
        }
        let owner = Arc::new(AttachedGuardrail::new());
        if !session.install_attached_guardrail(self.identity.clone(), owner, self.head_limits) {
            // Duplicate claims are structural ambiguity, not dependency
            // availability, and are never softened by either fail-open flag.
            if let Some((_, existing)) = session
                .ctx()
                .ai_state()
                .and_then(|state| state.attached_guardrail.as_ref())
            {
                existing.publish(GuardrailSummary {
                    outcome: "fail-close",
                    reason: "duplicate-claim",
                    ..GuardrailSummary::default()
                });
            }
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        }
        log.set_internal_token(self.identity.clone());
        PluginRunningResult::GoodNext
    }
}

#[async_trait]
impl UpstreamResponse for Guardrail {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    fn suppress_automatic_log(&self) -> bool {
        true
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        let Some(owner) = session.attached_guardrail(&self.identity) else {
            return PluginRunningResult::GoodNext;
        };
        let status = session.get_response_status().unwrap_or(0);
        let content_type_ok = session
            .get_response_header("content-type")
            .as_deref()
            .is_some_and(is_sse_content_type);
        let encoding_ok = response_is_identity_encoded(session.response_header_map());
        let canonical_ai = session
            .ctx()
            .ai_state()
            .is_some_and(|state| state.stream_processor.is_some());
        if !(200..300).contains(&status) || !content_type_ok || !encoding_ok || !canonical_ai {
            return self.precommit_failure(session, &owner, "response-not-applicable", log);
        }
        let Ok(processor) = GuardrailResponseProcessor::new(
            Arc::clone(&self.client),
            Arc::clone(&self.permits),
            Arc::clone(&owner),
            self.window_bytes,
            self.overlap_bytes,
            self.max_request_bytes,
            self.max_replacement_bytes,
            self.fail_open,
            self.head_limits.is_some(),
        ) else {
            return self.precommit_failure(session, &owner, "window-config", log);
        };
        if !session.install_semantic_response_processor(Box::new(processor)) {
            return self.precommit_failure(session, &owner, "late-response-processor", log);
        }
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/guards/semantic/plugin_test.rs"]
mod tests;
