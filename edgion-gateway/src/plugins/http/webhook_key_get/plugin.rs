//! WebhookKeyGet plugin implementation
//!
//! Resolves webhook vars into request context once per request. Resolution
//! failures never reject the request: a configured `default` is written, and
//! with no default the var is left unwritten.

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::WebhookKeyGetConfig;

use crate::link_sys::providers::webhook::resolve_webhook_key;
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
            let resolved = resolve_webhook_key(
                session,
                &var.webhook_ref,
                None,
                var.path_override.as_deref(),
                &var.extract,
            )
            .await;
            let outcome = select_outcome(resolved.as_deref(), var.default.as_deref());
            apply_outcome(session, plugin_log, &var.name, outcome);
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::common::WebhookExtract;
    use edgion_resources::resources::edgion_plugins::WebhookVar;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn var(name: &str, webhook_ref: &str, default: Option<&str>) -> WebhookVar {
        WebhookVar {
            name: name.to_string(),
            webhook_ref: webhook_ref.to_string(),
            path_override: None,
            extract: WebhookExtract::BodyText,
            default: default.map(|s| s.to_string()),
        }
    }

    fn wire_ctx_store(mock: &mut MockPluginSession) -> Arc<Mutex<HashMap<String, String>>> {
        let store = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        mock.expect_set_ctx_var().returning({
            let store = store.clone();
            move |k, v| {
                store.lock().insert(k.to_string(), v.to_string());
                Ok(())
            }
        });
        mock.expect_get_ctx_var().returning({
            let store = store.clone();
            move |k| store.lock().get(k).cloned()
        });
        store
    }

    #[test]
    fn resolved_outcome_writes_ctx_var() {
        let mut session = MockPluginSession::new();
        wire_ctx_store(&mut session);
        let mut plugin_log = PluginLog::new("WebhookKeyGet");

        let outcome = select_outcome(Some("gold"), Some("free"));
        assert_eq!(outcome, ResolutionOutcome::Resolved("gold"));
        apply_outcome(&mut session, &mut plugin_log, "tenant", outcome);
        assert_eq!(session.get_ctx_var("tenant").as_deref(), Some("gold"));
    }

    #[test]
    fn missing_resolution_with_default_writes_default() {
        let mut session = MockPluginSession::new();
        wire_ctx_store(&mut session);
        let mut plugin_log = PluginLog::new("WebhookKeyGet");

        let outcome = select_outcome(None, Some("free"));
        assert_eq!(outcome, ResolutionOutcome::Default("free"));
        apply_outcome(&mut session, &mut plugin_log, "tenant", outcome);
        assert_eq!(session.get_ctx_var("tenant").as_deref(), Some("free"));
    }

    #[test]
    fn missing_resolution_without_default_leaves_var_unwritten() {
        let mut session = MockPluginSession::new();
        wire_ctx_store(&mut session);
        let mut plugin_log = PluginLog::new("WebhookKeyGet");

        let outcome = select_outcome(None, None);
        assert_eq!(outcome, ResolutionOutcome::Skip);
        apply_outcome(&mut session, &mut plugin_log, "tenant", outcome);
        assert_eq!(session.get_ctx_var("tenant"), None);
    }

    #[test]
    fn multiple_outcomes_apply_in_configuration_order() {
        let config = WebhookKeyGetConfig {
            vars: vec![var("first", "ns/one", None), var("second", "ns/two", None)],
        };
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut session = MockPluginSession::new();
        session.expect_set_ctx_var().returning({
            let writes = writes.clone();
            move |name, value| {
                writes.lock().push((name.to_string(), value.to_string()));
                Ok(())
            }
        });
        let mut plugin_log = PluginLog::new("WebhookKeyGet");

        for (var, resolved) in config.vars.iter().zip([Some("a"), Some("b")]) {
            let outcome = select_outcome(resolved, var.default.as_deref());
            apply_outcome(&mut session, &mut plugin_log, &var.name, outcome);
        }

        assert_eq!(
            *writes.lock(),
            vec![
                ("first".to_string(), "a".to_string()),
                ("second".to_string(), "b".to_string())
            ]
        );
    }
}
