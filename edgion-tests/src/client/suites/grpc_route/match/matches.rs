// gRPC match rulesTest suite
//
// Required config files (in examples/conf/):
// - GRPCRoute_edge_match-test.yaml         # gRPC match rules test route
// - EndpointSlice_edge_test-grpc.yaml      # gRPC backend service discovery
// - Service_edge_test-grpc.yaml            # gRPC service definition
// - Gateway_edge_example-gateway.yaml      # Gateway config
// - GatewayClass__public-gateway.yaml      # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

// Use proto module from parent
// Use proto module from grpc_route
use super::super::test::test_service_client::TestServiceClient;
use super::super::test::HelloRequest;

pub struct GrpcMatchTestSuite;

impl GrpcMatchTestSuite {
    /// Helper to create a tonic endpoint with a specific hostname for :authority header
    fn make_endpoint(ctx: &TestContext, hostname: &str) -> Result<tonic::transport::Endpoint, String> {
        let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
        let endpoint =
            tonic::transport::Endpoint::from_shared(grpc_url).map_err(|e| format!("Invalid endpoint: {}", e))?;
        let origin_uri: http::Uri = format!("http://{}:{}", hostname, ctx.grpc_port)
            .parse()
            .map_err(|e| format!("Invalid origin URI: {}", e))?;
        Ok(endpoint.origin(origin_uri))
    }

