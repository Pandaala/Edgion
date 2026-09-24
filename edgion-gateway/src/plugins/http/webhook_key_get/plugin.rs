//! WebhookKeyGet plugin implementation
//!
//! Resolves webhook vars into request context once per request. Resolution
//! failures never reject the request: a configured `default` is written, and
//! with no default the var is left unwritten.

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::WebhookKeyGetConfig;

use crate::link_sys::providers::webhook::resolve_webhook_key;
use crate::plugins::http::common::outbound::request::OutboundRequestDiagnostics;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

#[derive(Debug, PartialEq, Eq)]
enum ResolutionOutcome<'a> {
    Resolved(&'a str),
    Default(&'a str),
    Skip,
}

fn select_outcome<'a>(resolved: Option<&'a str>, default: Option<&'a str>) -> ResolutionOutcome<'a> {
    match (resolved, default) {
        (Some(value), _) => ResolutionOutcome::Resolved(value),
        (None, Some(value)) => ResolutionOutcome::Default(value),
        (None, None) => ResolutionOutcome::Skip,
    }
}

fn apply_outcome(
    session: &mut dyn PluginSession,
    plugin_log: &mut PluginLog,
    name: &str,
    outcome: ResolutionOutcome<'_>,
) {
    let value = match outcome {
        ResolutionOutcome::Resolved(value) => {
            plugin_log.push_fmt("key {} set", la![name]);
            value
        }
        ResolutionOutcome::Default(value) => {
            plugin_log.push_fmt("key {} default", la![name]);
            value
        }
        ResolutionOutcome::Skip => {
            plugin_log.push_fmt("key {} skip", la![name]);
            return;
        }
    };

    if session.set_ctx_var(name, value).is_err() {
        plugin_log.push_err("ctx var set fail");
    }
}

/// WebhookKeyGet plugin: resolve webhook vars into request context.
pub struct WebhookKeyGet {
    name: String,
    config: WebhookKeyGetConfig,
}

impl WebhookKeyGet {
    /// Create a new WebhookKeyGet plugin from configuration.
    ///
    /// The production path always calls [`resolve_webhook_key`].
    pub fn create(config: &WebhookKeyGetConfig) -> Box<dyn RequestFilter> {
        let mut validated_config = config.clone();
        let _ = validated_config.validate_and_compile();
        Box::new(WebhookKeyGet {
            name: "WebhookKeyGet".to_string(),
            config: validated_config,
        })
    }
}

#[async_trait]
impl RequestFilter for WebhookKeyGet {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        for var in &self.config.vars {
            let mut diagnostics = OutboundRequestDiagnostics::default();
            let resolved = resolve_webhook_key(
                session,
                &var.webhook_ref,
                var.path_override.as_deref(),
                &var.extract,
                &mut diagnostics,
            )
            .await;
            diagnostics.emit(plugin_log);
            let outcome = select_outcome(resolved.as_deref(), var.default.as_deref());
            apply_outcome(session, plugin_log, &var.name, outcome);
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/webhook_key_get/plugin_test.rs"]
mod tests;
