//! Single request entry and internal response companion for Guardrail.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use super::attached::{AttachedGuardrail, GuardrailSummary};
use super::response::{response_head_hold_limits, GuardrailResponseProcessor};
use crate::plugins::http::common::outbound::client::{ManagedOutboundClient, PreparedManagedHttp};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter, UpstreamResponse};
use crate::routes::http::proxy_http::content_type::{is_sse_content_type, response_is_identity_encoded};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

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
    fn build(input: &PreparedManagedHttp<'_>, identity: String) -> Result<Self, String> {
        let EdgionPlugin::Guardrail(config) = input.plugin() else {
            return Err("owning Guardrail configuration required".into());
        };
        if let Some(error) = config.validation_error() {
            return Err(error);
        }
        let client = input.build_client()?;
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

    pub(crate) fn create_attached(
        input: &PreparedManagedHttp<'_>,
        identity: String,
    ) -> Option<(Box<dyn RequestFilter>, Box<dyn UpstreamResponse>)> {
        Self::build(input, identity).ok().map(|plugin| {
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
    ) -> PluginRunningResult {
        if self.fail_open {
            session.ai().disable_semantic_guard();
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
        if session.ctx().has_unsupported_native_guard_plan() {
            log.push_err("responses-guard-unsupported");
            return crate::ai::protocols::error::ai_error_response(
                session,
                crate::ai::AiProxyError::UnsupportedParameter,
            );
        }
        if matches!(session.ctx().ai_state().and_then(|state| state.attempt_plan.as_ref()),
            Some(crate::ai::AiAttemptPlan::Ready(plan)) if !plan.requested_streaming)
        {
            log.push_err("guardrail-requires-stream");
            return crate::ai::protocols::error::ai_error_response(
                session,
                crate::ai::AiProxyError::UnsupportedParameter,
            );
        }
        let owner = Arc::new(AttachedGuardrail::new());
        if !session
            .ai()
            .install_attached_guardrail(self.identity.clone(), owner, self.head_limits)
        {
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

    async fn run_upstream_response(
        &self,
        session: &mut dyn PluginSession,
        _log: &mut PluginLog,
    ) -> PluginRunningResult {
        let Some(owner) = session.ai().attached_guardrail(&self.identity) else {
            return PluginRunningResult::GoodNext;
        };
        let status = session.get_response_status().unwrap_or(0);
        // Earlier response filters can rewrite a successful status while retaining
        // the model output. Only skip an error that also came from the provider.
        if (400..=599).contains(&status)
            && session
                .ctx()
                .get_current_upstream()
                .and_then(|upstream| upstream.status)
                .is_some_and(|original| (400..=599).contains(&original))
        {
            session.ai().disable_semantic_guard();
            owner.publish(GuardrailSummary {
                outcome: "skipped",
                reason: "upstream-error",
                ..GuardrailSummary::default()
            });
            return PluginRunningResult::GoodNext;
        }
        let content_type_ok = session
            .response_header_map()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(is_sse_content_type);
        let encoding_ok = response_is_identity_encoded(session.response_header_map());
        let canonical_ai = session
            .ctx()
            .ai_state()
            .is_some_and(|state| state.stream_processor.is_some());
        if !(200..300).contains(&status) || !content_type_ok || !encoding_ok || !canonical_ai {
            return self.precommit_failure(session, &owner, "response-not-applicable");
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
            return self.precommit_failure(session, &owner, "window-config");
        };
        if !session.ai().install_semantic_response_processor(Box::new(processor)) {
            return self.precommit_failure(session, &owner, "late-response-processor");
        }
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/guards/semantic/plugin_test.rs"]
mod tests;
