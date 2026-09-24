use super::OtlpStartupGuard;
use crate::link_sys::providers::otlp::OtlpApply;
use crate::link_sys::runtime::runner::ApplyOwner;
use crate::link_sys::runtime::store::{otlp_client_count, otlp_retiring_client_count, seed_published_otlp};

#[test]
fn startup_error_drains_active_otlp_before_runtime_drop() {
    const CHILD: &str = "EDGION_OTLP_STARTUP_DRAIN_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cli::otlp_startup_tests::startup_error_drains_active_otlp_before_runtime_drop",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    // Process shutdown is terminal global state, so this runs in an isolated
    // child. The real SDK worker is created without emitting or opening IO.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let client = runtime.block_on(async {
        let resource = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1", "kind": "LinkSys",
            "metadata": {"namespace": "startup-test", "name": "collector"},
            "spec": {"type": "otlp", "config": {"endpoint": "http://collector.example.test:4317"}}
        }))
        .unwrap();
        let key = "startup-test/collector".to_string();
        let client = OtlpApply::build(&key, &resource).unwrap();
        client.publish_lazy_test_pipeline().unwrap();
        seed_published_otlp(key, client.clone());
        client
    });
    assert_eq!(otlp_client_count(), 1);
    let startup_result: Result<(), &'static str> = (|| {
        let _drain = OtlpStartupGuard {
            runtime: &runtime,
            armed: true,
        };
        Err("startup_failed")?
    })();
    assert_eq!(startup_result, Err("startup_failed"));
    assert_eq!(otlp_client_count(), 0);
    assert_eq!(otlp_retiring_client_count(), 0);
    assert!(!client.healthy());
    runtime.block_on(client.shutdown()).unwrap();
    drop(client);
    drop(runtime);
}
