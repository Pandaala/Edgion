use super::super::vm_manager::{tests::actual_runtime, CalloutGrant, WasmVmManager};
use super::*;
use edgion_resources::plugin_configs::WasmConfig;

const GUEST: &str = r#"(module
 (memory (export "memory") 1)
 (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
 (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
 (func (export "proxy_on_delete") (param i32)))"#;

#[tokio::test]
async fn terminal_and_cancellation_release_active_admission_once() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::request::tests",
        "terminal_and_cancellation_release_active_admission_once",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "terminal_and_cancellation_release_active_admission_once",
        || async {
            let file = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(file.path(), wat::parse_str(GUEST).unwrap()).unwrap();
            let mut config = WasmConfig::default();
            config.source.file = Some(file.path().to_string_lossy().into_owned());
            config.max_active_requests = 1;
            let manager = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
            let entry = runtime.get_or_load().await.unwrap();
            for cancel in [false, true] {
                let mut lease = entry.acquire(100).unwrap();
                lease.open_request(0, true).unwrap();
                let binding = WasmRequestBinding::new(Arc::new(()), lease);
                assert!(entry.acquire(100).is_err());
                let loan = binding.take().unwrap();
                if cancel {
                    drop(loan);
                    assert!(binding.take().is_none());
                    assert_eq!(entry.pool_len(), 0, "abandoned HTTP state is never pooled");
                } else {
                    loan.complete();
                    assert!(binding.finish());
                    assert!(binding.finish());
                    assert_eq!(entry.pool_len(), 1);
                }
                let lease = entry.acquire(100).unwrap();
                drop(lease);
            }
        },
    )
    .await;
}

#[tokio::test]
async fn closing_owner_while_loaned_prevents_resurrection() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::request::tests",
        "closing_owner_while_loaned_prevents_resurrection",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "closing_owner_while_loaned_prevents_resurrection",
        || async {
            let file = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(file.path(), wat::parse_str(GUEST).unwrap()).unwrap();
            let mut config = WasmConfig::default();
            config.source.file = Some(file.path().to_string_lossy().into_owned());
            let manager = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
            let entry = runtime.get_or_load().await.unwrap();
            let mut lease = entry.acquire(100).unwrap();
            lease.open_request(0, false).unwrap();
            let binding = WasmRequestBinding::new(Arc::new(()), lease);
            let loan = binding.take().unwrap();
            binding.abort();
            loan.complete();
            assert!(binding.take().is_none());
            assert_eq!(entry.pool_len(), 0);
        },
    )
    .await;
}

#[tokio::test]
async fn terminal_while_borrowed_and_deferred_done_discard_execution() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::request::tests",
        "terminal_while_borrowed_and_deferred_done_discard_execution",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "terminal_while_borrowed_and_deferred_done_discard_execution",
        || async {
            for deferred in [false, true] {
                let file = tempfile::NamedTempFile::new().unwrap();
                let guest = if deferred {
                    GUEST.replace("(result i32) (i32.const 1)", "(result i32) (i32.const 0)")
                } else {
                    GUEST.into()
                };
                std::fs::write(file.path(), wat::parse_str(guest).unwrap()).unwrap();
                let mut config = WasmConfig::default();
                config.source.file = Some(file.path().to_string_lossy().into_owned());
                let manager = WasmVmManager::new_for_test();
                let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
                let entry = runtime.get_or_load().await.unwrap();
                let mut lease = entry.acquire(100).unwrap();
                lease.open_request(0, true).unwrap();
                let binding = WasmRequestBinding::new(Arc::new(()), lease);
                let loan = if deferred { None } else { Some(binding.take().unwrap()) };
                assert!(!binding.finish());
                if let Some(loan) = loan {
                    loan.complete();
                }
                assert!(binding.take().is_none());
                assert_eq!(entry.pool_len(), 0);
            }
        },
    )
    .await;
}
