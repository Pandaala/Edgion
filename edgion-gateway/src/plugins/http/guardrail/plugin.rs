//! Single request entry and internal response companion for Guardrail.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use super::attached::{AttachedGuardrail, GuardrailSummary};
use super::response::GuardrailResponseProcessor;
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter, UpstreamResponse};
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
        Ok(Self {
            identity,
            client: Arc::new(client),
            permits: Arc::new(Semaphore::new(config.max_concurrent)),
            window_bytes: config.window_bytes,
            overlap_bytes: config.overlap_bytes,
            max_request_bytes: config.max_request_bytes,
            max_replacement_bytes: config.max_replacement_bytes,
            fail_open: config.fail_open,
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
        let owner = Arc::new(AttachedGuardrail::new());
        if !session.install_attached_guardrail(self.identity.clone(), owner) {
            // Duplicate claims are structural ambiguity, not dependency
            // availability, and are never softened by either fail-open flag.
            if let Some((_, existing)) = session.ctx().attached_guardrail.as_ref() {
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
            .and_then(|value| value.split(';').next().map(str::trim).map(str::to_owned))
            .is_some_and(|value| value.eq_ignore_ascii_case("text/event-stream"));
        let encoding_ok = session
            .get_response_header("content-encoding")
            .is_none_or(|value| value.eq_ignore_ascii_case("identity"));
        let canonical_ai = session.ctx().ai_stream_processor.is_some();
        if !(200..300).contains(&status) || !content_type_ok || !encoding_ok || !canonical_ai {
            return self.precommit_failure(session, &owner, "response-not-applicable", log);
        }
        let processor = match GuardrailResponseProcessor::new(
            Arc::clone(&self.client),
            Arc::clone(&self.permits),
            Arc::clone(&owner),
            self.window_bytes,
            self.overlap_bytes,
            self.max_request_bytes,
            self.max_replacement_bytes,
            self.fail_open,
        ) {
            Ok(processor) => processor,
            Err(_) => return self.precommit_failure(session, &owner, "window-config", log),
        };
        if !session.install_semantic_response_processor(Box::new(processor)) {
            return self.precommit_failure(session, &owner, "late-response-processor", log);
        }
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> GuardrailConfig {
        serde_yaml::from_str(
            r#"
target:
  url: https://guard.example
timeoutMs: 1000
maxResponseBytes: 4096
request:
  path:
    template: /v1/decision
windowBytes: 16
maxConcurrent: 2
"#,
        )
        .unwrap()
    }

    #[test]
    fn one_snapshot_clone_reuses_client_pool_and_semaphore() {
        let snapshot = Guardrail::build(&config(), "test/requestPlugins/#0".to_string()).unwrap();
        let companion = snapshot.clone();
        assert!(Arc::ptr_eq(&snapshot.client, &companion.client));
        assert!(Arc::ptr_eq(&snapshot.permits, &companion.permits));

        let reloaded = Guardrail::build(&config(), "test/requestPlugins/#0".to_string()).unwrap();
        assert!(!Arc::ptr_eq(&snapshot.client, &reloaded.client));
        assert!(!Arc::ptr_eq(&snapshot.permits, &reloaded.permits));
    }
}
