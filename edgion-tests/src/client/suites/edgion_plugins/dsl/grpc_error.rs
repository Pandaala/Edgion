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

    /// Endpoint for the response-stage denial route.
    fn build_resp_deny_endpoint(ctx: &TestContext) -> Result<tonic::transport::Endpoint, String> {
        let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
        let mut ep =
            tonic::transport::Endpoint::from_shared(grpc_url).map_err(|e| format!("Invalid endpoint: {}", e))?;
        let origin_uri = format!("http://dsl-grpc-resp-deny.example.com:{}", ctx.grpc_port)
            .parse()
            .map_err(|e| format!("Invalid origin URI: {}", e))?;
        ep = ep.origin(origin_uri);
        Ok(ep)
    }

    /// Drive the response-stage denial route with `x-deny-mode` and return the
    /// resulting gRPC status.
    async fn call_resp_deny(ctx: &TestContext, mode: &str) -> Result<Option<tonic::Status>, String> {
        let endpoint = Self::build_resp_deny_endpoint(ctx)?;
        let mut client = TestServiceClient::connect(endpoint)
            .await
            .map_err(|e| format!("Failed to connect: {}", e))?;

        let mut request = tonic::Request::new(HelloRequest {
            name: "grpc-resp-deny".to_string(),
        });
        request.metadata_mut().insert("x-deny-mode", mode.parse().unwrap());

        match client.say_hello(request).await {
            Ok(_) => Ok(None),
            Err(status) => Ok(Some(status)),
        }
    }

    // =====================================================================
    // Test 4: response-stage ErrResponse on a gRPC route
    //
    // The upstream answered normally; the response-stage plugin denies. A gRPC
    // route can never carry a local reply (`build_response_local_reply` refuses
    // every gRPC request), so this is delivered by the response-stage ABORT.
    // Before that abort existed, the denial was a log line and the upstream's
    // successful response was forwarded to the client unchanged.
    // =====================================================================
    fn test_resp_stage_deny_maps_to_grpc_status() -> TestCase {
        TestCase::new(
            "grpc_resp_stage_deny_delivers_status",
            "DSL response deny on a gRPC route -> abort delivers PERMISSION_DENIED, not the upstream reply",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match Self::call_resp_deny(&ctx, "http").await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(None) => TestResult::failed(
                            start.elapsed(),
                            "Response-stage denial was dropped: the upstream response reached the client".to_string(),
                        ),
                        Ok(Some(status)) => {
                            // fail_to_proxy maps the plugin's HTTP 403 through
                            // http_to_grpc_status -> PERMISSION_DENIED (7).
                            if status.code() == tonic::Code::PermissionDenied {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Denial delivered as {:?}", status.code()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected PERMISSION_DENIED, got {:?} ({})",
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
    // Test 5: response-stage ErrGrpcError keeps the plugin's exact code
    //
    // The response-stage runners have no gRPC writer of their own (the sync one
    // cannot await), so this denial is carried to `fail_to_proxy` verbatim.
    // RESOURCE_EXHAUSTED (8) is deliberately a code no HTTP status maps to: if
    // the abort ever routes it through `http_to_grpc_status`, it degrades to
    // INTERNAL (13) and this test catches it.
    // =====================================================================
    fn test_resp_stage_grpc_error_keeps_exact_code() -> TestCase {
        TestCase::new(
            "grpc_resp_stage_grpc_error_exact_code",
            "DSL response grpc_error on a gRPC route -> exact RESOURCE_EXHAUSTED survives the abort",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match Self::call_resp_deny(&ctx, "grpc").await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(None) => TestResult::failed(
                            start.elapsed(),
                            "Response-stage grpc_error was dropped: the upstream response reached the client"
                                .to_string(),
                        ),
                        Ok(Some(status)) => {
                            if status.code() == tonic::Code::ResourceExhausted {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Exact gRPC code preserved: {:?} ({})", status.code(), status.message()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected RESOURCE_EXHAUSTED (8), got {:?} ({}) — the code was re-derived \
                                         from an HTTP status instead of carried verbatim",
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
            Self::test_resp_stage_deny_maps_to_grpc_status(),
            Self::test_resp_stage_grpc_error_keeps_exact_code(),
        ]
    }
}
