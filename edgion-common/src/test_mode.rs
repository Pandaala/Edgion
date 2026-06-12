//! Process-wide test-mode flags shared by both planes.
//!
//! These are simple `AtomicBool` toggles (NOT configuration): the Controller
//! test-mode flag and the Gateway integration-testing-mode flag. They live in
//! `common` because both binaries flip and read them at startup.

/// Global test mode flag (Controller)
/// When enabled:
/// - metrics test_key/test_type annotations are processed
static GLOBAL_TEST_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Global integration testing mode flag (Gateway)
/// When enabled via --integration-testing-mode:
/// - Access Log Store: stores complete access logs in memory, queryable via Admin API
/// - Metrics Test Data: collects test data labels on backend_requests_total metric
///   This flag MUST NEVER be used in production.
static GLOBAL_INTEGRATION_TESTING_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Initialize global test mode (called once at startup)
pub fn init_global_test_mode(enabled: bool) {
    GLOBAL_TEST_MODE.store(enabled, std::sync::atomic::Ordering::SeqCst);
    if enabled {
        tracing::info!(
            component = "test_mode",
            "Test mode enabled: metrics test features active"
        );
    }
}

/// Check if test mode is enabled
pub fn is_test_mode() -> bool {
    GLOBAL_TEST_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Initialize global integration testing mode (called once at gateway startup)
pub fn init_integration_testing_mode(enabled: bool) {
    GLOBAL_INTEGRATION_TESTING_MODE.store(enabled, std::sync::atomic::Ordering::SeqCst);
    if enabled {
        tracing::warn!(
            component = "integration_testing",
            "Integration testing mode enabled: Access Log Store + Metrics Test Data active. \
             DO NOT enable in production — `edgion_backend_requests_total` will carry \
             per-request `test_data` labels (e.g., backend IP, latency_ms) when Gateway \
             annotations opt in, causing Prometheus cardinality to scale with request count."
        );
    }
}

/// Check if integration testing mode is enabled
#[inline]
pub fn is_integration_testing_mode() -> bool {
    GLOBAL_INTEGRATION_TESTING_MODE.load(std::sync::atomic::Ordering::Relaxed)
}
