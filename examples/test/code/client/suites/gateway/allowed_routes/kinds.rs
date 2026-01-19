// AllowedRoutes Kinds test suite
//
// Required config files:
// - Gateway/AllowedRoutes/Kinds/Gateway.yaml         # Gateway with allowedRoutes.kinds: [HTTPRoute]
// - Gateway/AllowedRoutes/Kinds/HTTPRoute.yaml       # HTTPRoute (allowed)
// - Gateway/AllowedRoutes/Kinds/GRPCRoute.yaml       # GRPCRoute (denied)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct AllowedRoutesKindsTestSuite;

impl AllowedRoutesKindsTestSuite {
    /// Test HTTPRoute is allowed when kinds includes HTTPRoute
    fn test_http_route_allowed() -> TestCase {
        TestCase::new(
            "http_route_allowed",
            "Test HTTPRoute is allowed when specified in kinds",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = "http://127.0.0.1:31213/health".to_string();

                    let response = client.get(&url).header("Host", "kinds-http.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "✓ HTTPRoute allowed when specified in kinds (status: {})",
                                        resp.status()
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 200 OK for HTTPRoute with kinds restriction, got {}",
                                        resp.status()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test GRPCRoute is denied when kinds doesn't include GRPCRoute
    fn test_grpc_route_denied() -> TestCase {
        TestCase::new(
            "grpc_route_denied",
            "Test GRPCRoute is denied when not in kinds (negative test)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Use gRPC client to test
                    // Since we don't have a dedicated gRPC client here, we'll use HTTP/2
                    // In a real scenario, this would fail at routing level
                    let client = reqwest::Client::builder().http2_prior_knowledge().build().unwrap();

                    let url = "http://127.0.0.1:31213/test.TestService/SayHello".to_string();

                    let response = client
                        .post(&url)
                        .header("Host", "kinds-grpc.example.com")
                        .header("content-type", "application/grpc")
                        .body(vec![0u8; 5]) // Minimal gRPC frame
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            // Should get 404 or connection error because route is not allowed
                            if resp.status() == 404 || !resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "✓ GRPCRoute correctly denied when not in kinds (status: {})",
                                        resp.status()
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 404 or error for GRPCRoute not in kinds, got {}",
                                        resp.status()
                                    ),
                                )
                            }
                        }
                        Err(_) => {
                            // Connection error is also acceptable (route rejected)
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "✓ GRPCRoute correctly denied (connection error)".to_string(),
                            )
                        }
                    }
                })
            },
        )
    }
}

impl TestSuite for AllowedRoutesKindsTestSuite {
    fn name(&self) -> &str {
        "Gateway AllowedRoutes Kinds Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_http_route_allowed(), Self::test_grpc_route_denied()]
    }
}
