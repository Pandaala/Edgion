use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct DegradeThresholdTestSuite;

impl DegradeThresholdTestSuite {
    /// With a 50% degrade threshold and every backend marked unhealthy by a
    /// misconfigured health-check path, the gateway must ignore health filtering
    /// and keep routing to the (actually serving) backend — returning 200 rather
    /// than the 503 a gateway without degrade mode returns once the pool goes
    /// all-unhealthy.
    ///
    /// The sibling HealthCheck suite (`unhealthy_backend_filtered`) proves the
    /// same probe config returns 503 WITHOUT a degrade threshold, so here we only
    /// need to confirm that, well after the unhealthy transition would have
    /// happened, requests still succeed (and never flip to 503).
    fn test_degrade_mode_serves_all_unhealthy() -> TestCase {
        TestCase::new(
            "degrade_mode_serves_all_unhealthy",
            "Degrade threshold routes to all backends when the whole pool is unhealthy",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    // unhealthyThreshold=1, interval=1s. By ~4s (iteration 8 at
                    // 500ms) the backend is certainly marked unhealthy, so any
                    // 200 from that point on can only come from degrade mode, and a
                    // 503 means degrade mode did NOT engage.
                    const TRANSITION_DONE_ITER: usize = 8;
                    let mut observed_statuses = Vec::new();

                    for i in 0..16 {
                        match ctx
                            .http_client
                            .get(&url)
                            .header("Host", "panic.example.com")
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                observed_statuses.push(status);
                                if i >= TRANSITION_DONE_ITER {
                                    match status {
                                        200 => {
                                            return TestResult::passed_with_message(
                                                start.elapsed(),
                                                format!(
                                                    "Degrade mode served 200 after unhealthy transition, statuses: {:?}",
                                                    observed_statuses
                                                ),
                                            );
                                        }
                                        503 => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                format!(
                                                    "Expected 200 via degrade mode, got 503 (degrade did not engage), statuses: {:?}",
                                                    observed_statuses
                                                ),
                                            );
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request failed while waiting for degrade mode: {}", e),
                                );
                            }
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }

                    TestResult::failed(
                        start.elapsed(),
                        format!(
                            "Never observed a conclusive post-transition response, statuses: {:?}",
                            observed_statuses
                        ),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for DegradeThresholdTestSuite {
    fn name(&self) -> &str {
        "DegradeThreshold"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/DegradeThreshold")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_degrade_mode_serves_all_unhealthy()]
    }
}
