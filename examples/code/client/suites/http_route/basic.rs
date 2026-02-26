// HTTP Test suite
//
// Required config files (in examples/conf/):
// - EndpointSlice_edge_test-http.yaml         # HTTP backend service discovery
// - Service_edge_test-http.yaml               # HTTP service definition
// - httproute_default_example-route.yaml      # HTTP routing rules（Host: test.example.com）
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct HttpTestSuite;

impl HttpTestSuite {
    fn test_health_check() -> TestCase {
        TestCase::new("health_check", "Test HTTP health checkendpoint", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                let mut request = ctx.http_client.get(format!("{}/health", ctx.http_url()));
                if let Some(host) = &ctx.http_host {
                    request = request.header("Host", host);
                }

                match request.send().await {
                    Ok(response) => {
                        if response.status().is_success() {
                            TestResult::passed_with_message(start.elapsed(), "Health check OK".to_string())
                        } else {
                            TestResult::failed(start.elapsed(), format!("Unexpected status: {}", response.status()))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        })
    }

    fn test_echo_get() -> TestCase {
        TestCase::new("echo_get", "Test HTTP GET echo", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                let mut request = ctx.http_client.get(format!("{}/echo", ctx.http_url()));
                if let Some(host) = &ctx.http_host {
                    request = request.header("Host", host);
                }

                match request.send().await {
                    Ok(response) => match response.text().await {
                        Ok(body) => {
                            if body.contains("Server:") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Response does not contain expected content".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                    },
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        })
    }

    fn test_echo_post() -> TestCase {
        TestCase::new("echo_post", "Test HTTP POST echo", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let test_body = "Hello World";

                let mut request = ctx.http_client.post(format!("{}/echo", ctx.http_url())).body(test_body);
                if let Some(host) = &ctx.http_host {
                    request = request.header("Host", host);
                }

                match request.send().await {
                    Ok(response) => match response.text().await {
                        Ok(body) => {
                            if body.contains(test_body) {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Echo mismatch. Got: {}", body))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                    },
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        })
    }

    fn test_status_codes() -> TestCase {
        TestCase::new("status_codes", "Test custom status code return", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let test_codes = vec![200, 404, 500];

                for code in test_codes {
                    let mut request = ctx.http_client.get(format!("{}/status/{}", ctx.http_url(), code));
                    if let Some(host) = &ctx.http_host {
                        request = request.header("Host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            if response.status().as_u16() != code {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status {}, got {}", code, response.status()),
                                );
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), e.to_string()),
                    }
                }

                TestResult::passed_with_message(start.elapsed(), "All status codes returned correctly".to_string())
            })
        })
    }

    fn test_delay() -> TestCase {
        TestCase::new("delay", "delay", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let delay_seconds = 1;

                let mut request = ctx
                    .http_client
                    .get(format!("{}/delay/{}", ctx.http_url(), delay_seconds));
                if let Some(host) = &ctx.http_host {
                    request = request.header("Host", host);
                }

                match request.send().await {
                    Ok(response) => {
                        let elapsed = start.elapsed();
                        if response.status().is_success() && elapsed.as_secs() >= delay_seconds {
                            TestResult::passed_with_message(elapsed, format!("Delayed {}s as expected", delay_seconds))
                        } else {
                            TestResult::failed(elapsed, format!("Delay not working correctly. Elapsed: {:?}", elapsed))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        })
    }
}

#[async_trait]
impl TestSuite for HttpTestSuite {
    fn name(&self) -> &str {
        "HTTP"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_health_check(),
            Self::test_echo_get(),
            Self::test_echo_post(),
            Self::test_status_codes(),
            Self::test_delay(),
        ]
    }
}
