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
