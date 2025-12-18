// WebSocket 测试套件

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures::{SinkExt, StreamExt};

pub struct WebSocketTestSuite;

impl WebSocketTestSuite {
    fn test_websocket_echo() -> TestCase {
        TestCase::new(
            "websocket_echo",
            "测试 WebSocket echo 功能",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                let ws_url = ctx.websocket_url();
                let test_message = "Hello WebSocket";
                
                match connect_async(&ws_url).await {
                    Ok((mut ws_stream, _)) => {
                        // 发送消息
                        if let Err(e) = ws_stream.send(Message::Text(test_message.to_string())).await {
                            return TestResult::failed(start.elapsed(), format!("Send error: {}", e));
                        }
                        
                        // 接收响应
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            ws_stream.next()
                        ).await {
                            Ok(Some(Ok(Message::Text(response)))) => {
                                let expected = format!("Echo: {}", test_message);
                                if response == expected {
                                    TestResult::passed(start.elapsed())
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!("Echo mismatch. Expected: {}, Got: {}", expected, response)
                                    )
                                }
                            }
                            Ok(Some(Ok(_))) => TestResult::failed(start.elapsed(), "Unexpected message type".to_string()),
                            Ok(Some(Err(e))) => TestResult::failed(start.elapsed(), format!("Receive error: {}", e)),
                            Ok(None) => TestResult::failed(start.elapsed(), "Connection closed".to_string()),
                            Err(_) => TestResult::failed(start.elapsed(), "Timeout waiting for response".to_string()),
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Connection error: {}", e)),
                }
            })
        )
    }
}

#[async_trait]
impl TestSuite for WebSocketTestSuite {
    fn name(&self) -> &str {
        "WebSocket"
    }
    
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_websocket_echo(),
        ]
    }
}

