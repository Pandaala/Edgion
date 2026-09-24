use super::TlsOffloadBudget;
use crate::routes::common::resolve_gateway_stream_plugin_store_key;
use pingora_core::server::configuration::ServerConf;

#[test]
fn tls_offload_budget_enforces_process_thread_limit() {
    let conf = ServerConf {
        downstream_tls_offload_threadpools: Some(2),
        downstream_tls_offload_thread_per_pool: Some(2),
        ..Default::default()
    };
    let mut budget = TlsOffloadBudget::new(&conf).unwrap();
    for _ in 0..16 {
        budget.reserve("app/gateway", "https").unwrap();
    }
    let err = budget.reserve("app/gateway", "tls-tcp").unwrap_err();
    assert!(err.to_string().contains("68 threads for 17 terminating listeners"));
    assert_eq!(budget.acceptors, 16);

    let mut disabled = TlsOffloadBudget::new(&ServerConf::default()).unwrap();
    disabled.reserve("app/gateway", "https").unwrap();
    assert_eq!(disabled.acceptors, 0);
}

#[test]
fn gateway_stream_plugin_store_key_uses_gateway_namespace() {
    assert_eq!(
        resolve_gateway_stream_plugin_store_key("  policy  ", "app").unwrap(),
        Some("app/policy".to_string())
    );
}

#[test]
fn gateway_stream_plugin_store_key_rejects_qualified_name() {
    assert!(resolve_gateway_stream_plugin_store_key("shared/policy", "app").is_err());
}
