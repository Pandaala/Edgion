// HTTPRoute/MultiPort test suite
//
// Validates per-port route isolation: routes bound to one listener port
// must NOT be reachable from a different listener port.
//
// Gateway config (in examples/test/conf/HTTPRoute/MultiPort/):
// - Gateway with two listeners: http-port-a (31300), http-port-b (31301)
// - HTTPRoute route-port-a → parentRef sectionName=http-port-a
// - HTTPRoute route-port-b → parentRef sectionName=http-port-b
//
// Expected behavior:
// - port-a.example.com/echo is reachable on port 31300, NOT on port 31301
// - port-b.example.com/echo is reachable on port 31301, NOT on port 31300

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

const PORT_A: u16 = 31300;
const PORT_B: u16 = 31301;

pub struct MultiPortTestSuite;

impl MultiPortTestSuite {
    fn test_port_a_reachable_on_correct_port() -> TestCase {
        TestCase::new(
            "port_a_reachable_on_correct_port",
            "route-port-a should be reachable on port 31300",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("http://{}:{}/echo", ctx.target_host, PORT_A);
                    match ctx
                        .http_client
                        .get(&url)
                        .header("Host", "port-a.example.com")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            if response.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "route-port-a reachable on port A".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Unexpected status on port A: {}", response.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Port A request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_port_b_reachable_on_correct_port() -> TestCase {
        TestCase::new(
            "port_b_reachable_on_correct_port",
            "route-port-b should be reachable on port 31301",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("http://{}:{}/echo", ctx.target_host, PORT_B);
                    match ctx
                        .http_client
                        .get(&url)
                        .header("Host", "port-b.example.com")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            if response.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "route-port-b reachable on port B".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Unexpected status on port B: {}", response.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Port B request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_port_a_not_reachable_on_wrong_port() -> TestCase {
        TestCase::new(
            "port_a_not_reachable_on_wrong_port",
            "route-port-a should NOT be reachable on port 31301 (isolation test)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("http://{}:{}/echo", ctx.target_host, PORT_B);
                    match ctx
                        .http_client
                        .get(&url)
                        .header("Host", "port-a.example.com")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Correctly got 404 on wrong port (isolation works)".to_string(),
                                )
                            } else if status >= 200 && status < 300 {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Route leaked to wrong port! Got {} instead of 404 — port isolation broken",
                                        status
                                    ),
                                )
                            } else {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got non-success status {} on wrong port (acceptable)", status),
                                )
                            }
                        }
                        Err(e) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Connection error on wrong port (acceptable): {}", e),
                        ),
                    }
                })
            },
        )
    }

    fn test_port_b_not_reachable_on_wrong_port() -> TestCase {
        TestCase::new(
            "port_b_not_reachable_on_wrong_port",
            "route-port-b should NOT be reachable on port 31300 (isolation test)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("http://{}:{}/echo", ctx.target_host, PORT_A);
                    match ctx
                        .http_client
                        .get(&url)
                        .header("Host", "port-b.example.com")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Correctly got 404 on wrong port (isolation works)".to_string(),
                                )
                            } else if status >= 200 && status < 300 {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Route leaked to wrong port! Got {} instead of 404 — port isolation broken",
                                        status
                                    ),
                                )
                            } else {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got non-success status {} on wrong port (acceptable)", status),
                                )
                            }
                        }
                        Err(e) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Connection error on wrong port (acceptable): {}", e),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for MultiPortTestSuite {
    fn name(&self) -> &str {
        "HTTPRoute/MultiPort"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_port_a_reachable_on_correct_port(),
            Self::test_port_b_reachable_on_correct_port(),
            Self::test_port_a_not_reachable_on_wrong_port(),
            Self::test_port_b_not_reachable_on_wrong_port(),
        ]
    }
}
