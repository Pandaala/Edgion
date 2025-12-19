// gRPC-HTTPS 测试套件
// 只在 Gateway 模式下测试

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

// 引入 proto 生成的代码
pub mod test {
    tonic::include_proto!("test");
}

use test::{test_service_client::TestServiceClient, HelloRequest};

pub struct GrpcHttpsTestSuite;

impl GrpcHttpsTestSuite {
    fn test_grpc_https_say_hello() -> TestCase {
        TestCase::new(
            "grpc_https_say_hello",
            "gRPC-HTTPS SayHello 测试",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                // Note: gRPC-HTTPS requires proper TLS configuration with tonic[tls-roots] feature
                // For now, we skip this test as it requires additional setup
                return TestResult::failed(
                    start.elapsed(),
                    format!("gRPC-HTTPS test requires tonic TLS feature configuration. Port: {}", ctx.grpc_https_port)
                );
            })
        )
    }
}

#[async_trait]
impl TestSuite for GrpcHttpsTestSuite {
    fn name(&self) -> &str {
        "gRPC-HTTPS"
    }
    
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_grpc_https_say_hello(),
        ]
    }
}

