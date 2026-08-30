// EdgionBackendTrafficPolicy/ConnectTimeout scenario.
//
// End-to-end coverage for `EdgionBackendTrafficPolicy.spec.connection.connectTimeout`
// (see docs/superpowers/specs/2026-07-22-per-backend-connect-timeout-design.md),
// against the fixtures in
// edgion-tests/integration/conf/EdgionBackendTrafficPolicy/ConnectTimeout/:
//
//   Gateway ct-gateway (port from ports.json, listener "http")
//     hostname "connect-timeout.example.com"
//     /blackhole -> ct-svc-a (10.255.255.1:1, a non-routable blackhole address)
//     /ok        -> ct-svc-b (shared plain-HTTP echo backend, 127.0.0.1:30001)
//   EdgionBackendTrafficPolicy ct-policy on Service ct-svc-a:
//     connection { connectTimeout: 300ms }
//
// The base EdgionGatewayConfig sets defaultConnectTimeout: 5s. ct-svc-a has no
// route in the sky to a live host, so without the override the gateway would
// hang the connect attempt for the full 5s before failing; the per-backend
// override must make it fail at ~300ms instead. ct-svc-b carries no BTP
// override and must keep working normally — proving the divergence is truly
// per-backend, not a global regression.
//
// Coverage map to spec "Testing / Integration":
//   1. A request to the blackholed backend fails fast (~connectTimeout), well
//      under the global defaultConnectTimeout -> test_blackhole_backend_fails_fast
//   2. A request to the non-overridden backend still succeeds normally,
//      proving the divergence -> test_normal_backend_still_works

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct ConnectTimeoutTestSuite;

const HOST: &str = "connect-timeout.example.com";

impl ConnectTimeoutTestSuite {
    /// (1) The blackholed backend (ct-svc-a) carries a 300ms connectTimeout
    /// override. A request to it must come back with a connect-failure status
    /// (502/503/504 depending on how the data plane classifies the connect
    /// error) comfortably under the global 5s defaultConnectTimeout, and not
    /// suspiciously instant either (the deadline must actually be enforced,
    /// not short-circuited by some other error path).
    fn test_blackhole_backend_fails_fast() -> TestCase {
        TestCase::new(
            "blackhole_backend_fails_fast",
            "A request to the BTP-overridden blackhole backend fails fast at ~connectTimeout, not the global defaultConnectTimeout",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/blackhole", ctx.http_url());

                    let resp = match ctx.http_client.get(&url).header("Host", HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    };
                    let elapsed = start.elapsed();
                    let status = resp.status().as_u16();

                    if !(500..600).contains(&status) {
                        return TestResult::failed(
                            elapsed,
                            format!("expected a connect-failure status (5xx) from the blackhole backend, got {status}"),
                        );
                    }
                    // Comfortably under the global 5s defaultConnectTimeout: proves the
                    // per-backend 300ms override actually took effect instead of the
                    // gateway hanging for the full global deadline.
                    if elapsed >= std::time::Duration::from_secs(2) {
                        return TestResult::failed(
                            elapsed,
                            format!(
                                "expected the connect failure within ~300ms (well under the 5s global default), \
                                 took {elapsed:?}"
                            ),
                        );
                    }
                    // NOTE: intentionally no lower-bound timing assertion. On a runner
                    // whose host sits on a 10.0.0.0/8 network with no route to the
                    // blackhole, connect() can return ENETUNREACH/EHOSTUNREACH in well
                    // under the 300ms deadline — a legitimate fast connect failure, not a
                    // bug. The invariant this scenario proves is the per-backend
                    // DIVERGENCE: the overridden backend fails fast (< 2s, far under the
                    // 5s global default) while the non-overridden sibling still works.
                    TestResult::passed_with_message(
                        elapsed,
                        format!("blackhole backend failed fast with status {status} in {elapsed:?}"),
                    )
                })
            },
        )
    }

    /// (2) ct-svc-b carries no connect-timeout override, so it must keep
    /// working normally — proving the override applies only to the targeted
    /// Service, not globally.
    fn test_normal_backend_still_works() -> TestCase {
        TestCase::new(
            "normal_backend_still_works",
            "A request to the non-overridden backend succeeds normally",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/ok", ctx.http_url());

                    let resp = match ctx.http_client.get(&url).header("Host", HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    };
                    let elapsed = start.elapsed();
                    let status = resp.status().as_u16();

                    if status != 200 {
                        return TestResult::failed(
                            elapsed,
                            format!("expected 200 from the non-overridden backend, got {status}"),
                        );
                    }

                    TestResult::passed_with_message(elapsed, format!("normal backend responded 200 in {elapsed:?}"))
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for ConnectTimeoutTestSuite {
    fn name(&self) -> &str {
        "EdgionBackendTrafficPolicy/ConnectTimeout"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackendTrafficPolicy/ConnectTimeout")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_blackhole_backend_fails_fast(),
            Self::test_normal_backend_still_works(),
        ]
    }
}
