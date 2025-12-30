// gRPC 匹配规则测试套件
//
// 依赖的配置文件（位于 examples/conf/）：
// - GRPCRoute_edge_match-test.yaml         # gRPC 匹配规则测试路由
// - EndpointSlice_edge_test-grpc.yaml      # gRPC 后端服务发现
// - Service_edge_test-grpc.yaml            # gRPC 服务定义
// - Gateway_edge_example-gateway.yaml      # Gateway 配置
// - GatewayClass__public-gateway.yaml      # GatewayClass 配置

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

// 引入 proto 生成的代码
pub mod test {
    tonic::include_proto!("test");
}

use test::test_service_client::TestServiceClient;
use test::HelloRequest;

pub struct GrpcMatchTestSuite;

impl GrpcMatchTestSuite {
    /// 测试 hostname 正面匹配
    fn test_hostname_match_positive() -> TestCase {
        TestCase::new(
            "grpc_hostname_match_positive",
            "测试 gRPC hostname 正面匹配",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                let grpc_url = format!("http://127.0.0.1:{}", ctx.grpc_port);
                let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                    Ok(mut ep) => {
                        // 设置正确的 hostname
                        let origin_uri = match format!("http://grpc-match.example.com:{}", ctx.grpc_port).parse() {
                            Ok(uri) => uri,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Invalid origin URI: {}", e)
                                );
                            }
                        };
                        ep = ep.origin(origin_uri);
                        ep
                    },
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Invalid endpoint: {}", e)
                        );
                    }
                };
                
                let mut client = match TestServiceClient::connect(endpoint).await {
                    Ok(c) => c,
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to connect: {}", e)
                        );
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
                                format!("Hostname match successful: {}", reply.message)
                            )
                        } else {
                            TestResult::failed(
                                start.elapsed(),
                                format!("Unexpected response: {}", reply.message)
                            )
                        }
                    },
                    Err(e) => {
                        TestResult::failed(
                            start.elapsed(),
                            format!("RPC failed: {}", e)
                        )
                    }
                }
            })
        )
    }

    /// 测试 hostname 负面匹配（错误的 hostname 应该失败）
    fn test_hostname_match_negative() -> TestCase {
        TestCase::new(
            "grpc_hostname_match_negative",
            "测试 gRPC hostname 负面匹配",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                let grpc_url = format!("http://127.0.0.1:{}", ctx.grpc_port);
                let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                    Ok(mut ep) => {
                        // 设置错误的 hostname
                        let origin_uri = match format!("http://wrong-hostname.example.com:{}", ctx.grpc_port).parse() {
                            Ok(uri) => uri,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Invalid origin URI: {}", e)
                                );
                            }
                        };
                        ep = ep.origin(origin_uri);
                        ep
                    },
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Invalid endpoint: {}", e)
                        );
                    }
                };
                
                let mut client = match TestServiceClient::connect(endpoint).await {
                    Ok(c) => c,
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to connect: {}", e)
                        );
                    }
                };
                
                let request = tonic::Request::new(HelloRequest {
                    name: "ShouldFail".to_string(),
                });
                
                match client.say_hello(request).await {
                    Ok(_) => {
                        TestResult::failed(
                            start.elapsed(),
                            "Expected failure but got success (hostname should not match)".to_string()
                        )
                    },
                    Err(e) => {
                        // 应该失败（404 或其他错误）
                        // HTTP 404 may be mapped to Internal by tonic client
                        if e.code() == tonic::Code::NotFound || 
                           e.code() == tonic::Code::Unavailable ||
                           e.code() == tonic::Code::Internal {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Correctly rejected wrong hostname: {:?}", e.code())
                            )
                        } else {
                            TestResult::failed(
                                start.elapsed(),
                                format!("Unexpected error code: {:?}", e.code())
                            )
                        }
                    }
                }
            })
        )
    }

    /// 测试精确匹配 service + method
    fn test_exact_service_method_match() -> TestCase {
        TestCase::new(
            "grpc_exact_service_method_match",
            "测试 gRPC 精确 service+method 匹配",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                let grpc_url = format!("http://127.0.0.1:{}", ctx.grpc_port);
                let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                    Ok(mut ep) => {
                        let origin_uri = match format!("http://grpc-match.example.com:{}", ctx.grpc_port).parse() {
                            Ok(uri) => uri,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Invalid origin URI: {}", e)
                                );
                            }
                        };
                        ep = ep.origin(origin_uri);
                        ep
                    },
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Invalid endpoint: {}", e)
                        );
                    }
                };
                
                let mut client = match TestServiceClient::connect(endpoint).await {
                    Ok(c) => c,
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to connect: {}", e)
                        );
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
                            format!("Exact service+method match successful: {}", reply.message)
                        )
                    },
                    Err(e) => {
                        TestResult::failed(
                            start.elapsed(),
                            format!("RPC failed: {}", e)
                        )
                    }
                }
            })
        )
    }

    /// 测试 sectionName 不匹配（路由配置 sectionName: https，但通过 HTTP listener 访问）
    fn test_section_name_mismatch() -> TestCase {
        TestCase::new(
            "grpc_section_name_mismatch",
            "测试 gRPC sectionName 不匹配",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                // 使用 HTTP listener (10080)
                // 但是 GRPCRoute 配置的是 sectionName: https
                // 所以应该不匹配，返回 404
                let grpc_url = format!("http://127.0.0.1:{}", ctx.grpc_port);
                let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                    Ok(mut ep) => {
                        // 设置 hostname 为 grpc-section-wrong.example.com
                        let origin_uri = match format!("http://grpc-section-wrong.example.com:{}", ctx.grpc_port).parse() {
                            Ok(uri) => uri,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Invalid origin URI: {}", e)
                                );
                            }
                        };
                        ep = ep.origin(origin_uri);
                        ep
                    },
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Invalid endpoint: {}", e)
                        );
                    }
                };
                
                let mut client = match TestServiceClient::connect(endpoint).await {
                    Ok(c) => c,
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to connect: {}", e)
                        );
                    }
                };
                
                let request = tonic::Request::new(HelloRequest {
                    name: "SectionTest".to_string(),
                });
                
                match client.say_hello(request).await {
                    Ok(_) => {
                        TestResult::failed(
                            start.elapsed(),
                            "Expected failure for wrong sectionName but got success".to_string()
                        )
                    },
                    Err(e) => {
                        // 应该失败（404 或其他错误）
                        // HTTP 404 may be mapped to Internal by tonic client
                        if e.code() == tonic::Code::NotFound || 
                           e.code() == tonic::Code::Unavailable ||
                           e.code() == tonic::Code::Internal {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Correctly rejected wrong sectionName: {:?}", e.code())
                            )
                        } else {
                            TestResult::failed(
                                start.elapsed(),
                                format!("Unexpected error code: {:?}, message: {}", e.code(), e.message())
                            )
                        }
                    }
                }
            })
        )
    }

    /// 测试带 header 的精确匹配
    fn test_header_match() -> TestCase {
        TestCase::new(
            "grpc_header_match",
            "测试 gRPC header 匹配",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                let grpc_url = format!("http://127.0.0.1:{}", ctx.grpc_port);
                let endpoint = match tonic::transport::Endpoint::from_shared(grpc_url.clone()) {
                    Ok(mut ep) => {
                        let origin_uri = match format!("http://grpc-match.example.com:{}", ctx.grpc_port).parse() {
                            Ok(uri) => uri,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Invalid origin URI: {}", e)
                                );
                            }
                        };
                        ep = ep.origin(origin_uri);
                        ep
                    },
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Invalid endpoint: {}", e)
                        );
                    }
                };
                
                let mut client = match TestServiceClient::connect(endpoint).await {
                    Ok(c) => c,
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to connect: {}", e)
                        );
                    }
                };
                
                // 添加 header
                let mut request = tonic::Request::new(HelloRequest {
                    name: "HeaderTest".to_string(),
                });
                request.metadata_mut().insert("x-test-header", "test-value".parse().unwrap());
                
                match client.say_hello(request).await {
                    Ok(response) => {
                        let reply = response.into_inner();
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Header match successful: {}", reply.message)
                        )
                    },
                    Err(e) => {
                        TestResult::failed(
                            start.elapsed(),
                            format!("RPC with header failed: {}", e)
                        )
                    }
                }
            })
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
            Self::test_section_name_mismatch(),
        ]
    }
}

