// gRPC 测试套件

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

// 引入 proto 生成的代码
pub mod test {
    tonic::include_proto!("test");
}

use test::test_service_client::TestServiceClient;
use test::HelloRequest;

pub struct GrpcTestSuite;

impl GrpcTestSuite {
    /// 测试 gRPC SayHello RPC
    fn test_grpc_say_hello() -> TestCase {
        TestCase::new(
            "grpc_say_hello",
            "gRPC SayHello 测试",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                // 构建连接 URL
                let grpc_url = format!("http://127.0.0.1:{}", ctx.grpc_port);
                
                // 创建 gRPC 客户端
                let mut client = match TestServiceClient::connect(grpc_url.clone()).await {
                    Ok(c) => c,
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to connect to {}: {}", grpc_url, e)
                        );
                    }
                };
                
                // 创建请求
                let mut request = tonic::Request::new(HelloRequest {
                    name: "Edgion".to_string(),
                });
                
                // Gateway 模式：使用 metadata 设置 authority
                // 注意：这实际上不会设置 :authority pseudo-header，
                // 但可以设置普通的 authority header
                if let Some(ref host) = ctx.grpc_host {
                    use tonic::metadata::AsciiMetadataValue;
                    let authority_value = match AsciiMetadataValue::try_from(host.as_str()) {
                        Ok(v) => v,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Invalid authority value: {}", e)
                            );
                        }
                    };
                    request.metadata_mut().insert("host", authority_value);
                }
                
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
impl TestSuite for GrpcTestSuite {
    fn name(&self) -> &str {
        "gRPC"
    }
    
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_grpc_say_hello(),
        ]
    }
}
