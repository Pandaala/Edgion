use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct HealthCheckTestSuite;

impl HealthCheckTestSuite {
    fn test_healthy_backend_available() -> TestCase {
        TestCase::new(
            "healthy_backend_available",
            "Service-level active health check keeps healthy backend routable",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let mut last_status = None;
                    let mut last_error = None;

                    // Give health-check loop a short warm-up window after Gateway startup.
                    for _ in 0..8 {
                        match ctx
                            .http_client
                            .get(&url)
                            .header("Host", "hc-healthy.example.com")
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                last_status = Some(status);
                                if status == 200 {
                                    return TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Healthy backend request returned 200".to_string(),
                                    );
                                }
                            }
                            Err(e) => {
                                last_error = Some(e.to_string());
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }

                    TestResult::failed(
                        start.elapsed(),
                        format!(
                            "Expected healthy backend to return 200, last_status={:?}, last_error={:?}",
                            last_status, last_error
                        ),
                    )
                })
            },
        )
    }

    fn test_unhealthy_backend_filtered() -> TestCase {
        TestCase::new(
            "unhealthy_backend_filtered",
            "Service-level active health check marks backend unhealthy and selection returns 503",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let mut observed_statuses = Vec::new();

                    // unhealthyThreshold=1, interval=1s in YAML, poll for transition.
                    for _ in 0..12 {
                        match ctx
                            .http_client
                            .get(&url)
                            .header("Host", "hc-unhealthy.example.com")
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                observed_statuses.push(status);
                                if status == 503 {
                                    return TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!(
                                            "Unhealthy backend filtered, observed statuses: {:?}",
                                            observed_statuses
                                        ),
                                    );
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request failed while waiting for unhealthy transition: {}", e),
                                );
                            }
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }

                    TestResult::failed(
                        start.elapsed(),
                        format!(
                            "Expected 503 after unhealthy transition, observed statuses: {:?}",
                            observed_statuses
                        ),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for HealthCheckTestSuite {
    fn name(&self) -> &str {
        "HealthCheck"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/HealthCheck")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_healthy_backend_available(),
            Self::test_unhealthy_backend_filtered(),
        ]
    }
}
