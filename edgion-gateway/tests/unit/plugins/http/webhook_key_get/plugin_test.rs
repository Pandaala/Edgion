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
