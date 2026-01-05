// gRPC 测试套件
//
// 依赖的配置文件（位于 examples/conf/）：
// - EndpointSlice_edge_test-grpc.yaml         # gRPC 后端服务发现
// - Service_edge_test-grpc.yaml               # gRPC 服务定义
// - GRPCRoute_edge_test-grpc.yaml             # gRPC 路由规则（Host: grpc.test.example.com）
// - Gateway_edge_example-gateway.yaml         # Gateway 配置
// - GatewayClass__public-gateway.yaml         # GatewayClass 配置

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

// Use pre-generated proto code
#[path = "../../proto_gen/test.rs"]
pub mod test;

use test::test_service_client::TestServiceClient;
use test::HelloRequest;

pub struct GrpcTestSuite;

impl GrpcTestSuite {
    /// 测试 gRPC SayHello RPC
    fn test_grpc_say_hello() -> TestCase {
        TestCase::new("grpc_say_hello", "gRPC SayHello 测试", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                // 构建连接 URL
                let grpc_url = format!("http://127.0.0.1:{}", ctx.grpc_port);

                // 创建 gRPC 客户端，通过 origin 设置 :authority
                let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                    Ok(mut ep) => {
                        // Gateway 模式：设置 origin 来控制 :authority 伪头部
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

                // 创建请求
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
