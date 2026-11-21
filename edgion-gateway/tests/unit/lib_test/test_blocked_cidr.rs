//! Process-wide operator blocked-CIDR fixture for Gateway unit tests.
//!
//! The shared address classifier consults one install-once list per process,
//! so every unit test that needs a blocked address goes through [`install`]
//! and uses [`BLOCKED_CIDR_TEST_IP`]. No other unit test may use that address.

/// TEST-NET-3 address the Gateway unit-test process treats as blocked.
pub(crate) const BLOCKED_CIDR_TEST_IP: &str = "203.0.113.200";

/// Install the shared test list exactly once for this test process.
pub(crate) fn install() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let matcher = edgion_common::egress::compile_blocked_cidrs(&["203.0.113.200/32"])
            .expect("valid test CIDR")
            .expect("non-empty test list");
        edgion_common::egress::install_blocked_cidrs(matcher).expect("first blocked CIDR installation");
    });
}
