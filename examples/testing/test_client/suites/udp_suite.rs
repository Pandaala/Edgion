// UDP 测试套件
//
// 依赖的配置文件（位于 examples/conf/）：
// - EndpointSlice_edge_test-udp.yaml          # UDP 后端服务发现
// - Service_edge_test-udp.yaml                # UDP 服务定义
// - UDPRoute_edge_test-udp.yaml               # UDP 路由规则（监听 19002 端口）
// - Gateway_edge_example-gateway.yaml         # Gateway 配置
// - GatewayClass__public-gateway.yaml         # GatewayClass 配置

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;
use tokio::net::UdpSocket;

pub struct UdpTestSuite;

impl UdpTestSuite {
    fn test_udp_send_receive() -> TestCase {
        TestCase::new("udp_send_receive", "测试 UDP 发送和接收", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let test_data = b"Hello UDP";

                match UdpSocket::bind("0.0.0.0:0").await {
                    Ok(socket) => {
                        // 发送数据
                        if let Err(e) = socket.send_to(test_data, &ctx.udp_addr()).await {
                            return TestResult::failed(start.elapsed(), e.to_string());
                        }

                        // 接收响应
                        let mut buffer = vec![0u8; 1024];
                        match tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv_from(&mut buffer))
                            .await
                        {
                            Ok(Ok((n, _))) => {
                                if n > 0 && &buffer[..n] == test_data {
                                    TestResult::passed(start.elapsed())
                                } else {
                                    TestResult::failed(start.elapsed(), "Echo data mismatch".to_string())
                                }
                            }
                            Ok(Err(e)) => TestResult::failed(start.elapsed(), e.to_string()),
                            Err(_) => {
                                TestResult::failed(start.elapsed(), "Timeout waiting for UDP response".to_string())
                            }
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        })
    }
}

#[async_trait]
impl TestSuite for UdpTestSuite {
    fn name(&self) -> &str {
        "UDP"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_udp_send_receive()]
    }
}
