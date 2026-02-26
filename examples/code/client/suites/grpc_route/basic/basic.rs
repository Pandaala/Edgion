// gRPC Test suite
//
// Required config files (in examples/conf/):
// - EndpointSlice_edge_test-grpc.yaml         # gRPC backend service discovery
// - Service_edge_test-grpc.yaml               # gRPC service definition
// - GRPCRoute_edge_test-grpc.yaml             # gRPC routing rules（Host: grpc.test.example.com）
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

// Use proto module from grpc_route
use super::super::test::test_service_client::TestServiceClient;
use super::super::test::HelloRequest;

pub struct GrpcTestSuite;

impl GrpcTestSuite {
    /// Test gRPC SayHello RPC
    fn test_grpc_say_hello() -> TestCase {
        TestCase::new("grpc_say_hello", "gRPC SayHello test", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                // Build connection URL
                let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);

                // Create gRPC client，Passed origin 设置 :authority
                let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                    Ok(mut ep) => {
                        // Gateway Mode: set origin to control :authority pseudo-header
                        if let Some(ref host) = ctx.grpc_host {
                            let origin_uri = match format!("http://{}:{}", host, ctx.grpc_port).parse() {
                                Ok(uri) => uri,
                                Err(e) => {
                                    return TestResult::failed(start.elapsed(), format!("Invalid origin URI: {}", e));
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

                // Create request
                let request = tonic::Request::new(HelloRequest {
                    name: "Edgion".to_string(),
                });

                match client.say_hello(request).await {
                    Ok(response) => {
                        let reply = response.into_inner();
                        if reply.message.contains("Hello, Edgion!") {
                            let msg = if ctx.grpc_host.is_some() {
                                format!("Response: {}", reply.message)
                            } else {
                                format!("Response: {}, Server: {}", reply.message, reply.server_addr)
                            };
                            TestResult::passed_with_message(start.elapsed(), msg)
                        } else {
                            TestResult::failed(start.elapsed(), format!("Unexpected response: {}", reply.message))
                        }
                    }
                    Err(e) => TestResult::failed(
                        start.elapsed(),
                        format!("RPC failed: {} (status: {:?})", e.message(), e.code()),
                    ),
                }
            })
        })
    }
}

#[async_trait]
impl TestSuite for GrpcTestSuite {
    fn name(&self) -> &str {
        "gRPC"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_grpc_say_hello()]
    }
}
