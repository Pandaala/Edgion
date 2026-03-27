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
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Use no_proxy to bypass system proxy for local testing
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let url = format!("http://{}:31213/health", ctx.target_host);

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
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Use HTTP/2 client with correct hostname in the URL authority
                    // so that :authority pseudo-header carries the right hostname.
                    let client = reqwest::Client::builder()
                        .http2_prior_knowledge()
                        .no_proxy()
                        .build()
                        .unwrap();

                    // Resolve the target host but set kinds-grpc.example.com as :authority
                    let url = format!("http://{}:31213/test.TestService/SayHello", ctx.target_host);

                    let response = client
                        .post(&url)
                        .header("Host", "kinds-grpc.example.com")
                        .header("content-type", "application/grpc")
                        .body(vec![0u8; 5]) // Minimal gRPC frame
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            // gRPC errors are returned as HTTP 200 with grpc-status in headers/trailers.
                            // Check grpc-status header for non-zero value (= gRPC error).
                            let grpc_status = resp
                                .headers()
                                .get("grpc-status")
                                .and_then(|v| v.to_str().ok())
                                .and_then(|s| s.parse::<i32>().ok());

                            if resp.status() == 404 || !resp.status().is_success() {
                                // HTTP-level rejection
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "✓ GRPCRoute correctly denied when not in kinds (HTTP status: {})",
                                        resp.status()
                                    ),
                                )
                            } else if let Some(code) = grpc_status {
                                if code != 0 {
                                    // gRPC-level rejection (HTTP 200 + grpc-status error)
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!(
                                            "✓ GRPCRoute correctly denied when not in kinds (grpc-status: {})",
                                            code
                                        ),
                                    )
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        "Expected gRPC error for GRPCRoute not in kinds, got grpc-status: 0 (OK)".to_string(),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 404 or gRPC error for GRPCRoute not in kinds, got {} with no grpc-status",
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
