// gRPC Unmatched Route Test Suite
//
// Tests that gRPC requests to unmatched routes return the correct gRPC status codes.
// This reproduces the issue where requests return Internal(13) instead of Unimplemented(12).
//
// Required config files (in edgion-tests/integration/conf/GRPCRoute/UnmatchedRoute/):
// - GRPCRoute.yaml              # gRPC route with only SayHello method
// - Service_test-grpc.yaml      # gRPC service definition
// - EndpointSlice_test-grpc.yaml # gRPC backend service discovery

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;
use tonic::Code;

// Use proto module from parent grpc_route
use super::test::test_service_client::TestServiceClient;
use super::test::{HelloRequest, NumberRequest};

pub struct GrpcUnmatchedRouteTestSuite;

impl GrpcUnmatchedRouteTestSuite {
    /// Test that unmatched method returns gRPC UNIMPLEMENTED (status 12)
    ///
    /// Setup:
    /// - GRPCRoute configured to route only test.TestService.SayHello
    /// - Test tries to call test.TestService.StreamNumbers (not configured)
    ///
    /// Expected:
    /// - gRPC status code: 12 (Unimplemented)
    /// - Not Internal(13) or other error codes
    ///
    /// This reproduces the bug where unmatched gRPC methods return Internal(13)
    /// instead of the correct Unimplemented(12) status.
    fn test_unmatched_method() -> TestCase {
        TestCase::new(
            "grpc_unmatched_method",
            "Test gRPC unmatched method returns Unimplemented(12)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);

                    // Create gRPC client
                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(mut ep) => {
                            // Set correct hostname that matches GRPCRoute
                            if let Some(ref host) = ctx.grpc_host {
                                let origin_uri = match format!("http://{}:{}", host, ctx.grpc_port).parse() {
                                    Ok(uri) => uri,
                                    Err(e) => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Invalid origin URI: {}", e),
                                        );
                                    }
                                };
                                ep = ep.origin(origin_uri);
                            }
                            ep
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Failed to connect to {}: {}", grpc_url, e),
                            );
                        }
                    };

                    // Try to call StreamNumbers method, which is NOT configured in the GRPCRoute
                    // The GRPCRoute only allows test.TestService.SayHello
                    let request = tonic::Request::new(NumberRequest { count: 5 });

                    match client.stream_numbers(request).await {
                        Ok(_response) => {
                            // If we get here, it means the method was routed (unexpected in this test)
                            TestResult::failed(
                                start.elapsed(),
                                "StreamNumbers should NOT be routed (expected Unimplemented), but succeeded unexpectedly"
                                    .to_string(),
                            )
                        }
                        Err(e) => {
                            let status = tonic::Status::from_error(Box::new(e));
                            let code = status.code();

                            // We expect gRPC Unimplemented (12), not Internal (13)
                            if code == Code::Unimplemented {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Correctly returned Unimplemented(12): {}", status.message()),
                                )
                            } else if code == Code::Internal {
                                // This is the bug we're trying to reproduce
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "✗ BUG: Returned Internal(13) instead of Unimplemented(12). Message: {}",
                                        status.message()
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "✗ Unexpected gRPC status {:?}: {} (expected Unimplemented/12)",
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

    /// Test that matched method succeeds
    ///
    /// This verifies the baseline: when a method IS matched, it should return OK(0)
    fn test_matched_method() -> TestCase {
        TestCase::new(
            "grpc_matched_method",
            "Test gRPC matched method returns OK",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);

                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(mut ep) => {
                            if let Some(ref host) = ctx.grpc_host {
                                let origin_uri = match format!("http://{}:{}", host, ctx.grpc_port).parse() {
                                    Ok(uri) => uri,
                                    Err(e) => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Invalid origin URI: {}", e),
                                        );
                                    }
                                };
                                ep = ep.origin(origin_uri);
                            }
                            ep
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Failed to connect to {}: {}", grpc_url, e),
                            );
                        }
                    };

                    let request = tonic::Request::new(HelloRequest {
                        name: "MatchedTest".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            if reply.message.contains("Hello, MatchedTest!") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Matched method succeeded: {}", reply.message),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected response: {}", reply.message))
                            }
                        }
                        Err(e) => {
                            let status = tonic::Status::from_error(Box::new(e));
                            TestResult::failed(
                                start.elapsed(),
                                format!("RPC failed with status {:?}: {}", status.code(), status.message()),
                            )
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GrpcUnmatchedRouteTestSuite {
    fn name(&self) -> &str {
        "GRPCRoute/UnmatchedRoute"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_matched_method(), Self::test_unmatched_method()]
    }
}