    /// Test hostname positive match
    fn test_hostname_match_positive() -> TestCase {
        TestCase::new(
            "grpc_hostname_match_positive",
            "Test gRPC hostname positive match",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(mut ep) => {
                            // Set correct hostname
                            let origin_uri = match format!("http://grpc-match.example.com:{}", ctx.grpc_port).parse() {
                                Ok(uri) => uri,
                                Err(e) => {
                                    return TestResult::failed(start.elapsed(), format!("Invalid origin URI: {}", e));
                                }
                            };
                            ep = ep.origin(origin_uri);
                            ep
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    let request = tonic::Request::new(HelloRequest {
                        name: "HostnameTest".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            if reply.message.contains("Hello, HostnameTest!") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Hostname match successful: {}", reply.message),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected response: {}", reply.message))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("RPC failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test hostname negative match（wrong hostname should fail）
    fn test_hostname_match_negative() -> TestCase {
        TestCase::new(
            "grpc_hostname_match_negative",
            "Test gRPC hostname negative match",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(mut ep) => {
                            // Set wrong hostname
                            let origin_uri = match format!("http://wrong-hostname.example.com:{}", ctx.grpc_port)
                                .parse()
                            {
                                Ok(uri) => uri,
                                Err(e) => {
                                    return TestResult::failed(start.elapsed(), format!("Invalid origin URI: {}", e));
                                }
                            };
                            ep = ep.origin(origin_uri);
                            ep
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    let request = tonic::Request::new(HelloRequest {
                        name: "ShouldFail".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(_) => TestResult::failed(
                            start.elapsed(),
                            "Expected failure but got success (hostname should not match)".to_string(),
                        ),
                        Err(e) => {
                            // should fail with Unimplemented (grpc-status 12) when GRPCRoutes exist but none match
                            if e.code() == tonic::Code::Unimplemented
                                || e.code() == tonic::Code::NotFound
                                || e.code() == tonic::Code::Unavailable
                            {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Correctly rejected wrong hostname: {:?}", e.code()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Unexpected error code: {:?} (expected NotFound/Unavailable, got RST_STREAM?)",
                                        e.code()
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    /// Test exact match service + method
    fn test_exact_service_method_match() -> TestCase {
        TestCase::new(
            "grpc_exact_service_method_match",
            "Test gRPC exact service+method match",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(mut ep) => {
                            let origin_uri = match format!("http://grpc-match.example.com:{}", ctx.grpc_port).parse() {
                                Ok(uri) => uri,
                                Err(e) => {
                                    return TestResult::failed(start.elapsed(), format!("Invalid origin URI: {}", e));
                                }
                            };
                            ep = ep.origin(origin_uri);
                            ep
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    let request = tonic::Request::new(HelloRequest {
                        name: "ExactMatch".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Exact service+method match successful: {}", reply.message),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("RPC failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test sectionName mismatchmatch（route config sectionName: https，but accessed via HTTP listener）
    fn test_section_name_mismatch() -> TestCase {
        TestCase::new(
            "grpc_section_name_mismatch",
            "Test gRPC sectionName mismatch",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    //  HTTP listener (10080)
                    //  GRPCRoute config sectionName: https
                    // so should not match，returns 404
                    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(mut ep) => {
                            //  hostname  grpc-section-wrong.example.com
                            let origin_uri = match format!("http://grpc-section-wrong.example.com:{}", ctx.grpc_port)
                                .parse()
                            {
                                Ok(uri) => uri,
                                Err(e) => {
                                    return TestResult::failed(start.elapsed(), format!("Invalid origin URI: {}", e));
                                }
                            };
                            ep = ep.origin(origin_uri);
                            ep
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    let request = tonic::Request::new(HelloRequest {
                        name: "SectionTest".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(_) => TestResult::failed(
                            start.elapsed(),
                            "Expected failure for wrong sectionName but got success".to_string(),
                        ),
                        Err(e) => {
                            // should fail with Unimplemented (grpc-status 12) when GRPCRoutes exist but none match
                            if e.code() == tonic::Code::Unimplemented
                                || e.code() == tonic::Code::NotFound
                                || e.code() == tonic::Code::Unavailable
                            {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Correctly rejected wrong sectionName: {:?}", e.code()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Unexpected error code: {:?}, message: {} (expected NotFound/Unavailable)",
                                        e.code(),
                                        e.message()
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    /// Test regex method match: hostname grpc-regex.example.com only has regex rules,
    /// so SayHello must be matched by the regex rule "Say.*" to succeed.
    fn test_regex_method_match() -> TestCase {
        TestCase::new(
            "grpc_regex_method_match",
            "Test gRPC RegularExpression method match (Say.*)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(mut ep) => {
                            let origin_uri = match format!("http://grpc-regex.example.com:{}", ctx.grpc_port).parse() {
                                Ok(uri) => uri,
                                Err(e) => {
                                    return TestResult::failed(start.elapsed(), format!("Invalid origin URI: {}", e));
                                }
                            };
                            ep = ep.origin(origin_uri);
                            ep
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    let request = tonic::Request::new(HelloRequest {
                        name: "RegexMethodTest".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            if reply.message.contains("Hello, RegexMethodTest!") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Regex method match (Say.*) successful: {}", reply.message),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected response: {}", reply.message))
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Regex method match failed (code={:?}): {}. \
                                 This likely means RegularExpression match type is not implemented.",
                                e.code(),
                                e.message()
                            ),
                        ),
                    }
                })
            },
        )
    }

    /// Test regex service-only match: hostname grpc-regex-svc.example.com only has
    /// a regex service rule "test\\..*" (no method specified), so any method on
    /// test.TestService should match.
    fn test_regex_service_only_match() -> TestCase {
        TestCase::new(
            "grpc_regex_service_only_match",
            "Test gRPC RegularExpression service-only match (test\\..*)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(mut ep) => {
                            let origin_uri = match format!("http://grpc-regex-svc.example.com:{}", ctx.grpc_port)
                                .parse()
                            {
                                Ok(uri) => uri,
                                Err(e) => {
                                    return TestResult::failed(start.elapsed(), format!("Invalid origin URI: {}", e));
                                }
                            };
                            ep = ep.origin(origin_uri);
                            ep
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    let request = tonic::Request::new(HelloRequest {
                        name: "RegexServiceTest".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            if reply.message.contains("Hello, RegexServiceTest!") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Regex service-only match (test\\..*) successful: {}", reply.message),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected response: {}", reply.message))
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Regex service-only match failed (code={:?}): {}. \
                                 This likely means RegularExpression match type is not implemented.",
                                e.code(),
                                e.message()
                            ),
                        ),
                    }
                })
            },
        )
    }

    /// Test that regex method does NOT match a wrong hostname (negative case).
    /// Uses grpc-regex.example.com's rules but with a different hostname,
    /// so no route should match.
    fn test_regex_no_match_wrong_hostname() -> TestCase {
        TestCase::new(
            "grpc_regex_no_match_wrong_hostname",
            "Test gRPC regex route does not match wrong hostname",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
                    let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                        Ok(mut ep) => {
                            // Use a hostname that no GRPCRoute matches
                            let origin_uri =
                                match format!("http://grpc-regex-nonexistent.example.com:{}", ctx.grpc_port).parse() {
                                    Ok(uri) => uri,
                                    Err(e) => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Invalid origin URI: {}", e),
                                        );
                                    }
                                };
                            ep = ep.origin(origin_uri);
                            ep
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                        }
                    };

                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                        }
                    };

                    let request = tonic::Request::new(HelloRequest {
                        name: "ShouldNotMatch".to_string(),
                    });

                    match client.say_hello(request).await {
                        Ok(_) => TestResult::failed(
                            start.elapsed(),
                            "Expected failure but got success (wrong hostname should not match regex route)"
                                .to_string(),
                        ),
                        Err(e) => {
                            if e.code() == tonic::Code::Unimplemented
                                || e.code() == tonic::Code::NotFound
                                || e.code() == tonic::Code::Unavailable
                            {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Correctly rejected wrong hostname for regex route: {:?}", e.code()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Unexpected error code: {:?} (expected NotFound/Unavailable, got RST_STREAM?)",
                                        e.code()
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    /// Test exact with headermatch
    fn test_header_match() -> TestCase {
        TestCase::new("grpc_header_match", "Test gRPC header match", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
                let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                    Ok(mut ep) => {
                        let origin_uri = match format!("http://grpc-match.example.com:{}", ctx.grpc_port).parse() {
                            Ok(uri) => uri,
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Invalid origin URI: {}", e));
                            }
                        };
                        ep = ep.origin(origin_uri);
                        ep
                    }
                    Err(e) => {
                        return TestResult::failed(start.elapsed(), format!("Invalid endpoint: {}", e));
                    }
                };

                let mut client = match TestServiceClient::connect(endpoint).await {
                    Ok(c) => c,
                    Err(e) => {
                        return TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e));
                    }
                };

                // Add header
                let mut request = tonic::Request::new(HelloRequest {
                    name: "HeaderTest".to_string(),
                });
                request
                    .metadata_mut()
                    .insert("x-test-header", "test-value".parse().unwrap());

                match client.say_hello(request).await {
                    Ok(response) => {
                        let reply = response.into_inner();
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Header match successful: {}", reply.message),
                        )
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("RPC with header failed: {}", e)),
                }
            })
        })
    }

    /// Test header-only match (no method specified) — Version: one should match rule 0
    fn test_header_only_match_version_one() -> TestCase {
        TestCase::new(
            "grpc_header_only_match_version_one",
            "Test gRPC header-only match: Version=one matches rule 0",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match Self::make_endpoint(&ctx, "grpc-header-only.example.com") {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Connect failed: {}", e)),
                    };

                    let mut request = tonic::Request::new(HelloRequest {
                        name: "HeaderOnlyV1".to_string(),
                    });
                    request.metadata_mut().insert("version", "one".parse().unwrap());

                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Header-only Version=one matched: {}", reply.message),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("RPC failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test header-only match (no method specified) — Version: two should match rule 1
    fn test_header_only_match_version_two() -> TestCase {
        TestCase::new(
            "grpc_header_only_match_version_two",
            "Test gRPC header-only match: Version=two matches rule 1",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match Self::make_endpoint(&ctx, "grpc-header-only.example.com") {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Connect failed: {}", e)),
                    };

                    let mut request = tonic::Request::new(HelloRequest {
                        name: "HeaderOnlyV2".to_string(),
                    });
                    request.metadata_mut().insert("version", "two".parse().unwrap());

                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Header-only Version=two matched: {}", reply.message),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("RPC failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test header-only match with multiple headers (AND logic)
    fn test_header_only_multi_header_and() -> TestCase {
        TestCase::new(
            "grpc_header_only_multi_header_and",
            "Test gRPC header-only match: Color=blue AND Shape=circle matches rule 2",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match Self::make_endpoint(&ctx, "grpc-header-only.example.com") {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Connect failed: {}", e)),
                    };

                    let mut request = tonic::Request::new(HelloRequest {
                        name: "MultiHeader".to_string(),
                    });
                    request.metadata_mut().insert("color", "blue".parse().unwrap());
                    request.metadata_mut().insert("shape", "circle".parse().unwrap());

                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Multi-header AND match: {}", reply.message),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("RPC failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test that partial multi-header does NOT match (AND logic — all headers required)
    fn test_header_only_partial_header_no_match() -> TestCase {
        TestCase::new(
            "grpc_header_only_partial_header_no_match",
            "Test gRPC header-only: Color=blue alone (without Shape) should NOT match rule 2",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match Self::make_endpoint(&ctx, "grpc-header-only.example.com") {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Connect failed: {}", e)),
                    };

                    // Only set Color=blue without Shape=circle — should NOT match rule 2
                    // and no other rule matches Color=blue → UNIMPLEMENTED
                    let mut request = tonic::Request::new(HelloRequest {
                        name: "PartialHeader".to_string(),
                    });
                    request.metadata_mut().insert("color", "blue".parse().unwrap());

                    match client.say_hello(request).await {
                        Ok(_) => TestResult::failed(
                            start.elapsed(),
                            "Expected UNIMPLEMENTED but got success (partial header should not match AND rule)"
                                .to_string(),
                        ),
                        Err(e) => {
                            if e.code() == tonic::Code::Unimplemented {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Correctly returned UNIMPLEMENTED for partial header: {:?}", e.code()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected UNIMPLEMENTED, got {:?}: {}", e.code(), e.message()),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    /// Test that a non-matching header returns gRPC UNIMPLEMENTED
    fn test_header_only_no_match_unimplemented() -> TestCase {
        TestCase::new(
            "grpc_header_only_no_match_unimplemented",
            "Test gRPC header-only: non-matching header returns UNIMPLEMENTED",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match Self::make_endpoint(&ctx, "grpc-header-only.example.com") {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Connect failed: {}", e)),
                    };

                    // Send a header that matches NO rule
                    let mut request = tonic::Request::new(HelloRequest {
                        name: "NoMatch".to_string(),
                    });
                    request.metadata_mut().insert("version", "three".parse().unwrap());

                    match client.say_hello(request).await {
                        Ok(_) => {
                            TestResult::failed(start.elapsed(), "Expected UNIMPLEMENTED but got success".to_string())
                        }
                        Err(e) => {
                            if e.code() == tonic::Code::Unimplemented {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Correctly returned UNIMPLEMENTED: {:?}", e.code()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected UNIMPLEMENTED, got {:?}: {}", e.code(), e.message()),
                                )
                            }
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GrpcMatchTestSuite {
    fn name(&self) -> &str {
        "gRPC Match"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_hostname_match_positive(),
            Self::test_hostname_match_negative(),
            Self::test_exact_service_method_match(),
            Self::test_header_match(),
            Self::test_header_only_match_version_one(),
            Self::test_header_only_match_version_two(),
            Self::test_header_only_multi_header_and(),
            Self::test_header_only_partial_header_no_match(),
            Self::test_header_only_no_match_unimplemented(),
            Self::test_section_name_mismatch(),
            Self::test_regex_method_match(),
            Self::test_regex_service_only_match(),
            Self::test_regex_no_match_wrong_hostname(),
        ]
    }
}
