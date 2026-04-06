// AllowedRoutes Kinds test suite
//
// Required config files:
// - Gateway/AllowedRoutes/Kinds/Gateway.yaml         # Gateway with allowedRoutes.kinds: [HTTPRoute]
// - Gateway/AllowedRoutes/Kinds/HTTPRoute.yaml       # HTTPRoute (allowed)
// - Gateway/AllowedRoutes/Kinds/GRPCRoute.yaml       # GRPCRoute (denied)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::grpc_route::test::test_service_client::TestServiceClient;
use crate::suites::grpc_route::test::HelloRequest;
use std::time::Instant;
use tonic::Code;

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

                    // Use tonic gRPC client so that grpc-status from HTTP/2 trailers
                    // is properly read. reqwest cannot access trailers.
                    let grpc_url = format!("http://{}:31213", ctx.target_host);
                    let origin_uri: http::Uri = "http://kinds-grpc.example.com:31213".parse().unwrap();

                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(ep) => ep.origin(origin_uri),
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            // Connection error is acceptable (route rejected at connection level)
                            return TestResult::passed_with_message(
                                start.elapsed(),
                                format!("✓ GRPCRoute correctly denied (connection error: {})", e),
                            );
                        }
                    };

                    let request = tonic::Request::new(HelloRequest {
                        name: "KindsTest".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(_) => TestResult::failed(
                            start.elapsed(),
                            "Expected gRPC error for GRPCRoute not in kinds, but RPC succeeded".to_string(),
                        ),
                        Err(status) => {
                            let code = status.code();
                            if code == Code::Unimplemented || code == Code::NotFound || code == Code::Unavailable {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ GRPCRoute correctly denied when not in kinds (gRPC {:?})", code),
                                )
                            } else {
                                // Any non-OK gRPC status means the route was not served
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "✓ GRPCRoute denied (unexpected code {:?}: {}, but route was not served)",
                                        code,
                                        status.message()
                                    ),
                                )
                            }
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
