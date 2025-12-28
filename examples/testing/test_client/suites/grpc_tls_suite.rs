// gRPC TLS 测试套件
// 测试通过 HTTPS (TLS) 的 gRPC 连接
//
// 依赖的配置文件（位于 examples/conf/）：
// - EndpointSlice_edge_test-grpc.yaml         # gRPC 后端服务发现
// - Service_edge_test-grpc.yaml               # gRPC 服务定义
// - GRPCRoute_edge_test-grpc-https.yaml       # gRPC TLS 路由规则（Host: grpc-tls.test.example.com）
// - Gateway_edge_tls-terminate-gateway.yaml   # TLS 终止 Gateway 配置（监听 18443 端口）
// - EdgionTls_edge_edge-tls.yaml              # TLS 证书配置
// - Secret_edge_edge-tls.yaml                 # TLS 证书 Secret
// - GatewayClass__public-gateway.yaml         # GatewayClass 配置
// 
// 生成的证书文件：
// - examples/testing/certs/server.crt         # 服务端证书（由 generate_certs.sh 生成）
// - examples/testing/certs/server.key         # 服务端私钥
// - examples/testing/certs/ca.pem             # CA 证书

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

// 引入 proto 生成的代码
pub mod test {
    tonic::include_proto!("test");
}

use test::test_service_client::TestServiceClient;
use test::HelloRequest;
use tonic::transport::{Certificate, Channel, ClientTlsConfig};

pub struct GrpcTlsTestSuite;

impl GrpcTlsTestSuite {
    /// 测试 gRPC over TLS SayHello RPC
    fn test_grpc_tls_say_hello() -> TestCase {
        TestCase::new(
            "grpc_tls_say_hello",
            "gRPC TLS SayHello 测试",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                // 构建 HTTPS 连接 URL
                let grpc_url = format!("https://127.0.0.1:{}", ctx.grpc_https_port);
                
                // 读取 CA 证书
                let ca_pem = match std::fs::read_to_string("examples/testing/certs/ca.pem") {
                    Ok(pem) => pem,
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to read CA certificate: {}", e)
                        );
                    }
                };
                
                let ca = Certificate::from_pem(ca_pem);
                
                // 配置 TLS - 使用 CA 证书和 domain_name
                let domain_name = ctx.grpc_host.as_deref().unwrap_or("localhost");
                let tls = ClientTlsConfig::new()
                    .ca_certificate(ca)
                    .domain_name(domain_name);
                
                // 创建 Channel
                let channel = match Channel::from_shared(grpc_url.clone()) {
                    Ok(mut endpoint) => {
                        // Gateway 模式：设置 origin 来控制 :authority 伪头部
                        if let Some(ref host) = ctx.grpc_host {
                            let origin_uri = match format!("https://{}:{}", host, ctx.grpc_https_port).parse() {
                                Ok(uri) => uri,
                                Err(e) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Invalid origin URI: {}", e)
                                    );
                                }
                            };
                            endpoint = endpoint.origin(origin_uri);
                        }
                        
                        match endpoint.tls_config(tls) {
                            Ok(ep) => {
                                match ep.connect().await {
                                    Ok(ch) => ch,
                                    Err(e) => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Failed to connect to {}: {}", grpc_url, e)
                                        );
                                    }
                                }
                            },
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Failed to configure TLS: {}", e)
                                );
                            }
                        }
                    },
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Invalid endpoint: {}", e)
                        );
                    }
                };
                
                let mut client = TestServiceClient::new(channel);
                
                // 创建请求 - 与 grpc_suite 完全一致
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
                            TestResult::failed(
                                start.elapsed(),
                                format!("Unexpected response: {}", reply.message)
                            )
                        }
                    },
                    Err(e) => {
                        TestResult::failed(
                            start.elapsed(),
                            format!("RPC failed: {} (status: {:?})", e.message(), e.code())
                        )
                    }
                }
            })
        )
    }
}

#[async_trait]
impl TestSuite for GrpcTlsTestSuite {
    fn name(&self) -> &str {
        "gRPC-TLS"
    }
    
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_grpc_tls_say_hello(),
        ]
    }
}


