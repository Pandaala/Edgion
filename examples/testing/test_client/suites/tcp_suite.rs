// TCP 测试套件

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TcpTestSuite;

impl TcpTestSuite {
    fn test_tcp_connection() -> TestCase {
        TestCase::new(
            "tcp_connection",
            "测试 TCP 连接建立",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                match TcpStream::connect(&ctx.tcp_addr()).await {
                    Ok(_stream) => {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "TCP connection established".to_string()
                        )
                    }
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        )
    }
    
    fn test_tcp_echo() -> TestCase {
        TestCase::new(
            "tcp_echo",
            "测试 TCP echo 功能",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                let test_data = b"Hello TCP";
                
                match TcpStream::connect(&ctx.tcp_addr()).await {
                    Ok(mut stream) => {
                        // 发送数据
                        if let Err(e) = stream.write_all(test_data).await {
                            return TestResult::failed(start.elapsed(), e.to_string());
                        }
                        
                        // 读取响应
                        let mut buffer = vec![0u8; 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                if n > 0 && &buffer[..n] == test_data {
                                    TestResult::passed(start.elapsed())
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        "Echo data mismatch".to_string()
                                    )
                                }
                            }
                            Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        )
    }
}

#[async_trait]
impl TestSuite for TcpTestSuite {
    fn name(&self) -> &str {
        "TCP"
    }
    
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_tcp_connection(),
            Self::test_tcp_echo(),
        ]
    }
}

