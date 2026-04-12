// DSL grpc_error() integration tests
//
// Verifies that the DSL `return grpc_error(code, message)` syntax produces
// a proper gRPC error response (HTTP 200 + grpc-status / grpc-message trailers)
// through the full Gateway data path.
//
// Required config files:
// - EdgionPlugins/Dsl/23_EdgionPlugins_dsl-grpc-error.yaml
// - EdgionPlugins/Dsl/GRPCRoute_dsl-grpc-error.yaml
// - GRPCRoute/Basic/Gateway.yaml  (grpc-test-gateway, port 31050)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

// Proto types — re-exported from the grpc_route module
use crate::suites::grpc_route::test::test_service_client::TestServiceClient;
use crate::suites::grpc_route::test::HelloRequest;

pub struct DslGrpcErrorTestSuite;

impl DslGrpcErrorTestSuite {
    /// Helper: build a tonic Endpoint targeting the gRPC gateway with the
    /// correct `:authority` pseudo-header (`dsl-grpc-error.example.com`).
    fn build_endpoint(ctx: &TestContext) -> Result<tonic::transport::Endpoint, String> {
        let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
        let mut ep =
            tonic::transport::Endpoint::from_shared(grpc_url).map_err(|e| format!("Invalid endpoint: {}", e))?;

        // Override :authority to match the GRPCRoute hostname, not the default grpc_host.
        let authority_host = "dsl-grpc-error.example.com";
        let origin_uri = format!("http://{}:{}", authority_host, ctx.grpc_port)
            .parse()
            .map_err(|e| format!("Invalid origin URI: {}", e))?;
        ep = ep.origin(origin_uri);
        Ok(ep)
    }

    // =====================================================================
    // Test 1: no Authorization header → gRPC UNAUTHENTICATED (16)
    // =====================================================================
    fn test_grpc_error_unauthenticated() -> TestCase {
        TestCase::new(
            "grpc_error_unauthenticated",
            "DSL grpc_error: no auth header → status 16 UNAUTHENTICATED",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let endpoint = match Self::build_endpoint(&ctx) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    // Request WITHOUT authorization header
                    let request = tonic::Request::new(HelloRequest {
                        name: "grpc-error-test".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(_) => TestResult::failed(
                            start.elapsed(),
                            "Expected gRPC error 16 (UNAUTHENTICATED), but got Ok".to_string(),
                        ),
                        Err(status) => {
                            if status.code() == tonic::Code::Unauthenticated {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Correctly received UNAUTHENTICATED: code={:?}, message={}",
                                        status.code(),
                                        status.message()
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected status 16 (UNAUTHENTICATED), got {:?} ({})",
                                        status.code(),
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

    // =====================================================================
    // Test 2: Authorization: "invalid" → gRPC PERMISSION_DENIED (7)
    // =====================================================================
    fn test_grpc_error_permission_denied() -> TestCase {
        TestCase::new(
            "grpc_error_permission_denied",
            "DSL grpc_error: auth=invalid → status 7 PERMISSION_DENIED",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let endpoint = match Self::build_endpoint(&ctx) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    // Request WITH authorization = "invalid"
                    let mut request = tonic::Request::new(HelloRequest {
                        name: "grpc-error-test".to_string(),
                    });
                    request
                        .metadata_mut()
                        .insert("authorization", "invalid".parse().unwrap());

                    match client.say_hello(request).await {
                        Ok(_) => TestResult::failed(
                            start.elapsed(),
                            "Expected gRPC error 7 (PERMISSION_DENIED), but got Ok".to_string(),
                        ),
                        Err(status) => {
                            if status.code() == tonic::Code::PermissionDenied {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Correctly received PERMISSION_DENIED: code={:?}, message={}",
                                        status.code(),
                                        status.message()
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected status 7 (PERMISSION_DENIED), got {:?} ({})",
                                        status.code(),
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

    // =====================================================================
    // Test 3: Authorization: "valid" → request passes through to backend
    // =====================================================================
    fn test_grpc_error_pass_through() -> TestCase {
        TestCase::new(
            "grpc_error_pass_through",
            "DSL grpc_error: auth=valid → normal gRPC response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let endpoint = match Self::build_endpoint(&ctx) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    // Request WITH authorization = "valid" → should pass DSL and reach backend
                    let mut request = tonic::Request::new(HelloRequest {
                        name: "Edgion".to_string(),
                    });
                    request.metadata_mut().insert("authorization", "valid".parse().unwrap());

                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            if reply.message.contains("Hello, Edgion!") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Pass-through OK: {}", reply.message),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Unexpected response message: {}", reply.message),
                                )
                            }
                        }
                        Err(status) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected success, got gRPC error {:?}: {}",
                                status.code(),
                                status.message()
                            ),
                        ),
                    }
                })
            },
        )
    }
}

impl TestSuite for DslGrpcErrorTestSuite {
    fn name(&self) -> &str {
        "DSL gRPC Error"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_grpc_error_unauthenticated(),
            Self::test_grpc_error_permission_denied(),
            Self::test_grpc_error_pass_through(),
        ]
    }
}
