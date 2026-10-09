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
    let snapshot = build_owned_guardrail();
    let companion = snapshot.clone();
    assert!(Arc::ptr_eq(&snapshot.client, &companion.client));
    assert!(Arc::ptr_eq(&snapshot.permits, &companion.permits));

    let reloaded = build_owned_guardrail();
    assert!(!Arc::ptr_eq(&snapshot.client, &reloaded.client));
    assert!(!Arc::ptr_eq(&snapshot.permits, &reloaded.permits));
}

fn build_owned_guardrail() -> Guardrail {
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"guard","namespace":"tenant"},"spec":{"requestPlugins":[{"type":"Guardrail","config":config()}]}
    })).unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let field = entry.managed_http_config().unwrap().unwrap();
    let defaults = edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot::from_configs([]);
    let input = PreparedManagedHttp::new(&field, &defaults).unwrap();
    Guardrail::build(&input, "test/requestPlugins/#0".to_string()).unwrap()
}

use edgion_resources::resources::edgion_plugins::GuardrailConfig;
