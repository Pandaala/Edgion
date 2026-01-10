// WebSocket Test suite
//
// Required config files (in examples/conf/):
// - EndpointSlice_edge_test-websocket.yaml    # WebSocket backend service discovery
// - Service_edge_test-websocket.yaml          # WebSocket service definition
// - httproute_default_example-route.yaml      # WebSocket routing rules（Host: test.example.com）
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use std::time::Instant;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

pub struct WebSocketTestSuite;

impl WebSocketTestSuite {
    fn test_websocket_echo() -> TestCase {
        TestCase::new("websocket_echo", "Test WebSocket echo", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let test_message = "Hello WebSocket";

                // Build WebSocket connection request
                let ws_url = ctx.websocket_url();
                let mut request = ws_url.into_client_request().unwrap();

                // Gateway Mode：设置 Host header
                if let Some(ref host) = ctx.http_host {
                    request.headers_mut().insert("Host", host.parse().unwrap());
                }

                match connect_async(request).await {
                    Ok((mut ws_stream, _)) => {
                        // Send message
                        if let Err(e) = ws_stream.send(Message::Text(test_message.to_string())).await {
                            return TestResult::failed(start.elapsed(), format!("Send error: {}", e));
                        }

                        // Receive response
                        match tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next()).await {
                            Ok(Some(Ok(Message::Text(response)))) => {
                                let expected = format!("Echo: {}", test_message);
                                if response == expected {
                                    TestResult::passed(start.elapsed())
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!("Echo mismatch. Expected: {}, Got: {}", expected, response),
                                    )
                                }
                            }
                            Ok(Some(Ok(_))) => {
                                TestResult::failed(start.elapsed(), "Unexpected message type".to_string())
                            }
                            Ok(Some(Err(e))) => TestResult::failed(start.elapsed(), format!("Receive error: {}", e)),
                            Ok(None) => TestResult::failed(start.elapsed(), "Connection closed".to_string()),
                            Err(_) => TestResult::failed(start.elapsed(), "Timeout waiting for response".to_string()),
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Connection error: {}", e)),
                }
            })
        })
    }
}

#[async_trait]
impl TestSuite for WebSocketTestSuite {
    fn name(&self) -> &str {
        "WebSocket"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_websocket_echo()]
    }
}
