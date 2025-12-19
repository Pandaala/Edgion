// gRPC-HTTPS 测试套件
// 只在 Gateway 模式下测试

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct GrpcHttpsTestSuite;

impl GrpcHttpsTestSuite {
    fn test_grpc_https_say_hello() -> TestCase {
        TestCase::new(
            "grpc_https_say_hello",
            "gRPC-HTTPS SayHello 测试",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                // Build gRPC-HTTPS URL
                let grpc_url = if let Some(ref host) = ctx.grpc_host {
                    format!("https://{}:{}", host, ctx.grpc_https_port)
                } else {
                    ctx.grpc_https_url()
                };
                
                // Note: For now, we skip TLS config as tonic's TLS support requires additional features
                // This test will fail until TLS is properly configured in the Gateway
                TestResult::failed(
                    start.elapsed(),
                    format!("gRPC-HTTPS test not yet implemented. TLS support requires tonic[tls] feature. URL: {}", grpc_url)
                )
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

