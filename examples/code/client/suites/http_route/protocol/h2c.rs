use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct H2cTestSuite;

impl H2cTestSuite {
    fn test_h2c_backend_protocol() -> TestCase {
        TestCase::new(
            "h2c_backend_protocol",
            "Test HTTPRoute backend appProtocol=h2c uses HTTP/2 upstream",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .http2_prior_knowledge()
                        .no_proxy()
                        .build()
                        .unwrap();

                    let url = format!("{}/version", ctx.http_url());

                    let response = client.get(&url).send().await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status();
                            let body = match resp.text().await {
                                Ok(body) => body,
                                Err(err) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Failed to read response body: {}", err),
                                    );
                                }
                            };

                            if !status.is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected success status, got {} with body: {}", status, body),
                                );
                            }

                            if body.contains("Version: HTTP/2.0") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Verified backend received H2C upstream request: {}", body.trim()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected backend to observe HTTP/2.0 upstream request, got body: {}",
                                        body.trim()
                                    ),
                                )
                            }
                        }
                        Err(err) => TestResult::failed(start.elapsed(), format!("Request failed: {}", err)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for H2cTestSuite {
    fn name(&self) -> &str {
        "H2C"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_h2c_backend_protocol()]
    }
}
