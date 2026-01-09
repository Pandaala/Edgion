// Stream Plugins 测试套件 - 测试 EdgionStreamPlugins 功能
//
// 依赖的配置文件（位于 examples/conf/）：
// - EndpointSlice_edge_test-tcp.yaml          # TCP 后端服务发现
// - Service_edge_test-tcp.yaml                # TCP 服务定义
// - TCPRoute_edge_test-tcp-with-plugins.yaml  # 带插件的 TCP 路由（监听 19010 端口）
// - EdgionStreamPlugins_edge_test-ip-filter.yaml  # IP 过滤 stream 插件配置
//   注：该插件配置了允许的 IP 地址列表（127.0.0.1, 192.168.0.0/16）
// - Gateway_edge_example-gateway.yaml         # Gateway 配置
// - GatewayClass__public-gateway.yaml         # GatewayClass 配置

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[allow(dead_code)]
pub struct StreamPluginsTestSuite;

#[allow(dead_code)]
impl StreamPluginsTestSuite {
    /// 测试允许的 IP 可以连接（本地测试）
    fn test_tcp_allowed_ip() -> TestCase {
        TestCase::new(
            "tcp_allowed_ip",
            "测试允许的 IP 可以建立 TCP 连接",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let tcp_filtered_addr = format!("{}:19010", ctx.target_host);

                    match TcpStream::connect(&tcp_filtered_addr).await {
                        Ok(_stream) => TestResult::passed_with_message(
                            start.elapsed(),
                            "TCP connection with stream plugin passed - IP allowed".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e)),
                    }
                })
            },
        )
    }

    /// 测试带插件的 TCP echo 功能
    fn test_tcp_echo_with_plugin() -> TestCase {
        TestCase::new(
            "tcp_echo_with_plugin",
            "测试带 IP 限制插件的 TCP echo 功能",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let tcp_filtered_addr = format!("{}:19010", ctx.target_host);
                    let test_data = b"Hello from stream plugin test!";

                    match TcpStream::connect(&tcp_filtered_addr).await {
                        Ok(mut stream) => {
                            // 发送测试数据
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), format!("Failed to write data: {}", e));
                            }

                            // 读取 echo 响应
                            let mut buffer = vec![0u8; 1024];
                            match stream.read(&mut buffer).await {
                                Ok(n) if n > 0 => {
                                    if &buffer[..n] == test_data {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Echo successful with plugin - {} bytes", n),
                                        )
                                    } else {
                                        TestResult::failed(start.elapsed(), "Echo data mismatch".to_string())
                                    }
                                }
                                Ok(_) => TestResult::failed(start.elapsed(), "No data received".to_string()),
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to read response: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Failed to connect: {}", e)),
                    }
                })
            },
        )
    }

    /// 测试插件已应用到路由
    fn test_plugin_applied() -> TestCase {
        TestCase::new(
            "plugin_applied",
            "验证 stream plugin 已应用到 TCPRoute",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let tcp_filtered_addr = format!("{}:19010", ctx.target_host);

                    // 尝试连接并发送数据，验证插件不会阻止正常流量
                    match TcpStream::connect(&tcp_filtered_addr).await {
                        Ok(mut stream) => {
                            let test_data = b"Plugin test";

                            // 写入数据
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }

                            // 读取响应以确认连接正常
                            let mut buffer = vec![0u8; 1024];
                            match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buffer))
                                .await
                            {
                                Ok(Ok(n)) if n > 0 => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Plugin applied and allows valid connections".to_string(),
                                ),
                                Ok(Ok(_)) => {
                                    TestResult::failed(start.elapsed(), "Connection closed unexpectedly".to_string())
                                }
                                Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("Read error: {}", e)),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timeout - plugin may be blocking".to_string(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Connection failed (plugin may be denying): {}", e),
                        ),
                    }
                })
            },
        )
    }

    /// 测试多次连接都能正常工作
    fn test_multiple_connections() -> TestCase {
        TestCase::new(
            "multiple_connections",
            "测试带插件的 TCP 支持多次连接",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let tcp_filtered_addr = format!("{}:19010", ctx.target_host);
                    let connection_count = 3;

                    for i in 0..connection_count {
                        match TcpStream::connect(&tcp_filtered_addr).await {
                            Ok(mut stream) => {
                                let test_data = format!("Connection #{}", i + 1).into_bytes();

                                // 发送数据
                                if let Err(e) = stream.write_all(&test_data).await {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Connection {} write failed: {}", i + 1, e),
                                    );
                                }

                                // 读取响应
                                let mut buffer = vec![0u8; 1024];
                                match stream.read(&mut buffer).await {
                                    Ok(n) if n > 0 && &buffer[..n] == test_data.as_slice() => {
                                        // 成功，继续下一个连接
                                    }
                                    Ok(_) => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Connection {} echo mismatch", i + 1),
                                        );
                                    }
                                    Err(e) => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Connection {} read failed: {}", i + 1, e),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Connection {} failed: {}", i + 1, e),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("All {} connections successful with plugin", connection_count),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for StreamPluginsTestSuite {
    fn name(&self) -> &str {
        "StreamPlugins"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_tcp_allowed_ip(),
            Self::test_tcp_echo_with_plugin(),
            Self::test_plugin_applied(),
            Self::test_multiple_connections(),
        ]
    }
}
