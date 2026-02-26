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
                            // should fail（404 or other error）
                            // HTTP 404 may be mapped to Internal by tonic client
                            if e.code() == tonic::Code::NotFound
                                || e.code() == tonic::Code::Unavailable
                                || e.code() == tonic::Code::Internal
                            {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Correctly rejected wrong hostname: {:?}", e.code()),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected error code: {:?}", e.code()))
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
                            // should fail（404 or other error）
                            // HTTP 404 may be mapped to Internal by tonic client
                            if e.code() == tonic::Code::NotFound
                                || e.code() == tonic::Code::Unavailable
                                || e.code() == tonic::Code::Internal
                            {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Correctly rejected wrong sectionName: {:?}", e.code()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Unexpected error code: {:?}, message: {}", e.code(), e.message()),
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
            Self::test_section_name_mismatch(),
        ]
    }
}
