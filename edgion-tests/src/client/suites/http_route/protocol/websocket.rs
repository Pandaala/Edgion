// WebSocket Test suite
//
// Required config files (in edgion-tests/integration/conf/):
// - EndpointSlice_edge_test-websocket.yaml    # WebSocket backend service discovery
// - Service_edge_test-websocket.yaml          # WebSocket service definition
// - httproute_default_example-route.yaml      # WebSocket routing rules (Host: test.example.com)
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

pub struct WebSocketTestSuite {
    gateway: bool,
}

impl WebSocketTestSuite {
    pub fn new(gateway: bool) -> Self {
        Self { gateway }
    }

    fn test_websocket_echo() -> TestCase {
        TestCase::new("websocket_echo", "Test WebSocket echo", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let test_message = "Hello WebSocket";

                // Build WebSocket connection request
                let ws_url = ctx.websocket_url();
                let mut request = ws_url.into_client_request().unwrap();

                // Set Host header for gateway routing.
                if let Some(ref host) = ctx.http_host {
                    request.headers_mut().insert("Host", host.parse().unwrap());
                }

                match connect_async(request).await {
                    Ok((mut ws_stream, _)) => {
                        // Send message
                        if let Err(e) = ws_stream.send(Message::Text(test_message.to_string().into())).await {
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

    /// A response-stage plugin denial on a REAL protocol switch must stop the
    /// upgrade.
    ///
    /// This is the one denial shape that cannot be delivered by replacing the
    /// response: the upstream already answered `101`, so
    /// `apply_response_local_reply` refuses and the denial has to be enforced by
    /// aborting the exchange with the plugin's status. The gateway previously
    /// answered such a refusal by logging `deny response 403` and forwarding the
    /// 101 anyway — the handshake completed and the plugin's decision was lost,
    /// which is exactly the traffic a response-stage policy exists to stop.
    ///
    /// Asserting on "the upgrade did NOT complete" is the load-bearing part; the
    /// 403 in the failed handshake is the secondary confirmation that the
    /// plugin's own status reached the client.
    fn test_websocket_response_stage_deny_blocks_upgrade() -> TestCase {
        TestCase::new(
            "websocket_response_stage_deny_blocks_upgrade",
            "Response-stage deny on a real 101 aborts the exchange instead of completing the upgrade",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let ws_url = format!("ws://{}:{}/ws", ctx.target_host, ctx.websocket_port);
                    let mut request = ws_url.into_client_request().unwrap();
                    request
                        .headers_mut()
                        .insert("Host", "ws-resp-deny.example.com".parse().unwrap());

                    match connect_async(request).await {
                        Ok(_) => TestResult::failed(
                            start.elapsed(),
                            "Upgrade completed: the response-stage denial was dropped and the 101 forwarded"
                                .to_string(),
                        ),
                        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Upgrade refused with the plugin's own 403".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Upgrade refused, but with status {} instead of the plugin's 403",
                                        status
                                    ),
                                )
                            }
                        }
                        // The upgrade did not complete, which is the invariant
                        // under test, but the client never saw the plugin's
                        // status — report it rather than pass silently.
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Upgrade refused without an HTTP response carrying the denial: {}", e),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for WebSocketTestSuite {
    fn name(&self) -> &str {
        "WebSocket"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = vec![Self::test_websocket_echo()];
        if self.gateway {
            cases.push(Self::test_websocket_response_stage_deny_blocks_upgrade());
        }
        cases
    }
}
